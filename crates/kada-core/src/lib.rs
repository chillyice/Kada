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

/// 操作系统动作：对文件/目录执行复制、剪切、粘贴、删除、新建、压缩或取属性。
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

fn default_var() -> String {
    "file".into()
}

fn default_status_interval_ms() -> u64 {
    1000
}

/// 条件：供「条件判断」动作在触发前求值，为真才执行后续动作。
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
}

impl Condition {
    /// 在当前变量上下文下求值。路径类条件支持 `{变量名}` 占位符。
    /// 变量/字段不存在时视为「条件不成立」（返回 false）。
    pub fn matches(&self, vars: &Vars) -> bool {
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
        }
    }
}

/// 距今多少秒（文件修改时间在未来时视为 0 秒，即「刚修改」）。
fn elapsed_since_now(t: std::time::SystemTime) -> u64 {
    match std::time::SystemTime::now().duration_since(t) {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    }
}

/// 应用操作：启动、关闭、查询运行状态、重启。
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
    Text { #[serde(default)] text: String, #[serde(default)] mode: TextMode },
    /// 执行命令（CMD 或 PowerShell），结果进消息中心。
    Command { shell: Shell, command: String, #[serde(default)] show_output: bool },
    /// 执行 CMD 命令（旧版动作，加载时自动迁移为 `Command(Cmd)`）。
    Cmd { command: String, #[serde(default)] show_output: bool },
    /// 执行 PowerShell 命令（旧版动作，加载时自动迁移为 `Command(Powershell)`）。
    Powershell { command: String, #[serde(default)] show_output: bool },
    /// 启动可执行文件，可选参数。（旧版动作，加载时自动迁移为 `App::Launch`。）
    Launch { program: String, args: Vec<String> },
    /// 在文件管理器中打开某个目录。（旧版动作，加载时自动迁移为 `Os::OpenFolder`。）
    OpenFolder { path: String },
    /// 同时按下若干键（组合键，如 ["Ctrl","C"]；单键即点按）。
    Keys { keys: Vec<String> },
    /// 暂停 ms 毫秒。
    PauseMs { ms: u64 },
    /// 强制结束目标程序的所有进程（Windows `taskkill /F /T`，Linux `pkill -f`）。
    /// （旧版动作，加载时自动迁移为 `App::Close`。）
    CloseProgram { program: String },
    /// 操作系统动作（文件复制/剪切/粘贴/删除/新建/压缩/取属性）。
    Os { operation: OsOperation },
    /// 应用动作（打开/关闭/查询状态/重启）。
    App { operation: AppOperation },
    /// 条件判断：`condition` 为真时按顺序执行 `then`，否则执行 `otherwise`（可空）。
    If {
        condition: Condition,
        #[serde(default)]
        then: Vec<Action>,
        #[serde(default)]
        otherwise: Vec<Action>,
    },
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
            Action::Command { command, .. } => non_empty("命令", command),
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
            Action::Os { operation } => {
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
            Action::App { operation } => {
                use AppOperation::*;
                match operation {
                    Launch { program, .. } | Close { program } | Restart { program, .. } => {
                        non_empty("程序", program)
                    }
                    // 变量名允许为空（执行时回退到 "app"），只要求程序非空。
                    Status { program, .. } => non_empty("程序", program),
                }
            }
            Action::If { condition, then, otherwise } => {
                condition.validate()?;
                for a in then.iter().chain(otherwise.iter()) {
                    a.validate()?;
                }
                Ok(())
            }
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Action::Text { text: String::new(), mode: TextMode::Input }
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
    /// 文本值。
    Text(String),
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
/// 布尔→"true"/"false"、文本→原文）。变量或字段不存在返回 None。
pub fn var_field(var: &str, field: &str, vars: &Vars) -> Option<String> {
    match vars.get(var)? {
        Value::File(obj) => file_field(obj, field),
        Value::Bool(b) => match field {
            "" | "value" => Some(b.to_string()),
            _ => None,
        },
        Value::Text(t) => match field {
            "" | "value" => Some(t.clone()),
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
        Action::Launch { program, args } => {
            Action::App { operation: AppOperation::Launch { program, args } }
        }
        Action::CloseProgram { program } => {
            Action::App { operation: AppOperation::Close { program } }
        }
        Action::Cmd { command, show_output } => Action::Command {
            shell: Shell::Cmd,
            command,
            show_output,
        },
        Action::Powershell { command, show_output } => Action::Command {
            shell: Shell::Powershell,
            command,
            show_output,
        },
        Action::OpenFolder { path } => {
            Action::Os { operation: OsOperation::OpenFolder { path } }
        }
        Action::If { condition, then, otherwise } => Action::If {
            condition,
            then: then.into_iter().map(migrate_action).collect(),
            otherwise: otherwise.into_iter().map(migrate_action).collect(),
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

    let mut out = Config { settings: cfg.settings.clone(), folders, ..Default::default() };

    for s in &cfg.shortcuts {
        let label = s.name.clone().unwrap_or_else(|| "（未命名）".into());
        let mut item = s.clone();

        if let Some(fid) = &item.folder {
            if !valid_folder_ids.contains(fid) {
                ignored.push(format!("快捷键「{label}」所属目录已忽略：目录不存在"));
                item.folder = None;
            }
        }

        let mut kept_triggers = Vec::new();
        for t in &item.triggers {
            match t.parse::<Shortcut>() {
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
        let ok_to = r.to.parse::<Key>().is_ok();
        if ok_from && ok_to {
            out.remaps.push(r.clone());
        } else {
            ignored.push(format!("改键「{} → {}」已忽略：键名无法解析", r.from, r.to));
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
            shortcuts: vec![ShortcutItem {
                name: Some("地址".into()),
                description: Some("常用收货地址".into()),
                triggers: vec!["Ctrl+Alt+K".into(), "Alt+K".into()],
                actions: vec![Action::Text { text: "上海市徐汇区……".into(), mode: TextMode::Input }],
                folder: None,
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
        assert_eq!(
            cfg.shortcuts[0].actions,
            vec![Action::Text { text: "hi".into(), mode: TextMode::Input }]
        );
    }

    #[test]
    fn actions_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Text { text: "你好".into(), mode: TextMode::Input },
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
            actions: vec![Action::Text { text: String::new(), mode: TextMode::Input }],
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

#[cfg(test)]
mod os_and_sanitize_tests {
    use super::*;

    #[test]
    fn os_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::Os {
                operation: OsOperation::Copy { source: "C:\\a".into(), dest: "D:\\b".into() },
            },
            Action::Os {
                operation: OsOperation::GetFileProps { path: "C:\\f.txt".into(), var: "f".into() },
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
            operation: OsOperation::Copy { source: "".into(), dest: "".into() }
        }
        .validate()
        .is_err());
        assert!(Action::Os { operation: OsOperation::Delete { path: "  ".into() } }
            .validate()
            .is_err());
        assert!(Action::Os {
            operation: OsOperation::GetFileProps { path: "x".into(), var: "".into() }
        }
        .validate()
        .is_ok());
    }

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

    #[test]
    fn sanitize_drops_invalid_parts_keeps_valid() {
        let cfg = Config {
            folders: vec![],
            shortcuts: vec![
                ShortcutItem {
                    triggers: vec!["Ctrl+K".into()],
                    actions: vec![Action::Text { text: "ok".into(), mode: TextMode::Input }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Bad+Key".into()],
                    actions: vec![Action::Text { text: "bad".into(), mode: TextMode::Input }],
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Cmd { command: "  ".into(), show_output: false }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            remaps: vec![
                Remap { from: "CapsLock".into(), to: "Ctrl".into(), enabled: true },
                Remap { from: "NotAKey".into(), to: "Ctrl".into(), enabled: true },
            ],
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

    #[test]
    fn condition_equals_and_field_resolution() {
        let vars = sample_vars();
        assert!(Condition::Equals { var: "f".into(), field: "ext".into(), value: ".txt".into() }
            .matches(&vars));
        assert!(Condition::NotEquals { var: "f".into(), field: "ext".into(), value: ".zip".into() }
            .matches(&vars));
        // field 为空 → 比较完整路径
        assert!(Condition::Equals { var: "f".into(), field: String::new(), value: "C:\\d\\a.txt".into() }
            .matches(&vars));
        // 变量或字段不存在 → 条件不成立
        assert!(!Condition::Equals { var: "f".into(), field: "nope".into(), value: "x".into() }
            .matches(&vars));
        assert!(!Condition::Equals { var: "missing".into(), field: String::new(), value: "x".into() }
            .matches(&vars));
    }

    #[test]
    fn condition_path_predicates() {
        let mut vars = sample_vars();
        // 当前目录（必然存在、是目录）+ 一个必不存在的路径
        assert!(Condition::Exists { path: ".".into() }.matches(&vars));
        assert!(Condition::IsDir { path: ".".into() }.matches(&vars));
        assert!(!Condition::IsFile { path: ".".into() }.matches(&vars));
        assert!(Condition::NotExists { path: "___kada_no_such_path___".into() }.matches(&vars));
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
        assert!(Condition::Exists { path: "{cur}".into() }.matches(&vars));
        assert!(Condition::IsDir { path: "{cur}".into() }.matches(&vars));
    }

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
        assert!(Condition::ModifiedWithin { path: tmp_path.clone(), minutes: 10 }.matches(&vars));
        let _ = std::fs::remove_file(&tmp);

        // 不存在的路径 → 条件不成立
        assert!(!Condition::ModifiedWithin { path: "___kada_no_such_path___".into(), minutes: 10 }
            .matches(&vars));
        // 路径为空 → 校验拒绝
        assert!(Condition::ModifiedWithin { path: "  ".into(), minutes: 10 }.validate().is_err());
        assert!(Condition::ModifiedWithin { path: tmp_path, minutes: 10 }.validate().is_ok());
    }

    #[test]
    fn if_action_validate_recurses() {
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Text { text: "ok".into(), mode: TextMode::Input }],
            otherwise: vec![],
        }
        .validate()
        .is_ok());
        // 条件为空 → 拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: "  ".into() },
            then: vec![],
            otherwise: vec![],
        }
        .validate()
        .is_err());
        // 嵌套动作非法 → 递归拒绝
        assert!(Action::If {
            condition: Condition::Exists { path: ".".into() },
            then: vec![Action::Cmd { command: "  ".into(), show_output: false }],
            otherwise: vec![],
        }
        .validate()
        .is_err());
    }

    #[test]
    fn if_action_json_roundtrip() {
        // 递归嵌套的 If 也能完整往返（校验 serde tag 与 field 默认值）。
        let a = Action::If {
            condition: Condition::Equals { var: "f".into(), field: "ext".into(), value: ".txt".into() },
            then: vec![Action::Text { text: "yes".into(), mode: TextMode::Input }],
            otherwise: vec![Action::If {
                condition: Condition::Exists { path: "{f}".into() },
                then: vec![],
                otherwise: vec![Action::PauseMs { ms: 10 }],
            }],
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a, "json: {json}");
    }

    #[test]
    fn app_action_json_roundtrip_and_validate() {
        let actions = vec![
            Action::App {
                operation: AppOperation::Launch { program: "notepad.exe".into(), args: vec!["a.txt".into()] },
            },
            Action::App {
                operation: AppOperation::Status {
                    program: "notepad.exe".into(),
                    var: "running".into(),
                    retries: 3,
                    interval_ms: 500,
                },
            },
            Action::App {
                operation: AppOperation::Restart { program: "notepad.exe".into(), args: vec![] },
            },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "{a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions);

        // 程序为空必被拒绝；Status 变量名可空。
        assert!(Action::App { operation: AppOperation::Close { program: "".into() } }
            .validate()
            .is_err());
        assert!(Action::App {
            operation: AppOperation::Status {
                program: "x".into(),
                var: "".into(),
                retries: 0,
                interval_ms: 0,
            }
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn migrate_legacy_launch_and_close_program() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Launch { program: "notepad.exe".into(), args: vec![] },
                    Action::CloseProgram { program: "notepad.exe".into() },
                    Action::If {
                        condition: Condition::Exists { path: ".".into() },
                        then: vec![Action::Launch { program: "x".into(), args: vec![] }],
                        otherwise: vec![],
                    },
                ],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        cfg.migrate();
        let acts = &cfg.shortcuts[0].actions;
        assert!(matches!(acts[0], Action::App { operation: AppOperation::Launch { .. } }));
        assert!(matches!(acts[1], Action::App { operation: AppOperation::Close { .. } }));
        // 嵌套 If 里的旧动作也迁移
        match &acts[2] {
            Action::If { then, .. } => {
                assert!(matches!(then[0], Action::App { operation: AppOperation::Launch { .. } }));
            }
            other => panic!("expected If, got {other:?}"),
        }
    }

    #[test]
    fn bool_value_substitution_and_compare() {
        let mut vars = Vars::new();
        vars.insert("running".into(), Value::Bool(true));
        vars.insert("text".into(), Value::Text("hello".into()));
        assert_eq!(substitute_vars("{running}", &vars), "true");
        assert_eq!(substitute_vars("{running.value}", &vars), "true");
        assert_eq!(substitute_vars("{text}", &vars), "hello");
        // 条件判断能比较布尔变量
        assert!(Condition::Equals { var: "running".into(), field: "".into(), value: "true".into() }
            .matches(&vars));
        assert!(Condition::Equals { var: "text".into(), field: "".into(), value: "hello".into() }
            .matches(&vars));
    }
}

#[cfg(test)]
mod new_action_and_folder_tests {
    use super::*;

    #[test]
    fn text_mode_and_shell_json_roundtrip() {
        let actions = vec![
            Action::Text { text: "hi".into(), mode: TextMode::Input },
            Action::Text { text: "".into(), mode: TextMode::ToUpper },
            Action::Text { text: "".into(), mode: TextMode::ToLower },
            Action::Command { shell: Shell::Cmd, command: "echo hi".into(), show_output: false },
            Action::Command { shell: Shell::Powershell, command: "Get-Date".into(), show_output: true },
        ];
        for a in &actions {
            assert!(a.validate().is_ok(), "应通过校验: {a:?}");
        }
        let json = serde_json::to_string(&actions).unwrap();
        let back: Vec<Action> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, actions, "json: {json}");

        // 旧版 text（无 mode）默认 Input；Command 命令为空被拒绝。
        let legacy: Action = serde_json::from_str(r#"{"type":"text","text":"x"}"#).unwrap();
        assert_eq!(legacy, Action::Text { text: "x".into(), mode: TextMode::Input });
        assert!(Action::Command { shell: Shell::Cmd, command: "  ".into(), show_output: false }
            .validate()
            .is_err());
    }

    #[test]
    fn migrate_cmd_powershell_open_folder() {
        let mut cfg = Config {
            shortcuts: vec![ShortcutItem {
                triggers: vec!["Ctrl+K".into()],
                actions: vec![
                    Action::Cmd { command: "echo hi".into(), show_output: true },
                    Action::Powershell { command: "Get-Date".into(), show_output: false },
                    Action::OpenFolder { path: "C:\\Users".into() },
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
        assert!(matches!(acts[2], Action::Os { operation: OsOperation::OpenFolder { .. } }));
    }

    #[test]
    fn os_open_folder_new_folder_validate() {
        assert!(Action::Os { operation: OsOperation::OpenFolder { path: "C:\\".into() } }
            .validate()
            .is_ok());
        assert!(Action::Os { operation: OsOperation::NewFolder { path: "C:\\x".into() } }
            .validate()
            .is_ok());
        assert!(Action::Os { operation: OsOperation::OpenFolder { path: "  ".into() } }
            .validate()
            .is_err());
        assert!(Action::Os { operation: OsOperation::NewFolder { path: "".into() } }
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
                    actions: vec![Action::Text { text: "x".into(), mode: TextMode::Input }],
                    folder: Some("a".into()),
                    enabled: true,
                    ..Default::default()
                },
                ShortcutItem {
                    triggers: vec!["Ctrl+J".into()],
                    actions: vec![Action::Text { text: "y".into(), mode: TextMode::Input }],
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