//! 鼠标手势（规划 7.2-MG）：按住右键画一条轨迹，松手时按方向串匹配。
//!
//! 表示法是**方向串**而不是原始点模板：8 个方向桶（`U`/`D`/`L`/`R` + 四个斜向），
//! 段长阈值抽稀 + 相邻同向合并，手抖自己被吸收掉。方案与取舍见
//! `docs/鼠标手势与滚轮快捷键规划.md`。
//!
//! **方向串的写法**（`D,R` 与 `DR` 是两回事）：
//! - 单个方向就写它自己：`D`、`R`、`DR`（右下斜向）；
//! - 先后两段用逗号（也认空格 / 分号 / 斜杠）分隔：`D,R` = 先下再右；
//! - 斜向是一段 45° 方向，`DR` 永远表示「一段右下」，不会拆成 `D,R`；
//! - 相邻重复自动去重：`D,D,R` → `D,R`。
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
    /// 方向串：`U`/`D`/`L`/`R` 与斜向 `UR`/`DR`/`DL`/`UL`；多段用逗号分隔（如 `D,R,U,L`）。
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    R,
    DR,
    D,
    DL,
    L,
    UL,
    U,
    UR,
}

impl Dir {
    /// 方向串里的写法（斜向是两个字母，但仍只是**一个**方向）。
    fn token(self) -> &'static str {
        match self {
            Dir::R => "R",
            Dir::DR => "DR",
            Dir::D => "D",
            Dir::DL => "DL",
            Dir::L => "L",
            Dir::UL => "UL",
            Dir::U => "U",
            Dir::UR => "UR",
        }
    }

    fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "R" => Dir::R,
            "DR" => Dir::DR,
            "D" => Dir::D,
            "DL" => Dir::DL,
            "L" => Dir::L,
            "UL" => Dir::UL,
            "U" => Dir::U,
            "UR" => Dir::UR,
            _ => return None,
        })
    }

    fn from_letter(c: char) -> Option<Dir> {
        Some(match c {
            'R' => Dir::R,
            'D' => Dir::D,
            'L' => Dir::L,
            'U' => Dir::U,
            _ => return None,
        })
    }

    /// 位移向量 → 方向桶（45° 一格）。
    fn from_vec(dx: f64, dy: f64) -> Option<Dir> {
        if dx == 0.0 && dy == 0.0 {
            return None;
        }
        let mut deg = dy.atan2(dx).to_degrees();
        if deg < 0.0 {
            deg += 360.0;
        }
        Some(match deg {
            d if d < 22.5 || d >= 337.5 => Dir::R,
            d if d < 67.5 => Dir::DR,
            d if d < 112.5 => Dir::D,
            d if d < 157.5 => Dir::DL,
            d if d < 202.5 => Dir::L,
            d if d < 247.5 => Dir::UL,
            d if d < 292.5 => Dir::U,
            _ => Dir::UR,
        })
    }

    /// 桶序号（`R` 起顺时针，45° 一格）。
    fn index(self) -> i32 {
        match self {
            Dir::R => 0,
            Dir::DR => 1,
            Dir::D => 2,
            Dir::DL => 3,
            Dir::L => 4,
            Dir::UL => 5,
            Dir::U => 6,
            Dir::UR => 7,
        }
    }

    /// 两个方向相差不超过一格（≤45°）：同一条（略带抖动的）直线会被分到相邻两桶，
    /// 判定时要把它们并回一段——这正是「斜线被判成 DR/R 来回跳」的根源。
    fn adjacent(self, other: Dir) -> bool {
        let d = (self.index() - other.index()).rem_euclid(8);
        d.min(8 - d) <= 1
    }
}

/// 多段方向串的分隔符（逗号最好读，也认空格 / 分号 / 斜杠 / 加号）。
///
/// **`-` 不算分隔符**：`up-right` 这类兼容写法要能整段折成斜向 `UR`。
fn is_sep(c: char) -> bool {
    matches!(c, ',' | '，' | '、' | ';' | '；' | '/' | '>' | '+' | '|') || c.is_whitespace()
}

fn is_dir_letter(c: char) -> bool {
    matches!(c, 'U' | 'D' | 'L' | 'R')
}

/// 方向串归一化：统一成 `D` / `DR` / `D,R` 这样的规范写法。
///
/// - 单个方向（含斜向）→ 一个 token：`dr`、`↘`、`DOWNRIGHT` 都归一成 `DR`；
/// - 显式分隔的多段 → 每段一个 token：`D R` / `d,r` / `D-R`（`-` 不分割）见下方；
/// - **没有分隔符的旧写法按字母拆**：`DRUL` → `D,R,U,L`（老配置里它就是四段）；
///   两个字母且是合法斜向（`DR`）→ 仍然是一个斜向 token，不会被拆成 `D,R`；
/// - 相邻重复去重：`D,D,R` → `D,R`。
///
/// 认得箭头（`↑` `↓` `←` `→` `↗`…）与兼容词形（`UP` / `DOWN` / `UPRIGHT`…）——
/// 用户从别的工具搬配置时不用逐条改写。
pub fn normalize_pattern(s: &str) -> String {
    let mut t = s.trim().to_uppercase();
    // 词形先转（长的先转，避免 `UPRIGHT` 被 `UP` 吃掉）。
    for (from, to) in [
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
    // 箭头是明确的单方向：每个都补一个分隔符，`↓→↑←` 才不会被拼成 `DRUL` 那串。
    for (from, to) in [
        ("↖", "UL,"),
        ("↗", "UR,"),
        ("↙", "DL,"),
        ("↘", "DR,"),
        ("↑", "U,"),
        ("↓", "D,"),
        ("←", "L,"),
        ("→", "R,"),
    ] {
        t = t.replace(from, to);
    }
    pattern_tokens(&t)
        .iter()
        .map(|d| d.token())
        .collect::<Vec<_>>()
        .join(",")
}

/// 把（已转大写 / 已换词形的）方向串解析成 token 列表，相邻同向合并。
fn pattern_tokens(t: &str) -> Vec<Dir> {
    let mut out = Vec::new();
    let push = |out: &mut Vec<Dir>, d: Dir| {
        if out.last() != Some(&d) {
            out.push(d);
        }
    };
    if t.chars().any(is_sep) {
        for part in t.split(is_sep) {
            let letters: String = part.chars().filter(|c| is_dir_letter(*c)).collect();
            if letters.is_empty() {
                continue;
            }
            match Dir::parse(&letters) {
                Some(d) => push(&mut out, d),
                // 一段里塞了多个方向（`UD`）：按单字母拆开。
                None => {
                    for c in letters.chars() {
                        if let Some(d) = Dir::from_letter(c) {
                            push(&mut out, d);
                        }
                    }
                }
            }
        }
        return out;
    }
    // 没有分隔符：整体是一个方向（`DR` 斜向），不是就按单字母拆（旧写法 `DRUL`）。
    let letters: String = t.chars().filter(|c| is_dir_letter(*c)).collect();
    if letters.is_empty() {
        return out;
    }
    match Dir::parse(&letters) {
        Some(d) => push(&mut out, d),
        None => {
            for c in letters.chars() {
                if let Some(d) = Dir::from_letter(c) {
                    push(&mut out, d);
                }
            }
        }
    }
    out
}

/// 折线简化（Ramer–Douglas–Peucker）：只留下偏离主干超过 `eps` 的拐点。
///
/// 这一步是方向串稳定的关键——手画直线的那点抖动、轻微弧线都会被压成一条主干，
/// 真正的直角拐弯仍然保留；不做这一步，16px 一段的量化会把一条斜线拆成
/// `D`+`DR`（用户看到的「我画的是 DR，它判成 DDR」）。
fn simplify(points: &[(i32, i32)], eps: f64) -> Vec<(i32, i32)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let (ax, ay) = (first.0 as f64, first.1 as f64);
    let (dx, dy) = (last.0 as f64 - ax, last.1 as f64 - ay);
    let len = (dx * dx + dy * dy).sqrt();
    let mut idx = 0usize;
    let mut max = 0.0f64;
    for (i, p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let (px, py) = (p.0 as f64, p.1 as f64);
        let d = if len == 0.0 {
            // 首尾重合（画了个圈）：退化成到端点的距离。
            ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
        } else {
            ((dx * (ay - py) - (ax - px) * dy) / len).abs()
        };
        if d > max {
            max = d;
            idx = i;
        }
    }
    if max <= eps {
        return vec![first, last];
    }
    let mut left = simplify(&points[..=idx], eps);
    let right = simplify(&points[idx..], eps);
    left.pop(); // 去掉重复的拐点
    left.extend(right);
    left
}

/// 轨迹点 → 方向串（规范写法：单 token 直接写，多段用逗号分隔）。
///
/// 先 RDP 简化（吸收手抖），再按 `min_segment` 抽稀：从当前锚点走满一段才定一个方向；
/// 不足一段的位移并进下一段（不落 token）。**相邻 ≤45° 的两段并成一段**（用位移向量相加后
/// 再定向），所以一条略带抖动的斜线不会在 `R` / `DR` 两桶之间来回跳；真正的 90° 拐弯保留。
pub fn pattern_from_points(points: &[(i32, i32)], min_segment: i32) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let min = min_segment.max(1) as f64;
    let simplified = simplify(points, (min / 4.0).max(2.0));
    // 每段先按「位移向量」累积；相邻 ≤45° 的段合并（向量相加，方向取平均）。
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut anchor = simplified[0];
    for &p in &simplified[1..] {
        let dx = (p.0 - anchor.0) as f64;
        let dy = (p.1 - anchor.1) as f64;
        // 还没走满一段：位移留在锚点里，攒到下一段一起判方向。
        if (dx * dx + dy * dy).sqrt() < min {
            continue;
        }
        let mut merged = false;
        if let Some((rx, ry)) = runs.last_mut() {
            if let (Some(cur), Some(new)) = (Dir::from_vec(*rx, *ry), Dir::from_vec(dx, dy)) {
                if cur.adjacent(new) {
                    *rx += dx;
                    *ry += dy;
                    merged = true;
                }
            }
        }
        if !merged {
            runs.push((dx, dy));
        }
        anchor = p;
    }
    let mut tokens: Vec<Dir> = Vec::new();
    for (dx, dy) in runs {
        if let Some(dir) = Dir::from_vec(dx, dy) {
            if tokens.last() != Some(&dir) {
                tokens.push(dir);
            }
        }
    }
    tokens.iter().map(|d| d.token()).collect::<Vec<_>>().join(",")
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

/// 配置里有没有启用的手势（没手势时不该抢右键）。
pub fn gesture_enabled(cfg: &Config) -> bool {
    cfg.gestures
        .iter()
        .any(|g| g.enabled && !normalize_pattern(&g.pattern).is_empty())
}

/// 按方向串找手势（**精确匹配**，不做前缀匹配：`D` 不该吃掉 `DR`）。
///
/// `D,R`（先下再右）与 `DR`（右下斜向）是两条不同的方向串，各自只命中自己。
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
        // 四段直角：每段之间是真正的拐弯，逗号分隔。
        assert_eq!(pattern_from_points(&pts, 16), "D,R,U,L");
        let diag = [(0, 0), (20, 20), (40, 40), (60, 60)];
        // 一条斜线只是一个方向，哪怕它有两个字母。
        assert_eq!(pattern_from_points(&diag, 16), "DR");
    }

    /// 手抖的斜线不能拆成 `D` + `DR`（用户报的「画 DR 被判成 DDR」）。
    #[test]
    fn jittery_diagonal_is_one_token() {
        let pts = [
            (0, 0),
            (4, 7),
            (11, 10),
            (17, 22),
            (28, 26),
            (35, 39),
            (49, 44),
            (61, 66),
        ];
        assert_eq!(pattern_from_points(&pts, 16), "DR");
        // 更细的段长也不该把一个方向拆成两段。
        assert_eq!(pattern_from_points(&pts, 8), "DR");
    }

    /// 先下再右（真的转了 90°）与右下斜向是两条不同的方向串。
    #[test]
    fn down_then_right_is_not_the_diagonal() {
        let pts = [(0, 0), (0, 20), (0, 40), (0, 60), (20, 60), (50, 60)];
        assert_eq!(pattern_from_points(&pts, 16), "D,R");
        assert_ne!(pattern_from_points(&pts, 16), "DR");
        assert_eq!(normalize_pattern("D,R"), "D,R");
        assert_eq!(normalize_pattern("DR"), "DR");
    }

    /// 相邻重复去重：`D,D,R` 与 `D,R` 是同一条。
    #[test]
    fn repeated_directions_collapse() {
        assert_eq!(normalize_pattern("D,D,R"), "D,R");
        assert_eq!(normalize_pattern("D D R"), "D,R");
        assert_eq!(normalize_pattern("DDR"), "D,R");
        assert_eq!(normalize_pattern("ddr"), "D,R");
        // 隔开的方向不去重：先下、再右、再下。
        assert_eq!(normalize_pattern("D,R,D"), "D,R,D");
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
        // 显式分隔（空格）的多段写成逗号分隔；`up-right` 这种连写仍是单个斜向。
        assert_eq!(normalize_pattern("down right up left"), "D,R,U,L");
        assert_eq!(normalize_pattern("UP"), "U");
        assert_eq!(normalize_pattern("↓→↑←"), "D,R,U,L");
        assert_eq!(normalize_pattern("up-right"), "UR");
        assert_eq!(normalize_pattern("↘"), "DR");
        assert_eq!(normalize_pattern("DOWNRIGHT"), "DR");
    }

    #[test]
    fn find_gesture_is_exact_and_skips_disabled() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("DRUL"), gesture("D")];
        let ctx = TriggerContext::empty();
        // 旧写法 `DRUL` 归一到 `D,R,U,L`，与画出来的四段一致。
        assert!(find_gesture(&cfg, "D,R,U,L", &ctx).is_some());
        // 前缀不算命中：`D` 不该吃掉 `DRUL`。
        assert!(find_gesture(&cfg, "DR", &ctx).is_none());
        assert!(find_gesture(&cfg, "UL", &ctx).is_none());
        cfg.gestures[0].enabled = false;
        assert!(find_gesture(&cfg, "D,R,U,L", &ctx).is_none());
    }

    /// `D,R` 与 `DR` 互不命中：方向串精确匹配，不再靠字母拼接猜。
    #[test]
    fn diagonal_and_two_segments_do_not_match_each_other() {
        let mut cfg = Config::default();
        cfg.gestures = vec![gesture("D,R")];
        let ctx = TriggerContext::empty();
        assert!(find_gesture(&cfg, "D,R", &ctx).is_some());
        assert!(find_gesture(&cfg, "DR", &ctx).is_none());

        cfg.gestures = vec![gesture("DR")];
        assert!(find_gesture(&cfg, "DR", &ctx).is_some());
        assert!(find_gesture(&cfg, "D,R", &ctx).is_none());
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
