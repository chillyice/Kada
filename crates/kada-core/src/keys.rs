// 键模型（C-02 从 lib.rs 搬出）：修饰键 / 按键枚举 / 快捷键解析与匹配。
//
// 平台层只产出 [`RawEvent`]，由这里的 [`matches`] 统一判定触发。

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;
/// 修饰键（跨平台中性名；macOS 的 Cmd 即 Meta）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Modifier {
    Ctrl,
    Alt,
    Shift,
    Meta,
}

/// 用一个宏从**同一份清单**同时定义枚举与 [`key_name`]，保证两者不会漂移；顺带给出
/// [`Key::ALL`]（前端全键表一致性校验、任何「遍历所有键」的需求都据它）。
macro_rules! keys {
    ($($variant:ident => $name:literal),* $(,)?) => {
        /// 单个按键。与具体 OS 键码无关，由平台层映射进来。
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        pub enum Key { $($variant),* }

        impl Key {
            /// 全部按键，按声明顺序（与 [`key_name`] 同源，不可能漏项）。
            pub const ALL: &'static [Key] = &[$(Key::$variant),*];
        }

        /// 键的中性名（显示与配置落盘用）。
        pub fn key_name(k: Key) -> &'static str {
            match k { $(Key::$variant => $name),* }
        }
    };
}

keys! {
    A => "A", B => "B", C => "C", D => "D", E => "E", F => "F", G => "G", H => "H",
    I => "I", J => "J", K => "K", L => "L", M => "M", N => "N", O => "O", P => "P",
    Q => "Q", R => "R", S => "S", T => "T", U => "U", V => "V", W => "W", X => "X",
    Y => "Y", Z => "Z",
    Digit0 => "0", Digit1 => "1", Digit2 => "2", Digit3 => "3", Digit4 => "4",
    Digit5 => "5", Digit6 => "6", Digit7 => "7", Digit8 => "8", Digit9 => "9",
    F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6",
    F7 => "F7", F8 => "F8", F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
    Comma => ",", Period => ".", Slash => "/", Backslash => "\\",
    Semicolon => ";", Quote => "\"", Backquote => "`",
    Minus => "-", Equal => "=", BracketLeft => "[", BracketRight => "]",
    Enter => "Enter", Escape => "Esc", Tab => "Tab", Space => "Space",
    Backspace => "Backspace", Delete => "Delete", Insert => "Insert", CapsLock => "CapsLock",
    Shift => "Shift", Control => "Ctrl", Alt => "Alt", Meta => "Meta",
    Home => "Home", End => "End", PageUp => "PageUp", PageDown => "PageDown",
    ArrowUp => "Up", ArrowDown => "Down", ArrowLeft => "Left", ArrowRight => "Right",
    F13 => "F13", F14 => "F14", F15 => "F15", F16 => "F16", F17 => "F17", F18 => "F18",
    F19 => "F19", F20 => "F20", F21 => "F21", F22 => "F22", F23 => "F23", F24 => "F24",
    // 媒体键（音量/播放控制）。
    MediaPlayPause => "MediaPlayPause", MediaPrev => "MediaPrev", MediaNext => "MediaNext",
    VolumeMute => "VolumeMute", VolumeDown => "VolumeDown", VolumeUp => "VolumeUp",
    // 小键盘数字区 + NumLock（独立映射，与主键区数字键区分）。
    Numpad0 => "Numpad0", Numpad1 => "Numpad1", Numpad2 => "Numpad2", Numpad3 => "Numpad3",
    Numpad4 => "Numpad4", Numpad5 => "Numpad5", Numpad6 => "Numpad6", Numpad7 => "Numpad7",
    Numpad8 => "Numpad8", Numpad9 => "Numpad9",
    NumpadAdd => "NumpadAdd", NumpadSubtract => "NumpadSubtract",
    NumpadMultiply => "NumpadMultiply", NumpadDivide => "NumpadDivide",
    NumpadDecimal => "NumpadDecimal", NumpadEnter => "NumpadEnter", NumLock => "NumLock",
    // 鼠标键（中键 / 侧键 / 滚轮上下一格，作为触发键与改键目标；左右键不纳入，
    // 避免全局误拦截点击）。滚轮按「一格 = 一次按下 + 抬起」当离散键（7.2-MG 余项①）。
    MouseMiddle => "MouseMiddle", MouseBack => "MouseBack", MouseForward => "MouseForward",
    WheelUp => "WheelUp", WheelDown => "WheelDown",
}

impl Key {
    /// 是否为修饰键（`Shift`/`Control`/`Alt`/`Meta`）。修饰键的「按住」由 mods 状态跟踪，
    /// 而非「普通键按下集合」——所以它**从不被吞、也不进按住集合**（和弦成员的修饰要求、
    /// 注入修饰的 mods 补全都按这个区分，见 [`Key::as_modifier`]）。
    pub fn is_modifier(&self) -> bool {
        matches!(self, Key::Shift | Key::Control | Key::Alt | Key::Meta)
    }

    /// `Key` 是裸修饰键时给出对应修饰位（`Ctrl`/`Alt`/`Shift`/`Meta`），否则 `None`。
    /// 与 [`modifier_key`] 互逆。
    pub fn as_modifier(self) -> Option<Modifier> {
        match self {
            Key::Control => Some(Modifier::Ctrl),
            Key::Alt => Some(Modifier::Alt),
            Key::Shift => Some(Modifier::Shift),
            Key::Meta => Some(Modifier::Meta),
            _ => None,
        }
    }

    /// 是不是鼠标键（中键 / 侧键 / 滚轮）：它们走鼠标钩子，不是键盘钩子。
    /// 触摸板手势的触发键只接受键盘键（见 `sanitize_config`）。
    pub fn is_mouse(self) -> bool {
        matches!(
            self,
            Key::MouseMiddle | Key::MouseBack | Key::MouseForward | Key::WheelUp | Key::WheelDown
        )
    }
}

/// `Modifier` → 裸修饰键 `Key`（与 [`Key::as_modifier`] 互逆）。
pub fn modifier_key(m: Modifier) -> Key {
    match m {
        Modifier::Ctrl => Key::Control,
        Modifier::Alt => Key::Alt,
        Modifier::Shift => Key::Shift,
        Modifier::Meta => Key::Meta,
    }
}

/// 一次键盘事件（钩子层产出）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawEvent {
    pub key: Key,
    pub mods: BTreeSet<Modifier>,
    pub pressed: bool,
}

/// 一个快捷键组合。`triggers` 里的任一组命中即触发。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Shortcut {
    pub mods: BTreeSet<Modifier>,
    pub key: Key,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseError {
    Empty,
    UnknownModifier(String),
    UnknownKey(String),
    /// 和弦触发键非法（成员为空 / 成员不足 / 成员含修饰键）。
    ChordInvalid(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "快捷键为空"),
            ParseError::UnknownModifier(s) => write!(f, "未知修饰键: {s}"),
            ParseError::UnknownKey(s) => write!(f, "未知按键: {s}"),
            ParseError::ChordInvalid(s) => write!(f, "和弦触发键无效: {s}"),
        }
    }
}

impl std::error::Error for ParseError {}

pub(crate) mod parse {
    use super::*;

    /// 解析 "Ctrl+Alt+K" 形式的字符串。
    /// 单独的修饰键名（"Shift"）按按键处理，供改键配置使用。
    pub fn shortcut(s: &str) -> Result<Shortcut, ParseError> {
        let parts: Vec<&str> = s
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            return Err(ParseError::Empty);
        }
        if parts.len() == 1 {
            return Ok(Shortcut {
                mods: BTreeSet::new(),
                key: key(parts[0])?,
            });
        }
        let mut mods = BTreeSet::new();
        let mut main_key = None;
        for part in parts {
            if let Some(m) = modifier(part) {
                mods.insert(m);
            } else if main_key.is_none() {
                main_key = Some(key(part)?);
            } else {
                return Err(ParseError::UnknownKey(part.to_string()));
            }
        }
        let Some(main_key) = main_key else {
            return Err(ParseError::Empty);
        };
        Ok(Shortcut {
            mods,
            key: main_key,
        })
    }

    /// 修饰键名 → `Modifier`（`Ctrl`/`Control`/`Cmd`/`Win`、`Alt`/`Option`、`Shift`、
    /// `Meta`/`Super`）。和弦成员解析（`parse_chord`）也要用它，故对 crate 根可见。
    pub(crate) fn modifier(s: &str) -> Option<Modifier> {
        match s {
            "Ctrl" | "Control" | "Cmd" | "Win" => Some(Modifier::Ctrl),
            "Alt" | "Option" => Some(Modifier::Alt),
            "Shift" => Some(Modifier::Shift),
            "Meta" | "Super" => Some(Modifier::Meta),
            _ => None,
        }
    }

    /// 单个键名（"K"、"F5"、"Shift"、"CapsLock"……）。
    pub fn key(s: &str) -> Result<Key, ParseError> {
        use Key::*;
        Ok(match s {
            "Shift" => Shift,
            "Ctrl" | "Control" => Control,
            "Alt" => Alt,
            "Meta" | "Super" => Meta,
            "Enter" => Enter,
            "Esc" | "Escape" => Escape,
            "Tab" => Tab,
            "Space" => Space,
            "Backspace" => Backspace,
            "Delete" => Delete,
            "Insert" => Insert,
            "CapsLock" | "Caps" => CapsLock,
            "Home" => Home,
            "End" => End,
            "PageUp" | "PgUp" => PageUp,
            "PageDown" | "PgDn" => PageDown,
            "Up" => ArrowUp,
            "Down" => ArrowDown,
            "Left" => ArrowLeft,
            "Right" => ArrowRight,
            "F1" => F1,
            "F2" => F2,
            "F3" => F3,
            "F4" => F4,
            "F5" => F5,
            "F6" => F6,
            "F7" => F7,
            "F8" => F8,
            "F9" => F9,
            "F10" => F10,
            "F11" => F11,
            "F12" => F12,
            "F13" => F13,
            "F14" => F14,
            "F15" => F15,
            "F16" => F16,
            "F17" => F17,
            "F18" => F18,
            "F19" => F19,
            "F20" => F20,
            "F21" => F21,
            "F22" => F22,
            "F23" => F23,
            "F24" => F24,
            "," => Comma,
            "." => Period,
            "/" => Slash,
            "\\" => Backslash,
            ";" => Semicolon,
            "\"" => Quote,
            "`" => Backquote,
            "-" => Minus,
            "=" => Equal,
            "[" => BracketLeft,
            "]" => BracketRight,
            "MediaPlayPause" | "PlayPause" => MediaPlayPause,
            "MediaPrev" | "PrevTrack" => MediaPrev,
            "MediaNext" | "NextTrack" => MediaNext,
            "VolumeMute" | "Mute" => VolumeMute,
            "VolumeDown" | "VolDown" => VolumeDown,
            "VolumeUp" | "VolUp" => VolumeUp,
            "NumLock" => NumLock,
            "Numpad0" => Numpad0,
            "Numpad1" => Numpad1,
            "Numpad2" => Numpad2,
            "Numpad3" => Numpad3,
            "Numpad4" => Numpad4,
            "Numpad5" => Numpad5,
            "Numpad6" => Numpad6,
            "Numpad7" => Numpad7,
            "Numpad8" => Numpad8,
            "Numpad9" => Numpad9,
            "NumpadAdd" | "NumpadPlus" => NumpadAdd,
            "NumpadSubtract" | "NumpadMinus" => NumpadSubtract,
            "NumpadMultiply" | "NumpadStar" => NumpadMultiply,
            "NumpadDivide" | "NumpadSlash" => NumpadDivide,
            "NumpadDecimal" | "NumpadDot" => NumpadDecimal,
            "NumpadEnter" => NumpadEnter,
            "MouseMiddle" | "MB3" | "Middle" => MouseMiddle,
            "MouseBack" | "MB4" | "XButton1" | "X1" => MouseBack,
            "MouseForward" | "MB5" | "XButton2" | "X2" => MouseForward,
            "WheelUp" | "ScrollUp" | "WheelPlus" => WheelUp,
            "WheelDown" | "ScrollDown" | "WheelMinus" => WheelDown,
            _ => letter_or_digit(s).ok_or(ParseError::UnknownKey(s.to_string()))?,
        })
    }

    /// 单个 ASCII 字母 / 数字。
    fn letter_or_digit(s: &str) -> Option<Key> {
        use Key::*;
        let c = s.chars().next()?;
        if s.len() != 1 {
            return None;
        }
        Some(match c {
            'A' => A,
            'B' => B,
            'C' => C,
            'D' => D,
            'E' => E,
            'F' => F,
            'G' => G,
            'H' => H,
            'I' => I,
            'J' => J,
            'K' => K,
            'L' => L,
            'M' => M,
            'N' => N,
            'O' => O,
            'P' => P,
            'Q' => Q,
            'R' => R,
            'S' => S,
            'T' => T,
            'U' => U,
            'V' => V,
            'W' => W,
            'X' => X,
            'Y' => Y,
            'Z' => Z,
            '0' => Digit0,
            '1' => Digit1,
            '2' => Digit2,
            '3' => Digit3,
            '4' => Digit4,
            '5' => Digit5,
            '6' => Digit6,
            '7' => Digit7,
            '8' => Digit8,
            '9' => Digit9,
            _ => return None,
        })
    }
}

impl FromStr for Shortcut {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse::shortcut(s)
    }
}

impl FromStr for Key {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse::key(s)
    }
}

/// 把键名渲染回 "Ctrl+Alt+K" 形式（UI 展示 / 配置落盘共用）。
pub fn format_shortcut(s: &Shortcut) -> String {
    use Modifier::*;
    let mut parts: Vec<&str> = vec![];
    for (m, name) in [
        (Ctrl, "Ctrl"),
        (Alt, "Alt"),
        (Shift, "Shift"),
        (Meta, "Meta"),
    ] {
        if s.mods.contains(&m) {
            parts.push(name);
        }
    }
    parts.push(key_name(s.key));
    parts.join("+")
}

/// 事件 `e` 是否命中快捷键 `s`：主键一致，且事件修饰键 ⊇ 快捷键修饰键。
/// （按下 Ctrl+Shift+K 也会命中 Ctrl+K——宽松规则避免用户按错一个多余的
///  Shift 就触发失败。）
pub fn matches(e: &RawEvent, s: &Shortcut) -> bool {
    e.key == s.key && e.mods.is_superset(&s.mods)
}
