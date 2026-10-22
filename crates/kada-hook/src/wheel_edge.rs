//! 边缘滚动（EXT-11）：光标贴在屏幕四边时，滚轮把「哪条边」交给壳层。
//!
//! 钩子回调里只做**纯坐标状态判断**（比对屏幕矩形与光标点、维护「本次停留已触发」），
//! 不建窗、不做 IO、不执行动作。真正执行由壳层在出口回调里开线程做（同 [`crate::hotcorner`]）。
//!
//! 去重口径：一次停留（贴边期间）只上报一次；**停留不重复**；离开边缘超过
//! [`REARM_DISTANCE`] 像素后才重新武装。`SIZE` 是「离屏幕边多少像素算贴边」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 触发区宽度（像素）：光标离屏幕边这么近就算贴边。
pub const SIZE: i32 = 3;
/// 重新武装距离（像素）：光标离开边缘超过它才算「真的离开」。
pub const REARM_DISTANCE: i32 = 24;

/// 屏幕四条边。优先级（角上同时命中时）：上 → 下 → 左 → 右。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Edge::Top,
            1 => Edge::Bottom,
            2 => Edge::Left,
            _ => Edge::Right,
        }
    }
}

/// 边配置：总闸 + 四边开关 + 显示器矩形（屏幕物理坐标，含任务栏区域）。
///
/// 顺序固定为上 / 下 / 左 / 右；矩形顺序 = 壳层枚举显示器的顺序。
#[derive(Clone, Debug, Default)]
struct Config {
    enabled: bool,
    edges: [bool; 4],
    /// `(left, top, right, bottom)`（right / bottom 为开区间，同 Windows 的 RECT）。
    monitors: Vec<(i32, i32, i32, i32)>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG: Mutex<Option<Config>> = Mutex::new(None);
/// 当前所在的边（`monitor_index * 4 + edge_index`）；`None` = 不在触发区 / 待重新武装。
static ACTIVE: Mutex<Option<usize>> = Mutex::new(None);

/// 出口（壳层注册）。**回调在钩子线程上跑**，壳层要自己开线程做后续。
pub type EdgeSink = Arc<dyn Fn(Edge) + Send + Sync>;
static SINK: Mutex<Option<EdgeSink>> = Mutex::new(None);

/// 壳层按配置推送：总闸 + 四边开关 + 当前显示器矩形。
pub fn configure(enabled: bool, edges: [bool; 4], monitors: Vec<(i32, i32, i32, i32)>) {
    ENABLED.store(enabled, Ordering::Relaxed);
    *ACTIVE.lock().unwrap() = None;
    *CONFIG.lock().unwrap() = Some(Config { enabled, edges, monitors });
}

/// 注册出口（`None` = 取消注册）。
pub fn set_sink(sink: Option<EdgeSink>) {
    *SINK.lock().unwrap() = sink;
}

/// 清空状态（钩子重装 / 关开关时用）。
pub fn reset() {
    *ACTIVE.lock().unwrap() = None;
}

/// 是否启用（壳层 / 测试自检用）。
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 光标移动：离开当前边超过 [`REARM_DISTANCE`] 就重新武装。
///
/// 只维护状态、不上报——边缘滚动靠滚轮事件触发，移动本身什么也不做。
pub fn on_move(pt: (i32, i32)) {
    if !enabled() {
        return;
    }
    let Some(cfg) = CONFIG.lock().unwrap().clone() else { return };
    let mut active = ACTIVE.lock().unwrap();
    if let Some(i) = *active {
        if !within(&cfg, i, pt, REARM_DISTANCE) {
            *active = None;
        }
    }
}

/// 滚轮事件：本次停留还没触发、且光标贴边 → 返回这条边并上报一次。
///
/// 已经触发过的这次停留一律返回 `None`（停留不重复）；离开够远后由 [`on_move`] 重新武装。
pub fn on_wheel(pt: (i32, i32)) -> Option<Edge> {
    if !enabled() {
        return None;
    }
    let cfg = CONFIG.lock().unwrap().clone()?;
    if !cfg.enabled || cfg.monitors.is_empty() {
        return None;
    }

    let mut active = ACTIVE.lock().unwrap();
    if let Some(i) = *active {
        if within(&cfg, i, pt, REARM_DISTANCE) {
            return None;
        }
        *active = None;
    }
    for (mi, rect) in cfg.monitors.iter().enumerate() {
        for ei in 0..4 {
            if !cfg.edges[ei] {
                continue;
            }
            if within_rect(*rect, Edge::from_index(ei), pt, SIZE) {
                *active = Some(mi * 4 + ei);
                let sink = SINK.lock().unwrap().clone();
                drop(active);
                let edge = Edge::from_index(ei);
                if let Some(sink) = sink {
                    sink(edge);
                }
                return Some(edge);
            }
        }
    }
    None
}

/// 已记录的边 `i` 是否仍在重新武装距离内（不在触发区内，但还没算离开）。
fn within(cfg: &Config, i: usize, pt: (i32, i32), distance: i32) -> bool {
    match cfg.monitors.get(i / 4) {
        Some(rect) => within_rect(*rect, Edge::from_index(i % 4), pt, distance),
        None => false,
    }
}

/// `pt` 是否在显示器某条边附近 `distance` 像素内。
///
/// right / bottom 是开区间，故用 `-1`（与光标可取的最大坐标一致）。
fn within_rect(rect: (i32, i32, i32, i32), edge: Edge, pt: (i32, i32), distance: i32) -> bool {
    match edge {
        Edge::Top => pt.1 - rect.1 <= distance && pt.1 >= rect.1,
        Edge::Bottom => (rect.3 - 1) - pt.1 <= distance && pt.1 < rect.3,
        Edge::Left => pt.0 - rect.0 <= distance && pt.0 >= rect.0,
        Edge::Right => (rect.2 - 1) - pt.0 <= distance && pt.0 < rect.2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态是全局静态，几件事必须串在一个测试里（同 `hotcorner` 的测试口径）。
    #[test]
    fn wheel_edge_fires_once_and_rearms_after_leaving() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        set_sink(Some(Arc::new(move |e: Edge| g.lock().unwrap().push(e))));
        let monitor = vec![(0, 0, 1920, 1080)];

        // 关着 = 完全不动。
        configure(false, [true; 4], monitor.clone());
        assert_eq!(on_wheel((0, 0)), None);

        configure(true, [true, false, false, false], monitor);
        // 贴上边滚轮 → 触发一次。
        assert_eq!(on_wheel((500, 1)), Some(Edge::Top));
        // 停留不重复（继续滚也没用）。
        assert_eq!(on_wheel((500, 0)), None);
        // 只移开一点（还在重新武装距离内）也不重复。
        on_move((500, 10));
        assert_eq!(on_wheel((500, 1)), None);
        // 离开够远、再贴回来 → 重新触发。
        on_move((500, 100));
        assert_eq!(on_wheel((500, 1)), Some(Edge::Top));

        // 没启用的边不触发。
        assert_eq!(on_wheel((1919, 1079)), None);
        assert_eq!(got.lock().unwrap().len(), 2);

        // 四条边都认得（右 / 下边界是开区间）。
        configure(true, [false, true, true, true], vec![(0, 0, 1920, 1080)]);
        assert_eq!(on_wheel((1919, 500)), Some(Edge::Right));
        on_move((1500, 500));
        assert_eq!(on_wheel((1919, 500)), Some(Edge::Right));
        on_move((1500, 500));
        assert_eq!(on_wheel((500, 1079)), Some(Edge::Bottom));
        on_move((500, 500));
        assert_eq!(on_wheel((0, 500)), Some(Edge::Left));

        // 多显示器：四边配置应用到每台显示器。
        configure(true, [true, false, false, false], vec![(0, 0, 100, 100), (100, 0, 300, 100)]);
        assert_eq!(on_wheel((50, 50)), None, "屏幕中间不贴边");
        assert_eq!(on_wheel((101, 1)), Some(Edge::Top), "第二台显示器的上边");
        assert_eq!(got.lock().unwrap().len(), 7);

        set_sink(None);
        configure(false, [false; 4], Vec::new());
    }
}
