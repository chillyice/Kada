#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 咔哒桌面壳。
//!
//! 配置驱动：启动时加载 JSON 配置 → 把平台钩子接入配置匹配 → 托盘常驻
//! （关窗不退出）。命令层承载 UI 的读写配置。
//!
//! 跨平台要点：事件管道用自有的 [`Ev`]，各平台钩子都把它喂给 [`decide`] /
//! [`Recorder`] 判定。Windows / Linux 已有钩子实现，其它平台（macOS）壳仍
//! 可编译运行（改键/快捷键/录制暂不可用，托盘与配置界面可用）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use kada_actions::{
    abort as actions_abort, release_held_keys, run_actions, ActionStep, CommandResult, Reporter,
    RunOptions, RunRecord, RunStatus, TriggerCtx,
};
// 只有 Q-06 之后搬进 `cfg(test)` 的双击判定用得到。
#[cfg(test)]
use kada_core::ClipboardMenu;
use kada_core::{
    config_needs_mouse_hook, detect_conflicts, diagnose_input_env, gesture_capture_enabled,
    is_touchpad_trigger, parse_snippet_index, publish_payload, sanitize_config, scan_snippet,
    snippet, snippet_title, touchpad_gesture_enabled, Action, ClipboardOperation, Config, Conflict,
    IndexEntry, Key, Remap, Settings, Severity, Shortcut, ShortcutItem, SnippetMeta, SnippetPick,
    SnippetScan, TextExpansion, TextInjectMode, TriggerContext, Vars,
    DEFAULT_SNIPPET_COMMUNITY_INDEX_URL, DEFAULT_SNIPPET_INDEX_URL, DEFAULT_SNIPPET_PUBLISH_URL,
    SYSTEM_SHORTCUTS,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// 输入决策引擎（tap-hold/层/和弦/键序列/热串的状态机），与 Tauri 解耦、可单测。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod engine;

/// 软件更新（Tauri updater + GitHub Releases + Ed25519 签名），与 Tauri 壳解耦、可单测。
mod update;

/// 配置读写（原子写 + 损坏自愈）：安卓版 S1 起搬进 `kada-core`，桌面壳与安卓壳共用。
use kada_core::config_io;

/// 配置外部修改监听（手改 JSON / 恢复备份 / 同步落盘后自动生效），与 Tauri 壳解耦、可单测。
mod config_watch;

/// 网络叶子（T-02 从 lib.rs 搬出）：卡带下载 / 发布 POST / 统一 reqwest 客户端。
mod net;
pub(crate) use net::strip_urls;
use net::{http_get_text, http_post_json};

/// 执行记录表与消息中心（T-02 从 lib.rs 搬出）：RunRecord 增删改 / 命令日志 / 托盘红点。
mod results;
use results::*;

/// 浮窗（T-03 从 lib.rs 搬出）：触发气泡 / 状态悬浮窗 / 设置变量输入窗。
mod windows;
use windows::*;

/// 托盘（T-03 从 lib.rs 搬出）：自绘托盘菜单 / Linux 原生菜单 / 红点图标。
mod tray;
use tray::*;

/// 命令层（T-04 从 lib.rs 搬出）：按域分组的 `#[tauri::command]`。
mod commands;
// 测试直调设备名的稳定片段提取（T-04 搬进 commands/devices.rs）。
#[cfg(test)]
pub(crate) use commands::devices::device_choices;
// 测试直调的内置示例与索引合并（T-04 搬进 commands/config.rs）。
#[cfg(test)]
pub(crate) use commands::config::{
    bundled_examples, fetch_snippet, merge_index_entries, PublishEnvelope,
    AUTOMATION_EXAMPLE_SNIPPETS, BASIC_EXAMPLE_SNIPPETS,
};

/// 复制快捷菜单 + 快捷键提示框（T-03 从 lib.rs 搬出）。
mod overlays;
use overlays::*;

/// 手势（T-05 从 lib.rs 搬出）：鼠标手势 / 触摸板手势 / 角动作 / 边缘滚动的浮层与监听线程。
mod gestures;
use gestures::{
    hide_gesture_overlay, spawn_gesture_listener, spawn_hot_corner_listener,
    spawn_trackpad_listener, spawn_wheel_edge_listener, trackpad_recording_key,
};
pub(crate) use gestures::{GESTURE_OVERLAY_READY, TRACKPAD_RECORD};

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use engine::{Engine, HotstringHit, Inject};

/// 引擎接线（T-05 从 lib.rs 搬出）：定时器 / 复位巡检 / 配置监听 / 暂停重载 / `fire`。
mod engine_host;
#[cfg(target_os = "macos")]
use engine_host::report_secure_input;
use engine_host::spawn_config_watcher;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use engine_host::spawn_engine_ticker;
pub(crate) use engine_host::{fire, reload_config_now, save_settings_patch, toggle_pause};

/// 录制器（T-05 从 lib.rs 搬出）：键盘宏录制与鼠标操作录制。
mod recorder;
pub(crate) use recorder::{key_name, MouseRecorder, Recorder};

/// 主窗口（T-05 从 lib.rs 搬出）：按需建窗 / 拉到最前 / 界面外链交给系统浏览器。
mod main_window;
#[cfg(test)]
pub(crate) use main_window::external_link_to_open;
pub(crate) use main_window::show_main_window;

/// 运行时状态与状态指示载荷（T-05 从 lib.rs 搬出）：KadaState / 状态快照 / 各浮窗载荷。
mod state;
pub(crate) use state::{
    build_status, EngineStatus, HintsState, KadaState, StatusPayload, ToastPayload,
};
// 测试直调的注入判定（T-05 搬进 engine_host.rs）。
#[cfg(all(
    test,
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
pub(crate) use engine_host::injects_keys;

/// 决策层（T-05 从 lib.rs 搬出）：事件模型 / 单条事件的决定 / 快捷键匹配 / 热串 / 双击唤醒 / 复位巡检。
mod decide;
#[cfg(all(
    test,
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
pub(crate) use decide::FocusTracker;
#[cfg(target_os = "macos")]
pub(crate) use decide::SECURE_INPUT_TICKS;
#[cfg(test)]
pub(crate) use decide::WAKE_DOUBLE_TAP_MS;
pub(crate) use decide::{pause_active, set_pause_lease, wake_double_tap};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
pub(crate) use decide::{spawn_hotstring, to_ev, ResetWatch, ENGINE_TICK_MS};
/// 单条事件的判定与快捷键匹配（安卓版 S1-a 起搬进 `kada-core`，桌面壳与安卓壳共用）。
pub(crate) use kada_core::{decide, Ev, Outcome};

/// 把平台注入（SendInput / uinput）接到引擎的 [`Inject`] 通道。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
struct SimulatedInject;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl Inject for SimulatedInject {
    fn down(&mut self, key: Key) {
        input::simulate::down(key);
    }

    fn up(&mut self, key: Key) {
        input::simulate::up(key);
    }
}

// 平台输入层。
#[cfg(target_os = "windows")]
mod input {
    //! Windows：全局低层键盘钩子（kada-hook）。
    pub use kada_hook::win::{
        current_device, current_pointer_device, focus_window, foreground_fullscreen,
        foreground_window, frontmost_context, hotkey_occupied, input_devices, process_running,
        reinstall_count, set_mouse_enabled, set_mouse_recorder, set_wheel_enabled, simulate, start,
        Action as HookAction, HookHandle, KeyEvent,
    };

    /// 提权窗口注入（规划 7.2-㉑a）：Windows 有整套实现（提权伴随进程 + 命名管道），
    /// 见 `kada_hook::elevated`；`supported()` 在其它平台恒 false。
    pub use kada_hook::elevated;

    pub fn hooks_supported() -> bool {
        true
    }

    /// 输入环境探测（接管者 + HVCI）：Windows 侧实现完整，见 `kada_hook::win::input_env`。
    pub fn input_env() -> kada_core::InputEnv {
        kada_hook::platform_input_env()
    }

    /// 本平台是否**做了**输入环境诊断。Windows 做了。
    pub fn input_env_supported() -> bool {
        true
    }
}

#[cfg(target_os = "linux")]
mod input {
    //! Linux：evdev + uinput 全局钩子（kada-hook），X11 / Wayland 通用。
    // 注意 `reinstall_count` **不在这份名单里**：下面有同名的包装函数（带语义说明），
    // 名单里再 import 一次就是 E0255 重定义——Windows 分支不走这里，本机编译不会报，
    // 只有 Linux/macOS 的 CI（或容器构建）才拦得住（2026-10-04 起这条就是红的，见 变更归档）。
    pub use kada_hook::linux::{
        current_device, current_pointer_device, frontmost_context, input_devices,
        set_mouse_recorder, simulate, start, Action as HookAction, HookHandle, KeyEvent,
    };

    /// 提权窗口注入（规划 7.2-㉑a）：这套东西是 Windows 专属（UIPI 的补丁），Linux 上
    /// `supported()` 恒 false——界面据此说「本平台暂不支持」。
    pub use kada_hook::elevated;

    /// 输入层是否已启动：无权限（没进 input 组 / udev 没放行 /dev/uinput）时
    /// `start` 会失败，这里如实报 false——录制入口据此给解释，而不是录到一片空白。
    pub fn hooks_supported() -> bool {
        kada_hook::linux::hooks_supported()
    }

    /// 鼠标钩子按需安装是 Windows 低层钩子（`WH_MOUSE_LL`）的优化；Linux 的鼠标键与键盘
    /// 同走 evdev，无此开关，这里是空实现。
    pub fn set_mouse_enabled(_on: bool) {}

    /// 滚轮触发键的闸门（7.2-MG 余项①）：滚轮触发键目前只接了 Windows，Linux 空实现。
    pub fn set_wheel_enabled(_on: bool) {}

    /// evdev 钩子没有「被系统摘除后重装」这条路（不依赖系统钩子链），0 = 没重装过；
    /// 设备拔出导致的丢事件由 `reinstall_count`（`kada-hook::linux`）计数，语义与
    /// Windows 的钩子重装一致：壳层 `ResetWatch` 轮询本计数复位 `Engine` 状态。
    pub fn reinstall_count() -> u64 {
        kada_hook::linux::reinstall_count()
    }

    /// evdev 层拿不到前台窗口（要接 X11 / Wayland 协议，见规划 7.3-⑲），故「前台切换复位」
    /// 在 Linux 上暂时恒不成立——与「前台应用/窗口」条件在该平台的受限一致。
    pub fn foreground_window() -> Option<isize> {
        None
    }

    /// 把窗口提到前台：Linux 拿不到前台窗口（见上），没有可还焦点的目标——输入窗收起后
    /// 交给窗口管理器决定焦点落点。
    pub fn focus_window(_hwnd: isize) {}

    /// 全屏程序自动暂停手势：Linux 拿不到前台窗口，恒 false（手势本也只在 Windows 采集）。
    pub fn foreground_fullscreen() -> bool {
        false
    }

    /// 输入环境探测（接管者 + HVCI）：Linux 读 `/proc` 找抢占输入设备的改键守护进程，
    /// 见 `kada_hook::linux::input_env`（HVCI 恒 `None`）。
    pub fn input_env() -> kada_core::InputEnv {
        kada_hook::platform_input_env()
    }

    /// 本平台是否**做了**输入环境诊断。Linux 做了（进程名口径）。
    pub fn input_env_supported() -> bool {
        true
    }
}

#[cfg(target_os = "macos")]
mod input {
    //! macOS：CGEventTap 全局钩子（kada-hook），需要「辅助功能」权限。
    pub use kada_hook::macos::{
        current_device, current_pointer_device, foreground_window, frontmost_context,
        input_devices, reinstall_count, set_mouse_recorder, simulate, start, Action as HookAction,
        HookHandle, KeyEvent,
    };

    /// 提权窗口注入（规划 7.2-㉑a）：macOS 没有 UIPI 这一层（它的问题是「辅助功能」授权与
    /// 安全输入），本档不做，`supported()` 恒 false。
    pub use kada_hook::elevated;

    /// 输入层是否可用 = 有没有「辅助功能」权限。
    ///
    /// 与另两个平台不同：macOS 的能力是**运行时授权**的，没权限时 `CGEventTapCreate` 必失败，
    /// 录制只会录到一片空白。如实报 false，用户看到的是「要授权」而不是「录了半天没东西」。
    pub fn hooks_supported() -> bool {
        kada_hook::macos::accessibility_granted()
    }

    /// 鼠标钩子按需安装是 Windows 低层钩子的优化；macOS 的 `CGEventTap` 本就一把抓全部
    /// 事件，无此开关，这里是空实现。
    pub fn set_mouse_enabled(_on: bool) {}

    /// 滚轮触发键的闸门（7.2-MG 余项①）：滚轮触发键目前只接了 Windows，macOS 空实现。
    pub fn set_wheel_enabled(_on: bool) {}

    /// 全屏程序自动暂停手势：macOS 手势尚未接线（7.2-MG 余项②），恒 false。
    pub fn foreground_fullscreen() -> bool {
        false
    }

    /// 把窗口提到前台：macOS 的 `foreground_window()` 给的是 pid，还焦点要另走
    /// `NSRunningApplication`——本档不做，输入窗收起后交给系统决定焦点落点。
    pub fn focus_window(_hwnd: isize) {}

    /// 输入环境探测：libproc 进程表 + `DIVERTERS` 目录（见 `kada_hook::macos::input_env`）。
    pub fn input_env() -> kada_core::InputEnv {
        kada_hook::platform_input_env()
    }

    /// 本平台是否**做了**输入环境诊断。macOS 已随 7.1-㊽ 的余项补齐（libproc 进程表），
    /// 三平台现在都做了。
    pub fn input_env_supported() -> bool {
        true
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
mod input {
    //! 其它平台：无输入层实现。占位类型保证壳可编译。
    /// 平台事件（无实现时不可构造）。
    pub enum KeyEvent {}

    pub struct HookHandle;

    pub fn hooks_supported() -> bool {
        false
    }

    /// 无输入层实现：无鼠标钩子可开关。
    pub fn set_mouse_enabled(_on: bool) {}

    /// 无输入层实现：滚轮触发键不生效。
    pub fn set_wheel_enabled(_on: bool) {}

    /// 无输入层实现：没有鼠标录制。
    pub fn set_mouse_recorder(_f: Option<crate::MouseRecordCallback>) {}

    /// 提权窗口注入（规划 7.2-㉑a）：本平台没有，`supported()` 恒 false。
    pub use kada_hook::elevated;

    /// 无输入层实现：没有设备上下文。
    pub fn current_device() -> Option<String> {
        None
    }

    /// 无输入层实现：没有指针（鼠标 / 触摸板 / 数位板）设备上下文。
    pub fn current_pointer_device() -> Option<String> {
        None
    }

    /// 无输入层实现：没有可选的输入设备。
    pub fn input_devices() -> Vec<kada_core::InputDevice> {
        Vec::new()
    }

    /// 无输入层实现：没有前台窗口，全屏判定恒 false。
    pub fn foreground_fullscreen() -> bool {
        false
    }

    /// 无输入层实现：没有窗口可提到前台。
    pub fn focus_window(_hwnd: isize) {}

    /// 无输入层实现：不做输入环境探测。
    pub fn input_env() -> kada_core::InputEnv {
        kada_core::InputEnv::default()
    }

    /// 无输入层实现：没做诊断。
    pub fn input_env_supported() -> bool {
        false
    }
}

/// 鼠标录制回调：钩子层每次鼠标触发（点击 / 滚轮）调用一次（见 [`MouseRecorder`]）。
/// 仅在无输入层的占位模块里用到（有输入层时直接用各平台签名）。
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
type MouseRecordCallback = Box<dyn FnMut(kada_hook::MouseRecord) + Send>;

// 动作执行引擎（run_action / run_os / run_app / run_cmd / transform_case 等）
// 已迁出到 `kada-actions` crate；壳层通过 `kada_actions::run_actions` 调用。

#[cfg(test)]
mod tests;

/// 同步存档 / 自启 / 输入环境探测（T-05 从 lib.rs 搬出）：叶子函数，不碰状态机。
mod app_env;
#[cfg(test)]
pub(crate) use app_env::SYNC_FILE_NAME;
pub(crate) use app_env::{
    input_env_cached, launched_by_autostart, platform_caps, push_sync, sync_autostart, sync_file,
};

/// 重启后的让位时长（毫秒，V2.62）：够旧进程走完 `exit` 并释放单实例锁，又短到用户
/// 察觉不到（更新 / 重启本来就有一两秒的启动时间）。
const RELAUNCH_GRACE_MS: u64 = 600;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 自更新重启的竞态（V2.62）：更新 / `app.restart()` 都是「先拉起新进程、再退出旧进程」，
    // 新进程往往比旧进程释放单实例锁更早跑到守卫那里，会被判成第二个实例直接退掉——表现
    // 就是「更新完 Kada 自己消失了」。起进程时打了 [`update::RELAUNCH_ENV`] 标记的，先让出
    // 一小段时间等旧进程真的走完；标记立刻清掉，别让后面拉起的子进程继承。
    if std::env::var_os(update::RELAUNCH_ENV).is_some() {
        std::env::remove_var(update::RELAUNCH_ENV);
        std::thread::sleep(Duration::from_millis(RELAUNCH_GRACE_MS));
    }
    let builder = tauri::Builder::default();
    // 单实例守卫（V2.62）**必须最先注册**：第二个实例在这里就被拦下并退出，
    // 同时把已经跑着的那个实例的主窗口拉到前台（冷启动按需建窗，这里会建出来）。
    //
    // 为什么非要有它：全局键盘钩子是**一条链**（后装先调，吞键返回非 0 就断链），
    // 而执行记录只存在**进程内存**里——双开时只有拿到钩子的那个实例执行动作并落记录，
    // 另一个实例的消息页永远是空的，看起来就像「很多条目都不记录执行记录了」。
    // 提权伴随进程（`kada.exe --kada-agent <pid>`）走 `main.rs` 的分派、不到这里，不受影响。
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        show_main_window(app);
    }));
    builder
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::config::get_config,
            commands::config::set_config,
            commands::devices::get_conflicts,
            commands::devices::input_env_report,
            commands::devices::input_devices,
            commands::devices::running_tools,
            commands::devices::hidpp_status,
            commands::devices::hidpp_unlock,
            commands::devices::elevated_status,
            commands::devices::elevated_start,
            commands::devices::elevated_stop,
            commands::config::export_config,
            commands::config::sync_now,
            commands::config::sync_import_path,
            commands::config::default_sync_dir,
            commands::config::export_fragment,
            commands::config::fetch_snippet_index,
            commands::config::fetch_snippet,
            commands::config::install_snippet,
            commands::config::prepare_publish,
            commands::config::submit_publish,
            commands::config::import_config,
            commands::config::load_sample_config,
            commands::record::set_paused,
            commands::record::abort_actions,
            commands::record::run_entry,
            commands::record::start_record,
            commands::record::stop_record,
            commands::record::start_mouse_record,
            commands::record::stop_mouse_record,
            commands::record::start_trackpad_record,
            commands::record::stop_trackpad_record,
            commands::results::get_command_results,
            commands::results::get_run_records,
            commands::results::get_unread,
            commands::results::get_toast_payload,
            commands::windows::get_status_payload,
            overlays::get_clipboard_menu_payload,
            overlays::clipboard_menu_ready,
            overlays::clipboard_menu_exec,
            tray::get_tray_menu_payload,
            tray::tray_menu_ready,
            tray::tray_menu_exec,
            commands::windows::hud_ready,
            commands::windows::prompt_ready,
            commands::windows::prompt_done,
            commands::windows::get_prompt_payload,
            commands::windows::gesture_overlay_ready,
            overlays::get_hints_state,
            overlays::hints_ready,
            overlays::hints_move,
            overlays::hints_commit,
            overlays::hints_prefs,
            overlays::hints_set_visible,
            commands::results::mark_results_read,
            commands::results::clear_command_results,
            commands::results::clear_command_logs,
            commands::results::take_pending_result_modal,
            commands::results::copy_result_text,
            commands::results::open_result_log,
            update::get_update_status,
            update::check_update,
            update::download_update,
            update::install_update,
            update::take_update_dialog
        ])
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let file = dir.join("config.json");

            // 配置读取走 config_io：原子写 + 损坏自愈。解析失败不静默清空——原文件留档，
            // 能从 .bak 恢复就恢复，经过作为告警在托盘建好后推给消息中心（见文件末尾）。
            let config_io::LoadOutcome {
                config: loaded,
                warnings: load_warnings,
            } = config_io::load(&file);
            let config = Arc::new(RwLock::new(loaded));
            let paused_until = Arc::new(AtomicU64::new(0));
            let rec = Arc::new(Mutex::new(None::<Recorder>));
            let mrec = Arc::new(Mutex::new(None::<MouseRecorder>));
            let results: Arc<Mutex<Vec<CommandResult>>> = Arc::new(Mutex::new(Vec::new()));
            let unread = Arc::new(AtomicBool::new(false));
            let runs: Arc<Mutex<Vec<RunRecord>>> = Arc::new(Mutex::new(Vec::new()));
            // 外部修改监听的基线：此刻磁盘上的内容就是刚读进来的这一份。
            let cfg_watch = Arc::new(Mutex::new(config_watch::ConfigWatcher::new(&file)));
            // 「配置被整份换掉」的旗标：监听线程置位，状态机定时器线程消费（见 spawn_engine_ticker）。
            let cfg_stale = Arc::new(AtomicBool::new(false));

            // 托盘图标：基础 + 带红点（未读态）。转为 owned 以存入 state。
            let base_icon: Option<tauri::image::Image<'static>> =
                app.default_window_icon().map(|img| {
                    tauri::image::Image::new_owned(img.rgba().to_vec(), img.width(), img.height())
                });
            let unread_icon = base_icon.as_ref().map(with_red_dot);

            // 钩子接线：事件即时查表；（Windows / Linux / macOS 都有实现）
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            let engine = Arc::new(Mutex::new(Engine::new()));
            // 复制快捷菜单（EXT-13）：钩子回调只做双击判定与置旗标；建窗 / 收窗在后台线程。
            let clipboard_menu_showing = Arc::new(AtomicBool::new(false));
            let clipboard_menu_hide = Arc::new(AtomicBool::new(false));
            let clipboard_menu_serial = Arc::new(AtomicU64::new(0));

            // 鼠标钩子按需安装：启动即按当前配置决定要不要挂（配置里没有鼠标键触发、
            // 也没有启用的鼠标手势就不挂，缩小全局拦截面）。运行中配置变化由状态机
            // 定时器线程跟进（见 spawn_engine_ticker）。
            // 此刻钩子还没起，`set_mouse_enabled` 只记下值，首次安装按它决定。
            input::set_mouse_enabled(config_needs_mouse_hook(&config.read().unwrap()));
            input::set_wheel_enabled(kada_core::config_uses_wheel(&config.read().unwrap()));

            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            let hook_start = {
                let cfg = config.clone();
                let pause = paused_until.clone();
                let r = rec.clone();
                let app_handle = app.handle().clone();
                let results = results.clone();
                let unread = unread.clone();
                let runs = runs.clone();
                let engine = engine.clone();
                let menu_showing = clipboard_menu_showing.clone();
                let menu_hide = clipboard_menu_hide.clone();
                let mut last_tap: Option<Instant> = None;
                input::start(move |ev: input::KeyEvent| {
                    let raw = ev;
                    let ev = to_ev(&raw);
                    // 录制中：所有事件进时间线、放行；快捷键/改键全暂停。
                    {
                        let mut g = r.lock().unwrap();
                        if let Some(recorder) = g.as_mut() {
                            recorder.push(&ev);
                            return input::HookAction::Allow;
                        }
                    }
                    // 复制快捷菜单开着时按 Esc：只置旗标（收回由菜单轮询线程做），按键本身照常放行。
                    if menu_showing.load(Ordering::Relaxed)
                        && matches!(
                            &raw,
                            input::KeyEvent::Down {
                                key: Key::Escape,
                                ..
                            }
                        )
                    {
                        menu_hide.store(true, Ordering::Relaxed);
                    }
                    // 快速唤醒：双击唤醒键唤出主窗口（被动检测，不拦截按键）。
                    // 判定留在回调里（只读配置 + 比时间，很快），**建窗口丢后台线程**：
                    // 冷启动时 WebView 要几百毫秒，同步建会把钩子回调拖过系统超时线、
                    // 钩子被摘掉，之后整个应用静默失效（开机自启时主窗口还没预建，
                    // 首次双击唤醒正好撞在这条路上）。
                    if wake_double_tap(&cfg, &mut last_tap, &ev) {
                        let app = app_handle.clone();
                        std::thread::spawn(move || show_main_window(&app));
                    }
                    let guard = cfg.read().unwrap();
                    if pause_active(&pause) || guard.settings.paused {
                        return input::HookAction::Allow;
                    }
                    // 前置状态机（tap-hold → 和弦 → 键序列）：被吞掉的键不进 decide。
                    // 状态机没凑成快捷键时会自行回放被吞的键，不会让按键变哑。
                    let action_timeout = guard.settings.action_timeout_ms;
                    let text_mode = guard.settings.text_inject_mode;
                    // 触发侧 `when` 门控上下文（7.2-㊶）：前台窗口**惰性查询**——只有真的
                    // 命中带 `when` 的条目时才查（macOS 的前台查询走 AX 调用，不该让每次
                    // 按键都买单）；设备在该事件上同步取（Linux evdev / Windows Raw Input
                    // 有真值，macOS 恒 None，见 7.2-㊷）。
                    // 触发设备：**指针类触发取指针设备**（鼠标 / 触摸板 / 数位板），其余取键盘
                    // 设备——两条分开记，混着用会把「中键在鼠标 B 上按」判成「键盘 A 按的」。
                    // 同一跳延迟的口径不变（`WM_INPUT` 可能晚于钩子回调一条，见 kada-hook）。
                    let device = if ev.is_pointer_input() {
                        input::current_pointer_device()
                    } else {
                        input::current_device()
                    };
                    let fetch_frontmost = || input::frontmost_context();
                    let when_ctx = TriggerContext::lazy(&fetch_frontmost, device.as_deref());
                    // 全局排除列表（EXT-04）：命中的程序里所有触发一律不生效——不吞键、
                    // 不执行动作（含文本扩展）。判定在共享入口、每个事件最多查一次前台。
                    if when_ctx.app_excluded(&guard.settings.exclude_apps) {
                        return input::HookAction::Allow;
                    }
                    // 全局全屏暂停（EXT-05）：前台全屏程序里所有触发一律不生效——不吞键、
                    // 不执行动作（含文本扩展），退出全屏自动恢复。与手势自带的全屏让路
                    // 独立（那个恒生效、不看开关）。macOS / Linux 判定恒 false，待真机。
                    if guard.settings.ignore_fullscreen && input::foreground_fullscreen() {
                        return input::HookAction::Allow;
                    }
                    // 触摸板手势（7.2-MG）：触发键按下一瞬间 = 开始采集，这一下吞掉（它只是
                    // 「进入采集」的开关，没有任何输出）；抬起 = 把轨迹交给出口，识别与执行在
                    // 后台线程（钩子回调里不做识别）。放在三条让路判定**之后**，让路口径与
                    // 快捷键完全一致；采集期间的按键自动重复也一并吞掉（不然长按会往应用里打字）。
                    // 采集期间 Esc = 作废这一笔（右键手势靠「松开右键」取消，触摸板这边手指还
                    // 在板上，需要一个中途退出的口）；连按下带抬起一起吞，免得应用收到孤立 Esc。
                    // **Esc 自己就是触发键时不算撤销**：那一下要走「触发键」那条路（按下起笔、
                    // 抬起收尾），否则按住 Esc 画一笔永远作废。
                    if kada_hook::trackpad::armed()
                        && !kada_hook::trackpad::armed_with(Key::Escape)
                        && matches!(
                            &ev,
                            Ev::Down {
                                key: Key::Escape,
                                ..
                            } | Ev::Up { key: Key::Escape }
                        )
                    {
                        if matches!(&ev, Ev::Down { .. }) {
                            kada_hook::trackpad::cancel();
                            // 收起轨迹浮层要在后台线程（钩子回调里不碰窗口）。
                            let app = app_handle.clone();
                            std::thread::spawn(move || hide_gesture_overlay(&app));
                        }
                        return input::HookAction::Block;
                    }
                    if let Ev::Down { key, repeat, .. } = &ev {
                        if *repeat {
                            if kada_hook::trackpad::armed_with(*key) {
                                return input::HookAction::Block;
                            }
                        } else if kada_hook::trackpad::supported()
                            && (is_touchpad_trigger(&guard, *key)
                                || trackpad_recording_key() == Some(*key))
                            && kada_hook::trackpad::arm(*key)
                        {
                            return input::HookAction::Block;
                        }
                    }
                    if let Ev::Up { key } = &ev {
                        if kada_hook::trackpad::armed_with(*key) {
                            let anchor = kada_hook::trackpad::anchor();
                            kada_hook::trackpad::finish(*key);
                            // 这一笔只在屏幕上「借道」：光标放回起笔位置，别把用户的鼠标留在别处。
                            // 用户可以关掉（`touchpad_restore_cursor`）；平台做不到（Linux 没有
                            // 客户端 warp）也直接跳过（`restores_cursor`），不假装复位。
                            if guard.settings.touchpad_restore_cursor
                                && kada_hook::trackpad::restores_cursor()
                            {
                                if let Some(pt) = anchor {
                                    kada_hook::set_cursor_pos(pt);
                                }
                            }
                            return input::HookAction::Block;
                        }
                    }
                    // 复制快捷菜单（双击 Ctrl+C）已删除：不再旁路观察，也不会再弹菜单。
                    let mut fire_hit = |actions: Vec<Action>, trigger: String, name: String| {
                        fire(
                            app_handle.clone(),
                            results.clone(),
                            unread.clone(),
                            runs.clone(),
                            actions,
                            trigger,
                            name,
                            action_timeout,
                            text_mode,
                            device.clone(),
                            None,
                        );
                    };
                    let Some(ev) = engine.lock().unwrap().step(
                        &ev,
                        &guard,
                        &when_ctx,
                        &mut SimulatedInject,
                        &mut fire_hit,
                    ) else {
                        return input::HookAction::Block;
                    };
                    let layer = engine.lock().unwrap().active_layer().map(str::to_string);
                    match decide(&ev, &guard, layer.as_deref(), &when_ctx) {
                        Outcome::Shortcut {
                            actions,
                            trigger,
                            name,
                        } => {
                            fire(
                                app_handle.clone(),
                                results.clone(),
                                unread.clone(),
                                runs.clone(),
                                actions,
                                trigger,
                                name,
                                action_timeout,
                                text_mode,
                                device.clone(),
                                None,
                            );
                            input::HookAction::Block
                        }
                        Outcome::Replace(to) => input::HookAction::Replace(to),
                        Outcome::Pass => {
                            // 命中文本扩展：后缀键（空格/回车/Tab）吞掉，改由后台线程
                            // 回删触发词 + 注入替换文本 + 补回后缀（注入要上百毫秒，绝不能
                            // 跑在钩子回调里——回调超时会被系统摘掉钩子，之后全部功能失效）。
                            match engine.lock().unwrap().hotstring(&ev, &guard.expansions) {
                                Some(hit) => {
                                    spawn_hotstring(
                                        app_handle.clone(),
                                        results.clone(),
                                        unread.clone(),
                                        hit,
                                        text_mode,
                                    );
                                    input::HookAction::Block
                                }
                                None => input::HookAction::Allow,
                            }
                        }
                    }
                })
            };

            // 起不来的处置分两种，因为「起不来」在平台的含义不同：
            //
            // - Windows：起不来是异常（资源被占），让 setup 失败、应用报错退出，
            //   用户至少能看见「哪里不对」，而不是一个看着还活着、按键全不响的托盘。
            // - Linux / macOS：起不来多半是**权限没配好**（Linux 没进 input 组 / udev 没放行
            //   /dev/uinput；macOS 没授「辅助功能」）。此时若让 setup 失败，用户连设置页都
            //   进不去，也就永远拿不到指引（Linux 的 `start` 错误串带能照抄的修复命令，
            //   macOS 的 `start` 已顺手弹过系统授权框）。改为照常启动 + 把指引记进消息中心
            //   （托盘建好后统一推，见下面 startup_warnings），修好权限重启即可。
            #[cfg(windows)]
            let (hook_handle, hook_warning): (
                Option<input::HookHandle>,
                Option<(String, String)>,
            ) = (
                Some(hook_start.map_err(|e: std::io::Error| e.to_string())?),
                None,
            );

            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let (hook_handle, hook_warning): (
                Option<input::HookHandle>,
                Option<(String, String)>,
            ) = match hook_start {
                Ok(h) => (Some(h), None),
                Err(e) => (None, Some(("输入层".to_string(), e.to_string()))),
            };

            #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
            let (hook_handle, hook_warning): (
                Option<input::HookHandle>,
                Option<(String, String)>,
            ) = (None, None);

            // 定时推进状态机（键序列超时回放 / 连击等待窗提交，见 spawn_engine_ticker），
            // 顺带把输入状态可视指示推给界面（托盘提示 + 悬浮指示窗，见规划 7.3-⑬）。
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            spawn_engine_ticker(
                engine.clone(),
                config.clone(),
                paused_until.clone(),
                rec.clone(),
                cfg_stale.clone(),
                app.handle().clone(),
            );

            // 鼠标手势的轨迹出口（规划 7.2-MG）：钩子线程上回调，这里只注册；开不开由
            // 定时器线程按配置推（总开关 / 有没有启用的手势）。
            // 放在 `KadaState` 之前：`results` / `unread` / `runs` 三个 Arc 会被 state 拿走。
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            spawn_gesture_listener(
                app.handle().clone(),
                config.clone(),
                results.clone(),
                unread.clone(),
                runs.clone(),
            );

            // 触摸板手势的轨迹出口（7.2-MG）：同样是钩子线程回调、壳层开线程识别。
            // 开不开由定时器线程按配置推（总开关 + 有没有触摸板手势 + 平台是否接了鼠标移动）。
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            spawn_trackpad_listener(
                app.handle().clone(),
                config.clone(),
                results.clone(),
                unread.clone(),
                runs.clone(),
            );

            // 触发角（EXT-10）：出口在钩子线程上，只开线程；开不开与四角开关由定时器线程推。
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            spawn_hot_corner_listener(
                app.handle().clone(),
                config.clone(),
                engine.clone(),
                results.clone(),
                unread.clone(),
                runs.clone(),
            );

            // 边缘滚动（EXT-11）：同样只注册出口，触发判定在钩子里。
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
            spawn_wheel_edge_listener(
                app.handle().clone(),
                config.clone(),
                engine.clone(),
                results.clone(),
                unread.clone(),
                runs.clone(),
            );

            let state = KadaState {
                config,
                paused_until,
                rec,
                mrec,
                file,
                cfg_watch,
                cfg_stale,
                _hook: Mutex::new(hook_handle),
                results,
                pending_result_modal: Mutex::new(None),
                unread,
                runs,
                tray: Mutex::new(None),
                tray_base: base_icon.clone(),
                tray_unread: unread_icon,
                toast: Arc::new(Mutex::new(None)),
                status: Arc::new(Mutex::new(None)),
                clipboard_menu_showing,
                clipboard_menu_hide,
                clipboard_menu_serial,
                clipboard_menu_anchor: Mutex::new(None),
                clipboard_menu_foreground: Mutex::new(None),
                clipboard_menu_payload: Mutex::new(None),
                prompt_payload: Mutex::new(None),
                prompt_tx: Mutex::new(None),
                hints_tray: Mutex::new(None),
                pause_tray: Mutex::new(None),
                tray_menu_showing: Arc::new(AtomicBool::new(false)),
                tray_menu_serial: Arc::new(AtomicU64::new(0)),
                tray_menu_anchor: Mutex::new(None),
                tray_menu_payload: Mutex::new(None),
                hints_placed: AtomicBool::new(false),
            };
            app.manage(state);
            // 「设置变量」的输入框交给壳层画：动作链在后台线程里等，窗口按主题渲染
            // （没装 Tauri 的调用方——单测 / 其它集成——仍用系统输入框，见 kada_actions::prompt）。
            {
                let app = app.handle().clone();
                kada_actions::prompt::install(Box::new(move |prompt, default| {
                    ui_prompt(&app, prompt, default)
                }));
            }
            // 外部修改监听（手改 JSON / 恢复备份 / 同步落盘后自动生效），见 spawn_config_watcher。
            spawn_config_watcher(app.handle().clone());
            // 更新状态单独托管：设置页/托盘都要读，与配置无关（不进 config.json）。
            // 已下载的更新包落在 `app_data_dir()/updates/`，重启后据此恢复「待安装」状态。
            app.manage(update::UpdateState::new(
                app.package_info().version.to_string(),
                dir.join("updates"),
            ));
            // 自动更新（`Settings.auto_update`）：下载好的包在长时间空闲时装；免安装版
            // 另在下次启动时装。没勾时这条线程只是每 30 秒看一眼，什么都不做。
            update::spawn_auto_installer(app.handle());

            // 确保开机自启注册项带 `--autostart` 参数（旧版注册的是裸 exe 路径，无法区分启动来源）。
            let autostart_enabled = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.autostart)
                .unwrap_or(false);
            sync_autostart(app.handle(), autostart_enabled);

            // 主窗口按需创建：双击 exe 手动启动时默认打开页面；开机自启（带 --autostart 参数）时静默到托盘。
            // 气泡窗口则等到第一次触发快捷键时才懒创建（见 ensure_toast）。
            if !launched_by_autostart() {
                show_main_window(app.handle());
            }

            // 快捷键提示框的初值：「启动时要不要建窗」用它（托盘菜单的勾选态由
            // `build_tray_native_menu` / `tray_menu_payload` 自己现读配置）。
            let hints_visible_at_startup = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.hints.visible)
                .unwrap_or(false);

            // 托盘：常驻后台，关窗不退出。
            //
            // 菜单分两路（点击分发共用 [`tray_menu_action`]）：
            // - Windows / macOS：不挂原生菜单——系统画的那套灰白配色跟不上四套主题，
            //   右键改成弹自绘浮窗（[`spawn_tray_menu`]）；
            // - Linux：libappindicator 收不到托盘点击事件，只能继续挂原生菜单。
            #[cfg(target_os = "linux")]
            let native_tray_menu = Some(build_tray_native_menu(app.handle())?);
            #[cfg(not(target_os = "linux"))]
            let native_tray_menu: Option<Menu<tauri::Wry>> = None;

            let mut tray_builder = TrayIconBuilder::new()
                .icon(base_icon.unwrap())
                // 托盘提示的初值；有层 / 修饰键生效时由 publish_status 换成当前状态摘要。
                .tooltip("咔哒 Kada")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| tray_menu_action(app, event.id.as_ref()));
            if let Some(menu) = native_tray_menu.as_ref() {
                tray_builder = tray_builder.menu(menu);
            }
            let tray_icon = tray_builder
                .on_tray_icon_event(|tray, event| match event {
                    // 双击托盘图标（左键）唤起主窗口。
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => show_main_window(tray.app_handle()),
                    // 右键：自绘菜单（Linux 收不到这条事件，那边是原生菜单自己弹）。
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } => spawn_tray_menu(
                        tray.app_handle(),
                        (position.x.round() as i32, position.y.round() as i32),
                    ),
                    _ => {}
                })
                .build(app)?;
            *app.state::<KadaState>().tray.lock().unwrap() = Some(tray_icon);

            // 快捷键提示框：配置里记着要显示才建（懒创建，冷启动零 WebView）。
            // 与主窗口同一个时机——托盘建好之后，`ensure_hints` 只是建窗，位置与显示交给
            // 前端量完内容后的 `hints_ready`。
            if hints_visible_at_startup {
                let app = app.handle().clone();
                std::thread::spawn(move || {
                    let _ = ensure_hints(&app);
                });
            }
            // 悬停让位巡检（光标进框即穿透让位，停留 [`HINTS_HOVER_DWELL_MS`] 再恢复）：
            // 与建不建框无关——框是懒创建的，巡检自己等它出现。
            spawn_hints_hover_watch(app.handle().clone());

            // 启动期的告警推给消息中心：配置损坏 / 自愈（`load_warnings`），以及输入层起不来
            // （macOS 缺「辅助功能」权限——这条必须让用户看见，否则「按键全不响」无从下手）。
            // 放在托盘建好之后：commit_result 会叠托盘红点，此时托盘已存在才叠得上
            // （应用内「消息」入口的红点由前端拉取 unread 得到）。
            let mut startup_warnings: Vec<(String, String, String)> = load_warnings
                .into_iter()
                .map(|w| ("配置".to_string(), w.summary, w.detail))
                .collect();
            if let Some((label, detail)) = hook_warning {
                startup_warnings.push((label, "输入层未启动".to_string(), detail));
            }
            if !startup_warnings.is_empty() {
                let st = app.state::<KadaState>();
                let time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                for (label, summary, detail) in startup_warnings {
                    commit_result(
                        app.handle(),
                        &st.results,
                        &st.unread,
                        CommandResult {
                            kind: "config".into(),
                            label,
                            trigger: "启动加载".into(),
                            name: String::new(),
                            command: summary,
                            stdout: String::new(),
                            stderr: detail,
                            exit_code: None,
                            detached: false,
                            log_path: None,
                            stdout_file: None,
                            stderr_file: None,
                            show_output: false,
                            time: time.clone(),
                            run_id: None,
                        },
                    );
                }
            }

            // 启动后台检查更新 + 每日复查：延迟几秒、仅 release 构建、查不到就静默（见 update::spawn_periodic_check）。
            update::spawn_periodic_check(app.handle());

            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗 = 隐藏到托盘，应用继续跑。
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Kada");
}
