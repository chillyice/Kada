import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { installBrowserMock } from "./mock";
import { localizeStaticDom, tr, trt } from "./i18n";
import {
  fetchCommandResults,
  fetchConfig,
  fetchRunRecords,
  fetchUnread,
  fetchUpdateStatus,
  normalizeCommandScriptFields,
  refreshConflicts,
  refreshRunningTools,
  saveConfig,
} from "./api";
import { el, toast } from "./dom";
import { state } from "./state";
import { bindMessages, markRead, renderMessages, showResultModal } from "./messages";
import { bind } from "./events";
import { renderConflictBubble } from "./form";
import { hideRowMenu, renderListPane } from "./list";
import {
  applyExternalConfig,
  confirmLeaveDraft,
  discardDraft,
  renderDetail,
  renderLayers,
  trackpadRecordSink,
} from "./editor";
import type { TrackpadRecordPayload } from "./editor";
import {
  loadMarket,
  renderMarketPage,
} from "./market";
import {
  bindUpdateDialog,
  openUpdateDialog,
  setUpdateStatus,
  syncSettings,
  syncUpdate,
} from "./settings";
import type { UpdateDialogPayload } from "./settings";
import { actionHasContent } from "./options";
import type {
  Action,
  CommandResult,
  Config,
  Conflict,
  Gesture,
  HintsState,
  HotCornerRule,
  ListKind,
  Remap,
  RunRecord,
  Section,
  ShortcutItem,
  StatusPayload,
  TextExpansion,
  TouchpadGesture,
  UpdateStatus,
  WheelEdgeRule,
} from "./types";

// 静态界面文案先翻（index.html 里的整块文本 / 属性），之后动态渲染的部分都走 tr()/trt()。
localizeStaticDom();

/** 冲突三组：硬冲突（需处理）/ 软冲突（遮蔽·重叠）/ 平台能力·环境限制（仅提示，不阻止保存）。 */
export function splitConflicts(list: Conflict[]): { errors: Conflict[]; warns: Conflict[]; caps: Conflict[] } {
  return {
    errors: list.filter((c) => c.severity === "error"),
    warns: list.filter((c) => c.severity === "warn" && !c.platform),
    caps: list.filter((c) => c.platform),
  };
}
export const KIND_SECTION: Record<ListKind, Section> = {
  shortcut: "shortcuts",
  remap: "remaps",
  expansion: "expansions",
  gesture: "gestures",
  touchpad: "touchpads",
  hotcorner: "hotcorners",
  wheeledge: "wheeledges",
};
export function isListSection(s: Section): boolean {
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
export function currentKind(): ListKind {
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
export function currentDraft():
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


// 可变状态集中在 ./state（模块顶层不互相 import 对方的 let，避免 TDZ / 循环依赖）。

// ---- 列表折叠 / 列表渲染 / 行菜单 / 目录与多选 / 拖拽（U-03 已搬到 ./list）----
// 入口只保留 askConfirm() / resolveConfirm()（通用确认框，消息中心与编辑页都要用），
// 以及 render() 里对 renderListPane() 的一行调用。
// ---- IPC ----
export async function load() {
  state.cfg = await fetchConfig();
  await refreshRunningTools();
  state.messages = await fetchCommandResults();
  state.runs = await fetchRunRecords();
  state.unread = await fetchUnread();
  setUpdateStatus(await fetchUpdateStatus());
  await refreshConflicts();
  render();
  syncSettings();
  syncUpdate();
}

// 落盘整份配置。返回 true = 真的写进去了——未保存守卫靠它决定「保存并离开」要不要放行，
// 失败还放行等于把用户的改动悄悄丢掉。
export async function save(): Promise<boolean> {
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
export function numpadKeyFromEvent(e: KeyboardEvent): string | null {
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

/** 登记进行中的录入捕获解除函数（录入会话由编辑页 / 本文件的录入按钮共用）。 */
export function setCaptureCleanup(fn: (() => void) | null) {
  activeCaptureCleanup = fn;
}

/** 当前捕获若是 `fn` 就解除登记；返回是否真的解除（避免后开的会话被先关的顶掉）。 */
export function endCapture(fn: () => void): boolean {
  if (activeCaptureCleanup !== fn) return false;
  activeCaptureCleanup = null;
  return true;
}

/** 中断当前录入捕获并解除暂停；无捕获时是空操作。 */
export function stopCapture() {
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

export function startCapture(onCommit: (combo: string) => void, btn: HTMLButtonElement) {
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
    setCaptureCleanup(null);
    window.removeEventListener("keydown", handler, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("wheel", onWheel, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  setCaptureCleanup(finish);
  window.addEventListener("keydown", handler, true);
  window.addEventListener("mousedown", onMouse, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: false });
}

// 键序列录入：逐键累积 comboFromEvent 出的组合（空格拼接实时预览），Enter 提交、Esc 取消。
// 序列至少 2 步（单步就是普通组合键，走「录入组合键」）。
export function startSequenceCapture(onCommit: (seq: string) => void, btn: HTMLButtonElement) {
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
    setCaptureCleanup(null);
    window.removeEventListener("keydown", handler, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("wheel", onWheel, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  setCaptureCleanup(finish);
  window.addEventListener("keydown", handler, true);
  window.addEventListener("mousedown", onMouse, true);
  window.addEventListener("wheel", onWheel, { capture: true, passive: false });
  update();
}

// 和弦录入：按住多个键，松开时提交（成员用 & 连接、按字典序，至少 2 键）。Esc 取消。
// 成员是普通键（修饰键 / CapsLock 不当成员）；按住修饰键录入时把它写在**第一个**成员上
// （`Ctrl+F&J` = 按住 Ctrl 的同时把 F、J 一起按住）——后端对成员修饰要求的判定只看「凑齐
// 那一刻按着没有」，写在第一个成员上等价于要求全程按着，显示也最省事。
export function startChordCapture(onCommit: (chord: string) => void, btn: HTMLButtonElement) {
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
    setCaptureCleanup(null);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("keyup", onKey, true);
    window.removeEventListener("mousedown", onMouse, true);
    window.removeEventListener("mouseup", onMouse, true);
    void invoke("set_paused", { paused: false });
    btn.disabled = false;
    captureHint(null);
  };
  setCaptureCleanup(finish);
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
export const THEME_KEY = "kada.theme";
const DEFAULT_THEME = "crt";
const THEMES = new Set(["crt", "crt-light", "dark", "light"]);
// 老版本存过 "auto"（跟随系统）这一档，已废弃——读到不认识的旧值一律落回默认主题。
export function loadTheme(): string {
  const stored = localStorage.getItem(THEME_KEY);
  return stored && THEMES.has(stored) ? stored : DEFAULT_THEME;
}
export function applyTheme(t: string) {
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

export function render() {
  renderListPane();
  renderConflictBubble();
  renderDetail();
  renderMessages();
  if (state.section === "layers") renderLayers();
  if (state.section === "market") renderMarketPage();
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

export function resolveConfirm(ok: boolean) {
  const pending = confirmPending;
  confirmPending = null;
  document.getElementById("confirm-modal")!.classList.add("hidden");
  pending?.(ok);
}

/** 确认框是否正开着（Esc 只在开着时拦截；状态仍归入口，绑定在 ./events）。 */
export function isConfirmPending(): boolean {
  return confirmPending !== null;
}

// ---- 编辑页 / 触摸板录入 / 改键下拉（U-03 已搬到 ./editor）----
// 入口只保留 render() 里对 renderDetail() 的一行调用与全局事件绑定。
// ---- 动作列表渲染 / 内联校验 / 命令脚本字段 / 冲突提示（U-03 已搬到 ./form）----
// 入口只保留 render() 里的 renderConflictBubble()；所有字段 / 拖拽绑定都在 ./events。


// ---- 设置 / 软件更新 / 更新对话框（U-03 已搬到 ./settings）----
// 入口只保留 syncSettings() / syncUpdate() / bindSettings() / bindUpdateDialog() 的调用点，
// 以及 update-status / update-dialog 的事件订阅；卡片填充、提权 / 输入层诊断 / 罗技解锁、
// 配置导入 / 导出与同步存档、更新对话框都跟着设置页进了 settings.ts。

// ---- 鼠标手势（规划 7.2-MG）----
// 手势的条目与编辑在「快捷键」页的「鼠标操作」分类里（与快捷键/改键/文本扩展并列），
// 设置页只留一个「鼠标操作」总开关：关掉 = 鼠标类条目不显示、行为不生效，配置保留。

// ---- 复制快捷菜单（EXT-13，已删除；只读编辑器已搬进 ./settings）----
// 旧配置里残留的 clipboard_menu 仍要在设置页只读展示，函数跟着设置页走。


// ---- 事件绑定（U-03 已搬到 ./events）----
// 入口只保留 bind() 的调用点；所有 addEventListener 注册（含键盘快捷键、拖拽、录入按钮、
// 动作字段、页签 / 模式菜单）都在 ./events，方便一处核对「哪些控件绑了没」。


// 切页会丢弃草稿（草稿只属于当前编辑页），所以先过一道未保存守卫。
export async function switchSection(tab: Section) {
  if (!(await confirmLeaveDraft())) return;
  switchSectionNow(tab);
}

// 品牌字标：固定节奏的自动演示——亮一会儿，四字一起按下，再回弹并迸出星屑。
// 纯装饰，不响应鼠标；系统开了「减少动态效果」就整段不跑，只留静止的发光字。
export function startBrandLogoLoop() {
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
export function syncRailTab(tab: Section) {
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

export async function stopRecIfAny() {
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
    setUpdateStatus(event.payload);
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


