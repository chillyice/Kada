import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openFile, save as saveFile, ask } from "@tauri-apps/plugin-dialog";

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
  | { kind: "window_title_contains"; text: string };
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
  | { type: "if"; condition: Condition; then: Action[]; otherwise: Action[]; description?: string };
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
  tap_timeout_ms: number;
  oneshot: string | null;
  sticky: string | null;
  tap2: string | null;
  tap3: string | null;
  enabled: boolean;
};
type TextExpansion = { trigger: string; replace: string; enabled: boolean };
type Settings = { autostart: boolean; paused: boolean; wake_key?: string | null };
type Config = { folders: Folder[]; layers: Layer[]; shortcuts: ShortcutItem[]; remaps: Remap[]; expansions: TextExpansion[]; settings: Settings };
type Conflict = { severity: "error" | "warn"; message: string };
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
  { value: "text", label: "文本（输入 / 转大小写）" },
  { value: "command", label: "执行命令" },
  { value: "keys", label: "按键组合" },
  { value: "pause_ms", label: "延迟" },
  { value: "os", label: "操作系统（文件/目录）" },
  { value: "app", label: "应用（打开/关闭/状态/重启）" },
  { value: "if", label: "条件判断" },
];

// 动作类型图标（收起态展示用）。
const ACTION_ICONS: Record<Action["type"], string> = {
  text: "✏️",
  command: "💻",
  keys: "⌨️",
  pause_ms: "⏱️",
  os: "📁",
  app: "🚀",
  if: "🔀",
};

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
    case "keys":
      return { type: "keys", keys: [] };
    case "pause_ms":
      return { type: "pause_ms", ms: 0 };
    case "os":
      return { type: "os", operation: newOsOp("copy") };
    case "app":
      return { type: "app", operation: newAppOp("launch") };
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
    case "keys":
      return a.keys.length > 0;
    case "pause_ms":
      return a.ms > 0;
    case "os":
      return osHasContent(a.operation);
    case "app":
      return appHasContent(a.operation);
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
    case "keys":
      return `按键：${a.keys.join("+")}`;
    case "pause_ms":
      return `延迟 ${a.ms}ms`;
    case "os":
      return osSummary(a.operation);
    case "app":
      return appSummary(a.operation);
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
  settings: { autostart: false, paused: false },
};
let section: Section = "shortcuts";
let selected: number | null = null; // 当前列表中的选中下标
let draftNew = false; // 当前编辑是否为“新增”未保存项
let draft: ShortcutItem | null = null; // 快捷键编辑草稿（隔离，保存才写回 cfg）
let remapDraft: Remap | null = null; // 改键编辑草稿
let expansionDraft: TextExpansion | null = null; // 文本扩展编辑草稿
let recording = false;
let conflicts: Conflict[] = [];
let search = "";
let messages: CommandResult[] = []; // 消息中心（最新在前）
let msgIndex = 0; // 当前选中的结果 tab
let unread = false; // 未读红点
let clipboard: { kind: "shortcut"; srcIndex: number; cut: boolean } | null = null; // 剪切/复制板
let editingFolderId: string | null = null; // 正在行内改名的目录 id
let editingLayerId: string | null = null; // 正在行内改名的层 id

// ---- 拖拽排序/移动 ----
type DragPayload = { kind: "shortcut"; idx: number } | { kind: "folder"; id: string };
type DropZone =
  | { kind: "root" }
  | { kind: "folder"; folderId: string; pos: "before" | "into" | "after" }
  | { kind: "shortcut"; idx: number; pos: "before" | "after" };
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

// ---- 键名选项（改键下拉用），对齐 kada-core 的 key_name ----
const KEY_OPTIONS = [
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ".split(""),
  ..."0123456789".split(""),
  ...Array.from({ length: 24 }, (_, i) => `F${i + 1}`),
  ",", ".", "/", "\\", ";", '"', "`", "-", "=", "[", "]",
  "Enter", "Esc", "Tab", "Space", "Backspace", "Delete", "Insert", "CapsLock",
  "Shift", "Ctrl", "Alt", "Meta",
  "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
  "NumLock", "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4",
  "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
  "NumpadAdd", "NumpadSubtract", "NumpadMultiply", "NumpadDivide",
  "NumpadDecimal", "NumpadEnter",
  "MediaPlayPause", "MediaPrev", "MediaNext", "VolumeMute", "VolumeDown", "VolumeUp",
  "MouseMiddle", "MouseBack", "MouseForward",
];

// ---- IPC ----
async function load() {
  cfg = await invoke<Config>("get_config");
  messages = await invoke<CommandResult[]>("get_command_results");
  unread = await invoke<boolean>("get_unread");
  await refreshConflicts();
  render();
  syncSettings();
}

async function save() {
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
    return;
  }
  await refreshConflicts();
  const errs = conflicts.filter((c) => c.severity === "error").length;
  const warns = conflicts.filter((c) => c.severity === "warn").length;
  const notes: string[] = [];
  if (ignored.length) notes.push(`已忽略 ${ignored.length} 处无效配置`);
  if (errs || warns) notes.push(`${errs} 处冲突${warns ? `、${warns} 处遮蔽提示` : ""}`);
  if (notes.length) toast(`已保存：${notes.join("；")}`);
  else toast("已保存");
  render();
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
    conflicts = await invoke<Conflict[]>("get_conflicts", {
      config: currentConfigForConflicts(),
    });
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
  Quote: '"', Backquote: "`", Minus: "-", Equal: "=",
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

function startCapture(onCommit: (combo: string) => void, btn: HTMLButtonElement) {
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
    window.removeEventListener("keydown", handler, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入组合键";
  };
  window.addEventListener("keydown", handler, true);
}

// 键序列录入：逐键累积 comboFromEvent 出的组合（空格拼接实时预览），Enter 提交、Esc 取消。
// 序列至少 2 步（单步就是普通组合键，走「录入组合键」）。
function startSequenceCapture(onCommit: (seq: string) => void, btn: HTMLButtonElement) {
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
    window.removeEventListener("keydown", handler, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    btn.textContent = "录入序列";
  };
  window.addEventListener("keydown", handler, true);
  update();
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

// 圆圈问号：hover 显示说明（用于解释「会写入变量」的动作，如取文件属性/查询应用状态）。
function helpIcon(tip: string): HTMLElement {
  const s = el("span", "help", "?");
  s.setAttribute("data-tip", tip);
  return s;
}

// 内联可见的变量引用说明：把「有哪些字段、怎么引用」直接列在输入框旁，替代 hover 才显示的问号。
function varUsage(tip: string): HTMLElement {
  return el("div", "var-usage", tip);
}

function render() {
  renderListPane();
  renderConflicts();
  renderDetail();
  renderMessages();
  if (section === "settings") renderLayers();
}

function renderListPane() {
  document.getElementById("list-pane")!.classList.toggle(
    "hidden",
    section === "settings" || section === "messages" || section === "help",
  );
  document.getElementById("add-folder-btn")!.classList.toggle("hidden", section !== "shortcuts");
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
}

function renderShortcuts() {
  const list = document.getElementById("shortcut-list")!;
  list.replaceChildren();
  const q = search.trim().toLowerCase();
  const matches = (s: ShortcutItem) => {
    const hay = `${s.triggers.join(" ")} ${s.name ?? ""} ${s.description ?? ""} ${s.actions
      .map(actionSummary)
      .join(" ")}`.toLowerCase();
    return !q || hay.includes(q);
  };

  const renderShortcutsIn = (folderId: string | null, depth: number) => {
    cfg.shortcuts.forEach((s, i) => {
      if ((s.folder ?? null) === folderId && matches(s)) {
        list.append(shortcutRow(s, i, depth));
      }
    });
  };

  const searching = q !== "";
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

function shortcutRow(s: ShortcutItem, idx: number, depth: number): HTMLElement {
  const li = el("li", "row" + (selected === idx ? " selected" : ""));
  li.dataset.idx = String(idx);
  // 缩进用 margin 而非 padding：整行盒子随层级右移，层级关系和边框范围都对齐。
  li.style.marginLeft = `${depth * 14}px`;
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
    openDetail(idx);
  });
  return li;
}

function folderRow(f: Folder, depth: number, collapsed: boolean): HTMLElement {
  const li = el("li", "folder-row");
  li.dataset.folderId = f.id;
  li.style.marginLeft = `${depth * 14}px`;
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
  target: { kind: "shortcut"; idx: number } | { kind: "folder"; id: string },
) {
  hideRowMenu();
  const menu = document.getElementById("row-menu")!;
  menu.replaceChildren();
  const items: { label: string; run: () => void }[] = [];
  if (target.kind === "shortcut") {
    items.push(
      { label: "剪切", run: () => cutShortcut(target.idx) },
      { label: "复制", run: () => copyShortcut(target.idx) },
      { label: "删除", run: () => deleteShortcutAt(target.idx) },
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
    b.addEventListener("click", () => {
      hideRowMenu();
      it.run();
    });
    menu.append(b);
  }
  // 菜单紧贴三点按钮：先显示再测宽，右缘对齐按钮右缘（贴近按钮、避免右溢出）。
  menu.classList.remove("hidden");
  const rect = anchor.getBoundingClientRect();
  const w = menu.offsetWidth;
  menu.style.left = `${Math.max(8, rect.right - w)}px`;
  menu.style.top = `${rect.bottom + 4}px`;
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

async function deleteFolder(id: string) {
  const target = cfg.folders.find((f) => f.id === id);
  if (!target) return;
  const ok = await ask(`删除目录「${target.name}」及其中的所有快捷键？`, {
    title: "删除目录",
    kind: "warning",
  });
  if (!ok) return;
  const ids = collectDescendantIds(id);
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
  toast("已复制，可用「粘贴」插入");
}

function cutShortcut(idx: number) {
  clipboard = { kind: "shortcut", srcIndex: idx, cut: true };
  toast("已剪切，可用「粘贴」移动");
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

function deleteShortcutAt(idx: number) {
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

// ---- 拖拽排序/移动 ----
function clearDropIndicators() {
  document
    .querySelectorAll(".drop-before, .drop-after, .drop-into, .drop-forbidden")
    .forEach((n) =>
      n.classList.remove("drop-before", "drop-after", "drop-into", "drop-forbidden"),
    );
  document.getElementById("shortcut-list")!.classList.remove("drop-root");
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
  } else if (zone.kind === "shortcut") {
    newParent = cfg.shortcuts[zone.idx]?.folder ?? null;
  }
  return newParent !== null && collectDescendantIds(selfId).has(newParent);
}

function computeDropZoneAt(x: number, y: number): DropZone | null {
  const list = document.getElementById("shortcut-list")!;
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
    return { kind: "shortcut", idx, pos };
  }
  return { kind: "root" };
}

function showDropIndicator(zone: DropZone) {
  const list = document.getElementById("shortcut-list")!;
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

function moveShortcut(srcIdx: number, targetFolder: string | null, insertIdx?: number) {
  const [moved] = cfg.shortcuts.splice(srcIdx, 1);
  if (!moved) return;
  moved.folder = targetFolder;
  if (insertIdx === undefined) {
    cfg.shortcuts.push(moved);
  } else {
    const at = insertIdx > srcIdx ? insertIdx - 1 : insertIdx;
    cfg.shortcuts.splice(at, 0, moved);
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
  const selObj = selected !== null ? cfg.shortcuts[selected] : null;

  if (dragPayload.kind === "shortcut") {
    const srcIdx = dragPayload.idx;
    if (!cfg.shortcuts[srcIdx]) return;
    if (zone.kind === "root") {
      moveShortcut(srcIdx, null);
    } else if (zone.kind === "folder" && zone.pos === "into") {
      moveShortcut(srcIdx, zone.folderId);
    } else if (zone.kind === "shortcut") {
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
    } else if (zone.kind === "shortcut") {
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
    if (search.trim()) return; // 搜索时禁用拖拽
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
  const q = search.trim().toLowerCase();
  cfg.remaps.forEach((r, i) => {
    const hay = `${r.from} ${remapSummary(r)} ${layerName(r.hold_layer)}`.toLowerCase();
    if (q && !hay.includes(q)) return;

    const li = el("li", "row" + (selected === i ? " selected" : ""));
    li.dataset.idx = String(i);
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = r.enabled;
    on.addEventListener("change", (e) => {
      e.stopPropagation();
      r.enabled = on.checked;
      void save();
    });

    const isTiming = !!(r.tap || r.hold || r.hold_layer || r.oneshot || r.sticky || r.tap2 || r.tap3);
    li.append(
      on,
      el("span", "triggers", r.from),
      el("span", "arrow", isTiming ? "⇥" : "→"),
      el("span", "triggers", remapSummary(r)),
    );
    li.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("input, button")) return;
      openDetail(i);
    });
    list.append(li);
  });
}

function renderExpansions() {
  const list = document.getElementById("expansion-list")!;
  list.replaceChildren();
  const q = search.trim().toLowerCase();
  cfg.expansions.forEach((e, i) => {
    const hay = `${e.trigger} ${e.replace}`.toLowerCase();
    if (q && !hay.includes(q)) return;

    const li = el("li", "row" + (selected === i ? " selected" : ""));
    li.dataset.idx = String(i);
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
    );
    li.addEventListener("click", (ev) => {
      if ((ev.target as HTMLElement).closest("input, button")) return;
      openDetail(i);
    });
    list.append(li);
  });
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
}

function openDetail(i: number, isNew = false) {
  selected = i;
  draftNew = isNew;
  if (section === "shortcuts") {
    draft = isNew
      ? { name: "", description: "", folder: null, layer: null, triggers: [], actions: [newAction("text")], enabled: true }
      : deepClone(cfg.shortcuts[i]);
    // 打开已创建的、含多个动作的快捷键时，动作默认收起；新建/新增的动作保持展开。
    if (!isNew && draft.actions.length > 1) collapseAllActions(draft.actions);
    (document.getElementById("edit-desc") as HTMLInputElement).value = draft.description ?? "";
    fillLayerSelect(document.getElementById("edit-layer") as HTMLSelectElement, draft.layer ?? null, "assign");
    renderTriggers(draft);
    renderActions(draft);
    setShortcutTitle();
  } else if (section === "remaps") {
    remapDraft = isNew
      ? { from: "CapsLock", to: "Ctrl", tap: null, hold: null, layer: null, hold_layer: null, tap_timeout_ms: 200, oneshot: null, sticky: null, tap2: null, tap3: null, enabled: true }
      : deepClone(cfg.remaps[i]);
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
    (document.getElementById("remap-timeout") as HTMLInputElement).value = String(
      remapDraft.tap_timeout_ms || 200,
    );
    syncRemapEditor();
    document.getElementById("remap-title")!.textContent = draftNew
      ? "新增改键"
      : `${remapDraft.from} → ${remapSummary(remapDraft)}`;
  } else if (section === "expansions") {
    expansionDraft = isNew
      ? { trigger: "", replace: "", enabled: true }
      : deepClone(cfg.expansions[i]);
    (document.getElementById("edit-exp-trigger") as HTMLInputElement).value =
      expansionDraft.trigger;
    (document.getElementById("edit-exp-replace") as HTMLTextAreaElement).value =
      expansionDraft.replace;
    document.getElementById("expansion-title")!.textContent = draftNew
      ? "新增文本扩展"
      : expansionDraft.trigger || "（未命名）";
  }
  renderListPane();
  renderDetail();
}

function closeDetail() {
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
      void refreshConflicts().then(renderConflicts);
    });
    chip.append(x);
    wrap.append(chip);
  });
}

// ---- 动作列表渲染 ----
function renderActions(s: ShortcutItem) {
  const wrap = document.getElementById("action-list")!;
  renderActionList(wrap, s.actions, () => renderActions(s));
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

// 把一个动作列表渲染进 container；任何结构变化（增删/移动/改类型/拖拽）都通过 rerender()
// 触发整体重绘。嵌套的「条件判断」动作同样用它递归渲染 then/otherwise 分支。
// 收起态只显示 序号 + 类型图标 + 描述；展开态为三行：标题行 / 类型筛选行 / 输入行。
function renderActionList(
  container: HTMLElement,
  actions: Action[],
  rerender: () => void,
): void {
  container.replaceChildren();
  actions.forEach((a, i) => {
    const collapsed = collapsedActions.has(a);
    const row = el("div", "action-row" + (collapsed ? " collapsed" : ""));
    row.dataset.idx = String(i);
    // 拖拽命中计算用：把「所在列表 + 下标」直接挂在元素上（非序列化属性）。
    (row as unknown as { _dropList?: Action[] })._dropList = actions;
    (row as unknown as { _dropIndex?: number })._dropIndex = i;

    // 第一行：拖动手柄 + 序号 + 图标 + 标题（描述） + 删除
    const head = el("div", "action-head");

    const grip = el("span", "action-grip", "⋮⋮");
    grip.title = "拖动排序 / 嵌套；点击展开或收起";
    head.append(grip);

    const seq = el("span", "action-seq", String(i + 1));
    head.append(seq);

    const ico = el("span", "action-ico", ACTION_ICONS[a.type]);
    head.append(ico);

    // 拖拽 + 点击展开/收起：手柄、序号、图标都可触发（扩大命中范围）。
    for (const handle of [grip, seq, ico]) {
      attachActionDragStart(handle, row, actions, i);
    }

    if (collapsed) {
      const title = el("span", "action-title", actionDescription(a));
      title.title = "点击展开";
      title.addEventListener("click", () => {
        toggleActionCollapsed(a);
        rerender();
      });
      head.append(title);
    } else {
      const titleInput = el("input", "action-input action-title-input") as HTMLInputElement;
      titleInput.type = "text";
      titleInput.value = a.description ?? "";
      titleInput.placeholder = "动作标题（可选）";
      titleInput.addEventListener("input", () => {
        a.description = titleInput.value.trim() || undefined;
      });
      head.append(titleInput);

      const del = el("button", "action-del") as HTMLButtonElement;
      del.title = "删除动作";
      del.innerHTML =
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>';
      del.addEventListener("click", () => {
        actions.splice(i, 1);
        collapsedActions.delete(a);
        rerender();
      });
      head.append(del);
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

    container.append(row);
  });
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
      a.ms = parseInt(num.value, 10) || 0;
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
        op.retries = parseInt(retries.value, 10) || 0;
      });
      const ival = el("input", "action-input action-input-num") as HTMLInputElement;
      ival.type = "number";
      ival.min = "0";
      ival.value = String(op.interval_ms);
      ival.addEventListener("input", () => {
        op.interval_ms = parseInt(ival.value, 10) || 0;
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
        cond.minutes = parseInt(mins.value, 10) || 0;
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

    body.append(el("div", "branch-label", "满足条件时执行"));
    const thenList = el("div", "action-branch");
    (thenList as unknown as { _dropList?: Action[] })._dropList = a.then;
    renderActionList(thenList, a.then, rerender);
    const addThen = el("button", "add-inline", "＋ 添加动作");
    addThen.addEventListener("click", () => {
      a.then.push(newAction("text"));
      rerender();
    });
    body.append(thenList, addThen);

    body.append(el("div", "branch-label", "否则执行（可空）"));
    const elseList = el("div", "action-branch");
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
  input.addEventListener("input", () => onInput(input.value));
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
function renderConflicts() {
  const listEl = document.getElementById("conflict-list")!;
  listEl.replaceChildren();
  if (conflicts.length === 0) return;
  listEl.append(el("div", "conflict-head", "⚠ 冲突提醒"));
  for (const c of conflicts) {
    const d = document.createElement("div");
    d.className = "conflict " + (c.severity === "error" ? "error" : "warn");
    d.textContent = c.message;
    listEl.append(d);
  }
}

// ---- 设置 ----
function syncSettings() {
  (document.getElementById("set-autostart") as HTMLInputElement).checked = cfg.settings.autostart;
  (document.getElementById("set-paused") as HTMLInputElement).checked = cfg.settings.paused;
  (document.getElementById("set-wake-key") as HTMLSelectElement).value = cfg.settings.wake_key ?? "";
}

function bindSettings() {
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const wakeKey = document.getElementById("set-wake-key") as HTMLSelectElement;
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
  document.getElementById("export-config")!.addEventListener("click", () => void exportConfig());
  document.getElementById("import-config")!.addEventListener("click", () => void importConfig());
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

async function importConfig() {
  const path = await openFile({
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path || Array.isArray(path)) return;
  try {
    const ignored = await invoke<string[]>("import_config", { path });
    await load();
    toast(ignored.length ? `已导入（忽略 ${ignored.length} 处无效配置）` : "已导入");
  } catch (e) {
    toast(`导入失败: ${e}`);
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
}

// 修饰键下拉（单次/粘滞只接受 Ctrl/Alt/Shift/Meta）。
const MOD_OPTIONS = ["Ctrl", "Alt", "Shift", "Meta"];

function fillModifierSelect(sel: HTMLSelectElement, value: string | null) {
  sel.replaceChildren();
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = "（无）";
  sel.append(empty);
  for (const m of MOD_OPTIONS) {
    const o = document.createElement("option");
    o.value = m;
    o.textContent = m;
    sel.append(o);
  }
  sel.value = value ?? "";
}

// 根据所选形态，切换「单次/粘滞修饰」「短按/双击/三击/长按/切层 + 阈值」与「普通改键」表单显示。
function syncRemapEditor() {
  if (!remapDraft) return;
  const isModifier = !!(remapDraft.oneshot || remapDraft.sticky);
  const isTapHold = !!(remapDraft.tap || remapDraft.hold || remapDraft.tap2 || remapDraft.tap3);
  const isLayerKey = !!remapDraft.hold_layer;
  // 修饰模式与 tap-hold/切层/普通改键互斥：隐藏后者；反之隐藏修饰下拉无意义（保留可见）。
  document.getElementById("remap-tap-row")!.classList.toggle("hidden", isModifier);
  document.getElementById("remap-hold-row")!.classList.toggle("hidden", isModifier);
  document.getElementById("remap-hold-layer-row")!.classList.toggle("hidden", isModifier);
  document.getElementById("remap-tap2-row")!.classList.toggle("hidden", isModifier || !isTapHold);
  document.getElementById("remap-tap3-row")!.classList.toggle("hidden", isModifier || !isTapHold);
  document.getElementById("remap-timeout-row")!.classList.toggle("hidden", isModifier || !isTapHold);
  document.getElementById("remap-to-row")!.classList.toggle("hidden", isModifier || isTapHold || isLayerKey);
}

// 填充「所属层」（assign）/「长按进入层」（hold）下拉。
function fillLayerSelect(sel: HTMLSelectElement, value: string | null, mode: "assign" | "hold") {
  sel.replaceChildren();
  const empty = document.createElement("option");
  empty.value = "";
  empty.textContent = mode === "hold" ? "（不切层）" : "基层层（始终生效）";
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
  if (!id) return "基层层";
  return cfg.layers.find((l) => l.id === id)?.name ?? "基层层";
}

// 改键形态的人类可读摘要（列表行 + 详情标题共用），与 kada-core 的 Remap::describe 对齐。
function remapSummary(r: Remap): string {
  if (r.sticky) return `粘滞 ${r.sticky}`;
  if (r.oneshot) return `单击 ${r.oneshot}`;
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
  const ok = await ask(`删除层「${target.name}」？该层的快捷键/改键会回到基层层，指向它的切层键会失效。`, {
    title: "删除层",
    kind: "warning",
  });
  if (!ok) return;
  cfg.layers = cfg.layers.filter((l) => l.id !== id);
  cfg.shortcuts.forEach((s) => {
    if (s.layer === id) s.layer = null;
  });
  cfg.remaps.forEach((r) => {
    if (r.layer === id) r.layer = null;
    if (r.hold_layer === id) r.hold_layer = null;
  });
  if (editingLayerId === id) editingLayerId = null;
  void save();
}

// ---- 事件绑定 ----
function bind() {
  document.querySelectorAll<HTMLButtonElement>(".rail-item").forEach((b) => {
    b.addEventListener("click", () => switchSection(b.dataset.tab as Section));
  });

  document.getElementById("add-btn")!.addEventListener("click", () => {
    if (section === "shortcuts") {
      openDetail(-1, true);
    } else if (section === "remaps") {
      openDetail(-1, true);
    } else if (section === "expansions") {
      openDetail(-1, true);
    }
  });

  document.getElementById("add-folder-btn")!.addEventListener("click", () => {
    if (section === "shortcuts") addFolder(null);
  });

  document.getElementById("add-layer-btn")!.addEventListener("click", addLayer);

  document.getElementById("list-search")!.addEventListener("input", (e) => {
    search = (e.target as HTMLInputElement).value;
    renderListPane();
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
      void refreshConflicts().then(renderConflicts);
    }, btn);
  });

  document.getElementById("edit-sequence")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || !draft) return;
    const d = draft;
    startSequenceCapture((seq) => {
      if (!d.triggers.includes(seq)) d.triggers.push(seq);
      renderTriggers(d);
      void refreshConflicts().then(renderConflicts);
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
    if (section !== "shortcuts" || !draft) return;
    void stopRecIfAny();
    draft.description = (document.getElementById("edit-desc") as HTMLInputElement).value || null;
    draft.layer = (document.getElementById("edit-layer") as HTMLSelectElement).value || null;
    if (draftNew) cfg.shortcuts.unshift(draft);
    else if (selected !== null) cfg.shortcuts[selected] = draft;
    draftNew = false;
    const saved = draft;
    void save().then(() => flashRow(cfg.shortcuts.indexOf(saved), "shortcut-list"));
    // 保存后留在编辑页（不清空 draft）；若空的新条目被保存清洗剔除，则回落到关闭编辑页。
    selected = cfg.shortcuts.indexOf(saved);
    if (selected < 0) {
      draft = null;
      selected = null;
    }
  });

  document.getElementById("cancel-shortcut")!.addEventListener("click", closeDetail);
  document.getElementById("detail-close")!.addEventListener("click", closeDetail);
  document.getElementById("delete-shortcut")!.addEventListener("click", () => {
    if (section !== "shortcuts" || !draft) return;
    if (draftNew) {
      // 新增项尚未落盘，删除 = 直接丢弃草稿。
      discardDraft();
      draftNew = false;
      selected = null;
      render();
      return;
    }
    if (selected !== null) cfg.shortcuts.splice(selected, 1);
    draft = null;
    draftNew = false;
    selected = null;
    void save();
  });

  document.getElementById("save-remap")!.addEventListener("click", () => {
    if (section !== "remaps" || !remapDraft) return;
    remapDraft.from = (document.getElementById("remap-from") as HTMLSelectElement).value;
    remapDraft.tap = (document.getElementById("remap-tap") as HTMLSelectElement).value || null;
    remapDraft.hold = (document.getElementById("remap-hold") as HTMLSelectElement).value || null;
    remapDraft.oneshot =
      (document.getElementById("remap-oneshot") as HTMLSelectElement).value || null;
    remapDraft.sticky = (document.getElementById("remap-sticky") as HTMLSelectElement).value || null;
    remapDraft.tap2 = (document.getElementById("remap-tap2") as HTMLSelectElement).value || null;
    remapDraft.tap3 = (document.getElementById("remap-tap3") as HTMLSelectElement).value || null;
    remapDraft.layer = (document.getElementById("remap-layer") as HTMLSelectElement).value || null;
    remapDraft.hold_layer =
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value || null;
    remapDraft.tap_timeout_ms =
      parseInt((document.getElementById("remap-timeout") as HTMLInputElement).value, 10) || 200;
    // 任一非普通改键形态（tap-hold/切层/单次/粘滞/连击）→ 清空「改为」；否则用「改为」。
    remapDraft.to = remapDraft.tap ||
      remapDraft.hold ||
      remapDraft.hold_layer ||
      remapDraft.oneshot ||
      remapDraft.sticky ||
      remapDraft.tap2 ||
      remapDraft.tap3
      ? ""
      : (document.getElementById("remap-to") as HTMLSelectElement).value;
    if (draftNew) cfg.remaps.unshift(remapDraft);
    else if (selected !== null) cfg.remaps[selected] = remapDraft;
    const saved = remapDraft;
    remapDraft = null;
    draftNew = false;
    selected = null;
    void save().then(() => flashRow(cfg.remaps.indexOf(saved), "remap-list"));
  });

  document.getElementById("cancel-remap")!.addEventListener("click", closeDetail);
  document.getElementById("remap-close")!.addEventListener("click", closeDetail);
  document.getElementById("delete-remap")!.addEventListener("click", () => {
    if (section !== "remaps" || !remapDraft) return;
    if (draftNew) {
      discardDraft();
      draftNew = false;
      selected = null;
      render();
      return;
    }
    if (selected !== null) cfg.remaps.splice(selected, 1);
    remapDraft = null;
    draftNew = false;
    selected = null;
    void save();
  });

  document.getElementById("save-expansion")!.addEventListener("click", () => {
    if (section !== "expansions" || !expansionDraft) return;
    expansionDraft.trigger = (
      document.getElementById("edit-exp-trigger") as HTMLInputElement
    ).value.trim();
    expansionDraft.replace = (document.getElementById("edit-exp-replace") as HTMLTextAreaElement)
      .value;
    if (draftNew) cfg.expansions.unshift(expansionDraft);
    else if (selected !== null) cfg.expansions[selected] = expansionDraft;
    const saved = expansionDraft;
    expansionDraft = null;
    draftNew = false;
    selected = null;
    void save().then(() => flashRow(cfg.expansions.indexOf(saved), "expansion-list"));
  });

  document.getElementById("cancel-expansion")!.addEventListener("click", closeDetail);
  document.getElementById("expansion-close")!.addEventListener("click", closeDetail);
  document.getElementById("delete-expansion")!.addEventListener("click", () => {
    if (section !== "expansions" || !expansionDraft) return;
    if (draftNew) {
      discardDraft();
      draftNew = false;
      selected = null;
      render();
      return;
    }
    if (selected !== null) cfg.expansions.splice(selected, 1);
    expansionDraft = null;
    draftNew = false;
    selected = null;
    void save();
  });

  fillKeySelect(document.getElementById("remap-from") as HTMLSelectElement, "CapsLock");
  fillKeySelect(document.getElementById("remap-to") as HTMLSelectElement, "Ctrl");
  fillKeySelect(document.getElementById("remap-tap") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-hold") as HTMLSelectElement, "", true);
  fillModifierSelect(document.getElementById("remap-oneshot") as HTMLSelectElement, "");
  fillModifierSelect(document.getElementById("remap-sticky") as HTMLSelectElement, "");
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
      remapDraft.hold_layer = null; // 长按输出键与切层互斥
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-hold-layer")!.addEventListener("change", (e) => {
    if (!remapDraft) return;
    remapDraft.hold_layer = (e.target as HTMLSelectElement).value || null;
    if (remapDraft.hold_layer) {
      remapDraft.hold = null;
      (document.getElementById("remap-hold") as HTMLSelectElement).value = "";
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

  bindSettings();
}

function switchSection(tab: Section) {
  void stopRecIfAny();
  discardDraft();
  draftNew = false;
  selected = null;
  section = tab;
  search = "";
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

function showResultModal(r: CommandResult) {
  document.getElementById("modal-title")!.textContent = `命令结果 · ${r.label}`;
  document.getElementById("modal-body")!.replaceChildren(resultCard(r));
  document.getElementById("result-modal")!.classList.remove("hidden");
}

function hideResultModal() {
  document.getElementById("result-modal")!.classList.add("hidden");
}

function bindMessages() {
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
  const render = (payload: { name: string; trigger: string } | null) => {
    const name = payload?.name ?? "";
    const trigger = payload?.trigger ?? "";
    title.textContent = name || trigger;
    sub.textContent = (name ? `${trigger} · ` : "") + "正在执行…";
  };
  await listen<{ name: string; trigger: string }>("toast-show", (e) => render(e.payload));
  // 首次懒创建后补一次拉取：窗口刚建好时 toast-show 事件可能早于监听器注册，
  // 用后端暂存的载荷兜底，保证第一次触发也有内容。
  render(
    await invoke<{ name: string; trigger: string } | null>("get_toast_payload").catch(() => null),
  );
}

if (location.hash === "#toast") {
  void bootstrapToast();
} else {
  bind();
  bindMessages();
  void (async () => {
    await initEvents();
    await load();
  })();
}
