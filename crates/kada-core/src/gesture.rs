//! 鼠标手势（规划 7.2-MG）：按住右键画一条轨迹，松手时按方向串匹配。
//!
//! 表示法是**方向串**而不是原始点模板：8 个方向桶（`U`/`D`/`L`/`R` + 四个斜向），
//! 段长阈值抽稀 + 相邻同向合并，手抖自己被吸收掉。方案与取舍见
//! `docs/鼠标手势与滚轮快捷键规划.md`。
//!
//! 平台无关的**纯逻辑**：钩子只负责收集轨迹点，识别在壳层做（钩子回调里不许耗时）。

use crate::{Action, Config, TriggerContext};
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
    /// 触发侧条件门控（`when`，规划 7.2-MG 余项）：仅当条件成立时这条才参与匹配，
    /// 否则跳过（落到下一条同方向串的手势，没有就回放右键）。`None` = 无条件。
    #[cfg(feature = "automation")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<crate::Condition>,
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

    /// `when` 的判重指纹（见 [`crate::WhenKey`]）。
    pub(crate) fn when_key(&self) -> crate::WhenKey {
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

/// 这个前台进程名是否命中**手势排除列表**（每行一个，语义同 `when` 的「前台应用」：
/// 不区分大小写，含 `*`/`?` 走通配、否则子串）。
pub fn app_excluded(patterns: &[String], name: &str) -> bool {
    patterns
        .iter()
        .any(|p| !p.trim().is_empty() && crate::match_name(p.trim(), name))
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
/// 认得箭头（`↑` `↓` `←` `→` `↗`…）与兼容词形（`UP` / `DOWN` /
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

/// 轨迹是否绕成一个圈，返回包围盒 `(左, 上, 右, 下)`（屏幕坐标）。
///
/// 判据是**绕包围盒中心的有向转角**累计超过 270°——手画的圈不必首尾贴合，椭的、歪的
/// 都算；来回折线 / 八字（正负抵消）不算。盒子任一边小于 16px 也不算（防手抖点一下）。
pub fn lasso_bbox(points: &[(i32, i32)]) -> Option<(i32, i32, i32, i32)> {
    if points.len() < 8 {
        return None;
    }
    let (mut l, mut t, mut r, mut b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(x, y) in points {
        l = l.min(x);
        t = t.min(y);
        r = r.max(x);
        b = b.max(y);
    }
    if r - l < 16 || b - t < 16 {
        return None;
    }
    let (cx, cy) = ((l + r) as f64 / 2.0, (t + b) as f64 / 2.0);
    let mut turn = 0.0f64;
    for pair in points.windows(2) {
        let (ax, ay) = (pair[0].0 as f64 - cx, pair[0].1 as f64 - cy);
        let (bx, by) = (pair[1].0 as f64 - cx, pair[1].1 as f64 - cy);
        let cross = ax * by - ay * bx;
        // 正对穿中心（共线反向）时 atan2 会把 180° 记成同一符号，来回直线就会被误判成圈；
        // 共线没有「转弯」，直接不计。
        if cross.abs() < 1e-9 {
            continue;
        }
        turn += cross.atan2(ax * bx + ay * by);
    }
    if turn.abs() >= std::f64::consts::PI * 1.5 {
        Some((l, t, r, b))
    } else {
        None
    }
}

/// 手势采集是否该开：有配置好的手势，或开了「圈选截图」（自带手势，不需要配置条目）。
pub fn gesture_capture_enabled(cfg: &Config) -> bool {
    gesture_enabled(cfg) || cfg.settings.gesture_screenshot
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
///
/// 同方向串多条时按配置顺序取**第一条 `when` 成立**的（`when` 不成立 = 该条不存在，
/// 落到下一条；规则与快捷键 / 改键一致）。
pub fn find_gesture<'a>(
    cfg: &'a Config,
    pattern: &str,
    ctx: &TriggerContext,
) -> Option<&'a Gesture> {
    let pattern = normalize_pattern(pattern);
    if pattern.is_empty() {
        return None;
    }
    cfg.gestures.iter().find(|g| {
        g.enabled && normalize_pattern(&g.pattern) == pattern && g.when_matches(ctx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture(pattern: &str) -> Gesture {
        Gesture {
            name: Some("测试".into()),
            pattern: pattern.into(),
            actions: Vec::new(),
            #[cfg(feature = "automation")]
            when: None,
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
    fn lasso_bbox_accepts_a_loop_and_rejects_zigzag() {
        // 近似圆的 16 个点（半径 40，中心 100,100）。
        let circle: Vec<(i32, i32)> = (0..16)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 16.0;
                ((100.0 + 40.0 * a.cos()) as i32, (100.0 + 40.0 * a.sin()) as i32)
            })
            .collect();
        assert_eq!(lasso_bbox(&circle), Some((60, 60, 140, 140)));

        // 来回画一条横线：绕中心转角正负抵消，不算圈。
        let zigzag = [(0, 0), (100, 0), (0, 0), (100, 0), (0, 0), (100, 0), (0, 0), (100, 0)];
        assert_eq!(lasso_bbox(&zigzag), None);

        // 太小 / 点太少都不算。
        assert_eq!(lasso_bbox(&[(0, 0), (5, 0), (5, 5), (0, 5)]), None);
    }

    #[test]
    fn gesture_capture_turns_on_with_lasso_screenshot() {
        let mut cfg = Config::default();
        cfg.settings.experimental = true;
        cfg.settings.gesture_screenshot = false;
        assert!(!gesture_capture_enabled(&cfg));
        cfg.settings.gesture_screenshot = true;
        assert!(gesture_capture_enabled(&cfg));
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
        let ctx = TriggerContext::empty();
        assert!(find_gesture(&cfg, "DRUL", &ctx).is_some());
        // 前缀不算命中：`D` 不该吃掉 `DRUL`。
        assert!(find_gesture(&cfg, "DR", &ctx).is_none());
        assert!(find_gesture(&cfg, "UL", &ctx).is_none());
        cfg.gestures[0].enabled = false;
        assert!(find_gesture(&cfg, "DRUL", &ctx).is_none());
    }

    #[cfg(feature = "automation")]
    #[test]
    fn when_gates_gestures_and_falls_through() {
        use crate::{Condition, FrontmostContext};
        let chrome = FrontmostContext {
            process_name: "chrome.exe".into(),
            window_title: "Chrome".into(),
        };
        let mut cfg = Config::default();
        let mut gated = gesture("D");
        gated.when = Some(Condition::FrontmostApp { app: "chrome".into() });
        cfg.gestures = vec![gated, gesture("D")];

        // 前台是 chrome → 走第一条（带 when）。
        let hit = find_gesture(&cfg, "D", &TriggerContext::fixed(Some(&chrome), None)).unwrap();
        assert!(hit.when.is_some());
        // 前台不是 chrome → 第一条不存在，落到第二条无门控的。
        let hit = find_gesture(&cfg, "D", &TriggerContext::empty()).unwrap();
        assert!(hit.when.is_none());
        // 只有带 when 的一条时，条件不成立 = 没有命中（回放右键）。
        cfg.gestures.truncate(1);
        assert!(find_gesture(&cfg, "D", &TriggerContext::empty()).is_none());
    }

    #[test]
    fn app_excluded_matches_substring_and_wildcards() {
        let list = vec!["photoshop.exe".to_string(), "*blender*".to_string(), "  ".into()];
        assert!(app_excluded(&list, "Photoshop.exe"));
        assert!(app_excluded(&list, "blender"));
        assert!(!app_excluded(&list, "notepad.exe"));
        assert!(!app_excluded(&[], "anything"));
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
