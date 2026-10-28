//! 触摸板手势的**采集**（规划 7.2-MG 触摸板手势）：按住一个键，在触摸板上画，松键收尾。
//!
//! 数据来源是**普通鼠标移动**：单指在触摸板上滑动，三个平台都会合成成相对鼠标移动，
//! 所以这里不碰 HID、不用新权限、非精确触摸板也能用（代价：画的时候光标会跟着走，
//! 轨迹还带指针加速——加速是标量增益，方向不变，只有弧长分布会变）。
//!
//! 与右键手势（[`crate::gesture`]）的分工：
//! - 那边**吞右键**、没命中要回放；这边吞的是**用户指定的触发键**，它本来就只是
//!   「进入采集」的开关，没有输出，所以没命中什么都不用补；
//! - 采集状态机同样是平台无关的，识别在壳层（`kada_core::pattern_from_stroke`）。
//!
//! 钩子回调里只做「记点 / 判开关」，**不做识别、不碰 IO**：回调超时会被系统摘钩子。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::gesture::Stroke;
use crate::Key;

/// 轨迹出口（壳层注册）。回调在钩子线程上跑，壳层要自己开线程做后续。
/// 带上触发键：同一次轨迹按**哪个键**开采集，去匹配哪个来源的条目。
pub type StrokeSink = Arc<dyn Fn(Key, Stroke) + Send + Sync>;

/// 触摸板手势的闸门（壳层按配置推：总开关 + 有启用的触摸板手势）。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 轨迹出口（`None` = 没人接手，轨迹直接丢掉）。
static SINK: Mutex<Option<StrokeSink>> = Mutex::new(None);

#[derive(Default)]
struct Tracker {
    /// 正在采集（触发键按住中）。
    armed: bool,
    /// 采集由哪个键开启：只有**它**的抬起才收尾（别的键抬起不该把这次采集截断）。
    key: Option<Key>,
    /// 开始时刻（超时 / 展示用）。
    start: Option<Instant>,
    /// 轨迹点（屏幕坐标累计；Windows / macOS 是真实光标位置）。
    points: Vec<(i32, i32)>,
    /// 起笔位置（松键后把光标放回去）。
    anchor: Option<(i32, i32)>,
    /// 上一次记下轨迹的时刻（空闲超时判定：停手太久 = 这一笔不连贯，作废）。
    last_move: Option<Instant>,
    /// 已作废（Esc / 双指滚动 / 空闲超时）：还在等触发键抬起，但**不执行**。
    /// 保持 `armed` 是为了让抬起仍然被吞掉——按下被吞了、抬起放走，应用会收到一个孤立 keyup。
    voided: bool,
    /// 上一个记下的点（抽稀用）。
    last: (i32, i32),
}

static TRACKER: Mutex<Tracker> = Mutex::new(Tracker {
    armed: false,
    key: None,
    start: None,
    points: Vec::new(),
    anchor: None,
    last_move: None,
    voided: false,
    last: (0, 0),
});

/// 轨迹点数上限：长轨迹 + 高频移动会吃内存，方向量化用不到这么密。
const MAX_POINTS: usize = 512;
/// 相邻两点的最小距离（像素）：小于它不记点（手抖）。
const MIN_STEP: i32 = 1;
/// 停手多久算「这一笔不连贯」（松键时判）：触摸板上一笔正常几秒内画完，
/// 停 3 秒以上多半是走神 / 改主意了——按作废处理，别拿半截轨迹去执行动作。
const IDLE_LIMIT: Duration = Duration::from_millis(3000);

/// 本平台能不能采：目前只有 Windows 接了鼠标钩子的移动事件。
///
/// macOS 的 `CGEventTap` 也拿得到绝对坐标，Linux 的 evdev 只有相对位移——两者接进来
/// 是同一套接口（`on_move` 吃累计坐标），但**没真机验过手感之前不假装能用**：
/// 不支持时壳层直接不 arm（触发键照常输出，不是静默吞掉）。
pub fn supported() -> bool {
    cfg!(windows)
}

/// 开/关采集。关掉时清空状态（触发键立刻回到普通按键）。
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if !on {
        reset();
    }
}

/// 采集是否开着。
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 注册轨迹出口（`None` = 取消注册）。
pub fn set_sink(sink: Option<StrokeSink>) {
    *SINK.lock().unwrap() = sink;
}

/// 清空采集状态（钩子重装 / 前台切换 / 配置整份换掉：**缓冲里的丢弃、不回放**，
/// 与 `Engine::reset` 同一口径）。
pub fn reset() {
    let mut t = TRACKER.lock().unwrap();
    t.armed = false;
    t.key = None;
    t.start = None;
    t.points.clear();
    t.anchor = None;
    t.last_move = None;
    t.voided = false;
    t.last = (0, 0);
}

/// 当前是不是正在采集（浮层按这个开关显示 / 收起）。
pub fn armed() -> bool {
    enabled() && TRACKER.lock().unwrap().armed
}

/// 浮层预览用：采集期间（含已作废、等松键的那段）都算「在画」。
pub fn drawing() -> bool {
    armed()
}

/// 这次采集是不是 `key` 开的（壳层据此决定「这个键的重复按下 / 抬起要不要吞」）。
pub fn armed_with(key: Key) -> bool {
    let t = TRACKER.lock().unwrap();
    t.armed && t.key == Some(key)
}

/// 触发键按下：`true` = 这次按下归我们（壳层吞掉它）。
///
/// **只接受键盘键**（配置层已经挡掉鼠标键）：按住修饰键也没问题，它照常进 mods，
/// 只是不再产出自己的按键。
pub fn arm(key: Key) -> bool {
    if !enabled() {
        return false;
    }
    let mut t = TRACKER.lock().unwrap();
    if t.armed {
        // 已经有一次采集在跑：这一下不抢（否则两笔轨迹会混在一起）。
        return false;
    }
    t.armed = true;
    t.key = Some(key);
    t.start = Some(Instant::now());
    t.points.clear();
    t.anchor = None;
    t.last_move = None;
    t.voided = false;
    t.last = (0, 0);
    true
}

/// 鼠标移动：采集期间才记点（移动本身一律放行）。
pub fn on_move(pt: (i32, i32)) {
    if !enabled() {
        return;
    }
    let mut t = TRACKER.lock().unwrap();
    if !t.armed {
        return;
    }
    // 已作废（Esc / 双指）：这一笔不再收点——否则松手前继续滑动会把轨迹重新画回
    // 浮层上，看上去像「Esc 没生效」。作废只能靠松键结束。
    if t.voided {
        return;
    }
    if t.anchor.is_none() {
        // 起笔：第一个移动事件的位置就是光标锚点（松键后复位到这里）。
        t.anchor = Some(pt);
        t.points.push(pt);
        t.last = pt;
        t.last_move = Some(Instant::now());
        return;
    }
    let dx = (pt.0 - t.last.0) as i64;
    let dy = (pt.1 - t.last.1) as i64;
    if (dx * dx + dy * dy) < (MIN_STEP * MIN_STEP) as i64 {
        return;
    }
    t.points.push(pt);
    t.last = pt;
    t.last_move = Some(Instant::now());
    if t.points.len() > MAX_POINTS {
        let drop = t.points.len() / 2;
        t.points.drain(0..drop);
    }
}

/// 作废这次采集（Esc / 双指滚动）：轨迹丢掉、**不执行**，但仍然等触发键抬起。
///
/// 不在这里改成「没在采集」的原因见 [`Tracker::voided`]：按下被吞了，抬起必须也吞掉，
/// 否则应用会收到一个没有对应按下的 keyup。
pub fn cancel() {
    let mut t = TRACKER.lock().unwrap();
    if !t.armed {
        return;
    }
    t.voided = true;
    t.points.clear();
    t.last_move = None;
}

/// 起笔时记下的光标位置（松键后把光标放回去；`None` = 还没动过）。
pub fn anchor() -> Option<(i32, i32)> {
    TRACKER.lock().unwrap().anchor
}

/// 当前轨迹快照（浮层预览用；**不要**在钩子回调里调）。
pub fn points() -> Vec<(i32, i32)> {
    if !enabled() {
        return Vec::new();
    }
    TRACKER.lock().unwrap().points.clone()
}

/// 触发键抬起：`true` = 这次抬起归我们（壳层吞掉），并把轨迹交给出口。
///
/// 触发键抬起：`true` = 这次抬起归我们（壳层吞掉），并把轨迹交给出口。
///
/// **作废的（Esc / 双指 / 停手超过 [`IDLE_LIMIT`]）也照样回调出口**，只是带空点集——
/// 壳层要靠这次回调去收起轨迹浮层（不然浮层会一直挂在屏幕上）。
/// 出口在锁外调（它可能要开线程、要读配置）。
pub fn finish(key: Key) -> bool {
    if !enabled() {
        return false;
    }
    let (key, stroke, sink) = {
        let mut t = TRACKER.lock().unwrap();
        if !t.armed || t.key != Some(key) {
            return false;
        }
        t.armed = false;
        t.key = None;
        // 停手太久 = 这一笔不连贯（用户走神 / 改主意）：按作废处理。
        let idle = t
            .last_move
            .map(|m| m.elapsed() > IDLE_LIMIT)
            .unwrap_or(true);
        let voided = std::mem::take(&mut t.voided) || idle;
        let elapsed_ms = t
            .start
            .take()
            .map(|s| s.elapsed().as_millis() as u64)
            .unwrap_or(0);
        let mut points = std::mem::take(&mut t.points);
        if voided {
            points.clear();
        }
        t.last_move = None;
        let stroke = Stroke {
            points,
            elapsed_ms,
            lasso: None,
        };
        // 出口**只借用不取走**（`Arc` 克隆）：取走等于「第一次之后都不再上报」。
        let sink = SINK.lock().unwrap().clone();
        (key, stroke, sink)
    };
    if let Some(sink) = sink {
        sink(key, stroke);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 采集状态是**全局静态**（钩子线程与测试共用），所以这几件事必须串在**一个**测试里：
    /// 拆开并行跑会互相踩（同 `gesture` 的测试口径）。
    #[test]
    fn trackpad_capture_lifecycle() {
        set_enabled(false);
        assert!(!arm(Key::F9), "关着的时候不抢键");
        assert!(!finish(Key::F9));

        set_enabled(true);
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        set_sink(Some(Arc::new(move |k: Key, s: Stroke| {
            g.lock().unwrap().push((k, s));
        })));

        // 完整一次：按住 → 画 → 松开，轨迹交给出口。
        assert!(arm(Key::F9));
        assert!(armed());
        on_move((100, 100));
        on_move((140, 100));
        on_move((180, 100));
        assert_eq!(points().len(), 3);
        assert_eq!(anchor(), Some((100, 100)));
        assert!(finish(Key::F9));
        {
            let strokes = got.lock().unwrap();
            assert_eq!(strokes.len(), 1);
            assert_eq!(strokes[0].0, Key::F9, "轨迹要带上触发键");
            assert_eq!(strokes[0].1.points.first(), Some(&(100, 100)));
            assert_eq!(strokes[0].1.points.last(), Some(&(180, 100)));
            assert_eq!(strokes[0].1.lasso, None);
        }

        // 别的键抬起不收尾；同一次采集不会被第二个键接管。
        assert!(arm(Key::F9));
        assert!(!arm(Key::CapsLock), "采集进行中不换键");
        assert!(!finish(Key::CapsLock), "不是它开的，不归它收");
        assert!(finish(Key::F9));
        assert_eq!(got.lock().unwrap().len(), 2);

        // 作废（双指滚动 / Esc）：轨迹丢掉，但**仍在采集**（等松键），抬起照样吞掉并回调
        // 出口（带空点集——壳层要靠这次回调收起轨迹浮层）。
        assert!(arm(Key::F9));
        on_move((0, 0));
        on_move((60, 0));
        cancel();
        assert!(armed(), "作废后仍在采集，直到触发键抬起");
        assert!(points().is_empty(), "作废后轨迹清空（浮层不再画）");
        // 松键前继续滑动也不收点：否则轨迹会重新画回浮层，看上去像「Esc 没生效」。
        on_move((120, 0));
        assert!(points().is_empty(), "作废后不再收点");
        assert!(finish(Key::F9), "抬起仍归我们（按下被吞过）");
        {
            let strokes = got.lock().unwrap();
            assert_eq!(strokes.len(), 3, "作废也回调一次（为了收浮层）");
            assert!(strokes[2].1.points.is_empty(), "但点集是空的，不执行");
        }

        // 没动过就松手：空轨迹也交给出口（壳层按「太短」丢掉）。
        assert!(arm(Key::F9));
        assert!(finish(Key::F9));
        assert_eq!(got.lock().unwrap().len(), 4);
        assert!(got.lock().unwrap().last().unwrap().1.points.is_empty());

        // 停手超过 IDLE_LIMIT（默认 3 秒）再松键：这一笔不连贯 → 作废、不执行。
        // 直接把「最后一次移动」拨到 4 秒前，不真睡（测试要快）。
        assert!(arm(Key::F9));
        on_move((0, 0));
        on_move((90, 0));
        TRACKER.lock().unwrap().last_move = Some(Instant::now() - Duration::from_secs(4));
        assert!(finish(Key::F9));
        assert!(
            got.lock().unwrap().last().unwrap().1.points.is_empty(),
            "停手太久不执行"
        );

        // 关掉总闸：立刻清状态，触发键回到普通按键。
        assert!(arm(Key::F9));
        set_enabled(false);
        assert!(!armed());
        assert!(!finish(Key::F9));

        set_sink(None);
        set_enabled(false);
    }
}
