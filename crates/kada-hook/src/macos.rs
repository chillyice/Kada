//! macOS 键盘钩子（`CGEventTap`）。
//!
//! 职责与 Windows / Linux 后端对齐：把系统键盘（含可绑定的鼠标中键 / 侧键）事件翻译成
//! [`KeyEvent`]，回调决定 [`Action`]，注入由 [`simulate`] 完成。壳层的 `decide` /
//! `Recorder` / `Engine` 拿到的就是这套接口，三平台共用同一份判定逻辑。
//!
//! 机制要点：
//! - `CGEventTap` 挂在**独立线程的 CFRunLoop** 上（`kCGSessionEventTap` +
//!   `HeadInsertEventTap` + `Default` 选项），处理函数直接跑在回调里、不跨线程调度，
//!   保证顺序与低延迟。**回调里绝不能做耗时操作**：超时会被系统停用（好在 macOS 会
//!   主动回调通知，见「自愈」），之后全部功能静默失效。
//!   **位置必须是会话级（session），不能图「最早看到」去装 HID 级**：`CGEventTapCreate`
//!   在 HID 位置只有 root 建得出来，普通用户（哪怕已授辅助功能权限）拿到的永远是 NULL
//!   ——`CGEvent.h` 写明「Only processes running as the root user may locate an event tap
//!   at the point where HID events enter the window server; for other users, this function
//!   returns NULL」。会话级是辅助功能权限覆盖得到的最前一层，仍早于窗口服务器把事件派发给
//!   应用，吞键 / 改键照常有效。代价是**我们自己注入到 HID 的事件也会顺流经过这个 tap**，
//!   靠下面的「注入识别」放行。
//! - 修饰键状态取**事件自带的 `flags`**（该事件生成那一刻的修饰键快照）而不是全局查询：
//!   少一次队列滞后，且「事件键本身是修饰键」的方向修正天然包含在内。`FlagsChanged`
//!   事件的方向只能从 flags 读——事件里的修饰位是该键**变化之后**的状态。CapsLock 例外
//!   （macOS 不给它发 keyDown/keyUp，见 [`CAPS_DOWN`]）。
//! - 自动重复取 macOS 自己的 `kCGKeyboardEventAutorepeat` 字段，**并**叠加「该键已按下且
//!   未抬起」的本地判定（[`HELD_KEYS`]），两者取或：字段缺失时不能把长按连发当连打，
//!   字段误报时也不能把连打当连发（热串触发词 `addr` 的双写 d、双击/三击改键都靠这个）。
//! - `Block`/`Replace` 登记 [`SWALLOWED`]，后续 keyup 一并吞掉防幽灵；`Replace` 保持
//!   「按下-抬起」配对（按住原键 = 按住目标键）。**所有** keyup（含被吞掉的）都以
//!   观察者身份回调 handler，返回值被忽略——状态机（和弦按住集合 / tap-hold 短长按判定）
//!   依赖抬起事件，漏掉就会卡死。
//! - **注入识别**：我们注入的事件一律从 **Private 事件源**发出，tap 回调靠
//!   `kCGEventSourceStateID == kCGEventSourceStatePrivate` 认出并原样放行（另叠加一条
//!   「创建者就是本进程」的兜底判据，见 [`is_injected`]）。这与 Windows 的 `LLKHF_INJECTED`
//!   是同一件事，用的是 CoreGraphics 里本来就为「这个事件是谁造的」而设的字段；不回环是
//!   硬要求——否则自己注入的 ⌘V 会被自己当成快捷键吞掉。
//! - **自愈**（比 Windows 好办）：tap 回调太慢被系统停用时，系统会**主动**回调
//!   `kCGEventTapDisabledByTimeout` / `kCGEventTapDisabledByUserInput`，当场重新启用并复位
//!   运行时状态；另有 1s 巡检线程兜住「休眠唤醒后收不到事件」与「没有任何通知的静默失效」
//!   （`CGEventSourceSecondsSinceLastEventType` 与自己的心跳对照，等价于 Windows 的
//!   `GetLastInputInfo`）。
//! - **权限**：监听（`CGEventTapCreate`）与注入都需要「辅助功能」权限。未授权时 tap 建不出来，
//!   [`start`] 返回一句能照做的错误并**顺手弹一次系统授权框**（[`request_accessibility`]）。
//!
//! 已知天花板（升级路径）：
//! - **媒体键**（播放 / 上一曲 / 下一曲 / 音量 / 静音）在 macOS 上走 `NX_SYSDEFINED`
//!   系统定义事件，不是虚拟键码，本版本不识别——好处是它们**原样放行**，媒体键功能照常；
//!   代价是绑不了。Apple 键盘的旧式 `0x48/0x49/0x4A`（音量+/-/静音）键码是否真的出现随
//!   机型而异，故一律不映射，免得把别的键错认成音量键。
//! - **F21~F24** 在 macOS 没有对应键码（也没有这种硬件），无法绑定。
//! - **CapsLock** 只发 `FlagsChanged`，方向靠自记的翻转状态（[`CAPS_DOWN`]）；要
//!   「CapsLock 当 Ctrl」这类改键，macOS「系统设置 → 键盘 → 修饰键」本身就支持，更稳。
//! - 与 Windows 同样的天花板：低层 tap 拦不住**提权**进程，也拦不住**安全输入**（Secure
//!   Input，例如终端里输入密码时系统会关掉所有 tap）；驱动级方案见 `docs/需求设计说明书.md`
//!   7.3-㉑。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::runloop::{kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CallbackResult, EventField, KeyCode,
};
use core_graphics::event_source::CGEventSourceStateID;
use core_graphics::geometry::CGPoint;

use kada_core::{FrontmostContext, Key, Modifier};

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
/// 已决定吞掉的键：后续 keyup 也要吞。
static SWALLOWED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Replace 注入后仍按下的宿主原键 → 目标键。
static REPLACED_DOWN: LazyLock<Mutex<HashMap<Key, Key>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// 当前物理按下的键（down 且未 up），用于识别自动重复。
static HELD_KEYS: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// CapsLock 的「按下/抬起」翻转状态。
///
/// macOS 的 CapsLock 从不产生 keyDown/keyUp，只发 `FlagsChanged`，而事件里的 AlphaShift 位
/// 反映的是**开关**状态（按一下开、再按一下关）而不是物理按下/抬起，靠它判方向会把「抬起」
/// 也读成「按下」。所以按「每个事件翻转一次」记录方向：正常一按一放得到一对 down/up。
/// 万一某个键盘对一次按键只发一个事件，则表现为「一按一个 tap」——至少不会卡在按住态
/// （卡住会让 `hold` 形态的改键 / 切层键永远不释放）。
static CAPS_DOWN: AtomicBool = AtomicBool::new(false);
/// tap 句柄的原始指针（`CFMachPortRef`）。0 = tap 未装。
///
/// 只为了两件事：看门狗线程重新启用 tap（`CGEventTapEnable`），以及 tap 被系统停用时
/// 在回调里当场重新启用。CoreGraphics 没有「通过 callback proxy 重新启用」这条路。
static TAP: AtomicI64 = AtomicI64::new(0);
/// 钩子心跳：tap 回调被调用一次就刷新（进程启动后的毫秒数）。tap 被系统停用 / 休眠唤醒后
/// 收不到事件，这个时间戳就随停止前进——这是「tap 还活着」唯一的证据。
static HEARTBEAT_MS: AtomicU64 = AtomicU64::new(0);
/// tap 重新启用累计次数（诊断用；壳层也靠它发现「输入层断过一次线」并复位状态机）。
static REINSTALLS: AtomicU64 = AtomicU64::new(0);
/// 钩子线程的 CFRunLoop：Drop 时用它打断 `CFRunLoopRunInMode`，不等 100ms 轮询周期。
static RUN_LOOP: Mutex<Option<CFRunLoop>> = Mutex::new(None);
/// 进程启动时刻（心跳与看门狗的时间基准）。
static START: LazyLock<Instant> = LazyLock::new(Instant::now);
/// 看门狗巡检间隔。
const WATCHDOG_INTERVAL_MS: u64 = 1_000;
/// 判定「tap 已失效」的宽限：系统记下的最后按键比我们的心跳新这么多毫秒，就说明事件没送到
/// 我们手里。**必须大于巡检间隔**，否则两轮之间的输入可能整段落空、永远不被发现。
const HEARTBEAT_GRACE_MS: u64 = 1_500;
/// 相邻两次巡检的间隔超过这个值 → 机器睡过（`Instant` 把睡眠时间也算进去）。取 10s 是为了
/// 让一次调度延迟不至于被误判成唤醒——误判的代价只是一次无害的重新启用。
const SUSPEND_GAP_MS: u64 = 10_000;

/// 进程启动至今的毫秒数（心跳与看门狗共用同一时间轴）。
fn now_ms() -> u64 {
    START.elapsed().as_millis() as u64
}

/// 钩子句柄。Drop 时停掉看门狗、打断钩子线程的 run loop 并回收。
pub struct HookHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// 看门狗停机信号：Drop 时丢掉它，看门狗从 `recv_timeout` 立刻收到断开并退出
    /// （比等它睡完一个巡检间隔才退出快，不拖慢应用退出）。
    watchdog: Option<JoinHandle<()>>,
    watchdog_stop: Option<mpsc::Sender<()>>,
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 先停看门狗，否则它可能在 tap 已经卸掉之后还去重新启用一个死指针。
        self.watchdog_stop.take();
        if let Some(w) = self.watchdog.take() {
            let _ = w.join();
        }
        // 立刻打断 run loop（否则要等一个 100ms 轮询周期）。
        if let Some(loop_) = RUN_LOOP.lock().unwrap().take() {
            loop_.stop();
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 装上全局键盘与鼠标键 tap（独立线程跑 CFRunLoop），并启动自愈看门狗。
/// 处理函数在钩子线程回调内同步执行。
pub fn start<F>(handler: F) -> io::Result<HookHandle>
where
    F: FnMut(KeyEvent) -> Action + Send + 'static,
{
    // 没有「辅助功能」权限时 `CGEventTapCreate` 必然失败（首次启动就是这个状态）。
    // 先查一次给出能照做的错误，并弹一次系统授权框——否则用户看到的只是「按了没反应」。
    if !accessibility_granted() {
        request_accessibility();
        return Err(io::Error::other(format!(
            "未获得「辅助功能」权限，输入层无法启动：{}",
            ACCESSIBILITY_HINT
        )));
    }

    *HANDLER.lock().unwrap() = Some(Box::new(handler));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("kada-hook".into())
        .spawn(move || {
            if let Err(e) = tap_loop(&ready_tx, &stop2) {
                eprintln!("kada-hook: {e}");
            }
        })?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = join.join();
            *HANDLER.lock().unwrap() = None;
            return Err(io::Error::other(e));
        }
        Err(_) => return Err(io::Error::other("钩子线程启动失败")),
    }

    // 看门狗只负责「发现失效」并重新启用 tap，真正的 tap 生命周期归钩子线程。
    let (watchdog_stop, watchdog_rx) = mpsc::channel::<()>();
    let watchdog = thread::Builder::new()
        .name("kada-hook-watchdog".into())
        .spawn(move || watchdog_loop(watchdog_rx))?;
    Ok(HookHandle { stop, join: Some(join), watchdog: Some(watchdog), watchdog_stop: Some(watchdog_stop) })
}

/// 钩子线程主体：建 tap → 挂到本线程的 run loop → 循环跑到停机信号。
fn tap_loop(ready: &mpsc::Sender<Result<(), String>>, stop: &AtomicBool) -> io::Result<()> {
    let tap = match CGEventTap::new(
        // 会话级而不是 HID 级：HID 位置的 tap 只有 root 建得出来（见模块头）。
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        events_of_interest(),
        tap_callback,
    ) {
        Ok(t) => t,
        Err(()) => {
            let msg = format!("CGEventTapCreate 失败（{}）", ACCESSIBILITY_HINT);
            let _ = ready.send(Err(msg.clone()));
            return Err(io::Error::other(msg));
        }
    };

    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(s) => s,
        Err(()) => {
            let msg = "创建 run loop source 失败".to_string();
            let _ = ready.send(Err(msg.clone()));
            return Err(io::Error::other(msg));
        }
    };
    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    TAP.store(tap.mach_port().as_concrete_TypeRef() as i64, Ordering::Relaxed);
    HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    *RUN_LOOP.lock().unwrap() = Some(run_loop.clone());
    let _ = ready.send(Ok(()));

    // run loop 里除了这个 tap 没有别的 source，没人处理事件时会按时返回 TimedOut，
    // 借此轮询停机位（`HookHandle::drop` 还会 `CFRunLoopStop` 立刻打断，双保险）。
    while !stop.load(Ordering::Relaxed) {
        let _ = CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, Duration::from_millis(100), false);
    }

    TAP.store(0, Ordering::Relaxed);
    *RUN_LOOP.lock().unwrap() = None;
    run_loop.remove_source(&source, unsafe { kCFRunLoopCommonModes });
    *HANDLER.lock().unwrap() = None;
    Ok(())
}

/// tap 要看的几类事件：键盘按下/抬起、修饰键变化、鼠标「其它键」（中键 / 侧键）。
///
/// 有意不看鼠标左/右键与滚轮、移动：那些事件量大，且不属于键模型（拦它们只会让点击变哑）。
fn events_of_interest() -> Vec<CGEventType> {
    vec![
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
    ]
}

/// tap 回调：一切判定的入口，跑在钩子线程上。**这里只准做判定，不准做耗时操作。**
fn tap_callback(_proxy: CGEventTapProxy, etype: CGEventType, event: &CGEvent) -> CallbackResult {
    // 心跳：tap 被系统停用后就再也不会回调，看门狗靠这个时间戳判断它还活着。
    // 注入事件同样刷新心跳（我们确实收到了事件），否则「只注入、不敲键盘」的时段
    // 会被看门狗误判成失效——那会白复位一次输入状态。
    HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);

    // 系统把 tap 停用了：回调太慢（`kCGEventTapDisabledByTimeout`）或输入过载。
    // 这是 macOS 主动给的自愈信号，当场重新启用并复位运行时状态。
    match etype {
        CGEventType::TapDisabledByTimeout => {
            reinstall_tap("回调超时被系统停用");
            return CallbackResult::Keep;
        }
        CGEventType::TapDisabledByUserInput => {
            reinstall_tap("系统因输入过载停用");
            return CallbackResult::Keep;
        }
        _ => {}
    }

    // 自己注入的事件一律放行（防回环）。必须放在停用判定之后：那两个特殊事件的
    // 类型位上带的是「最后一个事件」，可能正是我们注入的那个。
    //
    // 也必须放在**任何锁之前**：这条判定只读事件字段、不碰 `SWALLOWED` 等登记表，而
    // `Replace` 是在持有 `REPLACED_DOWN` 的情况下注入的——万一注入的事件同步回到本回调，
    // 早退的这条路不取任何锁，也就不会自锁。
    if is_injected(event) {
        return CallbackResult::Keep;
    }

    match etype {
        CGEventType::KeyDown => handle_key(event, true),
        CGEventType::KeyUp => handle_key(event, false),
        CGEventType::FlagsChanged => handle_flags_changed(event),
        CGEventType::OtherMouseDown => handle_mouse(event, true),
        CGEventType::OtherMouseUp => handle_mouse(event, false),
        _ => CallbackResult::Keep,
    }
}

/// 键盘按下 / 抬起。
fn handle_key(event: &CGEvent, down: bool) -> CallbackResult {
    let Some(key) = code_to_key(keycode_of(event)) else {
        // 未知键码（媒体键、Fn 等）：原样放行，保持这些键的原生功能。
        return CallbackResult::Keep;
    };
    let mods = flags_to_mods(event.get_flags());
    // 自动重复：macOS 自己的字段 + 「仍按住」的本地判定（取或，理由见模块头）。
    let repeat = down
        && (event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0);
    dispatch(key, mods, down, repeat)
}

/// 修饰键（与 CapsLock）状态变化。
fn handle_flags_changed(event: &CGEvent) -> CallbackResult {
    let Some(key) = code_to_key(keycode_of(event)) else {
        return CallbackResult::Keep;
    };
    let flags = event.get_flags();
    // CapsLock：只有 FlagsChanged，方向靠自记的翻转状态（见 [`CAPS_DOWN`]）。
    if key == Key::CapsLock {
        let down = CAPS_DOWN.fetch_xor(true, Ordering::Relaxed);
        return dispatch(key, flags_to_mods(flags), down, false);
    }
    let Some(m) = key_as_modifier(key) else {
        // 不是修饰键也不是 CapsLock 的 FlagsChanged（不该出现）：放行。
        return CallbackResult::Keep;
    };
    // 方向：事件里的修饰位是该键**变化之后**的状态。左右同名修饰键（左/右 ⌘ 等）在
    // 键模型里合并成一个 `Key`，于是第二个同族修饰键按下会被读成重复按下、抬起会被读成
    // 又一次按下——与 Windows 的 `GetKeyState` + `HELD_KEYS` 同一处已知不对称。
    let down = flags.contains(modifier_flag(m));
    dispatch(key, flags_to_mods(flags), down, false)
}

/// 鼠标「其它键」（中键 / 侧键 MB4 / MB5）。只处理这三个键：左/右键与滚轮不在事件
/// 掩码里，连回调都不会进。
fn handle_mouse(event: &CGEvent, down: bool) -> CallbackResult {
    let button = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);
    let Some(key) = mouse_button_to_key(button) else {
        return CallbackResult::Keep;
    };
    let mods = flags_to_mods(event.get_flags());
    dispatch(key, mods, down, false)
}

/// 事件的键盘虚拟键码。
fn keycode_of(event: &CGEvent) -> u16 {
    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16
}

/// 事件是不是我们自己注入的。
///
/// 判据见 [`looks_injected`]；这里只负责把两条字段读出来。
fn is_injected(event: &CGEvent) -> bool {
    looks_injected(
        event.get_integer_value_field(EventField::EVENT_SOURCE_STATE_ID),
        event.get_integer_value_field(EventField::EVENT_SOURCE_UNIX_PROCESS_ID),
        i64::from(std::process::id()),
    )
}

/// 注入识别本体（纯函数，两条判据任一成立即算自己的事件）：
///
/// 1. **事件源状态 id 是 Private**——我们注入的事件都从 [`CGEventSourceStateID::Private`]
///    事件源发出，硬件事件来自 HID / 组合会话状态。这是 CoreGraphics 里本来就为「这个事件
///    是谁造的」而设的字段，语义与 Windows 的 `LLKHF_INJECTED` 等价。
/// 2. **创建事件的进程就是本进程**（`kCGEventSourceUnixProcessID`）——硬件事件由窗口服务器
///    创建，其它应用注入的事件带的是它们的 pid，都不可能等于我们。
///
/// 为什么留两条而不是只认 Private：判据 1 依赖「Private 源的事件读出来就带 Private 这个
/// 标记」这条系统行为，而本仓库没有 macOS 开发机、**这条行为没有任何办法在开发机上验**。
/// 一旦它不成立，放行判据失效的后果是自己注入的 ⌘V 被自己当成按键处理（用户恰好绑了 ⌘V
/// 就是无限触发）。多一条只多读一个整数、又不可能误伤硬件事件（pid 不会等于我们）的判据，
/// 成本几乎为零，却把「只有一条判据且它悄悄地不成立」这种失效模式堵掉了。
fn looks_injected(state_id: i64, creator_pid: i64, self_pid: i64) -> bool {
    state_id == CGEventSourceStateID::Private as i64 || creator_pid == self_pid
}

/// 把一个按下/抬起决定派给 handler 并落实处置。键盘与鼠标键共用。
fn dispatch(key: Key, mods: BTreeSet<Modifier>, down: bool, repeat: bool) -> CallbackResult {
    if !down {
        note_key_up(key);
        let swallowed = SWALLOWED.lock().unwrap().remove(&key);
        if swallowed {
            if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
                simulate::up(target);
            }
        }
        // 无论吞掉与否都回调 handler（观察者）：tap-hold 需要在 keyup 时判定 tap/hold、
        // 和弦靠抬起维护成员集合，漏掉被吞键的抬起会让该键与整个状态机永久卡住。
        if let Some(f) = HANDLER.lock().unwrap().as_mut() {
            let _ = f(KeyEvent::Up { key, mods });
        }
        return if swallowed { CallbackResult::Drop } else { CallbackResult::Keep };
    }

    let repeat = repeat || !HELD_KEYS.lock().unwrap().insert(key);
    let action = {
        let mut h = HANDLER.lock().unwrap();
        match h.as_mut() {
            Some(f) => f(KeyEvent::Down { key, mods, repeat }),
            None => Action::Allow,
        }
    };

    match action {
        Action::Allow => CallbackResult::Keep,
        Action::Block => {
            SWALLOWED.lock().unwrap().insert(key);
            CallbackResult::Drop
        }
        Action::Replace(target) => {
            SWALLOWED.lock().unwrap().insert(key);
            if !repeat {
                // 按住原键 = 按住目标键：只在首个按下注入一次，抬起时配对释放。
                // 自动重复不再注入（否则目标键会被连发）。
                let mut r = REPLACED_DOWN.lock().unwrap();
                if !r.contains_key(&key) {
                    r.insert(key, target);
                    simulate::down(target);
                }
            }
            CallbackResult::Drop
        }
    }
}

/// 键抬起：移出「按下集合」。无论事件最终吞掉与否都必须调用，否则集合只增不减，
/// 后续所有该键的按下都会被误判为自动重复。
fn note_key_up(key: Key) {
    HELD_KEYS.lock().unwrap().remove(&key);
}

/// 重新启用 tap（看门狗 / 系统停用通知 / 人工兜底都用这一条路）。返回 false = tap 没装着。
///
/// 重新启用前必须**复位运行时状态**：断线期间那些物理键的抬起已经不可能再经过我们，
/// 「按住集合」里的键会永远等不到抬起（后续每次按下都被当成自动重复 → 该键彻底变哑），
/// 被替换的键也永远收不到配对的 up（目标键在系统看来一直按着）。
fn reinstall_tap(reason: &str) -> bool {
    let tap = TAP.load(Ordering::Relaxed);
    if tap == 0 {
        return false;
    }
    reset_runtime_state();
    unsafe { CGEventTapEnable(tap as *mut c_void, true) };
    // 心跳从这一刻重新起算：否则「最后一次 tap 事件」还停在很久以前，看门狗下一轮就会把刚
    // 重新启用的 tap 再判成失效（与 Windows 重装钩子时刷新心跳同理）。
    HEARTBEAT_MS.store(now_ms(), Ordering::Relaxed);
    REINSTALLS.fetch_add(1, Ordering::Relaxed);
    eprintln!("kada-hook: 重建输入 tap（{reason}）");
    true
}

/// 复位 tap 层运行时状态（重新启用 / 卸下时调用）。
fn reset_runtime_state() {
    // 先把「已注入且被认为按着」的目标键补一个 up，再清登记表。
    for target in take_replaced_down() {
        simulate::up(target);
    }
    SWALLOWED.lock().unwrap().clear();
    HELD_KEYS.lock().unwrap().clear();
    CAPS_DOWN.store(false, Ordering::Relaxed);
}

/// 取走所有待释放的替换目标键（宿主原键 → 已注入并按住的键），并清空登记表。
fn take_replaced_down() -> Vec<Key> {
    REPLACED_DOWN.lock().unwrap().drain().map(|(_, target)| target).collect()
}

/// 自愈重装累计次数（诊断 / 壳层「输入状态复位」用）。
pub fn reinstall_count() -> u64 {
    REINSTALLS.load(Ordering::Relaxed)
}

/// 看门狗线程：盯住「tap 还活着」的证据，失效时重新启用。
///
/// 系统主动停用那两条路由回调自己处理（[`tap_callback`]），这里管的是**没有任何通知**的
/// 两种失效：休眠唤醒后收不到事件、以及说不清原因的静默失效。两条判据：
/// 1. **心跳落后**——系统记下的最后一次硬件按键比我们的心跳新
///    （[`CGEventSourceSecondsSinceLastEventType`] 之于 [`HEARTBEAT_MS`]，等价于 Windows 的
///    `GetLastInputInfo` 之于钩子心跳）。只看**先后**不看绝对时间：闲置时两边一起停在原处，
///    只有「系统收到了输入而我们的心跳还落在后面」才说明事件没送到。
/// 2. **巡检间隔突然变大**——`Instant` 把睡眠时间算在内，说明机器睡过。
fn watchdog_loop(stop: mpsc::Receiver<()>) {
    let mut prev = now_ms();
    // 连续触发次数：一次成功的重新启用会让触发条件下一轮自然消失（心跳被刷新、gap 归零），
    // 所以「下一轮又触发」就等于上次没修好 → 指数退避，别每秒重装。
    let mut streak: u32 = 0;
    loop {
        let wait = WATCHDOG_INTERVAL_MS << streak.min(4); // 1s → 最多 16s
        match stop.recv_timeout(Duration::from_millis(wait)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = now_ms();
        let gap = now.saturating_sub(prev);
        prev = now;

        let reason = if tap_dead(now) {
            Some("收不到事件（疑似被系统停用或休眠唤醒后失效）")
        } else if gap > SUSPEND_GAP_MS {
            Some("休眠唤醒")
        } else {
            None
        };

        let Some(reason) = reason else {
            streak = 0;
            continue;
        };
        streak = streak.saturating_add(1);
        if !reinstall_tap(reason) {
            return; // tap 没了，看门狗没有意义
        }
        // 自愈是「无声兜底」（用户此刻没有动作可做），只在 stderr 留痕 + 累加计数，
        // 不进消息中心。
        if streak > 1 {
            eprintln!("kada-hook: 看门狗连续第 {streak} 次重建输入 tap");
        }
    }
}

/// tap 是不是已经收不到事件了（判据见 [`watchdog_loop`]）。
///
/// 只看键盘事件（[`CGEventType::KeyDown`]）：`CGEventSourceSecondsSinceLastEventType` 一次
/// 只能问一类事件，而我们的 tap 收到的正好是键盘 + 鼠标中/侧键。若把鼠标左键也算进「系统侧
/// 的信号」，用户在纯鼠标操作时段就会被误判成失效（每次点击都触发一次重建）。
fn tap_dead(now: u64) -> bool {
    let since_system =
        unsafe { CGEventSourceSecondsSinceLastEventType(CGEventSourceStateID::HIDSystemState, CGEventType::KeyDown) };
    if (since_system * 1000.0) as u64 > HEARTBEAT_GRACE_MS {
        return false; // 系统侧也没有新输入：闲置不是失效
    }
    now.saturating_sub(HEARTBEAT_MS.load(Ordering::Relaxed)) > HEARTBEAT_GRACE_MS
}

/// 通用修饰键 → 事件 flags 里对应的位。
fn modifier_flag(m: Modifier) -> CGEventFlags {
    match m {
        Modifier::Shift => CGEventFlags::CGEventFlagShift,
        Modifier::Ctrl => CGEventFlags::CGEventFlagControl,
        Modifier::Alt => CGEventFlags::CGEventFlagAlternate,
        Modifier::Meta => CGEventFlags::CGEventFlagCommand,
    }
}

/// 事件 flags → 修饰键集合。
///
/// 只认四个修饰键位；AlphaShift（CapsLock 开关状态）、NumericPad、SecondaryFn 不是
/// 键模型里的修饰键，不能混进来——否则按着 CapsLock 打的每个键都会被当成「带修饰」，
/// 匹配不到任何快捷键。
fn flags_to_mods(flags: CGEventFlags) -> BTreeSet<Modifier> {
    let mut mods = BTreeSet::new();
    for m in [Modifier::Ctrl, Modifier::Alt, Modifier::Shift, Modifier::Meta] {
        if flags.contains(modifier_flag(m)) {
            mods.insert(m);
        }
    }
    mods
}

/// [`Key`] 是修饰键时返回对应修饰位（键模型里左右同名键合并）。
fn key_as_modifier(k: Key) -> Option<Modifier> {
    match k {
        Key::Shift => Some(Modifier::Shift),
        Key::Control => Some(Modifier::Ctrl),
        Key::Alt => Some(Modifier::Alt),
        Key::Meta => Some(Modifier::Meta),
        _ => None,
    }
}

/// 鼠标「其它键」按钮号 → [`Key`]（2 = 中键，3 = 后退/MB4，4 = 前进/MB5）。
///
/// macOS 给非左/右/中键统一发 `OtherMouse*`，按钮号在 `kCGMouseEventButtonNumber` 里；
/// 3/4 这两个编号是鼠标驱动约定俗成的「后退 / 前进」。
fn mouse_button_to_key(button: i64) -> Option<Key> {
    match button {
        2 => Some(Key::MouseMiddle),
        3 => Some(Key::MouseBack),
        4 => Some(Key::MouseForward),
        _ => None,
    }
}

/// [`Key`] → `OtherMouse*` 的按钮号。
fn key_to_mouse_button(k: Key) -> Option<u32> {
    match k {
        Key::MouseMiddle => Some(2),
        Key::MouseBack => Some(3),
        Key::MouseForward => Some(4),
        _ => None,
    }
}

/// [`Key`] → macOS 虚拟键码（ANSI 位置语义）。无法映射的返回 None。
///
/// 用位置键码（`ANSI_*`）而不是字符：macOS 的键码本来就标识**物理位置**，字符由输入法
/// 按布局翻译，所以 Dvorak 用户看到的字符顺序不同但键码与我们的模型一致。
pub fn key_to_code(k: Key) -> Option<u16> {
    use Key::*;
    let code = match k {
        A => KeyCode::ANSI_A, B => KeyCode::ANSI_B, C => KeyCode::ANSI_C,
        D => KeyCode::ANSI_D, E => KeyCode::ANSI_E, F => KeyCode::ANSI_F,
        G => KeyCode::ANSI_G, H => KeyCode::ANSI_H, I => KeyCode::ANSI_I,
        J => KeyCode::ANSI_J, K => KeyCode::ANSI_K, L => KeyCode::ANSI_L,
        M => KeyCode::ANSI_M, N => KeyCode::ANSI_N, O => KeyCode::ANSI_O,
        P => KeyCode::ANSI_P, Q => KeyCode::ANSI_Q, R => KeyCode::ANSI_R,
        S => KeyCode::ANSI_S, T => KeyCode::ANSI_T, U => KeyCode::ANSI_U,
        V => KeyCode::ANSI_V, W => KeyCode::ANSI_W, X => KeyCode::ANSI_X,
        Y => KeyCode::ANSI_Y, Z => KeyCode::ANSI_Z,
        Digit0 => KeyCode::ANSI_0, Digit1 => KeyCode::ANSI_1, Digit2 => KeyCode::ANSI_2,
        Digit3 => KeyCode::ANSI_3, Digit4 => KeyCode::ANSI_4, Digit5 => KeyCode::ANSI_5,
        Digit6 => KeyCode::ANSI_6, Digit7 => KeyCode::ANSI_7, Digit8 => KeyCode::ANSI_8,
        Digit9 => KeyCode::ANSI_9,
        F1 => KeyCode::F1, F2 => KeyCode::F2, F3 => KeyCode::F3, F4 => KeyCode::F4,
        F5 => KeyCode::F5, F6 => KeyCode::F6, F7 => KeyCode::F7, F8 => KeyCode::F8,
        F9 => KeyCode::F9, F10 => KeyCode::F10, F11 => KeyCode::F11, F12 => KeyCode::F12,
        F13 => KeyCode::F13, F14 => KeyCode::F14, F15 => KeyCode::F15, F16 => KeyCode::F16,
        F17 => KeyCode::F17, F18 => KeyCode::F18, F19 => KeyCode::F19, F20 => KeyCode::F20,
        Comma => KeyCode::ANSI_COMMA, Period => KeyCode::ANSI_PERIOD,
        Slash => KeyCode::ANSI_SLASH, Backslash => KeyCode::ANSI_BACKSLASH,
        Semicolon => KeyCode::ANSI_SEMICOLON, Quote => KeyCode::ANSI_QUOTE,
        Backquote => KeyCode::ANSI_GRAVE, Minus => KeyCode::ANSI_MINUS,
        Equal => KeyCode::ANSI_EQUAL, BracketLeft => KeyCode::ANSI_LEFT_BRACKET,
        BracketRight => KeyCode::ANSI_RIGHT_BRACKET,
        Enter => KeyCode::RETURN, Escape => KeyCode::ESCAPE, Tab => KeyCode::TAB,
        Space => KeyCode::SPACE,
        // macOS 上退格与「向前删除」是两个键码（Windows 那边 `Delete` 才是向前删除）。
        Backspace => KeyCode::DELETE, Delete => KeyCode::FORWARD_DELETE,
        // macOS 没有 Insert 键：PC 键盘的 Insert 由 Help 键位（0x72）承载，故两者互映。
        Insert => KeyCode::HELP,
        CapsLock => KeyCode::CAPS_LOCK,
        Shift => KeyCode::SHIFT, Control => KeyCode::CONTROL, Alt => KeyCode::OPTION,
        Meta => KeyCode::COMMAND,
        Home => KeyCode::HOME, End => KeyCode::END,
        PageUp => KeyCode::PAGE_UP, PageDown => KeyCode::PAGE_DOWN,
        ArrowUp => KeyCode::UP_ARROW, ArrowDown => KeyCode::DOWN_ARROW,
        ArrowLeft => KeyCode::LEFT_ARROW, ArrowRight => KeyCode::RIGHT_ARROW,
        // macOS 没有 NumLock 键；小键盘 Clear（0x47）是它唯一对应的键位。
        NumLock => KeyCode::ANSI_KEYPAD_CLEAR,
        Numpad0 => KeyCode::ANSI_KEYPAD_0, Numpad1 => KeyCode::ANSI_KEYPAD_1,
        Numpad2 => KeyCode::ANSI_KEYPAD_2, Numpad3 => KeyCode::ANSI_KEYPAD_3,
        Numpad4 => KeyCode::ANSI_KEYPAD_4, Numpad5 => KeyCode::ANSI_KEYPAD_5,
        Numpad6 => KeyCode::ANSI_KEYPAD_6, Numpad7 => KeyCode::ANSI_KEYPAD_7,
        Numpad8 => KeyCode::ANSI_KEYPAD_8, Numpad9 => KeyCode::ANSI_KEYPAD_9,
        NumpadAdd => KeyCode::ANSI_KEYPAD_PLUS, NumpadSubtract => KeyCode::ANSI_KEYPAD_MINUS,
        NumpadMultiply => KeyCode::ANSI_KEYPAD_MULTIPLY,
        NumpadDivide => KeyCode::ANSI_KEYPAD_DIVIDE,
        NumpadDecimal => KeyCode::ANSI_KEYPAD_DECIMAL, NumpadEnter => KeyCode::ANSI_KEYPAD_ENTER,
        // 媒体键（NX_SYSDEFINED，非虚拟键码）、F21~F24（无此硬件）：见模块头「已知天花板」。
        MediaPlayPause | MediaPrev | MediaNext | VolumeMute | VolumeDown | VolumeUp => return None,
        F21 | F22 | F23 | F24 => return None,
        // 鼠标键不是键码，注入走 [`simulate`] 的鼠标事件。
        MouseMiddle | MouseBack | MouseForward => return None,
    };
    Some(code)
}

/// macOS 虚拟键码 → [`Key`]（左右同名修饰键合并）。
fn code_to_key(code: u16) -> Option<Key> {
    use Key::*;
    Some(match code {
        c if c == KeyCode::SHIFT || c == KeyCode::RIGHT_SHIFT => Shift,
        c if c == KeyCode::CONTROL || c == KeyCode::RIGHT_CONTROL => Control,
        c if c == KeyCode::OPTION || c == KeyCode::RIGHT_OPTION => Alt,
        c if c == KeyCode::COMMAND || c == KeyCode::RIGHT_COMMAND => Meta,
        c if c == KeyCode::ANSI_A => A, c if c == KeyCode::ANSI_B => B,
        c if c == KeyCode::ANSI_C => C, c if c == KeyCode::ANSI_D => D,
        c if c == KeyCode::ANSI_E => E, c if c == KeyCode::ANSI_F => F,
        c if c == KeyCode::ANSI_G => G, c if c == KeyCode::ANSI_H => H,
        c if c == KeyCode::ANSI_I => I, c if c == KeyCode::ANSI_J => J,
        c if c == KeyCode::ANSI_K => K, c if c == KeyCode::ANSI_L => L,
        c if c == KeyCode::ANSI_M => M, c if c == KeyCode::ANSI_N => N,
        c if c == KeyCode::ANSI_O => O, c if c == KeyCode::ANSI_P => P,
        c if c == KeyCode::ANSI_Q => Q, c if c == KeyCode::ANSI_R => R,
        c if c == KeyCode::ANSI_S => S, c if c == KeyCode::ANSI_T => T,
        c if c == KeyCode::ANSI_U => U, c if c == KeyCode::ANSI_V => V,
        c if c == KeyCode::ANSI_W => W, c if c == KeyCode::ANSI_X => X,
        c if c == KeyCode::ANSI_Y => Y, c if c == KeyCode::ANSI_Z => Z,
        c if c == KeyCode::ANSI_0 => Digit0, c if c == KeyCode::ANSI_1 => Digit1,
        c if c == KeyCode::ANSI_2 => Digit2, c if c == KeyCode::ANSI_3 => Digit3,
        c if c == KeyCode::ANSI_4 => Digit4, c if c == KeyCode::ANSI_5 => Digit5,
        c if c == KeyCode::ANSI_6 => Digit6, c if c == KeyCode::ANSI_7 => Digit7,
        c if c == KeyCode::ANSI_8 => Digit8, c if c == KeyCode::ANSI_9 => Digit9,
        c if c == KeyCode::F1 => F1, c if c == KeyCode::F2 => F2,
        c if c == KeyCode::F3 => F3, c if c == KeyCode::F4 => F4,
        c if c == KeyCode::F5 => F5, c if c == KeyCode::F6 => F6,
        c if c == KeyCode::F7 => F7, c if c == KeyCode::F8 => F8,
        c if c == KeyCode::F9 => F9, c if c == KeyCode::F10 => F10,
        c if c == KeyCode::F11 => F11, c if c == KeyCode::F12 => F12,
        c if c == KeyCode::F13 => F13, c if c == KeyCode::F14 => F14,
        c if c == KeyCode::F15 => F15, c if c == KeyCode::F16 => F16,
        c if c == KeyCode::F17 => F17, c if c == KeyCode::F18 => F18,
        c if c == KeyCode::F19 => F19, c if c == KeyCode::F20 => F20,
        c if c == KeyCode::ANSI_COMMA => Comma, c if c == KeyCode::ANSI_PERIOD => Period,
        c if c == KeyCode::ANSI_SLASH => Slash,
        c if c == KeyCode::ANSI_BACKSLASH => Backslash,
        c if c == KeyCode::ANSI_SEMICOLON => Semicolon,
        c if c == KeyCode::ANSI_QUOTE => Quote,
        c if c == KeyCode::ANSI_GRAVE => Backquote,
        c if c == KeyCode::ANSI_MINUS => Minus,
        c if c == KeyCode::ANSI_EQUAL => Equal,
        c if c == KeyCode::ANSI_LEFT_BRACKET => BracketLeft,
        c if c == KeyCode::ANSI_RIGHT_BRACKET => BracketRight,
        c if c == KeyCode::RETURN => Enter, c if c == KeyCode::ESCAPE => Escape,
        c if c == KeyCode::TAB => Tab, c if c == KeyCode::SPACE => Space,
        c if c == KeyCode::DELETE => Backspace,
        c if c == KeyCode::FORWARD_DELETE => Delete,
        c if c == KeyCode::HELP => Insert,
        c if c == KeyCode::CAPS_LOCK => CapsLock,
        c if c == KeyCode::HOME => Home, c if c == KeyCode::END => End,
        c if c == KeyCode::PAGE_UP => PageUp, c if c == KeyCode::PAGE_DOWN => PageDown,
        c if c == KeyCode::UP_ARROW => ArrowUp, c if c == KeyCode::DOWN_ARROW => ArrowDown,
        c if c == KeyCode::LEFT_ARROW => ArrowLeft,
        c if c == KeyCode::RIGHT_ARROW => ArrowRight,
        c if c == KeyCode::ANSI_KEYPAD_CLEAR => NumLock,
        c if c == KeyCode::ANSI_KEYPAD_0 => Numpad0,
        c if c == KeyCode::ANSI_KEYPAD_1 => Numpad1,
        c if c == KeyCode::ANSI_KEYPAD_2 => Numpad2,
        c if c == KeyCode::ANSI_KEYPAD_3 => Numpad3,
        c if c == KeyCode::ANSI_KEYPAD_4 => Numpad4,
        c if c == KeyCode::ANSI_KEYPAD_5 => Numpad5,
        c if c == KeyCode::ANSI_KEYPAD_6 => Numpad6,
        c if c == KeyCode::ANSI_KEYPAD_7 => Numpad7,
        c if c == KeyCode::ANSI_KEYPAD_8 => Numpad8,
        c if c == KeyCode::ANSI_KEYPAD_9 => Numpad9,
        c if c == KeyCode::ANSI_KEYPAD_PLUS => NumpadAdd,
        c if c == KeyCode::ANSI_KEYPAD_MINUS => NumpadSubtract,
        c if c == KeyCode::ANSI_KEYPAD_MULTIPLY => NumpadMultiply,
        c if c == KeyCode::ANSI_KEYPAD_DIVIDE => NumpadDivide,
        c if c == KeyCode::ANSI_KEYPAD_DECIMAL => NumpadDecimal,
        c if c == KeyCode::ANSI_KEYPAD_ENTER => NumpadEnter,
        _ => return None,
    })
}

/// 前台应用进程 id 的查询与窗口标题的读取。
///
/// 走辅助功能接口（属性名 `AXFocusedApplication`）：这是 macOS 上唯一不需要额外依赖、
/// 也不需要「屏幕录制」权限就能拿到前台应用的途径（`CGWindowList` 拿窗口**标题**要屏幕录制
/// 权限，比辅助功能权限更难拿）。**粒度是应用不是窗口**——壳层用它做「前台切换 → 复位输入
/// 状态」的变化检测，同应用内换窗口不会触发复位（风险小得多：被吞的键仍落在同一个进程里）。
fn frontmost_pid() -> Option<i32> {
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }
        // AX 查询默认能等 6 秒（前台应用无响应时），而轮询路径每 60ms 调一次，必须限时。
        AXUIElementSetMessagingTimeout(system_wide, AX_MESSAGING_TIMEOUT_S);
        let attr = CFString::new(AX_FOCUSED_APPLICATION_ATTR);
        let mut app: CFTypeRef = ptr::null();
        let err = AXUIElementCopyAttributeValue(system_wide, attr.as_concrete_TypeRef(), &mut app);
        CFRelease(system_wide);
        if err != 0 || app.is_null() {
            return None;
        }
        let mut pid: i32 = 0;
        let ok = AXUIElementGetPid(app as AXUIElementRef, &mut pid) == 0;
        CFRelease(app);
        (ok && pid > 0).then_some(pid)
    }
}

/// 前台窗口标题（`None` = 取不到：应用不支持辅助功能 / 没有焦点窗口 / 超时）。
fn focused_window_title(pid: i32) -> Option<String> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, AX_MESSAGING_TIMEOUT_S);
        let mut window: CFTypeRef = ptr::null();
        let attr = CFString::new(AX_FOCUSED_WINDOW_ATTR);
        let err = AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut window);
        CFRelease(app);
        if err != 0 || window.is_null() {
            return None;
        }
        let mut title: CFTypeRef = ptr::null();
        let attr = CFString::new(AX_TITLE_ATTR);
        let err = AXUIElementCopyAttributeValue(
            window as AXUIElementRef,
            attr.as_concrete_TypeRef(),
            &mut title,
        );
        CFRelease(window);
        if err != 0 {
            return None;
        }
        ax_string(title)
    }
}

/// 由进程 id 取可执行文件名（如 `Google Chrome`）。失败返回 None。
///
/// 用 `proc_pidpath`（libSystem）：`NSRunningApplication` 那条路要 ObjC 运行时依赖，
/// 只为拿一个名字不值得。**注意与 Windows 的差异**：那边给的是 `chrome.exe` 这样的镜像名，
/// 这里是可执行文件名（没有扩展名）——从 Windows 同步过去的「前台应用」条件在 macOS 上要改。
fn process_name_of(pid: i32) -> Option<String> {
    let mut buf = [0u8; 1024];
    let len = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if len <= 0 {
        return None;
    }
    let path = String::from_utf8_lossy(&buf[..len as usize]).into_owned();
    path.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_string)
}

/// 取当前前台应用上下文（进程名 + 焦点窗口标题），供「按前台应用/窗口」类条件求值。
///
/// 与 Windows 版同接口；取不到窗口标题时返回空串（条件按「不匹配」处理），取不到前台应用
/// 才返回 None。这条路径只在**求值条件**时走（不是每次按键），可以承担两次 AX 查询。
pub fn frontmost_context() -> Option<FrontmostContext> {
    let pid = frontmost_pid()?;
    Some(FrontmostContext {
        process_name: process_name_of(pid).unwrap_or_default(),
        window_title: focused_window_title(pid).unwrap_or_default(),
    })
}

/// 当前前台应用的标识（壳层做「前台切换 → 复位输入状态」的变化检测）。
///
/// **本进程自己成为前台时报 `None`**（同 Windows 版口径）：主窗 / 气泡 / 状态指示 / 提示框
/// 都是我们自己弹的，不是用户换应用；被 `FocusTracker` 当成切换会复位输入状态，踢掉按住的
/// momentary 层、丢弃凑到一半的和弦（锁定层不受影响——正是「hold_layer 触发和弦失败、
/// lock_layer 正常」的来源）。
pub fn foreground_window() -> Option<isize> {
    let pid = frontmost_pid()?;
    if pid == std::process::id() as i32 {
        return None;
    }
    Some(pid as isize)
}

/// 最近一次事件的设备标识。**macOS 恒 `None`**：`CGEventTap` 事件不带设备信息
/// （真实区分多键盘要接 IOHIDManager，见规划 7.3-㉓ 的后续）。壳层据此把「设备是」
/// 条件在本平台标注为不可用。
pub fn current_device() -> Option<String> {
    None
}

/// 「辅助功能」权限是否已授予（CGEventTap 监听与注入的前置条件）。
pub fn accessibility_granted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// 给用户看的授权指引（错误文案与告警共用一句话，避免两处说法不一致）。
pub const ACCESSIBILITY_HINT: &str =
    "系统设置 → 隐私与安全性 → 辅助功能 → 勾选「Kada」（改完要重启 Kada 才生效）";

/// 弹一次系统的授权引导框（点「打开系统设置」直达上条那个面板）。
///
/// 已授权时调用无害；用户之前拒绝过也不会反复骚扰（系统自己记着）。只在 [`start`]
/// 发现未授权时调一次。
pub fn request_accessibility() {
    unsafe {
        let key = CFString::new(AX_TRUSTED_CHECK_OPTION_PROMPT);
        let options = CFDictionary::from_CFType_pairs(&[(
            key.as_CFType(),
            CFBoolean::true_value().as_CFType(),
        )]);
        let _ = AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef() as *const c_void);
    }
}

// AX 查询的消息超时（秒）：默认 6 秒太久（前台应用卡住时能把调用方拖住 6 秒）。
// 只对「发消息给别的进程」的查询有效——轮询路径查的是系统级元素，不涉及其它进程，
// 不受这个值影响；而求值「前台窗口标题」时若目标应用忙，宁可在 150ms 后放弃（条件按
// 不匹配处理），也不要卡住整条动作链。
const AX_MESSAGING_TIMEOUT_S: f32 = 0.15;

/// 辅助功能元素句柄（`AXUIElementRef`）。
type AXUIElementRef = *mut c_void;

// CoreGraphics 里本模块用到的、core-graphics 没有包出来的函数。
//
// 重复声明同一个符号没有副作用（与 crate 里的声明指向同一处），但别把签名写错：
// 这里的指针类型 ABI 上都等价于 `CGEventSourceRef` / `CGEventRef` / `CFMachPortRef`。
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    // 启用 / 停用一个 tap（看门狗与系统停用通知都要用）。
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
    // 系统最后一次发出某类事件至今的秒数（看门狗判「tap 是否漏事件」）。
    fn CGEventSourceSecondsSinceLastEventType(
        state_id: CGEventSourceStateID,
        event_type: CGEventType,
    ) -> f64;
    // 当前物理修饰键状态（`wait_modifiers_released` 用）。
    fn CGEventSourceFlagsState(state_id: CGEventSourceStateID) -> CGEventFlags;
    fn CGEventSourceCreate(state_id: CGEventSourceStateID) -> *mut c_void;
    fn CGEventCreate(source: *mut c_void) -> *mut c_void;
    fn CGEventCreateMouseEvent(
        source: *mut c_void,
        mouse_type: CGEventType,
        pos: CGPoint,
        button: u32,
    ) -> *mut c_void;
    fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
    fn CGEventPost(location: CGEventTapLocation, event: *mut c_void);
}

// 辅助功能（AX）接口。`AXUIElementRef` 是 CFType（需 CFRelease）。
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
}

// CoreFoundation 的 `CFGetTypeID`：把 AX 拿回来的 CFType 先验明真身再用（见 [`ax_string`]）。
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
}

// AX 属性名 / 选项名的字面量。
//
// **故意不链 `kAX*` 符号**：SDK 里这些名字有的（有的版本）是 `#define kAXTitleAttribute
// CFSTR("AXTitle")` 这样的宏，有的是导出的 `CFStringRef` 变量。写成 `extern static` 时，
// 宏那种情况下是**链接期**才报的未定义符号——本仓库在 Windows 上开发，macOS 的链接在
// 开发机上跑不了，这类错只会在真机（或 CI 的 macOS job）上炸。属性名本身是稳定 ABI，
// 直接造句更省事，也少一处「只有真机才能验」的隐患。
const AX_FOCUSED_APPLICATION_ATTR: &str = "AXFocusedApplication";
const AX_FOCUSED_WINDOW_ATTR: &str = "AXFocusedWindow";
const AX_TITLE_ATTR: &str = "AXTitle";
const AX_TRUSTED_CHECK_OPTION_PROMPT: &str = "AXTrustedCheckOptionPrompt";

/// 把 AX 的 `Copy` 规则返回值当字符串取用（不是字符串就释放并返回 None）。
///
/// 必须验类型：`AXUIElementCopyAttributeValue` 的返回值是 `CFTypeRef`，只有文档承诺是
/// 字符串的那些属性（本模块只取标题 `AXTitle`）才是 CFString，而类型对不上时按 CFString
/// 去解析是未定义行为（不是报错、是崩）。这条路径跑在「前台窗口标题」条件求值与 60ms
/// 巡检线程上，崩一次就是整个应用没了。
unsafe fn ax_string(value: CFTypeRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    if CFGetTypeID(value) != CFString::type_id() {
        CFRelease(value);
        return None;
    }
    // `Copy` 规则 = 我们持有 +1，交给 CFString 托管（Drop 时释放），不必手工 CFRelease。
    Some(CFString::wrap_under_create_rule(value as CFStringRef).to_string())
}

// `libproc` 的 `proc_pidpath`（在 libSystem 里，不用额外 link）。
extern "C" {
    fn proc_pidpath(pid: i32, buffer: *mut c_void, size: u32) -> i32;
}

/// 输入注入：按键 / 组合键 / 鼠标键 / 文本（与 Windows、Linux 后端同接口）。
pub mod simulate {
    //! 全部从 **Private 事件源**发出，好让 tap 认出「这是自己发的」并放行（防回环，
    //! 见模块头「注入识别」）。
    //!
    //! 事件投递到 **HID 位置**（`CGEventPost(kCGHIDEventTap, ..)`）——注意这与「建 tap」
    //! 的要求相反：建 tap 在 HID 位置只有 root 能做（故我们那个 tap 装在会话级），
    //! 而**投递**事件到 HID 位置不需要 root，这正是合成按键的常规做法。投出去的事件仍会
    //! 顺流经过我们自己那个会话级 tap，靠注入识别放行。
    //!
    //! 文本走剪贴板 + ⌘V（对中文等 Unicode 最稳，与 Windows 的 Ctrl+V、Linux 版一致）；
    //! 副作用是短暂占用并恢复剪贴板。

    use std::ffi::c_void;
    use std::io;
    use std::time::{Duration, Instant};

    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, ScrollEventUnit,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    use kada_core::{Key, MouseButton, MouseOp};

    use super::{
        key_to_code, key_to_mouse_button, modifier_flag, CFRelease, CGEventCreate,
        CGEventCreateMouseEvent, CGEventGetLocation, CGEventPost, CGEventSourceCreate,
        CGEventSourceFlagsState,
    };

    pub fn down(k: Key) {
        post(k, true);
    }

    pub fn up(k: Key) {
        post(k, false);
    }

    pub fn tap(k: Key) {
        down(k);
        up(k);
    }

    /// 按一串组合键：全部按下 → 稍停 → 逆序松开，避免组合键太快导致目标程序收不到。
    ///
    /// 每发一个键都把「当前该带的修饰位」显式写进事件 flags：注入的修饰键虽然会更新系统
    /// 状态位，但同一批事件里的后续键不该依赖那个时序（`⌘V` 的 V 必须自带 Command 位，
    /// 否则部分应用读事件 flags 时看不到修饰键）。
    pub fn chord(keys: &[Key]) {
        let mut flags = physical_flags();
        for k in keys {
            if let Some(m) = super::key_as_modifier(*k) {
                flags |= modifier_flag(m);
            }
            post_with(*k, true, flags);
        }
        std::thread::sleep(Duration::from_millis(30));
        for k in keys.iter().rev() {
            if let Some(m) = super::key_as_modifier(*k) {
                flags &= !modifier_flag(m);
            }
            post_with(*k, false, flags);
        }
    }

    /// 发一个键事件，修饰位取「当前物理状态 + 该键自身的修饰位」。
    fn post(k: Key, down: bool) {
        let mut flags = physical_flags();
        if let Some(m) = super::key_as_modifier(k) {
            // 修饰键自己那一发：按下带上自己的位，抬起前先摘掉（与系统的 FlagsChanged
            // 语义一致——事件里的位反映「变化之后」的状态）。
            if down {
                flags |= modifier_flag(m);
            } else {
                flags &= !modifier_flag(m);
            }
        }
        post_with(k, down, flags);
    }

    fn post_with(k: Key, down: bool, flags: CGEventFlags) {
        if let Some(button) = key_to_mouse_button(k) {
            post_mouse(button, down);
            return;
        }
        let Some(code) = key_to_code(k) else {
            return; // 本平台没有这个键（媒体键 / F21~F24）：静默无操作
        };
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
            return;
        };
        let Ok(event) = CGEvent::new_keyboard_event(source, code, down) else {
            return;
        };
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
    }

    /// 注入鼠标其它键（中键 / 侧键）。
    ///
    /// core-graphics 的 `CGMouseButton` 只声明了左/右/中三个枚举值，而侧键是 3/4，
    /// 传进去是构造非法枚举值（UB），故这里直接走原始接口。
    /// 坐标必须给：`OtherMouse*` 事件不自带位置，用当前光标位置（`CGEventCreate(NULL)`
    /// 生成的事件带的就是当前鼠标位置，这是 CoreGraphics 里读光标的常规做法）。
    fn post_mouse(button: u32, down: bool) {
        let ty = if down { CGEventType::OtherMouseDown } else { CGEventType::OtherMouseUp };
        let pos = unsafe {
            let probe = CGEventCreate(std::ptr::null_mut());
            if probe.is_null() {
                core_graphics::geometry::CGPoint::new(0.0, 0.0)
            } else {
                let p = CGEventGetLocation(probe);
                CFRelease(probe as *const c_void);
                p
            }
        };
        unsafe {
            let source = CGEventSourceCreate(CGEventSourceStateID::Private);
            if source.is_null() {
                return;
            }
            let event = CGEventCreateMouseEvent(source, ty, pos, button);
            if !event.is_null() {
                CGEventPost(CGEventTapLocation::HID, event);
                CFRelease(event as *const c_void);
            }
            CFRelease(source as *const c_void);
        }
    }

    /// 鼠标模拟：移动 / 点击 / 滚轮（[`kada_core::MouseOp`]，7.1-㊱）。
    /// 移动与滚轮都是**相对**量（移动 = 当前坐标 + 增量）；点击作用在当前光标位置。
    pub fn mouse(op: &MouseOp) -> io::Result<()> {
        match op {
            MouseOp::Move { dx, dy } => {
                let p = current_pos();
                let target = CGPoint::new(p.x + f64::from(*dx), p.y + f64::from(*dy));
                post_mouse_at(CGEventType::MouseMoved, target, 0)?;
            }
            MouseOp::Click { button } => {
                let (down, up, num) = match button {
                    MouseButton::Left => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, 0),
                    MouseButton::Right => {
                        (CGEventType::RightMouseDown, CGEventType::RightMouseUp, 1)
                    }
                    MouseButton::Middle => {
                        (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, 2)
                    }
                };
                let p = current_pos();
                post_mouse_at(down, p, num)?;
                post_mouse_at(up, p, num)?;
            }
            MouseOp::Scroll { dx, dy } => {
                let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
                    return Err(io::Error::other("CGEventSource 创建失败"));
                };
                let event = CGEvent::new_scroll_event(source, ScrollEventUnit::LINE, 2, *dy, *dx, 0)
                    .map_err(|_| io::Error::other("滚动事件创建失败"))?;
                event.post(CGEventTapLocation::HID);
            }
        }
        Ok(())
    }

    /// 读当前光标位置（`CGEventCreate(NULL)` 生成的事件带的就是当前鼠标位置）。
    fn current_pos() -> CGPoint {
        unsafe {
            let probe = CGEventCreate(std::ptr::null_mut());
            if probe.is_null() {
                CGPoint::new(0.0, 0.0)
            } else {
                let p = CGEventGetLocation(probe);
                CFRelease(probe as *const c_void);
                p
            }
        }
    }

    /// 在指定坐标投递一个鼠标事件（`button`：0 左 / 1 右 / 2 中）。
    fn post_mouse_at(ty: CGEventType, pos: CGPoint, button: u32) -> io::Result<()> {
        unsafe {
            let source = CGEventSourceCreate(CGEventSourceStateID::Private);
            if source.is_null() {
                return Err(io::Error::other("CGEventSource 创建失败"));
            }
            let event = CGEventCreateMouseEvent(source, ty, pos, button);
            let ok = !event.is_null();
            if ok {
                CGEventPost(CGEventTapLocation::HID, event);
                CFRelease(event as *const c_void);
            }
            CFRelease(source as *const c_void);
            if !ok {
                return Err(io::Error::other("鼠标事件创建失败"));
            }
        }
        Ok(())
    }

    /// 当前物理修饰键（Shift / Ctrl / Option / Command）状态位。
    fn physical_flags() -> CGEventFlags {
        (unsafe { CGEventSourceFlagsState(CGEventSourceStateID::HIDSystemState) })
            & (CGEventFlags::CGEventFlagShift
                | CGEventFlags::CGEventFlagControl
                | CGEventFlags::CGEventFlagAlternate
                | CGEventFlags::CGEventFlagCommand)
    }

    /// 把文本粘贴到当前焦点控件。
    pub fn type_text(text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        let prev = cb.get_text().ok();
        cb.set_text(text.to_string()).map_err(io::Error::other)?;
        // macOS 的粘贴是 ⌘V（键模型里 Meta 就是 ⌘）。
        chord(&[Key::Meta, Key::V]);
        // 等目标程序处理完粘贴再恢复剪贴板，避免竞态（同 Windows / Linux 版）。
        std::thread::sleep(Duration::from_millis(80));
        if let Some(p) = prev {
            let _ = cb.set_text(p);
        }
        Ok(())
    }

    /// 把文本逐字符直发到当前焦点控件（unicode string 事件，不经过剪贴板）。
    ///
    /// 目标程序吞 ⌘V（游戏 / 终端）时的兜底（规划 7.3-㉒），与 Windows 的
    /// `KEYEVENTF_UNICODE` 同位：给每个字符造一个键码 0xFFFF（不对应任何实体键）的
    /// 键盘事件，字符写在事件的 unicode string 字段（`CGEventKeyboardSetUnicodeString`），
    /// 目标程序直接收到字符、不经过键盘布局，物理修饰键（⌃⌥⇧⌘）污染不了它。
    /// 换行 / 制表发真实键码（Return / Tab，`\r` 跳过）——多数程序不认 Unicode 控制码。
    pub fn type_text_unicode(text: &str) -> io::Result<()> {
        let steps = unicode_steps(text);
        if steps.is_empty() {
            return Ok(());
        }
        for step in steps {
            match step {
                UnicodeStep::Key(k) => tap(k),
                UnicodeStep::Chars(units) => post_unicode(&units)?,
            }
        }
        Ok(())
    }

    /// 文本直发的单个步骤（拆分逻辑可单测）：真实按键，或一组 UTF-16 码元。
    #[derive(Debug, PartialEq, Eq)]
    enum UnicodeStep {
        Key(Key),
        Chars(Vec<u16>),
    }

    /// 把文本拆成直发步骤：`\n`→Return、`\t`→Tab、`\r` 跳过，其余每字符一组码元
    /// （BMP 外字符的代理对整组放进同一个事件，由系统 / 目标程序拼回）。
    fn unicode_steps(text: &str) -> Vec<UnicodeStep> {
        let mut steps = Vec::new();
        // 一个 char 最多两个 UTF-16 码元（char::encode_utf16 写缓冲区，非迭代器）。
        let mut buf = [0u16; 2];
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => steps.push(UnicodeStep::Key(Key::Enter)),
                '\t' => steps.push(UnicodeStep::Key(Key::Tab)),
                _ => steps.push(UnicodeStep::Chars(ch.encode_utf16(&mut buf).to_vec())),
            }
        }
        steps
    }

    /// 发一个「无实体键」的字符事件（down 带码元 + up）。事件造不出来按失败上报——
    /// 与 Windows 版对 `SendInput` 被拦截的上报同口径，别让「贴不出」变成无声无息。
    fn post_unicode(units: &[u16]) -> io::Result<()> {
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) else {
            return Err(io::Error::other("CGEventSource 创建失败"));
        };
        let down = CGEvent::new_keyboard_event(source, 0xFFFF, true)
            .map_err(|_| io::Error::other("字符事件创建失败"))?;
        down.set_string_from_utf16_unchecked(units);
        down.post(CGEventTapLocation::HID);
        // up 事件不带字符串、失败不值得报：字符已送出，只影响「按住」时长的口径。
        if let Ok(source) = CGEventSource::new(CGEventSourceStateID::Private) {
            if let Ok(up) = CGEvent::new_keyboard_event(source, 0xFFFF, false) {
                up.post(CGEventTapLocation::HID);
            }
        }
        Ok(())
    }

    /// 读取剪贴板文本（用于「转大小写」动作：复制选中 → 读剪贴板 → 转换 → 粘贴）。
    pub fn get_clipboard_text() -> io::Result<String> {
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        cb.get_text().map_err(io::Error::other)
    }

    /// 等物理修饰键全部松开（最多 `timeout_ms`），返回是否等到了。
    ///
    /// 与 Windows 版同因同理：触发带修饰键的快捷键（如 ⌃⌥T）时修饰键仍物理按住，直接注入
    /// 会被污染成 ⌃⌥V，文本输不出来。这里直接查 HID 状态的修饰位（等价于 Windows 的
    /// `GetKeyState`），不是靠我们自己维护的集合——注入的键也在里面，正是我们要等的。
    pub fn wait_modifiers_released(timeout_ms: u64) -> bool {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if physical_flags().is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    mod unicode_tests {
        use super::*;

        #[test]
        fn unicode_steps_split() {
            // \r\n 只打一次回车；换行 / 制表发真实键码。
            assert_eq!(unicode_steps("\r\n"), vec![UnicodeStep::Key(Key::Enter)]);
            assert_eq!(unicode_steps("\t"), vec![UnicodeStep::Key(Key::Tab)]);
            // 普通字符与 BMP 外字符（emoji，代理对整组进同一事件）。
            assert_eq!(
                unicode_steps("a\u{1F600}"),
                vec![
                    UnicodeStep::Chars(vec![0x61]),
                    UnicodeStep::Chars(vec![0xD83D, 0xDE00]),
                ]
            );
            assert!(unicode_steps("").is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::event_source::CGEventSource;
    use kada_core::key_name;

    #[test]
    fn mapping_roundtrip() {
        for k in [
            Key::A, Key::K, Key::Digit9, Key::F12, Key::CapsLock, Key::Control, Key::Alt,
            Key::Shift, Key::Meta, Key::Enter, Key::Escape, Key::Space, Key::Backspace,
            Key::Delete, Key::Comma, Key::Minus, Key::Slash, Key::BracketLeft, Key::Quote,
            Key::ArrowUp, Key::PageDown, Key::F13, Key::F20, Key::NumLock, Key::Numpad0,
            Key::Numpad9, Key::NumpadAdd, Key::NumpadSubtract, Key::NumpadMultiply,
            Key::NumpadDivide, Key::NumpadDecimal, Key::NumpadEnter, Key::Insert,
        ] {
            let code = key_to_code(k).unwrap();
            assert_eq!(code_to_key(code), Some(k), "roundtrip {}", key_name(k));
        }
        // 主键区回车与小键盘回车在 macOS 上是两个不同键码（不像 Windows 共用 VK_RETURN）。
        assert_ne!(key_to_code(Key::Enter), key_to_code(Key::NumpadEnter));
        // 媒体键（NX_SYSDEFINED，非键码）与 F21~F24（无此硬件）不映射。
        for k in [
            Key::MediaPlayPause, Key::MediaPrev, Key::MediaNext, Key::VolumeMute,
            Key::VolumeDown, Key::VolumeUp, Key::F21, Key::F22, Key::F23, Key::F24,
        ] {
            assert_eq!(key_to_code(k), None, "{} 不该有键码", key_name(k));
        }
        // 鼠标键不是键码，走鼠标事件注入。
        assert_eq!(key_to_code(Key::MouseBack), None);
    }

    #[test]
    fn modifier_left_right_merge() {
        for (code, want) in [
            (KeyCode::SHIFT, Key::Shift),
            (KeyCode::RIGHT_SHIFT, Key::Shift),
            (KeyCode::CONTROL, Key::Control),
            (KeyCode::RIGHT_CONTROL, Key::Control),
            (KeyCode::OPTION, Key::Alt),
            (KeyCode::RIGHT_OPTION, Key::Alt),
            (KeyCode::COMMAND, Key::Meta),
            (KeyCode::RIGHT_COMMAND, Key::Meta),
        ] {
            assert_eq!(code_to_key(code), Some(want));
        }
        assert_eq!(code_to_key(KeyCode::CAPS_LOCK), Some(Key::CapsLock));
        assert_eq!(code_to_key(KeyCode::FUNCTION), None, "Fn 不进键模型，要原样放行");
    }

    #[test]
    fn flags_to_mods_ignores_non_modifier_bits() {
        let flags = CGEventFlags::CGEventFlagControl
            | CGEventFlags::CGEventFlagCommand
            | CGEventFlags::CGEventFlagAlphaShift
            | CGEventFlags::CGEventFlagNumericPad
            | CGEventFlags::CGEventFlagSecondaryFn;
        let mods = flags_to_mods(flags);
        assert!(mods.contains(&Modifier::Ctrl) && mods.contains(&Modifier::Meta));
        assert!(!mods.contains(&Modifier::Shift) && !mods.contains(&Modifier::Alt));
        assert_eq!(mods.len(), 2, "CapsLock / 小键盘 / Fn 位不能算修饰键");
        assert!(flags_to_mods(CGEventFlags::CGEventFlagAlphaShift).is_empty());
    }

    #[test]
    fn mouse_buttons_map_both_ways() {
        assert_eq!(code_to_key(KeyCode::ANSI_A), Some(Key::A));
        assert_eq!(mouse_button_to_key(2), Some(Key::MouseMiddle));
        assert_eq!(mouse_button_to_key(3), Some(Key::MouseBack));
        assert_eq!(mouse_button_to_key(4), Some(Key::MouseForward));
        assert_eq!(mouse_button_to_key(0), None, "左键不进键模型");
        assert_eq!(mouse_button_to_key(1), None, "右键不进键模型");
        assert_eq!(key_to_mouse_button(Key::MouseBack), Some(3));
        assert_eq!(key_to_mouse_button(Key::A), None);
    }

    #[test]
    fn injected_events_are_recognizable() {
        // 「自己注入的事件必须能被 tap 认出来」是防回环的硬前提：认不出就会把注入的 ⌘V
        // 当成用户按键吞掉 / 触发别的快捷键（用户恰好绑了 ⌘V 就是无限触发）。
        // 这里只锁「本进程造的事件必须被认出」这个结论，不锁死是靠哪条判据认出来的——
        // 两条各自独立，任一成立即可（判据本身的边界在下面那条纯逻辑测试里钉）。
        let Ok(private) = CGEventSource::new(CGEventSourceStateID::Private) else {
            panic!("Private 事件源创建失败");
        };
        let own = CGEvent::new_keyboard_event(private, KeyCode::ANSI_V, true).unwrap();
        assert!(is_injected(&own), "本进程从 Private 事件源造出的事件必须被认出");
    }

    #[test]
    fn injection_guard_boundaries() {
        // 判据 ①：事件源是 Private（我们注入的默认形态）。
        assert!(looks_injected(CGEventSourceStateID::Private as i64, 1, 999));
        // 判据 ②：创建者就是本进程（判据 ① 万一不成立时的兜底）。
        assert!(looks_injected(0, 999, 999));
        // 硬件事件：窗口服务器创建（pid 不是我们）且事件源不是 Private → 必须放行，
        // 否则用户按下的每个键都会被当成自己的注入吞掉。
        assert!(!looks_injected(CGEventSourceStateID::HIDSystemState as i64, 1234, 999));
        assert!(!looks_injected(0, 1234, 999));
    }

    #[test]
    fn caps_lock_direction_alternates() {
        // CapsLock 只有 FlagsChanged、且事件里的 AlphaShift 位反映开关而非物理按下，
        // 方向只能靠翻转记录：一按一放要得到一对 down/up。
        CAPS_DOWN.store(false, Ordering::Relaxed);
        assert!(!CAPS_DOWN.fetch_xor(true, Ordering::Relaxed), "第一次事件 = 按下");
        assert!(CAPS_DOWN.fetch_xor(true, Ordering::Relaxed), "第二次 = 抬起");
        assert!(!CAPS_DOWN.fetch_xor(true, Ordering::Relaxed), "第三次 = 再次按下");
        reset_runtime_state();
        assert!(!CAPS_DOWN.load(Ordering::Relaxed), "复位后回到未按下");
    }

    #[test]
    fn reset_releases_stale_state() {
        // 模拟「tap 被停用时正按着 / 正吞着键」留下的脏状态。
        SWALLOWED.lock().unwrap().insert(Key::K);
        HELD_KEYS.lock().unwrap().insert(Key::K);
        REPLACED_DOWN.lock().unwrap().insert(Key::K, Key::Control);
        assert_eq!(take_replaced_down(), vec![Key::Control], "先取出待释放的目标键补 up");
        reset_runtime_state();
        assert!(SWALLOWED.lock().unwrap().is_empty(), "被吞的键必须清空，否则永远等不到抬起");
        assert!(
            HELD_KEYS.lock().unwrap().is_empty(),
            "按住集合必须清空，否则该键后续按下全被判成自动重复（键变哑）"
        );
    }

    #[test]
    fn repeat_needs_key_still_held() {
        // HELD_KEYS 口径与 Windows / Linux 一致：抬起后再按是新的按下，按住期间再 down 才是重复。
        note_key_up(Key::K);
        assert!(!HELD_KEYS.lock().unwrap().contains(&Key::K));
        assert!(HELD_KEYS.lock().unwrap().insert(Key::K), "首次 down");
        assert!(!HELD_KEYS.lock().unwrap().insert(Key::K), "仍按住 → 自动重复");
        note_key_up(Key::K);
        assert!(HELD_KEYS.lock().unwrap().insert(Key::K), "抬起后再按 = 新的按下");
        note_key_up(Key::K);
    }
}
