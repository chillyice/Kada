// 静态 HTML 的元素级文案提取（与 `ui/src/i18n.ts` 的 `localizeStaticDom` 同一口径）。
//
// 取「整块文案」：块级标签（p / summary / li / button / option / label / td / th / h1-h3 /
// dt / dd / a）整块算一条；span / div 只有在**纯文本叶子**（没有元素子节点）时才算一条
// ——`<span class="set-main"><span class=set-name>…</span>…</span>` 这种容器要拆开，
// 否则会把标题和说明拼成一条巨长的 key。
//
// 含表单控件 / svg / 图标的元素跳过（整块替换 innerHTML 会把控件弄丢），其子元素照常处理。
// 不引第三方 HTML 解析器：index.html 是我们自己的、格式规整，逐标签扫描足够。
import { readFileSync } from "node:fs";

const ENTITIES = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#39;": "'" };
const VOID = new Set(["input", "br", "img", "meta", "link", "hr", "source", "col"]);
const CONTROLS = new Set(["input", "select", "textarea", "svg", "img"]);
const BLOCK = new Set([
  "p", "summary", "li", "button", "option", "label", "td", "th", "h1", "h2", "h3", "dt", "dd", "a",
]);
const CJK = /[\u4e00-\u9fff]/;

export function decode(s) {
  return s
    .replace(/&#(\d+);/g, (_, d) => String.fromCodePoint(Number(d)))
    .replace(/&#x([0-9a-f]+);/gi, (_, h) => String.fromCodePoint(parseInt(h, 16)))
    .replace(/&(amp|lt|gt|quot);|&#39;/g, (m) => ENTITIES[m] ?? m);
}

export function norm(s) {
  return s.replace(/\s+/g, " ").trim();
}

export function htmlCandidates(file) {
  const src = readFileSync(file, "utf8").replace(/<!--[\s\S]*?-->/g, "");
  const root = { tag: "#root", children: [], text: "", bad: false };
  const stack = [root];
  const re = /<!--[\s\S]*?-->|<\/?[a-zA-Z][^>]*>|[^<]+/g;
  let m;
  while ((m = re.exec(src))) {
    const tok = m[0];
    if (tok.startsWith("<!--")) continue;
    if (tok[0] !== "<") {
      const text = decode(tok);
      for (const el of stack) el.text += text;
      continue;
    }
    const close = tok[1] === "/";
    const name = (close ? tok.slice(2) : tok.slice(1)).match(/^[a-zA-Z0-9:-]+/)?.[0]?.toLowerCase() ?? "";
    if (!name) continue;
    if (close) {
      let i = stack.length - 1;
      while (i > 0 && stack[i].tag !== name) i--;
      if (i <= 0) continue;
      stack.length = i;
      continue;
    }
    const el = { tag: name, children: [], text: "", bad: CONTROLS.has(name) };
    stack[stack.length - 1].children.push(el);
    if (el.bad) for (const parent of stack) parent.bad = true;
    if (!VOID.has(name)) stack.push(el);
  }

  const out = new Set();
  function walk(el, insideCandidate) {
    const text = norm(el.text);
    const leafish = BLOCK.has(el.tag) || ((el.tag === "span" || el.tag === "div") && el.children.length === 0);
    const isCandidate = !insideCandidate && !el.bad && leafish && CJK.test(text);
    if (isCandidate) out.add(text);
    for (const c of el.children) walk(c, insideCandidate || isCandidate);
  }
  walk(root, false);
  return out;
}
