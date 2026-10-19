// 卡带索引生成器（规划 7.2-㉕ 卡带市场）：`snippets/*.json` → `snippets/index.json`。
//
// 卡带市场没有服务端：索引就是一份放在网上、能下载到的静态 JSON，本仓库把它托管在 GitHub
// 上（客户端默认地址见 `kada_core::DEFAULT_SNIPPET_INDEX_URL`）。所以「收录一个卡带」=
// 往 `snippets/` 里丢一个用「导出为卡带」导出的 JSON，再重跑本脚本生成索引。
//
// 用法：
//   node scripts/build-snippet-index.mjs           # 生成 / 更新 snippets/index.json
//   node scripts/build-snippet-index.mjs --check   # 只校验索引与卡带是否已同步（CI 用）
//
// 自建索引的人不需要本脚本：索引格式就三行，照着 `snippets/index.json` 手写也行。

import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const DIR = join(ROOT, 'snippets');
const OUT = join(DIR, 'index.json');
// 卡带地址的基址：默认 GitHub raw；镜像 / 换分支用环境变量改（如 KADA_INDEX_BASE）。
const BASE = (process.env.KADA_INDEX_BASE || 'https://raw.githubusercontent.com/chillyice/Kada/main/snippets')
  .replace(/\/+$/, '');

const problems = [];

const files = readdirSync(DIR)
  .filter((f) => f.endsWith('.json') && f !== 'index.json')
  .sort();

const snippets = [];
for (const file of files) {
  let part;
  try {
    part = JSON.parse(readFileSync(join(DIR, file), 'utf8'));
  } catch (e) {
    problems.push(`${file} 不是合法 JSON：${e.message}`);
    continue;
  }
  // 元信息在卡带顶层的 `kada_snippet`（导出卡带时写进去的那个键，客户端解析时忽略它）。
  const meta = part.kada_snippet || {};
  if (!meta.name) {
    problems.push(`${file} 缺 kada_snippet.name（索引列表要靠它显示名字）`);
    continue;
  }
  const n = ['shortcuts', 'remaps', 'expansions'].reduce(
    (sum, k) => sum + (Array.isArray(part[k]) ? part[k].length : 0),
    0,
  );
  if (n === 0) problems.push(`${file} 里一条条目也没有，收进索引也没人能装出东西`);
  snippets.push({
    name: meta.name,
    url: `${BASE}/${file}`,
    description: meta.description || '',
    author: meta.author || '',
    tags: Array.isArray(meta.tags) ? meta.tags : [],
  });
}

snippets.sort((a, b) => a.name.localeCompare(b.name, 'zh'));
const text = JSON.stringify({ version: 1, snippets }, null, 2) + '\n';

if (process.argv.includes('--check')) {
  let current = '';
  try {
    // 按 LF 比较：Windows 的 core.autocrlf=true 会把检出后的文件变成 CRLF，而本脚本
    // 写出的永远是 LF——直接比字符串会让 Windows CI 误报「索引不同步」。
    current = readFileSync(OUT, 'utf8').replace(/\r\n/g, '\n');
  } catch {
    problems.push('snippets/index.json 不存在，先跑 node scripts/build-snippet-index.mjs');
  }
  if (current && current !== text) {
    problems.push('snippets/index.json 与 snippets/ 不同步，重跑 node scripts/build-snippet-index.mjs');
  }
} else if (problems.length === 0) {
  writeFileSync(OUT, text);
  console.log(`已生成 ${OUT}（${snippets.length} 条）`);
}

if (problems.length) {
  for (const p of problems) console.error(`✗ ${p}`);
  process.exit(1);
}