// 配置读写命令（T-04 从 lib.rs 搬出）：`get_config` / `set_config`。
// 写盘口径见 说明书 §5.6：先清洗、原子写、`cfg_watch` 销账、广播 `config-updated`。

use tauri::Emitter;

use crate::{
    apply_hints_visible, config_io, push_sync, sanitize_config, sync_autostart, Config, KadaState,
};

/// 读取当前配置。
#[tauri::command]
pub(crate) fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 保存配置：先逐条清洗（坏触发键/动作/改键被单独忽略，不影响其余配置），
/// 写入磁盘并即时生效。返回被忽略内容的说明（供 UI 提示），只有真正失败（写盘等）才报错。
/// 落盘走 [`config_io::save`]（原子替换 + 留存上次良好副本），不会留下半截 JSON。
#[tauri::command]
pub(crate) fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    config: Config,
) -> Result<Vec<String>, String> {
    let (clean, ignored) = sanitize_config(&config);
    let autostart = clean.settings.autostart;
    // 提示框的显示开关也在这份配置里（设置页勾选 / 取消就走的这条路）：改了就顺带把窗口与
    // 托盘勾选对齐——否则「设置里勾上了但窗口不出来」得重启才知道。
    let hints_visible = clean.settings.hints.visible;
    let hints_before = state.config.read().unwrap().settings.hints.visible;
    // 「写盘 + 改内存 + 给监听销账」三件事同持 `cfg_watch` 锁：巡检线程拿同一把锁，
    // 不会插在中间把刚写的配置当成外部修改再加载一遍（外部真的改了也照样检得出）。
    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &clean)?;
    *state.config.write().unwrap() = clean;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(&app, autostart);
    // 广播给别的窗口（提示框窗口据 `config-updated` 重画列表，主界面据它跟上提示框的位置 /
    // 外观）——`set_config` 只改内存不通知的话，提示框会一直显示保存前的旧快捷键。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    if hints_visible != hints_before {
        apply_hints_visible(&app, hints_visible);
    }
    Ok(ignored)
}
