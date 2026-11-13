// 动作模型（C-03 从 lib.rs 搬出）：各类 Operation / Condition / Action 本体与默认值。
// 这里只有纯数据与校验，执行分支在 `kada-actions`。

use serde::{Deserialize, Serialize};

use crate::gesture;
use crate::keys::Key;

#[cfg(feature = "automation")]
use crate::{default_true, is_true, substitute_vars, var_field, Vars};

/// 虚拟桌面切换方向（[`SystemOperation::SwitchDesktop`]）：只有「上一个 / 下一个」。
/// 缺省 [`Next`](DesktopDirection::Next)——手工配置只写 `{"op":"switch_desktop"}` 时也能用。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopDirection {
    /// 向左 / 上一个桌面。
    Prev,
    /// 向右 / 下一个桌面。
    #[default]
    Next,
}

#[cfg(feature = "automation")]
impl DesktopDirection {
    /// 中文短标签（动作摘要 / 执行记录 / 界面下拉共用一套说法）。
    pub fn label(self) -> &'static str {
        match self {
            DesktopDirection::Prev => "上一个",
            DesktopDirection::Next => "下一个",
        }
    }
}

/// 文件/目录动作：对文件与目录做复制、剪切、粘贴、删除、新建、压缩、取属性。
///
/// 与 [`SystemOperation`]（操作系统动作）是**两类独立的动作**：界面下拉里一个叫「文件/目录」、
/// 一个叫「操作系统」。分类依据是「作用对象」——动的是文件系统就归这里，动的是操作系统本身
/// （虚拟桌面这类）就归 [`SystemOperation`]。
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
    /// 用系统默认程序打开文件（按扩展名关联的程序；传入目录则打开文件管理器）。
    OpenFile { path: String },
    /// 在文件管理器中定位（选中）某个文件或目录（Windows `explorer /select,`）。
    Reveal { path: String },
    /// 把 `source`（文件或目录）压缩为 `dest` 的 zip 归档。
    Zip { source: String, dest: String },
    /// 把 `source` 的 zip 压缩包解压到 `dest` 目录。
    Unzip { source: String, dest: String },
    /// 取文件属性，存入变量 `var`（默认 "file"），供后续动作用占位符引用。
    GetFileProps {
        path: String,
        #[serde(default = "default_var")]
        var: String,
    },
}

/// 操作系统动作：对操作系统本身做的动作（虚拟桌面 / 回收站 / 显示器 / 桌面图标 /
/// 后台发送 / 亮度）。与 [`OsOperation`]（文件/目录动作）分开，见那里的分类依据。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SystemOperation {
    /// 切换虚拟桌面：注入**系统默认**的切换快捷键（Windows `Win+Ctrl+←/→`、macOS `⌃←/→`、
    /// Linux GNOME `Super+PageUp/PageDown`）。不改系统键位设置，也不读当前桌面序号——
    /// 「切到第 3 个桌面」这种要按次数的需求请用「按键组合」动作自排。
    SwitchDesktop {
        #[serde(default)]
        direction: DesktopDirection,
    },
    /// 清空回收站（Windows `SHEmptyRecycleBin`，不弹确认 / 进度 / 声音）。
    EmptyRecycleBin,
    /// 关闭显示器（Windows 广播 `SC_MONITORPOWER`，动一下鼠标 / 按键就会亮回来）。
    MonitorOff,
    /// 切换桌面图标显示（Windows 给桌面 `SysListView32` 发 `0x7402`）。
    ToggleDesktopIcons,
    /// 向**非前台**窗口发送按键 / 文本（原 `Action::SendTo`，入口已并入操作系统）：
    /// Windows 用 `PostMessage`，全程不激活、不抢焦点，也不回落到前台注入。`target` 匹配
    /// 进程名或窗口标题（忽略大小写，含 `*`/`?` 走通配，否则子串）；多个命中取 Z 序最上层。
    /// 只有 Windows 实现，其它平台执行时报错并由冲突检测标平台缺失。目标程序靠 Raw Input /
    /// 焦点 / IME 收键时（Chromium、UWP、游戏、提权窗口）通常收不到——这是 `PostMessage`
    /// 的固有限制，不是回退前台的信号。
    SendTo {
        target: String,
        payload: SendPayload,
    },
    /// 显示器亮度（原 `Action::Brightness`，入口已并入操作系统）：主显示器调高 / 调低 /
    /// 恢复原始值。Windows 走 DDC/CI，其它平台执行报错并由冲突检测标平台缺失。
    Brightness {
        #[serde(default, skip_serializing_if = "is_default_brightness_op")]
        operation: BrightnessOperation,
        #[serde(
            default = "default_brightness_step",
            skip_serializing_if = "is_default_brightness_step"
        )]
        step: i64,
    },
}

/// 当前窗口动作（[`Action::Window`]，EXT-14）：对**触发那一刻的前台窗口**执行操作，
/// 不按程序名匹配、也不激活窗口（不抢焦点）。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum WindowOperation {
    /// 最大化 / 还原（按当前是否最大化切换）。
    #[default]
    MaximizeRestore,
    /// 最小化。
    Minimize,
    /// 置顶切换（最前层 ↔ 普通层）。
    ToggleTopmost,
    /// 在所在显示器的工作区里居中（最大化 / 最小化时先还原）。
    Center,
    /// 关闭（发送关闭请求，让目标程序自己走保存 / 确认流程）。
    Close,
    /// 关闭同类窗口：目标窗口所在进程的全部顶层窗口。
    CloseSameApp,
}

#[cfg(feature = "automation")]
impl WindowOperation {
    /// 中文短标签（动作摘要 / 执行记录 / 界面下拉共用一套说法）。
    pub fn label(self) -> &'static str {
        match self {
            WindowOperation::MaximizeRestore => "最大化 / 还原",
            WindowOperation::Minimize => "最小化",
            WindowOperation::ToggleTopmost => "置顶切换",
            WindowOperation::Center => "居中",
            WindowOperation::Close => "关闭",
            WindowOperation::CloseSameApp => "关闭同类窗口",
        }
    }
}

/// 外部工具的一个能力（占位）：目录里先列出能做什么，真正接入前执行会明确报错。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExternalCapability {
    /// 稳定 id（配置里存它）。
    pub id: &'static str,
    /// 展示名。
    pub name: &'static str,
}

/// 本机能对接的外部工具（按进程名探测是否在运行）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExternalToolInfo {
    /// 稳定 id（配置里存它）。
    pub id: &'static str,
    /// 展示名。
    pub name: &'static str,
    /// 进程名匹配片段：与进程可执行名（小写）做子串匹配。
    pub processes: &'static [&'static str],
    /// 工具能力（当前都是占位，执行只报「尚未接入」）。
    pub capabilities: &'static [ExternalCapability],
}

/// 已知外部工具目录。**只登记、不假装已接通**：探测到进程在跑时界面才出现对应动作，
/// 执行时明确报「尚未接入」，不会静默失败。
pub const EXTERNAL_TOOLS: &[ExternalToolInfo] = &[
    ExternalToolInfo {
        id: "bit-pouch",
        name: "比特口袋",
        processes: &["bit-pouch", "bitpouch", "比特口袋"],
        capabilities: &[ExternalCapability {
            id: "open",
            name: "打开",
        }],
    },
    ExternalToolInfo {
        id: "auto-assistant",
        name: "自动化助手",
        processes: &[
            "automation-assistant",
            "auto-assistant",
            "autoassistant",
            "自动化助手",
        ],
        capabilities: &[ExternalCapability {
            id: "run",
            name: "运行",
        }],
    },
];

/// 按 id 找外部工具；找不到返回 `None`。
pub fn external_tool(id: &str) -> Option<&'static ExternalToolInfo> {
    EXTERNAL_TOOLS.iter().find(|t| t.id == id)
}

/// 旧「截图」动作的目标（只为读得动老配置保留；功能已移除，执行会报错）。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOperation {
    /// 全部显示器拼成的虚拟屏幕。
    #[default]
    FullScreen,
    /// 冻结整块虚拟屏幕后让用户框选。
    Region,
    /// 触发那一刻的前台窗口。
    Window,
}

/// 旧「截图」动作的出口（只为读得动老配置保留）。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOutput {
    /// 写进系统剪贴板（图片）。
    #[default]
    Clipboard,
    /// 存成 PNG。
    File,
    /// 贴图浮窗。
    Pin,
}

/// 显示器亮度调节方向（[`Action::Brightness`]，EXT-22）。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessOperation {
    /// 调高 `step`。
    #[default]
    Up,
    /// 调低 `step`。
    Down,
    /// 恢复本次运行里第一次调节前的原始亮度（进程内缓存）。
    Restore,
}

#[cfg(feature = "automation")]
impl BrightnessOperation {
    /// 中文短标签。
    pub fn label(self) -> &'static str {
        match self {
            BrightnessOperation::Up => "调高",
            BrightnessOperation::Down => "调低",
            BrightnessOperation::Restore => "恢复",
        }
    }
}

#[cfg(feature = "automation")]
fn default_brightness_step() -> i64 {
    10
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

/// 触发侧条件（`when`）求值上下文：当前前台窗口 + 输入设备。
///
/// 前台上下文**惰性获取**：只有当某个带 `when` 的条目真的被判定时才查询（macOS 的前台
/// 查询走 AX 调用，不该让每次按键都买单）。壳层用 [`TriggerContext::lazy`] 传平台查询函数，
/// 单测用 [`TriggerContext::empty`] / [`TriggerContext::fixed`]。
#[cfg_attr(not(feature = "automation"), allow(dead_code))]
pub struct TriggerContext<'a> {
    /// 固定前台（单测）；`Some` 时直接用它、不查。
    fixed: Option<&'a FrontmostContext>,
    /// 惰性前台查询（壳层）；只在首次需要时调用一次。
    fetch: Option<&'a dyn Fn() -> Option<FrontmostContext>>,
    /// 触发事件的设备标识（仅 Linux 拿得到真设备名）。
    device: Option<&'a str>,
    /// 惰性查询的缓存（固定值不走这里）。
    cached: std::cell::OnceCell<Option<FrontmostContext>>,
}

impl<'a> TriggerContext<'a> {
    /// 无前台 / 无设备的空上下文（无 `when` 的按键路径、基础版、单测用）。
    pub fn empty() -> TriggerContext<'static> {
        TriggerContext {
            fixed: None,
            fetch: None,
            device: None,
            cached: std::cell::OnceCell::new(),
        }
    }

    /// 固定前台 + 设备（单测用）。
    pub fn fixed(frontmost: Option<&'a FrontmostContext>, device: Option<&'a str>) -> Self {
        TriggerContext {
            fixed: frontmost,
            fetch: None,
            device,
            cached: std::cell::OnceCell::new(),
        }
    }

    /// 惰性前台查询 + 设备（壳层用）：`fetch` 只在该事件真的需要判定 `when` 时被调用一次。
    pub fn lazy(fetch: &'a dyn Fn() -> Option<FrontmostContext>, device: Option<&'a str>) -> Self {
        TriggerContext {
            fixed: None,
            fetch: Some(fetch),
            device,
            cached: std::cell::OnceCell::new(),
        }
    }

    /// 当前前台上下文（固定值优先；否则首次调用时查询并缓存）。
    fn frontmost(&self) -> Option<&FrontmostContext> {
        if let Some(f) = self.fixed {
            return Some(f);
        }
        self.cached
            .get_or_init(|| self.fetch.and_then(|f| f()))
            .as_ref()
    }

    /// 当前前台进程名（惰性；无前台时 `None`）。全局排除列表在基础版也要用。
    pub fn frontmost_process(&self) -> Option<&str> {
        self.frontmost().map(|f| f.process_name.as_str())
    }

    /// 当前前台是否命中排除列表（空列表 / 查不到前台 = 不排除）。
    /// 语义同 `when` 的前台应用：不区分大小写、`*`/`?` 通配、否则子串。
    pub fn app_excluded(&self, patterns: &[String]) -> bool {
        !patterns.is_empty()
            && self
                .frontmost_process()
                .is_some_and(|name| gesture::app_excluded(patterns, name))
    }

    /// `when` 条件是否满足（`None` = 无门控，恒真）。触发时无变量，`vars` 为空表。
    #[cfg(feature = "automation")]
    pub(crate) fn when_ok(&self, when: Option<&Condition>) -> bool {
        match when {
            None => true,
            Some(c) => c.matches(&Vars::new(), self.frontmost(), self.device),
        }
    }
}

/// 条件：供「条件判断」动作 / 触发侧 `when` 门控求值，为真才生效。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    Equals {
        var: String,
        #[serde(default)]
        field: String,
        value: String,
    },
    /// 变量字段不等于某值。
    NotEquals {
        var: String,
        #[serde(default)]
        field: String,
        value: String,
    },
    /// 文件/目录的修改时间在最近 `minutes` 分钟内（`path` 支持 `{变量名}` 占位符）。
    ModifiedWithin {
        path: String,
        #[serde(default)]
        minutes: u64,
    },
    /// 前台进程名匹配（不区分大小写；含 `*`/`?` 走通配，否则子串匹配）。
    FrontmostApp { app: String },
    /// 前台进程名不匹配。
    NotFrontmostApp { app: String },
    /// 前台窗口标题包含该文本（不区分大小写）。
    WindowTitleContains { text: String },
    /// 触发键来自某台输入设备（多键盘按设备区分，见规划 7.1-㊷）。
    /// `id` 与设备标识（Linux evdev 设备名 / Windows HID 设备路径）按「不区分大小写、
    /// 含 `*`/`?` 走通配、否则子串」匹配；无设备上下文（macOS / 取不到设备）时恒不成立。
    DeviceIs { id: String },
}

#[cfg(feature = "automation")]
impl Condition {
    /// 在当前变量上下文 + 前台窗口上下文 + 设备上下文下求值。路径类条件支持 `{变量名}` 占位符。
    /// 变量/字段不存在时视为「条件不成立」（返回 false）；前台类条件在无前台上下文
    /// （`frontmost` 为 None，如 Linux/Wayland 受限）时同样视为不成立。设备类条件在无设备
    /// 上下文（`device` 为 None，如 macOS）时也不成立。
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

/// 输入设备的大类（设置页「设备是」下拉按它分组）。
///
/// 「设备是」原本只认键盘，现在覆盖**指针设备**：鼠标、触摸板、数位板。类别的用处只有两个
/// ——下拉里分组、以及同一块硬件被枚举成多条时（触摸板的鼠标口 + 触摸口）挑最具体的那条。
/// 它**不参与匹配**：配置里存的仍是 [`device_pattern`] 压出的字符串，任何类别的设备都用
/// 同一套子串 / 通配规则命中。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Touchpad,
    Tablet,
    Other,
}

impl DeviceKind {
    /// 序列化给界面的稳定 id（前端据此分组；不是界面文案）。
    pub fn id(self) -> &'static str {
        match self {
            DeviceKind::Keyboard => "keyboard",
            DeviceKind::Mouse => "mouse",
            DeviceKind::Touchpad => "touchpad",
            DeviceKind::Tablet => "tablet",
            DeviceKind::Other => "other",
        }
    }

    /// 同一块硬件的多条枚举里留哪一条：数位板 > 触摸板 > 鼠标 > 键盘 > 其它。
    ///
    /// 数位板会同时暴露「笔」（数位板）与鼠标口，触摸板也有触摸口与鼠标口——取最具体的那
    /// 个，用户看到的才是「这是块数位板」，而不是一堆同名的鼠标。
    pub fn rank(self) -> u8 {
        match self {
            DeviceKind::Tablet => 4,
            DeviceKind::Touchpad => 3,
            DeviceKind::Mouse => 2,
            DeviceKind::Keyboard => 1,
            DeviceKind::Other => 0,
        }
    }
}

/// 本机的一台输入设备（设置页「设备是」下拉的数据源）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    /// 平台给的设备标识（Windows 设备接口路径 / Linux evdev 名字）。
    pub name: String,
    pub kind: DeviceKind,
}

/// 把设备标识压成**稳定的匹配模式**（「设备是」下拉选中后存进配置的值）。
///
/// Windows 的设备接口路径形如
/// `\\?\HID#VID_046D&PID_C52B&MI_00#7&b3c8a2e&0&0000#{884b96c3-…}`：前半段由硬件决定、
/// 稳定，后半段是枚举实例号——换个 USB 口、重新配一次接收器槽位就会变。所以只留
/// `VID_046D&PID_C52B` 当模式，别把一整条会过期的长串存进配置。ACPI 内置键盘
/// （`\\?\ACPI#MSI0007#4&…`）同理取 `ACPI#MSI0007`；Linux 的 evdev 名字（如
/// `AT Translated Set 2 keyboard`）本来就是稳定标识，原样返回；其余形态去掉尾部的
/// `#{接口类 GUID}` 兜底。
///
/// 返回值不含通配符，直接就是 [`match_name`] 认的子串模式。
pub fn device_pattern(device: &str) -> String {
    let trimmed = device.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 设备路径都是 ASCII；出现非 ASCII（多字节）就走兜底，别按字节切片切坏字符。
    if trimmed.is_ascii() {
        let lower = trimmed.to_ascii_lowercase();
        if let (Some(v), Some(p)) = (lower.find("vid_"), lower.find("pid_")) {
            if p > v {
                let seg = |i: usize| trimmed[i..].split(['&', '#']).next().unwrap_or("");
                let (vid, pid) = (seg(v), seg(p));
                if !vid.is_empty() && !pid.is_empty() {
                    return format!("{vid}&{pid}");
                }
            }
        }
        if let Some(a) = lower.find("acpi#") {
            if let Some(hid) = trimmed[a..].split('#').nth(1) {
                if !hid.is_empty() {
                    return format!("ACPI#{hid}");
                }
            }
        }
    }
    // 没有 VID/PID / ACPI 可压（一些 OEM 触摸板只有硬件 ID，如 `\?\HID#CUST0001&Col01#…`）：
    // 退到**稳定前缀** `\?\HID#CUST0001`——实例号换口 / 重新枚举都会变，`&ColNN` 也只影响
    // 同一块硬件的集合，都不该进配置。
    device_stable_prefix(trimmed)
}

/// 去掉第二段末尾的 `&ColNN`（HID 顶层集合编号，同一块硬件的不同集合各占一个）。
fn strip_col(segment: &str) -> &str {
    match segment.rfind("&Col") {
        Some(i)
            if segment.len() > i + 4 && segment[i + 4..].bytes().all(|b| b.is_ascii_digit()) =>
        {
            &segment[..i]
        }
        _ => segment,
    }
}

/// 设备标识的**稳定前缀**：`\\?\HID#VID_04F3&PID_31B9&MI_00`（`\?\HID#CUST0001` 这类只有
/// 硬件 ID 的路径也一样）。去掉 `#{接口类 GUID}`、`#实例号` 与 `&ColNN`。
fn device_stable_prefix(device: &str) -> String {
    let base = device.split("#{").next().unwrap_or(device);
    let mut parts = base.split('#');
    let first = parts.next().unwrap_or("");
    let second = parts.next().unwrap_or("");
    let second = strip_col(second);
    if second.is_empty() {
        base.to_string()
    } else {
        format!("{first}#{second}")
    }
}

/// 设备标识 → **硬件身份**：同一块硬件暴露的多个集合（触摸板的触摸口 + 鼠标口、数位板的
/// 笔口 + 鼠标口）落到同一个身份，别的设备不会。
///
/// 与 [`device_stable_prefix`] 不同的是它去掉了「集合」这一层但**保留父设备实例**
/// （`#5&135c4e80&0&0000` / `#5&135c4e80&0&0001` → `#5&135c4e80`）：实例是这次枚举里
/// 这台父设备的身份——同一块硬件的多个集合共用，两台同型号设备不共用（`7&b3c8a2e` vs
/// `7&22cd7391`）。设置页靠它判「这两条枚举是不是同一块硬件」，判错了会出两种事故：把
/// 接收器下的键盘当成鼠标的另一个口删掉，或把两台同型号设备并成一台。
///
/// Linux 的 evdev 名字没有 `#`，原样返回（同名就当同一台——本来也分辨不出来）。
pub fn device_hardware_key(device: &str) -> String {
    let base = device.split("#{").next().unwrap_or(device);
    let mut parts = base.splitn(3, '#');
    let first = parts.next().unwrap_or("");
    let second = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let second = strip_col(second);
    let rest = strip_collection_index(rest);
    if second.is_empty() {
        base.to_string()
    } else if rest.is_empty() {
        format!("{first}#{second}")
    } else {
        format!("{first}#{second}#{rest}")
    }
}

/// 实例段去掉末尾的 `&0&NNNN`（HID 集合在父设备实例下的序号）：`5&135c4e80&0&0000`
/// → `5&135c4e80`。父设备实例（`5&135c4e80`）保留——那才是「哪一台设备」。
fn strip_collection_index(instance: &str) -> &str {
    let mut it = instance.rsplitn(3, '&');
    let last = it.next().unwrap_or("");
    let second = it.next().unwrap_or("");
    let rest = it.next();
    if second == "0" && !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        rest.unwrap_or("")
    } else {
        instance
    }
}

/// 前台进程名匹配：不区分大小写；模式含 `*`/`?` 时按通配匹配，否则按子串匹配。
///
/// **整条路径也认**：用户常从任务管理器 / 快捷方式里把 `C:\...\chrome.exe` 整条粘进
/// 排除列表（有时还带外层引号），而 `name` 只有文件名——比较前先剥掉引号与目录部分。
/// 空模式一律不匹配（`contains("")` 恒真，不能让它排除一切）。
///
/// 不按 `automation` 门控：手势排除列表（[`gesture::app_excluded`]）在基础版里也用它。
pub(crate) fn match_name(pattern: &str, name: &str) -> bool {
    let trimmed = pattern
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim();
    if trimmed.is_empty() {
        return false;
    }
    let p = trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .to_lowercase();
    if p.is_empty() {
        return false;
    }
    let n = name.to_lowercase();
    if p.contains('*') || p.contains('?') {
        wildcard_match(&p, &n)
    } else {
        n.contains(&p)
    }
}

/// 通配匹配：`*` 匹配任意序列、`?` 匹配单个字符。`pattern`/`text` 均已转小写。
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

/// 应用操作：启动、关闭、查询运行状态、重启、窗口控制。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AppOperation {
    /// 打开（启动）程序，可选参数。
    Launch { program: String, args: Vec<String> },
    /// 关闭程序（结束所有同名进程）。
    Close { program: String },
    /// 把程序的主窗口最大化（已最小化则先还原）。
    Maximize { program: String },
    /// 把程序的主窗口最小化。
    Minimize { program: String },
    /// 把程序的主窗口放置到前台（已最小化则先还原；**不会启动程序**）。
    Activate { program: String },
    /// 查询程序是否在运行，结果写入变量 `var`（布尔值，供后续条件判断用）。
    /// `retries`：未运行时的重试次数（0 = 只查一次）；`interval_ms`：每次重试间隔。
    Status {
        program: String,
        #[serde(default = "default_var")]
        var: String,
        #[serde(default)]
        retries: u32,
        #[serde(default = "default_status_interval_ms")]
        interval_ms: u64,
    },
    /// 重启程序：先关闭再重新启动。
    Restart {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// 当前窗口（原 `Action::Window`，入口已并入应用动作，EXT-14）：对**触发那一刻的前台
    /// 窗口**执行操作，不按程序名匹配、也不激活窗口（不抢焦点）。只有 Windows 有前台窗口
    /// 句柄，其它平台执行报错并由冲突检测标平台缺失。
    CurrentWindow {
        #[serde(default)]
        action: WindowOperation,
    },
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

/// 鼠标按键（[`Action::Mouse`] 的点击目标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// 左键。
    #[default]
    Left,
    /// 右键。
    Right,
    /// 中键（滚轮按下）。
    Middle,
    /// 后退键（侧键 MB4）。
    Back,
    /// 前进键（侧键 MB5）。
    Forward,
}

/// 鼠标模拟操作（[`Action::Mouse`]，规划 7.1-㊱ / EXT-16）。
///
/// `Move` / `Scroll` 的坐标与滚轮量是**相对**值；`MoveTo` 是**绝对**屏幕坐标
/// （主显示器左上角 = `(0,0)`，多屏可为负；Windows 为物理像素、macOS 为点）。
/// 移动 / 滚动不改按键状态；点击作用在**当前**光标位置，故常见用法是先 `Move` 再 `Click`。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MouseOp {
    /// 光标相对当前位置移动 `(dx, dy)` 像素（屏幕坐标系：右下为正）。
    Move { dx: i32, dy: i32 },
    /// 光标移动到绝对屏幕坐标 `(x, y)`（主显示器左上角为原点，多屏可为负）。
    /// 目前 Windows / macOS 实现；Linux 执行时明确报错（平台软冲突）。
    MoveTo { x: i32, y: i32 },
    /// 光标移动到**本次手势的起点**（右键按下点）。只对鼠标手势触发有意义；
    /// 快捷键 / 录制等路径没有起点，执行时报错。
    MoveToOrigin,
    /// 在当前光标位置点击一下（按下 + 抬起）。
    Click {
        #[serde(default)]
        button: MouseButton,
    },
    /// 滚轮滚动：`dy>0` 上滚 / `dy<0` 下滚、`dx` 为横向滚轮（单位「格」）。
    Scroll { dx: i32, dy: i32 },
}

/// 「后台发送」动作的内容（EXT-16 二阶段）：发给非前台窗口的按键组合或文本。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SendPayload {
    /// 按键组合（按顺序按下、逆序抬起）。
    Keys {
        #[serde(default)]
        keys: Vec<String>,
    },
    /// 原样文本（支持 `{变量}` 占位符，执行时替换）。
    Text {
        #[serde(default)]
        text: String,
    },
}

/// 「按键组合」动作的注入方式（EXT-16）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeysMode {
    /// 按下 → 稍停 → 逆序抬起（默认；老配置缺这个字段即此）。
    #[default]
    Tap,
    /// 只按下并保持，链结束时自动抬起（也可后面用 `Release` 显式抬起）。
    Press,
    /// 抬起（逆序）；用于配合前面的 `Press` 做「按住 → 做别的 → 松开」。
    Release,
}

/// 剪贴板动作（[`Action::Clipboard`]，EXT-15）：v1 只处理文本（图片剪贴板列在规划后续）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClipboardOperation {
    /// 把文本写入系统剪贴板（支持 `{变量}` 占位符；空文本 = 清空剪贴板）。
    SetClipboard {
        #[serde(default)]
        text: String,
    },
    /// 读取剪贴板文本到变量 `var`；剪贴板里没有文本时执行失败。
    GetClipboard {
        #[serde(default)]
        var: String,
    },
    /// 临时文本：写入剪贴板 → 粘贴到当前焦点 → 恢复原剪贴板（只恢复文本内容）。
    #[serde(rename = "tempstr")]
    TempStr {
        #[serde(default)]
        text: String,
    },
}

/// 算法动作的子操作（[`Action::Transform`]，EXT-18）：纯文本变换，不依赖平台。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformOperation {
    /// base64 编码（标准字母表带 `=` 填充，输入按 UTF-8 字节）。
    #[default]
    Base64Encode,
    /// base64 解码（忽略换行 / 空白；结果必须是 UTF-8 文本）。
    Base64Decode,
    /// URL 编码（RFC 3986：保留 `A-Za-z0-9-._~`，其余按 UTF-8 字节转 `%XX`）。
    UrlEncode,
    /// URL 解码（`%XX` 还原、`+` 还原为空格；结果必须是 UTF-8 文本）。
    UrlDecode,
    /// SHA-256 哈希，小写十六进制（输入按 UTF-8 字节）。
    HashSha256,
    /// SHA-512 哈希，小写十六进制（输入按 UTF-8 字节）。
    HashSha512,
}

/// `serde` 的 `skip_serializing_if` 帮手：`false` 的布尔字段不落盘——默认值写进 JSON 只是噪音
/// （配置、导出卡带、分享出去的卡带都会带上）。
pub(crate) fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}

/// 同上，给 `u32`（如触发角的防误触停留毫秒）。
pub(crate) fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// `serde` 帮手：旧截图动作的缺省字段不落盘（只为老配置形状稳定）。
#[cfg(feature = "automation")]
fn is_default_capture_op(v: &CaptureOperation) -> bool {
    matches!(v, CaptureOperation::FullScreen)
}

/// `serde` 帮手：旧截图动作的缺省出口不落盘。
#[cfg(feature = "automation")]
fn is_default_capture_output(v: &CaptureOutput) -> bool {
    matches!(v, CaptureOutput::Clipboard)
}

/// `serde` 帮手：亮度的默认方向 / 默认步长不落盘。
#[cfg(feature = "automation")]
fn is_default_brightness_op(v: &BrightnessOperation) -> bool {
    matches!(v, BrightnessOperation::Up)
}

#[cfg(feature = "automation")]
fn is_default_brightness_step(v: &i64) -> bool {
    *v == default_brightness_step()
}

/// `serde` 帮手：默认的 `tap` 不落盘（老配置与新配置的普通按键动作保持同一形状）。
fn is_tap_mode(m: &KeysMode) -> bool {
    matches!(m, KeysMode::Tap)
}

/// 本步失败后整条动作链怎么办（CS-13）：默认继续跑后续动作（既有语义），
/// `stop` 是显式选择——失败后只把当前这一步收尾，后面的动作不再执行。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnError {
    /// 继续执行后续动作（默认，缺字段也是它）。
    #[default]
    Continue,
    /// 本步失败后停整条链（并行组里已开跑的兄弟不取消，组结束后才停）。
    Stop,
}

/// `serde` 帮手：默认的 `continue` 不落盘。
#[cfg(feature = "automation")]
fn is_continue(v: &OnError) -> bool {
    matches!(v, OnError::Continue)
}

/// 同一条命令 / 脚本上一次还没跑完时，这次触发怎么办（CS-17）。
///
/// 以「动作指纹 + 触发名」为键在**整个进程**里登记在跑的实例：跨触发链生效——同一个脚本
/// 被快捷键 A 起了一次、还没跑完又被 A / 同一动作再触发时按这里的选择处理。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfRunning {
    /// 照常再起一份（默认，旧行为）。
    #[default]
    Run,
    /// 直接跳过：记一条「已在运行，已跳过」的步骤，不执行本步。
    Skip,
    /// 排队等上一次结束再执行；等待期间可被「停止执行」中止。
    Queue,
}

/// `serde` 帮手：默认的 `run` 不落盘。
#[cfg(feature = "automation")]
fn is_if_running_default(v: &IfRunning) -> bool {
    matches!(v, IfRunning::Run)
}

/// 等待动作的缺省轮询间隔（毫秒）。
#[cfg(feature = "automation")]
pub fn default_wait_interval_ms() -> u64 {
    200
}

/// `serde` 帮手：缺省轮询间隔不落盘。
#[cfg(feature = "automation")]
fn is_default_wait_interval(v: &u64) -> bool {
    *v == default_wait_interval_ms()
}

/// 等待动作的目标（CS-18）。
#[cfg(feature = "automation")]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WaitTarget {
    /// 等文件 / 目录出现（`path` 支持 `{变量}` 与 `~`）。
    File { path: String },
    /// 等进程启动（`program` 按镜像名匹配，忽略大小写；Windows 用 `tasklist`、Unix 用 `pgrep`）。
    Process { program: String },
    /// 等窗口出现（`title` 按子串 / 通配匹配，忽略大小写；**目前只有 Windows 实现**）。
    Window { title: String },
}

/// `serde` 帮手：默认的「从不弹窗」不落盘（老布尔 false 也按默认处理）。
#[cfg(feature = "automation")]
fn is_show_output_default(v: &ShowOutput) -> bool {
    matches!(
        v,
        ShowOutput::Bool(false) | ShowOutput::Mode(ShowOutputMode::Never)
    )
}

/// 「弹窗显示结果」的三种口径（CS-11）：老配置的 `true` / `false` 照旧读，
/// 新口径序列化成 `"always"` / `"on_error"` / `"never"`。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowOutputMode {
    /// 每次都弹。
    Always,
    /// 只在失败时弹（成功只进消息中心）。
    OnError,
    /// 从不弹，只亮未读红点（默认）。
    Never,
}

/// `show_output` 的兼容形态：老配置是布尔，新配置可写三态字符串。
#[cfg(feature = "automation")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ShowOutput {
    Bool(bool),
    Mode(ShowOutputMode),
}

#[cfg(feature = "automation")]
impl Default for ShowOutput {
    fn default() -> Self {
        Self::Bool(false)
    }
}

#[cfg(feature = "automation")]
impl From<bool> for ShowOutput {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

#[cfg(feature = "automation")]
impl ShowOutput {
    /// 这次结果该不该弹窗：`failed` 是本次执行的真实成败（命令看退出码表）。
    pub fn shows(self, failed: bool) -> bool {
        match self {
            Self::Bool(v) => v,
            Self::Mode(ShowOutputMode::Always) => true,
            Self::Mode(ShowOutputMode::OnError) => failed,
            Self::Mode(ShowOutputMode::Never) => false,
        }
    }

    /// 老布尔口径：给「只知道开关、不知道本次成败」的地方用（如动作失败记录）。
    pub fn is_on(self) -> bool {
        !matches!(self, Self::Bool(false) | Self::Mode(ShowOutputMode::Never))
    }
}

/// 内置脚本的源码上限（UTF-8 字节）：超长多半是误粘贴，执行前直接拒绝。
pub const INLINE_SCRIPT_MAX_BYTES: usize = 64 * 1024;

/// 内置脚本的操作数上限（Rhai 指令数）：死循环只能跑到这里，不会永久占住执行线程。
pub const INLINE_SCRIPT_MAX_OPS: u64 = 200_000;

/// 触发后执行的单个动作。一个快捷键可挂多个动作，按顺序执行。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// 文本动作：原样输入文本，或把当前选中/剪贴板文本转大小写后粘贴。
    ///
    /// `masked` 只影响**界面展示**（编辑器里这段文本以 `***` 代替，动作摘要与执行记录同款掩掉），
    /// 不改实际注入的内容——给密码 / 口令这类不该被人一眼看到的文本用。
    Text {
        #[serde(default)]
        text: String,
        #[serde(default)]
        mode: TextMode,
        #[serde(default, skip_serializing_if = "is_false")]
        masked: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 执行命令（CMD 或 PowerShell），结果进消息中心；`var` 非空时把标准输出（去首尾空白）写入该变量。
    ///
    /// `working_dir` 是命令运行时的工作目录（空 = 继承 Kada 进程 cwd，支持 `~` 与 `{变量}`、
    /// 相对路径）；`timeout_ms` 为本步超时（缺省跟全局设置，`0` = 不限时）；`ok_exit_codes`
    /// 为「算成功」的退出码表（空 = 只有 0，`robocopy` 的 1、安装器的 3010 靠它放行）。
    ///
    /// `hide_window` = 执行时不弹出子进程的控制台窗口（Windows 专有：GUI 进程 spawn
    /// `cmd`/`powershell` 会新建一个黑窗，按一次快捷键就闪一下）。默认 `false` 保持原样——
    /// 交互式命令（要在黑窗里敲字、看进度的）靠它才能用；纯后台命令打开它更清净。
    ///
    /// `wait = false`（CS-12）= 启动后不管：不捕获输出、不可被「停止执行」收回；
    /// `on_error = stop`（CS-13）= 本步失败后停整条链；`show_output` 为三态（CS-11）。
    #[cfg(feature = "automation")]
    Command {
        shell: Shell,
        command: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        working_dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ok_exit_codes: Vec<i32>,
        #[serde(default, skip_serializing_if = "is_show_output_default")]
        show_output: ShowOutput,
        #[serde(default, skip_serializing_if = "is_false")]
        hide_window: bool,
        #[serde(default)]
        var: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        wait: bool,
        #[serde(default, skip_serializing_if = "is_continue")]
        on_error: OnError,
        #[serde(default, skip_serializing_if = "is_if_running_default")]
        if_running: IfRunning,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 执行 CMD 命令（旧版动作，加载时自动迁移为 `Command(Cmd)`）。
    #[cfg(feature = "automation")]
    Cmd {
        command: String,
        #[serde(default)]
        show_output: bool,
        #[serde(default)]
        var: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 执行 PowerShell 命令（旧版动作，加载时自动迁移为 `Command(Powershell)`）。
    #[cfg(feature = "automation")]
    Powershell {
        command: String,
        #[serde(default)]
        show_output: bool,
        #[serde(default)]
        var: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 启动可执行文件，可选参数。（旧版动作，加载时自动迁移为 `App::Launch`。）
    #[cfg(feature = "automation")]
    Launch {
        program: String,
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 在文件管理器中打开某个目录。（旧版动作，加载时自动迁移为 `Os::OpenFolder`。）
    #[cfg(feature = "automation")]
    OpenFolder {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 按键动作：`tap` 按下再松开（默认）；`press` 只按下并保持到链结束 / 显式抬起；
    /// `release` 抬起（逆序）。按住类按键在失败 / 中止 / 输入复位时也会自动抬起。
    Keys {
        keys: Vec<String>,
        #[serde(default, skip_serializing_if = "is_tap_mode")]
        mode: KeysMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 向**非前台**窗口发送按键 / 文本（原独立动作类型；**加载时自动迁移为
    /// `System::SendTo`**，界面入口已收进「操作系统」）。语义见 [`SystemOperation::SendTo`]。
    SendTo {
        target: String,
        payload: SendPayload,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 暂停 ms 毫秒。
    PauseMs {
        ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 鼠标模拟：移动 / 点击 / 滚轮（全键盘操作，规划 7.1-㊱）。
    Mouse {
        op: MouseOp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 剪贴板动作（EXT-15）：写入 / 读取文本到变量 / 临时文本（粘贴后恢复原剪贴板）。
    Clipboard {
        operation: ClipboardOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 算法动作（EXT-18）：base64 / URL 编解码、SHA-256 / SHA-512 哈希。
    /// `input` 支持 `{变量}` 占位符；结果写进变量 `var`，`inject` 为真时再按文本注入当前焦点。
    Transform {
        operation: TransformOperation,
        #[serde(default)]
        input: String,
        #[serde(default)]
        var: String,
        #[serde(default, skip_serializing_if = "is_false")]
        inject: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 设置变量：弹一个输入框，把用户填的内容写进变量 `var`（文本动作的变量来源之一）。
    /// `prompt` / `default` 支持 `{变量}` 占位符；取消或留空 = 本次执行报错中止。
    /// `popup` 默认开；关掉 = 不弹框、直接把 `default`（代入变量后）写进变量，
    /// 此时 `prompt` 不展示也不校验（`default` 为空 = 写一个空文本变量，是合法值）。
    #[cfg(feature = "automation")]
    Ask {
        #[serde(default)]
        var: String,
        #[serde(default)]
        prompt: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        default: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        popup: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 动作分组：**纯粹的组织结构**，成员按顺序执行（等价于把它们直接摊在这一层）。
    /// 只影响编辑页的展示与执行记录的分层（成员记录挂在分组这一步下面），不改执行语义；
    /// 可嵌套（分组里还能放分组 / 条件判断 / 并行执行）。
    Group {
        /// 分组名称（纯展示，可空）。
        #[serde(default)]
        name: String,
        #[serde(default)]
        actions: Vec<Action>,
    },
    /// 禁用标记（编辑页每条动作右侧的「禁用」开关）：内容原样保留，执行时整条跳过、
    /// 不占一条执行记录。关掉开关即还原成被包住的那条动作。
    Disabled { action: Box<Action> },
    /// 强制结束目标程序的所有进程（Windows `taskkill /F /T`，Linux `pkill -f`）。
    /// （旧版动作，加载时自动迁移为 `App::Close`。）
    #[cfg(feature = "automation")]
    CloseProgram {
        program: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 文件/目录动作（复制/剪切/粘贴/删除/新建/压缩/取属性）。
    #[cfg(feature = "automation")]
    Os {
        operation: OsOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 操作系统动作（切换虚拟桌面）。与 [`Action::Os`]（文件/目录）是两类，见 [`SystemOperation`]。
    #[cfg(feature = "automation")]
    System {
        operation: SystemOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 应用动作（打开/关闭/查询状态/重启）。
    #[cfg(feature = "automation")]
    App {
        operation: AppOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 用系统默认浏览器打开网址。
    #[cfg(feature = "automation")]
    OpenUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
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
    ///
    /// `interpreter_args` 在脚本路径之前、`args` 在脚本路径之后（均为 argv，逐项做 `{变量}` 替换）；
    /// `working_dir` 是脚本运行时的工作目录（空 = 继承 Kada 进程 cwd，支持 `~` 与相对路径）；
    /// `timeout_ms` 为本步超时（缺省跟全局设置，`0` = 不限时）；`ok_exit_codes` 为「算成功」的
    /// 退出码表（空 = 只有 0）。
    ///
    /// `hide_window` 与 [`Action::Command`] 同义：执行时不弹出子进程的控制台窗口。
    /// `wait` / `on_error` / `show_output` 也与 [`Action::Command`] 同义（CS-11~CS-13）。
    #[cfg(feature = "automation")]
    Script {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interpreter: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        interpreter_args: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        working_dir: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        ok_exit_codes: Vec<i32>,
        #[serde(default, skip_serializing_if = "is_show_output_default")]
        show_output: ShowOutput,
        #[serde(default, skip_serializing_if = "is_false")]
        hide_window: bool,
        #[serde(default)]
        var: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        wait: bool,
        #[serde(default, skip_serializing_if = "is_continue")]
        on_error: OnError,
        #[serde(default, skip_serializing_if = "is_if_running_default")]
        if_running: IfRunning,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 内置脚本（规划 7.2-㉟）：用内嵌的 Rhai 引擎执行一小段脚本，**沙箱内运行**——
    /// 不落盘、不联网、不启动进程，只能读写动作链变量并产出文本。
    ///
    /// 脚本可用函数（见 `kada-actions` 的 `script` 模块）：
    /// `get_var(名)` / `has_var(名)` / `set_var(名, 值)` / `trigger()` / `shortcut_name()` /
    /// `print(值)`。脚本的返回值（非空时）与 `print` 的输出进入本步结果；
    /// 填了 `var` 则把结果存进该变量供后续动作用 `{变量名}` 引用。
    ///
    /// 执行有界：最多 [`INLINE_SCRIPT_MAX_OPS`] 次操作、源码不超过 [`INLINE_SCRIPT_MAX_BYTES`]
    /// 字节，并沿用动作链超时（`settings.action_timeout_ms`）与「停止执行」。
    #[cfg(feature = "automation")]
    InlineScript {
        #[serde(default)]
        source: String,
        #[serde(default)]
        var: String,
        #[serde(default, skip_serializing_if = "is_false")]
        show_output: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 并行执行：成员各起一个线程同时开跑，**本步等全部成员结束后才进下一步**（汇合屏障，
    /// 规划 7.2-㉞）。注入类成员（文本 / 按键 / 鼠标）之间仍由链内注入锁串行，只有命令 /
    /// 脚本 / 文件 / 延迟这类非注入步骤才真正并发；成员写变量按数组顺序合并（后者胜）。
    #[cfg(feature = "automation")]
    Parallel {
        #[serde(default)]
        actions: Vec<Action>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 等待某个条件成立（CS-18）：文件 / 目录出现、进程启动、窗口出现。
    ///
    /// 轮询间隔 `interval_ms`（缺省 200，最小 50）；`timeout_ms` 缺省跟全局「动作超时」，
    /// `0` = 一直等（仍可被「停止执行」中止）。超时 = 本步失败（按链的默认语义继续后续动作）。
    /// `var` 非空时把命中的内容写进变量（文件 = 实际路径、进程 = 程序名、窗口 = 窗口标题）。
    #[cfg(feature = "automation")]
    WaitFor {
        target: WaitTarget,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        #[serde(
            default = "default_wait_interval_ms",
            skip_serializing_if = "is_default_wait_interval"
        )]
        interval_ms: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        var: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 当前窗口动作（EXT-14；**加载时自动迁移为 `App::CurrentWindow`**，界面入口已收进
    /// 「应用操作」）：对触发那一刻的前台窗口最大化 / 还原、最小化、置顶切换、
    /// 居中、关闭、关闭同类窗口。目标窗口在触发时抓取（手势路径同理），**不激活窗口**。
    /// 目前只有 Windows 实现，其它平台执行时报「不支持」（冲突检测同时标软冲突）。
    #[cfg(feature = "automation")]
    Window {
        operation: WindowOperation,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 旧「截图」动作：**功能已移除**，只为读得动老配置保留；执行会明确报错，
    /// 界面也不再提供新建入口（截图改为鼠标手势：右键画圈 + 左键确认）。
    #[cfg(feature = "automation")]
    Capture {
        #[serde(default, skip_serializing_if = "is_default_capture_op")]
        operation: CaptureOperation,
        #[serde(default, skip_serializing_if = "is_default_capture_output")]
        output: CaptureOutput,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 外部工具能力（占位）：`tool` / `capability` 对应 [`EXTERNAL_TOOLS`] 的 id。
    /// 目前只做「探测到工具在运行 → 动作里出现」，执行会明确报「尚未接入」。
    #[cfg(feature = "automation")]
    Tool {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tool: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        capability: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    /// 显示器亮度（EXT-22；**加载时自动迁移为 `System::Brightness`**，界面入口已收进
    /// 「操作系统」）：主显示器调高 / 调低 / 恢复原始值。Windows 走 DDC/CI，
    /// 其它平台执行报错并由冲突检测标平台缺失。
    #[cfg(feature = "automation")]
    Brightness {
        #[serde(default, skip_serializing_if = "is_default_brightness_op")]
        operation: BrightnessOperation,
        #[serde(
            default = "default_brightness_step",
            skip_serializing_if = "is_default_brightness_step"
        )]
        step: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
}

impl Action {
    /// 校验动作可执行：键名可解析、必要字段非空。非法返回中文错误。
    pub fn validate(&self) -> Result<(), String> {
        let check_key = |label: &str, k: &str| {
            k.parse::<Key>()
                .map_err(|e| format!("{label}「{k}」无效：{e}"))?;
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
            // 禁用的动作照样校验：打开开关后它要原样跑起来，别把非法配置藏到开关后面。
            Action::Disabled { action } => action.validate(),
            Action::Clipboard { operation, .. } => match operation {
                // 空文本 = 清空剪贴板，是合法操作。
                ClipboardOperation::SetClipboard { .. } => Ok(()),
                ClipboardOperation::GetClipboard { var } => non_empty("变量名", var),
                ClipboardOperation::TempStr { text } => non_empty("文本", text),
            },
            Action::Transform {
                input, var, inject, ..
            } => {
                non_empty("输入", input)?;
                if !*inject && var.trim().is_empty() {
                    return Err("算法动作要填结果变量名，或勾选「直接注入结果」".into());
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Ask {
                var, prompt, popup, ..
            } => {
                non_empty("变量名", var)?;
                // 不弹框时提示语没地方显示，空着不算错。
                if *popup {
                    non_empty("提示语", prompt)?;
                }
                Ok(())
            }
            Action::Mouse { op, .. } => match op {
                MouseOp::Move { dx, dy } | MouseOp::Scroll { dx, dy } => {
                    if *dx == 0 && *dy == 0 {
                        return Err("鼠标移动 / 滚轮的偏移不能同时为 0".into());
                    }
                    Ok(())
                }
                MouseOp::MoveTo { .. } | MouseOp::MoveToOrigin | MouseOp::Click { .. } => Ok(()),
            },
            Action::Group { actions, .. } => {
                for a in actions {
                    a.validate()?;
                }
                Ok(())
            }
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
            Action::OpenFolder { path, .. } => non_empty("文件夹", path),
            Action::Keys { keys, .. } => {
                if keys.is_empty() {
                    return Err("按键组合不能为空".into());
                }
                for k in keys {
                    check_key("组合键", k)?;
                }
                Ok(())
            }
            Action::SendTo {
                target, payload, ..
            } => {
                non_empty("目标窗口", target)?;
                match payload {
                    SendPayload::Keys { keys } => {
                        if keys.is_empty() {
                            return Err("后台发送的按键组合不能为空".into());
                        }
                        for k in keys {
                            check_key("后台发送", k)?;
                        }
                    }
                    SendPayload::Text { .. } => {}
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
                    Delete { path }
                    | NewFile { path }
                    | NewFolder { path }
                    | OpenFolder { path }
                    | OpenFile { path } => non_empty("路径", path),
                    Reveal { path } => non_empty("路径", path),
                    Zip { source, dest } | Unzip { source, dest } => {
                        non_empty("源路径", source)?;
                        non_empty("目标路径", dest)?;
                        Ok(())
                    }
                    // 变量名允许为空（执行时回退到 "file"），只要求路径非空。
                    GetFileProps { path, .. } => non_empty("路径", path),
                }
            }
            // 系统操作里除「后台发送」外没有必填字段；平台不支持在执行时报错。
            #[cfg(feature = "automation")]
            Action::System { operation, .. } => {
                use SystemOperation::*;
                match operation {
                    SendTo { target, payload } => {
                        non_empty("目标窗口", target)?;
                        match payload {
                            SendPayload::Keys { keys } => {
                                if keys.is_empty() {
                                    return Err("后台发送的按键组合不能为空".into());
                                }
                                for k in keys {
                                    check_key("后台发送", k)?;
                                }
                            }
                            SendPayload::Text { .. } => {}
                        }
                        Ok(())
                    }
                    Brightness { operation, step } => {
                        if !matches!(operation, BrightnessOperation::Restore)
                            && !(1..=100).contains(step)
                        {
                            return Err("亮度步长要在 1~100 之间".into());
                        }
                        Ok(())
                    }
                    SwitchDesktop { .. } | EmptyRecycleBin | MonitorOff | ToggleDesktopIcons => {
                        Ok(())
                    }
                }
            }
            #[cfg(feature = "automation")]
            Action::App { operation, .. } => {
                use AppOperation::*;
                match operation {
                    Launch { program, .. }
                    | Close { program }
                    | Maximize { program }
                    | Minimize { program }
                    | Activate { program }
                    | Restart { program, .. } => non_empty("程序", program),
                    // 变量名允许为空（执行时回退到 "app"），只要求程序非空。
                    Status { program, .. } => non_empty("程序", program),
                    // 当前窗口没有必填字段（操作有缺省值），平台支持与否由冲突检测标出。
                    CurrentWindow { .. } => Ok(()),
                }
            }
            #[cfg(feature = "automation")]
            Action::OpenUrl { url, .. } => non_empty("网址", url),
            #[cfg(feature = "automation")]
            Action::If {
                condition,
                then,
                otherwise,
                ..
            } => {
                condition.validate()?;
                for a in then.iter().chain(otherwise.iter()) {
                    a.validate()?;
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Script { path, .. } => non_empty("脚本路径", path),
            #[cfg(feature = "automation")]
            Action::InlineScript { source, .. } => {
                non_empty("脚本内容", source)?;
                if source.len() > INLINE_SCRIPT_MAX_BYTES {
                    return Err(format!(
                        "脚本内容超过上限（{} KB）",
                        INLINE_SCRIPT_MAX_BYTES / 1024
                    ));
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Parallel { actions, .. } => {
                for a in actions {
                    a.validate()?;
                }
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::WaitFor { target, var, .. } => {
                match target {
                    WaitTarget::File { path } => non_empty("路径", path)?,
                    WaitTarget::Process { program } => non_empty("进程名", program)?,
                    WaitTarget::Window { title } => non_empty("窗口标题", title)?,
                }
                // 变量名可空（不写变量），只校验目标非空。
                let _ = var;
                Ok(())
            }
            // 窗口动作没有必填字段（操作有缺省值），恒可执行；平台支持与否由冲突检测标出。
            #[cfg(feature = "automation")]
            Action::Window { .. } => Ok(()),
            #[cfg(feature = "automation")]
            Action::Capture { .. } => Ok(()),
            #[cfg(feature = "automation")]
            Action::Tool {
                tool, capability, ..
            } => {
                non_empty("外部工具", tool)?;
                non_empty("工具能力", capability)?;
                Ok(())
            }
            #[cfg(feature = "automation")]
            Action::Brightness {
                operation, step, ..
            } => {
                if !matches!(operation, BrightnessOperation::Restore) && !(1..=100).contains(step) {
                    return Err("亮度步长要在 1~100 之间".into());
                }
                Ok(())
            }
        }
    }
}

impl Default for Action {
    fn default() -> Self {
        Action::Text {
            text: String::new(),
            mode: TextMode::Input,
            masked: false,
            description: None,
        }
    }
}
