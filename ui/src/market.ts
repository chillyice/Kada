import { invoke } from "@tauri-apps/api/core";
import { save as saveFile } from "@tauri-apps/plugin-dialog";
import { el, toast } from "./dom";
import { tr, trt } from "./i18n";
import { state } from "./state";
import { actionSummary } from "./options";
import { arrOf, kindLabel, kindOf, labelOf } from "./list";
import {
  load,
  remapSummary,
  switchSection,
} from "./main";
import { showImportResult } from "./settings";
import type { ImportOutcome } from "./settings";
import type {
  AnyEntry,
  FolderEntry,
  Gesture,
  HotCornerRule,
  ListKind,
  Remap,
  ShortcutItem,
  TextExpansion,
  TouchpadGesture,
  WheelEdgeRule,
} from "./types";

// 配置卡带 + 卡带市场 + 卡带发布（U-03 从 main.ts 搬出）。市场没有服务端：
// 索引是一份静态 JSON，搜索 / 标签筛选 / 机审展示 / 发布提交都在本模块。
// 事件绑定跟视图走（bindMarket），由入口的 bind() 调用。
// ---- 配置卡带（规划 7.2-㉕）：把若干条目导出成一个可分享的 JSON 文件 ----

// 卡带点名表（后端 `export_fragment` 按「类别 + 下标」取条目）。
type SnippetPick = { kind: ListKind; idx: number };

// 当前选中集的点名表（对象引用 → 各自数组里的下标；不在任何数组里的略过）。
export function picksOfSelection(): SnippetPick[] {
  const picks: SnippetPick[] = [];
  for (const o of state.picked) {
    const kind = kindOf(o);
    if (!kind) continue;
    const idx = arrOf(kind).indexOf(o as AnyEntry);
    if (idx >= 0) picks.push({ kind, idx });
  }
  return picks;
}

// 某个目录（含全部子目录）里所有条目的点名表——「导出该目录为卡带」用。
export function picksInFolder(rootId: string): SnippetPick[] {
  const ids = new Set<string>([rootId]);
  // 目录树可嵌套，扫到不再新增为止（目录数量级很小，一趟轮询足够）。
  for (let grew = true; grew; ) {
    grew = false;
    for (const f of state.cfg.folders) {
      if (f.parent && ids.has(f.parent) && !ids.has(f.id)) {
        ids.add(f.id);
        grew = true;
      }
    }
  }
  const picks: SnippetPick[] = [];
  const scan = (arr: FolderEntry[], kind: ListKind) =>
    arr.forEach((e, idx) => {
      if (e.folder && ids.has(e.folder)) picks.push({ kind, idx });
    });
  scan(state.cfg.shortcuts, "shortcut");
  scan(state.cfg.remaps, "remap");
  scan(state.cfg.expansions, "expansion");
  return picks;
}

// 导出卡带：选路径 → 后端摘出这些条目写成 JSON。
// 卡带是「只含这几条的子集配置」，对方用「导入配置 → 合并」原样吃下，不会碰本机设置。
export async function exportFragment(picks: SnippetPick[]) {
  if (!picks.length) return toast(tr("没有可导出的条目"));
  const path = await saveFile({
    defaultPath: tr("kada-卡带.json"),
    filters: [{ name: "JSON", extensions: ["json"] }],
  });
  if (!path) return;
  try {
    const n = await invoke<number>("export_fragment", { path, picks });
    toast(trt`已导出 ${n} 项到 ${path}（对方用「导入配置 → 合并」即可）`);
  } catch (e) {
    toast(trt`导出失败: ${e}`);
  }
}

// ---- 卡带市场（规划 7.2-㉕）：左栏一级页 / 搜索 / 标签筛选 / 一键安装 ----
//
// 没有服务端：索引是一份能下载到的静态 JSON（地址在设置里，默认指向 ihomy 的
// snippets/index.json）。索引一次拿全，搜索与筛选都在本地做——几百条以内不值得为它加服务端。

type IndexEntry = {
  name: string;
  url: string;
  description?: string;
  author?: string;
  tags?: string[];
  level?: number;
};

// 卡带机审结果（后端 `scan_snippet`）：0 安全 / 1 低 / 2 中 / 3 高。
type SnippetScan = {
  level: number;
  entries: number;
  findings: string[];
  legal_flags: string[];
};

// 卡带下载结果（后端 `fetch_snippet`）：清洗后的条目 + 原文（装的时候回传原文，
// 保证「看到什么装什么」）。
type SnippetFetch = {
  text: string;
  scan: SnippetScan;
  preview: {
    shortcuts: ShortcutItem[];
    remaps: Remap[];
    expansions: TextExpansion[];
    gestures?: Gesture[];
    touchpad_gestures?: TouchpadGesture[];
    hot_corners?: HotCornerRule[];
    wheel_edges?: WheelEdgeRule[];
  };
};

let marketEntries: IndexEntry[] = [];
// 成功拿到过索引；失败不置位，重新进页面会自动重试。
let marketLoaded = false;
let marketLoading = false;
let marketError = "";
let marketQuery = "";
let marketTag: string | null = null;
// 本次会话里装过的卡带：只给卡片标一下，不落盘——用户可能随后删掉条目，
// 持久化成「已安装」会骗人。
const marketInstalled = new Set<string>();

// 预览框点「安装」时要用：null = 没待装的卡带。`level` = 机审危险等级（高危要额外确认）。
let snippetPending: { text: string; name: string; level: number } | null = null;

// 进页面 / 点「重新拉取」时调用；`force` = 忽略已拿到的索引重新下载。
export async function loadMarket(force = false) {
  if (marketLoading) return;
  if (marketLoaded && !force) {
    renderMarketPage();
    return;
  }
  marketLoading = true;
  marketError = "";
  renderMarketPage();
  try {
    marketEntries = await invoke<IndexEntry[]>("fetch_snippet_index");
    marketLoaded = true;
  } catch (e) {
    marketEntries = [];
    marketError = String(e);
  } finally {
    marketLoading = false;
    renderMarketPage();
  }
}

// 设置页换了索引地址：下次进市场必须重新拉（由入口在保存设置后调用）。
export function resetMarketCache() {
  marketLoaded = false;
}

// 索引里的标签按出现次数排序（同次数按字典序）：常用的分类先出现，列表不会每次刷新换个顺序。
function marketTags(): string[] {
  const count = new Map<string, number>();
  for (const e of marketEntries) {
    for (const raw of e.tags ?? []) {
      const tag = raw.trim();
      if (tag) count.set(tag, (count.get(tag) ?? 0) + 1);
    }
  }
  return [...count.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "zh"))
    .map(([tag]) => tag);
}

export function renderMarketPage() {
  const note = document.getElementById("market-note");
  const tagsBox = document.getElementById("market-tags");
  const list = document.getElementById("market-list");
  if (!note || !tagsBox || !list) return;

  note.textContent = marketLoading
    ? tr("正在拉取索引…")
    : marketError
      ? trt`拉取失败：${marketError}（可在「设置 → 配置与数据」里换索引地址）`
      : marketLoaded
        ? trt`共 ${marketEntries.length} 条`
        : tr("还没拉取索引");
  // 发布只对官方市场开放：自定义索引是别人的市场，把卡带发到 ihomy 既不在用户眼前，
  // 也没有取得同意（服务端也不认自定义索引里的规则）。
  document.getElementById("market-publish")?.classList.toggle("hidden", !marketIsOfficial());

  // 索引换了（或第一条标签被删掉）时把已选标签收掉，免得停在一个不存在的标签上筛出空列表。
  const tags = marketTags();
  if (marketTag && !tags.includes(marketTag)) marketTag = null;
  tagsBox.classList.toggle("hidden", tags.length < 2);
  const tagButton = (tag: string | null, label: string): HTMLButtonElement => {
    const b = el("button", "market-tag", label) as HTMLButtonElement;
    const on = tag === marketTag;
    b.classList.toggle("active", on);
    b.setAttribute("aria-pressed", String(on));
    b.addEventListener("click", () => {
      marketTag = tag;
      renderMarketPage();
    });
    return b;
  };
  tagsBox.replaceChildren(tagButton(null, tr("全部")), ...tags.map((t) => tagButton(t, t)));

  if (marketLoading) {
    list.replaceChildren(el("p", "market-empty", tr("正在拉取索引…")));
    return;
  }
  if (marketError) {
    list.replaceChildren(
      el("p", "market-empty", tr("索引拉不下来：检查网络，或在「设置 → 配置与数据」里换一个索引地址后点「重新拉取」。")),
    );
    return;
  }
  const q = marketQuery.trim().toLowerCase();
  const hit = marketEntries.filter((e) => {
    if (marketTag && !(e.tags ?? []).includes(marketTag)) return false;
    if (!q) return true;
    return [e.name, e.description ?? "", e.author ?? "", ...(e.tags ?? [])]
      .join(" ")
      .toLowerCase()
      .includes(q);
  });
  if (!hit.length) {
    list.replaceChildren(
      el("p", "market-empty", q || marketTag ? tr("没有匹配的卡带") : tr("索引里一条卡带也没有")),
    );
    return;
  }
  list.replaceChildren(...hit.map(marketCard));
}

function riskLevel(level: number | undefined): number {
  return typeof level === "number" ? Math.max(0, Math.min(3, level)) : -1;
}

function riskLabel(level: number): string {
  return level <= 0
    ? tr("安全")
    : level === 1
      ? tr("低风险")
      : level === 2
        ? tr("中风险")
        : tr("高风险");
}

function riskHint(level: number): string {
  return level <= 0
    ? tr("只有文本 / 改键 / 层这类内容，不执行程序")
    : level === 1
      ? tr("会注入按键或文本、操作鼠标 / 剪贴板")
      : level === 2
        ? tr("会启动程序、打开文件 / 网址或改系统状态")
        : tr("含命令 / 脚本动作，可能运行任意代码");
}

function riskBadge(level: number): HTMLElement {
  const b = el("span", `risk-badge risk-${riskLevel(level)}`, riskLabel(level));
  b.title = riskHint(level);
  return b;
}

// 机器检查结果块：发布页与安装预览共用。只显示结论与命中项，不渲染卡带内容。
function renderScanBox(box: HTMLElement, scan: SnippetScan | null, error = "") {
  box.replaceChildren();
  if (error) {
    box.append(el("p", "scan-error", error));
    return;
  }
  if (!scan) {
    box.append(el("p", "scan-empty", tr("选择条目后开始机器检查")));
    return;
  }
  const level = riskLevel(scan.level);
  const head = el("div", "scan-head");
  head.append(riskBadge(level), el("span", "scan-count", trt`共 ${scan.entries} 条`));
  box.append(head);
  if (scan.findings.length) {
    const ul = el("ul", "scan-list");
    for (const f of scan.findings) ul.append(el("li", undefined, f));
    box.append(ul);
  }
  if (scan.legal_flags.length) {
    const flags = el("div", "scan-legal");
    flags.append(el("strong", undefined, tr("法务关键词（会重点人工审核）：")));
    for (const f of scan.legal_flags) flags.append(el("span", undefined, f));
    box.append(flags);
  }
  if (!scan.findings.length && !scan.legal_flags.length) {
    box.append(el("p", "scan-empty", tr("未发现风险动作或法务关键词")));
  }
}

export const DEFAULT_SNIPPET_INDEX_URL = "https://ihomy.top/files/kada/snippets/index.json";

function marketIsOfficial(): boolean {
  const url = state.cfg.settings.snippet_index_url?.trim();
  return !url || url === DEFAULT_SNIPPET_INDEX_URL;
}

function marketCard(e: IndexEntry): HTMLElement {
  const card = el("div", "market-card");
  const head = el("div", "market-card-head");
  const installed = marketInstalled.has(e.name);
  const btn = el(
    "button",
    installed ? undefined : "primary",
    installed ? tr("已安装") : tr("安装…"),
  ) as HTMLButtonElement;
  card.classList.toggle("installed", installed);
  btn.title = installed
    ? tr("再装一次只会补上缺失的条目（撞车的跳过）")
    : tr("先预览里面的条目，确认后再合并进来");
  btn.addEventListener("click", () => void previewSnippet(e));
  head.append(el("span", "market-name", e.name));
  if (typeof e.level === "number") head.append(riskBadge(e.level));
  head.append(el("span", "flex-spacer"), btn);
  card.append(head);
  if (e.description) card.append(el("div", "market-desc", e.description));
  if (e.tags?.length) {
    const chips = el("div", "market-tags market-card-tags");
    for (const t of e.tags) {
      const chip = el("button", "market-tag", t) as HTMLButtonElement;
      chip.title = trt`只看「${t}」标签`;
      chip.addEventListener("click", () => {
        marketTag = t;
        renderMarketPage();
      });
      chips.append(chip);
    }
    card.append(chips);
  }
  if (e.author) card.append(el("div", "market-meta", trt`分享者：${e.author}`));
  return card;
}

// 「索引设置」：索引地址留在设置页（配置与数据），市场页只负责浏览——两件事不挤在一屏。
async function gotoMarketSettings() {
  await switchSection("settings");
  document.getElementById("snippet-index-row")?.scrollIntoView({ block: "center", behavior: "smooth" });
  (document.getElementById("set-snippet-index") as HTMLInputElement | null)?.focus();
}

// 卡带装进来就会跑：危险等级由后端 `scan_snippet` 统一判定（发布页与安装预览同源）。
async function previewSnippet(e: IndexEntry) {
  try {
    const r = await invoke<SnippetFetch>("fetch_snippet", { url: e.url });
    const p = r.preview;
    const total =
      p.shortcuts.length +
      p.remaps.length +
      p.expansions.length +
      (p.gestures?.length ?? 0) +
      (p.touchpad_gestures?.length ?? 0) +
      (p.hot_corners?.length ?? 0) +
      (p.wheel_edges?.length ?? 0);
    const level = riskLevel(r.scan.level);
    snippetPending = { text: r.text, name: e.name, level };
    document.getElementById("snippet-name")!.textContent = trt`${e.name}：共 ${total} 条`;
    const lines: HTMLElement[] = [];
    for (const s of p.shortcuts)
      lines.push(
        el(
          "li",
          undefined,
          trt`快捷键 ${s.triggers.join(" / ")}${s.name ? ` · ${s.name}` : ""} → ${s.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const r2 of p.remaps)
      lines.push(el("li", undefined, trt`改键 ${r2.from} → ${remapSummary(r2)}`));
    for (const x of p.expansions)
      lines.push(el("li", undefined, trt`文本扩展 ${x.trigger} → ${x.replace.slice(0, 40)}`));
    for (const g of p.gestures ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`鼠标操作 ${g.pattern}${g.name ? ` · ${g.name}` : ""} → ${g.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const g of p.touchpad_gestures ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`触摸板手势 ${g.trigger ?? ""}·${g.pattern}${g.name ? ` · ${g.name}` : ""} → ${g.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const c of p.hot_corners ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`角动作 ${c.spot}${c.name ? ` · ${c.name}` : ""} → ${c.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    for (const w of p.wheel_edges ?? [])
      lines.push(
        el(
          "li",
          undefined,
          trt`边缘滚动 ${w.edge}${w.name ? ` · ${w.name}` : ""} → ${w.actions
            .map(actionSummary)
            .join(" → ")}`,
        ),
      );
    document.getElementById("snippet-list")!.replaceChildren(...lines);
    renderScanBox(document.getElementById("snippet-scan")!, r.scan);
    const dangerous = level >= 2;
    const warn = document.getElementById("snippet-warn")!;
    warn.classList.toggle("hidden", !dangerous);
    if (dangerous) {
      warn.textContent =
        level >= 3
          ? tr("⚠ 危险等级：高。这个卡带含命令 / 脚本动作，装进来照着触发键就会执行任意代码。")
          : tr("⚠ 危险等级：中。这个卡带会启动程序、打开文件 / 网址或改系统状态，请先看清动作。");
    }
    const consentRow = document.getElementById("snippet-consent-row")!;
    const consent = document.getElementById("snippet-consent") as HTMLInputElement;
    consentRow.classList.toggle("hidden", !dangerous);
    consent.checked = false;
    (document.getElementById("snippet-install") as HTMLButtonElement).disabled = dangerous;
    document.getElementById("snippet-modal")!.classList.remove("hidden");
  } catch (err) {
    toast(trt`卡带下载失败：${err}`);
  }
}

function hideSnippetPreview() {
  document.getElementById("snippet-modal")!.classList.add("hidden");
  snippetPending = null;
}

async function confirmSnippetInstall() {
  const pending = snippetPending;
  if (!pending) return;
  if (pending.level >= 2) {
    const consent = document.getElementById("snippet-consent") as HTMLInputElement;
    if (!consent.checked) {
      toast(tr("请先勾选「我已了解风险」再安装"));
      return;
    }
  }
  hideSnippetPreview();
  try {
    const r = await invoke<ImportOutcome>("install_snippet", { text: pending.text });
    await load();
    marketInstalled.add(pending.name);
    renderMarketPage();
    showImportResult(r);
  } catch (e) {
    toast(trt`安装失败: ${e}`);
  }
}

// ---- 卡带发布（本机机审 → ihomy 服务端复检 → 人工审核）----
//
// 发布内容 = 选中的条目组成子集配置（与「导出卡带」同一口径），目录 / 设置 / 本机路径
// 一律不进 JSON。危险等级与法务关键词只做提示：真正能不能上架由 ihomy 的人工审核决定。

type PublishPick = { kind: ListKind; idx: number };
type PreparedPublish = { content: string; scan: SnippetScan };
type PublishReceipt = { id: number; status: string; level: number };

const PUBLISH_KINDS: ListKind[] = [
  "shortcut",
  "remap",
  "expansion",
  "gesture",
  "touchpad",
  "hotcorner",
  "wheeledge",
];

let publishSelected = new Set<string>();
let publishScan: SnippetScan | null = null;
let publishScanError = "";
let publishScanBusy = false;
let publishNameTouched = false;
let publishSubmitting = false;
let publishSeq = 0;
let publishTimer: number | undefined;

function publishKey(kind: ListKind, idx: number): string {
  return `${kind}:${idx}`;
}

function publishPicks(): PublishPick[] {
  const picks: PublishPick[] = [];
  for (const kind of PUBLISH_KINDS) {
    arrOf(kind).forEach((_, idx) => {
      if (publishSelected.has(publishKey(kind, idx))) picks.push({ kind, idx });
    });
  }
  return picks;
}

function readPublishMeta() {
  const value = (id: string) => (document.getElementById(id) as HTMLInputElement).value.trim();
  const tags = value("publish-tags")
    .split(/[,，]/)
    .map((t) => t.trim())
    .filter(Boolean);
  return {
    name: value("publish-name"),
    description: (document.getElementById("publish-desc") as HTMLTextAreaElement).value.trim(),
    author: value("publish-author"),
    tags,
  };
}

// 未手动改过名称时，按选择自动起名（单条取条目名，多条报条数）。
function syncPublishName() {
  if (publishNameTouched) return;
  const picks = publishPicks();
  const name =
    picks.length === 0
      ? ""
      : picks.length === 1
        ? labelOf(arrOf(picks[0].kind)[picks[0].idx] as object)
        : trt`卡带（${picks.length} 项）`;
  (document.getElementById("publish-name") as HTMLInputElement).value = name;
}

function renderPublishPicks() {
  const box = document.getElementById("publish-picks");
  if (!box) return;
  const rows: HTMLElement[] = [];
  for (const kind of PUBLISH_KINDS) {
    arrOf(kind).forEach((o, idx) => {
      const key = publishKey(kind, idx);
      const row = el("label", "publish-pick-row");
      const cb = el("input") as HTMLInputElement;
      cb.type = "checkbox";
      cb.checked = publishSelected.has(key);
      cb.addEventListener("change", () => {
        if (cb.checked) publishSelected.add(key);
        else publishSelected.delete(key);
        document.getElementById("publish-count")!.textContent = trt`已选 ${publishSelected.size} 条`;
        syncPublishName();
        schedulePublishScan();
      });
      row.append(cb, el("span", "publish-pick-kind", kindLabel(kind)), el("span", "publish-pick-name", labelOf(o as object)));
      rows.push(row);
    });
  }
  if (!rows.length) rows.push(el("p", "scan-empty", tr("当前配置里没有可发布的条目")));
  box.replaceChildren(...rows);
  document.getElementById("publish-count")!.textContent = trt`已选 ${publishSelected.size} 条`;
}

function updatePublishSubmit() {
  const consent = document.getElementById("publish-consent") as HTMLInputElement;
  const btn = document.getElementById("publish-submit") as HTMLButtonElement;
  btn.disabled =
    publishSubmitting ||
    publishScanBusy ||
    !publishScan ||
    !!publishScanError ||
    publishSelected.size === 0 ||
    !consent.checked;
}

function schedulePublishScan() {
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  const box = document.getElementById("publish-scan");
  if (publishSelected.size === 0) {
    publishSeq++;
    publishScan = null;
    publishScanError = "";
    publishScanBusy = false;
    if (box) renderScanBox(box, null);
    updatePublishSubmit();
    return;
  }
  publishScanBusy = true;
  publishScan = null;
  publishScanError = "";
  if (box) box.replaceChildren(el("p", "scan-empty", tr("正在做机器检查…")));
  updatePublishSubmit();
  publishTimer = window.setTimeout(() => void runPublishScan(), 250);
}

async function runPublishScan() {
  const seq = ++publishSeq;
  const picks = publishPicks();
  const meta = readPublishMeta();
  const box = document.getElementById("publish-scan");
  try {
    const r = await invoke<PreparedPublish>("prepare_publish", { picks, meta });
    if (seq !== publishSeq) return;
    publishScan = r.scan;
    publishScanError = "";
    if (box) renderScanBox(box, r.scan);
  } catch (e) {
    if (seq !== publishSeq) return;
    publishScan = null;
    publishScanError = String(e);
    if (box) renderScanBox(box, null, publishScanError);
  } finally {
    if (seq === publishSeq) {
      publishScanBusy = false;
      updatePublishSubmit();
    }
  }
}

export function openPublish(initial: PublishPick[] = []) {
  if (!marketIsOfficial()) {
    toast(tr("当前使用自定义索引，不能发布到官方市场；请先在设置里换回内置地址"));
    return;
  }
  publishSelected = new Set(initial.map((p) => publishKey(p.kind, p.idx)));
  publishNameTouched = false;
  publishScan = null;
  publishScanError = "";
  publishScanBusy = false;
  publishSubmitting = false;
  publishSeq++;
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  (document.getElementById("publish-desc") as HTMLTextAreaElement).value = "";
  (document.getElementById("publish-tags") as HTMLInputElement).value = "";
  (document.getElementById("publish-author") as HTMLInputElement).value =
    localStorage.getItem("kada.publish.author") ?? "";
  (document.getElementById("publish-consent") as HTMLInputElement).checked = false;
  renderPublishPicks();
  syncPublishName();
  renderScanBox(document.getElementById("publish-scan")!, null);
  updatePublishSubmit();
  document.getElementById("publish-modal")!.classList.remove("hidden");
  schedulePublishScan();
}

function closePublish() {
  document.getElementById("publish-modal")!.classList.add("hidden");
  if (publishTimer !== undefined) window.clearTimeout(publishTimer);
  publishSeq++;
  publishSubmitting = false;
}

async function submitPublish() {
  if (publishSubmitting || !publishScan || publishScanError || publishSelected.size === 0) return;
  const meta = readPublishMeta();
  publishSubmitting = true;
  updatePublishSubmit();
  try {
    const r = await invoke<PublishReceipt>("submit_publish", { picks: publishPicks(), meta });
    localStorage.setItem("kada.publish.author", meta.author);
    closePublish();
    toast(trt`已提交审核（编号 #${r.id}，服务端判定：${riskLabel(r.level)}）；人工通过后才会出现在市场`);
  } catch (e) {
    toast(trt`发布失败：${e}`);
  } finally {
    publishSubmitting = false;
    updatePublishSubmit();
  }
}
// 卡带市场 / 发布页的事件绑定（原在入口 bind() 里，跟着视图搬出来）。
export function bindMarket() {
  // 卡带市场：设置页的按钮跳到独立市场页，搜索在本地过滤（索引已全量拿在手上）。
  document.getElementById("open-market")!.addEventListener("click", () => void switchSection("market"));
  document.getElementById("market-settings")!.addEventListener("click", () => void gotoMarketSettings());
  document.getElementById("market-reload")!.addEventListener("click", () => void loadMarket(true));
  document.getElementById("market-search")!.addEventListener("input", (e) => {
    marketQuery = (e.target as HTMLInputElement).value;
    renderMarketPage();
  });
  document.getElementById("snippet-install")!.addEventListener("click", () => void confirmSnippetInstall());
  document.getElementById("snippet-cancel")!.addEventListener("click", hideSnippetPreview);
  document.getElementById("snippet-modal")!.querySelector(".modal-mask")!.addEventListener("click", hideSnippetPreview);
  (document.getElementById("snippet-consent") as HTMLInputElement).addEventListener("change", (e) => {
    (document.getElementById("snippet-install") as HTMLButtonElement).disabled = !(e.target as HTMLInputElement).checked;
  });

  // 发布卡带：入口在市场标题行与行菜单（⋯ / 右键）都开同一个发布页。
  document.getElementById("market-publish")!.addEventListener("click", () => openPublish());
  document.getElementById("publish-close")!.addEventListener("click", closePublish);
  document.getElementById("publish-cancel")!.addEventListener("click", closePublish);
  document.getElementById("publish-modal")!.querySelector(".modal-mask")!.addEventListener("click", closePublish);
  document.getElementById("publish-select-all")!.addEventListener("click", () => {
    for (const kind of PUBLISH_KINDS) {
      arrOf(kind).forEach((_, idx) => publishSelected.add(publishKey(kind, idx)));
    }
    syncPublishName();
    renderPublishPicks();
    schedulePublishScan();
  });
  document.getElementById("publish-select-none")!.addEventListener("click", () => {
    publishSelected.clear();
    syncPublishName();
    renderPublishPicks();
    schedulePublishScan();
  });
  (document.getElementById("publish-name") as HTMLInputElement).addEventListener("input", () => {
    publishNameTouched = true;
    schedulePublishScan();
  });
  (document.getElementById("publish-desc") as HTMLTextAreaElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-author") as HTMLInputElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-tags") as HTMLInputElement).addEventListener("input", schedulePublishScan);
  (document.getElementById("publish-consent") as HTMLInputElement).addEventListener("change", updatePublishSubmit);
  document.getElementById("publish-submit")!.addEventListener("click", () => void submitPublish());
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    if (!document.getElementById("publish-modal")!.classList.contains("hidden")) {
      e.preventDefault();
      closePublish();
    }
  });
}
