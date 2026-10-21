// 界面文案 key 提取 + 完整性检查（i18n）。
//
// 用法：
//   node scripts/check-i18n.mjs            # 检查 i18n.en.ts 是否覆盖源码里的全部中文文案
//   node scripts/check-i18n.mjs --keys     # 打印去重后的 key 列表（生成字典用）
//
// key 的口径（与 `ui/src/i18n.ts` 的 `tr` / `trt` 一致）：
//   - `tr("…")`：字符串字面量的 cooked 值；
//   - `trt\`…${x}…\``：模板的 cooked 片段用 `{0}` / `{1}` 拼接；
//   - `ui/index.html` 的静态文本节点与 title / placeholder / aria-label / alt / value。
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { htmlCandidates } from "./i18n-html.mjs";

const require = createRequire(import.meta.url);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ts = require(join(root, "ui/node_modules/typescript/lib/typescript.js"));
const CJK = /[\u4e00-\u9fff]/;

function tsKeys() {
  const file = join(root, "ui/src/main.ts");
  const src = readFileSync(file, "utf8");
  const sf = ts.createSourceFile(file, src, ts.ScriptTarget.ES2022, true, ts.ScriptKind.TS);
  const keys = new Set();
  const unwrapped = [];
  function visit(n) {
    if (ts.isCallExpression(n) && ts.isIdentifier(n.expression) && n.expression.text === "tr") {
      const a = n.arguments[0];
      if (a && (ts.isStringLiteral(a) || ts.isNoSubstitutionTemplateLiteral(a))) keys.add(a.text);
      return;
    }
    if (ts.isTaggedTemplateExpression(n) && ts.isIdentifier(n.tag) && n.tag.text === "trt") {
      const t = n.template;
      if (ts.isTemplateExpression(t)) {
        let key = t.head.text;
        t.templateSpans.forEach((s, i) => (key += `{${i}}` + s.literal.text));
        keys.add(key);
        // 插值里的字面量也要求翻译（`…${cond ? tr("上") : tr("下")}…`）。
        for (const s of t.templateSpans) visit(s.expression);
      } else {
        keys.add(t.text);
      }
      return;
    }
    if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n) || ts.isTemplateExpression(n)) {
      const text = ts.isTemplateExpression(n)
        ? n.head.text + n.templateSpans.map((s) => s.literal.text).join("")
        : n.text;
      if (CJK.test(text)) {
        const { line } = sf.getLineAndCharacterOfPosition(n.getStart(sf));
        // 故意保留中文的地方（比如拿后端中文文案做前缀匹配）在行尾写 `i18n:ignore`。
        const lineText = src.slice(sf.getLineStarts()[line], sf.getLineStarts()[line + 1] ?? src.length);
        if (!lineText.includes("i18n:ignore")) {
          unwrapped.push(`${line + 1}: ${text.slice(0, 60)}`);
        }
      }
      return;
    }
    ts.forEachChild(n, visit);
  }
  visit(sf);
  return { keys, unwrapped };
}

function readDict() {
  const file = join(root, "ui/src/i18n.en.json");
  if (!existsSync(file)) return new Map();
  return new Map(Object.entries(JSON.parse(readFileSync(file, "utf8"))));
}

const { keys: tsKeysSet, unwrapped } = tsKeys();
const keys = new Set([...tsKeysSet, ...htmlCandidates(join(root, "ui/index.html"))]);

if (process.argv.includes("--keys")) {
  const sorted = [...keys].sort();
  console.log(JSON.stringify(sorted, null, 1));
  console.error(`${sorted.length} keys`);
  if (unwrapped.length) {
    console.log(`! ${unwrapped.length} 处中文没包 tr()/trt：`);
    for (const u of unwrapped) console.log("  - " + u);
  }
} else {
  const dict = readDict();
  const missing = [...keys].filter((k) => !dict.has(k)).sort();
  let bad = false;
  if (missing.length) {
    bad = true;
    console.error(`✗ i18n: ${missing.length} 条文案没有英文（共 ${keys.size} 条）`);
    for (const k of missing.slice(0, 40)) console.error("  - " + JSON.stringify(k));
  } else {
    console.log(`✓ i18n: ${keys.size} 条文案全部有英文`);
  }
  if (unwrapped.length) {
    bad = true;
    console.error(`✗ i18n: ${unwrapped.length} 处中文没包 tr()/trt：`);
    for (const u of unwrapped.slice(0, 40)) console.error("  - " + u);
  }
  if (bad) process.exitCode = 1;
}
