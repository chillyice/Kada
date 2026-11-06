//! 咔哒核心：跨平台的键模型、快捷键解析与匹配。
//!
//! 零重量纯逻辑：Windows / macOS / Linux 钩子层都产出 [`RawEvent`]，
//! 由这里的匹配器统一判定触发。配置模型以 JSON 形式落盘和跨平台同步。
//!
//! 键名命名参考 platform 无关的中性名（Enter/Escape），macOS 的 ⌘
//! 由 [`Modifier::Meta`] 承载，平台层负责把 OS 键映射成 [`Key`]。

// 测试通过 `use crate::*` 取用 `BTreeSet`（子模块不继承父模块的 `use`，只能挂在根上）。
#[cfg(test)]
use std::collections::BTreeSet;

/// 配置合并（导入的「合并」方式）：只增不删、不制造新冲突、设置保留本机。
mod merge;
pub use merge::{merge_configs, MergeReport};

/// 配置卡带（分享用）：摘出被点名的条目组成可分享的子集配置（见规划 7.2-㉕）。
mod snippet;
pub use snippet::{
    parse_snippet_index, publish_payload, scan_snippet, snippet, snippet_title, IndexEntry,
    PickKind, SnippetIndex, SnippetMeta, SnippetPick, SnippetScan,
    DEFAULT_SNIPPET_COMMUNITY_INDEX_URL, DEFAULT_SNIPPET_INDEX_URL, DEFAULT_SNIPPET_PUBLISH_URL,
    SNIPPET_INDEX_VERSION, SNIPPET_MAX_BYTES, SNIPPET_MAX_DEPTH, SNIPPET_MAX_ENTRIES,
};

/// 内置示例配置：设置页「载入示例配置」一键并入（合并语义，只增不删）。
pub mod sample;

/// 鼠标手势 / 触摸板手势（规划 7.2-MG）：方向串 + 形状模板。纯逻辑，识别在壳层做。
mod gesture;
pub use gesture::{
    app_excluded, find_gesture, find_touchpad, find_touchpad_by_template, gesture_capture_enabled,
    gesture_enabled, is_touchpad_trigger, lasso_bbox, normalize_pattern, pattern_from_points,
    pattern_from_stroke, template_distance, template_from_points, touchpad_gesture_enabled,
    Gesture, TouchpadGesture, TEMPLATE_MAX_DISTANCE, TEMPLATE_POINTS,
};

/// 键模型（C-02 从 lib.rs 搬出）：修饰键 / 按键枚举 / 快捷键解析与匹配。
mod keys;
pub use keys::{
    format_shortcut, key_name, matches, modifier_key, Key, Modifier, ParseError, RawEvent, Shortcut,
};

/// 触发单元（C-02 续）：组合 / 序列 / 和弦的解析与时序，以及热串的键→字符映射。
mod trigger;
pub use trigger::{
    is_hotstring_terminator, key_to_char, ChordAdvance, ChordTracker, SeqAdvance, SequenceTracker,
    Trigger, DEFAULT_CHORD_TIMEOUT_MS, DEFAULT_SEQUENCE_TIMEOUT_MS,
};

/// 动作模型（C-03 从 lib.rs 搬出）：各类 Operation / Condition / Action 本体与默认值。
mod action;
pub use action::*;

// ---------------------------------------------------------------------------
// 配置模型：咔哒的配置是跨平台同步介质，直接以 JSON 形式落盘/交换。
// 快捷键以 "Ctrl+Alt+K" 文本存放（人类可读、可编辑），加载时解析校验。
// ---------------------------------------------------------------------------
mod config;
pub use config::*;

/// 变量模型（C-03 从 lib.rs 搬出）：文件属性 / 文本值 / 变量表与替换。
mod vars;
pub use vars::*;

/// 触发侧 `when` 门控的判重指纹：把条件本身算进去——同触发键 + 不同 `when` 是两条独立
/// 条目（正是「同快捷键按前台分流」的用法），不该报「触发键重复」。基础版没有 `Condition`，
/// 恒为 `None`（判重行为与从前一致）。
#[cfg(feature = "automation")]
type WhenKey = Option<Condition>;
#[cfg(not(feature = "automation"))]
type WhenKey = Option<()>;

/// 冲突检测（C-04 从 lib.rs 搬出）：Severity / Conflict / PlatformCaps / 平台与输入环境诊断。
mod conflicts;
pub use conflicts::*;

/// 配置 / 动作迁移（C-04 从 lib.rs 搬出）：旧版动作归一化与设置越界值钳制。
mod migrate;
pub use migrate::*;

/// 配置清洗（C-04 从 lib.rs 搬出）：保存 / 导入前剔掉不可用条目并说明忽略原因。
mod sanitize;
pub use sanitize::*;

#[cfg(test)]
mod tests;
