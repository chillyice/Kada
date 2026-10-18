//! 软件更新：Tauri updater + 可切换更新源 + Ed25519 签名。
//!
//! 更新源由 `Settings.update_source` 决定（见 `kada_core::UpdateSource`）：
//! **默认 ihomy 镜像**（服务器每天从 GitHub Releases 同步最新版与安装包，国内可直连），
//! 能直连 GitHub 的人可在设置页切到官方 Releases。两边的清单格式相同、签名原样保留，
//! 客户端只认签名——镜像换 URL 不影响校验。
//!
//! 三种触发方式见 [`Mode`]。要点：
//!
//! - 启动检查（`Auto`）完全静默——网络不通、还没发过 Release、清单里没有本平台条目
//!   都不该打扰用户，只有**真发现新版本**才提示一次；之后每 24 小时再查一次
//!   （应用常驻时也能收到新版本，见 [`spawn_periodic_check`]）。
//! - 手动检查（`Manual`）一定给反馈：无更新、出错都弹窗，发现新版本先确认再装。
//! - **Windows 上「安装」这一步会退出应用**（安装器限制，见 Tauri updater 文档）：
//!   `download_and_install` 走到安装时进程退出，由安装器拉起新版本。所以安装后不需要
//!   （也来不及）再由本进程做收尾，更不该在启动时自动走到安装——那会把用户正在用的
//!   应用突然关掉。启动检查因此只提示、不安装。

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use kada_core::UpdateSource;
use serde::Serialize;
use tauri::{Manager, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::UpdaterExt;

/// 启动后台检查的延迟（秒）：避开启动瞬间的钩子安装与窗口初始化。
const STARTUP_CHECK_DELAY_SECS: u64 = 8;

/// 启动检查之后的自动检查间隔：应用常驻（托盘常开）时也要能在一天内看到新版本。
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

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

/// 当前程序的安装形态。更新这一步**不允许改变形态**：安装版只能原地（注册表记录的
/// 原目录）更新，免安装版只能下载新包手动替换 exe，两者不能互相转换。
///
/// 判据是「当前 exe 所在目录 == NSIS 安装器写在注册表里的安装目录」，而不是
/// 「注册表里有没有值」：同一台机器上既装了安装版、又解压了一份便携 exe 时，
/// 两个副本必须各按自己的形态工作——只查注册表会把便携 exe 误判成安装版，
/// 一点更新就被装成安装版，还多出一份副本。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    /// NSIS 安装版：更新走安装器，装回注册表记录的原目录。
    Installed,
    /// 免安装版（裸 exe）：不参与自动安装更新，只能手动替换。
    Portable,
}

/// 注册表里 NSIS 安装器写下的安装目录（`HKCU` = 当前用户安装，`HKLM` = 全机安装）。
#[cfg(windows)]
fn registered_install_dir() -> Option<PathBuf> {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

    // NSIS 模板的 `${MANUPRODUCTKEY}`：`Software\{MANUFACTURER}\{PRODUCTNAME}`，
    // 默认值就是安装时的 `$INSTDIR`（见打包模板的 `WriteRegStr SHCTX ...`）。
    const KEY: &str = r"Software\kada\Kada";
    for root in [CURRENT_USER, LOCAL_MACHINE] {
        let Ok(key) = root.open(KEY) else { continue };
        let Ok(dir) = key.get_string("") else { continue };
        // 防御：卸载项的 `InstallLocation` 是带引号的，个别安装器来源也可能照搬过来。
        let dir = dir.trim().trim_matches('"');
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    None
}

/// 目录比较（本函数只服务 Windows 判定）：大小写不敏感、尾部分隔符可有可无。
/// 不做 `canonicalize`——那要求目录存在，还要处理 `\\?\` 前缀。
fn same_dir(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    }
    norm(a) == norm(b)
}

/// 纯判定（便于单测）：exe 目录与注册表记录的安装目录一致才算安装版。
/// 取不到自身目录或注册表没有记录时按免安装处理——**宁可只提示手动替换，
/// 也不擅自跑安装器**。
fn classify_install(exe_dir: Option<&Path>, registered: Option<&Path>) -> InstallKind {
    match (exe_dir, registered) {
        (Some(dir), Some(installed)) if same_dir(dir, installed) => InstallKind::Installed,
        _ => InstallKind::Portable,
    }
}

/// 当前进程的安装形态。非 Windows 只有安装版一种分发形态
/// （macOS dmg / Linux deb、AppImage），不存在「免安装」分支。
pub fn install_kind() -> InstallKind {
    #[cfg(windows)]
    {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        classify_install(exe_dir.as_deref(), registered_install_dir().as_deref())
    }
    #[cfg(not(windows))]
    {
        InstallKind::Installed
    }
}

/// 更新状态快照：设置页 `get_update_status` 读取，也随 `update-status` 事件推送。
#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    /// 当前版本（取自 `tauri.conf.json` 的 `version`）。
    pub current: String,
    /// 安装形态：前端据此隐藏「下载并安装」（免安装版只能手动替换）。
    pub kind: InstallKind,
    #[serde(flatten)]
    pub phase: UpdatePhase,
}

/// 更新器运行时状态（`app.manage` 托管）。
pub struct UpdateState {
    status: Mutex<UpdateStatus>,
}

impl UpdateState {
    pub fn new(current: String) -> Self {
        Self {
            status: Mutex::new(UpdateStatus {
                current,
                kind: install_kind(),
                phase: UpdatePhase::Idle,
            }),
        }
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

/// 当前选择的更新源。拿不到状态（极早的定时器线程）时按默认源（ihomy）走。
fn current_source(app: &tauri::AppHandle) -> UpdateSource {
    app.try_state::<crate::KadaState>()
        .and_then(|state| state.config.read().ok().map(|cfg| cfg.settings.update_source))
        .unwrap_or_default()
}

/// 当前选择更新源的 `latest.json` 地址。
///
/// 每次检查都重新读配置：设置页切换后，下一次检查（含手动「检查更新」）立即生效，
/// 不需要重启。
fn endpoint(app: &tauri::AppHandle) -> Url {
    Url::parse(current_source(app).endpoint()).expect("内置更新源地址必须是合法 URL")
}

/// 当前更新源对应的发布页：免安装版只能手动更新，提示框里的「打开发布页」用它。
fn download_page(app: &tauri::AppHandle) -> &'static str {
    match current_source(app) {
        UpdateSource::Ihomy => "https://ihomy.top/kada",
        UpdateSource::Github => "https://github.com/chillyice/Kada/releases/latest",
    }
}

/// 按当前设置装配更新器：`endpoints` 覆盖 `tauri.conf.json` 里的静态默认。
fn build_updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    app.updater_builder()
        .endpoints(vec![endpoint(app)])
        .map_err(|e| format!("更新源地址无效：{e}"))?
        .build()
        .map_err(|e| format!("更新器初始化失败：{e}"))
}

/// 启动后台检查 + 每日复查：延迟 [`STARTUP_CHECK_DELAY_SECS`] 秒先查一次，之后每
/// [`CHECK_INTERVAL_SECS`] 再查一次（应用常驻时也能在新版本发布后一天内看到）。
///
/// **只在 release 构建里跑**：dev 查的是线上清单，而版本号还是开发中的号，
/// 「每次 `tauri dev` 都发一次网络请求」没有意义；调试更新流程请用托盘「检查更新」。
pub fn spawn_periodic_check(app: &tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(STARTUP_CHECK_DELAY_SECS));
        loop {
            spawn(&app, Mode::Auto);
            std::thread::sleep(Duration::from_secs(CHECK_INTERVAL_SECS));
        }
    });
}

/// 检查 + 按模式决定后续动作。
async fn run(app: tauri::AppHandle, mode: Mode) {
    let updater = match build_updater(&app) {
        Ok(updater) => updater,
        Err(e) => return fail(&app, mode, e),
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

    // 免安装版不能走「下载并安装」：Windows 上 updater 装的是 NSIS 安装包，一点就把
    // 便携 exe 变成安装版（还会在安装目录多出一份副本）。免安装版的更新方式只有一种：
    // 下载新的免安装包、退出应用后替换 exe。
    if install_kind() == InstallKind::Portable {
        match mode {
            Mode::Auto => crate::show_toast_note(
                &app,
                &format!("发现新版本 {version}"),
                "免安装版请手动下载替换 kada.exe",
            ),
            Mode::Manual => {
                let notes = update.body.clone().unwrap_or_default();
                let current = app.package_info().version.to_string();
                let base = format!(
                    "发现新版本 v{version}（当前 v{current}）。\n\n\
                     当前运行的是免安装版（裸 exe），不参与自动安装更新：\
                     请到下载页获取新的免安装包，退出咔哒后替换 kada.exe。"
                );
                let message = if notes.trim().is_empty() {
                    base
                } else {
                    format!("{base}\n\n更新说明：\n{notes}")
                };
                let url = download_page(&app);
                app.dialog()
                    .message(message)
                    .title("发现新版本（免安装版）")
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        "打开发布页".into(),
                        "稍后".into(),
                    ))
                    .show(move |confirmed| {
                        if confirmed {
                            let _ = kada_actions::open_url(url);
                        }
                    });
            }
        }
        return;
    }

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
    // 兜底闸门：免安装版（裸 exe）绝不调用 updater 的安装器——那是「交叉改变安装方式」
    // 的唯一来源（装完会在安装目录多一份安装版，便携副本还留在原处）。
    // 前端对免安装版已经隐藏了「下载并安装」，这里防的是旧界面 / 直接调命令。
    if install_kind() == InstallKind::Portable {
        let url = download_page(&app);
        app.dialog()
            .message(
                "当前是免安装版（裸 exe），不能自动安装更新（那会把它变成安装版）。\n\n\
                 请到下载页获取新的免安装包，退出咔哒后替换 kada.exe。",
            )
            .title("免安装版更新")
            .buttons(MessageDialogButtons::OkCancelCustom(
                "打开发布页".into(),
                "知道了".into(),
            ))
            .show(move |confirmed| {
                if confirmed {
                    let _ = kada_actions::open_url(url);
                }
            });
        return;
    }
    if app.state::<UpdateState>().snapshot().phase.busy() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let updater = match build_updater(&app) {
            Ok(updater) => updater,
            Err(e) => return fail(&app, Mode::Manual, e),
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
            kind: InstallKind::Portable,
            phase: UpdatePhase::Available { version: "0.2.0".into(), notes: Some("修了些东西".into()) },
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["phase"], "available");
        assert_eq!(json["current"], "0.1.0");
        assert_eq!(json["kind"], "portable");
        assert_eq!(json["version"], "0.2.0");
        assert_eq!(json["notes"], "修了些东西");

        let idle = serde_json::to_value(UpdateStatus {
            current: "0.1.0".into(),
            kind: InstallKind::Installed,
            phase: UpdatePhase::Idle,
        })
        .unwrap();
        assert_eq!(idle["phase"], "idle");
        assert_eq!(idle["kind"], "installed");
    }

    /// 安装形态判定的铁律：**只有 exe 目录与注册表记录的安装目录一致才算安装版**。
    /// 便携 exe 即便跑在「装过安装版」的机器上，也绝不能被判成安装版（否则一次更新
    /// 就会把它装成安装版，安装方式被交叉改变）。
    #[test]
    fn install_kind_requires_exe_dir_to_match_registered_dir() {
        let installed = Path::new(r"C:\Users\me\AppData\Local\Kada");
        // 一致（含大小写 / 尾部分隔符差异）→ 安装版。
        assert_eq!(
            classify_install(Some(installed), Some(installed)),
            InstallKind::Installed
        );
        assert_eq!(
            classify_install(
                Some(Path::new(r"c:\users\ME\appdata\local\Kada\")),
                Some(installed)
            ),
            InstallKind::Installed
        );
        // 目录对不上（便携 exe，哪怕机器上有安装记录）→ 免安装版。
        assert_eq!(
            classify_install(Some(Path::new(r"D:\Tools\Kada")), Some(installed)),
            InstallKind::Portable
        );
        // 没有注册表记录 / 取不到自身目录 → 按免安装处理（不擅自跑安装器）。
        assert_eq!(classify_install(Some(installed), None), InstallKind::Portable);
        assert_eq!(classify_install(None, Some(installed)), InstallKind::Portable);
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
