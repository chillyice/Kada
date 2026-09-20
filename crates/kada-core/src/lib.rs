//! 咔哒核心：跨平台的键模型、快捷键解析与匹配。
//!
//! 零重量纯逻辑：Windows / macOS / Linux 钩子层都产出 [`RawEvent`]，
//! 由这里的匹配器统一判定触发。配置模型以 JSON 形式落盘和跨平台同步。
//!
//! 键名命名参考 platform 无关的中性名（Enter/Escape），macOS 的 ⌘
//! 由 [`Modifier::Meta`] 承载，平台层负责把 OS 键映射成 [`Key`]。

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

/// 单个按键。与具体 OS 键码无关，由平台层映射进来。
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum Key {
    A, B, C, D, E, F, G, H, I, J, K, L, M,
    N, O, P, Q, R, S, T, U, V, W, X, Y, Z,
    Digit0, Digit1, Digit2, Digit3, Digit4,
    Digit5, Digit6, Digit7, Digit8, Digit9,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    Comma, Period, Slash, Backslash, Semicolon, Quote, Backquote,
    Minus, Equal, BracketLeft, BracketRight,
    Enter, Escape, Tab, Space, Backspace, Delete, Insert,
    CapsLock, Shift, Control, Alt, Meta,
    Home, End, PageUp, PageDown,
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,
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
            "," => Comma, "." => Period, "/" => Slash, "\\" => Backslash,
            ";" => Semicolon, "\"" => Quote, "`" => Backquote,
            "-" => Minus, "=" => Equal, "[" => BracketLeft, "]" => BracketRight,
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
    }
}

/// 事件 `e` 是否命中快捷键 `s`：主键一致，且事件修饰键 ⊇ 快捷键修饰键。
/// （按下 Ctrl+Shift+K 也会命中 Ctrl+K——宽松规则避免用户按错一个多余的
///  Shift 就触发失败。）
pub fn matches(e: &RawEvent, s: &Shortcut) -> bool {
    e.key == s.key && e.mods.is_superset(&s.mods)
}

// ---------------------------------------------------------------------------
// 配置模型：咔哒的配置是跨平台同步介质，直接以 JSON 形式落盘/交换。
// 快捷键以 "Ctrl+Alt+K" 文本存放（人类可读、可编辑），加载时解析校验。
// ---------------------------------------------------------------------------

use serde::{Deserialize, Serialize};

/// 触发后执行的单个动作。一个快捷键可挂多个动作，按顺序执行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 把文本粘贴到当前焦点（中文等 Unicode 走剪贴板最稳）。
    Text { text: String },
    /// 执行 CMD 命令（Windows `cmd /C`；Linux `sh -c`）。
    Cmd { command: String, #[serde(default)] show_output: bool },
    /// 执行 PowerShell 命令（仅 Windows）。
    Powershell { command: String, #[serde(default)] show_output: bool },
    /// 启动可执行文件，可选参数。
    Launch { program: String, args: Vec<String> },
    /// 在文件管理器中打开某个目录。
    OpenFolder { path: String },
    /// 同时按下若干键（组合键，如 ["Ctrl","C"]；单键即点按）。
    Keys { keys: Vec<String> },
    /// 暂停 ms 毫秒。
    PauseMs { ms: u64 },
    /// 强制结束目标程序的所有进程（Windows `taskkill /F /T`，Linux `pkill -f`）。
    CloseProgram { program: String },
}

impl Action {
    /// 校验动作可执行：键名可解析、必要字段非空。非法返回中文错误。
    pub fn validate(&self) -> Result<(), String> {
        let check_key = |label: &str, k: &str| {
            k.parse::<Key>().map_err(|e| format!("{label}「{k}」无效：{e}"))?;
            Ok::<(), String>(())
        };
        let non_empty = |label: &str, v: &str| {
            if v.trim().is_empty() {
                return Err(format!("{label}不能为空"));
            }
            Ok(())
        };
        match self {
            Action::Text { .. } | Action::PauseMs { .. } => Ok(()),
            Action::Cmd { command, .. } => non_empty("CMD 命令", command),
            Action::Powershell { command, .. } => non_empty("PowerShell 命令", command),
            Action::Launch { program, .. } => non_empty("程序路径", program),
            Action::CloseProgram { program } => non_empty("程序名", program),
            Action::OpenFolder { path } => non_empty("目录", path),
            Action::Keys { keys } => {
                if keys.is_empty() {
                    return Err("按键组合不能为空".into());
                }
                for k in keys {
                    check_key("组合键", k)?;
                }
                Ok(())
            }
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Action::Text { text: String::new() }
    }
}

/// 一条快捷键规则：一个或多个触发组合 → 一串动作（按顺序执行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ShortcutItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 简短描述（列表/气泡展示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 如 ["Ctrl+Alt+K"]，可多个。
    #[serde(default)]
    pub triggers: Vec<String>,
    /// 触发后按顺序执行的多个动作。
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

/// 一条改键规则：`from` 键按下时改发 `to` 键。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Remap {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub enabled: bool,
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
    /// 启动后最小化到托盘（不弹主窗口）。
    #[serde(default)]
    pub launch_minimized: bool,
}

/// 根配置。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub shortcuts: Vec<ShortcutItem>,
    #[serde(default)]
    pub remaps: Vec<Remap>,
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
/// - 硬冲突（[`Severity::Error`]，应阻止保存）：重复触发键、重复改键来源、
///   改键来源与快捷键主键相同（改键优先，快捷键将失效）。
/// - 软冲突（[`Severity::Warn`]，仅提示）：触发键超集重叠（更宽松的组合会遮蔽更具体的组合）。
pub fn detect_conflicts(cfg: &Config) -> Vec<Conflict> {
    use std::collections::HashMap;

    let mut out: Vec<Conflict> = Vec::new();

    // 1) 重复触发键（enabled 且跨不同条目）
    let mut seen: HashMap<(BTreeSet<Modifier>, Key), String> = HashMap::new();
    for s in &cfg.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                let k = (sc.mods, sc.key);
                if let Some(first) = seen.get(&k) {
                    out.push(Conflict {
                        severity: Severity::Error,
                        message: format!("触发键「{t}」与「{first}」重复，多个快捷键共用同一组合"),
                    });
                } else {
                    seen.insert(k, t.clone());
                }
            }
        }
    }

    // 2) 重复改键来源
    let mut remap_from: HashMap<Key, String> = HashMap::new();
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        if let Ok(from) = r.from.parse::<Key>() {
            if let Some(first) = remap_from.get(&from) {
                out.push(Conflict {
                    severity: Severity::Error,
                    message: format!("改键来源「{first}」重复，多条改键都从「{first}」改起"),
                });
            } else {
                remap_from.insert(from, r.from.clone());
            }
        }
    }

    // 3) 改键来源与快捷键主键相同
    for r in &cfg.remaps {
        if !r.enabled {
            continue;
        }
        let Ok(from) = r.from.parse::<Key>() else { continue };
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
                                "改键「{} → {}」会拦截按键「{}」，使快捷键「{t}」失效",
                                r.from,
                                r.to,
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
            shortcuts: vec![ShortcutItem {
                name: Some("地址".into()),
                description: Some("常用收货地址".into()),
                triggers: vec!["Ctrl+Alt+K".into(), "Alt+K".into()],
                actions: vec![Action::Text { text: "上海市徐汇区……".into() }],
                enabled: true,
            }],
            remaps: vec![Remap { from: "CapsLock".into(), to: "Ctrl".into(), enabled: true }],
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
        assert_eq!(cfg.shortcuts[0].actions, vec![Action::Text { text: "hi".into() }]);
    }

    #[test]
    fn actions_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Text { text: "你好".into() },
            Action::Cmd { command: "echo hi".into(), show_output: false },
            Action::Powershell { command: "Get-Date".into(), show_output: true },
            Action::Launch { program: "notepad.exe".into(), args: vec!["a.txt".into()] },
            Action::OpenFolder { path: "C:\\Users".into() },
            Action::Keys { keys: vec!["Ctrl".into(), "C".into()] },
            Action::PauseMs { ms: 100 },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "动作应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 坏键名 / 空组合 / 空命令必被拒绝
        assert!(Action::Keys { keys: vec!["NotAKey".into()] }.validate().is_err());
        assert!(Action::Keys { keys: vec![] }.validate().is_err());
        assert!(Action::Cmd { command: "  ".into(), show_output: false }.validate().is_err());
        assert!(Action::Launch { program: "".into(), args: vec![] }.validate().is_err());
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;

    fn item(trigger: &str) -> ShortcutItem {
        ShortcutItem {
            triggers: vec![trigger.into()],
            actions: vec![Action::Text { text: String::new() }],
            enabled: true,
            ..Default::default()
        }
    }

    fn remap(from: &str, to: &str) -> Remap {
        Remap { from: from.into(), to: to.into(), enabled: true }
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
}