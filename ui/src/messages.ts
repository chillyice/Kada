import { invoke } from "@tauri-apps/api/core";
import { el, onClickAsync, toast } from "./dom";
import { tr, trt } from "./i18n";
import { state } from "./state";
import { actionSummary, unwrapAction } from "./options";
import { msgEntries, renderRunCard, runState } from "./runs";
import { collapsedActions } from "./list";
import { askConfirm, renderConflictList, switchSectionNow } from "./main";
import { confirmLeaveDraft, openDetail, renderDetail } from "./editor";
import type { Action, ActionStep, CommandResult, RunRecord, Section } from "./types";

// 消息中心 + 结果卡深链（U-03 从 main.ts 搬出）：执行记录列表 / 视图切换 / 结果卡 /
// 「定位到动作」。单条记录怎么画在 runs.ts；这里负责页面编排与深链。
// ---- 消息中心 ----
function renderBadge() {
  document.getElementById("msg-badge")!.classList.toggle("hidden", !state.unread);
}

export function markRead() {
  state.unread = false;
  renderBadge();
  void invoke("mark_results_read");
}

// inline = 标签与值同排一行（用于「开始 / 结束 / 状态」这类紧凑的元信息）。
export function msgField(k: string, v: string, code = false, inline = false): HTMLElement {
  const row = el("div", inline ? "msg-field inline" : "msg-field");
  row.append(el("span", "msg-k", k));
  const val = code ? el("pre", "msg-v") : el("span", "msg-v");
  val.textContent = v;
  row.append(val);
  return row;
}

// 结果卡 / 步骤详情共用的两个动作（CS-10）：「复制全文」走后端剪贴板（不依赖 WebView
// 权限），「打开完整日志」只交给壳层校验路径后在文件管理器里定位。
function copyResultButton(text: string): HTMLButtonElement {
  const btn = el("button", "ghost msg-action-btn", tr("复制全文")) as HTMLButtonElement;
  btn.type = "button";
  onClickAsync(btn, async () => {
    try {
      await invoke("copy_result_text", { text });
      toast(tr("已复制"));
    } catch (e) {
      toast(trt`复制失败: ${e}`);
    }
  });
  return btn;
}

function openLogButton(path: string): HTMLButtonElement {
  const btn = el("button", "ghost msg-action-btn", tr("打开完整日志")) as HTMLButtonElement;
  btn.type = "button";
  btn.title = path;
  onClickAsync(btn, async () => {
    try {
      await invoke("open_result_log", { path });
    } catch (e) {
      toast(trt`打开日志失败: ${e}`);
    }
  });
  return btn;
}

export function resultActions(r: { stdout: string; stderr: string; log_path?: string | null }): HTMLElement {
  const row = el("div", "msg-card-actions");
  const text = [r.stdout, r.stderr].filter((s) => s.length > 0).join("\n");
  if (text) row.append(copyResultButton(text));
  if (r.log_path) row.append(openLogButton(r.log_path));
  return row;
}

// stdout / stderr 输出块（CS-21）：默认折叠 + 行内筛选。
// 一条 npm install 的输出能有好几屏，展开态又有 200px 上限——先收起，需要时再展开 / 搜。
export function msgOutputField(k: string, text: string): HTMLElement {
  const row = el("div", "msg-field");
  row.append(el("span", "msg-k", k));
  const wrap = el("div", "msg-output");
  const tools = el("div", "msg-output-tools");
  const search = el("input", "msg-output-search") as HTMLInputElement;
  search.type = "search";
  search.placeholder = tr("在输出里筛选…");
  const toggle = el("button", "ghost msg-action-btn", tr("展开")) as HTMLButtonElement;
  toggle.type = "button";
  const pre = el("pre", "msg-v msg-output-pre");
  const lineCount = text.split("\n").length;
  const long = lineCount > 8 || text.length > 800;
  if (long) pre.classList.add("collapsed");
  else toggle.classList.add("hidden");
  const apply = (value: string) => {
    const q = value.trim();
    if (!q) {
      pre.textContent = text;
      toggle.classList.toggle("hidden", !long);
      return;
    }
    const hits = text
      .split("\n")
      .filter((line) => line.toLowerCase().includes(q.toLowerCase()));
    pre.textContent = hits.length ? hits.join("\n") : tr("（没有匹配的行）");
    toggle.classList.add("hidden");
    pre.classList.remove("collapsed");
  };
  search.addEventListener("input", () => apply(search.value));
  toggle.addEventListener("click", () => {
    const collapsed = pre.classList.toggle("collapsed");
    toggle.textContent = collapsed ? tr("展开") : tr("收起");
  });
  apply("");
  tools.append(search, toggle);
  wrap.append(tools, pre);
  row.append(wrap);
  return row;
}

// ---- 结果卡深链（CS-20）：从执行记录跳回动作编辑器 ----
// 先按记录里的「名称 / 触发键」找到动作所在条目（快捷键 / 手势 / 触摸板 / 角动作 / 边缘滚动），
// 再在条目里按动作标题或摘要定位到具体那一条。动作被删掉 / 改名时如实提示，不假装跳到了。
function findRunOwner(run: { name?: string | null; trigger: string }): { section: Section; index: number } | null {
  const name = (run.name ?? "").trim();
  const trigger = run.trigger.trim();
  const byName = (n?: string | null) => name.length > 0 && (n ?? "").trim() === name;
  const i = state.cfg.shortcuts.findIndex(
    (s) => byName(s.name) || (!name && s.triggers.some((t) => t.trim() === trigger)),
  );
  if (i >= 0) return { section: "shortcuts", index: i };
  const simple: [
    Section,
    { name?: string | null; pattern?: string | null; edge?: string; spot?: string }[],
  ][] = [
    ["gestures", state.cfg.gestures ?? []],
    ["touchpads", state.cfg.touchpad_gestures ?? []],
    ["hotcorners", state.cfg.hot_corners ?? []],
    ["wheeledges", state.cfg.wheel_edges ?? []],
  ];
  for (const [section, list] of simple) {
    const j = list.findIndex((it) => {
      if (byName(it.name)) return true;
      if (name) return false;
      const trig = (it.pattern ?? "").trim();
      return trig === trigger || it.edge === trigger || it.spot === trigger;
    });
    if (j >= 0) return { section, index: j };
  }
  return null;
}

function currentEditorActions(): Action[] | null {
  switch (state.section) {
    case "shortcuts":
      return state.draft?.actions ?? null;
    case "gestures":
      return state.gestureDraft?.actions ?? null;
    case "touchpads":
      return state.touchpadDraft?.actions ?? null;
    case "hotcorners":
      return state.hotcornerDraft?.actions ?? null;
    case "wheeledges":
      return state.wheeledgeDraft?.actions ?? null;
    default:
      return null;
  }
}

function expandAllActions(actions: Action[]) {
  for (const a of actions) {
    const act = unwrapAction(a);
    collapsedActions.delete(act);
    if (act.type === "if") {
      expandAllActions(act.then);
      expandAllActions(act.otherwise);
    } else if (act.type === "parallel") {
      expandAllActions(act.actions);
    } else if (act.type === "group") {
      expandAllActions(act.actions);
    }
  }
}

// 在已渲染的动作列表里找那一行：优先动作标题（后端摘要 = 「标题（正文）」），
// 其次摘要互相包含，最后同类型的第一个（正文被改过也至少落到大致位置）。
function locateActionRow(match: { title: string; kind?: string }): HTMLElement | null {
  const rows = [
    ...document.querySelectorAll<HTMLElement>("#action-list .action-row"),
  ];
  const info = rows.map((row) => {
    const list = (row as unknown as { _dropList?: Action[] })._dropList;
    const idx = (row as unknown as { _dropIndex?: number })._dropIndex;
    const act = idx === undefined ? undefined : list?.[idx];
    return { row, act: act ? unwrapAction(act) : null };
  });
  const title = match.title.trim();
  if (title) {
    for (const { row, act } of info) {
      if (!act) continue;
      const desc = "description" in act ? (act.description?.trim() ?? "") : "";
      const summary = actionSummary(act);
      if (desc && (title === desc || title.startsWith(`${desc}（`))) return row;
      if (summary && (title === summary || title.includes(summary) || summary.includes(title))) {
        return row;
      }
    }
  }
  if (match.kind) {
    return info.find(({ act }) => act?.type === match.kind)?.row ?? null;
  }
  return null;
}

// 打开条目 → 展开全部动作 → 滚到目标行并闪一下。
async function openActionAt(
  owner: { section: Section; index: number } | null,
  match: { title: string; kind?: string },
): Promise<boolean> {
  if (!owner) {
    toast(tr("没找到这条记录对应的动作（可能已删除或改名）"));
    return false;
  }
  if (!(await confirmLeaveDraft())) return false;
  switchSectionNow(owner.section);
  openDetail(owner.index);
  const actions = currentEditorActions();
  if (actions) {
    expandAllActions(actions);
    renderDetail();
  }
  const row = locateActionRow(match);
  if (!row) {
    toast(tr("已打开对应条目，但没找到这条动作（可能已被删除）"));
    return false;
  }
  row.scrollIntoView({ block: "center", behavior: "smooth" });
  row.classList.remove("action-locate");
  void row.offsetWidth; // 强制重排以重启动画
  row.classList.add("action-locate");
  row.addEventListener("animationend", () => row.classList.remove("action-locate"), { once: true });
  return true;
}

export function locateStepButton(run: RunRecord, step: ActionStep): HTMLButtonElement {
  const btn = el("button", "ghost msg-action-btn", tr("定位到动作")) as HTMLButtonElement;
  btn.type = "button";
  btn.title = tr("跳到动作编辑器里的这一条");
  btn.addEventListener("click", () => {
    void openActionAt(findRunOwner(run), { title: step.title, kind: step.kind });
  });
  return btn;
}

function locateResultButton(r: CommandResult): HTMLButtonElement {
  const btn = el("button", "ghost msg-action-btn", tr("定位到动作")) as HTMLButtonElement;
  btn.type = "button";
  btn.title = tr("跳到动作编辑器里的这一条");
  btn.addEventListener("click", () => {
    void openActionAt(findRunOwner({ name: r.name, trigger: r.trigger }), { title: r.command });
  });
  return btn;
}

function resultCard(r: CommandResult): HTMLElement {
  const card = el("div", "msg-card");
  card.append(msgField(tr("快捷键"), r.trigger));
  if (r.name) card.append(msgField(tr("名称"), r.name));
  card.append(msgField(tr("时间"), r.time));
  card.append(msgField(tr("命令"), r.command, true));
  if (r.detached) {
    card.append(
      msgField(tr("状态"), tr("已启动，不等待完成；未捕获输出（「停止执行」也收不回它）")),
    );
  }
  card.append(msgOutputField("stdout", r.stdout));
  if (r.stderr) card.append(msgOutputField("stderr", r.stderr));
  if (!r.detached) {
    card.append(msgField(tr("退出码"), r.exit_code === null ? "—" : String(r.exit_code)));
  }
  const actions = resultActions(r);
  if (r.run_id !== null) actions.prepend(locateResultButton(r));
  if (actions.childElementCount > 0) card.append(actions);
  return card;
}

// ---- 执行记录 / 时间线 / 流水线（U-03 已搬到 ./runs）----
// renderRunCard / msgEntries / runState / stepLabel / stepDetail 由 runs.ts 提供；renderMessages 仍在入口。


export function renderMessages() {
  renderBadge();

  // 冲突标签角标 + 冲突清单（常驻刷新，无论当前在哪个标签）。
  const conflictCount = document.getElementById("msg-conflict-count")!;
  conflictCount.textContent = String(state.conflicts.length);
  conflictCount.classList.toggle("hidden", state.conflicts.length === 0);
  renderConflictList();

  // 视图切换：执行记录 / 冲突。
  const onResults = state.msgView === "results";
  document.getElementById("msg-results-view")!.classList.toggle("hidden", !onResults);
  document.getElementById("msg-conflicts-view")!.classList.toggle("hidden", onResults);
  document.getElementById("msg-view-results")!.classList.toggle("active", onResults);
  document.getElementById("msg-view-conflicts")!.classList.toggle("active", !onResults);
  document.getElementById("clear-messages")!.classList.toggle("hidden", !onResults);

  // 执行记录视图内容（tablist / 空态 / 记录卡）。
  const tablist = document.getElementById("msg-tablist")!;
  const empty = document.getElementById("msg-empty")!;
  const content = document.getElementById("msg-content")!;
  const entries = msgEntries();
  tablist.replaceChildren();
  const has = entries.length > 0;
  empty.classList.toggle("hidden", has);
  content.classList.toggle("hidden", !has);
  (document.getElementById("msg-prev") as HTMLButtonElement).disabled = !has;
  (document.getElementById("msg-next") as HTMLButtonElement).disabled = !has;
  if (!has) return;
  // 选中项按 id 记住（后台线程会持续推更新，按下标记住会漂）；找不到就退回原下标。
  let sel = entries.findIndex((e) => e.id === state.msgSelId);
  if (sel < 0) {
    sel = Math.min(Math.max(state.msgIndex, 0), entries.length - 1);
    state.msgSelStep = -1;
  }
  state.msgIndex = sel;
  state.msgSelId = entries[sel].id;
  entries.forEach((e, i) => {
    const st = runState(e.run);
    const t = el("button", `msg-tab ${st.cls}` + (i === state.msgIndex ? " active" : ""));
    // 状态只留一个色点（与任务名同一行平齐），文字标签收进 tooltip。
    t.append(el("i", "msg-tab-dot"), el("span", "msg-tab-title", e.label));
    t.title = `${st.label} · ${e.label} · ${e.time}`;
    t.addEventListener("click", () => {
      state.msgIndex = i;
      state.msgSelId = e.id;
      state.msgSelStep = -1;
      renderMessages();
    });
    tablist.append(t);
  });
  const active = tablist.children[state.msgIndex] as HTMLElement | undefined;
  active?.scrollIntoView({ block: "nearest", inline: "nearest" });
  renderRunCard(entries[state.msgIndex].run);
}

// 弹窗打开前的焦点：关掉后还回去（还给已重绘消失的元素等于没还，isConnected 挡掉）。
let resultModalPrevFocus: HTMLElement | null = null;

export function showResultModal(r: CommandResult) {
  document.getElementById("modal-title")!.textContent = trt`命令结果 · ${r.label}`;
  document.getElementById("modal-body")!.replaceChildren(resultCard(r));
  const modal = document.getElementById("result-modal")!;
  if (modal.classList.contains("hidden")) {
    resultModalPrevFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }
  modal.classList.remove("hidden");
  document.getElementById("modal-close")!.focus();
}

// 停止正在执行的动作：后端返回请求发出时「正在执行的动作链数量」。
// 0 说明此刻没有可停的东西——如实告诉用户，而不是假装停成功了。
export async function abortActions() {
  try {
    const running = await invoke<number>("abort_actions");
    toast(running > 0 ? trt`已请求停止（${running} 条正在执行）` : tr("当前没有正在执行的动作"));
  } catch (e) {
    toast(trt`停止失败: ${e}`);
  }
}

export function hideResultModal() {
  document.getElementById("result-modal")!.classList.add("hidden");
  const prev = resultModalPrevFocus;
  resultModalPrevFocus = null;
  if (prev && prev.isConnected) prev.focus();
}

export function bindMessages() {
  document.getElementById("msg-view-results")!.addEventListener("click", () => {
    state.msgView = "results";
    renderMessages();
  });
  document.getElementById("msg-view-conflicts")!.addEventListener("click", () => {
    state.msgView = "conflicts";
    renderMessages();
  });
  document.getElementById("msg-mode-timeline")!.addEventListener("click", () => {
    state.msgMode = "timeline";
    state.msgSelStep = -1;
    renderMessages();
  });
  document.getElementById("msg-mode-pipeline")!.addEventListener("click", () => {
    state.msgMode = "pipeline";
    renderMessages();
  });
  document.getElementById("msg-prev")!.addEventListener("click", () => {
    const entries = msgEntries();
    if (!entries.length) return;
    state.msgIndex = (state.msgIndex - 1 + entries.length) % entries.length;
    state.msgSelId = entries[state.msgIndex].id;
    state.msgSelStep = -1;
    renderMessages();
  });
  document.getElementById("msg-next")!.addEventListener("click", () => {
    const entries = msgEntries();
    if (!entries.length) return;
    state.msgIndex = (state.msgIndex + 1) % entries.length;
    state.msgSelId = entries[state.msgIndex].id;
    state.msgSelStep = -1;
    renderMessages();
  });
  onClickAsync(document.getElementById("clear-logs")!, async () => {
    const ok = await askConfirm({
      title: tr("清理命令日志"),
      text: tr(
        "删除所有完整日志文件（执行记录里保留的输出摘录不受影响，仍可复制 / 定位到动作）。清理后「打开完整日志」会失效。",
      ),
      okLabel: tr("清理"),
      danger: true,
    });
    if (!ok) return;
    try {
      const n = await invoke<number>("clear_command_logs");
      toast(n > 0 ? trt`已清理 ${n} 份日志` : tr("没有可清理的日志"));
    } catch (e) {
      toast(trt`清理失败: ${e}`);
    }
  });
  document.getElementById("clear-messages")!.addEventListener("click", () => {
    void invoke("clear_command_results");
    state.messages = [];
    state.runs = [];
    state.msgIndex = 0;
    state.msgSelId = "";
    state.msgSelStep = -1;
    renderMessages();
  });
  document.getElementById("modal-close")!.addEventListener("click", hideResultModal);
  document.querySelector(".modal-mask")!.addEventListener("click", hideResultModal);
}
