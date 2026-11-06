import { invoke } from "@tauri-apps/api/core";
import { tr, trt } from "./i18n";

// 组合键 / 序列 / 和弦的录入捕获（U-04 从 main.ts 搬出）：捕获期间暂停快捷键，
// 按键 / 鼠标侧键 / 滚轮都按键名提交；窗口失焦 / 隐藏 / 关详情时统一解除，别让
// Rust 侧停在暂停态。编辑页的触摸板录入与本模块共用「同一时刻只有一个录入会话」的锁。
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
