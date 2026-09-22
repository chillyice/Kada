//! 输入注入：按键/组合键/文本。
//!
//! 文本走剪贴板 + `Ctrl+V`（对中文等 Unicode 最稳）；副作用是短暂占用并
//! 恢复剪贴板。`// ponytail: 若目标程序吞粘贴（游戏/终端），后续可用
//! KEYEVENTF_UNICODE 直发兜底，两者加开关。

use std::io;

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use kada_core::Key;

use crate::win::key_to_vk;

pub fn down(k: Key) {
    if is_mouse_button(k) {
        mouse_button(k, false);
    } else {
        send_one(k, KEYBD_EVENT_FLAGS(0));
    }
}

pub fn up(k: Key) {
    if is_mouse_button(k) {
        mouse_button(k, true);
    } else {
        send_one(k, KEYEVENTF_KEYUP);
    }
}

pub fn tap(k: Key) {
    down(k);
    up(k);
}

fn send_one(k: Key, flags: KEYBD_EVENT_FLAGS) {
    let Some(vk) = key_to_vk(k) else { return };
    let input = keyboard_input(vk, flags);
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 鼠标键（中键/侧键）不是虚拟键码，注入走 `SendInput` 的鼠标事件。
fn is_mouse_button(k: Key) -> bool {
    matches!(k, Key::MouseMiddle | Key::MouseBack | Key::MouseForward)
}

/// 发送鼠标按钮按下/抬起。`mouseData` 高 16 位为 X 按钮号（1=MB4/后退，2=MB5/前进）。
fn mouse_button(k: Key, up: bool) {
    let (flags, x_button) = match k {
        Key::MouseMiddle => (
            if up { MOUSEEVENTF_MIDDLEUP } else { MOUSEEVENTF_MIDDLEDOWN },
            0u32,
        ),
        Key::MouseBack => (
            if up { MOUSEEVENTF_XUP } else { MOUSEEVENTF_XDOWN },
            1u32,
        ),
        Key::MouseForward => (
            if up { MOUSEEVENTF_XUP } else { MOUSEEVENTF_XDOWN },
            2u32,
        ),
        _ => return,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: x_button << 16,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

pub fn chord(keys: &[Key]) {
    for k in keys {
        down(*k);
    }
    for k in keys.iter().rev() {
        up(*k);
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

fn keyboard_input(w_vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(w_vk),
                wScan: 0,
                time: 0,
                dwExtraInfo: 0,
                dwFlags: flags,
            },
        },
    }
}