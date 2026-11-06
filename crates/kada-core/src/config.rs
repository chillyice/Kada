// 配置模型（C-03 从 lib.rs 搬出）：目录 / 层 / 条目 / 改键 / 文本扩展 / 浮窗与角动作设置 / Config。
// 配置是跨平台同步介质，直接以 JSON 落盘/交换；快捷键以 "Ctrl+Alt+K" 文本存放，加载时解析校验。

use serde::{Deserialize, Serialize};

use crate::gesture::{Gesture, TouchpadGesture};
use crate::keys::Key;
use crate::snippet::DEFAULT_SNIPPET_INDEX_URL;
use crate::trigger::{DEFAULT_CHORD_TIMEOUT_MS, DEFAULT_SEQUENCE_TIMEOUT_MS};
#[cfg(feature = "automation")]
use crate::Condition;
use crate::{key_name, Action, TriggerContext, WhenKey};

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
    /// 层标记（纯展示，后端匹配不用）：一个调色板色值（如 `#4f9cf9`）或一个 emoji 字符。
    /// 二者取一（前端保证互斥），用于列表条目左下角与层列表的视觉区分。缺省 = 无标记。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
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
    /// 触发侧条件门控（`when`，规划 7.2-㊶）：仅当条件成立时这条才参与匹配，否则跳过
    /// （落到下一条同触发键的条目，没有就放行原键）。`None` = 无条件。
    /// 触发前求值（此刻没有变量，`Condition::Equals` 之类永假）。
    #[cfg(feature = "automation")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    /// 如 ["Ctrl+Alt+K"]，可多个。
    #[serde(default)]
    pub triggers: Vec<String>,
    /// 触发后按顺序执行的多个动作。
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

impl ShortcutItem {
    /// 触发侧 `when` 是否满足（无 `when` / 基础版恒真）。
    pub fn when_matches(&self, ctx: &TriggerContext) -> bool {
        #[cfg(feature = "automation")]
        {
            ctx.when_ok(self.when.as_ref())
        }
        #[cfg(not(feature = "automation"))]
        {
            let _ = ctx;
            true
        }
    }

    /// `when` 的判重指纹（见 [`WhenKey`]）。
    pub(crate) fn when_key(&self) -> WhenKey {
        #[cfg(feature = "automation")]
        {
            self.when.clone()
        }
        #[cfg(not(feature = "automation"))]
        {
            None
        }
    }
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
    /// 条目标题（编辑页标题行编辑；空 = 列表标题退回形态摘要，原键单列一格）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 简短描述（详情页「详情」页签）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub from: String,
    /// 普通改键目标（其余形态字段留空时使用）。
    pub to: String,
    /// 所属目录 id（`None` = 根级）。只用于列表分组展示，不参与触发判定。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 短按输出键（tap-hold；空 = 无短按行为）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tap: Option<String>,
    /// 长按输出键（tap-hold，典型为修饰键）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    /// 归属层 id（`None` = 基础层；`Some` = 仅该层激活时生效）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// 触发侧条件门控（`when`，规划 7.2-㊶）：仅当条件成立时这条改键才生效。
    /// `None` = 无条件。触发（`from` 按下）前求值。
    #[cfg(feature = "automation")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
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
        self.oneshot
            .as_deref()
            .is_some_and(|v| !v.trim().is_empty())
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

    /// 触发侧 `when` 是否满足（无 `when` / 基础版恒真）。
    pub fn when_matches(&self, ctx: &TriggerContext) -> bool {
        #[cfg(feature = "automation")]
        {
            ctx.when_ok(self.when.as_ref())
        }
        #[cfg(not(feature = "automation"))]
        {
            let _ = ctx;
            true
        }
    }

    /// `when` 的判重指纹（见 [`WhenKey`]）。
    pub(crate) fn when_key(&self) -> WhenKey {
        #[cfg(feature = "automation")]
        {
            self.when.clone()
        }
        #[cfg(not(feature = "automation"))]
        {
            None
        }
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
    /// 条目标题（编辑页标题行编辑；空 = 列表标题退回替换文本，触发词单列一格）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 简短描述（详情页「详情」页签）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 触发词（如 `:addr`、`;sig`），不含触发后缀。
    pub trigger: String,
    /// 展开文本（支持 `{date}` / `{time}` / `{clipboard}` 动态片段占位符）。
    pub replace: String,
    /// 所属目录 id（`None` = 根级）。只用于列表分组展示，不参与匹配。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
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
        self.opacity = self
            .opacity
            .clamp(Self::OPACITY_RANGE.0, Self::OPACITY_RANGE.1);
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

/// 复制快捷菜单（EXT-13）：选中文字后**双击 Ctrl+C** 在光标处弹出动作菜单。
///
/// 触发是**旁路观察**（不吞键，复制行为仍是系统 / 前台程序自己的），菜单本身只做展示；
/// 每个条目内部是一段 [`Action`]，点击后走与快捷键相同的 `fire` 链路（注入锁 / 步骤上报 /
/// 超时与中止全部继承）。缺字段 = 关闭。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardMenu {
    /// 是否启用。关掉只是不弹菜单，条目配置保留。
    #[serde(default)]
    pub enabled: bool,
    /// 两次 Ctrl+C 的最大间隔（毫秒）。
    #[serde(default = "default_double_copy_ms")]
    pub double_copy_ms: u64,
    /// 菜单条目（数组顺序即显示顺序，可嵌套子菜单）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<MenuEntry>,
}

impl ClipboardMenu {
    /// 双击间隔上下限（毫秒）。
    pub const DOUBLE_COPY_MS_RANGE: (u64, u64) = (150, 2000);
    /// 子菜单最大嵌套层数（顶层算第 1 层）。
    pub const MAX_DEPTH: usize = 3;

    /// 归一化：间隔钳到合法区间（手改 JSON 写坏也不至于让菜单永远弹不出来）。
    pub fn normalized(mut self) -> Self {
        self.double_copy_ms = self
            .double_copy_ms
            .clamp(Self::DOUBLE_COPY_MS_RANGE.0, Self::DOUBLE_COPY_MS_RANGE.1);
        self
    }
}

impl Default for ClipboardMenu {
    fn default() -> Self {
        Self {
            enabled: false,
            double_copy_ms: default_double_copy_ms(),
            items: Vec::new(),
        }
    }
}

/// 两次 Ctrl+C 最大间隔的缺省值（毫秒）。
pub fn default_double_copy_ms() -> u64 {
    500
}

/// 触发角（EXT-10）的一个角：是否启用、展示名、触发后执行的动作链。
///
/// 与鼠标手势一样复用 [`Action`] 与壳层 `fire` 链路（步骤上报 / 执行记录 / 注入锁 /
/// 超时中止全部继承）。缺字段 = 关闭。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HotCorner {
    #[serde(default)]
    pub enabled: bool,
    /// 角名（列表 / 气泡展示用）；空 = 用方位名（左上角 / 右上角 / 左下角 / 右下角）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 触发后按顺序执行的动作。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

impl HotCorner {
    /// 展示名：有名字用名字，否则退回方位名。
    pub fn label(&self, fallback: &str) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            fallback.to_string()
        } else {
            name.to_string()
        }
    }
}

/// 触发角（EXT-10）：光标碰到屏幕四角时执行动作。四角各自启用 / 命名 / 配动作，
/// 同一份四角配置应用到每台显示器的对应角。
///
/// **默认关**（缺字段即关，老配置照常读）。触发判定在钩子层（纯坐标状态机，
/// `kada-hook::hotcorner`），执行在壳层；只在 Windows 落地，其余平台走
/// [`PlatformCaps::hot_corners`] 软冲突。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HotCorners {
    /// 总开关。关掉只是不触发，四角配置保留。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub top_left: HotCorner,
    #[serde(default)]
    pub top_right: HotCorner,
    #[serde(default)]
    pub bottom_left: HotCorner,
    #[serde(default)]
    pub bottom_right: HotCorner,
}

impl HotCorners {
    /// 总开关开着、且至少一个角启用——钩子层据此决定要不要装鼠标钩子 / 判定角。
    pub fn any_enabled(&self) -> bool {
        self.enabled
            && (self.top_left.enabled
                || self.top_right.enabled
                || self.bottom_left.enabled
                || self.bottom_right.enabled)
    }
}

/// 边缘滚动（EXT-11）的一条边：是否启用、展示名、滚轮触发后执行的动作链。
///
/// 与触发角一样复用 [`Action`] 与壳层 `fire` 链路（步骤上报 / 执行记录 / 注入锁 /
/// 超时中止全部继承）。缺字段 = 关闭。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WheelEdge {
    #[serde(default)]
    pub enabled: bool,
    /// 边名（列表 / 气泡展示用）；空 = 用方位名（上边缘 / 下边缘 / 左边缘 / 右边缘）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// 触发后按顺序执行的动作。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
}

impl WheelEdge {
    /// 展示名：有名字用名字，否则退回方位名。
    pub fn label(&self, fallback: &str) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            fallback.to_string()
        } else {
            name.to_string()
        }
    }
}

/// 边缘滚动（EXT-11）：光标贴在屏幕四边时滚轮触发动作。四边各自启用 / 命名 / 配动作，
/// 同一份四边配置应用到每台显示器的对应边。
///
/// **默认关**（缺字段即关，老配置照常读）。触发判定在钩子层（纯坐标状态机，
/// `kada-hook::wheel_edge`），执行在壳层；只在 Windows 落地，其余平台走
/// [`PlatformCaps::wheel_edges`] 软冲突。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WheelEdges {
    /// 总开关。关掉只是不触发，四边配置保留。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub top: WheelEdge,
    #[serde(default)]
    pub bottom: WheelEdge,
    #[serde(default)]
    pub left: WheelEdge,
    #[serde(default)]
    pub right: WheelEdge,
}

impl WheelEdges {
    /// 总开关开着、且至少一条边启用——钩子层据此决定要不要装鼠标钩子 / 判定边缘。
    pub fn any_enabled(&self) -> bool {
        self.enabled
            && (self.top.enabled || self.bottom.enabled || self.left.enabled || self.right.enabled)
    }
}

/// 触发角的位置（新条目模型：一个角可以有多条规则，按数组顺序取第一条启用的）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotCornerSpot {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl HotCornerSpot {
    pub const ALL: [HotCornerSpot; 4] = [
        HotCornerSpot::TopLeft,
        HotCornerSpot::TopRight,
        HotCornerSpot::BottomLeft,
        HotCornerSpot::BottomRight,
    ];

    /// 方位名（没填名称时的兜底展示名）。
    pub fn label(self) -> &'static str {
        match self {
            HotCornerSpot::TopLeft => "左上角",
            HotCornerSpot::TopRight => "右上角",
            HotCornerSpot::BottomLeft => "左下角",
            HotCornerSpot::BottomRight => "右下角",
        }
    }
}

/// 防误触停留时长的上限（毫秒）：再长就等于「不触发」了，钳住免得一次误配挂死。
pub const HOT_CORNER_DWELL_MAX_MS: u32 = 3000;

/// 一条角动作规则：位置 + 名称 + 动作链。缺字段 = 关闭。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotCornerRule {
    #[serde(default)]
    pub spot: HotCornerSpot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 条目描述（只做展示，与快捷键的 `description` 同义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 归属层：`None` = 基础层（恒生效）；`Some` = 仅在该层激活时参与匹配。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// **防误触**：光标要在角上停留这么久才触发（毫秒；`0` / 缺省 = 碰即触发）。
    /// 贴边扫过（关窗、拖滚动条、切窗口）常会蹭到角落，停留一下再触发才不会误动作。
    #[serde(default, skip_serializing_if = "crate::is_zero_u32")]
    pub dwell_ms: u32,
    /// 触发侧条件门控（同快捷键）：不成立 = 这条不存在，落到下一条同位置的规则。
    #[cfg(feature = "automation")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    /// 归属文件夹（`None` = 根级）：与快捷键共用一套目录树，只影响列表组织。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 统一展示顺序：鼠标操作 / 角动作 / 边缘滚动共用一个序列，只影响列表顺序；
    /// 匹配仍是「同类型、同位置 / 同方向串」内按数组顺序取第一条。缺字段 = 0。
    #[serde(default, skip_serializing_if = "crate::is_zero_u64")]
    pub order: u64,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

impl HotCornerRule {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.spot.label().to_string())
    }

    /// 触发侧 `when` 是否满足（无 `when` / 基础版恒真）。
    pub fn when_matches(&self, ctx: &TriggerContext) -> bool {
        #[cfg(feature = "automation")]
        {
            ctx.when_ok(self.when.as_ref())
        }
        #[cfg(not(feature = "automation"))]
        {
            let _ = ctx;
            true
        }
    }

    /// 层语义同快捷键：`active_layer` 与条目归属层严格相等（`None` = 基础层）。
    pub fn layer_matches(&self, active_layer: Option<&str>) -> bool {
        self.layer.as_deref() == active_layer
    }
}

/// 边缘滚动的位置（新条目模型：一条边可以有多条规则，取第一条启用的）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelEdgeSpot {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl WheelEdgeSpot {
    pub const ALL: [WheelEdgeSpot; 4] = [
        WheelEdgeSpot::Top,
        WheelEdgeSpot::Bottom,
        WheelEdgeSpot::Left,
        WheelEdgeSpot::Right,
    ];

    /// 方位名（没填名称时的兜底展示名）。
    pub fn label(self) -> &'static str {
        match self {
            WheelEdgeSpot::Top => "上边缘",
            WheelEdgeSpot::Bottom => "下边缘",
            WheelEdgeSpot::Left => "左边缘",
            WheelEdgeSpot::Right => "右边缘",
        }
    }
}

/// 一条边缘滚动规则：位置 + 名称 + 动作链。缺字段 = 关闭。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WheelEdgeRule {
    #[serde(default)]
    pub edge: WheelEdgeSpot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 条目描述（只做展示，与快捷键的 `description` 同义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 归属层：`None` = 基础层（恒生效）；`Some` = 仅在该层激活时参与匹配。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<String>,
    /// 触发侧条件门控（同快捷键）：不成立 = 这条不存在，落到下一条同位置的规则。
    #[cfg(feature = "automation")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    /// 归属文件夹（`None` = 根级）：与快捷键共用一套目录树，只影响列表组织。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 统一展示顺序：鼠标操作 / 角动作 / 边缘滚动共用一个序列，只影响列表顺序；
    /// 匹配仍是「同类型、同位置 / 同方向串」内按数组顺序取第一条。缺字段 = 0。
    #[serde(default, skip_serializing_if = "crate::is_zero_u64")]
    pub order: u64,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

impl WheelEdgeRule {
    pub fn label(&self) -> String {
        self.name
            .clone()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.edge.label().to_string())
    }

    /// 触发侧 `when` 是否满足（无 `when` / 基础版恒真）。
    pub fn when_matches(&self, ctx: &TriggerContext) -> bool {
        #[cfg(feature = "automation")]
        {
            ctx.when_ok(self.when.as_ref())
        }
        #[cfg(not(feature = "automation"))]
        {
            let _ = ctx;
            true
        }
    }

    /// 层语义同快捷键：`active_layer` 与条目归属层严格相等（`None` = 基础层）。
    pub fn layer_matches(&self, active_layer: Option<&str>) -> bool {
        self.layer.as_deref() == active_layer
    }
}

/// 复制快捷菜单的一个条目：动作 / 子菜单 / 分割线。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuEntry {
    /// 普通条目：点击执行 `action`。`label` 是菜单上显示的文字。
    Action {
        #[serde(default)]
        label: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        enabled: bool,
        action: Action,
    },
    /// 子菜单：`items` 与顶层同构（可继续嵌套，见 [`ClipboardMenu::MAX_DEPTH`]）。
    Submenu {
        #[serde(default)]
        label: String,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        enabled: bool,
        #[serde(default)]
        items: Vec<MenuEntry>,
    },
    /// 分割线（纯展示，不可点击）。
    Separator,
}

/// `serde` 的 `skip_serializing_if` 帮手：`true` 的布尔字段不落盘（`enabled` 缺省即 true）。
pub(crate) fn is_true(v: &bool) -> bool {
    *v
}

/// 软件更新源（见 `docs/需求设计说明书.md` §5.5）。
///
/// 国内网络到 GitHub 不稳定，默认走 ihomy 服务器上的每日镜像；镜像里的安装包与清单
/// 仍是 GitHub 发布时签好的那一份（`.sig` 原样保留），客户端照旧做 Ed25519 校验。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateSource {
    /// ihomy 镜像（默认）：服务器每天从 GitHub Releases 同步最新版与安装包。
    #[default]
    Ihomy,
    /// 官方 GitHub Releases：能直连的用户可切过去（镜像还没同步到新版时也走它）。
    Github,
}

impl UpdateSource {
    /// 该更新源的 `latest.json` 地址。
    pub fn endpoint(self) -> &'static str {
        match self {
            UpdateSource::Ihomy => "https://ihomy.top/files/kada/updates/latest.json",
            UpdateSource::Github => {
                "https://github.com/chillyice/Kada/releases/latest/download/latest.json"
            }
        }
    }
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
    /// 触发提醒：每次触发（快捷键 / 改键 / 文本扩展 / 手势 / 触发角 / 边缘滚动）在右下角
    /// 弹一条 3 秒气泡，显示动作名 + 触发键。缺字段取 `true`（老行为），关掉只影响这类
    /// 「已触发」气泡，不影响错误 / 命令结果等普通消息。
    #[serde(default = "default_true")]
    pub trigger_toast: bool,
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
    /// 卡带市场的索引地址（规划 7.2-㉕）：一份能 GET 到的静态 JSON，浏览器 / 搜索 /
    /// 一键安装都靠它，别处自建索引就填自己的。
    ///
    /// 缺字段取 [`DEFAULT_SNIPPET_INDEX_URL`]（本仓库的 `snippets/index.json`）。
    /// 填空白串等价于「用内置默认」：留着旧值删干净反而会让人以为索引被关掉了。
    #[serde(default = "default_snippet_index_url")]
    pub snippet_index_url: String,
    /// 「同步存档」目录（如 OneDrive 里的一个文件夹）：非空时，每次配置落盘都会把同一份 JSON
    /// 额外写一份 `kada-config.json` 过去，换机器 / 重装后可从它恢复（规划 7.2-㉝）。
    ///
    /// 走的是**本地文件夹**，不登录任何账号——云端同步交给 OneDrive 等客户端自己做。空 / 缺字段
    /// = 关闭同步（口径同 `wake_key`：`None` 且不把字段写进 JSON）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_dir: Option<String>,
    /// 输入层环境诊断（规划 7.2-㊽）：探测有没有会「抢走按键」的厂商软件在跑，有就在
    /// 配置用到鼠标键时给一条「为什么按了没反应」的提示（见 [`diagnose_input_env`]）。
    ///
    /// 缺字段取 `true`（[`default_true`]）：它只在**探测到接管者且配置真用到鼠标键**时才出声，
    /// 没装这类软件的机器上一条都不显示；而这类提示本来就有「检测到了但没真影响我」的误报
    /// 余地，这个开关就是给那种情况留的——关掉即整条链路静默（探测也不再做）。
    #[serde(default = "default_true")]
    pub input_env_diag: bool,
    /// 输入层环境诊断里**已经确认误报、不再提示**的接管者 id（[`DIVERTERS`] 的 `id`）。
    ///
    /// 只掐提示、不改探测：被忽略的条目仍出现在设置页卡片里（旁边给「恢复提示」），
    /// 只是不再进 [`diagnose_input_env`] 的冲突列表。缺字段 = 空 = 一条都不忽略。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_env_ignored: Vec<String>,
    /// 提权窗口注入（规划 7.2-㉑a / `docs/底层输入能力规划.md` 档位 B）：前台是提权窗口时，
    /// 把注入交给一个**按需拉起的提权进程**去做（UIPI 只拦「低完整性级别往高级别」的输入，
    /// 同级之间是通的），补上「按了快捷键，提权窗口里没反应」。
    ///
    /// 缺字段取 `false`（`#[serde(default)]` 的零值）：它是**默认关**的可选能力——打开意味着
    /// 要弹一次 UAC、多一个提权进程在跑，这种事不该替用户决定；关着时行为与没有这个功能
    /// 完全一致（见 `docs/底层输入能力规划.md` §1 的「只增能力，不做依赖」）。
    #[serde(default)]
    pub elevated_inject: bool,
    /// 软件更新源（见 `docs/需求设计说明书.md` §5.5）：`ihomy`（默认，国内可直连的每日镜像）
    /// / `github`（官方 Releases）。
    ///
    /// 缺字段取 [`UpdateSource::Ihomy`]：老配置与国内用户初次安装都落在能连通的源上；
    /// 能直连 GitHub 的人在设置页切一次即可。
    #[serde(default)]
    pub update_source: UpdateSource,
    /// 自动更新（见 `docs/需求设计说明书.md` §5.5）：发现新版本后**自动下载**，并在
    /// 「长时间空闲」时自动安装（免安装版另在下次启动时安装）；不勾选则只能在设置页
    /// 手动点「下载」「安装」两步。
    ///
    /// 缺字段取 `false`：下载与安装都会打断使用（安装还会退出并重启应用），替用户决定
    /// 「什么时候动我的程序」关乎信任，必须显式勾选才做。检查版本本身不受这个开关影响
    /// （启动后 8 秒 + 每 24 小时照常检查）。
    #[serde(default)]
    pub auto_update: bool,
    /// **新功能总开关**：关掉后本轮新增的功能一律不可用（当前管住「鼠标手势与滚轮快捷键」，
    /// 规划 7.2-MG）。关闭 = 入口从界面隐藏 + 行为不生效，**配置保留**（重新打开即恢复）。
    ///
    /// 缺字段取 `false`（`#[serde(default)]` 的零值）：**新功能默认关**——这类功能多半带
    /// 「全局抢右键」这类副作用，替用户打开不合适；要用的人自己开。开着 = 放行。
    #[serde(default)]
    pub experimental: bool,
    /// 鼠标手势的**起手距离**（像素）：光标累计移动超过它才算「开始画手势」，
    /// 之前的移动不计入轨迹（避免轻微手抖就触发）。
    #[serde(default = "default_gesture_start_distance")]
    pub gesture_start_distance: i32,
    /// 鼠标手势的**超时**（毫秒）：画到一半停住超过这么久就放弃这次手势。
    #[serde(default = "default_gesture_timeout_ms")]
    pub gesture_timeout_ms: u64,
    /// 鼠标手势的方向量化**段长**（像素）：一段方向至少这么长，抖动自己被吸收掉。
    #[serde(default = "default_gesture_min_segment")]
    pub gesture_min_segment: i32,
    /// **右键圈选截图**（自带手势，不需要配置条目）：按住右键绕一圈、松手后冻结画面，
    /// 左键点击确认 → 包围盒截图写进剪贴板。缺字段取 `true`（总闸 `experimental` 仍管着）。
    /// 目前只在 Windows 实现（GDI），其它平台画圈会提示不支持。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub gesture_screenshot: bool,
    /// 鼠标手势的**排除程序**（每行一个进程名）：命中的程序里不接管右键，手势完全不生效
    /// （右键回放）。语义与 `when` 的「前台应用」一致：不区分大小写、含 `*`/`?` 走通配、
    /// 否则子串匹配。空 = 不排除。给游戏 / CAD / 远程桌面这类「右键拖拽本来是正常操作」的
    /// 程序留的（规划 7.2-MG 余项）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gesture_exclude: Vec<String>,
    /// **全局排除程序**（每行一个进程名）：命中的程序里所有 Kada 触发一律不生效
    /// （快捷键 / 改键 / 文本扩展 / 手势），既不吞键也不执行动作；手势仍按未命中口径
    /// 回放右键。语义同 `gesture_exclude` 与 `when` 的「前台应用」：不区分大小写、
    /// 含 `*`/`?` 走通配、否则子串。空 = 不排除。与 `gesture_exclude`（只排手势）独立。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_apps: Vec<String>,
    /// **全局全屏暂停**（EXT-05）：前台是全屏程序（游戏 / 视频 / F11）时所有 Kada 触发一律
    /// 不生效（快捷键 / 改键 / 文本扩展 / 手势），退出全屏自动恢复；不吞键、不执行动作。
    ///
    /// 缺字段取 `false`（`#[serde(default)]` 的零值）：全屏判定只在 Windows 真判，且部分
    /// 用户在全屏游戏里仍要用快捷键；替用户打开不合适。关着时行为与没有这个功能一致。
    /// 与手势自带的全屏让路独立——那个不看不听、默认恒生效（保住既有语义）。
    #[serde(default)]
    pub ignore_fullscreen: bool,
    /// 手势轨迹颜色（`#RRGGBB`）。
    #[serde(default = "default_gesture_trail_color")]
    pub gesture_trail_color: String,
    /// 手势轨迹线宽（像素）。
    #[serde(default = "default_gesture_trail_width")]
    pub gesture_trail_width: i32,
    /// 手势结果标签字号（像素）。
    #[serde(default = "default_gesture_label_font_size")]
    pub gesture_label_font_size: i32,
    /// 手势结果标签相对末端的偏移（像素）。
    #[serde(default = "default_gesture_label_offset")]
    pub gesture_label_offset: i32,
    /// 手势未命中时的失败色（`#RRGGBB`）。
    #[serde(default = "default_gesture_fail_color")]
    pub gesture_fail_color: String,
    /// **触摸板手势总开关**：与 `experimental`（鼠标操作）**各自独立**，同样缺字段 = 关。
    /// 关掉 = 触摸板手势入口隐藏 + 不采集（触发键照常输出，不静默吞），**配置保留**。
    #[serde(default)]
    pub touchpad: bool,
    /// 触摸板手势的**起手距离**（像素）：这一笔至少移动这么远才算数（同时也是轨迹浮层
    /// 亮出的阈值）。与鼠标手势的 `gesture_start_distance` 分开：一个是「开始画」，
    /// 一个是「画够长」，两种手感本来就不该共用一个值。
    #[serde(default = "default_gesture_start_distance")]
    pub touchpad_start_distance: i32,
    /// 触摸板手势的**排除程序**（每行一个进程名）：命中的程序里不执行触摸板手势——触发键
    /// 已经被吞掉（它是纯开关），所以这里只是「不执行」、没有右键要回放。语义同
    /// `gesture_exclude`：不区分大小写、含 `*`/`?` 走通配、否则子串。空 = 不排除。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touchpad_exclude: Vec<String>,
    /// **松键后把光标放回起笔点**（触摸板手势）。缺字段 = 开。
    ///
    /// 触摸板画一笔是「借道」屏幕：光标会跟着手指在屏幕上走，不还原等于打乱用户正在用的
    /// 鼠标位置。关掉的人是不希望「松手那一下光标突然跳回去」（比如同时要接着用指针）。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub touchpad_restore_cursor: bool,
    /// **采集期间显示轨迹浮层**（触摸板手势）。缺字段 = 开。
    ///
    /// 关掉只是不画预览 / 不弹结果浮层（未命中仍会出文字气泡报方向串），采集与识别照旧。
    /// 与鼠标手势共用同一块窗，但开关各管各的（只影响触摸板这条采集来源）。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub touchpad_overlay: bool,
    /// 复制快捷菜单（EXT-13）：双击 Ctrl+C 弹出动作菜单，见 [`ClipboardMenu`]。缺字段 = 关闭。
    #[serde(default)]
    pub clipboard_menu: ClipboardMenu,
    /// **旧版**触发角（EXT-10）四角配置：只在加载旧 JSON 时读，`migrate()` 会转成
    /// [`Config::hot_corners`] 后清空；新配置不再写这个键。
    #[serde(default, skip_serializing)]
    pub hot_corners: HotCorners,
    /// **旧版**边缘滚动（EXT-11）四边配置：迁移口径同 `hot_corners`。
    #[serde(default, skip_serializing)]
    pub wheel_edges: WheelEdges,
}

/// 鼠标手势起手距离的缺省值（像素）。
pub fn default_gesture_start_distance() -> i32 {
    22
}

/// 鼠标手势超时的缺省值（毫秒）。
pub fn default_gesture_timeout_ms() -> u64 {
    1500
}

/// 鼠标手势方向段长的缺省值（像素）。
pub fn default_gesture_min_segment() -> i32 {
    16
}

/// 手势轨迹颜色的缺省值。
pub fn default_gesture_trail_color() -> String {
    "#4c9aff".to_string()
}

/// 手势轨迹线宽的缺省值（像素）。
pub fn default_gesture_trail_width() -> i32 {
    5
}

/// 手势结果标签字号的缺省值（像素）。
pub fn default_gesture_label_font_size() -> i32 {
    14
}

/// 手势结果标签偏移的缺省值（像素）。
pub fn default_gesture_label_offset() -> i32 {
    16
}

/// 手势失败色的缺省值。
pub fn default_gesture_fail_color() -> String {
    "#ff5f56".to_string()
}

/// `snippet_index_url` 的缺省值（供 serde 与 [`Settings::default`] 共用）。
pub fn default_snippet_index_url() -> String {
    DEFAULT_SNIPPET_INDEX_URL.to_string()
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
            trigger_toast: true,
            text_inject_mode: TextInjectMode::Clipboard,
            hints: HintsWindow::default(),
            snippet_index_url: default_snippet_index_url(),
            sync_dir: None,
            input_env_diag: true,
            input_env_ignored: Vec::new(),
            elevated_inject: false,
            update_source: UpdateSource::Ihomy,
            auto_update: false,
            experimental: false,
            gesture_start_distance: default_gesture_start_distance(),
            gesture_timeout_ms: default_gesture_timeout_ms(),
            gesture_min_segment: default_gesture_min_segment(),
            gesture_screenshot: true,
            gesture_exclude: Vec::new(),
            exclude_apps: Vec::new(),
            ignore_fullscreen: false,
            gesture_trail_color: default_gesture_trail_color(),
            gesture_trail_width: default_gesture_trail_width(),
            gesture_label_font_size: default_gesture_label_font_size(),
            gesture_label_offset: default_gesture_label_offset(),
            gesture_fail_color: default_gesture_fail_color(),
            touchpad: false,
            touchpad_start_distance: default_gesture_start_distance(),
            touchpad_exclude: Vec::new(),
            touchpad_restore_cursor: true,
            touchpad_overlay: true,
            clipboard_menu: ClipboardMenu::default(),
            hot_corners: HotCorners::default(),
            wheel_edges: WheelEdges::default(),
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
    /// 鼠标手势（规划 7.2-MG）：按住右键画轨迹、松手按方向串匹配。
    #[serde(default)]
    pub gestures: Vec<Gesture>,
    /// 触摸板手势：按住一个**键盘键**、在触摸板上画一笔，松手按方向串（或形状模板）匹配。
    /// 与鼠标手势是两个分类（触发方式、吞吐口径、列表分组都不同），别混着塞进 `gestures`。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touchpad_gestures: Vec<TouchpadGesture>,
    /// 触发角（EXT-10）：光标碰到屏幕四角执行动作。同一位置可有多条，按顺序取第一条启用的。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hot_corners: Vec<HotCornerRule>,
    /// 边缘滚动（EXT-11）：光标贴屏幕四边滚动滚轮执行动作。同位置可有多条，取第一条启用的。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wheel_edges: Vec<WheelEdgeRule>,
    #[serde(default)]
    pub settings: Settings,
}
