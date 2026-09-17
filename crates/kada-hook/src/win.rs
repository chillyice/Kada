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

use kada_core::{Key, Modifier};

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

/// 安装全局键盘钩子。处理函数在钩子线程回调内同步执行。
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
    let hhook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }
        .map_err(|e| io::Error::other(format!("SetWindowsHookEx 失败: {e}")))?;
    HOOK.store(hhook.0 as isize, Ordering::Relaxed);
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
        _ = UnhookWindowsHookEx(hhook);
    }
    HOOK.store(0, Ordering::Relaxed);
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
    let Some(key) = vk_to_key(VIRTUAL_KEY(kb.vkCode as u16)) else {
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
    is_repeat
}

/// 虚拟键码 ↔ [`Key`]。映射使用 US 布局语义，OEM 标点键的实际位置因
/// 键盘布局而异（中文键盘 Shift 后字符不同，但键码相同）。
pub fn key_to_vk(k: Key) -> Option<u16> {
    use Key::*;
    Some(match k {
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
    }.0)
}

/// 取 [`Key`] 名的可打印形式，未知键码返回 None（放行）。
fn vk_to_key(vk: VIRTUAL_KEY) -> Option<Key> {
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
        VK_OEM_COMMA => Comma, VK_OEM_PERIOD => Period, VK_OEM_2 => Slash,
        VK_OEM_5 => Backslash, VK_OEM_1 => Semicolon, VK_OEM_7 => Quote,
        VK_OEM_3 => Backquote, VK_OEM_MINUS => Minus, VK_OEM_PLUS => Equal,
        VK_OEM_4 => BracketLeft, VK_OEM_6 => BracketRight,
        VK_RETURN => Enter, VK_ESCAPE => Escape, VK_TAB => Tab, VK_SPACE => Space,
        VK_BACK => Backspace, VK_DELETE => Delete, VK_INSERT => Insert,
        VK_CAPITAL => CapsLock,
        VK_HOME => Home, VK_END => End, VK_PRIOR => PageUp, VK_NEXT => PageDown,
        VK_UP => ArrowUp, VK_DOWN => ArrowDown, VK_LEFT => ArrowLeft,
        VK_RIGHT => ArrowRight,
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
        ] {
            let vk = key_to_vk(k).unwrap();
            let name = key_name(k);
            assert_eq!(vk_to_key(VIRTUAL_KEY(vk)), Some(k), "vk roundtrip {name}");
        }
    }

    #[test]
    fn swallow_up_only_for_registered_keys() {
        // 未登记的 keyup 放行
        assert!(!swallow(WM_KEYUP, &KBDLLHOOKSTRUCT::default()));
    }
}