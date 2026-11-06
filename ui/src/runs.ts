import { el, fmtMs } from "./dom";
import { tr, trt } from "./i18n";
import { state } from "./state";
import { ACTION_ICONS, ACTION_TYPE_LABELS, condSummary } from "./options";
import type { ActionBase, ActionStep, Condition, RunRecord } from "./types";
import { abortActions, locateStepButton, msgField, msgOutputField, renderMessages, resultActions } from "./messages";

// 执行记录 / 时间线 / 流水线（U-03 从 main.ts 搬出）：只放「一条记录怎么画」。
// 页面级编排（renderMessages / 消息中心 / 结果卡）仍在入口，等 messages 批次再搬。
// ---- 执行记录（时间线 / 流水线）----

// 执行时刻（绝对钟点）：记录的起始 epoch 毫秒 + 相对毫秒 → HH:MM:SS.mmm
function clockAt(run: RunRecord, offsetMs: number): string {
  const d = new Date(run.start_epoch_ms + offsetMs);
  const p = (n: number, w = 2) => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

const STEP_STATUS_LABEL: Record<string, string> = {
  ok: tr("成功"),
  error: tr("失败"),
  abort: tr("已中止"),
  running: tr("执行中"),
};

const RUN_STATUS_LABEL: Record<string, string> = {
  running: tr("执行中"),
  ok: tr("成功"),
  error: tr("有失败"),
  abort: tr("已中止"),
};

// 记录列表里一眼要分得清的状态：跑起来 = 执行中；一个动作都没落 = 未执行
//（记录先落、动作还没开跑，或修饰键没松开整条被跳过）；跑完按结论分执行完成 / 有失败 / 已中止。
// class 用 is-*（别用 st-*：那套是给带边框的元素设 border-color 的，挂到 tab 上会给整格染边框）。
export function runState(run: RunRecord): { cls: string; label: string } {
  if (run.status === "running") {
    return run.steps.length ? { cls: "is-running", label: tr("执行中") } : { cls: "is-idle", label: tr("未执行") };
  }
  if (!run.steps.length) return { cls: "is-idle", label: tr("未执行") };
  if (run.status === "ok") return { cls: "is-ok", label: tr("执行完成") };
  if (run.status === "error") return { cls: "is-error", label: tr("有失败") };
  return { cls: "is-abort", label: tr("已中止") };
}

// 记录 tab 的一行：优先用名称，否则触发键（时刻不进标签，悬停 tooltip 里看）。
function entryTitle(run: RunRecord): string {
  return run.name || run.trigger;
}

// 消息中心一条 tab：执行记录，或一条不属于任何执行的普通消息（配置变更、热串失败等）。
// 后者在界面上按「只有一个动作块的记录」渲染，两类共用一套时间线 / 流水线。
type MsgEntry = { id: string; time: string; label: string; run: RunRecord };
export function msgEntries(): MsgEntry[] {
  const fromRuns: MsgEntry[] = state.runs.map((r) => ({
    id: `r${r.id}`,
    time: r.time,
    label: entryTitle(r),
    run: r,
  }));
  const fromMsgs: MsgEntry[] = state.messages
    .filter((r) => r.run_id === null)
    .map((r) => ({
      id: `m${r.time}-${r.kind}-${r.command}`,
      time: r.time,
      label: r.name ? `${r.label} · ${r.name}` : `${r.label} · ${r.trigger}`,
      run: {
        id: 0,
        trigger: r.trigger,
        name: r.name,
        time: r.time,
        start_epoch_ms: new Date(r.time.replace(" ", "T")).getTime() || Date.now(),
        duration_ms: 0,
        status: r.kind === "error" || r.kind === "abort" ? "error" : "ok",
        steps: [
          {
            id: 0,
            parent: null,
            lane: 0,
            kind: r.kind,
            title: r.command,
            branch: null,
            detail: null,
            status: r.kind === "ok" ? "ok" : "error",
            start_ms: 0,
            end_ms: 0,
            stdout: r.stdout,
            stderr: r.stderr,
            exit_code: r.exit_code,
          },
        ],
      } as RunRecord,
    }));
  return [...fromRuns, ...fromMsgs].sort((a, b) => (a.time < b.time ? 1 : a.time > b.time ? -1 : 0));
}

// 动作的一行标题：「类型 · 摘要」。摘要本身若已以类型名开头（后端渲的「并行执行 3 个动作」
// 之类）就不再重复一遍；条件判断改用条件摘要（光有「条件判断」四个字没信息量）。
function stepTitle(step: ActionStep): string {
  if (step.kind === "if" && step.detail) return condSummary(step.detail as Condition);
  return step.title || step.kind;
}

function stepLabel(step: ActionStep): string {
  const label = ACTION_TYPE_LABELS[step.kind as ActionBase["type"]] ?? step.kind;
  const title = stepTitle(step);
  return title.startsWith(label) ? title : `${label} · ${title}`;
}

// 动作块的详情（时间线点块看下方、流水线点块就地展开都用它）。
// **只列块上没有的信息**：动作名已经写在块标题里（时间线的块标题 / 详情面板标题、
// 流水线的块头），再列一行一模一样的「动作」纯属重复；条件判断的摘要在标题里已有一份
// （`stepTitle` 对 if 就是 `condSummary`），只在确实装不下时才补一行；
// 退出码为空（非命令类动作、或失败时没拿到）不占一行——以前这里恒有一行「退出码 —」。
function stepDetail(run: RunRecord, step: ActionStep): HTMLElement {
  const box = el("div", "msg-step-fields");
  const title = stepLabel(step);
  // 开始 / 结束 / 状态 挤在同一行（耗时不再单列：两个时刻相减即可看出）。
  const meta = el("div", "msg-step-meta");
  meta.append(
    msgField(tr("开始"), `${clockAt(run, step.start_ms)}（+${fmtMs(step.start_ms)}）`, false, true),
    msgField(tr("结束"), `${clockAt(run, step.end_ms)}（+${fmtMs(step.end_ms)}）`, false, true),
    msgField(tr("状态"), STEP_STATUS_LABEL[step.status] ?? step.status, false, true),
  );
  box.append(meta);
  if (step.kind === "if" && step.detail) {
    const summary = condSummary(step.detail as Condition);
    if (!title.includes(summary)) box.append(msgField(tr("条件"), summary));
    box.append(msgField(tr("走向"), step.branch ? tr("条件成立（then）") : tr("条件不成立（otherwise）")));
  }
  if (step.detached) {
    box.append(
      msgField(tr("状态"), tr("已启动，不等待完成；未捕获输出（「停止执行」也收不回它）")),
    );
  }
  if (step.stdout) box.append(msgOutputField("stdout", step.stdout));
  if (step.stderr) box.append(msgOutputField("stderr", step.stderr));
  if (step.exit_code !== null) box.append(msgField(tr("退出码"), String(step.exit_code)));
  const actions = resultActions(step);
  if (run.id !== 0) actions.prepend(locateStepButton(run, step));
  if (actions.childElementCount > 0) box.append(actions);
  return box;
}

// ---- 时间线：每个动作按真实起止时间占一段，横着排开（重叠的自动分泳道）----
// 时间块是贴着横轴挂下来的标签页：一行 42px（块 38px、两行内容），顶边不画线（轴线即顶边），
// 右侧 24px 渐变淡出；块宽尾部多出 12px 专门给后触发的块当遮盖（z-index 按触发顺序）——后块
// 的硬边切在前块渐变一半（alpha≈0.5）处，遮盖与渐变才都看得出来。max-width 把右边界压在区内：
// 末块恰好停在右侧留白前，不顶出页面。
const TL_LANE_H = 42;
const TL_TAIL_PX = 12;

function renderTimeline(run: RunRecord): HTMLElement {
  const host = el("div", "tl");
  const steps = run.steps.slice().sort((a, b) => a.start_ms - b.start_ms || a.id - b.id);
  if (steps.length === 0) {
    host.append(el("div", "tl-empty", run.status === "running" ? tr("正在执行…") : tr("没有可展示的动作")));
    return host;
  }
  const total = Math.max(run.duration_ms, ...steps.map((s) => s.end_ms), 1);
  // 泳道：贪心装不下就新开一条（区间分配），这样并行动作能真正并排看到。
  const laneEnds: number[] = [];
  const placed = steps.map((s) => {
    let lane = laneEnds.findIndex((e) => e <= s.start_ms);
    if (lane < 0) {
      lane = laneEnds.length;
      laneEnds.push(0);
    }
    laneEnds[lane] = s.end_ms;
    return { step: s, lane };
  });
  const ruler = el("div", "tl-ruler");
  for (let i = 0; i <= 4; i++) {
    const tick = el("span", "tl-tick", `+${fmtMs(Math.round((total * i) / 4))}`);
    tick.style.left = `${(i * 100) / 4}%`;
    ruler.append(tick);
  }
  host.append(ruler);
  const rows = el("div", "tl-rows");
  // 泳道数决定时间线的高度（每条 TL_LANE_H 一行，末行占 TL_LANE_H - 4）。
  rows.style.height = `${laneEnds.length * TL_LANE_H - 4}px`;
  for (const [i, { step, lane }] of placed.entries()) {
    const left = step.start_ms / total;
    const frac = Math.max((step.end_ms - step.start_ms) / total, 0.02);
    const block = el(
      "button",
      `tl-block st-${step.status}` + (step.id === state.msgSelStep ? " sel" : ""),
    );
    block.style.left = `${left * 100}%`;
    block.style.width = `calc(${frac * 100}% + ${TL_TAIL_PX}px)`;
    block.style.maxWidth = `calc(100% - ${left * 100}%)`;
    block.style.top = `${lane * TL_LANE_H}px`;
    block.style.zIndex = String(i + 1); // 后触发的盖在前一块尾巴上（标签页式遮盖）
    block.title = `${ACTION_ICONS[step.kind as ActionBase["type"]] ?? ""} ${stepLabel(step)}\n${clockAt(run, step.start_ms)} → ${clockAt(run, step.end_ms)}（${fmtMs(Math.max(0, step.end_ms - step.start_ms))}）`;
    const head = el("span", "tl-block-head");
    head.append(el("span", "tl-block-ico", ACTION_ICONS[step.kind as ActionBase["type"]] ?? "•"));
    head.append(el("span", "tl-block-title", stepLabel(step)));
    const meta = el("span", "tl-block-meta");
    meta.append(el("span", "tl-block-ms", fmtMs(Math.max(0, step.end_ms - step.start_ms))));
    meta.append(el("span", "tl-block-flag", STEP_STATUS_LABEL[step.status] ?? step.status));
    block.append(head, meta);
    block.addEventListener("click", () => {
      state.msgSelStep = state.msgSelStep === step.id ? -1 : step.id;
      renderMessages();
    });
    rows.append(block);
  }
  host.append(rows);
  return host;
}

// ---- 流水线：自上而下，条件 / 并行各占泳道（像 IDE 的提交图，只是从上往下）----
function renderPipeline(run: RunRecord): HTMLElement {
  const host = el("div", "pipe");
  if (run.steps.length === 0) {
    host.append(el("div", "tl-empty", run.status === "running" ? tr("正在执行…") : tr("没有可展示的动作")));
    return host;
  }
  const kids = new Map<number, ActionStep[]>();
  for (const s of run.steps) {
    const key = s.parent ?? -1;
    const list = kids.get(key);
    if (list) list.push(s);
    else kids.set(key, [s]);
  }
for (const list of kids.values()) list.sort((a, b) => a.lane - b.lane || a.id - b.id);

  const build = (step: ActionStep): HTMLElement => {
    const block = el("div", `pipe-block st-${step.status}`);
    const head = el("div", "pipe-head");
    head.append(el("span", "pipe-ico", ACTION_ICONS[step.kind as ActionBase["type"]] ?? "•"));
    head.append(el("span", "pipe-title", stepLabel(step)));
    head.append(el("span", "pipe-ms", fmtMs(Math.max(0, step.end_ms - step.start_ms))));
    head.append(el("span", `pipe-flag st-${step.status}`, STEP_STATUS_LABEL[step.status] ?? step.status));
    block.append(head);
    const detail = stepDetail(run, step);
    detail.classList.add("pipe-detail");
    detail.classList.add("hidden");
    block.append(detail);
    // 只在块头（标题行）点开 / 收起：块体里有筛选框、「展开」、复制、打开日志、定位到动作等控件，
    // 整块绑 click 会让「点一下筛选框 / 展开」连带把刚展开的详情收回去。
    block.addEventListener("click", (e) => {
      if (!(e.target as Element | null)?.closest(".pipe-head")) return;
      detail.classList.toggle("hidden");
    });

    const node = el("div", "pipe-node");
    node.append(block);
    const children = kids.get(step.id) ?? [];
    if (children.length) {
      const rail = el("div", "pipe-rail");
      children.forEach((c, i) => {
        const lane = el("div", "pipe-lane");
        // 条件只画走的那条分支；并行每个成员一条泳道（序号对应编排里的成员下标）；
        // 分组成员按顺序串在同一条泳道里，标一个「分组」便于和顶层区分。
        lane.append(
          el(
            "span",
            "pipe-lane-label",
            step.kind === "if"
              ? step.branch
                ? tr("条件成立")
                : tr("条件不成立")
              : step.kind === "parallel"
                ? trt`并行 ${i + 1}`
                : step.kind === "group"
                  ? tr("分组")
                  : "",
          ),
        );
        lane.append(build(c));
        rail.append(lane);
      });
      node.append(rail);
    }
    return node;
  };
  for (const s of kids.get(-1) ?? []) host.append(build(s));
  return host;
}

export function renderRunCard(run: RunRecord): void {
  const title = document.getElementById("msg-run-title")!;
  title.textContent = `${run.name || tr("（未命名）")} · ${run.trigger} · ${run.time}`;
  const badges = document.getElementById("msg-run-badges")!;
  badges.replaceChildren(
    el("span", `msg-run-badge st-${run.status}`, RUN_STATUS_LABEL[run.status] ?? run.status),
  );
  // 停止执行只对跑着的记录有意义：图标按钮紧贴「执行中」徽标右侧，跑完即消失。
  if (run.status === "running") {
    const stop = el("button", "msg-run-stop") as HTMLButtonElement;
    stop.type = "button";
    stop.title = tr("停止执行：剩余动作不再执行，正在跑的命令 / 脚本立即终止");
    stop.setAttribute("aria-label", tr("停止执行"));
    stop.innerHTML =
      '<svg viewBox="0 0 24 24"><rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor"/></svg>'; // 常量 SVG，非用户输入
    stop.addEventListener("click", () => void abortActions());
    badges.append(stop);
  }
  badges.append(
    el("span", "msg-run-badge", trt`总耗时 ${fmtMs(run.duration_ms)}`),
    el("span", "msg-run-badge", trt`${run.steps.length} 个动作`),
  );
  document.getElementById("msg-run-modes")!.classList.toggle("hidden", run.steps.length === 0);
  const timeline = document.getElementById("msg-timeline")!;
  const pipeline = document.getElementById("msg-pipeline")!;
  const onTimeline = state.msgMode === "timeline";
  timeline.classList.toggle("hidden", !onTimeline);
  pipeline.classList.toggle("hidden", onTimeline);
  (document.getElementById("msg-mode-timeline") as HTMLElement).classList.toggle("active", onTimeline);
  (document.getElementById("msg-mode-pipeline") as HTMLElement).classList.toggle("active", !onTimeline);
  const detail = document.getElementById("msg-step-detail")!;
  if (onTimeline) {
    timeline.replaceChildren(renderTimeline(run));
    pipeline.replaceChildren();
    const sel = run.steps.find((s) => s.id === state.msgSelStep);
    detail.classList.toggle("hidden", !sel);
    if (sel) detail.replaceChildren(el("div", "msg-step-title", stepLabel(sel)), stepDetail(run, sel));
  } else {
    timeline.replaceChildren();
    pipeline.replaceChildren(renderPipeline(run));
    detail.classList.add("hidden");
  }
}
