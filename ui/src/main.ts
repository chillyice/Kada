import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
import { toast } from "./dom";
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
import {
  bootstrapClipboardMenu,
  bootstrapGesture,
  bootstrapHints,
  bootstrapHud,
  bootstrapPrompt,
  bootstrapToast,
  bootstrapTrayMenu,
  mountTitlebar,
} from "./windows";
import { actionHasContent } from "./options";
import type {
  Action,
  CommandResult,
  Config,
  Conflict,
  Gesture,
  HotCornerRule,
  ListKind,
  Remap,
  RunRecord,
  Section,
  ShortcutItem,
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

// ---- 组合键录入（U-04 已搬到 ./input）----
// 组合键 / 序列 / 和弦录入与「同一时刻只有一个录入会话」的锁都在 input.ts；
// 录入按钮的事件绑定在 events.ts，编辑页的触摸板录入走同一组 start/stop 接口。


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

// ---- 独立窗口 / 自绘标题栏（U-04 已搬到 ./windows）----
// 触发气泡 / 输入状态 HUD / 提示框 / 手势轨迹 / 设置变量输入框 / 复制菜单 / 托盘菜单 / 自绘标题栏
// 都在 windows.ts；入口的启动分发按 location.hash 调对应 bootstrap。

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


