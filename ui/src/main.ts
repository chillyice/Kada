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
  | { kind: "modified_within"; path: string; minutes: number };
type AppOperation =
  | { op: "launch"; program: string; args: string[] }
  | { op: "close"; program: string }
  | { op: "status"; program: string; var: string; retries: number; interval_ms: number }
  | { op: "restart"; program: string; args: string[] };
type TextMode = "input" | "to_upper" | "to_lower";
type Shell = "cmd" | "powershell";
type Action =
  | { type: "text"; text: string; mode: TextMode }
  | { type: "command"; shell: Shell; command: string; show_output: boolean }
  | { type: "keys"; keys: string[] }
  | { type: "pause_ms"; ms: number }
  | { type: "os"; operation: OsOperation }
  | { type: "app"; operation: AppOperation }
  | { type: "if"; condition: Condition; then: Action[]; otherwise: Action[] };
type Folder = { id: string; name: string; parent?: string | null };
type ShortcutItem = {
  name?: string | null;
  description?: string | null;
  folder?: string | null;
  triggers: string[];
  actions: Action[];
  enabled: boolean;
};
type Remap = { from: string; to: string; enabled: boolean };
type Settings = { autostart: boolean; paused: boolean; launch_minimized: boolean; wake_key?: string | null };
type Config = { folders: Folder[]; shortcuts: ShortcutItem[]; remaps: Remap[]; settings: Settings };
type Conflict = { severity: "error" | "warn"; message: string };
type Section = "shortcuts" | "remaps" | "messages" | "settings";
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

// ---- 条件判断的子条件 ----
const CONDITION_TYPES: { value: Condition["kind"]; label: string }[] = [
  { value: "exists", label: "路径存在" },
  { value: "not_exists", label: "路径不存在" },
  { value: "is_file", label: "路径是文件" },
  { value: "is_dir", label: "路径是目录" },
  { value: "modified_within", label: "修改时间在…分钟内" },
  { value: "equals", label: "变量字段等于" },
  { value: "not_equals", label: "变量字段不等于" },
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
      return { type: "command", shell: "cmd", command: "", show_output: false };
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
    case "command":
      return `${a.shell === "cmd" ? "CMD" : "PowerShell"}：${a.command}`;
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

// ---- 状态 ----
let cfg: Config = {
  folders: [],
  shortcuts: [],
  remaps: [],
  settings: { autostart: false, paused: false, launch_minimized: false },
};
let section: Section = "shortcuts";
let selected: number | null = null; // 当前列表中的选中下标
let draftNew = false; // 当前编辑是否为“新增”未保存项
let draft: ShortcutItem | null = null; // 快捷键编辑草稿（隔离，保存才写回 cfg）
let remapDraft: Remap | null = null; // 改键编辑草稿
let recording = false;
let conflicts: Conflict[] = [];
let search = "";
let messages: CommandResult[] = []; // 消息中心（最新在前）
let msgIndex = 0; // 当前选中的结果 tab
let unread = false; // 未读红点
let clipboard: { kind: "shortcut"; srcIndex: number; cut: boolean } | null = null; // 剪切/复制板
let editingFolderId: string | null = null; // 正在行内改名的目录 id

// ---- 拖拽排序/移动 ----
type DragPayload = { kind: "shortcut"; idx: number } | { kind: "folder"; id: string };
type DropZone =
  | { kind: "root" }
  | { kind: "folder"; folderId: string; pos: "before" | "into" | "after" }
  | { kind: "shortcut"; idx: number; pos: "before" | "after" };
let dragPayload: DragPayload | null = null; // 拖拽源
let dragOverEl: HTMLElement | null = null; // 当前高亮目标（用于清 class）
let collapsedFolders = new Set<string>(); // 折叠的目录 id（内存态，重启恢复全展开）

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

// ---- 键名选项（改键下拉用），对齐 kada-core 的 key_name ----
const KEY_OPTIONS = [
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ".split(""),
  ..."0123456789".split(""),
  ...Array.from({ length: 12 }, (_, i) => `F${i + 1}`),
  ",", ".", "/", "\\", ";", '"', "`", "-", "=", "[", "]",
  "Enter", "Esc", "Tab", "Space", "Backspace", "Delete", "Insert", "CapsLock",
  "Shift", "Ctrl", "Alt", "Meta",
  "Home", "End", "PageUp", "PageDown", "Up", "Down", "Left", "Right",
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
};
const CODE_PUNCT: Record<string, string> = {
  Comma: ",", Period: ".", Slash: "/", Backslash: "\\", Semicolon: ";",
  Quote: '"', Backquote: "`", Minus: "-", Equal: "=",
  BracketLeft: "[", BracketRight: "]",
};

function codeToKey(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F(1[0-2]|[1-9])$/.test(code)) return code;
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

function render() {
  renderListPane();
  renderConflicts();
  renderDetail();
  renderMessages();
}

function renderListPane() {
  document.getElementById("list-pane")!.classList.toggle(
    "hidden",
    section === "settings" || section === "messages",
  );
  document.getElementById("add-folder-btn")!.classList.toggle("hidden", section !== "shortcuts");
  const title = document.getElementById("list-title")!;
  const shortcutList = document.getElementById("shortcut-list")!;
  const remapList = document.getElementById("remap-list")!;
  if (section === "shortcuts") {
    title.textContent = "快捷键";
    shortcutList.classList.remove("hidden");
    remapList.classList.add("hidden");
    renderShortcuts();
  } else if (section === "remaps") {
    title.textContent = "改键";
    shortcutList.classList.add("hidden");
    remapList.classList.remove("hidden");
    renderRemaps();
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

  const more = moreButton(() => showRowMenu(li, { kind: "shortcut", idx }));
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
  const more = moreButton(() => showRowMenu(li, { kind: "folder", id: f.id }));
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

function moreButton(onClick: () => void): HTMLButtonElement {
  const b = el("button", "row-more", "⋯") as HTMLButtonElement;
  b.title = "更多";
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick();
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
      { label: "粘贴到其后", run: () => pasteAfterShortcut(target.idx) },
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
  const rect = anchor.getBoundingClientRect();
  menu.style.top = `${rect.bottom + 2}px`;
  menu.style.left = `${rect.left}px`;
  menu.classList.remove("hidden");
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

function pasteAfterShortcut(idx: number) {
  if (!clipboard) return toast("剪贴板为空");
  const src = cfg.shortcuts[clipboard.srcIndex];
  if (!src) return;
  const copy = deepClone(src);
  copy.folder = cfg.shortcuts[idx].folder ?? null;
  cfg.shortcuts.splice(idx + 1, 0, copy);
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
    const hay = `${r.from} ${r.to}`.toLowerCase();
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

    li.append(
      on,
      el("span", "triggers", r.from),
      el("span", "arrow", "→"),
      el("span", "triggers", r.to),
    );
    li.addEventListener("click", (e) => {
      if ((e.target as HTMLElement).closest("input, button")) return;
      openDetail(i);
    });
    list.append(li);
  });
}

function renderDetail() {
  document.getElementById("detail-shortcuts")!.classList.toggle("hidden", section !== "shortcuts");
  document.getElementById("detail-remaps")!.classList.toggle("hidden", section !== "remaps");
  document.getElementById("detail-settings")!.classList.toggle("hidden", section !== "settings");
  document.getElementById("detail-messages")!.classList.toggle("hidden", section !== "messages");

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
      ? { name: "", description: "", folder: null, triggers: [], actions: [newAction("text")], enabled: true }
      : deepClone(cfg.shortcuts[i]);
    (document.getElementById("edit-desc") as HTMLInputElement).value = draft.description ?? "";
    renderTriggers(draft);
    renderActions(draft);
    setShortcutTitle();
  } else if (section === "remaps") {
    remapDraft = isNew
      ? { from: "CapsLock", to: "Ctrl", enabled: true }
      : deepClone(cfg.remaps[i]);
    (document.getElementById("remap-from") as HTMLSelectElement).value = remapDraft.from;
    (document.getElementById("remap-to") as HTMLSelectElement).value = remapDraft.to;
    document.getElementById("remap-title")!.textContent = draftNew
      ? "新增改键"
      : `${remapDraft.from} → ${remapDraft.to}`;
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
  // 草稿隔离：编辑只改 draft/remapDraft，取消/关闭/切换直接丢弃，cfg 不受影响。
  draft = null;
  remapDraft = null;
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

// 把一个动作列表渲染进 container；任何结构变化（增删/移动/改类型）都通过 rerender()
// 触发整体重绘。嵌套的「条件判断」动作同样用它递归渲染 then/otherwise 分支。
function renderActionList(
  container: HTMLElement,
  actions: Action[],
  rerender: () => void,
): void {
  container.replaceChildren();
  actions.forEach((a, i) => {
    const row = el("div", "action-row");
    row.dataset.idx = String(i);

    const head = el("div", "action-head");

    const up = el("button", "move", "↑") as HTMLButtonElement;
    up.disabled = i === 0;
    up.title = "上移";
    up.addEventListener("click", () => {
      [actions[i - 1], actions[i]] = [actions[i], actions[i - 1]];
      rerender();
    });
    const down = el("button", "move", "↓") as HTMLButtonElement;
    down.disabled = i === actions.length - 1;
    down.title = "下移";
    down.addEventListener("click", () => {
      [actions[i + 1], actions[i]] = [actions[i], actions[i + 1]];
      rerender();
    });
    head.append(up, down, el("span", "action-label", `动作 ${i + 1}`));

    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const t of ACTION_TYPES) {
      const o = el("option") as HTMLOptionElement;
      o.value = t.value;
      o.textContent = t.label;
      sel.append(o);
    }
    sel.value = a.type;
    sel.addEventListener("change", () => {
      actions[i] = newAction(sel.value as Action["type"]);
      rerender();
    });
    head.append(sel);

    const del = el("button", "danger", "删除");
    del.addEventListener("click", () => {
      actions.splice(i, 1);
      rerender();
    });
    head.append(del);

    row.append(head);
    row.append(actionFields(a, rerender));
    container.append(row);
  });
}

function actionFields(a: Action, rerender: () => void): HTMLElement {
  const body = el("div", "action-body");
  if (a.type === "text") {
    const modeSel = el("select", "action-type") as HTMLSelectElement;
    for (const m of [
      { value: "input" as TextMode, label: "输入文本" },
      { value: "to_upper" as TextMode, label: "小写转大写（选中/剪贴板）" },
      { value: "to_lower" as TextMode, label: "大写转小写（选中/剪贴板）" },
    ]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = m.value;
      opt.textContent = m.label;
      modeSel.append(opt);
    }
    modeSel.value = a.mode;
    modeSel.addEventListener("change", () => {
      a.mode = modeSel.value as TextMode;
      rerender();
    });
    body.append(modeSel);

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
    const shellSel = el("select", "action-type") as HTMLSelectElement;
    for (const s of [
      { value: "cmd" as Shell, label: "CMD" },
      { value: "powershell" as Shell, label: "PowerShell" },
    ]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = s.value;
      opt.textContent = s.label;
      shellSel.append(opt);
    }
    shellSel.value = a.shell;
    shellSel.addEventListener("change", () => {
      a.shell = shellSel.value as Shell;
      rerender();
    });
    body.append(shellSel);

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
    const opSel = el("select", "action-type") as HTMLSelectElement;
    for (const o of OS_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      opSel.append(opt);
    }
    opSel.value = op.op;
    opSel.addEventListener("change", () => {
      a.operation = newOsOp(opSel.value as OsOperation["op"]);
      rerender();
    });
    body.append(opSel);

    if (op.op === "copy" || op.op === "cut" || op.op === "zip") {
      body.append(pathField(op.source, (v) => (op.source = v), { file: true, dir: true }));
      body.append(pathField(op.dest, (v) => (op.dest = v), { file: true, dir: true }));
    } else if (op.op === "unzip") {
      body.append(pathField(op.source, (v) => (op.source = v), { file: true }));
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true }));
    } else if (op.op === "paste") {
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true }));
    } else if (op.op === "delete") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, dir: true }));
    } else if (op.op === "new_file") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true }));
    } else if (op.op === "new_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true }));
    } else if (op.op === "open_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true }));
    } else if (op.op === "get_file_props") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, dir: true }));
      const line = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = "变量名，后续用 {变量名} 或 {变量名.字段} 引用";
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      line.append(
        el("span", "unit-hint", "变量名"),
        vname,
        helpIcon(
          "写入文件属性变量。可用字段：name 文件名、path 完整路径、dir 所在目录、stem 主名、ext 扩展名、size 大小(字节)、modified 修改时间、is_dir 是否目录。引用 {变量名.字段}；单独 {变量名} 得到完整路径。",
        ),
      );
      body.append(line);
    }
  } else if (a.type === "app") {
    const op = a.operation;
    const opSel = el("select", "action-type") as HTMLSelectElement;
    for (const o of APP_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      opSel.append(opt);
    }
    opSel.value = op.op;
    opSel.addEventListener("change", () => {
      a.operation = newAppOp(opSel.value as AppOperation["op"]);
      rerender();
    });
    body.append(opSel);

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
      vname.placeholder = "变量名，写入 true/false，后续用 {变量名} 引用";
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      vline.append(
        el("span", "unit-hint", "变量名"),
        vname,
        helpIcon(
          "写入布尔变量：程序在运行则为 true，否则 false。引用 {变量名} 得到 true/false，可配合「条件判断」动作使用。",
        ),
      );
      body.append(vline);

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

    const condSel = el("select", "action-type") as HTMLSelectElement;
    for (const c of CONDITION_TYPES) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = c.value;
      opt.textContent = c.label;
      condSel.append(opt);
    }
    condSel.value = cond.kind;
    condSel.addEventListener("change", () => {
      a.condition = newCondition(condSel.value as Condition["kind"]);
      rerender();
    });
    body.append(condSel);

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
    renderActionList(thenList, a.then, rerender);
    const addThen = el("button", "add-inline", "＋ 添加动作");
    addThen.addEventListener("click", () => {
      a.then.push(newAction("text"));
      rerender();
    });
    body.append(thenList, addThen);

    body.append(el("div", "branch-label", "否则执行（可空）"));
    const elseList = el("div", "action-branch");
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
  opts: { file?: boolean; dir?: boolean } = { file: true },
): HTMLElement {
  const line = el("div", "action-line");
  const input = el("input", "action-input") as HTMLInputElement;
  input.type = "text";
  input.value = value;
  input.placeholder = "路径（可用 {变量名} 占位符）";
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
  (document.getElementById("set-launch-minimized") as HTMLInputElement).checked =
    cfg.settings.launch_minimized;
  (document.getElementById("set-wake-key") as HTMLSelectElement).value = cfg.settings.wake_key ?? "";
}

function bindSettings() {
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const minimized = document.getElementById("set-launch-minimized") as HTMLInputElement;
  const wakeKey = document.getElementById("set-wake-key") as HTMLSelectElement;
  autostart.addEventListener("change", () => {
    cfg.settings.autostart = autostart.checked;
    void save();
  });
  paused.addEventListener("change", () => {
    cfg.settings.paused = paused.checked;
    void save();
  });
  minimized.addEventListener("change", () => {
    cfg.settings.launch_minimized = minimized.checked;
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
function fillKeySelect(sel: HTMLSelectElement, value: string) {
  sel.replaceChildren();
  for (const k of KEY_OPTIONS) {
    const o = document.createElement("option");
    o.value = k;
    o.textContent = k;
    sel.append(o);
  }
  sel.value = value;
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
    }
  });

  document.getElementById("add-folder-btn")!.addEventListener("click", () => {
    if (section === "shortcuts") addFolder(null);
  });

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
    if (draftNew) cfg.shortcuts.unshift(draft);
    else if (selected !== null) cfg.shortcuts[selected] = draft;
    const saved = draft;
    draft = null;
    draftNew = false;
    selected = null;
    void save().then(() => flashRow(cfg.shortcuts.indexOf(saved), "shortcut-list"));
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
    remapDraft.to = (document.getElementById("remap-to") as HTMLSelectElement).value;
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

  fillKeySelect(document.getElementById("remap-from") as HTMLSelectElement, "CapsLock");
  fillKeySelect(document.getElementById("remap-to") as HTMLSelectElement, "Ctrl");

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
