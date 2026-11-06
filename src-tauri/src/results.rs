// 执行记录表与消息中心（T-02 从 lib.rs 搬出）：RunRecord 的增删改、命令完整日志落盘、托盘红点。
// 不参与输入判定，只做「记录 + 广播」（见 说明书 §5.7）。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{Emitter, Manager};

use kada_actions::{ActionStep, CommandResult, RunRecord};

use crate::{show_main_window, KadaState};

/// 执行记录表的条数上限（超出丢最旧的一条）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const MAX_RUNS: usize = 50;
/// 单条执行记录的动作条数上限（宏里套一大坨循环时会超；丢最旧的，中间的时间线仍完整）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const MAX_RUN_STEPS: usize = 500;

/// 新开一条执行记录（最新在前）并广播。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn start_run(
    app: &tauri::AppHandle,
    runs: &Mutex<Vec<RunRecord>>,
    unread: &AtomicBool,
    run: RunRecord,
) {
    {
        let mut g = runs.lock().unwrap();
        g.insert(0, run.clone());
        g.truncate(MAX_RUNS);
    }
    unread.store(true, Ordering::Relaxed);
    set_tray_unread(app, true);
    let _ = app.emit("run-update", run);
}

/// 就地改一条执行记录并返回改后的快照；记录已被清掉时返回 `None`。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn update_run(
    app: &tauri::AppHandle,
    runs: &Mutex<Vec<RunRecord>>,
    id: u64,
    f: impl FnOnce(&mut RunRecord),
) -> Option<RunRecord> {
    let snapshot = {
        let mut g = runs.lock().unwrap();
        let run = g.iter_mut().find(|r| r.id == id)?;
        f(run);
        if run.steps.len() > MAX_RUN_STEPS {
            run.steps.drain(..run.steps.len() - MAX_RUN_STEPS);
        }
        run.clone()
    };
    let _ = app.emit("run-update", &snapshot);
    Some(snapshot)
}

/// 按动作 id 合并一条执行记录：每个动作报两次（开始 `running` / 结束最终态，CS-07），
/// 同 id 第二次**覆盖**第一条，界面才是一条从「执行中」变「成功」的记录而不是两条。
/// 极端长链里正在跑的步骤可能已被 `MAX_RUN_STEPS` 裁掉，找不到就当新的一条 push
/// （时间线少一个起点，但不丢结论）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn upsert_step(steps: &mut Vec<ActionStep>, step: ActionStep) {
    if let Some(existing) = steps.iter_mut().find(|s| s.id == step.id) {
        *existing = step;
    } else {
        steps.push(step);
    }
}

/// 记一条动作执行记录（时间线的一个时间块 / 流水线的一个节点），顺带把总耗时推到最新。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn add_step(
    app: &tauri::AppHandle,
    runs: &Mutex<Vec<RunRecord>>,
    unread: &AtomicBool,
    id: u64,
    step: ActionStep,
) {
    let end = step.end_ms;
    let failed = step.status == "error";
    let snapshot = update_run(app, runs, id, move |run| {
        run.duration_ms = run.duration_ms.max(end);
        if failed && run.status != "abort" {
            run.status = "error".into();
        }
        upsert_step(&mut run.steps, step);
    });
    if snapshot.is_some() {
        unread.store(true, Ordering::Relaxed);
        set_tray_unread(app, true);
    }
}

/// 收尾：把整条记录的结论（跑完 / 有失败 / 被中止）定下来。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) fn finish_run(
    app: &tauri::AppHandle,
    runs: &Mutex<Vec<RunRecord>>,
    unread: &AtomicBool,
    id: u64,
    status: &'static str,
) {
    let _ = update_run(app, runs, id, |run| run.status = status.into());
    unread.store(true, Ordering::Relaxed);
    set_tray_unread(app, true);
}

/// 消息中心保留的最大条数（超出丢最旧的）。命令输出是内存态、重启即清，但常驻数周不设
/// 上限会把每条命令的全量输出都留在内存里。热串缓冲早有 64 上限，此处对齐。
const MAX_RESULTS: usize = 200;
/// 单条 stdout / stderr 保留的最大字节数（超出截断并加提示）。避免一条 `npm install`
/// 之类的大输出把整份消息中心撑大。
const MAX_RESULT_TEXT: usize = 16 * 1024;
/// 命令完整日志的保留上限（个文件，CS-10）：消息中心只留 16KB 截断版，全文在
/// `app_data_dir()/logs/commands/` 里，按修改时间删最旧的。
const MAX_COMMAND_LOGS: usize = 200;
/// 同秒内多次写日志的序号，避免文件名互相覆盖。
static COMMAND_LOG_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 把超长文本截到至多 `max` 字节，落在字符边界上（不腰断 UTF-8），并附截断提示。
fn truncate_text(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("\n…（输出过长，已截断）");
}

/// 完整命令日志的目录（CS-10）。拿不到应用数据目录时返回 `None`。
pub(crate) fn command_logs_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("logs").join("commands"))
}

/// 写一份完整日志；失败只 `eprintln`，绝不拖累结果进消息中心（CS-10）。
fn write_command_log(app: &tauri::AppHandle, result: &CommandResult) -> Option<String> {
    match write_command_log_inner(app, result) {
        Ok(path) => {
            prune_command_logs(app);
            Some(path.display().to_string())
        }
        Err(e) => {
            eprintln!("写命令日志失败: {e}");
            None
        }
    }
}

fn write_command_log_inner(
    app: &tauri::AppHandle,
    result: &CommandResult,
) -> Result<PathBuf, String> {
    use std::io::Write as _;

    let dir = command_logs_dir(app).ok_or_else(|| "拿不到应用数据目录".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建日志目录失败: {e}"))?;
    let serial = COMMAND_LOG_SERIAL.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(
        "{}-{serial:04}.log",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    let mut file = std::fs::File::create(&path).map_err(|e| format!("写日志文件失败: {e}"))?;
    let mut head = String::new();
    use std::fmt::Write as _;
    let _ = writeln!(head, "时间：{}", result.time);
    let _ = writeln!(head, "触发键：{}", result.trigger);
    let _ = writeln!(head, "名称：{}", result.name);
    let _ = writeln!(head, "动作：{}（{}）", result.kind, result.label);
    let _ = writeln!(head, "命令：{}", result.command);
    let _ = writeln!(
        head,
        "退出码：{}",
        result
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "无（未等待完成 / 进程未启动）".into())
    );
    if result.detached {
        let _ = writeln!(head, "等待：否（启动后不管，未捕获输出）");
    }
    file.write_all(head.as_bytes())
        .map_err(|e| format!("写日志文件失败: {e}"))?;
    // 输出超过内存上限时完整内容在 spool 临时文件里（CS-16）：直接流式拷进日志，
    // 不把整份输出重新读进内存（一个刷屏命令可能有几百 MB）。
    file.write_all(b"\n===== stdout =====\n")
        .and_then(|_| write_log_stream(&mut file, result.stdout_file.as_deref(), &result.stdout))
        .and_then(|_| file.write_all(b"\n===== stderr =====\n"))
        .and_then(|_| write_log_stream(&mut file, result.stderr_file.as_deref(), &result.stderr))
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("写日志文件失败: {e}"))?;
    drop(file);
    // spool 文件使命结束：完整内容已经进日志，临时文件不再保留。
    if let Some(p) = &result.stdout_file {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = &result.stderr_file {
        let _ = std::fs::remove_file(p);
    }
    // 明确不写 `KADA_VARS`：变量表可能含密码 / 私密文本（见计划 §8 设计口径）。
    Ok(path)
}

/// 日志里的一段输出：有 spool 文件就流式拷贝（失败退回内存尾巴），否则写内存里的文本。
fn write_log_stream(
    file: &mut std::fs::File,
    spool: Option<&str>,
    fallback: &str,
) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(p) = spool {
        if let Ok(mut src) = std::fs::File::open(p) {
            std::io::copy(&mut src, file)?;
            return Ok(());
        }
    }
    file.write_all(fallback.as_bytes())
}

/// 只保留最近 [`MAX_COMMAND_LOGS`] 份日志：按修改时间排序，删最旧的。
fn prune_command_logs(app: &tauri::AppHandle) {
    let dir = match command_logs_dir(app) {
        Some(d) => d,
        None => return,
    };
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    if logs.len() <= MAX_COMMAND_LOGS {
        return;
    }
    logs.sort_by_key(|(t, _)| *t);
    let drop = logs.len() - MAX_COMMAND_LOGS;
    for (_, path) in logs.into_iter().take(drop) {
        if let Err(e) = std::fs::remove_file(&path) {
            eprintln!("清理旧命令日志失败（{}）: {e}", path.display());
        }
    }
}

/// 记录一条命令结果：写入消息中心；弹窗开则唤起主窗口，否则亮未读红点。
/// 两种情况都会向主窗口发 `command-result` 事件。返回完整日志路径（若有），
/// 供 `run_chain` 抄进执行记录的 `log_path`（CS-10）。
pub(crate) fn commit_result(
    app: &tauri::AppHandle,
    results: &Mutex<Vec<CommandResult>>,
    unread: &AtomicBool,
    mut result: CommandResult,
) -> Option<String> {
    // 先落全量日志，再截断内存里的副本（CS-10）。
    if matches!(
        result.kind.as_str(),
        "cmd" | "powershell" | "script" | "inline_script"
    ) {
        result.log_path = write_command_log(app, &result);
    }
    // 无论日志写没写成，spool 临时文件都要清掉（写成功时上面已删，这里兜底）。
    if let Some(p) = &result.stdout_file {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = &result.stderr_file {
        let _ = std::fs::remove_file(p);
    }
    let log_path = result.log_path.clone();
    truncate_text(&mut result.stdout, MAX_RESULT_TEXT);
    truncate_text(&mut result.stderr, MAX_RESULT_TEXT);
    {
        let mut g = results.lock().unwrap();
        g.push(result.clone());
        // 超上限丢最旧的（消息中心按时间顺序追加，前面即最早）。
        if g.len() > MAX_RESULTS {
            let drop = g.len() - MAX_RESULTS;
            g.drain(0..drop);
        }
    }
    if result.show_output {
        // 冷启动补发（CS-11）：窗口还没建时 `emit` 没人接，先存一份，前端启动后取走。
        if app.get_webview_window("main").is_none() {
            if let Some(state) = app.try_state::<KadaState>() {
                *state.pending_result_modal.lock().unwrap() = Some(result.clone());
            }
        }
        show_main_window(app);
    } else {
        unread.store(true, Ordering::Relaxed);
        set_tray_unread(app, true);
    }
    let _ = app.emit("command-result", &result);
    log_path
}

/// 切换托盘图标的红点（未读态带红点，已读态还原基础图标）。
pub(crate) fn set_tray_unread(app: &tauri::AppHandle, unread: bool) {
    let state = app.state::<KadaState>();
    let tray = state.tray.lock().unwrap();
    if let Some(tray) = tray.as_ref() {
        let icon = if unread {
            state.tray_unread.clone()
        } else {
            state.tray_base.clone()
        };
        let _ = tray.set_icon(icon);
    }
}
