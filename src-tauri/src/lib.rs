#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 咔哒桌面壳。
//!
//! 配置驱动：启动时加载 JSON 配置 → 把平台钩子接入配置匹配 → 托盘常驻
//! （关窗不退出）。命令层承载 UI 的读写配置。
//!
//! 跨平台要点：事件管道用自有的 [`Ev`]，各平台钩子都把它喂给 [`decide`] /
//! [`Recorder`] 判定。Windows / Linux 已有钩子实现，其它平台（macOS）壳仍
//! 可编译运行（改键/快捷键/录制暂不可用，托盘与配置界面可用）。

use std::collections::{BTreeMap, BTreeSet};
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
    is_touchpad_trigger, matches, parse_snippet_index, publish_payload, sanitize_config,
    scan_snippet, snippet, snippet_title, touchpad_gesture_enabled, Action, ClipboardOperation,
    Config, Conflict, IndexEntry, Key, Modifier, RawEvent, Remap, Settings, Severity, Shortcut,
    ShortcutItem, SnippetMeta, SnippetPick, SnippetScan, TextExpansion, TextInjectMode,
    TriggerContext, Vars, DEFAULT_SNIPPET_COMMUNITY_INDEX_URL, DEFAULT_SNIPPET_INDEX_URL,
    DEFAULT_SNIPPET_PUBLISH_URL, SYSTEM_SHORTCUTS,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// 输入决策引擎（tap-hold/层/和弦/键序列/热串的状态机），与 Tauri 解耦、可单测。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
mod engine;

/// 软件更新（Tauri updater + GitHub Releases + Ed25519 签名），与 Tauri 壳解耦、可单测。
mod update;

/// 配置读写（原子写 + 损坏自愈），与 Tauri 壳解耦、可单测。
mod config_io;

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
// 测试直调的注入判定（T-05 搬进 engine_host.rs）。
#[cfg(all(
    test,
    any(target_os = "windows", target_os = "linux", target_os = "macos")
))]
pub(crate) use engine_host::injects_keys;

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

/// 跨平台键盘事件：各平台钩子统一喂给决策器/录制器。
#[derive(Clone, Debug)]
enum Ev {
    Down {
        key: Key,
        mods: BTreeSet<Modifier>,
        repeat: bool,
    },
    Up {
        key: Key,
    },
}

impl Ev {
    /// 事件来自哪个键（按下 / 抬起都算）。
    fn key(&self) -> Key {
        match self {
            Ev::Down { key, .. } | Ev::Up { key } => *key,
        }
    }

    /// 是不是**指针**（鼠标 / 触摸板 / 数位板）输入：中键 / 侧键 / 滚轮。
    /// 「设备是」据此在键盘设备与指针设备之间选一条（两条分开记，见 `input` 模块）。
    fn is_pointer_input(&self) -> bool {
        self.key().is_mouse()
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn to_ev(ev: &input::KeyEvent) -> Ev {
    match ev {
        input::KeyEvent::Down { key, mods, repeat } => Ev::Down {
            key: *key,
            mods: mods.clone(),
            repeat: *repeat,
        },
        input::KeyEvent::Up { key, .. } => Ev::Up { key: *key },
    }
}

/// 触发气泡载荷：名称 + 触发组合键。
#[derive(Clone, Serialize)]
struct ToastPayload {
    name: String,
    trigger: String,
    /// 自定义副标题：`Some` 时前端直接显示它（「发现新版本」等非执行中气泡用）；
    /// 缺省时前端按「触发键 · 正在执行…」渲染。
    #[serde(skip_serializing_if = "Option::is_none")]
    subtitle: Option<String>,
}

/// 输入状态快照（见规划 7.3-⑬）：当前激活层 + 注入中的键，由 [`Engine::status`] 产出。
///
/// 是一份**只读拷贝**（`Engine` 的状态在钩子回调里随时在变，拿引用出去等于把锁带出去）。
/// 键是「有序集合 → `Vec`」（`BTreeSet` 迭代顺序固定），于是两份快照可以直接 `==` 比出
/// 「状态变了没有」——定时器线程据此决定要不要推给界面，没变就一个字节都不发。
///
/// 三类形态的值域是**任意键**（非修饰键也能单次 / 粘滞 / 长按，见 7.3-⑭），所以这里存 [`Key`]
/// 而不是 `Modifier`：指示要如实说「单次 A」，而只有修饰键才参与后续键的 mods 补全。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(crate) struct EngineStatus {
    /// 激活层 id（`None` = 基础层）。
    pub layer: Option<String>,
    /// 该层是否来自「长按锁定层」（切换式）：松开切层键仍生效。
    pub layer_locked: bool,
    /// `hold` 长按的键（松开切层/改键键时释放）。
    pub hold: Vec<Key>,
    /// `sticky` 粘滞的键（单击锁定、再击解锁）。
    pub sticky: Vec<Key>,
    /// `oneshot` 单次的键（单击武装、下一个非修饰键后释放）。
    pub oneshot: Vec<Key>,
}

/// 「什么也没生效」= 基础层且没有任何注入中的键。生产侧由 [`StatusPayload::idle`] 判
/// （那里判的是显示名有没有定出来），这条只给单测当断言用。
#[cfg(test)]
impl EngineStatus {
    fn idle(&self) -> bool {
        self.layer.is_none()
            && self.hold.is_empty()
            && self.sticky.is_empty()
            && self.oneshot.is_empty()
    }
}

/// 输入状态指示里的一条注入键（见规划 7.3-⑬）。`kind` 决定界面上的用词与配色。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct StatusMod {
    /// `hold`（按住）/ `sticky`（粘滞）/ `oneshot`（单次）。
    kind: &'static str,
    /// 键的中性名（与触发键同一套用词：`Ctrl` / `Shift` / `A` / `MouseBack`…）。
    key: String,
}

/// 输入状态指示载荷：当前激活层 + 注入中的键（见规划 7.3-⑬）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct StatusPayload {
    /// 激活层的**显示名**（层没起名字时回落 id）；`None` = 基础层。
    #[serde(skip_serializing_if = "Option::is_none")]
    layer: Option<String>,
    /// 该层来自「长按锁定层」（切换式）：松开切层键仍生效。
    locked: bool,
    /// 注入中的键（按住 / 粘滞 / 单次）。
    mods: Vec<StatusMod>,
    /// 一句话摘要：托盘提示直接用它，也是悬浮指示的兜底文案。
    summary: String,
}

impl StatusPayload {
    /// 空状态（基础层 + 没有注入中的键）——指示据此隐藏。
    fn idle(&self) -> bool {
        self.layer.is_none() && self.mods.is_empty()
    }
}

/// 快捷键提示框（`index.html#hints`）的当前状态，见规划 7.3-㊳。
///
/// `x` / `y` 是窗口**当前**位置（逻辑像素），不是配置里存的那份：前端拖动时用它做差量基准，
/// 配置里的值可能比窗口实际位置旧（拖动中还没落盘）。首次摆放由后端算（工作区右侧），
/// 所以由 `hints_ready` 一并把落地坐标回给前端——不然前端手上的 (0,0) 会让第一次拖动跳一下。
#[derive(Clone, Debug, PartialEq, Serialize)]
struct HintsState {
    /// 是否应该显示（配置里的期望值）。
    visible: bool,
    /// 字号缩放百分比。
    scale: u16,
    /// 不透明度百分比。
    opacity: u8,
    /// 窗口当前左上角（逻辑像素）。
    x: f64,
    y: f64,
}

/// 修饰键形态的中文用词（托盘摘要与悬浮指示共用，避免前后端两份说法）。
fn mod_kind_label(kind: &str) -> &'static str {
    match kind {
        "hold" => "按住",
        "sticky" => "粘滞",
        "oneshot" => "单次",
        _ => "",
    }
}

/// 层 id → 界面显示名（层没起名字就用 id；id 已不在配置里——例如刚被外部改动删掉——也照原样显示）。
fn layer_label(cfg: &Config, id: &str) -> String {
    cfg.layers
        .iter()
        .find(|l| l.id == id)
        .map(|l| {
            if l.name.trim().is_empty() {
                l.id.clone()
            } else {
                l.name.clone()
            }
        })
        .unwrap_or_else(|| id.to_string())
}

/// 引擎快照 + 配置 → 指示载荷（纯函数，可单测）。
fn build_status(st: &EngineStatus, cfg: &Config) -> StatusPayload {
    let layer = st.layer.as_deref().map(|id| layer_label(cfg, id));
    let mut mods: Vec<StatusMod> = Vec::new();
    for (kind, set) in [
        ("hold", &st.hold),
        ("sticky", &st.sticky),
        ("oneshot", &st.oneshot),
    ] {
        for k in set {
            mods.push(StatusMod {
                kind,
                key: key_name(*k).to_string(),
            });
        }
    }
    let summary = status_summary(layer.as_deref(), st.layer_locked, &mods);
    StatusPayload {
        layer,
        locked: st.layer_locked,
        mods,
        summary,
    }
}

/// 指示摘要（托盘提示用）：`层：游戏（锁定） · 按住 Ctrl · 单次 A`。
fn status_summary(layer: Option<&str>, locked: bool, mods: &[StatusMod]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(l) = layer {
        // 两种来源与 `LayerState` 对齐：锁定 = 长按切换一次后保持（`lock_layer`）；
        // 其余 = 按住切层键期间生效（`hold_layer`）。
        parts.push(format!(
            "层：{l}{}",
            if locked {
                "（锁定）"
            } else {
                "（按住）"
            }
        ));
    }
    for m in mods {
        parts.push(format!("{} {}", mod_kind_label(m.kind), m.key));
    }
    parts.join(" · ")
}

/// 运行时状态：钩子持有的配置 + 配置落盘路径 + 消息中心。
struct KadaState {
    config: Arc<RwLock<Config>>,
    /// 录入捕获暂停的截止时刻（进程启动后的毫秒数；0 = 未暂停）。见 [`pause_active`]。
    paused_until: Arc<AtomicU64>,
    /// 录制器：Some = 正在录制（此时快捷键/改键全暂停）。
    rec: Arc<Mutex<Option<Recorder>>>,
    /// 鼠标操作录制器：Some = 正在录制鼠标（与 [`KadaState::rec`] 互斥使用）。
    mrec: Arc<Mutex<Option<MouseRecorder>>>,
    file: PathBuf,
    /// 外部修改监听的基线（见 [`config_watch`]）。**本进程每次写盘都要在这里销账**，而且写盘
    /// 与销账必须同持这把锁——否则巡检线程可能插在中间，把刚写的配置当成外部修改再加载一遍。
    cfg_watch: Arc<Mutex<config_watch::ConfigWatcher>>,
    /// 「配置被整份换掉了」的旗标：外部改动采纳后置位，由状态机定时器线程消费并复位 [`Engine`]
    /// （与旧配置绑定的按住层 / 待定和弦 / 注入中的修饰键都该跟着作废，见 [`spawn_engine_ticker`]）。
    cfg_stale: Arc<AtomicBool>,
    _hook: Mutex<Option<input::HookHandle>>,
    /// 消息中心：命令结果（内存态，重启清空）。
    results: Arc<Mutex<Vec<CommandResult>>>,
    /// 冷启动补发（CS-11）：`show_output` 为真、但主窗口当时还没建出来时先存一份，
    /// 前端启动并注册好监听器后调 `take_pending_result_modal` 取走（取走即清）。
    pending_result_modal: Mutex<Option<CommandResult>>,
    /// 是否有未读命令结果（红点）。
    unread: Arc<AtomicBool>,
    /// 消息中心的执行记录表（内存态，重启清空）：一次触发一条，内含逐步记录
    /// （时间线 / 流水线视图的数据源，见 [`fire`]）。
    runs: Arc<Mutex<Vec<RunRecord>>>,
    /// 最近一次触发气泡的载荷（懒创建 toast 窗口时，供前端加载后兜底读取）。
    toast: Arc<Mutex<Option<ToastPayload>>>,
    /// 最近一次输入状态指示的载荷（懒创建状态指示窗时，供前端加载后兜底读取）。
    status: Arc<Mutex<Option<StatusPayload>>>,
    /// 复制快捷菜单（EXT-13）是否处于「已请求 / 已显示」生命周期。
    clipboard_menu_showing: Arc<AtomicBool>,
    /// 钩子回调发现 Esc 时置位，菜单轮询线程消费后收回（回调里不碰窗口）。
    clipboard_menu_hide: Arc<AtomicBool>,
    /// 每次触发递增：旧的轮询线程据此让位（与 `GESTURE_VISUAL_SERIAL` 同一套路）。
    clipboard_menu_serial: Arc<AtomicU64>,
    /// 菜单锚点（物理像素，触发时的鼠标位置）。
    clipboard_menu_anchor: Mutex<Option<(i32, i32)>>,
    /// 触发那一刻的前台窗口：菜单开着时前台变了就收回，别把动作打进别的程序。
    clipboard_menu_foreground: Mutex<Option<isize>>,
    /// 最近一次菜单载荷（菜单窗懒创建后，前端加载时兜底渲染）。
    clipboard_menu_payload: Mutex<Option<ClipboardMenuPayload>>,
    /// 最近一次「设置变量」输入框载荷（输入窗懒创建后，前端加载时兜底渲染）。
    prompt_payload: Mutex<Option<PromptPayload>>,
    /// 输入窗的回传通道：`Some(值)` = 确定、`None` = 取消 / 关窗。在 [`ui_prompt`] 里填。
    prompt_tx: Mutex<Option<std::sync::mpsc::Sender<Option<String>>>>,
    /// 托盘图标（用于运行时叠 / 去红点）。
    tray: Mutex<Option<TrayIcon>>,
    /// 托盘基础图标（无红点）。
    tray_base: Option<tauri::image::Image<'static>>,
    /// 托盘带红点图标（未读态）。
    tray_unread: Option<tauri::image::Image<'static>>,
    /// 托盘菜单里的「快捷键提示」勾选项：提示框的可见性可能被窗口上的 × 改掉，
    /// 那时得把勾也抹掉——不记住它就只能干看着菜单与现状不一致。
    /// **只有 Linux 有原生菜单**（其余平台是自绘浮窗，勾选态每次弹出现读配置）。
    hints_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 托盘菜单里的「暂停快捷键触发」勾选项：`settings.paused` 也可能被设置页改掉，
    /// 由状态机定时器线程统一把勾对齐（见 [`spawn_engine_ticker`]）。
    /// **只有 Linux 有原生菜单**（其余平台是自绘浮窗，勾选态每次弹出现读配置）。
    pause_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 自绘托盘菜单是否处于「已请求 / 已显示」生命周期（Linux 走原生菜单，恒为 false）。
    tray_menu_showing: Arc<AtomicBool>,
    /// 每次弹出托盘菜单递增：旧的光标巡检线程据此让位（与 `GESTURE_VISUAL_SERIAL` 同一套路）。
    tray_menu_serial: Arc<AtomicU64>,
    /// 托盘菜单锚点（物理像素，右键那一刻的光标位置）：量完尺寸后据此贴图标定位。
    tray_menu_anchor: Mutex<Option<(i32, i32)>>,
    /// 最近一次托盘菜单载荷（菜单窗懒创建后，前端加载时兜底渲染）。
    tray_menu_payload: Mutex<Option<TrayMenuPayload>>,
    /// 本次会话是否已经给提示框摆过位：摆过之后原地不动（拖动 / 缩放都不再重定位），
    /// 免得「内容重排 → 又按配置里的旧坐标摆一次」把刚拖到的位置弹回去。
    hints_placed: AtomicBool,
}

/// 单条事件的决定。
enum Outcome {
    /// 命中快捷键：返回其全部动作（按顺序执行）+ 触发键与名称（供消息中心展示）。
    Shortcut {
        actions: Vec<Action>,
        trigger: String,
        name: String,
    },
    /// 命中改键：改发某个键。
    Replace(Key),
    /// 放行。
    Pass,
}

/// 暂停租约时长（毫秒）：录入捕获最多暂停这么久，超时自动恢复。
///
/// 暂停本来是「录入组合键/序列/和弦期间屏蔽触发，避免自己触发自己」，但前端若被中断
/// （切走窗口、关掉详情、窗口收进托盘）可能来不及解除，布尔量会永久卡在暂停态——表现
/// 为整个应用静默失效（快捷键、改键、文本扩展全不响应），用户完全看不出原因。
/// 改成租约：到期自动失效，前端正常解除仍即时生效。
const PAUSE_LEASE_MS: u64 = 60_000;

/// 状态机定时推进间隔（毫秒）。键序列超时、连击等待窗这类「只能靠时间判定」的等待态，
/// 必须由定时器驱动落地——只靠「下一个事件」懒判定的后果是：单独按一下序列 leader 键
/// （之后不按别的键）回放永远不发生，用户看到的是「这个键按了没反应」。见 [`Engine::tick`]。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const ENGINE_TICK_MS: u64 = 60;

/// 「安全输入」提示的巡检周期（拍数）：[`ENGINE_TICK_MS`] × 32 ≈ 2 秒——状态翻转后
/// 最多两秒出提示，又不必每拍都进一次系统调用（macOS 专属，见 [`report_secure_input`]）。
#[cfg(target_os = "macos")]
const SECURE_INPUT_TICKS: u32 = 32;

/// 进程启动时刻（暂停租约的时间基准）。
static PROCESS_START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// 进程启动至今的毫秒数。
fn now_ms() -> u64 {
    PROCESS_START.elapsed().as_millis() as u64
}

/// 暂停租约是否生效中。
fn pause_active(until: &AtomicU64) -> bool {
    let t = until.load(Ordering::Relaxed);
    t != 0 && now_ms() < t
}

/// 设置暂停租约（`true` = 暂停 [`PAUSE_LEASE_MS`]，`false` = 立即解除）。
fn set_pause_lease(until: &AtomicU64, paused: bool) {
    let deadline = if paused { now_ms() + PAUSE_LEASE_MS } else { 0 };
    until.store(deadline, Ordering::Relaxed);
}

/// 层语义匹配普通改键（非 tap-hold）：激活层条目优先、基础层条目兜底。
/// `ctx` 是触发侧 `when` 门控上下文：条件不成立的条目与「不存在」等价（跳过、落到下一条）。
fn match_plain_remap(
    cfg: &Config,
    key: Key,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<Key> {
    for r in &cfg.remaps {
        if !r.enabled || r.needs_timing_state() || r.layer.as_deref() != active_layer {
            continue;
        }
        if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
            if from == key && r.when_matches(ctx) {
                return Some(to);
            }
        }
    }
    if active_layer.is_some() {
        for r in &cfg.remaps {
            if !r.enabled || r.needs_timing_state() || r.layer.is_some() {
                continue;
            }
            if let (Ok(from), Ok(to)) = (r.from.parse::<Key>(), r.to.parse::<Key>()) {
                if from == key && r.when_matches(ctx) {
                    return Some(to);
                }
            }
        }
    }
    None
}

/// 层语义匹配快捷键：激活层条目优先、基础层条目兜底。返回 (动作, 触发键, 名称)。
fn match_shortcut(
    cfg: &Config,
    raw: &RawEvent,
    active_layer: Option<&str>,
    ctx: &TriggerContext,
) -> Option<(Vec<Action>, String, String)> {
    for s in &cfg.shortcuts {
        if !s.enabled || s.layer.as_deref() != active_layer {
            continue;
        }
        for t in &s.triggers {
            if let Ok(sc) = t.parse::<Shortcut>() {
                if matches(raw, &sc) && s.when_matches(ctx) {
                    return Some((
                        s.actions.clone(),
                        t.clone(),
                        s.name.clone().unwrap_or_default(),
                    ));
                }
            }
        }
    }
    if active_layer.is_some() {
        for s in &cfg.shortcuts {
            if !s.enabled || s.layer.is_some() {
                continue;
            }
            for t in &s.triggers {
                if let Ok(sc) = t.parse::<Shortcut>() {
                    if matches(raw, &sc) && s.when_matches(ctx) {
                        return Some((
                            s.actions.clone(),
                            t.clone(),
                            s.name.clone().unwrap_or_default(),
                        ));
                    }
                }
            }
        }
    }
    None
}

fn decide(ev: &Ev, cfg: &Config, active_layer: Option<&str>, ctx: &TriggerContext) -> Outcome {
    match ev {
        Ev::Down { key, mods, repeat } => {
            // 改键优先于快捷键：先消耗掉原生键，避免改键后键再触发快捷键。
            if let Some(to) = match_plain_remap(cfg, *key, active_layer, ctx) {
                return Outcome::Replace(to);
            }
            if !*repeat {
                let raw = RawEvent {
                    key: *key,
                    mods: mods.clone(),
                    pressed: true,
                };
                if let Some((actions, trigger, name)) = match_shortcut(cfg, &raw, active_layer, ctx)
                {
                    return Outcome::Shortcut {
                        actions,
                        trigger,
                        name,
                    };
                }
            }
            Outcome::Pass
        }
        Ev::Up { .. } => Outcome::Pass,
    }
}

/// 按「文本注入方式」设置发文本（规划 7.3-㉒）：默认剪贴板粘贴；目标程序吞粘贴
/// （游戏 / 终端）时切「逐字直发」。热串扩展与 Text 动作共用同一口径（后者在
/// kada-actions 内按 `RunOptions.text_inject_mode` 分派）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn type_text_by_mode(text: &str, mode: TextInjectMode) -> std::io::Result<()> {
    match mode {
        TextInjectMode::Clipboard => input::simulate::type_text(text),
        TextInjectMode::Unicode => input::simulate::type_text_unicode(text),
    }
}

/// 后台执行一次文本扩展：回删触发词 → 注入替换文本 → 补回后缀键。
///
/// 必须另起线程：注入走剪贴板 + `SendInput`，要上百毫秒，跑在钩子回调里会被系统判超时
/// 摘掉钩子（之后快捷键/改键/文本扩展全部失效）。后缀键在判定命中时已被吞掉，这里补回，
/// 保证「addr␣」展开成「我的地址␣」。注入失败会记进消息中心（没有控制台时不再无声无息）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_hotstring(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    hit: HotstringHit,
    text_mode: TextInjectMode,
) {
    std::thread::spawn(move || {
        for _ in 0..hit.backspaces {
            input::simulate::tap(Key::Backspace);
        }
        let expanded = resolve_hotstring(&hit.replace);
        if let Err(e) = type_text_by_mode(&expanded, text_mode) {
            commit_result(
                &app,
                &results,
                &unread,
                CommandResult {
                    kind: "hotstring".into(),
                    label: "文本扩展".into(),
                    command: hit.trigger.clone(),
                    trigger: hit.trigger.clone(),
                    name: String::new(),
                    stdout: String::new(),
                    stderr: format!("注入替换文本失败：{e}（触发词已被回删）"),
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    run_id: None,
                },
            );
            return;
        }
        input::simulate::tap(hit.terminator);
    });
}

/// 展开热串替换文本的动态片段：`{date}` / `{time}` / `{clipboard}`。
/// 未知占位符保持原样（不破坏用户输入的字面 `{...}`）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn resolve_hotstring(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let token = &after[..end];
                let val = match token {
                    "date" => Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                    "time" => Some(chrono::Local::now().format("%H:%M:%S").to_string()),
                    "clipboard" => input::simulate::get_clipboard_text().ok(),
                    _ => None,
                };
                match val {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('{');
                        out.push_str(token);
                        out.push('}');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 双击唤醒的判定窗口：两次「唤醒键按下」的间隔在此之内才算双击。
const WAKE_DOUBLE_TAP_MS: u64 = 400;

/// 快速唤醒判定：这一击是否构成「双击唤醒键」。
///
/// 被动检测——只报告命中、不拦截按键：唤醒键（UI 里建议选 Alt）常同时是修饰键，拦截会
/// 破坏 Alt+Tab / Alt+字母 等组合。两次按下（非自动重复）间隔 ≤[`WAKE_DOUBLE_TAP_MS`]
/// 判为双击；其间按下其它键则取消上一次单击计数（视作组合键的一部分）。唤醒键由
/// `Settings::wake_key` 指定，**`None` = 关闭快速唤醒**（是 `Settings` 的默认值）。
///
/// **只判定，不建窗口**：调用方命中后必须把唤窗丢到后台线程（见 `run` 里的接线）——
/// 冷启动时建 WebView 要几百毫秒，跑在钩子回调里会被系统判超时摘掉钩子，之后快捷键 /
/// 改键 / 热串全部静默失效。这里不持有 `AppHandle`，顺带也就能单测了。
fn wake_double_tap(cfg: &RwLock<Config>, last_tap: &mut Option<Instant>, ev: &Ev) -> bool {
    let Ev::Down { key, repeat, .. } = ev else {
        return false;
    };
    if *repeat {
        return false;
    }
    {
        let guard = cfg.read().unwrap();
        let Some(wake) = guard.settings.wake_key.as_ref() else {
            return false;
        };
        let Ok(wk) = wake.parse::<Key>() else {
            return false;
        };
        if *key != wk {
            // 按下其它键 → 上一次唤醒键单击不算数（可能是组合键的一部分）。
            last_tap.take();
            return false;
        }
    }
    let now = Instant::now();
    match *last_tap {
        Some(prev) if now.duration_since(prev) <= Duration::from_millis(WAKE_DOUBLE_TAP_MS) => {
            // 计完这一双击就归零：再要唤醒得重新点两下。
            *last_tap = None;
            true
        }
        _ => {
            *last_tap = Some(now);
            false
        }
    }
}

/// 前台窗口观察器：**连续两轮巡检看到同一个新窗口**才认「切换」。
///
/// 为什么要确认一轮：`GetForegroundWindow` 会被瞬态窗口抖一下（我们自己的触发气泡、
/// 右键菜单、UAC 提示、桌面切换瞬间），而那些抖动不是「用户换了工作窗口」。误判的代价
/// 是白复位一次——用户手上正凑的和弦 / 刚按的序列 leader 会被丢掉。多等一个巡检间隔
/// （[`ENGINE_TICK_MS`]，60ms）就能把抖动全滤掉，对「清理残留状态」完全没体感。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[derive(Debug)]
struct FocusTracker {
    /// 已确认的当前窗口。
    confirmed: Option<isize>,
    /// 刚看到、还等下一轮确认的新窗口。
    candidate: Option<isize>,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl FocusTracker {
    fn new(initial: Option<isize>) -> Self {
        Self {
            confirmed: initial,
            candidate: None,
        }
    }

    /// 观察一次当前前台窗口，返回是否是一次**真的**切换。
    fn observe(&mut self, now: Option<isize>) -> bool {
        match now {
            Some(w) if Some(w) == self.confirmed => {
                self.candidate = None;
                false
            }
            Some(w) => {
                if self.candidate == Some(w) {
                    self.confirmed = Some(w);
                    self.candidate = None;
                    true
                } else {
                    // 第一次看到，先记下来等下一轮确认。
                    self.candidate = Some(w);
                    false
                }
            }
            // 取不到句柄（锁屏、无前台窗口）：瞬态读空不作数，也不覆盖候选——
            // 锁屏读空、解锁回到同一个窗口时不该复位。
            None => false,
        }
    }
}

/// 输入状态复位的巡检状态（见规划 7.2-④）：上次看到的钩子重装次数 + 前台窗口观察器。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
struct ResetWatch {
    reinstalls: u64,
    focus: FocusTracker,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
impl ResetWatch {
    fn new() -> Self {
        Self {
            reinstalls: input::reinstall_count(),
            focus: FocusTracker::new(input::foreground_window()),
        }
    }

    /// 巡检一次，返回「需要复位」的原因（`None` = 一切正常）；基线无论如何都推进。
    ///
    /// 两条触发路径正是 `Engine` 那份时序状态会变脏的场景：
    /// - **钩子已重装**（底座自愈看门狗修好了被系统摘除 / 唤醒 / 解锁后的钩子）：那次断线
    ///   期间的 keyup 全丢了，底座自己复位了 `HELD_KEYS` 等三张表，壳层这份镜像也得复位——
    ///   通道就是底座单调递增的 `reinstall_count`（不用回调：那要在钩子线程的消息循环里跑
    ///   壳层代码，还得多一套跨线程注册；60ms 轮询一次足够，且完全不碰钩子层）。
    /// - **前台窗口切换**（Alt-Tab / 点别的窗口）：待定的和弦、序列、连击、热串缓冲都属于
    ///   上一个窗口，跟着过去会打进新窗口；按住式层也要退出（切层键的抬起可能永远不来）。
    fn poll(&mut self) -> Option<&'static str> {
        let reinstalls = input::reinstall_count();
        let reinstalled = reinstalls != self.reinstalls;
        self.reinstalls = reinstalls;

        let switched = self.focus.observe(input::foreground_window());

        if reinstalled {
            Some("钩子已重装")
        } else if switched {
            Some("前台窗口切换")
        } else {
            None
        }
    }
}

/// 把已存在的主窗口拉到最前：单靠 `set_focus` 常常抢不到前台（窗口被别的窗口压着、
/// 最小化、或被 `SetForegroundWindow` 的调用方限制挡住——双击 Alt 唤醒就会表现为
/// 「窗口出来了但没在最前」）。补两步：先取消最小化，再用「临时置顶 + 聚焦 + 撤掉
/// 置顶」把窗口顶到最前。已经在前台就什么都不做，免得凭空闪一下。
fn raise_main_window(w: &tauri::WebviewWindow) {
    let _ = w.unminimize();
    if w.is_focused().unwrap_or(false) {
        return;
    }
    let _ = w.set_always_on_top(true);
    let _ = w.show();
    let _ = w.set_focus();
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        let _ = w.set_always_on_top(false);
    });
}

/// 界面外的链接只放行 http(s) 交给系统浏览器——`about:blank` / `data:` / `javascript:`
/// 之类（新窗口请求里也会出现）不往外丢，免得拿系统默认程序去开一个不是网址的东西。
fn external_link_to_open(url: &str) -> Option<&str> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url)
    } else {
        None
    }
}

/// 按需创建/显示主窗口：首次打开时才真正建出 WebView，冷启动不加载任何界面。
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        raise_main_window(&w);
        return;
    }
    let mut builder =
        tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("咔哒 Kada")
            .inner_size(860.0, 560.0)
            // 下限 = 导航栏 64 + 列表栏 300 + 详情区最小可用 540。再窄详情区会挤爆
            // 「模式下拉 + 按键胶囊 + 三个录入图标」那一行（CSS 里没有断点可退）。
            .min_inner_size(900.0, 480.0)
            .resizable(true)
            .center();
    // 标题栏分平台，界面里那条是自绘的（见 `ui/src/main.ts` 的 `mountTitlebar`）：
    // - Windows / Linux 无边框 + **开投影**：tao 对「无边框 + 有投影」的窗保留整套 DWM 帧，
    //   缩放边、圆角、最大化到工作区都还是系统行为，只有标题区交给界面画；
    // - macOS 用 Overlay：红绿灯由系统画、界面铺到标题栏下方，不用自绘按钮，也不必处理
    //   三个交通灯的状态。别照抄浮窗那套 `shadow(false)`——那条是透明浮窗的规矩。
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false).shadow(true);
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
    }
    match builder
        // 界面外链（帮助页落款的 GitHub）交给系统默认浏览器。WebView 的「新窗口请求」
        // 没人接时会被 wry 直接吞掉（WebView2 里 `SetHandled(true)` 后什么都不做），
        // 于是 `<a target="_blank">` 点了毫无反应——必须自己接住。这里一律不建应用内
        // 新窗口（Deny），只把能交给浏览器的 http(s) 链接丢出去。
        .on_new_window(|url, _features| {
            if let Some(target) = external_link_to_open(url.as_str()) {
                let target = target.to_string();
                // 与钩子回调同一条纪律：发起进程调用不当场做，挪到后台线程。
                std::thread::spawn(move || {
                    if let Err(e) = kada_actions::open_url(&target) {
                        eprintln!("打开外链失败: {e}");
                    }
                });
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .build()
    {
        Ok(w) => raise_main_window(&w),
        Err(e) => {
            // 唤起现在跑在后台线程（见 `wake_double_tap` 的接线），冷启动期间连点两次双击
            // 可能两个线程都走到这里：主线程是串行处理的，后到的那次会撞上「窗口已存在」。
            // 窗口其实已经建好了，补一次显示即可，不必让用户看到一句吓人的创建失败。
            match app.get_webview_window("main") {
                Some(w) => raise_main_window(&w),
                None => eprintln!("创建主窗口失败: {e}"),
            }
        }
    }
}

// 动作执行引擎（run_action / run_os / run_app / run_cmd / transform_case 等）
// 已迁出到 `kada-actions` crate；壳层通过 `kada_actions::run_actions` 调用。

/// 宏录制器：把全局键盘事件转录成动作列表 [`Action`]。
/// 同一次按下的多个键合并成一条 `Keys`（组合键），事件间 ≥20ms 间隔记 `PauseMs`。
struct Recorder {
    started: std::time::Instant,
    last_ms: u64,
    held: Vec<Key>,
    chord: Vec<Key>,
    actions: Vec<Action>,
}

impl Recorder {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            last_ms: 0,
            held: vec![],
            chord: vec![],
            actions: vec![],
        }
    }

    fn now(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// 事件 → 动作；同一次按下的键并成一条组合键，间隙 ≥20ms 记延迟。
    fn push(&mut self, ev: &Ev) {
        let (key, down) = match ev {
            Ev::Down { key, repeat, .. } if !*repeat => (*key, true),
            Ev::Up { key, .. } => (*key, false),
            _ => return, // 重复 down 是按住，折叠
        };
        let ms = self.now();
        let gap = ms.saturating_sub(self.last_ms);
        self.last_ms = ms;
        if down {
            // 一组新组合键的开始（之前无按住的键）：先记下间隔
            if self.held.is_empty() && gap >= 20 {
                self.actions.push(Action::PauseMs {
                    ms: gap,
                    description: None,
                });
            }
            self.held.push(key);
            self.chord.push(key);
        } else {
            self.held.retain(|k| *k != key);
            // 组合键全部松开 → 落成一条 Keys 动作
            if self.held.is_empty() {
                let keys: Vec<String> = self
                    .chord
                    .drain(..)
                    .map(|k| key_name(k).to_string())
                    .collect();
                if !keys.is_empty() {
                    self.actions.push(Action::Keys {
                        keys,
                        mode: kada_core::KeysMode::Tap,
                        description: None,
                    });
                }
            }
        }
    }

    /// 结束：仍在按住的键按剩余组合键收尾，返回录制所得动作。
    fn finish(mut self) -> Vec<Action> {
        if !self.chord.is_empty() {
            let keys: Vec<String> = self
                .chord
                .drain(..)
                .map(|k| key_name(k).to_string())
                .collect();
            self.actions.push(Action::Keys {
                keys,
                mode: kada_core::KeysMode::Tap,
                description: None,
            });
        }
        self.actions
    }
}

fn key_name(k: Key) -> &'static str {
    kada_core::key_name(k)
}

/// 鼠标操作录制器：把钩子层上报的鼠标触发表成 [`Action::Mouse`] 序列——先按位移补一条
/// `Move`（为 0 则省，见吞键 / 判重口径），再补触发本身（`Click` / `Scroll`）。
///
/// 与键盘 [`Recorder`] 独立：鼠标录制只转录鼠标，按键一律不记。位移由钩子层算好
/// （Windows / macOS 取两次触发坐标之差、Linux 取累积相对量），壳层无平台分叉。
#[derive(Default)]
struct MouseRecorder {
    actions: Vec<Action>,
}

impl MouseRecorder {
    fn push(&mut self, ev: &kada_hook::MouseRecord) {
        // 滚轮量为 0（高精度滚轮的次格信号）不算一次触发，直接丢弃——`MouseOp::Scroll`
        // 的偏移不能同时为 0，落盘时会被当成坏条目。
        let op = match ev.kind {
            kada_hook::MouseRecordKind::Click(button) => kada_core::MouseOp::Click { button },
            kada_hook::MouseRecordKind::Scroll { dx, dy } if dx == 0 && dy == 0 => return,
            kada_hook::MouseRecordKind::Scroll { dx, dy } => kada_core::MouseOp::Scroll { dx, dy },
        };
        if ev.move_dx != 0 || ev.move_dy != 0 {
            self.actions.push(Action::Mouse {
                op: kada_core::MouseOp::Move {
                    dx: ev.move_dx,
                    dy: ev.move_dy,
                },
                description: None,
            });
        }
        self.actions.push(Action::Mouse {
            op,
            description: None,
        });
    }

    fn into_actions(self) -> Vec<Action> {
        self.actions
    }
}

#[cfg(test)]
mod tests;

/// 同步存档目录里写死的文件名：换机器只要在设置里选同一个目录就能找到同一份配置。
const SYNC_FILE_NAME: &str = "kada-config.json";

/// 同步存档文件的完整路径；未配置（`None` / 空白）时返回 `None`。
fn sync_file(cfg: &Config) -> Option<PathBuf> {
    let dir = cfg.settings.sync_dir.as_deref()?.trim();
    (!dir.is_empty()).then(|| Path::new(dir).join(SYNC_FILE_NAME))
}

/// 把配置同步写一份到「同步存档」目录（OneDrive 等）；未配置则什么都不做。
///
/// 与本地配置同一套 `config_io::save`（原子写 + `.bak`）：同步目录里也留一份可回滚副本，
/// 半截 JSON 不会被同步客户端传到别处。**尽力而为**——失败只打日志、绝不拦本地保存
/// （与 [`sync_autostart`] 同一取舍：OneDrive 目录临时不在 / 被占用，都不该让「改个快捷键」
/// 失败）。需要明确反馈时走 `sync_now` 命令。
fn push_sync(cfg: &Config) {
    let Some(file) = sync_file(cfg) else { return };
    if let Err(e) = config_io::save(&file, cfg) {
        eprintln!("同步存档写入失败（{}）: {e}", file.display());
    }
}

/// 把「开机自启」设置同步到系统（Windows 写 HKCU\...\CurrentVersion\Run，无需管理员权限）。
fn sync_autostart(app: &tauri::AppHandle, enabled: bool) {
    let res = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(e) = res {
        eprintln!("同步开机自启失败: {e}");
    }
}

/// 本次进程是否由「开机自启」拉起（autostart 插件注册的命令带 `--autostart` 参数）。
/// 用于区分「双击 exe 手动启动」（默认打开主窗口）与「随系统启动」（静默到托盘）。
fn launched_by_autostart() -> bool {
    std::env::args().any(|a| a == "--autostart")
}

/// 本机输入环境探测（接管者 + HVCI）的短时缓存。
///
/// [`get_conflicts`] 在**每次编辑配置**时都会被调（实时显示警告），而探测要走一次
/// Toolhelp 快照 / 扫一遍 `/proc`——几百个进程，单次几毫秒，不该每敲一个字重来一遍。
/// 缓存 3 秒：够覆盖连续编辑，也能在「退出厂商软件后回到界面」时及时翻篇（界面刷新
/// 走 [`input_env_report`]，那里也读同一份缓存）。
///
/// 与看门狗 / 定时器线程无锁关系：这是独立的一把锁，不参与「配置读锁必须先于 Engine 锁」
/// 那个顺序约束（见 `spawn_engine_ticker`）。
fn input_env_cached() -> kada_core::InputEnv {
    use std::sync::OnceLock;

    const TTL: Duration = Duration::from_secs(3);
    static CACHE: OnceLock<Mutex<Option<(Instant, kada_core::InputEnv)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    // 中毒（某次探测 panic）不该让诊断功能连带失效：拿回内层值继续用。返回的是**拷贝**，
    // 不把锁带出去（探测结果很小：几个 id + 一个 `Option<bool>`）。
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, env)) = guard.as_ref() {
        if at.elapsed() < TTL {
            return env.clone();
        }
    }
    let env = input::input_env();
    *guard = Some((Instant::now(), env.clone()));
    env
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
///
/// 各平台口径（与钩子层的实际能力一一对应，改钩子时同步这里）：
/// - **Windows**：键 / 前台 / 设备条件全支持——设备识别随 7.2-㊷ 落地（Raw Input 旁路，
///   见 `kada-hook::win::current_device`）。
/// - **Linux**：前台窗口取不到（X11 未接 / Wayland 无通用协议）→ 前台类条件恒不成立，
///   但设备条件可用（evdev 设备名）；键盘与鼠标键（中键/侧键）监听 + 注入都可用（7.3-㉔）。
/// - **macOS**：媒体键走独立的 `NX_SYSDEFINED` tap（7.1-㊹，监听与注入都可用）；
///   F21~F24 无对应键码；前台走 AX 可用；设备条件仍不可用（CGEventTap 不带设备信息，
///   见 7.2-㊷）。
fn platform_caps() -> kada_core::PlatformCaps {
    #[cfg(target_os = "linux")]
    {
        // 前台条件 false；设备条件可用（DeviceIs 在 Linux 真生效），故只覆盖前台。
        kada_core::PlatformCaps {
            frontmost_conditions: false,
            window_actions: false,
            window_wait: false,
            mouse_absolute: false,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            brightness: false,
            ..Default::default()
        }
    }
    #[cfg(target_os = "macos")]
    {
        let mut keys = std::collections::BTreeSet::new();
        for k in [Key::F21, Key::F22, Key::F23, Key::F24] {
            keys.insert(k);
        }
        kada_core::PlatformCaps {
            listen_unsupported: keys.clone(),
            inject_unsupported: keys,
            frontmost_conditions: true,
            device_conditions: false,
            window_actions: false,
            window_wait: false,
            mouse_absolute: true,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            brightness: false,
            ..Default::default()
        }
    }
    // Windows：键 / 前台 / 设备条件全支持——设备识别随 7.2-㊷ 落地（Raw Input 旁路，
    // 见 `kada-hook::win::current_device`）。
    #[cfg(target_os = "windows")]
    {
        kada_core::PlatformCaps::default()
    }
    // 其它无输入层平台：拿不到设备 → 设备条件恒不成立。
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        kada_core::PlatformCaps {
            device_conditions: false,
            window_actions: false,
            window_wait: false,
            background_input: false,
            shell_actions: false,
            hot_corners: false,
            wheel_edges: false,
            touchpad_gestures: false,
            brightness: false,
            ..Default::default()
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
