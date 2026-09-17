//! 平台钩子引擎：全局键盘事件监听 + 拦截。
//!
//! - Windows 后端：[`win`] 用 `SetWindowsHookEx(WH_KEYBOARD_LL)`，
//!   独立线程跑消息循环，回调内同步决定放行/吞掉/改键（`Action`），
//!   保证顺序与延迟（功耗低：幽灵按键没有）。
//! - Linux 后端（X11）M4 落地。
//!
//! 事件统一为轻量的键盘事件，键模型在 [`kada_core`]。

#[cfg(windows)]
pub mod win;

pub use kada_core::{format_shortcut, key_name, matches, Key, Modifier, RawEvent, Shortcut};