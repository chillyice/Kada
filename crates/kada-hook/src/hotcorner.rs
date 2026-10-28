//! 触发角（EXT-10）：光标碰到屏幕四角时把「哪个角」交给壳层。
//!
//! 钩子回调里只做**纯坐标状态判断**（比对屏幕矩形与光标点、维护「已触发 / 待重新武装」），
//! 不建窗、不做 IO、不执行动作。真正执行由壳层在出口回调里开线程做（同 [`crate::gesture`]）。
//!
//! 去重口径：进入触发区只上报一次；**停留不重复**；离开角落超过 [`REARM_DISTANCE`] 像素后
//! 才重新武装（小抖动不算离开）。`SIZE` 是「角点附近多少像素算碰到角」。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 触发区半径（像素）：光标离角点这么近就算碰到角。
pub const SIZE: i32 = 3;
/// 重新武装距离（像素）：光标离开角点超过它才算「真的离开了这个角」。
pub const REARM_DISTANCE: i32 = 24;
/// 防误触复查的容差（像素）：等够停留时长后再看光标还在不在角上——等待期间手会轻微抖，
/// 按 3px 的触发区卡就永远等不到，给到半个重新武装距离。
pub const DWELL_TOLERANCE: i32 = 12;

/// 四个角。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Corner::TopLeft,
            1 => Corner::TopRight,
            2 => Corner::BottomLeft,
            _ => Corner::BottomRight,
        }
    }
}

/// 角配置：总闸 + 四角开关 + 显示器矩形（屏幕物理坐标，含任务栏区域）。
///
/// 顺序固定为左上 / 右上 / 左下 / 右下；矩形顺序 = 壳层枚举显示器的顺序。
#[derive(Clone, Debug, Default)]
struct Config {
    enabled: bool,
    corners: [bool; 4],
    /// `(left, top, right, bottom)`（right / bottom 为开区间，同 Windows 的 RECT）。
    monitors: Vec<(i32, i32, i32, i32)>,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIG: Mutex<Option<Config>> = Mutex::new(None);
/// 当前所在的角（`monitor_index * 4 + corner_index`）；`None` = 不在任何触发区 / 待重新武装。
static ACTIVE: Mutex<Option<usize>> = Mutex::new(None);
/// 钩子最近一次记录的光标位置：防误触（停留判定）复查用，免得再向平台查一次光标。
static LAST_POINT: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// 出口（壳层注册）。**回调在钩子线程上跑**，壳层要自己开线程做后续。
pub type CornerSink = Arc<dyn Fn(Corner) + Send + Sync>;
static SINK: Mutex<Option<CornerSink>> = Mutex::new(None);

/// 壳层按配置推送：总闸 + 四角开关 + 当前显示器矩形。
///
/// 值没变也照常覆盖（壳层只在变化时调）；关闭时清空状态，右键以外的一切行为不变。
pub fn configure(enabled: bool, corners: [bool; 4], monitors: Vec<(i32, i32, i32, i32)>) {
    ENABLED.store(enabled, Ordering::Relaxed);
    *ACTIVE.lock().unwrap() = None;
    *CONFIG.lock().unwrap() = Some(Config { enabled, corners, monitors });
}

/// 注册出口（`None` = 取消注册）。
pub fn set_sink(sink: Option<CornerSink>) {
    *SINK.lock().unwrap() = sink;
}

/// 清空状态（钩子重装 / 关开关时用）。
pub fn reset() {
    *ACTIVE.lock().unwrap() = None;
    *LAST_POINT.lock().unwrap() = None;
}

/// 是否启用（壳层 / 测试自检用）。
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 光标移动：命中一个**刚刚进入**的角就返回它。
///
/// 停留不重复（`ACTIVE` 不变就不上报）；离开超过 [`REARM_DISTANCE`] 才清掉 `ACTIVE`。
pub fn on_move(pt: (i32, i32)) -> Option<Corner> {
    if !enabled() {
        return None;
    }
    *LAST_POINT.lock().unwrap() = Some(pt);
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
        for ci in 0..4 {
            if !cfg.corners[ci] {
                continue;
            }
            if within_rect(*rect, Corner::from_index(ci), pt, SIZE) {
                *active = Some(mi * 4 + ci);
                let sink = SINK.lock().unwrap().clone();
                drop(active);
                let corner = Corner::from_index(ci);
                if let Some(sink) = sink {
                    sink(corner);
                }
                return Some(corner);
            }
        }
    }
    None
}

/// 已记录的角 `i` 是否仍在重新武装距离内（不在触发区内，但还没算离开）。
fn within(cfg: &Config, i: usize, pt: (i32, i32), distance: i32) -> bool {
    match cfg.monitors.get(i / 4) {
        Some(rect) => within_rect(*rect, Corner::from_index(i % 4), pt, distance),
        None => false,
    }
}

/// 防误触复查：等够停留时长后，光标是否还在这个角附近（[`DWELL_TOLERANCE`] 内）。
///
/// 等待期间用户可能只是**扫过**角落（关窗口、拖滚动条），那就不能触发。壳层拿不到光标，
/// 所以由钩子最近一次记录的位置判断；多显示器时只要有一台的这个角还贴着就算。
pub fn still_near(corner: Corner) -> bool {
    if !enabled() {
        return false;
    }
    let Some(pt) = *LAST_POINT.lock().unwrap() else {
        return false;
    };
    let Some(cfg) = CONFIG.lock().unwrap().clone() else {
        return false;
    };
    cfg.enabled
        && cfg
            .monitors
            .iter()
            .any(|rect| within_rect(*rect, corner, pt, DWELL_TOLERANCE))
}

/// `pt` 是否在角点附近 `distance` 像素内。
///
/// 角点取显示器的四角；right / bottom 是开区间，故用 `-1`（与光标可取的最大坐标一致）。
fn within_rect(rect: (i32, i32, i32, i32), corner: Corner, pt: (i32, i32), distance: i32) -> bool {
    let (x, y) = match corner {
        Corner::TopLeft => (rect.0, rect.1),
        Corner::TopRight => (rect.2 - 1, rect.1),
        Corner::BottomLeft => (rect.0, rect.3 - 1),
        Corner::BottomRight => (rect.2 - 1, rect.3 - 1),
    };
    (pt.0 - x).abs() <= distance && (pt.1 - y).abs() <= distance
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态是全局静态，几件事必须串在一个测试里（同 `gesture` 的测试口径）。
    #[test]
    fn hot_corner_fires_once_and_rearms_after_leaving() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        set_sink(Some(Arc::new(move |c: Corner| g.lock().unwrap().push(c))));
        let monitor = vec![(0, 0, 1920, 1080)];

        // 关着 = 完全不动。
        configure(false, [true; 4], monitor.clone());
        assert_eq!(on_move((0, 0)), None);

        configure(true, [true, false, false, false], monitor);
        // 碰到左上角 → 触发一次。
        assert_eq!(on_move((1, 1)), Some(Corner::TopLeft));
        // 停留不重复。
        assert_eq!(on_move((2, 2)), None);
        assert_eq!(on_move((0, 0)), None);
        // 只离开一点（还在重新武装距离内）也不重复。
        assert_eq!(on_move((10, 10)), None);
        assert_eq!(on_move((1, 1)), None);
        // 离开够远再回来 → 重新触发。
        assert_eq!(on_move((100, 100)), None);
        assert_eq!(on_move((1, 1)), Some(Corner::TopLeft));

        // 没启用的角不触发。
        assert_eq!(on_move((1919, 1079)), None);
        assert_eq!(got.lock().unwrap().len(), 2);

        // 重配（换角）会清状态：右上角启用后立刻能触发。
        *ACTIVE.lock().unwrap() = None;
        configure(true, [false, true, false, false], vec![(0, 0, 1920, 1080)]);
        assert_eq!(on_move((1919, 1)), Some(Corner::TopRight));

        // 多显示器：四角配置应用到每台显示器。
        configure(true, [true, false, false, false], vec![(0, 0, 100, 100), (100, 0, 300, 100)]);
        assert_eq!(on_move((50, 50)), None, "屏幕中间不在任何角");
        assert_eq!(on_move((101, 1)), Some(Corner::TopLeft), "第二台显示器的左上角");
        assert_eq!(got.lock().unwrap().len(), 4);

        // 防误触复查：等够停留时长后光标还在角附近（容差内抖动也算），走远了就不算。
        assert!(still_near(Corner::TopLeft), "光标还贴着第二台显示器的左上角");
        on_move((108, 8));
        assert!(still_near(Corner::TopLeft), "容差内抖动仍算贴着");
        on_move((300, 100));
        assert!(!still_near(Corner::TopLeft), "走远了不再算贴着");
        configure(false, [false; 4], Vec::new());
        assert!(!still_near(Corner::TopLeft), "关掉后一律不算");

        set_sink(None);
        configure(false, [false; 4], Vec::new());
    }
}
