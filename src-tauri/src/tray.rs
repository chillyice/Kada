// 托盘（T-03 从 lib.rs 搬出）：自绘托盘菜单 + Linux 原生菜单 + 红点图标绘制。
// 菜单窗不可激活（Linux 收不到托盘点击，改走原生菜单），见 说明书 §6.3 / 架构设计 §3.21。

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};

use serde::Serialize;

use crate::{
    hints_set_visible, reload_config_now, show_main_window, toggle_pause, update, KadaState,
};

// ---- 托盘菜单（自绘浮窗） ----
//
// 原生托盘菜单由系统画（Windows 是 Win32 的灰白菜单、macOS 是 NSMenu），配色 / 圆角 / 字体
// 全在系统手里，跟四套主题（尤其 CRT 像素风）完全割裂。所以右键托盘图标不再挂原生菜单，
// 改成弹一个**不可激活**的透明浮窗，条目与勾选全走主题变量（`style.css` 的 `.tray-menu-panel`）。
//
// Linux 例外：libappindicator 那条路根本收不到托盘点击事件（Tauri 文档：`TrayIconEvent`
// Unsupported），只能继续挂原生菜单——那边用 [`build_tray_native_menu`]。

pub(crate) const TRAY_MENU_WINDOW: &str = "tray-menu";
/// 面板外那圈透明边（CSS 投影要留在这圈里，见 `.tray-menu-panel` 与 `bootstrapTrayMenu`）。
const TRAY_MENU_MARGIN: f64 = 10.0;
/// 菜单与托盘图标 / 工作区边缘的间隙（逻辑像素）。
const TRAY_MENU_GAP: f64 = 6.0;

/// 托盘菜单的一条：`checkable` 的条目左侧留出勾选位，勾选态随每次弹出重读配置。
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TrayMenuItemView {
    Action {
        id: String,
        label: String,
        checkable: bool,
        checked: bool,
        /// 「退出」这类不可逆条目：用强调色画，别跟普通条目混在一起。
        danger: bool,
    },
    Separator,
}

#[derive(Clone, Serialize)]
pub(crate) struct TrayMenuPayload {
    items: Vec<TrayMenuItemView>,
}

/// 组装托盘菜单的展示树。勾选态**每次弹出时现读配置**——暂停 / 提示框都能被别的入口改掉，
/// 记住一份旧值只会显示错（Linux 的原生菜单那边仍靠 `CheckMenuItem` 对齐，见 `pause_tray`）。
pub(crate) fn tray_menu_payload(app: &tauri::AppHandle) -> TrayMenuPayload {
    use TrayMenuItemView::{Action, Separator};
    let (paused, hints) = app
        .try_state::<KadaState>()
        .and_then(|st| {
            st.config
                .read()
                .ok()
                .map(|c| (c.settings.paused, c.settings.hints.visible))
        })
        .unwrap_or((false, false));
    let plain = |id: &str, label: &str| Action {
        id: id.to_string(),
        label: label.to_string(),
        checkable: false,
        checked: false,
        danger: false,
    };
    TrayMenuPayload {
        items: vec![
            plain("show", "打开咔哒"),
            Separator,
            Action {
                id: "pause".into(),
                label: "暂停快捷键触发".into(),
                checkable: true,
                checked: paused,
                danger: false,
            },
            Action {
                id: "hints".into(),
                label: "快捷键提示".into(),
                checkable: true,
                checked: hints,
                danger: false,
            },
            Separator,
            plain("reload", "重载配置"),
            plain("update", "检查更新"),
            Separator,
            Action {
                id: "quit".into(),
                label: "退出".into(),
                checkable: false,
                checked: false,
                danger: true,
            },
        ],
    }
}

/// 执行托盘菜单条目：原生菜单（Linux）与自绘菜单（其余平台）共用这一份分发。
pub(crate) fn tray_menu_action(app: &tauri::AppHandle, id: &str) {
    match id {
        "show" => show_main_window(app),
        // 暂停 / 恢复快捷键触发（勾选态由状态机定时器线程对齐，见 spawn_engine_ticker）。
        "pause" => toggle_pause(app),
        "reload" => reload_config_now(app),
        // 提示框的开关：常显浮窗的入口得在「随时够得着」的地方——窗口上的 × 一按，
        // 设置界面要是没开着，用户就只剩托盘这一个入口。
        "hints" => {
            let visible = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.hints.visible)
                .unwrap_or(false);
            if let Err(e) = hints_set_visible(app.clone(), app.state(), !visible) {
                eprintln!("切换快捷键提示框失败: {e}");
            }
        }
        "update" => update::spawn(app, update::Mode::Manual),
        "quit" => app.exit(0),
        _ => {}
    }
}

/// 按需创建托盘菜单窗。与 hints / clipboard-menu 同一类：**不可激活**
/// （`WS_EX_NOACTIVATE`）——一旦能激活，弹菜单这一下就把自己变成了前台窗口，前台条件
/// （前台应用 / 窗口标题）全读成我们自己，`ResetWatch` 还会当成「切过前台」复位输入状态。
///
/// 与 toast / hud 的差别：**不能** `set_ignore_cursor_events(true)`——条目要能点。
fn ensure_tray_menu(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) {
        return Some(w);
    }
    match tauri::WebviewWindowBuilder::new(
        app,
        TRAY_MENU_WINDOW,
        tauri::WebviewUrl::App("index.html#tray-menu".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同其余浮窗：别借系统投影（那圈 DWM 边框会挂在面板外面），投影由 CSS 自己画。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .zoom_hotkeys_enabled(false)
    .inner_size(220.0, 220.0)
    .visible(false)
    .build()
    {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("创建托盘菜单失败: {e}");
            None
        }
    }
}

/// 收回托盘菜单：隐藏窗口并作废当前轮询线程（`serial` 递增后旧线程自行退出）。
pub(crate) fn hide_tray_menu(app: &tauri::AppHandle) {
    if let Some(st) = app.try_state::<KadaState>() {
        st.tray_menu_showing.store(false, Ordering::Relaxed);
        st.tray_menu_serial.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) {
        let _ = w.hide();
    }
}

/// 右键托盘图标：记锚点、刷新载荷、弹自绘菜单。
///
/// 建窗是重活（首次上百毫秒）：别占着托盘事件线程。
pub(crate) fn spawn_tray_menu(app: &tauri::AppHandle, cursor: (i32, i32)) {
    let Some(st) = app.try_state::<KadaState>() else {
        return;
    };
    let payload = tray_menu_payload(app);
    // 新一次触发：先让旧菜单的轮询线程作废，再记本次的锚点与载荷。
    st.tray_menu_serial.fetch_add(1, Ordering::Relaxed);
    st.tray_menu_showing.store(true, Ordering::Relaxed);
    *st.tray_menu_anchor.lock().unwrap() = Some(cursor);
    st.tray_menu_payload
        .lock()
        .unwrap()
        .replace(payload.clone());
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(w) = ensure_tray_menu(&app) else {
            if let Some(st) = app.try_state::<KadaState>() {
                st.tray_menu_showing.store(false, Ordering::Relaxed);
            }
            return;
        };
        let _ = w.emit("tray-menu", &payload);
    });
}

/// 菜单渲染完成（前端量出内容宽高后调用）：贴托盘图标定位、钳进当前显示器工作区并显示。
#[tauri::command]
pub(crate) fn tray_menu_ready(app: tauri::AppHandle, width: f64, height: f64) {
    let Some(st) = app.try_state::<KadaState>() else {
        return;
    };
    if !st.tray_menu_showing.load(Ordering::Relaxed) {
        return;
    }
    let Some(cursor) = *st.tray_menu_anchor.lock().unwrap() else {
        return;
    };
    let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) else {
        return;
    };
    let monitor = app
        .monitor_from_point(cursor.0 as f64, cursor.1 as f64)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(m) = monitor else {
        return;
    };
    let scale = m.scale_factor();
    let work = m.work_area();
    let (wx, wy) = (work.position.x as f64, work.position.y as f64);
    let (ww, wh) = (work.size.width as f64, work.size.height as f64);
    // 尺寸来自界面，不可全信：宽 120~480、高 60~1200 CSS 像素。
    let (wp, hp) = (
        (width.clamp(120.0, 480.0) * scale).round(),
        (height.clamp(60.0, 1200.0) * scale).round(),
    );
    let gap = TRAY_MENU_GAP * scale;
    // 窗口比面板大一圈（`TRAY_MENU_MARGIN` 留给 CSS 投影），定位要按**面板**算：
    // 面板右下角贴图标、向左上展开（任务栏在底边 / macOS 菜单栏在顶边都成立）；
    // 上方放不下就翻到光标下方，最后兜底钳进工作区。
    let margin = TRAY_MENU_MARGIN * scale;
    let mut x = cursor.0 as f64 + gap + margin - wp;
    let mut y = cursor.1 as f64 - hp + margin - gap;
    if y + margin < wy {
        y = cursor.1 as f64 + gap - margin;
    }
    x = x.clamp(
        wx + gap - margin,
        (wx + ww - wp - gap + margin).max(wx + gap - margin),
    );
    y = y.clamp(
        wy + gap - margin,
        (wy + wh - hp - gap + margin).max(wy + gap - margin),
    );
    let _ = w.set_size(tauri::PhysicalSize::new(wp as u32, hp as u32));
    let _ = w.set_position(tauri::PhysicalPosition::new(
        x.round() as i32,
        y.round() as i32,
    ));
    let _ = w.show();
    spawn_tray_menu_watch(&app, &st);
}

/// 光标巡检：菜单不可激活，收不到失焦 / Esc，只能靠轮询收回。
///
/// 判据是「光标离开菜单周围一圈（`TRAY_MENU_GAP` 的量级）超过 400ms」——刚弹出来时光标
/// 正好压在托盘图标上（在菜单外），所以**不能**用「在窗口外就收」；点条目时前端自己收。
fn spawn_tray_menu_watch(app: &tauri::AppHandle, st: &KadaState) {
    let serial = st.tray_menu_serial.load(Ordering::Relaxed);
    let showing = st.tray_menu_showing.clone();
    let serials = st.tray_menu_serial.clone();
    let app = app.clone();
    std::thread::spawn(move || {
        // 光标要离开菜单多远才算「点了别处」：贴着图标边缘的手抖不算。
        let slack = 64.0;
        let mut outside_since: Option<Instant> = None;
        loop {
            std::thread::sleep(Duration::from_millis(60));
            if serials.load(Ordering::Relaxed) != serial || !showing.load(Ordering::Relaxed) {
                return;
            }
            let Some(w) = app.get_webview_window(TRAY_MENU_WINDOW) else {
                return;
            };
            if !w.is_visible().unwrap_or(false) {
                showing.store(false, Ordering::Relaxed);
                return;
            }
            if let (Ok(cursor), Ok(pos), Ok(size)) =
                (w.cursor_position(), w.outer_position(), w.outer_size())
            {
                let (left, top) = (pos.x as f64 - slack, pos.y as f64 - slack);
                let (right, bottom) = (
                    pos.x as f64 + size.width as f64 + slack,
                    pos.y as f64 + size.height as f64 + slack,
                );
                let inside =
                    cursor.x >= left && cursor.x <= right && cursor.y >= top && cursor.y <= bottom;
                if inside {
                    outside_since = None;
                } else {
                    let now = Instant::now();
                    let since = *outside_since.get_or_insert(now);
                    if now.saturating_duration_since(since) >= Duration::from_millis(400) {
                        hide_tray_menu(&app);
                        return;
                    }
                }
            }
        }
    });
}

/// 点击菜单条目：先收回菜单，再走与原生菜单同一份分发。
#[tauri::command]
pub(crate) fn tray_menu_exec(app: tauri::AppHandle, id: String) {
    hide_tray_menu(&app);
    tray_menu_action(&app, &id);
}

/// 读取最近一次托盘菜单载荷（菜单窗懒创建后，前端加载时兜底渲染）。
#[tauri::command]
pub(crate) fn get_tray_menu_payload(state: tauri::State<'_, KadaState>) -> Option<TrayMenuPayload> {
    state.tray_menu_payload.lock().unwrap().clone()
}

#[cfg(target_os = "linux")]
use tauri::menu::MenuItem;

/// Linux 的原生托盘菜单（那边收不到托盘点击事件，自绘浮窗没有入口）。
#[cfg(target_os = "linux")]
pub(crate) fn build_tray_native_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let st = app.state::<KadaState>();
    let (paused, hints) = {
        let cfg = st.config.read().unwrap();
        (cfg.settings.paused, cfg.settings.hints.visible)
    };
    let show_i = MenuItem::with_id(app, "show", "打开咔哒", true, None::<&str>)?;
    let pause_i =
        CheckMenuItem::with_id(app, "pause", "暂停快捷键触发", true, paused, None::<&str>)?;
    let hints_i = CheckMenuItem::with_id(app, "hints", "快捷键提示", true, hints, None::<&str>)?;
    let reload_i = MenuItem::with_id(app, "reload", "重载配置", true, None::<&str>)?;
    let update_i = MenuItem::with_id(app, "update", "检查更新", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    // 记住这个勾选项：提示框被窗口上的 × 关掉时，得把勾抹掉。
    *st.hints_tray.lock().unwrap() = Some(hints_i.clone());
    // 记住暂停勾选项：设置页改 `settings.paused` 时定时器线程据此对齐勾选。
    *st.pause_tray.lock().unwrap() = Some(pause_i.clone());
    Menu::with_items(
        app,
        &[&show_i, &pause_i, &hints_i, &reload_i, &update_i, &quit_i],
    )
}

/// 在图标右上角叠加一个红色圆点（未读标记），返回新图像。
pub(crate) fn with_red_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let w = icon.width() as usize;
    let h = icon.height() as usize;
    let mut rgba = icon.rgba().to_vec();
    let r = ((w.min(h) as f32) * 0.20).max(1.0) as usize;
    let cx = w.saturating_sub(r + 1);
    let cy = r + 1;
    for y in 0..h {
        for x in 0..w {
            let dx = x as isize - cx as isize;
            let dy = y as isize - cy as isize;
            if dx * dx + dy * dy <= (r as isize) * (r as isize) {
                let i = (y * w + x) * 4;
                rgba[i] = 0xe0; // R
                rgba[i + 1] = 0x50; // G
                rgba[i + 2] = 0x40; // B
                rgba[i + 3] = 0xff; // A
            }
        }
    }
    tauri::image::Image::new_owned(rgba, icon.width(), icon.height())
}
