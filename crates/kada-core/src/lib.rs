//! 咔哒核心：跨平台的键模型、快捷键解析与匹配。
//!
//! 零重量纯逻辑：Windows / macOS / Linux 钩子层都产出 [`RawEvent`]，
//! 由这里的匹配器统一判定触发。配置模型以 JSON 形式落盘和跨平台同步。
//!
//! 键名命名参考 platform 无关的中性名（Enter/Escape），macOS 的 ⌘
//! 由 [`Modifier::Meta`] 承载，平台层负责把 OS 键映射成 [`Key`]。

use std::collections::{BTreeMap, BTreeSet};
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
    // 鼠标键（中键 / 侧键，作为触发键与改键目标；左右键与滚轮不纳入，
    // 避免全局误拦截点击）。
    MouseMiddle => "MouseMiddle", MouseBack => "MouseBack", MouseForward => "MouseForward",
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

/// 配置合并（导入的「合并」方式）：只增不删、不制造新冲突、设置保留本机。
mod merge;
pub use merge::{merge_configs, MergeReport};

mod parse {
    use super::*;

    /// 解析 "Ctrl+Alt+K" 形式的字符串。
    /// 单独的修饰键名（"Shift"）按按键处理，供改键配置使用。
    pub fn shortcut(s: &str) -> Result<Shortcut, ParseError> {
        let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return Err(ParseError::Empty);
        }
        if parts.len() == 1 {
            return Ok(Shortcut { mods: BTreeSet::new(), key: key(parts[0])? });
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
        Ok(Shortcut { mods, key: main_key })
    }

    /// 修饰键名 → `Modifier`（`Ctrl`/`Control`/`Cmd`/`Win`、`Alt`/`Option`、`Shift`、
    /// `Meta`/`Super`）。和弦成员解析（`parse_chord`）也要用它，故对 crate 根可见。
    pub(super) fn modifier(s: &str) -> Option<Modifier> {
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
            "F1" => F1, "F2" => F2, "F3" => F3, "F4" => F4,
            "F5" => F5, "F6" => F6, "F7" => F7, "F8" => F8,
            "F9" => F9, "F10" => F10, "F11" => F11, "F12" => F12,
            "F13" => F13, "F14" => F14, "F15" => F15, "F16" => F16,
            "F17" => F17, "F18" => F18, "F19" => F19, "F20" => F20,
            "F21" => F21, "F22" => F22, "F23" => F23, "F24" => F24,
            "," => Comma, "." => Period, "/" => Slash, "\\" => Backslash,
            ";" => Semicolon, "\"" => Quote, "`" => Backquote,
            "-" => Minus, "=" => Equal, "[" => BracketLeft, "]" => BracketRight,
            "MediaPlayPause" | "PlayPause" => MediaPlayPause,
            "MediaPrev" | "PrevTrack" => MediaPrev,
            "MediaNext" | "NextTrack" => MediaNext,
            "VolumeMute" | "Mute" => VolumeMute,
            "VolumeDown" | "VolDown" => VolumeDown,
            "VolumeUp" | "VolUp" => VolumeUp,
            "NumLock" => NumLock,
            "Numpad0" => Numpad0, "Numpad1" => Numpad1, "Numpad2" => Numpad2,
            "Numpad3" => Numpad3, "Numpad4" => Numpad4, "Numpad5" => Numpad5,
            "Numpad6" => Numpad6, "Numpad7" => Numpad7, "Numpad8" => Numpad8,
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
            'A' => A, 'B' => B, 'C' => C, 'D' => D, 'E' => E, 'F' => F,
            'G' => G, 'H' => H, 'I' => I, 'J' => J, 'K' => K, 'L' => L,
            'M' => M, 'N' => N, 'O' => O, 'P' => P, 'Q' => Q, 'R' => R,
            'S' => S, 'T' => T, 'U' => U, 'V' => V, 'W' => W, 'X' => X,
            'Y' => Y, 'Z' => Z,
            '0' => Digit0, '1' => Digit1, '2' => Digit2, '3' => Digit3,
            '4' => Digit4, '5' => Digit5, '6' => Digit6, '7' => Digit7,
            '8' => Digit8, '9' => Digit9,
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
    for (m, name) in [(Ctrl, "Ctrl"), (Alt, "Alt"), (Shift, "Shift"), (Meta, "Meta")] {
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

// ---------------------------------------------------------------------------
// 键序列（leader key）触发：触发键从「单次组合」扩展为「组合 或 按键序列」。
// 核心只放纯逻辑（解析 + 匹配时序），超时/Esc 判定与注入由壳层负责。
// ---------------------------------------------------------------------------

/// 触发键单元：单个组合键，一串按键序列，或一组同时按住的键（和弦）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Trigger {
    /// 单次组合键（如 `Ctrl+K`）。
    Combo(Shortcut),
    /// 按键序列（如 `F9 J K`：依次按下 F9 → J → K）。首步即 leader 键。
    Sequence(Vec<Shortcut>),
    /// 和弦（如 `F&J`、`Ctrl+F&J`：同时按住这些键）。无序。成员可带修饰键，
    /// 也可全是修饰键（只作「要求按住」），但至少要有一个非修饰键成员。
    Chord(Vec<Shortcut>),
}

impl Trigger {
    /// 解析触发键字符串：单个 token → 组合键，多个 token（空白分隔）→ 序列，
    /// 含 `&` 的单个 token → 和弦。约定：组合键用 `+` 且不含空格；序列步之间
    /// 用空格；和弦成员之间用 `&`。`"Space"`（空格键）是单 token 仍为组合，
    /// `"F9 Space"` 则是「F9 后接空格」的序列。
    pub fn parse(s: &str) -> Result<Trigger, ParseError> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        match tokens.as_slice() {
            [] => Err(ParseError::Empty),
            [one] => {
                if one.contains('&') {
                    return parse_chord(one);
                }
                Ok(Trigger::Combo(one.parse()?))
            }
            _ => {
                let steps = tokens
                    .iter()
                    .map(|t| t.parse::<Shortcut>())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Trigger::Sequence(steps))
            }
        }
    }

    /// 是否为序列（≥2 步）。
    pub fn is_sequence(&self) -> bool {
        matches!(self, Trigger::Sequence(_))
    }

    /// 是否为和弦（同时按住多个键）。
    pub fn is_chord(&self) -> bool {
        matches!(self, Trigger::Chord(_))
    }

    /// 各步（组合键返回单元素切片，便于统一遍历）。
    pub fn steps(&self) -> &[Shortcut] {
        match self {
            Trigger::Combo(s) => std::slice::from_ref(s),
            Trigger::Sequence(steps) => steps,
            Trigger::Chord(members) => members,
        }
    }

    /// 首步（leader 键）。
    pub fn first(&self) -> &Shortcut {
        &self.steps()[0]
    }
}

/// 解析和弦触发键（`&` 分隔同时按住的键，如 `F&J`、`Ctrl+F&J`、`Ctrl+Alt&J`）。
///
/// 成员是 [`Shortcut`]：`key` = **要一起按住的键**，`mods` = **凑齐那一刻必须按住的修饰键**。
/// 于是 `Ctrl+F&J` = 「按住 Ctrl 的同时把 F、J 一起按住」。成员可以全是修饰要求
/// （`Ctrl+Alt&J` 的前半），但**至少要有一个非修饰键成员**——纯修饰键只表示「要求按住」，
/// 没有可吞的键，也就没有「同时按下」的判定时机。
fn parse_chord(s: &str) -> Result<Trigger, ParseError> {
    let raw: Vec<&str> = s.split('&').collect();
    if raw.iter().any(|m| m.trim().is_empty()) {
        return Err(ParseError::ChordInvalid("和弦成员不能为空".into()));
    }
    let members: Vec<Shortcut> = raw
        .iter()
        .map(|m| parse_chord_member(m))
        .collect::<Result<_, _>>()?;
    if members.len() < 2 {
        return Err(ParseError::ChordInvalid("和弦至少需要两个键".into()));
    }
    for (i, m) in members.iter().enumerate() {
        if members[..i].contains(m) {
            return Err(ParseError::ChordInvalid(format!(
                "和弦成员「{}」重复",
                format_shortcut(m)
            )));
        }
    }
    if !members.iter().any(|m| !m.key.is_modifier()) {
        return Err(ParseError::ChordInvalid(
            "和弦至少要有一个非修饰键成员（修饰键只作「要求按住」用）".into(),
        ));
    }
    Ok(Trigger::Chord(members))
}

/// 解析一个和弦成员：常规组合（`Ctrl+F`）直接走 [`Shortcut`]；纯修饰键组合（`Ctrl+Alt`）
/// 没有主键，交给 [`modifier_only_member`] 折叠。解析失败时保留 [`Shortcut`] 那条更具体的
/// 报错（「未知按键: K」比笼统的「和弦无效」有用）。
fn parse_chord_member(s: &str) -> Result<Shortcut, ParseError> {
    match s.parse::<Shortcut>() {
        Ok(sc) => Ok(sc),
        Err(e) => modifier_only_member(s).ok_or(e),
    }
}

/// 纯修饰键成员（`Ctrl+Alt`：每个 token 都是修饰键名、且至少两个）折叠成
/// `{mods: 前面的, key: 最后一个}`。取最后一个当「成员键」是为了沿用同一套 `Shortcut`
/// 表示，且 [`format_shortcut`] 按 Ctrl→Alt→Shift→Meta 渲染，显示会原样回到 `Ctrl+Alt`。
fn modifier_only_member(s: &str) -> Option<Shortcut> {
    let parts: Vec<&str> = s.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    let mut mods: Vec<Modifier> =
        parts.iter().map(|p| parse::modifier(p)).collect::<Option<Vec<_>>>()?;
    if mods.len() < 2 {
        return None;
    }
    let key = modifier_key(mods.pop()?);
    Some(Shortcut { mods: mods.into_iter().collect(), key })
}

/// 键序列匹配的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeqAdvance {
    /// 事件与任何序列无关（未开始 / 断链后重置）。
    NoMatch,
    /// 事件推进了某条序列（应吞掉，继续等待下一键）。
    Advance,
    /// 事件完成了一条序列，`usize` 为命中序列在传入列表中的下标（应吞掉并触发）。
    Complete(usize),
}

/// 键序列匹配的运行时状态（纯逻辑，时序由壳层驱动）。记录当前激活的候选序列
/// 及其已匹配步数，天然支持共享前缀（如 `F9 J K` 与 `F9 J L` 并存）。
#[derive(Clone, Debug, Default)]
pub struct SequenceTracker {
    /// (序列下标, 完整步骤, 已匹配步数)。空 = 未处于序列模式。
    candidates: Vec<(usize, Vec<Shortcut>, usize)>,
}

/// 键序列等待窗的默认毫秒数（leader 后超过该时长未按下一步即回退）。
///
/// 实际取值来自 [`Settings::sequence_timeout_ms`]（设置页可改，`0` = 不限时）；
/// 本常量只是缺省值：老配置（写于该字段出现之前）与 `Settings::default` 都用它。
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 1000;

/// 和弦等待窗的默认毫秒数（按下部分成员后，等其余成员的最长时间）。
///
/// 实际取值来自 [`Settings::chord_timeout_ms`]（设置页可改，`0` = 不限时：等成员键
/// 全部抬起才定论）；本常量只是缺省值。
pub const DEFAULT_CHORD_TIMEOUT_MS: u64 = 1000;

impl SequenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否有正在进行的序列。
    pub fn is_active(&self) -> bool {
        !self.candidates.is_empty()
    }

    /// 清空序列状态（超时 / Esc / 触发完成后由壳层调用）。
    pub fn reset(&mut self) {
        self.candidates.clear();
    }

    /// 喂入一个按键事件，返回推进结果。`sequences` 是当前层生效的所有序列步骤，
    /// 顺序与调用方的触发器列表一致（`Complete(usize)` 的 `usize` 即其下标）。
    pub fn advance(&mut self, ev: &RawEvent, sequences: &[Vec<Shortcut>]) -> SeqAdvance {
        if self.candidates.is_empty() {
            // 以本事件作为某序列首步（leader）开启序列。共享同一 leader 的多条序列
            // （如「F9 J K」与「F9 J L」）必须一并建立候选，否则只有第一条能走到下一步。
            for (idx, steps) in sequences.iter().enumerate() {
                let Some(first) = steps.first() else { continue };
                if matches(ev, first) {
                    self.candidates.push((idx, steps.clone(), 1));
                }
            }
            return if self.candidates.is_empty() {
                SeqAdvance::NoMatch
            } else {
                SeqAdvance::Advance
            };
        }

        // 推进所有命中下一步的候选；任一走完即完成，全都不命中则断链重置。
        let mut advanced: Vec<(usize, Vec<Shortcut>, usize)> = Vec::new();
        for (idx, steps, progress) in self.candidates.iter() {
            let idx = *idx;
            let progress = *progress;
            let Some(next) = steps.get(progress) else { continue };
            if matches(ev, next) {
                if progress + 1 == steps.len() {
                    self.reset();
                    return SeqAdvance::Complete(idx);
                }
                advanced.push((idx, steps.clone(), progress + 1));
            }
        }
        if advanced.is_empty() {
            // 所有候选都断链：重置，事件放行给普通 decide。
            self.reset();
            return SeqAdvance::NoMatch;
        }
        self.candidates = advanced;
        SeqAdvance::Advance
    }
}

/// 和弦（同时按住多个键）匹配的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChordAdvance {
    /// 该键不是任何和弦的成员（集合不变）。
    NoMatch,
    /// 该键是某和弦成员但尚未凑齐（应吞掉，继续等待其它成员）。
    Await,
    /// 该键凑齐了某和弦，`usize` 为命中和弦在传入列表中的下标（应吞掉并触发）。
    Complete(usize),
}

/// 和弦（同时按住多个键）匹配的运行时状态（纯逻辑，时序由壳层驱动）。
/// 记录当前按住的成员键集合，凑齐某和弦的所有成员即触发。成员键按下即被吞，
/// 集合由 [`ChordTracker::press`] / [`ChordTracker::release`] 成对维护：抬起即移出，
/// 保证「同时按住」判定准确，也让壳层能据此判定「没凑成 → 原键回放」的时机。
#[derive(Clone, Debug, Default)]
pub struct ChordTracker {
    /// 当前按住、尚未凑齐的成员键。**只放非修饰键**：修饰键成员（`Ctrl+F` 里的 Ctrl、
    /// `Ctrl+Alt&J` 里的 Ctrl/Alt）是「要求按住」而非「可吞的成员」，它们从不被吞，
    /// 其按住状态由传入的 `mods` 判定。
    held: BTreeSet<Key>,
}

impl ChordTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否有正在等待凑齐的和弦。
    pub fn is_active(&self) -> bool {
        !self.held.is_empty()
    }

    /// 当前按住的成员键（壳层据此判断「成员已全部抬起 → 定论回放」）。
    pub fn held(&self) -> &BTreeSet<Key> {
        &self.held
    }

    /// 清空和弦状态（超时 / 触发完成后由壳层调用）。
    pub fn reset(&mut self) {
        self.held.clear();
    }

    /// 喂入一个成员键按下。`chords` 是当前层生效的所有和弦成员集合，顺序与调用方的
    /// 触发器列表一致（`Complete(usize)` 的 `usize` 即其下标）。和弦成员是无序的
    /// 「同时按住」：所有成员键都在按住集合中即命中。
    ///
    /// `mods` 是**按下这一刻**按住的修饰键（钩子层按物理状态填入，已含刚按下的那个
    /// 修饰键）。成员的修饰要求按它判：凑齐那一刻必须都按着——`Ctrl+F&J` 里 F 的 Ctrl
    /// 要求、`Ctrl+Alt&J` 里那两个纯修饰键成员都走这里，故修饰键无需进按住集合。
    pub fn press(
        &mut self,
        key: Key,
        mods: &BTreeSet<Modifier>,
        chords: &[Vec<Shortcut>],
    ) -> ChordAdvance {
        // 修饰键成员（`Ctrl+F&J` 里的 Ctrl、`Ctrl+Alt&J` 里的 Ctrl/Alt）只作「要求按住」用，
        // 不是可吞的成员：壳层从不把它交给这里（吞掉修饰键会把 `Ctrl+Alt+Tab` 变成
        // `Ctrl+Tab`），这里再挡一道，保证按住集合里只有非修饰键。
        if key.is_modifier() {
            return ChordAdvance::NoMatch;
        }
        // 事件键不是任何和弦成员：集合不变（断链与否由壳层决定，成员键不回放）。
        if !chords.iter().any(|c| c.iter().any(|m| m.key == key)) {
            return ChordAdvance::NoMatch;
        }
        self.held.insert(key);
        for (idx, chord) in chords.iter().enumerate() {
            if chord.iter().all(|m| self.member_held(m, mods)) {
                self.held.clear();
                return ChordAdvance::Complete(idx);
            }
        }
        ChordAdvance::Await
    }

    /// 一个成员是否已满足：主键要么在按住集合里，要么本身是修饰键（看 `mods`），
    /// 且成员的修饰要求全部在 `mods` 里。
    fn member_held(&self, m: &Shortcut, mods: &BTreeSet<Modifier>) -> bool {
        let key_ok = match m.key.as_modifier() {
            Some(md) => mods.contains(&md),
            None => self.held.contains(&m.key),
        };
        key_ok && m.mods.is_subset(mods)
    }

    /// 喂入一个成员键抬起。返回该键原本是否处于按住集合中（即确实是待凑齐的成员）。
    /// 命中触发的和弦时集合已在 [`ChordTracker::press`] 里清空，故返回 false。
    pub fn release(&mut self, key: Key) -> bool {
        self.held.remove(&key)
    }
}

// ---------------------------------------------------------------------------
// 文本扩展（hotstring）：输入触发词 + 后缀自动展开。核心只放纯逻辑（键→字符
// 映射、后缀判定、触发词匹配），缓冲与注入由壳层负责。
// ---------------------------------------------------------------------------

/// 热串触发后缀：空格 / 回车 / Tab。输入这些键且前面的可打印字符命中触发词时展开。
pub fn is_hotstring_terminator(key: Key) -> bool {
    matches!(key, Key::Space | Key::Enter | Key::Tab)
}

/// [`Key`] + Shift → 可打印字符（US 布局语义，与 `kada-hook` 的键码映射一致）。
/// 非可打印键（方向键/功能键/编辑键/修饰键/媒体键/鼠标键等）返回 None。
/// 注意：`Space` 虽可打印，但调用方应先判 `is_hotstring_terminator` 再调用本函数。
pub fn key_to_char(key: Key, shift: bool) -> Option<char> {
    use Key::*;
    let c = match key {
        A => if shift { 'A' } else { 'a' },
        B => if shift { 'B' } else { 'b' },
        C => if shift { 'C' } else { 'c' },
        D => if shift { 'D' } else { 'd' },
        E => if shift { 'E' } else { 'e' },
        F => if shift { 'F' } else { 'f' },
        G => if shift { 'G' } else { 'g' },
        H => if shift { 'H' } else { 'h' },
        I => if shift { 'I' } else { 'i' },
        J => if shift { 'J' } else { 'j' },
        K => if shift { 'K' } else { 'k' },
        L => if shift { 'L' } else { 'l' },
        M => if shift { 'M' } else { 'm' },
        N => if shift { 'N' } else { 'n' },
        O => if shift { 'O' } else { 'o' },
        P => if shift { 'P' } else { 'p' },
        Q => if shift { 'Q' } else { 'q' },
        R => if shift { 'R' } else { 'r' },
        S => if shift { 'S' } else { 's' },
        T => if shift { 'T' } else { 't' },
        U => if shift { 'U' } else { 'u' },
        V => if shift { 'V' } else { 'v' },
        W => if shift { 'W' } else { 'w' },
        X => if shift { 'X' } else { 'x' },
        Y => if shift { 'Y' } else { 'y' },
        Z => if shift { 'Z' } else { 'z' },
        Digit0 => if shift { ')' } else { '0' },
        Digit1 => if shift { '!' } else { '1' },
        Digit2 => if shift { '@' } else { '2' },
        Digit3 => if shift { '#' } else { '3' },
        Digit4 => if shift { '$' } else { '4' },
        Digit5 => if shift { '%' } else { '5' },
        Digit6 => if shift { '^' } else { '6' },
        Digit7 => if shift { '&' } else { '7' },
        Digit8 => if shift { '*' } else { '8' },
        Digit9 => if shift { '(' } else { '9' },
        Comma => if shift { '<' } else { ',' },
        Period => if shift { '>' } else { '.' },
        Slash => if shift { '?' } else { '/' },
        Backslash => if shift { '|' } else { '\\' },
        Semicolon => if shift { ':' } else { ';' },
        Quote => if shift { '"' } else { '\'' },
        Backquote => if shift { '~' } else { '`' },
        Minus => if shift { '_' } else { '-' },
        Equal => if shift { '+' } else { '=' },
        BracketLeft => if shift { '{' } else { '[' },
        BracketRight => if shift { '}' } else { ']' },
        Space => ' ',
        _ => return None,
    };
    Some(c)
}

// ---------------------------------------------------------------------------
// 配置模型：咔哒的配置是跨平台同步介质，直接以 JSON 形式落盘/交换。
// 快捷键以 "Ctrl+Alt+K" 文本存放（人类可读、可编辑），加载时解析校验。
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

/// 操作系统动作：对文件/目录执行复制、剪切、粘贴、删除、新建、压缩或取属性。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OsOperation {
    /// 复制：把 `source`（文件或目录）复制到 `dest`（目录或完整路径）。
    Copy { source: String, dest: String },
    /// 剪切：把 `source` 移动到 `dest`。
    Cut { source: String, dest: String },
    /// 粘贴：把最近一次复制/剪切的来源复制到 `dest`。
    Paste { dest: String },
    /// 删除文件或目录。
    Delete { path: String },
    /// 新建空文件（自动创建父目录）。
    NewFile { path: String },
    /// 新建目录（自动创建父目录）。
    NewFolder { path: String },
    /// 在文件管理器中打开某个目录。
    OpenFolder { path: String },
    /// 把 `source`（文件或目录）压缩为 `dest` 的 zip 归档。
    Zip { source: String, dest: String },
    /// 把 `source` 的 zip 压缩包解压到 `dest` 目录。
    Unzip { source: String, dest: String },
    /// 取文件属性，存入变量 `var`（默认 "file"），供后续动作用占位符引用。
    GetFileProps { path: String, #[serde(default = "default_var")] var: String },
}

#[cfg(feature = "automation")]
fn default_var() -> String {
    "file".into()
}

#[cfg(feature = "automation")]
fn default_status_interval_ms() -> u64 {
    1000
}

/// 前台窗口上下文（供「按前台应用/窗口」类条件求值）。由平台层在钩子回调里抓取。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmostContext {
    /// 前台进程名（如 `chrome.exe`、`Code`）。
    pub process_name: String,
    /// 前台窗口标题。
    pub window_title: String,
}

/// 条件：供「条件判断」动作在触发前求值，为真才执行后续动作。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Condition {
    /// 路径存在（文件或目录）。
    Exists { path: String },
    /// 路径不存在。
    NotExists { path: String },
    /// 路径是文件。
    IsFile { path: String },
    /// 路径是目录。
    IsDir { path: String },
    /// 变量字段等于某值（`field` 为空时比较完整路径）。
    Equals { var: String, #[serde(default)] field: String, value: String },
    /// 变量字段不等于某值。
    NotEquals { var: String, #[serde(default)] field: String, value: String },
    /// 文件/目录的修改时间在最近 `minutes` 分钟内（`path` 支持 `{变量名}` 占位符）。
    ModifiedWithin { path: String, #[serde(default)] minutes: u64 },
    /// 前台进程名匹配（不区分大小写；含 `*`/`?` 走通配，否则子串匹配）。
    FrontmostApp { app: String },
    /// 前台进程名不匹配。
    NotFrontmostApp { app: String },
    /// 前台窗口标题包含该文本（不区分大小写）。
    WindowTitleContains { text: String },
    /// 触发键来自某台输入设备（多键盘按设备区分，见规划 7.3-㉓）。
    /// `id` 与设备标识（Linux evdev 的设备名）按「不区分大小写、含 `*`/`?` 走通配、
    /// 否则子串」匹配；无设备上下文（Windows / macOS / 取不到设备）时恒不成立。
    DeviceIs { id: String },
}

#[cfg(feature = "automation")]
impl Condition {
    /// 在当前变量上下文 + 前台窗口上下文 + 设备上下文下求值。路径类条件支持 `{变量名}` 占位符。
    /// 变量/字段不存在时视为「条件不成立」（返回 false）；前台类条件在无前台上下文
    /// （`frontmost` 为 None，如 Linux/Wayland 受限）时同样视为不成立。设备类条件在无设备
    /// 上下文（`device` 为 None，如 Windows / macOS）时也不成立。
    pub fn matches(
        &self,
        vars: &Vars,
        frontmost: Option<&FrontmostContext>,
        device: Option<&str>,
    ) -> bool {
        use Condition::*;
        let exists = |path: &str| std::path::Path::new(&substitute_vars(path, vars)).exists();
        match self {
            Exists { path } => exists(path),
            NotExists { path } => !exists(path),
            IsFile { path } => std::path::Path::new(&substitute_vars(path, vars)).is_file(),
            IsDir { path } => std::path::Path::new(&substitute_vars(path, vars)).is_dir(),
            Equals { var, field, value } => var_field(var, field, vars)
                .map(|v| v == substitute_vars(value, vars))
                .unwrap_or(false),
            NotEquals { var, field, value } => var_field(var, field, vars)
                .map(|v| v != substitute_vars(value, vars))
                .unwrap_or(false),
            ModifiedWithin { path, minutes } => {
                let p = substitute_vars(path, vars);
                match std::fs::metadata(&p) {
                    Ok(m) => m
                        .modified()
                        .map(|t| elapsed_since_now(t) <= minutes.saturating_mul(60))
                        .unwrap_or(false),
                    Err(_) => false,
                }
            }
            // 前台进程/窗口标题条件：依赖平台层抓取的前台上下文（无上下文 → 不成立）。
            FrontmostApp { app } => frontmost.is_some_and(|f| match_name(app, &f.process_name)),
            NotFrontmostApp { app } => frontmost.is_some_and(|f| !match_name(app, &f.process_name)),
            WindowTitleContains { text } => frontmost
                .is_some_and(|f| f.window_title.to_lowercase().contains(&text.to_lowercase())),
            // 设备条件：依赖平台层抓取的设备标识（无上下文 → 不成立）。
            DeviceIs { id } => device.is_some_and(|d| match_name(id, d)),
        }
    }

    /// 校验条件必要字段非空。
    pub fn validate(&self) -> Result<(), String> {
        use Condition::*;
        let non_empty = |label: &str, v: &str| {
            if v.trim().is_empty() {
                return Err(format!("{label}不能为空"));
            }
            Ok(())
        };
        match self {
            Exists { path } | NotExists { path } | IsFile { path } | IsDir { path } => {
                non_empty("路径", path)
            }
            Equals { var, .. } | NotEquals { var, .. } => non_empty("变量名", var),
            ModifiedWithin { path, .. } => non_empty("路径", path),
            FrontmostApp { app } | NotFrontmostApp { app } => non_empty("进程名", app),
            WindowTitleContains { text } => non_empty("窗口标题", text),
            DeviceIs { id } => non_empty("设备标识", id),
        }
    }
}

/// 前台进程名匹配：不区分大小写；模式含 `*`/`?` 时按通配匹配，否则按子串匹配。
#[cfg(feature = "automation")]
fn match_name(pattern: &str, name: &str) -> bool {
    let p = pattern.to_lowercase();
    let n = name.to_lowercase();
    if p.contains('*') || p.contains('?') {
        wildcard_match(&p, &n)
    } else {
        n.contains(&p)
    }
}

/// 通配匹配：`*` 匹配任意序列、`?` 匹配单个字符。`pattern`/`text` 均已转小写。
#[cfg(feature = "automation")]
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == t[j] || p[i] == '?') {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// 距今多少秒（文件修改时间在未来时视为 0 秒，即「刚修改」）。
#[cfg(feature = "automation")]
fn elapsed_since_now(t: std::time::SystemTime) -> u64 {
    match std::time::SystemTime::now().duration_since(t) {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    }
}

/// 应用操作：启动、关闭、查询运行状态、重启。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AppOperation {
    /// 打开（启动）程序，可选参数。
    Launch { program: String, args: Vec<String> },
    /// 关闭程序（结束所有同名进程）。
    Close { program: String },
    /// 查询程序是否在运行，结果写入变量 `var`（布尔值，供后续条件判断用）。
    /// `retries`：未运行时的重试次数（0 = 只查一次）；`interval_ms`：每次重试间隔。
    Status {
        program: String,
        #[serde(default = "default_var")] var: String,
        #[serde(default)] retries: u32,
        #[serde(default = "default_status_interval_ms")] interval_ms: u64,
    },
    /// 重启程序：先关闭再重新启动。
    Restart { program: String, #[serde(default)] args: Vec<String> },
}

/// 文本动作的方式：直接输入 / 把选中或剪贴板文本转大写 / 转小写。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    /// 原样输入文本。
    #[default]
    Input,
    /// 把当前选中/剪贴板文本转为大写后粘贴。
    ToUpper,
    /// 把当前选中/剪贴板文本转为小写后粘贴。
    ToLower,
}

/// 执行命令动作所用的 Shell。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    /// Windows `cmd /C`（Linux 回退 `sh -c`）。
    Cmd,
    /// PowerShell（仅 Windows）。
    Powershell,
}

/// 触发后执行的单个动作。一个快捷键可挂多个动作，按顺序执行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 文本动作：原样输入文本，或把当前选中/剪贴板文本转大小写后粘贴。
    Text { #[serde(default)] text: String, #[serde(default)] mode: TextMode, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 执行命令（CMD 或 PowerShell），结果进消息中心；`var` 非空时把标准输出（去首尾空白）写入该变量。
    #[cfg(feature = "automation")]
    Command { shell: Shell, command: String, #[serde(default)] show_output: bool, #[serde(default)] var: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 执行 CMD 命令（旧版动作，加载时自动迁移为 `Command(Cmd)`）。
    #[cfg(feature = "automation")]
    Cmd { command: String, #[serde(default)] show_output: bool, #[serde(default)] var: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 执行 PowerShell 命令（旧版动作，加载时自动迁移为 `Command(Powershell)`）。
    #[cfg(feature = "automation")]
    Powershell { command: String, #[serde(default)] show_output: bool, #[serde(default)] var: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 启动可执行文件，可选参数。（旧版动作，加载时自动迁移为 `App::Launch`。）
    #[cfg(feature = "automation")]
    Launch { program: String, args: Vec<String>, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 在文件管理器中打开某个目录。（旧版动作，加载时自动迁移为 `Os::OpenFolder`。）
    #[cfg(feature = "automation")]
    OpenFolder { path: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 同时按下若干键（组合键，如 ["Ctrl","C"]；单键即点按）。
    Keys { keys: Vec<String>, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 暂停 ms 毫秒。
    PauseMs { ms: u64, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 强制结束目标程序的所有进程（Windows `taskkill /F /T`，Linux `pkill -f`）。
    /// （旧版动作，加载时自动迁移为 `App::Close`。）
    #[cfg(feature = "automation")]
    CloseProgram { program: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 操作系统动作（文件复制/剪切/粘贴/删除/新建/压缩/取属性）。
    #[cfg(feature = "automation")]
    Os { operation: OsOperation, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 应用动作（打开/关闭/查询状态/重启）。
    #[cfg(feature = "automation")]
    App { operation: AppOperation, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 用系统默认浏览器打开网址。
    #[cfg(feature = "automation")]
    OpenUrl { url: String, #[serde(default, skip_serializing_if = "Option::is_none")] description: Option<String> },
    /// 条件判断：`condition` 为真时按顺序执行 `then`，否则执行 `otherwise`（可空）。
    #[cfg(feature = "automation")]
    If {
        condition: Condition,
        #[serde(default)]
        then: Vec<Action>,
        #[serde(default)]
        otherwise: Vec<Action>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 执行脚本文件（可选解释器，如 python/node/bash）。`path` 支持 `{变量名}` 占位符；
    /// 通过环境变量注入 `KADA_TRIGGER`/`KADA_NAME`/`KADA_VARS`（变量表 JSON），结果进消息中心。
    #[cfg(feature = "automation")]
    Script {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interpreter: Option<String>,
        #[serde(default)]
        show_output: bool,
        #[serde(default)]
        var: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl Action {
    /// 校验动作可执行：键名可解析、必要字段非空。非法返回中文错误。
    pub fn validate(&self) -> Result<(), String> {
        let check_key = |label: &str, k: &str| {
            k.parse::<Key>().map_err(|e| format!("{label}「{k}」无效：{e}"))?;
            Ok::<(), String>(())
        };
        #[cfg(feature = "automation")]
        let non_empty = |label: &str, v: &str| {
            if v.trim().is_empty() {
                return Err(format!("{label}不能为空"));
            }
            Ok(())
        };
        match self {
            Action::Text { .. } | Action::PauseMs { .. } => Ok(()),
            #[cfg(feature = "automation")]
            Action::Command { command, .. } => non_empty("命令", command),
            #[cfg(feature = "automation")]
            Action::Cmd { command, .. } => non_empty("CMD 命令", command),
            #[cfg(feature = "automation")]
            Action::Powershell { command, .. } => non_empty("PowerShell 命令", command),
            #[cfg(feature = "automation")]
            Action::Launch { program, .. } => non_empty("程序路径", program),
            #[cfg(feature = "automation")]
            Action::CloseProgram { program, .. } => non_empty("程序名", program),
            #[cfg(feature = "automation")]
            Action::OpenFolder { path, .. } => non_empty("目录", path),
            Action::Keys { keys, .. } => {
                if keys.is_empty() {
                    return Err("按键组合不能为空".into());
                }
                for k in keys {
                    check_key("组合键", k)?;
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Os { operation, .. } => {
                use OsOperation::*;
                match operation {
                    Copy { source, dest } | Cut { source, dest } => {
                        non_empty("源路径", source)?;
                        non_empty("目标路径", dest)?;
                        Ok(())
                    }
                    Paste { dest } => non_empty("目标路径", dest),
                    Delete { path } | NewFile { path } | NewFolder { path } | OpenFolder { path } => {
                        non_empty("路径", path)
                    }
                    Zip { source, dest } | Unzip { source, dest } => {
                        non_empty("源路径", source)?;
                        non_empty("目标路径", dest)?;
                        Ok(())
                    }
                    // 变量名允许为空（执行时回退到 "file"），只要求路径非空。
                    GetFileProps { path, .. } => non_empty("路径", path),
                }
            }
            #[cfg(feature = "automation")]
            Action::App { operation, .. } => {
                use AppOperation::*;
                match operation {
                    Launch { program, .. } | Close { program } | Restart { program, .. } => {
                        non_empty("程序", program)
                    }
                    // 变量名允许为空（执行时回退到 "app"），只要求程序非空。
                    Status { program, .. } => non_empty("程序", program),
                }
            }
            #[cfg(feature = "automation")]
            Action::OpenUrl { url, .. } => non_empty("网址", url),
            #[cfg(feature = "automation")]
            Action::If { condition, then, otherwise, .. } => {
                condition.validate()?;
                for a in then.iter().chain(otherwise.iter()) {
                    a.validate()?;
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Script { path, .. } => non_empty("脚本路径", path),
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Action::Text { text: String::new(), mode: TextMode::Input, description: None }
    }
}

/// 目录节点：可嵌套（`parent` 指向父目录 id，`None` 为根级）。
/// 目录只做分组展示，不影响快捷键的执行/冲突检测（那些仍扁平遍历 `shortcuts`）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Folder {
    /// 稳定标识（前端用 UUID 生成；旧配置无此字段时由 UI 补齐）。
    pub id: String,
    /// 目录显示名。
    pub name: String,
    /// 父目录 id；`None` 表示根级目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// 一个键位层：快捷键/改键可归属某层，仅当该层激活时才生效（`None` = 基础层，始终生效）。
/// 层与目录不同——目录只分组展示，层决定「哪些条目参与匹配」。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Layer {
    /// 稳定标识（前端用 UUID 生成）。
    pub id: String,
    /// 层显示名。
    pub name: String,
}

/// 一条快捷键规则：一个或多个触发组合 → 一串动作（按顺序执行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShortcutItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 简短描述（列表/气泡展示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 所属目录 id（`None` 表示未分组）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 归属层 id（`None` = 基础层，始终生效；`Some` = 仅该层激活时生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// 如 ["Ctrl+Alt+K"]，可多个。
    #[serde(default)]
    pub triggers: Vec<String>,
    /// 触发后按顺序执行的多个动作。
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

/// 一条改键规则。
///
/// 形态（互斥，运行时按优先级 `sticky > oneshot > tap-hold > 普通 to` 判定）：
/// - **普通改键**：`from` 按下改发 `to` 键（其余形态字段都留空时）。
/// - **tap-hold**：`tap`（短按）/ `hold`（长按 ≥ `tap_timeout_ms`）至少一个非空时启用，
///   短按输出 `tap` 键、长按输出 `hold` 键；两者可只填其一。
/// - **tap-dance**：`tap2`（双击）/ `tap3`（三击）非空时，短按升级为「连击不同义」，
///   单击/双击/三击分别输出 `tap`/`tap2`/`tap3`（缺省回落上一级）。
/// - **切层键**：`hold_layer` 非空时，长按 `from` 进入该层、松开退回（momentary）。
/// - **锁定切层键**：`lock_layer` 非空时，长按 `from` 切换该层开/关（层保持生效，再长按一次退出）；
///   与 `hold_layer` 互斥——层内放和弦/序列时按住切层键要同时凑 3~4 个键，锁定式才用得起来。
/// - **单次键**：`oneshot` 非空时，单击 `from` 武装该键、应用到下一个非修饰键后自动释放。
/// - **粘滞键**：`sticky` 非空时，单击 `from` 锁定该键、再次单击解锁。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Remap {
    pub from: String,
    /// 普通改键目标（其余形态字段留空时使用）。
    pub to: String,
    /// 短按输出键（tap-hold；空 = 无短按行为）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<String>,
    /// 长按输出键（tap-hold，典型为修饰键）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    /// 归属层 id（`None` = 基础层；`Some` = 仅该层激活时生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// 长按进入的层 id（momentary 切层）；与 `hold`（输出键）互斥。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_layer: Option<String>,
    /// 长按**锁定**的层 id（切换式切层）：长按 `from` 切换该层开/关，层保持生效；与
    /// `hold_layer`（momentary，松开即退层）互斥。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_layer: Option<String>,
    /// tap-hold 判定阈值（毫秒）；0 视为默认 200。
    #[serde(default = "default_tap_timeout_ms")]
    pub tap_timeout_ms: u64,
    /// 单次键（任意键，典型为 `Ctrl/Alt/Shift/Meta`）：单击 `from` 武装、下一个非修饰键
    /// 抬起后自动释放。非修饰键就是「替你按住它，直到下一个键按完」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshot: Option<String>,
    /// 粘滞键（任意键，典型为 `Ctrl/Alt/Shift/Meta`）：单击 `from` 锁定、再次单击解锁。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<String>,
    /// 双击输出键（tap-dance）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap2: Option<String>,
    /// 三击输出键（tap-dance）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap3: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

/// tap-hold 判定阈值的默认值（毫秒）。
pub const DEFAULT_TAP_TIMEOUT_MS: u64 = 200;

fn default_tap_timeout_ms() -> u64 {
    DEFAULT_TAP_TIMEOUT_MS
}

impl Remap {
    /// 是否有 tap-hold / tap-dance / 切层 行为（短按/长按/双击/三击/长按切层/长按锁定层 任一非空白）。
    /// 不含 oneshot/sticky（那些是修饰键模式，见 [`Remap::is_oneshot`] / [`Remap::is_sticky`]）。
    pub fn is_tap_hold(&self) -> bool {
        let nonempty = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
        nonempty(&self.tap)
            || nonempty(&self.hold)
            || nonempty(&self.hold_layer)
            || nonempty(&self.lock_layer)
            || nonempty(&self.tap2)
            || nonempty(&self.tap3)
    }

    /// 是否单次键模式。
    pub fn is_oneshot(&self) -> bool {
        self.oneshot.as_deref().is_some_and(|v| !v.trim().is_empty())
    }

    /// 是否粘滞键模式。
    pub fn is_sticky(&self) -> bool {
        self.sticky.as_deref().is_some_and(|v| !v.trim().is_empty())
    }

    /// 是否需要时序状态机（tap-hold / 单次 / 粘滞 任一）。壳层据此决定把 `from` 交给
    /// tap-hold 状态机，而非当普通改键。
    pub fn needs_timing_state(&self) -> bool {
        self.is_tap_hold() || self.is_oneshot() || self.is_sticky()
    }

    /// 是否 tap-dance（双击/三击输出，短按升级为连击不同义）。
    pub fn is_multi_tap(&self) -> bool {
        let nonempty = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
        nonempty(&self.tap2) || nonempty(&self.tap3)
    }

    /// 短按输出键（解析失败返回 None）。
    pub fn tap_key(&self) -> Option<Key> {
        self.tap.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 长按输出键（解析失败返回 None）。
    pub fn hold_key(&self) -> Option<Key> {
        self.hold.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 长按进入的层 id（空白视为无）。
    pub fn hold_layer_id(&self) -> Option<&str> {
        self.hold_layer.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// 长按锁定的层 id（空白视为无）。
    pub fn lock_layer_id(&self) -> Option<&str> {
        self.lock_layer.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// 单次键（解析失败返回 None）。
    pub fn oneshot_key(&self) -> Option<Key> {
        self.oneshot.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 粘滞键（解析失败返回 None）。
    pub fn sticky_key(&self) -> Option<Key> {
        self.sticky.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 双击输出键（解析失败返回 None）。
    pub fn tap2_key(&self) -> Option<Key> {
        self.tap2.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 三击输出键（解析失败返回 None）。
    pub fn tap3_key(&self) -> Option<Key> {
        self.tap3.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 击数 1/2/3 对应的输出键（含缺省回落）：`[tap, tap2|tap, tap3|tap2|tap]`。
    pub fn tap_outputs(&self) -> [Option<Key>; 3] {
        let tap = self.tap_key();
        let tap2 = self.tap2_key().or(tap);
        let tap3 = self.tap3_key().or(tap2).or(tap);
        [tap, tap2, tap3]
    }

    /// 按击数取短按输出（1=单击、2=双击、3=三击），缺省回落上一级。
    pub fn tap_output(&self, count: u8) -> Option<Key> {
        match count {
            3 => self.tap_outputs()[2],
            2 => self.tap_outputs()[1],
            _ => self.tap_outputs()[0],
        }
    }

    /// 人类可读的改键形态描述（冲突提示 / 列表摘要用）。
    pub fn describe(&self) -> String {
        if let Some(s) = self.sticky_key() {
            return format!("粘滞 {}", key_name(s));
        }
        if let Some(o) = self.oneshot_key() {
            return format!("单击 {}", key_name(o));
        }
        if self.is_tap_hold() {
            let mut parts: Vec<String> = Vec::new();
            if let Some(t) = self.tap_key() {
                parts.push(format!("单击 {}", key_name(t)));
            }
            if let Some(t) = self.tap2_key() {
                parts.push(format!("双击 {}", key_name(t)));
            }
            if let Some(t) = self.tap3_key() {
                parts.push(format!("三击 {}", key_name(t)));
            }
            if let Some(h) = self.hold_key() {
                parts.push(format!("长按 {}", key_name(h)));
            }
            if self.hold_layer_id().is_some() {
                parts.push("长按进层".into());
            }
            if self.lock_layer_id().is_some() {
                parts.push("长按锁定层".into());
            }
            return parts.join(" · ");
        }
        self.to.clone()
    }

    /// tap-hold 判定阈值（毫秒）：0 归一化为默认 200。
    pub fn tap_timeout(&self) -> u64 {
        if self.tap_timeout_ms == 0 {
            DEFAULT_TAP_TIMEOUT_MS
        } else {
            self.tap_timeout_ms
        }
    }
}

/// 一条文本扩展（hotstring）：输入触发词 + 后缀（空格/回车/Tab）自动展开为替换文本。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TextExpansion {
    /// 触发词（如 `:addr`、`;sig`），不含触发后缀。
    pub trigger: String,
    /// 展开文本（支持 `{date}` / `{time}` / `{clipboard}` 动态片段占位符）。
    pub replace: String,
    #[serde(default)]
    pub enabled: bool,
}

impl TextExpansion {
    /// 校验扩展可保存：触发词不能为空（空触发词会匹配任意输入、每个后缀都触发）。
    /// 替换文本允许为空（等价于删除触发词）。
    pub fn validate(&self) -> Result<(), String> {
        if self.trigger.trim().is_empty() {
            return Err("触发词不能为空".into());
        }
        Ok(())
    }
}

/// 在输入缓冲尾部查找命中的文本扩展（最长触发词优先，避免短词遮蔽长词）。
/// `buffer` 是最近输入的可打印字符序列（不含触发后缀）。
pub fn match_expansion<'a>(
    buffer: &str,
    expansions: &'a [TextExpansion],
) -> Option<&'a TextExpansion> {
    expansions
        .iter()
        .filter(|e| e.enabled && !e.trigger.is_empty() && buffer.ends_with(&e.trigger))
        .max_by_key(|e| e.trigger.chars().count())
}

/// 动作执行超时的默认值（毫秒）。
///
/// 超过这个时长还没结束的命令/脚本会被连同子进程一起终止，并在消息中心记一条结果。
/// 没有超时的后果不是「慢」而是「坏」：脚本挂住就永久占住这条触发的执行线程，后续动作
/// 永不执行、线程也收不回来（见规划 7.2-⑦）。
pub const DEFAULT_ACTION_TIMEOUT_MS: u64 = 30_000;

/// 文本注入方式（[`Settings::text_inject_mode`]，见规划 7.3-㉒）。
///
/// 默认「剪贴板粘贴」：整段一次到位、中文最稳，但会短暂占用剪贴板，且被吞粘贴的
/// 目标程序（多数游戏 / 部分终端）直接无视；「逐字直发」把每个字符合成键盘事件
/// （Windows `KEYEVENTF_UNICODE` / macOS unicode string 事件），不经过剪贴板，
/// 专治吞粘贴的目标程序，代价是长文本逐字送、极快的流个别程序可能丢字。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextInjectMode {
    /// 剪贴板 + 粘贴组合键（默认）。
    #[default]
    Clipboard,
    /// 字符事件直发（`KEYEVENTF_UNICODE` / CGEvent unicode string）。
    Unicode,
}

// 手工实现 Deserialize 而非 derive：手改配置把值写错（如笔误 "unicod"）时回落默认的
// 剪贴板模式，而不是让整份配置解析失败——加载路径对解析失败是「留档 + 告警」的硬处理，
// 为一个可忽略的字段触发它不值当。
impl<'de> Deserialize<'de> for TextInjectMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "unicode" => TextInjectMode::Unicode,
            _ => TextInjectMode::Clipboard,
        })
    }
}

/// 快捷键提示框（常显浮窗，见规划 7.3-㊳）的持久化状态。
///
/// 单独一个结构而不是往 `Settings` 里再摊几个字段：这一组值同生共死（可见性 + 位置 + 外观），
/// 摊平了写一处就得连改好几个字段，读配置的人还得自己把它们拼回去。
///
/// 量纲刻意都是**整数**：`Config` 是全量 `Eq` 的类型，浮点会让它丢掉 `Eq`；百分比/逻辑像素
/// 也正好是人类可读、可手改 JSON 的值。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HintsWindow {
    /// 是否显示这份提示框。关掉（窗口上的 × / 设置页取消勾选 / 托盘菜单）只置 `false`，
    /// 位置与外观都留着，下次打开还在原地。
    #[serde(default)]
    pub visible: bool,
    /// 上次拖到的位置（逻辑像素，窗口左上角）；`None` = 还没拖过，首次显示落到工作区右侧。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    /// 字号缩放百分比（`100` = 基准），滚轮调整。
    #[serde(default = "default_hints_scale")]
    pub scale: u16,
    /// 不透明度百分比（`100` = 完全不透明），Ctrl + 滚轮调整。
    #[serde(default = "default_hints_opacity")]
    pub opacity: u8,
}

impl HintsWindow {
    /// 字号缩放的上下限（百分比）：下限保证还看得清，上限避免一个快捷键占满屏。
    pub const SCALE_RANGE: (u16, u16) = (60, 220);
    /// 不透明度的上下限（百分比）：下限留一点可见性（全透明等于关不掉又看不见）。
    pub const OPACITY_RANGE: (u8, u8) = (20, 100);
    /// 默认字号缩放（百分比）。
    pub const DEFAULT_SCALE: u16 = 100;
    /// 默认不透明度（百分比）：留一点点透，压在上面不挡视线。
    pub const DEFAULT_OPACITY: u8 = 92;

    /// 归一化：手改 JSON / 从别的机器同步过来的越界值在这里钳住（[`Config::migrate`] 调用）。
    ///
    /// 越界值不钳的后果很具体：`scale: 0` 会把提示框缩成看不见的一条，而用户明明勾着
    /// 「显示提示框」——想关掉它有的是正经开关，不该靠非法值达成。
    pub fn normalized(mut self) -> Self {
        self.scale = self.scale.clamp(Self::SCALE_RANGE.0, Self::SCALE_RANGE.1);
        self.opacity = self.opacity.clamp(Self::OPACITY_RANGE.0, Self::OPACITY_RANGE.1);
        self
    }
}

impl Default for HintsWindow {
    fn default() -> Self {
        Self {
            visible: false,
            x: None,
            y: None,
            scale: Self::DEFAULT_SCALE,
            opacity: Self::DEFAULT_OPACITY,
        }
    }
}

/// `HintsWindow::scale` 的缺省值（供 serde 与 [`HintsWindow::default`] 共用）。
pub fn default_hints_scale() -> u16 {
    HintsWindow::DEFAULT_SCALE
}

/// `HintsWindow::opacity` 的缺省值（供 serde 与 [`HintsWindow::default`] 共用）。
pub fn default_hints_opacity() -> u8 {
    HintsWindow::DEFAULT_OPACITY
}

/// 应用设置（配置的一部分，随 JSON 一起落盘/跨平台同步）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// 开机自启（跟随系统启动）。
    #[serde(default)]
    pub autostart: bool,
    /// 暂停所有快捷键/改键（全局开关，持久化）。
    #[serde(default)]
    pub paused: bool,
    /// 快速唤醒键：双击该键唤出主窗口（如 "Alt"）；`None` 表示关闭快速唤醒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_key: Option<String>,
    /// 命令/脚本类动作的执行超时（毫秒）；`0` = 不限时。
    ///
    /// 缺字段时取 [`DEFAULT_ACTION_TIMEOUT_MS`]：老配置（写于本字段出现之前）也要拿到
    /// 超时保护，若用 `#[serde(default)]` 的零值会让它们默认「不限时」，保护形同虚设。
    #[serde(default = "default_action_timeout_ms")]
    pub action_timeout_ms: u64,
    /// 键序列（leader key）等待窗（毫秒）：leader 按下后等下一步的最长时间，超时把已吞掉的
    /// 键原样回放；`0` = 不限时（一直等，直到断链或命中）。
    ///
    /// 手感因人而异（手慢的人嫌 1000ms 短、误触的人嫌它长），故进设置。缺字段取
    /// [`DEFAULT_SEQUENCE_TIMEOUT_MS`]——与 `action_timeout_ms` 同一个理由：老配置（写于本
    /// 字段出现之前）本来就按 1000ms 跑，`#[serde(default)]` 的零值会悄悄改成「不限时」。
    #[serde(default = "default_sequence_timeout_ms")]
    pub sequence_timeout_ms: u64,
    /// 和弦（`F&J` 同时按住）等待窗（毫秒）：按下部分成员后，等其余成员的最长时间；超时判定
    /// 「不是和弦」，把待定的键按输入顺序原样回放；`0` = 不限时（等成员键全部抬起才定论）。
    ///
    /// 缺字段取 [`DEFAULT_CHORD_TIMEOUT_MS`]（同上）。
    #[serde(default = "default_chord_timeout_ms")]
    pub chord_timeout_ms: u64,
    /// 输入状态悬浮指示：有层或注入的修饰键生效时，屏幕下方浮一条「当前激活层 + 修饰键
    /// （hold / oneshot / sticky）」，没有东西生效时自动消失；关掉就只留托盘提示。
    ///
    /// 缺字段取 `true`（[`default_true`]）：oneshot / sticky / 切层键这类形态「按了看不见状态」
    /// 是用户误判「按键失灵」的主因（见规划 7.3-⑬），默认开启；它只在真有东西生效时才出现、
    /// 平时不占屏幕，不想要的人在设置页关掉即可。用 `#[serde(default)]` 的零值会让老配置
    /// 静默拿不到这个指示。
    #[serde(default = "default_true")]
    pub show_status_hud: bool,
    /// 文本注入方式：`clipboard`（默认）剪贴板 + 粘贴 / `unicode` 逐字直发字符事件。
    ///
    /// 缺字段取剪贴板（[`TextInjectMode::Clipboard`]）：默认方案对中文最稳；只有目标程序
    /// 吞粘贴（游戏 / 终端，规划 7.3-㉒ 的场景）才需要切到直发，故不做「缺字段也开」。
    #[serde(default)]
    pub text_inject_mode: TextInjectMode,
    /// 快捷键提示框（常显速查浮窗）的可见性 / 位置 / 外观，见 [`HintsWindow`]。
    ///
    /// 缺字段取 [`HintsWindow::default`]（**默认不显示**）：它是一块常挂屏幕上的浮层，
    /// 和一个只在生效时才出现的状态指示不同——不请自来地占着屏幕比「找不到入口」更烦人。
    #[serde(default)]
    pub hints: HintsWindow,
}

/// `show_status_hud` 的缺省值（供 serde 与 [`Settings::default`] 共用）。
pub fn default_true() -> bool {
    true
}

/// `action_timeout_ms` 的缺省值（供 serde 与 [`Settings::default`] 共用）。
pub fn default_action_timeout_ms() -> u64 {
    DEFAULT_ACTION_TIMEOUT_MS
}

/// `sequence_timeout_ms` 的缺省值（供 serde 与 [`Settings::default`] 共用）。
pub fn default_sequence_timeout_ms() -> u64 {
    DEFAULT_SEQUENCE_TIMEOUT_MS
}

/// `chord_timeout_ms` 的缺省值（供 serde 与 [`Settings::default`] 共用）。
pub fn default_chord_timeout_ms() -> u64 {
    DEFAULT_CHORD_TIMEOUT_MS
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            paused: false,
            wake_key: None,
            action_timeout_ms: DEFAULT_ACTION_TIMEOUT_MS,
            sequence_timeout_ms: DEFAULT_SEQUENCE_TIMEOUT_MS,
            chord_timeout_ms: DEFAULT_CHORD_TIMEOUT_MS,
            show_status_hud: true,
            text_inject_mode: TextInjectMode::Clipboard,
            hints: HintsWindow::default(),
        }
    }
}

/// 根配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Config {
    /// 目录树（`Vec` 顺序即同级展示顺序；空目录也可持久化）。
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// 键位层（`Vec` 顺序即展示顺序）。
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub shortcuts: Vec<ShortcutItem>,
    #[serde(default)]
    pub remaps: Vec<Remap>,
    #[serde(default)]
    pub expansions: Vec<TextExpansion>,
    #[serde(default)]
    pub settings: Settings,
}

/// 冲突严重程度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
}

/// 一条快捷键/改键冲突。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub severity: Severity,
    pub message: String,
    /// 涉及的主要快捷键名称（无名称为空字符串），供 UI「消息→冲突」页标识是哪个快捷键。
    #[serde(default)]
    pub name: String,
}

/// 平台输入能力声明（冲突检测据此标注「当前平台用不了」的条目，见规划 7.3-⑲）。
///
/// 同一份配置可能在多平台间同步，而各平台能力不同：Linux 取不到前台窗口（前台类条件
/// 恒不成立）、macOS 绑不了媒体键与 F21~F24、鼠标键在部分平台监听/注入受限。这类
/// 「配置本身合法、只是当前平台不支持」按 [`Severity::Warn`] 提示，**不阻止保存**——
/// 同一份配置在别的平台可能是完全正常的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformCaps {
    /// 监听不到的键：不能作触发键（组合/序列/和弦）与改键来源，配置了也不会触发。
    pub listen_unsupported: BTreeSet<Key>,
    /// 注入不了的键：不能作改键目标与 `Keys` 动作成员，执行到会静默无输出。
    pub inject_unsupported: BTreeSet<Key>,
    /// 「前台应用 / 窗口标题」类条件是否可用（不可用时恒不成立，`If` 恒走 `otherwise`）。
    pub frontmost_conditions: bool,
    /// 「设备是」类条件是否可用（不可用时恒不成立，`If` 恒走 `otherwise`）。目前仅 Linux
    /// evdev 能给出真实设备标识，Windows / macOS 拿不到设备。
    pub device_conditions: bool,
}

impl Default for PlatformCaps {
    /// 全支持（没有任何能力缺失的假想平台；单测「零误报」用它当基准）。
    fn default() -> Self {
        Self {
            listen_unsupported: BTreeSet::new(),
            inject_unsupported: BTreeSet::new(),
            frontmost_conditions: true,
            device_conditions: true,
        }
    }
}

impl PlatformCaps {
    /// 是否全支持：全支持时平台扫描整段跳过（零误报零开销）。
    fn is_full(&self) -> bool {
        self.listen_unsupported.is_empty()
            && self.inject_unsupported.is_empty()
            && self.frontmost_conditions
            && self.device_conditions
    }
}

/// 和弦成员的指纹：`(成员键, 修饰要求)` 表（[`chord_signature`] 排序后成表，无序可比）。
type ChordSignature = Vec<(Key, BTreeSet<Modifier>)>;

/// 一个和弦触发条目：(触发键文本, 成员, 快捷键名)。
type ChordEntry = (String, Vec<Shortcut>, String);

/// 和弦成员的指纹（用于判重 / 遮蔽判定）：`(成员键, 修饰要求)` 排序后成表，无序可比。
fn chord_signature(members: &[Shortcut]) -> ChordSignature {
    let mut sig: Vec<(Key, BTreeSet<Modifier>)> =
        members.iter().map(|m| (m.key, m.mods.clone())).collect();
    sig.sort();
    sig
}

/// 和弦的成员键集合（去重；判「子集」用）。
fn chord_keys(members: &[Shortcut]) -> BTreeSet<Key> {
    members.iter().map(|m| m.key).collect()
}

/// `a` 的要求是否被 `b` 全覆盖：`a` 的每个成员都能在 `b` 里找到「同一个成员键、修饰要求更宽」
/// （`a.mods ⊆ b.mods`）的成员。覆盖意味着 `b` 凑齐时 `a` 一定也凑齐了——两个完全相同的
/// 和弦不算（那是重复触发键，另有硬冲突）。
fn chord_covers(a: &[Shortcut], b: &[Shortcut]) -> bool {
    a.iter().all(|m| b.iter().any(|n| n.key == m.key && m.mods.is_subset(&n.mods)))
        && chord_signature(a) != chord_signature(b)
}

/// 按「同层」分组收集所有启用的和弦触发键：(触发键文本, 成员, 快捷键名)。
///
/// 用 `Vec` 按配置顺序分组（不是 `HashMap`）：组内顺序就是运行时
/// `collect_chord_items` 的判定顺序（先判到的胜出），遮蔽判定依赖它。
fn chord_groups(cfg: &Config) -> Vec<(Option<String>, Vec<ChordEntry>)> {
    let mut groups: Vec<(Option<String>, Vec<ChordEntry>)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            let Ok(Trigger::Chord(members)) = Trigger::parse(t) else { continue };
            let entry = (t.clone(), members, s.name.clone().unwrap_or_default());
            match groups.iter_mut().find(|(layer, _)| layer == &s.layer) {
                Some((_, items)) => items.push(entry),
                None => groups.push((s.layer.clone(), vec![entry])),
            }
        }
    }
    groups
}

/// 检测配置中的快捷键/改键冲突。
///
/// - 硬冲突（[`Severity::Error`]，应阻止保存）：重复触发键（组合/序列/和弦）、
///   序列 leader 遮蔽单组合、重复改键来源、改键来源与快捷键主键相同（改键优先，快捷键将失效）。
/// - 软冲突（[`Severity::Warn`]，仅提示）：触发键超集重叠（更宽松的组合会遮蔽更具体的组合）；
///   和弦之间「一个的要求被另一个全覆盖」（更宽松的那个会先凑齐 → 更严的永远轮不到）；
///   平台能力缺失（[`PlatformCaps`]：监听不到的触发键、注入不了的目标、恒不成立的前台条件）。
pub fn detect_conflicts(cfg: &Config, caps: &PlatformCaps) -> Vec<Conflict> {
    use std::collections::HashMap;

    let mut out: Vec<Conflict> = Vec::new();

    // 1) 重复触发键（同层内 enabled 且跨不同条目；不同层可共用同键）。
    //    单组合按 (层, mods, key) 判重；序列按 (层, 完整字符串) 判重；
    //    和弦按 (层, 排序后的成员（键 + 修饰要求）列表) 判重（F&J 与 J&F 等价，
    //    但 F&J 与 Ctrl+F&J 是两条不同的触发键）。
    let mut seen_combo: HashMap<(Option<String>, BTreeSet<Modifier>, Key), String> = HashMap::new();
    let mut seen_seq: HashMap<(Option<String>, String), ()> = HashMap::new();
    let mut seen_chord: HashMap<(Option<String>, ChordSignature), String> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => {
                    let k = (s.layer.clone(), sc.mods, sc.key);
                    if let Some(first) = seen_combo.get(&k) {
                        out.push(Conflict {
                            severity: Severity::Error,
                            message: format!("触发键「{t}」与「{first}」重复，多个快捷键共用同一组合"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    } else {
                        seen_combo.insert(k, t.clone());
                    }
                }
                Ok(Trigger::Sequence(_)) => {
                    if seen_seq.insert((s.layer.clone(), t.clone()), ()).is_some() {
                        out.push(Conflict {
                            severity: Severity::Error,
                            message: format!("触发序列「{t}」重复，多个快捷键共用同一序列"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    }
                }
                Ok(Trigger::Chord(members)) => {
                    let k = (s.layer.clone(), chord_signature(&members));
                    if let Some(first) = seen_chord.get(&k) {
                        out.push(Conflict {
                            severity: Severity::Error,
                            message: format!("和弦「{t}」与「{first}」重复，多个快捷键共用同一和弦"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    } else {
                        seen_chord.insert(k, t.clone());
                    }
                }
                Err(_) => {}
            }
        }
    }

    // 1b) 序列 leader / 和弦成员遮蔽单组合或序列（同层内）：二者都会吞掉成员键的
    //     down，使「主键 = 该成员键」的单组合或序列 leader 失效。
    let mut combos_by_layer: HashMap<Option<String>, Vec<(String, Shortcut)>> = HashMap::new();
    let mut seq_leaders_by_layer: HashMap<Option<String>, Vec<(String, Shortcut)>> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => {
                    combos_by_layer.entry(s.layer.clone()).or_default().push((t.clone(), sc));
                }
                Ok(Trigger::Sequence(steps)) => {
                    if let Some(leader) = steps.first() {
                        seq_leaders_by_layer
                            .entry(s.layer.clone())
                            .or_default()
                            .push((t.clone(), leader.clone()));
                    }
                }
                _ => {}
            }
        }
    }
    // 序列 leader 遮蔽单组合。
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Sequence(steps)) = Trigger::parse(t) {
                let Some(leader) = steps.first() else { continue };
                if let Some(combos) = combos_by_layer.get(&s.layer) {
                    for (ct, csc) in combos {
                        if csc == leader {
                            out.push(Conflict {
                                severity: Severity::Error,
                                message: format!(
                                    "序列「{t}」的 leader「{0}」会吞掉该键，使组合键「{ct}」失效",
                                    format_shortcut(leader)
                                ),
                                name: s.name.clone().unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }
    // 和弦成员遮蔽单组合 / 序列 leader。只算非修饰键成员：修饰键成员（`Ctrl+F` 里的 Ctrl、
    // `Ctrl+Alt&J` 里的 Ctrl/Alt）从不被吞（吞掉修饰键会把 Ctrl+Alt+Tab 变成 Ctrl+Tab），
    // 也就遮蔽不了以它为「主键」的组合。
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Chord(members)) = Trigger::parse(t) {
                for m in members.iter().filter(|m| !m.key.is_modifier()) {
                    if let Some(combos) = combos_by_layer.get(&s.layer) {
                        for (ct, csc) in combos {
                            if csc.key == m.key {
                                out.push(Conflict {
                                    severity: Severity::Error,
                                    message: format!(
                                        "和弦「{t}」的成员「{0}」会吞掉该键，使组合键「{ct}」失效",
                                        key_name(m.key)
                                    ),
                                    name: s.name.clone().unwrap_or_default(),
                                });
                            }
                        }
                    }
                    if let Some(leaders) = seq_leaders_by_layer.get(&s.layer) {
                        for (st, leader) in leaders {
                            if leader.key == m.key {
                                out.push(Conflict {
                                    severity: Severity::Error,
                                    message: format!(
                                        "和弦「{t}」的成员「{0}」会吞掉该键，使序列「{st}」的 leader 失效",
                                        key_name(m.key)
                                    ),
                                    name: s.name.clone().unwrap_or_default(),
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // 1c) 和弦之间的超集 / 子集软冲突（同层内）：一个和弦的要求被另一个全覆盖时，它必然
    //     先凑齐并触发，更严的那个**永远轮不到**（同「层可达性」，两边都是合法配置，
    //     只是有一个不生效，所以是 Warn 不是 Error）。
    //     两种「先凑齐」的来源：① 成员键更少 → 更早凑齐（与书写顺序无关）；② 成员键相同
    //     但修饰要求更宽 → 同一刻凑齐，靠列表靠前先被判到。故按下面两条各判一次。
    for (_layer, items) in chord_groups(cfg) {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let (ti, ai, ni) = &items[i];
                let (tj, aj, nj) = &items[j];
                // 靠前的那条要求更宽 → 靠后那条永远轮不到。
                if chord_covers(ai, aj) {
                    out.push(Conflict {
                        severity: Severity::Warn,
                        message: format!(
                            "和弦「{tj}」永远触发不到：「{ti}」的要求更宽（成员更少或修饰要求更宽），凑齐时会先轮到它"
                        ),
                        name: nj.clone(),
                    });
                } else if chord_covers(aj, ai) && chord_keys(aj).len() < chord_keys(ai).len() {
                    // 靠后的那条成员严格更少 → 必然先凑齐，与书写顺序无关。
                    out.push(Conflict {
                        severity: Severity::Warn,
                        message: format!(
                            "和弦「{ti}」永远触发不到：「{tj}」的成员是它的子集，必然先凑齐并触发"
                        ),
                        name: ni.clone(),
                    });
                }
            }
        }
    }

    // 2) 重复改键来源（同层内；不同层可共用同键）
    let mut remap_from: HashMap<(Option<String>, Key), String> = HashMap::new();
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        if let Ok(from) = r.from.parse::<Key>() {
            let k = (r.layer.clone(), from);
            if let Some(first) = remap_from.get(&k) {
                out.push(Conflict {
                    severity: Severity::Error,
                    message: format!("改键来源「{first}」重复，多条改键都从「{first}」改起"),
                    name: String::new(),
                });
            } else {
                remap_from.insert(k, r.from.clone());
            }
        }
    }

    // 3) 改键来源与快捷键主键相同
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        let Ok(from) = r.from.parse::<Key>() else { continue };
        let desc = r.describe();
        for s in &cfg.shortcuts {
            if !s.enabled {
                continue;
            }
            for t in &s.triggers {
                if let Ok(sc) = t.parse::<Shortcut>() {
                    if sc.key == from {
                        out.push(Conflict {
                            severity: Severity::Error,
                            message: format!(
                                "改键「{} → {desc}」会拦截按键「{}」，使快捷键「{t}」失效",
                                r.from,
                                key_name(from)
                            ),
                            name: s.name.clone().unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }

    // 4) 触发键超集重叠（软冲突）
    let mut items: Vec<(String, BTreeSet<Modifier>, Key)> = Vec::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                items.push((t.clone(), sc.mods, sc.key));
            }
        }
    }
    for i in 0..items.len() {
        for j in 0..items.len() {
            if i == j {
                continue;
            }
            let (ti, mi, ki) = &items[i];
            let (tj, mj, kj) = &items[j];
            if ki != kj || mi == mj || !mi.is_subset(mj) {
                continue;
            }
            // mi ⊂ mj：更宽松的 ti 会遮蔽更具体的 tj
            out.push(Conflict {
                severity: Severity::Warn,
                message: format!("「{tj}」被「{ti}」遮蔽：按下 {tj} 时会先命中更宽松的「{ti}」"),
                name: String::new(),
            });
        }
    }

    // 5) 层可达性：层里有启用的条目，却没有任何「长按进入层 / 长按锁定层」的切层键指向它 →
    //    这些条目永远不会生效（`active_layer` 只由切层键驱动）。这是「层内快捷键配了但不
    //    触发」最难自查的原因，所以在清单里直接点出来。
    for l in &cfg.layers {
        let has_entries = cfg
            .shortcuts
            .iter()
            .any(|s| s.enabled && s.layer.as_deref() == Some(l.id.as_str()))
            || cfg.remaps.iter().any(|r| r.enabled && r.layer.as_deref() == Some(l.id.as_str()));
        if !has_entries {
            continue;
        }
        let reachable = cfg.remaps.iter().any(|r| {
            r.enabled
                && (r.hold_layer.as_deref() == Some(l.id.as_str())
                    || r.lock_layer.as_deref() == Some(l.id.as_str()))
        });
        if !reachable {
            out.push(Conflict {
                severity: Severity::Warn,
                message: format!(
                    "层「{}」没有切层键：层内的快捷键/改键永远不会生效（在「改键」里设一条「长按进入层」或「长按锁定层」指向它；层内放和弦/序列建议用「长按锁定层」）",
                    l.name
                ),
                name: String::new(),
            });
        }
    }

    // 6) 平台能力缺失：本平台监听不到的触发键/改键来源、注入不了的目标键、恒不成立的
    //    前台条件（7.3-⑲）。跨平台同步的配置在能力短板的平台上会「静默失效」，这些条目
    //    本身合法，只报 Warn 提示「在你这台机器上不会生效」。
    out.extend(platform_conflicts(cfg, caps));

    out
}

/// 平台能力缺失扫描（[`detect_conflicts`] 的第 6 段）。全支持（[`PlatformCaps::is_full`]）
/// 时直接返回空，不影响已有检测。
fn platform_conflicts(cfg: &Config, caps: &PlatformCaps) -> Vec<Conflict> {
    let mut out: Vec<Conflict> = Vec::new();
    if caps.is_full() {
        return out;
    }
    let name_of = |s: &ShortcutItem| s.name.clone().unwrap_or_default();
    let key_list = |keys: &BTreeSet<Key>| {
        keys.iter().map(|k| key_name(*k)).collect::<Vec<_>>().join("、")
    };

    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        // 触发键（组合/序列/和弦）里监听不到的键。
        for t in &s.triggers {
            let mut bad: BTreeSet<Key> = BTreeSet::new();
            match Trigger::parse(t) {
                Ok(Trigger::Combo(sc)) => collect_unlistenable(&sc, caps, &mut bad),
                Ok(Trigger::Sequence(steps)) => {
                    for sc in &steps {
                        collect_unlistenable(sc, caps, &mut bad);
                    }
                }
                Ok(Trigger::Chord(members)) => {
                    for sc in &members {
                        collect_unlistenable(sc, caps, &mut bad);
                    }
                }
                Err(_) => {}
            }
            if !bad.is_empty() {
                out.push(Conflict {
                    severity: Severity::Warn,
                    message: format!(
                        "触发键「{t}」要用到{}，当前平台监听不到这个键，这条触发不会生效",
                        key_list(&bad)
                    ),
                    name: name_of(s),
                });
            }
        }
        // 动作树：注入不了的键、恒不成立的前台/设备条件（`If` 可嵌套，递归扫）。
        let mut bad_inject: BTreeSet<Key> = BTreeSet::new();
        let mut bad_frontmost = false;
        let mut bad_device = false;
        scan_actions_platform(
            &s.actions,
            caps,
            &mut bad_inject,
            &mut bad_frontmost,
            &mut bad_device,
        );
        if !bad_inject.is_empty() {
            out.push(Conflict {
                severity: Severity::Warn,
                message: format!(
                    "动作要用到{}，当前平台注入不了这个键，执行到这一步会没有输出",
                    key_list(&bad_inject)
                ),
                name: name_of(s),
            });
        }
        if bad_frontmost {
            out.push(Conflict {
                severity: Severity::Warn,
                message: "动作里的「前台应用 / 窗口标题」条件在当前平台取不到前台窗口，恒不成立（恒走「否则」分支）".into(),
                name: name_of(s),
            });
        }
        if bad_device {
            out.push(Conflict {
                severity: Severity::Warn,
                message: "动作里的「设备是」条件在当前平台取不到设备，恒不成立（恒走「否则」分支）".into(),
                name: name_of(s),
            });
        }
    }

    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        if let Ok(k) = r.from.parse::<Key>() {
            if caps.listen_unsupported.contains(&k) {
                out.push(Conflict {
                    severity: Severity::Warn,
                    message: format!(
                        "改键来源「{}」当前平台监听不到，这条改键不会生效",
                        r.from
                    ),
                    name: String::new(),
                });
            }
        }
        // 各形态的输出目标（to/tap/hold/oneshot/sticky/tap2/tap3）都是要注入的键。
        let mut bad: BTreeSet<Key> = BTreeSet::new();
        let fields = [
            r.to.as_str(),
            r.tap.as_deref().unwrap_or(""),
            r.hold.as_deref().unwrap_or(""),
            r.oneshot.as_deref().unwrap_or(""),
            r.sticky.as_deref().unwrap_or(""),
            r.tap2.as_deref().unwrap_or(""),
            r.tap3.as_deref().unwrap_or(""),
        ];
        for field in fields {
            if let Ok(k) = field.trim().parse::<Key>() {
                if caps.inject_unsupported.contains(&k) {
                    bad.insert(k);
                }
            }
        }
        if !bad.is_empty() {
            out.push(Conflict {
                severity: Severity::Warn,
                message: format!(
                    "改键「{} → {}」的目标键当前平台注入不了，触发后没有输出",
                    r.from,
                    key_list(&bad)
                ),
                name: String::new(),
            });
        }
    }
    out
}

/// 一个 [`Shortcut`]（组合键）用到的全部键：主键 + 修饰键。
fn shortcut_keys(sc: &Shortcut) -> impl Iterator<Item = Key> + '_ {
    std::iter::once(sc.key).chain(sc.mods.iter().map(|m| modifier_key(*m)))
}

/// 把组合键里监听不到的键收进 `bad`。
fn collect_unlistenable(sc: &Shortcut, caps: &PlatformCaps, bad: &mut BTreeSet<Key>) {
    for k in shortcut_keys(sc) {
        if caps.listen_unsupported.contains(&k) {
            bad.insert(k);
        }
    }
}

/// 递归扫描动作树，收集注入不了的键（`Keys` 成员）与恒不成立的前台 / 设备条件（`If` 条件）。
/// 基础版没有 `If`/`Condition`，两个 `bad_*` 旗标无从写入。
#[cfg_attr(not(feature = "automation"), allow(unused_variables))]
fn scan_actions_platform(
    actions: &[Action],
    caps: &PlatformCaps,
    bad_inject: &mut BTreeSet<Key>,
    bad_frontmost: &mut bool,
    bad_device: &mut bool,
) {
    for a in actions {
        match a {
            Action::Keys { keys, .. } => {
                for k in keys {
                    if let Ok(key) = k.trim().parse::<Key>() {
                        if caps.inject_unsupported.contains(&key) {
                            bad_inject.insert(key);
                        }
                    }
                }
            }
            #[cfg(feature = "automation")]
            Action::If { condition, then, otherwise, .. } => {
                if matches!(
                    condition,
                    Condition::FrontmostApp { .. }
                        | Condition::NotFrontmostApp { .. }
                        | Condition::WindowTitleContains { .. }
                ) && !caps.frontmost_conditions
                {
                    *bad_frontmost = true;
                }
                if matches!(condition, Condition::DeviceIs { .. }) && !caps.device_conditions {
                    *bad_device = true;
                }
                scan_actions_platform(then, caps, bad_inject, bad_frontmost, bad_device);
                scan_actions_platform(otherwise, caps, bad_inject, bad_frontmost, bad_device);
            }
            _ => {}
        }
    }
}

/// 常见系统快捷键（Windows，中性键名：`Meta` = Win / ⌘ 键）+ 用途说明。
/// 用于检测咔哒快捷键与系统快捷键的冲突——精确匹配组合键（修饰键 + 主键都相同才算）。
pub const SYSTEM_SHORTCUTS: &[(&str, &str)] = &[
    ("Meta+E", "文件资源管理器"),
    ("Meta+R", "运行"),
    ("Meta+L", "锁定电脑"),
    ("Meta+D", "显示桌面"),
    ("Meta+I", "设置"),
    ("Meta+S", "搜索"),
    ("Meta+X", "快速链接菜单"),
    ("Meta+V", "剪贴板历史"),
    ("Meta+A", "通知中心"),
    ("Meta+N", "通知"),
    ("Meta+P", "投影"),
    ("Meta+K", "连接设备"),
    ("Meta+G", "游戏栏"),
    ("Meta+Tab", "任务视图"),
    ("Meta+Shift+S", "截图"),
    ("Meta+.", "表情符号面板"),
    ("Meta+,", "桌面预览"),
    ("Alt+Tab", "切换窗口"),
    ("Alt+F4", "关闭窗口"),
    ("Alt+Space", "窗口菜单"),
    ("Alt+Enter", "属性"),
    ("Ctrl+Esc", "开始菜单"),
    ("Ctrl+Shift+Esc", "任务管理器"),
    ("Ctrl+Alt+Delete", "安全选项"),
    ("Ctrl+Alt+Tab", "持久任务切换"),
];

/// 文件属性对象：`GetFileProps` 动作把文件/目录的属性存入变量，供后续动作引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileObject {
    /// 文件名（含扩展名）。
    pub name: String,
    /// 完整路径。
    pub path: String,
    /// 所在目录。
    pub dir: String,
    /// 不含扩展名的主干名。
    pub stem: String,
    /// 扩展名（含点；无扩展名为空）。
    pub ext: String,
    /// 文件大小（字节；目录为 0）。
    pub size: u64,
    /// 修改时间（Unix 秒）。
    pub modified: u64,
    /// 是否为目录。
    pub is_dir: bool,
}

/// 变量值：文件属性 / 布尔 / 文本，供后续动作用占位符引用。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// 文件属性对象（`GetFileProps` 写入）。
    File(FileObject),
    /// 布尔值（如「应用是否在运行」，`App::Status` 写入）。
    Bool(bool),
    /// 文本值（`Command` 动作写入：`text`=标准输出，`exit_code`=退出码）。
    Text(TextValue),
}

/// 文本变量的值：正文 + 可选退出码（仅命令结果变量带退出码，供 `{name.exit_code}` 引用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextValue {
    /// 文本正文（命令标准输出，去首尾空白与换行）。
    pub text: String,
    /// 命令退出码；普通文本为 `None`。
    pub exit_code: Option<i32>,
}

impl From<&str> for TextValue {
    fn from(s: &str) -> Self {
        TextValue { text: s.to_string(), exit_code: None }
    }
}

impl From<String> for TextValue {
    fn from(text: String) -> Self {
        TextValue { text, exit_code: None }
    }
}

/// 变量表：变量名 → 变量值。
pub type Vars = BTreeMap<String, Value>;

/// 用变量表替换字符串中的占位符：`{name}` → 完整路径，`{name.field}` → 对应字段。
/// 未知变量/字段保持原样（不破坏用户输入的字面 `{...}`）。
pub fn substitute_vars(s: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let token = &after[..end];
                match resolve_var(token, vars) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

fn resolve_var(token: &str, vars: &Vars) -> Option<String> {
    let (name, field) = match token.split_once('.') {
        Some((n, f)) => (n, Some(f)),
        None => (token, None),
    };
    var_field(name, field.unwrap_or(""), vars)
}

/// 读取变量 `var` 的某个字段值（`field` 为空时取变量整体值：文件→完整路径、
/// 布尔→"true"/"false"、文本→原文；`field="exit_code"` 时文本变量取命令退出码）。
/// 变量或字段不存在返回 None。
pub fn var_field(var: &str, field: &str, vars: &Vars) -> Option<String> {
    match vars.get(var)? {
        Value::File(obj) => file_field(obj, field),
        Value::Bool(b) => match field {
            "" | "value" => Some(b.to_string()),
            _ => None,
        },
        Value::Text(v) => match field {
            "" | "value" => Some(v.text.clone()),
            "exit_code" => v.exit_code.map(|c| c.to_string()),
            _ => None,
        },
    }
}

fn file_field(obj: &FileObject, field: &str) -> Option<String> {
    Some(match field {
        "" | "path" => obj.path.clone(),
        "name" => obj.name.clone(),
        "dir" => obj.dir.clone(),
        "stem" => obj.stem.clone(),
        "ext" => obj.ext.clone(),
        "size" => obj.size.to_string(),
        "modified" => obj.modified.to_string(),
        "is_dir" => obj.is_dir.to_string(),
        _ => return None,
    })
}

/// 把旧版动作迁移为新版（幂等）：`Launch` → `App::Launch`、`CloseProgram` → `App::Close`、
/// `Cmd` → `Command(Cmd)`、`Powershell` → `Command(Powershell)`、`OpenFolder` → `Os::OpenFolder`，
/// 递归处理 `If` 的嵌套动作。
pub fn migrate_action(a: Action) -> Action {
    match a {
        #[cfg(feature = "automation")]
        Action::Launch { program, args, description } => {
            Action::App { operation: AppOperation::Launch { program, args }, description }
        }
        #[cfg(feature = "automation")]
        Action::CloseProgram { program, description } => {
            Action::App { operation: AppOperation::Close { program }, description }
        }
        #[cfg(feature = "automation")]
        Action::Cmd { command, show_output, var, description } => Action::Command {
            shell: Shell::Cmd,
            command,
            show_output,
            var,
            description,
        },
        #[cfg(feature = "automation")]
        Action::Powershell { command, show_output, var, description } => Action::Command {
            shell: Shell::Powershell,
            command,
            show_output,
            var,
            description,
        },
        #[cfg(feature = "automation")]
        Action::OpenFolder { path, description } => {
            Action::Os { operation: OsOperation::OpenFolder { path }, description }
        }
        #[cfg(feature = "automation")]
        Action::If { condition, then, otherwise, description } => Action::If {
            condition,
            then: then.into_iter().map(migrate_action).collect(),
            otherwise: otherwise.into_iter().map(migrate_action).collect(),
            description,
        },
        other => other,
    }
}

impl Config {
    /// 原地归一化：把旧版动作迁移为 `App` 动作，并把设置的越界值钳回合法区间。
    pub fn migrate(&mut self) {
        for s in &mut self.shortcuts {
            s.actions = std::mem::take(&mut s.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
        }
        self.settings.hints = std::mem::take(&mut self.settings.hints).normalized();
    }
}

/// 清洗目录：去重（首个生效）、剔除空 id/空名、把指向不存在目录的 `parent` 置 None、
/// 打断父链成环（置为根级）。返回清洗后的目录与每处被忽略内容的说明。
fn clean_folders(folders: &[Folder]) -> (Vec<Folder>, Vec<String>) {
    use std::collections::HashMap;

    let mut out: Vec<Folder> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for f in folders {
        let id = f.id.trim().to_string();
        let name = f.name.trim().to_string();
        if id.is_empty() || name.is_empty() {
            ignored.push(format!("目录「{}」已忽略：id 或名称为空", name));
            continue;
        }
        if !seen.insert(id.clone()) {
            ignored.push(format!("目录「{name}」已忽略：id 重复"));
            continue;
        }
        out.push(Folder { id, name, parent: f.parent.clone() });
    }

    let ids: BTreeSet<String> = out.iter().map(|f| f.id.clone()).collect();
    let parent_of: HashMap<String, Option<String>> = out
        .iter()
        .map(|f| (f.id.clone(), f.parent.clone()))
        .collect();

    for f in &mut out {
        let mut cur = f.parent.clone();
        let mut hops: BTreeSet<String> = BTreeSet::new();
        let mut broken = false;
        while let Some(p) = cur {
            if p == f.id || !ids.contains(&p) || !hops.insert(p.clone()) {
                broken = true;
                break;
            }
            cur = parent_of.get(&p).cloned().flatten();
        }
        if broken {
            f.parent = None;
            ignored.push(format!("目录「{}」的父目录链无效（成环或不存在），已置为根级", f.name));
        }
    }

    (out, ignored)
}

fn clean_layers(layers: &[Layer]) -> (Vec<Layer>, Vec<String>) {
    let mut out: Vec<Layer> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for l in layers {
        let id = l.id.trim().to_string();
        let name = l.name.trim().to_string();
        if id.is_empty() || name.is_empty() {
            ignored.push(format!("层「{name}」已忽略：id 或名称为空"));
            continue;
        }
        if !seen.insert(id.clone()) {
            ignored.push(format!("层「{name}」已忽略：id 重复"));
            continue;
        }
        out.push(Layer { id, name });
    }

    (out, ignored)
}

/// 清洗配置：先迁移旧版动作，再丢弃无法解析的触发键/动作/改键，返回可安全保存的
/// 配置与每处被忽略内容的说明（供 UI 提示）。
///
/// 用于保证「单条配置有问题不影响其它配置保存」：坏条目被单独忽略，不再拖垮整份保存。
pub fn sanitize_config(cfg: &Config) -> (Config, Vec<String>) {
    let mut cfg = cfg.clone();
    cfg.migrate();
    let mut ignored: Vec<String> = Vec::new();

    let (folders, folder_ignored) = clean_folders(&cfg.folders);
    ignored.extend(folder_ignored);
    let valid_folder_ids: BTreeSet<String> = folders.iter().map(|f| f.id.clone()).collect();

    let (layers, layer_ignored) = clean_layers(&cfg.layers);
    ignored.extend(layer_ignored);
    let valid_layer_ids: BTreeSet<String> = layers.iter().map(|l| l.id.clone()).collect();

    let mut out = Config { settings: cfg.settings.clone(), folders, layers, ..Default::default() };

    for s in &cfg.shortcuts {
        let label = s.name.clone().unwrap_or_else(|| "（未命名）".into());
        let mut item = s.clone();

        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("快捷键「{label}」所属目录已忽略：目录不存在"));
                item.folder = None;
            }
        }

        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("快捷键「{label}」所属层已忽略：层不存在"));
                item.layer = None;
            }
        }

        let mut kept_triggers = Vec::new();
        for t in &item.triggers {
            match Trigger::parse(t) {
                Ok(_) => kept_triggers.push(t.clone()),
                Err(e) => ignored.push(format!("快捷键「{label}」的触发键「{t}」已忽略：{e}")),
            }
        }
        item.triggers = kept_triggers;

        let mut kept_actions = Vec::new();
        for a in &item.actions {
            match a.validate() {
                Ok(()) => kept_actions.push(a.clone()),
                Err(e) => ignored.push(format!("快捷键「{label}」的某个动作已忽略：{e}")),
            }
        }
        item.actions = kept_actions;

        let has_content = !item.triggers.is_empty()
            || !item.actions.is_empty()
            || item.name.is_some()
            || item.description.is_some();
        if has_content {
            out.shortcuts.push(item);
        } else {
            ignored.push(format!("快捷键「{label}」已忽略：无触发键、动作或名称"));
        }
    }

    for r in &cfg.remaps {
        let ok_from = r.from.parse::<Key>().is_ok();
        if !ok_from {
            ignored.push(format!("改键「{}」已忽略：原键无法解析", r.from));
            continue;
        }
        let mut item = r.clone();

        // 归属层 / 长按切层引用校验（断引用清空）。
        if let Some(lid) = &item.layer {
            if !valid_layer_ids.contains(lid) {
                ignored.push(format!("改键「{}」所属层已忽略：层不存在", r.from));
                item.layer = None;
            }
        }
        if let Some(hl) = &item.hold_layer {
            if !valid_layer_ids.contains(hl) {
                ignored.push(format!("改键「{}」的长按切层已忽略：层不存在", r.from));
                item.hold_layer = None;
            }
        }
        if let Some(ll) = &item.lock_layer {
            if !valid_layer_ids.contains(ll) {
                ignored.push(format!("改键「{}」的长按锁定层已忽略：层不存在", r.from));
                item.lock_layer = None;
            }
        }
        // 长按进入层（momentary）与长按锁定层（切换式）互斥：两者都设时保留切换式——层内
        // 放和弦/序列时 momentary 要同时按住切层键凑 3~4 个键，只有切换式用得起来。
        let nonblank = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
        if nonblank(&item.hold_layer) && nonblank(&item.lock_layer) {
            ignored.push(format!(
                "改键「{}」同时设了长按进入层与长按锁定层，保留长按锁定层（切换式切层）",
                r.from
            ));
            item.hold_layer = None;
        }

        // 单次/粘滞/双击/三击键名逐项校验（坏键名单独清空并提示）。
        let mut clear_key = |field: &mut Option<String>, label: &str| {
            if let Some(v) = field {
                if v.parse::<Key>().is_err() {
                    ignored.push(format!("改键「{}」的{label}「{v}」已忽略：键名无法解析", r.from));
                    *field = None;
                }
            }
        };
        clear_key(&mut item.oneshot, "单次键");
        clear_key(&mut item.sticky, "粘滞键");
        clear_key(&mut item.tap2, "双击键");
        clear_key(&mut item.tap3, "三击键");

        // 归一化（优先级 sticky > oneshot > tap-hold > 普通 to）：修饰模式清掉其它形态字段。
        if item.is_sticky() || item.is_oneshot() {
            if item.is_sticky() && item.is_oneshot() {
                ignored.push(format!("改键「{}」同时设了粘滞与单次修饰，保留粘滞", r.from));
                item.oneshot = None;
            }
            let mode = if item.is_sticky() { "粘滞" } else { "单次" };
            if item.is_tap_hold() || !item.to.trim().is_empty() {
                ignored.push(format!("改键「{}」已归一化为{mode}修饰（忽略短按/长按/双击/三击/切层/普通改键）", r.from));
            }
            item.tap = None;
            item.hold = None;
            item.tap2 = None;
            item.tap3 = None;
            item.hold_layer = None;
            item.lock_layer = None;
            item.to = String::new();
            out.remaps.push(item);
        } else if item.is_tap_hold() {
            // tap-hold / 切层：短按/长按键名逐项校验，坏的单独清空并提示。
            if let Some(t) = &item.tap {
                if t.parse::<Key>().is_err() {
                    ignored.push(format!("改键「{}」的短按键「{t}」已忽略：键名无法解析", r.from));
                    item.tap = None;
                }
            }
            if let Some(h) = &item.hold {
                if h.parse::<Key>().is_err() {
                    ignored.push(format!("改键「{}」的长按键「{h}」已忽略：键名无法解析", r.from));
                    item.hold = None;
                }
            }
            if item.is_tap_hold() {
                out.remaps.push(item);
            } else {
                ignored.push(format!("改键「{}」已忽略：短按/长按键名均无法解析", r.from));
            }
        } else if item.to.parse::<Key>().is_ok() {
            out.remaps.push(item);
        } else if item.to.trim().is_empty() {
            // 没有普通「改为」、也没落下任何有效形态（多半是「长按进入层」指向的层不存在，
            // 那条提示已在上面给过）。说「键名无法解析」会把人引偏，直接说没有可用输出。
            ignored.push(format!(
                "改键「{}」已忽略：没有可用的输出（改为/短按/长按/切层都为空或已失效）",
                item.from
            ));
        } else {
            ignored.push(format!("改键「{} → {}」已忽略：键名无法解析", item.from, item.to));
        }
    }

    for e in &cfg.expansions {
        match e.validate() {
            Ok(()) => out.expansions.push(e.clone()),
            Err(err) => ignored.push(format!("文本扩展「{}」已忽略：{err}", e.trigger)),
        }
    }

    // 唤醒键：无法解析成单键时清空（关闭快速唤醒），不拖垮整份配置。
    if let Some(wake) = out.settings.wake_key.as_deref() {
        if wake.parse::<Key>().is_err() {
            ignored.push(format!("唤醒键「{wake}」已忽略：键名无法解析"));
            out.settings.wake_key = None;
        }
    }

    (out, ignored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_roundtrip() {
        let cases = [
            "Ctrl+K",
            "Ctrl+Alt+Shift+Space",
            "A",
            "Alt+F4",
            "Shift+CapsLock", // Shift 作为修饰键 + CapsLock 主键
            "Meta+J",
            "Ctrl+-",
            "Alt+/",
            "Shift", // 裸修饰键（改键场景）
            "Ctrl",
        ];
        for s in cases {
            let st: Shortcut = s.parse().unwrap();
            let back = format_shortcut(&st);
            assert_eq!(back, s, "roundtrip {s} -> {back}");
        }
    }

    #[test]
    fn parse_errors() {
        assert!(matches!("".parse::<Shortcut>(), Err(ParseError::Empty)));
        assert!(matches!("+".parse::<Shortcut>(), Err(ParseError::Empty)));
        assert!(matches!("Foo+K".parse::<Shortcut>(), Err(_)));
        assert!(matches!("Ctrl+K+J".parse::<Shortcut>(), Err(_)));
    }

    #[test]
    fn new_keys_roundtrip_and_aliases() {
        // 新增键：解析 → 渲染回环必须一致。
        let cases = [
            "F13", "F24", "MediaPlayPause", "MediaPrev", "MediaNext",
            "VolumeMute", "VolumeDown", "VolumeUp", "NumLock",
            "Numpad0", "Numpad9", "NumpadAdd", "NumpadSubtract",
            "NumpadMultiply", "NumpadDivide", "NumpadDecimal", "NumpadEnter",
            "MouseMiddle", "MouseBack", "MouseForward",
        ];
        for s in cases {
            let k: Key = s.parse().unwrap();
            assert_eq!(key_name(k), s, "roundtrip {s}");
        }
        // 别名解析到同一键，且主键名渲染为规范名。
        assert_eq!("MB4".parse::<Key>().unwrap(), Key::MouseBack);
        assert_eq!(key_name("XButton2".parse::<Key>().unwrap()), "MouseForward");
        assert_eq!("NumpadPlus".parse::<Key>().unwrap(), Key::NumpadAdd);
        assert_eq!("VolUp".parse::<Key>().unwrap(), Key::VolumeUp);
        // 组合键触发：无修饰的鼠标键与带修饰的媒体键都能解析。
        let m: Shortcut = "Ctrl+VolumeUp".parse().unwrap();
        assert_eq!(format_shortcut(&m), "Ctrl+VolumeUp");
        let bare: Shortcut = "MouseBack".parse().unwrap();
        assert_eq!(format_shortcut(&bare), "MouseBack");
    }

    #[test]
    fn matcher_superset_rule() {
        let ctrl_k: Shortcut = "Ctrl+K".parse().unwrap();
        let ev = |key: Key, mods: &[Modifier], pressed: bool| RawEvent {
            key,
            mods: mods.iter().copied().collect(),
            pressed,
        };
        // Ctrl+K 命中
        assert!(matches(&ev(Key::K, &[Modifier::Ctrl], true), &ctrl_k));
        // Ctrl+Shift+K 也命中（修饰键超集）
        assert!(matches(
            &ev(Key::K, &[Modifier::Ctrl, Modifier::Shift], true),
            &ctrl_k
        ));
        // 缺修饰不命中
        assert!(!matches(&ev(Key::K, &[], true), &ctrl_k));
        assert!(!matches(&ev(Key::K, &[Modifier::Alt], true), &ctrl_k));
        // 主键不同不命中
        assert!(!matches(&ev(Key::J, &[Modifier::Ctrl], true), &ctrl_k));
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn config_json_roundtrip() {
        let cfg = Config {
            folders: vec![],
            layers: vec![],
            shortcuts: vec![ShortcutItem {
                name: Some("地址".into()),
                description: Some("常用收货地址".into()),
                triggers: vec!["Ctrl+Alt+K".into(), "Alt+K".into()],
                actions: vec![Action::Text { text: "上海市徐汇区……".into(), mode: TextMode::Input, description: None }],
                folder: None,
                layer: None,
                enabled: true,
            }],
            remaps: vec![Remap { from: "CapsLock".into(), to: "Ctrl".into(), enabled: true, ..Default::default() }],
            expansions: vec![],
            settings: Settings::default(),
        };
        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg, "json:\n{json}");
    }

    #[test]
    fn config_missing_fields_default() {
        // 手工编辑的配置允许缺字段、带未知字段。
        let json = r#"{
            "shortcuts": [{ "triggers": ["Ctrl+K"], "actions": [{ "type": "text", "text": "hi" }] }],
            "extra_field": 1
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.shortcuts[0].enabled, false);
        assert_eq!(
            cfg.shortcuts[0].actions,
            vec![Action::Text { text: "hi".into(), mode: TextMode::Input, description: None }]
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn actions_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Text { text: "你好".into(), mode: TextMode::Input, description: None },
            Action::Cmd { command: "echo hi".into(), show_output: false, var: "".into(), description: None },
            Action::Powershell { command: "Get-Date".into(), show_output: true, var: "".into(), description: None },
            Action::Launch { program: "notepad.exe".into(), args: vec!["a.txt".into()], description: None },
            Action::OpenFolder { path: "C:\\Users".into(), description: None },
            Action::Keys { keys: vec!["Ctrl".into(), "C".into()], description: None },
            Action::PauseMs { ms: 100, description: None },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 坏键名 / 空组合 / 空命令必被拒绝
        assert!(Action::Keys { keys: vec!["NotAKey".into()], description: None }.validate().is_err());
        assert!(Action::Keys { keys: vec![], description: None }.validate().is_err());
        assert!(Action::Cmd { command: "  ".into(), show_output: false, var: "".into(), description: None }.validate().is_err());
        assert!(Action::Launch { program: "".into(), args: vec![], description: None }.validate().is_err());
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;

    fn item(trigger: &str) -> ShortcutItem {
        ShortcutItem {
            triggers: vec![trigger.into()],
            actions: vec![Action::Text { text: String::new(), mode: TextMode::Input, description: None }],
            enabled: true,
            ..Default::default()
        }
    }

    fn remap(from: &str, to: &str) -> Remap {
        Remap { from: from.into(), to: to.into(), enabled: true, ..Default::default() }
    }

    fn errors(cfg: &Config) -> Vec<Conflict> {
        detect_conflicts(cfg, &PlatformCaps::default())
            .into_iter()
            .filter(|c| c.severity == Severity::Error)
            .collect()
    }

    fn warns(cfg: &Config) -> Vec<Conflict> {
        detect_conflicts(cfg, &PlatformCaps::default())
            .into_iter()
            .filter(|c| c.severity == Severity::Warn)
            .collect()
    }

    #[test]
    fn duplicate_triggers_are_error() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Ctrl+K")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn duplicate_remap_from_is_error() {
        let cfg = Config {
            remaps: vec![remap("CapsLock", "Ctrl"), remap("CapsLock", "Esc")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn remap_from_shadowing_shortcut_is_error() {
        let cfg = Config {
            shortcuts: vec![item("CapsLock")],
            remaps: vec![remap("CapsLock", "Ctrl")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn superset_overlap_is_warn_only() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Ctrl+Shift+K")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty());
        assert!(!warns(&cfg).is_empty());
    }

    #[test]
    fn clean_config_has_no_conflicts() {
        let cfg = Config {
            shortcuts: vec![item("Ctrl+K"), item("Alt+J")],
            remaps: vec![remap("CapsLock", "Ctrl")],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[test]
    fn sequence_duplicate_is_error() {
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9 J K")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn sequence_leader_shadows_combo_is_error() {
        // 序列「F9 J K」的 leader「F9」会吞掉 F9，使同名组合键「F9」失效。
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn sequence_no_superset_warn() {
        // 超集软冲突仅对单组合生效，序列跳过：F9 序列 + F9 组合应只有硬冲突（leader 遮蔽），
        // 不应额外产生超集 Warn。
        let cfg = Config {
            shortcuts: vec![item("F9 J K"), item("F9")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_duplicate_is_error_unordered() {
        // F&J 与 J&F 等价（无序），判重。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("J&F")],
            ..Default::default()
        };
        assert_eq!(errors(&cfg).len(), 1);
    }

    #[test]
    fn chord_member_shadows_combo_is_error() {
        // 和弦成员 F 会吞掉 F，使同名组合键 F 失效。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn chord_member_shadows_sequence_leader_is_error() {
        // 和弦成员 F 会吞掉 F，使序列「F G」的 leader 失效。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F G")],
            ..Default::default()
        };
        let errs = errors(&cfg);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        assert!(errs[0].message.contains("吞掉"), "msg: {}", errs[0].message);
    }

    #[test]
    fn chord_member_as_modifier_does_not_shadow_combo() {
        // 修饰键成员从不被吞（吞掉 Ctrl 会把 Ctrl+Alt+Tab 变成 Ctrl+Tab），
        // 所以它遮蔽不了以该修饰键为「主键」的组合。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+Alt&J"), item("Ctrl")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty(), "errs: {:?}", errors(&cfg));
    }

    #[test]
    fn chord_modifier_member_is_not_a_duplicate_of_bare_chord() {
        // F&J 与 Ctrl+F&J 是两条不同的触发键（后者多一个修饰要求），不是重复。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("Ctrl+F&J")],
            ..Default::default()
        };
        assert!(errors(&cfg).is_empty(), "errs: {:?}", errors(&cfg));
    }

    #[test]
    fn chord_superset_is_warned() {
        // F&J 会先凑齐 → 三成员的 F&J&K 永远轮不到（软冲突：两边都合法，只是有一个不生效）。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("F&J&K")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("永远"), "msg: {}", ws[0].message);
        assert!(ws[0].message.contains("F&J&K"), "msg: {}", ws[0].message);
        assert_eq!(ws[0].name, "", "name 指向永远不生效的那条（这里没起名）");

        // 书写顺序反过来照样报：成员更少的那个必然先凑齐，与顺序无关。
        let cfg = Config {
            shortcuts: vec![item("F&J&K"), item("F&J")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("F&J&K"), "msg: {}", ws[0].message);
    }

    #[test]
    fn chord_same_keys_lighter_mods_wins_by_order() {
        // 成员键相同、只差修饰要求：同一刻凑齐，靠前者先被判到 → 写在后面的那条轮不到。
        let cfg = Config {
            shortcuts: vec![item("F&J"), item("Ctrl+F&J")],
            ..Default::default()
        };
        let ws = warns(&cfg);
        assert_eq!(ws.len(), 1, "warns: {ws:?}");
        assert!(ws[0].message.contains("Ctrl+F&J"), "msg: {}", ws[0].message);

        // 反过来（更严的写在前面）：不按 Ctrl 时更宽松的那条仍能触发 → 两条都可达，不报。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+F&J"), item("F&J")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_different_mods_do_not_conflict() {
        // 修饰要求不同（互不包含）：各自在修饰键按下时生效，两条都可达。
        let cfg = Config {
            shortcuts: vec![item("Ctrl+F&J"), item("Alt+F&J")],
            ..Default::default()
        };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }

    #[test]
    fn chord_superset_only_within_same_layer() {
        // 不同层各自生效，不算遮蔽（与「同层内」的其余冲突判定口径一致）。
        let mut hi = item("F&J");
        hi.layer = Some("L1".into());
        let mut lo = item("F&J&K");
        lo.layer = Some("L2".into());
        let cfg = Config { shortcuts: vec![hi, lo], ..Default::default() };
        assert!(warns(&cfg).is_empty(), "warns: {:?}", warns(&cfg));
    }
}

#[cfg(test)]
mod settings_text_mode_tests {
    use super::*;

    #[test]
    fn text_inject_mode_parses_tolerantly() {
        let parse = |s: &str| serde_json::from_str::<Settings>(s).unwrap().text_inject_mode;
        assert_eq!(parse(r#"{"text_inject_mode":"unicode"}"#), TextInjectMode::Unicode);
        assert_eq!(parse(r#"{"text_inject_mode":"clipboard"}"#), TextInjectMode::Clipboard);
        // 老配置缺字段 = 默认剪贴板（7.3-㉒ 之前一直只有这一种方式）。
        assert_eq!(parse("{}"), TextInjectMode::Clipboard);
        // 手改写错值回落默认，不让整份配置解析失败（加载路径对解析失败是硬处理）。
        assert_eq!(parse(r#"{"text_inject_mode":"unicod"}"#), TextInjectMode::Clipboard);
        // 序列化用小写值，与解析约定一致。
        assert_eq!(serde_json::to_string(&TextInjectMode::Unicode).unwrap(), r#""unicode""#);
        assert_eq!(serde_json::to_string(&TextInjectMode::Clipboard).unwrap(), r#""clipboard""#);
    }
}

#[cfg(test)]
mod os_and_sanitize_tests {
    use super::*;

    #[cfg(feature = "automation")]
    #[test]
    fn os_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Os {
                operation: OsOperation::Copy { source: "C:\\a".into(), dest: "D:\\b".into() },
                description: None,
            },
            Action::Os {
                operation: OsOperation::GetFileProps { path: "C:\\f.txt".into(), var: "f".into() },
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 空路径必被拒绝；变量名允许为空（回退默认）。
        assert!(Action::Os {
            operation: OsOperation::Copy { source: "".into(), dest: "".into() },
            description: None,
        }
        .validate()
        .is_err());
        assert!(Action::Os { operation: OsOperation::Delete { path: "  ".into() }, description: None }
            .validate()
            .is_err());
        assert!(Action::Os {
            operation: OsOperation::GetFileProps { path: "x".into(), var: "".into() },
            description: None,
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn substitute_vars_expands_placeholders() {
        let mut vars = Vars::new();
        vars.insert(
            "f".into(),
            Value::File(FileObject {
                name: "a.txt".into(),
                path: "C:\\d\\a.txt".into(),
                dir: "C:\\d".into(),
                stem: "a".into(),
                ext: ".txt".into(),
                size: 12,
                modified: 0,
                is_dir: false,
            }),
        );
        assert_eq!(substitute_vars("{f}", &vars), "C:\\d\\a.txt");
        assert_eq!(substitute_vars("{f.name}", &vars), "a.txt");
        assert_eq!(substitute_vars("{f.ext}", &vars), ".txt");
        assert_eq!(substitute_vars("{f.size}", &vars), "12");
        assert_eq!(substitute_vars("{f.missing}", &vars), "{f.missing}");
        assert_eq!(substitute_vars("{unknown}", &vars), "{unknown}");
        assert_eq!(
            substitute_vars("copy {f} to {f.dir}", &vars),
            "copy C:\\d\\a.txt to C:\\d"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn sanitize_drops_invalid_parts_keeps_valid() {
        let cfg = Config {
            folders: vec![],
            layers: vec![],
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+K".into()],
                    actions: vec![Action::Text { text: "ok".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Bad+Key".into()],
                    actions: vec![Action::Text { text: "bad".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Cmd { command: "  ".into(), show_output: false, var: "".into(), description: None }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            remaps: vec![
                Remap { from: "CapsLock".into(), to: "Ctrl".into(), enabled: true, ..Default::default() },
                Remap { from: "NotAKey".into(), to: "Ctrl".into(), enabled: true, ..Default::default() },
            ],
            expansions: vec![],
            settings: Settings::default(),
        };
        let (clean, ignored) = sanitize_config(&cfg);

        // 三条快捷键都保留（各自至少还有有效触发键或有效动作），但坏部分被清掉。
        assert_eq!(clean.shortcuts.len(), 3, "ignored: {}", ignored.join("; "));
        assert_eq!(clean.shortcuts[0].triggers, vec!["Ctrl+K".to_string()]);
        assert_eq!(clean.shortcuts[0].actions.len(), 1);
        assert!(clean.shortcuts[1].triggers.is_empty(), "无效触发键应被清空");
        assert_eq!(clean.shortcuts[1].actions.len(), 1);
        assert!(clean.shortcuts[2].actions.is_empty(), "无效动作应被清空");
        assert_eq!(clean.shortcuts[2].triggers, vec!["Ctrl+J".to_string()]);

        assert_eq!(clean.remaps.len(), 1, "无效改键应被丢弃");
        assert!(ignored.iter().any(|m| m.contains("Bad+Key")));
        assert!(ignored.iter().any(|m| m.contains("命令")));
        assert!(ignored.iter().any(|m| m.contains("NotAKey")));
    }

    #[test]
    fn sanitize_wake_key_validates_and_clears_invalid() {
        // 合法唤醒键保留；非法（非单键）清空并提示。
        let ok = Config {
            settings: Settings { wake_key: Some("Alt".into()), ..Default::default() },
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&ok);
        assert_eq!(clean.settings.wake_key.as_deref(), Some("Alt"));
        assert!(ignored.is_empty());

        let bad = Config {
            settings: Settings { wake_key: Some("Ctrl+Alt".into()), ..Default::default() },
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&bad);
        assert_eq!(clean.settings.wake_key, None);
        assert!(ignored.iter().any(|m| m.contains("唤醒键")));
    }

    #[test]
    fn system_shortcut_list_all_parse() {
        // 清单里的每条都必须能解析成合法 Shortcut，且彼此不重复（防笔误）。
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for (combo, desc) in SYSTEM_SHORTCUTS {
            let sc = combo
                .parse::<Shortcut>()
                .unwrap_or_else(|e| panic!("系统快捷键「{combo}」（{desc}）解析失败: {e}"));
            assert!(seen.insert((sc.mods, sc.key)), "系统快捷键「{combo}」重复");
        }
    }

    #[cfg(feature = "automation")]
    fn sample_vars() -> Vars {
        let mut vars = Vars::new();
        vars.insert(
            "f".into(),
            Value::File(FileObject {
                name: "a.txt".into(),
                path: "C:\\d\\a.txt".into(),
                dir: "C:\\d".into(),
                stem: "a".into(),
                ext: ".txt".into(),
                size: 12,
                modified: 0,
                is_dir: false,
            }),
        );
        vars
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_equals_and_field_resolution() {
        let vars = sample_vars();
        assert!(Condition::Equals { var: "f".into(), field: "ext".into(), value: ".txt".into() }
            .matches(&vars, None, None));
        assert!(Condition::NotEquals { var: "f".into(), field: "ext".into(), value: ".zip".into() }
            .matches(&vars, None, None));
        // field 为空 → 比较完整路径
        assert!(Condition::Equals { var: "f".into(), field: String::new(), value: "C:\\d\\a.txt".into() }
            .matches(&vars, None, None));
        // 变量或字段不存在 → 条件不成立
        assert!(!Condition::Equals { var: "f".into(), field: "nope".into(), value: "x".into() }
            .matches(&vars, None, None));
        assert!(!Condition::Equals { var: "missing".into(), field: String::new(), value: "x".into() }
            .matches(&vars, None, None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_path_predicates() {
        let mut vars = sample_vars();
        // 当前目录（必然存在、是目录）+ 一个必不存在的路径
        assert!(Condition::Exists { path: ".".into() }.matches(&vars, None, None));
        assert!(Condition::IsDir { path: ".".into() }.matches(&vars, None, None));
        assert!(!Condition::IsFile { path: ".".into() }.matches(&vars, None, None));
        assert!(Condition::NotExists { path: "___kada_no_such_path___".into() }.matches(&vars, None, None));
        // 路径支持变量占位符：用指向真实存在的当前目录的变量验证替换。
        vars.insert(
            "cur".into(),
            Value::File(FileObject {
                name: ".".into(),
                path: ".".into(),
                dir: "".into(),
                stem: ".".into(),
                ext: "".into(),
                size: 0,
                modified: 0,
                is_dir: true,
            }),
        );
        assert!(Condition::Exists { path: "{cur}".into() }.matches(&vars, None, None));
        assert!(Condition::IsDir { path: "{cur}".into() }.matches(&vars, None, None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_modified_within() {
        let vars = sample_vars();
        // 新建一个临时文件：刚写入的 mtime 必在最近 10 分钟内（避免用目录 mtime，其只在增删文件时更新）。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!("kada_mtime_{}_{}.txt", std::process::id(), nanos));
        std::fs::write(&tmp, b"x").unwrap();
        let tmp_path = tmp.to_string_lossy().into_owned();
        assert!(Condition::ModifiedWithin { path: tmp_path.clone(), minutes: 10 }.matches(&vars, None, None));
        let _ = std::fs::remove_file(&tmp);

        // 不存在的路径 → 条件不成立
        assert!(!Condition::ModifiedWithin { path: "___kada_no_such_path___".into(), minutes: 10 }
            .matches(&vars, None, None));
        // 路径为空 → 校验拒绝
        assert!(Condition::ModifiedWithin { path: "  ".into(), minutes: 10 }.validate().is_err());
        assert!(Condition::ModifiedWithin { path: tmp_path, minutes: 10 }.validate().is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_frontmost_matching() {
        let ctx = |name: &str, title: &str| FrontmostContext {
            process_name: name.to_string(),
            window_title: title.to_string(),
        };
        let chrome = Some(ctx("chrome.exe", "Kada - Google Chrome"));
        let none: Option<FrontmostContext> = None;

        // 子串匹配（不区分大小写）
        assert!(Condition::FrontmostApp { app: "chrome".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::FrontmostApp { app: "CHROME.EXE".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::FrontmostApp { app: "code".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::NotFrontmostApp { app: "code".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::NotFrontmostApp { app: "chrome".into() }.matches(&Vars::new(), chrome.as_ref(), None));

        // 通配匹配（* / ?）
        assert!(Condition::FrontmostApp { app: "chrome.*".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::FrontmostApp { app: "chr?me.exe".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::FrontmostApp { app: "*.txt".into() }.matches(&Vars::new(), chrome.as_ref(), None));

        // 窗口标题子串（不区分大小写）
        assert!(Condition::WindowTitleContains { text: "chrome".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(Condition::WindowTitleContains { text: "CHROME".into() }.matches(&Vars::new(), chrome.as_ref(), None));
        assert!(!Condition::WindowTitleContains { text: "safari".into() }.matches(&Vars::new(), chrome.as_ref(), None));

        // 无前台上下文 → 前台条件一律不成立
        assert!(!Condition::FrontmostApp { app: "chrome".into() }.matches(&Vars::new(), none.as_ref(), None));
        assert!(!Condition::NotFrontmostApp { app: "code".into() }.matches(&Vars::new(), none.as_ref(), None));
        assert!(!Condition::WindowTitleContains { text: "x".into() }.matches(&Vars::new(), none.as_ref(), None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_device_matching() {
        let dev = Some("AT Translated Set 2 keyboard");
        let other = Some("Logitech USB Keyboard");

        // 子串匹配（不区分大小写）
        assert!(Condition::DeviceIs { id: "set 2".into() }.matches(&Vars::new(), None, dev));
        assert!(Condition::DeviceIs { id: "LOGITECH".into() }.matches(&Vars::new(), None, other));
        assert!(!Condition::DeviceIs { id: "logitech".into() }.matches(&Vars::new(), None, dev));

        // 通配匹配（* / ?）
        assert!(Condition::DeviceIs { id: "AT Translated*".into() }.matches(&Vars::new(), None, dev));
        assert!(Condition::DeviceIs { id: "*Set ? keyboard".into() }.matches(&Vars::new(), None, dev));

        // 无设备上下文（Windows / macOS / 取不到设备）→ 恒不成立
        assert!(!Condition::DeviceIs { id: "keyboard".into() }.matches(&Vars::new(), None, None));

        // 空设备标识 → 校验拒绝
        assert!(Condition::DeviceIs { id: "  ".into() }.validate().is_err());
        assert!(Condition::DeviceIs { id: "keyboard".into() }.validate().is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn if_action_validate_recurses() {
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Text { text: "ok".into(), mode: TextMode::Input, description: None }],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_ok());
        // 条件为空 → 拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: "  ".into() },
            then: vec![],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_err());
        // 嵌套动作非法 → 递归拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Cmd { command: "  ".into(), show_output: false, var: "".into(), description: None }],
            otherwise: vec![],
            description: None,
        }
        .validate()
        .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn if_action_json_roundtrip() {
        // 递归嵌套的 If 也能完整往返（校验 serde tag 与 field 默认值）。
        let a = Action::If {
            condition: Condition::Equals { var: "f".into(), field: "ext".into(), value: ".txt".into() },
            then: vec![Action::Text { text: "yes".into(), mode: TextMode::Input, description: None }],
            otherwise: vec![Action::If {
                condition: Condition::Exists { path: "{f}".into() },
                then: vec![],
                otherwise: vec![Action::PauseMs { ms: 10, description: None }],
                description: None,
            }],
            description: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a, "json: {json}");
    }

    #[cfg(feature = "automation")]
    #[test]
    fn app_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::App {
                operation: AppOperation::Launch { program: "notepad.exe".into(), args: vec!["a.txt".into()] },
                description: None,
            },
            Action::App {
                operation: AppOperation::Status {
                    program: "notepad.exe".into(),
                    var: "running".into(),
                    retries: 3,
                    interval_ms: 500,
                },
                description: None,
            },
            Action::App {
                operation: AppOperation::Restart { program: "notepad.exe".into(), args: vec![] },
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "{a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 程序为空必被拒绝；Status 变量名可空。
        assert!(Action::App { operation: AppOperation::Close { program: "".into() }, description: None }
            .validate()
            .is_err());
        assert!(Action::App {
            operation: AppOperation::Status {
                program: "x".into(),
                var: "".into(),
                retries: 0,
                interval_ms: 0,
            },
            description: None,
        }
        .validate()
        .is_ok());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn open_url_json_roundtrip_and_validate() {
        let a = Action::OpenUrl { url: "https://example.com?a=1&b=2".into(), description: None };
        assert!(a.validate().is_ok());
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains(r#""type":"open_url""#), "json: {json}");
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
        // 网址为空必被拒绝。
        assert!(Action::OpenUrl { url: "".into(), description: None }.validate().is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn migrate_legacy_launch_and_close_program() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Launch { program: "notepad.exe".into(), args: vec![], description: None },
                    Action::CloseProgram { program: "notepad.exe".into(), description: None },
                    Action::If {
                        condition: Condition::Exists { path: ".".into() },
                        then: vec![Action::Launch { program: "x".into(), args: vec![], description: None }],
                        otherwise: vec![],
                        description: None,
                    },
                ],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.migrate();
        let acts = &cfg.shortcuts[0].actions;
        assert!(matches!(acts[0], Action::App { operation: AppOperation::Launch { .. }, description: _ }));
        assert!(matches!(acts[1], Action::App { operation: AppOperation::Close { .. }, description: _ }));
        // 嵌套 If 里的旧动作也迁移
        match &acts[2] {
            Action::If { then, .. } => {
                assert!(matches!(then[0], Action::App { operation: AppOperation::Launch { .. }, description: _ }));
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[cfg(feature = "automation")]
    #[test]
    fn bool_value_substitution_and_compare() {
        let mut vars = Vars::new();
        vars.insert("running".into(), Value::Bool(true));
        vars.insert("text".into(), Value::Text("hello".into()));
        vars.insert("cmd".into(), Value::Text(TextValue { text: "E".into(), exit_code: Some(0) }));
        assert_eq!(substitute_vars("{running}", &vars), "true");
        assert_eq!(substitute_vars("{running.value}", &vars), "true");
        assert_eq!(substitute_vars("{text}", &vars), "hello");
        assert_eq!(substitute_vars("{cmd}", &vars), "E");
        assert_eq!(substitute_vars("{cmd.exit_code}", &vars), "0");
        assert_eq!(substitute_vars("{text.exit_code}", &vars), "{text.exit_code}");
        // 条件判断能比较布尔变量
        assert!(Condition::Equals { var: "running".into(), field: "".into(), value: "true".into() }
            .matches(&vars, None, None));
        assert!(Condition::Equals { var: "text".into(), field: "".into(), value: "hello".into() }
            .matches(&vars, None, None));
    }

    /// `sanitize_config` 的往返稳定性（原规划 7.3-⑰ 点名的缺口）：一份**全合法**的配置
    /// 过清洗后必须原样保留（无 ignored）、能 JSON 往返、且清洗幂等（再洗一次仍不变）。
    /// 已有的一堆用例锁的是「坏条目被丢掉」；这条锁另一面——合法配置不被误伤。
    #[test]
    fn sanitize_valid_config_is_unchanged_roundtrip_and_idempotent() {
        let cfg = Config {
            folders: vec![Folder { id: "f1".into(), name: "工作".into(), parent: None }],
            layers: vec![Layer { id: "l1".into(), name: "导航".into() }],
            shortcuts: vec![ShortcutItem {
                name: Some("测试".into()),
                description: Some("说明".into()),
                folder: Some("f1".into()),
                layer: Some("l1".into()),
                // 组合 / 序列 / 和弦三种触发都合法。
                triggers: vec!["Ctrl+K".into(), "F9 J".into(), "F&J".into()],
                actions: vec![
                    Action::Text { text: "hi".into(), mode: TextMode::Input, description: None },
                    Action::Keys { keys: vec!["Ctrl".into(), "S".into()], description: None },
                    Action::PauseMs { ms: 100, description: None },
                ],
                enabled: true,
            }],
            remaps: vec![
                // tap-hold（短按/长按）与普通改键各一条。
                Remap {
                    from: "CapsLock".into(),
                    tap: Some("Escape".into()),
                    hold: Some("Control".into()),
                    enabled: true,
                    ..Default::default()
                },
                Remap { from: "F1".into(), to: "Home".into(), enabled: true, ..Default::default() },
            ],
            expansions: vec![TextExpansion { trigger: ";addr".into(), replace: "某地".into(), enabled: true }],
            settings: Settings { wake_key: Some("Alt".into()), ..Default::default() },
        };

        let (clean, ignored) = sanitize_config(&cfg);
        assert!(ignored.is_empty(), "合法配置不该有忽略项：{ignored:?}");
        assert_eq!(clean, cfg, "合法配置过清洗后必须原样保留");

        // JSON 往返（配置是跨平台同步介质，保存 / 加载不能变形）。
        let json = serde_json::to_string(&clean).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, clean, "JSON 往返后配置应不变");

        // 幂等：清洗结果再洗一次仍不变、也无提示。
        let (again, ignored2) = sanitize_config(&clean);
        assert!(ignored2.is_empty(), "二次清洗不该有新提示：{ignored2:?}");
        assert_eq!(again, clean, "清洗应幂等");
    }
}

#[cfg(test)]
mod new_action_and_folder_tests {
    use super::*;

    #[cfg(feature = "automation")]
    #[test]
    fn text_mode_and_shell_json_roundtrip() {
        let actions = vec![
            Action::Text { text: "hi".into(), mode: TextMode::Input, description: None },
            Action::Text { text: "".into(), mode: TextMode::ToUpper, description: None },
            Action::Text { text: "".into(), mode: TextMode::ToLower, description: None },
            Action::Command { shell: Shell::Cmd, command: "echo hi".into(), show_output: false, var: "".into(), description: None },
            Action::Command { shell: Shell::Powershell, command: "Get-Date".into(), show_output: true, var: "".into(), description: None },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions, "json: {json}");

        // 旧版 text（无 mode）默认 Input；Command 命令为空被拒绝。
        let legacy: Action = serde_json::from_str(r#"{"type":"text","text":"x"}"#).unwrap();
        assert_eq!(legacy, Action::Text { text: "x".into(), mode: TextMode::Input, description: None });
        assert!(Action::Command { shell: Shell::Cmd, command: "  ".into(), show_output: false, var: "".into(), description: None }
            .validate()
            .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn migrate_cmd_powershell_open_folder() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Cmd { command: "echo hi".into(), show_output: true, var: "out".into(), description: None },
                    Action::Powershell { command: "Get-Date".into(), show_output: false, var: "".into(), description: None },
                    Action::OpenFolder { path: "C:\\Users".into(), description: None },
                ],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.migrate();
        let acts = &cfg.shortcuts[0].actions;
        assert!(matches!(acts[0], Action::Command { shell: Shell::Cmd, .. }));
        assert!(matches!(acts[1], Action::Command { shell: Shell::Powershell, .. }));
        assert!(matches!(acts[2], Action::Os { operation: OsOperation::OpenFolder { .. }, description: _ }));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn os_open_folder_new_folder_validate() {
        assert!(Action::Os { operation: OsOperation::OpenFolder { path: "C:\\".into() }, description: None }
            .validate()
            .is_ok());
        assert!(Action::Os { operation: OsOperation::NewFolder { path: "C:\\x".into() }, description: None }
            .validate()
            .is_ok());
        assert!(Action::Os { operation: OsOperation::OpenFolder { path: "  ".into() }, description: None }
            .validate()
            .is_err());
        assert!(Action::Os { operation: OsOperation::NewFolder { path: "".into() }, description: None }
            .validate()
            .is_err());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn script_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Script {
                path: "C:\\scripts\\do.ps1".into(),
                interpreter: None,
                show_output: false,
                var: "".into(),
                description: None,
            },
            Action::Script {
                path: "backup.py".into(),
                interpreter: Some("python".into()),
                show_output: true,
                var: "out".into(),
                description: None,
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "{a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 脚本路径为空必被拒绝。
        assert!(Action::Script {
            path: "  ".into(),
            interpreter: None,
            show_output: false,
            var: "".into(),
            description: None,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn sanitize_cleans_folders() {
        let cfg = Config {
            folders: vec![
                Folder { id: "a".into(), name: "工作".into(), parent: None },
                Folder { id: "b".into(), name: "子目录".into(), parent: Some("a".into()) },
                Folder { id: "a".into(), name: "重复".into(), parent: None }, // 重复 id
                Folder { id: "".into(), name: "空id".into(), parent: None }, // 空 id
                Folder { id: "c".into(), name: "孤儿父".into(), parent: Some("nope".into()) }, // 父不存在
                Folder { id: "d".into(), name: "成环".into(), parent: Some("d".into()) }, // 自环
            ],
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+K".into()],
                    actions: vec![Action::Text { text: "x".into(), mode: TextMode::Input, description: None }],
                    folder: Some("a".into()),
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Text { text: "y".into(), mode: TextMode::Input, description: None }],
                    folder: Some("missing".into()), // 引用不存在目录
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        // 保留 a、b、c、d（d 的环被断），去掉重复 a 与空 id。
        assert_eq!(clean.folders.len(), 4, "folders: {:?} ignored: {:?}", clean.folders, ignored);
        assert_eq!(clean.folders[0].name, "工作");
        assert_eq!(clean.folders[1].parent.as_deref(), Some("a"));
        assert!(clean.folders.iter().find(|f| f.id == "c").unwrap().parent.is_none());
        assert!(clean.folders.iter().find(|f| f.id == "d").unwrap().parent.is_none());
        // 无效目录引用被清空
        assert_eq!(clean.shortcuts[0].folder.as_deref(), Some("a"));
        assert!(clean.shortcuts[1].folder.is_none());
    }
}

#[cfg(test)]
mod hotstring_tests {
    use super::*;

    #[test]
    fn key_to_char_letters_digits_punct() {
        assert_eq!(key_to_char(Key::A, false), Some('a'));
        assert_eq!(key_to_char(Key::A, true), Some('A'));
        assert_eq!(key_to_char(Key::Z, false), Some('z'));
        assert_eq!(key_to_char(Key::Digit1, false), Some('1'));
        assert_eq!(key_to_char(Key::Digit1, true), Some('!'));
        assert_eq!(key_to_char(Key::Semicolon, false), Some(';'));
        assert_eq!(key_to_char(Key::Semicolon, true), Some(':'));
        assert_eq!(key_to_char(Key::Comma, true), Some('<'));
        assert_eq!(key_to_char(Key::Minus, true), Some('_'));
        assert_eq!(key_to_char(Key::Space, false), Some(' '));
        // 非可打印键返回 None
        assert_eq!(key_to_char(Key::Enter, false), None);
        assert_eq!(key_to_char(Key::ArrowUp, false), None);
        assert_eq!(key_to_char(Key::Control, false), None);
        assert_eq!(key_to_char(Key::F13, false), None);
        assert_eq!(key_to_char(Key::MouseBack, false), None);
    }

    #[test]
    fn terminator_detection() {
        assert!(is_hotstring_terminator(Key::Space));
        assert!(is_hotstring_terminator(Key::Enter));
        assert!(is_hotstring_terminator(Key::Tab));
        assert!(!is_hotstring_terminator(Key::A));
        assert!(!is_hotstring_terminator(Key::Backspace));
    }

    #[test]
    fn match_expansion_longest_wins() {
        let exps = vec![
            TextExpansion { trigger: "addr".into(), replace: "地址".into(), enabled: true },
            TextExpansion { trigger: "email".into(), replace: "邮箱".into(), enabled: true },
            TextExpansion { trigger: "work-email".into(), replace: "工作邮箱".into(), enabled: true },
            TextExpansion { trigger: "off".into(), replace: "关".into(), enabled: false }, // 停用
        ];
        assert_eq!(match_expansion(":addr", &exps).unwrap().replace, "地址");
        // 最长匹配优先（work-email 而非 email）
        assert_eq!(match_expansion("work-email", &exps).unwrap().replace, "工作邮箱");
        // 停用的不命中
        assert_eq!(match_expansion("off", &exps), None);
        // 无命中
        assert_eq!(match_expansion("xyz", &exps), None);
        // 空触发词不参与匹配
        let exps2 = vec![TextExpansion { trigger: "".into(), replace: "x".into(), enabled: true }];
        assert_eq!(match_expansion("", &exps2), None);
    }

    #[test]
    fn expansion_json_roundtrip_and_validate() {
        let exps = vec![
            TextExpansion { trigger: ":addr".into(), replace: "上海市…".into(), enabled: true },
            TextExpansion { trigger: ";sig".into(), replace: "{date}".into(), enabled: false },
        ];
        let json = serde_json::to_string(&exps).unwrap();
        let back: Vec<TextExpansion> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, exps);

        // 空触发词必被拒绝；空替换允许（等价删除触发词）
        assert!(TextExpansion { trigger: "  ".into(), replace: "x".into(), enabled: true }
            .validate()
            .is_err());
        assert!(TextExpansion { trigger: "ok".into(), replace: "".into(), enabled: true }
            .validate()
            .is_ok());
    }

    #[test]
    fn sanitize_drops_invalid_expansions() {
        let cfg = Config {
            expansions: vec![
                TextExpansion { trigger: "addr".into(), replace: "地址".into(), enabled: true },
                TextExpansion { trigger: "  ".into(), replace: "坏".into(), enabled: true },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.expansions.len(), 1);
        assert!(ignored.iter().any(|m| m.contains("文本扩展")));
    }
}

#[cfg(test)]
mod tap_hold_tests {
    use super::*;

    #[test]
    fn remap_tap_hold_detection_and_keys() {
        let ordinary = Remap {
            from: "CapsLock".into(),
            to: "Ctrl".into(),
            enabled: true,
            ..Default::default()
        };
        assert!(!ordinary.is_tap_hold());
        assert_eq!(ordinary.tap_key(), None);
        assert_eq!(ordinary.hold_key(), None);
        assert_eq!(ordinary.tap_timeout(), DEFAULT_TAP_TIMEOUT_MS); // 0 归一化为 200

        let tap_hold = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: Some("Ctrl".into()),
            tap_timeout_ms: 0,
            enabled: true,
            ..Default::default()
        };
        assert!(tap_hold.is_tap_hold());
        assert_eq!(tap_hold.tap_key(), Some(Key::Escape));
        assert_eq!(tap_hold.hold_key(), Some(Key::Control));
        assert_eq!(tap_hold.tap_timeout(), 200);

        // 只填 tap、只填 hold 均可；自定义阈值生效
        let tap_only = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: None,
            tap_timeout_ms: 150,
            enabled: true,
            ..Default::default()
        };
        assert!(tap_only.is_tap_hold());
        assert_eq!(tap_only.tap_key(), Some(Key::Escape));
        assert_eq!(tap_only.hold_key(), None);
        assert_eq!(tap_only.tap_timeout(), 150);
    }

    #[test]
    fn remap_json_roundtrip_tap_hold() {
        let r = Remap {
            from: "CapsLock".into(),
            to: "".into(),
            tap: Some("Esc".into()),
            hold: Some("Ctrl".into()),
            tap_timeout_ms: 180,
            enabled: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Remap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);

        // 旧配置（无 tap/hold/tap_timeout_ms）反序列化 → 普通改键，阈值默认 200
        let legacy = r#"{"from":"CapsLock","to":"Ctrl","enabled":true}"#;
        let l: Remap = serde_json::from_str(legacy).unwrap();
        assert!(!l.is_tap_hold());
        assert_eq!(l.tap_timeout(), DEFAULT_TAP_TIMEOUT_MS);
    }

    #[test]
    fn sanitize_tap_hold_keeps_valid_drops_invalid() {
        let cfg = Config {
            remaps: vec![
                Remap {
                    from: "CapsLock".into(),
                    to: "".into(),
                    tap: Some("Esc".into()),
                    hold: Some("Ctrl".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
                // tap 非法、hold 合法 → 清空 tap、保留 hold
                Remap {
                    from: "A".into(),
                    to: "".into(),
                    tap: Some("NotAKey".into()),
                    hold: Some("Shift".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
                // tap/hold 都非法 → 整条忽略
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    tap: Some("NotAKey".into()),
                    hold: Some("Bad".into()),
                    tap_timeout_ms: 200,
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.remaps.len(), 2);
        assert!(clean.remaps[0].is_tap_hold());
        assert!(clean.remaps[1].is_tap_hold());
        assert_eq!(clean.remaps[1].tap, None);
        assert_eq!(clean.remaps[1].hold.as_deref(), Some("Shift"));
        assert!(ignored.iter().any(|m| m.contains("均无法解析")));
    }
}

#[cfg(test)]
mod layer_tests {
    use super::*;

    #[test]
    fn layer_and_hold_layer_detection() {
        let remap = Remap {
            from: "Space".into(),
            to: "".into(),
            hold_layer: Some("symbols".into()),
            ..Default::default()
        };
        assert!(remap.is_tap_hold()); // hold_layer 也走 tap-hold 状态机
        assert_eq!(remap.hold_layer_id(), Some("symbols"));
        assert_eq!(remap.tap_key(), None);
        assert_eq!(remap.hold_key(), None);

        let blank = Remap {
            from: "Space".into(),
            to: "".into(),
            hold_layer: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(blank.hold_layer_id(), None);
    }

    #[test]
    fn layer_json_roundtrip() {
        let layers = vec![
            Layer { id: "base".into(), name: "基础".into() },
            Layer { id: "symbols".into(), name: "符号".into() },
        ];
        let json = serde_json::to_string(&layers).unwrap();
        let back: Vec<Layer> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, layers);
    }

    #[test]
    fn layer_without_switch_key_is_reported() {
        // 层里有启用的条目、却没有任何切层键指向它 → 永久不可达，必须在冲突清单里点出来，
        // 否则用户只能看到「层内快捷键配了但不触发」而查不出原因。
        let layered = ShortcutItem {
            layer: Some("symbols".into()),
            triggers: vec!["5&Y".into()],
            actions: vec![],
            enabled: true,
            ..Default::default()
        };
        let base = Config {
            layers: vec![Layer { id: "symbols".into(), name: "符号".into() }],
            shortcuts: vec![layered],
            ..Default::default()
        };
        let hits = detect_conflicts(&base, &PlatformCaps::default());
        assert!(
            hits.iter().any(|c| c.message.contains("没有切层键")),
            "无切层键的层应被报出"
        );

        // 加一条指向它的「长按进入层」改键后，提示消失。
        let with_key = Config {
            remaps: vec![Remap {
                from: "Space".into(),
                to: "".into(),
                hold_layer: Some("symbols".into()),
                enabled: true,
                ..Default::default()
            }],
            ..base.clone()
        };
        assert!(!detect_conflicts(&with_key, &PlatformCaps::default()).iter().any(|c| c.message.contains("没有切层键")));

        // 「长按锁定层」同样是切层键，指向它也算可达（层内放和弦/序列只能用这种）。
        let with_lock = Config {
            remaps: vec![Remap {
                from: "Space".into(),
                to: "".into(),
                lock_layer: Some("symbols".into()),
                enabled: true,
                ..Default::default()
            }],
            ..base.clone()
        };
        assert!(
            !detect_conflicts(&with_lock, &PlatformCaps::default()).iter().any(|c| c.message.contains("没有切层键")),
            "长按锁定层也应算可达：{:?}",
            detect_conflicts(&with_lock, &PlatformCaps::default())
        );

        // 层里没有启用的条目时不报（空层不算问题）。
        let empty = Config {
            layers: vec![Layer { id: "symbols".into(), name: "符号".into() }],
            ..Default::default()
        };
        assert!(!detect_conflicts(&empty, &PlatformCaps::default()).iter().any(|c| c.message.contains("没有切层键")));
    }

    #[test]
    fn lock_layer_detection_and_summary() {
        let remap = Remap {
            from: "Tab".into(),
            to: "".into(),
            lock_layer: Some("symbols".into()),
            ..Default::default()
        };
        assert!(remap.is_tap_hold(), "lock_layer 也走 tap-hold 状态机");
        assert!(remap.needs_timing_state());
        assert_eq!(remap.lock_layer_id(), Some("symbols"));
        assert_eq!(remap.hold_layer_id(), None);
        assert!(remap.describe().contains("长按锁定层"), "摘要要能看出是锁定式：{}", remap.describe());

        let blank = Remap {
            from: "Tab".into(),
            to: "".into(),
            lock_layer: Some("  ".into()),
            ..Default::default()
        };
        assert_eq!(blank.lock_layer_id(), None);
    }

    #[test]
    fn sanitize_keeps_lock_layer_and_drops_blank_refs() {
        let cfg = Config {
            layers: vec![Layer { id: "symbols".into(), name: "符号".into() }],
            remaps: vec![
                Remap {
                    from: "Tab".into(),
                    to: "".into(),
                    lock_layer: Some("symbols".into()),
                    ..Default::default()
                },
                Remap {
                    from: "A".into(),
                    to: "".into(),
                    lock_layer: Some("nope".into()), // 层不存在 → 清空 → 整条忽略
                    ..Default::default()
                },
                // 同时设了 momentary 与切换式：保留切换式（层内和弦/序列只能靠它）。
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    hold_layer: Some("symbols".into()),
                    lock_layer: Some("symbols".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.remaps.len(), 2);
        assert_eq!(clean.remaps[0].lock_layer.as_deref(), Some("symbols"));
        assert_eq!(clean.remaps[0].hold_layer, None);
        assert_eq!(clean.remaps[1].hold_layer, None);
        assert_eq!(clean.remaps[1].lock_layer.as_deref(), Some("symbols"));
        assert!(ignored.iter().any(|m| m.contains("层不存在")));
        assert!(ignored.iter().any(|m| m.contains("保留长按锁定层")));
    }

    #[test]
    fn sanitize_cleans_layers_and_dangling_refs() {
        let cfg = Config {
            layers: vec![
                Layer { id: "symbols".into(), name: "符号".into() },
                Layer { id: "symbols".into(), name: "重复".into() }, // 重复 id → 忽略
                Layer { id: "".into(), name: "坏".into() }, // 空 id → 忽略
            ],
            shortcuts: vec![ShortcutItem {
                layer: Some("symbols".into()),
                triggers: vec!["1".into()],
                actions: vec![Action::Text { text: "F1".into(), mode: TextMode::Input, description: None }],
                enabled: true,
                ..Default::default()
            }, ShortcutItem {
                layer: Some("nope".into()), // 层不存在 → 清空
                triggers: vec!["2".into()],
                actions: vec![],
                enabled: true,
                ..Default::default()
            }],
            remaps: vec![Remap {
                from: "Space".into(),
                to: "".into(),
                hold_layer: Some("symbols".into()),
                ..Default::default()
            }, Remap {
                from: "A".into(),
                to: "".into(),
                hold_layer: Some("nope".into()), // 层不存在 → 清空 → 整条忽略
                ..Default::default()
            }],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.layers.len(), 1);
        assert_eq!(clean.layers[0].id, "symbols");
        assert_eq!(clean.shortcuts.len(), 2);
        assert_eq!(clean.shortcuts[0].layer.as_deref(), Some("symbols"));
        assert_eq!(clean.shortcuts[1].layer, None);
        assert_eq!(clean.remaps.len(), 1);
        assert_eq!(clean.remaps[0].hold_layer.as_deref(), Some("symbols"));
        assert!(ignored.iter().any(|m| m.contains("层")));
    }
}

#[cfg(test)]
mod sequence_tests {
    use super::*;

    fn ev(key: Key, mods: &[Modifier]) -> RawEvent {
        RawEvent { key, mods: mods.iter().copied().collect(), pressed: true }
    }

    /// 把 "F9 J K" 解析成一串步骤。
    fn seq(s: &str) -> Vec<Shortcut> {
        s.split_whitespace().map(|t| t.parse().unwrap()).collect()
    }

    #[test]
    fn trigger_parse_single_sequence_empty_bad() {
        // 单 token → Combo。
        let t = Trigger::parse("Ctrl+K").unwrap();
        assert!(matches!(t, Trigger::Combo(_)));
        assert!(!t.is_sequence());
        assert_eq!(t.steps().len(), 1);

        // 多 token → Sequence。
        let t = Trigger::parse("F9 J K").unwrap();
        assert!(matches!(t, Trigger::Sequence(ref steps) if steps.len() == 3));
        assert!(t.is_sequence());
        assert_eq!(t.first(), &"F9".parse::<Shortcut>().unwrap());

        // "Space" 单 token 仍是 Combo；"F9 Space" 是序列。
        assert!(matches!(Trigger::parse("Space").unwrap(), Trigger::Combo(_)));
        assert!(matches!(
            Trigger::parse("F9 Space").unwrap(),
            Trigger::Sequence(ref steps) if steps.len() == 2
        ));

        // 组合键可作为序列步（"F9 Ctrl+K"）。
        assert!(matches!(
            Trigger::parse("F9 Ctrl+K").unwrap(),
            Trigger::Sequence(ref steps) if steps.len() == 2
        ));

        // 空串 / 纯空白 → Empty。
        assert!(matches!(Trigger::parse(""), Err(ParseError::Empty)));
        assert!(matches!(Trigger::parse("   "), Err(ParseError::Empty)));

        // 坏步 → 解析失败（任一坏步都不保留）。
        assert!(Trigger::parse("F9 NotAKey").is_err());
    }

    #[test]
    fn sequence_tracker_start_advance_complete() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert!(!tr.is_active());

        // F9 → 建立候选（leader 吞掉）。
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());

        // J → 推进。
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());

        // K → 完成（下标 0），状态清空。
        assert_eq!(tr.advance(&ev(Key::K, &[]), &seqs), SeqAdvance::Complete(0));
        assert!(!tr.is_active());
    }

    #[test]
    fn sequence_tracker_shared_prefix() {
        let seqs = vec![seq("F9 J K"), seq("F9 J L")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::K, &[]), &seqs), SeqAdvance::Complete(0));

        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::J, &[]), &seqs), SeqAdvance::Advance);
        assert_eq!(tr.advance(&ev(Key::L, &[]), &seqs), SeqAdvance::Complete(1));
    }

    #[test]
    fn sequence_tracker_chain_break_resets() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::F9, &[]), &seqs), SeqAdvance::Advance);
        assert!(tr.is_active());
        // 断链：按下无关键 → NoMatch 并重置。
        assert_eq!(tr.advance(&ev(Key::X, &[]), &seqs), SeqAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn sequence_tracker_no_match_when_idle() {
        let seqs = vec![seq("F9 J K")];
        let mut tr = SequenceTracker::new();
        assert_eq!(tr.advance(&ev(Key::A, &[]), &seqs), SeqAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn sanitize_keeps_valid_sequence_drops_invalid() {
        let cfg = Config {
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["F9 J K".into()],
                    actions: vec![Action::Text { text: "ok".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["F9 BadStep".into()],
                    actions: vec![Action::Text { text: "bad".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.shortcuts.len(), 2, "ignored: {}", ignored.join("; "));
        // 合法序列保留；坏步序列被清空并提示。
        assert_eq!(clean.shortcuts[0].triggers, vec!["F9 J K".to_string()]);
        assert!(clean.shortcuts[1].triggers.is_empty());
        assert!(ignored.iter().any(|m| m.contains("F9 BadStep")));
    }
}

#[cfg(test)]
mod chord_tests {
    use super::*;

    /// 把 "F&J" 解析成一组成员键（走真正的和弦解析路径，含纯修饰键成员的折叠）。
    fn chord(s: &str) -> Vec<Shortcut> {
        match Trigger::parse(s).unwrap() {
            Trigger::Chord(members) => members,
            _ => panic!("不是和弦: {s}"),
        }
    }

    /// 不带修饰键按下一个成员键（多数用例的默认情形）。
    fn press(tr: &mut ChordTracker, key: Key, chords: &[Vec<Shortcut>]) -> ChordAdvance {
        tr.press(key, &BTreeSet::new(), chords)
    }

    #[test]
    fn chord_tracker_await_then_complete() {
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert!(!tr.is_active());
        // F 是成员 → Await（吞掉），未凑齐。
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.is_active());
        // J 凑齐 → Complete(0)，状态清空。
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
        assert!(!tr.is_active());
    }

    #[test]
    fn chord_tracker_unordered() {
        // 先按 J 再按 F 也命中（无序）。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_three_members() {
        let chords = vec![chord("D&F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::D, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_non_member_keeps_held() {
        // 非成员键不影响按住集合：成员仍按住，后续补齐成员照样命中。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.is_active());
        assert_eq!(press(&mut tr, Key::X, &chords), ChordAdvance::NoMatch);
        assert!(tr.is_active(), "非成员键不打断已按住的成员");
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_release_removes_member() {
        // 成员抬起即移出集合：抬手后单独按其它成员不构成「同时按住」（不会误触发）。
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert!(tr.release(Key::F), "F 原本在按住集合里");
        assert!(!tr.is_active());
        assert!(!tr.release(Key::F), "已抬起的键再抬起返回 false");
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_shared_member() {
        let chords = vec![chord("F&J"), chord("F&K")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::K, &chords), ChordAdvance::Complete(1));

        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::F, &chords), ChordAdvance::Await);
        assert_eq!(press(&mut tr, Key::J, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_no_match_when_idle() {
        let chords = vec![chord("F&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(press(&mut tr, Key::A, &chords), ChordAdvance::NoMatch);
        assert!(!tr.is_active());
    }

    #[test]
    fn chord_tracker_member_mods_must_be_held_when_completing() {
        // Ctrl+F&J：Ctrl 是「要求按住」——F、J 都按着但 Ctrl 没按就不算凑齐。
        let chords = vec![chord("Ctrl+F&J")];
        let ctrl = BTreeSet::from([Modifier::Ctrl]);
        let mut tr = ChordTracker::new();
        assert_eq!(tr.press(Key::F, &BTreeSet::new(), &chords), ChordAdvance::Await);
        assert_eq!(
            tr.press(Key::J, &BTreeSet::new(), &chords),
            ChordAdvance::Await,
            "缺 Ctrl 时不能凑齐（等待窗到点会把它当普通按键回放）"
        );
        assert!(tr.is_active());
        // 补上 Ctrl 后重新按一次 J（F 还按着）→ 凑齐。
        assert_eq!(tr.press(Key::J, &ctrl, &chords), ChordAdvance::Complete(0));
    }

    #[test]
    fn chord_tracker_modifier_only_member_requires_mods_down() {
        // Ctrl+Alt&J：前一个成员是纯修饰键要求，只按时不参与按住集合、也不吞键。
        let chords = vec![chord("Ctrl+Alt&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(tr.press(Key::J, &BTreeSet::from([Modifier::Ctrl]), &chords),
            ChordAdvance::Await, "只有 Ctrl 还差 Alt");
        assert_eq!(
            tr.press(Key::J, &BTreeSet::from([Modifier::Ctrl, Modifier::Alt]), &chords),
            ChordAdvance::Complete(0)
        );
        assert!(!tr.held().contains(&Key::Alt), "修饰键成员不进按住集合");
    }

    #[test]
    fn chord_tracker_modifier_key_is_never_a_member() {
        // 修饰键成员不做「按住集合」的登记（壳层会先行放行，这里是兜底的那道）。
        let chords = vec![chord("Ctrl+Alt&J")];
        let mut tr = ChordTracker::new();
        assert_eq!(
            tr.press(Key::Alt, &BTreeSet::from([Modifier::Alt]), &chords),
            ChordAdvance::NoMatch
        );
        assert!(!tr.is_active());
    }

    #[test]
    fn sanitize_keeps_valid_chord_drops_invalid() {
        let cfg = Config {
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+F&J".into()],
                    actions: vec![Action::Text { text: "ok".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["F&F".into()],
                    actions: vec![Action::Text { text: "bad".into(), mode: TextMode::Input, description: None }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.shortcuts.len(), 2, "ignored: {}", ignored.join("; "));
        // 合法和弦（成员带修饰）保留；坏和弦（成员重复）被清空并提示。
        assert_eq!(clean.shortcuts[0].triggers, vec!["Ctrl+F&J".to_string()]);
        assert!(clean.shortcuts[1].triggers.is_empty());
        assert!(ignored.iter().any(|m| m.contains("F&F")), "ignored: {ignored:?}");
    }
    #[test]
    fn trigger_parse_chord() {
        // F&J → Chord（2 成员，无序）。
        let t = Trigger::parse("F&J").unwrap();
        assert!(matches!(t, Trigger::Chord(ref m) if m.len() == 2));
        assert!(t.is_chord());
        assert!(!t.is_sequence());
        assert_eq!(t.steps().len(), 2);
        assert_eq!(t.first(), &"F".parse::<Shortcut>().unwrap());

        // 三成员和弦。
        assert!(matches!(Trigger::parse("D&F&J").unwrap(), Trigger::Chord(ref m) if m.len() == 3));

        // 单键不含 & 仍是 Combo。
        assert!(matches!(Trigger::parse("F").unwrap(), Trigger::Combo(_)));

        // 坏输入：空成员。
        assert!(matches!(Trigger::parse("F&"), Err(ParseError::ChordInvalid(_))));
        assert!(matches!(Trigger::parse("F&&J"), Err(ParseError::ChordInvalid(_))));
        // 成员重复（同一个键 + 同一个修饰要求按两次不算更「同时」）。
        assert!(matches!(Trigger::parse("F&F"), Err(ParseError::ChordInvalid(_))));
        assert!(matches!(Trigger::parse("Ctrl+F&Ctrl+F"), Err(ParseError::ChordInvalid(_))));
        // 全是修饰键成员：没有可吞的键，也就没有「同时按下」的判定时机。
        assert!(matches!(Trigger::parse("Ctrl&Alt"), Err(ParseError::ChordInvalid(_))));
        // 一个成员里两个主键（`Ctrl+K+L`）无法表达，报未知按键而不是含糊的和弦无效。
        assert!(Trigger::parse("Ctrl+K+L&J").is_err());
    }

    #[test]
    fn trigger_parse_chord_with_modifier_members() {
        // 成员可带修饰：Ctrl+F&J = 按住 Ctrl 的同时把 F、J 一起按住。
        let t = Trigger::parse("Ctrl+F&J").unwrap();
        let Trigger::Chord(members) = &t else { panic!("应为和弦") };
        assert_eq!(members[0].key, Key::F);
        assert_eq!(members[0].mods, BTreeSet::from([Modifier::Ctrl]));
        assert_eq!(members[1].key, Key::J);
        assert!(members[1].mods.is_empty());
        assert_eq!(format_shortcut(t.first()), "Ctrl+F", "显示要能原样渲染回去");

        // 纯修饰键成员（`Ctrl+Alt`）：折叠成「要求 Ctrl 按住 + 成员键 Alt」，显示不变。
        let t = Trigger::parse("Ctrl+Alt&J").unwrap();
        let Trigger::Chord(members) = &t else { panic!("应为和弦") };
        assert_eq!(members[0].key, Key::Alt);
        assert_eq!(members[0].mods, BTreeSet::from([Modifier::Ctrl]));
        assert_eq!(format_shortcut(t.first()), "Ctrl+Alt");
        assert_eq!(members[1].key, Key::J);

        // 单个修饰键成员本来就是合法裸键（无需折叠）。
        let t = Trigger::parse("F&Shift").unwrap();
        let Trigger::Chord(members) = &t else { panic!("应为和弦") };
        assert_eq!(members[1].key, Key::Shift);
        assert!(members[1].mods.is_empty());
    }
}

/// 设置里的两个等待窗（序列 / 和弦）都是「缺字段拿默认值、0 = 不限时是有效取值」，
/// 两类取值都别被 serde 缺省或清洗改掉。
#[cfg(test)]
mod settings_timeout_tests {
    use super::*;

    #[test]
    fn defaults_match_the_two_constants() {
        let d = Settings::default();
        assert_eq!(d.sequence_timeout_ms, DEFAULT_SEQUENCE_TIMEOUT_MS);
        assert_eq!(d.chord_timeout_ms, DEFAULT_CHORD_TIMEOUT_MS);
    }

    #[test]
    fn old_config_without_timeout_fields_gets_defaults() {
        // 老配置（写于这两个字段出现之前）应拿到 1000ms，而不是 `#[serde(default)]` 的 0
        // （0 在这里 = 不限时，等于悄悄把等待窗取消）。
        let old: Config =
            serde_json::from_str(r#"{"settings":{"autostart":true,"action_timeout_ms":5000}}"#).unwrap();
        assert_eq!(old.settings.sequence_timeout_ms, DEFAULT_SEQUENCE_TIMEOUT_MS);
        assert_eq!(old.settings.chord_timeout_ms, DEFAULT_CHORD_TIMEOUT_MS);
        assert_eq!(old.settings.action_timeout_ms, 5000, "已有的字段照旧");
    }

    #[test]
    fn zero_means_unlimited_and_survives_roundtrip() {
        let cfg = Config {
            settings: Settings { sequence_timeout_ms: 0, chord_timeout_ms: 2500, ..Default::default() },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.settings, cfg.settings);
        assert_eq!(back.settings.sequence_timeout_ms, 0, "0（不限时）不能被缺省值盖回 1000");
    }
}

#[cfg(test)]
mod hints_window_tests {
    use super::*;

    /// 老配置（写于提示框出现之前）缺 `hints`：默认**不显示**（常挂屏幕的浮层不请自来更烦人），
    /// 但字号 / 透明度要有正经初值，别落到 0 变成看不见的一条。
    #[test]
    fn old_config_without_hints_gets_defaults() {
        let old: Config = serde_json::from_str(r#"{"settings":{"autostart":true}}"#).unwrap();
        assert!(!old.settings.hints.visible);
        assert_eq!(old.settings.hints.scale, HintsWindow::DEFAULT_SCALE);
        assert_eq!(old.settings.hints.opacity, HintsWindow::DEFAULT_OPACITY);
        assert_eq!(old.settings.hints.x, None);
    }

    /// 位置 / 外观要能原样往返：重启后提示框得回到用户拖到的地方、保持他调好的大小与透明度。
    #[test]
    fn hints_roundtrip_keeps_position_and_look() {
        let cfg = Config {
            settings: Settings {
                hints: HintsWindow {
                    visible: true,
                    x: Some(-1200),
                    y: Some(48),
                    scale: 140,
                    opacity: 60,
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(back.settings, cfg.settings);
    }

    /// 没拖过的窗口不必往 JSON 里写 `null`：`skip_serializing_if` 让「没位置」就是个缺字段，
    /// 手改配置时也少两行噪音。
    #[test]
    fn unplaced_hints_omit_coordinates() {
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!json.contains("\"x\""), "缺省位置不该落进 JSON：{json}");
        assert!(json.contains("\"hints\""));
    }

    /// 归一化：手改 JSON / 别的机器同步过来的越界值必须被钳住，且 `migrate` 会顺手调用它
    /// （加载路径只 `migrate()` 不 `sanitize_config()`，这里是唯一的兜底）。
    #[test]
    fn migrate_clamps_out_of_range_look() {
        let mut cfg = Config {
            settings: Settings {
                hints: HintsWindow { visible: true, scale: 0, opacity: 250, ..Default::default() },
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.migrate();
        assert_eq!(cfg.settings.hints.scale, HintsWindow::SCALE_RANGE.0, "缩成 0 等于把窗口弄没");
        assert_eq!(cfg.settings.hints.opacity, HintsWindow::OPACITY_RANGE.1);
        assert!(cfg.settings.hints.visible, "钳值不该顺手改可见性");
    }
}

#[cfg(test)]
mod advanced_modifier_tests {
    use super::*;

    fn remap(fields: impl FnOnce(&mut Remap)) -> Remap {
        let mut r = Remap {
            from: "CapsLock".into(),
            to: "Ctrl".into(),
            enabled: true,
            ..Default::default()
        };
        fields(&mut r);
        r
    }

    #[test]
    fn oneshot_sticky_detection_and_needs_timing() {
        let plain = remap(|_| {});
        assert!(!plain.is_oneshot());
        assert!(!plain.is_sticky());
        assert!(!plain.needs_timing_state());

        let oneshot = remap(|r| {
            r.to = String::new();
            r.oneshot = Some("Ctrl".into());
        });
        assert!(oneshot.is_oneshot());
        assert!(!oneshot.is_sticky());
        assert!(oneshot.needs_timing_state());
        assert_eq!(oneshot.oneshot_key(), Some(Key::Control));

        let sticky = remap(|r| {
            r.to = String::new();
            r.sticky = Some("Shift".into());
        });
        assert!(sticky.is_sticky());
        assert!(!sticky.is_oneshot());
        assert!(sticky.needs_timing_state());
        assert_eq!(sticky.sticky_key(), Some(Key::Shift));
    }

    #[test]
    fn multi_tap_detection_and_output_fallback() {
        // 只有 tap：单击/双击/三击都回落 tap。
        let tap_only = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
        });
        assert!(!tap_only.is_multi_tap());
        assert_eq!(tap_only.tap_output(1), Some(Key::Escape));
        assert_eq!(tap_only.tap_output(2), Some(Key::Escape));
        assert_eq!(tap_only.tap_output(3), Some(Key::Escape));

        // tap + tap2：双击 tap2、三击回落 tap2。
        let tap2 = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
        });
        assert!(tap2.is_multi_tap());
        assert_eq!(tap2.tap_output(1), Some(Key::Escape));
        assert_eq!(tap2.tap_output(2), Some(Key::Backspace));
        assert_eq!(tap2.tap_output(3), Some(Key::Backspace));

        // tap + tap2 + tap3：三击 tap3。
        let tap3 = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
            r.tap3 = Some("Delete".into());
        });
        assert!(tap3.is_multi_tap());
        assert_eq!(tap3.tap_output(1), Some(Key::Escape));
        assert_eq!(tap3.tap_output(2), Some(Key::Backspace));
        assert_eq!(tap3.tap_output(3), Some(Key::Delete));
    }

    #[test]
    fn tap_outputs_returns_fallback_array() {
        let r = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
        });
        assert_eq!(r.tap_outputs(), [Some(Key::Escape), Some(Key::Backspace), Some(Key::Backspace)]);
        // 只有 tap：三击都回落 tap。
        let t = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
        });
        assert_eq!(t.tap_outputs(), [Some(Key::Escape), Some(Key::Escape), Some(Key::Escape)]);
    }

    #[test]
    fn describe_covers_all_modes() {
        assert_eq!(remap(|_| {}).describe(), "Ctrl");
        assert_eq!(remap(|r| { r.oneshot = Some("Ctrl".into()); r.to = String::new(); }).describe(), "单击 Ctrl");
        assert_eq!(remap(|r| { r.sticky = Some("Shift".into()); r.to = String::new(); }).describe(), "粘滞 Shift");
        assert_eq!(remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.hold = Some("Ctrl".into());
        }).describe(), "单击 Esc · 长按 Ctrl");
        assert_eq!(remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
        }).describe(), "单击 Esc · 双击 Backspace");
    }

    #[test]
    fn remap_json_roundtrip_new_fields() {
        let r = remap(|r| {
            r.to = String::new();
            r.tap = Some("Esc".into());
            r.tap2 = Some("Backspace".into());
            r.tap3 = Some("Delete".into());
            r.oneshot = Some("Ctrl".into());
            r.sticky = Some("Shift".into());
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: Remap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
        // 旧配置（无新字段）反序列化 → 新字段全为 None。
        let legacy = r#"{"from":"CapsLock","to":"Ctrl","enabled":true}"#;
        let l: Remap = serde_json::from_str(legacy).unwrap();
        assert!(l.oneshot.is_none() && l.sticky.is_none() && l.tap2.is_none() && l.tap3.is_none());
    }

    #[test]
    fn sanitize_oneshot_sticky_normalizes_and_drops_invalid() {
        let cfg = Config {
            remaps: vec![
                // 粘滞 + 单次同时设 → 保留粘滞、清掉单次与 tap-hold 字段。
                Remap {
                    from: "CapsLock".into(),
                    to: "".into(),
                    sticky: Some("Shift".into()),
                    oneshot: Some("Ctrl".into()),
                    tap: Some("Esc".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 单次修饰合法 → 保留；普通 to 清空。
                Remap {
                    from: "A".into(),
                    to: "B".into(),
                    oneshot: Some("Alt".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 坏键名 → 清空后无形态 → 整条忽略。
                Remap {
                    from: "B".into(),
                    to: "".into(),
                    oneshot: Some("NotAKey".into()),
                    enabled: true,
                    ..Default::default()
                },
                // 坏 tap2 → 清空；tap 仍保留为 tap-hold。
                Remap {
                    from: "C".into(),
                    to: "".into(),
                    tap: Some("Esc".into()),
                    tap2: Some("Bad".into()),
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (clean, ignored) = sanitize_config(&cfg);
        assert_eq!(clean.remaps.len(), 3, "remaps: {:?}\nignored: {:?}", clean.remaps, ignored);

        // 第 0 条：粘滞 Shift，其它清空。
        assert_eq!(clean.remaps[0].sticky.as_deref(), Some("Shift"));
        assert!(clean.remaps[0].oneshot.is_none());
        assert!(clean.remaps[0].tap.is_none());
        // 第 1 条：单次 Alt，to 清空。
        assert_eq!(clean.remaps[1].oneshot.as_deref(), Some("Alt"));
        assert_eq!(clean.remaps[1].to, "");
        // 第 2 条：坏 tap2 清空，保留 tap。
        assert_eq!(clean.remaps[2].tap.as_deref(), Some("Esc"));
        assert!(clean.remaps[2].tap2.is_none());
        // 提示信息覆盖坏键名与归一化。
        assert!(ignored.iter().any(|m| m.contains("NotAKey")));
        assert!(ignored.iter().any(|m| m.contains("Bad")));
        assert!(ignored.iter().any(|m| m.contains("保留粘滞")));
    }
}
#[cfg(test)]
mod platform_caps_tests {
    use super::*;

    /// Linux 口径的 caps：前台条件不可用（Wayland / X11 都未接前台查询）。
    fn linux_caps() -> PlatformCaps {
        PlatformCaps { frontmost_conditions: false, ..Default::default() }
    }

    /// macOS 口径的 caps：媒体键 + F21~F24 无对应键码，监听与注入都不行。
    fn macos_caps() -> PlatformCaps {
        let mut keys = BTreeSet::new();
        for k in [
            Key::MediaPlayPause, Key::MediaPrev, Key::MediaNext,
            Key::VolumeMute, Key::VolumeDown, Key::VolumeUp,
            Key::F21, Key::F22, Key::F23, Key::F24,
        ] {
            keys.insert(k);
        }
        PlatformCaps {
            listen_unsupported: keys.clone(),
            inject_unsupported: keys,
            frontmost_conditions: true,
            device_conditions: false,
        }
    }

    fn shortcut(name: &str, trigger: &str, actions: Vec<Action>) -> ShortcutItem {
        ShortcutItem {
            name: Some(name.into()),
            triggers: vec![trigger.into()],
            actions,
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn full_caps_zero_false_positives() {
        // 全支持（Windows）：平台扫描不产出任何条目。
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+VolumeUp", vec![])],
            ..Default::default()
        };
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[test]
    fn frontmost_condition_flagged_on_linux() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::If {
                    condition: Condition::FrontmostApp { app: "firefox".into() },
                    then: vec![Action::Text { text: "x".into(), mode: TextMode::default(), description: None }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("前台"));
        assert!(hits[0].message.contains("恒不成立"));
        // Windows 口径下同配置零提示。
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[test]
    fn device_condition_flagged_when_unsupported() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::If {
                    condition: Condition::DeviceIs { id: "keyboard".into() },
                    then: vec![Action::Text { text: "x".into(), mode: TextMode::default(), description: None }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        // macOS 不支持设备条件 → 报一条「设备」警告。
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("设备"));
        assert!(hits[0].message.contains("恒不成立"));
        // 支持设备条件的平台（Linux / Windows，本例用默认）零提示。
        assert!(detect_conflicts(&cfg, &PlatformCaps::default()).is_empty());
    }

    #[test]
    fn nested_if_frontmost_also_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "F9",
                vec![Action::If {
                    condition: Condition::Exists { path: "/tmp".into() },
                    then: vec![Action::If {
                        condition: Condition::WindowTitleContains { text: "编辑".into() },
                        then: vec![],
                        otherwise: vec![],
                        description: None,
                    }],
                    otherwise: vec![],
                    description: None,
                }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &linux_caps());
        assert!(hits.iter().any(|c| c.message.contains("前台")), "{hits:?}");
    }

    #[test]
    fn unsupported_trigger_key_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut("s", "Ctrl+VolumeUp", vec![])],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].message.contains("VolumeUp"));
        assert!(hits[0].message.contains("监听不到"));
        // 序列 leader 与和弦成员同样要报。
        let cfg2 = Config {
            shortcuts: vec![
                shortcut("seq", "F9 F21 K", vec![]),
                shortcut("chord", "F22&J", vec![]),
            ],
            ..Default::default()
        };
        let hits2 = detect_conflicts(&cfg2, &macos_caps());
        assert!(hits2.iter().any(|c| c.message.contains("「F9 F21 K」")), "{hits2:?}");
        assert!(hits2.iter().any(|c| c.message.contains("「F22&J」")), "{hits2:?}");
    }

    #[test]
    fn disabled_entries_not_flagged() {
        let mut s = shortcut("s", "Ctrl+VolumeUp", vec![]);
        s.enabled = false;
        let cfg = Config { shortcuts: vec![s], ..Default::default() };
        assert!(detect_conflicts(&cfg, &macos_caps()).is_empty());
    }

    #[test]
    fn remap_sides_flagged() {
        let cfg = Config {
            remaps: vec![
                Remap { from: "VolumeMute".into(), to: "A".into(), enabled: true, ..Default::default() },
                Remap { from: "B".into(), to: "F24".into(), enabled: true, ..Default::default() },
            ],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(hits.iter().any(|c| c.message.contains("改键来源") && c.message.contains("VolumeMute")), "{hits:?}");
        assert!(hits.iter().any(|c| c.message.contains("目标键") && c.message.contains("F24")), "{hits:?}");
    }

    #[test]
    fn keys_action_inject_flagged() {
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "Ctrl+K",
                vec![Action::Keys { keys: vec!["F21".into(), "Enter".into()], description: None }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(!hits.is_empty(), "{hits:?}");
        assert!(hits.iter().any(|c| c.message.contains("F21") && c.message.contains("注入不了")));
    }

    #[test]
    fn listen_and_inject_reported_separately() {
        // 同一条快捷键：触发键监听不到 + 动作注入不了，两条提示分开报。
        let cfg = Config {
            shortcuts: vec![shortcut(
                "s",
                "VolumeUp",
                vec![Action::Keys { keys: vec!["F23".into()], description: None }],
            )],
            ..Default::default()
        };
        let hits = detect_conflicts(&cfg, &macos_caps());
        assert!(hits.iter().any(|c| c.message.contains("监听不到")), "{hits:?}");
        assert!(hits.iter().any(|c| c.message.contains("注入不了")), "{hits:?}");
    }
}
