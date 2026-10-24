//! 鼠标手势的**采集**（规划 7.2-MG）：平台无关的状态机 + 轨迹出口。
//!
//! 钩子回调里只做三件事：记点、判阈值、返回「这一下要不要吞」。**识别不在这里做**
//! （识别在壳层，`kada_core::gesture` 是纯函数）——钩子回调超会被系统静默摘掉，
//! 里面绝不能有耗时操作。
//!
//! 吞放铁律：右键按下先吞（否则菜单已经弹了），抬起时把轨迹交给出口；**没匹配上手势
//! 由壳层回放一次完整右键**（按下-抬起配对），不把用户的右键吞掉。
//!
//! 圈选截图（EXT-20）是**右键按住期间点左键**才确认：只画圈不截、右键照常回放。确认了就把
//! 点击那一刻的包围盒放进 [`Stroke`]，右键抬起时一并交给出口。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// 一次手势的轨迹。
#[derive(Clone, Debug, Default)]
pub struct Stroke {
    /// 轨迹点（按下点 → 移动点 → 抬起点），屏幕坐标、y 向下。
    pub points: Vec<(i32, i32)>,
    /// 从按下到抬起的毫秒数（超时判定用）。
    pub elapsed_ms: u64,
    /// 圈选截图的包围盒（屏幕坐标，左 / 上 / 右 / 下）：`Some` = 这次手势点了左键确认。
    /// 取**点左键那一刻**的包围盒，之后右键还没松时的抖动不再改变选区。
    pub lasso: Option<(i32, i32, i32, i32)>,
}

/// 轨迹出口（壳层注册）。**回调在钩子线程上跑**，壳层要自己开线程做后续。
///
/// 用 `Arc` 包裹：出口要能**反复**取用（每次右键抬起都要交给它），不能像 `Box` 那样
/// 取走就没了——取走的后果是「第一次右键之后所有手势都不再上报」。
pub type StrokeSink = Arc<dyn Fn(Stroke) + Send + Sync>;

/// 手势是否启用（壳层按配置推）。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 轨迹出口（`None` = 没人接手，抬起的右键照旧吞掉但没人处理）。
static SINK: Mutex<Option<StrokeSink>> = Mutex::new(None);

/// 采集状态机。
#[derive(Default)]
struct Tracker {
    /// 右键已被我们吞下（按下的那一刻置位，抬起时清掉）。
    down: bool,
    /// 按下时刻（超时判定用）。
    start: Option<Instant>,
    /// 轨迹点。
    points: Vec<(i32, i32)>,
    /// 上一个记下的点（抽稀用）。
    last: (i32, i32),
    /// 这次手势里左键确认过的圈选矩形（点左键那一刻定下来）。
    lasso: Option<(i32, i32, i32, i32)>,
    /// 确认时那一记左键被吞了，它的抬起也要连续吞掉（不然下层窗口收到一个孤立的 up）。
    left_swallowed: bool,
}

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker {
    down: false,
    start: None,
    points: Vec::new(),
    last: (0, 0),
    lasso: None,
    left_swallowed: false,
});

/// 轨迹点数上限：长轨迹 + 高频移动会吃内存，方向量化用不到这么密。
const MAX_POINTS: usize = 512;
/// 相邻两点的最小距离（像素）：小于它不记点（手抖）。
const MIN_STEP: i32 = 2;

/// 开/关手势采集。关掉时右键完全放行（回到系统行为）。
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        reset();
    }
}

/// 手势采集是否开着。
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 注册轨迹出口（`None` = 取消注册）。
pub fn set_sink(sink: Option<StrokeSink>) {
    *SINK.lock().unwrap() = sink;
}

/// 清空采集状态（关开关 / 钩子重装时用，避免残留半截轨迹）。
pub fn reset() {
    let mut t = TRACKER.lock().unwrap();
    t.down = false;
    t.start = None;
    t.points.clear();
    t.last = (0, 0);
    t.lasso = None;
    t.left_swallowed = false;
}

/// 当前是否正在画（右键按住中）。浮层按这个开关显示 / 收起。
pub fn drawing() -> bool {
    enabled() && TRACKER.lock().unwrap().down
}

/// 取一份当前轨迹快照（屏幕坐标、y 向下）。浮层预览用；**不要**在钩子回调里调。
pub fn points() -> Vec<(i32, i32)> {
    if !enabled() {
        return Vec::new();
    }
    TRACKER.lock().unwrap().points.clone()
}

/// 右键按下：返回 `true` = 这一下应当被吞掉。
pub fn on_right_down(pt: (i32, i32)) -> bool {
    if !enabled() {
        return false;
    }
    let mut t = TRACKER.lock().unwrap();
    t.down = true;
    t.start = Some(Instant::now());
    t.points.clear();
    t.points.push(pt);
    t.last = pt;
    t.lasso = None;
    t.left_swallowed = false;
    true
}

/// 鼠标移动：手势进行中才记点（移动本身一律放行）。
pub fn on_move(pt: (i32, i32)) {
    if !enabled() {
        return;
    }
    let mut t = TRACKER.lock().unwrap();
    if !t.down {
        return;
    }
    let dx = (pt.0 - t.last.0) as i64;
    let dy = (pt.1 - t.last.1) as i64;
    if (dx * dx + dy * dy) < (MIN_STEP * MIN_STEP) as i64 {
        return;
    }
    t.points.push(pt);
    t.last = pt;
    if t.points.len() > MAX_POINTS {
        let drop = t.points.len() / 2;
        t.points.drain(0..drop);
    }
}

/// 左键按下：右键按住期间、且当前轨迹已经绕成一个圈 = 确认圈选截图。
///
/// 返回 `true` = 这一记左键要吞掉（它只是「确认」按钮，不该点到底下的窗口）。**只画圈不点
/// 左键不截图**——那只是普通右键拖动，右键由壳层原样回放。
pub fn on_left_down(pt: (i32, i32)) -> bool {
    if !enabled() {
        return false;
    }
    let mut t = TRACKER.lock().unwrap();
    if !t.down || t.lasso.is_some() {
        return false;
    }
    let dx = (pt.0 - t.last.0) as i64;
    let dy = (pt.1 - t.last.1) as i64;
    if (dx * dx + dy * dy) >= (MIN_STEP * MIN_STEP) as i64 {
        t.points.push(pt);
        t.last = pt;
    }
    let Some(rect) = kada_core::lasso_bbox(&t.points) else {
        return false;
    };
    t.lasso = Some(rect);
    t.left_swallowed = true;
    true
}

/// 左键抬起：确认用的那一记左键要连抬起一起吞掉（吞了 down 不吞 up，下层会收到孤立 up）。
pub fn on_left_up() -> bool {
    if !enabled() {
        return false;
    }
    let mut t = TRACKER.lock().unwrap();
    std::mem::take(&mut t.left_swallowed)
}

/// 右键抬起：返回 `true` = 这一下应当被吞掉（轨迹已交给出口回调）。
pub fn on_right_up(pt: (i32, i32)) -> bool {
    if !enabled() {
        return false;
    }
    let (stroke, sink) = {
        let mut t = TRACKER.lock().unwrap();
        if !t.down {
            return false;
        }
        t.down = false;
        t.left_swallowed = false;
        let dx = (pt.0 - t.last.0) as i64;
        let dy = (pt.1 - t.last.1) as i64;
        if (dx * dx + dy * dy) >= (MIN_STEP * MIN_STEP) as i64 {
            t.points.push(pt);
            t.last = pt;
        }
        let elapsed_ms = t
            .start
            .take()
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let stroke = Stroke {
            points: std::mem::take(&mut t.points),
            elapsed_ms,
            lasso: t.lasso.take(),
        };
        // 出口**只借用不取走**（`Arc` 克隆）：取走等于「第一次右键之后所有手势都不再上报」。
        let sink = SINK.lock().unwrap().clone();
        (stroke, sink)
    };
    // 出口回调在锁外调（它可能要开线程、要读配置）。
    if let Some(sink) = sink {
        sink(stroke);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 采集状态是**全局静态**（钩子线程与测试共用），所以这几件事必须串在**一个**测试里：
    /// 拆成多个 `#[test]` 会并行跑、`set_enabled` 互相踩，测出来的结果没有意义。
    #[test]
    fn right_button_gestures() {
        // 关着 = 右键完全放行（回到系统行为）。
        set_enabled(false);
        assert!(!on_right_down((0, 0)));
        assert!(!on_right_up((0, 0)));

        set_enabled(true);
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        set_sink(Some(Arc::new(move |s: Stroke| {
            g.lock().unwrap().push(s);
        })));

        // 一次完整手势：按下-移动-抬起，全程吞掉，轨迹交给出口。
        assert!(on_right_down((10, 10)));
        on_move((40, 10));
        on_move((80, 10));
        assert!(on_right_up((80, 10)));
        {
            let strokes = got.lock().unwrap();
            assert_eq!(strokes.len(), 1);
            assert_eq!(strokes[0].points.first(), Some(&(10, 10)));
            assert_eq!(strokes[0].points.last(), Some(&(80, 10)));
            assert_eq!(strokes[0].lasso, None, "没点左键 = 不截图");
        }

        // 圈选截图：右键按住画完圈，点左键才确认；只画圈不点 = 不截图。
        assert!(on_right_down((100, 100)));
        let circle: Vec<(i32, i32)> = (0..=16)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / 16.0;
                ((140.0 + 40.0 * a.cos()) as i32, (140.0 + 40.0 * a.sin()) as i32)
            })
            .collect();
        for p in &circle[1..] {
            on_move(*p);
        }
        // 左键离圈心很远（不是圈上）也能确认——判断看的是已经画出的轨迹。
        assert!(on_left_down((100, 100)), "圈 + 左键 = 确认");
        assert!(on_left_up(), "确认用的左键连抬起一起吞");
        assert!(!on_left_up(), "只吞一次");
        assert!(!on_left_down((100, 100)), "已确认后再点左键不再吃");
        // 右键还没松时抖一下，选区不变。
        on_move((200, 200));
        assert!(on_right_up((200, 200)));
        {
            let strokes = got.lock().unwrap();
            let lasso = strokes.last().unwrap().lasso.expect("确认后要带上包围盒");
            assert_eq!(lasso, (100, 100, 180, 180));
        }

        // 只画圈不点左键：右键照常交给出口，但没有包围盒（壳层回放右键）。
        assert!(on_right_down((400, 400)));
        for p in &circle[1..] {
            on_move((p.0 + 300, p.1 + 300));
        }
        assert!(on_right_up((700, 700)));
        assert_eq!(got.lock().unwrap().last().unwrap().lasso, None);

        // 出口必须**反复**可用：取走（`take`）的实现会让「第一次右键之后所有手势都不再上报」，
        // 而第一次右键往往只是普通点击——用户看到的就是「手势完全不生效」。
        for i in 0..5 {
            assert!(on_right_down((i, 0)));
            on_move((i + 30, 0));
            assert!(on_right_up((i + 30, 0)));
        }
        assert_eq!(got.lock().unwrap().len(), 8);

        set_sink(None);
        set_enabled(false);
    }
}
