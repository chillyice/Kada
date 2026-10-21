// 界面文案（i18n）：中文是源码里的原文，英文在 `i18n.en.json` 里按原文索引。
//
// - `tr("中文")`：普通字符串，查表；查不到（或当前是中文）原样返回；
// - `trt`…${x}…``：带插值的模板，key 是「中式原文片段用 {0}/{1} 拼起来」，英文值里用同样的
//   占位符，运行时把值填回去（不做二次替换，值里出现 `{0}` 也不会被吃掉）；
// - `localizeStaticDom()`：把 index.html 里的静态整块文案翻译掉（只翻纯文本块，见
//   `scripts/i18n-html.mjs` 的同一口径）。
//
// 语言存在 localStorage（`kada.locale`）：它是「这台机器上用哪种语言看界面」，与配置同步无关
// （同一份配置给不同母语的人用，各看各的）。切换后整页 reload —— 比逐块重画可靠得多。
import EN from "./i18n.en.json";

const LOCALE_KEY = "kada.locale";
export type Locale = "zh" | "en";

function systemLocale(): Locale {
  return navigator.language?.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function currentLocale(): Locale {
  const v = localStorage.getItem(LOCALE_KEY);
  return v === "zh" || v === "en" ? v : systemLocale();
}

let locale: Locale = currentLocale();

/** 显式选语言（`null` = 跟随系统）；只写 localStorage，刷新后生效。 */
export function applyLocale(l: Locale | null) {
  if (l) localStorage.setItem(LOCALE_KEY, l);
  else localStorage.removeItem(LOCALE_KEY);
  locale = l ?? systemLocale();
  document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
}

/** 当前选择（`null` = 跟随系统），设置页下拉框用。 */
export function savedLocale(): Locale | null {
  const v = localStorage.getItem(LOCALE_KEY);
  return v === "zh" || v === "en" ? v : null;
}

export function isEnglish(): boolean {
  return locale === "en";
}

export function tr(zh: string): string {
  return locale === "zh" ? zh : (EN as Record<string, string>)[zh] ?? zh;
}

export function trt(strings: TemplateStringsArray, ...values: unknown[]): string {
  let key = strings[0];
  for (let i = 1; i < strings.length; i++) key += `{${i - 1}}` + strings[i];
  const tpl = locale === "zh" ? key : (EN as Record<string, string>)[key] ?? key;
  return tpl.replace(/\{(\d+)\}/g, (m, i) => {
    const v = values[Number(i)];
    return v === undefined || v === null ? m : String(v);
  });
}

/** 静态 HTML 的整块翻译：只替换「纯文本块」的 innerHTML（含 <code>/<strong> 这类行内标签），
 * 跳过自身或子孙里有表单控件 / svg / 图片的块，避免把控件与图标抹掉。
 *
 * 「整块」口径与 scripts/i18n-html.mjs 一致：块级标签，或没有元素子节点的 span / div ——
 * 否则 `<div class="set-card-head"><span>…</span></div>` 会整块命中，把 span 的样式一起换掉。 */
const BLOCK_TAGS = new Set([
  "p", "summary", "li", "button", "option", "label", "td", "th", "h1", "h2", "h3", "dt", "dd", "a",
]);

export function localizeStaticDom(root: HTMLElement = document.body) {
  document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
  if (locale === "zh") return;
  const dict = EN as Record<string, string>;
  const els = [...root.querySelectorAll<HTMLElement>("*")];
  // 属性先翻：innerHTML 替换后新节点不在 els 里，属性就翻不到了。
  for (const el of els) {
    for (const attr of ["title", "placeholder", "aria-label", "alt"]) {
      const v = el.getAttribute(attr);
      if (!v) continue;
      const hit = dict[v.trim()];
      if (hit !== undefined) el.setAttribute(attr, hit);
    }
  }
  const title = dict[document.title.trim()];
  if (title !== undefined) document.title = title;
  for (const el of els) {
    if (el.closest("script,style")) continue;
    if (el.querySelector("input,select,textarea,svg,img")) continue;
    const tag = el.tagName.toLowerCase();
    const leafish = BLOCK_TAGS.has(tag) || ((tag === "span" || tag === "div") && el.children.length === 0);
    if (!leafish) continue;
    const key = (el.textContent ?? "").replace(/\s+/g, " ").trim();
    if (!key) continue;
    const hit = dict[key];
    if (hit !== undefined) el.innerHTML = hit;
  }
}
