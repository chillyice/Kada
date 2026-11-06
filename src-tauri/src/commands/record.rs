// 录制 / 暂停 / 手动执行命令（T-04 从 lib.rs 搬出）：录入暂停租约、中止、单条执行、宏与鼠标 /
// 触摸板录制。会话状态留在 lib.rs（定时器线程也要读），这里只做命令入口（见 说明书 §5.1 / §5.2）。

use crate::*;

/// 暂停/恢复快捷键触发：录入组合键/序列/和弦时暂停，避免自触发。
/// 暂停是限时租约（[`PAUSE_LEASE_MS`]），前端若被中断来不及解除也不会永久卡死。
#[tauri::command]
pub(crate) fn set_paused(state: tauri::State<'_, KadaState>, paused: bool) {
    set_pause_lease(&state.paused_until, paused);
}

/// 中止正在执行的动作：剩余动作不再执行、正在跑的命令/脚本（含子进程）立即强制终止。
/// 返回中止请求发出时**正在执行**的动作链数量——0 表示此刻没有可停的东西，前端据此
/// 给出「当前没有正在执行的动作」，而不是假装停成功了。
#[tauri::command]
pub(crate) fn abort_actions() -> usize {
    let running = actions_abort::running();
    actions_abort::request();
    running
}

/// 编辑页「执行」按钮：把当前草稿的动作串当一次触发跑掉（不落盘、不看 `when` 与启停），
/// 走的与快捷键命中同一条 [`fire`] 路径——气泡、消息中心、`show_output` 弹窗一并复用。
#[tauri::command]
pub(crate) fn run_entry(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    actions: Vec<Action>,
    name: String,
) {
    // 超时与注入模式在锁内取一次（`fire` 不再二次读配置，避免与保存写锁多一层纠缠）。
    let (timeout_ms, text_mode) = {
        let g = state.config.read().unwrap();
        (g.settings.action_timeout_ms, g.settings.text_inject_mode)
    };
    fire(
        app,
        state.results.clone(),
        state.unread.clone(),
        state.runs.clone(),
        actions,
        "手动执行".into(),
        name,
        timeout_ms,
        text_mode,
        None,
        None,
    );
}

/// 录制不可用时给用户的解释（[`input::hooks_supported`] 为 false 时用）。
///
/// false 不表示「平台没实现」，而是「输入层没起来」——macOS 是还没拿到辅助功能权限，
/// Linux 是 input 组 / udev 没配好（启动时消息中心里已有一条能照抄的修复指引）。
/// 说成「平台不支持」会让用户彻底放弃（他不知道要去修权限）。
#[cfg(any(target_os = "macos", target_os = "linux"))]
const RECORD_UNAVAILABLE: &str = "宏录制不可用：输入层未启动（见「消息」页启动时的权限指引），\
    按指引修复权限并重启 Kada 后可用";
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
const RECORD_UNAVAILABLE: &str = "当前平台暂不支持宏录制";

/// 开始录制宏：快捷键/改键随即暂停，所有按键进时间线。
#[tauri::command]
pub(crate) fn start_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
    if !input::hooks_supported() {
        return Err(RECORD_UNAVAILABLE.into());
    }
    if state.mrec.lock().unwrap().is_some() {
        return Err("已在录制中".into());
    }
    let mut g = state.rec.lock().unwrap();
    if g.is_some() {
        return Err("已在录制中".into());
    }
    *g = Some(Recorder::new());
    set_pause_lease(&state.paused_until, true);
    Ok(())
}

/// 停止录制，返回时间线步骤（可直接作为 Sequence 动作保存）。
#[tauri::command]
pub(crate) fn stop_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Action>, String> {
    if !input::hooks_supported() {
        return Err(RECORD_UNAVAILABLE.into());
    }
    let mut g = state.rec.lock().unwrap();
    let Some(rec) = g.take() else {
        return Err("没有正在进行的录制".into());
    };
    set_pause_lease(&state.paused_until, false);
    Ok(rec.finish())
}

/// 开始录制鼠标操作：快捷键/改键随即暂停，之后的鼠标点击 / 滚轮进时间线（按键不记）。
///
/// 钩子层按「录制中」临时挂上鼠标钩子（配置没用鼠标键触发时也要监听），并把当前光标
/// 位置记成位移原点——每次触发的位移即「移动到该处再触发」的移动量。
#[tauri::command]
pub(crate) fn start_mouse_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
    if !input::hooks_supported() {
        return Err(RECORD_UNAVAILABLE.into());
    }
    if state.rec.lock().unwrap().is_some() {
        return Err("已在录制中".into());
    }
    {
        let mut g = state.mrec.lock().unwrap();
        if g.is_some() {
            return Err("已在录制中".into());
        }
        *g = Some(MouseRecorder::default());
    }
    set_pause_lease(&state.paused_until, true);
    let mrec = state.mrec.clone();
    input::set_mouse_recorder(Some(Box::new(move |ev| {
        if let Some(r) = mrec.lock().unwrap().as_mut() {
            r.push(&ev);
        }
    })));
    Ok(())
}

/// 停止鼠标录制，返回鼠标操作动作时间线（`Move` / `Click` / `Scroll`）。
#[tauri::command]
pub(crate) fn stop_mouse_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Action>, String> {
    if !input::hooks_supported() {
        return Err(RECORD_UNAVAILABLE.into());
    }
    input::set_mouse_recorder(None);
    let rec = state.mrec.lock().unwrap().take();
    let Some(rec) = rec else {
        return Err("没有正在进行的鼠标录制".into());
    };
    set_pause_lease(&state.paused_until, false);
    Ok(rec.into_actions())
}

/// 设置页「从触摸板录一笔」：开一个**只回填配置、不执行动作**的采集会话。
///
/// 与宏录制不同，这里**不能**暂停快捷键：采集本身就是靠钩子接住触发键的。
/// 录制期间 `TRACKPAD_RECORD` 同时充当闸门覆盖（见定时器线程里的 `trackpad_on`）。
#[tauri::command]
pub(crate) fn start_trackpad_record(key: String) -> Result<(), String> {
    let k: Key = key.parse().map_err(|_| format!("无法识别的键名：{key}"))?;
    if k.is_mouse() {
        return Err("触发键只能是键盘键：鼠标键已经是右键手势那条路了".into());
    }
    if !kada_hook::trackpad::supported() {
        return Err("当前平台还不支持采集触摸板手势（目前只接了 Windows）".into());
    }
    *TRACKPAD_RECORD.lock().unwrap() = Some(k);
    // 立刻开闸，不等定时器那一拍；下一拍也会按「录制中」把闸门维持住。
    kada_hook::trackpad::set_enabled(true);
    // 鼠标钩子同样要立刻挂上：新建第一条时配置里还没有启用的触摸板手势，
    // 不临时挂钩子就收不到触摸板合成的鼠标移动（见定时器里的 `use_mouse`）。
    input::set_mouse_enabled(true);
    Ok(())
}

/// 结束「从触摸板录一笔」会话：清掉会话并复位采集状态（闸门由定时器按配置收回）。
#[tauri::command]
pub(crate) fn stop_trackpad_record() {
    *TRACKPAD_RECORD.lock().unwrap() = None;
    kada_hook::trackpad::reset();
}
