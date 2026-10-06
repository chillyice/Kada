//! 输入注入：按键/组合键/文本。
//!
//! 文本有两种注入方式（规划 7.3-㉒，按 `Settings.text_inject_mode` 二选一）：
//! - [`type_text`]：剪贴板 + `Ctrl+V` 粘贴（默认，对中文等 Unicode 最稳）；副作用是
//!   短暂占用并恢复剪贴板。
//! - [`type_text_unicode`]：`KEYEVENTF_UNICODE` 逐字符直发，不经过剪贴板——目标程序
//!   吞粘贴（游戏 / 终端）时的兜底。

use std::io;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use kada_core::{Key, MouseButton, MouseOp};

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
    let input = match scan_for_key(k, vk) {
        // 有扫描码就走扫描码注入（布局感知，见 [`scan_for_key`]）。
        Some((scan, extended)) => scan_input(scan, extended, flags),
        // 媒体键等取不到扫描码，退回虚拟键码注入（与旧行为一致）。
        None => keyboard_input(vk, flags),
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

/// 取按键在**目标（前台）布局**下的扫描码与扩展位，供扫描码注入。
///
/// 注入走扫描码而非虚拟键码（`KEYEVENTF_SCANCODE`）是「布局感知」：把 VK 直接交给
/// `SendInput` 时扫描码由系统按某个线程的布局去猜；显式按**前台布局**换算成扫描码，
/// 目标程序再按同一布局还原，注入的物理键与触发键一致。同一布局下 VK↔扫描码本是双射，
/// 故这对单布局用户是等价改写（加固），跨布局的 `Key` 语义迁移另议（7.2-㉚）。
///
/// `MAPVK_VK_TO_VSC_EX` 返回值的低字节是扫描码，高字节对**部分**扩展键为 `0xE0`；方向键 /
/// 编辑键簇它不给 `0xE0`，按 [`is_extended_key`] 补。返回 `0`（该 VK 在当前布局未分配，
/// 如法语 `VK_OEM_MINUS`）或键属于 [`keeps_vk_injection`] 时，调用方退回虚拟键码注入。
fn scan_for_key(k: Key, vk: u16) -> Option<(u16, bool)> {
    // 主键区 Enter 与小键盘 Enter 共用 `VK_RETURN`，只有扫描码能区分（主键区 0x1C /
    // 小键盘 E0 0x1C）。这是唯一能在注入侧修正这对同码键的地方。
    if k == Key::NumpadEnter {
        return Some((0x1C, true));
    }
    // 媒体 / 音量键保留虚拟键码注入（见 [`keeps_vk_injection`]）。
    if keeps_vk_injection(k) {
        return None;
    }
    let packed = unsafe { MapVirtualKeyExW(vk as u32, MAPVK_VK_TO_VSC_EX, Some(target_layout())) };
    // 低字节 = 扫描码；扩展位 = API 给的高字节 `0xE0`，或对 API 漏标的键（方向/编辑）按
    // [`is_extended_key`] 补。`packed == 0` = 该 VK 在此布局未分配 → 无扫描码，退回 VK。
    (packed != 0).then(|| ((packed & 0xFF) as u16, packed & 0xFF00 != 0 || is_extended_key(k)))
}

/// 媒体 / 音量键：`MapVirtualKeyEx` 也能给出 ACPI 扫描码（如 `E0 22`），但这类键的
/// **虚拟键码注入**才是已知可用路径，显式保留，不做扫描码换算（`Return None`）。
fn keeps_vk_injection(k: Key) -> bool {
    use Key::*;
    matches!(
        k,
        MediaPlayPause | MediaPrev | MediaNext | VolumeMute | VolumeDown | VolumeUp
    )
}

/// 扩展键（`E0` 前缀）判定。`MapVirtualKeyEx(MAPVK_VK_TO_VSC_EX)` 只对部分扩展键
/// （小键盘除号、Win 键等）返回 `0xE0`，方向键 / 编辑键簇它**返回的是不带 `E0` 的基准
/// 扫描码**——漏掉扩展位会把「方向键」注入成「小键盘数字」（NumLock 开时打出数字）。
fn is_extended_key(k: Key) -> bool {
    use Key::*;
    matches!(
        k,
        Insert | Delete | Home | End | PageUp | PageDown
            | ArrowUp | ArrowDown | ArrowLeft | ArrowRight
            | NumpadDivide | NumpadEnter | Meta
    )
}

/// 前台窗口的键盘布局（注入目标所用的布局）。取不到时退回本线程布局。
///
/// 键盘布局是**按线程**的：钩子线程、动作线程各自创建时的布局可能过期，唯独前台窗口
/// 的线程布局就是用户此刻在用的那个。
fn target_layout() -> HKL {
    // 一级：前台窗口所属线程的布局（= 用户此刻在用的那个）。
    let fg = unsafe { GetForegroundWindow() };
    if !fg.0.is_null() {
        let tid = unsafe { GetWindowThreadProcessId(fg, None) };
        if tid != 0 {
            let hkl = unsafe { GetKeyboardLayout(tid) };
            if !hkl.0.is_null() {
                return hkl;
            }
        }
    }
    // 二级：本线程布局（无前台窗口 / 查询失败时的兜底）。
    unsafe { GetKeyboardLayout(0) }
}

/// 一个扫描码键盘事件（`KEYEVENTF_SCANCODE`）：`wVk=0`、`wScan=扫描码`，扩展键另带
/// `KEYEVENTF_EXTENDEDKEY`。目标程序按自身（前台）布局把扫描码还原成虚拟键码 / 字符。
fn scan_input(scan: u16, extended: bool, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    let mut flags = flags | KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0), // 扫描码通道下 wVk 被忽略，置 0
                wScan: scan,
                time: 0,
                dwExtraInfo: 0,
                dwFlags: flags,
            },
        },
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

/// 鼠标模拟：移动 / 点击 / 滚轮（[`kada_core::MouseOp`]，7.1-㊱）。
/// 移动与滚轮都是**相对**量；点击作用在当前光标位置。
pub fn mouse(op: &MouseOp) -> io::Result<()> {
    match op {
        MouseOp::Move { dx, dy } => send_mouse(mouse_input(*dx, *dy, 0, MOUSEEVENTF_MOVE))?,
        MouseOp::Scroll { dx, dy } => {
            // WHEEL_DELTA = 120：mouseData 是「格数 × 120」的带符号值（负数按补码传）。
            if *dy != 0 {
                send_mouse(mouse_input(0, 0, wheel_data(*dy), MOUSEEVENTF_WHEEL))?;
            }
            if *dx != 0 {
                send_mouse(mouse_input(0, 0, wheel_data(*dx), MOUSEEVENTF_HWHEEL))?;
            }
        }
        MouseOp::Click { button } => {
            let (down, up) = match button {
                MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
                MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
                MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            };
            send_mouse(mouse_input(0, 0, 0, down))?;
            send_mouse(mouse_input(0, 0, 0, up))?;
        }
    }
    Ok(())
}

/// 滚轮格数 → `mouseData`（带符号值按 u32 补码，`SendInput` 会原样读回 i32）。
fn wheel_data(delta: i32) -> u32 {
    delta.wrapping_mul(120) as u32
}

/// 构造一个鼠标事件（纯函数，可单测）。
fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send_mouse(input: INPUT) -> io::Result<()> {
    let sent = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        return Err(io::Error::other(
            "SendInput 鼠标事件被系统拦截（目标可能处于安全桌面 / 提权窗口）",
        ));
    }
    Ok(())
}

/// 把文本粘贴到当前焦点控件（剪贴板方式，默认）。
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

/// 把文本逐字符直发到当前焦点控件（`KEYEVENTF_UNICODE`，不经过剪贴板）。
///
/// 目标程序吞剪贴板粘贴（游戏 / 终端）时的兜底（规划 7.3-㉒）：每个 UTF-16 码元合成
/// 一对按下/抬起事件（`wVk=0`、`wScan=码元`），目标程序直接收到字符、不经过键盘布局，
/// 物理修饰键（Ctrl/Alt 等）污染不了它，也就无需先等修饰键释放。BMP 外字符（emoji 等）
/// 的代理对按两个码元连发，由目标程序自行拼回。全部事件一次 `SendInput` 送入；被安全
/// 桌面 / UIPI 拦下（返回数不足）按失败上报——粘贴路径遇拦截只会「贴不出」，这里能
/// 把失败讲出来。
pub fn type_text_unicode(text: &str) -> io::Result<()> {
    let events = unicode_key_events(text);
    if events.is_empty() {
        return Ok(());
    }
    let len = events.len();
    let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != len {
        return Err(io::Error::other(format!(
            "SendInput 只送出 {sent}/{len} 个字符事件（可能被安全桌面 / 提权窗口拦截）"
        )));
    }
    Ok(())
}

/// 纯事件构造（可单测）：文本 → `KEYEVENTF_UNICODE` 键盘事件序列（按下+抬起成对）。
fn unicode_key_events(text: &str) -> Vec<INPUT> {
    let mut events: Vec<INPUT> = Vec::new();
    for unit in text.encode_utf16() {
        match unit {
            // \r\n 的 \r 跳过：回车由 \n 发，两遍会打两次回车。
            0x0D => {}
            // 换行 / 制表发真实虚拟键：多数程序不认 Unicode 控制码（0x0A/0x0D）。
            0x0A | 0x09 => {
                let vk = key_to_vk(if unit == 0x0A { Key::Enter } else { Key::Tab });
                if let Some(vk) = vk {
                    events.push(keyboard_input(vk, KEYBD_EVENT_FLAGS(0)));
                    events.push(keyboard_input(vk, KEYEVENTF_KEYUP));
                }
            }
            u => {
                events.push(unicode_input(u, KEYEVENTF_UNICODE));
                events.push(unicode_input(u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
            }
        }
    }
    events
}

/// 一个 `KEYEVENTF_UNICODE` 字符事件（`wVk=0`，`wScan=UTF-16 码元`）。
fn unicode_input(unit: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                time: 0,
                dwExtraInfo: 0,
                dwFlags: flags,
            },
        },
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 取事件关键字段（INPUT 不支持 PartialEq，借联合体读字段拼视图）。
    fn key_of(input: &INPUT) -> (u16, u16, KEYBD_EVENT_FLAGS) {
        let ki = unsafe { input.Anonymous.ki };
        (ki.wVk.0, ki.wScan, ki.dwFlags)
    }

    #[test]
    fn unicode_events_shape() {
        // 普通字符：按下+抬起成对，wVk=0、wScan=UTF-16 码元。
        let ev = unicode_key_events("a");
        assert_eq!(ev.len(), 2);
        assert_eq!(key_of(&ev[0]), (0, 'a' as u16, KEYEVENTF_UNICODE));
        assert_eq!(key_of(&ev[1]), (0, 'a' as u16, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));

        // BMP 外字符（emoji）：按两个代理码元连发，由目标程序拼回。
        let ev = unicode_key_events("\u{1F600}");
        assert_eq!(ev.len(), 4);
        assert_eq!(key_of(&ev[0]).1, 0xD83D);
        assert_eq!(key_of(&ev[2]).1, 0xDE00);

        // \n / \t 发真实虚拟键（wScan=0），\r 跳过（\r\n 只打一次回车）。
        let (vk_enter, vk_tab) = (key_to_vk(Key::Enter).unwrap(), key_to_vk(Key::Tab).unwrap());
        let ev = unicode_key_events("x\r\ny\t");
        assert_eq!(ev.len(), 8, "x(2) + 回车(2) + y(2) + Tab(2)");
        assert_eq!(key_of(&ev[2]), (vk_enter, 0, KEYBD_EVENT_FLAGS(0)));
        assert_eq!(key_of(&ev[3]), (vk_enter, 0, KEYEVENTF_KEYUP));
        assert_eq!(key_of(&ev[6]), (vk_tab, 0, KEYBD_EVENT_FLAGS(0)));

        // 空文本不产生事件（type_text_unicode 据此直接成功返回）。
        assert!(unicode_key_events("").is_empty());
    }

    /// 扫描码注入（7.2-㉚）：`wVk=0`、`wScan=扫描码`、恒带 `KEYEVENTF_SCANCODE`；
    /// 扩展键另带 `KEYEVENTF_EXTENDEDKEY`，抬起位原样保留。
    #[test]
    fn scan_injection_uses_scan_code() {
        let e = scan_input(0x1C, false, KEYBD_EVENT_FLAGS(0));
        let (vk, scan, flags) = key_of(&e);
        assert_eq!((vk, scan), (0, 0x1C));
        assert_ne!(flags.0 & KEYEVENTF_SCANCODE.0, 0, "走扫描码通道");
        assert_eq!(flags.0 & KEYEVENTF_EXTENDEDKEY.0, 0, "非扩展键不带扩展位");

        let e = scan_input(0x48, true, KEYEVENTF_KEYUP);
        let (vk, scan, flags) = key_of(&e);
        assert_eq!((vk, scan), (0, 0x48));
        assert_ne!(flags.0 & KEYEVENTF_SCANCODE.0, 0);
        assert_ne!(flags.0 & KEYEVENTF_EXTENDEDKEY.0, 0, "扩展键带扩展位");
        assert_ne!(flags.0 & KEYEVENTF_KEYUP.0, 0, "抬起位保留");
    }

    /// 主键区 / 小键盘 Enter 共用 `VK_RETURN`，只有扫描码能区分——注入侧据此修正。
    #[test]
    fn numpad_enter_keeps_extended_scan() {
        assert_eq!(scan_for_key(Key::NumpadEnter, 0x0D), Some((0x1C, true)));
    }

    /// 媒体 / 音量键保留虚拟键码注入（`scan_for_key` 直接返回 `None`）。
    #[test]
    fn media_keys_keep_vk_injection() {
        assert_eq!(scan_for_key(Key::MediaPlayPause, 0xB3), None);
        assert!(keeps_vk_injection(Key::VolumeUp));
        assert!(!keeps_vk_injection(Key::A));
    }

    /// 扩展键判定：方向键 / 编辑键簇 / 小键盘除号 / Win 键走扩展位，普通键不走。
    /// （`MAPVK_VK_TO_VSC_EX` 不给方向键 `E0`，靠这里补，否则注入成小键盘数字。）
    #[test]
    fn extended_key_set_matches_scan_quirks() {
        for k in
            [Key::ArrowUp, Key::Insert, Key::Home, Key::PageDown, Key::NumpadDivide, Key::Meta]
        {
            assert!(is_extended_key(k), "{k:?} 应判为扩展键");
        }
        for k in [Key::A, Key::Digit0, Key::Semicolon, Key::Enter, Key::Numpad0, Key::Shift] {
            assert!(!is_extended_key(k), "{k:?} 不应判为扩展键");
        }
    }

    /// 诊断用：打印本机若干键在前台布局下的 VK→扫描码换算（真机验证非 US 布局）。
    /// 跑法：`cargo test -p kada-hook --lib probe_scan_codes -- --ignored --nocapture`。
    #[test]
    #[ignore = "诊断用：需在目标键盘布局下人工跑，观察扫描码换算"]
    fn probe_scan_codes() {
        for k in [
            Key::Semicolon, Key::Slash, Key::Minus, Key::A, Key::Q,
            Key::NumpadEnter, Key::NumpadDivide, Key::ArrowUp, Key::MediaPlayPause,
        ] {
            let scan = key_to_vk(k).and_then(|vk| scan_for_key(k, vk));
            println!("{k:?} -> vk={:?} scan={scan:?}", key_to_vk(k));
        }
    }

    /// 鼠标事件构造（7.1-㊱）：移动带 `MOUSEEVENTF_MOVE` + 相对位移；滚轮 `mouseData`
    /// 是「格数 × 120」的带符号补码（向下滚为负）。
    #[test]
    fn mouse_event_shape() {
        let mi = |i: &INPUT| unsafe { i.Anonymous.mi };
        let mv = mi(&mouse_input(15, -8, 0, MOUSEEVENTF_MOVE));
        assert_eq!((mv.dx, mv.dy), (15, -8));
        assert_eq!(mv.dwFlags, MOUSEEVENTF_MOVE);

        assert_eq!(wheel_data(1), 120);
        assert_eq!(wheel_data(-1) as i32, -120, "向下滚是负值（补码）");
        let sc = mi(&mouse_input(0, 0, wheel_data(-3), MOUSEEVENTF_WHEEL));
        assert_eq!(sc.mouseData as i32, -360);
    }
}