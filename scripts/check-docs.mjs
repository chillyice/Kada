// 文档一致性检查（CI 用）：
//   1) 仓库内路径引用是否指向真实文件（只查「现状类」文档，归档是历史记录不查）
//   2) § 小节引用：限定引用（带文档名）必须存在；裸 § 引用必须能唯一定位
//   3) docs/代码地图.md 路由表：文件存在 + grep 令牌在源码里逐字存在
//   4) AGENTS.md 体积（注入上下文会被截断，尾部先丢）
//   5) docs/README.md 索引是否收录全部文档
// 用法: node scripts/check-docs.mjs   （退出码非 0 = 有问题）
import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const docsDir = join(root, "docs");
const rel = (p) => p.replace(root + "\\", "").replace(root + "/", "");

const docFiles = readdirSync(docsDir, { withFileTypes: true }).flatMap((d) =>
  d.isDirectory()
    ? readdirSync(join(docsDir, d.name))
        .filter((f) => f.endsWith(".md"))
        .map((f) => join(docsDir, d.name, f))
    : d.name.endsWith(".md")
      ? [join(docsDir, d.name)]
      : []
);
// 归档 = 历史记录，里面提到的文件可能已被删除，不查路径存在性
const isArchive = (f) => /变更归档\.md$|归档[\\/]/.test(f);
const mdFiles = [join(root, "AGENTS.md"), join(root, "README.md"), ...docFiles];

const problems = [];
const read = (f) => readFileSync(f, "utf8");

// ---- 1. 仓库内路径引用 ----
const REPO_PATH = /^(docs|scripts|src-tauri|crates|ui|\.github|kada-prime)\//;
const GLOB = /[*?{]/;
for (const f of mdFiles) {
  if (isArchive(f)) continue;
  for (const m of read(f).matchAll(/`([^`\n]*?)`/g)) {
    const ref = m[1].trim();
    if (!REPO_PATH.test(ref) || GLOB.test(ref) || ref.includes(" ")) continue;
    const p = ref.replace(/:\d+$/, ""); // file.rs:123 → file.rs
    if (!existsSync(join(root, p))) problems.push(`[path] ${rel(f)} -> ${ref}`);
  }
}

// ---- 2. § 小节引用 ----
// 规范命名空间：说明书（需求设计说明书.md）、架构设计、人工验证清单
const canonical = [
  { file: join(docsDir, "需求设计说明书.md"), name: "说明书", alias: ["需求设计说明书", "说明书"] },
  { file: join(docsDir, "架构设计.md"), name: "架构设计", alias: ["架构设计"] },
  { file: join(docsDir, "人工验证清单.md"), name: "人工验证清单", alias: ["人工验证清单", "验证清单"] },
];
const secIndex = new Map(); // "3.6" -> ["说明书", "架构设计"]
const secOf = new Map(); // "说明书" -> Set("3.6")
for (const { file, name } of canonical) {
  if (!existsSync(file)) continue;
  const set = new Set();
  for (const m of read(file).matchAll(/^#{2,4}\s+(\d+(?:\.\d+)*)/gm)) {
    set.add(m[1]);
    if (!secIndex.has(m[1])) secIndex.set(m[1], []);
    if (!secIndex.get(m[1]).includes(name)) secIndex.get(m[1]).push(name);
  }
  secOf.set(name, set);
}
// 入口文档：AI 每次会话都从这里进，§ 引用必须带文档名（否则同名小节指错）
const ENTRY = new Set([
  "AGENTS.md",
  "docs\\README.md",
  "docs/代码地图.md",
]);
for (const f of mdFiles) {
  if (isArchive(f)) continue; // 归档是历史记录，里面的 § 指向当时的编号
  const txt = read(f);
  for (const m of txt.matchAll(/§\s?(\d+(?:\.\d+)*)(?![\d.\-])/g)) {
    const num = m[1];
    // 往前找最近的文档名 → 限定引用（"说明书 §5.7 / 架构设计 §3.17" 里 §3.17 归架构设计）
    // 限定词必须与 § 在**同一句/同一表格单元**里：窗口不跨 `|` 与换行
    let before = txt.slice(Math.max(0, m.index - 40), m.index);
    const cut = Math.max(before.lastIndexOf("|"), before.lastIndexOf("\n"));
    if (cut >= 0) before = before.slice(cut + 1);
    let owner = null;
    let best = -1;
    for (const c of canonical) {
      for (const key of c.alias) {
        const at = before.lastIndexOf(key);
        if (at > best) {
          best = at;
          owner = c;
        }
      }
    }
    if (owner) {
      if (!secOf.get(owner.name)?.has(num))
        problems.push(`[sec-none] ${rel(f)} -> ${owner.name} §${num} 不存在`);
    } else {
      const hits = secIndex.get(num);
      if (!hits) problems.push(`[sec-none] ${rel(f)} -> §${num} 在任何规范文档里都不存在`);
      else if (hits.length > 1 && ENTRY.has(rel(f)))
        problems.push(`[sec-ambig] ${rel(f)} -> §${num} 同时命中 ${hits.join(" / ")}（要写文档名）`);
    }
  }
}

// ---- 3. 代码地图路由表：文件存在 + grep 令牌逐字存在 ----
const mapFile = join(docsDir, "代码地图.md");
if (existsSync(mapFile)) {
  const txt = read(mapFile);
  const block = txt.split("<!-- route-table -->")[1]?.split("<!-- /route-table -->")[0] ?? "";
  let rows = 0;
  for (const line of block.split(/\r?\n/)) {
    if (!line.trim().startsWith("|")) continue;
    const cells = line.split("|").map((c) => c.trim());
    if (cells[1]?.startsWith("症状") || cells[1]?.startsWith("---")) continue;
    const file = [...(cells[2] ?? "").matchAll(/`([^`]+)`/g)].map((m) => m[1]);
    const grep = [...(cells[3] ?? "").matchAll(/`([^`]+)`/g)].map((m) => m[1]);
    if (file.length !== 1 || grep.length === 0) {
      problems.push(`[map-row] 代码地图一行格式不对（文件列要 1 个路径、令牌列至少 1 个）: ${line.slice(0, 60)}`);
      continue;
    }
    rows++;
    const abs = join(root, file[0]);
    if (!existsSync(abs)) {
      problems.push(`[map-file] 代码地图 -> ${file[0]} 不存在`);
      continue;
    }
    const src = read(abs);
    for (const g of grep) {
      if (!src.includes(g)) problems.push(`[map-token] ${file[0]} 里找不到 \`${g}\``);
    }
  }
  if (rows < 10) problems.push(`[map-rows] 代码地图只解析到 ${rows} 行路由（应 ≥10）`);
} else {
  problems.push("[map] docs/代码地图.md 不存在");
}

// ---- 4. AGENTS.md 体积（注入上下文会被截断，尾部先丢）----
// 按 CRLF 计：CI 的 Windows checkout 会转 CRLF，用工作区 LF 尺寸量会漏判。
const A_LIMIT = 20 * 1024;
const aText = read(join(root, "AGENTS.md"));
const aSize = Buffer.byteLength(aText.replace(/\r?\n/g, "\r\n"), "utf8");
if (aSize > A_LIMIT) problems.push(`[size] AGENTS.md ${aSize} B（CRLF）> ${A_LIMIT} B`);

// ---- 5. docs/README.md 索引覆盖 ----
const idx = read(join(docsDir, "README.md"));
for (const f of docFiles) {
  const name = f.split("\\").pop().split("/").pop();
  if (name === "README.md" || isArchive(f)) continue;
  if (!idx.includes(name)) problems.push(`[index] docs/README.md 未收录 ${name}`);
}

console.log(problems.length ? problems.join("\n") : "OK: 文档无问题");
console.log(`\n--- 统计 ---`);
console.log(`AGENTS.md: ${aSize} B (上限 ${A_LIMIT} B, 余 ${A_LIMIT - aSize} B)`);
for (const f of mdFiles) {
  const txt = read(f);
  console.log(
    `${String(statSync(f).size).padStart(7)} B  ${String(txt.split(/\r?\n/).length).padStart(5)} 行  ` +
      `${String((txt.match(/^#{1,6}\s/gm) || []).length).padStart(4)} 标题  ${rel(f)}`
  );
}
process.exit(problems.length ? 1 : 0);
