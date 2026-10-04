import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openFile, save as saveFile, ask } from "@tauri-apps/plugin-dialog";
import { installBrowserMock } from "./mock";

type OsOperation =
  | { op: "copy"; source: string; dest: string }
  | { op: "cut"; source: string; dest: string }
  | { op: "paste"; dest: string }
  | { op: "delete"; path: string }
  | { op: "new_file"; path: string }
  | { op: "new_folder"; path: string }
  | { op: "open_folder"; path: string }
  | { op: "zip"; source: string; dest: string }
  | { op: "unzip"; source: string; dest: string }
  | { op: "get_file_props"; path: string; var: string };
type Condition =
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
type AppOperation =
  | { op: "launch"; program: string; args: string[] }
  | { op: "close"; program: string }
  | { op: "status"; program: string; var: string; retries: number; interval_ms: number }
  | { op: "restart"; program: string; args: string[] };
type TextMode = "input" | "to_upper" | "to_lower";
type Shell = "cmd" | "powershell";
type Action =
  | { type: "text"; text: string; mode: TextMode; description?: string }
  | { type: "command"; shell: Shell; command: string; show_output: boolean; var: string; description?: string }
  | { type: "keys"; keys: string[]; description?: string }
  | { type: "pause_ms"; ms: number; description?: string }
  | { type: "os"; operation: OsOperation; description?: string }
  | { type: "app"; operation: AppOperation; description?: string }
  | { type: "open_url"; url: string; description?: string }
  | { type: "if"; condition: Condition; then: Action[]; otherwise: Action[]; description?: string }
  | { type: "script"; path: string; interpreter?: string | null; show_output: boolean; var: string; description?: string };
type Folder = { id: string; name: string; parent?: string | null };
type Layer = { id: string; name: string };
type ShortcutItem = {
  name?: string | null;
  description?: string | null;
  folder?: string | null;
  layer?: string | null;
  triggers: string[];
  actions: Action[];
  enabled: boolean;
};
type Remap = {
  from: string;
  to: string;
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
  enabled: boolean;
};
type TextExpansion = { trigger: string; replace: string; enabled: boolean };
type Settings = {
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
};
type Config = { folders: Folder[]; layers: Layer[]; shortcuts: ShortcutItem[]; remaps: Remap[]; expansions: TextExpansion[]; settings: Settings };
/** 输入状态指示载荷（后端 `StatusPayload`）：当前激活层 + 注入中的修饰键，见规划 7.3-⑬。 */
type StatusPayload = {
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
type HintsState = { visible: boolean; scale: number; opacity: number; x: number; y: number };
type Conflict = { severity: "error" | "warn"; message: string; name: string };
type Section = "shortcuts" | "remaps" | "expansions" | "messages" | "settings" | "help";
type CommandResult = {
  kind: string;
  label: string;
  command: string;
  trigger: string;
  name: string;
  stdout: string;
  stderr: string;
  exit_code: number | null;
  show_output: boolean;
  time: string;
};

// ---- 动作类型预设 ----
const ACTION_TYPES: { value: Action["type"]; label: string }[] = [
  { value: "text", label: "文本操作" },
  { value: "command", label: "执行命令" },
  { value: "script", label: "执行脚本" },
  { value: "app", label: "应用操作" },
  { value: "open_url", label: "打开网址" },
  { value: "os", label: "文件/目录操作" },
  { value: "keys", label: "按键组合" },
  { value: "pause_ms", label: "延迟" },
  { value: "if", label: "条件判断" },
];

// 动作类型图标（收起态展示用）。
const ACTION_ICONS: Record<Action["type"], string> = {
  text: "✏️",
  command: "💻",
  script: "📜",
  keys: "⌨️",
  pause_ms: "⏱️",
  os: "📁",
  app: "🚀",
  open_url: "🌐",
  if: "🔀",
};

const ACTION_TYPE_LABELS = Object.fromEntries(
  ACTION_TYPES.map((t) => [t.value, t.label]),
) as Record<Action["type"], string>;

function actionTypeLabel(t: Action["type"]): string {
  return ACTION_TYPE_LABELS[t];
}

// ---- 变量流向（可视化编排的数据流标签） ----
// 该动作会把结果写进哪个变量（没有则空）。
function actionVarWrites(a: Action): string[] {
  switch (a.type) {
    case "command":
    case "script":
      return a.var.trim() ? [a.var.trim()] : [];
    case "os":
      return a.operation.op === "get_file_props" && a.operation.var.trim()
        ? [a.operation.var.trim()]
        : [];
    case "app":
      return a.operation.op === "status" && a.operation.var.trim()
        ? [a.operation.var.trim()]
        : [];
    default:
      return [];
  }
}

// 文本扩展专用的动态片段不是变量，从引用标签里排除。
const VAR_DYNAMIC = new Set(["date", "time", "clipboard"]);

// 该动作引用了哪些 {变量}（只扫承载占位符的字符串字段，条件分支里的动作单独算，不并入）。
// 不能扫 JSON 序列化原文——嵌套对象的花括号会被误当成变量引用。
function actionVarReads(a: Action): string[] {
  const texts: string[] = [];
  switch (a.type) {
    case "text":
      texts.push(a.text);
      break;
    case "command":
      texts.push(a.command);
      break;
    case "script":
      texts.push(a.path, a.interpreter ?? "");
      break;
    case "os": {
      const o = a.operation;
      if ("source" in o) texts.push(o.source, o.dest);
      if ("path" in o) texts.push(o.path);
      if ("dest" in o) texts.push(o.dest);
      break;
    }
    case "app": {
      const o = a.operation;
      texts.push(o.program);
      if ("args" in o) texts.push(...o.args);
      break;
    }
    case "open_url":
      texts.push(a.url);
      break;
    case "if": {
      const c = a.condition;
      if ("path" in c) texts.push(c.path);
      if ("value" in c) texts.push(c.value);
      if ("app" in c) texts.push(c.app);
      if ("text" in c) texts.push(c.text);
      break;
    }
    default:
      break;
  }
  const out = new Set<string>();
  for (const t of texts) {
    for (const m of t.matchAll(/\{([^{}\s]+)\}/g)) {
      const name = m[1].split(".")[0];
      if (name && !VAR_DYNAMIC.has(name)) out.add(name);
    }
  }
  return [...out];
}

// ---- 条件判断的子条件 ----
const CONDITION_TYPES: { value: Condition["kind"]; label: string }[] = [
  { value: "exists", label: "路径存在" },
  { value: "not_exists", label: "路径不存在" },
  { value: "is_file", label: "路径是文件" },
  { value: "is_dir", label: "路径是目录" },
  { value: "modified_within", label: "修改时间在…分钟内" },
  { value: "equals", label: "变量字段等于" },
  { value: "not_equals", label: "变量字段不等于" },
  { value: "frontmost_app", label: "前台应用是" },
  { value: "not_frontmost_app", label: "前台应用不是" },
  { value: "window_title_contains", label: "窗口标题包含" },
  { value: "device_is", label: "设备是" },
];

function newCondition(kind: Condition["kind"]): Condition {
  switch (kind) {
    case "exists":
    case "not_exists":
    case "is_file":
    case "is_dir":
      return { kind, path: "" };
    case "modified_within":
      return { kind, path: "", minutes: 5 };
    case "equals":
    case "not_equals":
      return { kind, var: "", field: "", value: "" };
    case "frontmost_app":
    case "not_frontmost_app":
      return { kind, app: "" };
    case "window_title_contains":
      return { kind, text: "" };
    case "device_is":
      return { kind, id: "" };
  }
  return { kind: "exists", path: "" };
}

// ---- 操作系统动作的子操作 ----
const OS_OPS: { value: OsOperation["op"]; label: string }[] = [
  { value: "copy", label: "复制（文件/目录）" },
  { value: "cut", label: "剪切（移动）" },
  { value: "paste", label: "粘贴（上次复制/剪切来源）" },
  { value: "delete", label: "删除" },
  { value: "new_file", label: "新建文件" },
  { value: "new_folder", label: "新建文件夹" },
  { value: "open_folder", label: "打开文件夹" },
  { value: "zip", label: "压缩为 zip" },
  { value: "unzip", label: "解压 zip" },
  { value: "get_file_props", label: "获取文件属性" },
];

function newOsOp(op: OsOperation["op"]): OsOperation {
  switch (op) {
    case "copy":
      return { op, source: "", dest: "" };
    case "cut":
      return { op, source: "", dest: "" };
    case "paste":
      return { op, dest: "" };
    case "delete":
      return { op, path: "" };
    case "new_file":
      return { op, path: "" };
    case "new_folder":
      return { op, path: "" };
    case "open_folder":
      return { op, path: "" };
    case "zip":
    case "unzip":
      return { op, source: "", dest: "" };
    case "get_file_props":
      return { op, path: "", var: "file" };
  }
  return { op: "copy", source: "", dest: "" };
}

// ---- 应用动作的子操作 ----
const APP_OPS: { value: AppOperation["op"]; label: string }[] = [
  { value: "launch", label: "打开（启动）" },
  { value: "close", label: "关闭" },
  { value: "status", label: "查询运行状态" },
  { value: "restart", label: "重启" },
];

function newAppOp(op: AppOperation["op"]): AppOperation {
  switch (op) {
    case "launch":
      return { op, program: "", args: [] };
    case "close":
      return { op, program: "" };
    case "status":
      return { op, program: "", var: "app", retries: 0, interval_ms: 1000 };
    case "restart":
      return { op, program: "", args: [] };
  }
  return { op: "launch", program: "", args: [] };
}

function newAction(type: Action["type"]): Action {
  switch (type) {
    case "text":
      return { type: "text", text: "", mode: "input" };
    case "command":
      return { type: "command", shell: "cmd", command: "", show_output: false, var: "" };
    case "script":
      return { type: "script", path: "", interpreter: null, show_output: false, var: "" };
    case "keys":
      return { type: "keys", keys: [] };
    case "pause_ms":
      return { type: "pause_ms", ms: 0 };
    case "os":
      return { type: "os", operation: newOsOp("copy") };
    case "app":
      return { type: "app", operation: newAppOp("launch") };
    case "open_url":
      return { type: "open_url", url: "" };
    case "if":
      return { type: "if", condition: newCondition("exists"), then: [], otherwise: [] };
  }
  return { type: "text", text: "", mode: "input" };
}

function osHasContent(o: OsOperation): boolean {
  switch (o.op) {
    case "copy":
    case "cut":
    case "zip":
    case "unzip":
      return o.source.trim().length > 0 && o.dest.trim().length > 0;
    case "paste":
      return o.dest.trim().length > 0;
    case "delete":
    case "new_file":
    case "new_folder":
    case "open_folder":
    case "get_file_props":
      return o.path.trim().length > 0;
  }
  return false;
}

function condHasContent(c: Condition): boolean {
  switch (c.kind) {
    case "exists":
    case "not_exists":
    case "is_file":
    case "is_dir":
    case "modified_within":
      return c.path.trim().length > 0;
    case "equals":
    case "not_equals":
      return c.var.trim().length > 0;
    case "frontmost_app":
    case "not_frontmost_app":
      return c.app.trim().length > 0;
    case "window_title_contains":
      return c.text.trim().length > 0;
    case "device_is":
      return c.id.trim().length > 0;
  }
  return false;
}

function appHasContent(o: AppOperation): boolean {
  switch (o.op) {
    case "launch":
    case "close":
    case "status":
    case "restart":
      return o.program.trim().length > 0;
  }
  return false;
}

function actionHasContent(a: Action): boolean {
  switch (a.type) {
    case "text":
      return a.mode === "input" ? a.text.trim().length > 0 : true;
    case "command":
      return a.command.trim().length > 0;
    case "script":
      return a.path.trim().length > 0;
    case "keys":
      return a.keys.length > 0;
    case "pause_ms":
      return a.ms > 0;
    case "os":
      return osHasContent(a.operation);
    case "app":
      return appHasContent(a.operation);
    case "open_url":
      return a.url.trim().length > 0;
    case "if":
      return condHasContent(a.condition) || a.then.length > 0 || a.otherwise.length > 0;
  }
  return false;
}

function osSummary(o: OsOperation): string {
  switch (o.op) {
    case "copy":
      return `复制：${o.source} → ${o.dest}`;
    case "cut":
      return `剪切：${o.source} → ${o.dest}`;
    case "paste":
      return `粘贴到：${o.dest}`;
    case "delete":
      return `删除：${o.path}`;
    case "new_file":
      return `新建文件：${o.path}`;
    case "new_folder":
      return `新建文件夹：${o.path}`;
    case "open_folder":
      return `打开文件夹：${o.path}`;
    case "zip":
      return `压缩：${o.source} → ${o.dest}`;
    case "unzip":
      return `解压：${o.source} → ${o.dest}`;
    case "get_file_props":
      return `取属性：${o.path} → ${o.var}`;
  }
  return "";
}

function condSummary(c: Condition): string {
  switch (c.kind) {
    case "exists":
      return `路径存在：${c.path}`;
    case "not_exists":
      return `路径不存在：${c.path}`;
    case "is_file":
      return `是文件：${c.path}`;
    case "is_dir":
      return `是目录：${c.path}`;
    case "modified_within":
      return `修改时间 ${c.minutes} 分钟内：${c.path}`;
    case "equals":
      return `${c.var}${c.field ? "." + c.field : ""} == ${c.value}`;
    case "not_equals":
      return `${c.var}${c.field ? "." + c.field : ""} != ${c.value}`;
    case "frontmost_app":
      return `前台应用是：${c.app}`;
    case "not_frontmost_app":
      return `前台应用不是：${c.app}`;
    case "window_title_contains":
      return `窗口标题包含：${c.text}`;
    case "device_is":
      return `设备是：${c.id}`;
  }
  return "";
}

function appSummary(o: AppOperation): string {
  switch (o.op) {
    case "launch":
      return `打开：${o.program}`;
    case "close":
      return `关闭：${o.program}`;
    case "status":
      return `查询状态：${o.program} → ${o.var}${o.retries ? `（重试 ${o.retries} 次 / ${o.interval_ms}ms）` : ""}`;
    case "restart":
      return `重启：${o.program}`;
  }
  return "";
}

function actionSummary(a: Action): string {
  switch (a.type) {
    case "text":
      if (a.mode === "to_upper") return "文本转大写（选中/剪贴板）";
      if (a.mode === "to_lower") return "文本转小写（选中/剪贴板）";
      return a.text ? `输入文本：${a.text}` : "输入文本";
    case "command": {
      const v = a.var ? ` → ${a.var}` : "";
      return `${a.shell === "cmd" ? "CMD" : "PowerShell"}：${a.command}${v}`;
    }
    case "script": {
      const v = a.var ? ` → ${a.var}` : "";
      const target = a.interpreter ? `${a.interpreter} ${a.path}` : a.path;
      return `脚本：${target}${v}`;
    }
    case "keys":
      return `按键：${a.keys.join("+")}`;
    case "pause_ms":
      return `延迟 ${a.ms}ms`;
    case "os":
      return osSummary(a.operation);
    case "app":
      return appSummary(a.operation);
    case "open_url":
      return a.url ? `打开网址：${a.url}` : "打开网址";
    case "if":
      return `如果 ${condSummary(a.condition)}（${a.then.length} 个动作${a.otherwise.length ? `，否则 ${a.otherwise.length} 个` : ""}）`;
  }
  return "";
}

// 收起态显示的描述：优先用用户填写的描述，否则回退到自动生成的动作摘要。
function actionDescription(a: Action): string {
  const d = a.description?.trim();
  return d ? d : actionSummary(a);
}

// ---- 状态 ----
let cfg: Config = {
  folders: [],
  layers: [],
  shortcuts: [],
  remaps: [],
  expansions: [],
  settings: {
    autostart: false,
    paused: false,
    action_timeout_ms: 30_000,
    sequence_timeout_ms: 1000,
    chord_timeout_ms: 1000,
    show_status_hud: true,
    hints: { visible: false, scale: 100, opacity: 92 },
  },
};
let section: Section = "shortcuts";
let selected: number | null = null; // 当前列表中的选中下标
let draftNew = false; // 当前编辑是否为“新增”未保存项
let draft: ShortcutItem | null = null; // 快捷键编辑草稿（隔离，保存才写回 cfg）
let remapDraft: Remap | null = null; // 改键编辑草稿
let expansionDraft: TextExpansion | null = null; // 文本扩展编辑草稿
// 草稿基线：打开编辑页（或上次保存）那一刻的草稿签名。当前签名与之不等 = 有未保存改动。
// 见「未保存改动守卫」一节。
let draftBaseline: string | null = null;
let recording = false;
let conflicts: Conflict[] = [];
let search = "";
let messages: CommandResult[] = []; // 消息中心（最新在前）
let msgIndex = 0; // 当前选中的结果 tab
let unread = false; // 未读红点
let conflictBubbleDismissed = false; // 快捷键列表里的冲突气泡是否已被用户消除
let msgView: "results" | "conflicts" = "results"; // 消息页标签：命令结果 / 冲突
let clipboard: { kind: "shortcut"; srcIndex: number; cut: boolean } | null = null; // 剪切/复制板
// 列表筛选（7.3-⑳）：与 search 叠加生效，「有冲突」在文本扩展分区不出现（扩展不参与冲突检测）。
let listFilter = { enabled: false, conflicted: false };
// 批量启停的选择集：存对象引用而非下标（增删/重排后引用仍指对那条）；切分区清空。
let picked = new Set<object>();
let editingFolderId: string | null = null; // 正在行内改名的目录 id
let editingLayerId: string | null = null; // 正在行内改名的层 id

// ---- 拖拽排序/移动 ----
type DragPayload =
  | { kind: "shortcut"; idx: number }
  | { kind: "folder"; id: string }
  | { kind: "remap"; idx: number }
  | { kind: "expansion"; idx: number };
type DropZone =
  | { kind: "root" }
  | { kind: "folder"; folderId: string; pos: "before" | "into" | "after" }
  | { kind: "row"; idx: number; pos: "before" | "after" };
let dragPayload: DragPayload | null = null; // 拖拽源
let dragOverEl: HTMLElement | null = null; // 当前高亮目标（用于清 class）
let collapsedFolders = new Set<string>(); // 折叠的目录 id（内存态，重启恢复全展开）
let collapsedActions = new WeakSet<Action>(); // 收起态的动作（按对象引用，重绘/重排后仍保持）

// pointer events 拖拽状态（不依赖 WebView2 的 HTML5 DnD，避免其兼容性抖动）。
interface DragState {
  payload: DragPayload;
  startX: number;
  startY: number;
  active: boolean; // 越过启动阈值后才算真正在拖拽
  sourceEl: HTMLElement;
}
let dragState: DragState | null = null;
let suppressClick = false; // 拖拽结束后抑制一次 click，避免误触「打开详情/折叠」

// 动作拖拽（详情编辑器内）：改变动作顺序或逻辑纵深（拖进/拖出条件分支）。
interface ActionDragState {
  list: Action[]; // 源动作所在列表（数组引用）
  index: number; // 源动作在列表中的下标
  startX: number;
  startY: number;
  active: boolean; // 越过启动阈值后才算真正在拖拽
  sourceEl: HTMLElement;
}
let actionDrag: ActionDragState | null = null;

// ---- 键名选项（改键下拉用），对齐 kada-core 的 `Key::ALL` / key_name ----
// 必须是**显式字面量数组**（不要用 spread/循环拼）：`crates/kada-core` 的测试会按字面量
// 解析这里，跟 `Key::ALL` 逐一比对——Rust 新增按键而这里漏了，测试就会红。
const KEY_OPTIONS = [
  "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
  "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
  "0", "1", "2", "3", "4", "5", "6", "7", "8", "9",
  "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
  "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
  ",", ".", "/", "\\", ";", "\"", "`", "-", "=", "[", "]",
  "Enter", "Esc", "Tab", "Space", "Backspace", "Delete", "Insert", "CapsLock",
  "Shift", "Ctrl", "Alt", "Meta",
  "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
  "MediaPlayPause", "MediaPrev", "MediaNext", "VolumeMute", "VolumeDown", "VolumeUp",
  "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4",
  "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
  "NumpadAdd", "NumpadSubtract", "NumpadMultiply", "NumpadDivide",
  "NumpadDecimal", "NumpadEnter", "NumLock",
  "MouseMiddle", "MouseBack", "MouseForward",
];

// ---- IPC ----
async function load() {
  cfg = await invoke<Config>("get_config");
  messages = await invoke<CommandResult[]>("get_command_results");
  unread = await invoke<boolean>("get_unread");
  updateStatus = await invoke<UpdateStatus>("get_update_status");
  await refreshConflicts();
  render();
  syncSettings();
  syncUpdate();
}

// 落盘整份配置。返回 true = 真的写进去了——未保存守卫靠它决定「保存并离开」要不要放行，
// 失败还放行等于把用户的改动悄悄丢掉。
async function save(): Promise<boolean> {
  cfg.shortcuts = cfg.shortcuts.filter(
    (s) => s.triggers.length > 0 || !!s.name || s.actions.some(actionHasContent),
  );
  cfg.expansions = cfg.expansions.filter((e) => e.trigger.trim().length > 0);
  let ignored: string[] = [];
  try {
    ignored = await invoke<string[]>("set_config", { config: cfg });
  } catch (e) {
    toast(`保存失败: ${e}`);
    render();
    return false;
  }
  await refreshConflicts();
  const errs = conflicts.filter((c) => c.severity === "error").length;
  const warns = conflicts.filter((c) => c.severity === "warn").length;
  const notes: string[] = [];
  if (ignored.length) {
    // 只报数量的话，被丢掉的条目（比如「长按进入层」指向的层已不存在，整条改键作废）用户
    // 根本查不出来——带上第一条原因，长文案由 .toast 的 max-width 折行。
    const first = ignored[0];
    notes.push(
      ignored.length === 1
        ? `已忽略 1 处无效配置：${first}`
        : `已忽略 ${ignored.length} 处无效配置（例：${first}）`,
    );
  }
  if (errs || warns) notes.push(`${errs} 处冲突${warns ? `、${warns} 处遮蔽提示` : ""}`);
  if (notes.length) toast(`已保存：${notes.join("；")}`);
  else toast("已保存");
  render();
  return true;
}

// 冲突检测用到的配置：编辑时把草稿合并进 cfg，让冲突在录入触发键的当下就实时显示，
// 而不是等保存后才知道。
function currentConfigForConflicts(): Config {
  if (section === "shortcuts" && draft) {
    const d = draft;
    const shortcuts = draftNew
      ? [d, ...cfg.shortcuts]
      : cfg.shortcuts.map((s, i) => (i === selected ? d : s));
    return { ...cfg, shortcuts };
  }
  return cfg;
}

async function refreshConflicts() {
  try {
    const next = await invoke<Conflict[]>("get_conflicts", {
      config: currentConfigForConflicts(),
    });
    // 冲突集合变化时，重新弹出气泡（用户消除过旧冲突、又出现新冲突时能再次被提醒）。
    if (JSON.stringify(next) !== JSON.stringify(conflicts)) conflictBubbleDismissed = false;
    conflicts = next;
  } catch {
    conflicts = [];
  }
}

function toast(msg: string) {
  const el = document.getElementById("save-status")!;
  el.textContent = msg;
  setTimeout(() => (el.textContent = ""), 2000);
}

// ---- 组合键录入 ----
const CODE_TABLE: Record<string, string> = {
  Enter: "Enter", Escape: "Esc", Tab: "Tab", Space: "Space",
  Backspace: "Backspace", Delete: "Delete", Insert: "Insert", CapsLock: "CapsLock",
  Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
  NumLock: "NumLock",
  Numpad0: "Numpad0", Numpad1: "Numpad1", Numpad2: "Numpad2",
  Numpad3: "Numpad3", Numpad4: "Numpad4", Numpad5: "Numpad5",
  Numpad6: "Numpad6", Numpad7: "Numpad7", Numpad8: "Numpad8", Numpad9: "Numpad9",
  NumpadAdd: "NumpadAdd", NumpadSubtract: "NumpadSubtract",
  NumpadMultiply: "NumpadMultiply", NumpadDivide: "NumpadDivide",
  NumpadDecimal: "NumpadDecimal", NumpadEnter: "NumpadEnter",
  MediaPlayPause: "MediaPlayPause", MediaTrackPrevious: "MediaPrev",
  MediaTrackNext: "MediaNext", AudioVolumeMute: "VolumeMute",
  AudioVolumeDown: "VolumeDown", AudioVolumeUp: "VolumeUp",
};
const CODE_PUNCT: Record<string, string> = {
  Comma: ",", Period: ".", Slash: "/", Backslash: "\\", Semicolon: ";",
  Quote: "\"", Backquote: "`", Minus: "-", Equal: "=",
  BracketLeft: "[", BracketRight: "]",
};

function codeToKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
  return CODE_PUNCT[code] ?? CODE_TABLE[code] ?? null;
}

function comboFromEvent(e: KeyboardEvent): string | null {
  const main = codeToKey(e.code);
  if (!main) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Ctrl");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");
  parts.push(main);
  return parts.join("+");
}

// 录入捕获（组合键/序列/和弦）进行中的解除函数。捕获开始时暂停快捷键（避免自己触发自己），
// 结束时解除；一旦捕获被中断（窗口失焦、详情被关、收进托盘）也必须解除——否则 Rust 侧会一直
// 处于暂停态，表现为「快捷键、改键、文本扩展全都没反应」而界面上看不出任何原因
//（Rust 侧另有 60 秒租约兜底，那是最后一道保险）。
let activeCaptureCleanup: (() => void) | null = null;

/** 中断当前录入捕获并解除暂停；无捕获时是空操作。 */
function stopCapture() {
  const cleanup = activeCaptureCleanup;
  activeCaptureCleanup = null;
  cleanup?.();
}

window.addEventListener("blur", stopCapture);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) stopCapture();
});

function startCapture(onCommit: (combo: string) => void, btn: HTMLButtonElement) {
  stopCapture();
  btn.disabled = true;
  btn.textContent = "请按组合键…（Esc 取消）";
  void invoke("set_paused", { paused: true });
  const handler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") return finish();
    const combo = comboFromEvent(e);
    if (combo) {
      onCommit(combo);
      finish();
    }
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", handler, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入组合键";
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", handler, true);
}

// 键序列录入：逐键累积 comboFromEvent 出的组合（空格拼接实时预览），Enter 提交、Esc 取消。
// 序列至少 2 步（单步就是普通组合键，走「录入组合键」）。
function startSequenceCapture(onCommit: (seq: string) => void, btn: HTMLButtonElement) {
  stopCapture();
  const steps: string[] = [];
  btn.disabled = true;
  void invoke("set_paused", { paused: true });
  const update = () => {
    btn.textContent = steps.length ? `序列：${steps.join(" ")}（Enter 提交，Esc 取消）` : "请按键序列…（Enter 提交，Esc 取消）";
  };
  const handler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Enter") {
      if (steps.length >= 2) onCommit(steps.join(" "));
      return finish();
    }
    if (e.code === "Escape") return finish();
    const combo = comboFromEvent(e);
    if (combo) {
      steps.push(combo);
      update();
    }
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", handler, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入序列";
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", handler, true);
  update();
}

// 和弦录入：按住多个键，松开时提交（成员用 & 连接、按字典序，至少 2 键）。Esc 取消。
// 成员是普通键（修饰键 / CapsLock 不当成员）；按住修饰键录入时把它写在**第一个**成员上
// （`Ctrl+F&J` = 按住 Ctrl 的同时把 F、J 一起按住）——后端对成员修饰要求的判定只看「凑齐
// 那一刻按着没有」，写在第一个成员上等价于要求全程按着，显示也最省事。
function startChordCapture(onCommit: (chord: string) => void, btn: HTMLButtonElement) {
  stopCapture();
  const held: string[] = [];
  let mods = "";
  btn.disabled = true;
  void invoke("set_paused", { paused: true });
  // 提交文本：成员按字典序，修饰键写在第一个成员上（见上）。
  const chordText = () => {
    const members = held.map((m, i) => (i === 0 && mods ? `${mods}+${m}` : m));
    return members.sort().join("&");
  };
  const update = () => {
    btn.textContent = held.length
      ? `和弦：${chordText()}（松开提交，Esc 取消）`
      : "请同时按住多个键…（松开提交，Esc 取消）";
  };
  const EXCLUDED = new Set([
    "ControlLeft", "ControlRight", "AltLeft", "AltRight",
    "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight", "CapsLock",
  ]);
  const onKey = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (e.code === "Escape") return finish();
    if (e.type === "keyup") {
      // 松开任一键即提交：至少 2 键才成立，否则视为取消。
      if (held.length >= 2) onCommit(chordText());
      finish();
      return;
    }
    // keydown（忽略自动重复）。修饰键不当成员：按下**第一个成员**时按着的修饰键会记下来，
    // 提交时写在那个成员上（后端按「凑齐那一刻按着没有」判定，等价于要求全程按着）。
    if (e.repeat) return;
    if (held.length === 0) mods = currentMods(e);
    if (EXCLUDED.has(e.code)) return;
    const k = codeToKey(e.code);
    if (k && !held.includes(k)) {
      held.push(k);
      update();
    }
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("keyup", onKey, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入和弦";
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("keyup", onKey, true);
  update();
}

// 当前按住的修饰键（与后端键名一致：Ctrl / Alt / Shift / Meta）。
function currentMods(e: KeyboardEvent): string {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Meta");
  return mods.join("+");
}

// ---- 渲染 ----
function el(tag: string, cls?: string, text?: string): HTMLElement {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined) n.textContent = text;
  return n;
}

// 深拷贝（配置是纯 JSON 数据，用结构化克隆隔离编辑草稿，避免误写回原配置）。
function deepClone<T>(x: T): T {
  return structuredClone(x);
}

// 圆圈问号：hover / 键盘聚焦显示说明（用于解释「会写入变量」的动作，如取文件属性/查询应用状态）。
function helpIcon(tip: string): HTMLElement {
  const s = el("span", "help", "?");
  s.setAttribute("data-tip", tip);
  s.tabIndex = 0;
  return s;
}

// 内联可见的变量引用说明：把「有哪些字段、怎么引用」直接列在输入框旁，替代 hover 才显示的问号。
function varUsage(tip: string): HTMLElement {
  return el("div", "var-usage", tip);
}

function render() {
  renderListPane();
  renderConflictBubble();
  renderDetail();
  renderMessages();
  if (section === "settings") renderLayers();
}

function renderListPane() {
  // 重绘会整棵重建列表行，焦点所在行先记下来、画完还回去——否则键盘用户
  // 一按开关 / 回车，焦点就掉回 body，Tab 又得从头数（行内的开关/按钮失焦后
  // 统一还给行本身，够用且不用记行内哪个子元素）。
  const act = document.activeElement as HTMLElement | null;
  const actRow = act?.closest(".row, .folder-row") as HTMLElement | null;
  const actSel =
    actRow?.dataset.idx != null
      ? `[data-idx="${actRow.dataset.idx}"]`
      : actRow?.dataset.folderId != null
        ? `[data-folder-id="${actRow.dataset.folderId}"]`
        : null;
  document.getElementById("list-pane")!.classList.toggle(
    "hidden",
    section === "settings" || section === "messages" || section === "help",
  );
  document.getElementById("add-folder-btn")!.classList.toggle("hidden", section !== "shortcuts");
  // 筛选/批量条状态（设置/消息页整个列表栏都藏了，不必分节处理）。
  // 选择集按当前分区数组剔除陈旧引用：条目被删后不该还留在集合里被批量操作。
  const sectionItems: object[] =
    section === "remaps" ? cfg.remaps : section === "expansions" ? cfg.expansions : cfg.shortcuts;
  for (const o of [...picked]) if (!sectionItems.includes(o)) picked.delete(o);
  document.getElementById("filter-conflicted")!.classList.toggle("hidden", section === "expansions");
  for (const [id, on] of [
    ["filter-enabled", listFilter.enabled],
    ["filter-conflicted", listFilter.conflicted],
  ] as const) {
    const b = document.getElementById(id)!;
    b.classList.toggle("on", on);
    b.setAttribute("aria-pressed", String(on));
  }
  document.getElementById("batch-count")!.textContent = String(picked.size);
  for (const id of ["batch-enable", "batch-disable", "batch-clear"]) {
    document.getElementById(id)!.toggleAttribute("disabled", picked.size === 0);
  }
  document
    .getElementById("paste-btn")!
    .classList.toggle("hidden", !clipboard || section !== "shortcuts");
  const title = document.getElementById("list-title")!;
  const shortcutList = document.getElementById("shortcut-list")!;
  const remapList = document.getElementById("remap-list")!;
  const expansionList = document.getElementById("expansion-list")!;
  if (section === "shortcuts") {
    title.textContent = "快捷键";
    shortcutList.classList.remove("hidden");
    remapList.classList.add("hidden");
    expansionList.classList.add("hidden");
    renderShortcuts();
  } else if (section === "remaps") {
    title.textContent = "改键";
    shortcutList.classList.add("hidden");
    remapList.classList.remove("hidden");
    expansionList.classList.add("hidden");
    renderRemaps();
  } else if (section === "expansions") {
    title.textContent = "文本扩展";
    shortcutList.classList.add("hidden");
    remapList.classList.add("hidden");
    expansionList.classList.remove("hidden");
    renderExpansions();
  }
  if (actRow && actSel) {
    document
      .querySelector<HTMLElement>(`#list-pane .row${actSel}, #list-pane .folder-row${actSel}`)
      ?.focus();
  }
}

// 条目是否卷入冲突：冲突文案把涉及的触发键/来源/层名都用「」引起来，拿条目的键去文案里找
// （后端 Conflict 只带名称，无名条目靠不上 name；UI 与后端同包发布，文案格式一体演化）。
function entryConflicted(keys: (string | null | undefined)[]): boolean {
  if (!conflicts.length) return false;
  return conflicts.some((c) =>
    keys.some(
      (k) =>
        k &&
        // 常规形态「键」；改键冲突的文案是「键 → 目标」合在一对引号里，得按前缀认。
        (c.message.includes(`「${k}」`) || c.message.includes(`「${k} →`)),
    ),
  );
}

// 三个分区各自的「当前是否可见」= 搜索 ∧ 筛选。渲染循环与「全选」共用，避免两份口径漂移。
function shortcutVisible(s: ShortcutItem): boolean {
  const q = search.trim().toLowerCase();
  if (q) {
    const hay = `${s.triggers.join(" ")} ${s.name ?? ""} ${s.description ?? ""} ${s.actions
      .map(actionSummary)
      .join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (listFilter.enabled && !s.enabled) return false;
  if (listFilter.conflicted && !entryConflicted([...s.triggers, s.layer ? layerName(s.layer) : null]))
    return false;
  return true;
}

function remapVisible(r: Remap): boolean {
  const q = search.trim().toLowerCase();
  if (q) {
    const hay = `${r.from} ${remapSummary(r)} ${layerName(r.hold_layer)} ${layerName(r.lock_layer)}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (listFilter.enabled && !r.enabled) return false;
  if (listFilter.conflicted && !entryConflicted([r.from, r.layer ? layerName(r.layer) : null]))
    return false;
  return true;
}

function expansionVisible(e: TextExpansion): boolean {
  const q = search.trim().toLowerCase();
  if (q && !`${e.trigger} ${e.replace}`.toLowerCase().includes(q)) return false;
  if (listFilter.enabled && !e.enabled) return false;
  return true;
}

function renderShortcuts() {
  const list = document.getElementById("shortcut-list")!;
  list.replaceChildren();

  const renderShortcutsIn = (folderId: string | null, depth: number) => {
    cfg.shortcuts.forEach((s, i) => {
      if ((s.folder ?? null) === folderId && shortcutVisible(s)) {
        list.append(shortcutRow(s, i, depth));
      }
    });
  };

  // 搜索或筛选期间强制展开目录：收着的目录会把命中的条目藏起来（筛选形同没生效）。
  const searching = search.trim() !== "" || listFilter.enabled || listFilter.conflicted;
  const renderFolders = (parentId: string | null, depth: number) => {
    for (const f of cfg.folders.filter((x) => (x.parent ?? null) === parentId)) {
      const collapsed = !searching && collapsedFolders.has(f.id);
      list.append(folderRow(f, depth, collapsed));
      if (collapsed) continue;
      renderShortcutsIn(f.id, depth + 1);
      renderFolders(f.id, depth + 1);
    }
  };

  // 根级未分组快捷键在前，随后是根级目录（递归展开）。
  renderShortcutsIn(null, 0);
  renderFolders(null, 0);
}

// 键盘可达：列表行可 Tab 聚焦、回车/空格激活（与点击同一路径）。
// role=option 让读屏把它当列表条目；aria-expanded 表达目录折叠态。
function makeRow(
  li: HTMLElement,
  run: () => void,
  extra?: { selected?: boolean; expanded?: boolean },
) {
  li.tabIndex = 0;
  li.setAttribute("role", "option");
  if (extra?.selected !== undefined) li.setAttribute("aria-selected", String(extra.selected));
  if (extra?.expanded !== undefined) li.setAttribute("aria-expanded", String(extra.expanded));
  li.addEventListener("keydown", (e) => {
    if (e.target !== li) return; // 行内开关/菜单/重命名框各有自己的键盘行为
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      run();
    }
  });
}

function shortcutRow(s: ShortcutItem, idx: number, depth: number): HTMLElement {
  const li = el("li", "row" + (selected === idx ? " selected" : "") + (picked.has(s) ? " picked" : ""));
  li.dataset.idx = String(idx);
  makeRow(li, () => void requestOpenDetail(idx), { selected: selected === idx });
  // 缩进用 margin 而非 padding：整行盒子随层级右移，层级关系和边框范围都对齐。
  li.style.marginLeft = `${depth * 20}px`;
  attachDragStart(li, { kind: "shortcut", idx });
  const on = el("input", "toggle") as HTMLInputElement;
  on.type = "checkbox";
  on.checked = s.enabled;
  on.title = "启用 / 停用此快捷键";
  on.addEventListener("change", (e) => {
    e.stopPropagation();
    s.enabled = on.checked;
    void save();
  });

  // 列表行只呈现「组合键 + 名称」；描述与动作摘要移入详情页。
  const trig = el("span", "row-trigger", s.triggers.join(" ") || "未设触发键");
  if (s.triggers.length === 0) trig.classList.add("row-trigger-empty");
  const name = el("span", "row-name", s.name || "（未命名）");

  const more = moreButton((btn) => showRowMenu(btn, { kind: "shortcut", idx }));
  li.append(on, trig, name, more);
  li.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest("input, button")) return; // 开关/菜单不打开详情
    if (suppressClick) return;
    if (e.ctrlKey || e.metaKey) {
      togglePicked(s);
      return;
    }
    void requestOpenDetail(idx);
  });
  return li;
}

function folderRow(f: Folder, depth: number, collapsed: boolean): HTMLElement {
  const li = el("li", "folder-row");
  li.dataset.folderId = f.id;
  li.style.marginLeft = `${depth * 20}px`;
  makeRow(
    li,
    () => {
      // 与点击同一路径：无子内容或重命名中不切换（没东西可展开）。
      if (folderHasChildren(f.id) && editingFolderId !== f.id) toggleFolder(f.id);
    },
    { expanded: !collapsed },
  );
  if (editingFolderId !== f.id) {
    attachDragStart(li, { kind: "folder", id: f.id });
  }
  const hasChildren = folderHasChildren(f.id);
  li.append(el("span", "folder-arrow", hasChildren ? (collapsed ? "▸" : "▾") : ""));
  li.append(el("span", "folder-ico", "📁"));
  if (editingFolderId === f.id) {
    const inp = el("input", "folder-input") as HTMLInputElement;
    inp.value = f.name;
    inp.placeholder = "目录名";
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const v = inp.value.trim();
      if (v) {
        f.name = v;
      } else {
        cfg.folders = cfg.folders.filter((x) => x.id !== f.id);
      }
      editingFolderId = null;
      void save();
      render();
    };
    inp.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        commit();
      } else if (e.key === "Escape") {
        done = true;
        editingFolderId = null;
        render();
      }
    });
    inp.addEventListener("blur", commit);
    li.append(inp);
    setTimeout(() => inp.focus(), 0);
  } else {
    li.append(el("span", "folder-name", f.name));
  }
  const more = moreButton((btn) => showRowMenu(btn, { kind: "folder", id: f.id }));
  li.append(more);

  // 点击目录行切换展开/收缩（重命名态与三点菜单/输入框除外）。
  if (hasChildren && editingFolderId !== f.id) {
    li.addEventListener("click", (e) => {
      if (suppressClick) return;
      if ((e.target as HTMLElement).closest("button, input")) return;
      toggleFolder(f.id);
    });
  }

  return li;
}

function moreButton(onClick: (btn: HTMLButtonElement) => void): HTMLButtonElement {
  const b = el("button", "row-more", "⋯") as HTMLButtonElement;
  b.title = "更多";
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick(b);
  });
  return b;
}

function showRowMenu(
  anchor: HTMLElement,
  target:
    | { kind: "shortcut"; idx: number }
    | { kind: "folder"; id: string }
    | { kind: "remap"; idx: number }
    | { kind: "expansion"; idx: number },
) {
  hideRowMenu();
  const menu = document.getElementById("row-menu")!;
  menu.replaceChildren();
  const items: { label: string; run: () => void }[] = [];
  if (target.kind === "shortcut") {
    items.push(
      { label: "复制一份", run: () => duplicateShortcut(target.idx) },
      { label: "剪切", run: () => cutShortcut(target.idx) },
      { label: "复制", run: () => copyShortcut(target.idx) },
      { label: "删除", run: () => void deleteShortcutAt(target.idx) },
    );
  } else if (target.kind === "remap") {
    items.push(
      { label: "复制一份", run: () => duplicateRemap(target.idx) },
      { label: "删除", run: () => void deleteRemapAt(target.idx) },
    );
  } else if (target.kind === "expansion") {
    items.push(
      { label: "复制一份", run: () => duplicateExpansion(target.idx) },
      { label: "删除", run: () => void deleteExpansionAt(target.idx) },
    );
  } else {
    items.push(
      { label: "新建子目录", run: () => addFolder(target.id) },
      { label: "重命名", run: () => renameFolder(target.id) },
      { label: "粘贴到该目录", run: () => pasteIntoFolder(target.id) },
      { label: "删除", run: () => void deleteFolder(target.id) },
    );
  }
  for (const it of items) {
    const b = el("button", "menu-item", it.label) as HTMLButtonElement;
    b.setAttribute("role", "menuitem");
    b.addEventListener("click", () => {
      hideRowMenu();
      it.run();
    });
    menu.append(b);
  }
  // 菜单紧贴三点按钮：先显示再测宽，右缘对齐按钮右缘（贴近按钮、避免右溢出）；
  // 底部放不下则向上翻转，避免列表末尾的行菜单被裁到页面底下看不到。
  menu.classList.remove("hidden");
  const rect = anchor.getBoundingClientRect();
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  menu.style.left = `${Math.min(Math.max(8, rect.right - w), Math.max(8, vw - w - 8))}px`;
  let top = rect.bottom + 4;
  if (top + h > vh - 4) {
    top = Math.max(4, rect.top - h - 4);
  }
  menu.style.top = `${top}px`;
  // 键盘：打开即聚焦第一项；Esc 关菜单并把焦点还给三点按钮，Tab 走人就收摊
  // （onkeydown 赋值而非 addEventListener：菜单是同一个元素反复复用，防监听器叠罗汉）。
  menu.querySelector<HTMLElement>(".menu-item")?.focus();
  menu.onkeydown = (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      hideRowMenu();
      anchor.focus();
    } else if (ev.key === "Tab") {
      hideRowMenu();
    }
  };
  setTimeout(() => document.addEventListener("click", hideRowMenu, { once: true }), 0);
}

function hideRowMenu() {
  document.getElementById("row-menu")!.classList.add("hidden");
}

// 目录是否有子内容（子目录或条目），用于决定是否显示展开箭头。
function folderHasChildren(id: string): boolean {
  return (
    cfg.folders.some((f) => f.parent === id) || cfg.shortcuts.some((s) => s.folder === id)
  );
}

// 切换目录展开/收缩（仅 UI 状态，不落盘）。
function toggleFolder(id: string) {
  if (collapsedFolders.has(id)) collapsedFolders.delete(id);
  else collapsedFolders.add(id);
  renderListPane();
}

function newFolderId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `f-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function addFolder(parentId: string | null) {
  const f: Folder = { id: newFolderId(), name: "", parent: parentId };
  cfg.folders.push(f);
  editingFolderId = f.id;
  renderListPane();
}

function renameFolder(id: string) {
  editingFolderId = id;
  renderListPane();
}

// 确认框统一入口。ask() 走 Tauri 的 dialog 插件（需 capabilities 放行 `dialog:allow-message`，
// 光放行 open/save 不够——ask 调的是插件的 message 命令）；被 ACL 拒绝或插件异常时它会 reject，
// 而 reject 直接冒出去只会变成一条没人看的 unhandled rejection，用户看到的就是「点了删除没反应」。
// 这里兜住并把原因摆到提示条上。
async function confirmAsk(
  message: string,
  opts: Parameters<typeof ask>[1],
): Promise<boolean> {
  try {
    return await ask(message, opts);
  } catch (e) {
    toast(`确认框打开失败：${e}`);
    return false;
  }
}

// 删除单条内容的统一确认。原先只有「删除目录 / 删除层」问一句，同为破坏性操作的单条删除
// （快捷键 / 改键 / 文本扩展）却是点了就没了——同类实体的安全级别不该不一致。
// 未落盘的新增项不弹：删除它等同「取消」，没有任何已保存的内容会丢。
async function confirmDelete(kind: string, name: string, discardsDraft: boolean): Promise<boolean> {
  const dirty = discardsDraft ? "该条还有未保存的改动，会一并丢弃。" : "";
  return await confirmAsk(`删除${kind}「${name}」？${dirty}删除后应用内无法撤销。`, {
    kind: "warning",
    title: `删除${kind}`,
  });
}

// 快捷键的展示名（无名时退回触发键；列表行显示的是「触发键 + 名称」，确认框要对得上号）。
function shortcutLabel(s: ShortcutItem): string {
  return s.name?.trim() || s.triggers.join(" / ") || "未命名";
}

async function deleteFolder(id: string) {
  const target = cfg.folders.find((f) => f.id === id);
  if (!target) return;
  const ids = collectDescendantIds(id);
  const n = cfg.shortcuts.filter((s) => s.folder && ids.has(s.folder)).length;
  const ok = await confirmAsk(
    `删除目录「${target.name}」${n ? `及其中的 ${n} 条快捷键` : "及其所有内容"}？删除后无法撤销。`,
    { title: "删除目录", kind: "warning" },
  );
  if (!ok) return;
  cfg.folders = cfg.folders.filter((f) => !ids.has(f.id));
  cfg.shortcuts = cfg.shortcuts.filter((s) => !(s.folder && ids.has(s.folder)));
  for (const fid of ids) collapsedFolders.delete(fid);
  discardDraft();
  draftNew = false;
  selected = null;
  void save();
}

function collectDescendantIds(id: string): Set<string> {
  const ids = new Set<string>([id]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const f of cfg.folders) {
      if (f.parent && ids.has(f.parent) && !ids.has(f.id)) {
        ids.add(f.id);
        changed = true;
      }
    }
  }
  return ids;
}

function copyShortcut(idx: number) {
  clipboard = { kind: "shortcut", srcIndex: idx, cut: false };
  renderListPane(); // 粘贴按钮随剪贴板出现
  toast("已复制，可用「粘贴」插入");
}

function cutShortcut(idx: number) {
  clipboard = { kind: "shortcut", srcIndex: idx, cut: true };
  renderListPane();
  toast("已剪切，可用「粘贴」移动");
}

// 原位复制一份（不走剪贴板，省去「先复制再找地方粘」两步）：插在源条目后一条。
function duplicateShortcut(idx: number) {
  const src = cfg.shortcuts[idx];
  if (!src) return;
  cfg.shortcuts.splice(idx + 1, 0, deepClone(src));
  void save();
}

function duplicateRemap(idx: number) {
  const src = cfg.remaps[idx];
  if (!src) return;
  cfg.remaps.splice(idx + 1, 0, deepClone(src));
  void save();
}

function duplicateExpansion(idx: number) {
  const src = cfg.expansions[idx];
  if (!src) return;
  cfg.expansions.splice(idx + 1, 0, deepClone(src));
  void save();
}

// 删除确认 + 选中下标修正（与 deleteShortcutAt 同一套口径；两个分区各一份是因为条目类型不同）。
async function deleteRemapAt(idx: number) {
  const target = cfg.remaps[idx];
  if (!target) return;
  if (
    !(await confirmDelete("改键", `${target.from} → ${remapSummary(target)}`, selected === idx && isDraftDirty()))
  ) {
    return;
  }
  cfg.remaps.splice(idx, 1);
  if (selected === idx) {
    discardDraft();
    draftNew = false;
    selected = null;
  } else if (selected !== null && selected > idx) {
    selected -= 1;
  }
  void save();
}

async function deleteExpansionAt(idx: number) {
  const target = cfg.expansions[idx];
  if (!target) return;
  if (!(await confirmDelete("文本扩展", target.trigger, selected === idx && isDraftDirty()))) return;
  cfg.expansions.splice(idx, 1);
  if (selected === idx) {
    discardDraft();
    draftNew = false;
    selected = null;
  } else if (selected !== null && selected > idx) {
    selected -= 1;
  }
  void save();
}

function pasteIntoFolder(folderId: string | null) {
  if (!clipboard) return toast("剪贴板为空");
  const src = cfg.shortcuts[clipboard.srcIndex];
  if (!src) return;
  const copy = deepClone(src);
  copy.folder = folderId;
  cfg.shortcuts.push(copy);
  if (clipboard.cut) {
    const at = cfg.shortcuts.indexOf(src);
    if (at >= 0) cfg.shortcuts.splice(at, 1);
    clipboard = null;
  }
  void save();
}

async function deleteShortcutAt(idx: number) {
  const target = cfg.shortcuts[idx];
  if (!target) return;
  // 正在编辑这条且草稿有改动 → 删除会连改动一起丢，要说清楚。
  if (!(await confirmDelete("快捷键", shortcutLabel(target), selected === idx && isDraftDirty()))) {
    return;
  }
  cfg.shortcuts.splice(idx, 1);
  if (selected === idx) {
    discardDraft();
    draftNew = false;
    selected = null;
  } else if (selected !== null && selected > idx) {
    selected -= 1;
  }
  void save();
}

// 详情页删除：确认通过才落删。三个编辑页共用一条路径，别再各写一遍（原先三处重复代码）。
async function deleteDetailEntry(kind: string, label: string, remove: () => void) {
  if (!(await confirmDelete(kind, label, isDraftDirty()))) return;
  remove();
  discardDraft();
  draftNew = false;
  selected = null;
  void save();
}

// 新增项尚未落盘，删除 = 直接丢弃草稿：没有已保存的内容会丢，不必再弹一次确认。
function discardNewDraft() {
  discardDraft();
  draftNew = false;
  selected = null;
  render();
}

// ---- 拖拽排序/移动 ----
// 当前分区的列表元素：落点计算/指示在三个列表间共用同一套几何（行结构一致）。
function activeListEl(): HTMLElement {
  return document.getElementById(
    section === "remaps" ? "remap-list" : section === "expansions" ? "expansion-list" : "shortcut-list",
  )!;
}

function clearDropIndicators() {
  document
    .querySelectorAll(".drop-before, .drop-after, .drop-into, .drop-forbidden")
    .forEach((n) =>
      n.classList.remove("drop-before", "drop-after", "drop-into", "drop-forbidden"),
    );
  document.querySelectorAll(".list").forEach((n) => n.classList.remove("drop-root"));
  dragOverEl = null;
}

function setDropIndicator(el: HTMLElement, cls: string) {
  if (dragOverEl === el) return;
  clearDropIndicators();
  el.classList.add(cls);
  dragOverEl = el;
}

// 目录移动后是否成环：新父目录不能是自身或其子孙。
function wouldCycle(zone: DropZone): boolean {
  if (!dragPayload || dragPayload.kind !== "folder") return false;
  const selfId = dragPayload.id;
  let newParent: string | null = null;
  if (zone.kind === "folder") {
    if (zone.pos === "into") newParent = zone.folderId;
    else newParent = cfg.folders.find((f) => f.id === zone.folderId)?.parent ?? null;
  } else if (zone.kind === "row") {
    newParent = cfg.shortcuts[zone.idx]?.folder ?? null;
  }
  return newParent !== null && collectDescendantIds(selfId).has(newParent);
}

function computeDropZoneAt(x: number, y: number): DropZone | null {
  const list = activeListEl();
  const hit = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!hit || !list.contains(hit)) return null; // 列表外视为无效落点
  const folderEl = hit.closest(".folder-row") as HTMLElement | null;
  if (folderEl) {
    const folderId = folderEl.dataset.folderId!;
    if (dragPayload?.kind === "folder") {
      const r = folderEl.getBoundingClientRect();
      const ratio = (y - r.top) / r.height;
      if (ratio < 0.25) return { kind: "folder", folderId, pos: "before" };
      if (ratio > 0.75) return { kind: "folder", folderId, pos: "after" };
      return { kind: "folder", folderId, pos: "into" };
    }
    return { kind: "folder", folderId, pos: "into" };
  }
  const rowEl = hit.closest(".row") as HTMLElement | null;
  if (rowEl) {
    const idx = Number(rowEl.dataset.idx);
    const r = rowEl.getBoundingClientRect();
    const pos = y - r.top < r.height / 2 ? "before" : "after";
    return { kind: "row", idx, pos };
  }
  // 平铺列表（改键/扩展）没有目录也没有「根」：行间缝隙/末尾空白按 y 找最近行插到它前/后，
  // 不能一律算成「根」（那是快捷键专属的落区语义）。
  if (dragPayload && (dragPayload.kind === "remap" || dragPayload.kind === "expansion")) {
    const rows = Array.from(list.querySelectorAll<HTMLElement>(".row"));
    for (const r of rows) {
      const rect = r.getBoundingClientRect();
      if (y < rect.bottom) {
        return {
          kind: "row",
          idx: Number(r.dataset.idx),
          pos: y < rect.top + rect.height / 2 ? "before" : "after",
        };
      }
    }
    const last = rows[rows.length - 1];
    if (last) return { kind: "row", idx: Number(last.dataset.idx), pos: "after" };
  }
  return { kind: "root" };
}

function showDropIndicator(zone: DropZone) {
  const list = activeListEl();
  if (zone.kind === "root") {
    clearDropIndicators();
    list.classList.add("drop-root");
    dragOverEl = list;
  } else if (zone.kind === "folder") {
    const el = list.querySelector(
      `.folder-row[data-folder-id="${zone.folderId}"]`,
    ) as HTMLElement | null;
    if (!el) return;
    setDropIndicator(
      el,
      zone.pos === "into" ? "drop-into" : zone.pos === "before" ? "drop-before" : "drop-after",
    );
  } else {
    const el = list.querySelector(`.row[data-idx="${zone.idx}"]`) as HTMLElement | null;
    if (!el) return;
    setDropIndicator(el, zone.pos === "before" ? "drop-before" : "drop-after");
  }
}

// 平铺列表内换位：摘出再插回（越过源位时目标下标 -1）。快捷键/改键/文本扩展共用一套算术。
function moveWithin<T>(arr: T[], srcIdx: number, insertIdx: number) {
  const [moved] = arr.splice(srcIdx, 1);
  if (!moved) return;
  const at = insertIdx > srcIdx ? insertIdx - 1 : insertIdx;
  arr.splice(at, 0, moved);
}

function moveShortcut(srcIdx: number, targetFolder: string | null, insertIdx?: number) {
  const moved = cfg.shortcuts[srcIdx];
  if (!moved) return;
  moved.folder = targetFolder;
  if (insertIdx === undefined) {
    cfg.shortcuts.splice(srcIdx, 1);
    cfg.shortcuts.push(moved);
  } else {
    moveWithin(cfg.shortcuts, srcIdx, insertIdx);
  }
}

function moveFolder(
  srcId: string,
  targetParent: string | null,
  beforeFolderId?: string,
  pos?: "before" | "after",
) {
  const src = cfg.folders.find((f) => f.id === srcId);
  if (!src) return;
  src.parent = targetParent;
  cfg.folders = cfg.folders.filter((f) => f.id !== srcId);
  let at = cfg.folders.length;
  if (beforeFolderId) {
    const targetIdx = cfg.folders.findIndex((f) => f.id === beforeFolderId);
    if (targetIdx >= 0) at = pos === "before" ? targetIdx : targetIdx + 1;
  }
  cfg.folders.splice(at, 0, src);
}

function applyDrop(zone: DropZone) {
  if (!dragPayload) return;

  // 改键/文本扩展：平铺换位，不涉及目录。
  if (dragPayload.kind === "remap" || dragPayload.kind === "expansion") {
    const arr: (Remap | TextExpansion)[] =
      dragPayload.kind === "remap" ? cfg.remaps : cfg.expansions;
    const selObj = selected !== null ? arr[selected] : null;
    if (zone.kind === "row") {
      moveWithin(arr, dragPayload.idx, zone.pos === "before" ? zone.idx : zone.idx + 1);
    }
    selected = selObj ? arr.indexOf(selObj) : null;
    void save();
    return;
  }

  const selObj = selected !== null ? cfg.shortcuts[selected] : null;

  if (dragPayload.kind === "shortcut") {
    const srcIdx = dragPayload.idx;
    if (!cfg.shortcuts[srcIdx]) return;
    if (zone.kind === "root") {
      moveShortcut(srcIdx, null);
    } else if (zone.kind === "folder" && zone.pos === "into") {
      moveShortcut(srcIdx, zone.folderId);
    } else if (zone.kind === "row") {
      const targetFolder = cfg.shortcuts[zone.idx]?.folder ?? null;
      moveShortcut(srcIdx, targetFolder, zone.pos === "before" ? zone.idx : zone.idx + 1);
    }
  } else {
    const srcId = dragPayload.id;
    if (zone.kind === "root") {
      moveFolder(srcId, null);
    } else if (zone.kind === "folder") {
      if (zone.pos === "into") {
        moveFolder(srcId, zone.folderId);
      } else {
        const target = cfg.folders.find((f) => f.id === zone.folderId);
        moveFolder(srcId, target?.parent ?? null, zone.folderId, zone.pos);
      }
    } else if (zone.kind === "row") {
      const targetFolder = cfg.shortcuts[zone.idx]?.folder ?? null;
      moveFolder(srcId, targetFolder);
    }
  }

  selected = selObj ? cfg.shortcuts.indexOf(selObj) : null;
  void save();
}

// 给一行绑定拖拽启动：pointerdown 记录起点，移动越过阈值后才真正进入拖拽。
function attachDragStart(li: HTMLElement, payload: DragPayload) {
  li.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return; // 仅左键
    if ((e.target as HTMLElement).closest("input, button")) return;
    // 搜索/筛选会隐藏条目，可见顺序 ≠ 配置顺序，换位落点算不准（与搜索同一处理：禁拖）。
    if (search.trim() || listFilter.enabled || listFilter.conflicted) return;
    dragState = {
      payload,
      startX: e.clientX,
      startY: e.clientY,
      active: false,
      sourceEl: li,
    };
  });
}

function updateDropTarget(x: number, y: number) {
  const zone = computeDropZoneAt(x, y);
  if (zone === null) {
    clearDropIndicators();
    return;
  }
  if (wouldCycle(zone)) {
    clearDropIndicators();
    const hit = document.elementFromPoint(x, y) as HTMLElement | null;
    const target = hit?.closest(".folder-row, .row") as HTMLElement | null;
    if (target) setDropIndicator(target, "drop-forbidden");
    return;
  }
  showDropIndicator(zone);
}

function onPointerMove(e: PointerEvent) {
  if (actionDrag) {
    if (!actionDrag.active) {
      if (Math.hypot(e.clientX - actionDrag.startX, e.clientY - actionDrag.startY) < 6) return;
      actionDrag.active = true;
      actionDrag.sourceEl.classList.add("dragging");
      actionDrag.sourceEl.style.pointerEvents = "none"; // 让 elementFromPoint 穿透源行
    }
    updateActionDropTarget(e.clientX, e.clientY);
    return;
  }
  if (!dragState) return;
  if (!dragState.active) {
    if (Math.hypot(e.clientX - dragState.startX, e.clientY - dragState.startY) < 6) return;
    dragState.active = true;
    dragPayload = dragState.payload;
    dragState.sourceEl.classList.add("dragging");
    dragState.sourceEl.style.pointerEvents = "none"; // 让 elementFromPoint 穿透源行
  }
  updateDropTarget(e.clientX, e.clientY);
}

function onPointerUp(e: PointerEvent) {
  if (actionDrag) {
    const { active, sourceEl, list, index } = actionDrag;
    if (active) {
      const res = actionDropAt(e.clientX, e.clientY);
      clearActionDropIndicators();
      if (res) applyActionDrop(list, index, res.target);
    } else {
      // 未拖拽：单击手柄 → 切换展开/收起
      const a = list[index];
      if (collapsedActions.has(a)) collapsedActions.delete(a);
      else collapsedActions.add(a);
    }
    sourceEl.classList.remove("dragging");
    sourceEl.style.pointerEvents = "";
    actionDrag = null;
    if (draft) renderActions(draft);
    return;
  }
  if (!dragState) return;
  const { active, sourceEl } = dragState;
  if (active) {
    suppressClick = true; // 拖拽结束，抑制随后的一次 click
    const zone = computeDropZoneAt(e.clientX, e.clientY);
    clearDropIndicators();
    if (zone !== null && !wouldCycle(zone)) applyDrop(zone);
  }
  sourceEl.classList.remove("dragging");
  sourceEl.style.pointerEvents = "";
  clearDropIndicators();
  dragPayload = null;
  dragState = null;
  setTimeout(() => (suppressClick = false), 0);
}

function renderRemaps() {
  const list = document.getElementById("remap-list")!;
  list.replaceChildren();
  cfg.remaps.forEach((r, i) => {
    if (!remapVisible(r)) return;

    const li = el("li", "row" + (selected === i ? " selected" : "") + (picked.has(r) ? " picked" : ""));
    li.dataset.idx = String(i);
    makeRow(li, () => void requestOpenDetail(i), { selected: selected === i });
    attachDragStart(li, { kind: "remap", idx: i });
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = r.enabled;
    on.addEventListener("change", (e) => {
      e.stopPropagation();
      r.enabled = on.checked;
      void save();
    });

    const isTiming = !!(r.tap || r.hold || r.hold_layer || r.lock_layer || r.oneshot || r.sticky || r.tap2 || r.tap3);
    li.append(
      on,
      el("span", "triggers", r.from),
      el("span", "arrow", isTiming ? "⇥" : "→"),
      el("span", "triggers", remapSummary(r)),
      moreButton((btn) => showRowMenu(btn, { kind: "remap", idx: i })),
    );
    li.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("input, button")) return;
      if (e.ctrlKey || e.metaKey) {
        togglePicked(r);
        return;
      }
      void requestOpenDetail(i);
    });
    list.append(li);
  });
}

function renderExpansions() {
  const list = document.getElementById("expansion-list")!;
  list.replaceChildren();
  cfg.expansions.forEach((e, i) => {
    if (!expansionVisible(e)) return;

    const li = el("li", "row" + (selected === i ? " selected" : "") + (picked.has(e) ? " picked" : ""));
    li.dataset.idx = String(i);
    makeRow(li, () => void requestOpenDetail(i), { selected: selected === i });
    attachDragStart(li, { kind: "expansion", idx: i });
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = e.enabled;
    on.addEventListener("change", (ev) => {
      ev.stopPropagation();
      e.enabled = on.checked;
      void save();
    });

    li.append(
      on,
      el("span", "triggers", e.trigger),
      el("span", "arrow", "→"),
      el("span", "row-name", e.replace || "（删除触发词）"),
      moreButton((btn) => showRowMenu(btn, { kind: "expansion", idx: i })),
    );
    li.addEventListener("click", (ev) => {
      if ((ev.target as HTMLElement).closest("input, button")) return;
      if (ev.ctrlKey || ev.metaKey) {
        togglePicked(e);
        return;
      }
      void requestOpenDetail(i);
    });
    list.append(li);
  });
}

// ---- 批量启停 / 全选（7.3-⑳） ----
function togglePicked(o: object) {
  if (picked.has(o)) picked.delete(o);
  else picked.add(o);
  renderListPane();
}

// 全选 = 当前「可见」的全部条目（搜索 + 筛选之后），与渲染同一套判定。
function selectAllVisible() {
  if (section === "remaps") cfg.remaps.forEach((r) => remapVisible(r) && picked.add(r));
  else if (section === "expansions") cfg.expansions.forEach((e) => expansionVisible(e) && picked.add(e));
  else cfg.shortcuts.forEach((s) => shortcutVisible(s) && picked.add(s));
  renderListPane();
}

async function setPickedEnabled(v: boolean) {
  if (!picked.size) return;
  let n = 0;
  for (const s of cfg.shortcuts) if (picked.has(s)) { s.enabled = v; n++; }
  for (const r of cfg.remaps) if (picked.has(r)) { r.enabled = v; n++; }
  for (const e of cfg.expansions) if (picked.has(e)) { e.enabled = v; n++; }
  picked.clear();
  const ok = await save(); // save 自带「已保存」提示；成功后盖上条数，失败保留错误提示
  if (ok) toast(`已${v ? "启用" : "停用"} ${n} 项`);
}

function renderDetail() {
  document.getElementById("detail-shortcuts")!.classList.toggle("hidden", section !== "shortcuts");
  document.getElementById("detail-remaps")!.classList.toggle("hidden", section !== "remaps");
  document.getElementById("detail-expansions")!.classList.toggle("hidden", section !== "expansions");
  document.getElementById("detail-settings")!.classList.toggle("hidden", section !== "settings");
  document.getElementById("detail-messages")!.classList.toggle("hidden", section !== "messages");
  document.getElementById("detail-help")!.classList.toggle("hidden", section !== "help");

  if (section === "shortcuts") {
    const has = draft !== null;
    document.getElementById("detail-empty")!.classList.toggle("hidden", has);
    document.getElementById("detail-editor")!.classList.toggle("hidden", !has);
    if (!has) setShortcutTitle();
  } else if (section === "remaps") {
    const has = remapDraft !== null;
    document.getElementById("remap-empty")!.classList.toggle("hidden", has);
    document.getElementById("remap-editor")!.classList.toggle("hidden", !has);
    if (!has) document.getElementById("remap-title")!.textContent = "改键";
  } else if (section === "expansions") {
    const has = expansionDraft !== null;
    document.getElementById("expansion-empty")!.classList.toggle("hidden", has);
    document.getElementById("expansion-editor")!.classList.toggle("hidden", !has);
    if (!has) document.getElementById("expansion-title")!.textContent = "文本扩展";
  }
  refreshDirty();
}

// 快捷键详情标题：显示名称（空则「（未命名）」），点击标题行内编辑名称。
function setShortcutTitle() {
  const title = document.getElementById("detail-title")!;
  if (!draft) {
    title.textContent = "快捷键";
    title.contentEditable = "false";
    title.classList.remove("title-editable");
    title.removeAttribute("title");
    title.onkeydown = null;
    title.onblur = null;
    return;
  }
  title.textContent = draft.name || "（未命名）";
  title.contentEditable = "true";
  title.classList.add("title-editable");
  title.title = "点击编辑名称";
  title.onkeydown = (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  };
  title.onblur = () => {
    if (!draft) return;
    const v = (title.textContent ?? "").replace(/\s+/g, " ").trim();
    draft.name = v || null;
    title.textContent = draft.name || "（未命名）";
  };
  // 名称是行内编辑的，没有对应 input 元素可监听：边打边同步进草稿，脏标记才跟得上。
  title.oninput = () => {
    if (!draft) return;
    const v = (title.textContent ?? "").replace(/\s+/g, " ").trim();
    draft.name = v || null;
    refreshDirty();
  };
}

// 把草稿渲染进编辑页（openDetail 与「撤销改动」共用，避免两条路各写一份填充逻辑）。
function fillShortcutEditor() {
  if (!draft) return;
  (document.getElementById("edit-desc") as HTMLInputElement).value = draft.description ?? "";
  fillLayerSelect(document.getElementById("edit-layer") as HTMLSelectElement, draft.layer ?? null, "assign");
  renderTriggers(draft);
  renderActions(draft);
  setShortcutTitle();
}

function fillRemapEditor() {
  if (!remapDraft) return;
  (document.getElementById("remap-from") as HTMLSelectElement).value = remapDraft.from;
  (document.getElementById("remap-to") as HTMLSelectElement).value = remapDraft.to;
  (document.getElementById("remap-tap") as HTMLSelectElement).value = remapDraft.tap ?? "";
  (document.getElementById("remap-hold") as HTMLSelectElement).value = remapDraft.hold ?? "";
  (document.getElementById("remap-oneshot") as HTMLSelectElement).value = remapDraft.oneshot ?? "";
  (document.getElementById("remap-sticky") as HTMLSelectElement).value = remapDraft.sticky ?? "";
  (document.getElementById("remap-tap2") as HTMLSelectElement).value = remapDraft.tap2 ?? "";
  (document.getElementById("remap-tap3") as HTMLSelectElement).value = remapDraft.tap3 ?? "";
  fillLayerSelect(document.getElementById("remap-layer") as HTMLSelectElement, remapDraft.layer, "assign");
  fillLayerSelect(document.getElementById("remap-hold-layer") as HTMLSelectElement, remapDraft.hold_layer, "hold");
  fillLayerSelect(document.getElementById("remap-lock-layer") as HTMLSelectElement, remapDraft.lock_layer, "hold");
  (document.getElementById("remap-timeout") as HTMLInputElement).value = String(
    remapDraft.tap_timeout_ms || 200,
  );
  syncRemapEditor();
  document.getElementById("remap-title")!.textContent = draftNew
    ? "新增改键"
    : `${remapDraft.from} → ${remapSummary(remapDraft)}`;
}

function fillExpansionEditor() {
  if (!expansionDraft) return;
  (document.getElementById("edit-exp-trigger") as HTMLInputElement).value =
    expansionDraft.trigger;
  (document.getElementById("edit-exp-replace") as HTMLTextAreaElement).value =
    expansionDraft.replace;
  document.getElementById("expansion-title")!.textContent = draftNew
    ? "新增文本扩展"
    : expansionDraft.trigger || "（未命名）";
}

function openDetail(i: number, isNew = false) {
  selected = i;
  draftNew = isNew;
  draftBaseline = null; // 填充期间先不标脏，填完统一标定基线
  if (section === "shortcuts") {
    draft = isNew
      ? { name: "", description: "", folder: null, layer: null, triggers: [], actions: [newAction("text")], enabled: true }
      : deepClone(cfg.shortcuts[i]);
    // 打开已创建的、含多个动作的快捷键时，动作默认收起；新建/新增的动作保持展开。
    if (!isNew && draft.actions.length > 1) collapseAllActions(draft.actions);
    fillShortcutEditor();
  } else if (section === "remaps") {
    remapDraft = isNew
      ? { from: "CapsLock", to: "Ctrl", tap: null, hold: null, layer: null, hold_layer: null, lock_layer: null, tap_timeout_ms: 200, oneshot: null, sticky: null, tap2: null, tap3: null, enabled: true }
      : deepClone(cfg.remaps[i]);
    fillRemapEditor();
  } else if (section === "expansions") {
    expansionDraft = isNew
      ? { trigger: "", replace: "", enabled: true }
      : deepClone(cfg.expansions[i]);
    fillExpansionEditor();
  }
  markDraftBaseline();
  renderListPane();
  renderDetail();
}

function closeDetail() {
  stopCapture();
  void stopRecIfAny();
  discardDraft();
  draftNew = false;
  selected = null;
  render();
}

function discardDraft() {
  // 草稿隔离：编辑只改 draft/remapDraft/expansionDraft，取消/关闭/切换直接丢弃，cfg 不受影响。
  draft = null;
  remapDraft = null;
  expansionDraft = null;
  draftBaseline = null;
  refreshDirty();
}

// ---- 编辑页保存 ----
// 三个编辑页的「保存」按钮与未保存守卫的「保存并离开」共用这三支。返回值 = 是否真的落盘。

// 快捷键：保存后留在编辑页（草稿继续可改）；空的新条目会被 save() 清洗剔除，那才回落到关闭编辑页。
async function saveShortcutDetail(): Promise<boolean> {
  if (section !== "shortcuts" || !draft) return false;
  await stopRecIfAny();
  draft.description = domValue("edit-desc") || null;
  draft.layer = domValue("edit-layer") || null;
  // 落盘的是草稿副本：草稿与 cfg 从此各持一份，之后继续在编辑页改动就不会悄悄改到内存里的
  // cfg（冲突检测 / 列表读的都是 cfg，「保存了但没保存」的样子很难查）。
  const item = deepClone(draft);
  if (draftNew) {
    cfg.shortcuts.unshift(item);
  } else if (selected !== null && cfg.shortcuts[selected]) {
    // 「启用」只有列表行的开关能改（编辑页没这个控件），别拿草稿里的旧值把用户刚点的开关盖回去。
    item.enabled = cfg.shortcuts[selected].enabled;
    cfg.shortcuts[selected] = item;
  }
  draftNew = false;
  const ok = await save();
  selected = cfg.shortcuts.indexOf(item);
  if (selected < 0) draft = null; // 空条目被清洗剔除 → 关闭编辑页
  // 保存成功才刷新基线；失败（或草稿已被剔除）保持原样，别让用户以为已经落盘。
  if (ok) draftBaseline = draftSignature();
  else if (!draft) draftBaseline = null;
  render();
  if (ok) flashRow(selected ?? -1, "shortcut-list");
  return ok;
}

async function saveRemapDetail(): Promise<boolean> {
  if (section !== "remaps" || !remapDraft) return false;
  remapDraft.from = domValue("remap-from");
  remapDraft.tap = domValue("remap-tap") || null;
  remapDraft.hold = domValue("remap-hold") || null;
  remapDraft.oneshot = domValue("remap-oneshot") || null;
  remapDraft.sticky = domValue("remap-sticky") || null;
  remapDraft.tap2 = domValue("remap-tap2") || null;
  remapDraft.tap3 = domValue("remap-tap3") || null;
  remapDraft.layer = domValue("remap-layer") || null;
  remapDraft.hold_layer = domValue("remap-hold-layer") || null;
  remapDraft.lock_layer = domValue("remap-lock-layer") || null;
  remapDraft.tap_timeout_ms = intIn(domValue("remap-timeout"), 50, 200, 2000);
  // 任一非普通改键形态（tap-hold/切层/单次/粘滞/连击）→ 清空「改为」；否则用「改为」。
  remapDraft.to = remapDraft.tap ||
    remapDraft.hold ||
    remapDraft.hold_layer ||
    remapDraft.lock_layer ||
    remapDraft.oneshot ||
    remapDraft.sticky ||
    remapDraft.tap2 ||
    remapDraft.tap3
    ? ""
    : domValue("remap-to");
  // 长按进入层（momentary）与长按锁定层（切换式）互斥：两者都选了保留切换式（层内放和弦/
  // 序列只能用切换式，见前端的提示文案），避免后端归一化时静默丢一个让用户困惑。
  if (remapDraft.hold_layer && remapDraft.lock_layer) {
    remapDraft.hold_layer = null;
    (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
    toast("同时选了「长按进入层」与「长按锁定层」，保留「长按锁定层」");
  }
  // 没有任何输出（改为/短按/长按/长按进入层/长按锁定层/单次/粘滞/双击/三击全空）→ 后端会当
  // 无效条目丢掉。这里直接拦住并说清楚，别让用户以为配好了（「长按进入层」没选中层就是这样丢的）。
  // 返回 false 让「保存并离开」也停下来，改键还留在编辑页。
  if (
    !remapDraft.tap &&
    !remapDraft.hold &&
    !remapDraft.hold_layer &&
    !remapDraft.lock_layer &&
    !remapDraft.oneshot &&
    !remapDraft.sticky &&
    !remapDraft.tap2 &&
    !remapDraft.tap3 &&
    !remapDraft.to
  ) {
    toast("请至少设置一种输出：改为 / 短按 / 长按 / 长按进入层 / 长按锁定层 / 单次 / 粘滞 / 双击 / 三击");
    return false;
  }
  const item = deepClone(remapDraft);
  if (draftNew) {
    cfg.remaps.unshift(item);
  } else if (selected !== null && cfg.remaps[selected]) {
    item.enabled = cfg.remaps[selected].enabled;
    cfg.remaps[selected] = item;
  }
  remapDraft = null; // 改键保存后关闭编辑页（原行为）
  draftNew = false;
  selected = null;
  draftBaseline = null;
  const ok = await save();
  if (ok) flashRow(cfg.remaps.indexOf(item), "remap-list");
  return ok;
}

async function saveExpansionDetail(): Promise<boolean> {
  if (section !== "expansions" || !expansionDraft) return false;
  const trigger = domValue("edit-exp-trigger").trim();
  // 触发词非法必须拦在保存前：save() 会把空触发词整条剔除——用户以为存了，其实条目没了。
  const err = expansionTriggerError(trigger);
  if (err) {
    const input = document.getElementById("edit-exp-trigger") as HTMLInputElement;
    showFieldError(input, err);
    input.focus();
    toast(`触发词无效：${err}`);
    return false;
  }
  expansionDraft.trigger = trigger;
  expansionDraft.replace = domValue("edit-exp-replace");
  const item = deepClone(expansionDraft);
  if (draftNew) {
    cfg.expansions.unshift(item);
  } else if (selected !== null && cfg.expansions[selected]) {
    item.enabled = cfg.expansions[selected].enabled;
    cfg.expansions[selected] = item;
  }
  expansionDraft = null; // 文本扩展保存后关闭编辑页（原行为）
  draftNew = false;
  selected = null;
  draftBaseline = null;
  const ok = await save();
  if (ok) flashRow(cfg.expansions.indexOf(item), "expansion-list");
  return ok;
}

// ---- 未保存改动守卫 ----
// 编辑草稿是 cfg 的副本（openDetail 时 deepClone / 新建模板），所以「改没改过」没法靠引用比对，
// 只能与基线比对：draftBaseline 记下打开编辑页（或上次保存）那一刻的草稿签名。
// 签名里必须带上「只在保存时才读回草稿」的那些输入框——描述 / 所属层、改键的原键与长按阈值、
// 文本扩展的触发词与替换文本——否则改这些字段脏标记不亮，离开时照样静默丢。

function domValue(id: string): string {
  const node = document.getElementById(id) as
    | HTMLInputElement
    | HTMLSelectElement
    | HTMLTextAreaElement
    | null;
  return node ? node.value : "";
}

// 数值框取值：解析失败回退默认值、越界收回到 [min, max]（input 的 min/max 属性就是口径）。
// 此前 `parseInt(x, 10) || 0` 对负数照单全收（-50ms 这种值一路落盘，后端 u64 反序列化直接报错）。
function intIn(v: string, min: number, fallback: number, max = Number.MAX_SAFE_INTEGER): number {
  const n = parseInt(v, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(max, Math.max(min, n));
}

function draftSignature(): string | null {
  if (section === "shortcuts") {
    if (!draft) return null;
    return JSON.stringify({
      ...draft,
      description: domValue("edit-desc") || null,
      layer: domValue("edit-layer") || null,
    });
  }
  if (section === "remaps") {
    if (!remapDraft) return null;
    return JSON.stringify({
      ...remapDraft,
      from: domValue("remap-from"),
      to: domValue("remap-to"),
      layer: domValue("remap-layer") || null,
      tap_timeout_ms: intIn(domValue("remap-timeout"), 50, 200, 2000),
    });
  }
  if (section === "expansions") {
    if (!expansionDraft) return null;
    return JSON.stringify({
      ...expansionDraft,
      trigger: domValue("edit-exp-trigger").trim(),
      replace: domValue("edit-exp-replace"),
    });
  }
  return null;
}

function markDraftBaseline() {
  draftBaseline = draftSignature();
  refreshDirty();
}

// 基线为 null = 没有草稿（或正在填充），一律不算脏——编辑内容清空时签名也是 null，
// 不这么判会把「草稿没了」当成「有改动」。
function isDraftDirty(): boolean {
  return draftBaseline !== null && draftSignature() !== draftBaseline;
}

// 脏标记与「撤销改动」的可用态。（标记放动作行，不放标题：标题就是行内编辑的名称输入框。）
function refreshDirty() {
  const dirty = isDraftDirty();
  for (const s of ["shortcuts", "remaps", "expansions"] as Section[]) {
    const onChange = dirty && s === section;
    document.getElementById(`dirty-${s}`)?.classList.toggle("hidden", !onChange);
    const undo = document.getElementById(`undo-${s}`) as HTMLButtonElement | null;
    if (undo) undo.disabled = !onChange;
  }
}

// 撤销改动：把草稿还原成基线内容（打开时 / 上次保存的），编辑页留在原地。
function revertDraft() {
  if (!draftBaseline) return;
  if (section === "shortcuts") {
    draft = JSON.parse(draftBaseline) as ShortcutItem;
    fillShortcutEditor();
  } else if (section === "remaps") {
    remapDraft = JSON.parse(draftBaseline) as Remap;
    fillRemapEditor();
  } else if (section === "expansions") {
    expansionDraft = JSON.parse(draftBaseline) as TextExpansion;
    fillExpansionEditor();
  } else {
    return;
  }
  refreshDirty();
  // 触发键可能被改回来了，冲突提示跟着重算（否则还挂着已撤销的那条冲突）。
  void refreshConflicts().then(renderConflictBubble);
  toast("已撤销未保存的改动");
}

type UnsavedChoice = "save" | "discard" | "cancel";
let unsavedPending: ((c: UnsavedChoice) => void) | null = null;

function draftLabel(): string {
  if (section === "shortcuts") return `快捷键「${draft?.name || "未命名"}」`;
  if (section === "remaps" && remapDraft) {
    return `改键「${remapDraft.from} → ${remapSummary(remapDraft)}」`;
  }
  if (section === "expansions") return `文本扩展「${expansionDraft?.trigger || "未命名"}」`;
  return "当前编辑";
}

function askUnsaved(): Promise<UnsavedChoice> {
  // 已经有一个确认框等着时忽略后来的请求（连点两行会同时进来）：让它当「取消」处理，
  // 否则先到的那个 promise 永远不 resolve，那一次点击就永远挂在那里。
  if (unsavedPending) return Promise.resolve("cancel");
  document.getElementById("unsaved-text")!.textContent = `${draftLabel()}有未保存的改动。`;
  document.getElementById("unsaved-modal")!.classList.remove("hidden");
  return new Promise((resolve) => (unsavedPending = resolve));
}

function resolveUnsaved(choice: UnsavedChoice) {
  const pending = unsavedPending;
  unsavedPending = null;
  document.getElementById("unsaved-modal")!.classList.add("hidden");
  pending?.(choice);
}

// 离开前的守卫：有未保存改动就问一句。true = 可以继续离开（已保存 / 已放弃）。
async function confirmLeaveDraft(): Promise<boolean> {
  if (!isDraftDirty()) return true;
  const choice = await askUnsaved();
  if (choice === "cancel") return false;
  if (choice === "discard") {
    discardDraft();
    draftNew = false;
    return true;
  }
  // 「保存并离开」：保存失败（或改键没设任何输出被判无效）就别走，留在编辑页别把改动丢了。
  return await saveCurrentDraft();
}

async function saveCurrentDraft(): Promise<boolean> {
  if (section === "shortcuts") return await saveShortcutDetail();
  if (section === "remaps") return await saveRemapDetail();
  if (section === "expansions") return await saveExpansionDetail();
  return false;
}

// 列表行点开 / 「+ 新增」：草稿有未保存改动时先问一句，别默默覆盖掉草稿。
async function requestOpenDetail(i: number, isNew = false) {
  if (!(await confirmLeaveDraft())) return;
  openDetail(i, isNew);
}

// 取消 / ×  关闭编辑页：同样是「离开」，走同一道守卫。
async function requestCloseDetail() {
  if (!(await confirmLeaveDraft())) return;
  closeDetail();
}

// 外部修改（后端监听 config.json 后广播的新配置，见规划 7.2-⑩）：把界面换成文件里的那一份。
//
// 触达编辑页时也算一次「离开当前编辑上下文」，所以与 leave 守卫同一套口径：**有未保存改动**
// 就问一句，两个按钮各自写明后果（一个丢弃改动、一个会在下次保存时覆盖文件里的外部改动）；
// 只是打开了编辑页却没改（干净草稿）就不问——关掉它什么都不会丢，留着反而危险：外部内容里
// 的同一下标可能已经是另一条，草稿一保存就写到别人身上去了。
async function applyExternalConfig(next: Config) {
  const editing = draft !== null || remapDraft !== null || expansionDraft !== null;
  if (editing) {
    if (isDraftDirty()) {
      const take = await confirmAsk(
        `${draftLabel()}有未保存的改动，而配置文件刚被外部修改（手改 JSON / 恢复备份 / 同步落盘）。` +
          "加载新内容会放弃这些改动。",
        {
          title: "配置已被外部修改",
          kind: "warning",
          okLabel: "加载新配置",
          cancelLabel: "保留我的改动（下次保存会覆盖外部改动）",
        },
      );
      // 保留编辑：界面与内存都不动，用户手上的编辑照旧。代价（下次保存覆盖外部改动）已经写在
      // 按钮上，消息中心里也留着一条外部改动的记录，之后不会再重复弹（后端已采纳过一次）。
      if (!take) return;
    }
    stopCapture();
    void stopRecIfAny();
    discardDraft();
    draftNew = false;
    selected = null;
  }
  cfg = next;
  await refreshConflicts();
  render();
  syncSettings();
  toast("配置文件已被外部修改，已重新加载");
}

function flashRow(i: number, listId: string) {
  if (i < 0) return;
  const row = document.querySelector<HTMLElement>(`#${listId} .row[data-idx="${i}"]`);
  if (!row) return;
  row.classList.remove("flash");
  void row.offsetWidth; // 强制重排以重启动画
  row.classList.add("flash");
  row.addEventListener("animationend", () => row.classList.remove("flash"), { once: true });
}

function renderTriggers(s: ShortcutItem) {
  const wrap = document.getElementById("edit-triggers")!;
  wrap.replaceChildren();
  s.triggers.forEach((t, i) => {
    const chip = el("span", "chip", t);
    const x = el("button", "chip-x", "×");
    x.addEventListener("click", () => {
      s.triggers.splice(i, 1);
      renderTriggers(s);
      void refreshConflicts().then(renderConflictBubble);
    });
    chip.append(x);
    wrap.append(chip);
  });
  refreshDirty();
}

// ---- 动作列表渲染（流程管线视图） ----
function renderActions(s: ShortcutItem) {
  const wrap = document.getElementById("action-list")!;
  renderActionList(wrap, s.actions, () => renderActions(s), {
    start: s.triggers.length ? s.triggers.join(" / ") : "未设触发键",
  });
  refreshDirty();
}

// 递归把某个动作列表的所有动作标记为收起（打开已有多动作的快捷键时用）。
function collapseAllActions(actions: Action[]) {
  for (const a of actions) {
    collapsedActions.add(a);
    if (a.type === "if") {
      collapseAllActions(a.then);
      collapseAllActions(a.otherwise);
    }
  }
}

// 流程管线的起止端点：⚡ 触发（附组合键）→ 步骤 → ✓ 执行完成。仅顶层列表有，分支泳道没有。
function flowTerminator(kind: "start" | "end", main: string, sub?: string): HTMLElement {
  const t = el("div", `flow-terminator ${kind}`);
  const rail = el("div", "flow-rail");
  rail.append(el("span", "flow-dot term", kind === "start" ? "⚡" : "✓"));
  const label = el("span", "flow-term-label");
  label.append(el("span", undefined, main));
  if (sub) label.append(el("b", undefined, sub));
  t.append(rail, label);
  return t;
}

// 单个流程节点：左侧时间线脊柱（序号圆点）+ 右侧动作卡片。
// 拖拽命中计算用：把「所在列表 + 下标」直接挂在卡片元素上（非序列化属性）。
function flowStep(a: Action, i: number, actions: Action[], rerender: () => void): HTMLElement {
  const collapsed = collapsedActions.has(a);
  const step = el("div", `flow-step type-${a.type}`);
  const row = el("div", "action-row" + (collapsed ? " collapsed" : ""));
  row.dataset.idx = String(i);
  (row as unknown as { _dropList?: Action[] })._dropList = actions;
  (row as unknown as { _dropIndex?: number })._dropIndex = i;

  const rail = el("div", "flow-rail");
  const dot = el("span", "flow-dot", String(i + 1));
  dot.title = "拖动排序 / 嵌套；点击展开或收起";
  rail.append(dot);

  // 第一行：拖动手柄 + 图标 + 类型名 + 标题（描述） + 变量流标签 + 删除
  const head = el("div", "action-head");

  const grip = el("span", "action-grip", "⋮⋮");
  grip.title = "拖动排序 / 嵌套；点击展开或收起";
  head.append(grip);

  const ico = el("span", "action-ico", ACTION_ICONS[a.type]);
  ico.title = actionTypeLabel(a.type);
  head.append(ico);

  head.append(el("span", "action-type-name", actionTypeLabel(a.type)));

  // 拖拽 + 点击展开/收起：脊柱圆点、手柄、图标都可触发（扩大命中范围）。
  for (const handle of [dot, grip, ico]) {
    attachActionDragStart(handle, row, actions, i);
  }

  // 变量流向标签：写出（⇒ var，绿）在前、引用（{var}，灰）在后，一眼看出数据怎么流。
  const tags = el("span", "action-vartags");
  for (const w of actionVarWrites(a)) tags.append(el("span", "var-tag write", `⇒ ${w}`));
  const reads = actionVarReads(a);
  for (const r of reads.slice(0, 3)) tags.append(el("span", "var-tag read", `{${r}}`));
  if (reads.length > 3) tags.append(el("span", "var-tag read", `+${reads.length - 3}`));

  if (collapsed) {
    const title = el("span", "action-title", actionDescription(a));
    title.title = "点击展开";
    title.addEventListener("click", () => {
      toggleActionCollapsed(a);
      rerender();
    });
    head.append(title, tags);
  } else {
    const titleInput = el("input", "action-input action-title-input") as HTMLInputElement;
    titleInput.type = "text";
    titleInput.value = a.description ?? "";
    titleInput.placeholder = "动作标题（可选）";
    titleInput.addEventListener("input", () => {
      a.description = titleInput.value.trim() || undefined;
    });
    const del = el("button", "action-del") as HTMLButtonElement;
    del.title = "删除动作";
    del.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>';
    del.addEventListener("click", () => {
      actions.splice(i, 1);
      collapsedActions.delete(a);
      rerender();
    });
    head.append(titleInput, tags, del);
  }

  row.append(head);

  if (!collapsed) {
    // 第二行：动作类型 + 子类型
    const typeRow = el("div", "action-type-row");
    typeRow.append(makeActionTypeSelect(a, actions, i, rerender));
    const sub = actionSubtypeSelect(a, rerender);
    if (sub) typeRow.append(sub);
    row.append(typeRow);
    // 第三行：其他输入内容
    row.append(actionFields(a, rerender));
  }

  step.append(rail, row);
  return step;
}

// 把一个动作列表渲染成流程管线进 container；任何结构变化（增删/移动/改类型/拖拽）都通过
// rerender() 触发整体重绘。嵌套的「条件判断」动作同样用它递归渲染 then/otherwise 分支
//（不带起止端点）。opts.start 传入触发摘要时在管线头部加 ⚡ 端点、尾部加 ✓ 端点。
// 收起态只显示 类型名 + 描述 + 变量标签；展开态为三行：标题行 / 类型筛选行 / 输入行。
function renderActionList(
  container: HTMLElement,
  actions: Action[],
  rerender: () => void,
  opts: { start?: string } = {},
): void {
  container.replaceChildren();
  container.classList.add("flow");
  if (opts.start !== undefined) {
    container.append(flowTerminator("start", "触发", opts.start));
  }
  actions.forEach((a, i) => {
    container.append(flowStep(a, i, actions, rerender));
  });
  if (opts.start !== undefined) {
    container.append(
      flowTerminator(
        "end",
        actions.length ? "执行完成" : "还没有动作 · 点下方「+ 添加动作」",
      ),
    );
  } else if (actions.length === 0) {
    container.append(el("div", "flow-empty", "空 · 拖入动作或点下方「添加动作」"));
  }
}

function toggleActionCollapsed(a: Action) {
  if (collapsedActions.has(a)) collapsedActions.delete(a);
  else collapsedActions.add(a);
}

function makeActionTypeSelect(
  a: Action,
  actions: Action[],
  i: number,
  rerender: () => void,
): HTMLElement {
  const sel = el("select", "action-type") as HTMLSelectElement;
  for (const t of ACTION_TYPES) {
    const o = el("option") as HTMLOptionElement;
    o.value = t.value;
    o.textContent = t.label;
    sel.append(o);
  }
  sel.value = a.type;
  sel.addEventListener("change", () => {
    const next = newAction(sel.value as Action["type"]);
    next.description = a.description;
    actions[i] = next;
    collapsedActions.delete(a);
    rerender();
  });
  return sel;
}

// 给动作的「摘要区」绑定拖拽启动：pointerdown 记录起点，移动越过阈值后进入拖拽。
function attachActionDragStart(
  handle: HTMLElement,
  row: HTMLElement,
  list: Action[],
  index: number,
) {
  handle.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    actionDrag = { list, index, startX: e.clientX, startY: e.clientY, active: false, sourceEl: row };
  });
}

type ActionDropTarget =
  | { kind: "before"; list: Action[]; index: number }
  | { kind: "after"; list: Action[]; index: number }
  | { kind: "into"; list: Action[] };

// 命中计算：从指针下的元素向上找，先碰到动作行按 before/after 处理，先碰到分支按追加处理。
function actionDropAt(
  x: number,
  y: number,
): { target: ActionDropTarget; el: HTMLElement } | null {
  if (!actionDrag) return null;
  const hit = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!hit) return null;
  let node: HTMLElement | null = hit;
  while (node && node !== document.body) {
    if (node.classList.contains("action-row")) {
      const list = (node as unknown as { _dropList?: Action[] })._dropList;
      const index = (node as unknown as { _dropIndex?: number })._dropIndex;
      if (list && index !== undefined) {
        const r = node.getBoundingClientRect();
        const before = y - r.top < r.height / 2;
        return {
          el: node,
          target: { kind: before ? "before" : "after", list, index: before ? index : index + 1 },
        };
      }
      return null;
    }
    if (node.classList.contains("action-branch")) {
      const list = (node as unknown as { _dropList?: Action[] })._dropList;
      return list ? { el: node, target: { kind: "into", list } } : null;
    }
    node = node.parentElement;
  }
  return null;
}

let actionDropEl: HTMLElement | null = null;
function clearActionDropIndicators() {
  if (actionDropEl) {
    actionDropEl.classList.remove("drop-before", "drop-after", "drop-into");
    actionDropEl = null;
  }
}

function updateActionDropTarget(x: number, y: number) {
  const res = actionDropAt(x, y);
  clearActionDropIndicators();
  if (!res) return;
  const cls =
    res.target.kind === "into"
      ? "drop-into"
      : res.target.kind === "before"
        ? "drop-before"
        : "drop-after";
  res.el.classList.add(cls);
  actionDropEl = res.el;
}

function applyActionDrop(srcList: Action[], srcIndex: number, target: ActionDropTarget) {
  const [moved] = srcList.splice(srcIndex, 1);
  if (!moved) return;
  if (target.kind === "into") {
    target.list.push(moved);
  } else {
    let idx = target.index;
    if (srcList === target.list && idx > srcIndex) idx -= 1;
    target.list.splice(idx, 0, moved);
  }
}

// 第二行的「子类型」筛选：随动作类型变化的二级下拉（文本方式 / Shell / OS 操作 / 应用操作 / 条件类型）。
// 无子类型的动作（按键、延迟）返回 null。
function actionSubtypeSelect(a: Action, rerender: () => void): HTMLElement | null {
  if (a.type === "text") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const m of [
      { value: "input" as TextMode, label: "输入文本" },
      { value: "to_upper" as TextMode, label: "小写转大写（选中/剪贴板）" },
      { value: "to_lower" as TextMode, label: "大写转小写（选中/剪贴板）" },
    ]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = m.value;
      opt.textContent = m.label;
      sel.append(opt);
    }
    sel.value = a.mode;
    sel.addEventListener("change", () => {
      a.mode = sel.value as TextMode;
      rerender();
    });
    return sel;
  }
  if (a.type === "command") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const s of [
      { value: "cmd" as Shell, label: "CMD" },
      { value: "powershell" as Shell, label: "PowerShell" },
    ]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = s.value;
      opt.textContent = s.label;
      sel.append(opt);
    }
    sel.value = a.shell;
    sel.addEventListener("change", () => {
      a.shell = sel.value as Shell;
      rerender();
    });
    return sel;
  }
  if (a.type === "os") {
    const op = a.operation;
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of OS_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = op.op;
    sel.addEventListener("change", () => {
      a.operation = newOsOp(sel.value as OsOperation["op"]);
      rerender();
    });
    return sel;
  }
  if (a.type === "app") {
    const op = a.operation;
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of APP_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = op.op;
    sel.addEventListener("change", () => {
      a.operation = newAppOp(sel.value as AppOperation["op"]);
      rerender();
    });
    return sel;
  }
  if (a.type === "if") {
    const cond = a.condition;
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const c of CONDITION_TYPES) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = c.value;
      opt.textContent = c.label;
      sel.append(opt);
    }
    sel.value = cond.kind;
    sel.addEventListener("change", () => {
      a.condition = newCondition(sel.value as Condition["kind"]);
      rerender();
    });
    return sel;
  }
  return null;
}

function actionFields(a: Action, rerender: () => void): HTMLElement {
  const body = el("div", "action-body");
  if (a.type === "text") {
    if (a.mode === "input") {
      const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
      ta.value = a.text;
      ta.placeholder = "要输入的文本（支持 {变量名} 占位符）……";
      ta.addEventListener("input", () => {
        a.text = ta.value;
      });
      body.append(ta);
    } else {
      body.append(
        el(
          "div",
          "unit-hint",
          a.mode === "to_upper"
            ? "把当前选中（或剪贴板）的文本转为大写，并粘贴回原处。"
            : "把当前选中（或剪贴板）的文本转为小写，并粘贴回原处。",
        ),
      );
    }
  } else if (a.type === "command") {
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = a.command;
    ta.placeholder =
      a.shell === "cmd"
        ? "CMD 命令，如 echo hello（可用 {变量名} / {变量名.字段} 引用变量）"
        : "PowerShell 命令，如 Get-Date（可用 {变量名} / {变量名.字段} 引用变量）";
    ta.addEventListener("input", () => {
      a.command = ta.value;
    });
    body.append(ta);

    const popup = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = a.show_output;
    cb.addEventListener("change", () => {
      a.show_output = cb.checked;
    });
    popup.append(cb, el("span", undefined, "弹窗显示结果"));
    body.append(popup);

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = "变量名（可选），留空则只进消息中心";
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(el("span", "unit-hint", "存为变量"), vname);
    body.append(vline);
    body.append(
      varUsage(
        "填写后把命令标准输出存入该变量：{变量名} 得到输出全文、{变量名.exit_code} 得到退出码（0=成功）。留空则维持现状，仅记入消息中心。示例：取盘符 (Get-Volume -FileSystemLabel '我的硬盘').DriveLetter 存为 drive，后面复制动作目标写 {drive}:\\备份\\。",
      ),
    );
  } else if (a.type === "script") {
    body.append(pathField(a.path, (v) => (a.path = v), { file: true, label: "脚本路径" }));

    const iline = el("div", "action-line");
    const interp = el("input", "action-input") as HTMLInputElement;
    interp.type = "text";
    interp.value = a.interpreter ?? "";
    interp.placeholder = "解释器（可选），如 python / node / bash；留空则直接执行脚本本身";
    interp.addEventListener("input", () => {
      const v = interp.value.trim();
      a.interpreter = v.length > 0 ? v : null;
    });
    iline.append(el("span", "unit-hint", "解释器"), interp);
    body.append(iline);

    const popup = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = a.show_output;
    cb.addEventListener("change", () => {
      a.show_output = cb.checked;
    });
    popup.append(cb, el("span", undefined, "弹窗显示结果"));
    body.append(popup);

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = "变量名（可选），留空则只进消息中心";
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(el("span", "unit-hint", "存为变量"), vname);
    body.append(vline);
    body.append(
      varUsage(
        "脚本执行时通过环境变量注入触发上下文：KADA_TRIGGER（触发组合，如 Ctrl+Alt+K）、KADA_NAME（快捷键名）、KADA_VARS（变量表 JSON）。填写变量名后把标准输出存入该变量：{变量名} 得输出全文、{变量名.exit_code} 得退出码。",
      ),
    );
  } else if (a.type === "keys") {
    const line = el("div", "action-line");
    const keys = el("input", "action-input") as HTMLInputElement;
    keys.type = "text";
    keys.value = a.keys.join("+");
    keys.placeholder = "如 Ctrl+C";
    keys.addEventListener("input", () => {
      a.keys = keys.value
        .split("+")
        .map((x) => x.trim())
        .filter(Boolean);
    });
    const cap = el("button", undefined, "录入");
    cap.addEventListener("click", (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      startCapture((combo) => {
        a.keys = combo.split("+");
        keys.value = combo;
      }, btn);
    });
    line.append(keys, cap);
    body.append(line);
  } else if (a.type === "pause_ms") {
    const line = el("div", "action-line");
    const num = el("input", "action-input") as HTMLInputElement;
    num.type = "number";
    num.min = "0";
    num.value = String(a.ms);
    num.placeholder = "如 500";
    num.addEventListener("input", () => {
      a.ms = intIn(num.value, 0, 0);
    });
    line.append(num, el("span", "unit-hint", "毫秒 (ms)"));
    body.append(line);
  } else if (a.type === "os") {
    const op = a.operation;
    if (op.op === "copy" || op.op === "cut") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, dir: true, label: "原始路径" }),
      );
      body.append(
        pathField(op.dest, (v) => (op.dest = v), { file: true, dir: true, label: "目标路径" }),
      );
    } else if (op.op === "zip") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, dir: true, label: "待压缩路径" }),
      );
      body.append(
        pathField(op.dest, (v) => (op.dest = v), { file: true, dir: true, label: "压缩文件路径" }),
      );
    } else if (op.op === "unzip") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, label: "压缩包路径" }),
      );
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true, label: "解压到目录" }));
    } else if (op.op === "paste") {
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true, label: "目标目录" }));
    } else if (op.op === "delete") {
      body.append(
        pathField(op.path, (v) => (op.path = v), { file: true, dir: true, label: "待删除路径" }),
      );
    } else if (op.op === "new_file") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, label: "新建文件路径" }));
    } else if (op.op === "new_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true, label: "新建文件夹路径" }));
    } else if (op.op === "open_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true, label: "打开目录" }));
    } else if (op.op === "get_file_props") {
      body.append(
        pathField(op.path, (v) => (op.path = v), { file: true, dir: true, label: "文件路径" }),
      );
      const line = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = "变量名，如 file";
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      line.append(el("span", "unit-hint", "变量名"), vname);
      body.append(line);
      body.append(
        varUsage(
          "写入文件属性变量。引用：{变量名} 得完整路径；可用 {变量名.name} 文件名 / {变量名.dir} 所在目录 / {变量名.stem} 主名 / {变量名.ext} 扩展名 / {变量名.size} 大小(字节) / {变量名.modified} 修改时间 / {变量名.is_dir} 是否目录。",
        ),
      );
    }
  } else if (a.type === "app") {
    const op = a.operation;
    if (op.op === "launch" || op.op === "restart") {
      const progWrap = el("div", "action-line");
      const prog = el("input", "action-input") as HTMLInputElement;
      prog.type = "text";
      prog.value = op.program;
      prog.placeholder = "程序路径/名，如 C:\\Windows\\notepad.exe 或 notepad.exe";
      prog.addEventListener("input", () => {
        op.program = prog.value;
      });
      const browse = el("button", undefined, "浏览…");
      browse.addEventListener("click", () => void pickFile(prog));
      progWrap.append(prog, browse);
      body.append(progWrap);

      const args = el("input", "action-input") as HTMLInputElement;
      args.type = "text";
      args.value = op.args.join(" ");
      args.placeholder = "参数（可选，空格分隔）";
      args.addEventListener("input", () => {
        op.args = args.value.split(/\s+/).filter(Boolean);
      });
      body.append(args);
    } else if (op.op === "close") {
      const line = el("div", "action-line");
      const name = el("input", "action-input") as HTMLInputElement;
      name.type = "text";
      name.value = op.program;
      name.placeholder = "程序名/路径，如 notepad.exe";
      name.addEventListener("input", () => {
        op.program = name.value.trim();
      });
      line.append(name);
      body.append(line);
    } else if (op.op === "status") {
      const line = el("div", "action-line");
      const name = el("input", "action-input") as HTMLInputElement;
      name.type = "text";
      name.value = op.program;
      name.placeholder = "程序名/路径，如 notepad.exe";
      name.addEventListener("input", () => {
        op.program = name.value.trim();
      });
      line.append(name);
      body.append(line);

      const vline = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = "变量名，如 app";
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      vline.append(el("span", "unit-hint", "变量名"), vname);
      body.append(vline);
      body.append(
        varUsage(
          "写入布尔变量：程序在运行则为 true，否则 false。引用 {变量名} 得到 true/false，可配合「条件判断」动作使用。",
        ),
      );

      const rline = el("div", "action-line");
      const retries = el("input", "action-input action-input-num") as HTMLInputElement;
      retries.type = "number";
      retries.min = "0";
      retries.value = String(op.retries);
      retries.addEventListener("input", () => {
        op.retries = intIn(retries.value, 0, 0);
      });
      const ival = el("input", "action-input action-input-num") as HTMLInputElement;
      ival.type = "number";
      ival.min = "0";
      ival.value = String(op.interval_ms);
      ival.addEventListener("input", () => {
        op.interval_ms = intIn(ival.value, 0, 0);
      });
      rline.append(
        el("span", "unit-hint", "未运行时重试"),
        retries,
        el("span", "unit-hint", "次 · 间隔"),
        ival,
        el("span", "unit-hint", "ms"),
        helpIcon(
          "程序未运行（变量为 false）时，按「重试次数 × 重试间隔」轮询重查，等程序起来；0 次 = 只查一次。",
        ),
      );
      body.append(rline);
    }
  } else if (a.type === "open_url") {
    const line = el("div", "action-line");
    const url = el("input", "action-input") as HTMLInputElement;
    url.type = "text";
    url.value = a.url;
    url.placeholder = "网址，如 https://example.com（支持 {变量名} 占位符）";
    url.addEventListener("input", () => {
      a.url = url.value.trim();
      showFieldError(url, urlError(url.value));
    });
    line.append(url);
    body.append(line);
    body.append(el("div", "unit-hint", "用系统默认浏览器打开该网页。"));
  } else if (a.type === "if") {
    const cond = a.condition;

    if (
      cond.kind === "exists" ||
      cond.kind === "not_exists" ||
      cond.kind === "is_file" ||
      cond.kind === "is_dir"
    ) {
      body.append(pathField(cond.path, (v) => (cond.path = v), { file: true, dir: true }));
    } else if (cond.kind === "modified_within") {
      body.append(pathField(cond.path, (v) => (cond.path = v), { file: true, dir: true }));
      const line = el("div", "action-line");
      const mins = el("input", "action-input") as HTMLInputElement;
      mins.type = "number";
      mins.min = "0";
      mins.value = String(cond.minutes);
      mins.addEventListener("input", () => {
        cond.minutes = intIn(mins.value, 0, 0);
      });
      line.append(el("span", "unit-hint", "修改时间在"), mins, el("span", "unit-hint", "分钟内为真"));
      body.append(line);
    } else if (cond.kind === "frontmost_app" || cond.kind === "not_frontmost_app") {
      const line = el("div", "action-line");
      const app = el("input", "action-input") as HTMLInputElement;
      app.type = "text";
      app.value = cond.app;
      app.placeholder = "进程名，如 chrome.exe（含 * / ? 走通配，否则子串匹配）";
      app.addEventListener("input", () => {
        cond.app = app.value;
      });
      line.append(el("span", "unit-hint", "进程名"), app);
      body.append(line);
    } else if (cond.kind === "window_title_contains") {
      const line = el("div", "action-line");
      const text = el("input", "action-input") as HTMLInputElement;
      text.type = "text";
      text.value = cond.text;
      text.placeholder = "窗口标题包含的文本（不区分大小写）";
      text.addEventListener("input", () => {
        cond.text = text.value;
      });
      line.append(el("span", "unit-hint", "包含文本"), text);
      body.append(line);
    } else if (cond.kind === "device_is") {
      const line = el("div", "action-line");
      const id = el("input", "action-input") as HTMLInputElement;
      id.type = "text";
      id.value = cond.id;
      id.placeholder = "设备名，如 AT Translated Set 2 keyboard（含 * / ? 走通配，否则子串匹配）";
      id.addEventListener("input", () => {
        cond.id = id.value;
      });
      line.append(el("span", "unit-hint", "设备名"), id);
      body.append(line);
      body.append(
        el(
          "div",
          "unit-hint",
          "按触发键来自哪台键盘区分（仅 Linux 生效；Windows / macOS 取不到设备，恒走「否则」分支）。",
        ),
      );
    } else {
      const lineVar = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = cond.var;
      vname.placeholder = "变量名，如 f";
      vname.addEventListener("input", () => {
        cond.var = vname.value.trim();
      });
      lineVar.append(el("span", "unit-hint", "变量名"), vname);
      body.append(lineVar);

      const lineField = el("div", "action-line");
      const fname = el("input", "action-input") as HTMLInputElement;
      fname.type = "text";
      fname.value = cond.field;
      fname.placeholder = "字段（空 = 完整路径），如 ext / size / is_dir";
      fname.addEventListener("input", () => {
        cond.field = fname.value.trim();
      });
      lineField.append(el("span", "unit-hint", "字段"), fname);
      body.append(lineField);

      const lineVal = el("div", "action-line");
      const val = el("input", "action-input") as HTMLInputElement;
      val.type = "text";
      val.value = cond.value;
      val.placeholder = "比较值，如 .txt";
      val.addEventListener("input", () => {
        cond.value = val.value;
      });
      lineVal.append(el("span", "unit-hint", "等于/不等于"), val);
      body.append(lineVal);
    }

    body.append(el("div", "branch-label then", "✓ 满足条件时执行"));
    const thenList = el("div", "action-branch branch-then");
    (thenList as unknown as { _dropList?: Action[] })._dropList = a.then;
    renderActionList(thenList, a.then, rerender);
    const addThen = el("button", "add-inline", "＋ 添加动作");
    addThen.addEventListener("click", () => {
      a.then.push(newAction("text"));
      rerender();
    });
    body.append(thenList, addThen);

    body.append(el("div", "branch-label else", "✗ 否则执行（可空）"));
    const elseList = el("div", "action-branch branch-else");
    (elseList as unknown as { _dropList?: Action[] })._dropList = a.otherwise;
    renderActionList(elseList, a.otherwise, rerender);
    const addElse = el("button", "add-inline", "＋ 添加动作");
    addElse.addEventListener("click", () => {
      a.otherwise.push(newAction("text"));
      rerender();
    });
    body.append(elseList, addElse);
  }
  return body;
}

// ---- 内联校验（7.3-⑳） ----
// 红边 + 容器下一行小字（复用同一 .field-error 节点）；msg 空 = 清除。
// 只在用户动过字段后才校验（input 事件驱动），打开编辑页时不给存量值一上来就标红。
function showFieldError(input: HTMLElement, msg: string) {
  input.classList.toggle("invalid", !!msg);
  const host = input.parentElement;
  if (!host) return;
  let hint = host.querySelector<HTMLElement>(":scope > .field-error");
  if (!msg) {
    hint?.remove();
    return;
  }
  if (!hint) {
    hint = el("div", "field-error");
    host.append(hint);
  }
  hint.textContent = msg;
}

// 路径合法性：剥掉 {变量} 占位符后，Windows 路径里不允许出现的字符 + 控制字符。
// 冒号放行（盘符 `C:\` 与 `\\server\share` 都要有冒号）；空值不在此拦（各动作的必填口径不同，
// 保存前有 actionHasContent 兜底）。
function pathError(v: string): string {
  const bare = v.replace(/\{[^}]*\}/g, "");
  if (/[<>|?*"]/.test(bare)) return '路径含不允许的字符（< > | ? * "）';
  if (/[\u0000-\u001f]/.test(bare)) return "路径含不可见控制字符";
  return "";
}

// 网址合法性：非空、http(s) 开头；含 {变量} 占位符的拼完才知道形态，放行。
function urlError(v: string): string {
  const t = v.trim();
  if (!t) return "网址不能为空";
  if (t.includes("{")) return "";
  if (!/^https?:\/\/\S+$/i.test(t)) return "需以 http:// 或 https:// 开头";
  return "";
}

// 文本扩展触发词（与后端 TextExpansion::validate 同口径）：非空、无空白字符。
function expansionTriggerError(v: string): string {
  if (!v) return "触发词不能为空";
  if (/\s/.test(v)) return "触发词不能含空格 / 回车 / Tab";
  return "";
}

// 路径输入行：一个文本框 + 可选的「文件…」「目录…」浏览按钮。
function pathField(
  value: string,
  onInput: (v: string) => void,
  opts: { file?: boolean; dir?: boolean; label?: string } = { file: true },
): HTMLElement {
  const line = el("div", "action-line");
  const input = el("input", "action-input") as HTMLInputElement;
  input.type = "text";
  input.value = value;
  input.placeholder = opts.label ? `${opts.label}（可用 {变量名}）` : "路径（可用 {变量名} 占位符）";
  // 全部路径字段都从这里过——校验放共用口，一处管住所有调用方（条件/复制/脚本…）。
  input.addEventListener("input", () => {
    onInput(input.value);
    showFieldError(input, pathError(input.value));
  });
  line.append(input);
  if (opts.file) {
    const bf = el("button", undefined, "文件…");
    bf.addEventListener("click", () => void pickFile(input));
    line.append(bf);
  }
  if (opts.dir) {
    const bd = el("button", undefined, "目录…");
    bd.addEventListener("click", () => void pickDir(input));
    line.append(bd);
  }
  return line;
}

function setAndNotify(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

async function pickFile(input: HTMLInputElement) {
  const path = await openFile();
  if (path && !Array.isArray(path)) setAndNotify(input, path);
}

async function pickDir(input: HTMLInputElement) {
  const path = await openFile({ directory: true });
  if (path && !Array.isArray(path)) setAndNotify(input, path);
}

// ---- 冲突提示 ----
// 快捷键列表里只显示一个可点击、可消除的气泡（冲突清单不常驻列表）；冲突详情常驻「消息 → 冲突」标签。
function renderConflictBubble() {
  const box = document.getElementById("conflict-list")!;
  box.replaceChildren();
  const show = section === "shortcuts" && conflicts.length > 0 && !conflictBubbleDismissed;
  box.classList.toggle("hidden", !show);
  if (!show) return;
  const errs = conflicts.filter((c) => c.severity === "error").length;
  const bubble = el("div", "conflict-bubble");
  bubble.addEventListener("click", openConflictView);
  bubble.append(
    el("span", "conflict-bubble-text", `⚠ ${conflicts.length} 处冲突${errs ? `（${errs} 处需处理）` : ""}`),
    el("span", "conflict-bubble-hint", "点击查看"),
  );
  const close = el("button", "conflict-bubble-close", "×") as HTMLButtonElement;
  close.title = "消除提醒";
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    conflictBubbleDismissed = true;
    renderConflictBubble();
  });
  bubble.append(close);
  box.append(bubble);
}

// 冲突详情（常驻「消息 → 冲突」标签）：显示是哪个快捷键 + 冲突说明。
function renderConflictList() {
  const box = document.getElementById("msg-conflict-list")!;
  box.replaceChildren();
  if (conflicts.length === 0) {
    box.append(el("div", "detail-empty", "暂无冲突"));
    return;
  }
  box.append(el("div", "conflict-head", `⚠ 冲突提醒（${conflicts.length} 处）`));
  for (const c of conflicts) {
    const d = el("div", "conflict " + (c.severity === "error" ? "error" : "warn"));
    if (c.name) d.append(el("div", "conflict-name", c.name));
    d.append(el("div", "conflict-msg", c.message));
    box.append(d);
  }
}

// 跳到「消息 → 冲突」标签查看冲突详情。
function openConflictView() {
  msgView = "conflicts";
  void switchSection("messages");
}

// ---- 设置 ----
/** 超时（毫秒）→ 界面上的秒数（配置里存毫秒，界面按秒录入；0 = 不限）。 */
function timeoutSeconds(ms: number): number {
  return Math.round((Number(ms) || 0) / 1000);
}

/** 等待窗（毫秒）→ 输入框文本（界面按毫秒录入；0 = 不限）。缺字段的老配置按默认 1000 显示。 */
function timeoutMs(ms: number | undefined): string {
  const n = Number(ms);
  return String(ms === undefined || !Number.isFinite(n) || n < 0 ? 1000 : Math.floor(n));
}

function syncSettings() {
  (document.getElementById("set-autostart") as HTMLInputElement).checked = cfg.settings.autostart;
  (document.getElementById("set-paused") as HTMLInputElement).checked = cfg.settings.paused;
  (document.getElementById("set-wake-key") as HTMLSelectElement).value = cfg.settings.wake_key ?? "";
  (document.getElementById("set-action-timeout") as HTMLInputElement).value = String(
    timeoutSeconds(cfg.settings.action_timeout_ms),
  );
  (document.getElementById("set-sequence-timeout") as HTMLInputElement).value = timeoutMs(
    cfg.settings.sequence_timeout_ms,
  );
  (document.getElementById("set-chord-timeout") as HTMLInputElement).value = timeoutMs(
    cfg.settings.chord_timeout_ms,
  );
  // 缺字段的老配置（写在该设置出现之前）按「开」显示，与后端 `default_true` 一致。
  (document.getElementById("set-status-hud") as HTMLInputElement).checked =
    cfg.settings.show_status_hud !== false;
  // 提示框默认「关」（后端 `HintsWindow::default` 也是 false）：老配置缺 `hints` 时显示为未勾选。
  (document.getElementById("set-hints") as HTMLInputElement).checked =
    cfg.settings.hints?.visible === true;
  // 缺字段的老配置按默认「剪贴板粘贴」显示，与后端 serde default 一致。
  (document.getElementById("set-text-inject") as HTMLSelectElement).value =
    cfg.settings.text_inject_mode ?? "clipboard";
}

function bindSettings() {
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const wakeKey = document.getElementById("set-wake-key") as HTMLSelectElement;
  const actionTimeout = document.getElementById("set-action-timeout") as HTMLInputElement;
  const sequenceTimeout = document.getElementById("set-sequence-timeout") as HTMLInputElement;
  const chordTimeout = document.getElementById("set-chord-timeout") as HTMLInputElement;
  const statusHud = document.getElementById("set-status-hud") as HTMLInputElement;
  const hints = document.getElementById("set-hints") as HTMLInputElement;
  autostart.addEventListener("change", () => {
    cfg.settings.autostart = autostart.checked;
    void save();
  });
  paused.addEventListener("change", () => {
    cfg.settings.paused = paused.checked;
    void save();
  });
  wakeKey.addEventListener("change", () => {
    cfg.settings.wake_key = wakeKey.value || null;
    void save();
  });
  actionTimeout.addEventListener("change", () => {
    // 负值 / 非数字按 0（不限时）处理：后端只认 u64，宁可放开也不静默变成 1 秒把人坑了。
    const secs = Math.max(0, Math.floor(Number(actionTimeout.value) || 0));
    actionTimeout.value = String(secs);
    cfg.settings.action_timeout_ms = secs * 1000;
    void save();
  });
  // 两个等待窗按毫秒录入（默认 1000）：负数 / 非数字同样按 0（不限时）处理。
  sequenceTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(sequenceTimeout.value) || 0));
    sequenceTimeout.value = String(ms);
    cfg.settings.sequence_timeout_ms = ms;
    void save();
  });
  chordTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(chordTimeout.value) || 0));
    chordTimeout.value = String(ms);
    cfg.settings.chord_timeout_ms = ms;
    void save();
  });
  statusHud.addEventListener("change", () => {
    cfg.settings.show_status_hud = statusHud.checked;
    void save();
  });
  // 提示框的开关：改动随整份配置落盘，后端 `set_config` 比对到 `visible` 变了就把窗口
  // 建出来 / 收起来，并把托盘菜单的勾对齐（见 `apply_hints_visible`）。
  hints.addEventListener("change", () => {
    cfg.settings.hints = { ...(cfg.settings.hints ?? { scale: 100, opacity: 92 }), visible: hints.checked };
    void save();
  });
  const textInject = document.getElementById("set-text-inject") as HTMLSelectElement;
  textInject.addEventListener("change", () => {
    cfg.settings.text_inject_mode = textInject.value as "clipboard" | "unicode";
    void save();
  });
  document.getElementById("export-config")!.addEventListener("click", () => void exportConfig());
  document.getElementById("import-config")!.addEventListener("click", () => void importConfig());
  document.getElementById("check-update")!.addEventListener("click", () => {
    void invoke("check_update");
  });
  document.getElementById("install-update")!.addEventListener("click", () => {
    void invoke("install_update");
  });
}

async function exportConfig() {
  const path = await saveFile({
    defaultPath: "kada-config.json",
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  try {
    await invoke("export_config", { path });
    toast("已导出到 " + path);
  } catch (e) {
    toast(`导出失败: ${e}`);
  }
}

// 导入结果（后端 ImportOutcome）：只回「忽略了几处」的话，用户查不出是哪条没进来，
// 所以明细逐条列出。
type ImportMode = "merge" | "replace";
type ImportOutcome = {
  mode: ImportMode;
  added: number;
  notes: string[];
  backup: string | null;
};

let importModePending: ((m: ImportMode | null) => void) | null = null;

// 导入方式选择。导入是唯一能一次改掉整份配置的入口，必须先明确选，不能点一下就算数。
function askImportMode(file: string): Promise<ImportMode | null> {
  // 已经有一个框等着时，后来的请求按「取消」处理（同未保存守卫：否则先到的 promise 永远挂着）。
  if (importModePending) return Promise.resolve(null);
  document.getElementById("import-file")!.textContent = `将导入 ${file}，请选择导入方式：`;
  document.getElementById("import-modal")!.classList.remove("hidden");
  return new Promise((resolve) => (importModePending = resolve));
}

function resolveImportMode(mode: ImportMode | null) {
  const pending = importModePending;
  importModePending = null;
  document.getElementById("import-modal")!.classList.add("hidden");
  pending?.(mode);
}

async function importConfig() {
  const path = await openFile({
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path || Array.isArray(path)) return;
  const mode = await askImportMode(fileName(path));
  if (!mode) return;
  try {
    const r = await invoke<ImportOutcome>("import_config", { path, mode });
    await load();
    showImportResult(r);
  } catch (e) {
    toast(`导入失败: ${e}`);
  }
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function showImportResult(r: ImportOutcome) {
  const skipped = r.notes.length;
  document.getElementById("import-result-text")!.textContent =
    r.mode === "merge"
      ? `已合并：新增 ${r.added} 条${skipped ? `，跳过 / 忽略 ${skipped} 条` : ""}。`
      : `已替换为导入的配置：共 ${r.added} 条${skipped ? `，忽略 ${skipped} 条` : ""}。`;
  const note = document.getElementById("import-result-note")!;
  note.classList.toggle("hidden", !r.backup);
  if (r.backup) note.textContent = `导入前的配置已留档：${r.backup}（覆盖回去即可回滚）`;
  const list = document.getElementById("import-result-list")!;
  list.replaceChildren(...r.notes.map((n) => el("li", undefined, n)));
  document.getElementById("import-result-modal")!.classList.remove("hidden");
}

function hideImportResult() {
  document.getElementById("import-result-modal")!.classList.add("hidden");
}

// ---- 软件更新 ----
// 状态源在后端（src-tauri/src/update.rs）：检查/下载/安装各阶段由 Rust 推进并 emit
// `update-status`，前端只渲染，不自己维护「正在检查」这类瞬时状态（否则窗口重开就丢）。
type UpdateStatus = { current: string } & (
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "up-to-date" }
  | { phase: "available"; version: string; notes?: string | null }
  | { phase: "downloading"; version: string; percent?: number | null }
  | { phase: "installing"; version: string }
  | { phase: "error"; message: string }
);

let updateStatus: UpdateStatus | null = null;

function syncUpdate() {
  const line = document.getElementById("update-status")!;
  const actions = document.getElementById("update-actions")!;
  const checkBtn = document.getElementById("check-update") as HTMLButtonElement;
  if (!updateStatus) {
    line.textContent = "正在读取版本…";
    actions.classList.add("hidden");
    return;
  }
  const current = `v${updateStatus.current}`;
  const busy =
    updateStatus.phase === "checking" ||
    updateStatus.phase === "downloading" ||
    updateStatus.phase === "installing";
  checkBtn.disabled = busy;
  actions.classList.toggle("hidden", updateStatus.phase !== "available");

  switch (updateStatus.phase) {
    case "idle":
      line.textContent = `当前版本 ${current}`;
      break;
    case "checking":
      line.textContent = `当前版本 ${current} · 正在检查更新…`;
      break;
    case "up-to-date":
      line.textContent = `当前版本 ${current}，已是最新版本`;
      break;
    case "available":
      line.textContent = `发现新版本 v${updateStatus.version}（当前 ${current}），可下载安装`;
      break;
    case "downloading": {
      const pct =
        updateStatus.percent === null || updateStatus.percent === undefined
          ? ""
          : ` ${Math.round(updateStatus.percent)}%`;
      line.textContent = `正在下载 v${updateStatus.version}…${pct}`;
      break;
    }
    case "installing":
      line.textContent = `正在安装 v${updateStatus.version}，应用即将退出…`;
      break;
    case "error":
      line.textContent = `检查更新失败：${updateStatus.message}`;
      break;
  }
}

// ---- 改键下拉 ----
function fillKeySelect(sel: HTMLSelectElement, value: string, withEmpty = false) {
  sel.replaceChildren();
  if (withEmpty) {
    const o = document.createElement("option");
    o.value = "";
    o.textContent = "（无）";
    sel.append(o);
  }
  for (const k of KEY_OPTIONS) {
    const o = document.createElement("option");
    o.value = k;
    o.textContent = k;
    sel.append(o);
  }
  sel.value = value;
  // 手写的值可能不在表里（`Escape`/`Control` 这类规范名与表里的 `Esc`/`Ctrl` 是别名关系）：
  // 补一个选项兜住，否则下拉一片空白、一保存就把用户写的值丢掉。
  if (value && sel.value !== value) {
    const o = document.createElement("option");
    o.value = value;
    o.textContent = value;
    sel.append(o);
    sel.value = value;
  }
}

// 单次 / 粘滞键的下拉：值域是**任意键**（修饰键是常见用法，非修饰键 = 替你按住它，
// 见规划 7.3-⑭），所以直接用全键表。

// 根据所选形态，切换「单次/粘滞键」「短按/双击/三击/长按/切层 + 阈值」与「普通改键」表单显示。
function syncRemapEditor() {
  if (!remapDraft) return;
  const isModKey = !!(remapDraft.oneshot || remapDraft.sticky);
  const isTapHold = !!(remapDraft.tap || remapDraft.hold || remapDraft.tap2 || remapDraft.tap3);
  const isLayerKey = !!(remapDraft.hold_layer || remapDraft.lock_layer);
  // 单次/粘滞键与 tap-hold/切层/普通改键互斥：隐藏后者；反之隐藏修饰下拉无意义（保留可见）。
  document.getElementById("remap-tap-row")!.classList.toggle("hidden", isModKey);
  document.getElementById("remap-hold-row")!.classList.toggle("hidden", isModKey);
  document.getElementById("remap-hold-layer-row")!.classList.toggle("hidden", isModKey);
  document.getElementById("remap-lock-layer-row")!.classList.toggle("hidden", isModKey);
  document.getElementById("remap-tap2-row")!.classList.toggle("hidden", isModKey || !isTapHold);
  document.getElementById("remap-tap3-row")!.classList.toggle("hidden", isModKey || !isTapHold);
  document.getElementById("remap-timeout-row")!.classList.toggle("hidden", isModKey || !isTapHold);
  document.getElementById("remap-to-row")!.classList.toggle("hidden", isModKey || isTapHold || isLayerKey);
  refreshDirty();
}

// 填充「所属层」（assign）/「长按进入层」（hold）下拉。
function fillLayerSelect(sel: HTMLSelectElement, value: string | null, mode: "assign" | "hold") {
  sel.replaceChildren();
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = mode === "hold" ? "（不切层）" : "基础层（始终生效）";
  sel.append(empty);
  for (const l of cfg.layers) {
    const o = document.createElement("option");
    o.value = l.id;
    o.textContent = l.name;
    sel.append(o);
  }
  sel.value = value ?? "";
}

function layerName(id: string | null | undefined): string {
  if (!id) return "基础层";
  return cfg.layers.find((l) => l.id === id)?.name ?? "基础层";
}

// 改键形态的人类可读摘要（列表行 + 详情标题共用），与 kada-core 的 Remap::describe 对齐。
function remapSummary(r: Remap): string {
  if (r.sticky) return `粘滞 ${r.sticky}`;
  if (r.oneshot) return `单击 ${r.oneshot}`;
  if (r.lock_layer) return `长按锁定层「${layerName(r.lock_layer)}」`;
  if (r.hold_layer) return `长按进层「${layerName(r.hold_layer)}」`;
  const parts: string[] = [];
  if (r.tap) parts.push(`单击 ${r.tap}`);
  if (r.tap2) parts.push(`双击 ${r.tap2}`);
  if (r.tap3) parts.push(`三击 ${r.tap3}`);
  if (r.hold) parts.push(`长按 ${r.hold}`);
  if (parts.length) return parts.join(" · ");
  return r.to;
}

function renderLayers() {
  const list = document.getElementById("layer-list")!;
  list.replaceChildren();
  cfg.layers.forEach((l) => {
    const row = el("div", "layer-row");
    if (editingLayerId === l.id) {
      const inp = el("input", "folder-input") as HTMLInputElement;
      inp.value = l.name;
      inp.placeholder = "层名";
      let done = false;
      const commit = () => {
        if (done) return;
        done = true;
        const v = inp.value.trim();
        if (v) l.name = v;
        else cfg.layers = cfg.layers.filter((x) => x.id !== l.id);
        editingLayerId = null;
        void save();
        renderLayers();
        renderDetail();
      };
      inp.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape") {
          done = true;
          editingLayerId = null;
          renderLayers();
        }
      });
      inp.addEventListener("blur", commit);
      row.append(inp);
      setTimeout(() => inp.focus(), 0);
    } else {
      row.append(el("span", "layer-name", l.name));
    }
    const del = el("button", "row-more", "×") as HTMLButtonElement;
    del.title = "删除层";
    del.addEventListener("click", (e) => {
      e.stopPropagation();
      void deleteLayer(l.id);
    });
    row.append(del);
    list.append(row);
  });
}

function addLayer() {
  const l: Layer = { id: newFolderId(), name: "" };
  cfg.layers.push(l);
  editingLayerId = l.id;
  renderLayers();
}

async function deleteLayer(id: string) {
  const target = cfg.layers.find((l) => l.id === id);
  if (!target) return;
  const ok = await confirmAsk(
    `删除层「${target.name}」？该层的快捷键/改键会回到基础层，指向它的切层键会失效。`,
    { title: "删除层", kind: "warning" },
  );
  if (!ok) return;
  cfg.layers = cfg.layers.filter((l) => l.id !== id);
  cfg.shortcuts.forEach((s) => {
    if (s.layer === id) s.layer = null;
  });
  cfg.remaps.forEach((r) => {
    if (r.layer === id) r.layer = null;
    if (r.hold_layer === id) r.hold_layer = null;
    if (r.lock_layer === id) r.lock_layer = null;
  });
  if (editingLayerId === id) editingLayerId = null;
  void save();
}

// ---- 事件绑定 ----
// 焦点是否在表单控件里（Delete 这类全局键不能抢文本编辑）。
function isFormTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return (
    !!el &&
    (el.tagName === "INPUT" ||
      el.tagName === "TEXTAREA" ||
      el.tagName === "SELECT" ||
      el.isContentEditable)
  );
}

function bind() {
  document.querySelectorAll<HTMLButtonElement>(".rail-item").forEach((b) => {
    b.addEventListener("click", () => void switchSection(b.dataset.tab as Section));
  });

  document.getElementById("add-btn")!.addEventListener("click", () => {
    if (section === "shortcuts" || section === "remaps" || section === "expansions") {
      void requestOpenDetail(-1, true);
    }
  });

  // 脏标记：编辑页里任何一次输入/选择/点按都可能改动草稿，用事件委托统一重算，
  // 好过给每个字段（含动态生成的动作字段）逐个插桩。
  for (const id of ["detail-editor", "remap-editor", "expansion-editor"]) {
    const pane = document.getElementById(id)!;
    pane.addEventListener("input", refreshDirty);
    pane.addEventListener("change", refreshDirty);
    pane.addEventListener("click", refreshDirty);
  }

  // 未保存确认框：遮罩与 Esc 一律按「继续编辑」处理（不做任何破坏性动作）。
  document.getElementById("unsaved-save")!.addEventListener("click", () => resolveUnsaved("save"));
  document.getElementById("unsaved-discard")!.addEventListener("click", () =>
    resolveUnsaved("discard"),
  );
  document.getElementById("unsaved-cancel")!.addEventListener("click", () =>
    resolveUnsaved("cancel"),
  );
  document.getElementById("unsaved-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", () => resolveUnsaved("cancel"));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && unsavedPending) {
      e.preventDefault();
      resolveUnsaved("cancel");
    }
  });

  // 导入方式：遮罩 / Esc 一律按「取消」——绝不替用户选一种导入方式。
  document.getElementById("import-merge")!.addEventListener("click", () => resolveImportMode("merge"));
  document.getElementById("import-replace")!.addEventListener("click", () =>
    resolveImportMode("replace"),
  );
  document.getElementById("import-cancel")!.addEventListener("click", () => resolveImportMode(null));
  document.getElementById("import-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", () => resolveImportMode(null));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && importModePending) {
      e.preventDefault();
      resolveImportMode(null);
    }
  });

  // 导入结果框：纯展示，点哪都关。
  document.getElementById("import-result-ok")!.addEventListener("click", hideImportResult);
  document.getElementById("import-result-close")!.addEventListener("click", hideImportResult);
  document.getElementById("import-result-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", hideImportResult);

  document.getElementById("add-folder-btn")!.addEventListener("click", () => {
    if (section === "shortcuts") addFolder(null);
  });

  document.getElementById("add-layer-btn")!.addEventListener("click", addLayer);

  document.getElementById("list-search")!.addEventListener("input", (e) => {
    search = (e.target as HTMLInputElement).value;
    renderListPane();
  });

  // 筛选条（7.3-⑳）：与搜索叠加；改完只重画列表栏（与搜索同路径）。
  document.getElementById("filter-enabled")!.addEventListener("click", () => {
    listFilter.enabled = !listFilter.enabled;
    renderListPane();
  });
  document.getElementById("filter-conflicted")!.addEventListener("click", () => {
    listFilter.conflicted = !listFilter.conflicted;
    renderListPane();
  });

  // 批量条：全选按「当前可见」取，启停对选中集一次性写入并落盘。
  document.getElementById("batch-all")!.addEventListener("click", selectAllVisible);
  document.getElementById("batch-enable")!.addEventListener("click", () => void setPickedEnabled(true));
  document.getElementById("batch-disable")!.addEventListener("click", () => void setPickedEnabled(false));
  document.getElementById("batch-clear")!.addEventListener("click", () => {
    picked.clear();
    renderListPane();
  });

  // 粘贴到根级（此前只能粘进目录，根级没入口）。
  document.getElementById("paste-btn")!.addEventListener("click", () => pasteIntoFolder(null));

  // 触发词内联校验（空/含空白即标红；保存处另有硬拦截）。
  document.getElementById("edit-exp-trigger")!.addEventListener("input", (e) => {
    const input = e.target as HTMLInputElement;
    showFieldError(input, expansionTriggerError(input.value.trim()));
  });

  document.getElementById("add-action")!.addEventListener("click", () => {
    if (section !== "shortcuts" || !draft) return;
    draft.actions.push(newAction("text"));
    renderActions(draft);
  });

  document.getElementById("edit-capture")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || !draft) return;
    const d = draft;
    startCapture((combo) => {
      if (!d.triggers.includes(combo)) d.triggers.push(combo);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  document.getElementById("edit-sequence")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || !draft) return;
    const d = draft;
    startSequenceCapture((seq) => {
      if (!d.triggers.includes(seq)) d.triggers.push(seq);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  document.getElementById("edit-chord")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || !draft) return;
    const d = draft;
    startChordCapture((chord) => {
      if (!d.triggers.includes(chord)) d.triggers.push(chord);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  document.getElementById("action-record")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || !draft) return;
    if (!recording) {
      try {
        await invoke("start_record");
      } catch (err) {
        toast(`开始录制失败: ${err}`);
        return;
      }
      recording = true;
      btn.classList.add("recording");
      btn.textContent = "■ 录制中（点此停止）";
    } else {
      recording = false;
      const actions = await invoke<Action[]>("stop_record");
      btn.classList.remove("recording");
      btn.textContent = "● 开始录制";
      // 录制结果进草稿，统一由「保存」落盘，避免未点保存就被写盘。
      draft.actions.push(...actions);
      renderActions(draft);
    }
  });

  document.getElementById("save-shortcut")!.addEventListener("click", () => {
    void saveShortcutDetail();
  });

  document.getElementById("cancel-shortcut")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("detail-close")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("undo-shortcuts")!.addEventListener("click", revertDraft);
  document.getElementById("delete-shortcut")!.addEventListener("click", () => {
    if (section !== "shortcuts" || !draft) return;
    const d = draft;
    if (draftNew) return discardNewDraft();
    void deleteDetailEntry("快捷键", shortcutLabel(d), () => {
      if (selected !== null) cfg.shortcuts.splice(selected, 1);
    });
  });

  document.getElementById("save-remap")!.addEventListener("click", () => {
    void saveRemapDetail();
  });
  document.getElementById("cancel-remap")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("remap-close")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("undo-remaps")!.addEventListener("click", revertDraft);
  document.getElementById("delete-remap")!.addEventListener("click", () => {
    if (section !== "remaps" || !remapDraft) return;
    const r = remapDraft;
    if (draftNew) return discardNewDraft();
    void deleteDetailEntry("改键", `${r.from} → ${remapSummary(r)}`, () => {
      if (selected !== null) cfg.remaps.splice(selected, 1);
    });
  });

  document.getElementById("save-expansion")!.addEventListener("click", () => {
    void saveExpansionDetail();
  });

  document.getElementById("cancel-expansion")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("expansion-close")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("undo-expansions")!.addEventListener("click", revertDraft);
  document.getElementById("delete-expansion")!.addEventListener("click", () => {
    if (section !== "expansions" || !expansionDraft) return;
    const exp = expansionDraft;
    if (draftNew) return discardNewDraft();
    void deleteDetailEntry("文本扩展", exp.trigger || "未命名", () => {
      if (selected !== null) cfg.expansions.splice(selected, 1);
    });
  });

  fillKeySelect(document.getElementById("remap-from") as HTMLSelectElement, "CapsLock");
  fillKeySelect(document.getElementById("remap-to") as HTMLSelectElement, "Ctrl");
  fillKeySelect(document.getElementById("remap-tap") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-hold") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-oneshot") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-sticky") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-tap2") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-tap3") as HTMLSelectElement, "", true);
  document.getElementById("remap-tap")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.tap = (e.target as HTMLSelectElement).value || null;
    syncRemapEditor();
  });
  document.getElementById("remap-hold")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.hold = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.hold) {
      // 长按输出键与切层互斥（进入层 / 锁定层都算切层）。
      remapDraft.hold_layer = null;
      remapDraft.lock_layer = null;
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
      (document.getElementById("remap-lock-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-hold-layer")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.hold_layer = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.hold_layer) {
      remapDraft.hold = null;
      remapDraft.lock_layer = null; // 进入层与锁定层互斥（切换式才用得起层内和弦/序列）
      (document.getElementById("remap-hold") as HTMLSelectElement).value = "";
      (document.getElementById("remap-lock-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-lock-layer")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.lock_layer = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.lock_layer) {
      remapDraft.hold = null;
      remapDraft.hold_layer = null;
      (document.getElementById("remap-hold") as HTMLSelectElement).value = "";
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-oneshot")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.oneshot = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.oneshot) {
      remapDraft.sticky = null; // 单次与粘滞互斥
      (document.getElementById("remap-sticky") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-sticky")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.sticky = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.sticky) {
      remapDraft.oneshot = null;
      (document.getElementById("remap-oneshot") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-tap2")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.tap2 = (e.target as HTMLSelectElement).value || null;
    syncRemapEditor();
  });
  document.getElementById("remap-tap3")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.tap3 = (e.target as HTMLSelectElement).value || null;
    syncRemapEditor();
  });

  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);

  // ---- 全局键盘：Ctrl+S 保存 / Ctrl+F 搜索 / Delete 删除 / Esc 关弹窗 ----
  // 注册在 unsaved / import 两处 Esc 之后：它们处理过会 preventDefault，这里让行
  // （一次 Esc 只关一层）。录入组合键 / 序列 / 宏录制在 window 捕获阶段
  // stopPropagation，按键到不了这里，不会被全局键抢走。
  document.addEventListener("keydown", (e) => {
    if (e.defaultPrevented) return;

    // Esc：关最上层的菜单 / 结果弹窗（unsaved / import 的 Esc 注册在先，已挡掉）。
    if (e.key === "Escape") {
      const menu = document.getElementById("row-menu")!;
      if (!menu.classList.contains("hidden")) {
        hideRowMenu();
        return;
      }
      const rm = document.getElementById("result-modal")!;
      if (!rm.classList.contains("hidden")) {
        e.preventDefault();
        hideResultModal();
      }
      return;
    }

    // 弹窗在场时全局键让路（模态自己的按钮自理；删除确认是原生 ask() 对话框）。
    if (document.querySelector(".modal:not(.hidden)")) return;
    const mod = e.ctrlKey || e.metaKey;

    // Ctrl+S：保存当前编辑页，等价点「保存」按钮（含校验与提示）。
    if (mod && !e.altKey && (e.key === "s" || e.key === "S")) {
      e.preventDefault();
      if (section === "shortcuts" && draft) void saveShortcutDetail();
      else if (section === "remaps" && remapDraft) void saveRemapDetail();
      else if (section === "expansions" && expansionDraft) void saveExpansionDetail();
      return;
    }

    // Ctrl+F：聚焦列表搜索框（列表被隐藏时先切页，切页自带未保存守卫）。
    if (mod && !e.altKey && (e.key === "f" || e.key === "F")) {
      e.preventDefault();
      const focusSearch = () => {
        const input = document.getElementById("list-search") as HTMLInputElement;
        input.focus();
        input.select();
      };
      if (document.getElementById("list-pane")!.classList.contains("hidden")) {
        void switchSection("shortcuts").then(focusSearch);
      } else {
        focusSearch();
      }
      return;
    }

    // Delete：删当前详情页这条（与「删除」按钮同一条路径，含确认框）；焦点在表单控件里时不动。
    if (e.key === "Delete" && !isFormTarget(e.target)) {
      const delId =
        section === "shortcuts" && draft
          ? "delete-shortcut"
          : section === "remaps" && remapDraft
            ? "delete-remap"
            : section === "expansions" && expansionDraft
              ? "delete-expansion"
              : null;
      if (delId) {
        e.preventDefault();
        document.getElementById(delId)!.click();
      }
    }
  });

  bindSettings();
}

// 切页会丢弃草稿（草稿只属于当前编辑页），所以先过一道未保存守卫。
async function switchSection(tab: Section) {
  if (!(await confirmLeaveDraft())) return;
  switchSectionNow(tab);
}

function switchSectionNow(tab: Section) {
  void stopRecIfAny();
  discardDraft();
  draftNew = false;
  selected = null;
  section = tab;
  search = "";
  listFilter = { enabled: false, conflicted: false };
  picked.clear();
  if (tab === "messages") markRead();
  (document.getElementById("list-search") as HTMLInputElement).value = "";
  document.querySelectorAll<HTMLButtonElement>(".rail-item").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === tab);
  });
  render();
}

async function stopRecIfAny() {
  if (recording) {
    recording = false;
    const btn = document.getElementById("action-record") as HTMLButtonElement;
    btn.classList.remove("recording");
    btn.textContent = "● 开始录制";
    await invoke("stop_record").catch(() => {});
  }
}

// ---- 消息中心 ----
function renderBadge() {
  document.getElementById("msg-badge")!.classList.toggle("hidden", !unread);
}

function markRead() {
  unread = false;
  renderBadge();
  void invoke("mark_results_read");
}

function tabLabel(r: CommandResult): string {
  return r.name ? `${r.label} · ${r.name}` : `${r.label} · ${r.trigger}`;
}

function msgField(k: string, v: string, code = false): HTMLElement {
  const row = el("div", "msg-field");
  row.append(el("span", "msg-k", k));
  const val = code ? el("pre", "msg-v") : el("span", "msg-v");
  val.textContent = v;
  row.append(val);
  return row;
}

function resultCard(r: CommandResult): HTMLElement {
  const card = el("div", "msg-card");
  card.append(msgField("快捷键", r.trigger));
  if (r.name) card.append(msgField("名称", r.name));
  card.append(msgField("时间", r.time));
  card.append(msgField("命令", r.command, true));
  card.append(msgField("stdout", r.stdout, true));
  if (r.stderr) card.append(msgField("stderr", r.stderr, true));
  card.append(msgField("退出码", r.exit_code === null ? "—" : String(r.exit_code)));
  return card;
}

function renderMessages() {
  renderBadge();

  // 冲突标签角标 + 冲突清单（常驻刷新，无论当前在哪个标签）。
  const conflictCount = document.getElementById("msg-conflict-count")!;
  conflictCount.textContent = String(conflicts.length);
  conflictCount.classList.toggle("hidden", conflicts.length === 0);
  renderConflictList();

  // 视图切换：命令结果 / 冲突。
  const onResults = msgView === "results";
  document.getElementById("msg-results-view")!.classList.toggle("hidden", !onResults);
  document.getElementById("msg-conflicts-view")!.classList.toggle("hidden", onResults);
  document.getElementById("msg-view-results")!.classList.toggle("active", onResults);
  document.getElementById("msg-view-conflicts")!.classList.toggle("active", !onResults);
  document.getElementById("clear-messages")!.classList.toggle("hidden", !onResults);

  // 命令结果视图内容（tablist / 空态 / 内容卡）。
  const tablist = document.getElementById("msg-tablist")!;
  const empty = document.getElementById("msg-empty")!;
  const content = document.getElementById("msg-content")!;
  tablist.replaceChildren();
  const has = messages.length > 0;
  empty.classList.toggle("hidden", has);
  content.classList.toggle("hidden", !has);
  (document.getElementById("msg-prev") as HTMLButtonElement).disabled = !has;
  (document.getElementById("msg-next") as HTMLButtonElement).disabled = !has;
  if (!has) return;
  if (msgIndex >= messages.length) msgIndex = 0;
  if (msgIndex < 0) msgIndex = messages.length - 1;
  messages.forEach((r, i) => {
    const t = el("button", "msg-tab" + (i === msgIndex ? " active" : ""), tabLabel(r));
    t.addEventListener("click", () => {
      msgIndex = i;
      renderMessages();
    });
    tablist.append(t);
  });
  const active = tablist.children[msgIndex] as HTMLElement | undefined;
  active?.scrollIntoView({ block: "nearest", inline: "nearest" });
  content.replaceChildren(resultCard(messages[msgIndex]));
}

// 弹窗打开前的焦点：关掉后还回去（还给已重绘消失的元素等于没还，isConnected 挡掉）。
let resultModalPrevFocus: HTMLElement | null = null;

function showResultModal(r: CommandResult) {
  document.getElementById("modal-title")!.textContent = `命令结果 · ${r.label}`;
  document.getElementById("modal-body")!.replaceChildren(resultCard(r));
  const modal = document.getElementById("result-modal")!;
  if (modal.classList.contains("hidden")) {
    resultModalPrevFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }
  modal.classList.remove("hidden");
  document.getElementById("modal-close")!.focus();
}

// 停止正在执行的动作：后端返回请求发出时「正在执行的动作链数量」。
// 0 说明此刻没有可停的东西——如实告诉用户，而不是假装停成功了。
async function abortActions() {
  try {
    const running = await invoke<number>("abort_actions");
    toast(running > 0 ? `已请求停止（${running} 条正在执行）` : "当前没有正在执行的动作");
  } catch (e) {
    toast(`停止失败: ${e}`);
  }
}

function hideResultModal() {
  document.getElementById("result-modal")!.classList.add("hidden");
  const prev = resultModalPrevFocus;
  resultModalPrevFocus = null;
  if (prev && prev.isConnected) prev.focus();
}

function bindMessages() {
  document.getElementById("msg-view-results")!.addEventListener("click", () => {
    msgView = "results";
    renderMessages();
  });
  document.getElementById("msg-view-conflicts")!.addEventListener("click", () => {
    msgView = "conflicts";
    renderMessages();
  });
  document.getElementById("abort-actions")!.addEventListener("click", () => void abortActions());
  document.getElementById("msg-prev")!.addEventListener("click", () => {
    if (!messages.length) return;
    msgIndex = (msgIndex - 1 + messages.length) % messages.length;
    renderMessages();
  });
  document.getElementById("msg-next")!.addEventListener("click", () => {
    if (!messages.length) return;
    msgIndex = (msgIndex + 1) % messages.length;
    renderMessages();
  });
  document.getElementById("clear-messages")!.addEventListener("click", () => {
    void invoke("clear_command_results");
    messages = [];
    msgIndex = 0;
    renderMessages();
  });
  document.getElementById("modal-close")!.addEventListener("click", hideResultModal);
  document.querySelector(".modal-mask")!.addEventListener("click", hideResultModal);
}

async function initEvents() {
  await listen<CommandResult>("command-result", (event) => {
    const r = event.payload;
    messages.unshift(r);
    msgIndex = 0;
    if (r.show_output) {
      showResultModal(r);
    } else {
      unread = true;
    }
    renderMessages();
  });
  await listen<UpdateStatus>("update-status", (event) => {
    updateStatus = event.payload;
    syncUpdate();
  });
  // 配置文件被外部改动后由后端广播（见 applyExternalConfig）。
  await listen<Config>("config-changed", (event) => {
    void applyExternalConfig(event.payload);
  });
  // 应用内别的窗口写盘后广播（`set_config` / 导入 / 提示框窗口拖了位置调了滚轮 / 托盘暂停）：
  // 只跟上「本界面不是唯一写入者」的那几块——它们在别处被改过，本界面手上这份已经旧了，
  // 下次保存会把旧值写回去（拖好的位置、调好的透明度、托盘刚切的暂停被打回原形）。
  // 其余字段本界面自己就是权威。
  await listen<Config>("config-updated", (event) => {
    const next = event.payload.settings?.hints;
    if (next) {
      cfg.settings.hints = next;
      // 只动这一个勾：整份 `syncSettings()` 会把用户正在输入框里敲到一半的超时数字覆盖掉。
      (document.getElementById("set-hints") as HTMLInputElement).checked = next.visible === true;
    }
    const paused = event.payload.settings?.paused === true;
    if (paused !== cfg.settings.paused) {
      cfg.settings.paused = paused;
      (document.getElementById("set-paused") as HTMLInputElement).checked = paused;
    }
  });
}

// 右下角触发气泡窗口（独立隐藏窗口，加载 index.html#toast）：
// 只渲染气泡，不听主界面逻辑。
async function bootstrapToast() {
  document.querySelector(".layout")?.remove();
  document.querySelector("#result-modal")?.remove();
  document.querySelector("#save-status")?.remove();
  const bubble = el("div", "toast-bubble");
  const title = el("div", "toast-title");
  const sub = el("div", "toast-sub");
  bubble.append(title, sub);
  document.body.append(bubble);
  // subtitle 由后端给：快捷键触发的气泡不带（显示「触发键 · 正在执行…」），
  // 「发现新版本」这类通知带自定义副标题。
  type ToastPayload = { name: string; trigger: string; subtitle?: string };
  const render = (payload: ToastPayload | null) => {
    const name = payload?.name ?? "";
    const trigger = payload?.trigger ?? "";
    title.textContent = name || trigger;
    sub.textContent =
      payload?.subtitle ?? (name ? `${trigger} · ` : "") + "正在执行…";
  };
  await listen<ToastPayload>("toast-show", (e) => render(e.payload));
  // 首次懒创建后补一次拉取：窗口刚建好时 toast-show 事件可能早于监听器注册，
  // 用后端暂存的载荷兜底，保证第一次触发也有内容。
  render(await invoke<ToastPayload | null>("get_toast_payload").catch(() => null));
}

// 输入状态悬浮指示窗（独立隐藏窗口，加载 index.html#hud）：
// 显示「当前激活层 + 注入中的键（按住 / 粘滞 / 单次）」。显示与隐藏全由后端决定（有东西
// 生效才亮出来、没有就藏回去），这里只负责画内容并把量到的尺寸回报给后端定位。
async function bootstrapHud() {
  document.querySelector(".layout")?.remove();
  document.querySelector("#result-modal")?.remove();
  document.querySelector("#save-status")?.remove();
  const box = el("div", "hud-box");
  const row = el("div", "hud-row");
  box.append(row);
  document.body.append(box);
  const MOD_KIND_LABEL: Record<string, string> = { hold: "按住", sticky: "粘滞", oneshot: "单次" };
  const render = (payload: StatusPayload | null) => {
    row.replaceChildren();
    if (payload?.layer) {
      const chip = el("span", "hud-chip hud-layer");
      chip.textContent = `层 · ${payload.layer}${payload.locked ? "（锁定）" : "（按住）"}`;
      row.append(chip);
    }
    for (const m of payload?.mods ?? []) {
      const chip = el("span", `hud-chip hud-${m.kind}`);
      chip.textContent = `${MOD_KIND_LABEL[m.kind] ?? m.kind} ${m.key}`;
      row.append(chip);
    }
    // 量完内容再回报尺寸：窗口要贴屏幕底部居中，宽高得先知道（多给 2px 余量，宁可多留一点
    // 透明边也不能把内容裁掉——`.hud-row` 居中，多出来的部分是看不见的）。
    const rect = row.getBoundingClientRect();
    void invoke("hud_ready", {
      width: Math.ceil(rect.width) + 14,
      height: Math.ceil(rect.height) + 14,
    }).catch(() => {});
  };
  await listen<StatusPayload>("status-change", (e) => render(e.payload));
  // 首次懒创建后补一次拉取：窗口刚建好时 status-change 事件可能早于监听器注册
  // （与触发气泡同一个套路）。
  render(await invoke<StatusPayload | null>("get_status_payload").catch(() => null));
}

// 快捷键提示框（独立窗口，加载 index.html#hints，见规划 7.3-㊳）：
// 常显的快捷键速查表——内容来自配置（按层分组），位置 / 字号 / 透明度由后端持久化。
// 交互：拖标题栏移动、滚轮调大小、Ctrl+滚轮调透明度、Shift+滚轮滚动长列表、× 关闭。
const HINTS_SCALE_RANGE: [number, number] = [60, 220];
const HINTS_OPACITY_RANGE: [number, number] = [20, 100];

function clampNum(n: number, [lo, hi]: [number, number]): number {
  return Math.min(hi, Math.max(lo, n));
}

async function bootstrapHints() {
  document.querySelector(".layout")?.remove();
  document.querySelector("#result-modal")?.remove();
  document.querySelector("#save-status")?.remove();

  const panel = el("div", "hints-panel");
  const head = el("div", "hints-head");
  head.append(el("span", "hints-title", "快捷键速查"));
  const close = el("button", "hints-close", "×");
  close.title = "关闭提示框（可从设置页或托盘菜单重新打开）";
  head.append(close);
  const body = el("div", "hints-body");
  const foot = el(
    "div",
    "hints-foot",
    // 两行是量着排的，别合回一行：21em 面板的内容宽只有 ~250px，这串提示单行要 288px，
    // 挤成一行末尾的「拖标题移动」就会被省略号吃掉（`style.css` 里 `.hints-foot` 配合）。
    "滚轮：大小 · Ctrl+滚轮：透明度\nShift+滚轮：滚动 · 拖标题移动",
  );
  panel.append(head, body, foot);
  document.body.append(panel);
  // 窗口尺寸按 `.hints-panel` 量：面板外留一圈透明边给投影（transparent 窗口会把投影
  // 一起裁掉，除非窗口本身比面板大）。**这一圈必须大于 CSS 投影的扩散半径**（见
  // `.hints-panel` 的 `box-shadow: 0 2px 8px` → 最远约 10px），留小了投影会在窗口边界上
  // 被切出一条直边。窗口也不借系统投影（`ensure_hints` 里 `shadow(false)`）——否则 tao
  // 给无边框窗保留的那圈 DWM 边框会挂在这条透明边上，像外面多套了一个空框。
  const PANEL_MARGIN = 10;

  let scale = 100;
  let opacity = 92;
  let winPos = { x: 0, y: 0 };

  const applyLook = () => {
    // 整块面板按比例缩放：字号是基准，间距 / 圆角 / 宽度都用 em，于是「调大小」是一个量。
    document.documentElement.style.setProperty("--hints-scale", String(scale / 100));
    panel.style.opacity = String(opacity / 100);
  };

  /** 量出面板尺寸回报后端（窗口大小由内容决定；内容随快捷键条数与字号变）。 */
  const measure = () => {
    // 列表上限按**屏幕**可用高度算，绝不能用 `100vh`：窗口高度正是这里要量出来的东西，
    // 拿 vh 当上限会形成「量出高度 → 撑大窗口 → 上限跟着变高 → 再量更大」的自增长循环
    // （每滚一次轮就长一截，直到后端 2000px 的钳制才停）。屏幕高度与窗口无关，才是真上限。
    const chrome = head.offsetHeight + foot.offsetHeight + PANEL_MARGIN * 2;
    const cap = Math.max(140, Math.round(window.screen.availHeight * 0.82) - chrome);
    body.style.maxHeight = `${cap}px`;
    const rect = panel.getBoundingClientRect();
    void invoke<HintsState | null>("hints_ready", {
      width: Math.ceil(rect.width) + PANEL_MARGIN * 2,
      height: Math.ceil(rect.height) + PANEL_MARGIN * 2,
    })
      .then((st) => {
        if (st) winPos = { x: st.x, y: st.y };
      })
      .catch(() => {});
  };

  /** 一条触发键 → kbd 片段。组合键 `+`、和弦 `&`、序列空格各按原样显示为分隔。 */
  const triggerEl = (trigger: string): HTMLElement => {
    const wrap = el("span", "hints-trigger");
    for (const tok of trigger.split(/([+& ])/)) {
      if (!tok) continue;
      if (tok === "+" || tok === "&") wrap.append(el("span", "hints-sep", tok));
      // 序列的空格用不断行细空格：普通空格在行内会被折叠掉，只剩两侧 padding，看不出来是两步。
      else if (tok === " ") wrap.append(el("span", "hints-sep", "\u2009\u2009"));
      else wrap.append(el("kbd", "hints-key", tok));
    }
    return wrap;
  };

  const render = (c: Config) => {
    body.replaceChildren();
    const base: ShortcutItem[] = [];
    const byLayer = new Map<string, ShortcutItem[]>();
    for (const s of c.shortcuts) {
      // 只列启用的：提示框是「现在能按什么」，停用条目混在里面只会误导。
      if (!s.enabled) continue;
      const known = s.layer && c.layers.some((l) => l.id === s.layer);
      if (known) {
        const arr = byLayer.get(s.layer!) ?? [];
        arr.push(s);
        byLayer.set(s.layer!, arr);
      } else {
        base.push(s);
      }
    }
    const groups: { title: string; items: ShortcutItem[] }[] = [];
    if (base.length) groups.push({ title: "基础层", items: base });
    for (const l of c.layers) {
      const items = byLayer.get(l.id);
      if (items?.length) groups.push({ title: l.name, items });
    }
    if (!groups.length) {
      body.append(el("div", "hints-empty", "还没有启用的快捷键"));
    }
    for (const g of groups) {
      const group = el("div", "hints-group");
      group.append(el("div", "hints-group-title", g.title));
      for (const s of g.items) {
        const row = el("div", "hints-item");
        const keys = el("span", "hints-keys");
        for (const t of s.triggers) keys.append(triggerEl(t));
        const name = s.name?.trim();
        const desc = s.description?.trim();
        const label = el("span", "hints-label", name || desc || "（未命名）");
        // 只放得下名字时，把描述挂成悬停提示——不额外占一行高度。
        if (name && desc) label.title = desc;
        row.append(keys, label);
        group.append(row);
      }
      body.append(group);
    }
    measure();
  };

  /** 滚轮改完的偏好落盘（节流）：滚轮一格一条 IPC 落盘太浪费，松手 400ms 后记一次。 */
  let persistTimer: number | null = null;
  const schedulePersist = () => {
    if (persistTimer !== null) clearTimeout(persistTimer);
    persistTimer = window.setTimeout(() => {
      persistTimer = null;
      void invoke("hints_prefs", { scale: Math.round(scale), opacity: Math.round(opacity) }).catch(
        () => {},
      );
    }, 400);
  };

  panel.addEventListener(
    "wheel",
    (e) => {
      // 一律 preventDefault：三种手势全归本窗口管，别让浏览器再插一脚（Shift+滚轮在 Chromium
      // 里默认是「换轴横滚」，列表横向没得滚，轮子就白转了）。
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        opacity = clampNum(opacity + (e.deltaY < 0 ? 4 : -4), HINTS_OPACITY_RANGE);
      } else if (e.shiftKey) {
        // 长列表自己滚（列表超高时 `.hints-body` 自身可滚）。deltaMode=1 是「行」不是像素。
        const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
        body.scrollTop += dy;
        return;
      } else {
        scale = clampNum(scale + (e.deltaY < 0 ? 5 : -5), HINTS_SCALE_RANGE);
      }
      applyLook();
      measure();
      schedulePersist();
    },
    { passive: false },
  );

  close.addEventListener("click", () => {
    void invoke("hints_set_visible", { visible: false }).catch(() => {});
  });

  // 拖动：用指针事件自己搬窗口，不走系统拖动（`startDragging` 对 `WS_EX_NOACTIVATE` 的窗
  // 不保证好用，而且拖完还得自己读回位置落盘）。差量基于 `screenX/screenY`，用指针捕获
  // 保证光标滑出窗口也继续跟手。
  let drag: { sx: number; sy: number; wx: number; wy: number; nx: number; ny: number } | null = null;
  head.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest(".hints-close")) return;
    e.preventDefault();
    drag = { sx: e.screenX, sy: e.screenY, wx: winPos.x, wy: winPos.y, nx: winPos.x, ny: winPos.y };
    head.setPointerCapture(e.pointerId);
    panel.classList.add("dragging");
  });
  head.addEventListener("pointermove", (e) => {
    if (!drag) return;
    drag.nx = Math.round(drag.wx + (e.screenX - drag.sx));
    drag.ny = Math.round(drag.wy + (e.screenY - drag.sy));
    void invoke("hints_move", { x: drag.nx, y: drag.ny }).catch(() => {});
  });
  const endDrag = (e: PointerEvent) => {
    if (!drag) return;
    winPos = { x: drag.nx, y: drag.ny };
    drag = null;
    panel.classList.remove("dragging");
    try {
      head.releasePointerCapture(e.pointerId);
    } catch {
      /* 指针已经没了，忽略 */
    }
    // 松手才落盘一次（拖动中每帧写盘既慢又浪费）。
    void invoke("hints_commit").catch(() => {});
  };
  head.addEventListener("pointerup", endDrag);
  head.addEventListener("pointercancel", endDrag);

  const st = await invoke<HintsState | null>("get_hints_state").catch(() => null);
  if (st) {
    scale = st.scale;
    opacity = st.opacity;
    winPos = { x: st.x, y: st.y };
  }
  applyLook();
  render(await invoke<Config>("get_config"));
  // 外观被别处改掉（一次都没拖过的窗口从设置里重开、配置被外部改动）时跟上。
  await listen<HintsState>("hints-state", (e) => {
    scale = e.payload.scale;
    opacity = e.payload.opacity;
    applyLook();
    measure();
  });
  // 快捷键改了就重画：应用内保存走 `config-updated`，手改 JSON 走 `config-changed`。
  await listen<Config>("config-updated", (e) => render(e.payload));
  await listen<Config>("config-changed", (e) => render(e.payload));
}

if (location.hash === "#toast") {
  void bootstrapToast();
} else if (location.hash === "#hud") {
  // 演示模式只在没有 Tauri 壳时生效（`index.html#hud` 纯浏览器直开时给一份样例载荷，
  // 方便调样式；壳内自动跳过）。触发气泡那边不装，因为它没有可预览的静态内容。
  installBrowserMock();
  void bootstrapHud();
} else if (location.hash === "#hints") {
  // 同 `#hud`：提示框也能纯浏览器直开预览样式（演示 IPC 给一份样例配置与状态）。
  installBrowserMock();
  void bootstrapHints();
} else {
  installBrowserMock();
  bind();
  bindMessages();
  void (async () => {
    await initEvents();
    await load();
  })();
}
