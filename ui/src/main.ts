import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { installBrowserMock } from "./mock";
import { applyLocale, localizeStaticDom, savedLocale, tr, trt } from "./i18n";
import {
  fetchCommandResults,
  fetchConfig,
  fetchRunRecords,
  fetchUnread,
  fetchUpdateStatus,
  normalizeCommandScriptFields,
  refreshConflicts,
  refreshInputDevices,
  refreshRunningTools,
  saveConfig,
} from "./api";
import { deepClone, domValue, el, helpIcon, intIn, onClickAsync, toast } from "./dom";
import { draftSignature, isDraftDirty, state } from "./state";
import { bindMessages, hideResultModal, markRead, renderMessages, showResultModal } from "./messages";
import {
  ACTION_ICONS,
  APP_OPS,
  BRIGHTNESS_OPS,
  CAPTURE_OPS,
  CLIPBOARD_OPS,
  CONDITION_TYPES,
  DESKTOP_DIRECTIONS,
  KEY_OPTIONS,
  MOUSE_BUTTON_LABELS,
  MOUSE_OPS,
  OS_OPS,
  SYSTEM_OPS,
  TRANSFORM_OPS,
  WAIT_TARGET_TYPES,
  WHEN_CONDITION_TYPES,
  WINDOW_OPS,
  actionDescription,
  actionHasContent,
  actionSummary,
  actionTypeChoices,
  actionTypeLabel,
  actionVarReads,
  actionVarWrites,
  isDisabled,
  newAction,
  newAppOp,
  newClipboardOp,
  newCondition,
  newMouseOp,
  newOsOp,
  newSystemOp,
  newWaitTarget,
  unwrapAction,
} from "./options";
import type {
  Action,
  ActionBase,
  AnyEntry,
  AppOperation,
  BrightnessOperation,
  CaptureOperation,
  ClipboardOperation,
  CommandResult,
  Condition,
  Config,
  Conflict,
  DesktopDirection,
  DragPayload,
  DropZone,
  ElevatedStatus,
  EntryTab,
  Folder,
  FolderEntry,
  Gesture,
  HidppStatusView,
  HidppUnlockView,
  HintsState,
  HotCornerRule,
  HotCornerSpot,
  IfRunning,
  InputDeviceList,
  InputEnvReport,
  KeysMode,
  Layer,
  ListKind,
  MenuEntry,
  MouseButton,
  MouseOp,
  OsOperation,
  Remap,
  RunRecord,
  Section,
  SendPayload,
  Shell,
  ShortcutItem,
  ShowOutput,
  StatusPayload,
  SystemOperation,
  TextExpansion,
  TextMode,
  TouchpadGesture,
  TransformOperation,
  UpdateStatus,
  WaitTarget,
  WheelEdgeRule,
  WheelEdgeSpot,
  WindowOperation,
} from "./types";

// 静态界面文案先翻（index.html 里的整块文本 / 属性），之后动态渲染的部分都走 tr()/trt()。
localizeStaticDom();

/** 冲突三组：硬冲突（需处理）/ 软冲突（遮蔽·重叠）/ 平台能力·环境限制（仅提示，不阻止保存）。 */
function splitConflicts(list: Conflict[]): { errors: Conflict[]; warns: Conflict[]; caps: Conflict[] } {
  return {
    errors: list.filter((c) => c.severity === "error"),
    warns: list.filter((c) => c.severity === "warn" && !c.platform),
    caps: list.filter((c) => c.platform),
  };
}
const KIND_SECTION: Record<ListKind, Section> = {
  shortcut: "shortcuts",
  remap: "remaps",
  expansion: "expansions",
  gesture: "gestures",
  touchpad: "touchpads",
  hotcorner: "hotcorners",
  wheeledge: "wheeledges",
};
function isListSection(s: Section): boolean {
  return (
    s === "shortcuts" ||
    s === "remaps" ||
    s === "expansions" ||
    s === "gestures" ||
    s === "touchpads" ||
    s === "hotcorners" ||
    s === "wheeledges"
  );
}
// 当前编辑页的条目类型 / 草稿（仅列表分区有效，别在设置/消息页调）。
function currentKind(): ListKind {
  return state.section === "remaps"
    ? "remap"
    : state.section === "expansions"
      ? "expansion"
      : state.section === "gestures"
        ? "gesture"
        : state.section === "touchpads"
          ? "touchpad"
          : state.section === "hotcorners"
            ? "hotcorner"
            : state.section === "wheeledges"
              ? "wheeledge"
              : "shortcut";
}
function currentDraft():
  | ShortcutItem
  | Remap
  | TextExpansion
  | Gesture
  | TouchpadGesture
  | HotCornerRule
  | WheelEdgeRule
  | null {
  if (state.section === "shortcuts") return state.draft;
  if (state.section === "remaps") return state.remapDraft;
  if (state.section === "expansions") return state.expansionDraft;
  if (state.section === "gestures") return state.gestureDraft;
  if (state.section === "touchpads") return state.touchpadDraft;
  if (state.section === "hotcorners") return state.hotcornerDraft;
  if (state.section === "wheeledges") return state.wheeledgeDraft;
  return null;
}

// 内置脚本的示例：读变量 → 拼接 → 存回变量。保持英文（脚本内容不是界面文案，
// 走翻译反而会让人复制到一段被翻过的代码）。
const INLINE_SCRIPT_SAMPLE = `// read a chain variable, build a string, write it back (the return value is the step result)
let who = get_var("name");
if who == "" {
    who = "Kada";
}
set_var("greeting", "Hello, " + who);
"Hello, " + who`;

// 可变状态集中在 ./state（模块顶层不互相 import 对方的 let，避免 TDZ / 循环依赖）。

// ---- 列表折叠（属于列表渲染，U-03 再搬） ----
const COLLAPSED_KEY = "kada.collapsedFolders";
function loadCollapsed(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? "[]"));
  } catch {
    return new Set();
  }
}
function saveCollapsed() {
  try {
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify([...collapsedFolders]));
  } catch {}
}
const collapsedFolders = loadCollapsed(); // 折叠的目录 id（localStorage 持久化，重启保持）
export const collapsedActions = new WeakSet<Action>(); // 收起态的动作（按对象引用，重绘/重排后仍保持）；messages 的深链展开要读

// ---- IPC ----
async function load() {
  state.cfg = await fetchConfig();
  await refreshRunningTools();
  state.messages = await fetchCommandResults();
  state.runs = await fetchRunRecords();
  state.unread = await fetchUnread();
  updateStatus = await fetchUpdateStatus();
  await refreshConflicts();
  render();
  syncSettings();
  syncUpdate();
}

// 落盘整份配置。返回 true = 真的写进去了——未保存守卫靠它决定「保存并离开」要不要放行，
// 失败还放行等于把用户的改动悄悄丢掉。
async function save(): Promise<boolean> {
  // 命令 / 脚本的空执行字段归一成缺省（空串 / 空数组 / null 不落盘）。
  for (const key of ["shortcuts", "gestures", "touchpad_gestures", "hot_corners", "wheel_edges"] as const) {
    const list = state.cfg[key] as unknown as { actions: Action[] }[] | undefined;
    if (Array.isArray(list)) {
      for (const item of list) {
        if (Array.isArray(item.actions)) item.actions = normalizeCommandScriptFields(item.actions);
      }
    }
  }
  state.cfg.shortcuts = state.cfg.shortcuts.filter(
    (s) => s.triggers.length > 0 || !!s.name || s.actions.some(actionHasContent),
  );
  state.cfg.expansions = state.cfg.expansions.filter((e) => e.trigger.trim().length > 0);
  // 鼠标操作 / 触摸板手势：方向串为空 = 这条永远匹配不上，后端清洗也会丢——先在前端剔掉。
  // 触摸板手势例外：方向串可以留空（纯形状条目，靠 `template` 兜底），两者都空才剔。
  if (state.cfg.gestures) state.cfg.gestures = state.cfg.gestures.filter((g) => g.pattern.trim().length > 0);
  if (state.cfg.touchpad_gestures)
    state.cfg.touchpad_gestures = state.cfg.touchpad_gestures.filter(
      (g) => g.pattern.trim().length > 0 || (g.template?.length ?? 0) > 0,
    );
  let ignored: string[] = [];
  try {
    ignored = await saveConfig(state.cfg);
  } catch (e) {
    toast(trt`保存失败: ${e}`);
    render();
    return false;
  }
  await refreshConflicts();
  const { errors: errs, warns, caps } = splitConflicts(state.conflicts);
  const notes: string[] = [];
  if (ignored.length) {
    // 只报数量的话，被丢掉的条目（比如「长按进入层」指向的层已不存在，整条改键作废）用户
    // 根本查不出来——带上第一条原因，长文案由 .toast 的 max-width 折行。
    const first = ignored[0];
    notes.push(
      ignored.length === 1
        ? trt`已忽略 1 处无效配置：${first}`
        : trt`已忽略 ${ignored.length} 处无效配置（例：${first}）`,
    );
  }
  if (errs.length || warns.length || caps.length)
    notes.push(
      trt`${errs.length} 处冲突${warns.length ? trt`、${warns.length} 处遮蔽提示` : ""}${
        caps.length ? trt`、${caps.length} 条当前平台用不了` : ""
      }`,
    );
  if (notes.length) toast(trt`已保存：${notes.join("；")}`);
  else toast(tr("已保存"));
  render();
  return true;
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

// 小键盘键的物理键名（`code` 才是物理位置，`key` 受 NumLock 影响：关掉时数字键打出
// Home/End、运算符打出 `+` 之类）。「原键」框里直接按小键盘键时用它落键名，而不是
// 把字符打进过滤框（`+` 还不是合法键名，保存时才报错）。
function numpadKeyFromEvent(e: KeyboardEvent): string | null {
  return e.code.startsWith("Numpad") ? CODE_TABLE[e.code] ?? null : null;
}

function comboFromEvent(e: KeyboardEvent): string | null {
  const main = codeToKey(e.code);
  if (!main) return null;
  return withMods(e, main);
}

// 鼠标键 → 键名（DOM `MouseEvent.button`：1=中键、3=后退 MB4、4=前进 MB5）。左/右键（0/2）
// 不纳入——那是普通点击，不该当触发键，也避免吞掉界面点击。名字与 `kada-core` 的 Key 同名。
function mouseKeyFromEvent(e: MouseEvent): string | null {
  switch (e.button) {
    case 1: return "MouseMiddle";
    case 3: return "MouseBack";
    case 4: return "MouseForward";
    default: return null;
  }
}

// 修饰键前缀（Ctrl/Alt/Shift/Meta，顺序同键盘录入）+ 主键；鼠标组合键复用同一口径。
function withMods(e: KeyboardEvent | MouseEvent, main: string): string {
  const mods = currentMods(e);
  return mods ? `${mods}+${main}` : main;
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

// 录入按钮现在是纯图标，进行中的实时提示（「请按组合键…」「和弦：F&J」）写到按键框里的状态条上。
function captureHint(text: string | null) {
  const status = document.getElementById("capture-status");
  if (!status) return;
  status.textContent = text ?? "";
  status.classList.toggle("hidden", !text);
}

function startCapture(onCommit: (combo: string) => void, btn: HTMLButtonElement) {
  stopCapture();
  btn.disabled = true;
  captureHint(tr("请按组合键或鼠标侧键 / 滚轮…（Esc 取消）"));
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
  const onMouse = (e: MouseEvent) => {
    const key = mouseKeyFromEvent(e);
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    onCommit(withMods(e, key));
    finish();
  };
  // 滚轮也当离散键录（一格一下）。不阻止默认的话页面会跟着滚。
  const onWheel = (e: WheelEvent) => {
    const key = wheelKeyFromEvent(e);
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    onCommit(withMods(e, key));
    finish();
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", handler, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("wheel", onWheel, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", handler, true);
  window.addEventListener("mousedown", onMouse, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: false });
}

// 键序列录入：逐键累积 comboFromEvent 出的组合（空格拼接实时预览），Enter 提交、Esc 取消。
// 序列至少 2 步（单步就是普通组合键，走「录入组合键」）。
function startSequenceCapture(onCommit: (seq: string) => void, btn: HTMLButtonElement) {
  stopCapture();
  const steps: string[] = [];
  btn.disabled = true;
  void invoke("set_paused", { paused: true });
  const update = () => {
    captureHint(steps.length ? trt`序列：${steps.join(" ")}（Enter 提交，Esc 取消）` : tr("请按键、鼠标侧键或滚轮序列…（Enter 提交，Esc 取消）"));
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
  const onMouse = (e: MouseEvent) => {
    const key = mouseKeyFromEvent(e);
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    steps.push(withMods(e, key));
    update();
  };
  const onWheel = (e: WheelEvent) => {
    const key = wheelKeyFromEvent(e);
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    steps.push(withMods(e, key));
    update();
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", handler, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("wheel", onWheel, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", handler, true);
  window.addEventListener("mousedown", onMouse, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: false });
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
    captureHint(held.length
      ? trt`和弦：${chordText()}（松开提交，Esc 取消）`
      : tr("请同时按住多个键或鼠标侧键…（松开提交，Esc 取消）"));
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
  // 鼠标键成员：按下加入、松开（与其他成员任一松开一样）提交。
  const onMouse = (e: MouseEvent) => {
    const key = mouseKeyFromEvent(e);
    if (!key) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.type === "mouseup") {
      if (held.length >= 2) onCommit(chordText());
      finish();
      return;
    }
    if (held.length === 0) mods = currentMods(e);
    if (!held.includes(key)) {
      held.push(key);
      update();
    }
  };
  const finish = () => {
    activeCaptureCleanup = null;
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("keyup", onKey, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("mouseup", onMouse, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  activeCaptureCleanup = finish;
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("keyup", onKey, true);
  window.addEventListener("mousedown", onMouse, true);
  window.addEventListener("mouseup", onMouse, true);
  update();
}

// 滚轮事件 → 离散键名（一格 = 一次按下 + 抬起；规划 7.2-MG 余项①）。
function wheelKeyFromEvent(e: WheelEvent): string | null {
  if (!e.deltaY) return null;
  return e.deltaY < 0 ? "WheelUp" : "WheelDown";
}

// 当前按住的修饰键（与后端键名一致：Ctrl / Alt / Shift / Meta）。键盘与鼠标事件都有这四个字段。
function currentMods(e: KeyboardEvent | MouseEvent): string {
  const mods: string[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Meta");
  return mods.join("+");
}

// ---- 渲染 ----
// 主题：纯 UI 偏好，不进 config.json（那份是跨机同步的，主题跟机器走而不是跟配置走），
// 跟折叠目录一样存 localStorage。四个窗口加载的是同一个 index.html，所以在 <html> 上打
// data-theme 就都跟着变（含气泡 / HUD / 提示框）。
// 不提供「跟随系统」档：「像素 CRT 深色」是默认主题。
const THEME_KEY = "kada.theme";
const DEFAULT_THEME = "crt";
const THEMES = new Set(["crt", "crt-light", "dark", "light"]);
// 老版本存过 "auto"（跟随系统）这一档，已废弃——读到不认识的旧值一律落回默认主题。
function loadTheme(): string {
  const stored = localStorage.getItem(THEME_KEY);
  return stored && THEMES.has(stored) ? stored : DEFAULT_THEME;
}
function applyTheme(t: string) {
  const root = document.documentElement;
  // CRT 有深 / 浅两档：共用一个 data-theme="crt"（同一套像素 / 扫描线规则），
  // 浅色档再用 data-crt="light" 覆盖调色板与亮度。其余主题清掉这个标记。
  if (t === "crt-light") {
    root.dataset.theme = "crt";
    root.dataset.crt = "light";
  } else if (t === "crt") {
    root.dataset.theme = "crt";
    root.dataset.crt = "dark";
  } else {
    root.dataset.theme = t;
    delete root.dataset.crt;
  }
}

function render() {
  renderListPane();
  renderConflictBubble();
  renderDetail();
  renderMessages();
  if (state.section === "layers") renderLayers();
  if (state.section === "market") renderMarketPage();
}

function renderListPane() {
  // 重绘会整棵重建列表行：锚点被拆掉的菜单没地方可依附，先收掉。
  if (state.menuAnchor && !state.menuAnchor.isConnected) hideRowMenu();
  // 重绘会整棵重建列表行，焦点所在行先记下来、画完还回去——否则键盘用户
  // 一按回车，焦点就掉回 body，Tab 又得从头数（行内按钮失焦后统一还给行本身，
  // 够用且不用记行内哪个子元素）。合并列表里同一下标可能分属三类条目，选择器带上 kind。
  const act = document.activeElement as HTMLElement | null;
  const actRow = act?.closest(".row, .folder-row") as HTMLElement | null;
  const actSel =
    actRow?.dataset.folderId != null
      ? `.folder-row[data-folder-id="${actRow.dataset.folderId}"]`
      : actRow?.dataset.idx != null && actRow.dataset.kind
        ? `.row[data-kind="${actRow.dataset.kind}"][data-idx="${actRow.dataset.idx}"]`
        : null;
  document.getElementById("list-pane")!.classList.toggle(
    "hidden",
    state.section === "settings" ||
      state.section === "layers" ||
      state.section === "market" ||
      state.section === "messages" ||
      state.section === "help",
  );
  // 多选集按各份条目数组剔除陈旧引用：条目被删后不该还留在集合里被批量操作。
  const all: object[] = [
    ...state.cfg.shortcuts,
    ...state.cfg.remaps,
    ...state.cfg.expansions,
    ...(state.cfg.gestures ?? []),
    ...(state.cfg.touchpad_gestures ?? []),
    ...(state.cfg.hot_corners ?? []),
    ...(state.cfg.wheel_edges ?? []),
  ];
  for (const o of [...state.picked]) if (!all.includes(o)) state.picked.delete(o);
  if (state.pickAnchor && !all.includes(state.pickAnchor)) state.pickAnchor = null;
  // 筛选按钮亮起来 = 有筛选在生效。
  document
    .getElementById("filter-btn")!
    .classList.toggle("on", state.listFilter.enabled || state.listFilter.conflicted);
  renderEntries();
  if (actRow && actSel) {
    document.querySelector<HTMLElement>(`#list-pane ${actSel}`)?.focus();
  }
}

// 条目是否卷入冲突：冲突文案把涉及的触发键/来源/层名都用「」引起来，拿条目的键去文案里找
// （后端 Conflict 只带名称，无名条目靠不上 name；UI 与后端同包发布，文案格式一体演化）。
function entryConflicted(keys: (string | null | undefined)[]): boolean {
  if (!state.conflicts.length) return false;
  return state.conflicts.some((c) =>
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
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${s.triggers.join(" ")} ${s.name ?? ""} ${s.description ?? ""} ${s.actions
      .map(actionSummary)
      .join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !s.enabled) return false;
  if (state.listFilter.conflicted && !entryConflicted([...s.triggers, s.layer ? layerName(s.layer) : null]))
    return false;
  return true;
}

function remapVisible(r: Remap): boolean {
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${r.from} ${remapSummary(r)} ${r.name ?? ""} ${r.description ?? ""} ${layerName(r.hold_layer)} ${layerName(r.lock_layer)}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !r.enabled) return false;
  if (state.listFilter.conflicted && !entryConflicted([r.from, r.layer ? layerName(r.layer) : null]))
    return false;
  return true;
}

function expansionVisible(e: TextExpansion): boolean {
  const q = state.search.trim().toLowerCase();
  if (q && !`${e.name ?? ""} ${e.description ?? ""} ${e.trigger} ${e.replace}`.toLowerCase().includes(q)) return false;
  if (state.listFilter.enabled && !e.enabled) return false;
  // 文本扩展不参与冲突检测：「有冲突」筛选开着时它必然不命中（与另两类保持同一口径）。
  if (state.listFilter.conflicted) return false;
  return true;
}

// 树形缩进：目录行 = 层级 × 16px；条目行再右移 15px（= 目录行的箭头列宽 + 行内左边距，
// 让同级条目的框体左缘正好落在目录图标的左缘上），子层各自再缩 16px。
// 缩进用 margin 而非 padding：整行盒子随层级右移，层级关系和边框范围都对齐。
const TREE_STEP = 16;
const ITEM_OFF = TREE_STEP - 1;

// 行首类型徽标（Postman 用 GET/POST/DEL 一眼分清请求类型，这里同理分清三类条目）。
const KIND_ICON: Record<ListKind, string> = {
  shortcut:
    '<svg viewBox="0 0 24 24"><path d="M13 2 3 14h7l-1 8 10-12h-7l1-8z" fill="currentColor"/></svg>',
  remap:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3 4 7l4 4"/><path d="M4 7h16"/><path d="m16 21 4-4-4-4"/><path d="M20 17H4"/></svg>',
  expansion:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 5h16"/><path d="M4 12h9"/><path d="M4 19h16"/><path d="m17 9 3 3-3 3"/></svg>',
  // 与发布页「鼠标手势与滚轮」卡片同款（右键方块 + 折线轨迹与箭头 + 圆点）——两处同步改。
  // 轨迹从方块底边中点（6.5, 7）起笔：与方块同一条竖中线，圆头端点正好并进底边描边。
  gesture:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M6.5 7v6a4 4 0 0 0 4 4h9.5"/><path d="M17 14l3 3-3 3"/><rect x="4" y="2" width="5" height="5"/><circle cx="20" cy="6" r="1.6"/></svg>',
  hotcorner:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20V4h16"/><path d="M4 13a9 9 0 0 1 9-9"/></svg>',
  // 触摸板手势：一块触摸板 + 单指滑动轨迹（与鼠标手势的「右键方块」区分开）。
  touchpad:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="4" y="3" width="16" height="18" rx="3"/><path d="M8.5 14c2.5-3.5 6-3.5 8 0"/><path d="M15 11.5l1.5 1.5-1.5 1.5"/></svg>',
  wheeledge:
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="14" rx="2"/><rect x="9" y="10" width="6" height="8" rx="1"/><path d="M12 11v2m0 0 1-1m-1 1-1-1"/></svg>',
};

function kindIcon(kind: ListKind): HTMLElement {
  const s = el("span", `row-kind kind-${kind}`);
  s.innerHTML = KIND_ICON[kind]; // 常量 SVG，非用户输入
  return s;
}

// 「正在编辑页里打开的那条」——合并列表后三类共用一个下标变量，得连类型一起比。
function isSel(kind: ListKind, idx: number): boolean {
  return state.section === KIND_SECTION[kind] && state.selected === idx;
}

// 合并列表：每层先条目（快捷键 → 改键 → 文本扩展）后子目录，根级同样排在目录树之前。
function renderEntries() {
  const list = document.getElementById("shortcut-list")!;
  list.replaceChildren();

  const renderIn = (folderId: string | null, depth: number) => {
    state.cfg.shortcuts.forEach((s, i) => {
      if ((s.folder ?? null) === folderId && shortcutVisible(s)) list.append(shortcutRow(s, i, depth));
    });
    // 改键/文本扩展与快捷键同规则归目录：换目录、换位置都靠拖拽（落点见 nearestDropZone）。
    state.cfg.remaps.forEach((r, i) => {
      if ((r.folder ?? null) === folderId && remapVisible(r)) list.append(remapRow(r, i, depth));
    });
    state.cfg.expansions.forEach((e, i) => {
      if ((e.folder ?? null) === folderId && expansionVisible(e)) list.append(expansionRow(e, i, depth));
    });
    // 鼠标类三类（手势 / 角动作 / 边缘滚动）共用 `order`，可互相换位并放进任意目录。
    const mouseRows: { kind: "gesture" | "hotcorner" | "wheeledge"; idx: number; order: number; rank: number; row: () => HTMLElement }[] = [];
    (state.cfg.gestures ?? []).forEach((g, i) => {
      if ((g.folder ?? null) === folderId && gestureVisible(g))
        mouseRows.push({ kind: "gesture", idx: i, order: g.order ?? 0, rank: 0, row: () => gestureRow(g, i, depth) });
    });
    (state.cfg.hot_corners ?? []).forEach((r, i) => {
      if ((r.folder ?? null) === folderId && hotCornerVisible(r))
        mouseRows.push({ kind: "hotcorner", idx: i, order: r.order ?? 0, rank: 1, row: () => hotCornerRow(r, i, depth) });
    });
    (state.cfg.wheel_edges ?? []).forEach((r, i) => {
      if ((r.folder ?? null) === folderId && wheelEdgeVisible(r))
        mouseRows.push({ kind: "wheeledge", idx: i, order: r.order ?? 0, rank: 2, row: () => wheelEdgeRow(r, i, depth) });
    });
    mouseRows
      .sort((a, b) => a.order - b.order || a.rank - b.rank || a.idx - b.idx)
      .forEach((m) => list.append(m.row()));
    // 触摸板手势**自成一组**：自己的 `order` 序列、排在鼠标操作之后，不与它们跨类型换位。
    const padRows: { idx: number; order: number; row: () => HTMLElement }[] = [];
    (state.cfg.touchpad_gestures ?? []).forEach((g, i) => {
      if ((g.folder ?? null) === folderId && touchpadVisible(g))
        padRows.push({ idx: i, order: g.order ?? 0, row: () => touchpadRow(g, i, depth) });
    });
    padRows
      .sort((a, b) => a.order - b.order || a.idx - b.idx)
      .forEach((p) => list.append(p.row()));
  };

  // 搜索或筛选期间强制展开目录：收着的目录会把命中的条目藏起来（筛选形同没生效）。
  const searching = state.search.trim() !== "" || state.listFilter.enabled || state.listFilter.conflicted;
  const renderFolders = (parentId: string | null, depth: number) => {
    for (const f of state.cfg.folders.filter((x) => (x.parent ?? null) === parentId)) {
      const collapsed = !searching && collapsedFolders.has(f.id);
      list.append(folderRow(f, depth, collapsed));
      if (collapsed) continue;
      renderIn(f.id, depth + 1);
      renderFolders(f.id, depth + 1);
    }
  };

  renderIn(null, 0);
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

// 行交互（三类条目共用）：单击开编辑页；Ctrl 加/减选；Shift 按可见顺序范围选；
// 右键在选中项上出菜单（点的行不在选中集里 → 先只选它）。
function listRow(li: HTMLElement, obj: object, kind: ListKind, idx: number) {
  li.dataset.kind = kind;
  li.dataset.idx = String(idx);
  makeRow(li, () => void requestOpenDetail(idx, false, kind), { selected: isSel(kind, idx) });
  li.addEventListener("click", (e) => {
    if (state.suppressClick) return;
    if ((e.target as HTMLElement).closest("input, button")) return; // 菜单/按钮不打开详情
    if (e.shiftKey) {
      pickRange(obj);
      return;
    }
    if (e.ctrlKey || e.metaKey) {
      state.pickAnchor = obj;
      togglePicked(obj);
      return;
    }
    state.pickAnchor = obj; // 记录锚点，之后的 Shift 点击从这条开始范围选
    void requestOpenDetail(idx, false, kind);
  });
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    if (!state.picked.has(obj)) selectOnly(obj, li);
    showMenu(li, selectionMenuItems(), { x: e.clientX, y: e.clientY });
  });
}

// Shift 范围选：按当前可见行的顺序，把锚点到目标之间整段收进选择集。**不跨目录**——
// 锚点与目标不在同一目录时只选目标本身（Shift 与 Ctrl 同为「在列表里选条目」，只是批量）。
function pickRange(target: object) {
  const objs = [...document.querySelectorAll<HTMLElement>("#shortcut-list .row")]
    .map(rowObject)
    .filter((o): o is object => o !== null);
  const folder = (target as { folder?: string | null }).folder ?? null;
  const inSame = (o: object) => ((o as { folder?: string | null }).folder ?? null) === folder;
  const a = state.pickAnchor ? objs.indexOf(state.pickAnchor) : -1;
  const b = objs.indexOf(target);
  if (a < 0 || b < 0 || !inSame(state.pickAnchor as object)) state.picked.add(target);
  else for (const o of objs.slice(Math.min(a, b), Math.max(a, b) + 1)) if (inSame(o)) state.picked.add(o);
  renderListPane();
}

// 行元素 → 条目对象（合并列表里同一下标可能分属多类，必须连 kind 一起读）。
function rowObject(li: HTMLElement): object | null {
  const idx = Number(li.dataset.idx);
  const kind = li.dataset.kind as ListKind;
  return kind === "shortcut"
    ? (state.cfg.shortcuts[idx] ?? null)
    : kind === "remap"
      ? (state.cfg.remaps[idx] ?? null)
      : kind === "expansion"
        ? (state.cfg.expansions[idx] ?? null)
        : kind === "gesture"
          ? ((state.cfg.gestures ?? [])[idx] ?? null)
          : kind === "touchpad"
            ? ((state.cfg.touchpad_gestures ?? [])[idx] ?? null)
            : null;
}

// 只选中这一行（右键/⋯菜单用）：不整棵重绘，菜单锚点还挂在按钮上，重绘会把它拆掉。
function selectOnly(obj: object, li: HTMLElement) {
  state.picked = new Set([obj]);
  document.querySelectorAll("#shortcut-list .picked").forEach((n) => n.classList.remove("picked"));
  li.classList.add("picked");
}

function shortcutRow(s: ShortcutItem, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("shortcut", idx) ? " selected" : "") +
      (state.picked.has(s) ? " picked" : "") +
      (s.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!s.enabled) li.title = tr("已停用（右键可启用）");
  attachDragStart(li, { kind: "shortcut", idx });
  listRow(li, s, "shortcut", idx);

  // 列表行只呈现「组合键 + 名称」；描述与动作摘要移入详情页。
  const trig = el("span", "row-trigger");
  if (s.triggers.length === 0) trig.classList.add("row-trigger-empty");
  appendLayerMark(trig, s.layer);
  trig.append(el("span", "row-trigger-text", s.triggers.join(" ") || tr("未设触发键")));
  const name = el("span", "row-name", s.name || tr("（未命名）"));

  const more = moreButton((btn) => openRowMenu(btn, li, s));
  li.append(
    kindIcon("shortcut"),
    trig,
    name,
    runButton(s.actions, s.name?.trim() || shortcutLabel(s)),
    more,
  );
  return li;
}

function remapRow(r: Remap, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("remap", idx) ? " selected" : "") +
      (state.picked.has(r) ? " picked" : "") +
      (r.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!r.enabled) li.title = tr("已停用（右键可启用）");
  attachDragStart(li, { kind: "remap", idx });
  listRow(li, r, "remap", idx);
  // 与快捷键行同一套观感：原键占「组合键」那一格，后面直接跟标题——
  // 没起名的用形态摘要（如「长按「层1」」）当标题，不再单独画箭头。
  const trig = el("span", "row-trigger");
  appendLayerMark(trig, r.layer);
  trig.append(el("span", "row-trigger-text", r.from));
  li.append(
    kindIcon("remap"),
    trig,
    el("span", "row-name", r.name?.trim() || remapSummary(r)),
    moreButton((btn) => openRowMenu(btn, li, r)),
  );
  return li;
}

function expansionRow(e: TextExpansion, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("expansion", idx) ? " selected" : "") +
      (state.picked.has(e) ? " picked" : "") +
      (e.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!e.enabled) li.title = tr("已停用（右键可启用）");
  attachDragStart(li, { kind: "expansion", idx });
  listRow(li, e, "expansion", idx);
  // 与快捷键行同一套观感：触发词占「组合键」那一格，最多展示 10 个字符（超了截断加省略号）。
  const trig = [...e.trigger];
  li.append(
    kindIcon("expansion"),
    el("span", "row-trigger", trig.length > 10 ? `${trig.slice(0, 10).join("")}…` : e.trigger),
    el("span", "row-name", e.name?.trim() || e.replace || tr("（删除触发词）")),
    moreButton((btn) => openRowMenu(btn, li, e)),
  );
  return li;
}

// 鼠标操作（手势 / 角动作 / 边缘滚动）这类新功能是否已由用户显式打开（设置页「鼠标操作」
// 总开关，默认关）。关着 = 列表里不出现这几类、新增菜单里没有它们、钩子也不拦右键。
function mouseFeaturesEnabled(): boolean {
  return state.cfg.settings.experimental === true;
}

// 触摸板手势是否已打开（它**有自己的开关**，与鼠标操作互不影响；默认关）。
// 关着 = 列表 / 模式菜单里不出现，也不采集（触发键照常输出）。
function touchpadFeaturesEnabled(): boolean {
  return state.cfg.settings.touchpad === true;
}

/** 角动作 / 边缘滚动：位置 + 动作（详情 / 层 / 条件与其它条目同口径）。 */
// 鼠标操作（手势）行：方向串占「组合键」那一格，后面跟名称。
// **鼠标操作总开关关着时整类不显示**（入口隐藏），配置本身留着。
function gestureRow(g: Gesture, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("gesture", idx) ? " selected" : "") +
      (state.picked.has(g) ? " picked" : "") +
      (g.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!g.enabled) li.title = tr("已停用（右键可启用）");
  listRow(li, g, "gesture", idx);
  attachDragStart(li, { kind: "gesture", idx });
  const trig = el("span", "row-trigger");
  trig.append(el("span", "row-trigger-text", g.pattern || tr("未设方向串")));
  li.append(
    kindIcon("gesture"),
    trig,
    el("span", "row-name", g.name?.trim() || tr("（未命名）")),
    runButton(g.actions, g.name?.trim() || g.pattern || tr("鼠标操作")),
    moreButton((btn) => openRowMenu(btn, li, g)),
  );
  return li;
}

// 触摸板手势行：触发键徽标 + 方向串 + 名称（单独分类，不再混在鼠标操作里）。
function touchpadRow(g: TouchpadGesture, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("touchpad", idx) ? " selected" : "") +
      (state.picked.has(g) ? " picked" : "") +
      (g.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!g.enabled) li.title = tr("已停用（右键可启用）");
  listRow(li, g, "touchpad", idx);
  attachDragStart(li, { kind: "touchpad", idx });
  const trig = el("span", "row-trigger");
  if (g.trigger) trig.append(el("span", "row-key-badge", g.trigger));
  trig.append(
    el("span", "row-trigger-text", g.pattern || (g.template ? tr("仅形状") : tr("未设方向串"))),
  );
  li.append(
    kindIcon("touchpad"),
    trig,
    el("span", "row-name", g.name?.trim() || tr("（未命名）")),
    runButton(g.actions, g.name?.trim() || g.pattern || tr("触摸板手势")),
    moreButton((btn) => openRowMenu(btn, li, g)),
  );
  return li;
}

// 鼠标操作条目的可见性：搜索命中名称 / 方向串 / 动作摘要；筛选只认「启用的」。
function gestureVisible(g: Gesture): boolean {
  if (!mouseFeaturesEnabled()) return false; // 总闸关着 = 这一类不出现
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${g.pattern} ${g.name ?? ""} ${g.description ?? ""} ${g.actions.map(actionSummary).join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !g.enabled) return false;
  return true;
}

// 触摸板手势的可见性：额外认触发键（用户常按「CapsLock」搜）。
function touchpadVisible(g: TouchpadGesture): boolean {
  if (!touchpadFeaturesEnabled()) return false;
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${g.pattern} ${g.trigger ?? ""} ${g.name ?? ""} ${g.description ?? ""} ${g.actions.map(actionSummary).join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !g.enabled) return false;
  return true;
}

const HOT_CORNER_SPOTS: { spot: HotCornerSpot; label: string }[] = [
  { spot: "top_left", label: tr("左上角") },
  { spot: "top_right", label: tr("右上角") },
  { spot: "bottom_left", label: tr("左下角") },
  { spot: "bottom_right", label: tr("右下角") },
];
const WHEEL_EDGE_SPOTS: { edge: WheelEdgeSpot; label: string }[] = [
  { edge: "top", label: tr("上边缘") },
  { edge: "bottom", label: tr("下边缘") },
  { edge: "left", label: tr("左边缘") },
  { edge: "right", label: tr("右边缘") },
];

function spotLabel(spot: HotCornerSpot | WheelEdgeSpot): string {
  return (
    HOT_CORNER_SPOTS.find((s) => s.spot === spot)?.label ??
    WHEEL_EDGE_SPOTS.find((s) => s.edge === spot)?.label ??
    ""
  );
}

// 角动作 / 边缘滚动行：位置作「触发键」那一格，后面跟名称。
function hotCornerRow(r: HotCornerRule, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("hotcorner", idx) ? " selected" : "") +
      (state.picked.has(r) ? " picked" : "") +
      (r.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!r.enabled) li.title = tr("已停用（右键可启用）");
  listRow(li, r, "hotcorner", idx);
  attachDragStart(li, { kind: "hotcorner", idx });
  const trig = el("span", "row-trigger");
  trig.append(el("span", "row-trigger-text", spotLabel(r.spot)));
  li.append(
    kindIcon("hotcorner"),
    trig,
    el("span", "row-name", r.name?.trim() || tr("（未命名）")),
    runButton(r.actions, r.name?.trim() || spotLabel(r.spot)),
    moreButton((btn) => openRowMenu(btn, li, r)),
  );
  return li;
}

function wheelEdgeRow(r: WheelEdgeRule, idx: number, depth: number): HTMLElement {
  const li = el(
    "li",
    "row" +
      (isSel("wheeledge", idx) ? " selected" : "") +
      (state.picked.has(r) ? " picked" : "") +
      (r.enabled ? "" : " off"),
  );
  li.style.marginLeft = `${depth * TREE_STEP + ITEM_OFF}px`;
  if (!r.enabled) li.title = tr("已停用（右键可启用）");
  listRow(li, r, "wheeledge", idx);
  attachDragStart(li, { kind: "wheeledge", idx });
  const trig = el("span", "row-trigger");
  trig.append(el("span", "row-trigger-text", spotLabel(r.edge)));
  li.append(
    kindIcon("wheeledge"),
    trig,
    el("span", "row-name", r.name?.trim() || tr("（未命名）")),
    runButton(r.actions, r.name?.trim() || spotLabel(r.edge)),
    moreButton((btn) => openRowMenu(btn, li, r)),
  );
  return li;
}

function hotCornerVisible(r: HotCornerRule): boolean {
  if (!mouseFeaturesEnabled()) return false;
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${spotLabel(r.spot)} ${r.name ?? ""} ${r.description ?? ""} ${r.actions.map(actionSummary).join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !r.enabled) return false;
  return true;
}

function wheelEdgeVisible(r: WheelEdgeRule): boolean {
  if (!mouseFeaturesEnabled()) return false;
  const q = state.search.trim().toLowerCase();
  if (q) {
    const hay = `${spotLabel(r.edge)} ${r.name ?? ""} ${r.description ?? ""} ${r.actions.map(actionSummary).join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (state.listFilter.enabled && !r.enabled) return false;
  return true;
}

function folderRow(f: Folder, depth: number, collapsed: boolean): HTMLElement {  const li = el("li", "folder-row");
  li.dataset.folderId = f.id;
  li.style.marginLeft = `${depth * TREE_STEP}px`;
  makeRow(
    li,
    () => {
      // 与点击同一路径：无子内容或重命名中不切换（没东西可展开）。
      if (folderHasChildren(f.id) && state.editingFolderId !== f.id) toggleFolder(f.id);
    },
    { expanded: !collapsed },
  );
  if (state.editingFolderId !== f.id) {
    attachDragStart(li, { kind: "folder", id: f.id });
  }
  const hasChildren = folderHasChildren(f.id);
  li.append(el("span", "folder-arrow", hasChildren ? (collapsed ? "▸" : "▾") : ""));
  li.append(el("span", "folder-ico", "📁"));
  if (state.editingFolderId === f.id) {
    const inp = el("input", "folder-input") as HTMLInputElement;
    inp.value = f.name;
    inp.placeholder = tr("文件夹名");
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const v = inp.value.trim();
      if (v) {
        f.name = v;
      } else {
        dropFolderTree(f.id);
        saveCollapsed();
      }
      state.editingFolderId = null;
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
        state.editingFolderId = null;
        render();
      }
    });
    inp.addEventListener("blur", commit);
    li.append(inp);
    setTimeout(() => inp.focus(), 0);
  } else {
    li.append(el("span", "folder-name", f.name));
  }
  // Postman 式 hover 操作：目录行右侧「＋ 直接新增快捷键」「⋯ 更多」。
  const add = el("button", "row-add", "＋") as HTMLButtonElement;
  add.title = tr("在此文件夹新增快捷键");
  add.addEventListener("click", (e) => {
    e.stopPropagation();
    void requestNew("shortcut", f.id);
  });
  const more = moreButton((btn) => showMenu(btn, folderMenuItems(f)));
  li.append(add, more);
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showMenu(li, folderMenuItems(f), { x: e.clientX, y: e.clientY });
  });

  // 点击目录行切换展开/收缩（重命名态与三点菜单/输入框除外）。
  if (hasChildren && state.editingFolderId !== f.id) {
    li.addEventListener("click", (e) => {
      if (state.suppressClick) return;
      if ((e.target as HTMLElement).closest("button, input")) return;
      toggleFolder(f.id);
    });
  }

  return li;
}

// 目录的菜单（⋯ / 右键共用）：新增在前，其后是目录自身的维护项。
function folderMenuItems(f: Folder): MenuItem[] {
  const items: MenuItem[] = [
    { label: tr("新增快捷键到此文件夹"), run: () => void requestNew("shortcut", f.id) },
    { label: tr("新增子文件夹"), run: () => addFolder(f.id) },
    { label: tr("重命名"), run: () => renameFolder(f.id) },
  ];
  // 目录只装快捷键：剪贴板里没有快捷键时，粘到目录里没有意义。
  if (state.clipboard?.items.some((i) => i.kind === "shortcut"))
    items.push({ label: tr("粘贴到该文件夹"), run: () => pasteIntoFolder(f.id) });
  const inFolder = picksInFolder(f.id);
  if (inFolder.length)
    items.push({
      label: trt`导出该文件夹为卡带（${inFolder.length} 项）`,
      run: () => void exportFragment(inFolder),
    });
  // 目录级启停：一次作用到该目录（含子目录）里的全部条目。
  const n = folderEntryCounts(collectDescendantIds(f.id)).reduce((sum, [, c]) => sum + c, 0);
  if (n) {
    items.push({ label: trt`启用该文件夹（${n} 项）`, run: () => void setFolderEnabled(f.id, true) });
    items.push({ label: trt`禁用该文件夹（${n} 项）`, run: () => void setFolderEnabled(f.id, false) });
  }
  items.push({ label: tr("删除"), run: () => void deleteFolder(f.id) });
  return items;
}

// ---- 配置卡带（规划 7.2-㉕）：把若干条目导出成一个可分享的 JSON 文件 ----

// 卡带点名表（后端 `export_fragment` 按「类别 + 下标」取条目）。
type SnippetPick = { kind: ListKind; idx: number };

// 当前选中集的点名表（对象引用 → 各自数组里的下标；不在任何数组里的略过）。
function picksOfSelection(): SnippetPick[] {
  const picks: SnippetPick[] = [];
  for (const o of state.picked) {
    const kind = kindOf(o);
    if (!kind) continue;
    const idx = arrOf(kind).indexOf(o as AnyEntry);
    if (idx >= 0) picks.push({ kind, idx });
  }
  return picks;
}

// 某个目录（含全部子目录）里所有条目的点名表——「导出该目录为卡带」用。
function picksInFolder(rootId: string): SnippetPick[] {
  const ids = new Set<string>([rootId]);
  // 目录树可嵌套，扫到不再新增为止（目录数量级很小，一趟轮询足够）。
  for (let grew = true; grew; ) {
    grew = false;
    for (const f of state.cfg.folders) {
      if (f.parent && ids.has(f.parent) && !ids.has(f.id)) {
        ids.add(f.id);
        grew = true;
      }
    }
  }
  const picks: SnippetPick[] = [];
  const scan = (arr: FolderEntry[], kind: ListKind) =>
    arr.forEach((e, idx) => {
      if (e.folder && ids.has(e.folder)) picks.push({ kind, idx });
    });
  scan(state.cfg.shortcuts, "shortcut");
  scan(state.cfg.remaps, "remap");
  scan(state.cfg.expansions, "expansion");
  return picks;
}

// 导出卡带：选路径 → 后端摘出这些条目写成 JSON。
// 卡带是「只含这几条的子集配置」，对方用「导入配置 → 合并」原样吃下，不会碰本机设置。
async function exportFragment(picks: SnippetPick[]) {
  if (!picks.length) return toast(tr("没有可导出的条目"));
  const path = await saveFile({
    defaultPath: tr("kada-卡带.json"),
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  try {
    const n = await invoke<number>("export_fragment", { path, picks });
    toast(trt`已导出 ${n} 项到 ${path}（对方用「导入配置 → 合并」即可）`);
  } catch (e) {
    toast(trt`导出失败: ${e}`);
  }
}

// ---- 卡带市场（规划 7.2-㉕）：左栏一级页 / 搜索 / 标签筛选 / 一键安装 ----
//
// 没有服务端：索引是一份能下载到的静态 JSON（地址在设置里，默认指向 ihomy 的
// snippets/index.json）。索引一次拿全，搜索与筛选都在本地做——几百条以内不值得为它加服务端。

type IndexEntry = {
  name: string;
  url: string;
  description?: string;
  author?: string;
  tags?: string[];
  level?: number;
};

// 卡带机审结果（后端 `scan_snippet`）：0 安全 / 1 低 / 2 中 / 3 高。
type SnippetScan = {
  level: number;
  entries: number;
  findings: string[];
  legal_flags: string[];
};

// 卡带下载结果（后端 `fetch_snippet`）：清洗后的条目 + 原文（装的时候回传原文，
// 保证「看到什么装什么」）。
type SnippetFetch = {
  text: string;
  scan: SnippetScan;
  preview: {
    shortcuts: ShortcutItem[];
    remaps: Remap[];
    expansions: TextExpansion[];
    gestures?: Gesture[];
    touchpad_gestures?: TouchpadGesture[];
    hot_corners?: HotCornerRule[];
    wheel_edges?: WheelEdgeRule[];
  };
};

let marketEntries: IndexEntry[] = [];
// 成功拿到过索引；失败不置位，重新进页面会自动重试。
let marketLoaded = false;
let marketLoading = false;
let marketError = "";
let marketQuery = "";
let marketTag: string | null = null;
// 本次会话里装过的卡带：只给卡片标一下，不落盘——用户可能随后删掉条目，
// 持久化成「已安装」会骗人。
const marketInstalled = new Set<string>();

// 预览框点「安装」时要用：null = 没待装的卡带。`level` = 机审危险等级（高危要额外确认）。
let snippetPending: { text: string; name: string; level: number } | null = null;

// 进页面 / 点「重新拉取」时调用；`force` = 忽略已拿到的索引重新下载。
async function loadMarket(force = false) {
  if (marketLoading) return;
  if (marketLoaded && !force) {
    renderMarketPage();
    return;
  }
  marketLoading = true;
  marketError = "";
  renderMarketPage();
  try {
    marketEntries = await invoke<IndexEntry[]>("fetch_snippet_index");
    marketLoaded = true;
  } catch (e) {
    marketEntries = [];
    marketError = String(e);
  } finally {
    marketLoading = false;
    renderMarketPage();
  }
}

// 索引里的标签按出现次数排序（同次数按字典序）：常用的分类先出现，列表不会每次刷新换个顺序。
function marketTags(): string[] {
  const count = new Map<string, number>();
  for (const e of marketEntries) {
    for (const raw of e.tags ?? []) {
      const tag = raw.trim();
      if (tag) count.set(tag, (count.get(tag) ?? 0) + 1);
    }
  }
  return [...count.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "zh"))
    .map(([tag]) => tag);
}

function renderMarketPage() {
  const note = document.getElementById("market-note");
  const tagsBox = document.getElementById("market-tags");
  const list = document.getElementById("market-list");
  if (!note || !tagsBox || !list) return;

  note.textContent = marketLoading
    ? tr("正在拉取索引…")
    : marketError
      ? trt`拉取失败：${marketError}（可在「设置 → 配置与数据」里换索引地址）`
      : marketLoaded
        ? trt`共 ${marketEntries.length} 条`
        : tr("还没拉取索引");
  // 发布只对官方市场开放：自定义索引是别人的市场，把卡带发到 ihomy 既不在用户眼前，
  // 也没有取得同意（服务端也不认自定义索引里的规则）。
  document.getElementById("market-publish")?.classList.toggle("hidden", !marketIsOfficial());

  // 索引换了（或第一条标签被删掉）时把已选标签收掉，免得停在一个不存在的标签上筛出空列表。
  const tags = marketTags();
  if (marketTag && !tags.includes(marketTag)) marketTag = null;
  tagsBox.classList.toggle("hidden", tags.length < 2);
  const tagButton = (tag: string | null, label: string): HTMLButtonElement => {
    const b = el("button", "market-tag", label) as HTMLButtonElement;
    const on = tag === marketTag;
    b.classList.toggle("active", on);
    b.setAttribute("aria-pressed", String(on));
    b.addEventListener("click", () => {
      marketTag = tag;
      renderMarketPage();
    });
    return b;
  };
  tagsBox.replaceChildren(tagButton(null, tr("全部")), ...tags.map((t) => tagButton(t, t)));

  if (marketLoading) {
    list.replaceChildren(el("p", "market-empty", tr("正在拉取索引…")));
    return;
  }
  if (marketError) {
    list.replaceChildren(
      el("p", "market-empty", tr("索引拉不下来：检查网络，或在「设置 → 配置与数据」里换一个索引地址后点「重新拉取」。")),
    );
    return;
  }
  const q = marketQuery.trim().toLowerCase();
  const hit = marketEntries.filter((e) => {
    if (marketTag && !(e.tags ?? []).includes(marketTag)) return false;
    if (!q) return true;
    return [e.name, e.description ?? "", e.author ?? "", ...(e.tags ?? [])]
      .join(" ")
      .toLowerCase()
      .includes(q);
  });
  if (!hit.length) {
    list.replaceChildren(
      el("p", "market-empty", q || marketTag ? tr("没有匹配的卡带") : tr("索引里一条卡带也没有")),
    );
    return;
  }
  list.replaceChildren(...hit.map(marketCard));
}

function riskLevel(level: number | undefined): number {
  return typeof level === "number" ? Math.max(0, Math.min(3, level)) : -1;
}

function riskLabel(level: number): string {
  return level <= 0
    ? tr("安全")
    : level === 1
      ? tr("低风险")
      : level === 2
        ? tr("中风险")
        : tr("高风险");
}

function riskHint(level: number): string {
  return level <= 0
    ? tr("只有文本 / 改键 / 层这类内容，不执行程序")
    : level === 1
      ? tr("会注入按键或文本、操作鼠标 / 剪贴板")
      : level === 2
        ? tr("会启动程序、打开文件 / 网址或改系统状态")
        : tr("含命令 / 脚本动作，可能运行任意代码");
}

function riskBadge(level: number): HTMLElement {
  const b = el("span", `risk-badge risk-${riskLevel(level)}`, riskLabel(level));
  b.title = riskHint(level);
  return b;
}

// 机器检查结果块：发布页与安装预览共用。只显示结论与命中项，不渲染卡带内容。
function renderScanBox(box: HTMLElement, scan: SnippetScan | null, error = "") {
  box.replaceChildren();
  if (error) {
    box.append(el("p", "scan-error", error));
    return;
  }
  if (!scan) {
    box.append(el("p", "scan-empty", tr("选择条目后开始机器检查")));
    return;
  }
  const level = riskLevel(scan.level);
  const head = el("div", "scan-head");
  head.append(riskBadge(level), el("span", "scan-count", trt`共 ${scan.entries} 条`));
  box.append(head);
  if (scan.findings.length) {
    const ul = el("ul", "scan-list");
    for (const f of scan.findings) ul.append(el("li", undefined, f));
    box.append(ul);
  }
  if (scan.legal_flags.length) {
    const flags = el("div", "scan-legal");
    flags.append(el("strong", undefined, tr("法务关键词（会重点人工审核）：")));
    for (const f of scan.legal_flags) flags.append(el("span", undefined, f));
    box.append(flags);
  }
  if (!scan.findings.length && !scan.legal_flags.length) {
    box.append(el("p", "scan-empty", tr("未发现风险动作或法务关键词")));
  }
}

const DEFAULT_SNIPPET_INDEX_URL = "https://ihomy.top/files/kada/snippets/index.json";

function marketIsOfficial(): boolean {
  const url = state.cfg.settings.snippet_index_url?.trim();
  return !url || url === DEFAULT_SNIPPET_INDEX_URL;
}

function marketCard(e: IndexEntry): HTMLElement {
  const card = el("div", "market-card");
  const head = el("div", "market-card-head");
  const installed = marketInstalled.has(e.name);
  const btn = el(
    "button",
    installed ? undefined : "primary",
    installed ? tr("已安装") : tr("安装…"),
  ) as HTMLButtonElement;
  card.classList.toggle("installed", installed);
  btn.title = installed
    ? tr("再装一次只会补上缺失的条目（撞车的跳过）")
    : tr("先预览里面的条目，确认后再合并进来");
  btn.addEventListener("click", () => void previewSnippet(e));
  head.append(el("span", "market-name", e.name));
  if (typeof e.level === "number") head.append(riskBadge(e.level));
  head.append(el("span", "flex-spacer"), btn);
  card.append(head);
  if (e.description) card.append(el("div", "market-desc", e.description));
  if (e.tags?.length) {
    const chips = el("div", "market-tags market-card-tags");
    for (const t of e.tags) {
      const chip = el("button", "market-tag", t) as HTMLButtonElement;
      chip.title = trt`只看「${t}」标签`;
      chip.addEventListener("click", () => {
        marketTag = t;
        renderMarketPage();
      });
      chips.append(chip);
    }
    card.append(chips);
  }
  if (e.author) card.append(el("div", "market-meta", trt`分享者：${e.author}`));
  return card;
}

// 「索引设置」：索引地址留在设置页（配置与数据），市场页只负责浏览——两件事不挤在一屏。
async function gotoMarketSettings() {
  await switchSection("settings");
  document.getElementById("snippet-index-row")?.scrollIntoView({ block: "center", behavior: "smooth" });
  (document.getElementById("set-snippet-index") as HTMLInputElement | null)?.focus();
}

// 卡带装进来就会跑：危险等级由后端 `scan_snippet` 统一判定（发布页与安装预览同源）。
async function previewSnippet(e: IndexEntry) {
  try {
    const r = await invoke<SnippetFetch>("fetch_snippet", { url: e.url });
    const p = r.preview;
    const total =
      p.shortcuts.length +
      p.remaps.length +
      p.expansions.length +
      (p.gestures?.length ?? 0) +
      (p.touchpad_gestures?.length ?? 0) +
      (p.hot_corners?.length ?? 0) +
      (p.wheel_edges?.length ?? 0);
    const level = riskLevel(r.scan.level);
    snippetPending = { text: r.text, name: e.name, level };
    document.getElementById("snippet-name")!.textContent = trt`${e.name}：共 ${total} 条`;
    const lines: HTMLElement[] = [];
    for (const s of p.shortcuts)
      lines.push(
        el(
          "li",
          undefined,
          trt`快捷键 ${s.triggers.join(" / ")}${s.name ? ` · ${s.name}` : ""} → ${s.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const r2 of p.remaps)
      lines.push(el("li", undefined, trt`改键 ${r2.from} → ${remapSummary(r2)}`));
    for (const x of p.expansions)
      lines.push(el("li", undefined, trt`文本扩展 ${x.trigger} → ${x.replace.slice(0, 40)}`));
    for (const g of p.gestures ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`鼠标操作 ${g.pattern}${g.name ? ` · ${g.name}` : ""} → ${g.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const g of p.touchpad_gestures ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`触摸板手势 ${g.trigger ?? ""}·${g.pattern}${g.name ? ` · ${g.name}` : ""} → ${g.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const c of p.hot_corners ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`角动作 ${c.spot}${c.name ? ` · ${c.name}` : ""} → ${c.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const w of p.wheel_edges ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`边缘滚动 ${w.edge}${w.name ? ` · ${w.name}` : ""} → ${w.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    document.getElementById("snippet-list")!.replaceChildren(...lines);
    renderScanBox(document.getElementById("snippet-scan")!, r.scan);
    const dangerous = level >= 2;
    const warn = document.getElementById("snippet-warn")!;
    warn.classList.toggle("hidden", !dangerous);
    if (dangerous) {
      warn.textContent =
        level >= 3
          ? tr("⚠ 危险等级：高。这个卡带含命令 / 脚本动作，装进来照着触发键就会执行任意代码。")
          : tr("⚠ 危险等级：中。这个卡带会启动程序、打开文件 / 网址或改系统状态，请先看清动作。");
    }
    const consentRow = document.getElementById("snippet-consent-row")!;
    const consent = document.getElementById("snippet-consent") as HTMLInputElement;
    consentRow.classList.toggle("hidden", !dangerous);
    consent.checked = false;
    (document.getElementById("snippet-install") as HTMLButtonElement).disabled = dangerous;
    document.getElementById("snippet-modal")!.classList.remove("hidden");
  } catch (err) {
    toast(trt`卡带下载失败：${err}`);
  }
}

function hideSnippetPreview() {
  document.getElementById("snippet-modal")!.classList.add("hidden");
  snippetPending = null;
}

async function confirmSnippetInstall() {
  const pending = snippetPending;
  if (!pending) return;
  if (pending.level >= 2) {
    const consent = document.getElementById("snippet-consent") as HTMLInputElement;
    if (!consent.checked) {
      toast(tr("请先勾选「我已了解风险」再安装"));
      return;
    }
  }
  hideSnippetPreview();
  try {
    const r = await invoke<ImportOutcome>("install_snippet", { text: pending.text });
    await load();
    marketInstalled.add(pending.name);
    renderMarketPage();
    showImportResult(r);
  } catch (e) {
    toast(trt`安装失败: ${e}`);
  }
}

// ---- 卡带发布（本机机审 → ihomy 服务端复检 → 人工审核）----
//
// 发布内容 = 选中的条目组成子集配置（与「导出卡带」同一口径），目录 / 设置 / 本机路径
// 一律不进 JSON。危险等级与法务关键词只做提示：真正能不能上架由 ihomy 的人工审核决定。

type PublishPick = { kind: ListKind; idx: number };
type PreparedPublish = { content: string; scan: SnippetScan };
type PublishReceipt = { id: number; status: string; level: number };

const PUBLISH_KINDS: ListKind[] = [
  "shortcut",
  "remap",
  "expansion",
  "gesture",
  "touchpad",
  "hotcorner",
  "wheeledge",
];

let publishSelected = new Set<string>();
let publishScan: SnippetScan | null = null;
let publishScanError = "";
let publishScanBusy = false;
let publishNameTouched = false;
let publishSubmitting = false;
let publishSeq = 0;
let publishTimer: number | undefined;

function publishKey(kind: ListKind, idx: number): string {
  return `${kind}:${idx}`;
}

function publishPicks(): PublishPick[] {
  const picks: PublishPick[] = [];
  for (const kind of PUBLISH_KINDS) {
    arrOf(kind).forEach((_, idx) => {
      if (publishSelected.has(publishKey(kind, idx))) picks.push({ kind, idx });
    });
  }
  return picks;
}

function readPublishMeta() {
  const value = (id: string) => (document.getElementById(id) as HTMLInputElement).value.trim();
  const tags = value("publish-tags")
    .split(/[,，]/)
    .map((t) => t.trim())
    .filter(Boolean);
  return {
    name: value("publish-name"),
    description: (document.getElementById("publish-desc") as HTMLTextAreaElement).value.trim(),
    author: value("publish-author"),
    tags,
  };
}

// 未手动改过名称时，按选择自动起名（单条取条目名，多条报条数）。
function syncPublishName() {
  if (publishNameTouched) return;
  const picks = publishPicks();
  const name =
    picks.length === 0
      ? ""
      : picks.length === 1
        ? labelOf(arrOf(picks[0].kind)[picks[0].idx] as object)
        : trt`卡带（${picks.length} 项）`;
  (document.getElementById("publish-name") as HTMLInputElement).value = name;
}

function renderPublishPicks() {
  const box = document.getElementById("publish-picks");
  if (!box) return;
  const rows: HTMLElement[] = [];
  for (const kind of PUBLISH_KINDS) {
    arrOf(kind).forEach((o, idx) => {
      const key = publishKey(kind, idx);
      const row = el("label", "publish-pick-row");
      const cb = el("input") as HTMLInputElement;
      cb.type = "checkbox";
      cb.checked = publishSelected.has(key);
      cb.addEventListener("change", () => {
        if (cb.checked) publishSelected.add(key);
        else publishSelected.delete(key);
        document.getElementById("publish-count")!.textContent = trt`已选 ${publishSelected.size} 条`;
        syncPublishName();
        schedulePublishScan();
      });
      row.append(cb, el("span", "publish-pick-kind", kindLabel(kind)), el("span", "publish-pick-name", labelOf(o as object)));
      rows.push(row);
    });
  }
  if (!rows.length) rows.push(el("p", "scan-empty", tr("当前配置里没有可发布的条目")));
  box.replaceChildren(...rows);
  document.getElementById("publish-count")!.textContent = trt`已选 ${publishSelected.size} 条`;
}

function updatePublishSubmit() {
  const consent = document.getElementById("publish-consent") as HTMLInputElement;
  const btn = document.getElementById("publish-submit") as HTMLButtonElement;
  btn.disabled =
    publishSubmitting ||
    publishScanBusy ||
    !publishScan ||
    !!publishScanError ||
    publishSelected.size === 0 ||
    !consent.checked;
}

function schedulePublishScan() {
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  const box = document.getElementById("publish-scan");
  if (publishSelected.size === 0) {
    publishSeq++;
    publishScan = null;
    publishScanError = "";
    publishScanBusy = false;
    if (box) renderScanBox(box, null);
    updatePublishSubmit();
    return;
  }
  publishScanBusy = true;
  publishScan = null;
  publishScanError = "";
  if (box) box.replaceChildren(el("p", "scan-empty", tr("正在做机器检查…")));
  updatePublishSubmit();
  publishTimer = window.setTimeout(() => void runPublishScan(), 250);
}

async function runPublishScan() {
  const seq = ++publishSeq;
  const picks = publishPicks();
  const meta = readPublishMeta();
  const box = document.getElementById("publish-scan");
  try {
    const r = await invoke<PreparedPublish>("prepare_publish", { picks, meta });
    if (seq !== publishSeq) return;
    publishScan = r.scan;
    publishScanError = "";
    if (box) renderScanBox(box, r.scan);
  } catch (e) {
    if (seq !== publishSeq) return;
    publishScan = null;
    publishScanError = String(e);
    if (box) renderScanBox(box, null, publishScanError);
  } finally {
    if (seq === publishSeq) {
      publishScanBusy = false;
      updatePublishSubmit();
    }
  }
}

function openPublish(initial: PublishPick[] = []) {
  if (!marketIsOfficial()) {
    toast(tr("当前使用自定义索引，不能发布到官方市场；请先在设置里换回内置地址"));
    return;
  }
  publishSelected = new Set(initial.map((p) => publishKey(p.kind, p.idx)));
  publishNameTouched = false;
  publishScan = null;
  publishScanError = "";
  publishScanBusy = false;
  publishSubmitting = false;
  publishSeq++;
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  (document.getElementById("publish-desc") as HTMLTextAreaElement).value = "";
  (document.getElementById("publish-tags") as HTMLInputElement).value = "";
  (document.getElementById("publish-author") as HTMLInputElement).value =
    localStorage.getItem("kada.publish.author") ?? "";
  (document.getElementById("publish-consent") as HTMLInputElement).checked = false;
  renderPublishPicks();
  syncPublishName();
  renderScanBox(document.getElementById("publish-scan")!, null);
  updatePublishSubmit();
  document.getElementById("publish-modal")!.classList.remove("hidden");
  schedulePublishScan();
}

function closePublish() {
  document.getElementById("publish-modal")!.classList.add("hidden");
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  publishSeq++;
  publishSubmitting = false;
}

async function submitPublish() {
  if (publishSubmitting || !publishScan || publishScanError || publishSelected.size === 0) return;
  const meta = readPublishMeta();
  publishSubmitting = true;
  updatePublishSubmit();
  try {
    const r = await invoke<PublishReceipt>("submit_publish", { picks: publishPicks(), meta });
    localStorage.setItem("kada.publish.author", meta.author);
    closePublish();
    toast(trt`已提交审核（编号 #${r.id}，服务端判定：${riskLabel(r.level)}）；人工通过后才会出现在市场`);
  } catch (e) {
    toast(trt`发布失败：${e}`);
  } finally {
    publishSubmitting = false;
    updatePublishSubmit();
  }
}

function moreButton(onClick: (btn: HTMLButtonElement) => void): HTMLButtonElement {
  const b = el("button", "row-more", "⋯") as HTMLButtonElement;
  b.title = tr("更多");
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick(b);
  });
  return b;
}

// 行内「执行」按钮（⋯ 左边）：把这条条目的动作串直接跑一遍。
// 没有动作串的条目（改键 / 文本扩展）不出这个按钮。
function runButton(actions: Action[], label: string): HTMLButtonElement {
  const b = el("button", "row-more row-run") as HTMLButtonElement;
  b.title = tr("执行");
  b.setAttribute("aria-label", tr("执行"));
  b.innerHTML =
    '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M8 5v14l11-7z"/></svg>';
  b.addEventListener("click", (e) => {
    e.stopPropagation();
    void runActions(actions, label);
  });
  return b;
}

type MenuItem = { label: string; run: () => void; on?: boolean };

// 选中集上的操作（行内 ⋯ 与右键菜单同一套，作用对象都是 picked）。
function selectionMenuItems(): MenuItem[] {
  const n = state.picked.size;
  return [
    { label: n > 1 ? trt`各复制一份（${n} 项）` : tr("复制一份"), run: duplicateSelection },
    { label: tr("复制"), run: () => copySelection(false) },
    { label: tr("剪切"), run: () => copySelection(true) },
    { label: n > 1 ? trt`导出为卡带（${n} 项）` : tr("导出为卡带"), run: () => void exportFragment(picksOfSelection()) },
    { label: n > 1 ? trt`发布为卡带（${n} 项）` : tr("发布为卡带…"), run: () => openPublish(picksOfSelection()) },
    { label: n > 1 ? trt`删除（${n} 项）` : tr("删除"), run: () => void deleteSelection() },
    { label: tr("启用"), run: () => void setPickedEnabled(true) },
    { label: tr("禁用"), run: () => void setPickedEnabled(false) },
  ];
}

// 行内 ⋯：这行不在选中集里就先只选它，然后出选中集菜单（与右键一致）。
function openRowMenu(anchor: HTMLElement, li: HTMLElement, obj: object) {
  if (!state.picked.has(obj)) selectOnly(obj, li);
  showMenu(anchor, selectionMenuItems());
}

// 通用下拉菜单：行 ⋯ / 行右键 / 目录右键 / 搜索栏的「筛选」「新增」共用同一个元素。
// 同一个锚点再点一次 = 收起（不重复弹出）。
function showMenu(anchor: HTMLElement, items: MenuItem[], at?: { x: number; y: number }) {
  const menu = document.getElementById("row-menu")!;
  if (state.menuAnchor === anchor && !menu.classList.contains("hidden")) {
    hideRowMenu();
    return;
  }
  menu.replaceChildren();
  for (const it of items) {
    const b = el("button", "menu-item" + (it.on ? " on" : ""), (it.on ? "✓ " : "") + it.label) as HTMLButtonElement;
    b.setAttribute("role", "menuitem");
    b.addEventListener("click", () => {
      hideRowMenu();
      it.run();
    });
    menu.append(b);
  }
  // 先显示再测宽：右缘对齐锚点右缘（贴近按钮、避免右溢出）；右键则贴着光标。
  // 底部放不下则向上翻转，避免列表末尾的菜单被裁到页面底下看不到。
  menu.classList.remove("hidden");
  const rect = anchor.getBoundingClientRect();
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  menu.style.left = `${Math.min(Math.max(8, at ? at.x : rect.right - w), Math.max(8, vw - w - 8))}px`;
  let top = (at ? at.y : rect.bottom) + 4;
  if (top + h > vh - 4) top = Math.max(4, (at ? at.y : rect.top) - h - 4);
  menu.style.top = `${top}px`;
  // 键盘：打开即聚焦第一项；Esc 关菜单并把焦点还给锚点，Tab 走人就收摊
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
  state.menuAnchor = anchor;
}

function hideRowMenu() {
  document.getElementById("row-menu")!.classList.add("hidden");
  state.menuAnchor = null;
}

// 点到菜单/它的锚点外面 = 关菜单（锚点自己的 click 先跑，这里要能分辨）。
function closeMenuUnlessIn(t: HTMLElement) {
  if (!state.menuAnchor) return;
  if (state.menuAnchor.contains(t) || t.closest("#row-menu")) return;
  hideRowMenu();
}

// 目录是否有子内容（子目录或条目），用于决定是否显示展开箭头。
function folderHasChildren(id: string): boolean {
  return (
    state.cfg.folders.some((f) => f.parent === id) ||
    state.cfg.shortcuts.some((s) => s.folder === id) ||
    state.cfg.remaps.some((r) => r.folder === id) ||
    state.cfg.expansions.some((e) => e.folder === id)
  );
}

// 切换目录展开/收缩（仅 UI 状态，不进配置文件，存 localStorage）。
function toggleFolder(id: string) {
  if (collapsedFolders.has(id)) collapsedFolders.delete(id);
  else collapsedFolders.add(id);
  saveCollapsed();
  renderListPane();
}

function newFolderId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `f-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function addFolder(parentId: string | null) {
  const f: Folder = { id: newFolderId(), name: "", parent: parentId };
  state.cfg.folders.push(f);
  state.editingFolderId = f.id;
  renderListPane();
}

function renameFolder(id: string) {
  state.editingFolderId = id;
  renderListPane();
}

// 确认框统一入口（自绘）。删除 / 层 / 目录 / 外部改动冲突这类「不可逆或会丢东西」的操作都走这里。
// 原先借系统 `ask()`：那是全应用唯一一处灰白系统框，跟四套主题割裂，而且被 ACL 拒绝时只会变成
// 一条没人看的 unhandled rejection（表现就是「点了删除没反应」）。
// **焦点默认落在「取消」**：危险操作不给隐式回车，习惯性连按回车不该把东西删掉。
interface ConfirmOptions {
  title: string;
  text: string;
  okLabel: string;
  cancelLabel?: string;
  /** 「确定」按钮用危险色（删除这类不可逆操作）。 */
  danger?: boolean;
}

let confirmPending: ((ok: boolean) => void) | null = null;

export function askConfirm(opts: ConfirmOptions): Promise<boolean> {
  // 已经有一个确认框在等：后来的请求按「取消」处理，否则先到那个 promise 永远不 resolve。
  if (confirmPending) return Promise.resolve(false);
  document.getElementById("confirm-title")!.textContent = opts.title;
  document.getElementById("confirm-text")!.textContent = opts.text;
  const ok = document.getElementById("confirm-ok") as HTMLButtonElement;
  ok.textContent = opts.okLabel;
  ok.className = opts.danger ? "danger" : "primary";
  document.getElementById("confirm-cancel")!.textContent = opts.cancelLabel ?? tr("取消");
  document.getElementById("confirm-modal")!.classList.remove("hidden");
  // 先摘掉 `hidden` 再 focus：`display: none` 的子树里 focus() 是空操作，焦点会留在原地。
  document.getElementById("confirm-cancel")!.focus();
  return new Promise((resolve) => (confirmPending = resolve));
}

function resolveConfirm(ok: boolean) {
  const pending = confirmPending;
  confirmPending = null;
  document.getElementById("confirm-modal")!.classList.add("hidden");
  pending?.(ok);
}

// 删除单条内容的统一确认。原先只有「删除目录 / 删除层」问一句，同为破坏性操作的单条删除
// （快捷键 / 改键 / 文本扩展）却是点了就没了——同类实体的安全级别不该不一致。
// 未落盘的新增项不弹：删除它等同「取消」，没有任何已保存的内容会丢。
async function confirmDelete(kind: string, name: string, discardsDraft: boolean): Promise<boolean> {
  const dirty = discardsDraft ? tr("该条还有未保存的改动，会一并丢弃。") : "";
  return await askConfirm({
    title: trt`删除${kind}`,
    text: trt`删除${kind}「${name}」？${dirty}删除后应用内无法撤销。`,
    okLabel: tr("删除"),
    danger: true,
  });
}

// 快捷键的展示名（无名时退回触发键；列表行显示的是「触发键 + 名称」，确认框要对得上号）。
function shortcutLabel(s: ShortcutItem): string {
  return s.name?.trim() || s.triggers.join(" / ") || tr("未命名");
}

const FOLDER_ENTRY_KINDS: [string, () => FolderEntry[]][] = [
  [tr("快捷键"), () => state.cfg.shortcuts],
  [tr("改键"), () => state.cfg.remaps],
  [tr("文本扩展"), () => state.cfg.expansions],
  [tr("鼠标操作"), () => (state.cfg.gestures ?? []) as unknown as FolderEntry[]],
  [tr("触摸板手势"), () => (state.cfg.touchpad_gestures ?? []) as unknown as FolderEntry[]],
  [tr("角动作"), () => (state.cfg.hot_corners ?? []) as unknown as FolderEntry[]],
  [tr("边缘滚动"), () => (state.cfg.wheel_edges ?? []) as unknown as FolderEntry[]],
];

// 这批目录（含子目录）里各类条目各有多少条，0 条的不报。
function folderEntryCounts(ids: Set<string>): [string, number][] {
  return FOLDER_ENTRY_KINDS.map(([label, list]) => [label, list().filter((e) => e.folder && ids.has(e.folder)).length] as [string, number]).filter(
    ([, n]) => n > 0,
  );
}

// 删目录 = 连目录带里面的条目一起删（三类条目都算，不然 folder 悬空、条目在列表里凭空消失又被 save 挪回根级）。
function dropFolderTree(id: string) {
  const ids = collectDescendantIds(id);
  for (const [, list] of FOLDER_ENTRY_KINDS) {
    const arr = list();
    // 原地删除：条目对象被草稿/选中态按引用持有，换数组会让它们变孤儿。
    for (let i = arr.length - 1; i >= 0; i--) {
      const f = arr[i].folder;
      if (f && ids.has(f)) arr.splice(i, 1);
    }
  }
  state.cfg.folders = state.cfg.folders.filter((f) => !ids.has(f.id));
  for (const fid of ids) collapsedFolders.delete(fid);
}

async function deleteFolder(id: string) {
  const target = state.cfg.folders.find((f) => f.id === id);
  if (!target) return;
  const detail = folderEntryCounts(collectDescendantIds(id))
    .map(([label, n]) => trt`${n} 条${label}`)
    .join("、");
  const ok = await askConfirm({
    title: tr("删除文件夹"),
    text: trt`删除文件夹「${target.name}」${detail ? trt`及其中的 ${detail}` : tr("及其所有内容")}？删除后无法撤销。`,
    okLabel: tr("删除"),
    danger: true,
  });
  if (!ok) return;
  dropFolderTree(id);
  saveCollapsed();
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  void save();
}

function collectDescendantIds(id: string): Set<string> {
  const ids = new Set<string>([id]);
  let changed = true;
  while (changed) {
    changed = false;
    for (const f of state.cfg.folders) {
      if (f.parent && ids.has(f.parent) && !ids.has(f.id)) {
        ids.add(f.id);
        changed = true;
      }
    }
  }
  return ids;
}

// 选中集操作（合并列表里各类条目共存，靠内容定位而不是下标）。
// AnyEntry / FolderEntry 定义在 ./types。
function kindOf(o: object): ListKind | null {
  if ((state.cfg.shortcuts as object[]).includes(o)) return "shortcut";
  if ((state.cfg.remaps as object[]).includes(o)) return "remap";
  if ((state.cfg.expansions as object[]).includes(o)) return "expansion";
  if ((state.cfg.gestures as object[]).includes(o)) return "gesture";
  if ((state.cfg.touchpad_gestures as object[]).includes(o)) return "touchpad";
  if ((state.cfg.hot_corners as object[]).includes(o)) return "hotcorner";
  if ((state.cfg.wheel_edges as object[]).includes(o)) return "wheeledge";
  return null;
}

function arrOf(kind: ListKind): AnyEntry[] {
  return kind === "shortcut"
    ? state.cfg.shortcuts
    : kind === "remap"
      ? state.cfg.remaps
      : kind === "expansion"
        ? state.cfg.expansions
        : kind === "gesture"
          ? (state.cfg.gestures ?? [])
          : kind === "touchpad"
            ? (state.cfg.touchpad_gestures ?? [])
            : kind === "hotcorner"
              ? (state.cfg.hot_corners ?? [])
              : (state.cfg.wheel_edges ?? []);
}

function kindLabel(kind: ListKind | null): string {
  return kind === "shortcut"
    ? tr("快捷键")
    : kind === "remap"
      ? tr("改键")
      : kind === "expansion"
        ? tr("文本扩展")
        : kind === "gesture"
          ? tr("鼠标操作")
          : kind === "touchpad"
            ? tr("触摸板手势")
            : kind === "hotcorner"
              ? tr("角动作")
              : kind === "wheeledge"
                ? tr("边缘滚动")
                : tr("条目");
}

// 条目的展示名（确认框要与列表行对得上号）：有标题用标题，否则退回各自的兜底。
function labelOf(o: object): string {
  const kind = kindOf(o);
  if (kind === "shortcut") return shortcutLabel(o as ShortcutItem);
  if (kind === "remap") {
    const r = o as Remap;
    return r.name?.trim() || `${r.from} → ${remapSummary(r)}`;
  }
  if (kind === "expansion") {
    const e = o as TextExpansion;
    return e.name?.trim() || e.trigger || tr("未命名");
  }
  if (kind === "gesture") {
    const g = o as Gesture;
    return g.name?.trim() || g.pattern || tr("未命名");
  }
  if (kind === "touchpad") {
    const g = o as TouchpadGesture;
    return g.name?.trim() || g.pattern || tr("未命名");
  }
  if (kind === "hotcorner") {
    const r = o as HotCornerRule;
    return r.name?.trim() || spotLabel(r.spot) || tr("未命名");
  }
  if (kind === "wheeledge") {
    const r = o as WheelEdgeRule;
    return r.name?.trim() || spotLabel(r.edge) || tr("未命名");
  }
  return tr("条目");
}

// 当前编辑页打开的那条（没有打开 / 不在列表分区时为 null）。
function selectedObject(): AnyEntry | null {
  if (state.selected === null) return null;
  if (state.section === "shortcuts") return state.cfg.shortcuts[state.selected] ?? null;
  if (state.section === "remaps") return state.cfg.remaps[state.selected] ?? null;
  if (state.section === "expansions") return state.cfg.expansions[state.selected] ?? null;
  if (state.section === "gestures") return (state.cfg.gestures ?? [])[state.selected] ?? null;
  if (state.section === "touchpads") return (state.cfg.touchpad_gestures ?? [])[state.selected] ?? null;
  if (state.section === "hotcorners") return (state.cfg.hot_corners ?? [])[state.selected] ?? null;
  if (state.section === "wheeledges") return (state.cfg.wheel_edges ?? [])[state.selected] ?? null;
  return null;
}

// 复制 / 剪切选中集：整批进剪贴板，粘贴时逐条落位（剪切的粘贴完才摘掉原条目）。
function copySelection(cut: boolean) {
  const items: NonNullable<typeof state.clipboard>["items"] = [];
  for (const o of state.picked) {
    const kind = kindOf(o);
    if (kind) items.push({ kind, obj: o as AnyEntry });
  }
  if (!items.length) return;
  state.clipboard = { items, cut };
  state.picked.clear();
  state.pickAnchor = null;
  renderListPane();
  const n = items.length;
  toast(cut ? trt`已剪切 ${n} 项，「＋ 新增 → 粘贴」移动过去` : trt`已复制 ${n} 项，「＋ 新增 → 粘贴」插进去`);
}

// 原位复制一份（不走剪贴板，省去「先复制再找地方粘」两步）：插在源条目后一条。
function duplicateSelection() {
  for (const o of state.picked) {
    const kind = kindOf(o);
    if (!kind) continue;
    const arr = arrOf(kind);
    const i = arr.indexOf(o as AnyEntry);
    if (i >= 0) arr.splice(i + 1, 0, deepClone(o as AnyEntry));
  }
  void save();
}

// 删除选中集（一次确认）。正在编辑的那条也在其中 → 连草稿一起丢。
async function deleteSelection() {
  const objs = [...state.picked];
  if (!objs.length) return;
  const n = objs.length;
  const editing = selectedObject();
  const hitEdit = editing !== null && objs.includes(editing);
  const what =
    n === 1 ? `${kindLabel(kindOf(objs[0]))}「${labelOf(objs[0])}」` : trt`选中的 ${n} 项`;
  const dirty = hitEdit && isDraftDirty() ? tr("正在编辑的那条还有未保存的改动，会一并丢弃。") : "";
  if (
    !(await askConfirm({
      title: tr("删除"),
      text: trt`删除${what}？${dirty}删除后应用内无法撤销。`,
      okLabel: tr("删除"),
      danger: true,
    }))
  ) {
    return;
  }
  const set = new Set(objs);
  state.cfg.shortcuts = state.cfg.shortcuts.filter((s) => !set.has(s));
  state.cfg.remaps = state.cfg.remaps.filter((r) => !set.has(r));
  state.cfg.expansions = state.cfg.expansions.filter((e) => !set.has(e));
  if (state.cfg.gestures) state.cfg.gestures = state.cfg.gestures.filter((g) => !set.has(g));
  if (state.cfg.touchpad_gestures)
    state.cfg.touchpad_gestures = state.cfg.touchpad_gestures.filter((g) => !set.has(g));
  if (state.cfg.hot_corners) state.cfg.hot_corners = state.cfg.hot_corners.filter((r) => !set.has(r));
  if (state.cfg.wheel_edges) state.cfg.wheel_edges = state.cfg.wheel_edges.filter((r) => !set.has(r));
  state.picked.clear();
  state.pickAnchor = null;
  if (editing) {
    if (hitEdit) {
      discardDraft();
      state.draftNew = false;
      state.selected = null;
    } else {
      state.selected = arrOf(kindOf(editing)!).indexOf(editing);
    }
  }
  void save();
}

function pasteIntoFolder(folderId: string | null) {
  if (!state.clipboard) return toast(tr("剪贴板为空"));
  const { items, cut } = state.clipboard;
  for (const it of items) {
    const copy = deepClone(it.obj);
    if (it.kind === "shortcut") {
      (copy as ShortcutItem).folder = folderId;
      state.cfg.shortcuts.push(copy as ShortcutItem);
    } else if (it.kind === "remap") {
      state.cfg.remaps.push(copy as Remap);
    } else if (it.kind === "gesture") {
      (state.cfg.gestures ?? (state.cfg.gestures = [])).push(copy as Gesture);
    } else if (it.kind === "touchpad") {
      (state.cfg.touchpad_gestures ?? (state.cfg.touchpad_gestures = [])).push(copy as TouchpadGesture);
    } else if (it.kind === "hotcorner") {
      (state.cfg.hot_corners ?? (state.cfg.hot_corners = [])).push(copy as HotCornerRule);
    } else if (it.kind === "wheeledge") {
      (state.cfg.wheel_edges ?? (state.cfg.wheel_edges = [])).push(copy as WheelEdgeRule);
    } else {
      state.cfg.expansions.push(copy as TextExpansion);
    }
  }
  if (cut) {
    const set = new Set(items.map((i) => i.obj));
    state.cfg.shortcuts = state.cfg.shortcuts.filter((s) => !set.has(s));
    state.cfg.remaps = state.cfg.remaps.filter((r) => !set.has(r));
    state.cfg.expansions = state.cfg.expansions.filter((e) => !set.has(e));
    if (state.cfg.gestures) state.cfg.gestures = state.cfg.gestures.filter((g) => !set.has(g));
    if (state.cfg.touchpad_gestures)
      state.cfg.touchpad_gestures = state.cfg.touchpad_gestures.filter((g) => !set.has(g));
    if (state.cfg.hot_corners) state.cfg.hot_corners = state.cfg.hot_corners.filter((r) => !set.has(r));
    if (state.cfg.wheel_edges) state.cfg.wheel_edges = state.cfg.wheel_edges.filter((r) => !set.has(r));
    state.clipboard = null;
  }
  void save();
}

// 详情页删除：确认通过才落删。三个编辑页共用一条路径，别再各写一遍（原先三处重复代码）。
async function deleteDetailEntry(kind: string, label: string, remove: () => void) {
  if (!(await confirmDelete(kind, label, isDraftDirty()))) return;
  remove();
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  void save();
}

// 新增项尚未落盘，删除 = 直接丢弃草稿：没有已保存的内容会丢，不必再弹一次确认。
function discardNewDraft() {
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  render();
}

// ---- 拖拽排序/移动 ----
// 合并列表只有一份：落点计算/指示都在它上面（三类行结构一致）。
function activeListEl(): HTMLElement {
  return document.getElementById("shortcut-list")!;
}

function clearDropIndicators() {
  document
    .querySelectorAll(".drop-before, .drop-after, .drop-into, .drop-forbidden")
    .forEach((n) =>
      n.classList.remove("drop-before", "drop-after", "drop-into", "drop-forbidden"),
    );
  document.querySelectorAll(".list").forEach((n) => n.classList.remove("drop-root"));
  state.dragOverEl = null;
}

function setDropIndicator(el: HTMLElement, cls: string) {
  // 同一行也要比 class：光标在行内越过中线时落点 before/after 得跟着翻，否则线钉死半行。
  if (state.dragOverEl === el && el.classList.contains(cls)) return;
  clearDropIndicators();
  el.classList.add(cls);
  state.dragOverEl = el;
}

// 目录移动后是否成环：新父目录不能是自身或其子孙。
function wouldCycle(zone: DropZone): boolean {
  if (!state.dragPayload || state.dragPayload.kind !== "folder") return false;
  const selfId = state.dragPayload.id;
  let newParent: string | null = null;
  if (zone.kind === "folder") {
    if (zone.pos === "into") newParent = zone.folderId;
    else newParent = state.cfg.folders.find((f) => f.id === zone.folderId)?.parent ?? null;
  } else if (zone.kind === "row") {
    newParent = state.cfg.shortcuts[zone.idx]?.folder ?? null;
  }
  return newParent !== null && collectDescendantIds(selfId).has(newParent);
}

// 合并列表里三类行同处一个 <ul>：落点必须先认 kind（下标分属不同数组，串了就是搬错条目）。
function isMouseKind(kind: string | null | undefined): boolean {
  return kind === "gesture" || kind === "hotcorner" || kind === "wheeledge";
}

/** 鼠标类三类共用的展示序列（`order` → 类型 → 数组下标，同一套比较器用于渲染与拖动）。 */
function mouseSequence(): { kind: "gesture" | "hotcorner" | "wheeledge"; item: Gesture | HotCornerRule | WheelEdgeRule; rank: number; idx: number }[] {
  const rank = { gesture: 0, hotcorner: 1, wheeledge: 2 } as const;
  const out: { kind: "gesture" | "hotcorner" | "wheeledge"; item: Gesture | HotCornerRule | WheelEdgeRule; rank: number; idx: number }[] = [];
  (state.cfg.gestures ?? []).forEach((g, i) => out.push({ kind: "gesture", item: g, rank: rank.gesture, idx: i }));
  (state.cfg.hot_corners ?? []).forEach((r, i) => out.push({ kind: "hotcorner", item: r, rank: rank.hotcorner, idx: i }));
  (state.cfg.wheel_edges ?? []).forEach((r, i) => out.push({ kind: "wheeledge", item: r, rank: rank.wheeledge, idx: i }));
  return out.sort((a, b) => (a.item.order ?? 0) - (b.item.order ?? 0) || a.rank - b.rank || a.idx - b.idx);
}

/** 触摸板手势自己的展示序列（自成一组，序列与鼠标操作互不干扰）。 */
function touchpadSequence(): { item: TouchpadGesture; idx: number }[] {
  return (state.cfg.touchpad_gestures ?? [])
    .map((item, idx) => ({ item, idx }))
    .sort((a, b) => (a.item.order ?? 0) - (b.item.order ?? 0) || a.idx - b.idx);
}

/** 新建触摸板手势排到本组最前：重排本组 `order`（新条目拿 0，其余顺延）。 */
function touchpadOrderFront(item: TouchpadGesture) {
  const seq = (state.cfg.touchpad_gestures ?? []).filter((g) => g !== item);
  seq.unshift(item);
  seq.forEach((g, i) => {
    g.order = i;
  });
}

/** 新建条目排到最前：重排三类共用的 `order`（新条目拿 0，其余顺延）。 */
function mouseOrderFront(item: Gesture | HotCornerRule | WheelEdgeRule) {
  const seq = mouseSequence().filter((e) => e.item !== item);
  seq.unshift({ kind: "gesture", item, rank: 0, idx: 0 });
  seq.forEach((e, i) => {
    e.item.order = i;
  });
}

function mouseItemOf(kind: ListKind, idx: number): (Gesture & HotCornerRule & WheelEdgeRule) | null {
  if (kind === "gesture") return ((state.cfg.gestures ?? [])[idx] as never) ?? null;
  if (kind === "hotcorner") return ((state.cfg.hot_corners ?? [])[idx] as never) ?? null;
  if (kind === "wheeledge") return ((state.cfg.wheel_edges ?? [])[idx] as never) ?? null;
  return null;
}

function touchpadItemOf(idx: number): TouchpadGesture | null {
  return (state.cfg.touchpad_gestures ?? [])[idx] ?? null;
}

function dragRowKind(): ListKind {
  if (!state.dragPayload) return "shortcut";
  return state.dragPayload.kind === "folder" ? "shortcut" : state.dragPayload.kind;
}

// 落点 → 承载它的行元素（指示与成环禁用共用，避免两处各查一遍选择器）。
function zoneEl(zone: DropZone): HTMLElement | null {
  if (zone.kind === "root") return null;
  const list = activeListEl();
  if (zone.kind === "folder") {
    return list.querySelector(
      `.folder-row[data-folder-id="${zone.folderId}"]`,
    ) as HTMLElement | null;
  }
  const kind = zone.targetKind ?? dragRowKind();
  return list.querySelector(
    `.row[data-kind="${kind}"][data-idx="${zone.idx}"]`,
  ) as HTMLElement | null;
}

// 单个候选元素上的落点：目录行按「上 1/4 放前 / 下 1/4 放后 / 中间放进」分三段；
// 普通行按中线分前后。快捷键/改键/扩展落到目录行上（不管哪一段）= 放进该目录（改键与
// 文本扩展同样能归目录）。候选可以是源行自己（源行 pointer-events 让位，光标常停在它身上）：
// 落自己 = 原地不动，由 applyDrop 直接放行，所以这里只给「前/后」不给「放进」，免得被当成做环。
function zoneOfEl(el: HTMLElement, y: number): DropZone {
  const r = el.getBoundingClientRect();
  if (el.classList.contains("folder-row")) {
    const folderId = el.dataset.folderId!;
    if (state.dragPayload?.kind !== "folder") return { kind: "folder", folderId, pos: "into" };
    const ratio = (y - r.top) / r.height;
    const isSelf = el === state.dragState?.sourceEl;
    return {
      kind: "folder",
      folderId,
      pos: ratio < 0.25 ? "before" : ratio > 0.75 ? "after" : isSelf ? (ratio < 0.5 ? "before" : "after") : "into",
    };
  }
  return {
    kind: "row",
    idx: Number(el.dataset.idx),
    pos: y < r.top + r.height / 2 ? "before" : "after",
    targetKind: el.dataset.kind as ListKind,
  };
}

// 光标没落在可直接命中的行上（异类行 / 行间缝隙 / 末尾空白 / 源行让开指针）时，按 y 在候选
// 里挑最近的元素定落点。缝隙按「接在上一行后面」算：行是平铺渲染的，接在光标上一行之后正好
// 落进这个缝里——不会一路退回「根」（顶栏那根线闪到列表顶上去，看着像落点自己跳走）。
// 上一行是目录行（缝在目录行正下方 = 该目录子树顶部）时改看下一行：下一行是条目就落它前面，
// 下一行是目录或没有就放进上面这个目录；列表顶端的缝落第一行之前。没有候选才回根。
function nearestDropZone(list: HTMLElement, y: number): DropZone | null {
  const kind = state.dragPayload?.kind;
  // 鼠标类三类共用一条展示序列；触摸板手势自成一组，只与本组行 + 目录行互为落点。
  const grouped = isMouseKind(kind) || kind === "touchpad";
  if (grouped) {
    const selector = isMouseKind(kind)
      ? '.row[data-kind="gesture"], .row[data-kind="hotcorner"], .row[data-kind="wheeledge"], .folder-row'
      : '.row[data-kind="touchpad"], .folder-row';
    const cands = Array.from(list.querySelectorAll<HTMLElement>(selector));
    const inside = cands.find((el) => {
      const r = el.getBoundingClientRect();
      return y >= r.top && y <= r.bottom;
    });
    const asZone = (el: HTMLElement): DropZone =>
      el.classList.contains("folder-row")
        ? { kind: "folder", folderId: el.dataset.folderId!, pos: "into" }
        : zoneOfEl(el, y);
    if (inside) return asZone(inside);
    // 整份列表首行之上 / 末行之下留一点空白：拖到那里 = 回根级（鼠标类只靠拖动改归属）。
    const allRows = Array.from(list.querySelectorAll<HTMLElement>(".row, .folder-row"));
    if (cands.length && allRows.length) {
      const first = allRows[0].getBoundingClientRect();
      const last = allRows[allRows.length - 1].getBoundingClientRect();
      if (y < first.top - 6 || y > last.bottom + 6) return { kind: "root" };
    }
    const up = cands.filter((el) => el.getBoundingClientRect().bottom < y).pop();
    const down = cands.find((el) => el.getBoundingClientRect().top > y);
    const only = up ?? down;
    return only ? asZone(only) : { kind: "root" };
  }
  // 快捷键/改键/扩展与目录互为落点；own = 拖目录时的成环黑名单（含源目录自己）。
  const own = kind === "folder" ? collectDescendantIds(state.dragPayload!.id) : null;
  const sel = `.folder-row, .row[data-kind="${dragRowKind()}"]`;
  const els = Array.from(list.querySelectorAll<HTMLElement>(sel)).filter((el) => {
    // 源目录子树里的条目：落它等于把目录塞回自己（成环），不算候选（源目录行本身算）。
    if (own && el.dataset.idx != null) {
      const f = state.cfg.shortcuts[Number(el.dataset.idx)]?.folder ?? null;
      if (f && own.has(f)) return false;
    }
    return true;
  });
  if (els.length === 0) return { kind: "root" };
  const isFolderRow = (el: HTMLElement) => el.classList.contains("folder-row");
  const folderZone = (el: HTMLElement, pos: "before" | "into" | "after"): DropZone => ({
    kind: "folder",
    folderId: el.dataset.folderId!,
    pos,
  });
  // 放进该目录；候选是源目录自己时只给「前/后」——放进自己放下后原地不动（applyDrop 早退），
  // 却在源行上亮「放进」的框，看着像真会动。
  const intoZone = (el: HTMLElement): DropZone =>
    el === state.dragState?.sourceEl ? zoneOfEl(el, y) : folderZone(el, "into");
  const inside = els.find((el) => {
    const r = el.getBoundingClientRect();
    return y >= r.top && y <= r.bottom;
  });
  if (inside) return zoneOfEl(inside, y);
  // 条目落到「某目录之前」= 该层级条目区的末尾（同级条目渲染在目录行之前），只有同级里
  // 排第一的目录才真落在它上面；落到非首个同级目录前会把条目甩回该层级块顶上，离光标
  // 好几行——那种情况改看别的候选。目录拖目录 = 同级换位，始终落在那儿，不受此限。
  const beforeFolderZone = (el: HTMLElement): DropZone | null => {
    const folderId = el.dataset.folderId!;
    if (state.dragPayload?.kind === "folder") return { kind: "folder", folderId, pos: "before" };
    const i = state.cfg.folders.findIndex((x) => x.id === folderId);
    if (i < 0) return null;
    const parent = state.cfg.folders[i].parent ?? null;
    const first = !state.cfg.folders.slice(0, i).some((x) => (x.parent ?? null) === parent);
    return first ? { kind: "folder", folderId, pos: "before" } : null;
  };
  const up = els.filter((el) => el.getBoundingClientRect().bottom < y).pop();
  const down = els.find((el) => el.getBoundingClientRect().top > y);
  if (!up || !down) {
    const only = up ?? down;
    if (!only) return { kind: "root" };
    if (!isFolderRow(only)) return zoneOfEl(only, y);
    return up ? intoZone(up) : (beforeFolderZone(down!) ?? { kind: "root" });
  }
  // 缝隙两边都有候选：离光标更近的那边才对得上「落在哪」——候选里夹着别的类型时（改键/
  // 扩展不在快捷键的候选里），远的那边会把落点拽出好几行。等远就按「接在上一行后面」。
  const gap = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    return y < r.top ? r.top - y : y > r.bottom ? y - r.bottom : 0;
  };
  if (gap(down) < gap(up)) {
    if (!isFolderRow(down)) return zoneOfEl(down, y);
    const z = beforeFolderZone(down);
    if (z) return z;
    return isFolderRow(up) ? intoZone(up) : zoneOfEl(up, y);
  }
  if (!isFolderRow(up)) return zoneOfEl(up, y); // 接在上一行后面 = 正好落进这个缝
  if (!isFolderRow(down)) return zoneOfEl(down, y); // 缝在目录行正下方 = 该目录子树顶部
  return intoZone(up); // 两边都是目录行：放进上面这个目录
}

function computeDropZoneAt(x: number, y: number): DropZone | null {
  const list = activeListEl();
  const hit = document.elementFromPoint(x, y) as HTMLElement | null;
  if (!hit || !list.contains(hit)) return null; // 列表外视为无效落点
  const folderEl = hit.closest(".folder-row") as HTMLElement | null;
  if (folderEl) {
    const folderId = folderEl.dataset.folderId!;
    if (state.dragPayload?.kind === "folder") {
      const r = folderEl.getBoundingClientRect();
      const ratio = (y - r.top) / r.height;
      if (ratio < 0.25) return { kind: "folder", folderId, pos: "before" };
      if (ratio > 0.75) return { kind: "folder", folderId, pos: "after" };
      return { kind: "folder", folderId, pos: "into" };
    }
    // 快捷键 / 改键 / 文本扩展落到目录行上 = 放进该目录。
    return { kind: "folder", folderId, pos: "into" };
  }
  const rowEl = hit.closest(".row") as HTMLElement | null;
  if (rowEl) {
    const rowKind = rowEl.dataset.kind as ListKind;
    const dragKind = dragRowKind();
    const ok = isMouseKind(dragKind)
      ? isMouseKind(rowKind)
      : dragKind === "touchpad"
        ? rowKind === "touchpad"
        : rowKind === dragKind;
    if (ok) {
      const r = rowEl.getBoundingClientRect();
      return {
        kind: "row",
        idx: Number(rowEl.dataset.idx),
        pos: y - r.top < r.height / 2 ? "before" : "after",
        targetKind: rowKind,
      };
    }
  }
  return nearestDropZone(list, y);
}

function showDropIndicator(zone: DropZone) {
  if (zone.kind === "root") {
    clearDropIndicators();
    activeListEl().classList.add("drop-root");
    state.dragOverEl = activeListEl();
    return;
  }
  const el = zoneEl(zone);
  if (!el) return;
  const cls =
    zone.kind === "folder"
      ? zone.pos === "into"
        ? "drop-into"
        : zone.pos === "before"
          ? "drop-before"
          : "drop-after"
      : zone.pos === "before"
        ? "drop-before"
        : "drop-after";
  setDropIndicator(el, cls);
}

// 平铺列表内换位：摘出再插回（越过源位时目标下标 -1）。快捷键/改键/文本扩展共用一套算术。
function moveWithin<T>(arr: T[], srcIdx: number, insertIdx: number) {
  const [moved] = arr.splice(srcIdx, 1);
  if (!moved) return;
  const at = insertIdx > srcIdx ? insertIdx - 1 : insertIdx;
  arr.splice(at, 0, moved);
}

function moveShortcut(srcIdx: number, targetFolder: string | null, insertIdx?: number) {
  const moved = state.cfg.shortcuts[srcIdx];
  if (!moved) return;
  moved.folder = targetFolder;
  if (insertIdx === undefined) {
    state.cfg.shortcuts.splice(srcIdx, 1);
    state.cfg.shortcuts.push(moved);
  } else {
    moveWithin(state.cfg.shortcuts, srcIdx, insertIdx);
  }
}

function moveFolder(
  srcId: string,
  targetParent: string | null,
  beforeFolderId?: string,
  pos?: "before" | "after",
) {
  const src = state.cfg.folders.find((f) => f.id === srcId);
  if (!src) return;
  src.parent = targetParent;
  state.cfg.folders = state.cfg.folders.filter((f) => f.id !== srcId);
  let at = state.cfg.folders.length;
  if (beforeFolderId) {
    const targetIdx = state.cfg.folders.findIndex((f) => f.id === beforeFolderId);
    if (targetIdx >= 0) at = pos === "before" ? targetIdx : targetIdx + 1;
  }
  state.cfg.folders.splice(at, 0, src);
}

// 目录行的「上/下沿」= 该目录所在层级的条目区（同级条目渲染在目录行之前，根级就是根）。
function folderParentOf(folderId: string): string | null {
  return state.cfg.folders.find((f) => f.id === folderId)?.parent ?? null;
}

function applyDrop(zone: DropZone) {
  if (!state.dragPayload) return;
  // 落回源行自己 = 原地不动（源行 pointer-events 让位，光标常就停在它身上）：不落盘、
  // 不重绘，免得「拿起又放下」白弹一次保存提示。
  if (zoneEl(zone) === state.dragState?.sourceEl) return;

  // 鼠标类：三类共用一条展示序列，可互相换位、放进目录或拖回根级。
  const payload = state.dragPayload;
  if (payload.kind === "gesture" || payload.kind === "hotcorner" || payload.kind === "wheeledge") {
    const srcKind = payload.kind;
    const moved = mouseItemOf(srcKind, payload.idx);
    if (!moved) return;
    let destFolder: string | null = null;
    if (zone.kind === "folder") destFolder = zone.folderId;
    else if (zone.kind === "row") {
      const target = mouseItemOf(zone.targetKind ?? srcKind, zone.idx);
      if (!target) return;
      destFolder = target.folder ?? null;
    } else if (zone.kind !== "root") return;

    // 同一类内换位仍要动数组（顺序 = 同位置 / 同方向串的优先级）；跨类型只改展示顺序。
    if (zone.kind === "row" && zone.targetKind === srcKind) {
      const target = mouseItemOf(srcKind, zone.idx);
      const arr = arrOf(srcKind);
      const from = arr.indexOf(moved);
      const ti = target ? arr.indexOf(target) : -1;
      if (from >= 0 && ti >= 0) {
        let at = zone.pos === "before" ? ti : ti + 1;
        if (at > from) at--;
        moveWithin(arr, from, at);
      }
    }
    moved.folder = destFolder;
    const seq = mouseSequence().filter((e) => e.item !== moved);
    let at = seq.length;
    if (zone.kind === "row") {
      const target = mouseItemOf(zone.targetKind ?? srcKind, zone.idx);
      const pos = seq.findIndex((e) => e.item === target);
      if (pos >= 0) at = zone.pos === "before" ? pos : pos + 1;
    } else if (zone.kind === "folder") {
      let last = -1;
      seq.forEach((e, i) => {
        if ((e.item.folder ?? null) === destFolder) last = i;
      });
      at = last >= 0 ? last + 1 : seq.length;
    }
    seq.splice(at, 0, { kind: srcKind, item: moved, rank: 0, idx: 0 });
    seq.forEach((e, i) => {
      e.item.order = i;
    });
    const movedIdx = arrOf(srcKind).indexOf(moved);
    if (state.section === KIND_SECTION[srcKind] && movedIdx >= 0) state.selected = movedIdx;
    void save();
    return;
  }

  // 触摸板手势：自成一组——可同组换位、放进目录 / 拖回根级，但不与鼠标操作跨类型换位。
  if (payload.kind === "touchpad") {
    const moved = touchpadItemOf(payload.idx);
    if (!moved) return;
    let destFolder: string | null = null;
    if (zone.kind === "folder") destFolder = zone.folderId;
    else if (zone.kind === "row") {
      if (zone.targetKind !== "touchpad") return;
      const target = touchpadItemOf(zone.idx);
      if (!target) return;
      destFolder = target.folder ?? null;
    } else if (zone.kind !== "root") return;

    const arr = state.cfg.touchpad_gestures ?? (state.cfg.touchpad_gestures = []);
    if (zone.kind === "row") {
      const target = touchpadItemOf(zone.idx);
      const from = arr.indexOf(moved);
      const ti = target ? arr.indexOf(target) : -1;
      if (from >= 0 && ti >= 0) {
        let at = zone.pos === "before" ? ti : ti + 1;
        if (at > from) at--;
        moveWithin(arr, from, at);
      }
    }
    moved.folder = destFolder;
    const seq = touchpadSequence().filter((e) => e.item !== moved);
    let at = seq.length;
    if (zone.kind === "row") {
      const target = touchpadItemOf(zone.idx);
      const pos = seq.findIndex((e) => e.item === target);
      if (pos >= 0) at = zone.pos === "before" ? pos : pos + 1;
    } else if (zone.kind === "folder") {
      let last = -1;
      seq.forEach((e, i) => {
        if ((e.item.folder ?? null) === destFolder) last = i;
      });
      at = last >= 0 ? last + 1 : seq.length;
    }
    seq.splice(at, 0, { item: moved, idx: 0 });
    seq.forEach((e, i) => {
      e.item.order = i;
    });
    const movedIdx = arr.indexOf(moved);
    if (state.section === KIND_SECTION.touchpad && movedIdx >= 0) state.selected = movedIdx;
    void save();
    return;
  }

  // 改键/文本扩展：平铺换位 + 归目录（目录行上算「放进」，缝隙按 nearestDropZone 落位）。
  if (state.dragPayload.kind === "remap" || state.dragPayload.kind === "expansion") {
    const arr: (Remap | TextExpansion)[] =
      state.dragPayload.kind === "remap" ? state.cfg.remaps : state.cfg.expansions;
    const srcIdx = state.dragPayload.idx;
    const moved = arr[srcIdx];
    if (!moved) return;
    const targetFolder =
      zone.kind === "row"
        ? (arr[zone.idx]?.folder ?? null)
        : zone.kind === "folder"
          ? zone.pos === "into"
            ? zone.folderId
            : folderParentOf(zone.folderId)
          : null;
    // 原地放下（没换目录、位置也没动）：不落盘不重绘，免得白弹一次保存提示。
    if (zone.kind === "row") {
      const at = zone.pos === "before" ? zone.idx : zone.idx + 1;
      if ((moved.folder ?? null) === targetFolder && (at > srcIdx ? at - 1 : at) === srcIdx) return;
    } else if (
      (moved.folder ?? null) === targetFolder &&
      arr.slice(srcIdx + 1).every((o) => (o.folder ?? null) !== targetFolder)
    ) {
      return; // 已是该层级最后一条 = 没动
    }
    // selected 是三类共用的下标变量：只有编辑页开着的就是这一类时才有意义去修。
    const secMatch = state.section === KIND_SECTION[state.dragPayload.kind];
    const selObj = secMatch && state.selected !== null ? arr[state.selected] : null;
    moved.folder = targetFolder;
    // 换目录时插到条目末尾（同目录内顺序只看数组相对次序，插末尾即该目录最后一条）。
    moveWithin(arr, srcIdx, zone.kind === "row" ? (zone.pos === "before" ? zone.idx : zone.idx + 1) : arr.length);
    if (secMatch) state.selected = selObj ? arr.indexOf(selObj) : null;
    void save();
    return;
  }

  const secMatch = state.section === "shortcuts";
  const selObj = secMatch && state.selected !== null ? state.cfg.shortcuts[state.selected] : null;

  if (state.dragPayload.kind === "shortcut") {
    const srcIdx = state.dragPayload.idx;
    if (!state.cfg.shortcuts[srcIdx]) return;
    if (zone.kind === "root") {
      moveShortcut(srcIdx, null);
    } else if (zone.kind === "folder") {
      // 放进 = 归该目录；上/下沿 = 该目录所在层级的条目区（根级目录的上/下沿就是根）。
      moveShortcut(srcIdx, zone.pos === "into" ? zone.folderId : folderParentOf(zone.folderId));
    } else if (zone.kind === "row") {
      const targetFolder = state.cfg.shortcuts[zone.idx]?.folder ?? null;
      moveShortcut(srcIdx, targetFolder, zone.pos === "before" ? zone.idx : zone.idx + 1);
    }
  } else {
    const srcId = (state.dragPayload as { kind: "folder"; id: string }).id;
    if (zone.kind === "root") {
      moveFolder(srcId, null);
    } else if (zone.kind === "folder") {
      if (zone.pos === "into") {
        moveFolder(srcId, zone.folderId);
      } else {
        const target = state.cfg.folders.find((f) => f.id === zone.folderId);
        moveFolder(srcId, target?.parent ?? null, zone.folderId, zone.pos);
      }
    } else if (zone.kind === "row") {
      const targetFolder = state.cfg.shortcuts[zone.idx]?.folder ?? null;
      moveFolder(srcId, targetFolder);
    }
  }

  if (secMatch) state.selected = selObj ? state.cfg.shortcuts.indexOf(selObj) : null;
  void save();
}

// 给一行绑定拖拽启动：pointerdown 记录起点，移动越过阈值后才真正进入拖拽。
function attachDragStart(li: HTMLElement, payload: DragPayload) {
  li.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return; // 仅左键
    if ((e.target as HTMLElement).closest("input, button")) return;
    // 搜索/筛选会隐藏条目，可见顺序 ≠ 配置顺序，换位落点算不准（与搜索同一处理：禁拖）。
    if (state.search.trim() || state.listFilter.enabled || state.listFilter.conflicted) return;
    state.dragState = {
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
    const target = zoneEl(zone);
    if (target) setDropIndicator(target, "drop-forbidden");
    return;
  }
  showDropIndicator(zone);
}

function onPointerMove(e: PointerEvent) {
  if (state.actionDrag) {
    if (!state.actionDrag.active) {
      if (Math.hypot(e.clientX - state.actionDrag.startX, e.clientY - state.actionDrag.startY) < 6) return;
      state.actionDrag.active = true;
      state.actionDrag.sourceEl.classList.add("dragging");
      state.actionDrag.sourceEl.style.pointerEvents = "none"; // 让 elementFromPoint 穿透源行
    }
    updateActionDropTarget(e.clientX, e.clientY);
    return;
  }
  if (!state.dragState) return;
  if (!state.dragState.active) {
    if (Math.hypot(e.clientX - state.dragState.startX, e.clientY - state.dragState.startY) < 6) return;
    state.dragState.active = true;
    state.dragPayload = state.dragState.payload;
    state.dragState.sourceEl.classList.add("dragging");
    state.dragState.sourceEl.style.pointerEvents = "none"; // 让 elementFromPoint 穿透源行
  }
  updateDropTarget(e.clientX, e.clientY);
}

function onPointerUp(e: PointerEvent) {
  if (state.actionDrag) {
    const { active, sourceEl, list, index } = state.actionDrag;
    if (active) {
      const res = actionDropAt(e.clientX, e.clientY);
      clearActionDropIndicators();
      if (res) applyActionDrop(list, index, res.target);
    } else {
      // 未拖拽：单击手柄 → 切换展开/收起（按动作本体记，禁用壳已剥掉）
      const a = unwrapAction(list[index]);
      if (collapsedActions.has(a)) collapsedActions.delete(a);
      else collapsedActions.add(a);
    }
    sourceEl.classList.remove("dragging");
    sourceEl.style.pointerEvents = "";
    state.actionDrag = null;
    if (state.draft) renderActions(state.draft);
    return;
  }
  if (!state.dragState) return;
  const { active, sourceEl } = state.dragState;
  if (active) {
    state.suppressClick = true; // 拖拽结束，抑制随后的一次 click
    const zone = computeDropZoneAt(e.clientX, e.clientY);
    clearDropIndicators();
    if (zone !== null && !wouldCycle(zone)) applyDrop(zone);
  }
  sourceEl.classList.remove("dragging");
  sourceEl.style.pointerEvents = "";
  clearDropIndicators();
  state.dragPayload = null;
  state.dragState = null;
  setTimeout(() => (state.suppressClick = false), 0);
}

// ---- 多选（Ctrl 加减 / Shift 范围）与批量启停 ----
function togglePicked(o: object) {
  if (state.picked.has(o)) state.picked.delete(o);
  else state.picked.add(o);
  renderListPane();
}

async function setPickedEnabled(v: boolean) {
  if (!state.picked.size) return;
  let n = 0;
  for (const s of state.cfg.shortcuts) if (state.picked.has(s)) { s.enabled = v; n++; }
  for (const r of state.cfg.remaps) if (state.picked.has(r)) { r.enabled = v; n++; }
  for (const e of state.cfg.expansions) if (state.picked.has(e)) { e.enabled = v; n++; }
  for (const g of state.cfg.gestures ?? []) if (state.picked.has(g)) { g.enabled = v; n++; }
  for (const g of state.cfg.touchpad_gestures ?? []) if (state.picked.has(g)) { g.enabled = v; n++; }
  for (const r of state.cfg.hot_corners ?? []) if (state.picked.has(r)) { r.enabled = v; n++; }
  for (const r of state.cfg.wheel_edges ?? []) if (state.picked.has(r)) { r.enabled = v; n++; }
  state.picked.clear();
  state.pickAnchor = null;
  const ok = await save(); // save 自带「已保存」提示；成功后盖上条数，失败保留错误提示
  if (ok) toast(trt`已${v ? tr("启用") : tr("停用")} ${n} 项`);
}

// 目录级启停：作用到该目录（含全部子目录）里的每一条。
async function setFolderEnabled(rootId: string, v: boolean) {
  const ids = collectDescendantIds(rootId);
  let n = 0;
  for (const [, list] of FOLDER_ENTRY_KINDS)
    for (const e of list())
      if (e.folder && ids.has(e.folder)) {
        e.enabled = v;
        n++;
      }
  if (!n) return;
  const ok = await save();
  if (ok) toast(trt`已${v ? tr("启用") : tr("停用")} ${n} 项`);
}

export function renderDetail() {
  const entry = isListSection(state.section);
  document.getElementById("detail-entry")!.classList.toggle("hidden", !entry);
  document.getElementById("detail-layers")!.classList.toggle("hidden", state.section !== "layers");
  document.getElementById("detail-market")!.classList.toggle("hidden", state.section !== "market");
  document.getElementById("detail-settings")!.classList.toggle("hidden", state.section !== "settings");
  document.getElementById("detail-messages")!.classList.toggle("hidden", state.section !== "messages");
  document.getElementById("detail-help")!.classList.toggle("hidden", state.section !== "help");

  if (entry) {
    const has = currentDraft() !== null;
    document.getElementById("detail-empty")!.classList.toggle("hidden", has);
    document.getElementById("detail-editor")!.classList.toggle("hidden", !has);
    // 标题行右侧那组按钮（执行/删除/撤销/保存/关闭）只在真有草稿时才有意义。
    document.getElementById("head-tools")!.classList.toggle("hidden", !has);
    if (has) syncEntry();
    else setEntryTitle();
  }
  refreshDirty();
}

// 标题行：显示名称（空则「（未命名）」），点击行内编辑——三类条目共用一个 name 字段。
// 没有草稿时只放当前类型的静态标签，不可编辑。
function setEntryTitle() {
  const title = document.getElementById("detail-title")!;
  const item = currentDraft();
  if (!item) {
    title.textContent = kindLabel(currentKind());
    title.contentEditable = "false";
    title.classList.remove("title-editable");
    title.removeAttribute("title");
    title.onkeydown = null;
    title.onblur = null;
    title.oninput = null;
    return;
  }
  title.textContent = item.name || tr("（未命名）");
  title.contentEditable = "true";
  title.classList.add("title-editable");
  title.title = tr("点击编辑名称");
  title.onkeydown = (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      (e.target as HTMLElement).blur();
    }
  };
  title.onblur = () => {
    const cur = currentDraft();
    if (!cur) return;
    const v = (title.textContent ?? "").replace(/\s+/g, " ").trim();
    cur.name = v || null;
    title.textContent = cur.name || tr("（未命名）");
  };
  // 名称是行内编辑的，没有对应 input 元素可监听：边打边同步进草稿，脏标记才跟得上。
  title.oninput = () => {
    const cur = currentDraft();
    if (!cur) return;
    cur.name = (title.textContent ?? "").replace(/\s+/g, " ").trim() || null;
    refreshDirty();
  };
}

// 把草稿渲染进编辑页（openDetail 与「撤销改动」共用，避免两条路各写一份填充逻辑）。
// 三类条目共用同一套 DOM：描述 / 层 / 条件 / 按键框只有一份，按类型显隐由 syncEntry() 管。
// 动作编排区（`#action-list`）的操作对象：**快捷键 → 快捷键草稿，鼠标操作 → 手势草稿**。
// 改键 / 文本扩展没有动作区，返回 null。
function currentActionsHost(): { actions: Action[] } | null {
  if (state.section === "shortcuts") return state.draft;
  if (state.section === "gestures") return state.gestureDraft;
  if (state.section === "touchpads") return state.touchpadDraft;
  if (state.section === "hotcorners") return state.hotcornerDraft;
  if (state.section === "wheeledges") return state.wheeledgeDraft;
  return null;
}

function fillShortcutEditor() {
  if (!state.draft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value = state.draft.description ?? "";
  fillLayerSelect(document.getElementById("edit-layer") as HTMLSelectElement, state.draft.layer ?? null, "assign");
  renderTriggers(state.draft);
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.draft?.when,
    (c) => {
      if (state.draft) state.draft.when = c;
    },
    refreshDirty,
  );
  renderActions(state.draft);
  setEntryTitle();
}

function fillRemapEditor() {
  if (!state.remapDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value = state.remapDraft.description ?? "";
  (document.getElementById("remap-from") as HTMLInputElement).value = state.remapDraft.from;
  closeKeyMenu();
  (document.getElementById("remap-to") as HTMLSelectElement).value = state.remapDraft.to;
  (document.getElementById("remap-tap") as HTMLSelectElement).value = state.remapDraft.tap ?? "";
  (document.getElementById("remap-hold") as HTMLSelectElement).value = state.remapDraft.hold ?? "";
  (document.getElementById("remap-oneshot") as HTMLSelectElement).value = state.remapDraft.oneshot ?? "";
  (document.getElementById("remap-sticky") as HTMLSelectElement).value = state.remapDraft.sticky ?? "";
  (document.getElementById("remap-tap2") as HTMLSelectElement).value = state.remapDraft.tap2 ?? "";
  (document.getElementById("remap-tap3") as HTMLSelectElement).value = state.remapDraft.tap3 ?? "";
  fillLayerSelect(document.getElementById("remap-layer") as HTMLSelectElement, state.remapDraft.layer, "assign");
  fillLayerSelect(document.getElementById("remap-hold-layer") as HTMLSelectElement, state.remapDraft.hold_layer, "hold");
  fillLayerSelect(document.getElementById("remap-lock-layer") as HTMLSelectElement, state.remapDraft.lock_layer, "hold");
  (document.getElementById("remap-timeout") as HTMLInputElement).value = String(
    state.remapDraft.tap_timeout_ms || 200,
  );
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.remapDraft?.when,
    (c) => {
      if (state.remapDraft) state.remapDraft.when = c;
    },
    refreshDirty,
  );
  syncRemapEditor();
  setEntryTitle();
}

function fillExpansionEditor() {
  if (!state.expansionDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value = state.expansionDraft.description ?? "";
  (document.getElementById("edit-exp-trigger") as HTMLInputElement).value =
    state.expansionDraft.trigger;
  (document.getElementById("edit-exp-replace") as HTMLTextAreaElement).value =
    state.expansionDraft.replace;
  setEntryTitle();
}

// 鼠标操作（手势）编辑页：方向串 + 描述 + 条件 + 动作（动作区与快捷键共用同一套编排器）。
/** 方向 token（与 core `gesture.rs` 的八方向同一口径，y 向下）。 */
type GestureDir = "R" | "DR" | "D" | "DL" | "L" | "UL" | "U" | "UR";

function gestureDirFromVec(dx: number, dy: number): GestureDir | null {
  if (dx === 0 && dy === 0) return null;
  let deg = (Math.atan2(dy, dx) * 180) / Math.PI;
  if (deg < 0) deg += 360;
  if (deg < 22.5 || deg >= 337.5) return "R";
  if (deg < 67.5) return "DR";
  if (deg < 112.5) return "D";
  if (deg < 157.5) return "DL";
  if (deg < 202.5) return "L";
  if (deg < 247.5) return "UL";
  if (deg < 292.5) return "U";
  return "UR";
}

/** 桶序号（`R` 起顺时针，45° 一格）。 */
const GESTURE_ORDER: GestureDir[] = ["R", "DR", "D", "DL", "L", "UL", "U", "UR"];

/** 两个方向相差不超过一格（≤45°）：略带抖动的同一条直线会被分到相邻两桶，要并回一段。 */
function gestureDirsAdjacent(a: GestureDir, b: GestureDir): boolean {
  const d = Math.abs(GESTURE_ORDER.indexOf(a) - GESTURE_ORDER.indexOf(b));
  return Math.min(d, 8 - d) <= 1;
}

/** 折线简化（RDP，与 core `simplify` 同一口径）：手抖 / 轻微弧线压成主干，真拐点保留。 */
function simplifyGesture(
  points: [number, number][],
  eps: number,
): [number, number][] {
  if (points.length <= 2) return points.slice();
  const first = points[0];
  const last = points[points.length - 1];
  const dx = last[0] - first[0];
  const dy = last[1] - first[1];
  const len = Math.hypot(dx, dy);
  let idx = 0;
  let max = 0;
  for (let i = 1; i < points.length - 1; i++) {
    const [px, py] = points[i];
    const d =
      len === 0
        ? Math.hypot(px - first[0], py - first[1])
        : Math.abs((dx * (first[1] - py) - (first[0] - px) * dy) / len);
    if (d > max) {
      max = d;
      idx = i;
    }
  }
  if (max <= eps) return [first, last];
  const left = simplifyGesture(points.slice(0, idx + 1), eps);
  const right = simplifyGesture(points.slice(idx), eps);
  left.pop();
  return [...left, ...right];
}

/**
 * 把画板上的轨迹转成方向串（规范写法：单 token 直接写，多段用逗号分隔）。
 * `D,R` = 先下再右，`DR` = 右下斜向——两者是不同的手势。
 */
function patternFromPoints(
  points: [number, number][],
  minSegment = state.cfg.settings.gesture_min_segment ?? 16,
): string {
  if (points.length < 2) return "";
  const min = Math.max(1, minSegment);
  const pts = simplifyGesture(points, Math.max(2, min / 4));
  // 每段先按位移向量累积；相邻 ≤45° 的段合并（向量相加，方向取平均）。
  const runs: [number, number][] = [];
  let anchor = pts[0];
  for (const p of pts.slice(1)) {
    const dx = p[0] - anchor[0];
    const dy = p[1] - anchor[1];
    // 还没走满一段：位移留在锚点里，攒到下一段一起判方向。
    if (Math.hypot(dx, dy) < min) continue;
    const cur = gestureDirFromVec(dx, dy);
    const last = runs[runs.length - 1];
    const prevDir = last ? gestureDirFromVec(last[0], last[1]) : null;
    if (last && cur && prevDir && gestureDirsAdjacent(prevDir, cur)) {
      last[0] += dx;
      last[1] += dy;
    } else {
      runs.push([dx, dy]);
    }
    anchor = p;
  }
  const tokens: GestureDir[] = [];
  for (const [dx, dy] of runs) {
    const dir = gestureDirFromVec(dx, dy);
    if (dir && tokens[tokens.length - 1] !== dir) tokens.push(dir);
  }
  return tokens.join(",");
}

/** 手势录入画板：画 → 松手翻译成方向串 → 确认后回填。 */
function openGesturePad(onDone: (pattern: string) => void) {
  const mask = el("div", "modal-mask gesture-pad-mask");
  const modal = el("div", "gesture-pad");
  const title = el("h3", undefined, tr("画一个手势"));
  const hint = el(
    "p",
    "hint",
    tr("按住鼠标左键在画板上画，松开后自动转成方向串；可重画后再确认。"),
  );
  const canvas = document.createElement("canvas");
  canvas.className = "gesture-pad-canvas";
  canvas.width = 640;
  canvas.height = 360;
  const result = el("div", "gesture-pad-result", tr("方向串：—"));
  let points: [number, number][] = [];
  let drawing = false;
  let pattern = "";

  const repaint = () => {
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (points.length < 2) return;
    ctx.strokeStyle = getComputedStyle(document.documentElement)
      .getPropertyValue("--accent")
      .trim() || "#4c9aff";
    ctx.lineWidth = 5;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(points[0][0], points[0][1]);
    for (const p of points.slice(1)) ctx.lineTo(p[0], p[1]);
    ctx.stroke();
  };

  const local = (e: PointerEvent): [number, number] => {
    const r = canvas.getBoundingClientRect();
    return [e.clientX - r.left, e.clientY - r.top];
  };

  canvas.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    drawing = true;
    pattern = "";
    points = [local(e)];
    canvas.setPointerCapture(e.pointerId);
    result.textContent = tr("方向串：—");
    repaint();
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!drawing) return;
    points.push(local(e));
    repaint();
  });
  const finish = () => {
    if (!drawing) return;
    drawing = false;
    pattern = patternFromPoints(points);
    result.textContent = pattern
      ? trt`方向串：${pattern}`
      : tr("没画出方向：再画长一点");
  };
  canvas.addEventListener("pointerup", finish);
  canvas.addEventListener("pointercancel", finish);

  const actions = el("div", "gesture-pad-actions");
  const clearBtn = el("button", "ghost", tr("重画"));
  clearBtn.addEventListener("click", () => {
    points = [];
    pattern = "";
    result.textContent = tr("方向串：—");
    repaint();
  });
  const cancelBtn = el("button", "ghost", tr("取消"));
  const okBtn = el("button", "primary", tr("确认录入"));
  okBtn.setAttribute("disabled", "true");
  const syncOk = () => {
    if (pattern) okBtn.removeAttribute("disabled");
    else okBtn.setAttribute("disabled", "true");
  };
  // pointerup 后同步按钮状态（画的过程中禁用）。
  canvas.addEventListener("pointerup", () => window.setTimeout(syncOk, 0));
  const close = () => mask.remove();
  cancelBtn.addEventListener("click", close);
  okBtn.addEventListener("click", () => {
    if (!pattern) return;
    close();
    onDone(pattern);
  });
  actions.append(clearBtn, cancelBtn, okBtn);
  modal.append(title, hint, canvas, result, actions);
  mask.append(modal);
  document.body.append(mask);
}

// ---- 触摸板手势：触发键 + 「从触摸板录入」 ----
// 触发键是独立的 `TouchpadGesture.trigger`（必填、只能键盘键）。录制由壳层临时开采集：
// 按触发键 → 在触摸板上画一笔 → 松键后壳层把方向串 + 形状模板推回来，**不执行动作**。
type TrackpadRecordPayload = {
  pattern: string;
  template: [number, number][] | null;
  points: number;
  elapsed_ms: number;
};

/** 正在「录入」时接收壳层回传的回调（`null` = 没在录）。 */
let trackpadRecordSink: ((p: TrackpadRecordPayload) => void) | null = null;

/** 触发键下拉：只有键盘键（鼠标键只走右键手势那条路）。 */
function fillTouchpadTriggerSelect() {
  const sel = document.getElementById("edit-touchpad-trigger") as HTMLSelectElement | null;
  if (!sel || sel.options.length > 0) return;
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = tr("选择触发键…");
  sel.append(placeholder);
  for (const k of KEY_OPTIONS) {
    if (k.startsWith("Mouse") || k.startsWith("Wheel")) continue;
    const o = document.createElement("option");
    o.value = k;
    o.textContent = k;
    sel.append(o);
  }
}

/** 触发键 / 模板变了之后，把「录入」「清除模板」与提示对齐。 */
function syncTouchpadUi() {
  const key = state.touchpadDraft?.trigger ?? "";
  document.getElementById("edit-touchpad-capture")?.classList.toggle("hidden", !key);
  document
    .getElementById("edit-touchpad-drop-template")
    ?.classList.toggle("hidden", !state.touchpadDraft?.template);
  const sel = document.getElementById("edit-touchpad-trigger") as HTMLSelectElement | null;
  if (sel) {
    sel.title = key
      ? trt`按住 ${key} 在触摸板上画一笔（触摸板手势）`
      : tr("先选一个触发键，再在触摸板上录入");
  }
}

/** 「录入」：开一个只回填编辑器的采集会话（开闸 / 识别 / 收尾都在壳层）。 */
function openTrackpadRecord(key: string) {
  const mask = el("div", "modal-mask gesture-pad-mask");
  const modal = el("div", "gesture-pad");
  const title = el("h3", "", tr("从触摸板录入"));
  const hint = el(
    "div",
    "unit-hint",
    trt`按住 ${key}，在触摸板上画一笔，松手后自动填入方向串与形状模板。Esc 作废这一笔。`,
  );
  const status = el("div", "gesture-pad-result", tr("等待按键…"));
  const actions = el("div", "gesture-pad-actions");
  const done = el("button", "ghost", tr("完成")) as HTMLButtonElement;
  done.type = "button";
  actions.append(done);
  modal.append(title, hint, status, actions);
  mask.append(modal);
  const onKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    // 采集中的 Esc 是「作废这一笔」，已被钩子吞掉、界面收不到；这里只负责关窗。
    e.preventDefault();
    stop();
  };
  function stop() {
    if (activeCaptureCleanup === stop) activeCaptureCleanup = null;
    trackpadRecordSink = null;
    window.removeEventListener("keydown", onKey, true);
    mask.remove();
    void invoke("stop_trackpad_record").catch(() => {});
  }
  // 窗口失焦 / 收进托盘 = 这次录入作废：不清会话的话触发键会一直被钩子吞掉，
  // 而面板已经看不见了（与录入暂停同一口径，见 `stopCapture`）。
  activeCaptureCleanup = stop;
  trackpadRecordSink = (p) => {
    if (!state.touchpadDraft) return;
    if (!p.pattern) {
      status.textContent = tr("这一笔没识别出来（太短 / 涂鸦 / 已作废），再画一笔");
      return;
    }
    state.touchpadDraft.pattern = p.pattern;
    state.touchpadDraft.template = p.template ?? null;
    state.touchpadDraft.trigger = key;
    (document.getElementById("edit-touchpad-pattern") as HTMLInputElement).value = p.pattern;
    renderActions(state.touchpadDraft, p.pattern);
    syncTouchpadUi();
    refreshDirty();
    status.textContent = trt`已录入 ${p.pattern}（${p.points} 个点）`;
    toast(trt`已录入方向串 ${p.pattern}，记得保存`);
  };
  done.addEventListener("click", stop);
  mask.addEventListener("click", (e) => {
    if (e.target === mask) stop();
  });
  window.addEventListener("keydown", onKey, true);
  document.body.append(mask);
  void invoke("start_trackpad_record", { key })
    .catch((e) => {
      // 平台不支持 / 输入层没起来：把后端的说法原样告诉用户，别留一个「等按键」的空壳。
      stop();
      toast(trt`没能开始录制：${String(e)}`);
    });
}

function fillGestureEditor() {
  if (!state.gestureDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value =
    state.gestureDraft.description ?? "";
  (document.getElementById("edit-gesture-pattern") as HTMLInputElement).value =
    state.gestureDraft.pattern;
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.gestureDraft?.when,
    (c) => {
      if (state.gestureDraft) state.gestureDraft.when = c;
    },
    refreshDirty,
  );
  renderActions(state.gestureDraft, state.gestureDraft.pattern || tr("未设方向串"));
  setEntryTitle();
}

function fillTouchpadEditor() {
  if (!state.touchpadDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value =
    state.touchpadDraft.description ?? "";
  (document.getElementById("edit-touchpad-pattern") as HTMLInputElement).value =
    state.touchpadDraft.pattern;
  const triggerSel = document.getElementById("edit-touchpad-trigger") as HTMLSelectElement | null;
  if (triggerSel) {
    // 手改配置里塞了鼠标键 / 未知键名时，下拉没有这一项 → 回落到空值（显示「选择触发键…」），
    // 但**不动草稿**：保存时后端清洗会照原样报错，别在界面上悄悄改用户配置。
    triggerSel.value = state.touchpadDraft.trigger ?? "";
  }
  syncTouchpadUi();
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.touchpadDraft?.when,
    (c) => {
      if (state.touchpadDraft) state.touchpadDraft.when = c;
    },
    refreshDirty,
  );
  renderActions(
    state.touchpadDraft,
    state.touchpadDraft.pattern || (state.touchpadDraft.template ? tr("仅形状") : tr("未设方向串")),
  );
  setEntryTitle();
}

/** 模拟显示器选点：四角 / 四边各一个可点位置，选中即写回草稿。 */
function spotPicker(
  kind: "hotcorner" | "wheeledge",
  current: HotCornerSpot | WheelEdgeSpot,
  pick: (spot: HotCornerSpot | WheelEdgeSpot) => void,
): HTMLElement {
  const box = el("div", "monitor-pick");
  const screen = el("div", "monitor-screen");
  const points =
    kind === "hotcorner"
      ? HOT_CORNER_SPOTS.map((s) => ({ key: s.spot, label: s.label, cls: `corner-${s.spot}` }))
      : WHEEL_EDGE_SPOTS.map((s) => ({ key: s.edge, label: s.label, cls: `edge-${s.edge}` }));
  for (const p of points) {
    const b = el("button", `monitor-point ${p.cls}` + (p.key === current ? " active" : ""), p.label) as HTMLButtonElement;
    b.type = "button";
    b.title = trt`放到${p.label}`;
    b.addEventListener("click", () => pick(p.key));
    screen.append(b);
  }
  box.append(
    screen,
    el("div", "unit-hint", tr("同一位置可建多条，按列表顺序取第一条启用的。")),
  );
  return box;
}

/** 位置选择：平时只占一行「位置：左上角」，点开才浮出显示器（选完自动收起）。 */
function spotField(
  kind: "hotcorner" | "wheeledge",
  current: HotCornerSpot | WheelEdgeSpot,
  pick: (spot: HotCornerSpot | WheelEdgeSpot) => void,
): HTMLElement {
  const box = el("details", "spot-field") as HTMLDetailsElement;
  const sum = el("summary", "spot-trigger", trt`位置：${spotLabel(current)}`);
  const pop = el("div", "spot-pop");
  pop.append(
    spotPicker(kind, current, (s) => {
      box.open = false;
      pick(s);
    }),
  );
  box.append(sum, pop);
  return box;
}

function fillHotCornerEditor() {
  if (!state.hotcornerDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value =
    state.hotcornerDraft.description ?? "";
  fillLayerSelect(
    document.getElementById("hotcorner-layer") as HTMLSelectElement,
    state.hotcornerDraft.layer ?? null,
    "assign",
  );
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.hotcornerDraft?.when,
    (c) => {
      if (state.hotcornerDraft) state.hotcornerDraft.when = c;
    },
    refreshDirty,
  );
  // 防误触停留：0 = 碰角立即触发；越界值钳进 0~3000（与壳层同一上限）。
  const dwell = document.getElementById("hotcorner-dwell") as HTMLInputElement;
  dwell.value = String(state.hotcornerDraft.dwell_ms ?? 0);
  dwell.oninput = () => {
    if (!state.hotcornerDraft) return;
    const n = Math.round(Number(dwell.value) || 0);
    state.hotcornerDraft.dwell_ms = Math.min(3000, Math.max(0, n));
    refreshDirty();
  };
  dwell.onblur = () => {
    if (!state.hotcornerDraft) return;
    dwell.value = String(state.hotcornerDraft.dwell_ms ?? 0);
  };
  const host = document.getElementById("key-hotcorner")!;
  host.replaceChildren(
    spotField("hotcorner", state.hotcornerDraft.spot, (spot) => {
      if (!state.hotcornerDraft) return;
      state.hotcornerDraft.spot = spot as HotCornerSpot;
      fillHotCornerEditor();
      refreshDirty();
    }),
  );
  renderActions(state.hotcornerDraft, spotLabel(state.hotcornerDraft.spot));
  setEntryTitle();
}

function fillWheelEdgeEditor() {
  if (!state.wheeledgeDraft) return;
  (document.getElementById("edit-desc") as HTMLTextAreaElement).value =
    state.wheeledgeDraft.description ?? "";
  fillLayerSelect(
    document.getElementById("wheeledge-layer") as HTMLSelectElement,
    state.wheeledgeDraft.layer ?? null,
    "assign",
  );
  renderWhenEditor(
    document.getElementById("edit-when")!,
    () => state.wheeledgeDraft?.when,
    (c) => {
      if (state.wheeledgeDraft) state.wheeledgeDraft.when = c;
    },
    refreshDirty,
  );
  const host = document.getElementById("key-wheeledge")!;
  host.replaceChildren(
    spotField("wheeledge", state.wheeledgeDraft.edge, (edge) => {
      if (!state.wheeledgeDraft) return;
      state.wheeledgeDraft.edge = edge as WheelEdgeSpot;
      fillWheelEdgeEditor();
      refreshDirty();
    }),
  );
  renderActions(state.wheeledgeDraft, spotLabel(state.wheeledgeDraft.edge));
  setEntryTitle();
}

// ---- 统一编辑页的模式 / 页签同步 ----
// 模式按钮（快捷键 / 改键 / 文本扩展）、按键框里显示哪一套、四个页签的可用性与当前页，
// 都在这里对齐当前类型。openDetail → renderDetail 每次都会走一遍。
function syncEntry() {
  const kind = currentKind();
  // 执行按钮只对「快捷键」有意义：改键 / 文本扩展 / 鼠标操作没有动作串可跑。
  document.getElementById("entry-run")!.classList.toggle("hidden", kind !== "shortcut");
  const label = document.getElementById("mode-label");
  if (label) label.textContent = kindLabel(kind);
  for (const k of [
    "shortcut",
    "remap",
    "expansion",
    "gesture",
    "touchpad",
    "hotcorner",
    "wheeledge",
  ] as ListKind[]) {
    const item = document.querySelector<HTMLElement>(`#mode-menu .mode-item[data-kind="${k}"]`);
    if (item) {
      item.classList.toggle("active", k === kind);
      // 鼠标类 / 触摸板各有自己的开关：关着时这些项不出现（入口隐藏，配置留着）。
      if (k === "gesture" || k === "hotcorner" || k === "wheeledge")
        item.classList.toggle("hidden", !mouseFeaturesEnabled());
      else if (k === "touchpad")
        item.classList.toggle("hidden", !touchpadFeaturesEnabled());
      // 已保存的条目不能改类型（改类型 = 换一个数组重存，草稿下标 / 删除路径都会错位）；
      // 新增态可以随意切，名称与描述由 switchDraftKind 带过去。
      item.classList.toggle("disabled", k !== kind && !state.draftNew);
      item.title =
        k === kind
          ? trt`${kindLabel(k)}（当前模式）`
          : state.draftNew
            ? trt`切到${kindLabel(k)}`
            : trt`${kindLabel(k)}（已保存的条目不能改类型）`;
    }
    document.getElementById(`key-${k}`)?.classList.toggle("hidden", k !== kind);
    document.getElementById(`layer-${k}`)?.classList.toggle("hidden", k !== kind);
  }
  // 动作区：快捷键 / 鼠标类触发 / 触摸板手势共用 `#work-shortcut`；改键 / 文本扩展各用自己的。
  document.getElementById("work-shortcut")!.classList.toggle(
    "hidden",
    kind !== "shortcut" &&
      kind !== "gesture" &&
      kind !== "touchpad" &&
      kind !== "hotcorner" &&
      kind !== "wheeledge",
  );
  document.getElementById("work-remap")!.classList.toggle("hidden", kind !== "remap");
  document.getElementById("work-expansion")!.classList.toggle("hidden", kind !== "expansion");
  closeModeMenu(); // 切条目 / 重渲染时别让上一次开的菜单还挂在那
  // 文本扩展没有「层」；手势 / 触摸板手势也没有 `layer` 字段（与鼠标操作同一口径），
  // 别显示一个空页签。快捷键 / 改键 / 角动作 / 边缘滚动能归属层。
  const hasLayer =
    kind !== "expansion" && kind !== "gesture" && kind !== "touchpad";
  const hasWhen = kind !== "expansion";
  const hasDetail = true;
  document.getElementById("tab-layer")!.classList.toggle("hidden", !hasLayer);
  document.getElementById("tab-when")!.classList.toggle("hidden", !hasWhen);
  // 触发角的「条件」里还带防误触停留，页签文字据此区分（其他类型仍是「条件」）。
  document.getElementById("tab-when")!.textContent =
    kind === "hotcorner" ? tr("条件（防误触）") : tr("条件");
  document.getElementById("tab-detail")!.classList.toggle("hidden", !hasDetail);
  document.getElementById("tab-work")!.textContent =
    kind === "remap" ? tr("操作") : kind === "expansion" ? tr("展开文本") : tr("动作");
  // 防误触的停留输入只属于触发角，别的类型在「条件」页签里不出现。
  document.getElementById("hotcorner-dwell-row")!.classList.toggle("hidden", kind !== "hotcorner");
  if ((state.entryTab === "layer" && !hasLayer) || (state.entryTab === "when" && !hasWhen)) state.entryTab = "detail";
  if (state.entryTab === "detail" && !hasDetail) state.entryTab = "work";
  applyEntryTab();
}

function applyEntryTab() {
  document.querySelectorAll<HTMLElement>("#entry-tabs .entry-tab").forEach((b) =>
    b.classList.toggle("active", b.dataset.tab === state.entryTab),
  );
  document.querySelectorAll<HTMLElement>("#detail-editor .entry-pane").forEach((p) =>
    p.classList.toggle("hidden", p.dataset.pane !== state.entryTab),
  );
}

// 新增态切换模式：换分区重开一份新草稿，把已填的名称 / 描述带过去，其余字段按新类型初始化。
// 已保存的条目切不动（syncEntry 里按钮是 disabled 的，这里再兜一道）。
function switchDraftKind(kind: ListKind) {
  if (kind === currentKind()) return;
  if (!state.draftNew) {
    toast(tr("已保存的条目不能改类型"));
    return;
  }
  stopCapture();
  void stopRecIfAny(); // 录制中切模式得先停，不然后台录的音轨挂在已丢弃的草稿上
  const prev = currentDraft();
  const seed = prev
    ? {
        name: prev.name ?? null,
        description: "description" in prev ? (prev.description ?? null) : null,
      }
    : undefined;
  const folderId = (prev as { folder?: string | null } | null)?.folder ?? null;
  discardDraft();
  state.selected = null;
  state.section = KIND_SECTION[kind];
  syncRailTab(state.section);
  openDetail(-1, true, folderId, seed);
  void refreshConflicts().then(renderConflictBubble);
}

// 标题行的执行按钮（只有快捷键会出现）：把当前草稿的动作串当一次触发跑掉（跑草稿 =
// 保存前就能验证）。改键 / 文本扩展没有动作串，按钮本身不出现（syncEntry 里隐藏）。
async function runCurrentEntry() {
  if (!state.draft) return;
  await runActions(state.draft.actions, state.draft.name?.trim() || shortcutLabel(state.draft));
}

// 行内「执行」按钮与编辑页的「执行」共用这一支：直接把动作串丢给壳层跑一遍。
async function runActions(actions: Action[], name: string) {
  if (!actions.length) {
    toast(tr("还没有动作：先在「动作」页签添加"));
    return;
  }
  try {
    await invoke("run_entry", { actions, name });
    toast(tr("已开始执行（结果见「消息」页）"));
  } catch (err) {
    toast(trt`执行失败: ${err}`);
  }
}

// 标题行的删除按钮：三类条目共用一条路径（与列表里的删除确认同一套）。
function deleteCurrentEntry() {
  const kind = currentKind();
  if (kind === "shortcut") {
    if (!state.draft) return;
    if (state.draftNew) return discardNewDraft();
    const d = state.draft;
    void deleteDetailEntry(tr("快捷键"), shortcutLabel(d), () => {
      if (state.selected !== null) state.cfg.shortcuts.splice(state.selected, 1);
    });
  } else if (kind === "remap") {
    if (!state.remapDraft) return;
    if (state.draftNew) return discardNewDraft();
    const r = state.remapDraft;
    void deleteDetailEntry(tr("改键"), r.name?.trim() || `${r.from} → ${remapSummary(r)}`, () => {
      if (state.selected !== null) state.cfg.remaps.splice(state.selected, 1);
    });
  } else if (kind === "expansion") {
    if (!state.expansionDraft) return;
    if (state.draftNew) return discardNewDraft();
    const e = state.expansionDraft;
    void deleteDetailEntry(tr("文本扩展"), e.name?.trim() || e.trigger || tr("未命名"), () => {
      if (state.selected !== null) state.cfg.expansions.splice(state.selected, 1);
    });
  } else if (kind === "gesture") {
    if (!state.gestureDraft) return;
    if (state.draftNew) return discardNewDraft();
    const g = state.gestureDraft;
    void deleteDetailEntry(tr("鼠标操作"), g.name?.trim() || g.pattern || tr("未命名"), () => {
      if (state.selected !== null) (state.cfg.gestures ?? []).splice(state.selected, 1);
    });
  } else if (kind === "touchpad") {
    if (!state.touchpadDraft) return;
    if (state.draftNew) return discardNewDraft();
    const g = state.touchpadDraft;
    void deleteDetailEntry(tr("触摸板手势"), g.name?.trim() || g.pattern || tr("未命名"), () => {
      if (state.selected !== null) (state.cfg.touchpad_gestures ?? []).splice(state.selected, 1);
    });
  } else if (kind === "hotcorner") {
    if (!state.hotcornerDraft) return;
    if (state.draftNew) return discardNewDraft();
    const r = state.hotcornerDraft;
    void deleteDetailEntry(tr("角动作"), r.name?.trim() || spotLabel(r.spot), () => {
      if (state.selected !== null) (state.cfg.hot_corners ?? []).splice(state.selected, 1);
    });
  } else {
    if (!state.wheeledgeDraft) return;
    if (state.draftNew) return discardNewDraft();
    const r = state.wheeledgeDraft;
    void deleteDetailEntry(tr("边缘滚动"), r.name?.trim() || spotLabel(r.edge), () => {
      if (state.selected !== null) (state.cfg.wheel_edges ?? []).splice(state.selected, 1);
    });
  }
}

// 改键「原键」的可过滤下拉：点击 / 输入都重画列表，点选项落值，blur / Esc 收起。
function renderKeyMenu(filter: string) {
  const menu = document.getElementById("key-menu")!;
  const q = filter.trim().toLowerCase();
  const keys = KEY_OPTIONS.filter((k) => !q || k.toLowerCase().includes(q));
  menu.replaceChildren();
  for (const k of keys) {
    const item = el("li", "key-item", k);
    // 用 mousedown 而不是 click：先 preventDefault，输入框不丢焦点（也就不会触发 blur 收起）。
    item.addEventListener("mousedown", (ev) => {
      ev.preventDefault();
      pickRemapKey(k);
    });
    menu.append(item);
  }
  menu.classList.toggle("hidden", keys.length === 0);
}

function pickRemapKey(k: string) {
  const input = document.getElementById("remap-from") as HTMLInputElement;
  input.value = k;
  showFieldError(input, "");
  if (state.remapDraft) state.remapDraft.from = k;
  closeKeyMenu();
  refreshDirty();
}

function closeKeyMenu() {
  document.getElementById("key-menu")?.classList.add("hidden");
}

function closeModeMenu() {
  document.getElementById("mode-menu")?.classList.add("hidden");
}

// 打开编辑页。新增快捷键可带 folderId（目录行的「＋」= 在该目录里直接新增）。
// seed = 新增态从别的模式切过来时带过来的名称/描述（见 switchDraftKind）。
// 三类草稿互斥：换类型打开前先把上一份清掉，免得两份草稿同时挂着。
export function openDetail(
  i: number,
  isNew = false,
  folderId: string | null = null,
  seed?: { name?: string | null; description?: string | null; pattern?: string | null },
) {
  state.draft = null;
  state.remapDraft = null;
  state.expansionDraft = null;
  state.gestureDraft = null;
  state.touchpadDraft = null;
  state.hotcornerDraft = null;
  state.wheeledgeDraft = null;
  state.selected = i;
  state.draftNew = isNew;
  state.draftBaseline = null; // 填充期间先不标脏，填完统一标定基线
  state.entryTab = "work"; // 打开条目默认停在「动作 / 操作 / 展开文本」页签
  if (state.section === "shortcuts") {
    state.draft = isNew
      ? { name: "", description: "", folder: folderId, layer: null, when: null, triggers: [], actions: [newAction("text")], enabled: true }
      : deepClone(state.cfg.shortcuts[i]);
    // 打开已创建的、含多个动作的快捷键时，动作默认收起；新建/新增的动作保持展开。
    if (!isNew && state.draft.actions.length > 1) collapseAllActions(state.draft.actions);
    if (isNew && seed) {
      state.draft.name = seed.name ?? "";
      state.draft.description = seed.description ?? "";
    }
    fillShortcutEditor();
  } else if (state.section === "remaps") {
    state.remapDraft = isNew
      ? { name: null, description: null, folder: folderId, from: "", to: "Ctrl", tap: null, hold: null, layer: null, when: null, hold_layer: null, lock_layer: null, tap_timeout_ms: 200, oneshot: null, sticky: null, tap2: null, tap3: null, enabled: true }
      : deepClone(state.cfg.remaps[i]);
    if (isNew && seed) {
      state.remapDraft.name = seed.name ?? null;
      state.remapDraft.description = seed.description ?? null;
    }
    fillRemapEditor();
  } else if (state.section === "expansions") {
    state.expansionDraft = isNew
      ? { name: null, description: null, folder: folderId, trigger: "", replace: "", enabled: true }
      : deepClone(state.cfg.expansions[i]);
    if (isNew && seed) {
      state.expansionDraft.name = seed.name ?? null;
      state.expansionDraft.description = seed.description ?? null;
    }
    fillExpansionEditor();
  } else if (state.section === "gestures") {
    state.gestureDraft = isNew
      ? {
          name: null,
          description: null,
          folder: folderId,
          pattern: "D",
          actions: [newAction("text")],
          enabled: true,
        }
      : deepClone((state.cfg.gestures ?? [])[i]);
    if (isNew && seed) {
      state.gestureDraft.name = seed.name ?? null;
      if (seed.pattern) state.gestureDraft.pattern = seed.pattern;
    }
    fillGestureEditor();
  } else if (state.section === "touchpads") {
    state.touchpadDraft = isNew
      ? {
          name: null,
          description: null,
          folder: folderId,
          trigger: null,
          pattern: "D",
          template: null,
          actions: [newAction("text")],
          enabled: true,
        }
      : deepClone((state.cfg.touchpad_gestures ?? [])[i]);
    if (isNew && seed) {
      state.touchpadDraft.name = seed.name ?? null;
      if (seed.pattern) state.touchpadDraft.pattern = seed.pattern;
    }
    fillTouchpadEditor();
  } else if (state.section === "hotcorners") {
    state.hotcornerDraft = isNew
      ? {
          spot: "top_left",
          name: null,
          description: null,
          layer: null,
          dwell_ms: 0,
          when: null,
          actions: [newAction("text")],
          enabled: true,
        }
      : deepClone((state.cfg.hot_corners ?? [])[i]);
    if (isNew && seed) state.hotcornerDraft.name = seed.name ?? null;
    fillHotCornerEditor();
  } else if (state.section === "wheeledges") {
    state.wheeledgeDraft = isNew
      ? {
          edge: "top",
          name: null,
          description: null,
          layer: null,
          when: null,
          actions: [newAction("text")],
          enabled: true,
        }
      : deepClone((state.cfg.wheel_edges ?? [])[i]);
    if (isNew && seed) state.wheeledgeDraft.name = seed.name ?? null;
    fillWheelEdgeEditor();
  }
  markDraftBaseline();
  renderListPane();
  renderDetail();
}

function closeDetail() {
  stopCapture();
  void stopRecIfAny();
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  render();
}

function discardDraft() {
  // 草稿隔离：编辑只改 draft/remapDraft/expansionDraft/gestureDraft，取消/关闭/切换直接丢弃，cfg 不受影响。
  state.draft = null;
  state.remapDraft = null;
  state.expansionDraft = null;
  state.gestureDraft = null;
  state.touchpadDraft = null;
  state.hotcornerDraft = null;
  state.wheeledgeDraft = null;
  state.draftBaseline = null;
  refreshDirty();
}

// ---- 编辑页保存 ----
// 三个编辑页的「保存」按钮与未保存守卫的「保存并离开」共用这三支。返回值 = 是否真的落盘。

// 快捷键：保存后留在编辑页（草稿继续可改）；空的新条目会被 save() 清洗剔除，那才回落到关闭编辑页。
async function saveShortcutDetail(): Promise<boolean> {
  if (state.section !== "shortcuts" || !state.draft) return false;
  await stopRecIfAny();
  state.draft.description = domValue("edit-desc") || null;
  state.draft.layer = domValue("edit-layer") || null;
  // 落盘的是草稿副本：草稿与 cfg 从此各持一份，之后继续在编辑页改动就不会悄悄改到内存里的
  // cfg（冲突检测 / 列表读的都是 cfg，「保存了但没保存」的样子很难查）。
  const item = deepClone(state.draft);
  if (state.draftNew) {
    state.cfg.shortcuts.unshift(item);
  } else if (state.selected !== null && state.cfg.shortcuts[state.selected]) {
    // 「启用」只有列表行的开关能改（编辑页没这个控件），别拿草稿里的旧值把用户刚点的开关盖回去。
    item.enabled = state.cfg.shortcuts[state.selected].enabled;
    state.cfg.shortcuts[state.selected] = item;
  }
  state.draftNew = false;
  const ok = await save();
  state.selected = state.cfg.shortcuts.indexOf(item);
  if (state.selected < 0) state.draft = null; // 空条目被清洗剔除 → 关闭编辑页
  // 保存成功才刷新基线；失败（或草稿已被剔除）保持原样，别让用户以为已经落盘。
  if (ok) state.draftBaseline = draftSignature();
  else if (!state.draft) state.draftBaseline = null;
  render();
  if (ok) flashRow(state.selected ?? -1, "shortcut");
  return ok;
}

async function saveRemapDetail(): Promise<boolean> {
  if (state.section !== "remaps" || !state.remapDraft) return false;
  state.remapDraft.description = domValue("edit-desc") || null;
  state.remapDraft.from = domValue("remap-from").trim();
  // 原键是必填：留空（新增时的默认）会被后端 sanitize 整条丢掉（toast 只报「原键无法解析」，
  // 用户以为存了其实条目没了）。保存前拦住并把输入框标红。别名（Escape 等）交给后端解析。
  if (!state.remapDraft.from) {
    const input = document.getElementById("remap-from") as HTMLInputElement;
    showFieldError(input, tr("请选择原键"));
    input.focus();
    toast(tr("请先选择原键"));
    return false;
  }
  state.remapDraft.tap = domValue("remap-tap") || null;
  state.remapDraft.hold = domValue("remap-hold") || null;
  state.remapDraft.oneshot = domValue("remap-oneshot") || null;
  state.remapDraft.sticky = domValue("remap-sticky") || null;
  state.remapDraft.tap2 = domValue("remap-tap2") || null;
  state.remapDraft.tap3 = domValue("remap-tap3") || null;
  state.remapDraft.layer = domValue("remap-layer") || null;
  state.remapDraft.hold_layer = domValue("remap-hold-layer") || null;
  state.remapDraft.lock_layer = domValue("remap-lock-layer") || null;
  state.remapDraft.tap_timeout_ms = intIn(domValue("remap-timeout"), 50, 200, 2000);
  // 任一非普通改键形态（tap-hold/切层/单次/粘滞/连击）→ 清空「改为」；否则用「改为」。
  state.remapDraft.to = state.remapDraft.tap ||
    state.remapDraft.hold ||
    state.remapDraft.hold_layer ||
    state.remapDraft.lock_layer ||
    state.remapDraft.oneshot ||
    state.remapDraft.sticky ||
    state.remapDraft.tap2 ||
    state.remapDraft.tap3
    ? ""
    : domValue("remap-to");
  // 长按进入层（momentary）与长按锁定层（切换式）互斥：两者都选了保留切换式（层内放和弦/
  // 序列只能用切换式，见前端的提示文案），避免后端归一化时静默丢一个让用户困惑。
  if (state.remapDraft.hold_layer && state.remapDraft.lock_layer) {
    state.remapDraft.hold_layer = null;
    (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
    toast(tr("同时选了「长按进入层」与「长按锁定层」，保留「长按锁定层」"));
  }
  // 没有任何输出（改为/短按/长按/长按进入层/长按锁定层/单次/粘滞/双击/三击全空）→ 后端会当
  // 无效条目丢掉。这里直接拦住并说清楚，别让用户以为配好了（「长按进入层」没选中层就是这样丢的）。
  // 返回 false 让「保存并离开」也停下来，改键还留在编辑页。
  if (
    !state.remapDraft.tap &&
    !state.remapDraft.hold &&
    !state.remapDraft.hold_layer &&
    !state.remapDraft.lock_layer &&
    !state.remapDraft.oneshot &&
    !state.remapDraft.sticky &&
    !state.remapDraft.tap2 &&
    !state.remapDraft.tap3 &&
    !state.remapDraft.to
  ) {
    toast(tr("请至少设置一种输出：改为 / 短按 / 长按 / 长按进入层 / 长按锁定层 / 单次 / 粘滞 / 双击 / 三击"));
    return false;
  }
  const item = deepClone(state.remapDraft);
  if (state.draftNew) {
    state.cfg.remaps.unshift(item);
  } else if (state.selected !== null && state.cfg.remaps[state.selected]) {
    item.enabled = state.cfg.remaps[state.selected].enabled;
    state.cfg.remaps[state.selected] = item;
  }
  // 保存后留在编辑页（草稿继续可改）：与快捷键同行为，别把用户刚存的条目关掉。
  state.draftNew = false;
  const ok = await save();
  state.selected = state.cfg.remaps.indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "remap");
  return ok;
}

async function saveExpansionDetail(): Promise<boolean> {
  if (state.section !== "expansions" || !state.expansionDraft) return false;
  state.expansionDraft.description = domValue("edit-desc") || null;
  const trigger = domValue("edit-exp-trigger").trim();
  // 触发词非法必须拦在保存前：save() 会把空触发词整条剔除——用户以为存了，其实条目没了。
  const err = expansionTriggerError(trigger);
  if (err) {
    const input = document.getElementById("edit-exp-trigger") as HTMLInputElement;
    showFieldError(input, err);
    input.focus();
    toast(trt`触发词无效：${err}`);
    return false;
  }
  state.expansionDraft.trigger = trigger;
  state.expansionDraft.replace = domValue("edit-exp-replace");
  const item = deepClone(state.expansionDraft);
  if (state.draftNew) {
    state.cfg.expansions.unshift(item);
  } else if (state.selected !== null && state.cfg.expansions[state.selected]) {
    item.enabled = state.cfg.expansions[state.selected].enabled;
    state.cfg.expansions[state.selected] = item;
  }
  // 保存后留在编辑页（与快捷键同行为）。
  state.draftNew = false;
  const ok = await save();
  state.selected = state.cfg.expansions.indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "expansion");
  return ok;
}

// 鼠标操作（手势）保存：方向串必填（空的后端清洗会整条丢掉），动作可空（空 = 只留条目不做事）。
async function saveGestureDetail(): Promise<boolean> {
  if (state.section !== "gestures" || !state.gestureDraft) return false;
  const pattern = domValue("edit-gesture-pattern").trim().toUpperCase();
  if (!pattern) {
    const input = document.getElementById("edit-gesture-pattern") as HTMLInputElement;
    showFieldError(input, tr("请填写方向串"));
    input.focus();
    toast(tr("请先填写方向串（如 D、DL、DRUL）"));
    return false;
  }
  state.gestureDraft.pattern = pattern;
  state.gestureDraft.description = domValue("edit-desc") || null;
  const item = deepClone(state.gestureDraft);
  const list = state.cfg.gestures ?? (state.cfg.gestures = []);
  if (state.draftNew) {
    list.unshift(item);
    mouseOrderFront(item);
  } else if (state.selected !== null && list[state.selected]) {
    item.enabled = list[state.selected].enabled;
    list[state.selected] = item;
  }
  // 保存后留在编辑页（与快捷键同行为）。
  state.draftNew = false;
  const ok = await save();
  state.selected = (state.cfg.gestures ?? []).indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "gesture");
  return ok;
}

// 触摸板手势保存：**触发键必填**；方向串与形状模板至少有一个（后端清洗会把两者都空的整条丢掉），
// 只有模板 = 「纯形状」条目（方向串留空，靠模板兜底匹配）；动作可空（空 = 只留条目不做事）。
async function saveTouchpadDetail(): Promise<boolean> {
  if (state.section !== "touchpads" || !state.touchpadDraft) return false;
  const trigger = domValue("edit-touchpad-trigger");
  if (!trigger) {
    const sel = document.getElementById("edit-touchpad-trigger") as HTMLSelectElement;
    showFieldError(sel, tr("请选择触发键"));
    sel.focus();
    toast(tr("请先选择一个键盘键作触发键"));
    return false;
  }
  const pattern = domValue("edit-touchpad-pattern").trim().toUpperCase();
  if (!pattern && !(state.touchpadDraft.template?.length ?? 0)) {
    const input = document.getElementById("edit-touchpad-pattern") as HTMLInputElement;
    showFieldError(input, tr("请填写方向串，或用「录入」画一笔形状"));
    input.focus();
    toast(tr("方向串可以留空（只认形状），但要先「录入」一笔留下形状模板"));
    return false;
  }
  state.touchpadDraft.trigger = trigger;
  state.touchpadDraft.pattern = pattern;
  state.touchpadDraft.template = state.touchpadDraft.template?.length ? state.touchpadDraft.template : null;
  state.touchpadDraft.description = domValue("edit-desc") || null;
  const item = deepClone(state.touchpadDraft);
  const list = state.cfg.touchpad_gestures ?? (state.cfg.touchpad_gestures = []);
  if (state.draftNew) {
    list.unshift(item);
    touchpadOrderFront(item);
  } else if (state.selected !== null && list[state.selected]) {
    item.enabled = list[state.selected].enabled;
    list[state.selected] = item;
  }
  // 保存后留在编辑页（与快捷键同行为）。
  state.draftNew = false;
  const ok = await save();
  state.selected = (state.cfg.touchpad_gestures ?? []).indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "touchpad");
  return ok;
}

// 角动作 / 边缘滚动保存：位置必选（草稿默认就是一个合法位置），动作可空。
async function saveHotCornerDetail(): Promise<boolean> {
  if (state.section !== "hotcorners" || !state.hotcornerDraft) return false;
  state.hotcornerDraft.description = domValue("edit-desc") || null;
  state.hotcornerDraft.layer = domValue("hotcorner-layer") || null;
  const item = deepClone(state.hotcornerDraft);
  const list = state.cfg.hot_corners ?? (state.cfg.hot_corners = []);
  if (state.draftNew) {
    list.unshift(item);
    mouseOrderFront(item);
  } else if (state.selected !== null && list[state.selected]) {
    item.enabled = list[state.selected].enabled;
    list[state.selected] = item;
  }
  // 保存后留在编辑页（与快捷键同行为）。
  state.draftNew = false;
  const ok = await save();
  state.selected = (state.cfg.hot_corners ?? []).indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "hotcorner");
  return ok;
}

async function saveWheelEdgeDetail(): Promise<boolean> {
  if (state.section !== "wheeledges" || !state.wheeledgeDraft) return false;
  state.wheeledgeDraft.description = domValue("edit-desc") || null;
  state.wheeledgeDraft.layer = domValue("wheeledge-layer") || null;
  const item = deepClone(state.wheeledgeDraft);
  const list = state.cfg.wheel_edges ?? (state.cfg.wheel_edges = []);
  if (state.draftNew) {
    list.unshift(item);
    mouseOrderFront(item);
  } else if (state.selected !== null && list[state.selected]) {
    item.enabled = list[state.selected].enabled;
    list[state.selected] = item;
  }
  // 保存后留在编辑页（与快捷键同行为）。
  state.draftNew = false;
  const ok = await save();
  state.selected = (state.cfg.wheel_edges ?? []).indexOf(item);
  if (ok) state.draftBaseline = draftSignature();
  render();
  if (ok) flashRow(state.selected ?? -1, "wheeledge");
  return ok;
}

// ---- 未保存改动守卫 ----
// 草稿签名与脏判定在 state.ts（draftSignature / isDraftDirty），这里只留触发重绘的部分。

function markDraftBaseline() {
  state.draftBaseline = draftSignature();
  refreshDirty();
}

// 脏标记与「撤销改动」的可用态。（标记放标题行右侧：标题本身就是名称输入框，动作行腾给了页签。）
function refreshDirty() {
  const dirty = isDraftDirty();
  document.getElementById("dirty-flag")?.classList.toggle("hidden", !dirty);
  const undo = document.getElementById("entry-undo") as HTMLButtonElement | null;
  if (undo) undo.disabled = !dirty;
  // 保存同理：没有改动就没有东西可落盘，按钮置灰（Ctrl+S 那条路径也按这个判定放行）。
  const save = document.getElementById("entry-save") as HTMLButtonElement | null;
  if (save) save.disabled = !dirty;
}

// 撤销改动：把草稿还原成基线内容（打开时 / 上次保存的），编辑页留在原地。
function revertDraft() {
  if (!state.draftBaseline) return;
  if (state.section === "shortcuts") {
    state.draft = JSON.parse(state.draftBaseline) as ShortcutItem;
    fillShortcutEditor();
  } else if (state.section === "remaps") {
    state.remapDraft = JSON.parse(state.draftBaseline) as Remap;
    fillRemapEditor();
  } else if (state.section === "expansions") {
    state.expansionDraft = JSON.parse(state.draftBaseline) as TextExpansion;
    fillExpansionEditor();
  } else if (state.section === "gestures") {
    state.gestureDraft = JSON.parse(state.draftBaseline) as Gesture;
    fillGestureEditor();
  } else if (state.section === "touchpads") {
    state.touchpadDraft = JSON.parse(state.draftBaseline) as TouchpadGesture;
    fillTouchpadEditor();
  } else if (state.section === "hotcorners") {
    state.hotcornerDraft = JSON.parse(state.draftBaseline) as HotCornerRule;
    fillHotCornerEditor();
  } else if (state.section === "wheeledges") {
    state.wheeledgeDraft = JSON.parse(state.draftBaseline) as WheelEdgeRule;
    fillWheelEdgeEditor();
  } else {
    return;
  }
  refreshDirty();
  // 触发键可能被改回来了，冲突提示跟着重算（否则还挂着已撤销的那条冲突）。
  void refreshConflicts().then(renderConflictBubble);
  toast(tr("已撤销未保存的改动"));
}

type UnsavedChoice = "save" | "discard" | "cancel";
let unsavedPending: ((c: UnsavedChoice) => void) | null = null;

function draftLabel(): string {
  if (state.section === "shortcuts") return trt`快捷键「${state.draft?.name || tr("未命名")}」`;
  if (state.section === "remaps" && state.remapDraft) {
    return trt`改键「${state.remapDraft.name?.trim() || `${state.remapDraft.from} → ${remapSummary(state.remapDraft)}`}」`;
  }
  if (state.section === "expansions") {
    return trt`文本扩展「${state.expansionDraft?.name?.trim() || state.expansionDraft?.trigger || tr("未命名")}」`;
  }
  if (state.section === "gestures") {
    return trt`鼠标操作「${state.gestureDraft?.name?.trim() || state.gestureDraft?.pattern || tr("未命名")}」`;
  }
  if (state.section === "touchpads") {
    return trt`触摸板手势「${state.touchpadDraft?.name?.trim() || state.touchpadDraft?.pattern || tr("未命名")}」`;
  }
  if (state.section === "hotcorners") {
    return trt`角动作「${state.hotcornerDraft?.name?.trim() || spotLabel(state.hotcornerDraft?.spot ?? "top_left")}」`;
  }
  if (state.section === "wheeledges") {
    return trt`边缘滚动「${state.wheeledgeDraft?.name?.trim() || spotLabel(state.wheeledgeDraft?.edge ?? "top")}」`;
  }
  return tr("当前编辑");
}

function askUnsaved(): Promise<UnsavedChoice> {
  // 已经有一个确认框等着时忽略后来的请求（连点两行会同时进来）：让它当「取消」处理，
  // 否则先到的那个 promise 永远不 resolve，那一次点击就永远挂在那里。
  if (unsavedPending) return Promise.resolve("cancel");
  document.getElementById("unsaved-text")!.textContent = trt`${draftLabel()}有未保存的改动。`;
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
export async function confirmLeaveDraft(): Promise<boolean> {
  if (!isDraftDirty()) return true;
  const choice = await askUnsaved();
  if (choice === "cancel") return false;
  if (choice === "discard") {
    discardDraft();
    state.draftNew = false;
    return true;
  }
  // 「保存并离开」：保存失败（或改键没设任何输出被判无效）就别走，留在编辑页别把改动丢了。
  return await saveCurrentDraft();
}

async function saveCurrentDraft(): Promise<boolean> {
  if (state.section === "shortcuts") return await saveShortcutDetail();
  if (state.section === "remaps") return await saveRemapDetail();
  if (state.section === "expansions") return await saveExpansionDetail();
  if (state.section === "gestures") return await saveGestureDetail();
  if (state.section === "touchpads") return await saveTouchpadDetail();
  if (state.section === "hotcorners") return await saveHotCornerDetail();
  if (state.section === "wheeledges") return await saveWheelEdgeDetail();
  return false;
}

// 列表行点开（合并列表：点哪一类的行就切到哪个编辑分区）/ 「＋ 新增」：
// 草稿有未保存改动时先问一句，别默默覆盖掉草稿。
async function requestOpenDetail(i: number, isNew = false, kind: ListKind = "shortcut") {
  if (!(await confirmLeaveDraft())) return;
  const tab = KIND_SECTION[kind];
  if (state.section !== tab) switchSectionNow(tab, { keepList: true });
  openDetail(i, isNew);
}

// 「＋ 新增」下拉 / 目录行的「＋」：开新草稿（可指定快捷键要落进的目录）。
async function requestNew(kind: ListKind, folderId: string | null = null) {
  if (!(await confirmLeaveDraft())) return;
  const tab = KIND_SECTION[kind];
  if (state.section !== tab) switchSectionNow(tab, { keepList: true });
  openDetail(-1, true, folderId);
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
  const editing =
    state.draft !== null ||
    state.remapDraft !== null ||
    state.expansionDraft !== null ||
    state.gestureDraft !== null ||
    state.touchpadDraft !== null ||
    state.hotcornerDraft !== null ||
    state.wheeledgeDraft !== null;
  if (editing) {
    if (isDraftDirty()) {
      const take = await askConfirm({
        title: tr("配置已被外部修改"),
        text:
          trt`${draftLabel()}有未保存的改动，而配置文件刚被外部修改（手改 JSON / 恢复备份 / 同步落盘）。` +
          tr("加载新内容会放弃这些改动。"),
        okLabel: tr("加载新配置"),
        cancelLabel: tr("保留我的改动（下次保存会覆盖外部改动）"),
      });
      // 保留编辑：界面与内存都不动，用户手上的编辑照旧。代价（下次保存覆盖外部改动）已经写在
      // 按钮上，消息中心里也留着一条外部改动的记录，之后不会再重复弹（后端已采纳过一次）。
      if (!take) return;
    }
    stopCapture();
    void stopRecIfAny();
    discardDraft();
    state.draftNew = false;
    state.selected = null;
  }
  state.cfg = next;
  await refreshConflicts();
  render();
  syncSettings();
  toast(tr("配置文件已被外部修改，已重新加载"));
}

// 合并列表里三类行同在一个 <ul>，靠 data-kind 认行（下标是各自数组的下标）。
function flashRow(i: number, kind: ListKind) {
  if (i < 0) return;
  const row = document.querySelector<HTMLElement>(`#shortcut-list .row[data-kind="${kind}"][data-idx="${i}"]`);
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
// `start` 是流程管线的起止说明（快捷键 = 触发键；鼠标操作 = 方向串）。
function renderActions(s: { actions: Action[]; triggers?: string[] }, start?: string) {
  const wrap = document.getElementById("action-list")!;
  renderActionList(wrap, s.actions, () => renderActions(s, start), {
    start: start ?? (s.triggers?.length ? s.triggers.join(" / ") : tr("未设触发键")),
  });
  refreshDirty();
}

// 递归把某个动作列表的所有动作标记为收起（打开已有多动作的快捷键时用）。
function collapseAllActions(actions: Action[]) {
  for (const a of actions) {
    // 收起状态按「动作本体」记（禁用壳已剥掉），与 flowStep 查找的 key 一致。
    const act = unwrapAction(a);
    collapsedActions.add(act);
    if (act.type === "if") {
      collapseAllActions(act.then);
      collapseAllActions(act.otherwise);
    } else if (act.type === "parallel") {
      collapseAllActions(act.actions);
    } else if (act.type === "group") {
      collapseAllActions(act.actions);
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
  // 禁用的动作外面套了一层壳：编辑与展示一律落在里面那条上，开关只动壳。
  const off = isDisabled(a);
  const act = unwrapAction(a);
  const collapsed = collapsedActions.has(act);
  // 「设置变量」用文本动作的类型名与配色（数据里仍是 `ask` 形态）。
  const typeValue: ActionBase["type"] = act.type === "ask" ? "text" : act.type;
  const step = el("div", `flow-step type-${typeValue}` + (off ? " is-off" : ""));
  const row = el("div", "action-row" + (collapsed ? " collapsed" : "") + (off ? " off" : ""));
  row.dataset.idx = String(i);
  (row as unknown as { _dropList?: Action[] })._dropList = actions;
  (row as unknown as { _dropIndex?: number })._dropIndex = i;

  const rail = el("div", "flow-rail");
  const dot = el("span", "flow-dot", String(i + 1));
  dot.title = tr("拖动排序 / 嵌套；点击展开或收起");
  rail.append(dot);

  // 第一行：拖动手柄 + 图标 + 类型名 + 标题（描述） + 变量流标签 + 禁用 + 删除
  const head = el("div", "action-head");

  const grip = el("span", "action-grip");
  grip.title = tr("拖动排序 / 嵌套；点击展开或收起");
  // 六个点的抓手：比原来的「⋮⋮」字号大一圈，命中区也一起放大（见 style.css）。
  grip.innerHTML =
    '<svg viewBox="0 0 12 18" aria-hidden="true" fill="currentColor"><circle cx="4" cy="4" r="1.5"/><circle cx="8" cy="4" r="1.5"/><circle cx="4" cy="9" r="1.5"/><circle cx="8" cy="9" r="1.5"/><circle cx="4" cy="14" r="1.5"/><circle cx="8" cy="14" r="1.5"/></svg>';
  head.append(grip);

  const ico = el("span", "action-ico", ACTION_ICONS[act.type]);
  ico.title = actionTypeLabel(act.type);
  head.append(ico);

  // 类型名本身就是切换入口：点它弹出动作类型列表（收起态也能直接换）。
  // 「设置变量」是文本操作的一种方式，类型名就显示「文本操作」（具体方式看子类型下拉）。
  const typePick = el("select", "action-type-name") as HTMLSelectElement;
  typePick.title = tr("点击切换动作类型");
  typePick.setAttribute("aria-label", tr("动作类型"));
  for (const t of actionTypeChoices(typeValue === "tool")) {
    const o = el("option") as HTMLOptionElement;
    o.value = t.value;
    o.textContent = t.label;
    typePick.append(o);
  }
  // 已移除 / 已合并的类型仍要能在老配置里看到并改走，不给它一个空下拉。
  if (!actionTypeChoices(typeValue === "tool").some((t) => t.value === typeValue)) {
    const o = el("option") as HTMLOptionElement;
    o.value = typeValue;
    o.textContent = actionTypeLabel(typeValue);
    typePick.append(o);
  }
  typePick.value = typeValue;
  typePick.addEventListener("change", () => {
    switchActionType(act, actions, i, typePick.value as ActionBase["type"], rerender);
  });
  head.append(typePick);

  // 拖拽 + 点击展开/收起：脊柱圆点、手柄、图标都可触发（扩大命中范围）。
  for (const handle of [dot, grip, ico]) {
    attachActionDragStart(handle, row, actions, i);
  }

  // 变量流向标签：写出（⇒ var，绿）在前、引用（{var}，灰）在后，一眼看出数据怎么流。
  const tags = el("span", "action-vartags");
  for (const w of actionVarWrites(act)) tags.append(el("span", "var-tag write", `⇒ ${w}`));
  const reads = actionVarReads(act);
  for (const r of reads.slice(0, 3)) tags.append(el("span", "var-tag read", `{${r}}`));
  if (reads.length > 3) tags.append(el("span", "var-tag read", `+${reads.length - 3}`));

  if (collapsed) {
    const title = el("span", "action-title", actionDescription(a));
    title.title = tr("点击展开");
    title.addEventListener("click", () => {
      toggleActionCollapsed(act);
      rerender();
    });
    head.append(title, tags);
  } else {
    const titleInput = el("input", "action-input action-title-input") as HTMLInputElement;
    titleInput.type = "text";
    // 分组没有「标题」这一层：直接编辑分组名称（纯展示，不参与执行）。
    if (act.type === "group") {
      const group = act;
      titleInput.value = group.name ?? "";
      titleInput.placeholder = tr("分组名称（可选）");
      titleInput.addEventListener("input", () => {
        group.name = titleInput.value;
      });
    } else {
      titleInput.value = act.description ?? "";
      titleInput.placeholder = tr("动作标题（可选）");
      titleInput.addEventListener("input", () => {
        act.description = titleInput.value.trim() || undefined;
      });
    }
    // 禁用开关（删除键左侧）：执行时整条跳过，内容原样留着。
    const offBtn = el("button", "action-off-btn" + (off ? " on" : "")) as HTMLButtonElement;
    offBtn.title = off ? tr("启用这个动作") : tr("禁用这个动作（执行时跳过）");
    offBtn.setAttribute("aria-label", offBtn.title);
    offBtn.innerHTML = off
      ? '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><path d="M14.12 14.12a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>'
      : '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>';
    offBtn.addEventListener("click", () => {
      actions[i] = off ? act : { type: "disabled", action: act };
      rerender();
    });
    const del = el("button", "action-del") as HTMLButtonElement;
    del.title = tr("删除动作");
    del.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/><line x1="10" y1="11" x2="10" y2="17"/><line x1="14" y1="11" x2="14" y2="17"/></svg>';
    del.addEventListener("click", () => {
      actions.splice(i, 1);
      collapsedActions.delete(act);
      rerender();
    });
    // 单动作试运行（CS-14）：只跑这一条（不保存、不跑前面的动作），强制弹结果。
    // 依赖 `{变量}` 时变量为空、命令可能报错——这是预期；提示写在按钮 title 里。
    if (act.type === "command" || act.type === "script") {
      const tryBtn = el("button", "action-try-btn", tr("试运行")) as HTMLButtonElement;
      tryBtn.type = "button";
      tryBtn.title = tr(
        "只跑这一条动作（不保存、不执行前面的动作）；依赖的 {变量} 为空时命令可能报错。",
      );
      tryBtn.addEventListener("click", () => {
        const preview = structuredClone(act);
        preview.show_output = "always";
        void runActions([preview], tr("试运行"));
      });
      head.append(titleInput, tags, tryBtn, offBtn, del);
    } else {
      head.append(titleInput, tags, offBtn, del);
    }
  }

  row.append(head);

  if (!collapsed) {
    // 第二行：子类型（类型本身由标题行的类型名切换，不再重复占一行）
    const sub = actionSubtypeSelect(act, rerender, (next) => {
      actions[i] = off ? { type: "disabled", action: next } : next;
    });
    if (sub) {
      const typeRow = el("div", "action-type-row");
      typeRow.append(sub);
      row.append(typeRow);
    }
    // 第三行：其他输入内容
    row.append(actionFields(act, rerender, { actions, idx: i }));
  }

  step.append(rail, row);
  return step;
}

// 把一个动作列表渲染成流程管线进 container；任何结构变化（增删/移动/改类型/拖拽）都通过
// rerender() 触发整体重绘。嵌套的「条件判断」动作同样用它递归渲染 then/otherwise 分支
//（不带起止端点）。opts.start 传入触发摘要时在管线头部加 ⚡ 端点、尾部加 ✓ 端点。
// 收起态只显示 类型名 + 描述 + 变量标签；展开态为 标题行（类型名可点切换）/ 子类型行 / 输入行。
function renderActionList(
  container: HTMLElement,
  actions: Action[],
  rerender: () => void,
  opts: { start?: string } = {},
): void {
  container.replaceChildren();
  container.classList.add("flow");
  // 拖拽命中靠这两样：`data-action-list` 用来在指针下找出所有可落列表，
  // `_dropList` 是这条列表对应的真实数组（非序列化属性）。
  container.dataset.actionList = "1";
  (container as unknown as { _dropList?: Action[] })._dropList = actions;
  if (opts.start !== undefined) {
    container.append(flowTerminator("start", tr("触发"), opts.start));
  }
  actions.forEach((a, i) => {
    container.append(flowStep(a, i, actions, rerender));
  });
  if (opts.start !== undefined) {
    container.append(
      flowTerminator(
        "end",
        actions.length ? tr("执行完成") : tr("还没有动作 · 点下方「+ 添加动作」"),
      ),
    );
  } else if (actions.length === 0) {
    container.append(el("div", "flow-empty", tr("空 · 拖入动作或点下方「添加动作」")));
  }
}

function toggleActionCollapsed(a: Action) {
  if (collapsedActions.has(a)) collapsedActions.delete(a);
  else collapsedActions.add(a);
}

// 换动作类型 = 换一个同位置的新动作；标题（description）能带就带。
// 分组没有 description；两种形态互转时能带就带，带不了就落回默认。
function convertedAction(from: ActionBase, type: ActionBase["type"]): ActionBase {
  const next = newAction(type);
  if (from.type !== "group" && next.type !== "group") next.description = from.description;
  return next;
}

// `a` 是当前那条动作本体（禁用壳已剥掉），列表里那一格可能是套了壳的——壳要留着。
function switchActionType(
  a: ActionBase,
  actions: Action[],
  i: number,
  type: ActionBase["type"],
  rerender: () => void,
) {
  if (type === a.type) return;
  const next = convertedAction(a, type);
  actions[i] = actions[i]?.type === "disabled" ? { type: "disabled", action: next } : next;
  collapsedActions.delete(a);
  rerender();
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
    state.actionDrag = { list, index, startX: e.clientX, startY: e.clientY, active: false, sourceEl: row };
  });
}

type ActionDropTarget =
  | { kind: "line"; list: Action[]; index: number }
  | { kind: "into"; list: Action[] };

type ActionDropHit = {
  target: ActionDropTarget;
  /** 插入线的位置（视口坐标）；`into` 时为空。 */
  line?: { y: number; left: number; width: number };
  container?: HTMLElement;
  forbidden: boolean;
};

// 一条可落动作的列表：顶层 `#action-list` 与每个分支泳道（if / parallel / group）。
// 命中不复用 `elementFromPoint`：卡片之间的 12px 缝隙属于 `.flow-step`，会漏判；
// 改成「几何包含」——指针落在哪条列表的矩形里就归哪条，嵌套时取面积最小的那条。
type ActionListBox = { el: HTMLElement; list: Action[]; rect: DOMRect };

function actionListBoxes(): ActionListBox[] {
  const root = document.querySelector<HTMLElement>("#action-list");
  if (!root) return [];
  const els = [root, ...root.querySelectorAll<HTMLElement>("[data-action-list]")];
  const out: ActionListBox[] = [];
  for (const el of els) {
    const list = (el as unknown as { _dropList?: Action[] })._dropList;
    if (!Array.isArray(list)) continue;
    out.push({ el, list, rect: el.getBoundingClientRect() });
  }
  return out;
}

// 列表里的动作卡片（起止端点 / 空提示不算）。`.flow-step` 的结构固定是 [脊柱, 卡片]。
function actionListRows(el: HTMLElement): HTMLElement[] {
  const rows: HTMLElement[] = [];
  for (const child of Array.from(el.children)) {
    if (!child.classList.contains("flow-step")) continue;
    const row = child.lastElementChild;
    if (row instanceof HTMLElement && row.classList.contains("action-row")) rows.push(row);
  }
  return rows;
}

// 某个动作内部还能落东西的列表（用于禁止把容器拖进自己肚子里）。
function nestedActionLists(a: Action): Action[][] {
  if (a.type === "disabled") return nestedActionLists(a.action);
  if (a.type === "if") {
    return [
      a.then,
      a.otherwise,
      ...a.then.flatMap(nestedActionLists),
      ...a.otherwise.flatMap(nestedActionLists),
    ];
  }
  if (a.type === "parallel" || a.type === "group") {
    return [a.actions, ...a.actions.flatMap(nestedActionLists)];
  }
  return [];
}

// 命中计算：先在所有列表里找包含指针的最内层，再按卡片中线算插入下标；
// 插入线画在两张卡片之间的空隙里（不是画在卡片上下边缘的内描边上）。
function actionDropAt(x: number, y: number): ActionDropHit | null {
  if (!state.actionDrag) return null;
  const moved = state.actionDrag.list[state.actionDrag.index];
  if (!moved) return null;
  const boxes = actionListBoxes();
  if (!boxes.length) return null;

  const inside = boxes.filter(
    (b) =>
      x >= b.rect.left &&
      x <= b.rect.right &&
      y >= b.rect.top &&
      y <= b.rect.bottom,
  );
  let box: ActionListBox | undefined;
  if (inside.length) {
    box = inside.reduce((a, b) =>
      a.rect.width * a.rect.height <= b.rect.width * b.rect.height ? a : b,
    );
  } else {
    // 指针滑到列表左右外侧（或刚好越过首尾）时，按垂直距离找最近的一条，容错 60px。
    let best = Infinity;
    for (const b of boxes) {
      if (x < b.rect.left - 60 || x > b.rect.right + 60) continue;
      const d = Math.max(0, b.rect.top - y, y - b.rect.bottom);
      if (d < best) {
        best = d;
        box = b;
      }
    }
    if (!box || best > 60) return null;
  }
  if (!box) return null;

  const forbidden = nestedActionLists(moved).includes(box.list);
  const rows = actionListRows(box.el);
  if (!rows.length) {
    return { target: { kind: "into", list: box.list }, container: box.el, forbidden };
  }

  let index = rows.length;
  for (let j = 0; j < rows.length; j++) {
    const r = rows[j].getBoundingClientRect();
    if (y < r.top + r.height / 2) {
      index = j;
      break;
    }
  }

  // 线落在相邻两张卡片的空隙中央；首尾各向外让 6px（贴着卡片但不压住内容）。
  const prev = index > 0 ? rows[index - 1].getBoundingClientRect() : null;
  const next = index < rows.length ? rows[index].getBoundingClientRect() : null;
  const yLine = prev && next ? (prev.bottom + next.top) / 2 : next ? next.top - 6 : prev!.bottom + 6;
  const first = rows[0].getBoundingClientRect();
  return {
    target: { kind: "line", list: box.list, index },
    line: { y: yLine, left: first.left, width: first.width },
    forbidden,
  };
}

let actionDropLine: HTMLElement | null = null;
let actionDropIntoEl: HTMLElement | null = null;

function clearActionDropIndicators() {
  actionDropLine?.remove();
  actionDropLine = null;
  if (actionDropIntoEl) {
    actionDropIntoEl.classList.remove("drop-into", "drop-forbidden");
    actionDropIntoEl = null;
  }
}

function showActionDropLine(g: { y: number; left: number; width: number }, forbidden: boolean) {
  if (!actionDropLine) {
    actionDropLine = el("div", "action-drop-line");
    document.body.append(actionDropLine);
  }
  actionDropLine.classList.toggle("forbidden", forbidden);
  actionDropLine.style.left = `${g.left}px`;
  actionDropLine.style.width = `${Math.max(0, g.width)}px`;
  actionDropLine.style.top = `${g.y}px`;
}

function updateActionDropTarget(x: number, y: number) {
  const hit = actionDropAt(x, y);
  clearActionDropIndicators();
  if (!hit) return;
  if (hit.target.kind === "into" && hit.container) {
    hit.container.classList.add(hit.forbidden ? "drop-forbidden" : "drop-into");
    actionDropIntoEl = hit.container;
    return;
  }
  if (hit.line) showActionDropLine(hit.line, hit.forbidden);
}

function applyActionDrop(srcList: Action[], srcIndex: number, target: ActionDropTarget) {
  const moved = srcList[srcIndex];
  if (!moved) return;
  // 把容器拖进自己的分支会造出环（序列化直接失败），一律拒绝。
  if (nestedActionLists(moved).includes(target.list)) return;
  srcList.splice(srcIndex, 1);
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
// 「设置变量」不是独立动作类型，而是文本操作的第四种方式——选中它 = 换成 `ask` 形态。
function actionSubtypeSelect(
  a: ActionBase,
  rerender: () => void,
  replace?: (next: ActionBase) => void,
): HTMLElement | null {
  if (a.type === "text" || a.type === "ask") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const m of [
      { value: "input", label: tr("输入文本") },
      { value: "to_upper", label: tr("小写转大写（选中/剪贴板）") },
      { value: "to_lower", label: tr("大写转小写（选中/剪贴板）") },
      { value: "ask", label: tr("设置变量") },
    ]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = m.value;
      opt.textContent = m.label;
      sel.append(opt);
    }
    sel.value = a.type === "ask" ? "ask" : a.mode;
    sel.addEventListener("change", () => {
      const v = sel.value;
      if (v === "ask" || a.type === "ask") {
        // 换形态要换整条动作（字段完全不同），走和类型下拉同一条路。
        if (replace) {
          replace(convertedAction(a, v === "ask" ? "ask" : "text"));
          rerender();
        }
        return;
      }
      a.mode = v as TextMode;
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
  if (a.type === "system") {
    const op = a.operation;
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of SYSTEM_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = op.op;
    sel.addEventListener("change", () => {
      // 子操作变了，动作类型始终是「操作系统」——不再把整条动作换成别的分类。
      a.operation = newSystemOp(sel.value as SystemOperation["op"]);
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
    // 「当前窗口」系列同属应用操作（原独立类型 `window`）：作用对象是触发时的前台窗口。
    for (const o of WINDOW_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = `current_window:${o.value}`;
      opt.textContent = trt`当前窗口：${o.label}`;
      sel.append(opt);
    }
    sel.value =
      op.op === "current_window" ? `current_window:${op.action}` : op.op;
    sel.addEventListener("change", () => {
      if (sel.value.startsWith("current_window:")) {
        a.operation = {
          op: "current_window",
          action: sel.value.slice("current_window:".length) as WindowOperation,
        };
      } else {
        a.operation = newAppOp(sel.value as AppOperation["op"]);
      }
      rerender();
    });
    return sel;
  }
  if (a.type === "window") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of WINDOW_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.operation.op;
    sel.addEventListener("change", () => {
      a.operation = { op: sel.value as WindowOperation };
      rerender();
    });
    return sel;
  }
  if (a.type === "capture") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of CAPTURE_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.operation;
    sel.addEventListener("change", () => {
      a.operation = sel.value as CaptureOperation;
      rerender();
    });
    return sel;
  }
  if (a.type === "tool") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const t of state.runningTools) {
      for (const c of t.capabilities) {
        const opt = el("option") as HTMLOptionElement;
        opt.value = `${t.id}\u0000${c.id}`;
        opt.textContent = `${t.name} · ${c.name}`;
        sel.append(opt);
      }
    }
    // 工具已经退出 / 老配置里的条目：仍要把当前值显示出来，改选才换。
    if (![...sel.options].some((o) => o.value === `${a.tool}\u0000${a.capability}`)) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = `${a.tool}\u0000${a.capability}`;
      opt.textContent = trt`${a.tool} · ${a.capability}（未检测到）`;
      sel.append(opt);
    }
    sel.value = `${a.tool}\u0000${a.capability}`;
    sel.addEventListener("change", () => {
      const [tool, capability] = sel.value.split("\u0000");
      a.tool = tool ?? "";
      a.capability = capability ?? "";
      rerender();
    });
    return sel;
  }
  if (a.type === "brightness") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of BRIGHTNESS_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.operation;
    sel.addEventListener("change", () => {
      a.operation = sel.value as BrightnessOperation;
      rerender();
    });
    return sel;
  }
  if (a.type === "clipboard") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of CLIPBOARD_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.operation.op;
    sel.addEventListener("change", () => {
      a.operation = newClipboardOp(sel.value as ClipboardOperation["op"]);
      rerender();
    });
    return sel;
  }
  if (a.type === "transform") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of TRANSFORM_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.operation;
    sel.addEventListener("change", () => {
      a.operation = sel.value as TransformOperation;
      rerender();
    });
    return sel;
  }
  if (a.type === "send_to") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const [value, label] of [
      ["keys", tr("按键")],
      ["text", tr("文本")],
    ] as [SendPayload["kind"], string][]) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = value;
      opt.textContent = label;
      sel.append(opt);
    }
    sel.value = a.payload.kind;
    sel.addEventListener("change", () => {
      a.payload =
        sel.value === "keys" ? { kind: "keys", keys: [] } : { kind: "text", text: "" };
      rerender();
    });
    return sel;
  }
  if (a.type === "mouse") {
    const sel = el("select", "action-type") as HTMLSelectElement;
    for (const o of MOUSE_OPS) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = o.value;
      opt.textContent = o.label;
      sel.append(opt);
    }
    sel.value = a.op.op;
    sel.addEventListener("change", () => {
      a.op = newMouseOp(sel.value as MouseOp["op"]);
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

/** 条件字段编辑器（`If` 动作与触发侧 `when` 门控共用）：按 `cond.kind` 渲染对应输入，
 * 原地改写 `cond`。调用方在切换 kind 时重建。 */
function conditionFields(cond: Condition): HTMLElement {
  const body = el("div", "action-body");

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
    line.append(el("span", "unit-hint", tr("修改时间在")), mins, el("span", "unit-hint", tr("分钟内为真")));
    body.append(line);
  } else if (cond.kind === "frontmost_app" || cond.kind === "not_frontmost_app") {
    const line = el("div", "action-line");
    const app = el("input", "action-input") as HTMLInputElement;
    app.type = "text";
    app.value = cond.app;
    app.placeholder = tr("进程名，如 chrome.exe（含 * / ? 走通配，否则子串匹配）");
    app.addEventListener("input", () => {
      cond.app = app.value;
    });
    line.append(el("span", "unit-hint", tr("进程名")), app);
    body.append(line);
  } else if (cond.kind === "window_title_contains") {
    const line = el("div", "action-line");
    const text = el("input", "action-input") as HTMLInputElement;
    text.type = "text";
    text.value = cond.text;
    text.placeholder = tr("窗口标题包含的文本（不区分大小写）");
    text.addEventListener("input", () => {
      cond.text = text.value;
    });
    line.append(el("span", "unit-hint", tr("包含文本")), text);
    body.append(line);
  } else if (cond.kind === "device_is") {
    // 选择式 + 手写兜底：下拉列出本机输入设备（键盘 / 鼠标 / 触摸板 / 数位板，后端枚举、
    // 值取稳定片段，按大类分组），选中就填进下面的设备标识；想写通配（`VID_046D*`）仍在
    // 输入框里改——输入框才是配置的真身。
    const pickLine = el("div", "action-line");
    const pick = el("select", "action-input") as HTMLSelectElement;
    // 先挂上「自定义」占位：列表是异步拉的，别让下拉在这一瞬间是空白的。
    pick.append(new Option(tr("自定义 / 手动输入"), ""));
    const recheck = el("button", "ghost", tr("重新检测")) as HTMLButtonElement;
    recheck.type = "button";
    const pickHint = el("div", "unit-hint unit-hint-wrap", "");
    pickLine.append(el("span", "unit-hint", tr("检测到的设备")), pick, recheck);
    body.append(pickLine, pickHint);

    const line = el("div", "action-line");
    const id = el("input", "action-input") as HTMLInputElement;
    id.type = "text";
    id.value = cond.id;
    id.placeholder = tr("设备标识，如 AT Translated Set 2 keyboard 或 VID_046D&PID_C31C（含 * / ? 走通配，否则子串匹配）");
    id.addEventListener("input", () => {
      cond.id = id.value;
      // 手改过（或改成通配）就不再算「选中的某台」：下拉退回「自定义」。
      if (pick.selectedIndex > 0 && id.value.trim().toLowerCase() !== pick.value.toLowerCase()) {
        pick.selectedIndex = 0;
      }
    });
    line.append(el("span", "unit-hint", tr("设备标识")), id);
    body.append(line);

    // 设备大类 → 下拉分组名。后端给的是稳定 id（不是文案），翻译留在前端。
    const kindLabel = (kind: string) =>
      kind === "keyboard"
        ? tr("键盘")
        : kind === "mouse"
          ? tr("鼠标")
          : kind === "touchpad"
            ? tr("触摸板")
            : kind === "tablet"
              ? tr("数位板")
              : tr("其它");
    const fillPicker = (list: InputDeviceList) => {
      pick.replaceChildren();
      pick.append(new Option(tr("自定义 / 手动输入"), ""));
      // 按大类分组（`<optgroup>`）：同一台电脑上键盘鼠标触摸板混在一起，不分组长得都一样。
      // 后端已按「键盘 → 鼠标 → 触摸板 → 数位板」排好，这里遇到新类别才新开一组。
      let group: HTMLOptGroupElement | null = null;
      let groupKind = "";
      for (const d of list.devices) {
        if (d.kind !== groupKind || !group) {
          groupKind = d.kind;
          group = document.createElement("optgroup");
          group.label = kindLabel(d.kind);
          pick.append(group);
        }
        const o = new Option(d.label, d.id);
        o.title = d.detail;
        group.append(o);
      }
      // 当前值正好是某个可选设备 → 选中它；否则「自定义」（配置可能是手写的通配 / 别处同步来的）。
      const hit = list.devices.find((d) => d.id.toLowerCase() === cond.id.trim().toLowerCase());
      pick.value = hit ? hit.id : "";
      pick.disabled = list.devices.length === 0;
      pickHint.textContent = list.devices.length
        ? tr("选一台设备会填进下面的设备标识；也可以直接手写（支持 * / ? 通配）。")
        : list.supported
          ? tr("没有检测到输入设备（输入层没起来 / 权限不足时读不到），可直接手写设备标识。")
          : tr("本平台取不到设备标识（macOS），这个条件恒不成立。");
    };
    pick.addEventListener("change", () => {
      if (!pick.value) return; // 「自定义」只是让用户接着手写，不动输入框
      cond.id = pick.value;
      id.value = pick.value;
    });
    onClickAsync(recheck, async () => {
      recheck.disabled = true;
      fillPicker(await refreshInputDevices(true));
      recheck.disabled = false;
    });
    // 首次渲染先用缓存 / 拉一次；元素可能已被下一个渲染丢掉，回填只碰自己这几个节点。
    void refreshInputDevices().then(fillPicker);

    body.append(
      el(
        "div",
        "unit-hint unit-hint-wrap",
        tr("按触发来自哪台设备区分：键盘按快捷键、鼠标键 / 手势 / 触摸板笔迹看指针设备（Linux 用 evdev 设备名、Windows 用 HID 设备路径，均支持 * / ? 通配或子串匹配；macOS 取不到设备，恒走「否则」分支）。"),
      ),
    );
  } else {
    const lineVar = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = cond.var;
    vname.placeholder = tr("变量名，如 f");
    vname.addEventListener("input", () => {
      cond.var = vname.value.trim();
    });
    lineVar.append(el("span", "unit-hint", tr("变量名")), vname);
    body.append(lineVar);

    const lineField = el("div", "action-line");
    const fname = el("input", "action-input") as HTMLInputElement;
    fname.type = "text";
    fname.value = cond.field;
    fname.placeholder = tr("字段（空 = 完整路径），如 ext / size / is_dir");
    fname.addEventListener("input", () => {
      cond.field = fname.value.trim();
    });
    lineField.append(el("span", "unit-hint", tr("字段")), fname);
    body.append(lineField);

    const lineVal = el("div", "action-line");
    const val = el("input", "action-input") as HTMLInputElement;
    val.type = "text";
    val.value = cond.value;
    val.placeholder = tr("比较值，如 .txt");
    val.addEventListener("input", () => {
      cond.value = val.value;
    });
    lineVal.append(el("span", "unit-hint", tr("等于/不等于")), val);
    body.append(lineVal);
  }

  return body;
}

/** 触发侧 `when` 门控编辑器：一个「无条件 / 条件类型」下拉 + 该条件的字段。
 * `get` 读当前草稿的 `when`，`set` 写回（`null` = 清除）。就地改写草稿对象。 */
function renderWhenEditor(
  host: HTMLElement,
  get: () => Condition | null | undefined,
  set: (c: Condition | null) => void,
  onDirty: () => void,
) {
  const draw = () => {
    host.innerHTML = "";
    const cur = get() ?? null;
    const sel = el("select", "when-kind") as HTMLSelectElement;
    const none = el("option") as HTMLOptionElement;
    none.value = "";
    none.textContent = tr("无条件（始终触发）");
    sel.append(none);
    for (const c of WHEN_CONDITION_TYPES) {
      const opt = el("option") as HTMLOptionElement;
      opt.value = c.value;
      opt.textContent = c.label;
      sel.append(opt);
    }
    sel.value = cur?.kind ?? "";
    sel.addEventListener("change", () => {
      const next = sel.value ? newCondition(sel.value as Condition["kind"]) : null;
      set(next);
      onDirty();
      draw();
    });
    host.append(sel);
    if (cur) host.append(conditionFields(cur));
  };
  draw();
}

// 鼠标操作编辑器里的「录制」按钮：录一段鼠标动作（移动到某处再点击 / 滚轮），录完替换掉
// 当前动作。与键盘宏录制（keyRecordButton）独立，结果只含鼠标操作动作。
function mouseRecordButton(
  rerender: () => void,
  ctx?: { actions: Action[]; idx: number },
): HTMLElement {
  const btn = el("button", "record action-mouse-record") as HTMLButtonElement;
  btn.textContent = state.mouseRecording ? tr("■ 录制中（点此停止）") : tr("● 录制鼠标操作");
  if (state.mouseRecording) btn.classList.add("recording");
  btn.title = tr("录一段鼠标动作：移动到某处点击 / 滚轮，录完替换当前动作");
  onClickAsync(btn, async () => {
    if (!state.mouseRecording) {
      try {
        await invoke("start_mouse_record");
      } catch (err) {
        toast(trt`开始录制失败: ${err}`);
        return;
      }
      state.mouseRecording = true;
      btn.classList.add("recording");
      btn.textContent = tr("■ 录制中（点此停止）");
      return;
    }
    state.mouseRecording = false;
    let rec: Action[];
    try {
      rec = await invoke<Action[]>("stop_mouse_record");
    } catch (err) {
      toast(trt`停止录制失败: ${err}`);
      btn.classList.remove("recording");
      btn.textContent = tr("● 录制鼠标操作");
      return;
    }
    btn.classList.remove("recording");
    btn.textContent = tr("● 录制鼠标操作");
    if (rec.length && ctx) {
      // 录到的鼠标动作替换掉当前这一条（占位），其余顺次插在后面。
      ctx.actions.splice(ctx.idx, 1, ...rec);
      rerender();
    } else {
      toast(tr("未录到鼠标动作"));
    }
  });
  return btn;
}

// 「按键组合」动作里的「开始录制」：录一段键盘宏（组合键 + 间隔），录完替换掉当前动作。
// 与鼠标录制（mouseRecordButton）各自独立，但两个入口都长在同一张动作卡片里。
function keyRecordButton(
  rerender: () => void,
  ctx?: { actions: Action[]; idx: number },
): HTMLElement {
  const btn = el("button", "record action-key-record") as HTMLButtonElement;
  btn.textContent = state.recording ? tr("■ 录制中（点此停止）") : tr("● 开始录制");
  if (state.recording) btn.classList.add("recording");
  btn.title = tr("录一段按键（组合键与按键间隔），录完替换当前动作");
  onClickAsync(btn, async () => {
    if (!state.recording) {
      try {
        await invoke("start_record");
      } catch (err) {
        toast(trt`开始录制失败: ${err}`);
        return;
      }
      state.recording = true;
      btn.classList.add("recording");
      btn.textContent = tr("■ 录制中（点此停止）");
      return;
    }
    state.recording = false;
    let rec: Action[];
    try {
      rec = await invoke<Action[]>("stop_record");
    } catch (err) {
      toast(trt`停止录制失败: ${err}`);
      btn.classList.remove("recording");
      btn.textContent = tr("● 开始录制");
      return;
    }
    btn.classList.remove("recording");
    btn.textContent = tr("● 开始录制");
    if (rec.length && ctx) {
      // 录到的按键动作替换掉当前这一条（占位），其余顺次插在后面。
      ctx.actions.splice(ctx.idx, 1, ...rec);
      rerender();
    } else {
      toast(tr("没有录到按键"));
    }
  });
  return btn;
}

/** 「后台发送」的字段（目标窗口 + 按键 / 文本）：系统操作的子操作与旧 `send_to` 类型共用。 */
function sendToFields(
  op: { target: string; payload: SendPayload },
  rerender: () => void,
): HTMLElement[] {
  const nodes: HTMLElement[] = [];
  nodes.push(
    el(
      "div",
      "unit-hint",
      tr("把按键或文本发给一个非前台窗口，全程不激活、不抢焦点。目标填进程名或窗口标题关键字（忽略大小写，支持 * 和 ?）；多个窗口时取最上层。仅 Windows 支持；浏览器 / Electron / UWP / 游戏 / 提权窗口通常收不到消息，且不会回落到前台注入。"),
    ),
  );
  const targetLine = el("div", "action-line");
  const target = el("input", "action-input") as HTMLInputElement;
  target.type = "text";
  target.value = op.target;
  target.placeholder = tr("目标窗口：进程名或标题关键字，如 chrome.exe / 记事本");
  target.addEventListener("input", () => {
    op.target = target.value;
  });
  targetLine.append(el("span", "unit-hint", tr("目标窗口")), target);
  nodes.push(targetLine);

  const kindLine = el("div", "action-line");
  const kind = el("select", "action-input") as HTMLSelectElement;
  for (const [value, label] of [
    ["keys", tr("按键")],
    ["text", tr("文本")],
  ] as [SendPayload["kind"], string][]) {
    const o = el("option") as HTMLOptionElement;
    o.value = value;
    o.textContent = label;
    kind.append(o);
  }
  kind.value = op.payload.kind;
  kind.addEventListener("change", () => {
    op.payload =
      kind.value === "keys" ? { kind: "keys", keys: [] } : { kind: "text", text: "" };
    rerender();
  });
  kindLine.append(el("span", "unit-hint", tr("发送内容")), kind);
  nodes.push(kindLine);

  if (op.payload.kind === "keys") {
    const line = el("div", "action-line");
    const keys = el("input", "action-input") as HTMLInputElement;
    keys.type = "text";
    keys.value = op.payload.keys.join("+");
    keys.placeholder = tr("如 Ctrl+S");
    keys.addEventListener("input", () => {
      op.payload = {
        kind: "keys",
        keys: keys.value
          .split("+")
          .map((x) => x.trim())
          .filter(Boolean),
      };
    });
    const cap = el("button", undefined, tr("录入")) as HTMLButtonElement;
    cap.addEventListener("click", (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      startCapture((combo) => {
        op.payload = { kind: "keys", keys: combo.split("+") };
        keys.value = combo;
      }, btn);
    });
    line.append(keys, cap);
    nodes.push(line);
  } else {
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = op.payload.text;
    ta.placeholder = tr("要发送的文本（支持 {变量名} 占位符）……");
    ta.addEventListener("input", () => {
      op.payload = { kind: "text", text: ta.value };
    });
    nodes.push(ta);
  }
  return nodes;
}

function actionFields(
  a: ActionBase,
  rerender: () => void,
  ctx?: { actions: Action[]; idx: number },
): HTMLElement {
  const body = el("div", "action-body");
  if (a.type === "ask") {
    // 设置变量（文本操作的第四种方式）：沿用文本动作的通用行样式，不另立一套表单卡。
    const line = (label: string, value: string, placeholder: string, set: (v: string) => void) => {
      const row = el("div", "action-line");
      const input = el("input", "action-input") as HTMLInputElement;
      input.type = "text";
      input.value = value;
      input.placeholder = placeholder;
      input.addEventListener("input", () => set(input.value));
      row.append(el("span", "unit-hint", label), input);
      return row;
    };
    body.append(
      line(tr("变量名"), a.var, tr("变量名（后面用 {变量名} 引用）"), (v) => (a.var = v.trim())),
      line(
        tr("提示语"),
        a.prompt,
        tr("弹框里问用户的话（支持 {变量名}）"),
        (v) => (a.prompt = v),
      ),
      line(
        tr("默认值"),
        a.default ?? "",
        tr("回车前先填好的内容（支持 {变量名}）"),
        (v) => (a.default = v),
      ),
      el("div", "unit-hint", tr("触发时弹输入框（跟随主题），填的内容存进变量；点取消或留空则本次执行中止。")),
    );
    return body;
  }
  if (a.type === "text") {
    if (a.mode === "input") {
      // 隐藏态不渲染真文本（连长度都不露），编辑框位置换成一块 *** 占位；要改内容先关掉开关。
      if (a.masked) {
        const masked = el("div", "action-textarea action-masked", "***");
        masked.title = tr("文本已隐藏（关掉下面的开关即可查看与编辑）");
        body.append(masked);
      } else {
        const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
        ta.value = a.text;
        ta.placeholder = tr("要输入的文本（支持 {变量名} 占位符）……");
        ta.addEventListener("input", () => {
          a.text = ta.value;
        });
        body.append(ta);
      }
      const hide = el("label", "action-check") as HTMLLabelElement;
      const hb = el("input", "toggle") as HTMLInputElement;
      hb.type = "checkbox";
      hb.checked = !!a.masked;
      hide.title = tr("勾上后这段文本在界面上以 *** 显示（执行记录里也掩掉），注入的内容不受影响");
      hb.addEventListener("change", () => {
        a.masked = hb.checked;
        rerender();
      });
      hide.append(hb, el("span", undefined, tr("隐藏内容（显示为 ***）")));
      body.append(hide);
    } else {
      body.append(
        el(
          "div",
          "unit-hint",
          a.mode === "to_upper"
            ? tr("把当前选中（或剪贴板）的文本转为大写，并粘贴回原处。")
            : tr("把当前选中（或剪贴板）的文本转为小写，并粘贴回原处。"),
        ),
      );
    }
  } else if (a.type === "command") {
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = a.command;
    ta.placeholder =
      a.shell === "cmd"
        ? tr("CMD 命令，如 echo hello（可用 {变量名} / {变量名.字段} 引用变量）")
        : tr("PowerShell 命令，如 Get-Date（可用 {变量名} / {变量名.字段} 引用变量）");
    ta.addEventListener("input", () => {
      a.command = ta.value;
    });
    body.append(ta);

    // 执行语义字段（CS-01~CS-05）：在哪跑 / 等多久 / 什么退出码算成功。
    body.append(workingDirField(a.working_dir, (v) => (a.working_dir = v)));
    body.append(timeoutField(a.timeout_ms, (v) => (a.timeout_ms = v)));
    body.append(okExitCodesField(a.ok_exit_codes, (v) => (a.ok_exit_codes = v)));
    body.append(...commandRunControlFields(a, rerender));

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = tr("变量名（可选），留空则只进消息中心");
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(
      el("span", "unit-hint", tr("存为变量")),
      vname,
      helpIcon(
        tr("填写后把命令标准输出存进该变量：{drive} 得到输出全文、{drive.exit_code} 得到退出码（0=成功）。留空则维持现状，仅记入消息中心。示例：取盘符 (Get-Volume -FileSystemLabel '我的硬盘').DriveLetter 存为 drive，后面复制动作目标写 {drive}:\\备份\\。"),
      ),
    );
    body.append(vline);
  } else if (a.type === "script") {
    body.append(pathField(a.path, (v) => (a.path = v), { file: true, label: tr("脚本路径") }));

    const iline = el("div", "action-line");
    const interp = el("input", "action-input") as HTMLInputElement;
    interp.type = "text";
    interp.value = a.interpreter ?? "";
    interp.placeholder = tr("解释器（可选），如 python / node / bash；留空则直接执行脚本本身");
    interp.addEventListener("input", () => {
      const v = interp.value.trim();
      a.interpreter = v.length > 0 ? v : null;
    });
    iline.append(el("span", "unit-hint", tr("解释器")), interp);
    body.append(iline);

    // 参数（CS-03）：解释器参数在脚本路径之前、脚本参数在之后；一行一个，
    // 逐项做 {变量} 替换，argv 原样传递（不做引号拆分）。
    body.append(
      linesField(
        tr("解释器参数"),
        a.interpreter_args,
        (v) => (a.interpreter_args = v),
        tr("写在脚本路径之前的参数，一行一个。例：python 的 -u（不缓冲输出）；PowerShell 的 -NoProfile 和 -File（-File 必须带上，否则参数里的空格会被 PowerShell 重新拆开）。"),
      ),
    );
    body.append(
      linesField(
        tr("脚本参数"),
        a.args,
        (v) => (a.args = v),
        tr("写在脚本路径之后的参数，一行一个，支持 {变量}。例：python -u script.py --file \"{path}\" 就是两行：--file 与 {path}。含空格的参数（如 C:\\My Files）整行原样到达，不需要自己加引号。"),
      ),
    );

    // 执行语义字段（CS-01~CS-05）：在哪跑 / 等多久 / 什么退出码算成功。
    body.append(workingDirField(a.working_dir, (v) => (a.working_dir = v)));
    body.append(timeoutField(a.timeout_ms, (v) => (a.timeout_ms = v)));
    body.append(okExitCodesField(a.ok_exit_codes, (v) => (a.ok_exit_codes = v)));
    body.append(...commandRunControlFields(a, rerender));

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = tr("变量名（可选），留空则只进消息中心");
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(
      el("span", "unit-hint", tr("存为变量")),
      vname,
      helpIcon(
        tr("脚本执行时通过环境变量注入触发上下文：KADA_TRIGGER（触发组合，如 Ctrl+Alt+K）、KADA_NAME（快捷键名）、KADA_VARS（变量表 JSON）。填写变量名后把标准输出存进该变量（{out} 得输出全文、{out.exit_code} 得退出码）。"),
      ),
    );
    body.append(vline);
  } else if (a.type === "inline_script") {
    const ta = el("textarea", "action-textarea action-code") as HTMLTextAreaElement;
    ta.value = a.source;
    ta.spellcheck = false;
    ta.placeholder = tr("Rhai 脚本，例如：\nlet who = get_var(\"name\");\n\"Hello, \" + who");
    ta.addEventListener("input", () => {
      a.source = ta.value;
    });
    body.append(ta);

    const tools = el("div", "action-line");
    const sample = el("button", undefined, tr("插入示例"));
    sample.addEventListener("click", () => {
      a.source = INLINE_SCRIPT_SAMPLE;
      rerender();
    });
    tools.append(sample, el("span", "unit-hint", tr("沙箱内运行：不读写文件、不联网、不启动进程")));
    body.append(tools);

    const flags = el("div", "action-checks");
    const popup = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = !!a.show_output;
    cb.addEventListener("change", () => {
      a.show_output = cb.checked;
    });
    popup.append(cb, el("span", undefined, tr("弹窗显示结果")));
    flags.append(popup);
    body.append(flags);

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = tr("变量名（可选），留空则只进消息中心");
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(
      el("span", "unit-hint", tr("存为变量")),
      vname,
      helpIcon(
        tr("脚本的返回值（没有返回值时用 print 的输出）作为本步结果；填写变量名后存进该变量，后续动作用 {变量名} 引用。可用函数：get_var(\"名\") 读变量、has_var(\"名\") 判存在、vars() 取整张变量表、set_var(\"名\", 值) 写变量、json_get(文本, \"a.b.0\") 从 JSON 取字段、trigger() 触发组合、shortcut_name() 快捷键名、print(值) 追加输出。最多执行 20 万条指令，并受动作超时与「停止执行」约束；语法与运行错误会标在脚本行号上。"),
      ),
    );
    body.append(vline);
  } else if (a.type === "wait_for") {
    const tline = el("div", "action-line");
    const tsel = el("select", "action-input") as HTMLSelectElement;
    for (const t of WAIT_TARGET_TYPES) tsel.append(new Option(t.label, t.value));
    tsel.value = a.target.kind;
    tsel.addEventListener("change", () => {
      a.target = newWaitTarget(tsel.value as WaitTarget["kind"]);
      rerender();
    });
    tline.append(el("span", "unit-hint", tr("等什么")), tsel);
    body.append(tline);

    if (a.target.kind === "file") {
      body.append(
        pathField(
          a.target.path,
          (v) => {
            if (a.target.kind === "file") a.target.path = v;
          },
          { file: true, dir: true, ph: tr("等这个路径出现（支持 {变量} 与 ~）") },
        ),
      );
    } else if (a.target.kind === "process") {
      const line = el("div", "action-line");
      const input = el("input", "action-input") as HTMLInputElement;
      input.type = "text";
      input.value = a.target.program;
      input.placeholder = "notepad.exe";
      input.addEventListener("input", () => {
        if (a.target.kind === "process") a.target.program = input.value.trim();
      });
      line.append(
        el("span", "unit-hint", tr("进程名")),
        input,
        helpIcon(
          tr("按可执行文件名匹配，忽略大小写。例：notepad.exe（Windows）/ python3（Linux、macOS）。"),
        ),
      );
      body.append(line);
    } else {
      const line = el("div", "action-line");
      const input = el("input", "action-input") as HTMLInputElement;
      input.type = "text";
      input.value = a.target.title;
      input.placeholder = tr("窗口标题包含的文字");
      input.addEventListener("input", () => {
        if (a.target.kind === "window") a.target.title = input.value.trim();
      });
      line.append(
        el("span", "unit-hint", tr("窗口标题")),
        input,
        helpIcon(
          tr("按窗口标题匹配（忽略大小写的子串 / 通配）。目前只有 Windows 能等窗口；其它平台执行时本步会明确报错。"),
        ),
      );
      body.append(line);
    }

    body.append(timeoutField(a.timeout_ms, (v) => (a.timeout_ms = v)));

    const iline = el("div", "action-line");
    const ival = el("input", "action-input") as HTMLInputElement;
    ival.type = "number";
    ival.min = "50";
    ival.step = "50";
    ival.value = String(a.interval_ms ?? 200);
    ival.addEventListener("input", () => {
      const n = Number(ival.value);
      if (!Number.isFinite(n) || n < 50) {
        showFieldError(ival, tr("请填不小于 50 的毫秒数"));
        return;
      }
      showFieldError(ival, "");
      a.interval_ms = Math.round(n);
    });
    iline.append(
      el("span", "unit-hint", tr("轮询间隔（毫秒）")),
      ival,
      helpIcon(tr("每隔多久检查一次（最小 50 毫秒，默认 200）。间隔太小会白白占 CPU，太大则命中后多等一会儿。")),
    );
    body.append(iline);

    const vline = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = tr("变量名（可选），留空则只记执行记录");
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    vline.append(
      el("span", "unit-hint", tr("命中后存为变量")),
      vname,
      helpIcon(
        tr("命中时写进该变量：文件 = 实际路径、进程 = 进程名、窗口 = 窗口标题。超时 = 本步失败，不写变量。"),
      ),
    );
    body.append(vline);

    body.append(
      el(
        "div",
        "unit-hint",
        tr(
          "超时留空 = 跟随设置页「动作超时」；0 = 一直等（仍可点「停止执行」）。超时算本步失败，按链的默认语义继续后面的动作。",
        ),
      ),
    );
  } else if (a.type === "keys") {
    // 从已保存的快捷键里挑触发组合：只收单个组合（序列 "F9 J K" 不能塞进一次按键注入）。
    const combos: { combo: string; label: string }[] = [];
    const seen = new Set<string>();
    for (const s of state.cfg.shortcuts) {
      const name = s.name?.trim();
      for (const t of s.triggers) {
        const combo = t.trim();
        if (!combo || combo.includes(" ") || seen.has(combo)) continue;
        seen.add(combo);
        combos.push({ combo, label: name ? `${name}（${combo}）` : combo });
      }
    }
    const line = el("div", "action-line");
    const mode = el("select", "action-input") as HTMLSelectElement;
    for (const [value, label] of [
      ["tap", tr("点按（按下后松开）")],
      ["press", tr("按住（保持到链结束）")],
      ["release", tr("抬起")],
    ] as [KeysMode, string][]) {
      const o = el("option") as HTMLOptionElement;
      o.value = value;
      o.textContent = label;
      mode.append(o);
    }
    mode.value = a.mode ?? "tap";
    mode.addEventListener("change", () => {
      a.mode = mode.value as KeysMode;
    });
    const keys = el("input", "action-input") as HTMLInputElement;
    keys.type = "text";
    keys.value = a.keys.join("+");
    keys.placeholder = tr("如 Ctrl+C");
    keys.addEventListener("input", () => {
      a.keys = keys.value
        .split("+")
        .map((x) => x.trim())
        .filter(Boolean);
    });
    if (combos.length) {
      const pick = el("select", "action-input") as HTMLSelectElement;
      const none = el("option") as HTMLOptionElement;
      none.value = "";
      none.textContent = tr("从已创建的快捷键选择…");
      pick.append(none);
      for (const c of combos) {
        const o = el("option") as HTMLOptionElement;
        o.value = c.combo;
        o.textContent = c.label;
        pick.append(o);
      }
      if (combos.some((c) => c.combo === keys.value)) pick.value = keys.value;
      pick.addEventListener("change", () => {
        if (!pick.value) return;
        keys.value = pick.value;
        a.keys = pick.value.split("+").map((x) => x.trim()).filter(Boolean);
      });
      body.append(pick);
    }
    const cap = el("button", undefined, tr("录入"));
    cap.addEventListener("click", (e) => {
      const btn = e.currentTarget as HTMLButtonElement;
      startCapture((combo) => {
        a.keys = combo.split("+");
        keys.value = combo;
      }, btn);
    });
    line.append(keys, cap);
    body.append(mode, line);
    // 整段按键录制挂在「按键组合」这一步上：录到的组合键与间隔替换掉当前这一条。
    const recLine = el("div", "action-line");
    recLine.append(keyRecordButton(rerender, ctx));
    body.append(recLine);
  } else if (a.type === "send_to") {
    // 旧类型兜底（加载时已迁移成 `system.send_to`），字段与系统操作里那一项共用。
    body.append(...sendToFields(a, rerender));
  } else if (a.type === "mouse") {
    const op = a.op;
    const line = el("div", "action-line");
    if (op.op === "move_to") {
      const x = el("input", "action-input") as HTMLInputElement;
      x.type = "number";
      x.value = String(op.x);
      x.placeholder = tr("横坐标");
      x.addEventListener("input", () => {
        op.x = intIn(x.value, -100000, 0, 100000);
      });
      const y = el("input", "action-input") as HTMLInputElement;
      y.type = "number";
      y.value = String(op.y);
      y.placeholder = tr("纵坐标");
      y.addEventListener("input", () => {
        op.y = intIn(y.value, -100000, 0, 100000);
      });
      line.append(
        x,
        y,
        el("span", "unit-hint", tr("绝对屏幕坐标，主屏左上角为 (0,0)，多屏可为负")),
      );
    } else if (op.op === "move_to_origin") {
      line.append(el("span", "unit-hint", tr("移动到本次手势的起点（仅手势触发可用）")));
    } else if (op.op === "move" || op.op === "scroll") {
      const dx = el("input", "action-input") as HTMLInputElement;
      dx.type = "number";
      dx.value = String(op.dx);
      dx.placeholder = tr("横向");
      dx.addEventListener("input", () => {
        op.dx = intIn(dx.value, -10000, 0, 10000);
      });
      const dy = el("input", "action-input") as HTMLInputElement;
      dy.type = "number";
      dy.value = String(op.dy);
      dy.placeholder = tr("纵向");
      dy.addEventListener("input", () => {
        op.dy = intIn(dy.value, -10000, 0, 10000);
      });
      line.append(
        dx,
        dy,
        el(
          "span",
          "unit-hint",
          op.op === "move"
            ? tr("像素，相对当前位置（右下为正）")
            : tr("格数：纵向正数上滚 / 负数下滚，横向正数右滚"),
        ),
      );
    } else {
      const sel = el("select", "action-input") as HTMLSelectElement;
      for (const b of ["left", "right", "middle", "back", "forward"] as MouseButton[]) {
        const o = el("option") as HTMLOptionElement;
        o.value = b;
        o.textContent = MOUSE_BUTTON_LABELS[b];
        sel.append(o);
      }
      sel.value = op.button;
      sel.addEventListener("change", () => {
        op.button = sel.value as MouseButton;
      });
      line.append(el("span", "unit-hint", tr("在当前光标位置点击")), sel);
    }
    body.append(line);
    body.append(mouseRecordButton(rerender, ctx));
  } else if (a.type === "clipboard") {
    const op = a.operation;
    if (op.op === "get_clipboard") {
      const line = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = tr("变量名，如 clip");
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      line.append(
        el("span", "unit-hint", tr("变量名")),
        vname,
        helpIcon(
          tr("把剪贴板里的文本存进该变量；剪贴板为空或只有图片时这一步会失败。后续动作可用 {clip} 引用。"),
        ),
      );
      body.append(line);
    } else {
      const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
      ta.value = op.text;
      ta.placeholder =
        op.op === "set_clipboard"
          ? tr("要写入剪贴板的文本（支持 {变量名} 占位符；留空 = 清空剪贴板）")
          : tr("要临时粘贴的文本（支持 {变量名} 占位符）");
      ta.addEventListener("input", () => {
        op.text = ta.value;
      });
      body.append(ta);
      body.append(
        el(
          "div",
          "unit-hint",
          op.op === "set_clipboard"
            ? tr("写入系统剪贴板，不会粘贴；之后用 Ctrl+V / ⌘V 或「粘贴」动作使用。")
            : tr("写入剪贴板后立即粘贴到当前焦点，再恢复原来的剪贴板文本（图片不恢复；图片剪贴板列为后续）。"),
        ),
      );
    }
  } else if (a.type === "transform") {
    const ta = el("textarea", "action-textarea") as HTMLTextAreaElement;
    ta.value = a.input;
    ta.placeholder = tr("要处理的文本（支持 {变量名} 占位符）……");
    ta.addEventListener("input", () => {
      a.input = ta.value;
    });
    body.append(ta);

    const line = el("div", "action-line");
    const vname = el("input", "action-input") as HTMLInputElement;
    vname.type = "text";
    vname.value = a.var;
    vname.placeholder = tr("变量名，如 result");
    vname.addEventListener("input", () => {
      a.var = vname.value.trim();
    });
    line.append(
      el("span", "unit-hint", tr("结果变量")),
      vname,
      helpIcon(tr("结果存进该变量，后续动作用 {result} 引用。变量与「直接注入」至少填一项。")),
    );
    body.append(line);

    const injectLine = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = !!a.inject;
    cb.addEventListener("change", () => {
      a.inject = cb.checked;
    });
    injectLine.append(cb, el("span", undefined, tr("直接注入结果到当前焦点")));
    body.append(injectLine);

    body.append(
      el(
        "div",
        "unit-hint",
        tr("base64 解码会忽略换行与空白；解码结果必须是 UTF-8 文本。非法输入（坏 base64、残缺的 %XX）会明确报错，不会静默丢内容。"),
      ),
    );
  } else if (a.type === "pause_ms") {
    const line = el("div", "action-line");
    const num = el("input", "action-input") as HTMLInputElement;
    num.type = "number";
    num.min = "0";
    num.value = String(a.ms);
    num.placeholder = tr("如 500");
    num.addEventListener("input", () => {
      a.ms = intIn(num.value, 0, 0);
    });
    line.append(num, el("span", "unit-hint", tr("毫秒 (ms)")));
    body.append(line);
  } else if (a.type === "os") {
    const op = a.operation;
    if (op.op === "copy" || op.op === "cut") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, dir: true, label: tr("原始路径") }),
      );
      body.append(
        pathField(op.dest, (v) => (op.dest = v), { file: true, dir: true, label: tr("目标路径") }),
      );
    } else if (op.op === "zip") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, dir: true, label: tr("待压缩路径") }),
      );
      body.append(
        pathField(op.dest, (v) => (op.dest = v), { file: true, dir: true, label: tr("压缩文件路径") }),
      );
    } else if (op.op === "unzip") {
      body.append(
        pathField(op.source, (v) => (op.source = v), { file: true, label: tr("压缩包路径") }),
      );
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true, label: tr("解压到文件夹") }));
    } else if (op.op === "paste") {
      body.append(pathField(op.dest, (v) => (op.dest = v), { dir: true, label: tr("目标文件夹") }));
    } else if (op.op === "delete") {
      body.append(
        pathField(op.path, (v) => (op.path = v), { file: true, dir: true, label: tr("待删除路径") }),
      );
    } else if (op.op === "new_file") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, label: tr("新建文件路径") }));
    } else if (op.op === "new_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true, label: tr("新建文件夹路径") }));
    } else if (op.op === "open_folder") {
      body.append(pathField(op.path, (v) => (op.path = v), { dir: true, label: tr("打开文件夹") }));
    } else if (op.op === "open_file") {
      body.append(pathField(op.path, (v) => (op.path = v), { file: true, label: tr("打开文件") }));
      body.append(
        el("div", "unit-hint", tr("用电脑里设置了关联的默认程序打开这个文件（按扩展名，如 .pdf 用 PDF 阅读器、.docx 用 Word）。")),
      );
    } else if (op.op === "reveal") {
      body.append(
        pathField(op.path, (v) => (op.path = v), { file: true, dir: true, label: tr("要定位的文件或文件夹") }),
      );
      body.append(el("div", "unit-hint", tr("仅 Windows 支持：在资源管理器里打开所在文件夹并选中它。")));
    } else if (op.op === "get_file_props") {
      body.append(
        pathField(op.path, (v) => (op.path = v), { file: true, dir: true, label: tr("文件路径") }),
      );
      const line = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = tr("变量名，如 file");
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      line.append(
        el("span", "unit-hint", tr("变量名")),
        vname,
        helpIcon(
          tr("把这个文件的属性写进该变量：{file} 得完整路径，还能取 .name / .dir / .ext / .size 等字段（字段清单见「帮助」页）。"),
        ),
      );
      body.append(line);
    }
  } else if (a.type === "system") {
    const op = a.operation;
    if (op.op === "switch_desktop") {
      const line = el("div", "action-line");
      const sel = el("select", "action-input") as HTMLSelectElement;
      for (const d of DESKTOP_DIRECTIONS) {
        const o = el("option") as HTMLOptionElement;
        o.value = d.value;
        o.textContent = d.label;
        sel.append(o);
      }
      sel.value = op.direction;
      sel.addEventListener("change", () => {
        op.direction = sel.value as DesktopDirection;
      });
      line.append(
        el("span", "unit-hint", tr("方向")),
        sel,
        helpIcon(
          tr("按各系统的默认键位切换：Windows 注入 Win+Ctrl+←/→，macOS 注入 ⌃←/→，Linux 注入 Super+PageUp/PageDown。系统里改过这些快捷键的话，注入的就不生效了，那种情况改用「按键组合」动作写自己的键位。"),
        ),
      );
      body.append(line);

      const hint = el("div", "unit-hint");
      hint.textContent =
        tr("切换虚拟桌面（不是切换显示器）。Windows：Win+Ctrl+←/→；macOS：⌃←/→；Linux（GNOME）：Super+PageUp/PageDown。");
      body.append(hint);
    } else if (op.op === "empty_recycle_bin") {
      body.append(
        el("div", "unit-hint", tr("永久清空回收站，不弹系统确认。仅 Windows 支持；其它平台会标平台冲突并在执行时报错。")),
      );
    } else if (op.op === "monitor_off") {
      body.append(
        el("div", "unit-hint", tr("立刻关闭显示器；动一下鼠标或按任意键就会亮回来。仅 Windows 支持。")),
      );
    } else if (op.op === "toggle_desktop_icons") {
      body.append(
        el("div", "unit-hint", tr("显示 / 隐藏桌面图标（等价于右键桌面 → 查看 → 显示桌面图标）。仅 Windows 支持。")),
      );
    } else if (op.op === "send_to") {
      // 原独立动作「后台发送」：类型仍是操作系统，只是子操作换了。
      body.append(...sendToFields(op, rerender));
    } else if (op.op === "brightness") {
      if (op.operation !== "restore") {
        const line = el("div", "action-line");
        const label = el("span", "unit-label", tr("步长"));
        const step = el("input", "action-input") as HTMLInputElement;
        step.type = "number";
        step.min = "1";
        step.max = "100";
        step.value = String(op.step);
        step.addEventListener("input", () => {
          op.step = intIn(step.value, 1, 10, 100);
        });
        line.append(label, step);
        body.append(line);
      }
      body.append(
        el("div", "unit-hint", tr("只调主显示器（DDC/CI，VCP 0x10）。「恢复」用本次运行第一次调节前缓存的原始亮度；显示器不支持时执行会明确报错。目前仅 Windows 支持。")),
      );
    }
  } else if (a.type === "window") {
    const hint = el("div", "unit-hint");
    hint.textContent = tr("作用对象是触发那一刻的前台窗口，不会激活或切换窗口。目前仅 Windows 支持，其它平台会标平台冲突并在执行时报错。");
    body.append(hint);
  } else if (a.type === "tool") {
    body.append(
      el("div", "unit-hint", tr("外部工具能力目前只做「探测到在跑就出现在动作里」：执行会明确报「尚未接入」，不会真的调用它。")),
    );
  } else if (a.type === "capture") {
    body.append(
      el("div", "unit-hint", tr("截图动作已移除（执行会报错）：现在改用鼠标手势——按住右键绕一圈，右键别松再点一下左键确认。")),
    );
  } else if (a.type === "brightness") {
    if (a.operation !== "restore") {
      const line = el("div", "action-line");
      const label = el("span", "unit-label", tr("步长"));
      const step = el("input", "action-input") as HTMLInputElement;
      step.type = "number";
      step.min = "1";
      step.max = "100";
      step.value = String(a.step);
      step.addEventListener("input", () => {
        a.step = intIn(step.value, 1, 10, 100);
      });
      line.append(label, step);
      body.append(line);
    }
    body.append(
      el("div", "unit-hint", tr("只调主显示器（DDC/CI，VCP 0x10）。「恢复」用本次运行第一次调节前缓存的原始亮度；显示器不支持时执行会明确报错。目前仅 Windows 支持。")),
    );
  } else if (a.type === "app") {
    const op = a.operation;
    if (op.op === "current_window") {
      // 原独立动作「窗口操作」：类型仍是应用操作，目标 = 触发那一刻的前台窗口。
      body.append(
        el("div", "unit-hint", tr("作用对象是触发那一刻的前台窗口，不会激活或切换窗口，也不按程序名匹配。目前仅 Windows 支持，其它平台会标平台冲突并在执行时报错。")),
      );
    } else if (op.op === "launch" || op.op === "restart") {
      const progWrap = el("div", "action-line");
      const prog = el("input", "action-input") as HTMLInputElement;
      prog.type = "text";
      prog.value = op.program;
      prog.placeholder = tr("程序路径/名，如 C:\\Windows\\notepad.exe 或 notepad.exe");
      prog.addEventListener("input", () => {
        op.program = prog.value;
      });
      const browse = el("button", undefined, tr("浏览…"));
      browse.addEventListener("click", () => void pickFile(prog));
      progWrap.append(prog, browse);
      body.append(progWrap);

      const args = el("input", "action-input") as HTMLInputElement;
      args.type = "text";
      args.value = op.args.join(" ");
      args.placeholder = tr("参数（可选，空格分隔）");
      args.addEventListener("input", () => {
        op.args = args.value.split(/\s+/).filter(Boolean);
      });
      body.append(args);
    } else if (
      op.op === "close" ||
      op.op === "maximize" ||
      op.op === "minimize" ||
      op.op === "activate"
    ) {
      const line = el("div", "action-line");
      const name = el("input", "action-input") as HTMLInputElement;
      name.type = "text";
      name.value = op.program;
      name.placeholder = tr("程序名/路径，如 notepad.exe");
      name.addEventListener("input", () => {
        op.program = name.value.trim();
      });
      line.append(name);
      body.append(line);
      if (op.op !== "close") {
        body.append(
          el(
            "div",
            "unit-hint",
            op.op === "activate"
              ? tr("按程序名找到它的主窗口并放到前台（不会启动程序）。")
              : tr("按程序名找到它的主窗口并调整窗口状态。Linux 需要 wmctrl（仅 X11 会话）。"),
          ),
        );
      }
    } else if (op.op === "status") {
      const line = el("div", "action-line");
      const name = el("input", "action-input") as HTMLInputElement;
      name.type = "text";
      name.value = op.program;
      name.placeholder = tr("程序名/路径，如 notepad.exe");
      name.addEventListener("input", () => {
        op.program = name.value.trim();
      });
      line.append(name);
      body.append(line);

      const vline = el("div", "action-line");
      const vname = el("input", "action-input") as HTMLInputElement;
      vname.type = "text";
      vname.value = op.var;
      vname.placeholder = tr("变量名，如 app");
      vname.addEventListener("input", () => {
        op.var = vname.value.trim();
      });
      vline.append(
        el("span", "unit-hint", tr("变量名")),
        vname,
        helpIcon(
          tr("写入布尔变量：程序在运行则为 true，否则 false。引用 {变量名} 得到 true/false，可配合「条件判断」动作使用。"),
        ),
      );
      body.append(vline);

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
        el("span", "unit-hint", tr("未运行时重试")),
        retries,
        el("span", "unit-hint", tr("次 · 间隔")),
        ival,
        el("span", "unit-hint", "ms"),
        helpIcon(
          tr("程序未运行（变量为 false）时，按「重试次数 × 重试间隔」轮询重查，等程序起来；0 次 = 只查一次。"),
        ),
      );
      body.append(rline);
    }
  } else if (a.type === "open_url") {
    const line = el("div", "action-line");
    const url = el("input", "action-input") as HTMLInputElement;
    url.type = "text";
    url.value = a.url;
    url.placeholder = tr("网址，如 example.com（不写 https:// 也行；支持 {变量名} 占位符）");
    url.addEventListener("input", () => {
      a.url = url.value.trim();
      showFieldError(url, urlError(url.value));
    });
    line.append(url);
    body.append(line);
    body.append(
      el("div", "unit-hint", tr("用系统默认浏览器打开该网页。没写协议头的会自动补 https://。")),
    );
  } else if (a.type === "if") {
    const cond = a.condition;
    body.append(conditionFields(cond));

    body.append(el("div", "branch-label then", tr("✓ 满足条件时执行")));
    const thenList = el("div", "action-branch branch-then");
    (thenList as unknown as { _dropList?: Action[] })._dropList = a.then;
    renderActionList(thenList, a.then, rerender);
    const addThen = el("button", "add-inline", tr("＋ 添加动作"));
    addThen.addEventListener("click", () => {
      a.then.push(newAction("text"));
      rerender();
    });
    body.append(thenList, addThen);

    body.append(el("div", "branch-label else", tr("✗ 否则执行（可空）")));
    const elseList = el("div", "action-branch branch-else");
    (elseList as unknown as { _dropList?: Action[] })._dropList = a.otherwise;
    renderActionList(elseList, a.otherwise, rerender);
    const addElse = el("button", "add-inline", tr("＋ 添加动作"));
    addElse.addEventListener("click", () => {
      a.otherwise.push(newAction("text"));
      rerender();
    });
    body.append(elseList, addElse);
  } else if (a.type === "parallel") {
    // 成员列表就是一条可落列表（`data-action-list` + `_dropList` 由 renderActionList 统一挂上）。
    body.append(el("div", "branch-label parallel", tr("⇉ 同时执行（全部成员结束后继续下一步）")));
    const memberList = el("div", "action-branch branch-parallel");
    (memberList as unknown as { _dropList?: Action[] })._dropList = a.actions;
    renderActionList(memberList, a.actions, rerender);
    const addMember = el("button", "add-inline", tr("＋ 添加动作"));
    addMember.addEventListener("click", () => {
      a.actions.push(newAction("text"));
      rerender();
    });
    body.append(memberList, addMember);
  } else if (a.type === "group") {
    body.append(
      el("div", "branch-label group", tr("▣ 按顺序执行（分组只做整理，不改变执行结果）")),
    );
    const memberList = el("div", "action-branch branch-group");
    (memberList as unknown as { _dropList?: Action[] })._dropList = a.actions;
    renderActionList(memberList, a.actions, rerender);
    const addMember = el("button", "add-inline", tr("＋ 添加动作"));
    addMember.addEventListener("click", () => {
      a.actions.push(newAction("text"));
      rerender();
    });
    body.append(memberList, addMember);
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
  if (/[<>|?*"]/.test(bare)) return tr('路径含不允许的字符（< > | ? * "）');
  if (/[\u0000-\u001f]/.test(bare)) return tr("路径含不可见控制字符");
  return "";
}

// 网址合法性：非空、http(s) 开头；含 {变量} 占位符的拼完才知道形态，放行。
// 网址只拦空值：格式完全放开（localhost:3000、www.xxx.com 都行），没写协议头的
// 由后端 `open_url` 补 `https://`。变量占位符同样原样放行。
function urlError(v: string): string {
  return v.trim() ? "" : tr("网址不能为空");
}

// 文本扩展触发词（与后端 TextExpansion::validate 同口径）：非空、无空白字符。
function expansionTriggerError(v: string): string {
  if (!v) return tr("触发词不能为空");
  if (/\s/.test(v)) return tr("触发词不能含空格 / 回车 / Tab");
  return "";
}

// 路径输入行：一个文本框 + 可选的「文件…」「目录…」浏览按钮。
// ---- 命令 / 脚本共用的执行字段（CS-01~CS-05） ----

// 工作目录：空 = 继承 Kada 进程工作目录；支持 {变量} 与 ~；目录不存在执行时才报错
// （移动硬盘 / 网络盘可能稍后才挂上，保存不拦）。
function workingDirField(value: string | undefined, onChange: (v: string) => void): HTMLElement {
  return pathField(value ?? "", onChange, {
    dir: true,
    ph: tr("工作目录（可空 = 跟随 Kada；支持 {变量} 与 ~）"),
  });
}

// 每动作超时（秒）：空 = 跟随设置页的全局「动作超时」；0 = 不限时；只覆盖本步。
function timeoutField(
  timeoutMs: number | null | undefined,
  onChange: (v: number | null) => void,
): HTMLElement {
  const line = el("div", "action-line");
  const input = el("input", "action-input") as HTMLInputElement;
  input.type = "number";
  input.min = "0";
  input.step = "any";
  input.value = timeoutMs == null ? "" : String(timeoutMs / 1000);
  input.addEventListener("input", () => {
    const raw = input.value.trim();
    if (!raw) {
      showFieldError(input, "");
      onChange(null);
      return;
    }
    const n = Number(raw);
    if (!Number.isFinite(n) || n < 0) {
      showFieldError(input, tr("请填不小于 0 的秒数（留空 = 跟随设置）"));
      return;
    }
    showFieldError(input, "");
    onChange(Math.round(n * 1000));
  });
  line.append(
    el("span", "unit-hint", tr("超时（秒）")),
    input,
    helpIcon(
      tr(
        "本步最长等多少秒：留空 = 用设置页「动作超时」的全局值；0 = 不限时；只覆盖这一步，不改全局。超时会把命令连同它启动的子进程一起结束。",
      ),
    ),
  );
  return line;
}

// 成功退出码：逗号分隔；空 = 只有 0 算成功。robocopy 的 1（有文件复制成功）、
// 安装器的 3010（需要重启）这类「非零但成功」的退出码靠它放行。
function okExitCodesField(
  codes: number[] | undefined,
  onChange: (v: number[]) => void,
): HTMLElement {
  const line = el("div", "action-line");
  const input = el("input", "action-input") as HTMLInputElement;
  input.type = "text";
  input.value = (codes ?? []).join(", ");
  input.placeholder = "0";
  input.addEventListener("input", () => {
    const raw = input.value.trim();
    if (!raw) {
      showFieldError(input, "");
      onChange([]);
      return;
    }
    const parts = raw.split(/[,，;；\s]+/).filter((p) => p.length > 0);
    const out: number[] = [];
    for (const p of parts) {
      const n = Number(p);
      if (!Number.isInteger(n) || n < -2147483648 || n > 2147483647) {
        showFieldError(input, tr("退出码要是整数（-2147483648 ~ 2147483647），用逗号分隔"));
        return;
      }
      out.push(n);
    }
    showFieldError(input, "");
    onChange(out);
  });
  line.append(
    el("span", "unit-hint", tr("成功退出码")),
    input,
    helpIcon(
      tr(
        "哪些退出码算成功（逗号分隔，空 = 只有 0）。例：robocopy 复制了文件时退出 1，填「0, 1」；MSI 安装完要求重启时退出 3010，填「0, 3010」。被超时 / 手动停止强杀的进程没有退出码，仍算失败。",
      ),
    ),
  );
  return line;
}

// 命令 / 脚本共用的运行开关（CS-11~CS-13、CS-17）：结果弹窗三态、重复触发、等待、失败即停、隐藏控制台。
// 返回的是若干行（取消「等待」时多一行醒目警告），调用方直接 `body.append(...)`。
function commandRunControlFields(
  a: {
    show_output: ShowOutput;
    wait?: boolean;
    on_error?: "continue" | "stop";
    hide_window?: boolean;
    if_running?: IfRunning;
  },
  rerender: () => void,
): HTMLElement[] {
  const popupLine = el("div", "action-line");
  const sel = el("select", "action-input") as HTMLSelectElement;
  sel.append(
    new Option(tr("从不弹窗（只亮红点）"), "never"),
    new Option(tr("只在失败时弹"), "on_error"),
    new Option(tr("每次都弹"), "always"),
  );
  const current =
    a.show_output === true ? "always" : a.show_output === false ? "never" : a.show_output;
  sel.value = current;
  sel.addEventListener("change", () => {
    a.show_output = sel.value as ShowOutput;
  });
  popupLine.append(
    el("span", "unit-hint", tr("结果弹窗")),
    sel,
    helpIcon(
      tr(
        "结果消息始终会进消息中心；这里只决定要不要额外弹窗。「只在失败时弹」按本步真实成败判断（退出码表与超时 / 中止都算）。",
      ),
    ),
  );

  const runLine = el("div", "action-line");
  const runSel = el("select", "action-input") as HTMLSelectElement;
  runSel.append(
    new Option(tr("照常再起一份"), "run"),
    new Option(tr("跳过，不执行"), "skip"),
    new Option(tr("排队等上一次结束"), "queue"),
  );
  runSel.value = a.if_running ?? "run";
  runSel.disabled = a.wait === false;
  runSel.addEventListener("change", () => {
    a.if_running = runSel.value as IfRunning;
  });
  runLine.append(
    el("span", "unit-hint", tr("已在运行时")),
    runSel,
    helpIcon(
      tr(
        "同一条命令 / 脚本上一次还没跑完，又触发了一次时怎么办：「跳过」只记一条步骤不执行；「排队」等上一次结束后再跑（等待期间可点「停止执行」）。跨快捷键生效，按动作内容 + 触发名识别；「启动后不管」的步骤不参与登记。",
      ),
    ),
  );

  const flags = el("div", "action-checks");
  const waitLabel = el("label", "action-check") as HTMLLabelElement;
  const wcb = el("input", "toggle") as HTMLInputElement;
  wcb.type = "checkbox";
  wcb.checked = a.wait !== false;
  wcb.addEventListener("change", () => {
    a.wait = wcb.checked;
    rerender();
  });
  waitLabel.append(wcb, el("span", undefined, tr("等它跑完再执行下一步")));

  const stopLabel = el("label", "action-check") as HTMLLabelElement;
  const scb = el("input", "toggle") as HTMLInputElement;
  scb.type = "checkbox";
  scb.checked = a.on_error === "stop";
  stopLabel.title = tr("默认不勾：这一步失败后仍继续执行后面的动作（和以前一样）");
  scb.addEventListener("change", () => {
    a.on_error = scb.checked ? "stop" : "continue";
  });
  stopLabel.append(scb, el("span", undefined, tr("失败时停止后续动作")));

  const hidden = el("label", "action-check") as HTMLLabelElement;
  const hcb = el("input", "toggle") as HTMLInputElement;
  hcb.type = "checkbox";
  hcb.checked = !!a.hide_window;
  hidden.title = tr("勾上后执行时不弹出黑色控制台窗口（Windows 专有，其余平台无效果）");
  hcb.addEventListener("change", () => {
    a.hide_window = hcb.checked;
  });
  hidden.append(hcb, el("span", undefined, tr("隐藏控制台窗口")));

  flags.append(waitLabel, stopLabel, hidden);
  const out: HTMLElement[] = [popupLine, runLine, flags];
  if (a.wait === false) {
    out.push(
      el(
        "div",
        "unit-hint unit-hint-wrap action-warning",
        tr(
          "「启动后不管」：不等待、不捕获输出、也不受「停止执行」控制——命令会自己跑完。变量不会写入，结果卡只显示已启动。",
        ),
      ),
    );
  }
  return out;
}

// 一行一个参数的 textarea（脚本参数 / 解释器参数）：脚本参数里带空格太常见
// （C:\My Files），空格拆分会把它拆坏，所以一行一个、不做引号解析。
function linesField(
  label: string,
  lines: string[] | undefined,
  onChange: (v: string[]) => void,
  tip: string,
): HTMLElement {
  const line = el("div", "action-line");
  const ta = el("textarea", "action-textarea grow") as HTMLTextAreaElement;
  ta.value = (lines ?? []).join("\n");
  ta.rows = 2;
  ta.spellcheck = false;
  ta.addEventListener("input", () => {
    onChange(
      ta.value
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s.length > 0),
    );
  });
  line.append(el("span", "unit-hint", label), ta, helpIcon(tip));
  return line;
}

function pathField(
  value: string,
  onInput: (v: string) => void,
  opts: { file?: boolean; dir?: boolean; label?: string; ph?: string } = { file: true },
): HTMLElement {
  const line = el("div", "action-line");
  const input = el("input", "action-input") as HTMLInputElement;
  input.type = "text";
  input.value = value;
  input.placeholder = opts.ph ?? (opts.label ? trt`${opts.label}（可用 {变量名}）` : tr("路径（可用 {变量名} 占位符）"));
  // 全部路径字段都从这里过——校验放共用口，一处管住所有调用方（条件/复制/脚本…）。
  input.addEventListener("input", () => {
    onInput(input.value);
    showFieldError(input, pathError(input.value));
  });
  line.append(input);
  if (opts.file) {
    const bf = el("button", undefined, tr("文件…"));
    bf.addEventListener("click", () => void pickFile(input));
    line.append(bf);
  }
  if (opts.dir) {
    const bd = el("button", undefined, tr("文件夹…"));
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
// 合并列表里只显示一个可点击、可消除的气泡（冲突清单不常驻列表）；冲突详情常驻「消息 → 冲突」标签。
function renderConflictBubble() {
  const box = document.getElementById("conflict-list")!;
  box.replaceChildren();
  // 「配了但总开关关着」也各说一声：鼠标操作与触摸板手势是**两枚独立开关**，
  // 关着时这些类**整类不显示**，用户在列表里看不到自己的条目，只会觉得「配了没反应」
  // ——这里把原因分开摆出来（点哪条就开哪枚，别用一枚开关去猜另一类）。
  const mouseOffCount = (state.cfg.gestures ?? []).length;
  const touchpadOffCount = (state.cfg.touchpad_gestures ?? []).length;
  const inList = isListSection(state.section);
  const mouseOff = inList && mouseOffCount > 0 && !mouseFeaturesEnabled();
  const touchpadOff = inList && touchpadOffCount > 0 && !touchpadFeaturesEnabled();
  const hasConflict = inList && state.conflicts.length > 0 && !state.conflictBubbleDismissed;
  const show = hasConflict || mouseOff || touchpadOff;
  box.classList.toggle("hidden", !show);
  if (!show) return;
  if (mouseOff) {
    const b = el("div", "conflict-bubble");
    b.addEventListener("click", () => void enableExperimentalFeatures());
    b.append(
      el(
        "span",
        "conflict-bubble-text",
        trt`⚠ 已配置 ${mouseOffCount} 条鼠标操作条目，但「鼠标操作」总开关未开启 → 不会触发`,
      ),
      el("span", "conflict-bubble-hint", tr("点击开启")),
    );
    box.append(b);
  }
  if (touchpadOff) {
    const b = el("div", "conflict-bubble");
    b.addEventListener("click", () => void enableTouchpadFeatures());
    b.append(
      el(
        "span",
        "conflict-bubble-text",
        trt`⚠ 已配置 ${touchpadOffCount} 条触摸板手势，但「触摸板手势」总开关未开启 → 不会触发`,
      ),
      el("span", "conflict-bubble-hint", tr("点击开启")),
    );
    box.append(b);
  }
  if (!hasConflict) return;
  const { errors, warns, caps } = splitConflicts(state.conflicts);
  const bits: string[] = [];
  if (errors.length || warns.length)
    bits.push(
      trt`${errors.length + warns.length} 处冲突${errors.length ? trt`（${errors.length} 处需处理）` : ""}`,
    );
  if (caps.length) bits.push(trt`${caps.length} 条当前环境用不了`);
  const bubble = el("div", "conflict-bubble");
  bubble.addEventListener("click", openConflictView);
  bubble.append(
    el("span", "conflict-bubble-text", `⚠ ${bits.join(" · ")}`),
    el("span", "conflict-bubble-hint", tr("点击查看")),
  );
  const close = el("button", "conflict-bubble-close", "×") as HTMLButtonElement;
  close.title = tr("消除提醒");
  close.addEventListener("click", (e) => {
    e.stopPropagation();
    state.conflictBubbleDismissed = true;
    renderConflictBubble();
  });
  bubble.append(close);
  box.append(bubble);
}

// 打开「鼠标操作」总开关并立刻落盘：手势这类功能默认关着，用户配好了却没打开就白配。
async function enableExperimentalFeatures() {
  state.cfg.settings.experimental = true;
  const box = document.getElementById("set-experimental") as HTMLInputElement | null;
  if (box) box.checked = true;
  syncMouseOpts();
  await save();
}

// 打开「触摸板手势」开关并立刻落盘（与上面那枚分开：两类各有自己的闸门）。
async function enableTouchpadFeatures() {
  state.cfg.settings.touchpad = true;
  const box = document.getElementById("set-touchpad") as HTMLInputElement | null;
  if (box) box.checked = true;
  syncTouchpadOpts();
  await save();
}

// 冲突归属的条目（「是哪个快捷键」标题 + 点击跳转编辑页用）。
// 后端 `Conflict` 只带名字：先按名字命中快捷键；没名字时按消息里 `「…」` 引用的触发键 /
// 改键来源兜底定位（与 `entryConflicted` 同一套「靠文案认条目」的口径，UI 与后端同包发布）。
function conflictOwner(c: Conflict): { kind: ListKind; index: number } | null {
  if (c.name) {
    const i = state.cfg.shortcuts.findIndex((s) => (s.name ?? "") === c.name);
    if (i >= 0) return { kind: "shortcut", index: i };
  }
  // 后端文案仍是中文（壳层消息不参与界面 i18n），所以这里按中文字面量判断，别套 tr()。
  const remapMsg = c.message.startsWith("改键"); // i18n:ignore
  for (const q of [...c.message.matchAll(/「([^」]+)」/g)].map((m) => m[1])) {
    if (!remapMsg) {
      const si = state.cfg.shortcuts.findIndex((s) => s.triggers.includes(q));
      if (si >= 0) return { kind: "shortcut", index: si };
    }
    const ri = state.cfg.remaps.findIndex((r) => r.from === q.split(" →")[0].trim());
    if (ri >= 0) return { kind: "remap", index: ri };
  }
  return null;
}

function entryLabel(ref: { kind: ListKind; index: number }): string {
  if (ref.kind === "shortcut" && state.cfg.shortcuts[ref.index])
    return state.cfg.shortcuts[ref.index].name?.trim() || tr("未命名快捷键");
  if (ref.kind === "remap" && state.cfg.remaps[ref.index])
    return state.cfg.remaps[ref.index].name?.trim() || tr("未命名改键");
  return "";
}

// 冲突详情（常驻「消息 → 冲突」标签）：显示是哪个快捷键 + 冲突说明。
export function renderConflictList() {
  const box = document.getElementById("msg-conflict-list")!;
  box.replaceChildren();
  if (state.conflicts.length === 0) {
    box.append(el("div", "detail-empty", tr("暂无冲突")));
    return;
  }
  box.append(el("div", "conflict-head", trt`⚠ 冲突提醒（${state.conflicts.length} 处）`));
  const { errors, warns, caps } = splitConflicts(state.conflicts);
  const groups: [string, Conflict[], string][] = [
    [tr("需处理"), errors, "error"],
    [tr("软冲突（遮蔽 / 重叠）"), warns, "warn"],
    [tr("平台 / 环境限制（配置合法，当前环境支持不了）"), caps, "platform"],
  ];
  for (const [title, items, kind] of groups) {
    if (!items.length) continue;
    box.append(el("div", "conflict-group", `${title}（${items.length}）`));
    for (const c of items) {
      const d = el("div", "conflict " + kind);
      const owner = conflictOwner(c);
      const label = owner ? entryLabel(owner) : c.name;
      if (label) {
        const head = el("div", "conflict-name", label);
        if (owner) {
          // 自己创建的快捷键/改键：点标题跳到它的编辑页。
          head.classList.add("conflict-link");
          head.title = tr("点击跳到编辑页");
          head.addEventListener("click", () => void requestOpenDetail(owner.index, false, owner.kind));
        }
        d.append(head);
      }
      d.append(el("div", "conflict-msg", c.message));
      box.append(d);
    }
  }
}

// 跳到「消息 → 冲突」标签查看冲突详情。
function openConflictView() {
  state.msgView = "conflicts";
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

// ---- 鼠标手势（规划 7.2-MG）----
// 手势的条目与编辑在「快捷键」页的「鼠标操作」分类里（与快捷键/改键/文本扩展并列），
// 设置页只留一个「鼠标操作」总开关：关掉 = 鼠标类条目不显示、行为不生效，配置保留。

// ---- 复制快捷菜单（EXT-13，已删除；保留只读编辑器函数供旧配置兜底）----
// 条目 = 一个 Action（可用「分组」串多步），编辑器内嵌在设置页；菜单浮窗只负责展示与点击，
// 动作执行始终在壳层（与快捷键同一条 fire 链路）。
function renderClipboardMenuEditor() {
  const host = document.getElementById("clipboard-menu-editor");
  if (!host) return;
  host.replaceChildren();
  const rerender = () => renderClipboardMenuEditor();
  // 只读展示时不要顺手往 cfg 里塞一个默认字段（没碰过菜单的用户不该因此多一份配置）。
  const menu = state.cfg.settings.clipboard_menu ?? { enabled: false, double_copy_ms: 500, items: [] };
  host.append(menuItemsEditor(menu.items, 1, rerender));
}

function menuItemsEditor(items: MenuEntry[], depth: number, rerender: () => void): HTMLElement {
  const list = el("div", "menu-editor-list");
  if (!items.length) {
    list.append(el("div", "unit-hint", tr("还没有条目：点下面的「＋ 动作」开始添加。")));
  }
  items.forEach((_, i) => list.append(menuEntryEditor(items, i, depth, rerender)));
  return list;
}

function moveMenuEntry(items: MenuEntry[], from: number, to: number) {
  if (to < 0 || to >= items.length || from === to) return;
  const [moved] = items.splice(from, 1);
  items.splice(to, 0, moved);
}

function menuEntryEditor(
  items: MenuEntry[],
  i: number,
  depth: number,
  rerender: () => void,
): HTMLElement {
  const item = items[i];
  const box = el("div", "menu-editor-entry");
  // 动作字段的改动在失焦时落盘（与设置页其它字段同一口径）。
  box.addEventListener("change", () => void save());

  const head = el("div", "menu-editor-head");
  if (item.kind === "separator") {
    head.append(el("span", "menu-editor-kind", tr("分割线")));
  } else {
    const label = el("input", "action-input menu-editor-label") as HTMLInputElement;
    label.type = "text";
    label.value = item.label;
    label.placeholder = tr("菜单上显示的文字");
    label.addEventListener("input", () => {
      item.label = label.value;
    });
    head.append(label);

    const on = el("label", "action-check") as HTMLLabelElement;
    const cb = el("input", "toggle") as HTMLInputElement;
    cb.type = "checkbox";
    cb.checked = item.enabled !== false;
    cb.title = tr("取消勾选 = 菜单里灰掉不可点（配置保留）");
    cb.addEventListener("change", () => {
      item.enabled = cb.checked;
    });
    on.append(cb, el("span", undefined, tr("启用")));
    head.append(on);
  }

  const up = el("button", "ghost menu-editor-btn", "↑") as HTMLButtonElement;
  up.title = tr("上移");
  up.disabled = i === 0;
  up.addEventListener("click", () => {
    moveMenuEntry(items, i, i - 1);
    rerender();
    void save();
  });
  const down = el("button", "ghost menu-editor-btn", "↓") as HTMLButtonElement;
  down.title = tr("下移");
  down.disabled = i === items.length - 1;
  down.addEventListener("click", () => {
    moveMenuEntry(items, i, i + 1);
    rerender();
    void save();
  });
  const del = el("button", "ghost menu-editor-btn danger", "×") as HTMLButtonElement;
  del.title = tr("删除这一条");
  del.addEventListener("click", () => {
    items.splice(i, 1);
    rerender();
    void save();
  });
  head.append(up, down, del);
  box.append(head);

  if (item.kind === "action") {
    const action = unwrapAction(item.action);
    const typeRow = el("div", "action-type-row");
    const typeSel = el("select", "action-type-name") as HTMLSelectElement;
    typeSel.title = tr("点击切换动作类型");
    typeSel.setAttribute("aria-label", tr("动作类型"));
    for (const t of actionTypeChoices(action.type === "tool")) {
      const o = el("option") as HTMLOptionElement;
      o.value = t.value;
      o.textContent = t.label;
      typeSel.append(o);
    }
    // 「设置变量」归文本操作：类型名显示「文本操作」，方式看子类型下拉。
    typeSel.value = action.type === "ask" ? "text" : action.type;
    typeSel.addEventListener("change", () => {
      item.action = convertedAction(action, typeSel.value as ActionBase["type"]);
      rerender();
      void save();
    });
    typeRow.append(typeSel);
    const sub = actionSubtypeSelect(
      action,
      () => {
        rerender();
        void save();
      },
      (next) => {
        item.action = next;
      },
    );
    if (sub) typeRow.append(sub);
    box.append(typeRow);
    box.append(
      actionFields(action, () => {
        rerender();
        void save();
      }),
    );
  } else if (item.kind === "submenu") {
    box.append(el("div", "menu-editor-sub", tr("子菜单")));
    box.append(menuItemsEditor(item.items, depth + 1, rerender));
    // 子菜单最多再嵌两层（顶层算第 1 层，与后端 `MAX_DEPTH` 一致）。
    if (depth < 3) {
      const adds = el("div", "set-actions menu-editor-adds");
      for (const [text, mk] of [
        [tr("＋ 动作"), () => ({ kind: "action", label: tr("新条目"), enabled: true, action: newAction("text") })],
        [
          tr("＋ 子菜单"),
          () => ({
            kind: "submenu",
            label: tr("新子菜单"),
            enabled: true,
            items: [{ kind: "action", label: tr("新条目"), enabled: true, action: newAction("text") }],
          }),
        ],
        [tr("＋ 分割线"), () => ({ kind: "separator" })],
      ] as [string, () => MenuEntry][]) {
        const b = el("button", "ghost", text) as HTMLButtonElement;
        b.addEventListener("click", () => {
          item.items.push(mk());
          rerender();
          void save();
        });
        adds.append(b);
      }
      box.append(adds);
    }
  }
  return box;
}

function syncSettings() {
  (document.getElementById("set-autostart") as HTMLInputElement).checked = state.cfg.settings.autostart;
  (document.getElementById("set-paused") as HTMLInputElement).checked = state.cfg.settings.paused;
  // 缺字段按「关」显示，与后端 serde default 一致。
  (document.getElementById("set-ignore-fullscreen") as HTMLInputElement).checked =
    state.cfg.settings.ignore_fullscreen === true;
  (document.getElementById("set-wake-key") as HTMLSelectElement).value = state.cfg.settings.wake_key ?? "";
  (document.getElementById("set-action-timeout") as HTMLInputElement).value = String(
    timeoutSeconds(state.cfg.settings.action_timeout_ms),
  );
  (document.getElementById("set-sequence-timeout") as HTMLInputElement).value = timeoutMs(
    state.cfg.settings.sequence_timeout_ms,
  );
  (document.getElementById("set-chord-timeout") as HTMLInputElement).value = timeoutMs(
    state.cfg.settings.chord_timeout_ms,
  );
  // 缺字段的老配置（写在该设置出现之前）按「开」显示，与后端 `default_true` 一致。
  (document.getElementById("set-status-hud") as HTMLInputElement).checked =
    state.cfg.settings.show_status_hud !== false;
  // 提示框默认「关」（后端 `HintsWindow::default` 也是 false）：老配置缺 `hints` 时显示为未勾选。
  (document.getElementById("set-hints") as HTMLInputElement).checked =
    state.cfg.settings.hints?.visible === true;
  // 触发提醒缺字段按「开」（与后端 `default_true` 一致）。
  (document.getElementById("set-trigger-toast") as HTMLInputElement).checked =
    state.cfg.settings.trigger_toast !== false;
  // 缺字段的老配置按默认「剪贴板粘贴」显示，与后端 serde default 一致。
  (document.getElementById("set-text-inject") as HTMLSelectElement).value =
    state.cfg.settings.text_inject_mode ?? "clipboard";
  // 索引地址：只显示自定义值；内置默认 / 空串都显示为空（后端把空串当默认）。
  // 老配置没有这个字段时同样显示空 = 用内置默认，别把内置 URL 摆给用户看。
  const snippetIndexValue = state.cfg.settings.snippet_index_url ?? "";
  (document.getElementById("set-snippet-index") as HTMLInputElement).value =
    snippetIndexValue === DEFAULT_SNIPPET_INDEX_URL ? "" : snippetIndexValue;
  // 同步存档目录：未设置时给一句引导语，设置后显示完整路径。
  document.getElementById("sync-dir")!.textContent =
    state.cfg.settings.sync_dir?.trim() || tr("未设置 —— 选一个文件夹（如 OneDrive）即可自动存一份");
  // 输入层环境诊断：缺字段的老配置按「开」显示，与后端 `default_true` 一致；
  // 结论每次进设置页重算（探测在后端，有 3 秒缓存，不构成负担）。
  (document.getElementById("set-input-env") as HTMLInputElement).checked =
    state.cfg.settings.input_env_diag !== false;
  void renderInputEnvReport();
  // 鼠标手势的实验开关（规划 7.2-MG）：缺字段按「关」显示，与后端 serde default 一致
  // （新功能默认关——它带「全局抢右键」的副作用，替用户打开不合适）。
  (document.getElementById("set-experimental") as HTMLInputElement).checked =
    state.cfg.settings.experimental === true;
  // 触摸板手势是**独立分类**：自己的开关与参数，跟上面那枚无关（缺字段 = 关 / 默认值）。
  (document.getElementById("set-touchpad") as HTMLInputElement).checked =
    state.cfg.settings.touchpad === true;
  (document.getElementById("set-touchpad-start-distance") as HTMLInputElement).value = String(
    state.cfg.settings.touchpad_start_distance ?? 22,
  );
  (document.getElementById("set-touchpad-exclude") as HTMLTextAreaElement).value = (
    state.cfg.settings.touchpad_exclude ?? []
  ).join("\n");
  // 缺字段按「开」显示（后端 serde default 就是 true）。
  (document.getElementById("set-touchpad-restore-cursor") as HTMLInputElement).checked =
    state.cfg.settings.touchpad_restore_cursor !== false;
  (document.getElementById("set-touchpad-overlay") as HTMLInputElement).checked =
    state.cfg.settings.touchpad_overlay !== false;
  (document.getElementById("set-gesture-screenshot") as HTMLInputElement).checked =
    state.cfg.settings.gesture_screenshot !== false;
  (document.getElementById("set-gesture-exclude") as HTMLTextAreaElement).value =
    (state.cfg.settings.gesture_exclude ?? []).join("\n");
  (document.getElementById("set-exclude-apps") as HTMLTextAreaElement).value =
    (state.cfg.settings.exclude_apps ?? []).join("\n");
  // 手势参数（EXT-01）：缺字段按后端 serde default 显示，避免老配置显示成 0。
  (document.getElementById("set-gesture-start-distance") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_start_distance ?? 22,
  );
  (document.getElementById("set-gesture-timeout") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_timeout_ms ?? 1500,
  );
  (document.getElementById("set-gesture-min-segment") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_min_segment ?? 16,
  );
  // 手势视觉（EXT-02）：颜色用取色器，数值同样回落后端默认。
  (document.getElementById("set-gesture-trail-color") as HTMLInputElement).value =
    state.cfg.settings.gesture_trail_color ?? "#4c9aff";
  (document.getElementById("set-gesture-trail-width") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_trail_width ?? 5,
  );
  (document.getElementById("set-gesture-label-font-size") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_label_font_size ?? 14,
  );
  (document.getElementById("set-gesture-label-offset") as HTMLInputElement).value = String(
    state.cfg.settings.gesture_label_offset ?? 16,
  );
  (document.getElementById("set-gesture-fail-color") as HTMLInputElement).value =
    state.cfg.settings.gesture_fail_color ?? "#ff5f56";
  // 提权窗口注入：缺字段的老配置按「关」显示（后端 serde default 就是 false）；状态每次都
  // 问一遍后端（提权进程可能已经退出，这个值会变）。
  (document.getElementById("set-elevated") as HTMLInputElement).checked =
    state.cfg.settings.elevated_inject === true;
  void renderElevatedReport();
  // 软件更新源：缺字段按默认 ihomy 显示（后端 serde default 一致）。
  (document.getElementById("set-update-source") as HTMLSelectElement).value =
    state.cfg.settings.update_source ?? "ihomy";
  (document.getElementById("set-auto-update") as HTMLInputElement).checked =
    state.cfg.settings.auto_update === true;
  // 复制快捷菜单（双击 Ctrl+C）已删除，设置卡不再渲染。
  // 旧配置里残留的条目仍能只读渲染（没有宿主节点时函数直接返回），不再提供编辑入口。
  renderClipboardMenuEditor();
  syncMouseOpts();
  syncTouchpadOpts();
}

/** 鼠标操作的参数设置块只在总开关打开时显示（关闭时配置仍保留）。 */
function syncMouseOpts() {
  document
    .getElementById("mouse-opts")
    ?.classList.toggle("hidden", state.cfg.settings.experimental !== true);
  syncGestureVisual();
}

/** 触摸板手势的参数块只在自己的开关打开时显示（与「鼠标操作」那枚互不影响）。 */
function syncTouchpadOpts() {
  document
    .getElementById("touchpad-opts")
    ?.classList.toggle("hidden", state.cfg.settings.touchpad !== true);
  syncGestureVisual();
}

/** 轨迹浮层是两边共用的一块窗：任一边开着就显示，两边都关着才整张卡片收起。 */
function syncGestureVisual() {
  document
    .getElementById("gesture-visual-card")
    ?.classList.toggle(
      "hidden",
      state.cfg.settings.experimental !== true && state.cfg.settings.touchpad !== true,
    );
}

/**
 * 重画「提权窗口注入」卡片的结论区（后端 `elevated_status`，规划 7.2-㉑a）。
 *
 * 三种结论分开说（见 [`ElevatedStatus`]）：本平台不支持 / 开关没开 / 开关开着但提权进程
 * 没连上。按钮跟着状态走——没起来时是「启动提权进程」（点了会弹 UAC），连上后是「断开」。
 */
async function renderElevatedReport() {
  const box = document.getElementById("elevated-report")!;
  const run = document.getElementById("elevated-toggle-run") as HTMLButtonElement;
  let s: ElevatedStatus;
  try {
    s = await invoke<ElevatedStatus>("elevated_status");
  } catch (e) {
    box.textContent = trt`状态读取失败：${e}`;
    run.hidden = true;
    return;
  }
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!s.supported) {
    run.hidden = true;
    line(tr("当前平台不支持（这是 Windows 专属能力：它的对手是 UIPI）。"));
    return;
  }
  const enabled = state.cfg.settings.elevated_inject === true;
  run.hidden = false;
  run.textContent = s.connected ? tr("断开提权进程") : tr("启动提权进程");
  if (!enabled) {
    // 开关关着时按钮没有意义（就算拉起来，注入也不会走它）——禁用而不是隐藏，
    // 让「先开开关」这件事看得见。
    run.disabled = true;
    run.title = tr("先打开上面的「提权窗口也生效」");
    line(tr("未启用。打开上面的开关后点「启动提权进程」（会弹一次 UAC 授权）。"));
    return;
  }
  run.disabled = false;
  run.title = "";
  if (s.connected) {
    line(tr("已连接：提权进程在运行，提权窗口里的注入会交给它。"));
  } else {
    line(tr("开关已打开，但提权进程没在运行——点右上角「启动提权进程」拉起它。"));
  }
  line(tr("它平时不运行：关掉开关、或退出 Kada，它就跟着退出，不会留在系统里。"));
}

/**
 * 重画「输入层环境」卡片的结论区（后端 `input_env_report`，规划 7.2-㊽）。
 *
 * 三种结论必须分开说，不能含糊成一句「没问题」：本平台没做这项探测 / 已关掉探测 /
 * 探测了 N 类但都没发现；只有第一种是能力缺口，后两种才是真的「干净」。
 */
async function renderInputEnvReport() {
  const box = document.getElementById("input-env-report")!;
  let r: InputEnvReport;
  try {
    r = await invoke<InputEnvReport>("input_env_report", { config: state.cfg });
  } catch (e) {
    box.textContent = trt`检测失败：${e}`;
    return;
  }
  // 罗技 HID++ 解锁只在 Windows 有实现（档位 C）；其余平台整块不显示，
  // 免得点了才发现「本平台不支持」。
  document.getElementById("hidpp-box")!.hidden = !r.hidpp_supported;
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!r.supported) {
    line(tr("当前平台暂不支持这项检测。"));
    return;
  }
  if (state.cfg.settings.input_env_diag === false) {
    line(tr("已关闭。开启后 Kada 才会检测会接管按键的厂商软件。"));
    return;
  }
  if (r.diverters.length === 0) {
    line(trt`未检测到会接管按键的厂商软件（已查 ${r.checked} 类）。`);
  } else {
    line(trt`检测到 ${r.diverters.length} 个可能接管按键的软件：`);
    for (const d of r.diverters) {
      const item = el("div", d.ignored ? "env-hit ignored" : "env-hit");
      const head = el("div", "env-hit-head");
      head.append(el("div", "env-hit-name", d.ignored ? trt`${d.name}（已忽略提示）` : d.name));
      head.append(inputEnvIgnoreButton(d));
      item.append(head);
      item.append(el("div", "env-hit-advice", d.advice));
      box.append(item);
    }
  }
  // 内存完整性：只报状态、不当问题说——它拦的是内核驱动，不影响 Kada 现在的用户态钩子。
  if (r.hvci === true) line(tr("系统「内存完整性」已开启（不影响 Kada 当前的工作方式，仅作记录）。"));
  else if (r.hvci === false) line(tr("系统「内存完整性」未开启。"));
  else line(tr("系统「内存完整性」状态读不到（该功能未配置，或系统未提供）。"));
}

/**
 * 命中项上的「忽略 / 恢复提示」按钮（规划 7.2-㊽ 余项②）。
 *
 * 粒度是**单个软件**，不是总开关：某个接管者在你这台机器上确实不影响用时，掐掉这一条
 * 就行，不必把整条诊断链关掉（后者连别的接管者也不报了）。
 */
function inputEnvIgnoreButton(d: InputEnvReport["diverters"][number]): HTMLButtonElement {
  const btn = el("button", "ghost env-hit-ignore", d.ignored ? tr("恢复提示") : tr("忽略")) as HTMLButtonElement;
  btn.title = d.ignored
    ? tr("恢复这条提示：它重新出现在冲突检测里")
    : tr("不再为这个软件提示（它仍在下方列表里，可随时恢复）");
  btn.addEventListener("click", () => void toggleInputEnvIgnore(d.id, !d.ignored));
  return btn;
}

/** 记住 / 撤销对某个接管者的忽略，落盘后重画结论区（冲突检测也随配置一起更新）。 */
async function toggleInputEnvIgnore(id: string, ignore: boolean) {
  const rest = (state.cfg.settings.input_env_ignored ?? []).filter((x) => x !== id);
  state.cfg.settings.input_env_ignored = ignore ? [...rest, id] : rest;
  await save();
  await renderInputEnvReport();
}

/**
 * 只读扫描罗技 HID++ 设备（`docs/底层输入能力规划.md` 档位 C）。
 *
 * 只在用户点按钮时跑：枚举要开 HID 集合、每个槽位还要等超时，不能拖慢设置页首屏。
 */
async function renderHidppStatus() {
  const box = document.getElementById("hidpp-report")!;
  box.textContent = tr("正在扫描罗技设备…");
  let s: HidppStatusView;
  try {
    s = await invoke<HidppStatusView>("hidpp_status");
  } catch (e) {
    box.textContent = trt`扫描失败：${e}`;
    return;
  }
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (!s.supported) {
    line(tr("当前平台不支持（只有 Windows 能直接解锁罗技侧键）。"));
    return;
  }
  if (s.error) line(s.error);
  if (s.devices.length === 0) {
    line(tr("没有找到可用的罗技 HID++ 设备（没接罗技设备，或控制接口被独占）。"));
  }
  for (const d of s.devices) {
    const item = el("div", "env-hit");
    item.append(el("div", "env-hit-name", d.name));
    item.append(
      el(
        "div",
        "env-hit-advice",
        trt`HID++ 索引 ${d.index === 255 ? tr("直连") : d.index} · 控件 ${d.control_count} 个 · ${d.channel} 报文`,
      ),
    );
    for (const c of d.controls) {
      const state = !c.present
        ? tr("设备上没有")
        : c.diverted
          ? c.persistent
            ? tr("被厂商软件接管（持久）")
            : tr("被厂商软件接管")
          : tr("标准报文");
      item.append(el("div", "env-hit-advice", `${c.label}：${state}`));
    }
    box.append(item);
  }
  if (s.keeper_active) line(tr("本次运行期间保持解锁中（设备重连后会自动再关）。"));
  if (s.keeper_error) line(trt`保持解锁的最近一次错误：${s.keeper_error}`);
}

/**
 * 解锁罗技侧键：把中键 / 后退 / 前进里处于 divert 的关掉，让它们回到标准报文。
 *
 * 后端同时会往消息中心推一条结论（用户点完按钮多半会离开设置页，结论不能只留在这里）。
 */
async function unlockHidpp() {
  const box = document.getElementById("hidpp-report")!;
  const btn = document.getElementById("hidpp-unlock") as HTMLButtonElement;
  btn.disabled = true;
  box.textContent = tr("正在解锁（读取并写入罗技控制接口）…");
  let r: HidppUnlockView;
  try {
    r = await invoke<HidppUnlockView>("hidpp_unlock");
  } catch (e) {
    box.textContent = trt`解锁失败：${e}`;
    btn.disabled = false;
    return;
  }
  btn.disabled = false;
  box.replaceChildren();
  const line = (text: string) => box.append(el("div", "env-line", text));
  if (r.error) {
    line(trt`解锁失败：${r.error}`);
    line(tr("可以在 Options+ / G HUB 里把该按键设为「无操作 / 默认功能」，或直接退出它。"));
    return;
  }
  if (r.devices.length === 0) {
    line(tr("没有找到罗技设备。"));
    return;
  }
  for (const d of r.devices) {
    const item = el("div", "env-hit");
    item.append(el("div", "env-hit-name", d.name));
    if (d.cleared.length) item.append(el("div", "env-hit-advice", trt`已解锁：${d.cleared.join("、")}`));
    if (d.already.length) item.append(el("div", "env-hit-advice", trt`本来就是标准报文：${d.already.join("、")}`));
    if (d.missing.length) item.append(el("div", "env-hit-advice", trt`设备上没有：${d.missing.join("、")}`));
    for (const f of d.failed) item.append(el("div", "env-hit-advice", trt`失败：${f}`));
    box.append(item);
  }
  line(
    r.keeping
      ? tr("已生效，本次运行期间会保持解锁（设备重连或厂商软件重启后自动再关）。")
      : tr("没有需要解锁的键：钩子已经能收到这些按键。"),
  );
}

function bindSettings() {
  // 主题不进配置（见 THEME_KEY 处注释），所以自己读写 localStorage + 直接打 data-theme。
  const theme = document.getElementById("set-theme") as HTMLSelectElement;
  theme.value = loadTheme();
  theme.addEventListener("change", () => {
    localStorage.setItem(THEME_KEY, theme.value);
    applyTheme(theme.value);
  });
  // 界面语言：只记本机（localStorage），切完整页 reload——静态文案与已渲染的界面一起换掉。
  const lang = document.getElementById("set-lang") as HTMLSelectElement;
  lang.value = savedLocale() ?? "system";
  lang.addEventListener("change", () => {
    applyLocale(lang.value === "system" ? null : (lang.value as "zh" | "en"));
    location.reload();
  });
  const autostart = document.getElementById("set-autostart") as HTMLInputElement;
  const paused = document.getElementById("set-paused") as HTMLInputElement;
  const wakeKey = document.getElementById("set-wake-key") as HTMLSelectElement;
  const actionTimeout = document.getElementById("set-action-timeout") as HTMLInputElement;
  const sequenceTimeout = document.getElementById("set-sequence-timeout") as HTMLInputElement;
  const chordTimeout = document.getElementById("set-chord-timeout") as HTMLInputElement;
  const statusHud = document.getElementById("set-status-hud") as HTMLInputElement;
  const hints = document.getElementById("set-hints") as HTMLInputElement;
  autostart.addEventListener("change", () => {
    state.cfg.settings.autostart = autostart.checked;
    void save();
  });
  paused.addEventListener("change", () => {
    state.cfg.settings.paused = paused.checked;
    void save();
  });
  // 全屏程序里暂停（EXT-05）：只是行为开关，配置照常保存。
  const ignoreFullscreen = document.getElementById("set-ignore-fullscreen") as HTMLInputElement;
  ignoreFullscreen.addEventListener("change", () => {
    state.cfg.settings.ignore_fullscreen = ignoreFullscreen.checked;
    void save();
  });
  wakeKey.addEventListener("change", () => {
    state.cfg.settings.wake_key = wakeKey.value || null;
    void save();
  });
  actionTimeout.addEventListener("change", () => {
    // 负值 / 非数字按 0（不限时）处理：后端只认 u64，宁可放开也不静默变成 1 秒把人坑了。
    const secs = Math.max(0, Math.floor(Number(actionTimeout.value) || 0));
    actionTimeout.value = String(secs);
    state.cfg.settings.action_timeout_ms = secs * 1000;
    void save();
  });
  // 两个等待窗按毫秒录入（默认 1000）：负数 / 非数字同样按 0（不限时）处理。
  sequenceTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(sequenceTimeout.value) || 0));
    sequenceTimeout.value = String(ms);
    state.cfg.settings.sequence_timeout_ms = ms;
    void save();
  });
  chordTimeout.addEventListener("change", () => {
    const ms = Math.max(0, Math.floor(Number(chordTimeout.value) || 0));
    chordTimeout.value = String(ms);
    state.cfg.settings.chord_timeout_ms = ms;
    void save();
  });
  statusHud.addEventListener("change", () => {
    state.cfg.settings.show_status_hud = statusHud.checked;
    void save();
  });
  const triggerToast = document.getElementById("set-trigger-toast") as HTMLInputElement;
  triggerToast.addEventListener("change", () => {
    state.cfg.settings.trigger_toast = triggerToast.checked;
    void save();
  });
  // 提示框的开关：改动随整份配置落盘，后端 `set_config` 比对到 `visible` 变了就把窗口
  // 建出来 / 收起来，并把托盘菜单的勾对齐（见 `apply_hints_visible`）。
  hints.addEventListener("change", () => {
    state.cfg.settings.hints = { ...(state.cfg.settings.hints ?? { scale: 100, opacity: 92 }), visible: hints.checked };
    void save();
  });
  const textInject = document.getElementById("set-text-inject") as HTMLSelectElement;
  textInject.addEventListener("change", () => {
    state.cfg.settings.text_inject_mode = textInject.value as "clipboard" | "unicode";
    void save();
  });
  // 索引地址：留空 = 内置默认（后端把空串也当默认，别让人以为索引被关掉了）。
  const snippetIndex = document.getElementById("set-snippet-index") as HTMLInputElement;
  snippetIndex.addEventListener("change", () => {
    state.cfg.settings.snippet_index_url = snippetIndex.value.trim();
    // 换了索引地址，下次进市场页必须重新拉，别把旧索引继续挂在列表里。
    marketLoaded = false;
    void save();
  });
  // 输入层环境诊断（规划 7.2-㊽）：关掉即「整条链路静默」——后端连探测都不做
  // （`get_conflicts` / `input_env_report` 都先看这个开关），所以改完要重画结论区。
  const inputEnv = document.getElementById("set-input-env") as HTMLInputElement;
  inputEnv.addEventListener("change", () => {
    state.cfg.settings.input_env_diag = inputEnv.checked;
    void save();
    void renderInputEnvReport();
  });
  // 鼠标操作总开关（规划 7.2-MG）：关掉后鼠标类功能一律不可用——**配置保留**
  // （只是入口隐藏 + 行为不生效），所以这里只改开关，不动 cfg.gestures。
  const experimental = document.getElementById("set-experimental") as HTMLInputElement;
  experimental.addEventListener("change", () => {
    state.cfg.settings.experimental = experimental.checked;
    syncMouseOpts();
    // 改完重画列表：快捷键页里鼠标类条目的显隐由它决定。
    void save().then(() => render());
  });
  // 触摸板手势总开关：与「鼠标操作」完全分开——只影响触摸板条目与采集，
  // 关掉时配置保留（入口隐藏 + 不采集，触发键照常输出）。
  const touchpad = document.getElementById("set-touchpad") as HTMLInputElement;
  touchpad.addEventListener("change", () => {
    state.cfg.settings.touchpad = touchpad.checked;
    syncTouchpadOpts();
    void save().then(() => render());
  });
  // 触摸板手势的起手距离（像素）：这一笔至少移动这么远才算数，钳在 1~50。
  const touchpadStart = document.getElementById(
    "set-touchpad-start-distance",
  ) as HTMLInputElement;
  touchpadStart.addEventListener("change", () => {
    const v = Math.min(50, Math.max(1, Math.floor(Number(touchpadStart.value) || 22)));
    touchpadStart.value = String(v);
    state.cfg.settings.touchpad_start_distance = v;
    void save();
  });
  // 触摸板手势的排除程序：每行一个，与鼠标手势的 `gesture_exclude` 各自一份名单。
  const touchpadExclude = document.getElementById(
    "set-touchpad-exclude",
  ) as HTMLTextAreaElement;
  touchpadExclude.addEventListener("change", () => {
    state.cfg.settings.touchpad_exclude = touchpadExclude.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 松手后光标回起笔点：关掉 = 光标留在这一笔走到的位置（有人还要接着用指针）。
  const touchpadRestore = document.getElementById(
    "set-touchpad-restore-cursor",
  ) as HTMLInputElement;
  touchpadRestore.addEventListener("change", () => {
    state.cfg.settings.touchpad_restore_cursor = touchpadRestore.checked;
    void save();
  });
  // 采集时显示轨迹浮层：只影响触摸板这条来源（右键手势没有单独开关）。
  const touchpadOverlay = document.getElementById("set-touchpad-overlay") as HTMLInputElement;
  touchpadOverlay.addEventListener("change", () => {
    state.cfg.settings.touchpad_overlay = touchpadOverlay.checked;
    void save();
  });
  const gestureScreenshot = document.getElementById(
    "set-gesture-screenshot",
  ) as HTMLInputElement;
  gestureScreenshot.addEventListener("change", () => {
    state.cfg.settings.gesture_screenshot = gestureScreenshot.checked;
    void save();
  });
  // 手势排除程序：每行一个（后端清洗会 trim 并丢空行），失焦时落盘。
  const gestureExclude = document.getElementById("set-gesture-exclude") as HTMLTextAreaElement;
  gestureExclude.addEventListener("change", () => {
    state.cfg.settings.gesture_exclude = gestureExclude.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 全局排除程序（EXT-04）：命中的程序里所有触发都不生效，与手势专用列表独立。
  const excludeApps = document.getElementById("set-exclude-apps") as HTMLTextAreaElement;
  excludeApps.addEventListener("change", () => {
    state.cfg.settings.exclude_apps = excludeApps.value
      .split("\n")
      .map((s) => s.trim())
      .filter(Boolean);
    void save();
  });
  // 手势参数（EXT-01）：三个数都是「手改 JSON 也允许」的值，这里按界面建议范围钳一下，
  // 免得存进去一个 0 把识别弄坏。失焦 / 回车时落盘。
  const gestureStart = document.getElementById("set-gesture-start-distance") as HTMLInputElement;
  gestureStart.addEventListener("change", () => {
    const v = Math.min(50, Math.max(1, Math.floor(Number(gestureStart.value) || 22)));
    gestureStart.value = String(v);
    state.cfg.settings.gesture_start_distance = v;
    void save();
  });
  const gestureTimeout = document.getElementById("set-gesture-timeout") as HTMLInputElement;
  gestureTimeout.addEventListener("change", () => {
    const v = Math.min(3000, Math.max(500, Math.floor(Number(gestureTimeout.value) || 1500)));
    gestureTimeout.value = String(v);
    state.cfg.settings.gesture_timeout_ms = v;
    void save();
  });
  const gestureMinSegment = document.getElementById("set-gesture-min-segment") as HTMLInputElement;
  gestureMinSegment.addEventListener("change", () => {
    const v = Math.min(40, Math.max(4, Math.floor(Number(gestureMinSegment.value) || 16)));
    gestureMinSegment.value = String(v);
    state.cfg.settings.gesture_min_segment = v;
    void save();
  });
  // 手势视觉（EXT-02）：颜色是 `#rrggbb`，数值范围与后端 sanitize 一致。
  const gestureTrailColor = document.getElementById("set-gesture-trail-color") as HTMLInputElement;
  gestureTrailColor.addEventListener("change", () => {
    state.cfg.settings.gesture_trail_color = gestureTrailColor.value;
    void save();
  });
  const gestureTrailWidth = document.getElementById("set-gesture-trail-width") as HTMLInputElement;
  gestureTrailWidth.addEventListener("change", () => {
    const v = Math.min(20, Math.max(1, Math.floor(Number(gestureTrailWidth.value) || 5)));
    gestureTrailWidth.value = String(v);
    state.cfg.settings.gesture_trail_width = v;
    void save();
  });
  const gestureLabelFont = document.getElementById(
    "set-gesture-label-font-size",
  ) as HTMLInputElement;
  gestureLabelFont.addEventListener("change", () => {
    const v = Math.min(48, Math.max(8, Math.floor(Number(gestureLabelFont.value) || 14)));
    gestureLabelFont.value = String(v);
    state.cfg.settings.gesture_label_font_size = v;
    void save();
  });
  const gestureLabelOffset = document.getElementById(
    "set-gesture-label-offset",
  ) as HTMLInputElement;
  gestureLabelOffset.addEventListener("change", () => {
    const v = Math.min(200, Math.max(0, Math.floor(Number(gestureLabelOffset.value) || 16)));
    gestureLabelOffset.value = String(v);
    state.cfg.settings.gesture_label_offset = v;
    void save();
  });
  const gestureFailColor = document.getElementById("set-gesture-fail-color") as HTMLInputElement;
  gestureFailColor.addEventListener("change", () => {
    state.cfg.settings.gesture_fail_color = gestureFailColor.value;
    void save();
  });
  // 复制快捷菜单（双击 Ctrl+C）已删除：不再有设置项与条目编辑器。
  // 「重新检测」：结论本身在后端有 3 秒缓存，这里绕开不了它，但连点几次足以跨过缓存
  // ——退出厂商软件后想让结论立刻翻篇，这是最省事的手动刷新入口。
  document
    .getElementById("input-env-refresh")!
    .addEventListener("click", () => void renderInputEnvReport());
  // 罗技 HID++ 解锁（档位 C）：扫描是只读的，解锁会写设备（可随时被厂商软件改回）。
  document.getElementById("hidpp-scan")!.addEventListener("click", () => void renderHidppStatus());
  document.getElementById("hidpp-unlock")!.addEventListener("click", () => void unlockHidpp());
  // 提权窗口注入（规划 7.2-㉑a）：**开关只写配置**，拉起 / 断开是按钮的动作——开关本身
  // 不弹 UAC（用户点「启动」才弹），关掉时后端会顺带断开通道，提权进程随之退出。
  const elevated = document.getElementById("set-elevated") as HTMLInputElement;
  elevated.addEventListener("change", () => {
    state.cfg.settings.elevated_inject = elevated.checked;
    void save();
    void renderElevatedReport();
  });
  // 启动 / 断开：这一下会弹 UAC（用户在等授权，界面上要说清楚），且后端在等提权进程把通道
  // 建起来（最长 30 秒）。所以按键就地禁用 + 写一句进度，别让人以为界面卡死了。
  const elevatedRun = document.getElementById("elevated-toggle-run") as HTMLButtonElement;
  onClickAsync(elevatedRun, async () => {
    const box = document.getElementById("elevated-report")!;
    const wasConnected = elevatedRun.textContent?.includes(tr("断开")) === true;
    elevatedRun.disabled = true;
    box.textContent = wasConnected ? tr("正在断开…") : tr("正在请求管理员授权（会弹 UAC，请在弹出的窗口里确认）…");
    try {
      if (wasConnected) await invoke("elevated_stop");
      else await invoke<string>("elevated_start");
    } catch (e) {
      box.textContent = trt`${wasConnected ? tr("断开") : tr("启动")}失败：${e}`;
      elevatedRun.disabled = false;
      return;
    }
    await renderElevatedReport();
  });
  document.getElementById("export-config")!.addEventListener("click", () => void exportConfig());
  document.getElementById("sync-choose")!.addEventListener("click", () => void chooseSyncDir());
  document.getElementById("sync-clear")!.addEventListener("click", () => {
    state.cfg.settings.sync_dir = null;
    void save();
  });
  document.getElementById("sync-now")!.addEventListener("click", () => void syncNow());
  document.getElementById("sync-restore")!.addEventListener("click", () => void syncRestore());
  document.getElementById("import-config")!.addEventListener("click", () => void importConfig());
  document.getElementById("load-sample")!.addEventListener("click", () => void loadSampleConfig());
  // 卡带市场：设置页的按钮跳到独立市场页，搜索在本地过滤（索引已全量拿在手上）。
  document.getElementById("open-market")!.addEventListener("click", () => void switchSection("market"));
  document.getElementById("market-settings")!.addEventListener("click", () => void gotoMarketSettings());
  document.getElementById("market-reload")!.addEventListener("click", () => void loadMarket(true));
  document.getElementById("market-search")!.addEventListener("input", (e) => {
    marketQuery = (e.target as HTMLInputElement).value;
    renderMarketPage();
  });
  document.getElementById("snippet-install")!.addEventListener("click", () => void confirmSnippetInstall());
  document.getElementById("snippet-cancel")!.addEventListener("click", hideSnippetPreview);
  document.getElementById("snippet-modal")!.querySelector(".modal-mask")!.addEventListener("click", hideSnippetPreview);
  (document.getElementById("snippet-consent") as HTMLInputElement).addEventListener("change", (e) => {
    (document.getElementById("snippet-install") as HTMLButtonElement).disabled = !(e.target as HTMLInputElement).checked;
  });

  // 发布卡带：入口在市场标题行与行菜单（⋯ / 右键）都开同一个发布页。
  document.getElementById("market-publish")!.addEventListener("click", () => openPublish());
  document.getElementById("publish-close")!.addEventListener("click", closePublish);
  document.getElementById("publish-cancel")!.addEventListener("click", closePublish);
  document.getElementById("publish-modal")!.querySelector(".modal-mask")!.addEventListener("click", closePublish);
  document.getElementById("publish-select-all")!.addEventListener("click", () => {
    for (const kind of PUBLISH_KINDS) {
      arrOf(kind).forEach((_, idx) => publishSelected.add(publishKey(kind, idx)));
    }
    syncPublishName();
    renderPublishPicks();
    schedulePublishScan();
  });
  document.getElementById("publish-select-none")!.addEventListener("click", () => {
    publishSelected.clear();
    syncPublishName();
    renderPublishPicks();
    schedulePublishScan();
  });
  (document.getElementById("publish-name") as HTMLInputElement).addEventListener("input", () => {
    publishNameTouched = true;
    schedulePublishScan();
  });
  (document.getElementById("publish-desc") as HTMLTextAreaElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-author") as HTMLInputElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-tags") as HTMLInputElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-consent") as HTMLInputElement).addEventListener("change", updatePublishSubmit);
  document.getElementById("publish-submit")!.addEventListener("click", () => void submitPublish());
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!document.getElementById("publish-modal")!.classList.contains("hidden")) {
      e.preventDefault();
      closePublish();
    }
  });
  // 更新源：写进配置后下一次检查（含手动「检查更新」）立即生效，后端每次检查都重读设置。
  const updateSource = document.getElementById("set-update-source") as HTMLSelectElement;
  updateSource.addEventListener("change", () => {
    state.cfg.settings.update_source = updateSource.value as "ihomy" | "github";
    void save();
  });
  document.getElementById("check-update")!.addEventListener("click", () => {
    void invoke("check_update");
  });
  const autoUpdate = document.getElementById("set-auto-update") as HTMLInputElement;
  autoUpdate.addEventListener("change", () => {
    state.cfg.settings.auto_update = autoUpdate.checked;
    void save();
    // 已经发现新版本时勾上就立刻开始下载：否则要等下一次检查（可能一天后）才动，
    // 用户会以为开关没生效。
    if (autoUpdate.checked && updateStatus?.phase === "available") {
      void invoke("download_update");
    }
  });
  document.getElementById("download-update")!.addEventListener("click", () => {
    void invoke("download_update");
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
    toast(tr("已导出到 ") + path);
  } catch (e) {
    toast(trt`导出失败: ${e}`);
  }
}

// 选同步存档目录：以本机探测到的 OneDrive 目录作默认位置（后端 `default_sync_dir`），
// 选定后写进配置并保存——保存本身就会立刻推一份过去（后端 `push_sync`，规划 7.2-㉝）。
async function chooseSyncDir() {
  const def = (await invoke<string | null>("default_sync_dir").catch(() => null)) ?? undefined;
  const dir = await openFile({ directory: true, defaultPath: def });
  if (!dir || Array.isArray(dir)) return;
  state.cfg.settings.sync_dir = dir;
  await save();
}

// 手动推送：把当前配置立刻写一份到同步存档目录，成功给明确反馈（与自动推送的静默失败区分）。
async function syncNow() {
  try {
    const path = await invoke<string>("sync_now");
    toast(tr("已同步到 ") + path);
  } catch (e) {
    toast(trt`同步失败: ${e}`);
  }
}

// 从存档恢复：后端给存档文件路径，走既有的「导入配置」流程（合并 / 替换 + 结果弹窗）。
async function syncRestore() {
  let path: string;
  try {
    path = await invoke<string>("sync_import_path");
  } catch (e) {
    toast(trt`无法读取同步存档: ${e}`);
    return;
  }
  const mode = await askImportMode(fileName(path));
  if (!mode) return;
  try {
    const r = await invoke<ImportOutcome>("import_config", { path, mode });
    await load();
    showImportResult(r);
  } catch (e) {
    toast(trt`恢复失败: ${e}`);
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
  document.getElementById("import-file")!.textContent = trt`将导入 ${file}，请选择导入方式：`;
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
    toast(trt`导入失败: ${e}`);
  }
}

// 载入内置示例配置：后端走合并语义（只增不删、不碰设置），结果复用导入结果弹窗展示。
async function loadSampleConfig() {
  try {
    const r = await invoke<ImportOutcome>("load_sample_config");
    await load();
    showImportResult(r);
  } catch (e) {
    toast(trt`载入示例失败: ${e}`);
  }
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function showImportResult(r: ImportOutcome) {
  const skipped = r.notes.length;
  document.getElementById("import-result-text")!.textContent =
    r.mode === "merge"
      ? trt`已合并：新增 ${r.added} 条${skipped ? trt`，跳过 / 忽略 ${skipped} 条` : ""}。`
      : trt`已替换为导入的配置：共 ${r.added} 条${skipped ? trt`，忽略 ${skipped} 条` : ""}。`;
  const note = document.getElementById("import-result-note")!;
  note.classList.toggle("hidden", !r.backup);
  if (r.backup) note.textContent = trt`导入前的配置已留档：${r.backup}（覆盖回去即可回滚）`;
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
// 下载与安装是两步：`available` 给「下载」，下载完进 `ready` 才给「安装」。
let updateStatus: UpdateStatus | null = null;

function syncUpdate() {
  const line = document.getElementById("update-status")!;
  const actions = document.getElementById("update-actions")!;
  const downloadBtn = document.getElementById("download-update")!;
  const installBtn = document.getElementById("install-update")!;
  const checkBtn = document.getElementById("check-update") as HTMLButtonElement;
  if (!updateStatus) {
    line.textContent = tr("正在读取版本…");
    actions.classList.add("hidden");
    return;
  }
  const current = `v${updateStatus.current}`;
  const portable = updateStatus.kind === "portable";
  const busy =
    updateStatus.phase === "checking" ||
    updateStatus.phase === "downloading" ||
    updateStatus.phase === "installing";
  checkBtn.disabled = busy;
  // 下载与安装是两步：`available` 只给「下载」；`ready`（已下载）才给「安装」。
  const canDownload = updateStatus.phase === "available";
  const canInstall = updateStatus.phase === "ready";
  actions.classList.toggle("hidden", !canDownload && !canInstall);
  downloadBtn.classList.toggle("hidden", !canDownload);
  installBtn.classList.toggle("hidden", !canInstall);

  switch (updateStatus.phase) {
    case "idle":
      line.textContent = trt`当前版本 ${current}`;
      break;
    case "checking":
      line.textContent = trt`当前版本 ${current} · 正在检查更新…`;
      break;
    case "up-to-date":
      line.textContent = trt`当前版本 ${current}，已是最新版本`;
      break;
    case "available":
      line.textContent = portable
        ? trt`发现新版本 v${updateStatus.version}（当前 ${current}），点「下载」获取更新包`
        : trt`发现新版本 v${updateStatus.version}（当前 ${current}），点「下载」获取安装包`;
      break;
    case "downloading": {
      const pct =
        updateStatus.percent === null || updateStatus.percent === undefined
          ? ""
          : ` ${Math.round(updateStatus.percent)}%`;
      line.textContent = trt`正在下载 v${updateStatus.version}…${pct}`;
      break;
    }
    case "ready": {
      // 安装会退出并重启：安装版交给安装器静默装完重启，免安装版原位替换 exe 后重启。
      const how = portable
        ? tr("点「安装」将替换 kada.exe 并重启应用")
        : tr("点「安装」将退出应用、由安装器静默安装后重启");
      const auto = state.cfg.settings.auto_update
        ? portable
          ? tr("；已开启自动更新，也会在长时间空闲或下次启动时自动安装")
          : tr("；已开启自动更新，也会在长时间空闲时自动安装")
        : "";
      line.textContent = trt`已下载 v${updateStatus.version}，${how}${auto}`;
      break;
    }
    case "installing":
      line.textContent = portable
        ? trt`正在替换程序到 v${updateStatus.version}，应用即将重启…`
        : trt`正在安装 v${updateStatus.version}，应用即将退出…`;
      break;
    case "error":
      line.textContent = trt`更新失败：${updateStatus.message}`;
      break;
  }
}

// 点「查看更新日志」：把清单里的更新说明（`notes`）用同一个自绘模态框展示。
// 说明只跟着发现 / 已下载两个阶段来（下载时已存进 `pending.json`，重启后仍在）；
// 没有说明就如实说「暂无」，不另造 changelog 数据源。
function openUpdateNotes() {
  const s = updateStatus;
  if (!s || (s.phase !== "available" && s.phase !== "ready")) return;
  const notes = (s.notes ?? "").trim();
  openUpdateDialog({
    tone: "info",
    title: trt`更新日志 v${s.version}`,
    text: notes || tr("暂无更新说明"),
    action: null,
  });
}

// ---- 更新对话框（自绘）----
// 壳层（`src-tauri/src/update.rs` 的 `show_dialog`）手动检查 / 下载 / 安装时的反馈都推
// `update-dialog` 过来，这里用与主界面同一套模态框渲染，不再弹灰白系统框。
// 托盘「检查更新」会先唤起主窗口，那条事件可能赶在页面加载完之前——挂载时再用
// `take_update_dialog` 补取一次（后端存一份、取走即清）。
type UpdateDialogPayload = {
  tone: "info" | "error";
  title: string;
  text: string;
  action?: "download" | null;
};

/** 主按钮要做什么；`null` = 纯说明（只有一个「知道了」）。 */
let updateDialogAction: "download" | null = null;

function openUpdateDialog(d: UpdateDialogPayload) {
  const modal = document.getElementById("update-modal")!;
  modal.classList.toggle("is-error", d.tone === "error");
  document.getElementById("update-title")!.textContent = d.title;
  document.getElementById("update-text")!.textContent = d.text;
  const download = d.action === "download";
  const ok = document.getElementById("update-ok") as HTMLButtonElement;
  const cancel = document.getElementById("update-cancel") as HTMLButtonElement;
  ok.textContent = download ? tr("下载") : tr("知道了");
  ok.className = "primary";
  cancel.textContent = tr("稍后");
  cancel.classList.toggle("hidden", !download);
  updateDialogAction = download ? "download" : null;
  modal.classList.remove("hidden");
  // 先摘 `hidden` 再 focus：`display:none` 的子树里 focus() 是空操作。
  ok.focus();
}

/** `confirmed` = 点了主按钮（说明框的「知道了」也算）；只有下载询问框才真的去下载。 */
function closeUpdateDialog(confirmed: boolean) {
  document.getElementById("update-modal")!.classList.add("hidden");
  const action = updateDialogAction;
  updateDialogAction = null;
  if (confirmed && action === "download") void invoke("download_update");
}

function bindUpdateDialog() {
  document.getElementById("update-ok")!.addEventListener("click", () => closeUpdateDialog(true));
  document.getElementById("update-cancel")!.addEventListener("click", () => closeUpdateDialog(false));
  document.getElementById("update-close")!.addEventListener("click", () => closeUpdateDialog(false));
  document.getElementById("update-notes")!.addEventListener("click", () => openUpdateNotes());
  document.getElementById("update-modal")!.querySelector(".modal-mask")!.addEventListener("click", () =>
    closeUpdateDialog(false),
  );
  // 与其它确认框同口径：Esc = 取消（下载询问框 = 不下载，纯说明框只是关掉）。
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (document.getElementById("update-modal")!.classList.contains("hidden")) return;
    e.preventDefault();
    closeUpdateDialog(false);
  });
}

// ---- 改键下拉 ----
function fillKeySelect(sel: HTMLSelectElement, value: string, withEmpty = false) {
  sel.replaceChildren();
  if (withEmpty) {
    const o = document.createElement("option");
    o.value = "";
    o.textContent = tr("（无）");
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
  if (!state.remapDraft) return;
  const isModKey = !!(state.remapDraft.oneshot || state.remapDraft.sticky);
  const isTapHold = !!(state.remapDraft.tap || state.remapDraft.hold || state.remapDraft.tap2 || state.remapDraft.tap3);
  const isLayerKey = !!(state.remapDraft.hold_layer || state.remapDraft.lock_layer);
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
  empty.textContent = mode === "hold" ? tr("（不切层）") : tr("基础层（始终生效）");
  sel.append(empty);
  for (const l of state.cfg.layers) {
    const o = document.createElement("option");
    o.value = l.id;
    o.textContent = l.name;
    sel.append(o);
  }
  sel.value = value ?? "";
}

function layerName(id: string | null | undefined): string {
  if (!id) return tr("基础层");
  return state.cfg.layers.find((l) => l.id === id)?.name ?? tr("基础层");
}

// 层标记调色板：固定色值，保证「已用颜色在别的层不可选」的判重稳定，也免得调出肉眼难分的近似色。
// 每层至多一个标记：颜色（`#rrggbb`）或一个 emoji 字符，互斥，存 core `Layer.mark`。
const LAYER_COLORS: { value: string; label: string }[] = [
  { value: "#e5484d", label: tr("红") },
  { value: "#f76b15", label: tr("橙") },
  { value: "#f5b300", label: tr("黄") },
  { value: "#46a758", label: tr("绿") },
  { value: "#12a594", label: tr("青") },
  { value: "#4f9cf9", label: tr("蓝") },
  { value: "#6e56cf", label: tr("紫") },
  { value: "#e93d82", label: tr("粉") },
  { value: "#a18072", label: tr("棕") },
  { value: "#8b8d98", label: tr("灰") },
];

function isLayerColor(mark: string | null | undefined): boolean {
  return !!mark && mark.startsWith("#");
}

function layerById(id: string | null | undefined): Layer | undefined {
  return id ? state.cfg.layers.find((l) => l.id === id) : undefined;
}

// 层标记贴在组合键 chip 的左下角（绝对定位，不占组合键的字符位）：颜色是与 chip 融为一体的角标——
// chip 内部左下角一枚直角被修圆的三角（直角落在角上、收在 9px 内，够不着从 6px 起排的组合键字符）；
// emoji 像贴纸贴在左下角——右缘对齐字符起点、探出 chip 外。两者都收在内边距里、不动字符起点，也不挡
// 鼠标（pointer-events:none）。
function appendLayerMark(trig: HTMLElement, layerId: string | null | undefined) {
  const mark = layerById(layerId)?.mark;
  if (!mark) return;
  const color = isLayerColor(mark);
  const s = el("span", `row-layer-mark ${color ? "mark-color" : "mark-emoji"}`);
  if (color) s.style.setProperty("--mark", mark);
  else s.textContent = mark;
  trig.append(s);
}

// 改键形态的人类可读摘要（列表行 + 详情标题共用），与 kada-core 的 Remap::describe 对齐。
function remapSummary(r: Remap): string {
  if (r.sticky) return trt`粘滞 ${r.sticky}`;
  if (r.oneshot) return trt`单击 ${r.oneshot}`;
  if (r.lock_layer) return trt`长按锁定层「${layerName(r.lock_layer)}」`;
  if (r.hold_layer) return trt`长按进层「${layerName(r.hold_layer)}」`;
  const parts: string[] = [];
  if (r.tap) parts.push(trt`单击 ${r.tap}`);
  if (r.tap2) parts.push(trt`双击 ${r.tap2}`);
  if (r.tap3) parts.push(trt`三击 ${r.tap3}`);
  if (r.hold) parts.push(trt`长按 ${r.hold}`);
  if (parts.length) return parts.join(" · ");
  return r.to;
}

function renderLayers() {
  const list = document.getElementById("layer-list")!;
  list.replaceChildren();
  // 一个层都没有时给一句说明：卡片里只剩「＋ 新建层」和一个折叠块，用户不知道层是干什么的。
  if (state.cfg.layers.length === 0) {
    list.append(
      el(
        "div",
        "set-empty",
        tr("还没有层。层让同一按键在不同状态下干不同事：在改键里配好「长按进入层 / 长按锁定层」后，层内的快捷键与改键才生效。"),
      ),
    );
    return;
  }
  // 颜色唯一：已被别的层占用的颜色在本层下拉里禁用（emoji 不受限）。
  const usedColors = new Set(
    state.cfg.layers.map((l) => l.mark).filter((m): m is string => isLayerColor(m)),
  );
  state.cfg.layers.forEach((l) => {
    const row = el("div", "layer-row");

    // 预览色块 / emoji（与列表条目组合键 chip 内的标记同义）。
    const swatch = el(
      "span",
      "layer-swatch" + (isLayerColor(l.mark) ? " mark-color" : l.mark ? " mark-emoji" : ""),
    );
    if (isLayerColor(l.mark)) swatch.style.setProperty("--mark", l.mark!);
    else if (l.mark) swatch.textContent = l.mark;
    row.append(swatch);

    // 名称：点击改名（新建时直接进改名态）。
    if (state.editingLayerId === l.id) {
      const inp = el("input", "layer-name-input") as HTMLInputElement;
      inp.value = l.name;
      inp.placeholder = tr("层名");
      let done = false;
      const commit = () => {
        if (done) return;
        done = true;
        const v = inp.value.trim();
        if (v) l.name = v;
        else state.cfg.layers = state.cfg.layers.filter((x) => x.id !== l.id);
        state.editingLayerId = null;
        void save();
      };
      inp.addEventListener("keydown", (e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          commit();
        } else if (e.key === "Escape") {
          done = true;
          state.editingLayerId = null;
          renderLayers();
        }
      });
      inp.addEventListener("blur", commit);
      row.append(inp);
      setTimeout(() => inp.focus(), 0);
    } else {
      const name = el("span", "layer-name", l.name);
      name.title = tr("点击改名");
      name.addEventListener("click", () => {
        state.editingLayerId = l.id;
        renderLayers();
      });
      row.append(name);
    }

    // 颜色下拉（含「无」）：已用颜色在别的层里禁用。
    const sel = el("select", "layer-color") as HTMLSelectElement;
    const none = document.createElement("option");
    none.value = "";
    none.textContent = tr("无颜色");
    sel.append(none);
    for (const c of LAYER_COLORS) {
      const o = document.createElement("option");
      o.value = c.value;
      o.textContent = c.label;
      o.style.color = c.value;
      if (usedColors.has(c.value) && c.value !== l.mark) o.disabled = true;
      sel.append(o);
    }
    sel.value = isLayerColor(l.mark) ? l.mark! : "";
    sel.title = tr("层颜色（已被其它层占用的颜色不可选）");
    sel.addEventListener("change", () => {
      if (sel.value) l.mark = sel.value; // 选颜色 → 盖掉旧标记（含 emoji）
      else if (isLayerColor(l.mark)) l.mark = null; // 选「无」→ 仅在原为颜色时清空，保留 emoji
      void save();
    });
    row.append(sel);

    // emoji 输入：填了就盖过颜色（互斥）。
    const emoji = el("input", "layer-emoji") as HTMLInputElement;
    emoji.value = isLayerColor(l.mark) ? "" : (l.mark ?? "");
    emoji.placeholder = "emoji";
    emoji.maxLength = 4;
    emoji.title = tr("层 emoji 图标（填了就顶掉颜色）");
    emoji.addEventListener("change", () => {
      const v = emoji.value.trim();
      l.mark = v || null;
      void save();
    });
    row.append(emoji);

    const del = el("button", "row-more layer-del", "×") as HTMLButtonElement;
    del.title = tr("删除层");
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
  state.cfg.layers.push(l);
  state.editingLayerId = l.id;
  renderLayers();
}

async function deleteLayer(id: string) {
  const target = state.cfg.layers.find((l) => l.id === id);
  if (!target) return;
  const ok = await askConfirm({
    title: tr("删除层"),
    text: trt`删除层「${target.name}」？该层的快捷键/改键会回到基础层，指向它的切层键会失效。`,
    okLabel: tr("删除"),
    danger: true,
  });
  if (!ok) return;
  state.cfg.layers = state.cfg.layers.filter((l) => l.id !== id);
  state.cfg.shortcuts.forEach((s) => {
    if (s.layer === id) s.layer = null;
  });
  state.cfg.remaps.forEach((r) => {
    if (r.layer === id) r.layer = null;
    if (r.hold_layer === id) r.hold_layer = null;
    if (r.lock_layer === id) r.lock_layer = null;
  });
  for (const r of [...(state.cfg.hot_corners ?? []), ...(state.cfg.wheel_edges ?? [])]) {
    if (r.layer === id) r.layer = null;
  }
  if (state.editingLayerId === id) state.editingLayerId = null;
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

// 只有真正能选中文字的控件才放行原生 Ctrl+A。复选框 / 单选 / 颜色 / 下拉这类控件拿到
// Ctrl+A 会退化成「选中整页」，在设置页点过勾选后按 Ctrl+A 就会整页刷蓝。
function isTextEntryTarget(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el) return false;
  if (el.isContentEditable) return true;
  if (el.tagName === "TEXTAREA") return true;
  if (el.tagName !== "INPUT") return false;
  const type = (el as HTMLInputElement).type;
  return ["text", "search", "url", "tel", "email", "password", "number"].includes(type);
}

// 鸣谢名单：写死在前端（纯展示，不进 config）。有「首页」的名字点击跳首页；没设首页的点一下从鼠标
// 点击处喷一串向上漂浮的字符泡泡（字符取自该人自己的池子）。
type ThanksEntry = { name: string; homepage?: string; pool?: string[] };
const THANKS_THANKS = [
  "🙏", "🙇", "🎉", "✨", "🌟", "💐", "🍻", "🎊", tr("谢谢"), tr("感谢"),
  "💻", "🖥️", "⌨️", "🖱️", "🔧", "⚙️", "🛠️", "💾", "⚡", "🎧",
  "(◍•ᴗ•◍)", "m(_ _)m", "(*´▽`*)", "ヽ(•‿•)ノ",
];
const THANKS_HEARTS = [
  "❤️", "🧡", "💛", "💚", "💙", "💜", "💗", "💖", "💕", "💞", "♥",
  "🥕", "(♡˙︶˙♡)", "(´▽`ʃ♡ƪ)",
];
const THANKS_ART = [
  "🎨", "🖌️", "🖍️", "✏️", "🖼️", "🎭", "🌈", "🖊️", "✒️", "📐", "✨",
  "(๑•̀ㅂ•́)و✧", "٩(ˊᗜˋ*)و", "✿",
];
const THANKS_IT = [
  "💻", "🖥️", "⌨️", "🖱️", "🖨️", "💾", "💿", "📟", "🛰️", "📡", "⚙️", "🔧",
  "⚡", tr("专业"), "( •̀ ω •́ )✧",
];
const THANKS: ThanksEntry[] = [
  { name: tr("大爷"), pool: THANKS_THANKS },
  { name: tr("胡萝北"), pool: THANKS_HEARTS },
  { name: tr("卯秀"), pool: THANKS_ART },
  { name: tr("青风云羽"), pool: THANKS_IT },
  { name: tr("斗机叔圣咏"), pool: THANKS_THANKS },
  { name: tr("楚轩"), pool: ["🐷", "🐽", "🐖", ...THANKS_THANKS] },
];

// 从鼠标点击处向四周炸开一串字符泡泡：随机方向与距离、随机字号，由透明变不透明后淡出，动画结束自删。
function burstBubbles(x: number, y: number, pool: string[]) {
  for (let i = 0; i < 14; i++) {
    const b = el("span", "thanks-bubble", pool[Math.floor(Math.random() * pool.length)]);
    b.style.left = `${x}px`;
    b.style.top = `${y}px`;
    const angle = Math.random() * Math.PI * 2;
    const dist = 60 + Math.random() * 110;
    b.style.setProperty("--dx", `${Math.round(Math.cos(angle) * dist)}px`);
    b.style.setProperty("--dy", `${Math.round(Math.sin(angle) * dist)}px`);
    b.style.fontSize = `${16 + Math.random() * 12}px`;
    b.style.animationDelay = `${i * 25}ms`;
    b.addEventListener("animationend", () => b.remove(), { once: true });
    document.body.append(b);
  }
}

function renderThanks() {
  const list = document.getElementById("thanks-list");
  if (!list) return;
  list.replaceChildren(
    ...THANKS.map((t) => {
      const node: HTMLElement = el(t.homepage ? "a" : "button", "thanks-name", t.name);
      if (t.homepage) {
        const a = node as HTMLAnchorElement;
        a.href = t.homepage;
        a.target = "_blank";
        a.rel = "noreferrer";
      } else {
        (node as HTMLButtonElement).type = "button";
      }
      // 按下缩一下；松手回弹（放大再回原样），没首页的顺带在点击处炸一串字符。
      let px = 0;
      let py = 0;
      node.addEventListener("pointerdown", (e) => {
        px = e.clientX;
        py = e.clientY;
        node.classList.add("pressing");
        node.setPointerCapture(e.pointerId);
      });
      const release = () => {
        if (!node.classList.contains("pressing")) return;
        node.classList.remove("pressing");
        node.classList.remove("spring");
        void node.offsetWidth; // 重启动画
        node.classList.add("spring");
        if (!t.homepage) burstBubbles(px, py, t.pool ?? THANKS_THANKS);
      };
      node.addEventListener("pointerup", release);
      node.addEventListener("pointercancel", release);
      node.addEventListener("animationend", () => node.classList.remove("spring"));
      return node;
    }),
  );
}

function bind() {
  document.querySelectorAll<HTMLButtonElement>(".rail-item").forEach((b) => {
    b.addEventListener("click", () => void switchSection(b.dataset.tab as Section));
  });
  // 位置浮层（<details>）：点外面任一处置回收起（原生 details 不帮这个忙）。
  document.addEventListener("click", (e) => {
    if ((e.target as HTMLElement | null)?.closest(".spot-field")) return;
    document
      .querySelectorAll<HTMLDetailsElement>(".spot-field[open]")
      .forEach((d) => (d.open = false));
  });
  startBrandLogoLoop();

  renderThanks();

  // 「＋ 新增」下拉：快捷键 / 目录 / 粘贴。改键、文本扩展、鼠标操作与触摸板手势不从这里进——
  // 统一编辑页里新增态点左上角的模式下拉就能切过去（见 switchDraftKind），入口只留一处。
  document.getElementById("add-btn")!.addEventListener("click", (e) => {
    const items: MenuItem[] = [
      { label: tr("新增快捷键"), run: () => void requestNew("shortcut") },
      { label: tr("新增文件夹"), run: () => addFolder(null) },
    ];
    if (state.clipboard) items.push({ label: tr("粘贴到根级"), run: () => pasteIntoFolder(null) });
    showMenu(e.currentTarget as HTMLElement, items);
  });

  // 脏标记：编辑页里任何一次输入/选择/点按都可能改动草稿，用事件委托统一重算，
  // 好过给每个字段（含动态生成的动作字段）逐个插桩。
  {
    const pane = document.getElementById("detail-editor")!;
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

  // 删除 / 外部改动确认：遮罩与 Esc 一律按「取消」——破坏性操作绝不能被一次误触放行。
  // 回车不接全局监听：「取消」默认拿焦点，回车就是取消；真要点「删除」得自己移过去。
  document.getElementById("confirm-ok")!.addEventListener("click", () => resolveConfirm(true));
  document.getElementById("confirm-cancel")!.addEventListener("click", () => resolveConfirm(false));
  document.getElementById("confirm-modal")!.querySelector(".modal-mask")!
    .addEventListener("click", () => resolveConfirm(false));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && confirmPending) {
      e.preventDefault();
      resolveConfirm(false);
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

  document.getElementById("add-layer-btn")!.addEventListener("click", addLayer);

  document.getElementById("list-search")!.addEventListener("input", (e) => {
    state.search = (e.target as HTMLInputElement).value;
    renderListPane();
  });

  // 筛选下拉（与搜索叠加；改完只重画列表栏，同搜索路径）。勾上 = 生效。
  document.getElementById("filter-btn")!.addEventListener("click", (e) => {
    const flip = (k: "enabled" | "conflicted") => () => {
      state.listFilter[k] = !state.listFilter[k];
      renderListPane();
    };
    showMenu(e.currentTarget as HTMLElement, [
      { label: tr("已启用"), on: state.listFilter.enabled, run: flip("enabled") },
      { label: tr("有冲突"), on: state.listFilter.conflicted, run: flip("conflicted") },
    ]);
  });

  // 菜单外部点击 / 别处右键 → 收起。常驻监听（菜单是同一个元素反复开关，不能用 once）。
  document.addEventListener("click", (e) => closeMenuUnlessIn(e.target as HTMLElement));
  document.addEventListener("contextmenu", (e) =>
    closeMenuUnlessIn(e.target as HTMLElement),
  );

  // 列表/详情滚动时菜单留在原地会盖错地方，直接收掉（scroll 不冒泡，用捕获阶段一次管全部滚动容器）。
  document.addEventListener("scroll", hideRowMenu, true);

  // 触发词内联校验（空/含空白即标红；保存处另有硬拦截）。
  document.getElementById("edit-exp-trigger")!.addEventListener("input", (e) => {
    const input = e.target as HTMLInputElement;
    showFieldError(input, expansionTriggerError(input.value.trim()));
  });

  document.getElementById("add-action")!.addEventListener("click", () => {
    const host = currentActionsHost();
    if (!host) return;
    host.actions.push(newAction("text"));
    renderActions(host);
  });

  // 新建的分组是空的、也是展开的：用户接着往里拖动作或点组内的「添加动作」即可。
  document.getElementById("add-group")!.addEventListener("click", () => {
    const host = currentActionsHost();
    if (!host) return;
    host.actions.push(newAction("group"));
    renderActions(host);
  });

  fillTouchpadTriggerSelect();
  const touchpadTrigger = document.getElementById("edit-touchpad-trigger") as HTMLSelectElement;
  touchpadTrigger.addEventListener("change", () => {
    if (state.section !== "touchpads" || !state.touchpadDraft) return;
    state.touchpadDraft.trigger = touchpadTrigger.value || null;
    // 换触发键 = 换了「哪只手 / 哪个键」录的那一笔：形状模板跟着作废（要留就重新录入）。
    state.touchpadDraft.template = null;
    syncTouchpadUi();
    refreshDirty();
  });
  document.getElementById("edit-touchpad-capture")!.addEventListener("click", () => {
    if (state.section !== "touchpads" || !state.touchpadDraft) return;
    const key = state.touchpadDraft.trigger;
    if (!key) {
      toast(tr("先选一个键盘键作触发键，再用触摸板录入"));
      return;
    }
    openTrackpadRecord(key);
  });
  document.getElementById("edit-touchpad-drop-template")!.addEventListener("click", () => {
    if (!state.touchpadDraft?.template) return;
    // 纯形状条目（方向串留空）只有模板这一个判据，清掉它这条就永远匹配不上了。
    if (!state.touchpadDraft.pattern.trim()) {
      toast(tr("方向串为空时不能清除形状模板（先填方向串）"));
      return;
    }
    state.touchpadDraft.template = null;
    syncTouchpadUi();
    refreshDirty();
    toast(tr("已清除形状模板，只保留方向串"));
  });

  document.getElementById("edit-gesture-record")!.addEventListener("click", () => {
    if (state.section !== "gestures" || !state.gestureDraft) return;
    openGesturePad((pattern) => {
      const input = document.getElementById("edit-gesture-pattern") as HTMLInputElement;
      input.value = pattern;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      if (state.gestureDraft) {
        state.gestureDraft.pattern = pattern;
        renderActions(state.gestureDraft, pattern);
      }
      toast(trt`已录入方向串 ${pattern}，记得保存`);
    });
  });

  document.getElementById("edit-capture")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (state.section !== "shortcuts" || !state.draft) return;
    const d = state.draft;
    startCapture((combo) => {
      if (!d.triggers.includes(combo)) d.triggers.push(combo);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  document.getElementById("edit-sequence")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (state.section !== "shortcuts" || !state.draft) return;
    const d = state.draft;
    startSequenceCapture((seq) => {
      if (!d.triggers.includes(seq)) d.triggers.push(seq);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  document.getElementById("edit-chord")!.addEventListener("click", (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    if (state.section !== "shortcuts" || !state.draft) return;
    const d = state.draft;
    startChordCapture((chord) => {
      if (!d.triggers.includes(chord)) d.triggers.push(chord);
      renderTriggers(d);
      void refreshConflicts().then(renderConflictBubble);
    }, btn);
  });

  // 标题行右侧的五个图标按钮（三类条目共用一套 id；执行只对快捷键出现，见 syncEntry）。
  document.getElementById("entry-run")!.addEventListener("click", () => void runCurrentEntry());
  document.getElementById("entry-save")!.addEventListener("click", () => {
    void saveCurrentDraft();
  });
  document.getElementById("entry-cancel")!.addEventListener("click", () => {
    void requestCloseDetail();
  });
  document.getElementById("entry-undo")!.addEventListener("click", revertDraft);
  document.getElementById("entry-delete")!.addEventListener("click", deleteCurrentEntry);

  // 模式选择（Postman 式下拉）：图标进菜单项（常量 SVG），点项切模式（新增态才切得动，见 syncEntry）。
  document.querySelectorAll("#mode-menu .mode-item").forEach((item) => {
    const kind = (item as HTMLElement).dataset.kind as ListKind;
    const icon = item.querySelector<HTMLElement>(".mi-icon");
    if (icon) icon.innerHTML = KIND_ICON[kind]; // 常量 SVG，非用户输入
  });
  document.getElementById("mode-trigger")!.addEventListener("click", (e) => {
    e.stopPropagation(); // 先别让 document 那条关闭监听把它立刻收掉
    document.getElementById("mode-menu")!.classList.toggle("hidden");
  });
  document.getElementById("mode-menu")!.addEventListener("click", (e) => {
    const item = (e.target as HTMLElement).closest<HTMLElement>(".mode-item");
    if (!item?.dataset.kind || item.classList.contains("disabled")) return;
    closeModeMenu();
    switchDraftKind(item.dataset.kind as ListKind);
  });
  document.addEventListener("click", closeModeMenu); // 点别处收起

  // 页签切换（详情 / 动作·操作·展开文本 / 层 / 条件）。
  document.getElementById("entry-tabs")!.addEventListener("click", (e) => {
    const btn = (e.target as HTMLElement).closest<HTMLElement>(".entry-tab");
    if (!btn?.dataset.tab) return;
    state.entryTab = btn.dataset.tab as EntryTab;
    applyEntryTab();
  });

  // 改键「原键」输入框 = 可过滤的按键下拉。点开（focus / click）先列全部按键，
  // 而不是拿框里已有的键去过滤（否则选好一个键后再点开只剩那一个）。
  const fromInput = document.getElementById("remap-from") as HTMLInputElement;
  fromInput.addEventListener("focus", () => renderKeyMenu(""));
  fromInput.addEventListener("click", () => renderKeyMenu(""));
  fromInput.addEventListener("input", () => {
    if (state.remapDraft) state.remapDraft.from = fromInput.value;
    renderKeyMenu(fromInput.value);
    refreshDirty();
  });
  fromInput.addEventListener("blur", closeKeyMenu);
  fromInput.addEventListener("keydown", (e) => {
    // 按到小键盘键 = 直接选这个键（NumLock 关着时它打出来的不是键名，而是导航/字符）。
    const numpad = numpadKeyFromEvent(e);
    if (numpad) {
      e.preventDefault();
      pickRemapKey(numpad);
      return;
    }
    if (e.key === "Escape") {
      e.stopPropagation(); // 只收下拉，别顺手把弹窗/菜单也关了
      closeKeyMenu();
    } else if (e.key === "Enter") {
      // 回车 = 取列表第一项（输入过滤后剩下的那个），不用非得点鼠标。
      const first = document.querySelector<HTMLElement>("#key-menu .key-item");
      if (first) {
        e.preventDefault();
        pickRemapKey(first.textContent ?? fromInput.value);
      }
    }
  });

  fillKeySelect(document.getElementById("remap-to") as HTMLSelectElement, "Ctrl");
  fillKeySelect(document.getElementById("remap-tap") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-hold") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-oneshot") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-sticky") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-tap2") as HTMLSelectElement, "", true);
  fillKeySelect(document.getElementById("remap-tap3") as HTMLSelectElement, "", true);
  document.getElementById("remap-tap")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.tap = (e.target as HTMLSelectElement).value || null;
    syncRemapEditor();
  });
  document.getElementById("remap-hold")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.hold = (e.target as HTMLSelectElement).value || null;
    if (state.remapDraft.hold) {
      // 长按输出键与切层互斥（进入层 / 锁定层都算切层）。
      state.remapDraft.hold_layer = null;
      state.remapDraft.lock_layer = null;
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
      (document.getElementById("remap-lock-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-hold-layer")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.hold_layer = (e.target as HTMLSelectElement).value || null;
    if (state.remapDraft.hold_layer) {
      state.remapDraft.hold = null;
      state.remapDraft.lock_layer = null; // 进入层与锁定层互斥（切换式才用得起层内和弦/序列）
      (document.getElementById("remap-hold") as HTMLSelectElement).value = "";
      (document.getElementById("remap-lock-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-lock-layer")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.lock_layer = (e.target as HTMLSelectElement).value || null;
    if (state.remapDraft.lock_layer) {
      state.remapDraft.hold = null;
      state.remapDraft.hold_layer = null;
      (document.getElementById("remap-hold") as HTMLSelectElement).value = "";
      (document.getElementById("remap-hold-layer") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-oneshot")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.oneshot = (e.target as HTMLSelectElement).value || null;
    if (state.remapDraft.oneshot) {
      state.remapDraft.sticky = null; // 单次与粘滞互斥
      (document.getElementById("remap-sticky") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-sticky")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.sticky = (e.target as HTMLSelectElement).value || null;
    if (state.remapDraft.sticky) {
      state.remapDraft.oneshot = null;
      (document.getElementById("remap-oneshot") as HTMLSelectElement).value = "";
    }
    syncRemapEditor();
  });
  document.getElementById("remap-tap2")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.tap2 = (e.target as HTMLSelectElement).value || null;
    syncRemapEditor();
  });
  document.getElementById("remap-tap3")!.addEventListener("change", (e) => {
    if (!state.remapDraft) return;
    state.remapDraft.tap3 = (e.target as HTMLSelectElement).value || null;
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

    // Ctrl+A：只在文本框（输入框 / 文本域 / contenteditable）里放行原生全选。
    // 复选框 / 颜色 / 下拉这类控件上拦掉——WebView 会把 Ctrl+A 退化成整页全选。
    if ((e.ctrlKey || e.metaKey) && !e.altKey && (e.key === "a" || e.key === "A")) {
      if (!isTextEntryTarget(e.target)) e.preventDefault();
      return;
    }

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

    // 弹窗在场时全局键让路（模态自己的按钮与 Esc 自理，见 `bind` 里各自动作）。
    if (document.querySelector(".modal:not(.hidden)")) return;
    const mod = e.ctrlKey || e.metaKey;

    // Ctrl+S：保存当前编辑页，等价点标题行的保存图标（含校验与提示）。没改动就与置灰的按钮同一口径，不落盘。
    if (mod && !e.altKey && (e.key === "s" || e.key === "S")) {
      e.preventDefault();
      if (isListSection(state.section) && currentDraft() && isDraftDirty()) void saveCurrentDraft();
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

    // Delete：删当前详情页这条（与标题行的删除图标同一条路径，含确认框）；焦点在表单控件里时不动。
    if (e.key === "Delete" && !isFormTarget(e.target) && isListSection(state.section) && currentDraft()) {
      e.preventDefault();
      deleteCurrentEntry();
    }
  });

  bindSettings();
}

// 切页会丢弃草稿（草稿只属于当前编辑页），所以先过一道未保存守卫。
async function switchSection(tab: Section) {
  if (!(await confirmLeaveDraft())) return;
  switchSectionNow(tab);
}

// 品牌字标：固定节奏的自动演示——亮一会儿，四字一起按下，再回弹并迸出星屑。
// 纯装饰，不响应鼠标；系统开了「减少动态效果」就整段不跑，只留静止的发光字。
function startBrandLogoLoop() {
  const brand = document.querySelector<HTMLElement>(".rail-brand");
  if (!brand) return;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
  // 整拍 6s：按下 240ms → 回弹 + 星屑（760ms 内播完）→ 余下时间静止微闪。
  const CYCLE_MS = 6000;
  const PRESS_MS = 240;
  const BURST_MS = 760;

  const beat = () => {
    brand.classList.add("is-pressed");
    window.setTimeout(() => {
      brand.classList.remove("is-pressed");
      // 每次先摘类、强制一次样式计算再挂回，星屑动画才会从头重播。
      brand.classList.remove("is-burst");
      void brand.offsetWidth;
      brand.classList.add("is-burst");
      window.setTimeout(() => brand.classList.remove("is-burst"), BURST_MS);
    }, PRESS_MS);
    window.setTimeout(beat, CYCLE_MS);
  };

  // 首屏先亮 1.5 秒再按第一下，别一进界面就是暗的。
  window.setTimeout(beat, 1500);

  // 窗口最小化 / 隐藏时定时器会被节流，可能正好卡在「按下」那一帧；
  // 回到前台先清掉中间态，别让字标停在暗色上等下一拍。
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) brand.classList.remove("is-pressed", "is-burst");
  });
}

// rail 有 6 个页签：改键 / 文本扩展没有自己的 rail 项，它们的列表行就长在「快捷键」这一页里
// （合并列表），所以落到「快捷键」上高亮，不另造页签；「键位层」与「卡带市场」都是独立一级页。
function syncRailTab(tab: Section) {
  // 改键 / 文本扩展 / 鼠标操作 / 触摸板手势都并进「快捷键」那一页（列表只有一份，行首图标区分类型）。
  const rail =
    tab === "remaps" ||
    tab === "expansions" ||
    tab === "gestures" ||
    tab === "touchpads" ||
    tab === "hotcorners" ||
    tab === "wheeledges"
      ? "shortcuts"
      : tab;
  document.querySelectorAll<HTMLButtonElement>(".rail-item").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === rail);
  });
}

// keepList：列表分区之间跳转（点行开别的类别的编辑页）时保住搜索/筛选/多选，
// 别把用户刚敲的关键词抹掉；切到消息/设置这类另起一页时仍整套复位。
export function switchSectionNow(tab: Section, opts?: { keepList?: boolean }) {
  void stopRecIfAny();
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  state.section = tab;
  if (!opts?.keepList) {
    state.search = "";
    state.listFilter = { enabled: false, conflicted: false };
    state.picked.clear();
    state.pickAnchor = null;
    (document.getElementById("list-search") as HTMLInputElement).value = "";
  }
  hideRowMenu();
  if (tab === "messages") markRead();
  syncRailTab(tab);
  // 市场页一进来就拉索引（已拿到的直接复用；上次失败的下次进来会重试）。
  if (tab === "market") void loadMarket();
  render();
}

async function stopRecIfAny() {
  if (state.recording) {
    // 录制按钮长在「按键组合」动作卡片里，随重绘自然复位；这里只收掉后端的时间线，
    // 不把录到的东西塞进任何草稿（离开编辑页 = 这次录制作废）。
    state.recording = false;
    await invoke("stop_record").catch(() => {});
  }
  if (state.mouseRecording) {
    state.mouseRecording = false;
    await invoke("stop_mouse_record").catch(() => {});
  }
}

// ---- 消息中心 / 结果卡深链（U-03 已搬到 ./messages）----
// renderMessages / bindMessages / 结果卡与「定位到动作」由 messages.ts 提供。


async function initEvents() {
  await listen<CommandResult>("command-result", (event) => {
    const r = event.payload;
    if (r.run_id === null) {
      // 非触发的普通消息（配置变更、热串注入失败等）：单独成条，弹窗 / 红点照旧。
      state.messages.unshift(r);
      state.msgSelId = `m${r.time}-${r.kind}-${r.command}`;
    }
    if (r.show_output) {
      showResultModal(r);
      // 冷启动补发的那条已经由启动流程弹过（或正躺在 pending 里）：窗口已开时新事件
      // 走监听器，把可能残留的 pending 清掉，避免同一条再弹第二次（CS-11）。
      void invoke("take_pending_result_modal").catch(() => null);
    } else {
      state.unread = true;
    }
    renderMessages();
  });
  // 冷启动补发（CS-11）：主窗口是 commit_result 之后才建出来的，事件早于监听器注册，
  // 后端把「弹窗结果」暂存了一份；监听完再取，取走即清。
  const pendingResult = await invoke<CommandResult | null>("take_pending_result_modal").catch(
    () => null,
  );
  if (pendingResult) showResultModal(pendingResult);
  // 执行记录：每条动作结束就推一次整张卡（后台线程边跑边刷，界面能看到逐步推进）。
  await listen<RunRecord>("run-update", (event) => {
    const run = event.payload;
    const i = state.runs.findIndex((r) => r.id === run.id);
    if (i >= 0) state.runs[i] = run;
    else {
      state.runs.unshift(run);
      state.msgSelId = `r${run.id}`;
      state.msgIndex = 0;
      state.unread = true;
    }
    renderMessages();
  });
  await listen<UpdateStatus>("update-status", (event) => {
    updateStatus = event.payload;
    syncUpdate();
  });
  await listen<UpdateDialogPayload>("update-dialog", (event) => openUpdateDialog(event.payload));
  // 主窗口刚被唤起时事件可能已落空：补取后端暂存的那一条（取走即清，不会反复弹）。
  const pendingDialog = await invoke<UpdateDialogPayload | null>("take_update_dialog").catch(() => null);
  if (pendingDialog) openUpdateDialog(pendingDialog);
  // 触摸板「录入」的轨迹回传（壳层在松键后推一次）：交给当前打开的录制面板。
  await listen<TrackpadRecordPayload>("trackpad-recorded", (e) =>
    trackpadRecordSink?.(e.payload),
  );
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
      state.cfg.settings.hints = next;
      // 只动这一个勾：整份 `syncSettings()` 会把用户正在输入框里敲到一半的超时数字覆盖掉。
      (document.getElementById("set-hints") as HTMLInputElement).checked = next.visible === true;
    }
    const paused = event.payload.settings?.paused === true;
    if (paused !== state.cfg.settings.paused) {
      state.cfg.settings.paused = paused;
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
      payload?.subtitle ?? (name ? `${trigger} · ` : "") + tr("正在执行…");
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
  const MOD_KIND_LABEL: Record<string, string> = { hold: tr("按住"), sticky: tr("粘滞"), oneshot: tr("单次") };
  const render = (payload: StatusPayload | null) => {
    row.replaceChildren();
    if (payload?.layer) {
      const chip = el("span", "hud-chip hud-layer");
      chip.textContent = trt`层 · ${payload.layer}${payload.locked ? tr("（锁定）") : tr("（按住）")}`;
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

// 「设置变量」输入窗（独立窗口，加载 index.html#prompt）：动作链在壳层等着这里回传。
// 文案与配色全走主题变量（与主界面 / 其余浮窗同一套），四个主题不用另写一套。
type PromptPayload = { prompt: string; default: string };

// CSS 投影占的透明边（`body` 的 padding），量尺寸时要一起报给壳层。
const PROMPT_PAD = 14;

async function bootstrapPrompt() {
  for (const sel of [
    ".layout",
    "#result-modal",
    "#save-status",
    "#row-menu",
    "#unsaved-modal",
    "#import-modal",
    "#import-result-modal",
    "#snippet-modal",
  ]) {
    document.querySelector(sel)?.remove();
  }
  const card = el("form", "prompt-card");
  const text = el("div", "prompt-text");
  const input = el("input", "prompt-input") as HTMLInputElement;
  input.type = "text";
  const actions = el("div", "prompt-actions");
  const cancel = el("button", "ghost", tr("取消")) as HTMLButtonElement;
  cancel.type = "button";
  const ok = el("button", "primary", tr("确定")) as HTMLButtonElement;
  ok.type = "submit";
  actions.append(cancel, ok);
  card.append(text, input, actions);
  document.body.append(card);

  // 一次只填一个框；`armed` 挡住重复回传（回车 + 关窗都会走到 `finish`）。
  let armed = false;
  const finish = (value: string | null) => {
    if (!armed) return;
    armed = false;
    void invoke("prompt_done", { value }).catch(() => {});
  };
  const render = (p: PromptPayload | null) => {
    if (!p) return;
    armed = true;
    text.textContent = p.prompt || tr("请输入内容");
    input.value = p.default;
    // 量完内容再报尺寸：壳层据此缩放、居中并显示（与 HUD / 提示框同一套路）。
    const rect = card.getBoundingClientRect();
    void invoke("prompt_ready", {
      width: Math.ceil(rect.width) + PROMPT_PAD * 2,
      height: Math.ceil(rect.height) + PROMPT_PAD * 2,
    })
      .then(() => {
        input.focus();
        input.select();
      })
      .catch(() => {});
  };
  card.addEventListener("submit", (e) => {
    e.preventDefault();
    finish(input.value);
  });
  cancel.addEventListener("click", () => finish(null));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      finish(null);
    }
  });
  await listen<PromptPayload>("prompt-show", (e) => render(e.payload));
  // 首次懒创建后补一次拉取：窗口刚建好时 prompt-show 可能早于监听器注册。
  render(await invoke<PromptPayload | null>("get_prompt_payload").catch(() => null));
}

// 快捷键提示框（独立窗口，加载 index.html#hints，见规划 7.3-㊳）：
// 常显的快捷键速查表——内容来自配置（按层分组），位置 / 字号 / 透明度由后端持久化。
// 交互：拖整块面板移动、滚轮调大小、Ctrl+滚轮调透明度、Shift+滚轮滚动长列表、× 关闭。
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
  head.append(el("span", "hints-title", tr("快捷键速查")));
  const close = el("button", "hints-close", "×");
  close.title = tr("关闭提示框（可从设置页或托盘菜单重新打开）");
  head.append(close);
  const body = el("div", "hints-body");
  const foot = el(
    "div",
    "hints-foot",
    // 两行是量着排的，别合回一行：21em 面板的内容宽只有 ~250px，这串提示单行要 288px，
    // 挤成一行末尾的「拖面板移动」就会被省略号吃掉（`style.css` 里 `.hints-foot` 配合）。
    tr("滚轮：大小 · Ctrl+滚轮：透明度\n拖面板：移动 · Shift+滚轮：滚动"),
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
  // 让位状态：后端已把窗口设成鼠标穿透（光标进框即让位，停留 2 秒才恢复，见
  // `spawn_hints_hover_watch`）。穿透之后界面就再也收不到鼠标事件，只能由它通知——
  // 变淡是给用户的信号：它已经不接鼠标了，想拖 / 想滚轮就先停一会儿。
  let peeking = false;

  const applyLook = () => {
    // 整块面板按比例缩放：字号是基准，间距 / 圆角 / 宽度都用 em，于是「调大小」是一个量。
    document.documentElement.style.setProperty("--hints-scale", String(scale / 100));
    panel.style.opacity = String((peeking ? opacity * 0.3 : opacity) / 100);
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
    if (base.length) groups.push({ title: tr("基础层"), items: base });
    for (const l of c.layers) {
      const items = byLayer.get(l.id);
      if (items?.length) groups.push({ title: l.name, items });
    }
    if (!groups.length) {
      body.append(el("div", "hints-empty", tr("还没有启用的快捷键")));
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
        const label = el("span", "hints-label", name || desc || tr("（未命名）"));
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
  // 保证光标滑出窗口也继续跟手。**整块面板都是拖动区**（不只标题）——面板不小，能抓着
  // 哪儿挪就挪哪儿；两处例外是 × 按钮和列表自己的滚动条（见下）。
  let drag: { sx: number; sy: number; wx: number; wy: number; nx: number; ny: number } | null = null;
  panel.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    if ((e.target as HTMLElement).closest(".hints-close")) return;
    // 滚动条不是拖动区：点在滚动条上要能拖列表，而不是把整个框搬走。
    // `offsetX > clientWidth` 即落在滚动条那一窄条上（滚动条不占 layout 盒，故拿 clientWidth 比）。
    if (e.target === body && e.offsetX > body.clientWidth) return;
    e.preventDefault();
    drag = { sx: e.screenX, sy: e.screenY, wx: winPos.x, wy: winPos.y, nx: winPos.x, ny: winPos.y };
    panel.setPointerCapture(e.pointerId);
    panel.classList.add("dragging");
  });
  panel.addEventListener("pointermove", (e) => {
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
      panel.releasePointerCapture(e.pointerId);
    } catch {
      /* 指针已经没了，忽略 */
    }
    // 松手才落盘一次（拖动中每帧写盘既慢又浪费）。
    void invoke("hints_commit").catch(() => {});
  };
  panel.addEventListener("pointerup", endDrag);
  panel.addEventListener("pointercancel", endDrag);

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
  // 悬停让位 / 恢复：只改透明度，不动位置与尺寸。
  await listen<boolean>("hints-peek", (e) => {
    peeking = e.payload;
    applyLook();
  });
  // 快捷键改了就重画：应用内保存走 `config-updated`，手改 JSON 走 `config-changed`。
  await listen<Config>("config-updated", (e) => render(e.payload));
  await listen<Config>("config-changed", (e) => render(e.payload));
}

// 手势轨迹浮层（独立窗口，加载 index.html#gesture，EXT-02）：
// 画的时候背后端每帧推轨迹点，这里只画线 + 末端箭头；松手后显示动作名 / 失败色。
// 窗口本身不可激活、鼠标穿透，显示与隐藏全由后端控制。
type GestureVisual = {
  trail_color: string;
  trail_width: number;
  label_font_size: number;
  label_offset: number;
  fail_color: string;
};
type GesturePreviewPayload = GestureVisual & {
  points: [number, number][];
  scale: number;
};
type GestureResultPayload = GesturePreviewPayload & {
  matched: boolean;
  label: string;
  pattern: string;
};

async function bootstrapGesture() {
  document.querySelector(".layout")?.remove();
  document.querySelector("#result-modal")?.remove();
  document.querySelector("#save-status")?.remove();

  const SVG_NS = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", "gesture-overlay");
  const line = document.createElementNS(SVG_NS, "polyline");
  line.setAttribute("fill", "none");
  line.setAttribute("stroke-linecap", "round");
  line.setAttribute("stroke-linejoin", "round");
  const arrow = document.createElementNS(SVG_NS, "polygon");
  svg.append(line, arrow);
  const label = el("div", "gesture-label");
  document.body.append(svg, label);

  const render = (
    p: GesturePreviewPayload,
    matched: boolean | null,
    name = "",
    pattern = "",
  ) => {
    const color = matched === false ? p.fail_color : p.trail_color;
    const pts = p.points.map(
      ([x, y]) => [x / (p.scale || 1), y / (p.scale || 1)] as [number, number],
    );
    line.setAttribute("stroke", color);
    line.setAttribute("stroke-width", String(p.trail_width));
    line.setAttribute("points", pts.map(([x, y]) => `${x},${y}`).join(" "));

    const last = pts[pts.length - 1];
    const prev = pts[pts.length - 2];
    if (last && prev) {
      const dx = last[0] - prev[0];
      const dy = last[1] - prev[1];
      const len = Math.hypot(dx, dy) || 1;
      const size = 10 + p.trail_width * 1.5;
      const bx = last[0] - (dx / len) * size * 1.6;
      const by = last[1] - (dy / len) * size * 1.6;
      const px = (-dy / len) * size * 0.6;
      const py = (dx / len) * size * 0.6;
      arrow.setAttribute("fill", color);
      arrow.setAttribute(
        "points",
        `${last[0]},${last[1]} ${bx + px},${by + py} ${bx - px},${by - py}`,
      );
      arrow.style.display = "";
    } else {
      arrow.style.display = "none";
    }

    const text = matched === null ? "" : matched ? name : trt`未匹配 ${pattern}`;
    label.textContent = text;
    label.style.display = text ? "block" : "none";
    label.style.color = color;
    label.style.fontSize = `${p.label_font_size}px`;
    if (last) {
      label.style.left = `${last[0] + p.label_offset}px`;
      label.style.top = `${last[1] + p.label_offset}px`;
    }
  };

  await listen<GesturePreviewPayload>("gesture-preview", (e) => render(e.payload, null));
  await listen<GestureResultPayload>("gesture-result", (e) =>
    render(e.payload, e.payload.matched, e.payload.label, e.payload.pattern),
  );
  // 监听器就绪后再告诉壳层：冷启动第一笔手势的轨迹事件不会白丢。
  await invoke("gesture_overlay_ready").catch(() => {});
}

// ---- 复制快捷菜单（独立窗口，加载 index.html#clipboard-menu，EXT-13）----
// 壳层只推「展示树」，点击回传配置下标路径；动作执行在壳层（与快捷键同一条 fire 链路）。
// 窗口不可激活、收不到失焦事件：Esc / 点外面 / 前台切换的收回全由壳层轮询线程负责。
type MenuNodeView =
  | { kind: "action"; label: string; enabled: boolean; path: number[] }
  | { kind: "submenu"; label: string; enabled: boolean; items: MenuNodeView[] }
  | { kind: "separator" };
type ClipboardMenuPayload = { items: MenuNodeView[] };

async function bootstrapClipboardMenu(preview = false) {
  for (const sel of [
    ".layout",
    "#result-modal",
    "#save-status",
    "#row-menu",
    "#unsaved-modal",
    "#import-modal",
    "#import-result-modal",
    "#snippet-modal",
  ]) {
    document.querySelector(sel)?.remove();
  }
  const panel = el("div", "clipboard-menu-panel");
  panel.addEventListener("contextmenu", (e) => e.preventDefault());
  document.body.append(panel);

  // 内容变了（渲染 / 展开子菜单）就回报尺寸：壳层据此定位并钳进工作区。
  const measure = () =>
    requestAnimationFrame(() => {
      const rect = panel.getBoundingClientRect();
      void invoke("clipboard_menu_ready", {
        width: Math.ceil(rect.width),
        height: Math.ceil(rect.height),
      }).catch(() => {});
    });

  const renderNode = (n: MenuNodeView): HTMLElement => {
    if (n.kind === "separator") return el("div", "clipboard-menu-sep");
    if (n.kind === "submenu") {
      const group = el("div", "clipboard-menu-group");
      const head = el("button", "clipboard-menu-item submenu") as HTMLButtonElement;
      head.disabled = !n.enabled;
      head.title = n.label;
      head.append(
        el("span", "clipboard-menu-label", n.label),
        el("span", "clipboard-menu-caret", "▸"),
      );
      // 子菜单就地展开（不用独立飞出层：窗口尺寸由壳层按内容钳，飞出层会被窗口边界裁掉）。
      head.addEventListener("click", () => {
        group.classList.toggle("open");
        measure();
      });
      const sub = el("div", "clipboard-menu-sub");
      for (const c of n.items) sub.append(renderNode(c));
      group.append(head, sub);
      return group;
    }
    const b = el("button", "clipboard-menu-item") as HTMLButtonElement;
    b.textContent = n.label;
    b.disabled = !n.enabled;
    b.addEventListener("click", () => {
      if (!n.enabled) return;
      void invoke("clipboard_menu_exec", { path: n.path }).catch(() => {});
    });
    return b;
  };

  const render = (p: ClipboardMenuPayload) => {
    panel.replaceChildren(...p.items.map(renderNode));
    measure();
  };

  await listen<ClipboardMenuPayload>("clipboard-menu", (e) => render(e.payload));
  // 首次建窗时 emit 可能早于监听器注册（与 hud / toast 同一个套路）：拉一次兜底。
  const pending = await invoke<ClipboardMenuPayload | null>("get_clipboard_menu_payload").catch(
    () => null,
  );
  if (pending) {
    render(pending);
  } else if (preview) {
    render({
      items: [
        { kind: "action", label: tr("粘贴为纯文本"), enabled: true, path: [0] },
        { kind: "separator" },
        { kind: "action", label: tr("搜索选中文字"), enabled: true, path: [2] },
        {
          kind: "submenu",
          label: tr("转换"),
          enabled: true,
          items: [
            { kind: "action", label: tr("转成大写"), enabled: true, path: [3, 0] },
            { kind: "action", label: tr("转成小写"), enabled: false, path: [3, 1] },
          ],
        },
      ],
    });
  }
}

// ---- 托盘菜单（独立窗口，加载 index.html#tray-menu）----
// 原生托盘菜单由系统画（Windows 是 Win32 灰白菜单），配色 / 圆角 / 字体跟四套主题完全不搭；
// 这里自己画一份。壳层每次右键都推一整份载荷（勾选态现读配置），点击回 `id` 由壳层分发。
// 窗口不可激活、收不到失焦：收回全由壳层的光标巡检线程负责（见 `spawn_tray_menu_watch`）。
type TrayMenuItem =
  | {
      kind: "action";
      id: string;
      label: string;
      checkable: boolean;
      checked: boolean;
      danger: boolean;
    }
  | { kind: "separator" };
type TrayMenuPayload = { items: TrayMenuItem[] };

// 壳层给的 `label` 是中文原文（壳层文案还没有 i18n，见说明书 §6.9）；界面侧按 id 再翻一次，
// 英文界面才跟着变。认不出的 id 原样用壳层文案——新增条目忘补翻译时不至于整条空白。
function trayMenuLabel(id: string, fallback: string): string {
  switch (id) {
    case "show":
      return tr("打开咔哒");
    case "pause":
      return tr("暂停快捷键触发");
    case "hints":
      return tr("快捷键提示");
    case "reload":
      return tr("重载配置");
    case "update":
      return tr("检查更新");
    case "quit":
      return tr("退出");
    default:
      return fallback;
  }
}

async function bootstrapTrayMenu(preview = false) {
  for (const sel of [
    ".layout",
    "#result-modal",
    "#save-status",
    "#row-menu",
    "#unsaved-modal",
    "#import-modal",
    "#import-result-modal",
    "#snippet-modal",
  ]) {
    document.querySelector(sel)?.remove();
  }
  const panel = el("div", "tray-menu-panel");
  panel.addEventListener("contextmenu", (e) => e.preventDefault());
  document.body.append(panel);

  // 面板外留一圈透明边给投影（transparent 窗口会把投影一起裁掉），与壳层
  // `TRAY_MENU_MARGIN` / CSS 的 `padding` 是同一个口径。
  const PANEL_MARGIN = 10;

  // 内容变了就回报尺寸：壳层据此定位并钳进工作区（面板尺寸随条目文案变）。
  // **同步量、不要 `requestAnimationFrame`**：窗口建出来是 `visible(false)`，隐藏窗口的
  // rAF 会被 Chromium 冻住 → 尺寸永远回报不了 → `tray_menu_ready` 永不触发 → 菜单永远不显示
  // （hud / hints 也都是同步量的）。`getBoundingClientRect` 自己就会强制布局，不需要等一帧。
  const measure = () => {
    const rect = panel.getBoundingClientRect();
    void invoke("tray_menu_ready", {
      width: Math.ceil(rect.width) + PANEL_MARGIN * 2,
      height: Math.ceil(rect.height) + PANEL_MARGIN * 2,
    }).catch(() => {});
  };

  const renderItem = (n: TrayMenuItem): HTMLElement => {
    if (n.kind === "separator") return el("div", "tray-menu-sep");
    const b = el("button", "tray-menu-item" + (n.danger ? " danger" : "")) as HTMLButtonElement;
    b.type = "button";
    // 勾选位无论有没有勾都占一格，标题才能上下对齐。
    if (n.checkable) {
      b.classList.add("checkable");
      b.append(el("span", "tray-menu-check", n.checked ? "✓" : ""));
    }
    b.append(el("span", "tray-menu-label", trayMenuLabel(n.id, n.label)));
    b.addEventListener("click", () => {
      void invoke("tray_menu_exec", { id: n.id }).catch(() => {});
    });
    return b;
  };

  const render = (p: TrayMenuPayload) => {
    // 每次弹出都重读一次主题：这个窗口是懒创建后长期复用的，主界面在设置页切主题只改了
    // 自己那份 document + localStorage，不会通知到这里；不重读就会一直用**建窗那一刻**的主题
    // （字体 / 圆角 / 投影全跟着变，面板尺寸也得重量）。
    applyTheme(loadTheme());
    panel.replaceChildren(...p.items.map(renderItem));
    measure();
  };

  await listen<TrayMenuPayload>("tray-menu", (e) => render(e.payload));
  // 首次建窗时 emit 可能早于监听器注册（与 hud / toast 同一个套路）：拉一次兜底。
  const pending = await invoke<TrayMenuPayload | null>("get_tray_menu_payload").catch(() => null);
  if (pending) {
    render(pending);
  } else if (preview) {
    // 纯浏览器直开时给一份样例菜单，方便调样式（标签只在壳层缺 id 时才会被用到）。
    render({
      items: [
        { kind: "action", id: "show", label: tr("打开咔哒"), checkable: false, checked: false, danger: false },
        { kind: "separator" },
        { kind: "action", id: "pause", label: tr("暂停快捷键触发"), checkable: true, checked: true, danger: false },
        { kind: "action", id: "hints", label: tr("快捷键提示"), checkable: true, checked: false, danger: false },
        { kind: "separator" },
        { kind: "action", id: "reload", label: tr("重载配置"), checkable: false, checked: false, danger: false },
        { kind: "action", id: "update", label: tr("检查更新"), checkable: false, checked: false, danger: false },
        { kind: "separator" },
        { kind: "action", id: "quit", label: tr("退出"), checkable: false, checked: false, danger: true },
      ],
    });
  }
}

// ---- 自绘标题栏（主窗口）----
// 壳层把主窗口设成 `decorations(false)`（macOS 走 Overlay，红绿灯由系统画），标题区由这里补。
// 拖动 / 双击最大化不在这里做：Tauri 内建的 `data-tauri-drag-region` 脚本接管，且会自动跳过
// button / input / tabindex 这些可点元素。这里只管三颗按钮、最大化图标切换，以及 Linux 的
// 窗口缩放热区——GTK 的无边框窗没有系统缩放边，不给热区就只能靠按钮最大化。
// 只在主窗口挂：其余四个浮窗加载的是同一份 index.html，走各自的 hash 分支。
type ResizeDir =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

const WIN_ICONS = {
  min: '<path d="M2 6h8"/>',
  max: '<rect x="2.5" y="2.5" width="7" height="7"/>',
  restore: '<rect x="2.5" y="4" width="6" height="6"/><path d="M4.5 2.5h5v5"/>',
  close: '<path d="M3 3l6 6M9 3l-6 6"/>',
};

function winIcon(paths: string): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 12 12");
  svg.setAttribute("aria-hidden", "true");
  // 必须显式给描边：SVG 的默认样式是 `fill: black; stroke: none`，只写 `<path d=…>` 画出来的
  // 是一条填充黑块——深色标题栏上等于不可见（上一版三颗按钮只有最大化能看见一点黑）。
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.2");
  svg.innerHTML = paths;
  return svg;
}

function mountTitlebar() {
  // 这一行必须在 `installBrowserMock()` 之前求值：演示模式会塞一个假的
  // __TAURI_INTERNALS__，之后就分不清是真壳还是纯浏览器了。
  const shell = "__TAURI_INTERNALS__" in window;
  const ua = navigator.userAgent;
  const mac = /Macintosh|Mac OS X/.test(ua);
  const linux = !mac && /Linux/.test(ua);

  const bar = document.createElement("div");
  bar.className = "titlebar";
  // `deep` = 整条标题栏（含里面的文字块）都是拖动区；按钮没有这个属性，所以照常可点。
  bar.setAttribute("data-tauri-drag-region", "deep");
  const mark = document.createElement("i");
  mark.className = "titlebar-mark";
  const title = document.createElement("span");
  title.className = "titlebar-title";
  title.textContent = tr("咔哒 Kada");
  const spacer = document.createElement("span");
  spacer.className = "titlebar-spacer";
  bar.append(mark, title, spacer);

  document.body.classList.add("has-titlebar");
  if (mac) document.body.classList.add("is-mac");
  document.body.insertBefore(bar, document.body.firstChild);

  // macOS 的红绿灯由系统画在左上角，自绘按钮会重复，也不该抢那块地方。
  if (mac) return;

  const controls = document.createElement("div");
  controls.className = "win-controls";
  const minBtn = document.createElement("button");
  minBtn.className = "win-btn min";
  minBtn.append(winIcon(WIN_ICONS.min));
  minBtn.title = tr("最小化");
  minBtn.setAttribute("aria-label", tr("最小化"));
  const maxBtn = document.createElement("button");
  maxBtn.className = "win-btn max";
  let maximized = false;
  const paintMax = () => {
    maxBtn.replaceChildren(winIcon(maximized ? WIN_ICONS.restore : WIN_ICONS.max));
    const label = maximized ? tr("还原") : tr("最大化");
    maxBtn.title = label;
    maxBtn.setAttribute("aria-label", label);
  };
  paintMax();
  const closeBtn = document.createElement("button");
  closeBtn.className = "win-btn close";
  closeBtn.append(winIcon(WIN_ICONS.close));
  closeBtn.title = tr("关闭");
  closeBtn.setAttribute("aria-label", tr("关闭"));
  controls.append(minBtn, maxBtn, closeBtn);
  bar.append(controls);

  // 纯浏览器演示：按钮留着当视觉参照，但不接 IPC（演示环境的 invoke 只会 reject）。
  if (!shell) return;

  const win = getCurrentWindow();
  minBtn.addEventListener("click", () => void win.minimize());
  // `toggleMaximize` 与拖拽区的双击走同一条命令（`internal_toggle_maximize`）。
  maxBtn.addEventListener("click", () => void win.toggleMaximize());
  // `close()` 会落进壳层的 `CloseRequested` → 收回托盘，跟点系统 × 是同一个结局，
  // 别换成 `hide()`（那会绕过壳层那处统一处理）。
  closeBtn.addEventListener("click", () => void win.close());
  const syncMax = async () => {
    let now: boolean;
    try {
      now = await win.isMaximized();
    } catch {
      return;
    }
    if (now === maximized) return;
    maximized = now;
    paintMax();
  };
  void win.onResized(() => void syncMax());
  void syncMax();

  if (!linux) return;
  const zones: [string, ResizeDir][] = [
    ["n", "North"],
    ["s", "South"],
    ["e", "East"],
    ["w", "West"],
    ["ne", "NorthEast"],
    ["nw", "NorthWest"],
    ["se", "SouthEast"],
    ["sw", "SouthWest"],
  ];
  for (const [cls, dir] of zones) {
    const z = document.createElement("div");
    z.className = `resize-zone ${cls}`;
    z.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      void win.startResizeDragging(dir).catch(() => {});
    });
    document.body.append(z);
  }
}

// 右键只留给界面自己设计过的菜单（列表行 / 目录行，它们在各自 handler 里先
// `preventDefault()` 再调 `showMenu`）。冒泡到 document 时还没被拦下的，就是没设计过的位置
// （输入框、空白、按钮、浮窗背景…）——一律拦掉 WebView 自带的「刷新 / 另存为 / 打印 / 检查」。
// 五个窗口共用这段守卫：浮窗同样不该弹系统菜单。
document.addEventListener("contextmenu", (e) => {
  if (!e.defaultPrevented) e.preventDefault();
});

// 先于一切渲染：主题切在首帧之前，避免深色闪一下再变白。五个窗口共用这段。
applyTheme(loadTheme());

// 别的窗口切了主题（设置页那个下拉）：主题只记本机 localStorage，同一份存储被另一个
// 窗口改写时浏览器会给本窗口派 `storage` 事件——就地重刷 `data-theme`，常显浮窗（提示框 /
// HUD / 托盘菜单）才不用等重启。改的是别的 key 就忽略。
window.addEventListener("storage", (e) => {
  if (e.key === null || e.key === THEME_KEY) applyTheme(loadTheme());
});

if (location.hash === "#toast") {
  void bootstrapToast();
} else if (location.hash === "#gesture") {
  void bootstrapGesture();
} else if (location.hash === "#hud") {
  // 演示模式只在没有 Tauri 壳时生效（`index.html#hud` 纯浏览器直开时给一份样例载荷，
  // 方便调样式；壳内自动跳过）。触发气泡那边不装，因为它没有可预览的静态内容。
  installBrowserMock();
  void bootstrapHud();
} else if (location.hash === "#hints") {
  // 同 `#hud`：提示框也能纯浏览器直开预览样式（演示 IPC 给一份样例配置与状态）。
  installBrowserMock();
  void bootstrapHints();
} else if (location.hash === "#clipboard-menu") {
  // 同 `#hud`：纯浏览器直开时给一份样例菜单，方便调样式。
  const preview = !("__TAURI_INTERNALS__" in window);
  installBrowserMock();
  void bootstrapClipboardMenu(preview);
} else if (location.hash === "#tray-menu") {
  // 同 `#clipboard-menu`：纯浏览器直开时给一份样例菜单，方便调样式。
  const preview = !("__TAURI_INTERNALS__" in window);
  installBrowserMock();
  void bootstrapTrayMenu(preview);
} else if (location.hash === "#prompt") {
  // 同 `#hud`：纯浏览器直开时给一份样例载荷，方便调样式。
  installBrowserMock();
  void bootstrapPrompt();
} else {
  mountTitlebar();
  installBrowserMock();
  bind();
  bindMessages();
  bindUpdateDialog();
  // 回到窗口时重新扫一遍外部工具：用户可能刚把比特口袋 / 自动化助手打开。
  // 只有「在跑的工具集合」真的变了才重画，避免每次切窗口都重置编辑区。
  window.addEventListener("focus", () => {
    const before = state.runningTools.map((t) => t.id).join(",");
    void refreshRunningTools().then(() => {
      if (state.runningTools.map((t) => t.id).join(",") !== before) render();
    });
  });
  void (async () => {
    await initEvents();
    await load();
  })();
}


