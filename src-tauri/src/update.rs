//! 软件更新：Tauri updater + GitHub Releases + Ed25519 签名。
//!
//! 更新源是仓库 Releases 上的 `latest.json`（见 `src-tauri/tauri.conf.json` 的
//! `plugins.updater`）。**公钥内嵌在配置里（随仓库），发布包由私钥签名**，客户端只认
//! 签名——私钥只在 CI Secret 里，泄露才需轮换公钥并随新版客户端下发。
//!
//! 三种触发方式见 [`Mode`]。要点：
//!
//! - 启动检查（`Auto`）完全静默——网络不通、还没发过 Release、清单里没有本平台条目
//!   都不该打扰用户，只有**真发现新版本**才提示一次。
//! - 手动检查（`Manual`）一定给反馈：无更新、出错都弹窗，发现新版本先确认再装。
//! - **Windows 上「安装」这一步会退出应用**（安装器限制，见 Tauri updater 文档）：
//!   `download_and_install` 走到安装时进程退出，由安装器拉起新版本。所以安装后不需要
//!   （也来不及）再由本进程做收尾，更不该在启动时自动走到安装——那会把用户正在用的
//!   应用突然关掉。启动检查因此只提示、不安装。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

/// 启动后台检查的延迟（秒）：避开启动瞬间的钩子安装与窗口初始化。
const STARTUP_CHECK_DELAY_SECS: u64 = 8;

/// 更新阶段（连同该阶段的字段一起序列化给前端，前端按 `phase` 分支渲染）。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum UpdatePhase {
    /// 本次运行还没查过。
    Idle,
    /// 正在检查清单。
    Checking,
    /// 已是最新版本。
    UpToDate,
    /// 发现新版本，尚未下载。
    Available { version: String, notes: Option<String> },
    /// 正在下载。
    Downloading { version: String, percent: Option<f64> },
    /// 下载完成，正在安装（Windows 上随后应用会退出）。
    Installing { version: String },
    /// 出错（网络不通、签名校验失败等）。
    Error { message: String },
}

impl UpdatePhase {
    /// 是否进行中：进行中不再发起第二次检查（托盘菜单可能被连点）。
    fn busy(&self) -> bool {
        matches!(
            self,
            UpdatePhase::Checking | UpdatePhase::Downloading { .. } | UpdatePhase::Installing { .. }
        )
    }
}

/// 由累计下载字节与总字节算整数百分比；总字节未知 / 为 0 时返回 `None`（不推进度）。
/// 累计超过总量时按 100 封顶（分片回调可能略过总量）。
fn progress_percent(got: u64, total: Option<u64>) -> Option<u64> {
    let total = total.filter(|t| *t > 0)?;
    // u128 中间量：`got * 100` 在超大 total（理论上）下会让 u64 溢出。
    let percent = (got.min(total) as u128 * 100 / total as u128).min(100);
    Some(percent as u64)
}

/// 进度节流：整数百分比与上次相同就不推（返回 false，调用方跳过 emit）。
/// 首次调用（`last` = `u64::MAX`）一定推。下载分片回调极密集，每片都 emit 会把
/// IPC 与前端渲染刷爆，所以只按整数百分比变化推。
fn should_emit_progress(last: &AtomicU64, percent: u64) -> bool {
    last.swap(percent, Ordering::Relaxed) != percent
}

/// 更新状态快照：设置页 `get_update_status` 读取，也随 `update-status` 事件推送。
#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    /// 当前版本（取自 `tauri.conf.json` 的 `version`）。
    pub current: String,
    #[serde(flatten)]
    pub phase: UpdatePhase,
}

/// 更新器运行时状态（`app.manage` 托管）。
pub struct UpdateState {
    status: Mutex<UpdateStatus>,
}

impl UpdateState {
    pub fn new(current: String) -> Self {
        Self { status: Mutex::new(UpdateStatus { current, phase: UpdatePhase::Idle }) }
    }

    /// 当前状态快照（UI 打开晚于检查时靠它兜底，事件只推增量）。
    pub fn snapshot(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }
}

/// 触发方式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 启动后台检查：静默，仅在发现新版本时用气泡提示一次。
    Auto,
    /// 手动检查：无更新/出错也弹窗反馈；发现新版本先确认再下载安装。
    Manual,
}

/// 写入阶段并推给前端（`update-status` 事件；主窗口不存在时事件自然落空，无副作用）。
fn set_phase(app: &tauri::AppHandle, phase: UpdatePhase) {
    let status = {
        let state = app.state::<UpdateState>();
        let mut status = state.status.lock().unwrap();
        status.phase = phase;
        status.clone()
    };
    let _ = tauri::Emitter::emit(app, "update-status", status);
}

/// 记失败：写状态；非静默模式额外弹窗（静默模式只更新设置页状态，不打扰使用）。
fn fail(app: &tauri::AppHandle, mode: Mode, message: String) {
    set_phase(app, UpdatePhase::Error { message: message.clone() });
    if mode != Mode::Auto {
        app.dialog()
            .message(message)
            .title("检查更新失败")
            .kind(MessageDialogKind::Error)
            .show(|_| {});
    }
}

/// 发起一次检查（已在检查/下载/安装中则忽略）。
pub fn spawn(app: &tauri::AppHandle, mode: Mode) {
    if app.state::<UpdateState>().snapshot().phase.busy() {
        return;
    }
    set_phase(app, UpdatePhase::Checking);
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run(app, mode).await });
}

/// 启动后台检查：延迟 [`STARTUP_CHECK_DELAY_SECS`] 秒再查，且**只在 release 构建里跑**。
///
/// dev 构建查的是同一个 Releases，而版本号还是开发中的号，「每次 `tauri dev` 都发一次
/// 网络请求」没有意义；调试更新流程请用托盘「检查更新」。
pub fn spawn_startup_check(app: &tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(STARTUP_CHECK_DELAY_SECS));
        spawn(&app, Mode::Auto);
    });
}

/// 检查 + 按模式决定后续动作。
async fn run(app: tauri::AppHandle, mode: Mode) {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => return fail(&app, mode, format!("更新器初始化失败：{e}")),
    };

    let found = match updater.check().await {
        Ok(found) => found,
        Err(e) => return fail(&app, mode, format!("检查更新失败：{e}")),
    };

    let Some(update) = found else {
        set_phase(&app, UpdatePhase::UpToDate);
        if mode == Mode::Manual {
            let current = app.package_info().version.to_string();
            app.dialog()
                .message(format!("当前版本 v{current}，已是最新版本。"))
                .title("检查更新")
                .show(|_| {});
        }
        return;
    };

    let version = update.version.clone();
    set_phase(&app, UpdatePhase::Available { version: version.clone(), notes: update.body.clone() });

    match mode {
        // 启动时静默发现：只提示，安装交给用户手动触发（安装会退出应用，不能替用户决定）。
        Mode::Auto => {
            crate::show_toast_note(&app, &format!("发现新版本 {version}"), "托盘右键「检查更新」可安装");
        }
        // 手动检查：说清「安装会退出应用」再让用户选。
        Mode::Manual => {
            let notes = update.body.clone().unwrap_or_default();
            let current = app.package_info().version.to_string();
            let message = if notes.trim().is_empty() {
                format!("发现新版本 v{version}（当前 v{current}）。\n\n现在下载并安装吗？安装时应用会退出，完成后自动打开新版本。")
            } else {
                format!("发现新版本 v{version}（当前 v{current}）。\n\n更新说明：\n{notes}\n\n现在下载并安装吗？安装时应用会退出，完成后自动打开新版本。")
            };
            let app_for_confirm = app.clone();
            app.dialog()
                .message(message)
                .title("发现新版本")
                .buttons(MessageDialogButtons::OkCancelCustom(
                    "下载并安装".into(),
                    "稍后".into(),
                ))
                .show(move |confirmed| {
                    if confirmed {
                        spawn_install(app_for_confirm);
                    }
                });
        }
    }
}

/// 下载并安装（下载带进度，安装后由安装器拉起新版本）。
///
/// 这里**重新拉一次清单**而不是把上次查到的 [`Update`] 存在状态里：换来的是一句
/// 「点安装时以最新清单为准」，也避免把一个可能过期的句柄长期驻留在状态中。
pub fn spawn_install(app: tauri::AppHandle) {
    if app.state::<UpdateState>().snapshot().phase.busy() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let updater = match app.updater() {
            Ok(updater) => updater,
            Err(e) => return fail(&app, Mode::Manual, format!("更新器初始化失败：{e}")),
        };
        let update = match updater.check().await {
            Ok(Some(update)) => update,
            Ok(None) => {
                set_phase(&app, UpdatePhase::UpToDate);
                app.dialog().message("已是最新版本，无需安装。").title("检查更新").show(|_| {});
                return;
            }
            Err(e) => return fail(&app, Mode::Manual, format!("检查更新失败：{e}")),
        };

        let version = update.version.clone();
        set_phase(&app, UpdatePhase::Downloading { version: version.clone(), percent: None });

        // 进度回调按「整数百分比变化」才推一次事件：下载分片回调极密集，
        // 每片都 emit 会把 IPC 与前端渲染刷爆。
        let downloaded = AtomicU64::new(0);
        let last_percent = AtomicU64::new(u64::MAX);
        let app_for_progress = app.clone();
        let version_for_progress = version.clone();
        let result = update
            .download_and_install(
                move |chunk: usize, total: Option<u64>| {
                    let got = downloaded.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                    let Some(percent) = progress_percent(got, total) else { return };
                    if !should_emit_progress(&last_percent, percent) {
                        return;
                    }
                    set_phase(
                        &app_for_progress,
                        UpdatePhase::Downloading {
                            version: version_for_progress.clone(),
                            percent: Some(percent as f64),
                        },
                    );
                },
                || {},
            )
            .await;

        match result {
            // Windows 上走到这里之前进程已被安装器接管（插件内部 `std::process::exit(0)`），
            // 能返回说明安装器还没退出应用；其它平台（Linux 的 deb / AppImage）装完**不会**
            // 拉起新版本，不自己重启就一直跑着旧二进制——等于「更新了但没生效」。
            Ok(()) => {
                set_phase(&app, UpdatePhase::Installing { version });
                app.restart();
            }
            Err(e) => fail(&app, Mode::Manual, format!("下载或安装失败：{e}")),
        }
    });
}

/// 读取更新状态（设置页挂载时兜底拉一次；后续靠 `update-status` 事件）。
#[tauri::command]
pub fn get_update_status(state: tauri::State<'_, UpdateState>) -> UpdateStatus {
    state.snapshot()
}

/// 手动检查更新（托盘菜单「检查更新」与设置页按钮共用）。
#[tauri::command]
pub fn check_update(app: tauri::AppHandle) {
    spawn(&app, Mode::Manual);
}

/// 下载并安装当前可用版本（设置页「下载并安装」）。
#[tauri::command]
pub fn install_update(app: tauri::AppHandle) {
    spawn_install(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_only_while_in_flight() {
        assert!(!UpdatePhase::Idle.busy());
        assert!(!UpdatePhase::UpToDate.busy());
        assert!(!UpdatePhase::Available { version: "1".into(), notes: None }.busy());
        assert!(!UpdatePhase::Error { message: "x".into() }.busy());
        assert!(UpdatePhase::Checking.busy());
        assert!(UpdatePhase::Downloading { version: "1".into(), percent: None }.busy());
        assert!(UpdatePhase::Installing { version: "1".into() }.busy());
    }

    /// 前端按 `phase` 字段分支渲染，序列化形状是前后端契约：改这里就要同步改 UI。
    #[test]
    fn status_serialization_shape() {
        let status = UpdateStatus {
            current: "0.1.0".into(),
            phase: UpdatePhase::Available { version: "0.2.0".into(), notes: Some("修了些东西".into()) },
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["phase"], "available");
        assert_eq!(json["current"], "0.1.0");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["notes"], "修了些东西");

        let idle = serde_json::to_value(UpdateStatus { current: "0.1.0".into(), phase: UpdatePhase::Idle }).unwrap();
        assert_eq!(idle["phase"], "idle");
    }

    #[test]
    fn progress_percent_clamps_and_ignores_unknown_total() {
        // 总字节未知（None / 0）→ 不推进度（否则会算出除零或恒 0 的假进度）。
        assert_eq!(progress_percent(500, None), None);
        assert_eq!(progress_percent(500, Some(0)), None);
        // 正常换算。
        assert_eq!(progress_percent(0, Some(1000)), Some(0));
        assert_eq!(progress_percent(250, Some(1000)), Some(25));
        assert_eq!(progress_percent(1000, Some(1000)), Some(100));
        // 累计超过总量（分片回调可能略过）→ 封顶 100，不溢出。
        assert_eq!(progress_percent(2000, Some(1000)), Some(100));
        // 极端大小不溢出（内部用 u128 中间量）。
        assert_eq!(progress_percent(u64::MAX, Some(u64::MAX)), Some(100));
    }

    #[test]
    fn progress_throttle_only_emits_on_percent_change() {
        let last = AtomicU64::new(u64::MAX);
        // 首次 0% 必须推（初始哨兵 u64::MAX ≠ 0）。
        assert!(should_emit_progress(&last, 0));
        // 同一整数百分比的分片不再推——这是避免 IPC 刷爆的关键。
        assert!(!should_emit_progress(&last, 0));
        assert!(!should_emit_progress(&last, 0));
        // 变化才推。
        assert!(should_emit_progress(&last, 1));
        assert!(!should_emit_progress(&last, 1));
        assert!(should_emit_progress(&last, 100));
        assert!(!should_emit_progress(&last, 100));
    }
}
