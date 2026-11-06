// 手势（T-05 从 lib.rs 搬出）：鼠标手势浮层与识别线程、触摸板手势、屏幕角动作、边缘滚动。
// 识别都在壳层做（钩子回调只记点），手势不进 `Key` 模型、不进 `Engine::step`（见 说明书 §5.9）。

use crate::*;

/// 手势浮层窗口的视觉参数（EXT-02），随预览 / 结果事件一起发给前端。
#[derive(Clone, Serialize)]
struct GestureVisual {
    trail_color: String,
    trail_width: i32,
    label_font_size: i32,
    label_offset: i32,
    fail_color: String,
}

#[derive(Clone, Serialize)]
struct GesturePreviewPayload {
    #[serde(flatten)]
    visual: GestureVisual,
    /// 窗口本地物理坐标（前端按 `scale` 换算成 CSS 像素）。
    points: Vec<(i32, i32)>,
    scale: f64,
}

#[derive(Clone, Serialize)]
struct GestureResultPayload {
    #[serde(flatten)]
    visual: GestureVisual,
    points: Vec<(i32, i32)>,
    scale: f64,
    matched: bool,
    label: String,
    pattern: String,
}

/// 结果展示序号：新一次手势 / 新结果会把它推进，旧的隐藏定时器据此让位。
static GESTURE_VISUAL_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 手势浮层前端是否已注册好 `gesture-preview` / `gesture-result` 监听器。
pub(crate) static GESTURE_OVERLAY_READY: AtomicBool = AtomicBool::new(false);

/// 触摸板手势的「录一笔」会话（设置页 → 鼠标操作编辑器）：`Some(触发键)` = 正在等一笔轨迹。
///
/// 它同时是一个**闸门覆盖**：新建第一条触摸板手势时，配置里还没有任何启用的条目，
/// `touchpad_gesture_enabled` 为假、采集根本开不了，所以录制期间必须强制开闸
/// （见 `spawn_engine_ticker` 里的 `trackpad_on` 与钩子入口的 `arm` 判定）。
pub(crate) static TRACKPAD_RECORD: Mutex<Option<Key>> = Mutex::new(None);

/// 正在录制的触发键（`None` = 没在录）。
pub(crate) fn trackpad_recording_key() -> Option<Key> {
    *TRACKPAD_RECORD.lock().unwrap()
}

/// 「录一笔」的结果：只回填编辑器，**不执行动作、不进执行记录**。
#[derive(Clone, Serialize)]
struct TrackpadRecordPayload {
    /// 识别出的方向串；空 = 这一笔太短 / 涂鸦 / 已作废（Esc / 双指 / 停手太久）。
    pattern: String,
    /// 归一化形状模板（[`kada_core::template_from_points`]）；点太少时为 `None`。
    template: Option<Vec<[i16; 2]>>,
    /// 这一笔记了多少个点（界面据此提示采样密度）。
    points: usize,
    elapsed_ms: u64,
}

/// 手势浮层窗口名（`index.html#gesture`）。
const GESTURE_WINDOW: &str = "gesture";

fn gesture_visual_of(s: &Settings) -> GestureVisual {
    GestureVisual {
        trail_color: s.gesture_trail_color.clone(),
        trail_width: s.gesture_trail_width,
        label_font_size: s.gesture_label_font_size,
        label_offset: s.gesture_label_offset,
        fail_color: s.gesture_fail_color.clone(),
    }
}

/// 按需创建手势轨迹浮层：铺满虚拟屏幕、透明、不激活、鼠标穿透。
///
/// **必须 `focusable(false)` + `set_ignore_cursor_events(true)`**：它能覆盖整个桌面，
/// 一旦能被激活就会抢前台 → `ResetWatch` 误判复位；鼠标穿透保证画手势时事件照常落到
/// 下面的窗口。同 toast / hud 的建窗约束，见 AGENTS §1.7。
fn ensure_gesture_overlay(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
        return Some(w);
    }
    GESTURE_OVERLAY_READY.store(false, Ordering::Relaxed);
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        GESTURE_WINDOW,
        tauri::WebviewUrl::App("index.html#gesture".into()),
    )
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .inner_size(1.0, 1.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建手势浮层失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    // 铺满虚拟屏幕（多显示器取并集；坐标用物理像素，与钩子轨迹同一口径）。
    let mut min = (i32::MAX, i32::MAX);
    let mut max = (i32::MIN, i32::MIN);
    if let Ok(monitors) = app.available_monitors() {
        for m in monitors {
            let p = m.position();
            let s = m.size();
            min.0 = min.0.min(p.x);
            min.1 = min.1.min(p.y);
            max.0 = max.0.max(p.x.saturating_add(s.width as i32));
            max.1 = max.1.max(p.y.saturating_add(s.height as i32));
        }
    }
    if min.0 < max.0 && min.1 < max.1 {
        let _ = w.set_position(tauri::PhysicalPosition::new(min.0, min.1));
        let _ = w.set_size(tauri::PhysicalSize::new(
            (max.0 - min.0) as u32,
            (max.1 - min.1) as u32,
        ));
    }
    Some(w)
}

/// 浮层窗口左上角（物理像素）与该窗口的缩放系数。
fn gesture_window_geom(app: &tauri::AppHandle) -> Option<((i32, i32), f64)> {
    let w = app.get_webview_window(GESTURE_WINDOW)?;
    let pos = w.outer_position().ok()?;
    let scale = w.scale_factor().ok()?;
    Some(((pos.x, pos.y), scale))
}

fn local_gesture_points(points: &[(i32, i32)], origin: (i32, i32)) -> Vec<(i32, i32)> {
    points
        .iter()
        .map(|p| (p.0 - origin.0, p.1 - origin.1))
        .collect()
}

/// 等手势浮层前端注册好监听器（上限 1 秒；前端加载失败也不至于卡死）。
fn wait_gesture_overlay_ready() {
    let deadline = Instant::now() + Duration::from_millis(1000);
    while !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 收起浮层，并让旧的隐藏定时器失效。
pub(crate) fn hide_gesture_overlay(app: &tauri::AppHandle) {
    GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
        let _ = w.hide();
    }
}

/// 「录一笔」收尾：识别这一笔 → 收起浮层 → 推给设置页。**不执行动作、不弹未匹配提示**。
///
/// 走的是与正式手势同一套识别（`pattern_from_stroke` / `template_from_points`），
/// 所以「录的时候是什么、跑的时候就是什么」——这也是录入界面唯一需要的保证。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn emit_trackpad_record(
    app: &tauri::AppHandle,
    cfg: &Arc<RwLock<Config>>,
    stroke: &kada_hook::gesture::Stroke,
) {
    let min_len = {
        let g = cfg.read().unwrap();
        // 与运行时（`handle_trackpad_stroke`）同一把尺子：触摸板用 `touchpad_start_distance`，
        // 不是右键手势的 `gesture_start_distance`——否则「录出来的」和「跑出来的」会不一样。
        g.settings.touchpad_start_distance as f64
    };
    let payload = TrackpadRecordPayload {
        pattern: kada_core::pattern_from_stroke(&stroke.points, min_len),
        template: kada_core::template_from_points(&stroke.points),
        points: stroke.points.len(),
        elapsed_ms: stroke.elapsed_ms,
    };
    hide_gesture_overlay(app);
    let _ = app.emit("trackpad-recorded", payload);
}

/// 展示一次手势结果（命中 = 动作名；未命中 = 失败色）。窗口隐藏交给延时线程。
fn show_gesture_result(
    app: &tauri::AppHandle,
    cfg: &Arc<RwLock<Config>>,
    points: &[(i32, i32)],
    matched: bool,
    label: String,
    pattern: String,
) {
    if points.len() < 2 {
        return;
    }
    if ensure_gesture_overlay(app).is_none() {
        return;
    }
    let Some((origin, scale)) = gesture_window_geom(app) else {
        return;
    };
    let visual = {
        let g = cfg.read().unwrap();
        gesture_visual_of(&g.settings)
    };
    let payload = GestureResultPayload {
        visual,
        points: local_gesture_points(points, origin),
        scale,
        matched,
        label,
        pattern,
    };
    let serial = GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        // 等前端注册好监听器（`gesture_overlay_ready`），否则事件会丢。
        if !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) {
            wait_gesture_overlay_ready();
        }
        if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) != serial {
            return;
        }
        if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
            let _ = w.show();
            let _ = w.emit("gesture-result", &payload);
        }
        std::thread::sleep(Duration::from_millis(900));
        if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) == serial {
            if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
                let _ = w.hide();
            }
        }
    });
}

/// 轨迹预览线程：按帧读钩子里的轨迹点，够起手距离才亮出浮层。
///
/// 钩子回调里只记点，**不往这里推**（回调超会被系统摘钩子）；这里轮询快照，
/// 只在点数变化时发事件（16ms 一帧，跟手不刷屏）。
///
/// **两条来源共用一个浮层**：右键手势（`kada_hook::gesture`）与触摸板手势
/// （`kada_hook::trackpad`，按住触发键采集）。两者不会同时进行——前者要按住右键、
/// 后者要按住触发键，所以按「先右键、后触摸板」取一个即可。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_gesture_overlay(app: tauri::AppHandle, cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut was_drawing = false;
        let mut shown = false;
        let mut suppressed = false;
        let mut last_len = usize::MAX;
        let mut preloaded = false;
        loop {
            // 采集期间快轮询（跟手），空闲时慢轮询：触摸板手势也要算进来，
            // 否则按住触发键画的时候只有 8 帧/秒，轨迹一顿一顿的。
            let interval = if kada_hook::gesture::enabled() || kada_hook::trackpad::enabled() {
                16
            } else {
                120
            };
            std::thread::sleep(Duration::from_millis(interval));
            let (drawing, points, from_touchpad) = if kada_hook::gesture::drawing() {
                (true, kada_hook::gesture::points(), false)
            } else if kada_hook::trackpad::drawing() {
                (true, kada_hook::trackpad::points(), true)
            } else {
                (false, Vec::new(), false)
            };
            if !drawing {
                was_drawing = false;
                shown = false;
                suppressed = false;
                last_len = usize::MAX;
                // 提前把浮层建好并等前端就绪：冷启动第一笔手势如果边画边建窗，
                // WebView 加载要几百毫秒，轨迹还没画出来就结束了。
                if !preloaded {
                    let on = {
                        let g = cfg.read().unwrap();
                        (g.settings.experimental && gesture_capture_enabled(&g))
                            || (g.settings.touchpad
                                && g.settings.touchpad_overlay
                                && touchpad_gesture_enabled(&g)
                                && kada_hook::trackpad::supported())
                            || trackpad_recording_key().is_some()
                    };
                    if on {
                        preloaded = ensure_gesture_overlay(&app).is_some();
                    }
                }
                continue;
            }
            if !was_drawing {
                was_drawing = true;
                shown = false;
                suppressed = false;
                last_len = usize::MAX;
            }
            if suppressed {
                continue;
            }
            if points.len() < 2 {
                continue;
            }
            let recording = trackpad_recording_key().is_some();
            let (visual, start_distance, on, overlay, exclude, exclude_apps) = {
                let g = cfg.read().unwrap();
                (
                    gesture_visual_of(&g.settings),
                    // 两条路的「够长才亮」各用自己那个值：右键手势是「起点之后开始画」，
                    // 触摸板是「这一笔至少多长」。
                    if from_touchpad {
                        g.settings.touchpad_start_distance
                    } else {
                        g.settings.gesture_start_distance
                    },
                    (if from_touchpad {
                        g.settings.touchpad
                            && touchpad_gesture_enabled(&g)
                            && kada_hook::trackpad::supported()
                    } else {
                        g.settings.experimental && gesture_capture_enabled(&g)
                    }) || recording,
                    // 浮层开关只影响触摸板这条来源（右键手势没有单独开关）。
                    !from_touchpad || g.settings.touchpad_overlay,
                    // 排除列表也按来源取：两条路各有自己的名单，别混用。
                    if from_touchpad {
                        g.settings.touchpad_exclude.clone()
                    } else {
                        g.settings.gesture_exclude.clone()
                    },
                    g.settings.exclude_apps.clone(),
                )
            };
            if !on {
                suppressed = true;
                continue;
            }
            // 关掉「显示轨迹浮层」就只做识别、不画预览；「录一笔」是设置页的录入工具，
            // 要看见自己画的线，不受这枚开关管。
            if !overlay && !recording {
                suppressed = true;
                continue;
            }
            if !shown {
                let first = points[0];
                let moved = points
                    .iter()
                    .map(|p| (p.0 - first.0).abs() + (p.1 - first.1).abs())
                    .max()
                    .unwrap_or(0);
                if moved < start_distance {
                    continue;
                }
                // 与 `handle_gesture` 同一口径：全屏 / 排除程序里不接管，也不画浮层。
                // 「录一笔」例外：那是设置页里的录入工具，前台就是 Kada 自己，
                // 命中排除列表也不该把预览藏掉。
                if !recording && input::foreground_fullscreen() {
                    suppressed = true;
                    continue;
                }
                if !recording && (!exclude.is_empty() || !exclude_apps.is_empty()) {
                    let fetch = || input::frontmost_context();
                    let ctx = TriggerContext::lazy(&fetch, None);
                    if ctx.app_excluded(&exclude) || ctx.app_excluded(&exclude_apps) {
                        suppressed = true;
                        continue;
                    }
                }
                let Some(w) = ensure_gesture_overlay(&app) else {
                    suppressed = true;
                    continue;
                };
                let serial = GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = w.show();
                shown = true;
                if !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) {
                    wait_gesture_overlay_ready();
                }
                // 等前端就绪的这段时间里，这一笔可能已经结束（右键已抬起 / 触发键
                // 已松开）：来源一换就别再往浮层上画了。
                let still_drawing = if kada_hook::gesture::drawing() {
                    true
                } else {
                    kada_hook::trackpad::drawing()
                };
                if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) != serial || !still_drawing {
                    continue;
                }
            }
            if points.len() == last_len {
                continue;
            }
            last_len = points.len();
            if let Some((origin, scale)) = gesture_window_geom(&app) {
                let payload = GesturePreviewPayload {
                    visual,
                    points: local_gesture_points(&points, origin),
                    scale,
                };
                if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
                    let _ = w.emit("gesture-preview", &payload);
                }
            }
        }
    });
}

/// 鼠标手势（规划 7.2-MG）：注册轨迹出口。
///
/// 回调**在钩子线程上**跑（低层钩子回调超会被系统静默摘掉），所以这里只做
/// 「拷贝数据 + 开线程」，识别与动作执行都在新线程里。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_gesture_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    let (overlay_app, overlay_cfg) = (app.clone(), cfg.clone());
    kada_hook::gesture::set_sink(Some(Arc::new(move |stroke| {
        let app = app.clone();
        let cfg = cfg.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 指针设备在**回调线程**上取（和 `fire` 取前台同一口径）：这一笔是哪台鼠标 / 触摸板 /
        // 数位板画的，`when` 里的「设备是」要用它。放到新线程里读会晚一拍、可能已被别的设备覆盖。
        let device = input::current_pointer_device();
        std::thread::spawn(move || handle_gesture(app, cfg, results, unread, runs, stroke, device));
    })));
    spawn_gesture_overlay(overlay_app, overlay_cfg);
}

/// 触摸板手势（7.2-MG）：注册轨迹出口。与右键手势同一个口径——回调在钩子线程上跑，
/// 这里只「拷贝数据 + 开线程」，识别与动作执行都在新线程里。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_trackpad_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::trackpad::set_sink(Some(Arc::new(move |key, stroke| {
        let app = app.clone();
        let cfg = cfg.clone();
        // 这一笔是哪块触摸板 / 数位板画的（手势的 `when` 里「设备是」要用它）：与鼠标手势
        // 同一口径，在回调线程上取一次带下去。
        let device = input::current_pointer_device();
        // 设置页的「录一笔」：只把识别结果推回界面，不执行任何动作（录制优先于正式匹配）。
        if trackpad_recording_key().is_some() {
            std::thread::spawn(move || emit_trackpad_record(&app, &cfg, &stroke));
            return;
        }
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        std::thread::spawn(move || {
            handle_trackpad_stroke(app, cfg, results, unread, runs, key, stroke, device)
        });
    })));
}

/// 一次触摸板手势的收尾：识别 → 命中就执行动作；**没命中什么都不用补**。
///
/// 与右键手势最大的不同：被吞掉的只有触发键本身（它只是「进入采集」的开关），
/// 所以这里没有 `replay_right_click`（吞键铁律要求回放的是「本可以正常工作的输入」，
/// 触发键不是）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_trackpad_stroke(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    trigger: Key,
    stroke: kada_hook::gesture::Stroke,
    device: Option<String>,
) {
    let (on, min_len, action_timeout, text_mode, exclude, exclude_apps, overlay) = {
        let g = cfg.read().unwrap();
        (
            g.settings.touchpad && touchpad_gesture_enabled(&g),
            g.settings.touchpad_start_distance as f64,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.touchpad_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.settings.touchpad_overlay,
        )
    };
    if !on {
        hide_gesture_overlay(&app);
        return;
    }
    // 让路：全屏程序、全局排除列表、手势排除列表。触发键已经被吞了（它没有输出），
    // 所以这里只是「不执行」，不是「回放」。
    if input::foreground_fullscreen() {
        hide_gesture_overlay(&app);
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        hide_gesture_overlay(&app);
        return;
    }
    // 触摸板上画一笔本来就慢，`gesture_timeout_ms` 那套「画太久作废」不适用：
    // 收尾时机是**松键**，不存在右键按住不放的副作用。
    let pattern = kada_core::pattern_from_stroke(&stroke.points, min_len);
    if pattern.is_empty() {
        // 太短 / 涂鸦 / 作废（Esc、双指、停手太久）：不猜，并把轨迹浮层收起来
        // （作废的那一笔靠这次回调收尾，否则浮层会一直挂着）。
        hide_gesture_overlay(&app);
        return;
    }
    let hit = {
        let g = cfg.read().unwrap();
        // 方向串是硬判据；**兜底模板只在没命中时用**（只加召回、不改判）。
        kada_core::find_touchpad(&g, trigger, &pattern, &ctx)
            .or_else(|| kada_core::find_touchpad_by_template(&g, &stroke.points, trigger, &ctx))
            .map(|g| (g.label(), g.actions.clone()))
    };
    let Some((name, actions)) = hit else {
        // 轨迹 + 失败色 + 方向串留在浮层上（约 900ms 自动收），另弹气泡说明怎么用。
        // 关掉「显示轨迹浮层」时只剩气泡（方向串照样报出来）。
        if overlay {
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                String::new(),
                pattern.clone(),
            );
        } else {
            hide_gesture_overlay(&app);
        }
        show_toast_note(
            &app,
            &format!("触摸板手势未匹配：{pattern}"),
            "把这个方向串填进那条手势的「方向串」就能用了。",
        );
        return;
    };
    if actions.is_empty() {
        if overlay {
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                name.clone(),
                pattern.clone(),
            );
        } else {
            hide_gesture_overlay(&app);
        }
        show_toast_note(
            &app,
            &format!("触摸板手势 {pattern} 没有动作"),
            "给这条手势配上动作才会执行。",
        );
        return;
    }
    if overlay {
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            true,
            name.clone(),
            pattern.clone(),
        );
    } else {
        hide_gesture_overlay(&app);
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("手势 {pattern}"),
        name,
        action_timeout,
        text_mode,
        device,
        stroke.points.first().copied(),
    );
}

/// 一次手势的收尾：识别 → 命中就执行动作；**没命中（或压根没画出轨迹）就把右键补回来**。
///
/// 吞键铁律：被吞掉的右键必须原样回放（按下-抬起配对），否则用户的右键就废了。
/// 回放走注入（带 `INJECTED` 标记），不会被自己的钩子再吃一次。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn handle_gesture(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    stroke: kada_hook::gesture::Stroke,
    device: Option<String>,
) {
    let (
        on,
        start_distance,
        min_segment,
        gesture_timeout,
        screenshot,
        action_timeout,
        text_mode,
        exclude,
        exclude_apps,
    ) = {
        let g = cfg.read().unwrap();
        (
            g.settings.experimental && gesture_capture_enabled(&g),
            g.settings.gesture_start_distance,
            g.settings.gesture_min_segment,
            g.settings.gesture_timeout_ms,
            g.settings.gesture_screenshot,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
        )
    };
    if !on {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 全屏程序自动暂停（规划 7.2-MG 余项⑤）：游戏 / 视频 / 远程桌面里右键拖拽本来就是正常
    // 操作，整条不接管、右键原样回放。判据见 `kada_hook::win::foreground_fullscreen`。
    if input::foreground_fullscreen() {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 排除程序：全局列表（EXT-04）与手势专用列表命中都整条不接管，右键原样回放。
    // 同一份上下文供后面的 `when` 复用，一次手势最多查一次前台。
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 位移没超过起手距离 = 普通右键点击（不是手势）；画太久 = 这次手势作废。两种都补回右键。
    let first = stroke.points.first().copied().unwrap_or((0, 0));
    let moved = stroke
        .points
        .iter()
        .map(|p| (p.0 - first.0).abs() + (p.1 - first.1).abs())
        .max()
        .unwrap_or(0);
    let pattern = kada_core::pattern_from_points(&stroke.points, min_segment);
    if moved < start_distance {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 圈选截图：右键按住画完圈、**再点一下左键**才算确认——那一下左键就是「拍照」。
    // 只画圈不点 = 普通右键拖动（下面按没匹配处理，右键原样回放），不会误截。
    if stroke.lasso.is_some() {
        if screenshot {
            run_lasso_capture(&app);
        } else {
            hide_gesture_overlay(&app);
            replay_stroke(&stroke);
        }
        return;
    }
    let matched = if stroke.elapsed_ms > gesture_timeout {
        None
    } else {
        let g = cfg.read().unwrap();
        // `when` 门控复用上面已建好的上下文（排除判定若查过前台就已缓存，不再查询）。
        kada_core::find_gesture(&g, &pattern, &ctx).map(|hit| (hit.label(), hit.actions.clone()))
    };
    if let Some((name, actions)) = matched {
        // 配了手势但没配动作 = 半条配置：这时**不执行**（执行空链等于把右键吞了），
        // 按「没命中」处理、把右键补回去。
        if actions.is_empty() {
            eprintln!("kada: 手势 {pattern}（{name}）没有配置动作（右键已补回）");
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                name.clone(),
                pattern.clone(),
            );
            show_toast_note(
                &app,
                &format!("手势 {pattern} 没有动作"),
                "右键已补回。给这条「鼠标操作」配上动作才会执行。",
            );
            replay_stroke(&stroke);
            return;
        }
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            true,
            name.clone(),
            pattern.clone(),
        );
        fire(
            app,
            results,
            unread,
            runs,
            actions,
            format!("手势 {pattern}"),
            name,
            action_timeout,
            text_mode,
            device,
            // 手势起点 = 右键按下点（EXT-16「移动到手势起点」）；快捷键路径没有起点。
            stroke.points.first().copied(),
        );
        return;
    }
    // 只画了个圈但没点左键：不截图，告诉用户怎么确认（旧版是右键一松就截图，容易误截）。
    if screenshot && kada_core::lasso_bbox(&stroke.points).is_some() {
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            false,
            String::new(),
            pattern.clone(),
        );
        show_toast_note(
            &app,
            "圈选截图未确认",
            "按住右键画圈后再点一下左键；只画圈不截图。",
        );
        replay_stroke(&stroke);
        return;
    }
    if stroke.elapsed_ms > gesture_timeout {
        show_gesture_result(&app, &cfg, &stroke.points, false, String::new(), pattern);
        replay_stroke(&stroke);
        return;
    }
    eprintln!("kada: 未配置的手势 {pattern}（右键已补回）");
    // **看得见的反馈**：没匹配上时除了把右键补回去，还要告诉用户「你画的这个方向串
    // 叫 DRUR」——否则界面毫无反应，用户只会以为功能坏了（试验台也是这么提示的：
    // 照抄方向串进配置就能用）。弹气泡，不进消息中心（避免每次误拖都留一条）。
    show_gesture_result(
        &app,
        &cfg,
        &stroke.points,
        false,
        String::new(),
        pattern.clone(),
    );
    show_toast_note(
        &app,
        &format!("手势未匹配：{pattern}"),
        "右键已补回。把这个方向串填进「快捷键 → 鼠标操作」就能用了。",
    );
    replay_stroke(&stroke);
}

/// Snipaste 在不在跑：在跑才把图递给它（按 F3 没人接会落到前台程序上，宁可不发）。
#[cfg(target_os = "windows")]
fn snipaste_running() -> bool {
    input::process_running("snipaste.exe")
}

#[cfg(not(target_os = "windows"))]
fn snipaste_running() -> bool {
    false
}

/// 圈选截图收尾：收起手势预览 → 等浮层撤掉 → 直接调出 Snipaste 的截图界面（F1）。
///
/// 只负责「进入截图态」：框选范围、矩形 / 椭圆 / 箭头等标注都在 Snipaste 里由用户自己画。
/// 不自动裁剪、不写剪贴板——自动裁完就等于把标注挡在门外。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn run_lasso_capture(app: &tauri::AppHandle) {
    hide_gesture_overlay(app);
    // 浮层隐藏是异步的，等一拍再按键，免得截到手势轨迹。
    std::thread::sleep(Duration::from_millis(80));
    if !snipaste_running() {
        show_toast_note(
            app,
            "圈选截图需要 Snipaste",
            "装好 Snipaste 并让它保持运行，画圈后点左键会自动调出它的截图界面",
        );
        return;
    }
    input::simulate::tap(Key::F1);
    show_toast_note(
        app,
        "已进入截图",
        "Snipaste 截图界面已打开，框选后用矩形 / 椭圆 / 箭头自己标注",
    );
}

/// 触发角（EXT-10）：注册出口。回调在钩子线程上跑，这里只开线程做后续（不建窗 / 不 IO）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_hot_corner_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::hotcorner::set_sink(Some(Arc::new(move |corner| {
        let app = app.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 触发角是光标移进去的：设备 = 最近一次指针设备（`when` 里的「设备是」用它）。
        let device = input::current_pointer_device();
        std::thread::spawn(move || {
            handle_hot_corner(app, cfg, engine, results, unread, runs, corner, device)
        });
    })));
}

/// 防误触停留的「代次」：每次进角 +1。等够停留时长后只有**最后一次**进角的那条能触发——
/// 用户等得心烦、离开又马上回来时，前一条等待线程不该替后一条提前执行（会双触发）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
static HOT_CORNER_DWELL_GEN: AtomicU64 = AtomicU64::new(0);

/// 一次触发角的收尾：全屏 / 排除程序与手势同口径让路；命中就交给 `fire`。
///
/// **不吞任何鼠标事件**，所以这里没有回放逻辑。触发时抓前台窗口由 `fire` 在调用线程上做。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_hot_corner(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    corner: kada_hook::hotcorner::Corner,
    device: Option<String>,
) {
    let spot = match corner {
        kada_hook::hotcorner::Corner::TopLeft => kada_core::HotCornerSpot::TopLeft,
        kada_hook::hotcorner::Corner::TopRight => kada_core::HotCornerSpot::TopRight,
        kada_hook::hotcorner::Corner::BottomLeft => kada_core::HotCornerSpot::BottomLeft,
        kada_hook::hotcorner::Corner::BottomRight => kada_core::HotCornerSpot::BottomRight,
    };
    let (on, action_timeout, text_mode, exclude, exclude_apps, rules, active) = {
        let g = cfg.read().unwrap();
        // 锁序铁律：配置读锁先于 Engine 锁。
        let active = engine.lock().unwrap().active_layer().map(|s| s.to_string());
        (
            g.settings.experimental,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.hot_corners.clone(),
            active,
        )
    };
    if !on {
        return;
    }
    // 全屏 / 排除程序：与手势同口径，整条让路。
    if input::foreground_fullscreen() {
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        return;
    }
    // 层语义同快捷键：激活层里的规则优先，基础层兜底；`when` 不成立 = 这条不存在。
    let pick = |want: Option<&str>| -> Option<(Vec<Action>, String, u64)> {
        rules
            .iter()
            .find(|r| r.enabled && r.spot == spot && r.layer_matches(want) && r.when_matches(&ctx))
            .map(|r| (r.actions.clone(), r.label(), u64::from(r.dwell_ms)))
    };
    let Some((actions, name, dwell_ms)) = active
        .as_deref()
        .and_then(|id| pick(Some(id)))
        .or_else(|| pick(None))
    else {
        return;
    };
    // 没配动作 = 半条配置，什么也不做（没有可执行的链，也没有要回放的按键）。
    if actions.is_empty() {
        return;
    }
    // 防误触：等够停留时长再确认光标还在角上——扫过角落（关窗口、拖滚动条）不算触发。
    // 钩子那边这个角已被占住（停留不重复上报），离开超过重新武装距离才清；所以等待期间
    // 只有「离开又马上回来」会再进一枪，靠代次让旧等待作废，免得同一次停留执行两遍。
    if dwell_ms > 0 {
        let generation = HOT_CORNER_DWELL_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        std::thread::sleep(Duration::from_millis(dwell_ms));
        if HOT_CORNER_DWELL_GEN.load(Ordering::Relaxed) != generation
            || !kada_hook::hotcorner::still_near(corner)
        {
            return;
        }
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("触发角 {name}"),
        name,
        action_timeout,
        text_mode,
        device,
        None,
    );
}

/// 边缘滚动（EXT-11）：注册出口。回调在钩子线程上跑，这里只开线程做后续（不建窗 / 不 IO）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn spawn_wheel_edge_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::wheel_edge::set_sink(Some(Arc::new(move |edge| {
        let app = app.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 边缘滚动是滚轮触发的：设备 = 最近一次指针设备（`when` 里的「设备是」用它）。
        let device = input::current_pointer_device();
        std::thread::spawn(move || {
            handle_wheel_edge(app, cfg, engine, results, unread, runs, edge, device)
        });
    })));
}

/// 一次边缘滚动的收尾：全屏 / 排除程序与手势同口径让路；命中就交给 `fire`。
///
/// 钩子层**没有吞滚轮**，所以这里没有回放逻辑。触发时抓前台窗口由 `fire` 在调用线程上做。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_wheel_edge(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    edge: kada_hook::wheel_edge::Edge,
    device: Option<String>,
) {
    let spot = match edge {
        kada_hook::wheel_edge::Edge::Top => kada_core::WheelEdgeSpot::Top,
        kada_hook::wheel_edge::Edge::Bottom => kada_core::WheelEdgeSpot::Bottom,
        kada_hook::wheel_edge::Edge::Left => kada_core::WheelEdgeSpot::Left,
        kada_hook::wheel_edge::Edge::Right => kada_core::WheelEdgeSpot::Right,
    };
    let (on, action_timeout, text_mode, exclude, exclude_apps, rules, active) = {
        let g = cfg.read().unwrap();
        // 锁序铁律：配置读锁先于 Engine 锁。
        let active = engine.lock().unwrap().active_layer().map(|s| s.to_string());
        (
            g.settings.experimental,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.wheel_edges.clone(),
            active,
        )
    };
    if !on {
        return;
    }
    // 全屏 / 排除程序：与手势同口径，整条让路。
    if input::foreground_fullscreen() {
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        return;
    }
    // 层语义同快捷键：激活层里的规则优先，基础层兜底；`when` 不成立 = 这条不存在。
    let pick = |want: Option<&str>| -> Option<(Vec<Action>, String)> {
        rules
            .iter()
            .find(|r| r.enabled && r.edge == spot && r.layer_matches(want) && r.when_matches(&ctx))
            .map(|r| (r.actions.clone(), r.label()))
    };
    let Some((actions, name)) = active
        .as_deref()
        .and_then(|id| pick(Some(id)))
        .or_else(|| pick(None))
    else {
        return;
    };
    // 没配动作 = 半条配置，什么也不做（滚轮本来就放行了）。
    if actions.is_empty() {
        return;
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("边缘滚动 {name}"),
        name,
        action_timeout,
        text_mode,
        device,
        None,
    );
}

/// 把被手势吞掉的右键补回来（按下-抬起配对）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn replay_right_click() {
    let op = kada_core::MouseOp::Click {
        button: kada_core::MouseButton::Right,
    };
    if let Err(e) = input::simulate::mouse(&op) {
        eprintln!("kada: 回放右键失败：{e}");
    }
}

/// 手势让路 / 未命中时补回被吞的键：右键一定补；**圈选确认过的那记左键也要补**——
/// 钩子层确认时把左右键都吞了，壳层若因为全屏 / 排除程序 / 开关关着而不截图，不补就少一次点击。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn replay_stroke(stroke: &kada_hook::gesture::Stroke) {
    replay_right_click();
    if stroke.lasso.is_some() {
        let op = kada_core::MouseOp::Click {
            button: kada_core::MouseButton::Left,
        };
        if let Err(e) = input::simulate::mouse(&op) {
            eprintln!("kada: 回放左键失败：{e}");
        }
    }
}
