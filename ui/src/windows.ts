import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { el } from "./dom";
import { tr, trt } from "./i18n";
import { applyTheme, loadTheme } from "./main";
import type { Config, HintsState, ShortcutItem, StatusPayload } from "./types";

// 独立窗口 + 自绘标题栏（U-04 从 main.ts 搬出）。五个窗口共用同一份 index.html：
// #toast / #hud / #hints / #gesture / #prompt / #clipboard-menu / #tray-menu 各走一个 bootstrap，
// 主窗口只多挂自绘标题栏。启动分发仍留在入口 main.ts，这里只导出各窗口的装配函数。
// 右下角触发气泡窗口（独立隐藏窗口，加载 index.html#toast）：
// 只渲染气泡，不听主界面逻辑。
export async function bootstrapToast() {
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
export async function bootstrapHud() {
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

export async function bootstrapPrompt() {
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

export async function bootstrapHints() {
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

export async function bootstrapGesture() {
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

export async function bootstrapClipboardMenu(preview = false) {
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

export async function bootstrapTrayMenu(preview = false) {
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

export function mountTitlebar() {
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
