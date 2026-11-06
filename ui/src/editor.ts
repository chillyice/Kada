import { invoke } from "@tauri-apps/api/core";
import { deepClone, domValue, el, intIn, toast } from "./dom";
import { draftSignature, isDraftDirty, state } from "./state";
import { tr, trt } from "./i18n";
import { refreshConflicts } from "./api";
import {
  HOT_CORNER_SPOTS,
  WHEEL_EDGE_SPOTS,
  deleteDetailEntry,
  discardNewDraft,
  kindLabel,
  mouseFeaturesEnabled,
  mouseOrderFront,
  newFolderId,
  renderListPane,
  shortcutLabel,
  spotLabel,
  touchpadFeaturesEnabled,
  touchpadOrderFront,
} from "./list";
import { KEY_OPTIONS, newAction } from "./options";
import { syncSettings } from "./settings";
import {
  KIND_SECTION,
  askConfirm,
  collapseAllActions,
  currentDraft,
  currentKind,
  endCapture,
  expansionTriggerError,
  isListSection,
  render,
  renderActions,
  renderConflictBubble,
  renderWhenEditor,
  save,
  setCaptureCleanup,
  showFieldError,
  stopCapture,
  stopRecIfAny,
  syncRailTab,
  switchSectionNow,
} from "./main";
import type {
  Action,
  Config,
  Gesture,
  HotCornerRule,
  HotCornerSpot,
  Layer,
  ListKind,
  Remap,
  ShortcutItem,
  TextExpansion,
  TouchpadGesture,
  WheelEdgeRule,
  WheelEdgeSpot,
} from "./types";

// 编辑页 / 统一编辑页外壳 / 触摸板手势录入 / 改键与层下拉（U-03 从 main.ts 搬出）。
// 这里只负责「当前选中条目怎么填进表单、怎么从表单写回草稿、怎么保存」；动作列表的渲染在
// 入口的动作列表段（等 form 批次搬走），左栏与入口的 render() 仍从 ./main 调用本模块的 renderDetail()。

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
export function currentActionsHost(): { actions: Action[] } | null {
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
export function openGesturePad(onDone: (pattern: string) => void) {
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
export type TrackpadRecordPayload = {
  pattern: string;
  template: [number, number][] | null;
  points: number;
  elapsed_ms: number;
};

/** 正在「录入」时接收壳层回传的回调（`null` = 没在录）。 */
export let trackpadRecordSink: ((p: TrackpadRecordPayload) => void) | null = null;

/** 触发键下拉：只有键盘键（鼠标键只走右键手势那条路）。 */
export function fillTouchpadTriggerSelect() {
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
export function syncTouchpadUi() {
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
export function openTrackpadRecord(key: string) {
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
    endCapture(stop);
    trackpadRecordSink = null;
    window.removeEventListener("keydown", onKey, true);
    mask.remove();
    void invoke("stop_trackpad_record").catch(() => {});
  }
  // 窗口失焦 / 收进托盘 = 这次录入作废：不清会话的话触发键会一直被钩子吞掉，
  // 而面板已经看不见了（与录入暂停同一口径，见 `stopCapture`）。
  setCaptureCleanup(stop);
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

export function applyEntryTab() {
  document.querySelectorAll<HTMLElement>("#entry-tabs .entry-tab").forEach((b) =>
    b.classList.toggle("active", b.dataset.tab === state.entryTab),
  );
  document.querySelectorAll<HTMLElement>("#detail-editor .entry-pane").forEach((p) =>
    p.classList.toggle("hidden", p.dataset.pane !== state.entryTab),
  );
}

// 新增态切换模式：换分区重开一份新草稿，把已填的名称 / 描述带过去，其余字段按新类型初始化。
// 已保存的条目切不动（syncEntry 里按钮是 disabled 的，这里再兜一道）。
export function switchDraftKind(kind: ListKind) {
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
export async function runCurrentEntry() {
  if (!state.draft) return;
  await runActions(state.draft.actions, state.draft.name?.trim() || shortcutLabel(state.draft));
}

// 行内「执行」按钮与编辑页的「执行」共用这一支：直接把动作串丢给壳层跑一遍。
export async function runActions(actions: Action[], name: string) {
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
export function deleteCurrentEntry() {
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
export function renderKeyMenu(filter: string) {
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

export function pickRemapKey(k: string) {
  const input = document.getElementById("remap-from") as HTMLInputElement;
  input.value = k;
  showFieldError(input, "");
  if (state.remapDraft) state.remapDraft.from = k;
  closeKeyMenu();
  refreshDirty();
}

export function closeKeyMenu() {
  document.getElementById("key-menu")?.classList.add("hidden");
}

export function closeModeMenu() {
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

export function discardDraft() {
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
export function refreshDirty() {
  const dirty = isDraftDirty();
  document.getElementById("dirty-flag")?.classList.toggle("hidden", !dirty);
  const undo = document.getElementById("entry-undo") as HTMLButtonElement | null;
  if (undo) undo.disabled = !dirty;
  // 保存同理：没有改动就没有东西可落盘，按钮置灰（Ctrl+S 那条路径也按这个判定放行）。
  const save = document.getElementById("entry-save") as HTMLButtonElement | null;
  if (save) save.disabled = !dirty;
}

// 撤销改动：把草稿还原成基线内容（打开时 / 上次保存的），编辑页留在原地。
export function revertDraft() {
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
export let unsavedPending: ((c: UnsavedChoice) => void) | null = null;

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

export function resolveUnsaved(choice: UnsavedChoice) {
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

export async function saveCurrentDraft(): Promise<boolean> {
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
export async function requestOpenDetail(i: number, isNew = false, kind: ListKind = "shortcut") {
  if (!(await confirmLeaveDraft())) return;
  const tab = KIND_SECTION[kind];
  if (state.section !== tab) switchSectionNow(tab, { keepList: true });
  openDetail(i, isNew);
}

// 「＋ 新增」下拉 / 目录行的「＋」：开新草稿（可指定快捷键要落进的目录）。
export async function requestNew(kind: ListKind, folderId: string | null = null) {
  if (!(await confirmLeaveDraft())) return;
  const tab = KIND_SECTION[kind];
  if (state.section !== tab) switchSectionNow(tab, { keepList: true });
  openDetail(-1, true, folderId);
}

// 取消 / ×  关闭编辑页：同样是「离开」，走同一道守卫。
export async function requestCloseDetail() {
  if (!(await confirmLeaveDraft())) return;
  closeDetail();
}

// 外部修改（后端监听 config.json 后广播的新配置，见规划 7.2-⑩）：把界面换成文件里的那一份。
//
// 触达编辑页时也算一次「离开当前编辑上下文」，所以与 leave 守卫同一套口径：**有未保存改动**
// 就问一句，两个按钮各自写明后果（一个丢弃改动、一个会在下次保存时覆盖文件里的外部改动）；
// 只是打开了编辑页却没改（干净草稿）就不问——关掉它什么都不会丢，留着反而危险：外部内容里
// 的同一下标可能已经是另一条，草稿一保存就写到别人身上去了。
export async function applyExternalConfig(next: Config) {
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

export function renderTriggers(s: ShortcutItem) {
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

// ---- 改键下拉 ----
export function fillKeySelect(sel: HTMLSelectElement, value: string, withEmpty = false) {
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
export function syncRemapEditor() {
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

export function layerName(id: string | null | undefined): string {
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
export function appendLayerMark(trig: HTMLElement, layerId: string | null | undefined) {
  const mark = layerById(layerId)?.mark;
  if (!mark) return;
  const color = isLayerColor(mark);
  const s = el("span", `row-layer-mark ${color ? "mark-color" : "mark-emoji"}`);
  if (color) s.style.setProperty("--mark", mark);
  else s.textContent = mark;
  trig.append(s);
}

// 改键形态的人类可读摘要（列表行 + 详情标题共用），与 kada-core 的 Remap::describe 对齐。
export function remapSummary(r: Remap): string {
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

export function renderLayers() {
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

export function addLayer() {
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
