use crate::config::{Action, VolumeDir};
use std::time::Duration;
use windows::core::{w, PCWSTR};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub fn execute(actions: &[Action], notify: &dyn Fn(&str)) {
    for (i, action) in actions.iter().enumerate() {
        match action {
            Action::Keys { keys } => match parse_combo(keys) {
                Some((mods, key)) => send_combo(&mods, key),
                None => notify(&format!("无法识别按键：{keys}")),
            },
            Action::Run { cmd } => run(cmd, notify),
            Action::Volume { dir } => match dir {
                VolumeDir::Up => tap_key(0xAF),
                VolumeDir::Down => tap_key(0xAE),
                VolumeDir::Mute => tap_key(0xAD),
            },
            Action::Notify { text } => notify(text),
            Action::Click { button } => click(button, notify),
            Action::Scroll { dy } => scroll(*dy),
            Action::Move { dx, dy } => move_by(*dx, *dy),
            Action::Text { text } => type_text(text),
            Action::Sleep { ms } => std::thread::sleep(Duration::from_millis(*ms)),
        }
        if i + 1 < actions.len() {
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

fn key_input(vk: u16, flags: KEYBD_EVENT_FLAGS, scan: u16) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub const MOUSE_MARKER: usize = 0x4B50_0001;

fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MOUSE_MARKER,
            },
        },
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        let _ = SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// 手势没命中时把被吞掉的右键补回来。注入事件带 INJECTED 标记，不会被自己的钩子再吃。
pub fn replay_right_click() {
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_RIGHTDOWN)]);
    std::thread::sleep(Duration::from_millis(6));
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_RIGHTUP)]);
}

fn tap_key(vk: u16) {
    send(&[
        key_input(vk, KEYBD_EVENT_FLAGS(0), 0),
        key_input(vk, KEYEVENTF_KEYUP, 0),
    ]);
}

fn send_combo(mods: &[u16], key: u16) {
    let mut inputs = Vec::with_capacity(mods.len() * 2 + 2);
    for &m in mods {
        inputs.push(key_input(m, KEYBD_EVENT_FLAGS(0), 0));
    }
    inputs.push(key_input(key, KEYBD_EVENT_FLAGS(0), 0));
    inputs.push(key_input(key, KEYEVENTF_KEYUP, 0));
    for &m in mods.iter().rev() {
        inputs.push(key_input(m, KEYEVENTF_KEYUP, 0));
    }
    send(&inputs);
}

fn click(button: &str, notify: &dyn Fn(&str)) {
    let (down, up) = match button.to_ascii_lowercase().as_str() {
        "left" | "左" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "right" | "右" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "middle" | "中" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        other => {
            notify(&format!("未知鼠标键：{other}"));
            return;
        }
    };
    send(&[mouse_input(0, 0, 0, down)]);
    std::thread::sleep(Duration::from_millis(8));
    send(&[mouse_input(0, 0, 0, up)]);
}

fn scroll(dy: i32) {
    let data = (dy * 120) as u32;
    send(&[mouse_input(0, 0, data, MOUSEEVENTF_WHEEL)]);
}

fn move_by(dx: i32, dy: i32) {
    send(&[mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE)]);
}

fn type_text(text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    for chunk in units.chunks(64) {
        let mut inputs = Vec::with_capacity(chunk.len() * 2);
        for &u in chunk {
            inputs.push(key_input(0, KEYEVENTF_UNICODE, u));
            inputs.push(key_input(0, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP, u));
        }
        send(&inputs);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run(cmd: &str, notify: &dyn Fn(&str)) {
    let file = wide(cmd);
    let h = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(file.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if h.0 as isize <= 32 {
        notify(&format!("打开失败：{cmd}"));
    }
}

fn parse_combo(keys: &str) -> Option<(Vec<u16>, u16)> {
    let parts: Vec<&str> = keys.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let mut mods = Vec::new();
    let mut key = None;
    for part in parts {
        match vk_for(part) {
            Some(vk) if is_modifier(part) => mods.push(vk),
            Some(vk) => key = Some(vk),
            None => return None,
        }
    }
    key.map(|k| (mods, k))
}

fn is_modifier(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "ctrl" | "control" | "shift" | "alt" | "win" | "meta"
    )
}

fn vk_for(name: &str) -> Option<u16> {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    let vk = match lower.as_str() {
        "ctrl" | "control" => 0x11,
        "shift" => 0x10,
        "alt" => 0x12,
        "win" | "meta" => 0x5B,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "tab" => 0x09,
        "space" => 0x20,
        "backspace" | "bksp" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "volumeup" | "volup" => 0xAF,
        "volumedown" | "voldown" => 0xAE,
        "volumemute" | "volmute" | "mute" => 0xAD,
        "mediaplaypause" | "playpause" => 0xB3,
        "medianext" | "next" => 0xB0,
        "mediaprev" | "prev" => 0xB1,
        "printscreen" | "prtsc" => 0x2C,
        _ => {
            let chars: Vec<char> = n.chars().collect();
            if chars.len() == 1 {
                let c = chars[0].to_ascii_uppercase();
                if c.is_ascii_alphabetic() || c.is_ascii_digit() {
                    return Some(c as u16);
                }
            }
            if lower.starts_with('f') {
                if let Ok(n) = lower[1..].parse::<u16>() {
                    if (1..=24).contains(&n) {
                        return Some(0x6F + n);
                    }
                }
            }
            return None;
        }
    };
    Some(vk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_combos() {
        assert_eq!(parse_combo("Ctrl+Alt+K"), Some((vec![0x11, 0x12], 0x4B)));
        assert_eq!(parse_combo("Alt+Left"), Some((vec![0x12], 0x25)));
        assert_eq!(parse_combo("F9"), Some((vec![], 0x78)));
        assert_eq!(parse_combo("VolumeUp"), Some((vec![], 0xAF)));
        assert_eq!(parse_combo("Ctrl+Shift+Tab"), Some((vec![0x11, 0x10], 0x09)));
        assert_eq!(parse_combo("NotAKey"), None);
    }
}
