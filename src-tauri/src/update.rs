//! 软件更新：Tauri updater + 可切换更新源 + Ed25519 签名。
//!
//! 更新源由 `Settings.update_source` 决定（见 `kada_core::UpdateSource`）：
//! **默认 ihomy 镜像**（服务器每天从 GitHub Releases 同步最新版与安装包，国内可直连），
//! 能直连 GitHub 的人可在设置页切到官方 Releases。两边的清单格式相同、签名原样保留，
//! 客户端只认签名——镜像换 URL 不影响校验。
//!
//! **检查、下载、安装是三件独立的事**：
//!
//! - **检查**永远自动：启动延迟 8 秒查一次，之后每 24 小时再查（[`spawn_periodic_check`]）；
//!   手动「检查更新」只是立刻再查一次。
//! - **下载**：`Settings.auto_update` 勾着就在发现新版本后直接下（[`run`]）；没勾只能在
//!   设置页点「下载」（[`spawn_download`]）。下载完落盘到 `app_data_dir/updates/` 并在
//!   重启后仍能恢复（`pending.json`），不是一个只活在内存里的句柄。
//! - **安装**：勾了自动更新时由 [`spawn_auto_installer`] 择机——**长时间不用电脑**
//!   （[`kada_hook::idle_seconds`] ≥ [`AUTO_INSTALL_IDLE_SECS`]）时装，免安装版另在
//!   **下次启动**时装；没勾则只能手动点「安装」（[`spawn_install`]）。
//!
//! 安装形态（[`InstallKind`]）决定用哪种包、怎么装，且**绝不交叉**：安装版只拿 NSIS
//! 安装包、装回注册表记录的原目录（`update.install` 会退出应用，由安装器以 `/S /R`
//! 静默装完重启）；免安装版只拿清单里的 `windows-x86_64-portable` 裸 exe、在启动/空闲时
//! 原位替换 `kada.exe` 再拉起新进程。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use kada_actions::CommandResult;
use kada_core::UpdateSource;
use serde::{Deserialize, Serialize};
use tauri::{Manager, Url};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Error as UpdateError, Update, Updater, UpdaterExt};

/// 启动后台检查的延迟（秒）：避开启动瞬间的钩子安装与窗口初始化。
const STARTUP_CHECK_DELAY_SECS: u64 = 8;

/// 启动检查之后的自动检查间隔：应用常驻（托盘常开）时也要能在一天内看到新版本。
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

/// 清单检查的时限（秒）：`latest.json` 只有几 KB，这么久还没回来就是网络或更新源不可达。
/// 没有时限时「正在检查更新…」会无限转下去，用户既看不到结果也看不到失败原因。
///
/// **只作用于清单请求**：插件的 `Update` 句柄不带这个时限（`check()` 里 `timeout: None`），
/// 所以下载更新包不会被这 20 秒掐断（镜像带宽低时大包需要更久）。
const CHECK_TIMEOUT_SECS: u64 = 20;

/// 勾了「自动更新」时，「长时间不使用电脑」的判定门槛（秒）：最后一次键鼠输入至今超过
/// 它才装机。10 分钟足够区分「人在旁边」与「人走了」，又不至于让更新拖到第二天。
const AUTO_INSTALL_IDLE_SECS: u64 = 10 * 60;

/// 自动安装调度的巡检间隔（秒）：只在有已下载的更新包时才有实际动作。
const AUTO_INSTALL_POLL_SECS: u64 = 30;

/// 自动安装失败后的冷却（秒）：离线时别每 30 秒重试一次（会一直刷错误状态）。
const AUTO_INSTALL_RETRY_SECS: u64 = 10 * 60;

/// 更新阶段（连同该阶段的字段一起序列化给前端，前端按 `phase` 分支渲染）。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum UpdatePhase {
    /// 本次运行还没查过。
    Idle,
    /// 正在检查清单（下载前的清单核对也算）。
    Checking,
    /// 已是最新版本。
    UpToDate,
    /// 发现新版本，尚未下载。
    Available { version: String, notes: Option<String> },
    /// 正在下载。
    Downloading { version: String, percent: Option<f64> },
    /// 已下载、等安装（手动等用户点「安装」；自动更新等空闲 / 下次启动）。
    Ready { version: String },
    /// 正在安装（安装版随后应用会退出并由安装器重启；免安装版换完 exe 立刻重启）。
    Installing { version: String },
    /// 出错（网络不通、签名校验失败等）。
    Error { message: String },
}

impl UpdatePhase {
    /// 是否进行中：进行中不再发起第二次检查 / 下载 / 安装（按钮可能被连点）。
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
/// 原目录）更新，免安装版只能原位替换自己的裸 exe，两者不交叉。
///
/// 判据是「当前 exe 所在目录 == NSIS 安装器写在注册表里的安装目录」，而不是
/// 「注册表里有没有值」：同一台机器上既装了安装版、又解压了一份便携 exe 时，
/// 两个副本必须各按自己的形态工作——只查注册表会把便携副本误判成安装版。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallKind {
    /// NSIS 安装版：更新走安装器，装回注册表记录的原目录。
    Installed,
    /// 免安装版（裸 exe）：更新用清单里的裸 exe 原位替换自己。
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

/// 清单里该形态该读哪个平台条目：免安装版读流水线额外发表的裸 exe 条目，其余用默认
/// （`windows-x86_64` 等）。
fn manifest_target(kind: InstallKind) -> Option<&'static str> {
    #[cfg(windows)]
    if kind == InstallKind::Portable {
        return Some("windows-x86_64-portable");
    }
    let _ = kind;
    None
}

/// 已下载、待安装的更新包（落盘成 `updates/pending.json`，重启后仍在）。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PendingUpdate {
    version: String,
    /// 包文件名（在 updates 目录内）。
    file: String,
    /// 下载时的安装形态：**形态不匹配的包绝不能装**（用 NSIS 包替换便携 exe，或反过来，
    /// 都会把程序弄成半个安装版）。同一份配置目录可能被两种形态共用。
    kind: InstallKind,
}

/// 更新状态快照：设置页 `get_update_status` 读取，也随 `update-status` 事件推送。
#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    /// 当前版本（取自 `tauri.conf.json` 的 `version`）。
    pub current: String,
    /// 安装形态：前端据此决定安装说明（免安装版是「替换 exe」而不是「安装器」）。
    pub kind: InstallKind,
    #[serde(flatten)]
    pub phase: UpdatePhase,
}

/// 更新器运行时状态（`app.manage` 托管）。
pub struct UpdateState {
    status: Mutex<UpdateStatus>,
    /// 下载目录（`app_data_dir()/updates`）。
    dir: PathBuf,
    pending: Mutex<Option<PendingUpdate>>,
    /// 上一次已记进消息中心的自动更新错误：自动安装每 10 分钟重试一次，
    /// 同样的错误只记一条，免得把「消息」页刷满。
    last_auto_error: Mutex<Option<String>>,
}

impl UpdateState {
    pub fn new(current: String, dir: PathBuf) -> Self {
        cleanup_stale_binaries();
        let kind = install_kind();
        let pending = load_pending(&dir, &current, kind);
        let phase = match &pending {
            Some(p) => UpdatePhase::Ready { version: p.version.clone() },
            None => UpdatePhase::Idle,
        };
        Self {
            status: Mutex::new(UpdateStatus { current, kind, phase }),
            dir,
            pending: Mutex::new(pending),
            last_auto_error: Mutex::new(None),
        }
    }

    /// 当前状态快照（UI 打开晚于检查时靠它兜底，事件只推增量）。
    pub fn snapshot(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    fn pending(&self) -> Option<PendingUpdate> {
        self.pending.lock().unwrap().clone()
    }
}

/// 触发方式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 启动后台检查：静默，仅在发现新版本时用气泡提示一次。
    Auto,
    /// 手动检查：无更新/出错也弹窗反馈。
    Manual,
}

/// 写入阶段并推给前端（`update-status` 事件；主窗口不存在时事件自然落空，无副作用）。
fn set_phase(app: &tauri::AppHandle, phase: UpdatePhase) {
    // 一次成功的检查/下载说明自动更新链路是通的：允许下一次同类错误重新记一条消息
    // （否则恢复后再坏，用户在新会话里也看不到提示——去重只针对「连续同错」）。
    let recovered = matches!(
        phase,
        UpdatePhase::UpToDate | UpdatePhase::Available { .. } | UpdatePhase::Ready { .. }
    );
    let status = {
        let state = app.state::<UpdateState>();
        if recovered {
            *state.last_auto_error.lock().unwrap() = None;
        }
        let mut status = state.status.lock().unwrap();
        status.phase = phase;
        status.clone()
    };
    let _ = tauri::Emitter::emit(app, "update-status", status);
}

/// 更新源的中文名：提示里要说清楚「查的是哪个源」，否则用户不知道该换哪一个。
fn source_label(source: UpdateSource) -> &'static str {
    match source {
        UpdateSource::Ihomy => "ihomy 镜像",
        UpdateSource::Github => "GitHub Releases",
    }
}

/// 把检查清单时的错误翻成「哪个源、什么原因、怎么办」。
///
/// `tauri-plugin-updater` 的错误是英文的，而且把好几种情况压成同一句：HTTP 非 2xx
/// 只写日志、最后统一报 `ReleaseNotFound`；免安装版缺清单条目则报一句英文的
/// `TargetNotFound`。原样抛给用户等于没提示，所以在这里分类翻译。
fn check_error_text(err: &UpdateError, source: UpdateSource) -> String {
    let src = source_label(source);
    match err {
        UpdateError::Reqwest(e) if e.is_timeout() => format!(
            "检查更新超时：{CHECK_TIMEOUT_SECS} 秒内没能连上{src}。请检查网络 / 代理，\
             或在「设置 → 软件更新」里换一个更新源后重试。"
        ),
        UpdateError::Reqwest(e) if e.is_connect() => format!(
            "连不上{src}：{e}。请检查网络 / 代理，或在「设置 → 软件更新」里换一个更新源后重试。"
        ),
        UpdateError::Reqwest(e) => format!("检查更新时网络请求失败（{src}）：{e}"),
        UpdateError::ReleaseNotFound => format!(
            "{src}没有返回可用的更新清单（latest.json）：地址可能不对、该源还没同步到这个版本，\
             或服务器暂时不可用。请稍后重试，或在「设置 → 软件更新」里换一个更新源。"
        ),
        UpdateError::TargetNotFound(target) => {
            let what = if target == "windows-x86_64-portable" {
                "免安装版（windows-x86_64-portable）".to_string()
            } else {
                format!("当前平台（{target}）")
            };
            format!(
                "{src}的更新清单里没有{what}条目，无法判断新版本。发布端可能漏发了这个文件；\
                 请稍后重试或换一个更新源。"
            )
        }
        UpdateError::TargetsNotFound(targets) => format!(
            "{src}的更新清单里没有当前平台的条目（找过：{}），无法判断新版本。\
             请稍后重试或换一个更新源。",
            targets.join("、")
        ),
        UpdateError::Serialization(e) => format!(
            "{src}的更新清单格式不正确，无法解析（{e}）。这份 latest.json 可能损坏；\
             请稍后重试或换一个更新源。"
        ),
        UpdateError::Semver(e) => format!(
            "{src}的更新清单里版本号不合法（{e}），无法比较版本。请稍后重试或换一个更新源。"
        ),
        UpdateError::EmptyEndpoints => {
            "没有配置更新源地址：请到「设置 → 软件更新」选一个更新源。".to_string()
        }
        UpdateError::InsecureTransportProtocol => {
            "更新源地址必须使用 https：请到「设置 → 软件更新」换回默认更新源。".to_string()
        }
        other => format!("检查更新失败（{src}）：{other}"),
    }
}

/// 下载/校验更新包时的错误翻译（签名不符、清单与包的版本对不上都属于「更新数据出错」）。
fn download_error_text(err: &UpdateError) -> String {
    match err {
        UpdateError::Minisign(_) => "更新包校验失败：签名与包内容不匹配（可能下载损坏，或更新源被篡改）。\
             已丢弃这个包，请重试；若反复失败请换一个更新源。"
            .to_string(),
        UpdateError::SignatureUtf8(_) | UpdateError::Base64(_) => {
            "更新清单里的签名数据格式不正确，无法校验更新包。请稍后重试或换一个更新源。".to_string()
        }
        UpdateError::MissingSignedVersion => "更新包的签名里没有版本号（发布端签名工具版本过低），\
             已拒绝安装。请等发布方重新打包后再试。"
            .to_string(),
        UpdateError::SignedVersionMismatch { signed, announced } => format!(
            "更新包签名的版本（v{signed}）与清单声明的版本（v{announced}）不一致，已阻止安装。\
             请换一个更新源或稍后重试。"
        ),
        UpdateError::Network(msg) => format!(
            "下载更新包失败：{msg}。更新源上的文件可能还没同步好，请稍后重试。"
        ),
        UpdateError::Reqwest(e) if e.is_timeout() => {
            "下载更新包超时：网络太慢或更新源不可用，请检查网络后重试。".to_string()
        }
        UpdateError::Reqwest(e) => format!("下载更新包时网络中断：{e}。请检查网络后重试。"),
        other => format!("下载失败：{other}"),
    }
}

/// 后台自动检查/下载/安装失败时记一条消息（**不弹窗**）。
///
/// 后台流程没有用户在场：如果失败只写进设置页状态，用户不打开设置就永远不知道
/// 「更新其实一直坏着」。记进消息中心（亮未读红点）既不抢焦点，又留得下原因。
/// 同一个错误只记一次——自动安装失败每 10 分钟重试一次，不做去重会刷满消息页。
fn record_auto_failure(app: &tauri::AppHandle, message: &str) {
    let update_state = app.state::<UpdateState>();
    {
        let mut last = update_state.last_auto_error.lock().unwrap();
        if last.as_deref() == Some(message) {
            return;
        }
        *last = Some(message.to_string());
    }
    let Some(state) = app.try_state::<crate::KadaState>() else { return };
    crate::commit_result(
        app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "error".into(),
            label: "更新".into(),
            trigger: "后台更新".into(),
            name: String::new(),
            command: message.to_string(),
            stdout: "后台检查 / 下载在启动后与每 24 小时各跑一次。可到「设置 → 软件更新」\
                     换更新源，或点「检查更新」手动重试。"
                .to_string(),
            stderr: String::new(),
            exit_code: None,
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 记失败：写状态；手动模式弹窗，自动模式记进消息中心（不打扰使用，但留得下痕迹）。
fn fail(app: &tauri::AppHandle, mode: Mode, message: String) {
    set_phase(app, UpdatePhase::Error { message: message.clone() });
    match mode {
        Mode::Manual => {
            app.dialog()
                .message(message)
                .title("更新失败")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
        Mode::Auto => record_auto_failure(app, &message),
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

/// 指定更新源的 `latest.json` 地址。
///
/// 每次检查都重新读配置（[`current_source`]）：设置页切换后，下一次检查（含手动
/// 「检查更新」）立即生效，不需要重启。
fn endpoint(source: UpdateSource) -> Url {
    Url::parse(source.endpoint()).expect("内置更新源地址必须是合法 URL")
}

/// 自动更新开关（每次判读都重读配置：用户勾掉后，空闲安装调度下一轮就停）。
fn auto_update_enabled(app: &tauri::AppHandle) -> bool {
    app.try_state::<crate::KadaState>()
        .and_then(|state| state.config.read().ok().map(|cfg| cfg.settings.auto_update))
        .unwrap_or(false)
}

/// 按当前设置装配更新器：`endpoints` 覆盖 `tauri.conf.json` 里的静态默认；
/// 免安装版换用清单里那个裸 exe 条目。
fn build_updater(
    app: &tauri::AppHandle,
    kind: InstallKind,
    source: UpdateSource,
) -> Result<Updater, String> {
    let mut builder = app
        .updater_builder()
        .endpoints(vec![endpoint(source)])
        .map_err(|e| format!("更新源地址无效：{e}"))?;
    if let Some(target) = manifest_target(kind) {
        builder = builder.target(target);
    }
    // 只限清单请求（见 [`CHECK_TIMEOUT_SECS`]）：不设时限时连不上更新源会一直转圈。
    builder = builder.timeout(Duration::from_secs(CHECK_TIMEOUT_SECS));
    builder.build().map_err(|e| format!("更新器初始化失败：{e}"))
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

/// 自动安装调度：勾了「自动更新」时，把已下载的包在**长时间空闲**时装掉；免安装版另在
/// **应用下次启动**时装（它没有安装器，只能在启动早期原位替换 exe —— 见模块文档）。
///
/// 只在 release 构建里跑（dev 里换掉的会是 `target/debug` 的 exe）。
pub fn spawn_auto_installer(app: &tauri::AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        // 第一轮 = 「本次启动」：只有免安装版借它实现「下次启动时安装」。
        let mut first_pass = true;
        let mut last_attempt: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(Duration::from_secs(AUTO_INSTALL_POLL_SECS));
            let startup_pass = std::mem::replace(&mut first_pass, false);
            let Some(pending) = app.state::<UpdateState>().pending() else { continue };
            if !auto_update_enabled(&app) {
                continue;
            }
            let kind = install_kind();
            let startup_due = startup_pass && kind == InstallKind::Portable;
            let idle_due = matches!(
                kada_hook::idle_seconds(),
                Some(secs) if secs >= AUTO_INSTALL_IDLE_SECS
            );
            if !startup_due && !idle_due {
                continue;
            }
            if last_attempt.is_some_and(|t| t.elapsed() < Duration::from_secs(AUTO_INSTALL_RETRY_SECS)) {
                continue;
            }
            last_attempt = Some(std::time::Instant::now());
            install_pending(&app, &pending, true);
        }
    });
}

/// 检查 + 按模式决定后续动作。
async fn run(app: tauri::AppHandle, mode: Mode) {
    let kind = install_kind();
    let source = current_source(&app);
    let updater = match build_updater(&app, kind, source) {
        Ok(updater) => updater,
        Err(e) => return fail(&app, mode, e),
    };

    let found = match updater.check().await {
        Ok(found) => found,
        Err(e) => return fail(&app, mode, check_error_text(&e, source)),
    };

    let Some(update) = found else {
        set_phase(&app, UpdatePhase::UpToDate);
        if mode == Mode::Manual {
            let current = app.package_info().version.to_string();
            app.dialog()
                .message(format!(
                    "当前版本 v{current}，已是最新版本。\n\n更新源：{}（{}）",
                    source_label(source),
                    source.endpoint()
                ))
                .title("检查更新")
                .show(|_| {});
        }
        return;
    };

    let version = update.version.clone();
    set_phase(&app, UpdatePhase::Available { version: version.clone(), notes: update.body.clone() });

    // 这个版本已经下过了（可能还是上一次运行里下的）：不重复下载，直接回到「待安装」。
    if app.state::<UpdateState>().pending().is_some_and(|p| p.version == version) {
        set_phase(&app, UpdatePhase::Ready { version });
        return;
    }

    // 勾了自动更新：发现即下载（安装时机由 spawn_auto_installer 决定，不在这里做）。
    if auto_update_enabled(&app) {
        match download_into(&app, update).await {
            Ok(()) => crate::show_toast_note(
                &app,
                &format!("更新 v{version} 已下载"),
                if mode == Mode::Manual { "空闲时会自动安装" } else { "电脑空闲时会自动安装" },
            ),
            Err(e) => fail(&app, mode, e),
        }
        return;
    }

    match mode {
        // 启动时静默发现：只提示，下载与安装都交给用户手动触发。
        Mode::Auto => {
            crate::show_toast_note(
                &app,
                &format!("发现新版本 {version}"),
                "可在设置页「软件更新」下载",
            );
        }
        // 手动检查：先问要不要下载（下载与安装是两步，装的时候才退出应用）。
        Mode::Manual => {
            let notes = update.body.clone().unwrap_or_default();
            let current = app.package_info().version.to_string();
            let message = if notes.trim().is_empty() {
                format!(
                    "发现新版本 v{version}（当前 v{current}）。\n\n现在下载吗？下载完成后可在设置页点「安装」。"
                )
            } else {
                format!(
                    "发现新版本 v{version}（当前 v{current}）。\n\n更新说明：\n{notes}\n\n现在下载吗？下载完成后可在设置页点「安装」。"
                )
            };
            let app_for_confirm = app.clone();
            app.dialog()
                .message(message)
                .title("发现新版本")
                .buttons(MessageDialogButtons::OkCancelCustom("下载".into(), "稍后".into()))
                .show(move |confirmed| {
                    if confirmed {
                        spawn_download(app_for_confirm);
                    }
                });
        }
    }
}

/// 下载当前可用版本（设置页「下载」按钮 / 手动检查的确认框）。
///
/// 重新拉一次清单：换来的是一句「点下载时以最新清单为准」，也避免把可能过期的句柄
/// 长期驻留在状态里。
pub fn spawn_download(app: tauri::AppHandle) {
    if app.state::<UpdateState>().snapshot().phase.busy() {
        return;
    }
    set_phase(&app, UpdatePhase::Checking);
    let kind = install_kind();
    let source = current_source(&app);
    tauri::async_runtime::spawn(async move {
        let updater = match build_updater(&app, kind, source) {
            Ok(updater) => updater,
            Err(e) => return fail(&app, Mode::Manual, e),
        };
        let update = match updater.check().await {
            Ok(Some(update)) => update,
            Ok(None) => {
                set_phase(&app, UpdatePhase::UpToDate);
                app.dialog()
                    .message(format!(
                        "已是最新版本（更新源：{}），无需下载。",
                        source_label(source)
                    ))
                    .title("检查更新")
                    .show(|_| {});
                return;
            }
            Err(e) => return fail(&app, Mode::Manual, check_error_text(&e, source)),
        };
        if app.state::<UpdateState>().pending().is_some_and(|p| p.version == update.version) {
            set_phase(&app, UpdatePhase::Ready { version: update.version.clone() });
            return;
        }
        if let Err(e) = download_into(&app, update).await {
            fail(&app, Mode::Manual, e);
        }
    });
}

/// 下载并落盘：阶段从 [`UpdatePhase::Downloading`] 走到 [`UpdatePhase::Ready`]。
async fn download_into(app: &tauri::AppHandle, update: Update) -> Result<(), String> {
    let version = update.version.clone();
    set_phase(app, UpdatePhase::Downloading { version: version.clone(), percent: None });

    // 进度回调按「整数百分比变化」才推一次事件：下载分片回调极密集，
    // 每片都 emit 会把 IPC 与前端渲染刷爆。
    let downloaded = AtomicU64::new(0);
    let last_percent = AtomicU64::new(u64::MAX);
    let app_for_progress = app.clone();
    let version_for_progress = version.clone();
    let bytes = update
        .download(
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
        .await
        .map_err(|e| download_error_text(&e))?;

    persist(app, &version, &bytes)?;
    set_phase(app, UpdatePhase::Ready { version });
    Ok(())
}

/// 把下载好的包写进 updates 目录并记下 `pending.json`（重启后 [`UpdateState::new`] 能恢复）。
fn persist(app: &tauri::AppHandle, version: &str, bytes: &[u8]) -> Result<(), String> {
    let state = app.state::<UpdateState>();
    let dir = state.dir.clone();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新文件夹失败：{e}"))?;
    let file = format!("update-{version}.pkg");
    let tmp = dir.join(format!("{file}.tmp"));
    std::fs::write(&tmp, bytes).map_err(|e| format!("写入更新包失败：{e}"))?;
    std::fs::rename(&tmp, dir.join(&file)).map_err(|e| format!("写入更新包失败：{e}"))?;
    let pending = PendingUpdate { version: version.to_string(), file, kind: install_kind() };
    write_pending(&dir, &pending)?;
    cleanup_packages(&dir, version);
    state.pending.lock().unwrap().replace(pending);
    Ok(())
}

/// `pending.json` 的原子写（临时文件 → rename）。
fn write_pending(dir: &Path, pending: &PendingUpdate) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(pending).map_err(|e| format!("序列化更新状态失败：{e}"))?;
    let tmp = dir.join("pending.json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("写入更新状态失败：{e}"))?;
    std::fs::rename(&tmp, dir.join("pending.json")).map_err(|e| format!("写入更新状态失败：{e}"))
}

/// 读回上次下载的包；形态不匹配 / 已是当前版本 / 包丢了都当作没有（并清掉残留）。
fn load_pending(dir: &Path, current: &str, kind: InstallKind) -> Option<PendingUpdate> {
    let text = std::fs::read_to_string(dir.join("pending.json")).ok()?;
    let pending: PendingUpdate = match serde_json::from_str(&text) {
        Ok(pending) => pending,
        Err(_) => {
            clear_pending_dir(dir);
            return None;
        }
    };
    if pending.kind != kind || pending.version == current || !dir.join(&pending.file).exists() {
        clear_pending_dir(dir);
        return None;
    }
    Some(pending)
}

fn clear_pending_dir(dir: &Path) {
    if let Ok(text) = std::fs::read_to_string(dir.join("pending.json")) {
        if let Ok(pending) = serde_json::from_str::<PendingUpdate>(&text) {
            let _ = std::fs::remove_file(dir.join(&pending.file));
        }
    }
    let _ = std::fs::remove_file(dir.join("pending.json"));
}

fn clear_pending(app: &tauri::AppHandle) {
    let state = app.state::<UpdateState>();
    clear_pending_dir(&state.dir);
    state.pending.lock().unwrap().take();
}

/// 删掉旧版本留下的包，别让 `updates/` 越积越多（只留当前待装的那个）。
fn cleanup_packages(dir: &Path, keep_version: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("update-") && name != format!("update-{keep_version}.pkg") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 清掉上次替换留下的 `kada.exe.old-*` / 未完成的 `kada.exe.new`（正在运行的旧文件删不掉，
/// 忽略错误，下次启动再清）。
#[cfg(windows)]
fn cleanup_stale_binaries() {
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(dir) = exe.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("kada.exe.old-") || name == "kada.exe.new" {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(not(windows))]
fn cleanup_stale_binaries() {}

/// 安装当前待装的包。`quiet` = 自动调度触发（失败只写状态，不弹窗）。
fn install_pending(app: &tauri::AppHandle, pending: &PendingUpdate, quiet: bool) {
    if app.state::<UpdateState>().snapshot().phase.busy() {
        return;
    }
    let kind = install_kind();
    if pending.kind != kind {
        // 形态不匹配的包绝不能装：用 NSIS 包换便携 exe（或反之）会把程序弄坏。
        clear_pending(app);
        set_phase(app, UpdatePhase::Idle);
        if !quiet {
            app.dialog()
                .message(
                    "已下载的更新包与当前安装形态不匹配（安装版 / 免安装版），已丢弃。\n\n请重新下载。",
                )
                .title("安装更新")
                .show(|_| {});
        }
        return;
    }
    set_phase(app, UpdatePhase::Installing { version: pending.version.clone() });
    match kind {
        InstallKind::Portable => {
            if let Err(e) = apply_portable_update(app, pending) {
                fail(app, if quiet { Mode::Auto } else { Mode::Manual }, e);
            }
        }
        InstallKind::Installed => {
            let app = app.clone();
            let pending = pending.clone();
            tauri::async_runtime::spawn(async move { install_via_updater(app, pending, quiet).await });
        }
    }
}

/// 安装版：重新核对一次清单（拿到 `Update` 句柄 + 确认版本没变），再把已下载的字节交给
/// updater——Windows 上它会以 `/S /R` 跑 NSIS 并退出本进程，装完由安装器拉起新版本。
async fn install_via_updater(app: tauri::AppHandle, pending: PendingUpdate, quiet: bool) {
    let mode = if quiet { Mode::Auto } else { Mode::Manual };
    let source = current_source(&app);
    let updater = match build_updater(&app, InstallKind::Installed, source) {
        Ok(updater) => updater,
        Err(e) => return fail(&app, mode, e),
    };
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            // 清单说已经是最新：这个包没用了（可能版本已被撤下）。
            clear_pending(&app);
            set_phase(&app, UpdatePhase::UpToDate);
            if !quiet {
                app.dialog()
                    .message(format!(
                        "更新清单显示已是最新版本（更新源：{}），已下载的更新包不再需要，已丢弃。",
                        source_label(source)
                    ))
                    .title("安装更新")
                    .show(|_| {});
            }
            return;
        }
        Err(e) => return fail(&app, mode, check_error_text(&e, source)),
    };
    if update.version != pending.version {
        // 清单已经翻页：丢掉旧包，等下一轮检查按新版本重新下载。
        clear_pending(&app);
        set_phase(
            &app,
            UpdatePhase::Available { version: update.version.clone(), notes: update.body.clone() },
        );
        if !quiet {
            app.dialog()
                .message(format!(
                    "更新清单上的版本已经变成 v{}（已下载的是 v{}），旧包已丢弃。\n\n\
                     请在设置页重新点「下载」。",
                    update.version, pending.version
                ))
                .title("安装更新")
                .show(|_| {});
        }
        return;
    }
    let path = app.state::<UpdateState>().dir.join(&pending.file);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            clear_pending(&app);
            set_phase(&app, UpdatePhase::Idle);
            return fail(&app, mode, format!("读取更新包失败：{e}"));
        }
    };
    if let Err(e) = update.install(&bytes) {
        return fail(&app, mode, format!("安装失败：{e}"));
    }
    // Windows 上走不到这里（install 内部 exit）。其它平台装完不会自己重启，
    // 不重启就一直跑着旧二进制——等于「更新了但没生效」。
    app.restart();
}

/// 免安装版：把新 exe 拷到程序目录 → 旧 exe 改名备份 → 新 exe 就位 → 拉起新进程并退出。
///
/// Windows 允许给**正在运行**的 exe 改名（不能覆盖），所以顺序必须是「先改名再落位」；
/// 落位失败就把备份改回去，保证程序不会两头都缺。旧文件由下次启动的
/// [`cleanup_stale_binaries`] 清掉。
fn apply_portable_update(app: &tauri::AppHandle, pending: &PendingUpdate) -> Result<(), String> {
    let package = app.state::<UpdateState>().dir.join(&pending.file);
    let exe = std::env::current_exe().map_err(|e| format!("取不到当前程序路径：{e}"))?;
    let dir = exe.parent().ok_or_else(|| "取不到程序所在文件夹".to_string())?;
    let staged = dir.join("kada.exe.new");
    std::fs::copy(&package, &staged).map_err(|e| format!("准备更新文件失败：{e}"))?;
    let backup = dir.join(format!("kada.exe.old-{}", env!("CARGO_PKG_VERSION")));
    if let Err(e) = std::fs::rename(&exe, &backup) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("替换程序失败（备份旧版本）：{e}"));
    }
    if let Err(e) = std::fs::rename(&staged, &exe) {
        let _ = std::fs::rename(&backup, &exe);
        let _ = std::fs::remove_file(&staged);
        return Err(format!("替换程序失败：{e}"));
    }
    clear_pending(app);
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(&exe)
        .args(args)
        .spawn()
        .map_err(|e| format!("启动新版本失败：{e}"))?;
    std::process::exit(0);
}

/// 手动安装（设置页「安装」按钮）。没有下载过的包时只提示先下载。
pub fn spawn_install(app: tauri::AppHandle) {
    let Some(pending) = app.state::<UpdateState>().pending() else {
        app.dialog()
            .message("还没有下载更新包，请先点「下载」。")
            .title("安装更新")
            .show(|_| {});
        return;
    };
    install_pending(&app, &pending, false);
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

/// 下载当前可用版本（设置页「下载」）。
#[tauri::command]
pub fn download_update(app: tauri::AppHandle) {
    spawn_download(app);
}

/// 安装已下载的版本（设置页「安装」）。
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
        assert!(!UpdatePhase::Ready { version: "1".into() }.busy());
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

        let ready = serde_json::to_value(UpdateStatus {
            current: "0.1.0".into(),
            kind: InstallKind::Installed,
            phase: UpdatePhase::Ready { version: "0.2.0".into() },
        })
        .unwrap();
        assert_eq!(ready["phase"], "ready");
        assert_eq!(ready["version"], "0.2.0");
        assert_eq!(ready["kind"], "installed");
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

    /// 形态决定读清单里哪个条目：只有 Windows 的免安装版有单独条目。
    #[test]
    fn manifest_target_only_portable_on_windows() {
        if cfg!(windows) {
            assert_eq!(manifest_target(InstallKind::Portable), Some("windows-x86_64-portable"));
        } else {
            assert_eq!(manifest_target(InstallKind::Portable), None);
        }
        assert_eq!(manifest_target(InstallKind::Installed), None);
    }

    /// `pending.json` 的恢复规则：形态不符 / 已是当前版本 / 包不见了都必须丢弃，
    /// 绝不让一个不匹配的包留在「待安装」里。
    #[test]
    fn pending_roundtrip_and_invalid_states_are_dropped() {
        let dir = std::env::temp_dir().join(format!("kada-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("update-2.0.0.pkg"), b"bytes").unwrap();
        let pending = PendingUpdate {
            version: "2.0.0".into(),
            file: "update-2.0.0.pkg".into(),
            kind: InstallKind::Portable,
        };
        write_pending(&dir, &pending).unwrap();
        assert!(load_pending(&dir, "1.0.0", InstallKind::Portable).is_some());
        assert!(load_pending(&dir, "1.0.0", InstallKind::Installed).is_none(), "形态不符要丢弃");

        write_pending(&dir, &pending).unwrap();
        std::fs::write(dir.join("update-2.0.0.pkg"), b"bytes").unwrap();
        assert!(load_pending(&dir, "2.0.0", InstallKind::Portable).is_none(), "已是当前版本要丢弃");

        write_pending(&dir, &pending).unwrap();
        let _ = std::fs::remove_file(dir.join("update-2.0.0.pkg"));
        assert!(load_pending(&dir, "1.0.0", InstallKind::Portable).is_none(), "包丢了要丢弃");

        let _ = std::fs::remove_dir_all(&dir);
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

    /// 失败提示必须说清「哪个源 + 什么原因 + 怎么办」，不能把插件英文原文甩给用户。
    /// 这里钉住最容易碰到、也最容易被含糊掉的三类：清单没拿到、缺平台条目、清单格式坏。
    #[test]
    fn check_errors_are_translated_with_source_and_next_step() {
        let msg = check_error_text(&UpdateError::ReleaseNotFound, UpdateSource::Ihomy);
        assert!(msg.contains("ihomy 镜像"), "{msg}");
        assert!(msg.contains("latest.json"), "{msg}");
        assert!(msg.contains("换一个更新源"), "{msg}");

        let msg = check_error_text(
            &UpdateError::TargetNotFound("windows-x86_64-portable".into()),
            UpdateSource::Github,
        );
        assert!(msg.contains("GitHub Releases"), "{msg}");
        assert!(msg.contains("免安装版"), "{msg}");

        let bad_json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let msg = check_error_text(&UpdateError::Serialization(bad_json), UpdateSource::Ihomy);
        assert!(msg.contains("更新清单格式不正确"), "{msg}");

        let msg = check_error_text(
            &UpdateError::TargetsNotFound(vec!["windows-x86_64".into(), "windows-x86_64-portable".into()]),
            UpdateSource::Ihomy,
        );
        assert!(msg.contains("windows-x86_64-portable"), "{msg}");
    }

    /// 「更新数据出错」里最危险的两类：签名缺版本号 / 签名版本与清单不一致。
    /// 必须明确告诉用户「已拒绝安装」，而不是笼统的「下载失败」。
    #[test]
    fn download_errors_explain_signature_and_version_mismatch() {
        let msg = download_error_text(&UpdateError::MissingSignedVersion);
        assert!(msg.contains("没有版本号"), "{msg}");
        assert!(msg.contains("拒绝安装"), "{msg}");

        let msg = download_error_text(&UpdateError::SignedVersionMismatch {
            signed: "1.0.0".into(),
            announced: "1.1.0".into(),
        });
        assert!(msg.contains("v1.0.0") && msg.contains("v1.1.0"), "{msg}");
        assert!(msg.contains("阻止安装"), "{msg}");

        let msg = download_error_text(&UpdateError::Network(
            "Download request failed with status: 404 Not Found".into(),
        ));
        assert!(msg.contains("404"), "{msg}");
        assert!(msg.contains("稍后重试"), "{msg}");
    }
}
