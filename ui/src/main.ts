import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";

type OsOperation =
  | { op: "copy"; source: string; dest: string }
  | { op: "cut"; source: string; dest: string }
  | { op: "paste"; dest: string }
  | { op: "delete"; path: string }
  | { op: "new_file"; path: string }
  | { op: "zip"; source: string; dest: string }
  | { op: "get_file_props"; path: string; var: string };
type Condition =
  | { kind: "exists"; path: string }
  | { kind: "not_exists"; path: string }
  | { kind: "is_file"; path: string }
  | { kind: "is_dir"; path: string }
  | { kind: "equals"; var: string; field: string; value: string }
  | { kind: "not_equals"; var: string; field: string; value: string };
type AppOperation =
  | { op: "launch"; program: string; args: string[] }
  | { op: "close"; program: string }
  | { op: "status"; program: string; var: string }
  | { op: "restart"; program: string; args: string[] };
type Action =
  | { type: "text"; text: string }
  | { type: "cmd"; command: string; show_output: boolean }
  | { type: "powershell"; command: string; show_output: boolean }
  | { type: "open_folder"; path: string }
  | { type: "keys"; keys: string[] }
  | { type: "pause_ms"; ms: number }
  | { type: "os"; operation: OsOperation }
  | { type: "app"; operation: AppOperation }
  | { type: "if"; condition: Condition; then: Action[]; otherwise: Action[] };
type ShortcutItem = {
  name?: string | null;
  description?: string | null;
  triggers: string[];
  actions: Action[];
  enabled: boolean;
};
type Remap = { from: string; to: string; enabled: boolean };
type Settings = { autostart: boolean; paused: boolean; launch_minimized: boolean };
type Config = { shortcuts: ShortcutItem[]; remaps: Remap[]; settings: Settings };
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
  { value: "text", label: "输入文本" },
  { value: "cmd", label: "执行 CMD 命令" },
  { value: "powershell", label: "执行 PowerShell 命令" },
  { value: "open_folder", label: "打开文件夹" },
  { value: "keys", label: "按键组合" },
  { value: "pause_ms", label: "延迟" },
  { value: "os", label: "操作系统（文件）" },
  { value: "app", label: "应用（打开/关闭/状态/重启）" },
  { value: "if", label: "条件判断" },
];

// ---- 条件判断的子条件 ----
const CONDITION_TYPES: { value: Condition["kind"]; label: string }[] = [
  { value: "exists", label: "路径存在" },
  { value: "not_exists", label: "路径不存在" },
  { value: "is_file", label: "路径是文件" },
  { value: "is_dir", label: "路径是目录" },
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
  { value: "zip", label: "压缩为 zip" },
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
    case "zip":
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
      return { op, program: "", var: "app" };
    case "restart":
      return { op, program: "", args: [] };
  }
  return { op: "launch", program: "", args: [] };
}

function newAction(type: Action["type"]): Action {
  switch (type) {
    case "text":
      return { type: "text", text: "" };
    case "cmd":
      return { type: "cmd", command: "", show_output: false };
    case "powershell":
      return { type: "powershell", command: "", show_output: false };
    case "open_folder":
      return { type: "open_folder", path: "" };
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
  return { type: "text", text: "" };
}

function osHasContent(o: OsOperation): boolean {
  switch (o.op) {
    case "copy":
    case "cut":
    case "zip":
      return o.source.trim().length > 0 && o.dest.trim().length > 0;
    case "paste":
      return o.dest.trim().length > 0;
    case "delete":
    case "new_file":
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
      return a.text.trim().length > 0;
    case "cmd":
    case "powershell":
      return a.command.trim().length > 0;
    case "open_folder":
      return a.path.trim().length > 0;
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
    case "zip":
      return `压缩：${o.source} → ${o.dest}`;
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
      return `查询状态：${o.program} → ${o.var}`;
    case "restart":
      return `重启：${o.program}`;
  }
  return "";
}

function actionSummary(a: Action): string {
  switch (a.type) {
    case "text":
      return a.text ? `输入文本：${a.text}` : "输入文本";
    case "cmd":
      return `CMD：${a.command}`;
    case "powershell":
      return `PowerShell：${a.command}`;
    case "open_folder":
      return `打开目录：${a.path}`;
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
  shortcuts: [],
  remaps: [],
  settings: { autostart: false, paused: false, launch_minimized: false },
};
let section: Section = "shortcuts";
let selected: number | null = null; // 当前列表中的选中下标
let draftNew = false; // 当前选中是否为“新增”未保存项（取消/切换时丢弃）
let recording = false;
let conflicts: Conflict[] = [];
let search = "";
let messages: CommandResult[] = []; // 消息中心（最新在前）
let msgIndex = 0; // 当前选中的结果 tab
let unread = false; // 未读红点

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

async function refreshConflicts() {
  try {
    conflicts = await invoke<Conflict[]>("get_conflicts", { config: cfg });
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
  cfg.shortcuts.forEach((s, i) => {
    const hay = `${s.triggers.join(" ")} ${s.name ?? ""} ${s.description ?? ""} ${s.actions
      .map(actionSummary)
      .join(" ")}`.toLowerCase();
    if (q && !hay.includes(q)) return;

    const li = el("li", "row" + (selected === i ? " selected" : ""));
    li.dataset.idx = String(i);
    const on = el("input", "toggle") as HTMLInputElement;
    on.type = "checkbox";
    on.checked = s.enabled;
    on.addEventListener("change", (e) => {
      e.stopPropagation();
      s.enabled = on.checked;
      void save();
    });

    const info = el("div", "row-info");
    info.append(
      el("span", "row-title", s.name || "（未命名）"),
      el("span", "row-sub", s.triggers.join("  ") || "（未设触发键）"),
      el(
        "span",
        "desc",
        s.description ||
          (s.actions.length === 0 ? "（无动作）" : s.actions.map(actionSummary).join(" · ")),
      ),
    );

    li.append(on, info);
    li.addEventListener("click", () => openDetail(i));
    list.append(li);
  });
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
    li.addEventListener("click", () => openDetail(i));
    list.append(li);
  });
}

function renderDetail() {
  document.getElementById("detail-shortcuts")!.classList.toggle("hidden", section !== "shortcuts");
  document.getElementById("detail-remaps")!.classList.toggle("hidden", section !== "remaps");
  document.getElementById("detail-settings")!.classList.toggle("hidden", section !== "settings");
  document.getElementById("detail-messages")!.classList.toggle("hidden", section !== "messages");

  if (section === "shortcuts") {
    const has = selected !== null && cfg.shortcuts[selected] !== undefined;
    document.getElementById("detail-empty")!.classList.toggle("hidden", has);
    document.getElementById("detail-editor")!.classList.toggle("hidden", !has);
    if (!has) document.getElementById("detail-title")!.textContent = "快捷键";
  } else if (section === "remaps") {
    const has = selected !== null && cfg.remaps[selected] !== undefined;
    document.getElementById("remap-empty")!.classList.toggle("hidden", has);
    document.getElementById("remap-editor")!.classList.toggle("hidden", !has);
    if (!has) document.getElementById("remap-title")!.textContent = "改键";
  }
}

function openDetail(i: number, isNew = false) {
  selected = i;
  draftNew = isNew;
  if (section === "shortcuts") {
    const s = cfg.shortcuts[i];
    (document.getElementById("edit-name") as HTMLInputElement).value = s.name ?? "";
    (document.getElementById("edit-desc") as HTMLInputElement).value = s.description ?? "";
    renderTriggers(s);
    renderActions(s);
    const title = document.getElementById("detail-title")!;
    title.textContent = draftNew
      ? "新增快捷键"
      : s.triggers.length
        ? s.triggers.join(" / ")
        : "（未设触发键）";
  } else if (section === "remaps") {
    const r = cfg.remaps[i];
    (document.getElementById("remap-from") as HTMLSelectElement).value = r.from;
    (document.getElementById("remap-to") as HTMLSelectElement).value = r.to;
    document.getElementById("remap-title")!.textContent = draftNew
      ? "新增改键"
      : `${r.from} → ${r.to}`;
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
  if (draftNew && selected !== null) {
    if (section === "shortcuts") cfg.shortcuts.splice(selected, 1);
    else cfg.remaps.splice(selected, 1);
  }
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
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = a.text;
    ta.placeholder = "要输入的文本……";
    ta.addEventListener("input", () => {
      a.text = ta.value;
    });
    body.append(ta);
  } else if (a.type === "cmd" || a.type === "powershell") {
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = a.command;
    ta.placeholder = a.type === "cmd" ? "cmd 命令，如 echo hello" : "PowerShell 命令，如 Get-Date";
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
  } else if (a.type === "open_folder") {
    const line = el("div", "action-line");
    const path = el("input", "action-input") as HTMLInputElement;
    path.type = "text";
    path.value = a.path;
    path.placeholder = "目录路径，如 C:\\Users";
    path.addEventListener("input", () => {
      a.path = path.value;
    });
    const browse = el("button", undefined, "浏览…");
    browse.addEventListener("click", () => void pickDir(path));
    line.append(path, browse);
    body.append(line);
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
    } else if (op.op === "paste") {
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true }));
    } else if (op.op === "delete") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, dir: true }));
    } else if (op.op === "new_file") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true }));
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
      line.append(el("span", "unit-hint", "变量名"), vname);
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
      vline.append(el("span", "unit-hint", "变量名"), vname);
      body.append(vline);
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
  const el = document.getElementById("conflict-list")!;
  el.replaceChildren();
  for (const c of conflicts) {
    const d = document.createElement("div");
    d.className = "conflict " + (c.severity === "error" ? "error" : "warn");
    d.textContent = c.message;
    el.append(d);
  }
}

// ---- 设置 ----
function syncSettings() {
  (document.getElementById("set-autostart") as HTMLInputElement).checked = cfg.settings.autostart;
  (document.getElementById("set-paused") as HTMLInputElement).checked = cfg.settings.paused;
  (document.getElementById("set-launch-minimized") as HTMLInputElement).checked =
    cfg.settings.launch_minimized;
}

function bindSettings() {
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const minimized = document.getElementById("set-launch-minimized") as HTMLInputElement;
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
      cfg.shortcuts.unshift({
        name: "",
        description: "",
        triggers: [],
        actions: [newAction("text")],
        enabled: true,
      });
      openDetail(0, true);
    } else if (section === "remaps") {
      cfg.remaps.unshift({ from: "CapsLock", to: "Ctrl", enabled: true });
      openDetail(0, true);
    }
  });

  document.getElementById("list-search")!.addEventListener("input", (e) => {
    search = (e.target as HTMLInputElement).value;
    renderListPane();
  });

  document.getElementById("add-action")!.addEventListener("click", () => {
    if (section !== "shortcuts" || selected === null) return;
    const s = cfg.shortcuts[selected];
    s.actions.push(newAction("text"));
    renderActions(s);
  });

  document.getElementById("edit-capture")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || selected === null || !cfg.shortcuts[selected]) return;
    const s = cfg.shortcuts[selected];
    startCapture((combo) => {
      if (!s.triggers.includes(combo)) s.triggers.push(combo);
      renderTriggers(s);
    }, btn);
  });

  document.getElementById("action-record")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (section !== "shortcuts" || selected === null) return;
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
      const s = cfg.shortcuts[selected];
      s.actions.push(...actions);
      renderActions(s);
      void save();
    }
  });

  document.getElementById("save-shortcut")!.addEventListener("click", () => {
    if (section !== "shortcuts" || selected === null) return;
    void stopRecIfAny();
    const s = cfg.shortcuts[selected];
    s.name = (document.getElementById("edit-name") as HTMLInputElement).value || null;
    s.description = (document.getElementById("edit-desc") as HTMLInputElement).value || null;
    const saved = s;
    draftNew = false;
    selected = null;
    void save().then(() => flashRow(cfg.shortcuts.indexOf(saved), "shortcut-list"));
  });

  document.getElementById("cancel-shortcut")!.addEventListener("click", closeDetail);
  document.getElementById("detail-close")!.addEventListener("click", closeDetail);
  document.getElementById("delete-shortcut")!.addEventListener("click", () => {
    if (section !== "shortcuts" || selected === null) return;
    cfg.shortcuts.splice(selected, 1);
    draftNew = false;
    selected = null;
    void save();
  });

  document.getElementById("save-remap")!.addEventListener("click", () => {
    if (section !== "remaps" || selected === null) return;
    const r = cfg.remaps[selected];
    r.from = (document.getElementById("remap-from") as HTMLSelectElement).value;
    r.to = (document.getElementById("remap-to") as HTMLSelectElement).value;
    const saved = r;
    draftNew = false;
    selected = null;
    void save().then(() => flashRow(cfg.remaps.indexOf(saved), "remap-list"));
  });

  document.getElementById("cancel-remap")!.addEventListener("click", closeDetail);
  document.getElementById("remap-close")!.addEventListener("click", closeDetail);
  document.getElementById("delete-remap")!.addEventListener("click", () => {
    if (section !== "remaps" || selected === null) return;
    cfg.remaps.splice(selected, 1);
    draftNew = false;
    selected = null;
    void save();
  });

  fillKeySelect(document.getElementById("remap-from") as HTMLSelectElement, "CapsLock");
  fillKeySelect(document.getElementById("remap-to") as HTMLSelectElement, "Ctrl");

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
