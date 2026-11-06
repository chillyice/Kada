import { deepClone, el, toast } from "./dom";
import { tr, trt } from "./i18n";
import { isDraftDirty, state } from "./state";
import {
  actionDropAt,
  applyActionDrop,
  askConfirm,
  clearActionDropIndicators,
  KIND_SECTION,
  render,
  renderActions,
  save,
  updateActionDropTarget,
} from "./main";
import {
  appendLayerMark,
  discardDraft,
  layerName,
  remapSummary,
  requestNew,
  requestOpenDetail,
  runActions,
} from "./editor";
import { exportFragment, openPublish, picksInFolder, picksOfSelection } from "./market";
import { actionSummary, unwrapAction } from "./options";
import type {
  Action,
  DragPayload,
  DropZone,
  AnyEntry,
  Folder,
  FolderEntry,
  Gesture,
  HotCornerRule,
  HotCornerSpot,
  ListKind,
  Remap,
  ShortcutItem,
  TextExpansion,
  TouchpadGesture,
  WheelEdgeRule,
  WheelEdgeSpot,
} from "./types";

// 左侧列表 / 目录树 / 行菜单 / 多选 / 拖拽（U-03 从 main.ts 搬出）。
// 入口只留 render() 总入口（调 renderListPane()）与列表之外的屏；行点击进编辑页仍走 ./main。

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

export function renderListPane() {
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
export const KIND_ICON: Record<ListKind, string> = {
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
export function mouseFeaturesEnabled(): boolean {
  return state.cfg.settings.experimental === true;
}

// 触摸板手势是否已打开（它**有自己的开关**，与鼠标操作互不影响；默认关）。
// 关着 = 列表 / 模式菜单里不出现，也不采集（触发键照常输出）。
export function touchpadFeaturesEnabled(): boolean {
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

export const HOT_CORNER_SPOTS: { spot: HotCornerSpot; label: string }[] = [
  { spot: "top_left", label: tr("左上角") },
  { spot: "top_right", label: tr("右上角") },
  { spot: "bottom_left", label: tr("左下角") },
  { spot: "bottom_right", label: tr("右下角") },
];
export const WHEEL_EDGE_SPOTS: { edge: WheelEdgeSpot; label: string }[] = [
  { edge: "top", label: tr("上边缘") },
  { edge: "bottom", label: tr("下边缘") },
  { edge: "left", label: tr("左边缘") },
  { edge: "right", label: tr("右边缘") },
];

export function spotLabel(spot: HotCornerSpot | WheelEdgeSpot): string {
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

// ---- 配置卡带 / 卡带市场 / 卡带发布（U-03 已搬到 ./market）----
// 入口只保留 renderMarketPage() / loadMarket() / bindMarket() 三个调用点。

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

export type MenuItem = { label: string; run: () => void; on?: boolean };

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
export function showMenu(anchor: HTMLElement, items: MenuItem[], at?: { x: number; y: number }) {
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

export function hideRowMenu() {
  document.getElementById("row-menu")!.classList.add("hidden");
  state.menuAnchor = null;
}

// 点到菜单/它的锚点外面 = 关菜单（锚点自己的 click 先跑，这里要能分辨）。
export function closeMenuUnlessIn(t: HTMLElement) {
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

export function newFolderId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `f-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function addFolder(parentId: string | null) {
  const f: Folder = { id: newFolderId(), name: "", parent: parentId };
  state.cfg.folders.push(f);
  state.editingFolderId = f.id;
  renderListPane();
}

function renameFolder(id: string) {
  state.editingFolderId = id;
  renderListPane();
}

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
export function shortcutLabel(s: ShortcutItem): string {
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
export function kindOf(o: object): ListKind | null {
  if ((state.cfg.shortcuts as object[]).includes(o)) return "shortcut";
  if ((state.cfg.remaps as object[]).includes(o)) return "remap";
  if ((state.cfg.expansions as object[]).includes(o)) return "expansion";
  if ((state.cfg.gestures as object[]).includes(o)) return "gesture";
  if ((state.cfg.touchpad_gestures as object[]).includes(o)) return "touchpad";
  if ((state.cfg.hot_corners as object[]).includes(o)) return "hotcorner";
  if ((state.cfg.wheel_edges as object[]).includes(o)) return "wheeledge";
  return null;
}

export function arrOf(kind: ListKind): AnyEntry[] {
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

export function kindLabel(kind: ListKind | null): string {
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
export function labelOf(o: object): string {
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

export function pasteIntoFolder(folderId: string | null) {
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
export async function deleteDetailEntry(kind: string, label: string, remove: () => void) {
  if (!(await confirmDelete(kind, label, isDraftDirty()))) return;
  remove();
  discardDraft();
  state.draftNew = false;
  state.selected = null;
  void save();
}

// 新增项尚未落盘，删除 = 直接丢弃草稿：没有已保存的内容会丢，不必再弹一次确认。
export function discardNewDraft() {
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
export function touchpadOrderFront(item: TouchpadGesture) {
  const seq = (state.cfg.touchpad_gestures ?? []).filter((g) => g !== item);
  seq.unshift(item);
  seq.forEach((g, i) => {
    g.order = i;
  });
}

/** 新建条目排到最前：重排三类共用的 `order`（新条目拿 0，其余顺延）。 */
export function mouseOrderFront(item: Gesture | HotCornerRule | WheelEdgeRule) {
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

export function onPointerMove(e: PointerEvent) {
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

export function onPointerUp(e: PointerEvent) {
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
