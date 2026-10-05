//! Windows 低层键盘钩子引擎（`WH_KEYBOARD_LL`）。
//!
//! 职责：把系统键盘事件翻译成 [`KeyEvent`]，回调处理函数决定 [`Action`]，
//! 注入由 [`simulate`] 完成。钩子在独立线程跑消息循环；处理函数直接跑在
//! 回调里，不做跨线程调度，保证顺序与低延迟。
//!
//! 机制要点：
//! - 修饰键状态用 `GetKeyState` 实时读（事件键本身的方向手动修正，避开
//!   队列滞后）；自动重复按「该键已按下且未抬起」判定（[`HELD_KEYS`]），
//!   不用时间窗——同键快速连打是正常输入，不能被当成重复丢掉。
//! - `Block`/`Replace` 会登记 [`SWALLOWED`]，后续 keyup 一并吞掉，防止
//!   幽灵按键；`Replace` 保持"按下-抬起"配对（按住原键 = 按住目标键）。
//!   **所有** keyup（含被吞掉的）都以观察者身份回调 handler，返回值被忽略——
//!   状态机（和弦按住集合 / tap-hold 短长按判定）依赖抬起事件，漏掉就会卡死。
//! - 注入事件带 `LLKHF_INJECTED`，一律放行，杜绝自我回环。
//! - `Replace` 注入走 `SendInput`，键码为虚拟键码（US 布局语义，差异见
//!   各键盘布局 OEM 键）；span nil。
//! - 自愈看门狗（[`watchdog_loop`]）：低层钩子会**静默失效**——回调超过
//!   `LowLevelHooksTimeout`（默认 300ms）被系统摘除、休眠唤醒后、会话解锁后
//!   都可能再也收不到事件，而托盘看着还活着（用户看到的是「快捷键全不响应」）。
//!   独立心跳线程巡检，命中即让钩子线程**重建两个钩子并复位运行时状态**。
//!
//! 已知天花板（升级路径）：
//! - 低层钩子拦不住 UAC 提权进程 / 部分游戏 → 驱动级拦截（Interception）。
//! - 文本注入默认走剪贴板粘贴（中文最稳），会短暂占用剪贴板；目标程序吞粘贴时
//!   用 `type_text_unicode` 逐字符直发兜底（规划 7.3-㉒）。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::RemoteDesktop::{
    WTSFreeMemory, WTSQuerySessionInformationW, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION,
    WTSSessionInfoEx, WTSINFOEXW,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use kada_core::{FrontmostContext, Key, Modifier, Shortcut};

pub mod simulate;

/// 一次键盘事件。
#[derive(Clone, Debug)]
pub enum KeyEvent {
    Down { key: Key, mods: BTreeSet<Modifier>, repeat: bool },
    Up { key: Key, mods: BTreeSet<Modifier> },
}

/// 处理函数对事件的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 放行。
    Allow,
    /// 吞掉本事件（连同后续 keyup）。
    Block,
    /// 吞掉原键、注入目标键。
    Replace(Key),
}

type Handler = Box<dyn FnMut(KeyEvent) -> Action + Send>;

static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);
static HOOK: AtomicIsize = AtomicIsize::new(0);
/// 鼠标低层钩子句柄（与键盘钩子同一线程、同一消息循环）。
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
/// 是否需要挂鼠标钩子：只有配置里引用了鼠标键（中键 / 侧键作触发键或改键来源）时才挂，
/// 否则少一个全局钩子 = 更小的拦截面。由壳层按配置设置（见 [`set_mouse_enabled`]）。
/// 默认 `true` 保险：没人显式关掉时维持旧行为（鼠标键照常可用）。
static MOUSE_ENABLED: AtomicBool = AtomicBool::new(true);
/// 已决定吞掉的键：后续 keyup 也要吞。
static SWALLOWED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Replace 注入后仍按下的宿主原键 → 目标键。
static REPLACED_DOWN: LazyLock<Mutex<HashMap<Key, Key>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// 当前物理按下的键（down 且未 up），用于识别自动重复。
static HELD_KEYS: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// 看门狗心跳（键盘 / 鼠标各一路）：钩子回调每次被调用都刷新对应那一路（`GetTickCount`
/// 毫秒）。钩子被系统摘除后就再也不会被调用、对应时间戳随之停摆——看门狗拿它和系统
/// `GetLastInputInfo` 对照，并靠「另一半是否看得到这次输入」把两路分开，从而发现
/// 「键盘钩子单独被摘、鼠标钩子还活着」这类半边失效（判据见 [`dead_hook`]）。
static LAST_KEY_TICK: AtomicU32 = AtomicU32::new(0);
static LAST_MOUSE_TICK: AtomicU32 = AtomicU32::new(0);
/// 钩子线程 id。看门狗靠它把重装请求投递到拥有消息循环的钩子线程：`SetWindowsHookEx`
/// 必须在那个线程上调用（钩子回调也在那儿跑）。0 = 钩子线程已退出。
static HOOK_TID: AtomicU32 = AtomicU32::new(0);
/// 自愈重装累计次数（诊断用）。
static REINSTALLS: AtomicU64 = AtomicU64::new(0);
/// 钩子线程的自定义消息：请求重建两个钩子。
const WM_APP_REINSTALL: u32 = WM_APP + 1;
/// 看门狗巡检间隔。
const WATCHDOG_INTERVAL_MS: u64 = 1_000;
/// 判定「钩子已失效」的宽限：系统记下的最后输入比钩子最后事件新这么多毫秒，就说明
/// 钩子漏掉了事件。**必须大于巡检间隔**，否则两轮之间的输入可能整段落空、永远不被发现。
const HEARTBEAT_GRACE_MS: i32 = 1_500;
/// 「归因」宽限：系统最后一次输入比某一路钩子心跳新出这么多毫秒，就认定那次输入不是
/// 该路带来的（该路钩子若活着必然看得到它），好让另一半去解释它。取值要小——它只做
/// 「排除」、不是失效判据；太大则「键盘紧贴着鼠标刚动完就按」这类紧邻输入会被漏判
/// （且系统记下的最后输入会钉在那一刻、不再增长）。回调解调度的抖动用它兜住。
const ATTRIB_GRACE_MS: i32 = 100;
/// 相邻两次巡检的间隔超过这个值 → 机器睡过（`GetTickCount` 把睡眠时间也算进去）。
/// 取 10s 是为了让一次调度延迟不至于被误判成唤醒——误判的代价只是一次无害的重装。
const SUSPEND_GAP_MS: i32 = 10_000;

/// 钩子句柄。Drop 时停掉看门狗、给钩子线程发 WM_QUIT 并回收。
pub struct HookHandle {
    tid: u32,
    join: Option<JoinHandle<()>>,
    watchdog: Option<JoinHandle<()>>,
    /// 看门狗停机信号：Drop 时丢掉它，看门狗从 `recv_timeout` 立刻收到断开并退出
    /// （比等它睡完一个巡检间隔才退出快，不拖慢应用退出）。
    watchdog_stop: Option<mpsc::Sender<()>>,
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        // 先停看门狗，否则它可能在钩子线程退出之后还去投递重装请求。
        self.watchdog_stop.take();
        if let Some(w) = self.watchdog.take() {
            let _ = w.join();
        }
        let _ = unsafe { PostThreadMessageW(self.tid, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 安装全局键盘与鼠标钩子（同一线程跑消息循环），并启动自愈看门狗。
/// 处理函数在钩子线程回调内同步执行。
pub fn start<F>(handler: F) -> io::Result<HookHandle>
where
    F: FnMut(KeyEvent) -> Action + Send + 'static,
{
    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    let (ready_tx, ready_rx) = mpsc::channel::<u32>();
    let join = thread::Builder::new()
        .name("kada-hook".into())
        .spawn(move || {
            if let Err(e) = hook_loop(&ready_tx) {
                eprintln!("kada-hook: {e}");
            }
        })?;
    let tid = ready_rx
        .recv()
        .map_err(|_| io::Error::other("钩子线程启动失败"))?;
    // 看门狗只负责「发现失效」并投递重装请求，真正装钩子的永远是钩子线程。
    let (watchdog_stop, watchdog_rx) = mpsc::channel::<()>();
    let watchdog = thread::Builder::new()
        .name("kada-hook-watchdog".into())
        .spawn(move || watchdog_loop(watchdog_rx))?;
    Ok(HookHandle {
        tid,
        join: Some(join),
        watchdog: Some(watchdog),
        watchdog_stop: Some(watchdog_stop),
    })
}

fn hook_loop(ready_tx: &mpsc::Sender<u32>) -> io::Result<()> {
    let tid = unsafe { GetCurrentThreadId() };
    install_hooks()?;
    HOOK_TID.store(tid, Ordering::Relaxed);
    let _ = ready_tx.send(tid);

    let mut msg = MSG::default();
    // GetMessageW 返回 0 = 收到 WM_QUIT
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        // 看门狗的重装请求：`SetWindowsHookEx` 必须在有消息循环的本线程上调用，
        // 所以看门狗只投递消息、由这里落地。
        if msg.message == WM_APP_REINSTALL {
            reinstall_hooks();
            continue;
        }
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unhook();
    reset_runtime_state();
    HOOK_TID.store(0, Ordering::Relaxed);
    *HANDLER.lock().unwrap() = None;
    Ok(())
}

/// 装上键盘钩子，并在需要时（配置引用了鼠标键）一并装鼠标钩子。只在钩子线程上调用。
fn install_hooks() -> io::Result<()> {
    let kbd = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }
        .map_err(|e| io::Error::other(format!("SetWindowsHookEx 键盘钩子失败: {e}")))?;
    // 鼠标钩子按需安装：配置没过鼠标键就不挂，缩小全局拦截面（见 [`MOUSE_ENABLED`]）。
    let mouse = if MOUSE_ENABLED.load(Ordering::Relaxed) {
        match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
            Ok(m) => m.0 as isize,
            Err(e) => {
                // 只挂上一半等于「键盘能用、鼠标键不能用」，不如整体退回让看门狗整轮重试。
                unsafe { _ = UnhookWindowsHookEx(kbd) };
                return Err(io::Error::other(format!("SetWindowsHookEx 鼠标钩子失败: {e}")));
            }
        }
    } else {
        0
    };
    HOOK.store(kbd.0 as isize, Ordering::Relaxed);
    MOUSE_HOOK.store(mouse, Ordering::Relaxed);
    // 心跳从这一刻重新起算：否则「最后一次钩子事件」还停在很久以前，看门狗下一轮
    // 就会把刚装好的钩子再判成失效。
    let now = unsafe { GetTickCount() };
    LAST_KEY_TICK.store(now, Ordering::Relaxed);
    LAST_MOUSE_TICK.store(now, Ordering::Relaxed);
    Ok(())
}

/// 卸下两个钩子（幂等：句柄已清零时什么也不做）。
fn unhook() {
    let kbd = HOOK.swap(0, Ordering::Relaxed);
    let mouse = MOUSE_HOOK.swap(0, Ordering::Relaxed);
    unsafe {
        if kbd != 0 {
            _ = UnhookWindowsHookEx(HHOOK(kbd as *mut c_void));
        }
        if mouse != 0 {
            _ = UnhookWindowsHookEx(HHOOK(mouse as *mut c_void));
        }
    }
}

/// 重建两个钩子（只在钩子线程上调用）：先卸旧、复位运行时状态、再装新的。
///
/// 先卸旧的这步不能省：钩子已经被系统摘除时 `UnhookWindowsHookEx` 只是失败（无害），
/// 但若旧钩子还活着而不卸就直接装新的，旧钩子会留在钩子链里**每个按键被处理两遍**
/// （快捷键触发两次）。
fn reinstall_hooks() {
    unhook();
    // 重装意味着中间丢过事件：那些物理键的抬起已经不可能再经过我们，只能就地复位，
    // 否则「按住集合」里的键永远等不到抬起（`detect_repeat` 把它的后续每次按下都
    // 当成自动重复 → 该键彻底变哑），被替换的键也永远收不到配对的 up。
    reset_runtime_state();
    match install_hooks() {
        Ok(()) => {
            REINSTALLS.fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => eprintln!("kada-hook: 重装钩子失败（{e}）；看门狗稍后重试"),
    }
}

/// 复位钩子层运行时状态（重装 / 退钩子时调用）。丢一次 keyup（钩子被摘除、锁屏、
/// Alt-Tab、提权窗口吞键）就会留下两份脏状态：`HELD_KEYS` 里那个键再也收不到抬起，
/// `SWALLOWED` / `REPLACED_DOWN` 里那条记录也永远等不到配对的抬起——后者尤其糟，
/// 被替换的目标键（常是 Ctrl 之类的修饰键）会在系统看来一直按着。
fn reset_runtime_state() {
    // 先把「已注入且被认为按着」的目标键补一个 up，再清登记表。
    for target in take_replaced_down() {
        simulate::up(target);
    }
    SWALLOWED.lock().unwrap().clear();
    HELD_KEYS.lock().unwrap().clear();
}

/// 取走所有待释放的替换目标键（宿主原键 → 已注入并按住的键），并清空登记表。
fn take_replaced_down() -> Vec<Key> {
    REPLACED_DOWN.lock().unwrap().drain().map(|(_, target)| target).collect()
}

/// 两个 `GetTickCount` 时间戳之差（毫秒，可正可负）。该计数是 32 位、约 49.7 天回绕，
/// 按无符号回绕相减再按有符号解释，在 ±24.8 天窗口内就是真实差值。
fn tick_diff(a: u32, b: u32) -> i32 {
    a.wrapping_sub(b) as i32
}

/// 判定哪一路钩子已经收不到事件（判据见 [`watchdog_loop`]）。`None` = 都还活着。
///
/// 只看「系统记下的最后输入」与两路心跳的**先后**，不看绝对时间：机器闲置时三者的
/// 时间戳一起停在原处，不构成失效。分路的关键在**归因**——`GetLastInputInfo` 是键盘
/// 与鼠标**合并**的信号，但鼠标钩子能看到**全部**鼠标事件（含移动），所以：
/// - 系统最后输入比鼠标心跳新出 `ATTRIB_GRACE_MS` ⇒ 那次输入不是鼠标（鼠标钩子活着
///   就看得到）⇒ 它是键盘输入；此时键盘心跳仍落后 >`HEARTBEAT_GRACE_MS` ⇒ **键盘钩子失效**。
/// - 对称地，系统最后输入比键盘心跳新出 `ATTRIB_GRACE_MS` ⇒ 它是鼠标输入；鼠标心跳
///   仍落后 >`HEARTBEAT_GRACE_MS` ⇒ **鼠标钩子失效**。
///
/// `mouse_hooked` = 鼠标钩子是否装着。没装就没有鼠标事件的观察者，无法把输入归因到
/// 键盘，退化为旧的「只看键盘心跳是否落后」（并保留鼠标活动造成的既有误判，维持原行为）。
///
/// 残留局限（已知）：鼠标**持续**移动时，系统最后输入始终约等于鼠标心跳，键盘输入被
/// 夹在两次鼠标事件之间、归因不出来——这半边失效要等到某次键盘输入处于两次巡检之间
/// 且其后没有鼠标事件时才被发现。
fn dead_hook(
    now: u32,
    last_input: u32,
    last_key: u32,
    last_mouse: u32,
    mouse_hooked: bool,
) -> Option<&'static str> {
    // 没有新输入：闲置不是失效。
    if tick_diff(now, last_input) >= HEARTBEAT_GRACE_MS {
        return None;
    }
    // 键盘失效：这次输入不是鼠标（鼠标钩子看得见全部鼠标事件）却键盘心跳停摆。
    if (!mouse_hooked || tick_diff(last_input, last_mouse) > ATTRIB_GRACE_MS)
        && tick_diff(last_input, last_key) > HEARTBEAT_GRACE_MS
    {
        return Some("键盘钩子回调超时被系统摘除");
    }
    // 鼠标失效：这次输入不是键盘（键盘钩子看得见全部键盘事件）却鼠标心跳停摆。
    if mouse_hooked
        && tick_diff(last_input, last_key) > ATTRIB_GRACE_MS
        && tick_diff(last_input, last_mouse) > HEARTBEAT_GRACE_MS
    {
        return Some("鼠标钩子回调超时被系统摘除");
    }
    None
}

/// 系统最后一次键盘 / 鼠标输入的时间戳（与 `GetTickCount` 同源）。查询失败返回 0 ——
/// `dead_hook` 会把它当成「很久没有输入」，不会据此重装。
fn last_input_tick() -> u32 {
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        info.dwTime
    } else {
        0
    }
}

/// 本会话是否已锁屏；`None` = 查不出来（当作「不知道」，不据此重装）。
///
/// `WTSSessionInfoEx` 的 `SessionFlags` 是**反直觉**字段：0 = 锁定、1 = 未锁定，
/// 不能当布尔值读；老系统只回 Level 0（没有这个标志位）时同样返回 None。
fn session_locked() -> Option<bool> {
    let mut buf = PWSTR::null();
    let mut len: u32 = 0;
    unsafe {
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &mut buf,
            &mut len,
        )
    }
    .ok()?;
    let locked = if buf.is_null() || (len as usize) < size_of::<WTSINFOEXW>() {
        None
    } else {
        let info = unsafe { &*buf.0.cast::<WTSINFOEXW>() };
        // Level 1 才带 SessionFlags；更高版本按「不知道」处理，别猜。
        if info.Level == 1 {
            Some(unsafe { info.Data.WTSInfoExLevel1.SessionFlags == 0 })
        } else {
            None
        }
    };
    if !buf.is_null() {
        unsafe { WTSFreeMemory(buf.0 as *mut c_void) };
    }
    locked
}

/// 请求钩子线程重建钩子（看门狗用；也可作为「钩子疑似失效」时的人工兜底）。
/// 返回 false = 投递失败（钩子线程已退出）。
pub fn request_reinstall() -> bool {
    let tid = HOOK_TID.load(Ordering::Relaxed);
    tid != 0 && unsafe { PostThreadMessageW(tid, WM_APP_REINSTALL, WPARAM(0), LPARAM(0)) }.is_ok()
}

/// 告诉钩子层「配置现在（不）需要鼠标钩子」：值变了才请求重装（重装会在钩子线程上按新值
/// 重挂，鼠标钩子随之增删）。启动前调用同样有效——此刻钩子线程还没起来，`request_reinstall`
/// 返回 false 什么也不做，值已被记下，首次安装即按它决定。见 [`MOUSE_ENABLED`]。
pub fn set_mouse_enabled(on: bool) {
    if MOUSE_ENABLED.swap(on, Ordering::Relaxed) != on {
        request_reinstall();
    }
}

/// 自愈重装累计次数（诊断 / 冒烟测试用）。
pub fn reinstall_count() -> u64 {
    REINSTALLS.load(Ordering::Relaxed)
}

/// 看门狗线程：盯住「钩子还活着」的证据，失效时把重装请求投回钩子线程。
///
/// 三条触发路径对应钩子静默失效的三种成因（托盘那时都还看着正常）：
/// 1. **回调超时被系统摘除**——回调超过 `LowLevelHooksTimeout`（默认 300ms）系统会
///    静默摘掉钩子且不给任何通知；靠 `GetLastInputInfo` 与**键盘 / 鼠标两路心跳**对照
///    发现，并能定出是哪一路（[`dead_hook`]）。
/// 2. **休眠唤醒**——`GetTickCount` 把睡眠时间算在内，两次巡检的间隔突然变大即说明
///    机器睡过（唤醒后钩子常常收不到事件）。
/// 3. **会话解锁**——锁屏期间的输入走安全桌面、不经过我们的钩子，解锁是用户回来继续
///    用快捷键的时刻，主动重建一次（`WTSInfoEx` 查本会话状态）。
fn watchdog_loop(stop: mpsc::Receiver<()>) {
    let mut prev_tick = unsafe { GetTickCount() };
    let mut locked = session_locked();
    // 连续触发次数：一次成功的重装会让触发条件下一轮自然消失（心跳被刷新、gap 归零、
    // 解锁是一次性跃迁），所以「下一轮又触发」就等于上次没修好 → 指数退避，别每秒重装。
    let mut streak: u32 = 0;
    loop {
        let wait = WATCHDOG_INTERVAL_MS << streak.min(4); // 1s → 最多 16s
        match stop.recv_timeout(Duration::from_millis(wait)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = unsafe { GetTickCount() };
        let gap = tick_diff(now, prev_tick);
        prev_tick = now;

        let session = session_locked();
        let just_unlocked = locked == Some(true) && session == Some(false);
        locked = session;

        let reason = if let Some(dead) = dead_hook(
            now,
            last_input_tick(),
            LAST_KEY_TICK.load(Ordering::Relaxed),
            LAST_MOUSE_TICK.load(Ordering::Relaxed),
            MOUSE_HOOK.load(Ordering::Relaxed) != 0,
        ) {
            Some(dead)
        } else if gap > SUSPEND_GAP_MS {
            Some("休眠唤醒")
        } else if just_unlocked {
            Some("会话解锁")
        } else {
            None
        };

        let Some(reason) = reason else {
            streak = 0;
            continue;
        };
        streak = streak.saturating_add(1);
        if !request_reinstall() {
            return; // 钩子线程没了，看门狗没有意义
        }
        // 自愈是「无声兜底」（用户此刻没有动作可做），只在 stderr 留痕 + 累加计数，
        // 不进消息中心。
        eprintln!(
            "kada-hook: 看门狗重建输入钩子（{reason}）{}",
            if streak > 1 { format!("，连续第 {streak} 次") } else { String::new() }
        );
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 心跳：键盘钩子被系统摘除后就再也不会被调用，看门狗靠这个时间戳判断它是否还活着。
    // `GetTickCount` 只读内核共享页，不算「回调里的耗时操作」。
    LAST_KEY_TICK.store(unsafe { GetTickCount() }, Ordering::Relaxed);
    if code >= 0 {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // 注入事件一律放行，防回环。
        if kb.flags.0 & LLKHF_INJECTED.0 == 0 && swallow(wparam.0 as u32, kb) {
            return LRESULT(1);
        }
    }
    let hook = HOOK.load(Ordering::Relaxed);
    let hhook = if hook == 0 { None } else { Some(HHOOK(hook as *mut std::ffi::c_void)) };
    unsafe { CallNextHookEx(hhook, code, wparam, lparam) }
}

fn swallow(wparam: u32, kb: &KBDLLHOOKSTRUCT) -> bool {
    let extended = kb.flags.0 & LLKHF_EXTENDED.0 != 0;
    let Some(key) = vk_to_key(VIRTUAL_KEY(kb.vkCode as u16), extended) else {
        return false;
    };
    let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);

    if !down {
        // keyup：先解除「按下」标记（自动重复判定的依据），被吞掉的键仍吞掉（防幽灵），
        // 但也会以观察者身份回调 handler（tap-hold 需要在 keyup 时判定 tap/hold）；
        // 其余纯观察通知、放行。
        note_key_up(key);
        let swallowed = SWALLOWED.lock().unwrap().remove(&key);
        if swallowed {
            if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
                simulate::up(target);
            }
        }
        if let Some(f) = HANDLER.lock().unwrap().as_mut() {
            let _ = f(KeyEvent::Up { key, mods: current_mods() });
        }
        return swallowed;
    }

    let mut mods = current_mods();
    if let Some(m) = key_as_modifier(key) {
        mods.insert(m);
    }
    let repeat = detect_repeat(key);

    let action = {
        let mut h = HANDLER.lock().unwrap();
        match h.as_mut() {
            Some(f) => f(KeyEvent::Down { key, mods, repeat }),
            None => Action::Allow,
        }
    };

    match action {
        Action::Allow => false,
        Action::Block => {
            SWALLOWED.lock().unwrap().insert(key);
            true
        }
        Action::Replace(target) => {
            SWALLOWED.lock().unwrap().insert(key);
            if !repeat {
                let mut r = REPLACED_DOWN.lock().unwrap();
                if !r.contains_key(&key) {
                    r.insert(key, target);
                    simulate::down(target);
                }
            }
            true
        }
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 心跳：鼠标钩子对**所有**鼠标事件都会被调用（不止我们翻译的中键/侧键），所以
    // 只要用户在动鼠标，这里就能证明鼠标钩子还活着。
    LAST_MOUSE_TICK.store(unsafe { GetTickCount() }, Ordering::Relaxed);
    if code >= 0 {
        let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        // 注入事件一律放行，防回环。
        if ms.flags & LLMHF_INJECTED == 0 && swallow_mouse(wparam.0 as u32, ms) {
            return LRESULT(1);
        }
    }
    let hook = MOUSE_HOOK.load(Ordering::Relaxed);
    let hhook = if hook == 0 { None } else { Some(HHOOK(hook as *mut std::ffi::c_void)) };
    unsafe { CallNextHookEx(hhook, code, wparam, lparam) }
}

/// 鼠标侧键/中键事件 → 决定吞掉/改键/放行。只处理中键与 X 侧键（MB4/MB5），
/// 左右键、滚轮、移动一律放行（不纳入键模型，避免全局误拦截点击）。
fn swallow_mouse(wparam: u32, ms: &MSLLHOOKSTRUCT) -> bool {
    // mouseData 高 16 位承载 X 按钮号：1 = XBUTTON1（后退/MB4），2 = XBUTTON2（前进/MB5）。
    let x_button = |ms: &MSLLHOOKSTRUCT| ms.mouseData >> 16;
    let (key, down) = match wparam {
        WM_MBUTTONDOWN => (Key::MouseMiddle, true),
        WM_MBUTTONUP => (Key::MouseMiddle, false),
        WM_XBUTTONDOWN => match x_button(ms) {
            1 => (Key::MouseBack, true),
            2 => (Key::MouseForward, true),
            _ => return false,
        },
        WM_XBUTTONUP => match x_button(ms) {
            1 => (Key::MouseBack, false),
            2 => (Key::MouseForward, false),
            _ => return false,
        },
        _ => return false,
    };

    if !down {
        // keyup：被吞掉的键仍吞掉（防幽灵），但同样以观察者身份回调 handler——与键盘路径
        // 一致。状态机靠抬起事件维护「按住集合」（和弦成员、tap-hold 的短按/长按判定），
        // 漏掉被吞键的抬起会让该键与整个状态机永久卡住（键「变哑」）。
        let swallowed = SWALLOWED.lock().unwrap().remove(&key);
        if swallowed {
            if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
                simulate::up(target);
            }
        }
        if let Some(f) = HANDLER.lock().unwrap().as_mut() {
            let _ = f(KeyEvent::Up { key, mods: current_mods() });
        }
        return swallowed;
    }

    let mods = current_mods();
    let action = {
        let mut h = HANDLER.lock().unwrap();
        match h.as_mut() {
            Some(f) => f(KeyEvent::Down { key, mods, repeat: false }),
            None => Action::Allow,
        }
    };

    match action {
        Action::Allow => false,
        Action::Block => {
            SWALLOWED.lock().unwrap().insert(key);
            true
        }
        Action::Replace(target) => {
            SWALLOWED.lock().unwrap().insert(key);
            REPLACED_DOWN.lock().unwrap().insert(key, target);
            simulate::down(target);
            true
        }
    }
}

fn current_mods() -> BTreeSet<Modifier> {
    let mut mods = BTreeSet::new();
    if key_is_down(VK_SHIFT) {
        mods.insert(Modifier::Shift);
    }
    if key_is_down(VK_CONTROL) {
        mods.insert(Modifier::Ctrl);
    }
    if key_is_down(VK_MENU) {
        mods.insert(Modifier::Alt);
    }
    if key_is_down(VK_LWIN) || key_is_down(VK_RWIN) {
        mods.insert(Modifier::Meta);
    }
    mods
}

fn key_is_down(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetKeyState(vk.0 as i32) }) < 0
}

fn key_as_modifier(k: Key) -> Option<Modifier> {
    match k {
        Key::Shift => Some(Modifier::Shift),
        Key::Control => Some(Modifier::Ctrl),
        Key::Alt => Some(Modifier::Alt),
        Key::Meta => Some(Modifier::Meta),
        _ => None,
    }
}

fn detect_repeat(key: Key) -> bool {
    // 自动重复 = 该键仍处于按下状态时又收到 down。用「按下集合」而非时间窗判定：同一键
    // 在 250ms 内连按两次是正常连打（热串触发词 `addr` 的双写 d、双击/三击改键），
    // 按时间窗会被误判成 repeat 而丢掉第二击，导致热串缓冲缺字 / 连击不计击数。
    !HELD_KEYS.lock().unwrap().insert(key)
}

/// 键抬起：移出「按下集合」。无论事件最终吞掉与否都必须调用，否则集合只增不减，
/// 后续所有该键的按下都会被误判为自动重复。
fn note_key_up(key: Key) {
    HELD_KEYS.lock().unwrap().remove(&key);
}

/// 虚拟键码 ↔ [`Key`]。映射使用 US 布局语义，OEM 标点键的实际位置因
/// 键盘布局而异（中文键盘 Shift 后字符不同，但键码相同）。
/// 鼠标键（中键/侧键）不是虚拟键码，返回 `None`——注入走 `simulate` 的鼠标事件。
pub fn key_to_vk(k: Key) -> Option<u16> {
    use Key::*;
    let vk = match k {
        A => VK_A, B => VK_B, C => VK_C, D => VK_D, E => VK_E, F => VK_F,
        G => VK_G, H => VK_H, I => VK_I, J => VK_J, K => VK_K, L => VK_L,
        M => VK_M, N => VK_N, O => VK_O, P => VK_P, Q => VK_Q, R => VK_R,
        S => VK_S, T => VK_T, U => VK_U, V => VK_V, W => VK_W, X => VK_X,
        Y => VK_Y, Z => VK_Z,
        Digit0 => VK_0, Digit1 => VK_1, Digit2 => VK_2, Digit3 => VK_3,
        Digit4 => VK_4, Digit5 => VK_5, Digit6 => VK_6, Digit7 => VK_7,
        Digit8 => VK_8, Digit9 => VK_9,
        F1 => VK_F1, F2 => VK_F2, F3 => VK_F3, F4 => VK_F4,
        F5 => VK_F5, F6 => VK_F6, F7 => VK_F7, F8 => VK_F8,
        F9 => VK_F9, F10 => VK_F10, F11 => VK_F11, F12 => VK_F12,
        F13 => VK_F13, F14 => VK_F14, F15 => VK_F15, F16 => VK_F16,
        F17 => VK_F17, F18 => VK_F18, F19 => VK_F19, F20 => VK_F20,
        F21 => VK_F21, F22 => VK_F22, F23 => VK_F23, F24 => VK_F24,
        Comma => VK_OEM_COMMA, Period => VK_OEM_PERIOD, Slash => VK_OEM_2,
        Backslash => VK_OEM_5, Semicolon => VK_OEM_1, Quote => VK_OEM_7,
        Backquote => VK_OEM_3, Minus => VK_OEM_MINUS, Equal => VK_OEM_PLUS,
        BracketLeft => VK_OEM_4, BracketRight => VK_OEM_6,
        Enter => VK_RETURN, Escape => VK_ESCAPE, Tab => VK_TAB, Space => VK_SPACE,
        Backspace => VK_BACK, Delete => VK_DELETE, Insert => VK_INSERT,
        CapsLock => VK_CAPITAL,
        Shift => VK_SHIFT, Control => VK_CONTROL, Alt => VK_MENU, Meta => VK_LWIN,
        Home => VK_HOME, End => VK_END, PageUp => VK_PRIOR, PageDown => VK_NEXT,
        ArrowUp => VK_UP, ArrowDown => VK_DOWN, ArrowLeft => VK_LEFT,
        ArrowRight => VK_RIGHT,
        MediaPlayPause => VK_MEDIA_PLAY_PAUSE, MediaPrev => VK_MEDIA_PREV_TRACK,
        MediaNext => VK_MEDIA_NEXT_TRACK, VolumeMute => VK_VOLUME_MUTE,
        VolumeDown => VK_VOLUME_DOWN, VolumeUp => VK_VOLUME_UP,
        Numpad0 => VK_NUMPAD0, Numpad1 => VK_NUMPAD1, Numpad2 => VK_NUMPAD2,
        Numpad3 => VK_NUMPAD3, Numpad4 => VK_NUMPAD4, Numpad5 => VK_NUMPAD5,
        Numpad6 => VK_NUMPAD6, Numpad7 => VK_NUMPAD7, Numpad8 => VK_NUMPAD8,
        Numpad9 => VK_NUMPAD9,
        NumpadAdd => VK_ADD, NumpadSubtract => VK_SUBTRACT,
        NumpadMultiply => VK_MULTIPLY, NumpadDivide => VK_DIVIDE,
        NumpadDecimal => VK_DECIMAL, NumpadEnter => VK_RETURN, NumLock => VK_NUMLOCK,
        // 鼠标键注入走 SendInput 鼠标事件（simulate），非虚拟键码。
        MouseMiddle | MouseBack | MouseForward => return None,
    };
    Some(vk.0)
}

/// 探测某快捷键是否已被系统或其他应用注册（Windows `RegisterHotKey` 试探）。
/// 返回 true 表示「已被占用」。组合键需至少一个修饰键且主键可映射为虚拟键码。
pub fn hotkey_occupied(shortcut: &Shortcut) -> bool {
    let Some(vk) = key_to_vk(shortcut.key) else { return false };
    if shortcut.mods.is_empty() {
        return false;
    }
    let mut mods: u32 = 0;
    for m in &shortcut.mods {
        mods |= match m {
            Modifier::Alt => MOD_ALT.0,
            Modifier::Ctrl => MOD_CONTROL.0,
            Modifier::Shift => MOD_SHIFT.0,
            Modifier::Meta => MOD_WIN.0,
        };
    }
    // 瞬时试探：注册成功说明空闲，立即注销；失败且错误码 1409 = 已被占用。
    let id = 0xB000 + vk as i32;
    unsafe {
        match RegisterHotKey(None, id, HOT_KEY_MODIFIERS(mods), vk as u32) {
            Ok(()) => {
                let _ = UnregisterHotKey(None, id);
                false
            }
            // ERROR_HOTKEY_ALREADY_REGISTERED = 1409（HRESULT 低 16 位承载 Win32 错误码）
            Err(e) => (e.code().0 as u32) & 0xFFFF == 1409,
        }
    }
}

/// 当前前台窗口句柄（`None` = 当前没有前台窗口，**或前台是本进程自己的窗口**）。
///
/// 壳层用它做「前台切换 → 输入状态复位」的变化检测（见规划 7.2-④）：**只看句柄变没变**，
/// 不查标题与进程名——[`frontmost_context`] 要 `OpenProcess` 拿镜像路径，那是判定条件时
/// 才值得付的代价，而这里每 60ms 轮询一次，必须便宜（`GetForegroundWindow` 只读一个全局）。
///
/// **本进程自己的窗口一律报 `None`**：主窗 / 触发气泡 / 状态指示 / 快捷键提示框都是「我们
/// 自己弹出来的」，不是用户换了工作窗口。浮窗虽是 `focusable(false)`，但 WebView2 的子窗
/// 仍可能让顶层窗口短暂成为前台；一旦被 `FocusTracker` 当成「前台切换」，就会 `Engine::reset`
/// ——按住的 momentary 层被踢掉、凑到一半的和弦被丢弃（锁定层因复位保留而不受影响，正是
/// 「按住切层键触发和弦失败、锁定层却正常」这个不对称的来源）。取不到 pid 时按「不是自己」
/// 处理（宁可多复位一次，也不错杀真实切换）。
pub fn foreground_window() -> Option<isize> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    if is_own_window(hwnd) {
        return None;
    }
    Some(hwnd.0 as isize)
}

/// 窗口句柄是否属于本进程（[`foreground_window`] 据此忽略我们自己的窗口）。
fn is_own_window(hwnd: HWND) -> bool {
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}

/// 取当前前台窗口上下文（进程名 + 窗口标题），供「按前台应用/窗口」类条件求值。
/// 无前台窗口或权限不足时返回 None。
pub fn frontmost_context() -> Option<FrontmostContext> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    // 窗口标题（无标题时为空串）。
    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    let window_title = String::from_utf16_lossy(&title_buf[..title_len.max(0) as usize]);
    // 进程名：窗口句柄 → 进程 id → 完整镜像路径 → 文件名。
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let process_name = process_name_of(pid).unwrap_or_default();
    Some(FrontmostContext { process_name, window_title })
}

/// 最近一次事件的设备标识。**Windows 恒 `None`**：`WH_KEYBOARD_LL` 拿不到设备信息
/// （真实区分多键盘要另挂 Raw Input `WM_INPUT` 旁路 + SetupAPI 枚举设备名，见规划
/// 7.3-㉓ 的后续）。壳层据此把「设备是」条件在本平台标注为不可用。
pub fn current_device() -> Option<String> {
    None
}

/// 由进程 id 取可执行文件名（如 `chrome.exe`）；失败返回 None。
fn process_name_of(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    ok.ok()?;
    let full = String::from_utf16_lossy(&buf[..len as usize]);
    full.rsplit(['\\', '/']).next().map(str::to_string)
}

/// 取 [`Key`] 名的可打印形式，未知键码返回 None（放行）。
/// `extended` 为钩子结构里的 `LLKHF_EXTENDED`：Windows 上主键区 Enter 与小键盘
/// Enter 共用 `VK_RETURN`，靠扩展位区分。
fn vk_to_key(vk: VIRTUAL_KEY, extended: bool) -> Option<Key> {
    use Key::*;
    Some(match vk {
        VK_LSHIFT | VK_RSHIFT | VK_SHIFT => Shift,
        VK_LCONTROL | VK_RCONTROL | VK_CONTROL => Control,
        VK_LMENU | VK_RMENU | VK_MENU => Alt,
        VK_LWIN | VK_RWIN => Meta,
        VK_A => A, VK_B => B, VK_C => C, VK_D => D, VK_E => E, VK_F => F,
        VK_G => G, VK_H => H, VK_I => I, VK_J => J, VK_K => K, VK_L => L,
        VK_M => M, VK_N => N, VK_O => O, VK_P => P, VK_Q => Q, VK_R => R,
        VK_S => S, VK_T => T, VK_U => U, VK_V => V, VK_W => W, VK_X => X,
        VK_Y => Y, VK_Z => Z,
        VK_0 => Digit0, VK_1 => Digit1, VK_2 => Digit2, VK_3 => Digit3,
        VK_4 => Digit4, VK_5 => Digit5, VK_6 => Digit6, VK_7 => Digit7,
        VK_8 => Digit8, VK_9 => Digit9,
        VK_F1 => F1, VK_F2 => F2, VK_F3 => F3, VK_F4 => F4,
        VK_F5 => F5, VK_F6 => F6, VK_F7 => F7, VK_F8 => F8,
        VK_F9 => F9, VK_F10 => F10, VK_F11 => F11, VK_F12 => F12,
        VK_F13 => F13, VK_F14 => F14, VK_F15 => F15, VK_F16 => F16,
        VK_F17 => F17, VK_F18 => F18, VK_F19 => F19, VK_F20 => F20,
        VK_F21 => F21, VK_F22 => F22, VK_F23 => F23, VK_F24 => F24,
        VK_OEM_COMMA => Comma, VK_OEM_PERIOD => Period, VK_OEM_2 => Slash,
        VK_OEM_5 => Backslash, VK_OEM_1 => Semicolon, VK_OEM_7 => Quote,
        VK_OEM_3 => Backquote, VK_OEM_MINUS => Minus, VK_OEM_PLUS => Equal,
        VK_OEM_4 => BracketLeft, VK_OEM_6 => BracketRight,
        VK_RETURN => if extended { NumpadEnter } else { Enter },
        VK_ESCAPE => Escape, VK_TAB => Tab, VK_SPACE => Space,
        VK_BACK => Backspace, VK_DELETE => Delete, VK_INSERT => Insert,
        VK_CAPITAL => CapsLock,
        VK_HOME => Home, VK_END => End, VK_PRIOR => PageUp, VK_NEXT => PageDown,
        VK_UP => ArrowUp, VK_DOWN => ArrowDown, VK_LEFT => ArrowLeft,
        VK_RIGHT => ArrowRight,
        VK_MEDIA_PLAY_PAUSE => MediaPlayPause, VK_MEDIA_PREV_TRACK => MediaPrev,
        VK_MEDIA_NEXT_TRACK => MediaNext, VK_VOLUME_MUTE => VolumeMute,
        VK_VOLUME_DOWN => VolumeDown, VK_VOLUME_UP => VolumeUp,
        VK_NUMPAD0 => Numpad0, VK_NUMPAD1 => Numpad1, VK_NUMPAD2 => Numpad2,
        VK_NUMPAD3 => Numpad3, VK_NUMPAD4 => Numpad4, VK_NUMPAD5 => Numpad5,
        VK_NUMPAD6 => Numpad6, VK_NUMPAD7 => Numpad7, VK_NUMPAD8 => Numpad8,
        VK_NUMPAD9 => Numpad9,
        VK_ADD => NumpadAdd, VK_SUBTRACT => NumpadSubtract,
        VK_MULTIPLY => NumpadMultiply, VK_DIVIDE => NumpadDivide,
        VK_DECIMAL => NumpadDecimal, VK_NUMLOCK => NumLock,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kada_core::key_name;
    use std::sync::Arc;

    /// 动全局状态的用例必须串行——cargo test 默认多线程并行，两个用例同时改
    /// [`HANDLER`] / [`SWALLOWED`] / [`HELD_KEYS`] / [`REPLACED_DOWN`] 会互相打断
    /// （尤其「复位运行时状态」的那条会把别人正依赖的状态清掉）。中毒（上一个用例
    /// panic）也继续跑，别让一个失败连带整片红。
    static HANDLER_LOCK: Mutex<()> = Mutex::new(());

    fn lock_handler() -> std::sync::MutexGuard<'static, ()> {
        HANDLER_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn mapping_roundtrip() {
        for k in [
            Key::A, Key::K, Key::Digit9, Key::F12, Key::CapsLock, Key::Control,
            Key::Alt, Key::Shift, Key::Meta, Key::Enter, Key::Escape, Key::Space,
            Key::Comma, Key::Minus, Key::Slash, Key::BracketLeft, Key::Quote,
            Key::ArrowUp, Key::PageDown,
            Key::F13, Key::F24, Key::MediaPlayPause, Key::MediaPrev, Key::MediaNext,
            Key::VolumeMute, Key::VolumeDown, Key::VolumeUp, Key::NumLock,
            Key::Numpad0, Key::Numpad9, Key::NumpadAdd, Key::NumpadSubtract,
            Key::NumpadMultiply, Key::NumpadDivide, Key::NumpadDecimal,
        ] {
            let vk = key_to_vk(k).unwrap();
            let name = key_name(k);
            assert_eq!(vk_to_key(VIRTUAL_KEY(vk), false), Some(k), "vk roundtrip {name}");
        }
        // NumpadEnter 与主键区 Enter 共用 VK_RETURN，靠扩展位区分。
        assert_eq!(key_to_vk(Key::NumpadEnter), Some(VK_RETURN.0));
        assert_eq!(vk_to_key(VK_RETURN, true), Some(Key::NumpadEnter));
        assert_eq!(vk_to_key(VK_RETURN, false), Some(Key::Enter));
        // 鼠标键不是虚拟键码。
        assert_eq!(key_to_vk(Key::MouseBack), None);
        assert_eq!(key_to_vk(Key::MouseMiddle), None);
    }

    #[test]
    fn swallow_up_only_for_registered_keys() {
        // 未登记的 keyup 放行
        assert!(!swallow(WM_KEYUP, &KBDLLHOOKSTRUCT::default()));
    }

    #[test]
    fn own_process_window_is_not_a_foreground_switch() {
        // 前台句柄落在本进程自己的窗口上（主窗 / 气泡 / 指示 / 提示框）时必须报 None：
        // 壳层的 `FocusTracker` 靠它判「前台切换 → 复位」，误判会踢掉按住的 momentary 层、
        // 丢掉凑到一半的和弦（锁定层因复位保留，正是 hold_layer 触发和弦失败、lock_layer
        // 正常这个不对称的来源）。用 message-only 窗口验证 pid 判定，不依赖真实前台。
        use windows::core::w;
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("kada-test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .expect("创建 message-only 窗口");
        assert!(is_own_window(hwnd), "本进程创建的窗口应判为「自己」");
        unsafe { let _ = DestroyWindow(hwnd); }
    }

    #[test]
    fn swallowed_mouse_up_still_notifies_handler() {
        let _g = lock_handler();
        // 被吞掉的鼠标抬起必须照样回调 handler：和弦成员/改键用中键或侧键时，状态机
        // 靠抬起维护「按住集合」，漏掉它会让该键与整个状态机永久卡死（键变哑）。
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        *HANDLER.lock().unwrap() = Some(Box::new(move |ev: KeyEvent| {
            sink.lock().unwrap().push(format!("{ev:?}"));
            Action::Block // 模拟状态机吞掉成员键
        }));

        // mouseData 高 16 位 = 1：XBUTTON1（后退/MB4）。
        let kb = MSLLHOOKSTRUCT { mouseData: 1 << 16, ..Default::default() };
        assert!(swallow_mouse(WM_XBUTTONDOWN, &kb), "成员键按下被吞");
        assert!(swallow_mouse(WM_XBUTTONUP, &kb), "抬起一并吞掉（防幽灵）");

        let seen = events.lock().unwrap().clone();
        *HANDLER.lock().unwrap() = None;
        let _ = SWALLOWED.lock().unwrap().remove(&Key::MouseBack);
        assert_eq!(seen.len(), 2, "按下与抬起都要回调 handler，实际：{seen:?}");
        assert!(seen[0].contains("Down") && seen[1].contains("Up"), "实际：{seen:?}");
    }

    #[test]
    fn keyboard_block_marks_swallowed_and_release_notifies() {
        let _g = lock_handler();
        // 键盘路径的 Block 登记与释放（鼠标路径有单独用例，键盘这条此前没覆盖）：
        // 被吞的按下要进 SWALLOWED（否则 keyup 放行会留下「幽灵按键」），抬起要从登记表
        // 移除，且按下/抬起都得以观察者身份回调 handler——漏掉抬起会让状态机永久卡死。
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        *HANDLER.lock().unwrap() = Some(Box::new(move |ev: KeyEvent| {
            sink.lock().unwrap().push(format!("{ev:?}"));
            Action::Block
        }));

        let kb = KBDLLHOOKSTRUCT { vkCode: 0x4D, ..Default::default() }; // VK_M
        assert!(swallow(WM_KEYDOWN, &kb), "Block 的按下要被吞");
        assert!(
            SWALLOWED.lock().unwrap().contains(&Key::M),
            "被吞的键要登记，keyup 才能一并吞掉（防幽灵按键）"
        );
        assert!(swallow(WM_KEYUP, &kb), "被吞键的抬起也要吞");
        assert!(
            !SWALLOWED.lock().unwrap().contains(&Key::M),
            "抬起后要从登记表移除"
        );

        let seen = events.lock().unwrap().clone();
        *HANDLER.lock().unwrap() = None;
        note_key_up(Key::M);
        assert_eq!(seen.len(), 2, "按下与抬起都要回调 handler，实际：{seen:?}");
        assert!(seen[0].contains("Down") && seen[1].contains("Up"), "实际：{seen:?}");
    }

    #[test]
    fn second_press_of_same_key_is_not_repeat() {
        let _g = lock_handler();
        // 敲两下同一个键（`addr` 的双写 d）：第二击不能被判成自动重复，否则热串缓冲少一个
        // 字符，触发词永远不命中（文本扩展整体失效）。同时验证抬起确实清了「按住集合」
        // ——若 note_key_up 被挪进「仅未吞键才调用」的分支，这个用例会立刻炸。
        let events: Arc<Mutex<Vec<KeyEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        *HANDLER.lock().unwrap() = Some(Box::new(move |ev: KeyEvent| {
            sink.lock().unwrap().push(ev);
            Action::Allow
        }));

        // VK_P：避开其它用例（它们用 D / K）。
        let key = VIRTUAL_KEY(0x50); // VK_P
        let kb = KBDLLHOOKSTRUCT { vkCode: key.0 as u32, ..Default::default() };
        for _ in 0..2 {
            swallow(WM_KEYDOWN, &kb);
            let seen = events.lock().unwrap().clone();
            match seen.last().expect("handler 每次都要收到事件") {
                KeyEvent::Down { key: k, repeat, .. } => {
                    assert_eq!(*k, Key::P);
                    assert!(!*repeat, "同键连打的第二击不能是自动重复");
                }
                other => panic!("期望 Down，实际 {other:?}"),
            }
            swallow(WM_KEYUP, &kb);
        }

        *HANDLER.lock().unwrap() = None;
        let _ = SWALLOWED.lock().unwrap().remove(&Key::P);
        note_key_up(Key::P);
    }

    #[test]
    fn auto_repeat_requires_key_still_held() {
        let _g = lock_handler();
        // 同键连打两次（中间有抬起）不是自动重复：热串触发词 `addr` 的双写 d、双击/三击
        // 改键都依赖第二击被当成独立按下，否则热串缓冲缺字（不触发扩展）、连击不计击数。
        assert!(!detect_repeat(Key::D), "首次按下不是重复");
        note_key_up(Key::D);
        assert!(!detect_repeat(Key::D), "抬起后再按不是重复");
        note_key_up(Key::D);

        // 未抬起再次 down = 自动重复（长按连发）。
        assert!(!detect_repeat(Key::K));
        assert!(detect_repeat(Key::K), "按住期间的再次 down 是重复");
        assert!(detect_repeat(Key::K));
        note_key_up(Key::K);
        assert!(!detect_repeat(Key::K), "抬起后又恢复成独立按下");
        note_key_up(Key::K);
    }

    #[test]
    fn modifier_and_toggle_keys_never_repeat_after_release() {
        let _g = lock_handler();
        // 双击唤醒（双击 Alt）依赖两次独立 down：抬起后再按不能被判成重复。
        for k in [Key::Alt, Key::Control, Key::Shift, Key::Meta, Key::CapsLock, Key::NumLock] {
            assert!(!detect_repeat(k), "{} 首次按下不是重复", key_name(k));
            note_key_up(k);
            assert!(!detect_repeat(k), "{} 抬起后再按不是重复", key_name(k));
            note_key_up(k);
        }
    }

    #[test]
    fn watchdog_spots_hook_that_stopped_receiving_events() {
        // 系统刚有输入，而两路钩子最后一次事件都停在同样久之前 → 钩子已被摘除。
        assert!(dead_hook(10_000, 10_000, 7_000, 7_000, true).is_some(), "系统有输入而钩子漏了 → 失效");
        // 正常时序：输入到了，对应钩子回调也跟着跑了（落差远小于宽限）。
        assert!(dead_hook(10_000, 9_900, 9_900, 9_900, true).is_none(), "钩子跟得上就不能重装");
        // 机器闲置：没有新输入，钩子没有事件是正常的——三个时间戳一起停在原处。
        assert!(dead_hook(60_000, 30_000, 30_000, 30_000, true).is_none(), "闲置不是失效");
        // 输入刚到、钩子回调还没跑完（落差在宽限内）→ 不能判失效。
        assert!(dead_hook(10_000, 10_000, 9_200, 9_200, true).is_none(), "回调延迟不算失效");
        // 钩子事件比系统记录的最后输入还新（我们自己的注入同样会刷新心跳）→ 活的。
        assert!(dead_hook(10_000, 9_900, 9_900, 10_000, true).is_none(), "心跳比输入新就是活的");
    }

    #[test]
    fn watchdog_separates_keyboard_and_mouse_half_failures() {
        // 键盘半边失效：鼠标刚动过（鼠标心跳新鲜），随后用户按键——系统最后输入是键盘那次、
        // 比鼠标心跳新出 > 归因宽限，而键盘心跳停在很久以前 → 定出是键盘失效。
        assert_eq!(
            dead_hook(10_000, 10_000, 7_000, 9_500, true),
            Some("键盘钩子回调超时被系统摘除"),
        );
        // 鼠标半边失效：键盘刚打过（键盘心跳新鲜），随后用户动鼠标，而鼠标心跳停在很久以前。
        assert_eq!(
            dead_hook(10_000, 10_000, 9_500, 7_000, true),
            Some("鼠标钩子回调超时被系统摘除"),
        );
        // 鼠标持续移动时系统最后输入≈鼠标心跳，键盘输入夹在两次鼠标事件之间归因不出来
        // ——已知残留局限：这一拍不判失效。
        assert!(dead_hook(10_000, 10_000, 7_000, 10_000, true).is_none(), "鼠标喂着心跳时键盘半边失效这一拍探不到");
        // 鼠标钩子没装：没有鼠标事件的观察者，退化为只看键盘心跳是否落后（与旧行为一致）。
        assert!(dead_hook(10_000, 10_000, 7_000, 0, false).is_some(), "没装鼠标钩子时仍能判键盘失效");
        // 闲置 / 没有新输入时不判。
        assert!(dead_hook(60_000, 30_000, 30_000, 30_000, true).is_none());
    }

    #[test]
    fn tick_diff_survives_32bit_wrap() {
        // GetTickCount 约 49.7 天归零：跨回绕的差值必须仍是对的，否则看门狗会在回绕点
        // 前后连续误判（误判本身无害，但会连着重装、把按键吃掉）。
        let before_wrap = u32::MAX - 4_000;
        assert_eq!(tick_diff(1_000, before_wrap), 5_001);
        assert_eq!(tick_diff(before_wrap, 1_000), -5_001);
        assert!(
            dead_hook(1_000, 1_000, before_wrap, before_wrap, true).is_some(),
            "跨回绕也要判得出失效"
        );
    }

    #[test]
    fn reset_releases_stale_state() {
        let _g = lock_handler();
        // 模拟「钩子被摘除时正按着 / 正吞着键」留下的脏状态。
        SWALLOWED.lock().unwrap().insert(Key::K);
        HELD_KEYS.lock().unwrap().insert(Key::K);
        reset_runtime_state();
        assert!(
            SWALLOWED.lock().unwrap().is_empty(),
            "被吞的键必须清空，否则它永远等不到抬起"
        );
        assert!(
            HELD_KEYS.lock().unwrap().is_empty(),
            "按住集合必须清空，否则该键的后续按下全被判成自动重复（键变哑）"
        );
        assert!(REPLACED_DOWN.lock().unwrap().is_empty());
    }

    #[test]
    fn reset_hands_out_replaced_keys_for_release() {
        let _g = lock_handler();
        // 重装前必须把「已注入并按着」的目标键取出来补一个 up：不补的话目标键在系统
        // 看来一直按着（常是 Ctrl 这类修饰键）。这里只验证登记表被取走并清空——真的
        // 注入走 simulate，单测不敲真键盘。
        REPLACED_DOWN.lock().unwrap().insert(Key::K, Key::Control);
        assert_eq!(take_replaced_down(), vec![Key::Control]);
        assert!(REPLACED_DOWN.lock().unwrap().is_empty(), "取走后要清空，避免重复释放");
    }

    #[test]
    #[ignore = "诊断用：需人工在有交互会话的机器上跑（锁屏与解锁各跑一次对比）"]
    fn probe_wts_session_state() {
        // 看门狗的「会话解锁」触发路径唯一无法自动化的部分：锁屏/解锁是一次性的人工
        // 状态，测试里造不出来。手动验证方式：解锁状态跑一次应打印 `locked=Some(false)`，
        // 锁屏（Win+L）后跑一次应打印 `locked=Some(true)`；查询失败打印 len=0。
        let mut buf = PWSTR::null();
        let mut len: u32 = 0;
        let query = unsafe {
            WTSQuerySessionInformationW(
                Some(WTS_CURRENT_SERVER_HANDLE),
                WTS_CURRENT_SESSION,
                WTSSessionInfoEx,
                &mut buf,
                &mut len,
            )
        };
        assert!(query.is_ok(), "WTSInfoEx 查询失败：{query:?}");
        assert!(!buf.is_null(), "查询成功但缓冲区为空");
        let info = unsafe { &*buf.0.cast::<WTSINFOEXW>() };
        println!(
            "WTSInfoEx: len={len} Level={} SessionState={:?} SessionFlags={} → locked={:?}",
            info.Level,
            unsafe { info.Data.WTSInfoExLevel1.SessionState },
            unsafe { info.Data.WTSInfoExLevel1.SessionFlags },
            session_locked(),
        );
        unsafe { WTSFreeMemory(buf.0 as *mut c_void) };
    }
}
