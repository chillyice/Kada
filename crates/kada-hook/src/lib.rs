//! 平台钩子引擎：全局键盘事件监听 + 拦截。
//!
//! - Windows 后端：[`win`] 用 `SetWindowsHookEx(WH_KEYBOARD_LL)`，
//!   独立线程跑消息循环，回调内同步决定放行/吞掉/改键（`Action`），
//!   保证顺序与延迟（功耗低：幽灵按键没有）。
//! - Linux 后端：[`linux`] 用 evdev + uinput（内核输入层），X11 / Wayland
//!   桌面都能全局拦截与改键，需要 root 或 `input` 组授权 `/dev/input` 与
//!   `/dev/uinput`。
//!
//! 事件统一为轻量的键盘事件，键模型在 [`kada_core`]。

#[cfg(windows)]
pub mod win;

#[cfg(target_os = "linux")]
pub mod linux;

pub use kada_core::{format_shortcut, key_name, matches, Key, Modifier, RawEvent, Shortcut};