//! Windows 低层键盘钩子引擎（`WH_KEYBOARD_LL`）。
//!
//! 职责：把系统键盘事件翻译成 [`KeyEvent`]，回调处理函数决定 [`Action`]，
//! 注入由 [`simulate`] 完成。钩子在独立线程跑消息循环；处理函数直接跑在
//! 回调里，不做跨线程调度，保证顺序与低延迟。
//!
//! 机制要点：
//! - 修饰键状态用 `GetKeyState` 实时读（事件键本身的方向手动修正，避开
//!   队列滞后）；自动重复根据同键 250ms 内再次 down 识别。
//! - `Block`/`Replace` 会登记 [`SWALLOWED`]，后续 keyup 一并吞掉，防止
//!   幽灵按键；`Replace` 保持"按下-抬起"配对（按住原键 = 按住目标键）。
//!   未吞掉的 keyup 会以观察者身份回调 handler（录制宏用），返回值被忽略。
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
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use kada_core::{Key, Modifier, Shortcut};

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
/// 上次 down 的键和时间，用于识别自动重复。
static LAST_DOWN: LazyLock<Mutex<Option<(Key, Instant)>>> = LazyLock::new(|| Mutex::new(None));

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
        // keyup：只吞掉之前登记的键；否则纯观察地通知 handler（录制用，
        // 返回值忽略，keyup 永远放行）。
        if SWALLOWED.lock().unwrap().remove(&key) {
            if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
                simulate::up(target);
            }
            return true;
        }
        if let Some(f) = HANDLER.lock().unwrap().as_mut() {
            let _ = f(KeyEvent::Up { key, mods: current_mods() });
        }
        return false;
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
        // keyup：只吞掉之前登记的键；否则纯观察地通知 handler（返回值忽略）。
        if SWALLOWED.lock().unwrap().remove(&key) {
            if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
                simulate::up(target);
            }
            return true;
        }
        if let Some(f) = HANDLER.lock().unwrap().as_mut() {
            let _ = f(KeyEvent::Up { key, mods: current_mods() });
        }
        return false;
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
    let mut last = LAST_DOWN.lock().unwrap();
    let is_repeat = matches!(last.as_ref(), Some((k, t)) if *k == key && t.elapsed() < Duration::from_millis(250));
    *last = Some((key, Instant::now()));
    // 修饰键 / 锁定键不产生自动重复：双击唤醒（如双击 Alt）依赖两次独立 down，
    // 快速连按不能被 250ms 窗口误判成 repeat，否则第二击被吞、唤醒失效。
    if key_as_modifier(key).is_some() || key == Key::CapsLock || key == Key::NumLock {
        return false;
    }
    is_repeat
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
    fn modifier_and_toggle_keys_are_never_repeat() {
        // 快速连按修饰键/锁定键不能被 250ms 窗口误判成自动重复：
        // 双击唤醒（如双击 Alt）需要两次独立 down 都送到 handler。
        for k in [Key::Alt, Key::Control, Key::Shift, Key::Meta, Key::CapsLock] {
            assert!(!detect_repeat(k), "{} 不应判为重复", key_name(k));
            assert!(!detect_repeat(k), "{} 第二次 down 也不应判为重复", key_name(k));
        }
    }
}