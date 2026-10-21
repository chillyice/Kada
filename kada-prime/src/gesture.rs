use crate::config::{Config, Gesture};
use std::time::Instant;

/// 8 方向（屏幕坐标，y 向下）。
const DIRECTIONS: [&str; 8] = ["R", "DR", "D", "DL", "L", "UL", "U", "UR"];

pub fn dist(a: (i32, i32), b: (i32, i32)) -> i32 {
    let dx = (b.0 - a.0) as i64;
    let dy = (b.1 - a.1) as i64;
    ((dx * dx + dy * dy) as f64).sqrt() as i32
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

/// 轨迹点 → 方向串。按 `min_segment` 抽稀，相邻同向合并。
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

/// 配置里的方向串归一化：支持 U/D/L/R、斜向两字母、箭头，以及 MouseInc 风格的
/// `UP` / `DOWN` / `UPLEFT` 这类单词。
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

pub fn find_gesture<'a>(cfg: &'a Config, pattern: &str) -> Option<&'a Gesture> {
    let pattern = normalize_pattern(pattern);
    if pattern.is_empty() {
        return None;
    }
    cfg.gestures
        .iter()
        .find(|g| g.enabled && normalize_pattern(&g.pattern) == pattern)
}

/// 手势状态机。放在 Runtime 的 Mutex 里，钩子回调只做加锁、追加点、判定阈值。
#[derive(Debug)]
pub struct State {
    pub right_down: bool,
    /// 已经越过起手距离，正在画。
    pub active: bool,
    /// 右键按住期间滚过滚轮（WheelSwitch），抬起时不再当手势。
    pub wheel_used: bool,
    /// 超时 / 被取消。
    pub cancelled: bool,
    pub start: (i32, i32),
    pub last: (i32, i32),
    pub last_move: Instant,
    pub points: Vec<(i32, i32)>,
    pub result: String,
    pub result_until: Option<Instant>,
    pub corner_inside: [bool; 4],
    pub last_corner_fire: Option<Instant>,
    pub last_edge_fire: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            right_down: false,
            active: false,
            wheel_used: false,
            cancelled: false,
            start: (0, 0),
            last: (0, 0),
            last_move: Instant::now(),
            points: Vec::new(),
            result: String::new(),
            result_until: None,
            corner_inside: [false; 4],
            last_corner_fire: None,
            last_edge_fire: None,
        }
    }
}

impl State {
    pub fn begin(&mut self, pt: (i32, i32)) {
        self.right_down = true;
        self.active = false;
        self.wheel_used = false;
        self.cancelled = false;
        self.start = pt;
        self.last = pt;
        self.last_move = Instant::now();
        self.points.clear();
        self.points.push(pt);
        self.result.clear();
        self.result_until = None;
    }

    /// 返回 true 表示这一刻刚好越过起手距离（调用方可据此显示轨迹层）。
    pub fn move_to(&mut self, pt: (i32, i32), start_distance: i32) -> bool {
        if !self.right_down || self.cancelled {
            return false;
        }
        self.last_move = Instant::now();
        if !self.active {
            if dist(self.start, pt) < start_distance.max(1) {
                return false;
            }
            self.active = true;
            self.points.clear();
            self.points.push(self.start);
        }
        if dist(self.last, pt) >= 2 {
            self.points.push(pt);
            self.last = pt;
            // 上限防长轨迹 + 高频移动吃内存；方向量化用不到这么密。
            if self.points.len() > 512 {
                self.points.drain(0..256);
            }
        }
        true
    }

    pub fn finish(&mut self, pt: (i32, i32)) {
        if self.right_down && dist(self.last, pt) >= 2 {
            self.points.push(pt);
            self.last = pt;
        }
        self.right_down = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(points: &[(i32, i32)]) -> Vec<(i32, i32)> {
        points.to_vec()
    }

    #[test]
    fn straight_directions() {
        assert_eq!(
            pattern_from_points(&line(&[(0, 0), (10, 0), (30, 0), (50, 0)]), 16),
            "R"
        );
        assert_eq!(
            pattern_from_points(&line(&[(0, 0), (0, 10), (0, 30), (0, 50)]), 16),
            "D"
        );
        assert_eq!(
            pattern_from_points(&line(&[(0, 0), (-10, 0), (-30, 0), (-50, 0)]), 16),
            "L"
        );
        assert_eq!(
            pattern_from_points(&line(&[(0, 0), (0, -10), (0, -30), (0, -50)]), 16),
            "U"
        );
    }

    #[test]
    fn multi_segment_pattern() {
        // 下 → 右 → 上 → 左（MouseInc 的四边形贴图手势）
        let pts = line(&[
            (0, 0),
            (0, 40),
            (40, 40),
            (40, 0),
            (0, 0),
        ]);
        assert_eq!(pattern_from_points(&pts, 16), "DRUL");
    }

    #[test]
    fn diagonal() {
        let pts = line(&[(0, 0), (20, 20), (40, 40), (60, 60)]);
        assert_eq!(pattern_from_points(&pts, 16), "DR");
    }

    #[test]
    fn jitter_does_not_add_noise() {
        let pts = line(&[(0, 0), (3, 1), (26, 0), (27, 1), (52, 0)]);
        assert_eq!(pattern_from_points(&pts, 16), "R");
    }

    #[test]
    fn normalization_accepts_words_and_arrows() {
        assert_eq!(normalize_pattern("down right up left"), "DRUL");
        assert_eq!(normalize_pattern("UP"), "U");
        assert_eq!(normalize_pattern("↓→↑←"), "DRUL");
        assert_eq!(normalize_pattern("ur"), "UR");
        assert_eq!(normalize_pattern("up-right"), "UR");
    }

    #[test]
    fn state_starts_after_distance() {
        let mut s = State::default();
        s.begin((0, 0));
        assert!(!s.move_to((5, 0), 22));
        assert!(!s.active);
        assert!(s.move_to((30, 0), 22));
        assert!(s.active);
        s.finish((50, 0));
        assert!(!s.right_down);
        assert!(s.points.len() >= 3);
    }
}
