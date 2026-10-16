//! 提权伴随进程：补上 UIPI 挡住的「往提权窗口注入」（规划 7.2-㉑a / 档位 B）。
//!
//! 为什么需要它：低层钩子**看得见**按键（抓取没问题），但**注入**进不去提权窗口——UIPI
//! 只允许向同级或更低完整性级别的进程发输入。把同一个 `kada.exe` 用 UAC 拉起一份、让它
//! 只负责重放注入，就补上了这一格（见 `docs/底层输入能力规划.md` 档位 B）。
//!
//! 形态与边界：
//! - **单二进制**：`kada.exe --kada-agent <父进程 pid>` 就是代理模式，不初始化 Tauri 壳、
//!   不开窗口、不碰配置。零打包改动、零卸载残留（`docs/底层输入能力规划.md` §6 未决问题 3）。
//! - **按需、用完即退**：只在设置页点「启动提权进程」时弹一次 UAC（[`start`]）；客户端
//!   断开（关开关 / Kada 退出）即退出，父进程没了也立刻退出（[`spawn_parent_watch`]），
//!   不会在系统里留下孤儿的提权进程。
//! - **只做注入**：通道上只有「按键 / 鼠标 / 文本 / 停顿」几类 op（[`Op`]），没有「执行
//!   命令」这种口子——提权进程的能力面必须和它的用途一样窄（见 [`verify_client`]）。
//! - **安全桌面仍不行**：UAC 提示 / 锁屏 / 登录界面连提权进程也注入不进去，本档不解决。
//!
//! 通道与信任边界：
//! - 通道名 `\\.\pipe\kada-uipi-<会话>-<用户 SID>`，DACL **只放行本用户**（[`SecurityDescriptor`]）；
//! - 服务端只服务「镜像路径 == 自身 exe 且同一用户」的客户端（[`verify_client`]）；
//! - 客户端只认「镜像路径 == 自身 exe 且确实提权」的服务端（[`verify_server`]）——名字被抢注
//!   时宁可连不上，也不要听一个来路不明的进程说「注入成功了」。

use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{mpsc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_CANCELLED, ERROR_INVALID_PARAMETER, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_FIRST_PIPE_INSTANCE,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
    GetNamedPipeServerProcessId, SetNamedPipeHandleState, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    WaitForSingleObject, INFINITE, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, SW_HIDE,
};

use kada_core::{key_name, Key, MouseButton, MouseOp};

use super::simulate::play_local;

/// 代理模式的命令行标志（`kada.exe --kada-agent <父进程 pid>`）；壳层据此在 `main` 里分派。
pub const AGENT_ARG: &str = "--kada-agent";

/// 握手行（服务端 → 客户端）：客户端连上后必须收到它，才认这个通道（版本不符就退回本地注入
/// ——宁可注入不进去，也不要按一版对不上的协议乱发事件）。
pub const HANDSHAKE: &str = "hello kada-agent 1";

/// 客户端问候（客户端 → 服务端）：服务端**收到它**才把这次会话当成「自己人」。
///
/// 只靠「连上了」不够：客户端校验完服务端可能掉头就走（对端不是提权进程 / 版本不符），
/// 这时服务端只看到「有人连上又断了」，若算作自己人走了，提权进程就被这一次白连带走了
/// （实测过，见 `docs/变更归档.md` V0.89）。有了这条问候，「连完就走」= 陌生人 → 继续等。
pub const CLIENT_HELLO: &str = "hello kada-client 1";

/// 通道名前缀。名字里带**会话号 + 用户 SID**：同一台机器上多用户 / 多会话各用各的通道，
/// 不会互相连到对方的提权进程（DACL 是第二道，见 [`SecurityDescriptor`]）。
const PIPE_PREFIX: &str = r"\\.\pipe\kada-uipi-";

/// 一批最多多少 op（防呆：对端异常时不该被一行行喂到天亮）。
const MAX_OPS: usize = 4096;
/// 单行上限（`text` 走十六进制，16KB 文本 = 32KB 行；留一倍余量）。
const MAX_LINE: usize = 128 * 1024;
/// 一批 op 的回复时限：超时即认为提权进程失去响应，回落本地注入（注入有界，见 §5.7）。
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
/// 「需要提权但通道没起来」的提示冷却：连按快捷键不该刷屏（一次提醒就够）。
const NOTICE_COOLDOWN_MS: u64 = 60_000;

/// 一个注入操作——本地重放与提权进程重放**共用同一份意图**。
///
/// 拆成 op 而不是直接 `SendInput` 的原因：同一份注入要能在两个进程里重放（提权进程那份才
/// 进得了提权窗口），而且 op 是纯数据，能单测、能上线（[`encode_batch`]）。
#[derive(Clone, PartialEq, Eq)]
pub enum Op {
    /// 按下某个键（含鼠标键）。
    Down(Key),
    /// 抬起某个键。
    Up(Key),
    /// 鼠标操作（移动 / 滚轮 / 点击）。
    Mouse(MouseOp),
    /// 逐字符直发一段文本（`KEYEVENTF_UNICODE`，不经剪贴板、一次发完）。
    Text(String),
    /// 停顿若干毫秒。
    Pause(u64),
}

// Debug 手写：Text 可能很长，调试输出里截断（免得日志被一整段文本糊住）。
impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Op::Down(k) => write!(f, "Down({})", key_name(*k)),
            Op::Up(k) => write!(f, "Up({})", key_name(*k)),
            Op::Mouse(m) => write!(f, "Mouse({m:?})"),
            Op::Text(t) => write!(f, "Text({} 字符)", t.chars().count()),
            Op::Pause(ms) => write!(f, "Pause({ms}ms)"),
        }
    }
}

/// 一批 op → 线上文本：一行一个 op，末尾一个空行表示「这批完了」。
pub fn encode_batch(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        out.push_str(&encode_op(op));
        out.push('\n');
    }
    out.push('\n');
    out
}

/// 一批 op ← 线上文本（先把整批解完再执行：坏行不会被执行到一半）。
pub fn decode_batch(lines: &[String]) -> Result<Vec<Op>, String> {
    lines
        .iter()
        .map(|l| decode_op(l).ok_or_else(|| format!("无法识别的 op：{l}")))
        .collect()
}

/// 一个 op → 一行。键名与配置落盘**同源**（`kada_core::key_name` ↔ `Key::from_str`），
/// 所以配置里能写的键这里就能传，不会两处表对不上。
fn encode_op(op: &Op) -> String {
    match op {
        Op::Down(k) => format!("down {}", key_name(*k)),
        Op::Up(k) => format!("up {}", key_name(*k)),
        Op::Mouse(MouseOp::Move { dx, dy }) => format!("mouse move {dx} {dy}"),
        Op::Mouse(MouseOp::Scroll { dx, dy }) => format!("mouse scroll {dx} {dy}"),
        Op::Mouse(MouseOp::Click { button }) => format!("mouse click {}", button_name(*button)),
        // 文本按 UTF-8 字节的十六进制走：文本里的空格 / 换行不能直接上线（协议按行分帧）。
        // 空文本用 `-` 占位（空字符串切不出字段，`split_whitespace` 会把它吃掉）。
        Op::Text(text) if text.is_empty() => "text -".to_string(),
        Op::Text(text) => format!("text {}", to_hex(text)),
        Op::Pause(ms) => format!("pause {ms}"),
    }
}

/// 一行 → 一个 op。`None` = 这行不合法（含多余字段——宁可判错也不要「看起来对」地错）。
fn decode_op(line: &str) -> Option<Op> {
    let mut it = line.split_whitespace();
    let op = match it.next()? {
        "down" => Op::Down(Key::from_str(it.next()?).ok()?),
        "up" => Op::Up(Key::from_str(it.next()?).ok()?),
        "pause" => Op::Pause(it.next()?.parse().ok()?),
        "text" => match it.next()? {
            "-" => Op::Text(String::new()),
            hex => Op::Text(from_hex(hex)?),
        },
        "mouse" => match it.next()? {
            "move" => Op::Mouse(MouseOp::Move { dx: num(&mut it)?, dy: num(&mut it)? }),
            "scroll" => Op::Mouse(MouseOp::Scroll { dx: num(&mut it)?, dy: num(&mut it)? }),
            "click" => Op::Mouse(MouseOp::Click { button: button_from(it.next()?)? }),
            _ => return None,
        },
        _ => return None,
    };
    it.next().is_none().then_some(op)
}

fn num(it: &mut std::str::SplitWhitespace<'_>) -> Option<i32> {
    it.next()?.parse().ok()
}

/// 鼠标键名（与配置里 `MouseButton` 的 `snake_case` 落盘名一致）。
fn button_name(b: MouseButton) -> &'static str {
    match b {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Back => "back",
        MouseButton::Forward => "forward",
    }
}

fn button_from(s: &str) -> Option<MouseButton> {
    Some(match s {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "back" => MouseButton::Back,
        "forward" => MouseButton::Forward,
        _ => return None,
    })
}

fn to_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn from_hex(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

/// 「需要提权、但通道没用上」的原因（平台侧只报 id、不出文案——措辞交给壳层）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// 开关没开（`settings.elevated_inject` 关着）。
    Disabled,
    /// 开关开着，但提权进程没在跑。
    NotRunning,
    /// 通道在，但这一批没送成（提权进程失去响应 / 报错）。
    Failed,
}

/// 一次「该走提权通道却没走成」的记录（供壳层推消息中心，见 [`take_notice`]）。
#[derive(Clone, Debug)]
pub struct Notice {
    /// 前台那个提权窗口的进程名（如 `notepad.exe`）；取不到时为空串。
    pub app: String,
    /// 为什么没用上。
    pub reason: Reason,
}

/// 提权通道的现状（设置页卡片显示它）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// 开关是否打开（`settings.elevated_inject`）。
    pub enabled: bool,
    /// 提权进程是否已连上。
    pub connected: bool,
}

/// 开关（`settings.elevated_inject`）：关着就**完全不路由**，行为与没有这个功能时一致。
static ENABLED: AtomicBool = AtomicBool::new(false);
/// 当前通道（`Some` = 提权进程在跑且已握手）。
static CHANNEL: LazyLock<Mutex<Option<Channel>>> = LazyLock::new(|| Mutex::new(None));
/// 待报的提示（见 [`take_notice`]）。
static NOTICE: LazyLock<Mutex<Option<Notice>>> = LazyLock::new(|| Mutex::new(None));
/// 上次报提示的时刻（`GetTickCount` 毫秒；0 = 还没报过），用于冷却。
static NOTICE_AT: AtomicU64 = AtomicU64::new(0);
/// 「开关没开」这类提示只报一次：它不影响用户此刻在做的事（本来也没开），多说就是打扰。
static DISABLED_TOLD: AtomicBool = AtomicBool::new(false);
/// 本进程是否提权（0=未知 1=否 2=是）：进程活一辈子值都不变，查一次记一辈子。
static SELF_ELEVATED: AtomicU8 = AtomicU8::new(0);
/// 前台窗口 → 「这个前台是不是提权进程的窗口」的缓存：换窗口才重查
/// （每次注入都查 4 次系统调用不值得，而注入是按键级的高频动作）。
static FG_CACHE: LazyLock<Mutex<(isize, Option<String>)>> = LazyLock::new(|| Mutex::new((0, None)));

/// 告诉钩子层「配置里这个开关现在是什么」。关掉时顺带断开通道——提权进程随之退出。
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        stop();
    }
}

/// 断开通道（提权进程那边的读会失败，随即自行退出）。
pub fn stop() {
    let taken = CHANNEL.lock().unwrap().take();
    drop(taken);
}

/// 通道现状。
pub fn status() -> Status {
    Status {
        enabled: ENABLED.load(Ordering::Relaxed),
        connected: CHANNEL.lock().unwrap().is_some(),
    }
}

/// 拉起提权进程并连上通道。**这是唯一会弹 UAC 的入口**，只由设置页的动作调用。
///
/// 顺序上先试一次「直接连」：提权进程可能已经在跑（比如 Kada 刚重启、进程还在），
/// 那就没必要再弹一次授权。连不上才 `runas` 拉起，再在 `wait` 内轮询等它把管道建起来。
pub fn start(wait: Duration) -> io::Result<String> {
    if CHANNEL.lock().unwrap().is_some() {
        return Ok("提权通道已连接".into());
    }
    let name = pipe_name()?;
    if let Ok(ch) = connect_named(&name, Duration::ZERO, true) {
        let pid = ch.agent_pid;
        *CHANNEL.lock().unwrap() = Some(ch);
        return Ok(format!("已连上正在运行的提权进程（pid {pid}）"));
    }
    spawn_elevated()?;
    let deadline = Instant::now() + wait;
    loop {
        match connect_named(&name, Duration::ZERO, true) {
            Ok(ch) => {
                let pid = ch.agent_pid;
                *CHANNEL.lock().unwrap() = Some(ch);
                return Ok(format!("提权进程已启动（pid {pid}）"));
            }
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(120)),
            Err(e) => {
                return Err(io::Error::other(format!(
                    "提权进程已拉起，但 {} 秒内没能连上通道：{e}",
                    wait.as_secs()
                )))
            }
        }
    }
}

/// 这次注入要不要交给提权进程（[`KadaState`] 的注入路径唯一的分叉点）。
///
/// - `None`：**不该走**（前台不是提权窗口）——调用方照旧本地注入，本档完全不介入；
/// - `Some(Ok)`：提权进程已经重放完了；
/// - `Some(Err)`：该走却没走成，调用方回落本地注入，同时留一条提示（[`take_notice`]）——
///   「失败可见」是硬要求，落回现行为之外还得让用户知道为什么。
pub fn forward(ops: &[Op]) -> Option<io::Result<()>> {
    let app = elevated_foreground()?;
    if !ENABLED.load(Ordering::Relaxed) {
        note(Notice { app, reason: Reason::Disabled });
        return None;
    }
    let attempt = CHANNEL.lock().unwrap().as_ref().map(|ch| ch.request(ops));
    match attempt {
        // 通道在，且这批送成了。
        Some(Ok(())) => Some(Ok(())),
        // 通道在，但这批没送成：撤掉它（别让下一次注入再白等 2 秒），回落本地并提示。
        Some(Err(e)) => {
            stop();
            note(Notice { app, reason: Reason::Failed });
            Some(Err(e))
        }
        // 开关开着但提权进程没起来：回落本地并提示怎么把它起来。
        None => {
            note(Notice { app, reason: Reason::NotRunning });
            None
        }
    }
}

/// 取走一条待报提示（壳层定时器线程每拍看一眼）。同一类提示有冷却，避免连按刷屏。
pub fn take_notice() -> Option<Notice> {
    let taken = NOTICE.lock().unwrap().take();
    if taken.is_some() {
        NOTICE_AT.store(unsafe { GetTickCount() } as u64, Ordering::Relaxed);
    }
    taken
}

/// 记一条待报提示（受冷却限制）。`Disabled` 只报一次（见 [`DISABLED_TOLD`]）。
fn note(notice: Notice) {
    if notice.reason == Reason::Disabled {
        if DISABLED_TOLD.swap(true, Ordering::Relaxed) {
            return;
        }
    } else {
        let now = unsafe { GetTickCount() } as u64;
        let last = NOTICE_AT.load(Ordering::Relaxed);
        if last != 0 && now.wrapping_sub(last) < NOTICE_COOLDOWN_MS {
            return;
        }
    }
    *NOTICE.lock().unwrap() = Some(notice);
}

/// 前台窗口是不是一个**提权进程**的窗口（本进程未提权时才有意义），返回它的可执行文件名。
///
/// `SendInput` 送进的是当前前台（的焦点窗口），所以「该不该走提权通道」就是问前台属谁。
/// 查不到（保护进程 / 前台是 0）按「不是提权窗口」处理：那是现状里的行为，本档不介入。
fn elevated_foreground() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    let key = hwnd.0 as isize;
    if let Ok(cache) = FG_CACHE.lock() {
        if cache.0 == key {
            return cache.1.clone();
        }
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let hit = (pid != 0 && !self_elevated() && process_elevated(pid).unwrap_or(false))
        .then(|| super::process_name_of(pid).unwrap_or_default());
    if let Ok(mut cache) = FG_CACHE.lock() {
        *cache = (key, hit.clone());
    }
    hit
}

/// 本进程是不是提权运行的（结果在进程生命周期内不变）。Kada 本体正常是非提权的；
/// 万一它以管理员身份启动，前台再高的完整性级别也够得着，就无需再转一道。
fn self_elevated() -> bool {
    match SELF_ELEVATED.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let v = process_elevated(unsafe { GetCurrentProcessId() }).unwrap_or(false);
            SELF_ELEVATED.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            v
        }
    }
}

/// 某个进程是否提权（`TokenElevation`）。查不到返回 `None`（保护进程等），由调用方定夺。
fn process_elevated(pid: u32) -> Option<bool> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut token = HANDLE::default();
    let opened = unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) };
    let _ = unsafe { CloseHandle(handle) };
    opened.ok()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(token) };
    queried.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// 取进程的完整镜像路径（校验「是不是同一个 exe」用）。
fn image_path(pid: u32) -> Option<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len)
    };
    let _ = unsafe { CloseHandle(handle) };
    ok.ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize]))
}

/// 取进程令牌的用户 SID（字符串形式）。
fn token_user_sid(pid: u32) -> io::Result<String> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| io::Error::other(format!("打开进程 {pid} 失败：{e}")))?;
    let mut token = HANDLE::default();
    let opened = unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) };
    let _ = unsafe { CloseHandle(handle) };
    opened.map_err(|e| io::Error::other(format!("取进程令牌失败：{e}")))?;
    // 先问长度，再按长度取一份（TOKEN_USER 尾部挂着变长的 SID）。
    let mut len = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
    let mut buf = vec![0u8; len.max(size_of::<TOKEN_USER>() as u32) as usize];
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut c_void),
            buf.len() as u32,
            &mut len,
        )
    };
    let _ = unsafe { CloseHandle(token) };
    queried.map_err(|e| io::Error::other(format!("取令牌用户失败：{e}")))?;
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let mut out = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut out) }
        .map_err(|e| io::Error::other(format!("SID 转字符串失败：{e}")))?;
    let text = unsafe { out.to_string() }.map_err(io::Error::other);
    let _ = unsafe { LocalFree(Some(HLOCAL(out.0 as *mut c_void))) };
    text
}

/// 通道名（会话 + 用户 SID 隔离，见 [`PIPE_PREFIX`]）。
fn pipe_name() -> io::Result<String> {
    let pid = unsafe { GetCurrentProcessId() };
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(pid, &mut session) }
        .map_err(|e| io::Error::other(format!("取会话号失败：{e}")))?;
    Ok(format!("{PIPE_PREFIX}{session}-{}", token_user_sid(pid)?))
}

/// 本机本会话的通道名（代理模式建它，客户端连它；两者必须是同一个名字）。
pub fn default_pipe_name() -> io::Result<String> {
    pipe_name()
}

/// 探针 / 自测用的通道名（换个后缀，免得和正在跑的提权进程抢同一个名字）。
pub fn probe_pipe_name() -> io::Result<String> {
    Ok(format!("{}-probe", pipe_name()?))
}

/// 管道的 DACL：**只放行本用户**。
///
/// 提权进程能往任何提权窗口注入，所以「谁能对它说话」就是这条功能的权限边界：默认
/// 安全描述符会带上本用户所属的其它主体（比如 Administrators），这里改成只认自己
/// 那个用户 SID（`D:P(A;;GA;;;<user>)`，P = 保护，不再继承任何 ACE）。
struct SecurityDescriptor {
    ptr: *mut c_void,
}

impl SecurityDescriptor {
    fn for_current_user() -> io::Result<Self> {
        let sid = token_user_sid(unsafe { GetCurrentProcessId() })?;
        let sddl = to_wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("构造通道安全描述符失败：{e}")))?;
        Ok(Self { ptr: sd.0 })
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        let _ = unsafe { LocalFree(Some(HLOCAL(self.ptr))) };
    }
}

/// 代理模式主入口：建通道 → 等客户端 → 按请求重放注入 → 客户端断开即退出。
///
/// `parent` 是拉起自己的 Kada 进程 pid（`None` = 手工运行，不守望）。父进程一没就退出，
/// 所以「Kada 崩了 / 被强杀」也不会留下一个能往提权窗口注入的孤儿进程。
///
/// 只对**校验过的自己人**服务：来路不明的连接（同用户下的其它程序）被拒后退回继续等——
/// 若「谁连一下就退出」，任何本机程序都能一键把提权进程踢掉（等于让整个功能可被随便打死）。
pub fn serve(name: &str, parent: Option<u32>) -> io::Result<()> {
    if let Some(pid) = parent {
        spawn_parent_watch(pid);
    }
    let sd = SecurityDescriptor::for_current_user()?;
    loop {
        let pipe = create_pipe(name, &sd)?;
        match unsafe { ConnectNamedPipe(pipe, None) } {
            Ok(()) => {}
            // 竞态：客户端在 `ConnectNamedPipe` 之前就连上了（这时报的不是失败）。
            Err(e) if (e.code().0 as u32 & 0xFFFF) == ERROR_PIPE_CONNECTED.0 => {}
            Err(e) => {
                let _ = unsafe { CloseHandle(pipe) };
                return Err(io::Error::other(format!("等客户端接入失败：{e}")));
            }
        }
        let own = serve_client(pipe);
        let _ = unsafe { DisconnectNamedPipe(pipe) };
        let _ = unsafe { CloseHandle(pipe) };
        match own {
            // 自己人断开 = Kada 关了开关或退出了：本进程随之结束（「平时不运行」）。
            Ok(true) => return Ok(()),
            // 不是自己人，或这一轮半路断了：继续等下一个。
            _ => continue,
        }
    }
}

/// 建一个管道实例。带 `FIRST_PIPE_INSTANCE`：名字被别人抢注（同用户下的其它程序）时**宁可
/// 建不起来**，也不做它的替身。客户端那边也有身份校验，这是双保险。
fn create_pipe(name: &str, sd: &SecurityDescriptor) -> io::Result<HANDLE> {
    let wide = to_wide(name);
    let attr = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.ptr,
        bInheritHandle: false.into(),
    };
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0 | FILE_FLAG_FIRST_PIPE_INSTANCE.0),
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            1,
            64 * 1024,
            64 * 1024,
            0,
            Some(&attr),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // `CreateNamedPipeW` 失败时给的是 INVALID_HANDLE_VALUE（不是 Result），
        // 失败原因在 last error 里。
        return Err(io::Error::other(format!(
            "创建提权通道 {name} 失败（{}）",
            io::Error::last_os_error()
        )));
    }
    Ok(handle)
}

/// 服务一个客户端，直到它断开（断开 = Kada 关了开关或退出了，本进程随之结束）。
/// 返回值：`Ok(true)` = 对方是自己人（断开后代理该退出），`Ok(false)` = 被拒的陌生人。
///
/// **校验过之后**（`verify_client` 通过）就不再有「半路出错」这条路：读不动、写不出去都
/// 只有一个解释——自己人走了（关了开关 / Kada 退出了）。所以 `?` 在这里一律换成
/// 「当作自己人走了」，否则一次断连会被当成陌生人、代理继续守着一条死通道等下一个。
fn serve_client(pipe: HANDLE) -> io::Result<bool> {
    let mut chan = Pipe::new(pipe);
    if let Err(e) = verify_client(pipe) {
        // 不是自己人：说一句再断。对方可能是探针或手写的调试脚本，留个缘由省得查半天。
        let _ = chan.write_line(&format!("err {e}"));
        return Ok(false);
    }
    if chan.write_line(HANDSHAKE).is_err() {
        // 连握手都没送出去：对端连上就走了（探针的拒绝路径 / 同名抢注者），**不算自己人**，
        // 回头继续等——否则任何本机程序连一下就关，就能把提权进程踢掉。
        return Ok(false);
    }
    // 等客户端那条问候：**收到它才算自己人**。这里之后的一切断开都当「自己人走了」→ 退出。
    match chan.read_line() {
        Ok(line) if line == CLIENT_HELLO => {}
        _ => return Ok(false),
    }
    loop {
        let mut lines: Vec<String> = Vec::new();
        loop {
            let line = match chan.read_line() {
                Ok(line) => line,
                Err(_) => return Ok(true),
            };
            if line.is_empty() {
                break;
            }
            if lines.len() >= MAX_OPS {
                let _ = chan.write_line("err 一批 op 太多");
                return Ok(true);
            }
            lines.push(line);
        }
        let reply = match decode_batch(&lines) {
            Err(e) => format!("err {e}"),
            Ok(ops) => match play_local(&ops) {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("err {e}"),
            },
        };
        if chan.write_line(&reply).is_err() {
            return Ok(true);
        }
    }
}

/// 服务端只服务「同一个可执行文件 + 同一个用户」的客户端。
///
/// 这是提权进程的**权限边界**：它能往任何提权窗口注入，谁要是都能在这个管道上发指令，
/// 等于给机器上所有进程发了把 UIPI 万能钥匙。两道校验：
/// 1. 客户端镜像路径与自身完全一致——提权进程就是从这个 exe 拉起来的，别人冒充不了
///    （能改写安装目录里那个 exe 的，已经等于能改 Kada 本体，不在本档防线内）；
/// 2. 客户端令牌的用户 SID 与自己一致——管道 DACL 之外的显式第二道。
fn verify_client(pipe: HANDLE) -> io::Result<()> {
    let mut pid = 0u32;
    unsafe { GetNamedPipeClientProcessId(pipe, &mut pid) }
        .map_err(|e| io::Error::other(format!("取客户端 pid 失败：{e}")))?;
    ensure_same_image(pid, "客户端")?;
    if token_user_sid(pid)? != token_user_sid(unsafe { GetCurrentProcessId() })? {
        return Err(io::Error::other("客户端不是同一个用户"));
    }
    Ok(())
}

/// 客户端只认「镜像路径 == 自身 exe 且确实提权运行」的服务端。
///
/// 抢注管道名是这类「按名字连本地服务」的常规攻击面：不校验的话，同用户下先占住名字的
/// 进程就能收走我们的注入请求（至少是信息泄露），还能骗我们「注入成功了」（静默失效）。
fn verify_server(pipe: HANDLE, require_elevated: bool) -> io::Result<u32> {
    let mut pid = 0u32;
    unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) }
        .map_err(|e| io::Error::other(format!("取服务端 pid 失败：{e}")))?;
    ensure_same_image(pid, "提权进程")?;
    if require_elevated && !process_elevated(pid).unwrap_or(false) {
        return Err(io::Error::other("通道对端不是提权运行的进程"));
    }
    Ok(pid)
}

/// 校验某进程的镜像路径与自身一致（大小写不敏感）。
fn ensure_same_image(pid: u32, who: &str) -> io::Result<()> {
    let other = image_path(pid).ok_or_else(|| io::Error::other(format!("取不到{who}的镜像路径")))?;
    let own = std::env::current_exe().map_err(io::Error::other)?;
    let own = own.to_string_lossy();
    if other.to_lowercase() != own.to_lowercase() {
        return Err(io::Error::other(format!("{who}不是 Kada 本体（{other}）")));
    }
    Ok(())
}

/// 用 UAC 拉起提权进程（`runas` = 唯一的提权入口，也是唯一的弹窗点）。
fn spawn_elevated() -> io::Result<()> {
    let exe = std::env::current_exe().map_err(io::Error::other)?;
    // 把自己 pid 捎过去：提权进程据此守望父进程（见 [`spawn_parent_watch`]）。
    let params = to_wide(&format!("{AGENT_ARG} {}", unsafe { GetCurrentProcessId() }));
    let verb = to_wide("runas");
    let file = to_wide(&exe.to_string_lossy());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        if (e.code().0 as u32 & 0xFFFF) == ERROR_CANCELLED.0 {
            io::Error::other("已取消提权（UAC 授权被拒绝）")
        } else {
            io::Error::other(format!("拉起提权进程失败：{e}"))
        }
    })?;
    if !info.hProcess.is_invalid() {
        let _ = unsafe { CloseHandle(info.hProcess) };
    }
    Ok(())
}

/// 父进程一退出就跟着退（提权进程绝不能变成留在系统里的孤儿）。
fn spawn_parent_watch(pid: u32) {
    thread::spawn(move || match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(handle) => {
            unsafe { WaitForSingleObject(handle, INFINITE) };
            std::process::exit(0)
        }
        // 父进程已经不在（pid 无效）→ 干净退出；其它错误（多半是无权开它）→ 当作异常退出。
        Err(e) if (e.code().0 as u32 & 0xFFFF) == ERROR_INVALID_PARAMETER.0 => {
            std::process::exit(0)
        }
        Err(_) => std::process::exit(2),
    });
}

/// 与提权进程的连接：一批 op = 一次请求 / 一次回复。
///
/// 读写全交给**一个专属线程**串起来（[`spawn_worker`]）：同步管道句柄上挂着一个读，会把
/// 同一句柄的写一起卡住（实测：读线程 + 调用方直接写，写会一直不返回）。一个线程按
/// 「写一批 → 读一条回复」的顺序做，天然串行、也不会有并发读写。调用方把批次丢进队列、
/// 带超时等回复：等待有上界（注入有界，见 §5.7），批次也不会交错。
struct Channel {
    /// 交给专属线程的批次队列（`Sender` 一没，专属线程就收尾退出）。
    jobs: mpsc::Sender<Job>,
    /// 通道句柄（只有本结构负责关：关掉会打断专属线程正卡着的那次读）。
    handle: isize,
    /// 提权进程 pid（写进设置页状态，便于用户确认到底连上了谁）。
    agent_pid: u32,
}

/// 一次请求：一批 op + 回复口。
struct Job {
    ops: Vec<Op>,
    reply: mpsc::Sender<io::Result<()>>,
}

// 句柄是裸指针包装（`HANDLE`），跨线程持有要显式声明；安全性由「句柄只在专属线程里读写、
// 只有本结构关闭」保证。
unsafe impl Send for Channel {}
unsafe impl Sync for Channel {}

impl Channel {
    fn request(&self, ops: &[Op]) -> io::Result<()> {
        let (tx, rx) = mpsc::channel();
        if self.jobs.send(Job { ops: ops.to_vec(), reply: tx }).is_err() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "提权进程已断开"));
        }
        match rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "提权进程没有在时限内回应",
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "提权进程已断开"))
            }
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        // 关句柄：专属线程若正卡在读写里会就此出来退出（本结构独占这个句柄，不会被别人关）。
        let _ = unsafe { CloseHandle(HANDLE(self.handle as *mut c_void)) };
    }
}

/// 通道的专属线程：先握手，然后**按顺序**处理批次（写一批 → 读一条回复）。
///
/// 一次读失败（对端断开 / 回了看不懂的东西）就收工退出：连接可疑，后面每批都会白等 2 秒，
/// 不如让调用方下次请求直接得到「已断开」。
fn spawn_worker(
    handle: isize,
    jobs: mpsc::Receiver<Job>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    thread::spawn(move || {
        let mut pipe = Pipe::new(HANDLE(handle as *mut c_void));
        // 握手：第一行必须是版本行。对端可能是另一个版本的提权进程（升级后残留的），
        // 版本对不上就退回本地注入，不要按错版协议乱发事件。
        match pipe.read_line() {
            Ok(line) if line == HANDSHAKE => {
                // 回一条问候：服务端据此把这次会话认成「自己人」（见 [`CLIENT_HELLO`]）。
                if let Err(e) = pipe.write_line(CLIENT_HELLO) {
                    let _ = ready.send(Err(format!("提权通道握手失败：{e}")));
                    return;
                }
                if ready.send(Ok(())).is_err() {
                    return;
                }
            }
            Ok(line) => {
                let _ = ready.send(Err(format!("提权通道握手失败：{line}")));
                return;
            }
            Err(e) => {
                let _ = ready.send(Err(format!("提权通道握手失败：{e}")));
                return;
            }
        }
        while let Ok(job) = jobs.recv() {
            let result = (|| -> io::Result<()> {
                pipe.write_line(&encode_batch(&job.ops))?;
                let line = pipe.read_line()?;
                match line.strip_prefix("err ") {
                    Some(msg) => Err(io::Error::other(msg.to_string())),
                    None if line == "ok" => Ok(()),
                    None => Err(io::Error::other(format!("提权通道回了一条看不懂的内容：{line}"))),
                }
            })();
            let broken = result.is_err();
            let _ = job.reply.send(result);
            if broken {
                return;
            }
        }
    });
}

/// 探针 / 自测用的客户端句柄（正常路径不走它，见 [`start`]）。
pub struct ProbeClient(Channel);

impl ProbeClient {
    /// 提权进程（这里是探针进程内的那一个）的 pid。
    pub fn agent_pid(&self) -> u32 {
        self.0.agent_pid
    }

    /// 发一批 op，等一次回复。
    pub fn request(&self, ops: &[Op]) -> io::Result<()> {
        self.0.request(ops)
    }
}

/// 按名字连一条通道（探针 / 自测入口；`elevated_probe` 例子用它验「不开 UAC 也能验的那一段」：
/// 管道 DACL、双方身份校验、行协议往返）。`wait` 内轮询重试，代理还没建好管道时用得上；
/// `require_elevated` 放开时跳过「对端必须提权」这一条（探针里的代理跑在本进程、不是提权的，
/// **闸门本身**由探针先用 `true` 那一遍验）。
pub fn connect_for_probe(name: &str, wait: Duration, require_elevated: bool) -> io::Result<ProbeClient> {
    connect_named(name, wait, require_elevated).map(ProbeClient)
}

/// 连上通道（`wait` 内轮询重试；`ZERO` = 只试一次）。
fn connect_named(name: &str, wait: Duration, require_elevated: bool) -> io::Result<Channel> {
    let wide = to_wide(name);
    let deadline = Instant::now() + wait;
    let handle = loop {
        let opened = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        match opened {
            Ok(handle) => break Ok(handle),
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
            Err(e) => break Err(e),
        }
    }
    .map_err(|e| io::Error::other(format!("打不开提权通道失败：{e}")))?;
    // 显式对齐成字节流读（服务端也是字节流；哪天服务端改成消息模式，这里也不至于读到半截）。
    let mode = PIPE_READMODE_BYTE;
    let _ = unsafe { SetNamedPipeHandleState(handle, Some(&mode), None, None) };
    let pid = match verify_server(handle, require_elevated) {
        Ok(pid) => pid,
        Err(e) => {
            let _ = unsafe { CloseHandle(handle) };
            return Err(e);
        }
    };
    let raw = handle.0 as isize;
    let (jobs_tx, jobs_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    spawn_worker(raw, jobs_rx, ready_tx);
    match ready_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(Ok(_)) => Ok(Channel { jobs: jobs_tx, handle: raw, agent_pid: pid }),
        Ok(Err(e)) => {
            let _ = unsafe { CloseHandle(handle) };
            Err(io::Error::other(e))
        }
        Err(_) => {
            let _ = unsafe { CloseHandle(handle) };
            Err(io::Error::other("提权通道握手超时"))
        }
    }
}

/// 裸句柄上的行式读写。句柄的**所有权不在它身上**（在 [`Channel`] / 服务端循环那边），
/// 这里只借用。
///
/// **同一个句柄不许并发读写**（见 [`Channel`]）：会用的地方要么在专属线程里，要么在服务端
/// 那个「读一批 → 写一条回复」的单线程循环里。
struct Pipe {
    handle: HANDLE,
    buf: Vec<u8>,
}

impl Pipe {
    fn new(handle: HANDLE) -> Self {
        Self { handle, buf: Vec::new() }
    }

    /// 读一行（阻塞；去掉行尾 `\n` / `\r`）。连接断开时返回 `Err`。
    fn read_line(&mut self) -> io::Result<String> {
        loop {
            if let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=pos).collect();
                let text = String::from_utf8_lossy(&line[..line.len() - 1]);
                return Ok(text.trim_end_matches('\r').to_string());
            }
            let mut chunk = [0u8; 4096];
            let mut read = 0u32;
            unsafe { ReadFile(self.handle, Some(&mut chunk), Some(&mut read), None) }.map_err(
                |e| io::Error::new(io::ErrorKind::BrokenPipe, format!("读通道失败：{e}")),
            )?;
            if read == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "通道已关闭"));
            }
            self.buf.extend_from_slice(&chunk[..read as usize]);
            if self.buf.len() > MAX_LINE {
                return Err(io::Error::other("通道来的行太长"));
            }
        }
    }

    fn write_line(&mut self, line: &str) -> io::Result<()> {
        write_all(self.handle, format!("{line}\n").as_bytes())
    }
}

/// 往管道写满一段字节。
fn write_all(handle: HANDLE, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let mut written = 0u32;
        // 切一段最多 4KB 再写：`WriteFile` 一次最多这么大一块才稳（跨大量数据时本就分段）。
        let chunk = &bytes[..bytes.len().min(4096)];
        unsafe { WriteFile(handle, Some(chunk), Some(&mut written), None) }.map_err(|e| {
            io::Error::new(io::ErrorKind::BrokenPipe, format!("写通道失败：{e}"))
        })?;
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "通道写入 0 字节"));
        }
        bytes = &bytes[written as usize..];
    }
    Ok(())
}

/// Rust 字符串 → 以 NUL 结尾的宽字符串。
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 键名往返：**所有**键都要能过一遍协议（表两处不同步就会在这里红）。
    #[test]
    fn every_key_round_trips() {
        for k in Key::ALL {
            for op in [Op::Down(*k), Op::Up(*k)] {
                let line = encode_op(&op);
                let back = decode_op(&line).unwrap_or_else(|| panic!("{line} 解不回来"));
                assert_eq!(format!("{back:?}"), format!("{op:?}"), "{line}");
            }
        }
    }

    /// 鼠标三种操作与五个按键名都要往返（负数、零位移都要）。
    #[test]
    fn mouse_ops_round_trip() {
        let ops = [
            Op::Mouse(MouseOp::Move { dx: 15, dy: -8 }),
            Op::Mouse(MouseOp::Move { dx: 0, dy: 0 }),
            Op::Mouse(MouseOp::Scroll { dx: -3, dy: 120 }),
            Op::Mouse(MouseOp::Click { button: MouseButton::Left }),
            Op::Mouse(MouseOp::Click { button: MouseButton::Right }),
            Op::Mouse(MouseOp::Click { button: MouseButton::Middle }),
            Op::Mouse(MouseOp::Click { button: MouseButton::Back }),
            Op::Mouse(MouseOp::Click { button: MouseButton::Forward }),
        ];
        for op in ops {
            let line = encode_op(&op);
            assert_eq!(format!("{:?}", decode_op(&line).unwrap()), format!("{op:?}"), "{line}");
        }
    }

    /// 文本走十六进制：多字节（中文 / emoji）、空格换行都要原样回来。
    #[test]
    fn text_round_trips() {
        for text in ["hello world", "咔哒 Kada\n第二行", "\u{1F600}", ""] {
            let line = encode_op(&Op::Text(text.to_string()));
            assert!(!line.contains(' ') || line.starts_with("text "), "文本不能带裸空格：{line}");
            match decode_op(&line) {
                Some(Op::Text(back)) => assert_eq!(back, text),
                other => panic!("{text:?} 解成了 {other:?}"),
            }
        }
    }

    /// 一批 op 的分帧：一行一个 op，末尾空行。
    #[test]
    fn batch_framing() {
        let ops = [
            Op::Down(Key::Control),
            Op::Down(Key::V),
            Op::Pause(30),
            Op::Up(Key::V),
            Op::Up(Key::Control),
        ];
        let wire = encode_batch(&ops);
        let lines: Vec<String> = wire.lines().map(str::to_string).collect();
        assert_eq!(lines.len(), 6, "5 个 op + 末尾空行：{wire:?}");
        assert_eq!(lines.last().unwrap(), "");
        let back = decode_batch(&lines[..5]).unwrap();
        assert_eq!(back.len(), 5);
        assert_eq!(format!("{back:?}"), format!("{:?}", ops.to_vec()));
    }

    /// 坏行要报错而不是被静默略过（**先整批解完再执行**，坏行不执行到一半）。
    #[test]
    fn bad_lines_are_rejected() {
        let bad = [
            "down NoSuchKey",
            "down",                       // 缺参数
            "down Ctrl extra",            // 多余字段
            "mouse click sideways",
            "mouse move 1",               // 缺 dy
            "text 4f6",                   // 十六进制长度为奇数
            "text zz",                    // 非十六进制
            "pause abc",
            "run calc",                   // 通道上不存在「执行命令」这类口子
            "",
        ];
        for line in bad {
            assert!(decode_op(line).is_none(), "{line:?} 不该被认下");
            assert!(decode_batch(&[line.to_string()]).is_err(), "{line:?} 该整批报错");
        }
    }
}
