import { invoke } from "@tauri-apps/api/core";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { el, helpIcon, intIn, onClickAsync, toast } from "./dom";
import { state } from "./state";
import { tr, trt } from "./i18n";
import { refreshInputDevices } from "./api";
import { collapsedActions, mouseFeaturesEnabled, touchpadFeaturesEnabled } from "./list";
import {
  ACTION_ICONS,
  APP_OPS,
  BRIGHTNESS_OPS,
  CAPTURE_OPS,
  CLIPBOARD_OPS,
  CONDITION_TYPES,
  DESKTOP_DIRECTIONS,
  MOUSE_BUTTON_LABELS,
  MOUSE_OPS,
  OS_OPS,
  SYSTEM_OPS,
  TRANSFORM_OPS,
  WAIT_TARGET_TYPES,
  WHEN_CONDITION_TYPES,
  WINDOW_OPS,
  actionDescription,
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
import { refreshDirty, requestOpenDetail, runActions } from "./editor";
import { syncMouseOpts, syncTouchpadOpts } from "./settings";
import { isListSection, save, splitConflicts, switchSection } from "./main";
import { startCapture } from "./input";
import type {
  Action,
  ActionBase,
  AppOperation,
  BrightnessOperation,
  CaptureOperation,
  ClipboardOperation,
  Condition,
  Conflict,
  DesktopDirection,
  IfRunning,
  InputDeviceList,
  KeysMode,
  ListKind,
  MouseButton,
  MouseOp,
  OsOperation,
  SendPayload,
  Shell,
  ShowOutput,
  SystemOperation,
  TextMode,
  TransformOperation,
  WaitTarget,
  WindowOperation,
} from "./types";

// 动作列表（流程管线视图）/ 内联校验 / 命令与脚本共用字段 / 冲突提示（U-03 从 main.ts 搬出）。
// 这里只管「一条动作怎么画、怎么改类型、拖拽怎么落位、字段怎么校验」；编辑页外壳与保存仍在
// ./editor，整屏重画顺序仍由入口 render() 决定。
// 内置脚本的示例：读变量 → 拼接 → 存回变量。保持英文（脚本内容不是界面文案，
// 走翻译反而会让人复制到一段被翻过的代码）。
const INLINE_SCRIPT_SAMPLE = `// read a chain variable, build a string, write it back (the return value is the step result)
let who = get_var("name");
if who == "" {
    who = "Kada";
}
set_var("greeting", "Hello, " + who);
"Hello, " + who`;

// ---- 动作列表渲染（流程管线视图） ----
// `start` 是流程管线的起止说明（快捷键 = 触发键；鼠标操作 = 方向串）。
export function renderActions(s: { actions: Action[]; triggers?: string[] }, start?: string) {
  const wrap = document.getElementById("action-list")!;
  renderActionList(wrap, s.actions, () => renderActions(s, start), {
    start: start ?? (s.triggers?.length ? s.triggers.join(" / ") : tr("未设触发键")),
  });
  refreshDirty();
}

// 递归把某个动作列表的所有动作标记为收起（打开已有多动作的快捷键时用）。
export function collapseAllActions(actions: Action[]) {
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

  // 变量流向：平铺标签（⇒ var / {var}）太吵，收成一个小图标——有写出显绿、只有引用显灰，
  // 悬停看具体写了 / 读了哪些变量。
  const writes = actionVarWrites(act);
  const reads = actionVarReads(act);
  const tags = el("span", "action-vartags");
  if (writes.length || reads.length) {
    const badge = el("span", "var-badge" + (writes.length ? " write" : ""));
    const lines: string[] = [];
    if (writes.length) lines.push(trt`写入变量：${writes.join("、")}`);
    if (reads.length) lines.push(trt`引用变量：${reads.join("、")}`);
    badge.title = lines.join("\n");
    badge.setAttribute("aria-label", lines.join("；"));
    badge.innerHTML =
      '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 4H6a2 2 0 0 0-2 2v3a2 2 0 0 1-2 2 2 2 0 0 1 2 2v3a2 2 0 0 0 2 2h2"/><path d="M16 4h2a2 2 0 0 1 2 2v3a2 2 0 0 0 2 2 2 2 0 0 0-2 2v3a2 2 0 0 1-2 2h-2"/></svg>';
    tags.append(badge);
  }

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
export function convertedAction(from: ActionBase, type: ActionBase["type"]): ActionBase {
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
export function actionDropAt(x: number, y: number): ActionDropHit | null {
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

  const first = rows[0]?.getBoundingClientRect();
  if (!first) return null;

  let index = rows.length;
  for (let j = 0; j < rows.length; j++) {
    const row = rows[j];
    if (!row) continue;
    const r = row.getBoundingClientRect();
    if (y < r.top + r.height / 2) {
      index = j;
      break;
    }
  }

  // 线落在相邻两张卡片的空隙中央；首尾各向外让 6px（贴着卡片但不压住内容）。
  const prev = index > 0 ? rows[index - 1]?.getBoundingClientRect() ?? null : null;
  const next = index < rows.length ? rows[index]?.getBoundingClientRect() ?? null : null;
  const yLine = prev && next ? (prev.bottom + next.top) / 2 : next ? next.top - 6 : prev ? prev.bottom + 6 : first.top;
  return {
    target: { kind: "line", list: box.list, index },
    line: { y: yLine, left: first.left, width: first.width },
    forbidden,
  };
}

let actionDropLine: HTMLElement | null = null;
let actionDropIntoEl: HTMLElement | null = null;

export function clearActionDropIndicators() {
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

export function updateActionDropTarget(x: number, y: number) {
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

export function applyActionDrop(srcList: Action[], srcIndex: number, target: ActionDropTarget) {
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
export function actionSubtypeSelect(
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
export function renderWhenEditor(
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

export function actionFields(
  a: ActionBase,
  rerender: () => void,
  ctx?: { actions: Action[]; idx: number },
): HTMLElement {
  const body = el("div", "action-body");
  if (a.type === "ask") {
    // 设置变量（文本操作的第四种方式）：沿用文本动作的通用行样式，不另立一套表单卡。
    const popupOn = a.popup !== false;
    const line = (
      label: string,
      value: string,
      placeholder: string,
      set: (v: string) => void,
      disabled = false,
    ) => {
      const row = el("div", "action-line");
      const input = el("input", "action-input") as HTMLInputElement;
      input.type = "text";
      input.value = value;
      input.placeholder = placeholder;
      input.disabled = disabled;
      input.addEventListener("input", () => set(input.value));
      row.append(el("span", "unit-hint", label), input);
      return row;
    };
    // 关掉弹框 = 这条动作不再问人，默认值就是写进变量的内容。
    const popup = el("label", "action-check") as HTMLLabelElement;
    const pb = el("input", "toggle") as HTMLInputElement;
    pb.type = "checkbox";
    pb.checked = popupOn;
    pb.addEventListener("change", () => {
      a.popup = pb.checked;
      rerender();
    });
    popup.title = tr("关掉后不弹框，直接把「默认值」存进变量");
    popup.append(pb, el("span", undefined, tr("弹出输入框")));
    body.append(
      line(tr("变量名"), a.var, tr("变量名（后面用 {变量名} 引用）"), (v) => (a.var = v.trim())),
      line(
        tr("提示语"),
        a.prompt,
        tr("弹框里问用户的话（支持 {变量名}）"),
        (v) => (a.prompt = v),
        !popupOn,
      ),
      line(
        tr("默认值"),
        a.default ?? "",
        popupOn
          ? tr("回车前先填好的内容（支持 {变量名}）")
          : tr("直接写进变量的内容（支持 {变量名}）"),
        (v) => (a.default = v),
      ),
      popup,
      el(
        "div",
        "unit-hint",
        popupOn
          ? tr("触发时弹输入框（跟随主题），填的内容存进变量；点取消或留空则本次执行中止。")
          : tr("不弹框：触发时直接把「默认值」（代入变量后）存进变量，空值也照写。"),
      ),
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
export function showFieldError(input: HTMLElement, msg: string) {
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
export function expansionTriggerError(v: string): string {
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
export function renderConflictBubble() {
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
  for (const q of [...c.message.matchAll(/「([^」]+)」/g)]
    .map((m) => m[1])
    .filter((q): q is string => q !== undefined)) {
    if (!remapMsg) {
      const si = state.cfg.shortcuts.findIndex((s) => s.triggers.includes(q));
      if (si >= 0) return { kind: "shortcut", index: si };
    }
    const ri = state.cfg.remaps.findIndex((r) => r.from === (q.split(" →")[0] ?? "").trim());
    if (ri >= 0) return { kind: "remap", index: ri };
  }
  return null;
}

function entryLabel(ref: { kind: ListKind; index: number }): string {
  const shortcut = ref.kind === "shortcut" ? state.cfg.shortcuts[ref.index] : undefined;
  if (shortcut) return shortcut.name?.trim() || tr("未命名快捷键");
  const remap = ref.kind === "remap" ? state.cfg.remaps[ref.index] : undefined;
  if (remap) return remap.name?.trim() || tr("未命名改键");
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
export function openConflictView() {
  state.msgView = "conflicts";
  void switchSection("messages");
}
