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

use crate::{Action, Config, Key, TriggerContext};
use serde::{Deserialize, Serialize};

/// 触发键在配置里落成键名（`"CapsLock"`），与快捷键 / 改键的 `from` 同一套写法。
/// `Key` 本身没有 serde 派生（配置里一律存字符串），这里只给 `Option<Key>` 用。
mod key_serde {
    use crate::Key;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Key>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(k) => s.serialize_str(crate::key_name(*k)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Key>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(s) => s.parse::<Key>().map(Some).map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

/// 一条鼠标手势。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Gesture {
    /// 条目标题（列表 / 气泡展示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 条目描述（只做展示，与快捷键的 `description` 同义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 方向串：`U`/`D`/`L`/`R` 与斜向 `UR`/`DR`/`DL`/`UL`；多段用逗号分隔（如 `D,R,U,L`）。
    #[serde(default)]
    pub pattern: String,
    /// 触发方式：`None` = 按住**右键**画（鼠标手势）；`Some(key)` = 按住该键、在触摸板上画
    /// （触摸板手势）。缺字段 = 右键（老配置原样读）。触发键本身被吞掉——它只是「进入
    /// 采集」，没有输出，所以没命中时也没有右键要回放。
    #[serde(default, with = "key_serde", skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Key>,
    /// 个人形状模板（触摸板手势的兜底）：[`template_from_points`] 归一化后的 24 个点，
    /// 每轴 `0..=1000`。**只在方向串匹配不上时才用**——匹配上了就不再拿它改判，
    /// 免得别人分享的模板把你这台机器锁死。缺字段 = 没有模板。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<Vec<[i16; 2]>>,
    /// 触发后按顺序执行的多个动作（复用 [`Action`]，与快捷键同一套动作引擎）。
    /// 归属文件夹（`None` = 根级）：与快捷键共用一套目录树，只影响列表组织。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 统一展示顺序：鼠标操作 / 角动作 / 边缘滚动共用一个序列，只影响列表顺序；
    /// 匹配仍是「同类型、同位置 / 同方向串」内按数组顺序取第一条。缺字段 = 0。
    #[serde(default, skip_serializing_if = "crate::is_zero_u64")]
    pub order: u64,
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

    /// 是不是**触摸板手势**（按住一个键来采集，而不是按住右键）。
    pub fn is_touchpad(&self) -> bool {
        self.trigger.is_some()
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

// ---------------------------------------------------------------------------
// 触摸板轨迹（7.2-MG 触摸板手势）：尺度无关的方向串 + 形状模板
//
// 触摸板上画一笔只有几厘米、采样稀疏（钩子事件还可能被合并）、还要经过指针加速，
// 右键拖拽那套**屏幕像素**阈值（`gesture_min_segment` / `eps = min/4`）在这里会直接崩：
// 短笔画被切成锯齿、拐角被抹平。所以这里的阈值全部**按笔画自身尺寸取比例**。
// ---------------------------------------------------------------------------

/// 重采样后固定点数（方向量化与模板共用）。
const STROKE_RESAMPLE: usize = 48;
/// 掐头去尾的比例（按弧长）：手指落下 / 抬起那一下最容易带出一个小钩子。
const STROKE_TRIM: f64 = 0.08;
/// RDP 容差 = 包围盒对角线 × 这个比例。
const STROKE_EPS_RATIO: f64 = 0.06;
/// 一段至少占总行程的比例：不足的并进下一段（不落 token）。
const STROKE_MIN_SEG_RATIO: f64 = 0.10;
/// 「先轴后斜」的吸附角：离正方向不超过它就算正方向。
const STROKE_AXIS_SNAP_DEG: f64 = 25.0;
/// 一段轨迹最多吐几个 token：再多基本是误识（触摸板上画不了那么细）。
const STROKE_MAX_TOKENS: usize = 4;
/// 模板点数（存储与比较一致）。
pub const TEMPLATE_POINTS: usize = 24;
/// 模板匹配阈值：平均点距 / 归一化边长（0 = 完全一样，0.14 ≈ 形状明显走样）。
pub const TEMPLATE_MAX_DISTANCE: f64 = 0.14;

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn path_length(pts: &[(f64, f64)]) -> f64 {
    pts.windows(2).map(|w| dist(w[0], w[1])).sum()
}

/// 去掉相邻重复点（触摸板会连发同一坐标的移动）。
fn dedupe_path(points: &[(i32, i32)]) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &(x, y) in points {
        let p = (x as f64, y as f64);
        if out.last() != Some(&p) {
            out.push(p);
        }
    }
    out
}

/// 沿折线取「距起点 `want`」处的一点（超出总长取末点）。
fn point_at(pts: &[(f64, f64)], want: f64) -> (f64, f64) {
    let Some(&last) = pts.last() else {
        return (0.0, 0.0);
    };
    let mut acc = 0.0f64;
    for pair in pts.windows(2) {
        let seg = dist(pair[0], pair[1]);
        if seg <= f64::EPSILON {
            continue;
        }
        if acc + seg >= want {
            let t = ((want - acc) / seg).clamp(0.0, 1.0);
            return (
                pair[0].0 + (pair[1].0 - pair[0].0) * t,
                pair[0].1 + (pair[1].1 - pair[0].1) * t,
            );
        }
        acc += seg;
    }
    last
}

/// 按弧长剪掉首尾各 `ratio`（0..0.45）的行程，两端用插值点补齐。
fn trim_path(pts: &[(f64, f64)], ratio: f64) -> Vec<(f64, f64)> {
    let total = path_length(pts);
    if total <= f64::EPSILON || pts.len() < 2 {
        return pts.to_vec();
    }
    let ratio = ratio.clamp(0.0, 0.45);
    let (lo, hi) = (total * ratio, total * (1.0 - ratio));
    if hi <= lo {
        return Vec::new();
    }
    let mut out = vec![point_at(pts, lo)];
    let mut acc = 0.0f64;
    for pair in pts.windows(2) {
        acc += dist(pair[0], pair[1]);
        if acc > lo && acc < hi {
            out.push(pair[1]);
        }
    }
    out.push(point_at(pts, hi));
    out.dedup_by(|a, b| dist(*a, *b) < 1e-6);
    out
}

/// 按弧长等距重采样成 `n` 个点。
fn resample_by_length(pts: &[(f64, f64)], n: usize) -> Vec<(f64, f64)> {
    let total = path_length(pts);
    if total <= f64::EPSILON || n < 2 {
        return Vec::new();
    }
    (0..n)
        .map(|i| point_at(pts, total * i as f64 / (n - 1) as f64))
        .collect()
}

/// 三点加权平滑（端点不动），两遍就够压掉量化噪声。
fn smooth_path(pts: &mut [(f64, f64)], passes: usize) {
    for _ in 0..passes {
        if pts.len() < 3 {
            return;
        }
        let src = pts.to_vec();
        for i in 1..src.len() - 1 {
            pts[i] = (
                (src[i - 1].0 + 2.0 * src[i].0 + src[i + 1].0) / 4.0,
                (src[i - 1].1 + 2.0 * src[i].1 + src[i + 1].1) / 4.0,
            );
        }
    }
}

/// RDP 简化（浮点版）：与 [`simplify`] 同一算法，给重采样后的轨迹用。
fn simplify_f(points: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    let (dx, dy) = (last.0 - first.0, last.1 - first.1);
    let len = (dx * dx + dy * dy).sqrt();
    let mut idx = 0usize;
    let mut max = 0.0f64;
    for (i, p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let d = if len == 0.0 {
            dist(*p, first)
        } else {
            ((dx * (first.1 - p.1) - (first.0 - p.0) * dy) / len).abs()
        };
        if d > max {
            max = d;
            idx = i;
        }
    }
    if max <= eps {
        return vec![first, last];
    }
    let mut left = simplify_f(&points[..=idx], eps);
    let right = simplify_f(&points[idx..], eps);
    left.pop();
    left.extend(right);
    left
}

fn bbox_diag(pts: &[(f64, f64)]) -> f64 {
    let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in pts {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    if !minx.is_finite() || !maxx.is_finite() {
        return 0.0;
    }
    ((maxx - minx).powi(2) + (maxy - miny).powi(2)).sqrt()
}

/// 方向量化（触摸板口径）：**先轴后斜**。
///
/// 离最近的正方向 ≤ [`STROKE_AXIS_SNAP_DEG`] 就算正方向，剩下来的扇区才给斜向。
/// 触摸板上画斜向本来就难，把 30° 的歪斜判成「横向」比吐出用户没画的 `DR` 更安全。
fn dir_of_run(dx: f64, dy: f64) -> Option<Dir> {
    let mut deg = dy.atan2(dx).to_degrees();
    if deg < 0.0 {
        deg += 360.0;
    }
    let axis = (deg / 90.0).round() * 90.0;
    let mut delta = (deg - axis).abs();
    if delta > 180.0 {
        delta = 360.0 - delta;
    }
    if delta <= STROKE_AXIS_SNAP_DEG {
        return match (axis as i64).rem_euclid(360) {
            0 => Some(Dir::R),
            90 => Some(Dir::D),
            180 => Some(Dir::L),
            _ => Some(Dir::U),
        };
    }
    Dir::from_vec(dx, dy)
}

/// 触摸板 / 任意鼠标移动的轨迹 → 方向串（**尺度无关**）。
///
/// 与 [`pattern_from_points`] 的分工：那边是右键拖拽（屏幕像素阈值够用），这边给
/// 「按住一个键、在触摸板上画」用。差别：
/// - 掐掉首尾各 8% 弧长（落下 / 抬起的钩子）；
/// - 按弧长重采样 48 点 + 平滑两遍（采样稀疏时也不把直线切碎）；
/// - RDP 容差与最小段长都按笔画自身尺寸取比例；
/// - 方向量化先轴后斜。
///
/// `min_len_px` 只是「太小不算数」的闸（手抖点一下），识别本身不依赖绝对长度。
pub fn pattern_from_stroke(points: &[(i32, i32)], min_len_px: f64) -> String {
    let raw = dedupe_path(points);
    if raw.len() < 2 {
        return String::new();
    }
    let total = path_length(&raw);
    if total < min_len_px.max(1.0) {
        return String::new();
    }
    let trimmed = trim_path(&raw, STROKE_TRIM);
    if trimmed.len() < 2 {
        return String::new();
    }
    let mut path = resample_by_length(&trimmed, STROKE_RESAMPLE);
    if path.len() < 3 {
        return String::new();
    }
    let trimmed_walk = path_length(&trimmed);
    smooth_path(&mut path, 2);
    let walk = path_length(&path);
    let span = bbox_diag(&path);
    if span < 2.0 || walk < min_len_px.max(1.0) * 0.5 {
        return String::new();
    }
    // 平滑吃掉大半行程 = 这一笔本身是来回抖 / 涂鸦：没有可识别的形状，别硬猜方向。
    if walk < trimmed_walk * 0.35 {
        return String::new();
    }
    // 来回涂鸦：行程远超包围盒，形状没有意义（正常一笔 walk 约为 span 的 1~3 倍）。
    if walk > span * 3.5 {
        return String::new();
    }
    let simplified = simplify_f(&path, (span * STROKE_EPS_RATIO).max(0.5));
    let min_seg = (walk * STROKE_MIN_SEG_RATIO).max(1.0);
    let mut runs: Vec<(f64, f64)> = Vec::new();
    let mut anchor = simplified[0];
    for &p in &simplified[1..] {
        let (dx, dy) = (p.0 - anchor.0, p.1 - anchor.1);
        if (dx * dx + dy * dy).sqrt() < min_seg {
            continue;
        }
        let merged = match runs.last_mut() {
            Some((rx, ry)) => match (Dir::from_vec(*rx, *ry), Dir::from_vec(dx, dy)) {
                (Some(a), Some(b)) if a.adjacent(b) => {
                    *rx += dx;
                    *ry += dy;
                    true
                }
                _ => false,
            },
            None => false,
        };
        if !merged {
            runs.push((dx, dy));
        }
        anchor = p;
    }
    let mut tokens: Vec<Dir> = Vec::new();
    for (dx, dy) in runs {
        if let Some(dir) = dir_of_run(dx, dy) {
            if tokens.last() != Some(&dir) {
                tokens.push(dir);
            }
        }
    }
    if tokens.len() > STROKE_MAX_TOKENS {
        return String::new();
    }
    tokens.iter().map(|d| d.token()).collect::<Vec<_>>().join(",")
}

/// 轨迹 → 归一化形状模板（[`TEMPLATE_POINTS`] 个点，每轴 `0..=1000`）。
///
/// 平移到包围盒原点、按**长边**缩放到 1000（保持长宽比、不旋转）：触摸板的方向对用户
/// 是有意义的，允许旋转会把「横向一笔」的模板匹配到「竖向一笔」上。太小 / 全是同一点
/// 的轨迹返回 `None`（没有形状可存）。
pub fn template_from_points(points: &[(i32, i32)]) -> Option<Vec<[i16; 2]>> {
    let raw = dedupe_path(points);
    if raw.len() < 2 {
        return None;
    }
    let trimmed = trim_path(&raw, STROKE_TRIM);
    if path_length(&trimmed) < 1.0 {
        return None;
    }
    let mut path = resample_by_length(&trimmed, TEMPLATE_POINTS);
    if path.len() != TEMPLATE_POINTS {
        return None;
    }
    smooth_path(&mut path, 2);
    let (mut minx, mut miny) = (f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &(x, y) in &path {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    }
    let scale = (maxx - minx).max(maxy - miny);
    if !scale.is_finite() || scale < 1.0 {
        return None;
    }
    Some(
        path.iter()
            .map(|&(x, y)| {
                [
                    (((x - minx) / scale) * 1000.0).round() as i16,
                    (((y - miny) / scale) * 1000.0).round() as i16,
                ]
            })
            .collect(),
    )
}

/// 两个模板的平均点距（按 1000 边长归一；`0` = 完全一样）。点数不一致 → `None`。
pub fn template_distance(a: &[[i16; 2]], b: &[[i16; 2]]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let sum: f64 = a
        .iter()
        .zip(b)
        .map(|(p, q)| {
            let dx = (p[0] as f64 - q[0] as f64) / 1000.0;
            let dy = (p[1] as f64 - q[1] as f64) / 1000.0;
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    Some(sum / a.len() as f64)
}

/// 轨迹是否绕成一个圈，返回包围盒 `(左, 上, 右, 下)`（屏幕坐标）。
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

/// 手势采集是否该开：有配置好的**右键**手势，或开了「圈选截图」（自带手势，不需要配置条目）。
///
/// 触摸板手势不走这条：它们只在按住触发键期间采集（[`touchpad_gesture_enabled`]），
/// 不该因为配了触摸板手势就全局抢右键。
pub fn gesture_capture_enabled(cfg: &Config) -> bool {
    gesture_enabled(cfg) || cfg.settings.gesture_screenshot
}

/// 配置里有没有启用的**右键**手势（没手势时不该抢右键）。
pub fn gesture_enabled(cfg: &Config) -> bool {
    cfg.gestures.iter().any(|g| {
        g.enabled && !g.is_touchpad() && !normalize_pattern(&g.pattern).is_empty()
    })
}

/// 配置里有没有启用的**触摸板**手势（按住触发键采集那一种）。
pub fn touchpad_gesture_enabled(cfg: &Config) -> bool {
    cfg.gestures
        .iter()
        .any(|g| g.enabled && g.is_touchpad() && !normalize_pattern(&g.pattern).is_empty())
}

/// 这个键是不是某条启用中的触摸板手势的**触发键**（壳层在按键回调里用它判「要不要进采集」）。
pub fn is_touchpad_trigger(cfg: &Config, key: Key) -> bool {
    cfg.gestures
        .iter()
        .any(|g| g.enabled && g.trigger == Some(key) && !normalize_pattern(&g.pattern).is_empty())
}

/// 按方向串找**右键**手势（精确匹配）。
pub fn find_gesture<'a>(
    cfg: &'a Config,
    pattern: &str,
    ctx: &TriggerContext,
) -> Option<&'a Gesture> {
    find_gesture_for(cfg, pattern, None, ctx)
}

/// 按方向串找手势（**精确匹配**，不做前缀匹配：`D` 不该吃掉 `DR`）。
///
/// `D,R`（先下再右）与 `DR`（右下斜向）是两条不同的方向串，各自只命中自己。
/// `trigger` 区分来源：`None` = 右键拖动，`Some(key)` = 按住该键（触摸板手势）——
/// 同一个方向串在两种来源下可以各配一条，互不串味。
/// 同方向串多条时按配置顺序取**第一条 `when` 成立**的（`when` 不成立 = 该条不存在，
/// 落到下一条；规则与快捷键 / 改键一致）。
pub fn find_gesture_for<'a>(
    cfg: &'a Config,
    pattern: &str,
    trigger: Option<Key>,
    ctx: &TriggerContext,
) -> Option<&'a Gesture> {
    let pattern = normalize_pattern(pattern);
    if pattern.is_empty() {
        return None;
    }
    cfg.gestures.iter().find(|g| {
        g.enabled
            && g.trigger == trigger
            && normalize_pattern(&g.pattern) == pattern
            && g.when_matches(ctx)
    })
}

/// 方向串没命中时的**模板兜底**：取形状最近、且在 [`TEMPLATE_MAX_DISTANCE`] 以内的一条。
///
/// 只加召回、不改判：已经由方向串命中的结果不会被模板推翻（见 [`Gesture::template`]）。
pub fn find_gesture_by_template<'a>(
    cfg: &'a Config,
    points: &[(i32, i32)],
    trigger: Option<Key>,
    ctx: &TriggerContext,
) -> Option<&'a Gesture> {
    let probe = template_from_points(points)?;
    cfg.gestures
        .iter()
        .filter(|g| g.enabled && g.trigger == trigger && g.when_matches(ctx))
        .filter_map(|g| {
            let t = g.template.as_deref()?;
            template_distance(t, &probe).map(|d| (d, g))
        })
        .filter(|(d, _)| *d <= TEMPLATE_MAX_DISTANCE)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, g)| g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gesture(pattern: &str) -> Gesture {
        Gesture {
            name: Some("测试".into()),
            description: None,
            pattern: pattern.into(),
            trigger: None,
            template: None,
            actions: Vec::new(),
            #[cfg(feature = "automation")]
            when: None,
            enabled: true,
            folder: None,
            order: 0,
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

    // ---- 触摸板手势（7.2-MG）：尺度无关的方向串 + 模板 ----

    /// 一笔从 `from` 到 `to` 的直线，带一点固定相位抖动（模拟触摸板噪声）。
    fn line(from: (f64, f64), to: (f64, f64), steps: usize) -> Vec<(i32, i32)> {
        (0..=steps)
            .map(|i| {
                let t = i as f64 / steps as f64;
                let x = from.0 + (to.0 - from.0) * t;
                let y = from.1 + (to.1 - from.1) * t;
                let jitter = if i == 0 || i == steps { 0.0 } else { (i as f64 * 1.7).sin() * 0.8 };
                ((x + jitter) as i32, (y + jitter * 0.5) as i32)
            })
            .collect()
    }

    #[test]
    fn stroke_pattern_is_scale_invariant() {
        // 同一形状、行程差 5 倍，方向串必须一致：阈值全是按比例取的。
        let small = line((0.0, 0.0), (50.0, 0.0), 12);
        let big = line((0.0, 0.0), (250.0, 0.0), 40);
        assert_eq!(pattern_from_stroke(&small, 24.0), "R");
        assert_eq!(pattern_from_stroke(&big, 24.0), "R");
        for (dx, dy, want) in [(0.0, 1.0, "D"), (-1.0, 0.0, "L"), (0.0, -1.0, "U")] {
            let a = line((100.0, 100.0), (100.0 + dx * 60.0, 100.0 + dy * 60.0), 15);
            assert_eq!(pattern_from_stroke(&a, 24.0), want, "dx={dx} dy={dy}");
        }
    }

    #[test]
    fn stroke_pattern_swallows_take_off_hooks() {
        // 手指落下 / 抬起那一下常带一个侧向小钩子：不能把「横向」判成 DR,R / R,DR。
        let mut pts = vec![(0, 14), (2, 8), (5, 3)];
        pts.extend(line((8.0, 0.0), (60.0, 0.0), 14));
        pts.extend([(65, 2), (68, 7), (70, 12)]);
        assert_eq!(pattern_from_stroke(&pts, 24.0), "R", "首尾钩子要掐掉");
    }

    #[test]
    fn stroke_pattern_keeps_real_corners() {
        // 先右后下：真拐角不能被平滑抹掉。
        let mut pts = line((0.0, 0.0), (50.0, 0.0), 10);
        pts.extend(line((50.0, 0.0), (50.0, 50.0), 10).into_iter().skip(1));
        assert_eq!(pattern_from_stroke(&pts, 24.0), "R,D");
    }

    #[test]
    fn stroke_pattern_snaps_to_axis_before_diagonal() {
        // 偏 20° 的「横向」算横向；45° 才是斜向。触摸板上斜向难画，必须先轴后斜。
        let flat = line((0.0, 0.0), (100.0, 36.0), 20); // ≈ 20°
        assert_eq!(pattern_from_stroke(&flat, 24.0), "R", "20° 歪斜仍算横向");
        let diag = line((0.0, 0.0), (100.0, 100.0), 20);
        assert_eq!(pattern_from_stroke(&diag, 24.0), "DR");
    }

    #[test]
    fn stroke_pattern_rejects_tiny_and_scribbles() {
        assert_eq!(pattern_from_stroke(&line((0.0, 0.0), (8.0, 0.0), 4), 24.0), "");
        assert_eq!(pattern_from_stroke(&[], 24.0), "");
        // 原地来回涂鸦：行程远超包围盒，形状没有意义。
        let mut scribble = Vec::new();
        for i in 0..40 {
            let x = if i % 2 == 0 { 0.0 } else { 40.0 };
            scribble.push((x as i32, (i % 3) as i32));
        }
        assert_eq!(pattern_from_stroke(&scribble, 24.0), "", "涂鸦不吐方向串");
    }

    #[test]
    fn stroke_pattern_never_exceeds_token_cap() {
        // 画了很多段：宁可判「不认识」，也别吐一串猜出来的方向。
        let mut pts = Vec::new();
        let dirs = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0), (1.0, 0.0), (0.0, 1.0)];
        let mut cur = (0.0f64, 0.0f64);
        for (dx, dy) in dirs {
            let next = (cur.0 + dx * 40.0, cur.1 + dy * 40.0);
            pts.extend(line(cur, next, 8));
            cur = next;
        }
        assert_eq!(pattern_from_stroke(&pts, 24.0), "");
    }

    #[test]
    fn template_normalizes_scale_and_ignores_offsets() {
        let mut a = line((0.0, 0.0), (40.0, 0.0), 10);
        a.extend(line((40.0, 0.0), (40.0, 40.0), 10).into_iter().skip(1));
        // 同一形状：放大 3 倍、整体平移。
        let mut b: Vec<(i32, i32)> = a.iter().map(|(x, y)| (x * 3 + 500, y * 3 + 700)).collect();
        let ta = template_from_points(&a).expect("模板");
        let tb = template_from_points(&b).expect("模板");
        assert_eq!(ta.len(), TEMPLATE_POINTS);
        assert!(ta.iter().flatten().all(|v| (0..=1000).contains(v)));
        let d = template_distance(&ta, &tb).expect("同点数");
        assert!(d < 0.05, "同形状缩放平移后应该几乎重合，实际 {d}");

        // 换一个形状：横向直线 vs L 形，必须明显更远。
        b.truncate(0);
        b.extend(line((500.0, 500.0), (900.0, 500.0), 20));
        let tc = template_from_points(&b).expect("模板");
        let dc = template_distance(&ta, &tc).expect("同点数");
        assert!(dc > TEMPLATE_MAX_DISTANCE, "不同形状不该落进阈值，实际 {dc}");
    }

    #[test]
    fn touchpad_and_right_drag_gestures_do_not_cross_match() {
        let mut cfg = Config::default();
        let mut right = gesture("D");
        right.name = Some("右键".into());
        let mut pad = gesture("D");
        pad.name = Some("触摸板".into());
        pad.trigger = Some(Key::CapsLock);
        cfg.gestures = vec![right, pad];

        let ctx = TriggerContext::empty();
        let hit = find_gesture_for(&cfg, "D", None, &ctx).expect("右键那条");
        assert_eq!(hit.name.as_deref(), Some("右键"));
        let hit = find_gesture_for(&cfg, "D", Some(Key::CapsLock), &ctx).expect("触摸板那条");
        assert_eq!(hit.name.as_deref(), Some("触摸板"));
        assert!(find_gesture_for(&cfg, "D", Some(Key::F9), &ctx).is_none());
        // 右键那条路径（`find_gesture`）不会命中触摸板条目。
        assert_eq!(
            find_gesture(&cfg, "D", &ctx).map(|g| g.is_touchpad()),
            Some(false)
        );
    }

    #[test]
    fn template_fallback_only_when_pattern_misses() {
        let stroke = line((0.0, 0.0), (90.0, 60.0), 18);
        let mut cfg = Config::default();
        let mut g = gesture("R");
        g.trigger = Some(Key::F9);
        g.template = template_from_points(&stroke);
        cfg.gestures = vec![g];
        let ctx = TriggerContext::empty();
        // 方向串没命中（R）→ 模板兜底命中同一条。
        let hit = find_gesture_by_template(&cfg, &stroke, Some(Key::F9), &ctx).expect("模板兜底");
        assert_eq!(hit.pattern, "R");
        // 来源不同不串味。
        assert!(find_gesture_by_template(&cfg, &stroke, None, &ctx).is_none());
    }

    #[test]
    fn gesture_trigger_and_template_roundtrip_in_json() {
        // 老配置（没有 trigger / template）原样读：仍是右键手势。
        let old: Gesture = serde_json::from_str(r#"{"pattern":"D","enabled":true,"actions":[]}"#)
            .expect("老配置要能读");
        assert_eq!(old.trigger, None);
        assert_eq!(old.template, None);

        // 新配置：触发键落成键名（与快捷键 / 改键的 from 同一套写法），模板落成点数组。
        let json = r#"{"pattern":"D","trigger":"CapsLock","enabled":true,"actions":[]}"#;
        let g: Gesture = serde_json::from_str(json).expect("触发键要能读");
        assert_eq!(g.trigger, Some(Key::CapsLock));
        let back = serde_json::to_string(&g).expect("要能写回");
        assert!(back.contains("\"trigger\":\"CapsLock\""), "{back}");

        // 键名拼错 = 解析失败（不静默降级成右键手势，否则会把右键抢走）。
        assert!(serde_json::from_str::<Gesture>(r#"{"pattern":"D","trigger":"NotAKey"}"#).is_err());

        let mut g = gesture("D");
        g.template = template_from_points(&line((0.0, 0.0), (60.0, 20.0), 12));
        let back = serde_json::to_string(&g).expect("模板要能写回");
        let again: Gesture = serde_json::from_str(&back).expect("模板要能读回");
        assert_eq!(again.template, g.template);
        assert_eq!(again.template.as_ref().map(|t| t.len()), Some(TEMPLATE_POINTS));
    }

    #[test]
    fn touchpad_enable_switch_does_not_take_over_right_drag() {
        let mut cfg = Config::default();
        // 圈选截图自带手势采集，先关掉它，只看「配了触摸板手势」这一项。
        cfg.settings.gesture_screenshot = false;
        cfg.gestures = vec![Gesture {
            pattern: "D".into(),
            trigger: Some(Key::CapsLock),
            enabled: true,
            ..Default::default()
        }];
        assert!(touchpad_gesture_enabled(&cfg));
        assert!(!gesture_enabled(&cfg), "只有触摸板手势时不该抢右键");
        assert!(!gesture_capture_enabled(&cfg));
        cfg.gestures[0].enabled = false;
        assert!(!touchpad_gesture_enabled(&cfg));
    }
}
