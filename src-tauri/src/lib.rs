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

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
use engine::{Engine, HotstringHit, Inject};

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

/// 状态机的超时推进线程：每 [`ENGINE_TICK_MS`] 调一次 [`Engine::tick`]，只在「等待态
/// 已过期」时动作（回放被吞的 leader / 中间步、提交连击等待窗），不会注入别的东西。
/// 录制中与暂停中不推进：那段时间事件不进状态机，推进只会把陈旧的等待态回放到用户
/// 正在录入的内容里。
///
/// 同一个线程顺带做**输入状态复位**巡检（钩子重装 / 前台切换，见 [`ResetWatch`]，以及
/// 「配置被整份换掉」见下面 `cfg_stale`）：复位与超时推进都作用于同一个 [`Engine`]，
/// 放在一起就不必为复位再起一个线程。巡检本身照常进行（基线要跟上），只是暂停/录制期间
/// 不落地。
///
/// 还顺带把**输入状态可视指示**推给界面（见规划 7.3-⑬）：状态变了才推（托盘提示 + 悬浮指示窗）。
/// 放在这条线程上是因为它是唯一一处「不在钩子回调里、又拿得到最新 `Engine` 状态」的地方——
/// 在回调里推 UI 等于把输入层挂在界面上。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_engine_ticker(
    engine: Arc<Mutex<Engine>>,
    cfg: Arc<RwLock<Config>>,
    paused_until: Arc<AtomicU64>,
    rec: Arc<Mutex<Option<Recorder>>>,
    cfg_stale: Arc<AtomicBool>,
    app: tauri::AppHandle,
) {
    std::thread::spawn(move || {
        let mut watch = ResetWatch::new();
        // 上一次推给界面的指示（载荷 + 指示开关）：两者任一变了才重新推。开关也参与比对，
        // 否则「关掉悬浮指示」要等到下次状态变化才生效（锁定层能让它一直挂着）。
        let mut last_status: Option<(StatusPayload, bool)> = None;
        // 上一次「配置是否需要鼠标钩子」：变了才通知钩子层（避免每拍都发重装请求）。
        let mut last_mouse: Option<bool> = None;
        // 上一次「滚轮是否作为触发键」的推送值（规划 7.2-MG 余项①）：没绑时钩子层直接放行。
        let mut last_wheel: Option<bool> = None;
        // 上一次「鼠标手势采集是否开着」（规划 7.2-MG）：变了才推给钩子层。
        let mut last_gesture: Option<bool> = None;
        // 上一次「触摸板手势采集是否开着」：只有按住触发键期间才采集，同样是变了才推。
        let mut last_trackpad: Option<bool> = None;
        // 触发角（EXT-10）/ 边缘滚动（EXT-11）：上一次推给钩子层的「总闸 + 四角/四边开关
        // + 显示器矩形」。显示器几何每 ~3 秒重取一次（热插拔 / 改分辨率），两者共用。
        type HotCornerPush = (bool, [bool; 4], Vec<(i32, i32, i32, i32)>);
        let mut last_hot_corners: Option<HotCornerPush> = None;
        let mut last_wheel_edges: Option<HotCornerPush> = None;
        let mut last_monitors: Vec<(i32, i32, i32, i32)> = Vec::new();
        let mut last_edge_on = false;
        let mut hot_corner_tick: u32 = 0;
        // 上一次「是否处于用户暂停」：变了才对齐托盘的「暂停/恢复」勾选（设置页 / 托盘自身 /
        // 外部改配置都能改它，这里是唯一一处常驻且拿得到最新配置的地方）。
        let mut last_paused: Option<bool> = None;
        // 上一次「提权窗口注入开关」：变了才通知钩子层（关掉时钩子层会顺带断开通道，
        // 提权进程随之退出——「平时不运行」，见 `kada_hook::elevated`）。
        let mut last_elevated: Option<bool> = None;
        // 「安全输入」巡检的拍计数与上次状态（macOS 专属，规划 7.2-㊸）：初始 false——
        // 应用启动时「安全输入」若已开着，第一拍（约 2 秒后）就该提示，而不是当作「没变」。
        #[cfg(target_os = "macos")]
        let mut secure_tick: u32 = 0;
        #[cfg(target_os = "macos")]
        let mut last_secure = false;
        loop {
            std::thread::sleep(Duration::from_millis(ENGINE_TICK_MS));
            let reset_reason = watch.poll();
            // 外部改动把配置整份换掉了（见 [`spawn_config_watcher`]）：跟上一条复位通道走同一套
            // 口径——注入的收回来、缓冲里的丢弃不回放、锁定层保留。切换键位层 / 删掉序列 leader
            // 之后，按住的旧层与待定的和弦都属于上一份配置，留着只会打进新配置里。
            let cfg_replaced = cfg_stale.swap(false, Ordering::Relaxed);
            let reason = reset_reason.or(if cfg_replaced {
                Some("配置已重新加载")
            } else {
                None
            });
            // 输入复位 = 「与物理键盘断过一次线」：触摸板手势正在采的缓冲一起丢掉
            // （不回放、不回调出口）。这里不持配置锁，锁序上只会拿采集状态。
            // 轨迹浮层也得跟着收：复位没有出口回调，靠回调收浮层的那条路走不到。
            if reason.is_some() {
                kada_hook::trackpad::reset();
                hide_gesture_overlay(&app);
            }
            // 配置读锁在 `Engine` 锁**之前**取、并一路持有到 `tick`：等待窗取值就来自配置
            // （`Settings.sequence_timeout_ms` / `chord_timeout_ms`），而钩子回调路径也是
            // 「先读配置、再锁引擎」——两条路径的加锁顺序必须一致，反过来会死锁。
            let (
                payload,
                hud_enabled,
                use_mouse,
                use_wheel,
                user_paused,
                elevated,
                gesture_on,
                trackpad_on,
                toast_on,
            ) = {
                let guard = cfg.read().unwrap();
                let frozen = rec.lock().unwrap().is_some()
                    || pause_active(&paused_until)
                    || guard.settings.paused;
                // 暂停 / 录制期间不推进状态机（那段时间事件根本不进状态机），但指示照常反映当前
                // 状态：暂停这个动作本身不该让已经按住的层从指示里消失。
                let status = if frozen {
                    engine.lock().unwrap().status()
                } else {
                    let mut engine = engine.lock().unwrap();
                    if let Some(reason) = reason {
                        // 动作链按住的键也要收回（EXT-16）：输入复位意味着「与物理键盘断过
                        // 一次线」，`Keys press` 留下的修饰键不收回就会一直粘在系统里。
                        release_held_keys();
                        // 只在真的丢掉了状态时留痕：复位本身是无声兜底（用户此刻没有动作可做），
                        // 进消息中心只会变成噪音。
                        if engine.reset(&mut SimulatedInject) {
                            eprintln!("kada: 输入状态已复位（{reason}）");
                        }
                    }
                    engine.tick(&guard, &mut SimulatedInject);
                    engine.status()
                };
                (
                    build_status(&status, &guard),
                    guard.settings.show_status_hud,
                    // 鼠标钩子：「录入触摸板手势」期间也必须挂着——新建第一条时配置里还没有
                    // 启用的触摸板手势，光开 trackpad 闸门不够，普通鼠标移动进不来，一笔都记不到。
                    config_needs_mouse_hook(&guard) || trackpad_recording_key().is_some(),
                    kada_core::config_uses_wheel(&guard),
                    guard.settings.paused,
                    guard.settings.elevated_inject,
                    // 手势采集的闸门：**新功能总开关**（规划 7.2-MG）关了、或没有启用的手势
                    // → 右键完全放行（回到系统行为，一个字节都不多吞）。
                    guard.settings.experimental && gesture_capture_enabled(&guard),
                    // 触摸板手势的闸门：**它自己的总开关**（与鼠标操作的 `experimental` 分开）；
                    // 没有触摸板手势 / 本平台没接鼠标移动采集时不 arm（触发键照常输出，不是静默吞掉）。
                    // **「录一笔」期间强制开闸**：新建第一条时配置里还没有启用的条目。
                    (guard.settings.touchpad
                        && touchpad_gesture_enabled(&guard)
                        && kada_hook::trackpad::supported())
                        || trackpad_recording_key().is_some(),
                    guard.settings.trigger_toast,
                )
            };
            TRIGGER_TOAST.store(toast_on, Ordering::Relaxed);
            // 锁（配置 / 引擎）到此释放：推指示要动托盘图标、建窗口、发事件，不能在锁里做。
            let changed = match last_status.as_ref() {
                Some((last, enabled)) => last != &payload || *enabled != hud_enabled,
                None => true,
            };
            if changed {
                publish_status(&app, &payload, hud_enabled);
                last_status = Some((payload, hud_enabled));
            }
            // 鼠标钩子按需安装：配置里增删了鼠标键触发（含外部改动 / 导入）就通知钩子层重挂。
            if last_mouse != Some(use_mouse) {
                input::set_mouse_enabled(use_mouse);
                last_mouse = Some(use_mouse);
            }
            // 滚轮触发键的闸门（规划 7.2-MG 余项①）：没绑时钩子层不构造事件、不碰判定。
            if last_wheel != Some(use_wheel) {
                input::set_wheel_enabled(use_wheel);
                last_wheel = Some(use_wheel);
            }
            // 鼠标手势（规划 7.2-MG）：开着才拦右键；关掉时钩子层把状态清干净，
            // 右键立刻回到系统行为（总开关在设置页，配置保留）。
            if last_gesture != Some(gesture_on) {
                kada_hook::gesture::set_enabled(gesture_on);
                last_gesture = Some(gesture_on);
            }
            // 触摸板手势（7.2-MG）：只在按住触发键期间采集；关掉时钩子层清干净状态。
            if last_trackpad != Some(trackpad_on) {
                kada_hook::trackpad::set_enabled(trackpad_on);
                if !trackpad_on {
                    // 关闸时若正画到一半，采集状态已被清掉、出口不会再回调：自己收浮层。
                    hide_gesture_overlay(&app);
                }
                last_trackpad = Some(trackpad_on);
            }
            // 触发角（EXT-10）/ 边缘滚动（EXT-11）：总闸 + 四角/四边开关 + 显示器矩形推给
            // 钩子层（回调里只做纯坐标判断）。两者共用同一份显示器几何，任一启用就按 ~3 秒
            // 重取一次，热插拔 / 改分辨率能跟上。
            hot_corner_tick = hot_corner_tick.wrapping_add(1);
            let (hot_on, hot_flags, wheel_on, wheel_flags) = {
                let g = cfg.read().unwrap();
                let any_corner = |spot| g.hot_corners.iter().any(|r| r.enabled && r.spot == spot);
                let any_edge = |edge| g.wheel_edges.iter().any(|r| r.enabled && r.edge == edge);
                (
                    g.settings.experimental && g.hot_corners.iter().any(|r| r.enabled),
                    [
                        any_corner(kada_core::HotCornerSpot::TopLeft),
                        any_corner(kada_core::HotCornerSpot::TopRight),
                        any_corner(kada_core::HotCornerSpot::BottomLeft),
                        any_corner(kada_core::HotCornerSpot::BottomRight),
                    ],
                    g.settings.experimental && g.wheel_edges.iter().any(|r| r.enabled),
                    [
                        any_edge(kada_core::WheelEdgeSpot::Top),
                        any_edge(kada_core::WheelEdgeSpot::Bottom),
                        any_edge(kada_core::WheelEdgeSpot::Left),
                        any_edge(kada_core::WheelEdgeSpot::Right),
                    ],
                )
            };
            let any_edge = hot_on || wheel_on;
            let refetch = any_edge
                && (hot_corner_tick.is_multiple_of(50)
                    || !last_edge_on
                    || last_monitors.is_empty());
            let monitors = if refetch {
                app.available_monitors()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| {
                        let pos = m.position();
                        let size = m.size();
                        (
                            pos.x,
                            pos.y,
                            pos.x + size.width as i32,
                            pos.y + size.height as i32,
                        )
                    })
                    .collect()
            } else {
                last_monitors.clone()
            };
            last_monitors = monitors.clone();
            last_edge_on = any_edge;
            let next_hot = (hot_on, hot_flags, monitors.clone());
            if last_hot_corners.as_ref() != Some(&next_hot) {
                kada_hook::hotcorner::configure(next_hot.0, next_hot.1, next_hot.2.clone());
                last_hot_corners = Some(next_hot);
            }
            let next_wheel = (wheel_on, wheel_flags, monitors);
            if last_wheel_edges.as_ref() != Some(&next_wheel) {
                kada_hook::wheel_edge::configure(next_wheel.0, next_wheel.1, next_wheel.2.clone());
                last_wheel_edges = Some(next_wheel);
            }
            // 托盘「暂停快捷键触发」的勾选对齐：设置页勾选、外部改配置、托盘自身点击都汇总到
            // 这一处比对（点菜单时 CheckMenuItem 已自动翻转，这里把状态钉死到真实配置值）。
            if last_paused != Some(user_paused) {
                if let Some(item) = app.state::<KadaState>().pause_tray.lock().unwrap().as_ref() {
                    let _ = item.set_checked(user_paused);
                }
                last_paused = Some(user_paused);
            }
            // 提权窗口注入的开关（规划 7.2-㉑a）：只在这里同步给钩子层——注入路径每一批都要
            // 读它，放配置锁里现读会拖慢热路径。关掉时钩子层会断开通道，提权进程随之退出。
            if last_elevated != Some(elevated) {
                input::elevated::set_enabled(elevated);
                last_elevated = Some(elevated);
            }
            // 「需要提权、但通道没用上」的提示（开关没开 / 提权进程没在跑 / 通道出错）：
            // 平台层带冷却、取走即报，不会连按刷屏。**失败必须可见**——否则用户看到的仍然
            // 是「按了没反应」，本档就白做了（见 `docs/底层输入能力规划.md` §1 第 5 条）。
            if let Some(notice) = input::elevated::take_notice() {
                report_elevated_notice(&app, notice);
            }
            // 「安全输入」提示（macOS 专属，规划 7.2-㊸）：低频轮询，状态**翻转**才报——
            // 终端输密码这类场景系统会关掉所有事件 tap，快捷键全失效；不提示的话用户看到的
            // 就是「Kada 坏了」，而其实退出那个场景会自己恢复。
            #[cfg(target_os = "macos")]
            {
                secure_tick += 1;
                if secure_tick >= SECURE_INPUT_TICKS {
                    secure_tick = 0;
                    let on = kada_hook::secure_input_enabled();
                    if on != last_secure {
                        last_secure = on;
                        report_secure_input(&app, on);
                    }
                }
            }
        }
    });
}

/// 「安全输入」状态翻转时进消息中心的一条提示（macOS 专属，规划 7.2-㊸）。
///
/// 开与关都说：开 = 「快捷键此刻全失效，不是 Kada 坏了」；关 = 「恢复了」（翻回来也报，
/// 免得用户不知道上一条「失效」是不是还成立）。两条都给下一步：关掉那个场景 / 重启 Kada。
#[cfg(target_os = "macos")]
fn report_secure_input(app: &tauri::AppHandle, on: bool) {
    let state = app.state::<KadaState>();
    let (command, stdout) = if on {
        (
            "系统「安全输入」已开启：快捷键此刻全部失效".to_string(),
            "终端输密码、`sudo` 密码框这类场景会让 macOS 暂时关掉所有事件监听（包括 Kada 的）\
             ——这是系统安全机制，Kada 与任何其它工具都绕不过去。退出那个场景后自动恢复。"
                .to_string(),
        )
    } else {
        (
            "系统「安全输入」已关闭：快捷键恢复".to_string(),
            "如果刚才「按了没反应」，现在应该恢复了；仍不响应时重启一次 Kada。".to_string(),
        )
    };
    commit_result(
        app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "macos".into(),
            label: "输入层".into(),
            trigger: "安全输入".into(),
            name: String::new(),
            command,
            stdout,
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            // 不弹窗：这条随时可能在用户输密码的当下出现，弹窗既抢焦点又不合时宜。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 「需要提权、但通道没用上」时进消息中心的一条提示。
///
/// 三种原因分开说，且都要给出**下一步怎么做**：本档的前提是「用户先知道发生了什么事」
/// （档位 A 立的规矩），到这里再补上「怎么让它通」。安全桌面的边界也在这里如实讲一句——
/// 那是这套东西**也做不到**的部分（见 `docs/底层输入能力规划.md` 档位 B 的边界）。
fn report_elevated_notice(app: &tauri::AppHandle, notice: input::elevated::Notice) {
    let state = app.state::<KadaState>();
    let target = if notice.app.is_empty() {
        "一个提权窗口".to_string()
    } else {
        format!("提权窗口（{}）", notice.app)
    };
    let (headline, detail) = match notice.reason {
        input::elevated::NoticeReason::Disabled => (
            format!("{target}里注入没生效：提权窗口注入还没打开"),
            "前台这个程序是**以管理员身份运行**的，UIPI 不允许 Kada 往里注入（这也是它设计上的\
             用途）。打开 设置 → 提权窗口注入 的「提权窗口也生效」，会弹一次 UAC 授权，之后\
             这类窗口里的文本 / 按键 / 鼠标动作就能生效。"
                .to_string(),
        ),
        input::elevated::NoticeReason::NotRunning => (
            format!("{target}里注入没生效：提权进程没在运行"),
            "开关是开着的，但提权进程没跑（没启动过、或者已经退出）。去 设置 → 提权窗口注入 \
             点「启动提权进程」（会弹一次 UAC）。"
                .to_string(),
        ),
        input::elevated::NoticeReason::Failed => (
            format!("{target}里注入没生效：提权通道出错"),
            "已经回落成普通注入，所以提权窗口里不会有反应。可在 设置 → 提权窗口注入 里重启\
             提权进程；持续失败请把这条消息连同复现步骤一起记下来反馈。"
                .to_string(),
        ),
    };
    commit_result(
        app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "elevated".into(),
            label: "提权窗口注入".into(),
            trigger: "提权窗口".into(),
            name: String::new(),
            command: headline,
            stdout: detail,
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            // 不弹窗：这条随时可能因为用户正好在提权窗口里按快捷键而出现，弹窗会抢焦点。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 配置外部修改监听的巡检线程（见 [`config_watch`] 与规划 7.2-⑩）。
///
/// 每秒看一眼配置文件：被外部改成了另一份**能解析**的内容就采纳（替换内存里那份 + 通知
/// 消息中心 + 广播给界面）；改成解析不动的半截内容就只提示、**谁都不动**（磁盘上那份留给
/// 用户自己修）。这样「手改 JSON / 恢复备份 / 同步客户端落盘」都能生效，且应用内的下一个
/// 保存不会再把外部改动盖掉——内存里那份已经跟着走了。
///
/// 巡检是轮询而非订阅文件系统事件：与钩子看门狗、[`ResetWatch`] 同一取舍——不引入额外依赖
/// 与平台分支，不用管编辑器「先写临时文件再改名」和同步客户端的各种怪癖，代价只是慢一拍
/// （[`config_watch::POLL_MS`]）。读的是一份几 KB 的文件，开销可以忽略。
fn spawn_config_watcher(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(config_watch::POLL_MS));
        let st = app.state::<KadaState>();
        // 判定 + 采纳在锁内一口气做完，通知放到锁外：`cfg_watch` 这把锁与 set_config /
        // import_config 共用，持锁期间不该去动托盘图标、发事件（也不能让写盘等它）。
        let (reloaded, bad) = {
            let current = st.config.read().unwrap().clone();
            let mut watch = st.cfg_watch.lock().unwrap();
            match watch.poll(&st.file, &current) {
                None => (None, None),
                Some(config_watch::Change::Unreadable(err)) => (None, Some(err)),
                Some(config_watch::Change::Reloaded(next)) => {
                    let autostart_changed = next.settings.autostart != current.settings.autostart;
                    let cfg = *next;
                    // 留一份给锁外的通知与广播（配置只有几 KB，且这条路径本来就罕见）。
                    *st.config.write().unwrap() = cfg.clone();
                    (Some((cfg, autostart_changed)), None)
                }
            }
        };
        // 坏内容：内存与磁盘都不动，只提示这一次（同一份内容不重复报，见 [`config_watch`]）。
        if let Some(err) = bad {
            commit_result(
                &app,
                &st.results,
                &st.unread,
                CommandResult {
                    kind: "config".into(),
                    label: "配置".into(),
                    trigger: "外部修改".into(),
                    name: String::new(),
                    command: "配置文件被外部修改，但内容无法解析：已保留当前配置".into(),
                    stdout: format!(
                        "磁盘上的 {} 未被改动（监听不会清理或回写外部内容）。\
                         改好后会自动重新加载；想放弃这次外部改动，可用 config.json.bak 覆盖回去。",
                        st.file.display(),
                    ),
                    stderr: err,
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
            continue;
        }
        // 采纳成功（上面已换进内存）：自启跟着系统同步一次，引擎复位交给定时器线程。
        let Some((cfg, autostart_changed)) = reloaded else {
            continue;
        };
        if autostart_changed {
            sync_autostart(&app, cfg.settings.autostart);
        }
        st.cfg_stale.store(true, Ordering::Relaxed);
        commit_result(
            &app,
            &st.results,
            &st.unread,
            CommandResult {
                kind: "config".into(),
                label: "配置".into(),
                trigger: "外部修改".into(),
                name: String::new(),
                command: "检测到配置文件被外部修改，已重新加载生效".into(),
                stdout: format!(
                    "快捷键 {} 条、改键 {} 条、文本扩展 {} 条、层 {} 个、目录 {} 个。\
                     误改可回滚到 config.json.bak（上次保存时的良好副本）。",
                    cfg.shortcuts.len(),
                    cfg.remaps.len(),
                    cfg.expansions.len(),
                    cfg.layers.len(),
                    cfg.folders.len(),
                ),
                stderr: String::new(),
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
        let _ = app.emit("config-changed", &cfg);
    });
}

/// 托盘「暂停 / 恢复快捷键触发」：切换 `settings.paused` 并落盘。
///
/// 与设置页的「暂停」勾选是同一个字段（走 [`save_settings_patch`]，会广播 `config-updated`，
/// 设置页随即跟上）；勾选态由状态机定时器线程统一对齐（见 [`spawn_engine_ticker`]）。
fn toggle_pause(app: &tauri::AppHandle) {
    let st = app.state::<KadaState>();
    let paused = !st.config.read().unwrap().settings.paused;
    if let Err(e) = save_settings_patch(app, &st, |s| s.paused = paused) {
        eprintln!("切换暂停失败: {e}");
    }
}

/// 托盘「重载配置」：立即从磁盘再读一份（手改 JSON 后想马上生效，不等监听线程的下一拍）。
///
/// 与 [`spawn_config_watcher`] 同口径——**能解析才采纳、解析不动就别碰磁盘**；区别是这条由
/// 用户主动触发，无论结果如何都给一条可见反馈，免得点了没反应。
fn reload_config_now(app: &tauri::AppHandle) {
    let st = app.state::<KadaState>();
    let result = config_io::read_only(&st.file);
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let command = match result {
        Ok(cfg) => {
            // 与 set_config 同一把锁序（cfg_watch → config.write，巡检线程也按此序）。
            let mut watch = st.cfg_watch.lock().unwrap();
            if *st.config.read().unwrap() == cfg {
                watch.adopt_own_write(&st.file);
                drop(watch);
                "配置已是最新，无需重载".to_string()
            } else {
                let autostart_changed =
                    st.config.read().unwrap().settings.autostart != cfg.settings.autostart;
                *st.config.write().unwrap() = cfg.clone();
                // 与外部改动同一条复位通道：旧配置绑定的按住层 / 待定和弦 / 注入修饰键作废。
                st.cfg_stale.store(true, Ordering::Relaxed);
                watch.adopt_own_write(&st.file);
                drop(watch);
                if autostart_changed {
                    sync_autostart(app, cfg.settings.autostart);
                }
                // 用 config-changed（而非 config-updated）：内存配置真被整份换掉了，界面若有
                // 未保存草稿得按「外部改动」的流程问用户，不能静默覆盖。
                let _ = app.emit("config-changed", &cfg);
                format!(
                    "已从磁盘重载配置：快捷键 {} 条、改键 {} 条、文本扩展 {} 条、层 {} 个。",
                    cfg.shortcuts.len(),
                    cfg.remaps.len(),
                    cfg.expansions.len(),
                    cfg.layers.len(),
                )
            }
        }
        Err(e) => {
            commit_result(
                app,
                &st.results,
                &st.unread,
                CommandResult {
                    kind: "error".into(),
                    label: "配置".into(),
                    trigger: "重载配置".into(),
                    name: String::new(),
                    command: "配置重载失败：磁盘内容无法解析，当前配置保持不变".into(),
                    stdout: String::new(),
                    stderr: e,
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: now.clone(),
                    run_id: None,
                },
            );
            return;
        }
    };
    commit_result(
        app,
        &st.results,
        &st.unread,
        CommandResult {
            kind: "config".into(),
            label: "配置".into(),
            trigger: "重载配置".into(),
            name: String::new(),
            command,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            detached: false,
            log_path: None,
            stdout_file: None,
            stderr_file: None,
            show_output: false,
            time: now,
            run_id: None,
        },
    );
}

/// 执行一串动作（异步跑，避免阻塞钩子回调）；命令类动作的结果进消息中心。
///
/// `timeout_ms` 是命令/脚本的执行超时（0 = 不限时），由调用点从配置读出后传入——调用点
/// 已经持有配置读锁，这里不再进线程里二次加锁（保存配置时写锁会等读锁，多一层没必要）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn fire(
    app: tauri::AppHandle,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    actions: Vec<Action>,
    trigger: String,
    name: String,
    timeout_ms: u64,
    text_mode: TextInjectMode,
    device: Option<String>,
    origin: Option<(i32, i32)>,
) {
    // 气泡窗口首次懒创建较慢（WebView 冷启动），放后台线程，避免阻塞钩子回调——
    // WH_KEYBOARD_LL 回调超时会被系统摘除，导致后续快捷键/改键/热串全部失效。
    {
        let app = app.clone();
        let name = name.clone();
        let trigger = trigger.clone();
        std::thread::spawn(move || show_toast(&app, &name, &trigger));
    }
    // 先落一条「正在执行」：界面立刻能点进去看（逐步汇报会持续刷新同一张卡）。
    // ponytail: 记录 id 用微秒时间戳——同一微秒起两条链的概率可忽略，省一个全局计数器。
    let id = chrono::Local::now().timestamp_micros() as u64;
    start_run(
        &app,
        &runs,
        &unread,
        RunRecord {
            id,
            trigger: trigger.clone(),
            name: name.clone(),
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            start_epoch_ms: chrono::Local::now().timestamp_millis() as u64,
            duration_ms: 0,
            status: "running".into(),
            steps: Vec::new(),
        },
    );
    // 触发设备由**调用方在回调线程上**取好带下来：`current_device` / `current_pointer_device`
    // 都是「最近一次事件」，放到下面 spawn 的线程里读会晚一拍、可能已被别的设备覆盖。
    // 键盘类触发带键盘设备、指针类（鼠标键 / 滚轮 / 手势 / 触摸板）带指针设备——混着用会把
    // 「中键在鼠标 B 上按」判成「键盘 A 按的」（见 7.2-㊷ / 说明书 §3.4）。
    // 触发那一刻的前台窗口（EXT-14「当前窗口」动作的目标）：必须在**调用线程**上抓，
    // 放进下面 spawn 的线程就晚了一拍（动作链还没跑，但前台可能已经变了）。
    // 只在 Windows 抓：macOS / Linux 没实现这个动作族，白付一次前台查询没必要。
    #[cfg(target_os = "windows")]
    let window = input::foreground_window();
    #[cfg(not(target_os = "windows"))]
    let window = None;
    std::thread::spawn(move || {
        // 触发带修饰键的快捷键（如 Ctrl+Alt+T）时修饰键仍物理按住，此时注入按键会被
        // 污染成 Ctrl+Alt+<键>（粘贴 Ctrl+V 变成 Ctrl+Alt+V）。只对真正注入按键的链
        // （Text / Keys，含 If 分支内）等修饰键释放：300ms 还没松就不注入——污染键打进
        // 目标程序轻则无效、重则误触别的快捷键（如 Ctrl+Alt+方向键转屏），跳过并记消息
        // 中心让用户知道为什么没生效。纯命令 / 文件 / 开 URL 的链不受修饰键影响，等
        // 300ms 只是白拖手感，照常直接执行（7.3-㉒ 顺带修「返回值被丢弃」）。
        if injects_keys(&actions) && !input::simulate::wait_modifiers_released(300) {
            commit_result(
                &app,
                &results,
                &unread,
                CommandResult {
                    kind: "error".into(),
                    label: "动作未执行".into(),
                    command: trigger.clone(),
                    trigger: trigger.clone(),
                    name: name.clone(),
                    stdout: String::new(),
                    stderr: "修饰键（Ctrl/Alt/Shift/Meta）在 300ms 内没有松开，为避免注入被污染 \
                             成 Ctrl+Alt+<键>，本次文本/按键动作已跳过——松键再触发即可。"
                        .into(),
                    exit_code: None,
                    detached: false,
                    log_path: None,
                    stdout_file: None,
                    stderr_file: None,
                    show_output: false,
                    time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                    run_id: Some(id),
                },
            );
            finish_run(&app, &runs, &unread, id, "error");
            return;
        }
        let mut vars: Vars = BTreeMap::new();
        let mut last_copied: Option<String> = None;
        let frontmost = input::frontmost_context();
        let mut commit = |result: CommandResult| commit_result(&app, &results, &unread, result);
        let mut on_step = |step: ActionStep| add_step(&app, &runs, &unread, id, step);
        let mut rep = Reporter {
            commit: &mut commit,
            on_step: &mut on_step,
        };
        let opts = RunOptions {
            action_timeout: Duration::from_millis(timeout_ms),
            text_inject_mode: text_mode,
        };
        let t = TriggerCtx {
            trigger: &trigger,
            name: &name,
            frontmost: frontmost.as_ref(),
            device: device.as_deref(),
            window,
            origin,
        };
        let status = run_actions(
            &mut rep,
            &actions,
            t,
            &mut vars,
            &mut last_copied,
            &opts,
            Some(id),
        );
        let mut failed = false;
        update_run(&app, &runs, id, |run| {
            failed = run.steps.iter().any(|s| s.status == "error");
        });
        finish_run(
            &app,
            &runs,
            &unread,
            id,
            match status {
                RunStatus::Aborted => "abort",
                RunStatus::Finished if failed => "error",
                RunStatus::Finished => "ok",
            },
        );
    });
}

/// 手势浮层窗口的视觉参数（EXT-02），随预览 / 结果事件一起发给前端。
#[derive(Clone, Serialize)]
struct GestureVisual {
    trail_color: String,
    trail_width: i32,
    label_font_size: i32,
    label_offset: i32,
    fail_color: String,
}

#[derive(Clone, Serialize)]
struct GesturePreviewPayload {
    #[serde(flatten)]
    visual: GestureVisual,
    /// 窗口本地物理坐标（前端按 `scale` 换算成 CSS 像素）。
    points: Vec<(i32, i32)>,
    scale: f64,
}

#[derive(Clone, Serialize)]
struct GestureResultPayload {
    #[serde(flatten)]
    visual: GestureVisual,
    points: Vec<(i32, i32)>,
    scale: f64,
    matched: bool,
    label: String,
    pattern: String,
}

/// 结果展示序号：新一次手势 / 新结果会把它推进，旧的隐藏定时器据此让位。
static GESTURE_VISUAL_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 手势浮层前端是否已注册好 `gesture-preview` / `gesture-result` 监听器。
static GESTURE_OVERLAY_READY: AtomicBool = AtomicBool::new(false);

/// 触摸板手势的「录一笔」会话（设置页 → 鼠标操作编辑器）：`Some(触发键)` = 正在等一笔轨迹。
///
/// 它同时是一个**闸门覆盖**：新建第一条触摸板手势时，配置里还没有任何启用的条目，
/// `touchpad_gesture_enabled` 为假、采集根本开不了，所以录制期间必须强制开闸
/// （见 `spawn_engine_ticker` 里的 `trackpad_on` 与钩子入口的 `arm` 判定）。
static TRACKPAD_RECORD: Mutex<Option<Key>> = Mutex::new(None);

/// 正在录制的触发键（`None` = 没在录）。
fn trackpad_recording_key() -> Option<Key> {
    *TRACKPAD_RECORD.lock().unwrap()
}

/// 「录一笔」的结果：只回填编辑器，**不执行动作、不进执行记录**。
#[derive(Clone, Serialize)]
struct TrackpadRecordPayload {
    /// 识别出的方向串；空 = 这一笔太短 / 涂鸦 / 已作废（Esc / 双指 / 停手太久）。
    pattern: String,
    /// 归一化形状模板（[`kada_core::template_from_points`]）；点太少时为 `None`。
    template: Option<Vec<[i16; 2]>>,
    /// 这一笔记了多少个点（界面据此提示采样密度）。
    points: usize,
    elapsed_ms: u64,
}

/// 手势浮层窗口名（`index.html#gesture`）。
const GESTURE_WINDOW: &str = "gesture";

fn gesture_visual_of(s: &Settings) -> GestureVisual {
    GestureVisual {
        trail_color: s.gesture_trail_color.clone(),
        trail_width: s.gesture_trail_width,
        label_font_size: s.gesture_label_font_size,
        label_offset: s.gesture_label_offset,
        fail_color: s.gesture_fail_color.clone(),
    }
}

/// 按需创建手势轨迹浮层：铺满虚拟屏幕、透明、不激活、鼠标穿透。
///
/// **必须 `focusable(false)` + `set_ignore_cursor_events(true)`**：它能覆盖整个桌面，
/// 一旦能被激活就会抢前台 → `ResetWatch` 误判复位；鼠标穿透保证画手势时事件照常落到
/// 下面的窗口。同 toast / hud 的建窗约束，见 AGENTS §1.7。
fn ensure_gesture_overlay(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
        return Some(w);
    }
    GESTURE_OVERLAY_READY.store(false, Ordering::Relaxed);
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        GESTURE_WINDOW,
        tauri::WebviewUrl::App("index.html#gesture".into()),
    )
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .inner_size(1.0, 1.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建手势浮层失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    // 铺满虚拟屏幕（多显示器取并集；坐标用物理像素，与钩子轨迹同一口径）。
    let mut min = (i32::MAX, i32::MAX);
    let mut max = (i32::MIN, i32::MIN);
    if let Ok(monitors) = app.available_monitors() {
        for m in monitors {
            let p = m.position();
            let s = m.size();
            min.0 = min.0.min(p.x);
            min.1 = min.1.min(p.y);
            max.0 = max.0.max(p.x.saturating_add(s.width as i32));
            max.1 = max.1.max(p.y.saturating_add(s.height as i32));
        }
    }
    if min.0 < max.0 && min.1 < max.1 {
        let _ = w.set_position(tauri::PhysicalPosition::new(min.0, min.1));
        let _ = w.set_size(tauri::PhysicalSize::new(
            (max.0 - min.0) as u32,
            (max.1 - min.1) as u32,
        ));
    }
    Some(w)
}

/// 浮层窗口左上角（物理像素）与该窗口的缩放系数。
fn gesture_window_geom(app: &tauri::AppHandle) -> Option<((i32, i32), f64)> {
    let w = app.get_webview_window(GESTURE_WINDOW)?;
    let pos = w.outer_position().ok()?;
    let scale = w.scale_factor().ok()?;
    Some(((pos.x, pos.y), scale))
}

fn local_gesture_points(points: &[(i32, i32)], origin: (i32, i32)) -> Vec<(i32, i32)> {
    points
        .iter()
        .map(|p| (p.0 - origin.0, p.1 - origin.1))
        .collect()
}

/// 等手势浮层前端注册好监听器（上限 1 秒；前端加载失败也不至于卡死）。
fn wait_gesture_overlay_ready() {
    let deadline = Instant::now() + Duration::from_millis(1000);
    while !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 手势浮层前端注册完监听器后回报（避免冷启动第一笔手势的轨迹事件丢失）。
#[tauri::command]
fn gesture_overlay_ready() {
    GESTURE_OVERLAY_READY.store(true, Ordering::Relaxed);
}

/// 收起浮层，并让旧的隐藏定时器失效。
fn hide_gesture_overlay(app: &tauri::AppHandle) {
    GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
        let _ = w.hide();
    }
}

/// 「录一笔」收尾：识别这一笔 → 收起浮层 → 推给设置页。**不执行动作、不弹未匹配提示**。
///
/// 走的是与正式手势同一套识别（`pattern_from_stroke` / `template_from_points`），
/// 所以「录的时候是什么、跑的时候就是什么」——这也是录入界面唯一需要的保证。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn emit_trackpad_record(
    app: &tauri::AppHandle,
    cfg: &Arc<RwLock<Config>>,
    stroke: &kada_hook::gesture::Stroke,
) {
    let min_len = {
        let g = cfg.read().unwrap();
        // 与运行时（`handle_trackpad_stroke`）同一把尺子：触摸板用 `touchpad_start_distance`，
        // 不是右键手势的 `gesture_start_distance`——否则「录出来的」和「跑出来的」会不一样。
        g.settings.touchpad_start_distance as f64
    };
    let payload = TrackpadRecordPayload {
        pattern: kada_core::pattern_from_stroke(&stroke.points, min_len),
        template: kada_core::template_from_points(&stroke.points),
        points: stroke.points.len(),
        elapsed_ms: stroke.elapsed_ms,
    };
    hide_gesture_overlay(app);
    let _ = app.emit("trackpad-recorded", payload);
}

/// 展示一次手势结果（命中 = 动作名；未命中 = 失败色）。窗口隐藏交给延时线程。
fn show_gesture_result(
    app: &tauri::AppHandle,
    cfg: &Arc<RwLock<Config>>,
    points: &[(i32, i32)],
    matched: bool,
    label: String,
    pattern: String,
) {
    if points.len() < 2 {
        return;
    }
    if ensure_gesture_overlay(app).is_none() {
        return;
    }
    let Some((origin, scale)) = gesture_window_geom(app) else {
        return;
    };
    let visual = {
        let g = cfg.read().unwrap();
        gesture_visual_of(&g.settings)
    };
    let payload = GestureResultPayload {
        visual,
        points: local_gesture_points(points, origin),
        scale,
        matched,
        label,
        pattern,
    };
    let serial = GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        // 等前端注册好监听器（`gesture_overlay_ready`），否则事件会丢。
        if !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) {
            wait_gesture_overlay_ready();
        }
        if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) != serial {
            return;
        }
        if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
            let _ = w.show();
            let _ = w.emit("gesture-result", &payload);
        }
        std::thread::sleep(Duration::from_millis(900));
        if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) == serial {
            if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
                let _ = w.hide();
            }
        }
    });
}

/// 轨迹预览线程：按帧读钩子里的轨迹点，够起手距离才亮出浮层。
///
/// 钩子回调里只记点，**不往这里推**（回调超会被系统摘钩子）；这里轮询快照，
/// 只在点数变化时发事件（16ms 一帧，跟手不刷屏）。
///
/// **两条来源共用一个浮层**：右键手势（`kada_hook::gesture`）与触摸板手势
/// （`kada_hook::trackpad`，按住触发键采集）。两者不会同时进行——前者要按住右键、
/// 后者要按住触发键，所以按「先右键、后触摸板」取一个即可。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_gesture_overlay(app: tauri::AppHandle, cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut was_drawing = false;
        let mut shown = false;
        let mut suppressed = false;
        let mut last_len = usize::MAX;
        let mut preloaded = false;
        loop {
            // 采集期间快轮询（跟手），空闲时慢轮询：触摸板手势也要算进来，
            // 否则按住触发键画的时候只有 8 帧/秒，轨迹一顿一顿的。
            let interval = if kada_hook::gesture::enabled() || kada_hook::trackpad::enabled() {
                16
            } else {
                120
            };
            std::thread::sleep(Duration::from_millis(interval));
            let (drawing, points, from_touchpad) = if kada_hook::gesture::drawing() {
                (true, kada_hook::gesture::points(), false)
            } else if kada_hook::trackpad::drawing() {
                (true, kada_hook::trackpad::points(), true)
            } else {
                (false, Vec::new(), false)
            };
            if !drawing {
                was_drawing = false;
                shown = false;
                suppressed = false;
                last_len = usize::MAX;
                // 提前把浮层建好并等前端就绪：冷启动第一笔手势如果边画边建窗，
                // WebView 加载要几百毫秒，轨迹还没画出来就结束了。
                if !preloaded {
                    let on = {
                        let g = cfg.read().unwrap();
                        (g.settings.experimental && gesture_capture_enabled(&g))
                            || (g.settings.touchpad
                                && g.settings.touchpad_overlay
                                && touchpad_gesture_enabled(&g)
                                && kada_hook::trackpad::supported())
                            || trackpad_recording_key().is_some()
                    };
                    if on {
                        preloaded = ensure_gesture_overlay(&app).is_some();
                    }
                }
                continue;
            }
            if !was_drawing {
                was_drawing = true;
                shown = false;
                suppressed = false;
                last_len = usize::MAX;
            }
            if suppressed {
                continue;
            }
            if points.len() < 2 {
                continue;
            }
            let recording = trackpad_recording_key().is_some();
            let (visual, start_distance, on, overlay, exclude, exclude_apps) = {
                let g = cfg.read().unwrap();
                (
                    gesture_visual_of(&g.settings),
                    // 两条路的「够长才亮」各用自己那个值：右键手势是「起点之后开始画」，
                    // 触摸板是「这一笔至少多长」。
                    if from_touchpad {
                        g.settings.touchpad_start_distance
                    } else {
                        g.settings.gesture_start_distance
                    },
                    (if from_touchpad {
                        g.settings.touchpad
                            && touchpad_gesture_enabled(&g)
                            && kada_hook::trackpad::supported()
                    } else {
                        g.settings.experimental && gesture_capture_enabled(&g)
                    }) || recording,
                    // 浮层开关只影响触摸板这条来源（右键手势没有单独开关）。
                    !from_touchpad || g.settings.touchpad_overlay,
                    // 排除列表也按来源取：两条路各有自己的名单，别混用。
                    if from_touchpad {
                        g.settings.touchpad_exclude.clone()
                    } else {
                        g.settings.gesture_exclude.clone()
                    },
                    g.settings.exclude_apps.clone(),
                )
            };
            if !on {
                suppressed = true;
                continue;
            }
            // 关掉「显示轨迹浮层」就只做识别、不画预览；「录一笔」是设置页的录入工具，
            // 要看见自己画的线，不受这枚开关管。
            if !overlay && !recording {
                suppressed = true;
                continue;
            }
            if !shown {
                let first = points[0];
                let moved = points
                    .iter()
                    .map(|p| (p.0 - first.0).abs() + (p.1 - first.1).abs())
                    .max()
                    .unwrap_or(0);
                if moved < start_distance {
                    continue;
                }
                // 与 `handle_gesture` 同一口径：全屏 / 排除程序里不接管，也不画浮层。
                // 「录一笔」例外：那是设置页里的录入工具，前台就是 Kada 自己，
                // 命中排除列表也不该把预览藏掉。
                if !recording && input::foreground_fullscreen() {
                    suppressed = true;
                    continue;
                }
                if !recording && (!exclude.is_empty() || !exclude_apps.is_empty()) {
                    let fetch = || input::frontmost_context();
                    let ctx = TriggerContext::lazy(&fetch, None);
                    if ctx.app_excluded(&exclude) || ctx.app_excluded(&exclude_apps) {
                        suppressed = true;
                        continue;
                    }
                }
                let Some(w) = ensure_gesture_overlay(&app) else {
                    suppressed = true;
                    continue;
                };
                let serial = GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = w.show();
                shown = true;
                if !GESTURE_OVERLAY_READY.load(Ordering::Relaxed) {
                    wait_gesture_overlay_ready();
                }
                // 等前端就绪的这段时间里，这一笔可能已经结束（右键已抬起 / 触发键
                // 已松开）：来源一换就别再往浮层上画了。
                let still_drawing = if kada_hook::gesture::drawing() {
                    true
                } else {
                    kada_hook::trackpad::drawing()
                };
                if GESTURE_VISUAL_SERIAL.load(Ordering::Relaxed) != serial || !still_drawing {
                    continue;
                }
            }
            if points.len() == last_len {
                continue;
            }
            last_len = points.len();
            if let Some((origin, scale)) = gesture_window_geom(&app) {
                let payload = GesturePreviewPayload {
                    visual,
                    points: local_gesture_points(&points, origin),
                    scale,
                };
                if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
                    let _ = w.emit("gesture-preview", &payload);
                }
            }
        }
    });
}

/// 鼠标手势（规划 7.2-MG）：注册轨迹出口。
///
/// 回调**在钩子线程上**跑（低层钩子回调超会被系统静默摘掉），所以这里只做
/// 「拷贝数据 + 开线程」，识别与动作执行都在新线程里。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_gesture_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    let (overlay_app, overlay_cfg) = (app.clone(), cfg.clone());
    kada_hook::gesture::set_sink(Some(Arc::new(move |stroke| {
        let app = app.clone();
        let cfg = cfg.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 指针设备在**回调线程**上取（和 `fire` 取前台同一口径）：这一笔是哪台鼠标 / 触摸板 /
        // 数位板画的，`when` 里的「设备是」要用它。放到新线程里读会晚一拍、可能已被别的设备覆盖。
        let device = input::current_pointer_device();
        std::thread::spawn(move || handle_gesture(app, cfg, results, unread, runs, stroke, device));
    })));
    spawn_gesture_overlay(overlay_app, overlay_cfg);
}

/// 触摸板手势（7.2-MG）：注册轨迹出口。与右键手势同一个口径——回调在钩子线程上跑，
/// 这里只「拷贝数据 + 开线程」，识别与动作执行都在新线程里。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_trackpad_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::trackpad::set_sink(Some(Arc::new(move |key, stroke| {
        let app = app.clone();
        let cfg = cfg.clone();
        // 这一笔是哪块触摸板 / 数位板画的（手势的 `when` 里「设备是」要用它）：与鼠标手势
        // 同一口径，在回调线程上取一次带下去。
        let device = input::current_pointer_device();
        // 设置页的「录一笔」：只把识别结果推回界面，不执行任何动作（录制优先于正式匹配）。
        if trackpad_recording_key().is_some() {
            std::thread::spawn(move || emit_trackpad_record(&app, &cfg, &stroke));
            return;
        }
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        std::thread::spawn(move || {
            handle_trackpad_stroke(app, cfg, results, unread, runs, key, stroke, device)
        });
    })));
}

/// 一次触摸板手势的收尾：识别 → 命中就执行动作；**没命中什么都不用补**。
///
/// 与右键手势最大的不同：被吞掉的只有触发键本身（它只是「进入采集」的开关），
/// 所以这里没有 `replay_right_click`（吞键铁律要求回放的是「本可以正常工作的输入」，
/// 触发键不是）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_trackpad_stroke(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    trigger: Key,
    stroke: kada_hook::gesture::Stroke,
    device: Option<String>,
) {
    let (on, min_len, action_timeout, text_mode, exclude, exclude_apps, overlay) = {
        let g = cfg.read().unwrap();
        (
            g.settings.touchpad && touchpad_gesture_enabled(&g),
            g.settings.touchpad_start_distance as f64,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.touchpad_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.settings.touchpad_overlay,
        )
    };
    if !on {
        hide_gesture_overlay(&app);
        return;
    }
    // 让路：全屏程序、全局排除列表、手势排除列表。触发键已经被吞了（它没有输出），
    // 所以这里只是「不执行」，不是「回放」。
    if input::foreground_fullscreen() {
        hide_gesture_overlay(&app);
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        hide_gesture_overlay(&app);
        return;
    }
    // 触摸板上画一笔本来就慢，`gesture_timeout_ms` 那套「画太久作废」不适用：
    // 收尾时机是**松键**，不存在右键按住不放的副作用。
    let pattern = kada_core::pattern_from_stroke(&stroke.points, min_len);
    if pattern.is_empty() {
        // 太短 / 涂鸦 / 作废（Esc、双指、停手太久）：不猜，并把轨迹浮层收起来
        // （作废的那一笔靠这次回调收尾，否则浮层会一直挂着）。
        hide_gesture_overlay(&app);
        return;
    }
    let hit = {
        let g = cfg.read().unwrap();
        // 方向串是硬判据；**兜底模板只在没命中时用**（只加召回、不改判）。
        kada_core::find_touchpad(&g, trigger, &pattern, &ctx)
            .or_else(|| kada_core::find_touchpad_by_template(&g, &stroke.points, trigger, &ctx))
            .map(|g| (g.label(), g.actions.clone()))
    };
    let Some((name, actions)) = hit else {
        // 轨迹 + 失败色 + 方向串留在浮层上（约 900ms 自动收），另弹气泡说明怎么用。
        // 关掉「显示轨迹浮层」时只剩气泡（方向串照样报出来）。
        if overlay {
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                String::new(),
                pattern.clone(),
            );
        } else {
            hide_gesture_overlay(&app);
        }
        show_toast_note(
            &app,
            &format!("触摸板手势未匹配：{pattern}"),
            "把这个方向串填进那条手势的「方向串」就能用了。",
        );
        return;
    };
    if actions.is_empty() {
        if overlay {
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                name.clone(),
                pattern.clone(),
            );
        } else {
            hide_gesture_overlay(&app);
        }
        show_toast_note(
            &app,
            &format!("触摸板手势 {pattern} 没有动作"),
            "给这条手势配上动作才会执行。",
        );
        return;
    }
    if overlay {
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            true,
            name.clone(),
            pattern.clone(),
        );
    } else {
        hide_gesture_overlay(&app);
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("手势 {pattern}"),
        name,
        action_timeout,
        text_mode,
        device,
        stroke.points.first().copied(),
    );
}

/// 一次手势的收尾：识别 → 命中就执行动作；**没命中（或压根没画出轨迹）就把右键补回来**。
///
/// 吞键铁律：被吞掉的右键必须原样回放（按下-抬起配对），否则用户的右键就废了。
/// 回放走注入（带 `INJECTED` 标记），不会被自己的钩子再吃一次。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn handle_gesture(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    stroke: kada_hook::gesture::Stroke,
    device: Option<String>,
) {
    let (
        on,
        start_distance,
        min_segment,
        gesture_timeout,
        screenshot,
        action_timeout,
        text_mode,
        exclude,
        exclude_apps,
    ) = {
        let g = cfg.read().unwrap();
        (
            g.settings.experimental && gesture_capture_enabled(&g),
            g.settings.gesture_start_distance,
            g.settings.gesture_min_segment,
            g.settings.gesture_timeout_ms,
            g.settings.gesture_screenshot,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
        )
    };
    if !on {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 全屏程序自动暂停（规划 7.2-MG 余项⑤）：游戏 / 视频 / 远程桌面里右键拖拽本来就是正常
    // 操作，整条不接管、右键原样回放。判据见 `kada_hook::win::foreground_fullscreen`。
    if input::foreground_fullscreen() {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 排除程序：全局列表（EXT-04）与手势专用列表命中都整条不接管，右键原样回放。
    // 同一份上下文供后面的 `when` 复用，一次手势最多查一次前台。
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 位移没超过起手距离 = 普通右键点击（不是手势）；画太久 = 这次手势作废。两种都补回右键。
    let first = stroke.points.first().copied().unwrap_or((0, 0));
    let moved = stroke
        .points
        .iter()
        .map(|p| (p.0 - first.0).abs() + (p.1 - first.1).abs())
        .max()
        .unwrap_or(0);
    let pattern = kada_core::pattern_from_points(&stroke.points, min_segment);
    if moved < start_distance {
        hide_gesture_overlay(&app);
        replay_stroke(&stroke);
        return;
    }
    // 圈选截图：右键按住画完圈、**再点一下左键**才算确认——那一下左键就是「拍照」。
    // 只画圈不点 = 普通右键拖动（下面按没匹配处理，右键原样回放），不会误截。
    if stroke.lasso.is_some() {
        if screenshot {
            run_lasso_capture(&app);
        } else {
            hide_gesture_overlay(&app);
            replay_stroke(&stroke);
        }
        return;
    }
    let matched = if stroke.elapsed_ms > gesture_timeout {
        None
    } else {
        let g = cfg.read().unwrap();
        // `when` 门控复用上面已建好的上下文（排除判定若查过前台就已缓存，不再查询）。
        kada_core::find_gesture(&g, &pattern, &ctx).map(|hit| (hit.label(), hit.actions.clone()))
    };
    if let Some((name, actions)) = matched {
        // 配了手势但没配动作 = 半条配置：这时**不执行**（执行空链等于把右键吞了），
        // 按「没命中」处理、把右键补回去。
        if actions.is_empty() {
            eprintln!("kada: 手势 {pattern}（{name}）没有配置动作（右键已补回）");
            show_gesture_result(
                &app,
                &cfg,
                &stroke.points,
                false,
                name.clone(),
                pattern.clone(),
            );
            show_toast_note(
                &app,
                &format!("手势 {pattern} 没有动作"),
                "右键已补回。给这条「鼠标操作」配上动作才会执行。",
            );
            replay_stroke(&stroke);
            return;
        }
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            true,
            name.clone(),
            pattern.clone(),
        );
        fire(
            app,
            results,
            unread,
            runs,
            actions,
            format!("手势 {pattern}"),
            name,
            action_timeout,
            text_mode,
            device,
            // 手势起点 = 右键按下点（EXT-16「移动到手势起点」）；快捷键路径没有起点。
            stroke.points.first().copied(),
        );
        return;
    }
    // 只画了个圈但没点左键：不截图，告诉用户怎么确认（旧版是右键一松就截图，容易误截）。
    if screenshot && kada_core::lasso_bbox(&stroke.points).is_some() {
        show_gesture_result(
            &app,
            &cfg,
            &stroke.points,
            false,
            String::new(),
            pattern.clone(),
        );
        show_toast_note(
            &app,
            "圈选截图未确认",
            "按住右键画圈后再点一下左键；只画圈不截图。",
        );
        replay_stroke(&stroke);
        return;
    }
    if stroke.elapsed_ms > gesture_timeout {
        show_gesture_result(&app, &cfg, &stroke.points, false, String::new(), pattern);
        replay_stroke(&stroke);
        return;
    }
    eprintln!("kada: 未配置的手势 {pattern}（右键已补回）");
    // **看得见的反馈**：没匹配上时除了把右键补回去，还要告诉用户「你画的这个方向串
    // 叫 DRUR」——否则界面毫无反应，用户只会以为功能坏了（试验台也是这么提示的：
    // 照抄方向串进配置就能用）。弹气泡，不进消息中心（避免每次误拖都留一条）。
    show_gesture_result(
        &app,
        &cfg,
        &stroke.points,
        false,
        String::new(),
        pattern.clone(),
    );
    show_toast_note(
        &app,
        &format!("手势未匹配：{pattern}"),
        "右键已补回。把这个方向串填进「快捷键 → 鼠标操作」就能用了。",
    );
    replay_stroke(&stroke);
}

/// Snipaste 在不在跑：在跑才把图递给它（按 F3 没人接会落到前台程序上，宁可不发）。
#[cfg(target_os = "windows")]
fn snipaste_running() -> bool {
    input::process_running("snipaste.exe")
}

#[cfg(not(target_os = "windows"))]
fn snipaste_running() -> bool {
    false
}

/// 圈选截图收尾：收起手势预览 → 等浮层撤掉 → 直接调出 Snipaste 的截图界面（F1）。
///
/// 只负责「进入截图态」：框选范围、矩形 / 椭圆 / 箭头等标注都在 Snipaste 里由用户自己画。
/// 不自动裁剪、不写剪贴板——自动裁完就等于把标注挡在门外。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn run_lasso_capture(app: &tauri::AppHandle) {
    hide_gesture_overlay(app);
    // 浮层隐藏是异步的，等一拍再按键，免得截到手势轨迹。
    std::thread::sleep(Duration::from_millis(80));
    if !snipaste_running() {
        show_toast_note(
            app,
            "圈选截图需要 Snipaste",
            "装好 Snipaste 并让它保持运行，画圈后点左键会自动调出它的截图界面",
        );
        return;
    }
    input::simulate::tap(Key::F1);
    show_toast_note(
        app,
        "已进入截图",
        "Snipaste 截图界面已打开，框选后用矩形 / 椭圆 / 箭头自己标注",
    );
}

/// 触发角（EXT-10）：注册出口。回调在钩子线程上跑，这里只开线程做后续（不建窗 / 不 IO）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_hot_corner_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::hotcorner::set_sink(Some(Arc::new(move |corner| {
        let app = app.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 触发角是光标移进去的：设备 = 最近一次指针设备（`when` 里的「设备是」用它）。
        let device = input::current_pointer_device();
        std::thread::spawn(move || {
            handle_hot_corner(app, cfg, engine, results, unread, runs, corner, device)
        });
    })));
}

/// 防误触停留的「代次」：每次进角 +1。等够停留时长后只有**最后一次**进角的那条能触发——
/// 用户等得心烦、离开又马上回来时，前一条等待线程不该替后一条提前执行（会双触发）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
static HOT_CORNER_DWELL_GEN: AtomicU64 = AtomicU64::new(0);

/// 一次触发角的收尾：全屏 / 排除程序与手势同口径让路；命中就交给 `fire`。
///
/// **不吞任何鼠标事件**，所以这里没有回放逻辑。触发时抓前台窗口由 `fire` 在调用线程上做。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_hot_corner(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    corner: kada_hook::hotcorner::Corner,
    device: Option<String>,
) {
    let spot = match corner {
        kada_hook::hotcorner::Corner::TopLeft => kada_core::HotCornerSpot::TopLeft,
        kada_hook::hotcorner::Corner::TopRight => kada_core::HotCornerSpot::TopRight,
        kada_hook::hotcorner::Corner::BottomLeft => kada_core::HotCornerSpot::BottomLeft,
        kada_hook::hotcorner::Corner::BottomRight => kada_core::HotCornerSpot::BottomRight,
    };
    let (on, action_timeout, text_mode, exclude, exclude_apps, rules, active) = {
        let g = cfg.read().unwrap();
        // 锁序铁律：配置读锁先于 Engine 锁。
        let active = engine.lock().unwrap().active_layer().map(|s| s.to_string());
        (
            g.settings.experimental,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.hot_corners.clone(),
            active,
        )
    };
    if !on {
        return;
    }
    // 全屏 / 排除程序：与手势同口径，整条让路。
    if input::foreground_fullscreen() {
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        return;
    }
    // 层语义同快捷键：激活层里的规则优先，基础层兜底；`when` 不成立 = 这条不存在。
    let pick = |want: Option<&str>| -> Option<(Vec<Action>, String, u64)> {
        rules
            .iter()
            .find(|r| r.enabled && r.spot == spot && r.layer_matches(want) && r.when_matches(&ctx))
            .map(|r| (r.actions.clone(), r.label(), u64::from(r.dwell_ms)))
    };
    let Some((actions, name, dwell_ms)) = active
        .as_deref()
        .and_then(|id| pick(Some(id)))
        .or_else(|| pick(None))
    else {
        return;
    };
    // 没配动作 = 半条配置，什么也不做（没有可执行的链，也没有要回放的按键）。
    if actions.is_empty() {
        return;
    }
    // 防误触：等够停留时长再确认光标还在角上——扫过角落（关窗口、拖滚动条）不算触发。
    // 钩子那边这个角已被占住（停留不重复上报），离开超过重新武装距离才清；所以等待期间
    // 只有「离开又马上回来」会再进一枪，靠代次让旧等待作废，免得同一次停留执行两遍。
    if dwell_ms > 0 {
        let generation = HOT_CORNER_DWELL_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        std::thread::sleep(Duration::from_millis(dwell_ms));
        if HOT_CORNER_DWELL_GEN.load(Ordering::Relaxed) != generation
            || !kada_hook::hotcorner::still_near(corner)
        {
            return;
        }
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("触发角 {name}"),
        name,
        action_timeout,
        text_mode,
        device,
        None,
    );
}

/// 边缘滚动（EXT-11）：注册出口。回调在钩子线程上跑，这里只开线程做后续（不建窗 / 不 IO）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_wheel_edge_listener(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
) {
    kada_hook::wheel_edge::set_sink(Some(Arc::new(move |edge| {
        let app = app.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let results = results.clone();
        let unread = unread.clone();
        let runs = runs.clone();
        // 边缘滚动是滚轮触发的：设备 = 最近一次指针设备（`when` 里的「设备是」用它）。
        let device = input::current_pointer_device();
        std::thread::spawn(move || {
            handle_wheel_edge(app, cfg, engine, results, unread, runs, edge, device)
        });
    })));
}

/// 一次边缘滚动的收尾：全屏 / 排除程序与手势同口径让路；命中就交给 `fire`。
///
/// 钩子层**没有吞滚轮**，所以这里没有回放逻辑。触发时抓前台窗口由 `fire` 在调用线程上做。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn handle_wheel_edge(
    app: tauri::AppHandle,
    cfg: Arc<RwLock<Config>>,
    engine: Arc<Mutex<Engine>>,
    results: Arc<Mutex<Vec<CommandResult>>>,
    unread: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<RunRecord>>>,
    edge: kada_hook::wheel_edge::Edge,
    device: Option<String>,
) {
    let spot = match edge {
        kada_hook::wheel_edge::Edge::Top => kada_core::WheelEdgeSpot::Top,
        kada_hook::wheel_edge::Edge::Bottom => kada_core::WheelEdgeSpot::Bottom,
        kada_hook::wheel_edge::Edge::Left => kada_core::WheelEdgeSpot::Left,
        kada_hook::wheel_edge::Edge::Right => kada_core::WheelEdgeSpot::Right,
    };
    let (on, action_timeout, text_mode, exclude, exclude_apps, rules, active) = {
        let g = cfg.read().unwrap();
        // 锁序铁律：配置读锁先于 Engine 锁。
        let active = engine.lock().unwrap().active_layer().map(|s| s.to_string());
        (
            g.settings.experimental,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
            g.wheel_edges.clone(),
            active,
        )
    };
    if !on {
        return;
    }
    // 全屏 / 排除程序：与手势同口径，整条让路。
    if input::foreground_fullscreen() {
        return;
    }
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, device.as_deref());
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        return;
    }
    // 层语义同快捷键：激活层里的规则优先，基础层兜底；`when` 不成立 = 这条不存在。
    let pick = |want: Option<&str>| -> Option<(Vec<Action>, String)> {
        rules
            .iter()
            .find(|r| r.enabled && r.edge == spot && r.layer_matches(want) && r.when_matches(&ctx))
            .map(|r| (r.actions.clone(), r.label()))
    };
    let Some((actions, name)) = active
        .as_deref()
        .and_then(|id| pick(Some(id)))
        .or_else(|| pick(None))
    else {
        return;
    };
    // 没配动作 = 半条配置，什么也不做（滚轮本来就放行了）。
    if actions.is_empty() {
        return;
    }
    fire(
        app,
        results,
        unread,
        runs,
        actions,
        format!("边缘滚动 {name}"),
        name,
        action_timeout,
        text_mode,
        device,
        None,
    );
}

/// 把被手势吞掉的右键补回来（按下-抬起配对）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn replay_right_click() {
    let op = kada_core::MouseOp::Click {
        button: kada_core::MouseButton::Right,
    };
    if let Err(e) = input::simulate::mouse(&op) {
        eprintln!("kada: 回放右键失败：{e}");
    }
}

/// 手势让路 / 未命中时补回被吞的键：右键一定补；**圈选确认过的那记左键也要补**——
/// 钩子层确认时把左右键都吞了，壳层若因为全屏 / 排除程序 / 开关关着而不截图，不补就少一次点击。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn replay_stroke(stroke: &kada_hook::gesture::Stroke) {
    replay_right_click();
    if stroke.lasso.is_some() {
        let op = kada_core::MouseOp::Click {
            button: kada_core::MouseButton::Left,
        };
        if let Err(e) = input::simulate::mouse(&op) {
            eprintln!("kada: 回放左键失败：{e}");
        }
    }
}

/// 这串动作里有没有「往目标程序注入输入」的动作（Text / Keys / Mouse / 临时文本，以及注入
/// 切换键的 `System::SwitchDesktop`，含 If / Parallel 分支内的）。
///
/// 修饰键没松时只有这类动作会被污染（见 [`fire`]），命令 / 文件 / 应用类照常执行；
/// 静态过一遍链足够准，If / Parallel 的嵌套两路都算「可能有」。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn injects_keys(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Text { .. } | Action::Keys { .. } | Action::Mouse { .. } => true,
        // 「临时文本」走剪贴板粘贴，会发出 Ctrl+V / ⌘V；写入 / 读取剪贴板本身不注入。
        Action::Clipboard {
            operation: ClipboardOperation::TempStr { .. },
            ..
        } => true,
        // 算法动作只有勾了「直接注入结果」才走 SendInput；写变量那条不注入。
        Action::Transform { inject, .. } => *inject,
        Action::Group { actions, .. } => injects_keys(actions),
        // 切换虚拟桌面也是注入（`Win+Ctrl+←` 这类）：触发它的修饰键还按着的话，注入的组合键
        // 会被污染成 `Ctrl+Alt+Win+Ctrl+←`——切桌面就成了别的东西。
        #[cfg(feature = "automation")]
        Action::System {
            operation: kada_core::SystemOperation::SwitchDesktop { .. },
            ..
        } => true,
        #[cfg(feature = "automation")]
        Action::If {
            then, otherwise, ..
        } => injects_keys(then) || injects_keys(otherwise),
        #[cfg(feature = "automation")]
        Action::Parallel { actions, .. } => injects_keys(actions),
        _ => false,
    })
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn fire(
    _app: tauri::AppHandle,
    _results: Arc<Mutex<Vec<CommandResult>>>,
    _unread: Arc<AtomicBool>,
    _runs: Arc<Mutex<Vec<RunRecord>>>,
    _actions: Vec<Action>,
    _trigger: String,
    _name: String,
    _timeout_ms: u64,
    _text_mode: TextInjectMode,
    _device: Option<String>,
    _origin: Option<(i32, i32)>,
) {
    // 无钩子即无触发入口，本分支不会运行（macOS 占位）。
}

/// 悬浮指示窗渲染完成（前端量出内容宽高后调用）：按内容调整窗口、贴工作区底部居中并显示。
#[tauri::command]
fn hud_ready(app: tauri::AppHandle, width: f64, height: f64) {
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
fn get_status_payload(state: tauri::State<'_, KadaState>) -> Option<StatusPayload> {
    state.status.lock().unwrap().clone()
}

/// 输入窗渲染完成（前端量出内容宽高）：居中到光标所在显示器、显示并拿到焦点。
#[tauri::command]
fn prompt_ready(app: tauri::AppHandle, width: f64, height: f64) {
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
fn prompt_done(state: tauri::State<'_, KadaState>, value: Option<String>) {
    if let Some(tx) = state.prompt_tx.lock().unwrap().take() {
        let _ = tx.send(value);
    }
}

/// 读取当前输入框载荷（输入窗懒创建后，前端加载时兜底渲染）。
#[tauri::command]
fn get_prompt_payload(state: tauri::State<'_, KadaState>) -> Option<PromptPayload> {
    state.prompt_payload.lock().unwrap().clone()
}

/// 保存配置的一小处改动（提示框的位置 / 外观 / 可见性）：与 [`set_config`] 同一套口径——
/// 「写盘 + 改内存 + 给监听销账」同持 `cfg_watch` 锁。
///
/// 走它而不是让提示框窗口回传整份配置：那个窗口手上没有（也不该有）用户正在编辑的草稿，
/// 整份回传等于用一个只看得到提示框的视图去覆盖配置。
fn save_settings_patch(
    app: &tauri::AppHandle,
    state: &KadaState,
    patch: impl FnOnce(&mut Settings),
) -> Result<(), String> {
    let mut watch = state.cfg_watch.lock().unwrap();
    let mut cfg = state.config.read().unwrap().clone();
    patch(&mut cfg.settings);
    // 位置 / 外观都是「手改 JSON 也允许」的值：顺手钳一下越界，别存进去一个 scale: 0。
    cfg.settings.hints = std::mem::take(&mut cfg.settings.hints).normalized();
    config_io::save(&state.file, &cfg)?;
    *state.config.write().unwrap() = cfg.clone();
    watch.adopt_own_write(&state.file);
    drop(watch);
    // 广播给别的窗口：主界面的 `cfg.settings.hints` 已经旧了，它下次保存会把旧值写回去
    // （用户在提示框里拖了位置、旋了滚轮，回设置页改个快捷键就被打回原形）。
    let _ = app.emit("config-updated", &cfg);
    push_sync(&cfg);
    Ok(())
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

/// 暂停/恢复快捷键触发：录入组合键/序列/和弦时暂停，避免自触发。
/// 暂停是限时租约（[`PAUSE_LEASE_MS`]），前端若被中断来不及解除也不会永久卡死。
#[tauri::command]
fn set_paused(state: tauri::State<'_, KadaState>, paused: bool) {
    set_pause_lease(&state.paused_until, paused);
}

/// 中止正在执行的动作：剩余动作不再执行、正在跑的命令/脚本（含子进程）立即强制终止。
/// 返回中止请求发出时**正在执行**的动作链数量——0 表示此刻没有可停的东西，前端据此
/// 给出「当前没有正在执行的动作」，而不是假装停成功了。
#[tauri::command]
fn abort_actions() -> usize {
    let running = actions_abort::running();
    actions_abort::request();
    running
}

/// 编辑页「执行」按钮：把当前草稿的动作串当一次触发跑掉（不落盘、不看 `when` 与启停），
/// 走的与快捷键命中同一条 [`fire`] 路径——气泡、消息中心、`show_output` 弹窗一并复用。
#[tauri::command]
fn run_entry(
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
fn start_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
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
fn stop_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Action>, String> {
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
fn start_mouse_record(state: tauri::State<'_, KadaState>) -> Result<(), String> {
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
fn stop_mouse_record(state: tauri::State<'_, KadaState>) -> Result<Vec<Action>, String> {
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
fn start_trackpad_record(key: String) -> Result<(), String> {
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
fn stop_trackpad_record() {
    *TRACKPAD_RECORD.lock().unwrap() = None;
    kada_hook::trackpad::reset();
}

/// 读取消息中心全部命令结果（最新在前）。
#[tauri::command]
fn get_command_results(state: tauri::State<'_, KadaState>) -> Vec<CommandResult> {
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
fn get_unread(state: tauri::State<'_, KadaState>) -> bool {
    state.unread.load(Ordering::Relaxed)
}

/// 读取最近一次触发气泡的载荷（toast 窗口懒创建后，前端加载时调用兜底渲染）。
#[tauri::command]
fn get_toast_payload(state: tauri::State<'_, KadaState>) -> Option<ToastPayload> {
    state.toast.lock().unwrap().clone()
}

/// 标记全部已读（熄灭红点，还原托盘图标）。
#[tauri::command]
fn mark_results_read(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

/// 冷启动补发（CS-11）：前端启动并注册好监听器后取走没弹出来的结果（取走即清）。
#[tauri::command]
fn take_pending_result_modal(state: tauri::State<'_, KadaState>) -> Option<CommandResult> {
    state.pending_result_modal.lock().unwrap().take()
}

/// 复制文本到系统剪贴板（CS-10）：走平台注入层，不依赖 WebView 的
/// `navigator.clipboard` 权限（各平台 / 各 WebView 不一致）。
#[tauri::command]
fn copy_result_text(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("没有可复制的文本".into());
    }
    input::simulate::set_clipboard_text(&text).map_err(|e| format!("复制失败：{e}"))
}

/// 打开完整命令日志（CS-10）：只允许 `app_data_dir()/logs/commands/` 下的文件，
/// 用系统文件管理器定位到该文件（Windows `explorer /select,`、macOS `open -R`、
/// Linux `xdg-open <父目录>`）。
#[tauri::command]
fn open_result_log(app: tauri::AppHandle, path: String) -> Result<(), String> {
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
fn clear_command_logs(app: tauri::AppHandle) -> Result<usize, String> {
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
fn clear_command_results(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.results.lock().unwrap().clear();
    state.runs.lock().unwrap().clear();
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
}

/// 读取全部执行记录（最新在前）。界面的「执行记录」标签页拿它铺列表。
#[tauri::command]
fn get_run_records(state: tauri::State<'_, KadaState>) -> Vec<RunRecord> {
    state.runs.lock().unwrap().clone()
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
            set_paused,
            abort_actions,
            run_entry,
            start_record,
            stop_record,
            start_mouse_record,
            stop_mouse_record,
            start_trackpad_record,
            stop_trackpad_record,
            get_command_results,
            get_run_records,
            get_unread,
            get_toast_payload,
            get_status_payload,
            overlays::get_clipboard_menu_payload,
            overlays::clipboard_menu_ready,
            overlays::clipboard_menu_exec,
            tray::get_tray_menu_payload,
            tray::tray_menu_ready,
            tray::tray_menu_exec,
            hud_ready,
            prompt_ready,
            prompt_done,
            get_prompt_payload,
            gesture_overlay_ready,
            overlays::get_hints_state,
            overlays::hints_ready,
            overlays::hints_move,
            overlays::hints_commit,
            overlays::hints_prefs,
            overlays::hints_set_visible,
            mark_results_read,
            clear_command_results,
            clear_command_logs,
            take_pending_result_modal,
            copy_result_text,
            open_result_log,
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
