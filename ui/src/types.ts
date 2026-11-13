// 平台无关的类型定义（从 main.ts 抽出，U-01）。只放 type / interface，别放运行时代码。

export type DesktopDirection = "prev" | "next";
/** 文件/目录动作的子操作（动作类型 `os`）。 */
export type OsOperation =
  | { op: "copy"; source: string; dest: string }
  | { op: "cut"; source: string; dest: string }
  | { op: "paste"; dest: string }
  | { op: "delete"; path: string }
  | { op: "new_file"; path: string }
  | { op: "new_folder"; path: string }
  | { op: "open_folder"; path: string }
  | { op: "open_file"; path: string }
  | { op: "reveal"; path: string }
  | { op: "zip"; source: string; dest: string }
  | { op: "unzip"; source: string; dest: string }
  | { op: "get_file_props"; path: string; var: string };
/** 操作系统动作的子操作（动作类型 `system`）——与文件/目录是两类，别混。 */
export type SystemOperation =
  | { op: "switch_desktop"; direction: DesktopDirection }
  | { op: "empty_recycle_bin" }
  | { op: "monitor_off" }
  | { op: "toggle_desktop_icons" }
  /** 向非前台窗口发按键 / 文本（原独立动作 `send_to`，入口并入操作系统）。 */
  | { op: "send_to"; target: string; payload: SendPayload }
  /** 显示器亮度（原独立动作 `brightness`，入口并入操作系统）。 */
  | { op: "brightness"; operation: BrightnessOperation; step: number };
/** 当前窗口动作（类型 `window`，EXT-14）：作用对象是触发那一刻的前台窗口。 */
export type WindowOperation = "maximize_restore" | "minimize" | "toggle_topmost" | "center" | "close" | "close_same_app";
/** 剪贴板动作（类型 `clipboard`，EXT-15）：v1 只处理文本。 */
export type ClipboardOperation =
  | { op: "set_clipboard"; text: string }
  | { op: "get_clipboard"; var: string }
  | { op: "tempstr"; text: string };
/** 旧截图动作（功能已移除，只为老配置还能渲染）。 */
export type CaptureOperation = "full_screen" | "region" | "window";
export type CaptureOutput = "clipboard" | "file" | "pin";
/** 外部工具（探测到进程在跑才出现；执行目前只报「尚未接入」）。 */
export type ExternalCapability = { id: string; name: string };
export type ExternalTool = {
  id: string;
  name: string;
  processes: string[];
  capabilities: ExternalCapability[];
};
/** 显示器亮度（类型 `brightness`，EXT-22）。 */
export type BrightnessOperation = "up" | "down" | "restore";
/** 算法动作（类型 `transform`，EXT-18）：base64 / URL 编解码、哈希。 */
export type TransformOperation =
  | "base64_encode"
  | "base64_decode"
  | "url_encode"
  | "url_decode"
  | "hash_sha256"
  | "hash_sha512";
export type Condition =
  | { kind: "exists"; path: string }
  | { kind: "not_exists"; path: string }
  | { kind: "is_file"; path: string }
  | { kind: "is_dir"; path: string }
  | { kind: "equals"; var: string; field: string; value: string }
  | { kind: "not_equals"; var: string; field: string; value: string }
  | { kind: "modified_within"; path: string; minutes: number }
  | { kind: "frontmost_app"; app: string }
  | { kind: "not_frontmost_app"; app: string }
  | { kind: "window_title_contains"; text: string }
  | { kind: "device_is"; id: string };
export type AppOperation =
  | { op: "launch"; program: string; args: string[] }
  | { op: "close"; program: string }
  | { op: "maximize"; program: string }
  | { op: "minimize"; program: string }
  | { op: "activate"; program: string }
  | { op: "status"; program: string; var: string; retries: number; interval_ms: number }
  | { op: "restart"; program: string; args: string[] }
  /** 对触发那一刻的前台窗口操作（原独立动作 `window`，入口并入应用操作）。 */
  | { op: "current_window"; action: WindowOperation };
export type TextMode = "input" | "to_upper" | "to_lower";
export type Shell = "cmd" | "powershell";
export type MouseButton = "left" | "right" | "middle" | "back" | "forward";
export type MouseOp =
  | { op: "move"; dx: number; dy: number }
  | { op: "move_to"; x: number; y: number }
  | { op: "move_to_origin" }
  | { op: "click"; button: MouseButton }
  | { op: "scroll"; dx: number; dy: number };
export type KeysMode = "tap" | "press" | "release";
/**
 * 语义动作的子操作（`Action::Semantic`）：安卓无障碍的「改键」替身。
 * 值的拼写必须与 `kada_core::SemanticOperation` 的 serde 名逐字相同（snake_case）。
 */
export type SemanticOperation =
  | "copy"
  | "cut"
  | "paste"
  | "select_all"
  | "ime_enter"
  | "scroll_forward"
  | "scroll_backward"
  | "back"
  | "home"
  | "recents"
  | "notifications"
  | "quick_settings"
  | "lock_screen"
  | "screenshot";
export type SendPayload =
  | { kind: "keys"; keys: string[] }
  | { kind: "text"; text: string };
export type ShowOutput = boolean | "always" | "on_error" | "never";
/** 同一动作上一次还没跑完时怎么办（CS-17）。 */
export type IfRunning = "run" | "skip" | "queue";
/** 等待动作的目标（CS-18）。`window` 目前只有 Windows 能等。 */
export type WaitTarget =
  | { kind: "file"; path: string }
  | { kind: "process"; program: string }
  | { kind: "window"; title: string };
export type ActionBase =
  | { type: "text"; text: string; mode: TextMode; masked?: boolean; description?: string }
  | { type: "command"; shell: Shell; command: string; working_dir?: string; timeout_ms?: number | null; ok_exit_codes?: number[]; show_output: ShowOutput; hide_window?: boolean; var: string; wait?: boolean; on_error?: "continue" | "stop"; if_running?: IfRunning; description?: string }
  | { type: "keys"; keys: string[]; mode?: KeysMode; description?: string }
  | { type: "send_to"; target: string; payload: SendPayload; description?: string }
  | { type: "pause_ms"; ms: number; description?: string }
  | { type: "mouse"; op: MouseOp; description?: string }
  | { type: "semantic"; operation: SemanticOperation; description?: string }
  | { type: "clipboard"; operation: ClipboardOperation; description?: string }
  | { type: "transform"; operation: TransformOperation; input: string; var: string; inject?: boolean; description?: string }
  | { type: "os"; operation: OsOperation; description?: string }
  | { type: "system"; operation: SystemOperation; description?: string }
  | { type: "window"; operation: { op: WindowOperation }; description?: string }
  | { type: "capture"; operation: CaptureOperation; output: CaptureOutput; path: string; description?: string }
  | { type: "tool"; tool: string; capability: string; description?: string }
  | { type: "brightness"; operation: BrightnessOperation; step: number; description?: string }
  | { type: "app"; operation: AppOperation; description?: string }
  | { type: "open_url"; url: string; description?: string }
  | { type: "if"; condition: Condition; then: Action[]; otherwise: Action[]; description?: string }
  | { type: "parallel"; actions: Action[]; description?: string }
  | { type: "group"; name?: string; actions: Action[] }
  | { type: "script"; path: string; interpreter?: string | null; interpreter_args?: string[]; args?: string[]; working_dir?: string; timeout_ms?: number | null; ok_exit_codes?: number[]; show_output: ShowOutput; hide_window?: boolean; var: string; wait?: boolean; on_error?: "continue" | "stop"; if_running?: IfRunning; description?: string }
  | { type: "inline_script"; source: string; var: string; show_output?: boolean; description?: string }
  | { type: "wait_for"; target: WaitTarget; timeout_ms?: number | null; interval_ms?: number; var: string; description?: string }
  | { type: "ask"; var: string; prompt: string; default?: string; popup?: boolean; description?: string };
/** 禁用标记（执行时整条跳过）：只包一层，界面编辑的一直是被包住的那条。 */
export type DisabledAction = { type: "disabled"; action: Action };
export type Action = ActionBase | DisabledAction;
export type Folder = { id: string; name: string; parent?: string | null };
export type Layer = { id: string; name: string; mark?: string | null };
export type ShortcutItem = {
  name?: string | null;
  description?: string | null;
  folder?: string | null;
  layer?: string | null;
  /** 触发侧条件门控（规划 7.2-㊶）：仅当条件成立时才触发；缺省 = 无条件。 */
  when?: Condition | null;
  triggers: string[];
  actions: Action[];
  enabled: boolean;
};
export type Remap = {
  /** 标题行里编辑的名称（三类条目统一：标题行 = name）。 */
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  from: string;
  to: string;
  /** 所属目录 id（根级留空）：只影响列表分组，不参与触发判定。 */
  folder?: string | null;
  tap: string | null;
  hold: string | null;
  layer: string | null;
  hold_layer: string | null;
  lock_layer: string | null;
  tap_timeout_ms: number;
  oneshot: string | null;
  sticky: string | null;
  tap2: string | null;
  tap3: string | null;
  /** 触发侧条件门控（规划 7.2-㊶）：仅当条件成立时才生效；缺省 = 无条件。 */
  when?: Condition | null;
  enabled: boolean;
};
export type TextExpansion = {
  /** 标题行里编辑的名称（三类条目统一：标题行 = name）。 */
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  trigger: string;
  replace: string;
  folder?: string | null;
  enabled: boolean;
};
export type Settings = {
  autostart: boolean;
  paused: boolean;
  wake_key?: string | null;
  /** 命令/脚本动作的执行超时（毫秒）；0 = 不限时。界面按秒录入。 */
  action_timeout_ms: number;
  /** 键序列（leader key）等待窗（毫秒）；0 = 不限时。界面按毫秒录入。 */
  sequence_timeout_ms: number;
  /** 和弦等待窗（毫秒）；0 = 不限时。界面按毫秒录入。 */
  chord_timeout_ms: number;
  /** 输入状态悬浮指示：有层 / 修饰键生效时在屏幕下方浮一条状态，没有时自动消失。 */
  show_status_hud: boolean;
  /** 触发提醒：每次触发在右下角弹 3 秒气泡。缺字段按开。 */
  trigger_toast?: boolean;
  /**
   * 文本注入方式（规划 7.3-㉒）：`clipboard` 剪贴板粘贴（默认，老配置缺字段也按它）
   * / `unicode` 逐字直发——目标程序吞粘贴（游戏 / 终端）时切这个。
   */
  text_inject_mode?: "clipboard" | "unicode";
  /**
   * 快捷键提示框（常显速查浮窗，规划 7.3-㊳）：可见性 + 位置（逻辑像素）+ 字号 / 透明度
   * 百分比。缺字段按后端默认（不显示 / 100% / 92%）。
   */
  hints?: {
    visible: boolean;
    x?: number | null;
    y?: number | null;
    scale: number;
    opacity: number;
  };
  /**
   * 卡带市场的索引地址（规划 7.2-㉕）：一份能下载到的静态 JSON；缺字段 / 空串 = 用后端内置默认。
   */
  snippet_index_url?: string;
  /**
   * 同步存档目录（规划 7.2-㉝）：每次配置落盘额外写一份 `kada-config.json` 到这里
   * （如 OneDrive 文件夹）；空 / 缺字段 = 关闭。
   */
  sync_dir?: string | null;
  /**
   * 输入层环境诊断（规划 7.2-㊽）：探测有没有会「抢走按键」的厂商软件在跑。
   * 缺字段按后端默认「开」（老配置也拿到这个能力——它只在真探测到东西且配置用到鼠标键时才出声）。
   */
  input_env_diag?: boolean;
  /**
   * 输入层环境诊断里已确认误报、不再提示的接管者 id（规划 7.2-㊽ 余项②）。只掐提示，
   * 不改探测：设置页卡片仍列出它们，旁边给「恢复提示」。缺字段 = 空。
   */
  input_env_ignored?: string[];
  /**
   * 提权窗口注入（规划 7.2-㉑a）：前台是提权窗口时，把注入交给一个按需拉起的提权进程去做
   * （UIPI 只拦「低往高」的输入）。缺字段 = 关（后端 `#[serde(default)]` 的零值）——
   * 它是默认关的可选能力：打开要弹一次 UAC，不替用户决定。
   */
  elevated_inject?: boolean;
  /**
   * 软件更新源（后端 `UpdateSource`）：`ihomy` 国内镜像（默认，服务器每天从 GitHub 同步）
   * / `github` 官方 Releases。缺字段按 ihomy（与后端 serde default 一致）。
   */
  update_source?: "ihomy" | "github";
  /**
   * 自动更新：发现新版本后自动下载，电脑长时间空闲时自动安装（免安装版另在下次启动时装）。
   * 缺字段 = 关（后端 `#[serde(default)]` 的零值）——自动下载 + 空闲安装会退出并重启应用，
   * 不替用户决定。
   */
  auto_update?: boolean;
  /**
   * **鼠标操作总开关**（规划 7.2-MG）：关掉后鼠标类功能一律不可用（入口隐藏 +
   * 行为不生效），**配置保留**（重新打开即恢复）。缺字段按后端默认「**关**」。
   * 触摸板手势**不受它管**（见下面的 `touchpad`）。
   */
  experimental?: boolean;
  /**
   * **触摸板手势总开关**：与 `experimental`（鼠标操作）互相独立，缺字段按后端默认「**关**」。
   * 关掉后触摸板手势入口隐藏 + 不采集（触发键照常输出），**配置保留**。
   */
  touchpad?: boolean;
  /**
   * 触摸板手势的起手距离（像素）：这一笔至少移动这么远才算数（也是轨迹浮层亮出的阈值）。
   * 缺字段按后端默认 22。
   */
  touchpad_start_distance?: number;
  /**
   * 触摸板手势的排除程序（每行一个进程名）：命中的程序里不执行触摸板手势
   * （触发键已被吞、没有右键回放）。语义同 `gesture_exclude`：不区分大小写、
   * `*`/`?` 通配、否则子串。缺字段 = 不排除。
   */
  touchpad_exclude?: string[];
  /**
   * 触摸板手势：松键后把光标放回起笔点。缺字段 = 开（后端 serde default 就是 true）——
   * 画一笔是「借道」屏幕，不还原会打乱用户正在用的指针位置。
   */
  touchpad_restore_cursor?: boolean;
  /**
   * 触摸板手势：采集时显示轨迹浮层（预览 + 结果）。缺字段 = 开。
   * 关掉只影响触摸板这条来源（右键手势没有单独开关），识别与气泡照旧。
   */
  touchpad_overlay?: boolean;
  /**
   * 鼠标手势的起手距离（像素）：光标累计移动超过它才算「开始画手势」。
   * 缺字段按后端默认 22。
   */
  gesture_start_distance?: number;
  /**
   * 鼠标手势的超时（毫秒）：画到一半停住超过这么久就放弃这次手势。
   * 缺字段按后端默认 1500。
   */
  gesture_timeout_ms?: number;
  /**
   * 鼠标手势方向量化的段长（像素）：一段方向至少这么长，抖动自己被吸收掉。
   * 缺字段按后端默认 16。
   */
  gesture_min_segment?: number;
  /**
   * 右键圈选截图（自带手势）：绕一圈后**右键别松**再点左键确认，包围盒写剪贴板。
   * 只画圈不截图。缺字段按后端默认 `true`（总闸 experimental 仍管着）。
   */
  gesture_screenshot?: boolean;
  /**
   * 手势排除程序（每行一个进程名）：命中的程序里不接管右键。语义同 `when` 的前台应用
   * （不区分大小写、`*`/`?` 通配、否则子串）。缺字段 = 不排除。
   */
  gesture_exclude?: string[];
  /**
   * 全局排除程序（每行一个进程名）：命中的程序里所有 Kada 触发一律不生效
   * （快捷键 / 改键 / 文本扩展 / 手势）。语义同 `when` 的前台应用
   * （不区分大小写、`*`/`?` 通配、否则子串）。缺字段 = 不排除。
   */
  exclude_apps?: string[];
  /**
   * 全屏程序里暂停（EXT-05）：前台全屏程序时所有 Kada 触发一律不生效，退出全屏自动恢复。
   * 缺字段 = 关。
   */
  ignore_fullscreen?: boolean;
  /** 手势轨迹颜色（`#RRGGBB`）。缺字段按后端默认 `#4c9aff`。 */
  gesture_trail_color?: string;
  /** 手势轨迹线宽（像素，1~20）。缺字段按后端默认 5。 */
  gesture_trail_width?: number;
  /** 手势结果标签字号（像素，8~48）。缺字段按后端默认 14。 */
  gesture_label_font_size?: number;
  /** 手势结果标签相对末端的偏移（像素，0~200）。缺字段按后端默认 16。 */
  gesture_label_offset?: number;
    /** 手势未命中失败色（`#RRGGBB`）。缺字段按后端默认 `#ff5f56`。 */
    gesture_fail_color?: string;
    /**
     * 复制快捷菜单（EXT-13）：双击 Ctrl+C 弹出动作菜单。缺字段 = 关（后端
     * `ClipboardMenu::default`）；`enabled` 缺省即 true（条目里的）。
     */
    clipboard_menu?: ClipboardMenu;
    /**
     * 旧版角动作配置（EXT-10）：只在老配置里出现，后端加载时迁移到根级 `hot_corners`。
     */
    hot_corners?: unknown;
    /**
     * 旧版边缘滚动配置（EXT-11）：同上，加载时迁移到根级 `wheel_edges`。
     */
    wheel_edges?: unknown;
  };
/**
 * 复制快捷菜单（EXT-13）：见后端 `ClipboardMenu`。条目数组顺序即显示顺序。
 */
export type MenuEntry =
  | { kind: "action"; label: string; enabled?: boolean; action: Action }
  | { kind: "submenu"; label: string; enabled?: boolean; items: MenuEntry[] }
  | { kind: "separator" };
export type ClipboardMenu = { enabled: boolean; double_copy_ms: number; items: MenuEntry[] };
/** 角动作位置（后端 `HotCornerSpot`）。 */
export type HotCornerSpot = "top_left" | "top_right" | "bottom_left" | "bottom_right";
/** 一条角动作规则（后端 `HotCornerRule`）：同位置可有多条，取第一条启用的。 */
export type HotCornerRule = {
  spot: HotCornerSpot;
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  /** 所属层：null / 缺省 = 基础层（恒生效）。 */
  layer?: string | null;
  /** 防误触：光标在角上停留这么久才触发（毫秒；0 / 缺省 = 碰即触发）。 */
  dwell_ms?: number;
  /** 触发侧条件门控（同快捷键）；缺省 = 无条件。 */
  when?: Condition | null;
  enabled: boolean;
  actions: Action[];
  /** 归属文件夹（null / 缺省 = 根级）。 */
  folder?: string | null;
  /** 统一展示顺序（三类共用；只影响列表顺序）。 */
  order?: number;
};
/** 边缘滚动位置（后端 `WheelEdgeSpot`）。 */
export type WheelEdgeSpot = "top" | "bottom" | "left" | "right";
/** 一条边缘滚动规则（后端 `WheelEdgeRule`）：同位置可有多条，取第一条启用的。 */
export type WheelEdgeRule = {
  edge: WheelEdgeSpot;
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  /** 所属层：null / 缺省 = 基础层（恒生效）。 */
  layer?: string | null;
  /** 触发侧条件门控（同快捷键）；缺省 = 无条件。 */
  when?: Condition | null;
  enabled: boolean;
  actions: Action[];
  /** 归属文件夹（null / 缺省 = 根级）。 */
  folder?: string | null;
  /** 统一展示顺序（三类共用；只影响列表顺序）。 */
  order?: number;
};
/**
 * 一条**鼠标手势**（规划 7.2-MG）：按住右键画轨迹、按方向串匹配。
 * 后端模型见 `kada-core` 的 `Gesture`；触摸板手势是另一个类型 / 数组（见 `TouchpadGesture`）。
 */
export type Gesture = {
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  /** 方向串：`U`/`D`/`L`/`R` 与斜向 `UR`/`DR`/`DL`/`UL`（如 `DRUL`）。 */
  pattern: string;
  enabled: boolean;
  actions: Action[];
  /** 触发侧条件门控（同快捷键）：条件不成立 = 这条不存在，回放右键。 */
  when?: Condition | null;
  /** 归属文件夹（null / 缺省 = 根级）。 */
  folder?: string | null;
  /** 统一展示顺序（鼠标操作三类共用；只影响列表顺序）。 */
  order?: number;
};
/** 一条**触摸板手势**（后端 `TouchpadGesture`）：按住触发键、在触摸板上画一笔。 */
export type TouchpadGesture = {
  name?: string | null;
  /** 简短描述（「详情」页签）。 */
  description?: string | null;
  /** 触发键：按住它进入采集；只能是键盘键（鼠标键由后端清洗掉）。 */
  trigger?: string | null;
  /** 方向串（同鼠标手势的写法）。 */
  pattern: string;
  /** 形状模板（24 点归一化；「录入」带上，只在方向串没命中时兜底）。 */
  template?: [number, number][] | null;
  enabled: boolean;
  actions: Action[];
  /** 触发侧条件门控（同快捷键）：条件不成立 = 这条不存在。 */
  when?: Condition | null;
  /** 归属文件夹（null / 缺省 = 根级）。 */
  folder?: string | null;
  /** 展示顺序：触摸板手势自成一组，与鼠标操作各自的序列互不干扰。 */
  order?: number;
};
export type Config = { folders: Folder[]; layers: Layer[]; shortcuts: ShortcutItem[]; remaps: Remap[]; expansions: TextExpansion[]; gestures?: Gesture[]; touchpad_gestures?: TouchpadGesture[]; hot_corners?: HotCornerRule[]; wheel_edges?: WheelEdgeRule[]; settings: Settings };
/** 输入状态指示载荷（后端 `StatusPayload`）：当前激活层 + 注入中的修饰键，见规划 7.3-⑬。 */
export type StatusPayload = {
  /** 激活层的显示名；缺省 = 基础层（没有层生效）。 */
  layer?: string;
  /** 该层来自「长按锁定层」：松开切层键仍生效。 */
  locked: boolean;
  /** 注入中的键（任意键，见 7.3-⑭），`kind` 决定用词：按住 / 粘滞 / 单次。 */
  mods: { kind: "hold" | "sticky" | "oneshot"; key: string }[];
  /** 后端拼好的一句话摘要（托盘提示用，悬浮窗不展示）。 */
  summary: string;
};
/**
 * 快捷键提示框状态（后端 `HintsState`，见规划 7.3-㊳）：`x` / `y` 是窗口**当前**位置
 * （逻辑像素），拖动时拿它当差量基准——配置里存的那份可能比窗口实际位置旧。
 */
export type HintsState = { visible: boolean; scale: number; opacity: number; x: number; y: number };
// `platform: true` = 平台能力缺失（配置合法、当前平台支持不了，见后端 `Conflict::platform`）。
export type Conflict = { severity: "error" | "warn"; message: string; name: string; platform?: boolean };

/**
 * 输入层环境诊断报告（后端 `InputEnvReport`，规划 7.2-㊽）：探测到的接管者（厂商软件把按键
 * 转交给自己，钩子看不到）、扫过的类别数、Windows 内存完整性状态。
 *
 * `supported: false` = **本平台还没做这项探测**（目前 macOS），与「查过了、很干净」是两句话，
 * 界面上必须分开说——把能力缺口说成「没问题」是最糟的一种谎。
 */
export type InputEnvReport = {
  supported: boolean;
  diverters: { id: string; name: string; advice: string; ignored: boolean }[];
  checked: number;
  hvci: boolean | null;
  /** 本平台是否支持「一键解锁罗技侧键」（档位 C，Windows 专属）。 */
  hidpp_supported: boolean;
};

/**
 * 罗技 HID++ 设备的只读状态（后端 `HidppStatusView`，`docs/底层输入能力规划.md` 档位 C）。
 *
 * `diverted: true` 就是「这个键被厂商软件拿走了」——Kada 的钩子根本收不到这次按下；
 * `persistent: true` 表示厂商软件还写了持久标志（重启 / 重连后仍然 divert）。
 */
export type HidppStatusView = {
  supported: boolean;
  keeper_active: boolean;
  keeper_error: string | null;
  error: string | null;
  devices: {
    name: string;
    product_id: number;
    index: number;
    control_count: number;
    channel: string;
    controls: { cid: number; label: string; present: boolean; diverted: boolean; persistent: boolean }[];
  }[];
};

/** 解锁结果（后端 `HidppUnlockView`）。`keeping` = 已开始后台保持。 */
export type HidppUnlockView = {
  error: string | null;
  keeping: boolean;
  devices: {
    name: string;
    cleared: string[];
    already: string[];
    missing: string[];
    failed: string[];
  }[];
};

/**
 * 「设备是」条件可选的输入设备（后端 `input_devices`，规划 7.1-㊷ 的收尾 + 余项）。
 *
 * `id` 是**稳定的匹配模式**（Windows 的 `VID_046D&PID_C52B` / `ACPI#MSI0007`、Linux 的
 * evdev 设备名）——设备路径里的枚举实例号换 USB 口就变，所以后端只给片段；`detail` 是
 * 完整路径，手写通配时可以直接抄。`kind` 是设备大类（`keyboard` / `mouse` / `touchpad` /
 * `tablet` / `other`），下拉按它分组。`supported: false`（macOS）= 本平台取不到设备。
 */
export type InputDeviceList = {
  supported: boolean;
  devices: { id: string; label: string; detail: string; kind: string }[];
};

/**
 * 提权窗口注入的现状（后端 `ElevatedStatus`，规划 7.2-㉑a）：
 * `supported` = 本平台有没有这套能力（Windows 专属，其余平台恒 false）；
 * `enabled` = 配置里的开关；`connected` = 提权进程是不是已经在跑并且连上了。
 *
 * 三种结论要分开说：「本平台不支持」是能力缺口、「开关没开」是用户的决定、「开关开着但没连上」
 * 才是「点了启动但没成功 / 提权进程已退出」——把前两种说成最后一种，用户会以为坏了。
 */
export type ElevatedStatus = { supported: boolean; enabled: boolean; connected: boolean };

/** 软件更新现状（后端 `get_update_status` / `update-status` 事件）。 */
export type UpdateStatus = { current: string; kind: "installed" | "portable" } & (
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "up-to-date" }
  | { phase: "available"; version: string; notes?: string | null }
  | { phase: "downloading"; version: string; percent?: number | null }
  | { phase: "ready"; version: string; notes?: string | null }
  | { phase: "installing"; version: string }
  | { phase: "error"; message: string }
);

export type Section =
  | "shortcuts"
  | "remaps"
  | "expansions"
  | "gestures"
  | "touchpads"
  | "hotcorners"
  | "wheeledges"
  | "layers"
  | "market"
  | "messages"
  | "settings"
  | "help";
// 合并列表里的条目类型：列表栏只有一份（行首图标区分类型），编辑页/详情页仍按类分。
// gesture = **鼠标操作**（7.2-MG）：按住右键画轨迹触发的条目；
// touchpad = **触摸板手势**：按住一个键盘键、在触摸板上画一笔触发的条目（单独分类）。
export type ListKind =
  | "shortcut"
  | "remap"
  | "expansion"
  | "gesture"
  | "touchpad"
  | "hotcorner"
  | "wheeledge";
// 编辑页的四个页签：详情 / 动作·操作·展开文本（work）/ 层 / 条件。
export type EntryTab = "detail" | "work" | "layer" | "when";
export type CommandResult = {
  kind: string;
  label: string;
  command: string;
  trigger: string;
  name: string;
  stdout: string;
  stderr: string;
  exit_code: number | null;
  detached?: boolean;
  log_path?: string | null;
  show_output: boolean;
  time: string;
  run_id: number | null;
};

// 一个动作的执行记录：时间线的时间块 / 流水线的节点（字段与 kada-actions 的 ActionStep 对齐）。
export type ActionStep = {
  id: number;
  parent: number | null;
  lane: number;
  kind: string;
  title: string;
  branch: boolean | null;
  detail: unknown;
  status: string;
  start_ms: number;
  end_ms: number;
  stdout: string;
  stderr: string;
  exit_code: number | null;
  detached?: boolean;
  log_path?: string | null;
};

// 一次触发 = 一条执行记录。
export type RunRecord = {
  id: number;
  trigger: string;
  name: string;
  time: string;
  start_epoch_ms: number;
  duration_ms: number;
  status: string;
  steps: ActionStep[];
};

// 拖拽排序 / 移动（列表与编辑器画布共用；pointer events，不依赖 WebView2 的 HTML5 DnD）。
export type DragPayload =
  | { kind: "shortcut"; idx: number }
  | { kind: "folder"; id: string }
  | { kind: "remap"; idx: number }
  | { kind: "expansion"; idx: number }
  | { kind: "gesture"; idx: number }
  | { kind: "touchpad"; idx: number }
  | { kind: "hotcorner"; idx: number }
  | { kind: "wheeledge"; idx: number };
export type DropZone =
  | { kind: "root" }
  | { kind: "folder"; folderId: string; pos: "before" | "into" | "after" }
  | { kind: "row"; idx: number; pos: "before" | "after"; targetKind?: ListKind };
export interface DragState {
  payload: DragPayload;
  startX: number;
  startY: number;
  active: boolean; // 越过启动阈值后才算真正在拖拽
  sourceEl: HTMLElement;
}
/** 动作拖拽（详情编辑器内）：改变动作顺序或逻辑纵深（拖进/拖出条件分支）。 */
export interface ActionDragState {
  list: Action[]; // 源动作所在列表（数组引用）
  index: number; // 源动作在列表中的下标
  startX: number;
  startY: number;
  active: boolean; // 越过启动阈值后才算真正在拖拽
  sourceEl: HTMLElement;
}

// 选中集操作（合并列表里各类条目共存，靠内容定位而不是下标）。
export type AnyEntry =
  | ShortcutItem
  | Remap
  | TextExpansion
  | Gesture
  | TouchpadGesture
  | HotCornerRule
  | WheelEdgeRule;
/** **有目录归属的**条目（鼠标类触发源没有目录：它们是全局触发源，不参与目录树）。 */
export type FolderEntry = ShortcutItem | Remap | TextExpansion;
