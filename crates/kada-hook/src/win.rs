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
//!
//! 已知天花板（升级路径）：
//! - 低层钩子拦不住 UAC 提权进程 / 部分游戏 → 驱动级拦截（Interception）。
//! - `type_text` 用剪贴板粘贴（中文最稳），会短暂占用剪贴板。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{mpsc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION,
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
/// 已决定吞掉的键：后续 keyup 也要吞。
static SWALLOWED: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Replace 注入后仍按下的宿主原键 → 目标键。
static REPLACED_DOWN: LazyLock<Mutex<HashMap<Key, Key>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// 当前物理按下的键（down 且未 up），用于识别自动重复。
static HELD_KEYS: LazyLock<Mutex<HashSet<Key>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// 钩子句柄。Drop 时给钩子线程发 WM_QUIT 并回收。
pub struct HookHandle {
    tid: u32,
    join: Option<JoinHandle<()>>,
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        let _ = unsafe { PostThreadMessageW(self.tid, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 安装全局键盘与鼠标钩子（同一线程跑消息循环）。处理函数在钩子线程回调内同步执行。
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
    Ok(HookHandle { tid, join: Some(join) })
}

fn hook_loop(ready_tx: &mpsc::Sender<u32>) -> io::Result<()> {
    let tid = unsafe { GetCurrentThreadId() };
    let kbd_hhook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }
        .map_err(|e| io::Error::other(format!("SetWindowsHookEx 键盘钩子失败: {e}")))?;
    let mouse_hhook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) }
        .map_err(|e| io::Error::other(format!("SetWindowsHookEx 鼠标钩子失败: {e}")))?;
    HOOK.store(kbd_hhook.0 as isize, Ordering::Relaxed);
    MOUSE_HOOK.store(mouse_hhook.0 as isize, Ordering::Relaxed);
    let _ = ready_tx.send(tid);

    let mut msg = MSG::default();
    // GetMessageW 返回 0 = 收到 WM_QUIT
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        _ = UnhookWindowsHookEx(kbd_hhook);
        _ = UnhookWindowsHookEx(mouse_hhook);
    }
    HOOK.store(0, Ordering::Relaxed);
    MOUSE_HOOK.store(0, Ordering::Relaxed);
    *SWALLOWED.lock().unwrap() = HashSet::new();
    *REPLACED_DOWN.lock().unwrap() = HashMap::new();
    *HELD_KEYS.lock().unwrap() = HashSet::new();
    *HANDLER.lock().unwrap() = None;
    Ok(())
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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

    /// 占用全局 [`HANDLER`] 的用例必须串行——cargo test 默认多线程并行，两个用例同时
    /// 换 HANDLER 会互相打断。中毒（上一个用例 panic）也继续跑，别让一个失败连带整片红。
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
        // 双击唤醒（双击 Alt）依赖两次独立 down：抬起后再按不能被判成重复。
        for k in [Key::Alt, Key::Control, Key::Shift, Key::Meta, Key::CapsLock, Key::NumLock] {
            assert!(!detect_repeat(k), "{} 首次按下不是重复", key_name(k));
            note_key_up(k);
            assert!(!detect_repeat(k), "{} 抬起后再按不是重复", key_name(k));
            note_key_up(k);
        }
    }
}