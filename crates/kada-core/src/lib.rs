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

/// 单个按键。与具体 OS 键码无关，由平台层映射进来。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Key {
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Digit0, Digit1, Digit2, Digit3, Digit4,
    Digit5, Digit6, Digit7, Digit8, Digit9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    F13, F14, F15, F16, F17, F18, F19, F20, F21, F22, F23, F24,
    Comma, Period, Slash, Backslash, Semicolon, Quote, Backquote,
    Minus, Equal, BracketLeft, BracketRight,
    Enter, Escape, Tab, Space, Backspace, Delete, Insert,
    CapsLock, Shift, Control, Alt, Meta,
    Home, End, PageUp, PageDown,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
    // 媒体键（音量/播放控制）。
    MediaPlayPause, MediaPrev, MediaNext, VolumeMute, VolumeDown, VolumeUp,
    // 小键盘数字区 + NumLock（独立映射，与主键区数字键区分）。
    Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7,
    Numpad8, Numpad9, NumpadAdd, NumpadSubtract, NumpadMultiply, NumpadDivide,
    NumpadDecimal, NumpadEnter, NumLock,
    // 鼠标键（中键 / 侧键，作为触发键与改键目标；左右键与滚轮不纳入，
    // 避免全局误拦截点击）。
    MouseMiddle, MouseBack, MouseForward,
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
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "快捷键为空"),
            ParseError::UnknownModifier(s) => write!(f, "未知修饰键: {s}"),
            ParseError::UnknownKey(s) => write!(f, "未知按键: {s}"),
        }
    }
}

impl std::error::Error for ParseError {}

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

    fn modifier(s: &str) -> Option<Modifier> {
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

pub fn key_name(k: Key) -> &'static str {
    use Key::*;
    match k {
        A => "A", B => "B", C => "C", D => "D", E => "E", F => "F",
        G => "G", H => "H", I => "I", J => "J", K => "K", L => "L",
        M => "M", N => "N", O => "O", P => "P", Q => "Q", R => "R",
        S => "S", T => "T", U => "U", V => "V", W => "W", X => "X",
        Y => "Y", Z => "Z",
        Digit0 => "0", Digit1 => "1", Digit2 => "2", Digit3 => "3",
        Digit4 => "4", Digit5 => "5", Digit6 => "6", Digit7 => "7",
        Digit8 => "8", Digit9 => "9",
        F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4",
        F5 => "F5", F6 => "F6", F7 => "F7", F8 => "F8",
        F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
        Comma => ",", Period => ".", Slash => "/", Backslash => "\\",
        Semicolon => ";", Quote => "\"", Backquote => "`",
        Minus => "-", Equal => "=", BracketLeft => "[", BracketRight => "]",
        Enter => "Enter", Escape => "Esc", Tab => "Tab", Space => "Space",
        Backspace => "Backspace", Delete => "Delete", Insert => "Insert",
        CapsLock => "CapsLock",
        Shift => "Shift", Control => "Ctrl", Alt => "Alt", Meta => "Meta",
        Home => "Home", End => "End", PageUp => "PageUp", PageDown => "PageDown",
        ArrowUp => "Up", ArrowDown => "Down", ArrowLeft => "Left", ArrowRight => "Right",
        F13 => "F13", F14 => "F14", F15 => "F15", F16 => "F16", F17 => "F17", F18 => "F18",
        F19 => "F19", F20 => "F20", F21 => "F21", F22 => "F22", F23 => "F23", F24 => "F24",
        MediaPlayPause => "MediaPlayPause", MediaPrev => "MediaPrev", MediaNext => "MediaNext",
        VolumeMute => "VolumeMute", VolumeDown => "VolumeDown", VolumeUp => "VolumeUp",
        Numpad0 => "Numpad0", Numpad1 => "Numpad1", Numpad2 => "Numpad2", Numpad3 => "Numpad3",
        Numpad4 => "Numpad4", Numpad5 => "Numpad5", Numpad6 => "Numpad6", Numpad7 => "Numpad7",
        Numpad8 => "Numpad8", Numpad9 => "Numpad9",
        NumpadAdd => "NumpadAdd", NumpadSubtract => "NumpadSubtract",
        NumpadMultiply => "NumpadMultiply", NumpadDivide => "NumpadDivide",
        NumpadDecimal => "NumpadDecimal", NumpadEnter => "NumpadEnter", NumLock => "NumLock",
        MouseMiddle => "MouseMiddle", MouseBack => "MouseBack", MouseForward => "MouseForward",
    }
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

/// 触发键单元：单个组合键，或一串按键序列（顺序按下，如 leader 键序列）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Trigger {
    /// 单次组合键（如 `Ctrl+K`）。
    Combo(Shortcut),
    /// 按键序列（如 `F9 J K`：依次按下 F9 → J → K）。首步即 leader 键。
    Sequence(Vec<Shortcut>),
}

impl Trigger {
    /// 解析触发键字符串：单个 token → 组合键，多个 token（空白分隔）→ 序列。
    /// 约定：组合键用 `+` 且不含空格；序列步之间用空格。`"Space"`（空格键）是
    /// 单 token 仍为组合，`"F9 Space"` 则是「F9 后接空格」的序列。
    pub fn parse(s: &str) -> Result<Trigger, ParseError> {
        let tokens: Vec<&str> = s.split_whitespace().collect();
        match tokens.as_slice() {
            [] => Err(ParseError::Empty),
            [one] => Ok(Trigger::Combo(one.parse()?)),
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

    /// 各步（组合键返回单元素切片，便于统一遍历）。
    pub fn steps(&self) -> &[Shortcut] {
        match self {
            Trigger::Combo(s) => std::slice::from_ref(s),
            Trigger::Sequence(steps) => steps,
        }
    }

    /// 首步（leader 键）。
    pub fn first(&self) -> &Shortcut {
        &self.steps()[0]
    }
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

/// 键序列超时的默认毫秒数（leader 后超过该时长未按下一步即回退）。
pub const DEFAULT_SEQUENCE_TIMEOUT_MS: u64 = 1000;

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
}

#[cfg(feature = "automation")]
impl Condition {
    /// 在当前变量上下文 + 前台窗口上下文下求值。路径类条件支持 `{变量名}` 占位符。
    /// 变量/字段不存在时视为「条件不成立」（返回 false）；前台类条件在无前台上下文
    /// （`frontmost` 为 None，如 Linux/Wayland 受限）时同样视为不成立。
    pub fn matches(&self, vars: &Vars, frontmost: Option<&FrontmostContext>) -> bool {
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

/// 一个键位层：快捷键/改键可归属某层，仅当该层激活时才生效（`None` = 基层层，始终生效）。
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
    /// 归属层 id（`None` = 基层层，始终生效；`Some` = 仅该层激活时生效）。
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
/// - **单次修饰**：`oneshot` 非空时，单击 `from` 武装该修饰键、应用到下一个非修饰键后自动释放。
/// - **粘滞修饰**：`sticky` 非空时，单击 `from` 锁定该修饰键、再次单击解锁。
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
    /// 归属层 id（`None` = 基层层；`Some` = 仅该层激活时生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// 长按进入的层 id（momentary 切层）；与 `hold`（输出键）互斥。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_layer: Option<String>,
    /// tap-hold 判定阈值（毫秒）；0 视为默认 200。
    #[serde(default = "default_tap_timeout_ms")]
    pub tap_timeout_ms: u64,
    /// 单次修饰键（`Ctrl/Alt/Shift/Meta`）：单击 `from` 武装、下一个非修饰键后自动释放。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oneshot: Option<String>,
    /// 粘滞修饰键（`Ctrl/Alt/Shift/Meta`）：单击 `from` 锁定、再次单击解锁。
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
    /// 是否有 tap-hold / tap-dance / 切层 行为（短按/长按/双击/三击/长按切层 任一非空白）。
    /// 不含 oneshot/sticky（那些是修饰键模式，见 [`Remap::is_oneshot`] / [`Remap::is_sticky`]）。
    pub fn is_tap_hold(&self) -> bool {
        let nonempty = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
        nonempty(&self.tap)
            || nonempty(&self.hold)
            || nonempty(&self.hold_layer)
            || nonempty(&self.tap2)
            || nonempty(&self.tap3)
    }

    /// 是否单次修饰模式。
    pub fn is_oneshot(&self) -> bool {
        self.oneshot.as_deref().is_some_and(|v| !v.trim().is_empty())
    }

    /// 是否粘滞修饰模式。
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

    /// 单次修饰键（解析失败返回 None）。
    pub fn oneshot_key(&self) -> Option<Key> {
        self.oneshot.as_deref().and_then(|s| s.parse::<Key>().ok())
    }

    /// 粘滞修饰键（解析失败返回 None）。
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

/// 应用设置（配置的一部分，随 JSON 一起落盘/跨平台同步）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
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
}

/// 检测配置中的快捷键/改键冲突。
///
/// - 硬冲突（[`Severity::Error`]，应阻止保存）：重复触发键（组合/序列）、
///   序列 leader 遮蔽单组合、重复改键来源、改键来源与快捷键主键相同（改键优先，快捷键将失效）。
/// - 软冲突（[`Severity::Warn`]，仅提示）：触发键超集重叠（更宽松的组合会遮蔽更具体的组合）。
pub fn detect_conflicts(cfg: &Config) -> Vec<Conflict> {
    use std::collections::HashMap;

    let mut out: Vec<Conflict> = Vec::new();

    // 1) 重复触发键（同层内 enabled 且跨不同条目；不同层可共用同键）。
    //    单组合按 (层, mods, key) 判重；序列按 (层, 完整字符串) 判重。
    let mut seen_combo: HashMap<(Option<String>, BTreeSet<Modifier>, Key), String> = HashMap::new();
    let mut seen_seq: HashMap<(Option<String>, String), ()> = HashMap::new();
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
                        });
                    }
                }
                Err(_) => {}
            }
        }
    }

    // 1b) 序列 leader 遮蔽单组合（同层内）：序列首步吞掉该键，使同名组合键失效。
    let mut combos_by_layer: HashMap<Option<String>, Vec<(String, Shortcut)>> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(Trigger::Combo(sc)) = Trigger::parse(t) {
                combos_by_layer.entry(s.layer.clone()).or_default().push((t.clone(), sc));
            }
        }
    }
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
                            });
                        }
                    }
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
            });
        }
    }

    out
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
    /// 原地归一化：把旧版动作迁移为 `App` 动作。
    pub fn migrate(&mut self) {
        for s in &mut self.shortcuts {
            s.actions = std::mem::take(&mut s.actions)
                .into_iter()
                .map(migrate_action)
                .collect();
        }
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

        // 单次/粘滞/双击/三击键名逐项校验（坏键名单独清空并提示）。
        let mut clear_key = |field: &mut Option<String>, label: &str| {
            if let Some(v) = field {
                if v.parse::<Key>().is_err() {
                    ignored.push(format!("改键「{}」的{label}「{v}」已忽略：键名无法解析", r.from));
                    *field = None;
                }
            }
        };
        clear_key(&mut item.oneshot, "单次修饰键");
        clear_key(&mut item.sticky, "粘滞修饰键");
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
        detect_conflicts(cfg)
            .into_iter()
            .filter(|c| c.severity == Severity::Error)
            .collect()
    }

    fn warns(cfg: &Config) -> Vec<Conflict> {
        detect_conflicts(cfg)
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
        assert!(detect_conflicts(&cfg).is_empty());
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
            .matches(&vars, None));
        assert!(Condition::NotEquals { var: "f".into(), field: "ext".into(), value: ".zip".into() }
            .matches(&vars, None));
        // field 为空 → 比较完整路径
        assert!(Condition::Equals { var: "f".into(), field: String::new(), value: "C:\\d\\a.txt".into() }
            .matches(&vars, None));
        // 变量或字段不存在 → 条件不成立
        assert!(!Condition::Equals { var: "f".into(), field: "nope".into(), value: "x".into() }
            .matches(&vars, None));
        assert!(!Condition::Equals { var: "missing".into(), field: String::new(), value: "x".into() }
            .matches(&vars, None));
    }

    #[cfg(feature = "automation")]
    #[test]
    fn condition_path_predicates() {
        let mut vars = sample_vars();
        // 当前目录（必然存在、是目录）+ 一个必不存在的路径
        assert!(Condition::Exists { path: ".".into() }.matches(&vars, None));
        assert!(Condition::IsDir { path: ".".into() }.matches(&vars, None));
        assert!(!Condition::IsFile { path: ".".into() }.matches(&vars, None));
        assert!(Condition::NotExists { path: "___kada_no_such_path___".into() }.matches(&vars, None));
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
        assert!(Condition::Exists { path: "{cur}".into() }.matches(&vars, None));
        assert!(Condition::IsDir { path: "{cur}".into() }.matches(&vars, None));
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
        assert!(Condition::ModifiedWithin { path: tmp_path.clone(), minutes: 10 }.matches(&vars, None));
        let _ = std::fs::remove_file(&tmp);

        // 不存在的路径 → 条件不成立
        assert!(!Condition::ModifiedWithin { path: "___kada_no_such_path___".into(), minutes: 10 }
            .matches(&vars, None));
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
        assert!(Condition::FrontmostApp { app: "chrome".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(Condition::FrontmostApp { app: "CHROME.EXE".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(!Condition::FrontmostApp { app: "code".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(Condition::NotFrontmostApp { app: "code".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(!Condition::NotFrontmostApp { app: "chrome".into() }.matches(&Vars::new(), chrome.as_ref()));

        // 通配匹配（* / ?）
        assert!(Condition::FrontmostApp { app: "chrome.*".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(Condition::FrontmostApp { app: "chr?me.exe".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(!Condition::FrontmostApp { app: "*.txt".into() }.matches(&Vars::new(), chrome.as_ref()));

        // 窗口标题子串（不区分大小写）
        assert!(Condition::WindowTitleContains { text: "chrome".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(Condition::WindowTitleContains { text: "CHROME".into() }.matches(&Vars::new(), chrome.as_ref()));
        assert!(!Condition::WindowTitleContains { text: "safari".into() }.matches(&Vars::new(), chrome.as_ref()));

        // 无前台上下文 → 前台条件一律不成立
        assert!(!Condition::FrontmostApp { app: "chrome".into() }.matches(&Vars::new(), none.as_ref()));
        assert!(!Condition::NotFrontmostApp { app: "code".into() }.matches(&Vars::new(), none.as_ref()));
        assert!(!Condition::WindowTitleContains { text: "x".into() }.matches(&Vars::new(), none.as_ref()));
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
            .matches(&vars, None));
        assert!(Condition::Equals { var: "text".into(), field: "".into(), value: "hello".into() }
            .matches(&vars, None));
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