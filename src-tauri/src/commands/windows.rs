// 浮窗回报命令（T-04 从 lib.rs 搬出）：手势浮层就绪、HUD / 状态载荷、设置变量输入窗的回传。
// 建窗与定位在根模块 `windows.rs` / `overlays.rs`，这里只做前端回传入口（见 说明书 §5.8 / §6.10）。

use crate::*;

/// 手势浮层前端注册完监听器后回报（避免冷启动第一笔手势的轨迹事件丢失）。
#[tauri::command]
pub(crate) fn gesture_overlay_ready() {
    GESTURE_OVERLAY_READY.store(true, Ordering::Relaxed);
}

/// 悬浮指示窗渲染完成（前端量出内容宽高后调用）：按内容调整窗口、贴工作区底部居中并显示。
#[tauri::command]
pub(crate) fn hud_ready(app: tauri::AppHandle, width: f64, height: f64) {
    let Some(w) = app.get_webview_window("hud") else {
        return;
    };
    // 尺寸来自界面，不可全信：钳一下，别让异常值把窗口撑满屏幕或只剩一条缝。
    let w_logical = width.clamp(120.0, 900.0);
    let h_logical = height.clamp(24.0, 240.0);
    let _ = w.set_size(tauri::LogicalSize::new(w_logical, h_logical));
    let state = app.state::<KadaState>();
    // 先读配置、再读状态载荷（加锁顺序与 [`publish_status`] 一致）。
    let enabled = state
        .config
        .read()
        .map(|c| c.settings.show_status_hud)
        .unwrap_or(true);
    let active = state
        .status
        .lock()
        .unwrap()
        .as_ref()
        .map(|p| !p.idle())
        .unwrap_or(false);
    if !enabled || !active {
        // 量尺寸这段时间里状态已经过去了（或指示被关掉）：别把刚藏起来的窗又亮回来。
        let _ = w.hide();
        return;
    }
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let work = m.work_area();
        let x =
            (work.position.x as f64 + (work.size.width as f64 - w_logical * scale) / 2.0) / scale;
        let y =
            (work.position.y as f64 + work.size.height as f64 - h_logical * scale - 24.0 * scale)
                / scale;
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
    let _ = w.show();
}

/// 读取当前输入状态载荷（悬浮指示窗懒创建后，前端加载时调用兜底渲染）。
#[tauri::command]
pub(crate) fn get_status_payload(state: tauri::State<'_, KadaState>) -> Option<StatusPayload> {
    state.status.lock().unwrap().clone()
}

/// 输入窗渲染完成（前端量出内容宽高）：居中到光标所在显示器、显示并拿到焦点。
#[tauri::command]
pub(crate) fn prompt_ready(app: tauri::AppHandle, width: f64, height: f64) {
    let Some(w) = app.get_webview_window(PROMPT_WINDOW) else {
        return;
    };
    // 尺寸来自界面，钳一下：提示语很长时别把窗口撑满屏幕。
    let w_logical = width.clamp(260.0, 900.0);
    let h_logical = height.clamp(120.0, 480.0);
    let _ = w.set_size(tauri::LogicalSize::new(w_logical, h_logical));
    // 放到**光标所在显示器**中央：多屏时用户刚在哪儿操作就在哪儿弹。
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten());
    if let Some(m) = monitor {
        let scale = m.scale_factor();
        let work = m.work_area();
        let x =
            (work.position.x as f64 + (work.size.width as f64 - w_logical * scale) / 2.0) / scale;
        let y =
            (work.position.y as f64 + (work.size.height as f64 - h_logical * scale) / 2.0) / scale;
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
    let _ = w.show();
    let _ = w.set_focus();
}

/// 输入窗回传结果：`Some` = 确定（空串按「留空」报错）、`None` = 取消 / Esc。
#[tauri::command]
pub(crate) fn prompt_done(state: tauri::State<'_, KadaState>, value: Option<String>) {
    if let Some(tx) = state.prompt_tx.lock().unwrap().take() {
        let _ = tx.send(value);
    }
}

/// 读取当前输入框载荷（输入窗懒创建后，前端加载时兜底渲染）。
#[tauri::command]
pub(crate) fn get_prompt_payload(state: tauri::State<'_, KadaState>) -> Option<PromptPayload> {
    state.prompt_payload.lock().unwrap().clone()
}
