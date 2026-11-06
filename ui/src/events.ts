import { el, toast } from "./dom";
import { isDraftDirty, state } from "./state";
import { tr, trt } from "./i18n";
import { refreshConflicts } from "./api";
import { hideResultModal } from "./messages";
import { bindSettings } from "./settings";
import { newAction } from "./options";
import {
  KIND_ICON,
  addFolder,
  hideRowMenu,
  pasteIntoFolder,
  showMenu,
  closeMenuUnlessIn,
  onPointerMove,
  onPointerUp,
  renderListPane,
} from "./list";
import type { MenuItem } from "./list";
import {
  applyEntryTab,
  closeKeyMenu,
  closeModeMenu,
  currentActionsHost,
  deleteCurrentEntry,
  addLayer,
  fillKeySelect,
  fillTouchpadTriggerSelect,
  openGesturePad,
  openTrackpadRecord,
  pickRemapKey,
  refreshDirty,
  renderKeyMenu,
  renderTriggers,
  requestCloseDetail,
  requestNew,
  resolveUnsaved,
  revertDraft,
  runCurrentEntry,
  saveCurrentDraft,
  switchDraftKind,
  syncRemapEditor,
  syncTouchpadUi,
  unsavedPending,
} from "./editor";
import { expansionTriggerError, renderActions, renderConflictBubble, showFieldError } from "./form";
import {
  currentDraft,
  isConfirmPending,
  isListSection,
  numpadKeyFromEvent,
  resolveConfirm,
  startBrandLogoLoop,
  startCapture,
  startChordCapture,
  startSequenceCapture,
  switchSection,
} from "./main";
import type { EntryTab, ListKind, Section } from "./types";

// 事件绑定（U-03 从 main.ts 搬出）：所有 addEventListener 注册集中在这里——
// 左栏切页、列表搜索 / 筛选 / 行菜单 / 拖拽、编辑页录入与页签、动作字段、
// 全局键盘快捷键、确认框、鸣谢墙。入口只调用 bind()。
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

export function bind() {
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
    if (e.key === "Escape" && isConfirmPending()) {
      e.preventDefault();
      resolveConfirm(false);
    }
  });

  // 导入方式 / 导入结果的弹窗绑定跟着设置页搬进 ./settings 的 bindSettings()。
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
