// 执行结果命令（T-04 从 lib.rs 搬出）：消息中心列表、结果弹窗、命令日志、执行记录读取。
// 数据结构与写入在根模块 `results.rs`，这里只做命令层（见 说明书 §5.4 / §5.7）。

use crate::*;

/// 读取消息中心全部命令结果（最新在前）。
#[tauri::command]
pub(crate) fn get_command_results(state: tauri::State<'_, KadaState>) -> Vec<CommandResult> {
    // 倒序产出，免去先克隆整表再 reverse 的那一趟。
    state
        .results
        .lock()
        .unwrap()
        .iter()
        .rev()
        .cloned()
        .collect()
}

/// 当前是否有未读命令结果（红点）。
#[tauri::command]
pub(crate) fn get_unread(state: tauri::State<'_, KadaState>) -> bool {
    state.unread.load(Ordering::Relaxed)
}

/// 读取最近一次触发气泡的载荷（toast 窗口懒创建后，前端加载时调用兜底渲染）。
#[tauri::command]
pub(crate) fn get_toast_payload(state: tauri::State<'_, KadaState>) -> Option<ToastPayload> {
    state.toast.lock().unwrap().clone()
}

/// 标记全部已读（熄灭红点，还原托盘图标）。
#[tauri::command]
pub(crate) fn mark_results_read(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

/// 冷启动补发（CS-11）：前端启动并注册好监听器后取走没弹出来的结果（取走即清）。
#[tauri::command]
pub(crate) fn take_pending_result_modal(
    state: tauri::State<'_, KadaState>,
) -> Option<CommandResult> {
    state.pending_result_modal.lock().unwrap().take()
}

/// 复制文本到系统剪贴板（CS-10）：走平台注入层，不依赖 WebView 的
/// `navigator.clipboard` 权限（各平台 / 各 WebView 不一致）。
#[tauri::command]
pub(crate) fn copy_result_text(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("没有可复制的文本".into());
    }
    input::simulate::set_clipboard_text(&text).map_err(|e| format!("复制失败：{e}"))
}

/// 打开完整命令日志（CS-10）：只允许 `app_data_dir()/logs/commands/` 下的文件，
/// 用系统文件管理器定位到该文件（Windows `explorer /select,`、macOS `open -R`、
/// Linux `xdg-open <父目录>`）。
#[tauri::command]
pub(crate) fn open_result_log(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let dir = command_logs_dir(&app)
        .ok_or_else(|| "拿不到应用数据目录".to_string())?
        .canonicalize()
        .map_err(|e| format!("命令日志目录不存在：{e}"))?;
    let file = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("日志文件不存在：{e}"))?;
    if !file.starts_with(&dir) || !file.is_file() {
        return Err("只能打开命令日志目录里的文件".into());
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", file.display()))
            .spawn()
            .map_err(|e| format!("打开文件管理器失败：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&file)
            .spawn()
            .map_err(|e| format!("打开 Finder 失败：{e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        let parent = file
            .parent()
            .ok_or_else(|| "日志文件没有父目录".to_string())?;
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| format!("打开文件管理器失败：{e}"))?;
    }
    Ok(())
}

/// 清理完整命令日志（CS-21）：删除 `app_data_dir()/logs/commands/` 下的文件，返回删除数量。
/// 只动这一个目录里的常规文件——目录本身、配置与别的日志都不碰。
#[tauri::command]
pub(crate) fn clear_command_logs(app: tauri::AppHandle) -> Result<usize, String> {
    let dir = match command_logs_dir(&app) {
        Some(d) => d,
        None => return Err("拿不到应用数据目录".into()),
    };
    if !dir.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("读取日志目录失败：{e}"))? {
        let entry = entry.map_err(|e| format!("读取日志目录失败：{e}"))?;
        let path = entry.path();
        if path.is_file() && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// 清空消息中心（命令结果与执行记录一起清，同时熄灭红点）。
#[tauri::command]
pub(crate) fn clear_command_results(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.results.lock().unwrap().clear();
    state.runs.lock().unwrap().clear();
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

/// 读取全部执行记录（最新在前）。界面的「执行记录」标签页拿它铺列表。
#[tauri::command]
pub(crate) fn get_run_records(state: tauri::State<'_, KadaState>) -> Vec<RunRecord> {
    state.runs.lock().unwrap().clone()
}
