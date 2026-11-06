// 运行时状态与状态指示载荷（T-05 从 lib.rs 搬出）：KadaState（钩子、消息中心、各浮窗草稿）。
// 状态指示由 `Engine::status` 快照 + 配置算出（见 说明书 §5.8）。

use crate::*;

/// 触发气泡载荷：名称 + 触发组合键。
#[derive(Clone, Serialize)]
pub(crate) struct ToastPayload {
    pub(crate) name: String,
    pub(crate) trigger: String,
    /// 自定义副标题：`Some` 时前端直接显示它（「发现新版本」等非执行中气泡用）；
    /// 缺省时前端按「触发键 · 正在执行…」渲染。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) subtitle: Option<String>,
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
    pub(crate) fn idle(&self) -> bool {
        self.layer.is_none()
            && self.hold.is_empty()
            && self.sticky.is_empty()
            && self.oneshot.is_empty()
    }
}

/// 输入状态指示里的一条注入键（见规划 7.3-⑬）。`kind` 决定界面上的用词与配色。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct StatusMod {
    /// `hold`（按住）/ `sticky`（粘滞）/ `oneshot`（单次）。
    pub(crate) kind: &'static str,
    /// 键的中性名（与触发键同一套用词：`Ctrl` / `Shift` / `A` / `MouseBack`…）。
    pub(crate) key: String,
}

/// 输入状态指示载荷：当前激活层 + 注入中的键（见规划 7.3-⑬）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct StatusPayload {
    /// 激活层的**显示名**（层没起名字时回落 id）；`None` = 基础层。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) layer: Option<String>,
    /// 该层来自「长按锁定层」（切换式）：松开切层键仍生效。
    pub(crate) locked: bool,
    /// 注入中的键（按住 / 粘滞 / 单次）。
    pub(crate) mods: Vec<StatusMod>,
    /// 一句话摘要：托盘提示直接用它，也是悬浮指示的兜底文案。
    pub(crate) summary: String,
}

impl StatusPayload {
    /// 空状态（基础层 + 没有注入中的键）——指示据此隐藏。
    pub(crate) fn idle(&self) -> bool {
        self.layer.is_none() && self.mods.is_empty()
    }
}

/// 快捷键提示框（`index.html#hints`）的当前状态，见规划 7.3-㊳。
///
/// `x` / `y` 是窗口**当前**位置（逻辑像素），不是配置里存的那份：前端拖动时用它做差量基准，
/// 配置里的值可能比窗口实际位置旧（拖动中还没落盘）。首次摆放由后端算（工作区右侧），
/// 所以由 `hints_ready` 一并把落地坐标回给前端——不然前端手上的 (0,0) 会让第一次拖动跳一下。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct HintsState {
    /// 是否应该显示（配置里的期望值）。
    pub(crate) visible: bool,
    /// 字号缩放百分比。
    pub(crate) scale: u16,
    /// 不透明度百分比。
    pub(crate) opacity: u8,
    /// 窗口当前左上角（逻辑像素）。
    pub(crate) x: f64,
    pub(crate) y: f64,
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
pub(crate) fn build_status(st: &EngineStatus, cfg: &Config) -> StatusPayload {
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
pub(crate) struct KadaState {
    pub(crate) config: Arc<RwLock<Config>>,
    /// 录入捕获暂停的截止时刻（进程启动后的毫秒数；0 = 未暂停）。见 [`pause_active`]。
    pub(crate) paused_until: Arc<AtomicU64>,
    /// 录制器：Some = 正在录制（此时快捷键/改键全暂停）。
    pub(crate) rec: Arc<Mutex<Option<Recorder>>>,
    /// 鼠标操作录制器：Some = 正在录制鼠标（与 [`KadaState::rec`] 互斥使用）。
    pub(crate) mrec: Arc<Mutex<Option<MouseRecorder>>>,
    pub(crate) file: PathBuf,
    /// 外部修改监听的基线（见 [`config_watch`]）。**本进程每次写盘都要在这里销账**，而且写盘
    /// 与销账必须同持这把锁——否则巡检线程可能插在中间，把刚写的配置当成外部修改再加载一遍。
    pub(crate) cfg_watch: Arc<Mutex<config_watch::ConfigWatcher>>,
    /// 「配置被整份换掉了」的旗标：外部改动采纳后置位，由状态机定时器线程消费并复位 [`Engine`]
    /// （与旧配置绑定的按住层 / 待定和弦 / 注入中的修饰键都该跟着作废，见 [`spawn_engine_ticker`]）。
    pub(crate) cfg_stale: Arc<AtomicBool>,
    pub(crate) _hook: Mutex<Option<input::HookHandle>>,
    /// 消息中心：命令结果（内存态，重启清空）。
    pub(crate) results: Arc<Mutex<Vec<CommandResult>>>,
    /// 冷启动补发（CS-11）：`show_output` 为真、但主窗口当时还没建出来时先存一份，
    /// 前端启动并注册好监听器后调 `take_pending_result_modal` 取走（取走即清）。
    pub(crate) pending_result_modal: Mutex<Option<CommandResult>>,
    /// 是否有未读命令结果（红点）。
    pub(crate) unread: Arc<AtomicBool>,
    /// 消息中心的执行记录表（内存态，重启清空）：一次触发一条，内含逐步记录
    /// （时间线 / 流水线视图的数据源，见 [`fire`]）。
    pub(crate) runs: Arc<Mutex<Vec<RunRecord>>>,
    /// 最近一次触发气泡的载荷（懒创建 toast 窗口时，供前端加载后兜底读取）。
    pub(crate) toast: Arc<Mutex<Option<ToastPayload>>>,
    /// 最近一次输入状态指示的载荷（懒创建状态指示窗时，供前端加载后兜底读取）。
    pub(crate) status: Arc<Mutex<Option<StatusPayload>>>,
    /// 复制快捷菜单（EXT-13）是否处于「已请求 / 已显示」生命周期。
    pub(crate) clipboard_menu_showing: Arc<AtomicBool>,
    /// 钩子回调发现 Esc 时置位，菜单轮询线程消费后收回（回调里不碰窗口）。
    pub(crate) clipboard_menu_hide: Arc<AtomicBool>,
    /// 每次触发递增：旧的轮询线程据此让位（与 `GESTURE_VISUAL_SERIAL` 同一套路）。
    pub(crate) clipboard_menu_serial: Arc<AtomicU64>,
    /// 菜单锚点（物理像素，触发时的鼠标位置）。
    pub(crate) clipboard_menu_anchor: Mutex<Option<(i32, i32)>>,
    /// 触发那一刻的前台窗口：菜单开着时前台变了就收回，别把动作打进别的程序。
    pub(crate) clipboard_menu_foreground: Mutex<Option<isize>>,
    /// 最近一次菜单载荷（菜单窗懒创建后，前端加载时兜底渲染）。
    pub(crate) clipboard_menu_payload: Mutex<Option<ClipboardMenuPayload>>,
    /// 最近一次「设置变量」输入框载荷（输入窗懒创建后，前端加载时兜底渲染）。
    pub(crate) prompt_payload: Mutex<Option<PromptPayload>>,
    /// 输入窗的回传通道：`Some(值)` = 确定、`None` = 取消 / 关窗。在 [`ui_prompt`] 里填。
    pub(crate) prompt_tx: Mutex<Option<std::sync::mpsc::Sender<Option<String>>>>,
    /// 托盘图标（用于运行时叠 / 去红点）。
    pub(crate) tray: Mutex<Option<TrayIcon>>,
    /// 托盘基础图标（无红点）。
    pub(crate) tray_base: Option<tauri::image::Image<'static>>,
    /// 托盘带红点图标（未读态）。
    pub(crate) tray_unread: Option<tauri::image::Image<'static>>,
    /// 托盘菜单里的「快捷键提示」勾选项：提示框的可见性可能被窗口上的 × 改掉，
    /// 那时得把勾也抹掉——不记住它就只能干看着菜单与现状不一致。
    /// **只有 Linux 有原生菜单**（其余平台是自绘浮窗，勾选态每次弹出现读配置）。
    pub(crate) hints_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 托盘菜单里的「暂停快捷键触发」勾选项：`settings.paused` 也可能被设置页改掉，
    /// 由状态机定时器线程统一把勾对齐（见 [`spawn_engine_ticker`]）。
    /// **只有 Linux 有原生菜单**（其余平台是自绘浮窗，勾选态每次弹出现读配置）。
    pub(crate) pause_tray: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 自绘托盘菜单是否处于「已请求 / 已显示」生命周期（Linux 走原生菜单，恒为 false）。
    pub(crate) tray_menu_showing: Arc<AtomicBool>,
    /// 每次弹出托盘菜单递增：旧的光标巡检线程据此让位（与 `GESTURE_VISUAL_SERIAL` 同一套路）。
    pub(crate) tray_menu_serial: Arc<AtomicU64>,
    /// 托盘菜单锚点（物理像素，右键那一刻的光标位置）：量完尺寸后据此贴图标定位。
    pub(crate) tray_menu_anchor: Mutex<Option<(i32, i32)>>,
    /// 最近一次托盘菜单载荷（菜单窗懒创建后，前端加载时兜底渲染）。
    pub(crate) tray_menu_payload: Mutex<Option<TrayMenuPayload>>,
    /// 本次会话是否已经给提示框摆过位：摆过之后原地不动（拖动 / 缩放都不再重定位），
    /// 免得「内容重排 → 又按配置里的旧坐标摆一次」把刚拖到的位置弹回去。
    pub(crate) hints_placed: AtomicBool,
}
