//! 平台钩子引擎：全局键盘事件监听 + 拦截。
//!
//! - Windows 后端：[`win`] 用 `SetWindowsHookEx(WH_KEYBOARD_LL)`，
//!   独立线程跑消息循环，回调内同步决定放行/吞掉/改键（`Action`），
//!   保证顺序与延迟（功耗低：幽灵按键没有）。
//! - Linux 后端：[`linux`] 用 evdev + uinput（内核输入层），X11 / Wayland
//!   桌面都能全局拦截与改键，需要 root 或 `input` 组授权 `/dev/input` 与
//!   `/dev/uinput`。
//! - macOS 后端：[`macos`] 用 `CGEventTap`（CoreGraphics 事件流），需要「辅助功能」
//!   权限；键码是 macOS 虚拟键码（ANSI 位置语义）。
//!
//! 事件统一为轻量的键盘事件，键模型在 [`kada_core`]。

#[cfg(windows)]
pub mod win;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;

pub use kada_core::{
    format_shortcut, key_name, matches, Key, Modifier, MouseButton, RawEvent, Shortcut,
};

/// 鼠标录制事件（鼠标操作录制的输入源，见壳体 `MouseRecorder`）：一次触发（点击 / 滚轮），
/// 附「自上一次触发以来的光标位移」。
///
/// 位移口径：Windows / macOS 取两次触发时光标位置之差（钩子事件自带坐标）；Linux 拿不到
/// 光标绝对位置，取两次触发之间累积的相对位移。三者对「移动到某处再触发」的语义一致，
/// 故壳层统一按增量处理。
#[derive(Clone, Copy, Debug)]
pub struct MouseRecord {
    /// 触发类型。
    pub kind: MouseRecordKind,
    /// 自上次触发以来的横向位移（像素）。
    pub move_dx: i32,
    /// 自上次触发以来的纵向位移（像素）。
    pub move_dy: i32,
}

/// 一次鼠标触发的类型。
#[derive(Clone, Copy, Debug)]
pub enum MouseRecordKind {
    /// 按下某个鼠标键（左/右/中/侧键，在按下那一刻记录）。
    Click(MouseButton),
    /// 滚轮滚动（`dy>0` 上滚 / `dy<0` 下滚，`dx` 横向，单位「格」）。
    Scroll { dx: i32, dy: i32 },
}