//! 鼠标手势（规划 7.2-MG）：按住右键画一条轨迹，松手时按方向串匹配。
//!
//! 表示法是**方向串**而不是原始点模板：8 个方向桶（`U`/`D`/`L`/`R` + 四个斜向），
//! 段长阈值抽稀 + 相邻同向合并，手抖自己被吸收掉。方案与取舍见
//! `docs/鼠标手势与滚轮快捷键规划.md`。
//!
//! 平台无关的**纯逻辑**：钩子只负责收集轨迹点，识别在壳层做（钩子回调里不许耗时）。

use crate::{Action, Config};
use serde::{Deserialize, Serialize};

/// 一条鼠标手势。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Gesture {
    /// 条目标题（列表 / 气泡展示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 方向串：`U`/`D`/`L`/`R` 与斜向 `UR`/`DR`/`DL`/`UL`（如 `DRUL`）。
    #[serde(default)]
    pub pattern: String,
    /// 触发后按顺序执行的多个动作（复用 [`Action`]，与快捷键同一套动作引擎）。
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub enabled: bool,
}

impl Gesture {
    /// 展示名：有标题用标题，否则退回方向串。
    pub fn label(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| self.pattern.clone())
    }
}

/// 8 个方向桶（屏幕坐标，y 向下）：东、东南、南、西南、西、西北、北、东北。
const DIRECTIONS: [&str; 8] = ["R", "DR", "D", "DL", "L", "UL", "U", "UR"];

/// 两点距离（像素）。
pub fn dist(a: (i32, i32), b: (i32, i32)) -> i32 {
    let dx = (b.0 - a.0) as i64;
    let dy = (b.1 - a.1) as i64;
    ((dx * dx + dy * dy) as f64).sqrt() as i32
}

/// 方向串归一化：只保留 `U`/`D`/`L`/`R` 四个字母，斜向写成两个字母（`UR`）。
///
/// 认得箭头（`↑` `↓` `←` `→` `↗`…）与 MouseInc 风格的单词（`UP` / `DOWN` /
/// `UPRIGHT`…）——用户从别的工具搬配置时不用逐条改写。
pub fn normalize_pattern(s: &str) -> String {
    let mut t = s.trim().to_uppercase();
    for (from, to) in [
        ("↖", "UL"),
        ("↗", "UR"),
        ("↙", "DL"),
        ("↘", "DR"),
        ("↑", "U"),
        ("↓", "D"),
        ("←", "L"),
        ("→", "R"),
        ("UPLEFT", "UL"),
        ("UPRIGHT", "UR"),
        ("DOWNLEFT", "DL"),
        ("DOWNRIGHT", "DR"),
        ("UP", "U"),
        ("DOWN", "D"),
        ("LEFT", "L"),
        ("RIGHT", "R"),
    ] {
        t = t.replace(from, to);
    }
    t.chars()
        .filter(|c| matches!(c, 'U' | 'D' | 'L' | 'R'))
        .collect()
}

/// 轨迹点 → 方向串。
///
/// 按 `min_segment` 抽稀（不足一段长度的位移并进下一段），每段取一个方向桶，
/// 相邻同向合并（`DDD` → `D`）。抖动因此不会多出方向。
pub fn pattern_from_points(points: &[(i32, i32)], min_segment: i32) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let min_segment = min_segment.max(1);
    let mut out = String::new();
    let mut anchor = points[0];
    for &p in &points[1..] {
        if dist(anchor, p) < min_segment {
            continue;
        }
        if let Some(dir) = direction(anchor, p) {
            if !out.ends_with(dir) {
                out.push_str(dir);
            }
            anchor = p;
        }
    }
    out
}

fn direction(a: (i32, i32), b: (i32, i32)) -> Option<&'static str> {
    let dx = (b.0 - a.0) as f64;
    let dy = (b.1 - a.1) as f64;
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    let mut deg = dy.atan2(dx).to_degrees();
    if deg < 0.0 {
        deg += 360.0;
    }
    let idx = match deg {
        d if d < 22.5 || d >= 337.5 => 0,
        d if d < 67.5 => 1,
        d if d < 112.5 => 2,
        d if d < 157.5 => 3,
        d if d < 202.5 => 4,
        d if d < 247.5 => 5,
        d if d < 292.5 => 6,
        _ => 7,
    };
    Some(DIRECTIONS[idx])
}

/// 配置里有没有启用的手势（没手势时不该抢右键）。
pub fn gesture_enabled(cfg: &Config) -> bool {
    cfg.gestures
        .iter()
        .any(|g| g.enabled && !normalize_pattern(&g.pattern).is_empty())
}

/// 按方向串找手势（**精确匹配**，不做前缀匹配：`D` 不该吃掉 `DR`）。
pub fn find_gesture<'a>(cfg: &'a Config, pattern: &str) -> Option<&'a Gesture> {
    let pattern = normalize_pattern(pattern);
    if pattern.is_empty() {
        return None;
    }
    cfg.gestures
        .iter()
        .find(|g| g.enabled && normalize_pattern(&g.pattern) == pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture(pattern: &str) -> Gesture {
        Gesture {
            name: Some("测试".into()),
            pattern: pattern.into(),
            actions: Vec::new(),
            enabled: true,
        }
    }

    #[test]
    fn straight_directions() {
        assert_eq!(
            pattern_from_points(&[(0, 0), (10, 0), (30, 0), (50, 0)], 16),
            "R"
        );
        assert_eq!(
            pattern_from_points(&[(0, 0), (0, 10), (0, 30), (0, 50)], 16),
            "D"
        );
        assert_eq!(
            pattern_from_points(&[(0, 0), (-10, 0), (-30, 0), (-50, 0)], 16),
            "L"
        );
        assert_eq!(
            pattern_from_points(&[(0, 0), (0, -10), (0, -30), (0, -50)], 16),
            "U"
        );
    }

    #[test]
    fn multi_segment_and_diagonal() {
        let pts = [(0, 0), (0, 40), (40, 40), (40, 0), (0, 0)];
        assert_eq!(pattern_from_points(&pts, 16), "DRUL");
        let diag = [(0, 0), (20, 20), (40, 40), (60, 60)];
        assert_eq!(pattern_from_points(&diag, 16), "DR");
    }

    #[test]
    fn jitter_does_not_add_noise() {
        let pts = [(0, 0), (3, 1), (26, 0), (27, 1), (52, 0)];
        assert_eq!(pattern_from_points(&pts, 16), "R");
    }

    #[test]
    fn normalization_accepts_words_and_arrows() {
        assert_eq!(normalize_pattern("down right up left"), "DRUL");
        assert_eq!(normalize_pattern("UP"), "U");
        assert_eq!(normalize_pattern("↓→↑←"), "DRUL");
        assert_eq!(normalize_pattern("up-right"), "UR");
    }

    #[test]
    fn find_gesture_is_exact_and_skips_disabled() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("DRUL"), gesture("D")];
        assert!(find_gesture(&cfg, "DRUL").is_some());
        // 前缀不算命中：`D` 不该吃掉 `DRUL`。
        assert!(find_gesture(&cfg, "DR").is_none());
        assert!(find_gesture(&cfg, "UL").is_none());
        cfg.gestures[0].enabled = false;
        assert!(find_gesture(&cfg, "DRUL").is_none());
    }

    #[test]
    fn gesture_enabled_requires_an_enabled_entry() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("D")];
        assert!(gesture_enabled(&cfg));
        cfg.gestures[0].enabled = false;
        assert!(!gesture_enabled(&cfg));
        cfg.gestures = vec![gesture("")];
        assert!(!gesture_enabled(&cfg));
    }

    #[test]
    fn mouse_hook_is_needed_for_gestures_but_the_switch_gates_it() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("D")];
        // 新功能默认**关**：没开总闸就不抢右键、鼠标钩子也不为手势而挂。
        assert!(!cfg.settings.experimental);
        assert!(!crate::config_needs_mouse_hook(&cfg));
        cfg.settings.experimental = true;
        assert!(crate::config_needs_mouse_hook(&cfg));
    }

    #[test]
    fn sanitize_drops_broken_and_duplicate_gestures() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("???"), gesture("D"), gesture("d")];
        let (clean, ignored) = crate::sanitize_config(&cfg);
        assert_eq!(clean.gestures.len(), 1, "ignored: {ignored:?}");
        assert_eq!(clean.gestures[0].pattern, "D");
        assert!(ignored.len() >= 2, "两处忽略都要报出来：{ignored:?}");
    }

    #[test]
    fn conflicts_warn_on_duplicate_gesture_pattern() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("D"), gesture("d")];
        let conflicts = crate::detect_conflicts(&cfg, &crate::PlatformCaps::default());
        assert!(
            conflicts.iter().any(|c| c.message.contains("方向串")),
            "重复方向串要报冲突：{conflicts:?}"
        );
    }

    #[test]
    fn merge_brings_gestures_and_skips_duplicates() {
        let mut current = Config::default();
        current.gestures = vec![gesture("D")];
        let mut incoming = Config::default();
        incoming.gestures = vec![gesture("R"), gesture("d")];
        let (out, report) = crate::merge_configs(&current, &incoming);
        assert_eq!(out.gestures.len(), 2);
        assert_eq!(report.gestures, 1);
        assert!(report.skipped.iter().any(|s| s.contains("方向串")));
    }
}
