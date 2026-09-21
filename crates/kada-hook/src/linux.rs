//! Linux 键盘钩子（evdev + uinput）。
//!
//! 职责与 Windows 后端对齐：把键盘事件翻译成 [`KeyEvent`]，回调决定
//! [`Action`]，注入由 `simulate` 完成。
//!
//! 机制：
//! - 打开 `/dev/input/event*` 中的键盘设备并用 [EVIOCGRAB] 独占抓取：被
//!   抓设备的所有按键事件只到达我们，原事件不再进应用。
//! - 用 `/dev/uinput` 建一个虚拟键盘，把 [`Action`] 结果转发给系统。事件
//!   全部走我们的 uinput 设备，天然不存在"收到自己注入事件"的回环。
//! - 自动重复：物理键长按时内核产生 value=2 的事件，`Allow` 原样转发、
//!   `Replace` 按目标键转发，长按连发不丢。
//! - 修饰键状态按事件流维护（`MODS_DOWN`），吞键/改键不影响真实状态位，
//!   与 Windows 的 `GetKeyState` 语义对齐。
//!
//! 已知天花板（升级路径）：
//! - 抓取需要 root 或 `input` 组 + udev 放开 `/dev/uinput`；无权限时
//!   `start` 报错，能读到的其它键盘静默跳过。
//! - 热插拔不在监听列表里（重启应用即可）；Wayland 上同一套代码可用。

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::{AttributeSet, Device, EventType, InputEvent, KeyCode, VirtualDevice};

use kada_core::{Key, Modifier};

/// 一次键盘事件。
#[derive(Clone, Debug)]
pub enum KeyEvent {
    Down { key: Key, mods: BTreeSet<Modifier>, repeat: bool },
    Up { key: Key, mods: BTreeSet<Modifier> },
}

/// 处理函数对事件的处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 放行（把原键转发给系统）。
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
/// 物理按下的修饰键码。吞键/改键不动它（与 Windows 的 GetKeyState 对齐）。
static MODS_DOWN: LazyLock<Mutex<HashSet<u16>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// uinput 虚拟键盘：`start` 创建，事件转发与 simulate 共用。
static VDEV: LazyLock<Mutex<Option<VirtualDevice>>> = LazyLock::new(|| Mutex::new(None));

/// 钩子句柄。Drop 时停止钩子线程并回收。
pub struct HookHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
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

    // 抓取所有键盘设备；无权限/已被其它 grab 占用的静默跳过。
    let mut devices: Vec<Device> = Vec::new();
    for (_, mut dev) in evdev::enumerate() {
        if is_keyboard(&dev) && dev.grab().is_ok() {
            devices.push(dev);
        }
    }
    if devices.is_empty() {
        *HANDLER.lock().unwrap() = None;
        return Err(io::Error::other(
            "未能抓取任何键盘：需要 root，或加入 input 组并允许访问 /dev/uinput",
        ));
    }

    // 虚拟键盘：声明全部键盘键码，保证转发的任意键都被内核接受。
    let keys: AttributeSet<KeyCode> = (1..=0x2ff).map(KeyCode).collect();
    let vdev = VirtualDevice::builder()?
        .name("kada-virtual-keyboard")
        .with_keys(&keys)?
        .build()?;
    *VDEV.lock().unwrap() = Some(vdev);

    for dev in &devices {
        dev.set_nonblocking(true)?;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("kada-hook".into())
        .spawn(move || {
            if let Err(e) = poll_loop(&mut devices, &stop2) {
                eprintln!("kada-hook: {e}");
            }
        })?;
    Ok(HookHandle { stop, join: Some(join) })
}

fn poll_loop(devices: &mut Vec<Device>, stop: &AtomicBool) -> io::Result<()> {
    while !stop.load(Ordering::Relaxed) {
        let mut i = 0;
        while i < devices.len() {
            match devices[i].fetch_events() {
                Ok(events) => {
                    for ev in events {
                        if ev.event_type() == EventType::KEY {
                            handle(ev.code(), ev.value());
                        }
                    }
                    i += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => i += 1,
                Err(_) => {
                    devices.remove(i); // 设备拔出等
                }
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn handle(code: u16, value: i32) {
    let Some(key) = code_to_key(code) else {
        // 未知键（媒体键等）：原样转发，保持设备功能可用。
        forward_raw(code, value);
        return;
    };
    if value == 0 {
        release(code, key);
    } else {
        press(key, code, value);
    }
}

fn press(key: Key, code: u16, value: i32) {
    if key_as_modifier(key).is_some() {
        MODS_DOWN.lock().unwrap().insert(code);
    }
    let repeat = value == 2;
    let action = {
        let mut h = HANDLER.lock().unwrap();
        match h.as_mut() {
            Some(f) => f(KeyEvent::Down { key, mods: current_mods(key), repeat }),
            None => Action::Allow,
        }
    };
    match action {
        Action::Allow => forward_raw(code, value),
        Action::Block => {
            SWALLOWED.lock().unwrap().insert(key);
        }
        Action::Replace(target) => {
            SWALLOWED.lock().unwrap().insert(key);
            REPLACED_DOWN.lock().unwrap().insert(key, target);
            if let Some(tc) = key_to_code(target) {
                forward_raw(tc, value);
            }
        }
    }
}

fn release(code: u16, key: Key) {
    if key_as_modifier(key).is_some() {
        MODS_DOWN.lock().unwrap().remove(&code);
    }
    if SWALLOWED.lock().unwrap().remove(&key) {
        if let Some(target) = REPLACED_DOWN.lock().unwrap().remove(&key) {
            if let Some(tc) = key_to_code(target) {
                forward_raw(tc, 0);
            }
        }
        return; // 吞掉原键的 keyup，防止幽灵
    }
    if let Some(f) = HANDLER.lock().unwrap().as_mut() {
        let _ = f(KeyEvent::Up { key, mods: current_mods(key) });
    }
    forward_raw(code, 0);
}

/// 当前按下的修饰键集合；`event_key` 若是修饰键则包含其自身
/// （方向修正，与 Windows 钩子一致）。
fn current_mods(event_key: Key) -> BTreeSet<Modifier> {
    let mods_down = MODS_DOWN.lock().unwrap();
    let mut mods = BTreeSet::new();
    for &code in mods_down.iter() {
        if let Some(m) = code_modifier(code) {
            mods.insert(m);
        }
    }
    if let Some(m) = key_as_modifier(event_key) {
        mods.insert(m);
    }
    mods
}

/// evdev 修饰键码 → 通用修饰键（左右手合并）。
fn code_modifier(code: u16) -> Option<Modifier> {
    if code == KeyCode::KEY_LEFTSHIFT.0 || code == KeyCode::KEY_RIGHTSHIFT.0 {
        Some(Modifier::Shift)
    } else if code == KeyCode::KEY_LEFTCTRL.0 || code == KeyCode::KEY_RIGHTCTRL.0 {
        Some(Modifier::Ctrl)
    } else if code == KeyCode::KEY_LEFTALT.0 || code == KeyCode::KEY_RIGHTALT.0 {
        Some(Modifier::Alt)
    } else if code == KeyCode::KEY_LEFTMETA.0 || code == KeyCode::KEY_RIGHTMETA.0 {
        Some(Modifier::Meta)
    } else {
        None
    }
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

fn is_keyboard(dev: &Device) -> bool {
    let Some(keys) = dev.supported_keys() else {
        return false;
    };
    keys.contains(KeyCode::KEY_A) && keys.contains(KeyCode::KEY_Z)
}

fn forward_raw(code: u16, value: i32) {
    let mut v = VDEV.lock().unwrap();
    if let Some(vdev) = v.as_mut() {
        let _ = vdev.emit(&[InputEvent::new(EventType::KEY.0, code, value)]);
    }
}

/// [`Key`] → evdev 键码。通用修饰键归一到左键（与 Windows 的 VK_* 归一一致）。
pub fn key_to_code(k: Key) -> Option<u16> {
    use Key::*;
    Some(match k {
        A => KeyCode::KEY_A, B => KeyCode::KEY_B, C => KeyCode::KEY_C,
        D => KeyCode::KEY_D, E => KeyCode::KEY_E, F => KeyCode::KEY_F,
        G => KeyCode::KEY_G, H => KeyCode::KEY_H, I => KeyCode::KEY_I,
        J => KeyCode::KEY_J, K => KeyCode::KEY_K, L => KeyCode::KEY_L,
        M => KeyCode::KEY_M, N => KeyCode::KEY_N, O => KeyCode::KEY_O,
        P => KeyCode::KEY_P, Q => KeyCode::KEY_Q, R => KeyCode::KEY_R,
        S => KeyCode::KEY_S, T => KeyCode::KEY_T, U => KeyCode::KEY_U,
        V => KeyCode::KEY_V, W => KeyCode::KEY_W, X => KeyCode::KEY_X,
        Y => KeyCode::KEY_Y, Z => KeyCode::KEY_Z,
        Digit0 => KeyCode::KEY_0, Digit1 => KeyCode::KEY_1, Digit2 => KeyCode::KEY_2,
        Digit3 => KeyCode::KEY_3, Digit4 => KeyCode::KEY_4, Digit5 => KeyCode::KEY_5,
        Digit6 => KeyCode::KEY_6, Digit7 => KeyCode::KEY_7, Digit8 => KeyCode::KEY_8,
        Digit9 => KeyCode::KEY_9,
        F1 => KeyCode::KEY_F1, F2 => KeyCode::KEY_F2, F3 => KeyCode::KEY_F3,
        F4 => KeyCode::KEY_F4, F5 => KeyCode::KEY_F5, F6 => KeyCode::KEY_F6,
        F7 => KeyCode::KEY_F7, F8 => KeyCode::KEY_F8, F9 => KeyCode::KEY_F9,
        F10 => KeyCode::KEY_F10, F11 => KeyCode::KEY_F11, F12 => KeyCode::KEY_F12,
        Comma => KeyCode::KEY_COMMA, Period => KeyCode::KEY_DOT,
        Slash => KeyCode::KEY_SLASH, Backslash => KeyCode::KEY_BACKSLASH,
        Semicolon => KeyCode::KEY_SEMICOLON, Quote => KeyCode::KEY_APOSTROPHE,
        Backquote => KeyCode::KEY_GRAVE, Minus => KeyCode::KEY_MINUS,
        Equal => KeyCode::KEY_EQUAL, BracketLeft => KeyCode::KEY_LEFTBRACE,
        BracketRight => KeyCode::KEY_RIGHTBRACE,
        Enter => KeyCode::KEY_ENTER, Escape => KeyCode::KEY_ESC,
        Tab => KeyCode::KEY_TAB, Space => KeyCode::KEY_SPACE,
        Backspace => KeyCode::KEY_BACKSPACE, Delete => KeyCode::KEY_DELETE,
        Insert => KeyCode::KEY_INSERT,
        CapsLock => KeyCode::KEY_CAPSLOCK,
        Shift => KeyCode::KEY_LEFTSHIFT, Control => KeyCode::KEY_LEFTCTRL,
        Alt => KeyCode::KEY_LEFTALT, Meta => KeyCode::KEY_LEFTMETA,
        Home => KeyCode::KEY_HOME, End => KeyCode::KEY_END,
        PageUp => KeyCode::KEY_PAGEUP, PageDown => KeyCode::KEY_PAGEDOWN,
        ArrowUp => KeyCode::KEY_UP, ArrowDown => KeyCode::KEY_DOWN,
        ArrowLeft => KeyCode::KEY_LEFT, ArrowRight => KeyCode::KEY_RIGHT,
    }.0)
}

/// evdev 键码 → [`Key`]（左右修饰键归一）。
fn code_to_key(code: u16) -> Option<Key> {
    use Key::*;
    Some(match code {
        c if c == KeyCode::KEY_LEFTSHIFT.0 || c == KeyCode::KEY_RIGHTSHIFT.0 => Shift,
        c if c == KeyCode::KEY_LEFTCTRL.0 || c == KeyCode::KEY_RIGHTCTRL.0 => Control,
        c if c == KeyCode::KEY_LEFTALT.0 || c == KeyCode::KEY_RIGHTALT.0 => Alt,
        c if c == KeyCode::KEY_LEFTMETA.0 || c == KeyCode::KEY_RIGHTMETA.0 => Meta,
        c if c == KeyCode::KEY_A.0 => A, c if c == KeyCode::KEY_B.0 => B,
        c if c == KeyCode::KEY_C.0 => C, c if c == KeyCode::KEY_D.0 => D,
        c if c == KeyCode::KEY_E.0 => E, c if c == KeyCode::KEY_F.0 => F,
        c if c == KeyCode::KEY_G.0 => G, c if c == KeyCode::KEY_H.0 => H,
        c if c == KeyCode::KEY_I.0 => I, c if c == KeyCode::KEY_J.0 => J,
        c if c == KeyCode::KEY_K.0 => K, c if c == KeyCode::KEY_L.0 => L,
        c if c == KeyCode::KEY_M.0 => M, c if c == KeyCode::KEY_N.0 => N,
        c if c == KeyCode::KEY_O.0 => O, c if c == KeyCode::KEY_P.0 => P,
        c if c == KeyCode::KEY_Q.0 => Q, c if c == KeyCode::KEY_R.0 => R,
        c if c == KeyCode::KEY_S.0 => S, c if c == KeyCode::KEY_T.0 => T,
        c if c == KeyCode::KEY_U.0 => U, c if c == KeyCode::KEY_V.0 => V,
        c if c == KeyCode::KEY_W.0 => W, c if c == KeyCode::KEY_X.0 => X,
        c if c == KeyCode::KEY_Y.0 => Y, c if c == KeyCode::KEY_Z.0 => Z,
        c if c == KeyCode::KEY_0.0 => Digit0, c if c == KeyCode::KEY_1.0 => Digit1,
        c if c == KeyCode::KEY_2.0 => Digit2, c if c == KeyCode::KEY_3.0 => Digit3,
        c if c == KeyCode::KEY_4.0 => Digit4, c if c == KeyCode::KEY_5.0 => Digit5,
        c if c == KeyCode::KEY_6.0 => Digit6, c if c == KeyCode::KEY_7.0 => Digit7,
        c if c == KeyCode::KEY_8.0 => Digit8, c if c == KeyCode::KEY_9.0 => Digit9,
        c if c == KeyCode::KEY_F1.0 => F1, c if c == KeyCode::KEY_F2.0 => F2,
        c if c == KeyCode::KEY_F3.0 => F3, c if c == KeyCode::KEY_F4.0 => F4,
        c if c == KeyCode::KEY_F5.0 => F5, c if c == KeyCode::KEY_F6.0 => F6,
        c if c == KeyCode::KEY_F7.0 => F7, c if c == KeyCode::KEY_F8.0 => F8,
        c if c == KeyCode::KEY_F9.0 => F9, c if c == KeyCode::KEY_F10.0 => F10,
        c if c == KeyCode::KEY_F11.0 => F11, c if c == KeyCode::KEY_F12.0 => F12,
        c if c == KeyCode::KEY_COMMA.0 => Comma, c if c == KeyCode::KEY_DOT.0 => Period,
        c if c == KeyCode::KEY_SLASH.0 => Slash, c if c == KeyCode::KEY_BACKSLASH.0 => Backslash,
        c if c == KeyCode::KEY_SEMICOLON.0 => Semicolon,
        c if c == KeyCode::KEY_APOSTROPHE.0 => Quote,
        c if c == KeyCode::KEY_GRAVE.0 => Backquote, c if c == KeyCode::KEY_MINUS.0 => Minus,
        c if c == KeyCode::KEY_EQUAL.0 => Equal, c if c == KeyCode::KEY_LEFTBRACE.0 => BracketLeft,
        c if c == KeyCode::KEY_RIGHTBRACE.0 => BracketRight,
        c if c == KeyCode::KEY_ENTER.0 => Enter, c if c == KeyCode::KEY_ESC.0 => Escape,
        c if c == KeyCode::KEY_TAB.0 => Tab, c if c == KeyCode::KEY_SPACE.0 => Space,
        c if c == KeyCode::KEY_BACKSPACE.0 => Backspace, c if c == KeyCode::KEY_DELETE.0 => Delete,
        c if c == KeyCode::KEY_INSERT.0 => Insert,
        c if c == KeyCode::KEY_CAPSLOCK.0 => CapsLock,
        c if c == KeyCode::KEY_HOME.0 => Home, c if c == KeyCode::KEY_END.0 => End,
        c if c == KeyCode::KEY_PAGEUP.0 => PageUp, c if c == KeyCode::KEY_PAGEDOWN.0 => PageDown,
        c if c == KeyCode::KEY_UP.0 => ArrowUp, c if c == KeyCode::KEY_DOWN.0 => ArrowDown,
        c if c == KeyCode::KEY_LEFT.0 => ArrowLeft, c if c == KeyCode::KEY_RIGHT.0 => ArrowRight,
        _ => return None,
    })
}

/// 输入注入：按键/组合键/文本（与 Windows 后端同接口）。
pub mod simulate {
    //! 文本走剪贴板 + `Ctrl+V`（对中文等 Unicode 最稳，与 Windows 一致）；
    //! 副作用是短暂占用并恢复剪贴板。粘贴被吞的极端场景后续可加直发兜底。

    use std::io;

    use evdev::{EventType, InputEvent};

    use kada_core::Key;

    use super::{key_to_code, VDEV};

    pub fn down(k: Key) {
        emit(k, 1);
    }

    pub fn up(k: Key) {
        emit(k, 0);
    }

    pub fn tap(k: Key) {
        down(k);
        up(k);
    }

    pub fn chord(keys: &[Key]) {
        for k in keys {
            down(*k);
        }
        for k in keys.iter().rev() {
            up(*k);
        }
    }

    fn emit(k: Key, value: i32) {
        let Some(code) = key_to_code(k) else {
            return;
        };
        let mut v = VDEV.lock().unwrap();
        if let Some(vdev) = v.as_mut() {
            let _ = vdev.emit(&[InputEvent::new(EventType::KEY.0, code, value)]);
        }
    }

    /// 把文本粘贴到当前焦点控件。
    pub fn type_text(text: &str) -> io::Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        let prev = cb.get_text().ok();
        cb.set_text(text.to_string()).map_err(io::Error::other)?;
        chord(&[Key::Control, Key::V]);
        if let Some(p) = prev {
            let _ = cb.set_text(p);
        }
        Ok(())
    }

    /// 读取剪贴板文本（用于「转大小写」动作：复制选中 → 读剪贴板 → 转换 → 粘贴）。
    pub fn get_clipboard_text() -> io::Result<String> {
        let mut cb = arboard::Clipboard::new().map_err(io::Error::other)?;
        cb.get_text().map_err(io::Error::other)
    }
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
            Key::Backspace, Key::Comma, Key::Minus, Key::Slash, Key::BracketLeft,
            Key::Quote, Key::ArrowUp, Key::PageDown,
        ] {
            let code = key_to_code(k).unwrap();
            assert_eq!(code_to_key(code), Some(k), "roundtrip {}", key_name(k));
        }
    }

    #[test]
    fn modifier_left_right_merge() {
        assert_eq!(code_modifier(KeyCode::KEY_LEFTCTRL.0), Some(Modifier::Ctrl));
        assert_eq!(code_modifier(KeyCode::KEY_RIGHTCTRL.0), Some(Modifier::Ctrl));
        assert_eq!(code_modifier(KeyCode::KEY_RIGHTALT.0), Some(Modifier::Alt));
        assert_eq!(code_modifier(KeyCode::KEY_LEFTMETA.0), Some(Modifier::Meta));
        assert_eq!(code_modifier(KeyCode::KEY_BACKSPACE.0), None);
    }
}