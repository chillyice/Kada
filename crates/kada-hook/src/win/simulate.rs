//! 输入注入：按键/组合键/文本。
//!
//! 文本走剪贴板 + `Ctrl+V`（对中文等 Unicode 最稳）；副作用是短暂占用并
//! 恢复剪贴板。`// ponytail: 若目标程序吞粘贴（游戏/终端），后续可用
//! KEYEVENTF_UNICODE 直发兜底，两者加开关。

use std::io;
use std::time::Duration;

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
    // 全部按下后稍停再逆序松开，避免组合键太快导致目标程序收不到。
    std::thread::sleep(Duration::from_millis(30));
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
    // 等目标程序处理完粘贴再恢复剪贴板：否则恢复太快，目标读到的是旧剪贴板
    // 内容（竞态），导致「按下快捷键却输不出文本」。
    std::thread::sleep(Duration::from_millis(80));
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

/// 等待物理按下的修饰键全部释放（最多 `timeout_ms` 毫秒）。
///
/// 触发带修饰键的快捷键（如 `Ctrl+Alt+T`）时，Ctrl/Alt 仍被物理按住；若此时直接注入
/// 文本/组合键，目标程序会把注入的键当成 `Ctrl+Alt+<键>`（粘贴 Ctrl+V 变成 Ctrl+Alt+V），
/// 导致「按下快捷键却输不出文本」。这里轮询 `GetKeyState` 直到修饰键全部抬起，
/// 返回是否在时限内等到（超时返回 false，调用方仍继续执行）。
pub fn wait_modifiers_released(timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        let held = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
            .iter()
            .any(|vk| unsafe { GetKeyState(vk.0 as i32) < 0 });
        if !held {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
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