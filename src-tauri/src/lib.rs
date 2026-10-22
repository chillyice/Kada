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
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use kada_actions::{
    abort as actions_abort, run_actions, ActionStep, CommandResult, Reporter, RunOptions,
    RunRecord, RunStatus, TriggerCtx,
};
use kada_core::{
    config_needs_mouse_hook, detect_conflicts, diagnose_input_env, matches,
    parse_snippet_index, sanitize_config, snippet, snippet_title, Action, Config, Conflict,
    IndexEntry, Key, Modifier, RawEvent, Remap, Settings, Severity, Shortcut, ShortcutItem,
    SnippetPick, TextExpansion, TextInjectMode, TriggerContext, Vars, DEFAULT_SNIPPET_INDEX_URL,
    SYSTEM_SHORTCUTS,
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

/// 平台输入层。
#[cfg(target_os = "windows")]
mod input {
    //! Windows：全局低层键盘钩子（kada-hook）。
    pub use kada_hook::win::{
        current_device, foreground_fullscreen, foreground_window, frontmost_context,
        hotkey_occupied, reinstall_count, set_mouse_enabled, set_mouse_recorder,
        set_wheel_enabled, simulate, start, Action as HookAction, HookHandle, KeyEvent,
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
        current_device, frontmost_context, set_mouse_recorder, simulate, start, Action as HookAction,
        HookHandle, KeyEvent,
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
        current_device, foreground_window, frontmost_context, reinstall_count, set_mouse_recorder,
        simulate, start, Action as HookAction, HookHandle, KeyEvent,
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

    /// 无输入层实现：没有前台窗口，全屏判定恒 false。
    pub fn foreground_fullscreen() -> bool {
        false
    }

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
    /// 是否有未读命令结果（红点）。
    unread: Arc<AtomicBool>,
    /// 消息中心的执行记录表（内存态，重启清空）：一次触发一条，内含逐步记录
    /// （时间线 / 流水线视图的数据源，见 [`fire`]）。
    runs: Arc<Mutex<Vec<RunRecord>>>,
    /// 最近一次触发气泡的载荷（懒创建 toast 窗口时，供前端加载后兜底读取）。
    toast: Arc<Mutex<Option<ToastPayload>>>,
    /// 最近一次输入状态指示的载荷（懒创建状态指示窗时，供前端加载后兜底读取）。
    status: Arc<Mutex<Option<StatusPayload>>>,
    /// 未匹配手势待添加的方向串（EXT-03）：主窗口可能是刚被唤出的，事件会早于前端监听器，
    /// 所以先落这里，前端加载后 `take_pending_gesture` 取走。
    pending_gesture: Arc<Mutex<Option<String>>>,
    /// 托盘图标（用于运行时叠 / 去红点）。
    tray: Mutex<Option<TrayIcon>>,
    /// 托盘基础图标（无红点）。
    tray_base: Option<tauri::image::Image<'static>>,
    /// 托盘带红点图标（未读态）。
    tray_unread: Option<tauri::image::Image<'static>>,
    /// 托盘菜单里的「快捷键提示」勾选项：提示框的可见性可能被窗口上的 × 改掉，
    /// 那时得把勾也抹掉——不记住它就只能干看着菜单与现状不一致。
    hints_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 托盘菜单里的「暂停快捷键触发」勾选项：`settings.paused` 也可能被设置页改掉，
    /// 由状态机定时器线程统一把勾对齐（见 [`spawn_engine_ticker`]）。
    pause_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
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
            // 配置读锁在 `Engine` 锁**之前**取、并一路持有到 `tick`：等待窗取值就来自配置
            // （`Settings.sequence_timeout_ms` / `chord_timeout_ms`），而钩子回调路径也是
            // 「先读配置、再锁引擎」——两条路径的加锁顺序必须一致，反过来会死锁。
            let (payload, hud_enabled, use_mouse, use_wheel, user_paused, elevated, gesture_on) = {
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
                    config_needs_mouse_hook(&guard),
                    kada_core::config_uses_wheel(&guard),
                    guard.settings.paused,
                    guard.settings.elevated_inject,
                    // 手势采集的闸门：**新功能总开关**（规划 7.2-MG）关了、或没有启用的手势
                    // → 右键完全放行（回到系统行为，一个字节都不多吞）。
                    guard.settings.experimental && kada_core::gesture_enabled(&guard),
                )
            };
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
            // 不弹窗：这条随时可能在用户输密码的当下出现，弹窗既抢焦点又不合时宜。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );
}

/// 提权窗口注入的现状（设置页卡片的数据源，规划 7.2-㉑a）。
#[derive(Serialize)]
struct ElevatedStatus {
    /// 本平台有没有这套能力（只有 Windows 有）。
    supported: bool,
    /// 开关（`settings.elevated_inject`）是否打开。
    enabled: bool,
    /// 提权进程是否已连上。
    connected: bool,
}

/// 提权窗口注入的现状。
#[tauri::command]
fn elevated_status() -> ElevatedStatus {
    let s = input::elevated::status();
    ElevatedStatus { supported: s.supported, enabled: s.enabled, connected: s.connected }
}

/// 拉起提权进程并连上通道（**唯一会弹 UAC 的地方**，只由设置页的按钮触发；连接有超时，
/// 用户不点 UAC 也不会把界面挂住）。
#[tauri::command]
fn elevated_start() -> Result<String, String> {
    input::elevated::start()
}

/// 断开提权通道（提权进程随之退出；开关状态不动，下次点「启动」或重启应用即可再来）。
#[tauri::command]
fn elevated_stop() -> Result<(), String> {
    input::elevated::stop();
    Ok(())
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
    // 触发设备在**回调线程上**取（同步）：`current_device` 是「最近一次事件的设备」，
    // 放到下面 spawn 的线程里读会晚一拍、可能已被别的设备覆盖。Linux evdev / Windows
    // Raw Input 有真值，macOS 恒 None（见 7.2-㊷）。
    let device = input::current_device();
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

/// 收起浮层，并让旧的隐藏定时器失效。
fn hide_gesture_overlay(app: &tauri::AppHandle) {
    GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window(GESTURE_WINDOW) {
        let _ = w.hide();
    }
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
    let newly = app.get_webview_window(GESTURE_WINDOW).is_none();
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
        // 刚建的窗口要等前端加载完监听器，否则第一个事件会丢（toast 用拉取兜底，
        // 这里只是短暂提示，等一拍再发更省事）。
        if newly {
            std::thread::sleep(Duration::from_millis(180));
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
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn spawn_gesture_overlay(app: tauri::AppHandle, cfg: Arc<RwLock<Config>>) {
    std::thread::spawn(move || {
        let mut was_drawing = false;
        let mut shown = false;
        let mut suppressed = false;
        let mut last_len = usize::MAX;
        loop {
            let interval = if kada_hook::gesture::enabled() { 16 } else { 120 };
            std::thread::sleep(Duration::from_millis(interval));
            if !kada_hook::gesture::drawing() {
                was_drawing = false;
                shown = false;
                suppressed = false;
                last_len = usize::MAX;
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
            let points = kada_hook::gesture::points();
            if points.len() < 2 {
                continue;
            }
            let (visual, start_distance, on, exclude, exclude_apps) = {
                let g = cfg.read().unwrap();
                (
                    gesture_visual_of(&g.settings),
                    g.settings.gesture_start_distance,
                    g.settings.experimental && kada_core::gesture_enabled(&g),
                    g.settings.gesture_exclude.clone(),
                    g.settings.exclude_apps.clone(),
                )
            };
            if !on {
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
                if input::foreground_fullscreen() {
                    suppressed = true;
                    continue;
                }
                if !exclude.is_empty() || !exclude_apps.is_empty() {
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
                let _ = w.show();
                GESTURE_VISUAL_SERIAL.fetch_add(1, Ordering::Relaxed);
                shown = true;
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
        std::thread::spawn(move || handle_gesture(app, cfg, results, unread, runs, stroke));
    })));
    spawn_gesture_overlay(overlay_app, overlay_cfg);
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
) {
    let (
        on,
        start_distance,
        min_segment,
        gesture_timeout,
        action_timeout,
        text_mode,
        exclude,
        exclude_apps,
    ) = {
        let g = cfg.read().unwrap();
        (
            g.settings.experimental && kada_core::gesture_enabled(&g),
            g.settings.gesture_start_distance,
            g.settings.gesture_min_segment,
            g.settings.gesture_timeout_ms,
            g.settings.action_timeout_ms,
            g.settings.text_inject_mode,
            g.settings.gesture_exclude.clone(),
            g.settings.exclude_apps.clone(),
        )
    };
    if !on {
        hide_gesture_overlay(&app);
        replay_right_click();
        return;
    }
    // 全屏程序自动暂停（规划 7.2-MG 余项⑤）：游戏 / 视频 / 远程桌面里右键拖拽本来就是正常
    // 操作，整条不接管、右键原样回放。判据见 `kada_hook::win::foreground_fullscreen`。
    if input::foreground_fullscreen() {
        hide_gesture_overlay(&app);
        replay_right_click();
        return;
    }
    // 排除程序：全局列表（EXT-04）与手势专用列表命中都整条不接管，右键原样回放。
    // 同一份上下文供后面的 `when` 复用，一次手势最多查一次前台。
    let fetch = || input::frontmost_context();
    let ctx = TriggerContext::lazy(&fetch, None);
    if ctx.app_excluded(&exclude_apps) || ctx.app_excluded(&exclude) {
        hide_gesture_overlay(&app);
        replay_right_click();
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
        replay_right_click();
        return;
    }
    if stroke.elapsed_ms > gesture_timeout {
        show_gesture_result(&app, &cfg, &stroke.points, false, String::new(), pattern);
        replay_right_click();
        return;
    }
    let matched = {
        let g = cfg.read().unwrap();
        // `when` 门控复用上面已建好的上下文（排除判定若查过前台就已缓存，不再查询）。
        kada_core::find_gesture(&g, &pattern, &ctx).map(|hit| (hit.label(), hit.actions.clone()))
    };
    let Some((name, actions)) = matched else {
        eprintln!("kada: 未配置的手势 {pattern}（右键已补回）");
        // **看得见的反馈**：没匹配上时除了把右键补回去，还要告诉用户「你画的这个方向串
        // 叫 DRUR」——否则界面毫无反应，用户只会以为功能坏了（试验台也是这么提示的：
        // 照抄方向串进配置就能用）。弹气泡，不进消息中心（避免每次误拖都留一条）。
        show_gesture_result(&app, &cfg, &stroke.points, false, String::new(), pattern.clone());
        show_toast_note(
            &app,
            &format!("手势未匹配：{pattern}"),
            "右键已补回。把这个方向串填进「快捷键 → 鼠标操作」就能用了。",
        );
        replay_right_click();
        // EXT-03 手势添加模式：唤出主窗口并打开「新增鼠标操作」，方向串预填成刚画的这条。
        // 只开界面、不执行任何动作；右键已在上面补回。
        if let Some(state) = app.try_state::<KadaState>() {
            state.pending_gesture.lock().unwrap().replace(pattern);
        }
        let add_app = app.clone();
        std::thread::spawn(move || {
            show_main_window(&add_app);
            let _ = add_app.emit("gesture-add", ());
        });
        return;
    };
    // 配了手势但没配动作 = 半条配置：这时**不执行**（执行空链等于把右键吞了），
    // 按「没命中」处理、把右键补回去。
    if actions.is_empty() {
        eprintln!("kada: 手势 {pattern}（{name}）没有配置动作（右键已补回）");
        show_gesture_result(&app, &cfg, &stroke.points, false, name.clone(), pattern.clone());
        show_toast_note(
            &app,
            &format!("手势 {pattern} 没有动作"),
            "右键已补回。给这条「鼠标操作」配上动作才会执行。",
        );
        replay_right_click();
        return;
    }
    show_gesture_result(&app, &cfg, &stroke.points, true, name.clone(), pattern.clone());
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

/// 执行记录表的条数上限（超出丢最旧的一条）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const MAX_RUNS: usize = 50;
/// 单条执行记录的动作条数上限（宏里套一大坨循环时会超；丢最旧的，中间的时间线仍完整）。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
const MAX_RUN_STEPS: usize = 500;

/// 新开一条执行记录（最新在前）并广播。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn start_run(
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
fn update_run(
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

/// 追加一条动作执行记录（时间线的一个时间块 / 流水线的一个节点），顺带把总耗时推到最新。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn add_step(
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
        run.steps.push(step);
    });
    if snapshot.is_some() {
        unread.store(true, Ordering::Relaxed);
        set_tray_unread(app, true);
    }
}

/// 收尾：把整条记录的结论（跑完 / 有失败 / 被中止）定下来。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn finish_run(
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

/// 这串动作里有没有「往目标程序注入输入」的动作（Text / Keys / Mouse，以及注入切换键的
/// `System::SwitchDesktop`，含 If / Parallel 分支内的）。
///
/// 修饰键没松时只有这类动作会被污染（见 [`fire`]），命令 / 文件 / 应用类照常执行；
/// 静态过一遍链足够准，If / Parallel 的嵌套两路都算「可能有」。
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "macos"))]
fn injects_keys(actions: &[Action]) -> bool {
    actions.iter().any(|a| match a {
        Action::Text { .. } | Action::Keys { .. } | Action::Mouse { .. } => true,
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
) {
    // 无钩子即无触发入口，本分支不会运行（macOS 占位）。
}

/// 触发序号：连按多个快捷键时，只有最后一次气泡到点后隐藏。
static TOAST_SERIAL: AtomicU64 = AtomicU64::new(0);

/// 按需创建右下角触发气泡窗口：首次触发快捷键时才真正建出 WebView（冷启动零成本）。
fn ensure_toast(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("toast") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        "toast",
        tauri::WebviewUrl::App("index.html#toast".into()),
    )
    .decorations(false)
    .transparent(true)
    // 浮窗一律 `shadow(false)`：tao 对「无边框 + 有投影」的窗会保留整套 DWM 边框（Win11 上是
    // 一圈 1px 圆角亮边 + 外投影），而这几个窗的内容都比窗口矩形小一圈（透明边是留给 CSS 投影
    // 的），那圈边框就挂在面板外面，看着像凭空多了一个空框。投影由 CSS 自己画，见 `style.css`
    // 里 `.toast-bubble` / `.hud-row` / `.hints-panel` 的 `box-shadow`。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    // 同 `ensure_hud`：`focused(false)` 只保证**第一次**显示不抢焦点（tao 首次用
    // `SW_SHOWNOACTIVATE`，之后清掉这个标记、退回 `SW_SHOW`），而气泡每次触发都 show 一次。
    // 加 `focusable(false)`（`WS_EX_NOACTIVATE`）才真的永不成为前台窗口——否则连按两次快捷键，
    // 第二次气泡就会把焦点从用户正在打字的窗口抢走，`ResetWatch` 还会把它当成「前台切换」
    // 白复位一次输入状态。
    .focusable(false)
    .inner_size(300.0, 60.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建气泡窗口失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    if let Ok(Some(m)) = app.primary_monitor() {
        use tauri::LogicalPosition;
        let scale = m.scale_factor();
        let work = m.work_area();
        let x = (work.position.x as f64 + work.size.width as f64 - 300.0 - 16.0) / scale;
        let y = (work.position.y as f64 + work.size.height as f64 - 60.0 - 16.0) / scale;
        let _ = w.set_position(LogicalPosition::new(x, y));
    }
    Some(w)
}

/// 按需创建屏幕下方的输入状态悬浮窗（见规划 7.3-⑬）：真有状态要显示时才建 WebView。
///
/// **必须 `focusable(false)`**——Windows 上就是 `WS_EX_NOACTIVATE`，否则这个窗能被激活，
/// 后果很具体：它在用户按住切层键期间一直挂在最上层，一旦成了前台窗口，① 前台条件（前台应用 /
/// 窗口标题）全读成我们自己这个窗；② `ResetWatch` 把它当作「前台切换」→ 复位输入状态，
/// 按住的 momentary 层当场被踢掉——正是本功能要消除的那种「按了没反应」。它只是一块看板：
/// 不接受点击、不接焦点。位置与尺寸在 [`hud_ready`] 里按内容定（要贴底部居中，宽高得先量出来）。
fn ensure_hud(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("hud") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        "hud",
        tauri::WebviewUrl::App("index.html#hud".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同 `ensure_toast`：别借系统投影，那圈 DWM 边框会挂在面板外面。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .inner_size(240.0, 44.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建状态指示窗失败: {e}");
            return None;
        }
    };
    let _ = w.set_ignore_cursor_events(true);
    Some(w)
}

/// 输入状态可视指示（见规划 7.3-⑬）：状态变化时刷新托盘提示，并按需显示 / 隐藏悬浮指示窗。
///
/// 由状态机定时器线程调用，**必须在配置读锁与 `Engine` 锁之外**：这里要动托盘图标、建窗口、
/// 发事件，而钩子回调正等着那把 `Engine` 锁——在锁里干这些就等于把整个输入层挂在界面上。
/// 重量级的那步（建 WebView）另外挪到自己的线程上，见函数末尾。
fn publish_status(app: &tauri::AppHandle, payload: &StatusPayload, hud_enabled: bool) {
    // 定时器线程比 `app.manage(KadaState)` 先起步（差几十毫秒），用 `try_state` 兜一下：
    // 拿不到就等下一拍，别在后台线程里 panic。
    let Some(state) = app.try_state::<KadaState>() else {
        return;
    };
    // 载荷存进 state：悬浮窗是懒创建的，前端加载后据此兜底渲染（与触发气泡同一个套路）。
    state.status.lock().unwrap().replace(payload.clone());
    // 托盘提示：悬停才看得见，但零成本、最不打扰，状态为空时回到应用名。
    if let Some(tray) = state.tray.lock().unwrap().as_ref() {
        let tip = if payload.idle() {
            "咔哒 Kada".to_string()
        } else {
            format!("咔哒 Kada · {}", payload.summary)
        };
        let _ = tray.set_tooltip(Some(&tip));
    }
    // 悬浮指示：设置里关掉了、或没有东西生效 → 收起来（窗口留着重用，不销毁）。
    if !hud_enabled || payload.idle() {
        if let Some(w) = app.get_webview_window("hud") {
            let _ = w.hide();
        }
        return;
    }
    // 建 WebView 是重活（首次上百毫秒），**不能就地做**：这条线程还要落地序列 / 和弦的超时
    // 回放（`Engine::tick`），占住它等于让被吞掉的键晚几百毫秒才补回来。与触发气泡同一条
    // 约定——唤窗一律交给别的线程（`show_toast` 也是在 `std::thread::spawn` 里建窗的）。
    let (app, payload) = (app.clone(), payload.clone());
    std::thread::spawn(move || {
        if let Some(w) = ensure_hud(&app) {
            // 显示交给前端：它渲染完量出内容宽高再调 [`hud_ready`]（要贴底部居中，尺寸得先量）。
            // 这次 emit 晚到也没关系：窗口若已过期，`hud_ready` 会再查一次当前状态并把它藏回去。
            let _ = w.emit("status-change", &payload);
        }
    });
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

// ---- 快捷键提示框（常显速查浮窗，见规划 7.3-㊳） ----

/// 按需创建快捷键提示框窗口（`index.html#hints`）：常显的快捷键速查表，可拖动、滚轮调大小 /
/// 透明度。首次打开（或启动时配置里勾着显示）才建 WebView，冷启动零成本。
///
/// **必须 `focusable(false)`**（Windows 即 `WS_EX_NOACTIVATE`），理由与 [`ensure_hud`] 完全一致：
/// 这个窗是**常显**的，一整天挂在所有窗口之上，一旦它能被激活，点它一下就把它自己变成了
/// `GetForegroundWindow`——前台条件（前台应用 / 窗口标题）全读成我们自己这个窗，`ResetWatch`
/// 还会把它当作「前台切换」复位输入状态（按住的层当场被踢掉）。
///
/// 与 hud / toast 的关键差别：**不能** `set_ignore_cursor_events(true)`——拖动、滚轮、关闭按钮
/// 都要靠鼠标事件；`WS_EX_NOACTIVATE` 只挡激活，不挡鼠标消息，所以交互照常。
fn ensure_hints(app: &tauri::AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("hints") {
        return Some(w);
    }
    let w = match tauri::WebviewWindowBuilder::new(
        app,
        "hints",
        tauri::WebviewUrl::App("index.html#hints".into()),
    )
    .decorations(false)
    .transparent(true)
    // 同 `ensure_toast`：别借系统投影，那圈 DWM 边框会挂在面板外面。
    .shadow(false)
    .skip_taskbar(true)
    .always_on_top(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    // Ctrl+滚轮在本窗口是「调透明度」，不能让它同时把整个 WebView 的缩放也改了
    // （Chromium 默认把 Ctrl+wheel 当浏览器缩放，两件事叠在一起会互相打架）。
    .zoom_hotkeys_enabled(false)
    .inner_size(320.0, 420.0)
    .visible(false)
    .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("创建快捷键提示框失败: {e}");
            return None;
        }
    };
    Some(w)
}

/// 提示框的当前状态（`hints_ready` 回给前端做拖动基准，`hints-state` 事件推给窗口）。
fn hints_state_of(app: &tauri::AppHandle) -> HintsState {
    let hints = app
        .state::<KadaState>()
        .config
        .read()
        .map(|c| c.settings.hints.clone())
        .unwrap_or_default();
    // 窗口真实位置优先于配置：拖动中配置还是旧的（松手才落盘），拿它当基准会跳。
    let pos = app.get_webview_window("hints").and_then(|w| {
        let scale = w.scale_factor().unwrap_or(1.0);
        w.outer_position()
            .ok()
            .map(|p| (p.x as f64 / scale, p.y as f64 / scale))
    });
    let (x, y) = pos.unwrap_or((
        hints.x.map(f64::from).unwrap_or(0.0),
        hints.y.map(f64::from).unwrap_or(0.0),
    ));
    HintsState {
        visible: hints.visible,
        scale: hints.scale,
        opacity: hints.opacity,
        x,
        y,
    }
}

/// 保存位置是否落在某块显示器的工作区里（至少露出一角够拖回来）。
///
/// 配置是跨平台同步的：在双屏机器上拖到副屏，同步到单屏机器上就落在屏幕外了——
/// 那时宁可回到默认位，也不能让提示框「显示着但看不见」。
fn hints_position_on_screen(app: &tauri::AppHandle, x: i32, y: i32, w: f64, h: f64) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let s = m.scale_factor();
        let area = m.work_area();
        let (mx, my) = (area.position.x as f64 / s, area.position.y as f64 / s);
        let (mw, mh) = (area.size.width as f64 / s, area.size.height as f64 / s);
        let visible_w = (x as f64 + w).min(mx + mw) - (x as f64).max(mx);
        let visible_h = (y as f64 + h).min(my + mh) - (y as f64).max(my);
        visible_w >= 120.0 && visible_h >= 40.0
    })
}

/// 提示框该落在哪：优先用保存的位置；没保存过、或那个位置已经不在任何显示器工作区里
/// （换过显示器 / 跨平台同步来的坐标）就回到主显示器工作区右侧的默认位。
fn hints_target_position(
    app: &tauri::AppHandle,
    logical_w: f64,
    logical_h: f64,
    x: Option<i32>,
    y: Option<i32>,
) -> Option<tauri::LogicalPosition<f64>> {
    if let (Some(x), Some(y)) = (x, y) {
        if hints_position_on_screen(app, x, y, logical_w, logical_h) {
            return Some(tauri::LogicalPosition::new(x as f64, y as f64));
        }
    }
    let m = app.primary_monitor().ok().flatten()?;
    let scale = m.scale_factor();
    let area = m.work_area();
    let (mx, my) = (
        area.position.x as f64 / scale,
        area.position.y as f64 / scale,
    );
    let (mw, mh) = (
        area.size.width as f64 / scale,
        area.size.height as f64 / scale,
    );
    // 默认贴右侧、垂直居中：不挡正在打字的输入框，也不挡底部居中的状态指示。
    let x = mx + mw - logical_w - 24.0;
    let y = my + (mh - logical_h) / 2.0;
    Some(tauri::LogicalPosition::new(
        x.max(mx + 8.0),
        y.max(my + 8.0),
    ))
}

/// 提示框渲染完成（前端量出内容宽高后调用）：按内容定尺寸、首次落地并显示。
///
/// 尺寸由界面量（`hints_ready` 与 `hud_ready` 同一个套路）：内容随快捷键条数与字号变，
/// 后端不该去猜；但界面给的值不可全信，钳一下免得异常值把窗口撑满屏或只剩一条缝。
#[tauri::command]
fn hints_ready(app: tauri::AppHandle, width: f64, height: f64) -> Option<HintsState> {
    let w = app.get_webview_window("hints")?;
    let logical_w = width.clamp(180.0, 900.0);
    let logical_h = height.clamp(120.0, 2000.0);
    let _ = w.set_size(tauri::LogicalSize::new(logical_w, logical_h));
    let state = app.try_state::<KadaState>()?;
    let (visible, x, y) = {
        let guard = state.config.read().ok()?;
        (
            guard.settings.hints.visible,
            guard.settings.hints.x,
            guard.settings.hints.y,
        )
    };
    if !visible {
        // 量尺寸这段时间里它已经被关掉了：别把刚藏起来的窗又亮回来（同 `hud_ready`）。
        let _ = w.hide();
        return Some(hints_state_of(&app));
    }
    // 只在本次会话还没摆过位时落地：摆过之后位置由用户拖拽掌控，重排/缩放都不再挪它。
    if !state.hints_placed.swap(true, Ordering::Relaxed) {
        if let Some(pos) = hints_target_position(&app, logical_w, logical_h, x, y) {
            let _ = w.set_position(pos);
        }
    }
    let _ = w.show();
    Some(hints_state_of(&app))
}

/// 读取提示框状态（窗口懒创建后前端加载时拉一次，拿到字号 / 透明度 / 实际位置）。
#[tauri::command]
fn get_hints_state(app: tauri::AppHandle) -> HintsState {
    hints_state_of(&app)
}

/// 拖动中：把窗口移到指定逻辑坐标。**不落盘**——拖动时每帧写盘既慢又浪费，
/// 松手由 [`hints_commit`] 一次记下。
#[tauri::command]
fn hints_move(app: tauri::AppHandle, x: f64, y: f64) {
    if let Some(w) = app.get_webview_window("hints") {
        let _ = w.set_position(tauri::LogicalPosition::new(x, y));
    }
}

/// 拖动结束：把当前位置记进配置，下次启动还在原地。
#[tauri::command]
fn hints_commit(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) -> Result<(), String> {
    let Some(w) = app.get_webview_window("hints") else {
        return Ok(());
    };
    let scale = w.scale_factor().unwrap_or(1.0);
    let Ok(pos) = w.outer_position() else {
        return Ok(());
    };
    let (x, y) = (
        (pos.x as f64 / scale).round() as i32,
        (pos.y as f64 / scale).round() as i32,
    );
    save_settings_patch(&app, &state, |s| {
        s.hints.x = Some(x);
        s.hints.y = Some(y);
    })
}

/// 滚轮调完大小 / 透明度后落盘（前端已经就地应用，这里只负责持久化；前端做了节流）。
#[tauri::command]
fn hints_prefs(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    scale: u16,
    opacity: u8,
) -> Result<(), String> {
    save_settings_patch(&app, &state, |s| {
        s.hints.scale = scale;
        s.hints.opacity = opacity;
    })
}

/// 显示 / 隐藏提示框（设置页勾选、托盘菜单、窗口上的 × 都走这里）。
///
/// 关掉只把 `visible` 置 false，位置与外观留着——这是「关掉了还能从软件里再打开」的前提：
/// 下次打开必须还在原地、还是他调好的样子。
#[tauri::command]
fn hints_set_visible(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    visible: bool,
) -> Result<(), String> {
    save_settings_patch(&app, &state, |s| s.hints.visible = visible)?;
    apply_hints_visible(&app, visible);
    Ok(())
}

/// 把提示框的可见性落到窗口与托盘：显示 / 隐藏 + 抹平托盘勾选 + 通知窗口。
///
/// 可见性有四个入口（窗口 × / 设置页勾选 / 托盘菜单 / 外部改配置后重载），无论从哪进来
/// 都得把另外三处对齐，否则会出现「菜单勾着、窗口却没了」这种查不出原因的错位。
fn apply_hints_visible(app: &tauri::AppHandle, visible: bool) {
    if let Some(item) = app.state::<KadaState>().hints_tray.lock().unwrap().as_ref() {
        let _ = item.set_checked(visible);
    }
    if visible {
        // 建 WebView 是重活（首次上百毫秒）：别占着托盘事件线程 / 命令线程。
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = ensure_hints(&app);
        });
    } else if let Some(w) = app.get_webview_window("hints") {
        let _ = w.hide();
    }
    let _ = app.emit("hints-state", hints_state_of(app));
}

/// 提示框「悬停让位」的巡检线程：光标一进框就穿透让位（不挡住后面的页面），停留够久再恢复
/// 可交互；挪出边界后下次再进来照样让位。
///
/// 为什么只能轮询、不能用鼠标事件：让位本身就是靠「这个窗不收鼠标消息」实现的，一让位
/// 就再也没有 `mouseenter` / `mouseleave` 可言了。要在穿透状态下还知道光标在不在框里，
/// 只能自己盯全局光标位置（同 [`spawn_config_watcher`] 的取舍：轮询，不引依赖与平台分支，
/// 代价只是慢一拍）。判定与副作用分开写：相位推演是纯函数 [`HintsHover`]，这条线程只负责
/// 取光标 / 窗口矩形、落地 `set_ignore_cursor_events` 与通知界面变淡。
fn spawn_hints_hover_watch(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut phase = HintsHover::Out;
        // 「光标进框」的那一刻：让位与恢复都按它算停留时长。
        let mut since = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(HINTS_HOVER_POLL_MS));
            // 还没建（懒创建）：没有窗口可管，等它出来。
            let Some(w) = app.get_webview_window("hints") else {
                continue;
            };
            if !w.is_visible().unwrap_or(false) {
                // 被关掉 / 还没显示：相位复位，下次显示重新从「进框即让位」开始。
                // 这里必须真的撤掉穿透标记，不能只改相位——否则关掉时正让着位，下次显示出来
                // 就是一块点不着的透明板。
                if phase != HintsHover::Out {
                    phase = HintsHover::Out;
                    let _ = w.set_ignore_cursor_events(false);
                    let _ = app.emit("hints-peek", false);
                }
                continue;
            }
            let inside = hints_cursor_over(&w);
            let next = hints_hover_step(phase, inside, since.elapsed().as_millis() as u64);
            if next != phase {
                if next == HintsHover::Passing {
                    since = Instant::now();
                }
                // 让位 = 穿透 + 变淡，恢复同理。穿透是系统层的事，用户看不见：同时通知界面
                // 变淡（`hints-peek`），否则「鼠标放上去它就不理我了」会像失灵。
                let pass_through = next == HintsHover::Passing;
                let _ = w.set_ignore_cursor_events(pass_through);
                let _ = app.emit("hints-peek", pass_through);
                phase = next;
            }
        }
    });
}

/// 光标停在提示框上超过这么久，提示框才恢复可交互：让位是为了点后面的页面，真想用这个框
/// （拖动 / 滚轮 / × 关闭）就得先「表明意图」停一会儿。
const HINTS_HOVER_DWELL_MS: u64 = 2000;
/// 悬停巡检的节拍：让位判定最迟晚这么一拍，代价是每秒十来次极廉价的取位查询。
const HINTS_HOVER_POLL_MS: u64 = 120;

/// 悬停让位的相位（[`hints_hover_step`] 的输入输出，纯判定、可单测）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HintsHover {
    /// 光标不在框内：可交互（默认状态）。
    Out,
    /// 刚进框：已穿透让位，等光标停留满 [`HINTS_HOVER_DWELL_MS`]。
    Passing,
    /// 停留够久：恢复可交互，一直保持到光标挪出边界——期间挪进挪出都不会再让位。
    Settled,
}

/// 推演下一拍该是什么相位。`dwell_ms` 是本拍距「光标进框」过了多久。
///
/// 相位只在 `Out ↔ Passing ↔ Settled` 之间走一格一格：出框无条件回 `Out`（下次再进来照样
/// 让位），`Passing` 攒够停留时长才升 `Settled`。
fn hints_hover_step(cur: HintsHover, inside: bool, dwell_ms: u64) -> HintsHover {
    match (cur, inside) {
        (_, false) => HintsHover::Out,
        (HintsHover::Settled, true) => HintsHover::Settled,
        (HintsHover::Passing, true) if dwell_ms >= HINTS_HOVER_DWELL_MS => HintsHover::Settled,
        _ => HintsHover::Passing,
    }
}

/// 光标是否落在提示框的窗口矩形内。
///
/// 拿不到光标位置（Wayland 没有全局光标等）就当「不在框内」：退化成让位前的行为，提示框
/// 一直可交互，不至于反过来把框弄成点不着的透明板。
fn hints_cursor_over(w: &tauri::WebviewWindow) -> bool {
    let (Ok(pos), Ok(size), Ok(cur)) = (w.outer_position(), w.outer_size(), w.cursor_position())
    else {
        return false;
    };
    point_in_rect((cur.x, cur.y), (pos.x, pos.y, size.width, size.height))
}

/// 光标（物理坐标）是否在窗口矩形内。独立成纯函数是为了能单测边界那一圈。
fn point_in_rect(cursor: (f64, f64), rect: (i32, i32, u32, u32)) -> bool {
    let (cx, cy) = cursor;
    let (x, y) = (rect.0 as f64, rect.1 as f64);
    let (w, h) = (rect.2 as f64, rect.3 as f64);
    cx >= x && cy >= y && cx < x + w && cy < y + h
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

/// 在屏幕右下角弹一个短暂的气泡（常驻 3 秒后自动消失）。
fn show_toast(app: &tauri::AppHandle, name: &str, trigger: &str) {
    let payload = ToastPayload {
        name: name.to_string(),
        trigger: trigger.to_string(),
        subtitle: None,
    };
    show_toast_payload(app, payload);
}

/// 弹一条自定义副标题的气泡（不含「正在执行…」，用于「发现新版本」这类通知）。
fn show_toast_note(app: &tauri::AppHandle, title: &str, subtitle: &str) {
    let payload = ToastPayload {
        name: title.to_string(),
        trigger: String::new(),
        subtitle: Some(subtitle.to_string()),
    };
    show_toast_payload(app, payload);
}

fn show_toast_payload(app: &tauri::AppHandle, payload: ToastPayload) {
    // 先把载荷写进状态：toast 窗口若是首次懒创建，前端加载后据此兜底渲染，
    // 避免「事件早于监听器注册」导致第一次触发无内容。
    app.state::<KadaState>()
        .toast
        .lock()
        .unwrap()
        .replace(payload.clone());
    let Some(w) = ensure_toast(app) else { return };
    let _ = w.emit("toast-show", payload);
    let _ = w.show();
    let serial = TOAST_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(3000));
        if TOAST_SERIAL.load(Ordering::Relaxed) == serial {
            if let Some(w) = app.get_webview_window("toast") {
                let _ = w.hide();
            }
        }
    });
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
    match tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
        .title("咔哒 Kada")
        .inner_size(860.0, 560.0)
        // 下限 = 导航栏 64 + 列表栏 300 + 详情区最小可用 540。再窄详情区会挤爆
        // 「模式下拉 + 按键胶囊 + 三个录入图标」那一行（CSS 里没有断点可退）。
        .min_inner_size(900.0, 480.0)
        .resizable(true)
        .center()
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

/// 消息中心保留的最大条数（超出丢最旧的）。命令输出是内存态、重启即清，但常驻数周不设
/// 上限会把每条命令的全量输出都留在内存里。热串缓冲早有 64 上限，此处对齐。
const MAX_RESULTS: usize = 200;
/// 单条 stdout / stderr 保留的最大字节数（超出截断并加提示）。避免一条 `npm install`
/// 之类的大输出把整份消息中心撑大。
const MAX_RESULT_TEXT: usize = 16 * 1024;

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

/// 记录一条命令结果：写入消息中心；弹窗开则唤起主窗口，否则亮未读红点。
/// 两种情况都会向主窗口发 `command-result` 事件。
fn commit_result(
    app: &tauri::AppHandle,
    results: &Mutex<Vec<CommandResult>>,
    unread: &AtomicBool,
    mut result: CommandResult,
) {
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
        show_main_window(app);
    } else {
        unread.store(true, Ordering::Relaxed);
        set_tray_unread(app, true);
    }
    let _ = app.emit("command-result", &result);
}

/// 切换托盘图标的红点（未读态带红点，已读态还原基础图标）。
fn set_tray_unread(app: &tauri::AppHandle, unread: bool) {
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

// 动作执行引擎（run_action / run_os / run_app / run_cmd / transform_case 等）
// 已迁出到 `kada-actions` crate；壳层通过 `kada_actions::run_actions` 调用。

/// 在图标右上角叠加一个红色圆点（未读标记），返回新图像。
fn with_red_dot(icon: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let w = icon.width() as usize;
    let h = icon.height() as usize;
    let mut rgba = icon.rgba().to_vec();
    let r = ((w.min(h) as f32) * 0.20).max(1.0) as usize;
    let cx = w.saturating_sub(r + 1);
    let cy = r + 1;
    for y in 0..h {
        for x in 0..w {
            let dx = x as isize - cx as isize;
            let dy = y as isize - cy as isize;
            if dx * dx + dy * dy <= (r as isize) * (r as isize) {
                let i = (y * w + x) * 4;
                rgba[i] = 0xe0; // R
                rgba[i + 1] = 0x50; // G
                rgba[i + 2] = 0x40; // B
                rgba[i + 3] = 0xff; // A
            }
        }
    }
    tauri::image::Image::new_owned(rgba, icon.width(), icon.height())
}

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
mod tests {
    use super::*;

    /// `ui/src/main.ts` 的键表是 `kada-core` 的手抄副本（改键下拉 / 组合键录入显示名），
    /// 一旦 Rust 新增 `Key` 而前端漏了，下拉会静默空白、保存时可能写空值（见规划 7.4-㉘）。
    /// 这里按字面量解析前端数组 / 对象，跟 [`Key::ALL`]（与 `key_name` 同源生成）逐一比对。
    ///
    /// 前端这两处必须是**显式字面量**（不能 spread / 循环拼），否则解析不到——这是有意的约束。
    #[test]
    fn frontend_key_table_matches_core() {
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/main.ts"))
                .expect("读取 ui/src/main.ts 失败");
        let names: std::collections::BTreeSet<&str> =
            Key::ALL.iter().map(|k| key_name(*k)).collect();

        // ① 改键下拉的全键表：必须与 core 完全一致（不多不少）。
        let options = quoted_literals(region(&src, "const KEY_OPTIONS = ["));
        let options: std::collections::BTreeSet<&str> =
            options.iter().map(String::as_str).collect();
        let missing: Vec<&&str> = names.difference(&options).collect();
        let extra: Vec<&&str> = options.difference(&names).collect();
        assert!(
            missing.is_empty(),
            "KEY_OPTIONS 缺少 core 里的键：{missing:?}"
        );
        assert!(
            extra.is_empty(),
            "KEY_OPTIONS 有 core 里没有的键（拼写错 / 已删除）：{extra:?}"
        );

        // ② 录入时 KeyboardEvent.code → 键名的映射：每个显示名都必须是合法键名。
        for table in ["const CODE_TABLE", "const CODE_PUNCT"] {
            for lit in quoted_literals(region(&src, table)) {
                assert!(
                    names.contains(lit.as_str()),
                    "{table} 里的「{lit}」不是合法键名"
                );
            }
        }
    }

    /// 卡带市场（7.2-㉕）：索引 / 卡带的地址只收 http(s)。设置里那个字符串不该变成读本地
    /// 文件的口子（`file://`、`\\?\C:\...` 之类一律拒在下载之前）。
    #[test]
    fn snippet_download_only_accepts_http_urls() {
        for bad in [
            "file:///C:/Windows/System32/config/SAM",
            r"\\?\C:\secrets.json",
            "ftp://example.com/index.json",
            "",
        ] {
            let err = tauri::async_runtime::block_on(http_get_text(bad)).unwrap_err();
            assert!(err.contains("不是 http"), "{bad} 应被拒：{err}");
        }
    }

    /// 卡带市场（7.2-㉕）端到端：真起一个本地 HTTP 服务，把仓库里的 `snippets/` 当托管端，
    /// 走一遍「下载 → 解析索引 → 下载卡带 → 清洗 → 预览」——顺带钉住「仓库里那几份卡带
    /// 文件本身就是合法的 `Config`」（字段名写错的话 serde 会静默忽略，装出来是空壳）。
    #[test]
    fn snippet_market_serves_repo_snippets_over_http() {
        let dir = format!("{}/../snippets", env!("CARGO_MANIFEST_DIR"));
        let (base, stop) = serve_dir(&dir);

        let index = tauri::async_runtime::block_on(http_get_text(&format!("{base}/index.json"))).unwrap();
        let entries = parse_snippet_index(&index).unwrap();
        assert!(!entries.is_empty(), "仓库里的索引没有条目");

        // 索引里的 url 指向 GitHub raw（上线才是那个地址），这里换成同一个本地服务：逐条下载，
        // 钉住「仓库里每份卡带都是合法且非空的 `Config`」（字段名写错 serde 会静默忽略，
        // 装出来是个空壳，而用户只会觉得「卡带市场是坏的」）。
        for e in &entries {
            let file = e.url.rsplit('/').next().unwrap();
            let got = tauri::async_runtime::block_on(fetch_snippet(format!("{base}/{file}")))
                .unwrap_or_else(|err| panic!("卡带 {file} 下载/解析失败：{err}"));
            let n = got.preview.shortcuts.len() + got.preview.remaps.len() + got.preview.expansions.len();
            assert!(n > 0, "卡带 {file} 清洗后一条条目都不剩");
        }

        let fetched =
            tauri::async_runtime::block_on(fetch_snippet(format!("{base}/caps-esc-ctrl.json"))).unwrap();
        let remap = &fetched.preview.remaps[0];
        assert_eq!(remap.from, "CapsLock");
        assert_eq!(remap.tap.as_deref(), Some("Escape"));
        assert_eq!(remap.hold.as_deref(), Some("Ctrl"));
        assert!(fetched.preview.shortcuts.is_empty());

        // 带层的卡带：被引用的层必须一起带过来（否则装进去层引用被清成基础层 = 静默改行为）。
        let vim = tauri::async_runtime::block_on(fetch_snippet(format!("{base}/vim-keys.json"))).unwrap();
        assert_eq!(vim.preview.remaps.len(), 6);
        assert!(fetched.text.len() > 0 && !vim.text.is_empty());
        stop.store(true, Ordering::Relaxed);
    }

    /// 单线程的「静态文件服务」：回一个请求就够测试用（每条连接单独 spawn）。
    fn serve_dir(dir: &str) -> (String, Arc<AtomicBool>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("端口绑定失败");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let dir = dir.to_string();
        let flag = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if flag.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut s) = stream else { continue };
                use std::io::{BufRead, BufReader, Write};
                let mut line = String::new();
                BufReader::new(&s).read_line(&mut line).unwrap_or_default();
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = std::fs::read(format!("{dir}{path}")).unwrap_or_default();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body));
            }
        });
        (base, stop)
    }

    /// 取 `marker` 起到其后第一个 `];` / `};` 之间的源码片段。
    fn region<'a>(src: &'a str, marker: &str) -> &'a str {
        let start = src
            .find(marker)
            .unwrap_or_else(|| panic!("找不到标记：{marker}"));
        let rest = &src[start..];
        let end = ["\n];", "\n};"]
            .iter()
            .filter_map(|m| rest.find(m))
            .min()
            .unwrap_or_else(|| panic!("找不到结束标记：{marker}"));
        &rest[..end]
    }

    /// 抽出片段里所有双引号字面量的内容（处理 `\\` 与 `\"` 两种转义）。
    fn quoted_literals(s: &str) -> Vec<String> {
        let bytes = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'"' {
                let (mut j, mut val) = (i + 1, String::new());
                while j < bytes.len() && bytes[j] != b'"' {
                    if bytes[j] == b'\\' && j + 1 < bytes.len() {
                        j += 1;
                        val.push(bytes[j] as char);
                    } else {
                        val.push(bytes[j] as char);
                    }
                    j += 1;
                }
                out.push(val);
                i = j + 1;
            } else {
                i += 1;
            }
        }
        out
    }

    #[test]
    fn external_link_filter_accepts_only_http() {
        // 界面外链（帮助页 GitHub）只把 http(s) 丢给系统浏览器，其余新窗口请求一律不理。
        assert_eq!(
            external_link_to_open("https://github.com/chillyice/Kada"),
            Some("https://github.com/chillyice/Kada")
        );
        assert_eq!(
            external_link_to_open("http://example.com/x?y=1&z=2"),
            Some("http://example.com/x?y=1&z=2")
        );
        for other in [
            "about:blank",
            "data:text/html,hi",
            "file:///C:/",
            "javascript:alert(1)",
            "tauri://localhost/index.html",
        ] {
            assert_eq!(external_link_to_open(other), None, "{other}");
        }
    }

    #[test]
    fn injects_keys_only_matches_key_injecting_actions() {
        // 只有 Text / Keys 会往目标程序注入按键：修饰键没松时等的是它们（见 fire）。
        let text = vec![Action::Text {
            text: "x".into(),
            mode: kada_core::TextMode::Input,
            masked: false,
            description: None,
        }];
        let keys = vec![Action::Keys {
            keys: vec!["Ctrl".into(), "C".into()],
            description: None,
        }];
        assert!(injects_keys(&text));
        assert!(injects_keys(&keys));

        // 鼠标模拟也注入输入：物理修饰键没松时同样会变成 Ctrl+点击 / 拖拽，等它释放。
        let mouse = vec![Action::Mouse {
            op: kada_core::MouseOp::Click {
                button: kada_core::MouseButton::Left,
            },
            description: None,
        }];
        assert!(injects_keys(&mouse));

        // 命令 / 暂停不受修饰键影响，不该白等 300ms。
        let plain = vec![
            Action::PauseMs {
                ms: 10,
                description: None,
            },
            #[cfg(feature = "automation")]
            Action::OpenUrl {
                url: "https://kada.test".into(),
                description: None,
            },
        ];
        assert!(!injects_keys(&plain));

        // If 分支里的注入动作也算数（两路并查）。
        #[cfg(feature = "automation")]
        {
            let branched = vec![Action::If {
                condition: kada_core::Condition::Exists {
                    path: "C:\\".into(),
                },
                then: vec![],
                otherwise: keys,
                description: None,
            }];
            assert!(injects_keys(&branched));
            let empty_branch = vec![Action::If {
                condition: kada_core::Condition::Exists {
                    path: "C:\\".into(),
                },
                then: vec![],
                otherwise: vec![],
                description: None,
            }];
            assert!(!injects_keys(&empty_branch));
        }

        // 切换虚拟桌面也注入组合键，同样要等物理修饰键松开。
        #[cfg(feature = "automation")]
        {
            let desktop = vec![Action::System {
                operation: kada_core::SystemOperation::SwitchDesktop {
                    direction: kada_core::DesktopDirection::Next,
                },
                description: None,
            }];
            assert!(injects_keys(&desktop));

            // 文件类操作不注入，别为它们白等 300ms。
            let file = vec![Action::Os {
                operation: kada_core::OsOperation::Delete {
                    path: "C:\\tmp\\a.txt".into(),
                },
                description: None,
            }];
            assert!(!injects_keys(&file));
        }
    }

    /// 假注入器：只记录发出的键，绝不真的注入（会打到跑测试的这台机器上）。
    #[derive(Default)]
    struct TestInject {
        log: Vec<String>,
    }

    impl engine::Inject for TestInject {
        fn down(&mut self, key: Key) {
            self.log.push(format!("down {}", key_name(key)));
        }
        fn up(&mut self, key: Key) {
            self.log.push(format!("up {}", key_name(key)));
        }
    }

    fn down(key: Key) -> Ev {
        Ev::Down {
            key,
            mods: BTreeSet::new(),
            repeat: false,
        }
    }

    fn up(key: Key) -> Ev {
        Ev::Up { key }
    }

    /// 走一遍钩子回调的决策链（与 `run()` 里的接线一致）：`engine.step` → `decide`。
    fn decide_via_engine(
        engine: &mut Engine,
        inj: &mut TestInject,
        cfg: &Config,
        ev: &Ev,
    ) -> Option<Outcome> {
        let mut fire = |_a: Vec<Action>, _t: String, _n: String| {};
        let ev = engine.step(ev, cfg, &TriggerContext::empty(), inj, &mut fire)?;
        Some(decide(
            &ev,
            cfg,
            engine.active_layer(),
            &TriggerContext::empty(),
        ))
    }

    /// 同 [`decide_via_engine`]，但把触发的触发键收集下来（和弦/序列在 `step` 里就 fire 了）。
    fn step_collecting_fire(
        engine: &mut Engine,
        inj: &mut TestInject,
        cfg: &Config,
        ev: &Ev,
        fired: &mut Vec<String>,
    ) -> Option<Ev> {
        let mut fire = |_a: Vec<Action>, t: String, _n: String| fired.push(t);
        engine.step(ev, cfg, &TriggerContext::empty(), inj, &mut fire)
    }

    /// 「长按进入层」的切层键（CapsLock → L1）+ 该层内一条快捷键（触发键由参数给出）。
    fn cfg_with_layer(trigger: &str) -> Config {
        Config {
            layers: vec![kada_core::Layer {
                id: "L1".into(),
                name: "层1".into(),
                ..Default::default()
            }],
            remaps: vec![kada_core::Remap {
                from: "CapsLock".into(),
                hold_layer: Some("L1".into()),
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![kada_core::ShortcutItem {
                layer: Some("L1".into()),
                triggers: vec![trigger.into()],
                actions: vec![Action::Text {
                    text: "x".into(),
                    mode: kada_core::TextMode::Input,
                    masked: false,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn layer_key_short_press_replays_original_key() {
        // 只设了「长按进入层」、没设短按输出：短按必须回放原键，否则这个键就变哑了
        //（用户按一下 CapsLock 什么也不发生）。
        let cfg = cfg_with_layer("K");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(
            inj.log,
            vec!["down CapsLock", "up CapsLock"],
            "短按回放原键"
        );
        assert_eq!(engine.active_layer(), None, "短按不进层");
    }

    #[test]
    fn layer_key_long_press_without_other_key_replays_original_key() {
        // 「一直按住切层键、期间没按别的键就松开」（层从没真正进过）：整个按键必须回放原键，
        // 否则这个键只是按久了一点就彻底没反应——用户看到的正是「长按切层键没反应」。
        let mut cfg = cfg_with_layer("K");
        cfg.remaps[0].tap_timeout_ms = 20;
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        std::thread::sleep(std::time::Duration::from_millis(30));
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(
            inj.log,
            vec!["down CapsLock", "up CapsLock"],
            "长按无输出时回放原键"
        );
        assert_eq!(engine.active_layer(), None, "没按别的键就不进层");
    }

    #[test]
    fn user_real_config_layer_key_tab_with_chord() {
        // 复刻用户真实配置（2026-09-24 14:25）：`Tab` 长按进「和弦层」，层内一条快捷键带
        // `5&Y`（和弦）与 `Alt+Z`（普通组合）。事件流按真人操作补齐：Tab 按住（含自动重复）
        // → 5 → Y → 松开。和弦与组合都应在按住切层键期间命中。
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer {
                id: LID.into(),
                name: "和弦层".into(),
                ..Default::default()
            }],
            remaps: vec![kada_core::Remap {
                from: "Tab".into(),
                to: String::new(),
                hold_layer: Some(LID.into()),
                tap_timeout_ms: 200,
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![kada_core::ShortcutItem {
                layer: Some(LID.into()),
                triggers: vec!["5&Y".into(), "Alt+Z".into()],
                actions: vec![Action::Text {
                    text: "pw".into(),
                    mode: kada_core::TextMode::Input,
                    masked: false,
                    description: None,
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };

        // ① 和弦：按住 Tab（含自动重复）→ 5 → Y
        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        for _ in 0..5 {
            let repeat = Ev::Down {
                key: Key::Tab,
                mods: BTreeSet::new(),
                repeat: true,
            };
            step_collecting_fire(&mut engine, &mut inj, &cfg, &repeat, &mut fired);
        }
        assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        assert_eq!(engine.active_layer(), Some(LID), "按住 Tab + 按 5 → 进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

        // ② 普通组合：松开和弦成员后，按住 Tab 再按 Alt+Z（真人按 Alt 时事件带 mods={Alt}）
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
        fired.clear();
        let alt_down = Ev::Down {
            key: Key::Alt,
            mods: BTreeSet::from([Modifier::Alt]),
            repeat: false,
        };
        step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_down, &mut fired);
        let z_down = Ev::Down {
            key: Key::Z,
            mods: BTreeSet::from([Modifier::Alt]),
            repeat: false,
        };
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &z_down, &mut fired);
        let layer = engine.active_layer().map(str::to_string);
        match decide(
            &out.expect("Alt+Z 应放行进 decide"),
            &cfg,
            layer.as_deref(),
            &TriggerContext::empty(),
        ) {
            Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "Alt+Z"),
            other => panic!(
                "层内 Alt+Z 应命中该层快捷键，实际 {:?}",
                matches!(other, Outcome::Pass)
            ),
        }

        // ③ 层内序列：按住 Tab 期间依次按 F9 J K 应触发
        let mut cfg2 = cfg.clone();
        cfg2.shortcuts[0].triggers = vec!["F9 J K".into()];
        let mut engine2 = Engine::new();
        let mut inj2 = TestInject::default();
        let mut fired2: Vec<String> = Vec::new();
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::Tab), &mut fired2);
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::F9), &mut fired2);
        assert_eq!(engine2.active_layer(), Some(LID), "按住 Tab + 按 F9 → 进层");
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::J), &mut fired2);
        step_collecting_fire(&mut engine2, &mut inj2, &cfg2, &down(Key::K), &mut fired2);
        assert_eq!(fired2, vec!["F9 J K"], "层内序列应触发");
    }

    #[test]
    fn layer_key_released_before_chord_does_not_fire() {
        // 层是 momentary 的（hold_layer）：松开切层键后层立即失效，此时按层内和弦不触发（这是
        // 设计，不是 bug）。「长按切层、松开、再按层内和弦/序列」的用法要用切换式切层
        //（lock_layer，见 locked_layer_* 用例）。
        let cfg = cfg_with_layer("5&Y");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();

        step_collecting_fire(
            &mut engine,
            &mut inj,
            &cfg,
            &down(Key::CapsLock),
            &mut fired,
        );
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::CapsLock), &mut fired);
        assert_eq!(engine.active_layer(), None, "松开切层键即退层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert!(
            fired.is_empty(),
            "层未激活时层内和弦不触发，实际：{fired:?}"
        );
    }

    #[test]
    fn locked_layer_key_long_press_then_chord_sequence_combo() {
        // 复刻用户真实配置（2026-09-24）：`Tab` 改成「长按锁定层」（切换式）指向「和弦层」，
        // 层内一条快捷键带 `5&Y`（和弦）+ `F9 J K`（序列）+ `Alt+Z`（普通组合），基础层也有一条
        // `Alt+Z`。用户诉求：**长按切层键之后（松手）** 层内的和弦与序列要能触发——momentary
        // 做不到（层随松手消失，层内和弦还得一直按住切层键），切换式才用得起来。
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer {
                id: LID.into(),
                name: "和弦层".into(),
                ..Default::default()
            }],
            remaps: vec![kada_core::Remap {
                from: "Tab".into(),
                to: String::new(),
                lock_layer: Some(LID.into()),
                tap_timeout_ms: 20, // 真实配置是 200；测试里压短，免得真等
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![
                kada_core::ShortcutItem {
                    name: Some("基础层 Alt+Z".into()),
                    triggers: vec!["Alt+Z".into()],
                    actions: vec![],
                    enabled: true,
                    ..Default::default()
                },
                kada_core::ShortcutItem {
                    name: Some("输入密码".into()),
                    layer: Some(LID.into()),
                    triggers: vec!["5&Y".into(), "F9 J K".into(), "Alt+Z".into()],
                    actions: vec![Action::Text {
                        text: "pw".into(),
                        mode: kada_core::TextMode::Input,
                        masked: false,
                        description: None,
                    }],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();

        // ① 长按 Tab（期间没按别的键）再松开 → 切进「和弦层」并保持生效。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        std::thread::sleep(std::time::Duration::from_millis(30));
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
        assert_eq!(engine.active_layer(), Some(LID), "长按切层后层保持生效");
        assert!(inj.log.is_empty(), "切层键不回放");

        // ② 层内和弦：松手后直接按 5 与 Y。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(fired, vec!["5&Y"], "长按切层后层内和弦应触发");

        // ③ 层内序列：F9 J K 依次按下。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Y), &mut fired);
        fired.clear();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::F9), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::J), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::K), &mut fired);
        assert_eq!(fired, vec!["F9 J K"], "长按切层后层内序列应触发");

        // ④ 层内普通组合：层条目优先于基础层同名条目。
        fired.clear();
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::F9), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::J), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::K), &mut fired);
        let alt_z = Ev::Down {
            key: Key::Z,
            mods: BTreeSet::from([Modifier::Alt]),
            repeat: false,
        };
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
        let layer = engine.active_layer().map(str::to_string);
        match decide(
            &out.expect("Alt+Z 应放行进 decide"),
            &cfg,
            layer.as_deref(),
            &TriggerContext::empty(),
        ) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "输入密码", "层内 Alt+Z 应优先"),
            Outcome::Replace(_) => panic!("不该走改键"),
            Outcome::Pass => panic!("层内 Alt+Z 未命中"),
        }

        // ⑤ 再长按一次切层键 → 退出锁定层，Alt+Z 回到基础层条目。
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Tab), &mut fired);
        std::thread::sleep(std::time::Duration::from_millis(30));
        step_collecting_fire(&mut engine, &mut inj, &cfg, &up(Key::Tab), &mut fired);
        assert_eq!(engine.active_layer(), None, "再长按一次退出切层");
        let out = step_collecting_fire(&mut engine, &mut inj, &cfg, &alt_z, &mut fired);
        match decide(
            &out.expect("Alt+Z 应放行进 decide"),
            &cfg,
            None,
            &TriggerContext::empty(),
        ) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "基础层 Alt+Z"),
            Outcome::Replace(_) => panic!("不该走改键"),
            Outcome::Pass => panic!("基础层 Alt+Z 未命中"),
        }
    }

    #[test]
    fn layer_key_long_press_activates_layer_shortcut() {
        // 长按切层键期间按下 K：K 应按「激活层优先」命中层内快捷键（而不是基础层兜底）。
        let cfg = cfg_with_layer("K");
        let mut engine = Engine::new();
        let mut inj = TestInject::default();

        assert!(decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::CapsLock)).is_none());
        assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
        let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
        assert_eq!(
            engine.active_layer(),
            Some("L1"),
            "按住切层键 + 按 K → 进层"
        );
        match out {
            Outcome::Shortcut { trigger, .. } => assert_eq!(trigger, "K"),
            _ => panic!("层内 K 应命中该层快捷键"),
        }

        // 松开切层键退回基础层，K 不再命中层条目。
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::K));
        decide_via_engine(&mut engine, &mut inj, &cfg, &up(Key::CapsLock));
        assert_eq!(engine.active_layer(), None, "松开切层键退回基础层");
        let out = decide_via_engine(&mut engine, &mut inj, &cfg, &down(Key::K)).expect("K 应放行");
        assert!(
            matches!(out, Outcome::Pass),
            "基础层下 K 不该命中层内快捷键"
        );
    }

    #[test]
    fn layer_chord_fires_while_layer_key_held() {
        // 复刻实际配法：层「和弦层」里放一条和弦快捷键（5&Y），用 CapsLock 长按进层
        //（CapsLock 只设了「长按进入层」）。按住切层键 + 同时按 5 与 Y → 该层和弦应触发。
        let cfg = cfg_with_layer("5&Y");
        let mut fired: Vec<String> = Vec::new();
        let mut inj = TestInject::default();
        let mut engine = Engine::new();

        step_collecting_fire(
            &mut engine,
            &mut inj,
            &cfg,
            &down(Key::CapsLock),
            &mut fired,
        );
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Digit5), &mut fired);
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(engine.active_layer(), Some("L1"), "按住切层键即进层");
        assert_eq!(fired, vec!["5&Y"], "层内和弦应触发");

        // 不进层时同一个和弦不该触发（层语义：条目只在该层激活时生效）。
        let mut fired2: Vec<String> = Vec::new();
        let mut inj2 = TestInject::default();
        let mut engine2 = Engine::new();
        step_collecting_fire(
            &mut engine2,
            &mut inj2,
            &cfg,
            &down(Key::Digit5),
            &mut fired2,
        );
        step_collecting_fire(&mut engine2, &mut inj2, &cfg, &down(Key::Y), &mut fired2);
        assert!(
            fired2.is_empty(),
            "基础层下不应命中层内和弦，实际：{fired2:?}"
        );
    }

    #[cfg(feature = "automation")]
    #[test]
    fn when_gate_routes_same_shortcut_by_frontmost() {
        use kada_core::{Condition, FrontmostContext, TriggerContext};
        let mk = |name: &str, when: Option<Condition>| kada_core::ShortcutItem {
            name: Some(name.into()),
            triggers: vec!["Ctrl+K".into()],
            actions: vec![],
            when,
            enabled: true,
            ..Default::default()
        };
        // 同触发键 Ctrl+K：一条只在 Chrome 生效，另一条无条件兜底。
        let cfg = Config {
            shortcuts: vec![
                mk(
                    "Chrome",
                    Some(Condition::FrontmostApp {
                        app: "chrome".into(),
                    }),
                ),
                mk("兜底", None),
            ],
            ..Default::default()
        };
        let ev = Ev::Down {
            key: Key::K,
            mods: BTreeSet::from([Modifier::Ctrl]),
            repeat: false,
        };

        let chrome = FrontmostContext {
            process_name: "chrome.exe".into(),
            window_title: String::new(),
        };
        match decide(&ev, &cfg, None, &TriggerContext::fixed(Some(&chrome), None)) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "Chrome"),
            other => panic!(
                "应命中 Chrome 条目，实际 Pass={}",
                matches!(other, Outcome::Pass)
            ),
        }

        // 别的前台：条件条目跳过，落到兜底条目。
        let code = FrontmostContext {
            process_name: "Code.exe".into(),
            window_title: String::new(),
        };
        match decide(&ev, &cfg, None, &TriggerContext::fixed(Some(&code), None)) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "兜底"),
            other => panic!(
                "应落到兜底条目，实际 Pass={}",
                matches!(other, Outcome::Pass)
            ),
        }

        // 无前台上下文（Linux）：条件条目恒不成立，仍落到兜底。
        match decide(&ev, &cfg, None, &TriggerContext::empty()) {
            Outcome::Shortcut { name, .. } => assert_eq!(name, "兜底"),
            _ => panic!("无前台下应落到兜底条目"),
        }
    }

    #[test]
    fn user_real_config_backtick_layer_chord_u_amp_y() {
        // 复刻用户 2026-09-30 真实配置：` (Backquote) 长按进「和弦层」，层内条目带
        // "U&Y"（和弦）+ "Alt+Z"；基础层另有一条 Alt+Z。事件流按真人操作：` 按住（含
        // 自动重复）→ U → Y。
        const LID: &str = "20d08484-41a4-48ab-90cf-bb8633f779df";
        let cfg = Config {
            layers: vec![kada_core::Layer {
                id: LID.into(),
                name: "和弦层".into(),
                ..Default::default()
            }],
            remaps: vec![kada_core::Remap {
                from: "`".into(),
                to: String::new(),
                hold_layer: Some(LID.into()),
                tap_timeout_ms: 200,
                enabled: true,
                ..Default::default()
            }],
            shortcuts: vec![
                kada_core::ShortcutItem {
                    name: Some("生成zcode".into()),
                    triggers: vec!["Alt+Z".into()],
                    actions: vec![],
                    enabled: true,
                    ..Default::default()
                },
                kada_core::ShortcutItem {
                    name: Some("输入密码".into()),
                    layer: Some(LID.into()),
                    triggers: vec!["Alt+Z".into(), "U&Y".into()],
                    actions: vec![],
                    enabled: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut engine = Engine::new();
        let mut inj = TestInject::default();
        let mut fired: Vec<String> = Vec::new();

        step_collecting_fire(
            &mut engine,
            &mut inj,
            &cfg,
            &down(Key::Backquote),
            &mut fired,
        );
        // 按住 ` 期间系统会发自动重复的 Down。
        for _ in 0..5 {
            let repeat = Ev::Down {
                key: Key::Backquote,
                mods: BTreeSet::new(),
                repeat: true,
            };
            step_collecting_fire(&mut engine, &mut inj, &cfg, &repeat, &mut fired);
        }
        assert_eq!(engine.active_layer(), None, "还没按别的键，未进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::U), &mut fired);
        assert_eq!(engine.active_layer(), Some(LID), "按住 ` + 按 U → 进层");
        step_collecting_fire(&mut engine, &mut inj, &cfg, &down(Key::Y), &mut fired);
        assert_eq!(fired, vec!["U&Y"], "按住切层键时 U&Y 应触发");
    }

    #[test]
    fn recorder_transcribes_events() {
        let down = |key: Key| Ev::Down {
            key,
            mods: BTreeSet::new(),
            repeat: false,
        };
        let up = |key: Key| Ev::Up { key };

        let mut r = Recorder::new();
        r.push(&down(Key::K));
        r.push(&up(Key::K));
        r.push(&down(Key::Control));
        r.push(&down(Key::C));
        r.push(&up(Key::C));
        r.push(&up(Key::Control));
        let actions = r.finish();

        // K 单键成一条组合键，Ctrl+C 合并成一条组合键（间隔若 ≥20ms 会有 PauseMs，忽略之）
        let combos: Vec<&Vec<String>> = actions
            .iter()
            .filter_map(|a| match a {
                Action::Keys { keys, .. } => Some(keys),
                _ => None,
            })
            .collect();
        assert!(combos.iter().any(|k| *k == &vec!["K".to_string()]));
        assert!(combos
            .iter()
            .any(|k| *k == &vec!["Ctrl".to_string(), "C".to_string()]));

        // 重复 down 折叠：按住 Ctrl 只产生一次组合键
        let mut r = Recorder::new();
        r.push(&down(Key::Control));
        r.push(&Ev::Down {
            key: Key::Control,
            mods: BTreeSet::new(),
            repeat: true,
        });
        r.push(&up(Key::Control));
        let ctrl = r
            .finish()
            .iter()
            .filter(|a| matches!(a, Action::Keys { keys, .. } if keys == &vec!["Ctrl".to_string()]))
            .count();
        assert_eq!(ctrl, 1, "重复 down 必须折叠");
    }

    #[test]
    fn mouse_recorder_transcribes_events() {
        use kada_hook::{MouseRecord, MouseRecordKind};

        let ev = |kind: MouseRecordKind, move_dx: i32, move_dy: i32| MouseRecord {
            kind,
            move_dx,
            move_dy,
        };
        let mut r = MouseRecorder::default();
        r.push(&ev(
            MouseRecordKind::Click(kada_core::MouseButton::Left),
            120,
            -40,
        ));
        r.push(&ev(MouseRecordKind::Scroll { dx: 0, dy: -3 }, 0, 0));
        r.push(&ev(
            MouseRecordKind::Click(kada_core::MouseButton::Back),
            0,
            60,
        ));

        let ops: Vec<kada_core::MouseOp> = r
            .into_actions()
            .into_iter()
            .map(|a| match a {
                Action::Mouse { op, .. } => op,
                other => panic!("鼠标录制只应产出 Action::Mouse，得到 {other:?}"),
            })
            .collect();

        // 位移非 0 先补 Move；若位移到下一个触发之间没挪动则不补（滚轮那条）。
        assert_eq!(
            ops,
            vec![
                kada_core::MouseOp::Move { dx: 120, dy: -40 },
                kada_core::MouseOp::Click {
                    button: kada_core::MouseButton::Left
                },
                kada_core::MouseOp::Scroll { dx: 0, dy: -3 },
                kada_core::MouseOp::Move { dx: 0, dy: 60 },
                kada_core::MouseOp::Click {
                    button: kada_core::MouseButton::Back
                },
            ]
        );
    }

    /// 只有「快速唤醒键」一项不同的配置。
    fn wake_cfg(wake_key: Option<&str>) -> RwLock<Config> {
        let mut cfg = Config::default();
        cfg.settings.wake_key = wake_key.map(str::to_string);
        RwLock::new(cfg)
    }

    #[test]
    fn wake_double_tap_needs_two_taps_in_window() {
        // 判定已与「建窗口」解耦（见 wake_double_tap 的注释），所以这层时序规则能单测。
        let cfg = wake_cfg(Some("Alt"));
        let mut last = None;
        assert!(
            !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "第一击只是单击"
        );
        assert!(
            wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "窗口内的第二击命中"
        );
        assert!(
            !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "计完归零，第三击重新算单击"
        );
        assert!(
            wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "第四击又凑成一次双击"
        );
    }

    #[test]
    fn wake_double_tap_window_expires_and_other_keys_reset() {
        let cfg = wake_cfg(Some("Alt"));

        // 上一次单击已落在判定窗口之外：这一击只算新的单击，不该唤窗。
        let mut last = Some(Instant::now() - Duration::from_millis(WAKE_DOUBLE_TAP_MS + 50));
        assert!(
            !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "超出窗口不算双击"
        );
        assert!(last.is_some(), "超时后要重新计时（这一击仍是单击）");

        // 中间按了别的键 → 上一次单击作废（那次多半是 Alt+Tab 这类组合的一部分）。
        let mut last = None;
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&cfg, &mut last, &down(Key::K)));
        assert!(last.is_none(), "按其它键要清掉单击计数");
        assert!(
            !wake_double_tap(&cfg, &mut last, &down(Key::Alt)),
            "清空后这一击是新单击"
        );

        // 自动重复的按下不是独立一击：长按唤醒键不该被当成连击而唤窗。
        let mut last = None;
        let repeated = Ev::Down {
            key: Key::Alt,
            mods: BTreeSet::new(),
            repeat: true,
        };
        assert!(
            !wake_double_tap(&cfg, &mut last, &repeated),
            "自动重复不算一击"
        );
        assert!(last.is_none(), "自动重复也不该开始计时");
    }

    #[test]
    fn wake_disabled_or_unparsable_key_never_wakes() {
        // 未设唤醒键 = 快速唤醒关闭：按多少次都不唤窗。
        let off = wake_cfg(None);
        let mut last = None;
        assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&off, &mut last, &down(Key::Alt)));

        // 配置里是解析不了的键名：当作关闭，既不 panic 也不唤窗（手工改配置能改出这种值）。
        let bad = wake_cfg(Some("没这个键"));
        let mut last = None;
        assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
        assert!(!wake_double_tap(&bad, &mut last, &down(Key::Alt)));
    }

    // ---- 前台切换判定（输入状态复位的触发条件，见规划 7.2-④） ----

    #[test]
    fn focus_tracker_needs_two_sightings_of_new_window() {
        let mut t = FocusTracker::new(Some(1));
        assert!(!t.observe(Some(1)), "同一窗口不算切换");
        assert!(!t.observe(Some(2)), "刚看到的新窗口先记下，等下一轮确认");
        assert!(t.observe(Some(2)), "连续两轮都是它 → 切换成立");
        assert!(!t.observe(Some(2)), "确认之后不再重复报告");
    }

    #[test]
    fn focus_tracker_ignores_transient_window_flicker() {
        // 触发气泡 / 右键菜单这类瞬态窗口只出现一轮：不该被当成「用户换了窗口」，
        // 否则用户手上正凑的和弦会被白复位掉。
        let mut t = FocusTracker::new(Some(1));
        assert!(!t.observe(Some(9)), "瞬态窗口：只看到一轮");
        assert!(!t.observe(Some(1)), "又回到原窗口 → 什么都不发生");
        assert!(!t.observe(Some(1)));
        // 真的换了：连续两轮都停在 2。
        assert!(!t.observe(Some(2)));
        assert!(t.observe(Some(2)));
    }

    #[test]
    fn focus_tracker_treats_missing_handle_as_no_change() {
        // 锁屏 / 桌面切换瞬间会读不到前台窗口：不作数，也不该把候选位冲掉
        //（否则锁屏读空后回到原窗口会被误判成切换）。
        let mut t = FocusTracker::new(Some(1));
        assert!(!t.observe(None));
        assert!(!t.observe(None));
        assert!(!t.observe(Some(1)), "读空之后回到原窗口：没换过");
    }

    // ---- 输入状态指示载荷（托盘提示 / 悬浮指示，见规划 7.3-⑬） ----

    /// 层 + 三类修饰键 → 载荷：层名要换成显示名，修饰键要按形态分开、摘要直接可读。
    #[test]
    fn status_payload_maps_layer_name_and_modifier_kinds() {
        let cfg = Config {
            layers: vec![kada_core::Layer {
                id: "L1".into(),
                name: "游戏层".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let st = EngineStatus {
            layer: Some("L1".into()),
            layer_locked: true,
            hold: vec![Key::Control],
            sticky: vec![],
            oneshot: vec![Key::Alt],
        };
        let p = build_status(&st, &cfg);
        assert_eq!(p.layer.as_deref(), Some("游戏层"), "层 id 要换成显示名");
        assert!(p.locked);
        let kinds: Vec<(&str, &str)> = p.mods.iter().map(|m| (m.kind, m.key.as_str())).collect();
        assert_eq!(
            kinds,
            vec![("hold", "Ctrl"), ("oneshot", "Alt")],
            "形态不能混为一谈"
        );
        assert_eq!(p.summary, "层：游戏层（锁定） · 按住 Ctrl · 单次 Alt");
        assert!(!p.idle());
    }

    /// 层没起名字就显示 id；id 已不在配置里（刚被外部改动删掉）也照原样显示，不能变成空白。
    #[test]
    fn status_payload_falls_back_to_layer_id() {
        let cfg = Config::default();
        let st = EngineStatus {
            layer: Some("ghost".into()),
            ..Default::default()
        };
        let p = build_status(&st, &cfg);
        assert_eq!(p.layer.as_deref(), Some("ghost"));
        assert_eq!(p.summary, "层：ghost（按住）");
        assert!(!p.locked, "没标锁定就按「按住」说");
    }

    /// 非修饰键也能单次 / 粘滞 / 长按（规划 7.3-⑭）：指示要按它的键名如实显示，
    /// 而不是「只认修饰键」时那样整条消失。
    #[test]
    fn status_payload_shows_plain_keys() {
        let st = EngineStatus {
            hold: vec![Key::Enter],
            sticky: vec![Key::MouseBack],
            oneshot: vec![Key::Alt],
            ..Default::default()
        };
        let p = build_status(&st, &Config::default());
        let kinds: Vec<(&str, &str)> = p.mods.iter().map(|m| (m.kind, m.key.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                ("hold", "Enter"),
                ("sticky", "MouseBack"),
                ("oneshot", "Alt")
            ]
        );
        assert_eq!(p.summary, "按住 Enter · 粘滞 MouseBack · 单次 Alt");
    }

    /// 空状态：指示要判成 idle（据此隐藏悬浮窗、托盘提示回到应用名），且序列化里不带 `layer`。
    #[test]
    fn status_payload_is_idle_when_nothing_is_active() {
        let p = build_status(&EngineStatus::default(), &Config::default());
        assert!(p.idle());
        assert_eq!(p.summary, "");
        assert!(!serde_json::to_string(&p).unwrap().contains("layer"));
    }

    // ---- 提示框悬停让位（见 `spawn_hints_hover_watch`） ----

    /// 进框即让位、停留够久才恢复、出框后下次照旧让位（`Out ↔ Passing → Settled`）。
    #[test]
    fn hints_hover_yields_then_settles_and_rearms() {
        let rect = (100, 200, 300, 400);
        // 光标还没进框：可交互。
        assert!(!point_in_rect((90.0, 200.0), rect));
        assert!(!point_in_rect((400.0, 200.0), rect), "右边界算外面");
        assert!(!point_in_rect((100.0, 600.0), rect), "下边界算外面");
        assert!(point_in_rect((100.0, 200.0), rect), "左上角算里面");
        assert!(point_in_rect((399.9, 599.9), rect));

        // 进框：让位（穿透）。
        let cur = hints_hover_step(HintsHover::Out, true, 0);
        assert_eq!(cur, HintsHover::Passing);
        // 让位期间挪出去：回可交互。
        assert_eq!(hints_hover_step(cur, false, 500), HintsHover::Out);
        // 让位期间还没停够：不恢复。
        assert_eq!(
            hints_hover_step(HintsHover::Passing, true, HINTS_HOVER_DWELL_MS - 1),
            HintsHover::Passing
        );
        // 停够了：恢复可交互，且一直保持到挪出边界。
        let settled = hints_hover_step(HintsHover::Passing, true, HINTS_HOVER_DWELL_MS);
        assert_eq!(settled, HintsHover::Settled);
        assert_eq!(
            hints_hover_step(settled, true, HINTS_HOVER_DWELL_MS + 60_000),
            HintsHover::Settled
        );
        // 挪出去之后再进来：又让位（否则「一直悬着」就再也点不到框了）。
        assert_eq!(hints_hover_step(settled, false, 0), HintsHover::Out);
        assert_eq!(
            hints_hover_step(HintsHover::Out, true, 0),
            HintsHover::Passing
        );
    }

    /// 同步存档（规划 7.2-㉝）：`sync_file` 拼固定文件名、空白目录视为未配置；
    /// `push_sync` 把整份配置原子写到该目录（写出去的是能解析回来的同一份配置）。
    #[test]
    fn sync_archive_writes_config_to_dir() {
        let mut cfg = Config::default();
        assert!(sync_file(&cfg).is_none(), "未配置同步目录时不该有任何同步文件");
        cfg.settings.sync_dir = Some("   ".into());
        assert!(sync_file(&cfg).is_none(), "空白目录视为未配置");

        let dir = std::env::temp_dir().join(format!("kada_sync_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        cfg.settings.sync_dir = Some(dir.display().to_string());
        cfg.settings.action_timeout_ms = 1234;

        assert_eq!(sync_file(&cfg).unwrap(), dir.join(SYNC_FILE_NAME));
        push_sync(&cfg);

        let text = std::fs::read_to_string(dir.join(SYNC_FILE_NAME)).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back.settings.action_timeout_ms, 1234, "同步出去的是整份配置");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

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

/// 读取当前配置。
#[tauri::command]
fn get_config(state: tauri::State<'_, KadaState>) -> Config {
    state.config.read().unwrap().clone()
}

/// 保存配置：先逐条清洗（坏触发键/动作/改键被单独忽略，不影响其余配置），
/// 写入磁盘并即时生效。返回被忽略内容的说明（供 UI 提示），只有真正失败（写盘等）才报错。
/// 落盘走 [`config_io::save`]（原子替换 + 留存上次良好副本），不会留下半截 JSON。
#[tauri::command]
fn set_config(
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
            ..Default::default()
        }
    }
}

/// 计算给定配置的冲突列表（前端把当前正在编辑的配置传入，实时展示警告）。
/// 四部分：① 内部冲突（重复触发键/改键遮蔽/超集重叠）；② 系统快捷键清单命中；
/// ③ Windows 下 RegisterHotKey 探测「其他应用/系统已注册」的真实占用；
/// ④ 平台能力缺失（`PlatformCaps`：本平台监听不到的触发键、注入不了的目标键、
///   恒不成立的前台条件——从 Windows 同步过来的配置在 Linux/macOS 上会静默失效，
///   在这里点出来，别让用户自己猜，见规划 7.3-⑲）；
/// ⑤ 输入层环境（`diagnose_input_env`，规划 7.2-㊽：探测到会 divert 按键的厂商代理且
///   配置用到了鼠标键时给一条「为什么按了没反应」——与 ④ 同一口径，归在同一组里）。
#[tauri::command]
fn get_conflicts(config: Config) -> Vec<Conflict> {
    let mut out = detect_conflicts(&config, &platform_caps());

    // ⑤ 输入层环境诊断（可关：设置页「识别厂商接管的按键」）。关掉后探测整条不跑，
    //    不只是不显示——这类提示的误报余地在那儿摆着，用户关它就该是真静默。
    if config.settings.input_env_diag {
        out.extend(diagnose_input_env(&config, &input_env_cached()));
    }

    // ② 系统快捷键清单（跨平台）：精确匹配；命中的触发键记下，供③跳过避免重复提示。
    let mut sys_hits: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &config.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            let Ok(sc) = t.parse::<Shortcut>() else {
                continue;
            };
            for (combo, desc) in SYSTEM_SHORTCUTS {
                if let Ok(sys) = combo.parse::<Shortcut>() {
                    if sys == sc {
                        out.push(Conflict {
                            severity: Severity::Warn,
                            platform: false,
                            message: format!("「{t}」与系统快捷键 {combo}（{desc}）冲突"),
                            name: s.name.clone().unwrap_or_default(),
                        });
                        sys_hits.insert(t.clone());
                    }
                }
            }
        }
    }

    // ③ Windows：RegisterHotKey 探测其它应用/系统的真实占用。
    #[cfg(windows)]
    for s in &config.shortcuts {
        if !s.enabled {
            continue;
        }
        for t in &s.triggers {
            if sys_hits.contains(t) {
                continue;
            }
            if let Ok(sc) = t.parse::<Shortcut>() {
                if input::hotkey_occupied(&sc) {
                    out.push(Conflict {
                        severity: Severity::Warn,
                        platform: false,
                        message: format!("「{t}」已被系统或其他应用占用"),
                        name: s.name.clone().unwrap_or_default(),
                    });
                }
            }
        }
    }

    out
}

/// 输入层环境诊断的明细报告（设置页「输入层环境」卡片 / 帮助页用，规划 7.2-㊽）。
///
/// 与 [`get_conflicts`] 第⑤段的区别是**口径**：那边只在「探测到接管者**且**配置用到鼠标键」
/// 时才出声（避免噪音），这边是用户主动来看的，所以把探测到的接管者、以及「有没有扫过」
/// 一并交代清楚——包括「本平台还没做」和「查过了没问题」这两种截然不同的结论。
#[derive(Serialize)]
struct InputEnvReport {
    /// 本平台**是否实现了**探测。false = 「这个平台没做」，不是「清理干净了」。
    supported: bool,
    /// 此次探测的结论（接管者）。
    diverters: Vec<InputEnvHit>,
    /// 扫过的接管者类别数（让「未检测到」有分量：不是没查，是查了 N 类都没中）。
    checked: usize,
    /// Windows 内存完整性（HVCI）：`Some(true/false)` = 已开启/已关闭；`None` = 读不到。
    hvci: Option<bool>,
    /// 本平台是否支持「一键解锁罗技侧键」（档位 C）。只在 Windows 为 true；前端据此
    /// 决定要不要显示那一组按钮（**不在这里做 HID 枚举**——那要几百毫秒，不能拖慢卡片）。
    hidpp_supported: bool,
}

/// 一个命中的接管者（展示名 + 处置办法都从 [`kada_core::DIVERTERS`] 来，前端不再抄一份）。
#[derive(Serialize)]
struct InputEnvHit {
    id: String,
    name: String,
    advice: String,
    /// 用户对这个接管者点过「忽略」：只影响冲突列表的提示，卡片里仍列出来（可恢复）。
    ignored: bool,
}

#[tauri::command]
fn input_env_report(config: Config) -> InputEnvReport {
    let supported = input::input_env_supported();
    // 不开诊断就不探测：前端据此显示「已关闭」，也省下这次进程表扫描。
    let env = if supported && config.settings.input_env_diag {
        input_env_cached()
    } else {
        kada_core::InputEnv::default()
    };
    let diverters = kada_core::DIVERTERS
        .iter()
        .filter(|d| env.diverters.iter().any(|id| id == d.id))
        .map(|d| InputEnvHit {
            id: d.id.to_string(),
            name: d.name.to_string(),
            advice: d.advice.to_string(),
            ignored: config.settings.input_env_ignored.iter().any(|id| id == d.id),
        })
        .collect();
    InputEnvReport {
        supported,
        diverters,
        checked: kada_core::DIVERTERS.len(),
        hvci: env.hvci,
        hidpp_supported: kada_hook::hidpp::supported(),
    }
}

// ---- 罗技 HID++ 解锁（`docs/底层输入能力规划.md` 档位 C）----
//
// 比档位 A 更进一步：不只告诉用户「谁拿走了按键」，而是直接把 divert 关掉，让按键回到
// 标准报文本。只在 Windows 有实现（见 `kada_hook::hidpp`），其余平台返回 supported=false。

/// 只读探测视图（设置页「罗技设备」小字）。
#[derive(Serialize)]
struct HidppStatusView {
    supported: bool,
    /// 是否正在后台保持解锁（本次 Kada 运行期间）。
    keeper_active: bool,
    keeper_error: Option<String>,
    /// 通道整体打不开之类的原因。
    error: Option<String>,
    devices: Vec<HidppDeviceView>,
}

#[derive(Serialize)]
struct HidppDeviceView {
    name: String,
    product_id: u16,
    /// HID++ 设备索引（`255` = 直连）。
    index: u8,
    control_count: u8,
    channel: String,
    controls: Vec<HidppControlView>,
}

#[derive(Serialize)]
struct HidppControlView {
    cid: u16,
    label: String,
    present: bool,
    diverted: bool,
    persistent: bool,
}

/// 解锁结果视图（前端直接铺在卡片里）。
#[derive(Serialize)]
struct HidppUnlockView {
    error: Option<String>,
    devices: Vec<HidppUnlockDeviceView>,
    /// 是否开始后台保持（有键被真正改掉时才是 true）。
    keeping: bool,
}

#[derive(Serialize)]
struct HidppUnlockDeviceView {
    name: String,
    cleared: Vec<String>,
    already: Vec<String>,
    missing: Vec<String>,
    failed: Vec<String>,
}

fn hidpp_status_view() -> HidppStatusView {
    let s = kada_hook::hidpp::status();
    HidppStatusView {
        supported: s.supported,
        keeper_active: s.keeper_active,
        keeper_error: s.keeper_error,
        error: s.error,
        devices: s
            .devices
            .into_iter()
            .map(|d| HidppDeviceView {
                name: d.name,
                product_id: d.product_id,
                index: d.index,
                control_count: d.control_count,
                channel: d.channel.to_string(),
                controls: d
                    .controls
                    .into_iter()
                    .map(|c| HidppControlView {
                        cid: c.cid,
                        label: c.label.to_string(),
                        present: c.present,
                        diverted: c.diverted,
                        persistent: c.persistent,
                    })
                    .collect(),
            })
            .collect(),
    }
}

#[tauri::command]
fn hidpp_status() -> HidppStatusView {
    hidpp_status_view()
}

/// 把 divert 关掉，并把结论同时推进消息中心（用户点完按钮后多半会离开设置页）。
#[tauri::command]
fn hidpp_unlock(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) -> HidppUnlockView {
    let r = kada_hook::hidpp::unlock();
    let keeping = r.devices.iter().any(|d| !d.cleared.is_empty());

    let mut lines: Vec<String> = Vec::new();
    for d in &r.devices {
        lines.push(format!("设备：{}", d.name));
        if !d.cleared.is_empty() {
            lines.push(format!("  已解锁：{}", d.cleared.join("、")));
        }
        if !d.already.is_empty() {
            lines.push(format!("  本来就是标准报文：{}", d.already.join("、")));
        }
        if !d.missing.is_empty() {
            lines.push(format!("  设备上没有这些控件：{}", d.missing.join("、")));
        }
        for f in &d.failed {
            lines.push(format!("  失败：{f}"));
        }
    }
    let (headline, detail) = match &r.error {
        Some(err) => (
            "罗技鼠标侧键解锁失败".to_string(),
            format!(
                "{err}\n\n控制接口打不开时，先在 Options+ / G HUB 里把该按键设为「无操作 / 默认功能」，或直接退出它。"
            ),
        ),
        None if keeping => (
            "已解锁罗技鼠标侧键".to_string(),
            format!(
                "{}\n\n本次 Kada 运行期间会保持（设备重连或厂商软件重启后会再关一次）。现在按侧键应能触发快捷键。",
                lines.join("\n")
            ),
        ),
        None => (
            "罗技鼠标侧键本来就是标准报文".to_string(),
            format!("{}\n\n不需要解锁：钩子已经能收到这些按键。", lines.join("\n")),
        ),
    };
    commit_result(
        &app,
        &state.results,
        &state.unread,
        CommandResult {
            kind: "hidpp".into(),
            label: "输入层".into(),
            trigger: "罗技 HID++".into(),
            name: String::new(),
            command: headline,
            stdout: detail,
            stderr: String::new(),
            exit_code: None,
            // 不弹窗：用户就在设置页看着结果，弹窗只会多一步要关。
            show_output: false,
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            run_id: None,
        },
    );

    HidppUnlockView {
        error: r.error,
        keeping,
        devices: r
            .devices
            .into_iter()
            .map(|d| HidppUnlockDeviceView {
                name: d.name,
                cleared: d.cleared.into_iter().map(str::to_string).collect(),
                already: d.already.into_iter().map(str::to_string).collect(),
                missing: d.missing.into_iter().map(str::to_string).collect(),
                failed: d.failed,
            })
            .collect(),
    }
}

/// 立即把当前配置同步写一份到「同步存档」目录，返回写入的文件路径（前端据此给明确反馈）。
/// 与自动推送（每次保存时的 [`push_sync`]）不同：这条失败要报错给用户看。
#[tauri::command]
fn sync_now(state: tauri::State<'_, KadaState>) -> Result<String, String> {
    let cfg = state.config.read().unwrap().clone();
    let file = sync_file(&cfg).ok_or("尚未设置同步存档目录")?;
    config_io::save(&file, &cfg)?;
    Ok(file.display().to_string())
}

/// 「从同步存档恢复」用：返回存档文件的路径，前端拿它走既有的导入流程（合并 / 替换 + 结果弹窗）。
/// 未配置目录 / 存档文件不存在都报错。
#[tauri::command]
fn sync_import_path(state: tauri::State<'_, KadaState>) -> Result<String, String> {
    let cfg = state.config.read().unwrap().clone();
    let file = sync_file(&cfg).ok_or("尚未设置同步存档目录")?;
    if !file.exists() {
        return Err(format!("同步存档里还没有配置：{}", file.display()));
    }
    Ok(file.display().to_string())
}

/// 探测本机 OneDrive 目录（给「选择目录」对话框做默认位置）：Windows 读 OneDrive 客户端写的
/// 环境变量（个人版 / 商业版 / 消费者版都试），macOS / Linux 猜常见的 `~/OneDrive`；都没有则 `None`。
#[tauri::command]
fn default_sync_dir(app: tauri::AppHandle) -> Option<String> {
    for key in ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"] {
        if let Ok(dir) = std::env::var(key) {
            if !dir.trim().is_empty() && Path::new(&dir).is_dir() {
                return Some(dir);
            }
        }
    }
    let p = app.path().home_dir().ok()?.join("OneDrive");
    p.is_dir().then(|| p.display().to_string())
}

/// 导出当前配置到指定路径（JSON）。
#[tauri::command]
fn export_config(state: tauri::State<'_, KadaState>, path: String) -> Result<(), String> {
    let cfg = state.config.read().unwrap().clone();
    let json = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))
}

/// 导入结果：给 UI 报清「新增了什么 / 跳过了什么 / 忽略了多少坏条目 / 原配置留档在哪」。
/// 原先只回一个忽略条数，用户根本查不出是哪条没进来。
#[derive(Serialize)]
struct ImportOutcome {
    /// 实际采用的导入方式（`merge` / `replace`）。
    mode: String,
    /// 新增条目总数（合并方式下是并入的条数；替换方式下是导入文件里的条目数）。
    added: usize,
    /// 明细：逐条说明被跳过的条目（合并）与被忽略的坏条目（清洗）。
    notes: Vec<String>,
    /// 替换导入时「导入前配置」的留档路径（合并导入为 `None`：没覆盖任何东西）。
    backup: Option<String>,
}

/// 把一份「来路不同的子集配置」合并进当前配置并落盘——「导入配置 → 合并」「载入示例配置」
/// 「从卡带市场安装卡带」三条路共用这一条：只增不删、不碰本机设置、坏条目清洗后逐条报出
/// （口径见 [`kada_core::merge_configs`]）。
///
/// 落盘口径与 [`set_config`] 一致：写盘 + 改内存 + 给外部修改监听销账同持 `cfg_watch` 锁。
fn merge_into_config(
    app: &tauri::AppHandle,
    state: &KadaState,
    incoming: Config,
) -> Result<ImportOutcome, String> {
    let (incoming, mut notes) = sanitize_config(&incoming);
    let current = state.config.read().unwrap().clone();
    let (merged, report) = kada_core::merge_configs(&current, &incoming);
    let added = report.added(); // 先取计数，再把 skipped 移进 notes（部分移动后不能再借 report）
    notes.extend(report.skipped);
    // 合并保留本机设置，自启按现值同步（不做系统层改动，只是让状态机与配置一致）。
    let autostart = current.settings.autostart;

    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &merged)?;
    *state.config.write().unwrap() = merged;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(app, autostart);
    // 合并不碰 settings，提示框可见性不会变——不需要 apply_hints_visible 那条对齐路径。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    Ok(ImportOutcome { mode: "merge".into(), added, notes, backup: None })
}

/// 从指定路径导入配置。
///
/// 两种方式：
/// - `merge`：并进当前配置——**只增不删**，现有条目一条不动；改键按原键、文本扩展按触发词、
///   快捷键按触发键判重，撞车的保留现有并逐条报出；`settings` 保留本机现值
///   （口径见 [`kada_core::merge_configs`]）。
/// - `replace`：整份覆盖（含设置）。因为不可逆，覆盖前先把现有配置复制留档
///   （`config.json.pre-import-<时间戳>`；随后 `config_io::save` 还会把旧内容留成 `.bak`）。
///
/// 两种方式都先对导入内容逐条清洗，坏条目被忽略并报出。
#[tauri::command]
fn import_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    path: String,
    mode: Option<String>,
) -> Result<ImportOutcome, String> {
    let replace = match mode.as_deref() {
        Some("merge") => false,
        // 缺省按整份替换（老前端 / 手调 IPC 的既有行为）。
        None | Some("replace") => true,
        Some(other) => return Err(format!("未知的导入方式：{other}")),
    };

    let s = std::fs::read_to_string(&path).map_err(|e| format!("读取失败：{e}"))?;
    let imported: Config = serde_json::from_str(&s).map_err(|e| format!("解析失败：{e}"))?;
    if !replace {
        // 合并交给共用的落地路径（导入 / 载入示例 / 安装卡带同一条）：它自己清洗、自己落盘。
        return merge_into_config(&app, &state, imported);
    }

    let (incoming, mut notes) = sanitize_config(&imported);
    let backup = config_io::archive_copy(&state.file, config_io::PRE_IMPORT_MARK);
    // 留档是尽力而为（可能没配置可读或写不进去），失败要说一声：`save` 那层的 `.bak`
    // 仍是被覆盖内容的回滚点，所以不为它中断导入。
    if backup.is_none() && state.file.exists() {
        notes.push("导入前的配置留档失败（配置仍已替换；可回滚到 config.json.bak）".into());
    }
    let added = incoming.folders.len()
        + incoming.layers.len()
        + incoming.shortcuts.len()
        + incoming.remaps.len()
        + incoming.expansions.len();
    let autostart = incoming.settings.autostart;
    let next = incoming;

    // 与 set_config 同一套口径：写盘 + 改内存 + 给监听销账同持一把锁（导入也是一种「自己写的盘」）。
    let hints_before = state.config.read().unwrap().settings.hints.visible;
    let hints_after = next.settings.hints.visible;
    let mut watch = state.cfg_watch.lock().unwrap();
    config_io::save(&state.file, &next)?;
    *state.config.write().unwrap() = next;
    watch.adopt_own_write(&state.file);
    drop(watch);
    sync_autostart(&app, autostart);
    // 导入也可能换掉提示框的设置（替换导入会连设置一起覆盖）：与 set_config 走同一条对齐路径。
    if let Ok(snapshot) = state.config.read() {
        let _ = app.emit("config-updated", &*snapshot);
        push_sync(&snapshot);
    }
    if hints_after != hints_before {
        apply_hints_visible(&app, hints_after);
    }
    Ok(ImportOutcome {
        mode: if replace {
            "replace".into()
        } else {
            "merge".into()
        },
        added,
        notes,
        backup: backup.map(|p| p.display().to_string()),
    })
}

/// 载入内置示例配置（设置页「载入示例配置」）：并入当前配置，走合并语义——只增不删、
/// 不碰本机设置（与 `import_config(mode = "merge")` 同一口径，只是内容内置在二进制里）。
#[tauri::command]
fn load_sample_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
) -> Result<ImportOutcome, String> {
    merge_into_config(&app, &state, kada_core::sample::sample_config())
}

/// 导出配置卡带（规划 7.2-㉕，配置分享 / 卡带市场）：把点名的那几条（单条、多选、或某个
/// 目录下的全部）写成一个可分享的 JSON 文件。
///
/// 卡带就是一份**子集 `Config`** + 一个顶层 `kada_snippet` 元信息键，所以接收方走现有的
/// 「导入配置 → 合并」原样就能吃下，导入侧无需改动、也不会碰本机设置。目录引用在摘取时剥掉
/// （id 是本机 UUID，带过去只会指向不存在的目录）。
///
/// 写入失败只报错，不碰磁盘上的配置（它压根不读配置路径）。
#[tauri::command]
fn export_fragment(
    state: tauri::State<'_, KadaState>,
    path: String,
    picks: Vec<SnippetPick>,
) -> Result<usize, String> {
    let part = snippet(&state.config.read().unwrap(), &picks);
    let n = part.shortcuts.len() + part.remaps.len() + part.expansions.len() + part.gestures.len();
    if n == 0 {
        return Err("没有可导出的条目".into());
    }
    let title = snippet_title(&part);

    let mut val = serde_json::to_value(&part).map_err(|e| e.to_string())?;
    if let Some(obj) = val.as_object_mut() {
        // 设置不带（合并导入压根不碰设置，带着走只会让分享出来的文件看着像整份配置）。
        obj.remove("settings");
        // 元信息键：`Config` 反序列化忽略未知字段，故卡带与整份配置同一套解析。
        // `version` 给将来的在线索引/格式演进留位置。
        obj.insert(
            "kada_snippet".into(),
            serde_json::json!({ "version": 1, "name": title }),
        );
    }
    let json = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败：{e}"))?;
    Ok(n)
}

// ---- 卡带市场（规划 7.2-㉕ 在线索引） ----
//
// 没有服务端：索引就是一份能 GET 到的静态 JSON（默认指向本仓库 `snippets/index.json`，
// 设置里可改），里面的每条指向一个卡带文件——卡带文件本身仍是「子集 Config + kada_snippet」，
// 安装走的是和「导入配置 → 合并」同一条落地路径。

/// 卡带下载的时限与体积上限：一份索引 / 一个卡带都是几百 KB 量级，超了当坏文件处理。
const SNIPPET_FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const SNIPPET_MAX_BYTES: usize = 4 * 1024 * 1024;

/// 下载一段文本（索引 / 卡带本体）。
///
/// 只收 http(s)：设置里的这个字符串不该变成读本地文件的口子（`file://` 之类一律拒）。
/// 体积上限是「下完再判」——几百 KB 的上限下没必要为流式计数多写一层。
async fn http_get_text(url: &str) -> Result<String, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("不是 http(s) 地址：{url}"));
    }
    // reqwest 用的是 `rustls-no-provider`：TLS 后端由**进程级** crypto provider 决定。updater
    // 是自己在建 client 前才装的 provider——没检查过更新的进程里没人装过，这里建 client 会
    // 直接 panic。装一次即可（已装时静默跳过）。
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let client = reqwest::Client::builder()
        .timeout(SNIPPET_FETCH_TIMEOUT)
        .user_agent(concat!("kada/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("网络初始化失败：{e}"))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("下载失败：HTTP {}", resp.status()));
    }
    let body = resp.bytes().await.map_err(|e| format!("下载中断：{e}"))?;
    if body.len() > SNIPPET_MAX_BYTES {
        return Err(format!("内容太大（{} MB），不像是索引或卡带", body.len() / 1024 / 1024));
    }
    String::from_utf8(body.to_vec()).map_err(|_| "下载内容不是 UTF-8 文本".to_string())
}

/// 索引地址：设置里留空等价于「用内置默认」（见 `Settings::snippet_index_url`）。
fn snippet_index_url(state: &KadaState) -> String {
    let url = state.config.read().unwrap().settings.snippet_index_url.trim().to_string();
    if url.is_empty() {
        DEFAULT_SNIPPET_INDEX_URL.to_string()
    } else {
        url
    }
}

/// 拉取卡带索引（浏览 / 搜索 / 一键安装都从它出发）。地址取设置里的索引地址。
#[tauri::command]
async fn fetch_snippet_index(
    state: tauri::State<'_, KadaState>,
) -> Result<Vec<IndexEntry>, String> {
    let url = snippet_index_url(&state);
    let text = http_get_text(&url).await?;
    let entries = parse_snippet_index(&text)?;
    if entries.is_empty() {
        return Err(format!("索引里一条卡带也没有（{url}）"));
    }
    Ok(entries)
}

/// 一个卡带的下载结果：原文 + 清洗后的条目。
///
/// 装的时候回传 `text` 而不是重新下载：用户确认时看到的正是要装进去的那份字节，
/// 中间不会换成另一份内容（卡带是别人托管的文件，改内容是常事）。
#[derive(Serialize)]
struct SnippetFetch {
    text: String,
    preview: SnippetPreview,
}

/// 卡带预览：条目原样交给前端渲染（前端已有各条类的展示与动作摘要，后端不重复造一遍）。
#[derive(Serialize)]
struct SnippetPreview {
    shortcuts: Vec<ShortcutItem>,
    remaps: Vec<Remap>,
    expansions: Vec<TextExpansion>,
    gestures: Vec<kada_core::Gesture>,
}

/// 下载一个卡带并给出预览，**不落盘**。
///
/// 预览这步不能省：卡带能带命令 / 脚本动作，等于「装进来就会跑」，必须让人先看清里面是什么。
#[tauri::command]
async fn fetch_snippet(url: String) -> Result<SnippetFetch, String> {
    let text = http_get_text(&url).await?;
    let part: Config = serde_json::from_str(&text).map_err(|e| format!("卡带解析失败：{e}"))?;
    let (part, _ignored) = sanitize_config(&part);
    if part.shortcuts.is_empty()
        && part.remaps.is_empty()
        && part.expansions.is_empty()
        && part.gestures.is_empty()
    {
        return Err("卡带里没有可用的条目（全是坏条目或本来就是空的）".into());
    }
    Ok(SnippetFetch {
        text,
        preview: SnippetPreview {
            shortcuts: part.shortcuts,
            remaps: part.remaps,
            expansions: part.expansions,
            gestures: part.gestures,
        },
    })
}

/// 把预览时拿到的卡带文本合并进配置：与「导入配置 → 合并」同一口径（只增不删、不碰设置）。
#[tauri::command]
fn install_snippet(
    app: tauri::AppHandle,
    state: tauri::State<'_, KadaState>,
    text: String,
) -> Result<ImportOutcome, String> {
    let part: Config = serde_json::from_str(&text).map_err(|e| format!("卡带解析失败：{e}"))?;
    merge_into_config(&app, &state, part)
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

/// 取走未匹配手势的方向串（EXT-03）：主界面据此打开「新增鼠标操作」并预填。
#[tauri::command]
fn take_pending_gesture(state: tauri::State<'_, KadaState>) -> Option<String> {
    state.pending_gesture.lock().unwrap().take()
}

/// 标记全部已读（熄灭红点，还原托盘图标）。
#[tauri::command]
fn mark_results_read(app: tauri::AppHandle, state: tauri::State<'_, KadaState>) {
    state.unread.store(false, Ordering::Relaxed);
    set_tray_unread(&app, false);
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
            get_config,
            set_config,
            get_conflicts,
            input_env_report,
            hidpp_status,
            hidpp_unlock,
            elevated_status,
            elevated_start,
            elevated_stop,
            export_config,
            sync_now,
            sync_import_path,
            default_sync_dir,
            export_fragment,
                fetch_snippet_index,
                fetch_snippet,
                install_snippet,
            import_config,
            load_sample_config,
            set_paused,
            abort_actions,
            run_entry,
            start_record,
            stop_record,
            start_mouse_record,
            stop_mouse_record,
            get_command_results,
            get_run_records,
            get_unread,
            get_toast_payload,
            get_status_payload,
            take_pending_gesture,
            hud_ready,
            get_hints_state,
            hints_ready,
            hints_move,
            hints_commit,
            hints_prefs,
            hints_set_visible,
            mark_results_read,
            clear_command_results,
            update::get_update_status,
            update::check_update,
            update::download_update,
            update::install_update
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
                let mut last_tap: Option<Instant> = None;
                input::start(move |ev: input::KeyEvent| {
                    let ev = to_ev(&ev);
                    // 录制中：所有事件进时间线、放行；快捷键/改键全暂停。
                    {
                        let mut g = r.lock().unwrap();
                        if let Some(recorder) = g.as_mut() {
                            recorder.push(&ev);
                            return input::HookAction::Allow;
                        }
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
                    let device = input::current_device();
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
                engine,
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
                unread,
                runs,
                tray: Mutex::new(None),
                tray_base: base_icon.clone(),
                tray_unread: unread_icon,
                toast: Arc::new(Mutex::new(None)),
                status: Arc::new(Mutex::new(None)),
                pending_gesture: Arc::new(Mutex::new(None)),
                hints_tray: Mutex::new(None),
                pause_tray: Mutex::new(None),
                hints_placed: AtomicBool::new(false),
            };
            app.manage(state);
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

            // 快捷键提示框的初值：托盘菜单的勾选态与「启动时要不要建窗」都用它。
            let hints_visible_at_startup = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.hints.visible)
                .unwrap_or(false);
            let paused_at_startup = app
                .state::<KadaState>()
                .config
                .read()
                .map(|c| c.settings.paused)
                .unwrap_or(false);

            // 托盘：常驻后台，关窗不退出。
            let show_i = MenuItem::with_id(app, "show", "打开咔哒", true, None::<&str>)?;
            let pause_i = CheckMenuItem::with_id(
                app,
                "pause",
                "暂停快捷键触发",
                true,
                paused_at_startup,
                None::<&str>,
            )?;
            let hints_i = CheckMenuItem::with_id(
                app,
                "hints",
                "快捷键提示",
                true,
                hints_visible_at_startup,
                None::<&str>,
            )?;
            let reload_i = MenuItem::with_id(app, "reload", "重载配置", true, None::<&str>)?;
            let update_i = MenuItem::with_id(app, "update", "检查更新", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&show_i, &pause_i, &hints_i, &reload_i, &update_i, &quit_i],
            )?;
            let tray_icon = TrayIconBuilder::new()
                .icon(base_icon.unwrap())
                // 托盘提示的初值；有层 / 修饰键生效时由 publish_status 换成当前状态摘要。
                .tooltip("咔哒 Kada")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    // 暂停 / 恢复快捷键触发（勾选态由状态机定时器线程对齐，见 spawn_engine_ticker）。
                    "pause" => toggle_pause(app),
                    "reload" => reload_config_now(app),
                    // 提示框的开关：常显浮窗的入口得在「随时够得着」的地方——窗口上的 × 一按，
                    // 设置界面要是没开着，用户就只剩托盘这一个入口。
                    "hints" => {
                        let visible = app
                            .state::<KadaState>()
                            .config
                            .read()
                            .map(|c| c.settings.hints.visible)
                            .unwrap_or(false);
                        if let Err(e) = hints_set_visible(app.clone(), app.state(), !visible) {
                            eprintln!("切换快捷键提示框失败: {e}");
                        }
                    }
                    "update" => update::spawn(app, update::Mode::Manual),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 双击托盘图标（左键）唤起主窗口。
                    if let TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            {
                let st = app.state::<KadaState>();
                *st.tray.lock().unwrap() = Some(tray_icon);
                // 记住这个勾选项：提示框被窗口上的 × 关掉时，得把勾抹掉。
                *st.hints_tray.lock().unwrap() = Some(hints_i.clone());
                // 记住暂停勾选项：设置页改 `settings.paused` 时定时器线程据此对齐勾选。
                *st.pause_tray.lock().unwrap() = Some(pause_i.clone());
            }

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
