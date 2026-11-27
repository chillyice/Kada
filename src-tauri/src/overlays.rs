// 快捷键提示框浮窗（7.3-㊳，T-03 从 lib.rs 搬出）：不可激活，能收鼠标、不抢前台；
// 还能拖动 / 滚轮调大小与透明度（见 说明书 §6.6 / 架构设计 §3.21）。

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};

use crate::{save_settings_patch, HintsState, KadaState};

// ---- 快捷键提示框（常显速查浮窗，见规划 7.3-㊳） ----

/// 按需创建快捷键提示框窗口（`index.html#hints`）：常显的快捷键速查表，可拖动、滚轮调大小 /
/// 透明度。首次打开（或启动时配置里勾着显示）才建 WebView，冷启动零成本。
///
/// **必须 `focusable(false)`**（Windows 即 `WS_EX_NOACTIVATE`），理由与 [`ensure_hud`] 完全一致：
/// 这个窗是**常显**的，一整天挂在所有窗口之上，一旦它能被激活，点它一下就把它自己变成了
/// `GetForegroundWindow`——前台条件（前台应用 / 窗口标题）全读成我们自己这个窗，`ResetWatch`
/// 还会把它当作「前台切换」复位输入状态（按住的层当场被踢掉）。
///
/// 与 hud / toast 的关键差别：**不能** `set_ignore_cursor_events(true)`——拖动、滚轮、关闭按钮
/// 都要靠鼠标事件；`WS_EX_NOACTIVATE` 只挡激活，不挡鼠标消息，所以交互照常。
pub(crate) fn ensure_hints(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("hints") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        "hints",
        tauri::WebviewUrl::App("index.html#hints".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同 `ensure_toast`：别借系统投影，那圈 DWM 边框会挂在面板外面。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    // Ctrl+滚轮在本窗口是「调透明度」，不能让它同时把整个 WebView 的缩放也改了
    // （Chromium 默认把 Ctrl+wheel 当浏览器缩放，两件事叠在一起会互相打架）。
    .zoom_hotkeys_enabled(false)
    .inner_size(320.0, 420.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建快捷键提示框失败: {e}");
            return None;
        }
    };
    Some(w)
}

/// 提示框的当前状态（`hints_ready` 回给前端做拖动基准，`hints-state` 事件推给窗口）。
fn hints_state_of(app: &tauri::AppHandle) -> HintsState {
    let hints = app
        .state::<KadaState>()
        .config
        .read()
        .map(|c| c.settings.hints.clone())
        .unwrap_or_default();
    // 窗口真实位置优先于配置：拖动中配置还是旧的（松手才落盘），拿它当基准会跳。
    let pos = app.get_webview_window("hints").and_then(|w| {
        let scale = w.scale_factor().unwrap_or(1.0);
        w.outer_position()
            .ok()
            .map(|p| (p.x as f64 / scale, p.y as f64 / scale))
    });
    let (x, y) = pos.unwrap_or((
        hints.x.map(f64::from).unwrap_or(0.0),
        hints.y.map(f64::from).unwrap_or(0.0),
    ));
    HintsState {
        visible: hints.visible,
        scale: hints.scale,
        opacity: hints.opacity,
        x,
        y,
    }
}

/// 保存位置是否落在某块显示器的工作区里（至少露出一角够拖回来）。
///
/// 配置是跨平台同步的：在双屏机器上拖到副屏，同步到单屏机器上就落在屏幕外了——
/// 那时宁可回到默认位，也不能让提示框「显示着但看不见」。
fn hints_position_on_screen(app: &tauri::AppHandle, x: i32, y: i32, w: f64, h: f64) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let s = m.scale_factor();
        let area = m.work_area();
        let (mx, my) = (area.position.x as f64 / s, area.position.y as f64 / s);
        let (mw, mh) = (area.size.width as f64 / s, area.size.height as f64 / s);
        let visible_w = (x as f64 + w).min(mx + mw) - (x as f64).max(mx);
        let visible_h = (y as f64 + h).min(my + mh) - (y as f64).max(my);
        visible_w >= 120.0 && visible_h >= 40.0
    })
}

/// 提示框该落在哪：优先用保存的位置；没保存过、或那个位置已经不在任何显示器工作区里
/// （换过显示器 / 跨平台同步来的坐标）就回到主显示器工作区右侧的默认位。
fn hints_target_position(
    app: &tauri::AppHandle,
    logical_w: f64,
    logical_h: f64,
    x: Option<i32>,
    y: Option<i32>,
) -> Option<tauri::LogicalPosition<f64>> {
    if let (Some(x), Some(y)) = (x, y) {
        if hints_position_on_screen(app, x, y, logical_w, logical_h) {
            return Some(tauri::LogicalPosition::new(x as f64, y as f64));
        }
    }
    let m = app.primary_monitor().ok().flatten()?;
    let scale = m.scale_factor();
    let area = m.work_area();
    let (mx, my) = (
        area.position.x as f64 / scale,
        area.position.y as f64 / scale,
    );
    let (mw, mh) = (
        area.size.width as f64 / scale,
        area.size.height as f64 / scale,
    );
    // 默认贴右侧、垂直居中：不挡正在打字的输入框，也不挡底部居中的状态指示。
    let x = mx + mw - logical_w - 24.0;
    let y = my + (mh - logical_h) / 2.0;
    Some(tauri::LogicalPosition::new(
        x.max(mx + 8.0),
        y.max(my + 8.0),
    ))
}

/// 提示框渲染完成（前端量出内容宽高后调用）：按内容定尺寸、首次落地并显示。
///
/// 尺寸由界面量（`hints_ready` 与 `hud_ready` 同一个套路）：内容随快捷键条数与字号变，
/// 后端不该去猜；但界面给的值不可全信，钳一下免得异常值把窗口撑满屏或只剩一条缝。
#[tauri::command]
pub(crate) fn hints_ready(app: tauri::AppHandle, width: f64, height: f64) -> Option<HintsState> {
    let w = app.get_webview_window("hints")?;
    let logical_w = width.clamp(180.0, 900.0);
    let logical_h = height.clamp(120.0, 2000.0);
    let _ = w.set_size(tauri::LogicalSize::new(logical_w, logical_h));
    let state = app.try_state::<KadaState>()?;
    let (visible, x, y) = {
        let guard = state.config.read().ok()?;
        (
            guard.settings.hints.visible,
            guard.settings.hints.x,
            guard.settings.hints.y,
        )
    };
    if !visible {
        // 量尺寸这段时间里它已经被关掉了：别把刚藏起来的窗又亮回来（同 `hud_ready`）。
        let _ = w.hide();
        return Some(hints_state_of(&app));
    }
    // 只在本次会话还没摆过位时落地：摆过之后位置由用户拖拽掌控，重排/缩放都不再挪它。
    if !state.hints_placed.swap(true, Ordering::Relaxed) {
        if let Some(pos) = hints_target_position(&app, logical_w, logical_h, x, y) {
            let _ = w.set_position(pos);
        }
    }
    let _ = w.show();
    Some(hints_state_of(&app))
}

/// 读取提示框状态（窗口懒创建后前端加载时拉一次，拿到字号 / 透明度 / 实际位置）。
#[tauri::command]
pub(crate) fn get_hints_state(app: tauri::AppHandle) -> HintsState {
    hints_state_of(&app)
}

/// 拖动中：把窗口移到指定逻辑坐标。**不落盘**——拖动时每帧写盘既慢又浪费，
/// 松手由 [`hints_commit`] 一次记下。
#[tauri::command]
pub(crate) fn hints_move(app: tauri::AppHandle, x: f64, y: f64) {
    if let Some(w) = app.get_webview_window("hints") {
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
}

/// 拖动结束：把当前位置记进配置，下次启动还在原地。
#[tauri::command]
pub(crate) fn hints_commit(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
) -> Result<(), String> {
    let Some(w) = app.get_webview_window("hints") else {
        return Ok(());
    };
    let scale = w.scale_factor().unwrap_or(1.0);
    let Ok(pos) = w.outer_position() else {
        return Ok(());
    };
    let (x, y) = (
        (pos.x as f64 / scale).round() as i32,
        (pos.y as f64 / scale).round() as i32,
    );
    save_settings_patch(&app, &state, |s| {
        s.hints.x = Some(x);
        s.hints.y = Some(y);
    })
}

/// 滚轮调完大小 / 透明度后落盘（前端已经就地应用，这里只负责持久化；前端做了节流）。
#[tauri::command]
pub(crate) fn hints_prefs(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    scale: u16,
    opacity: u8,
) -> Result<(), String> {
    save_settings_patch(&app, &state, |s| {
        s.hints.scale = scale;
        s.hints.opacity = opacity;
    })
}

/// 显示 / 隐藏提示框（设置页勾选、托盘菜单、窗口上的 × 都走这里）。
///
/// 关掉只把 `visible` 置 false，位置与外观留着——这是「关掉了还能从软件里再打开」的前提：
/// 下次打开必须还在原地、还是他调好的样子。
#[tauri::command]
pub(crate) fn hints_set_visible(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    visible: bool,
) -> Result<(), String> {
    save_settings_patch(&app, &state, |s| s.hints.visible = visible)?;
    apply_hints_visible(&app, visible);
    Ok(())
}

/// 把提示框的可见性落到窗口与托盘：显示 / 隐藏 + 抹平托盘勾选 + 通知窗口。
///
/// 可见性有四个入口（窗口 × / 设置页勾选 / 托盘菜单 / 外部改配置后重载），无论从哪进来
/// 都得把另外三处对齐，否则会出现「菜单勾着、窗口却没了」这种查不出原因的错位。
pub(crate) fn apply_hints_visible(app: &tauri::AppHandle, visible: bool) {
    if let Some(item) = app.state::<KadaState>().hints_tray.lock().unwrap().as_ref() {
        let _ = item.set_checked(visible);
    }
    if visible {
        // 建 WebView 是重活（首次上百毫秒）：别占着托盘事件线程 / 命令线程。
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = ensure_hints(&app);
        });
    } else if let Some(w) = app.get_webview_window("hints") {
        let _ = w.hide();
    }
    let _ = app.emit("hints-state", hints_state_of(app));
}

/// 提示框「悬停让位」的巡检线程：光标一进框就穿透让位（不挡住后面的页面），停留够久再恢复
/// 可交互；挪出边界后下次再进来照样让位。
///
/// 为什么只能轮询、不能用鼠标事件：让位本身就是靠「这个窗不收鼠标消息」实现的，一让位
/// 就再也没有 `mouseenter` / `mouseleave` 可言了。要在穿透状态下还知道光标在不在框里，
/// 只能自己盯全局光标位置（同 [`spawn_config_watcher`] 的取舍：轮询，不引依赖与平台分支，
/// 代价只是慢一拍）。判定与副作用分开写：相位推演是纯函数 [`HintsHover`]，这条线程只负责
/// 取光标 / 窗口矩形、落地 `set_ignore_cursor_events` 与通知界面变淡。
pub(crate) fn spawn_hints_hover_watch(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut phase = HintsHover::Out;
        // 「光标进框」的那一刻：让位与恢复都按它算停留时长。
        let mut since = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(HINTS_HOVER_POLL_MS));
            // 还没建（懒创建）：没有窗口可管，等它出来。
            let Some(w) = app.get_webview_window("hints") else {
                continue;
            };
            if !w.is_visible().unwrap_or(false) {
                // 被关掉 / 还没显示：相位复位，下次显示重新从「进框即让位」开始。
                // 这里必须真的撤掉穿透标记，不能只改相位——否则关掉时正让着位，下次显示出来
                // 就是一块点不着的透明板。
                if phase != HintsHover::Out {
                    phase = HintsHover::Out;
                    let _ = w.set_ignore_cursor_events(false);
                    let _ = app.emit("hints-peek", false);
                }
                continue;
            }
            let inside = hints_cursor_over(&w);
            let next = hints_hover_step(phase, inside, since.elapsed().as_millis() as u64);
            if next != phase {
                if next == HintsHover::Passing {
                    since = Instant::now();
                }
                // 让位 = 穿透 + 变淡，恢复同理。穿透是系统层的事，用户看不见：同时通知界面
                // 变淡（`hints-peek`），否则「鼠标放上去它就不理我了」会像失灵。
                let pass_through = next == HintsHover::Passing;
                let _ = w.set_ignore_cursor_events(pass_through);
                let _ = app.emit("hints-peek", pass_through);
                phase = next;
            }
        }
    });
}

/// 光标停在提示框上超过这么久，提示框才恢复可交互：让位是为了点后面的页面，真想用这个框
/// （拖动 / 滚轮 / × 关闭）就得先「表明意图」停一会儿。
pub(crate) const HINTS_HOVER_DWELL_MS: u64 = 2000;
/// 悬停巡检的节拍：让位判定最迟晚这么一拍，代价是每秒十来次极廉价的取位查询。
const HINTS_HOVER_POLL_MS: u64 = 120;

/// 悬停让位的相位（[`hints_hover_step`] 的输入输出，纯判定、可单测）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HintsHover {
    /// 光标不在框内：可交互（默认状态）。
    Out,
    /// 刚进框：已穿透让位，等光标停留满 [`HINTS_HOVER_DWELL_MS`]。
    Passing,
    /// 停留够久：恢复可交互，一直保持到光标挪出边界——期间挪进挪出都不会再让位。
    Settled,
}

/// 推演下一拍该是什么相位。`dwell_ms` 是本拍距「光标进框」过了多久。
///
/// 相位只在 `Out ↔ Passing ↔ Settled` 之间走一格一格：出框无条件回 `Out`（下次再进来照样
/// 让位），`Passing` 攒够停留时长才升 `Settled`。
pub(crate) fn hints_hover_step(cur: HintsHover, inside: bool, dwell_ms: u64) -> HintsHover {
    match (cur, inside) {
        (_, false) => HintsHover::Out,
        (HintsHover::Settled, true) => HintsHover::Settled,
        (HintsHover::Passing, true) if dwell_ms >= HINTS_HOVER_DWELL_MS => HintsHover::Settled,
        _ => HintsHover::Passing,
    }
}

/// 光标是否落在提示框的窗口矩形内。
///
/// 拿不到光标位置（Wayland 没有全局光标等）就当「不在框内」：退化成让位前的行为，提示框
/// 一直可交互，不至于反过来把框弄成点不着的透明板。
fn hints_cursor_over(w: &tauri::WebviewWindow) -> bool {
    let (Ok(pos), Ok(size), Ok(cur)) = (w.outer_position(), w.outer_size(), w.cursor_position())
    else {
        return false;
    };
    point_in_rect((cur.x, cur.y), (pos.x, pos.y, size.width, size.height))
}

/// 光标（物理坐标）是否在窗口矩形内。独立成纯函数是为了能单测边界那一圈。
pub(crate) fn point_in_rect(cursor: (f64, f64), rect: (i32, i32, u32, u32)) -> bool {
    let (cx, cy) = cursor;
    let (x, y) = (rect.0 as f64, rect.1 as f64);
    let (w, h) = (rect.2 as f64, rect.3 as f64);
    cx >= x && cy >= y && cx < x + w && cy < y + h
}
