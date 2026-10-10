// 发布产物一致性校验（发版流水线与本地都能跑）。
//
// 查的是「该签的都签了、版本号对齐了」——更新链路真跑起来之前，这几件事错了不会让流水线
// 报错，只会在客户端表现成「点了检查更新没反应」或「下完直接被拒装」，属于最难查的一类：
//   - 缺 .sig        → 客户端没得验签，拒装；
//   - key id 对不上  → 签包的私钥与 tauri.conf.json 内嵌公钥不是同一对，一律拒装；
//   - 版本号不一致  → requireSignedVersion 已开，客户端按 MissingSignedVersion /
//                      SignedVersionMismatch 拒装；三处版本号（conf / Cargo.toml / package.json）
//                      手工改漏一处就可能发生。
//
// **不做密码学校验**：minisign 是「预哈希 + 全局签名」两段，自校验要引入原生库，不值当。
// 这里只保证流水线自洽；抗伪造的真校验在客户端 updater 插件里（tauri-plugin-updater 的
// verify_signature：验不过直接拒装）。
//
// 用法：
//   node scripts/verify-release.mjs                                # 查 target/release/bundle
//   node scripts/verify-release.mjs --dir <产物目录>
//   node scripts/verify-release.mjs --latest <latest.json 路径>   # 顺带核对更新清单
//   node scripts/verify-release.mjs --selftest                     # 解析器自检（不需要真产物）

import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

// updater 支持的安装包格式：每一种都应当有同名 .sig（bundle.createUpdaterArtifacts 产出）。
const ARTIFACT_RE = /\.(exe|msi|deb|rpm|appimage)$/i;

const problems = [];
const fail = (msg) => problems.push(msg);

function readText(path) {
  return readFileSync(path, 'utf8');
}

function exists(path) {
  try {
    statSync(path);
    return true;
  } catch {
    return false;
  }
}

function argValue(flag) {
  const i = process.argv.indexOf(flag);
  return i >= 0 ? process.argv[i + 1] : undefined;
}

function walk(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

/** 公钥 → 8 字节 key id（hex）。conf 里的 pubkey 是「整个 .pub 文件」的 base64。 */
function pubkeyId(pubkeyB64) {
  const lines = Buffer.from(pubkeyB64, 'base64').toString('utf8').split(/\r?\n/);
  const body = lines.find((l) => !l.startsWith('untrusted comment:'));
  if (!body) throw new Error('公钥内容里找不到 base64 那一行');
  const bin = Buffer.from(body, 'base64');
  if (bin.length !== 42) throw new Error(`公钥长度不对：${bin.length}（应为 42）`);
  return bin.subarray(2, 10).toString('hex');
}

/**
 * .sig 文件本身是 minisign 签名**文本的 base64**（latest.json 里 `signature` 字段就是它的内容，
 * 客户端直接 base64 解一次再按 minisign 解析）。解开后是四行：
 *   untrusted comment / base64(算法2+keyid8+签名64) / trusted comment: … / base64(全局签名)
 * trusted comment 是 tab 分隔的 key:value，含 `version:`（requireSignedVersion 读的就是它）。
 */
function parseSig(sigFileText) {
  const lines = Buffer.from(sigFileText.trim(), 'base64').toString('utf8').split(/\r?\n/).filter((l) => l.length > 0);
  if (lines.length < 3) throw new Error('签名文件行数不对（不是 minisign 格式？）');
  const bin = Buffer.from(lines[1], 'base64');
  if (bin.length !== 74) throw new Error(`签名块长度不对：${bin.length}（应为 74）`);
  const comment = lines[2];
  if (!comment.startsWith('trusted comment: ')) throw new Error('缺 trusted comment 行');
  const version = comment
    .slice('trusted comment: '.length)
    .split('\t')
    .find((f) => f.startsWith('version:'))
    ?.slice('version:'.length);
  return { keyId: bin.subarray(2, 10).toString('hex'), version };
}

if (process.argv.includes('--selftest')) {
  const id = Buffer.alloc(8, 0xab);
  const pub = Buffer.concat([Buffer.from('ED'), id, Buffer.alloc(32)]);
  const pubText = `untrusted comment: minisign public key ${id.toString('hex')}\n${pub.toString('base64')}`;
  const sig = (comment) =>
    Buffer.from(
      [
        'untrusted comment: signature from minisign secret key',
        Buffer.concat([Buffer.from('ED'), id, Buffer.alloc(64)]).toString('base64'),
        `trusted comment: ${comment}`,
        Buffer.alloc(64).toString('base64'),
      ].join('\n'),
    ).toString('base64');
  const assert = (cond, what) => {
    if (!cond) {
      console.error(`自检失败：${what}`);
      process.exit(1);
    }
  };
  assert(pubkeyId(Buffer.from(pubText).toString('base64')) === id.toString('hex'), '公钥 key id 解析');
  const full = parseSig(sig('timestamp:1700000000\tfile:app.tar.gz\tversion:1.2.3'));
  assert(full.keyId === id.toString('hex') && full.version === '1.2.3', '完整签名解析');
  assert(parseSig(sig('timestamp:1700000000\tfile:app.tar.gz')).version === undefined, '缺 version 字段要能看出来');
  assert(pubkeyId(Buffer.from(pubText).toString('base64')) !== '00'.repeat(8), 'key id 不该恒为 0');
  console.log('解析器自检通过。');
  process.exit(0);
}

const conf = JSON.parse(readText(join(ROOT, 'src-tauri/tauri.conf.json')));
const version = String(conf.version);
const bundleDir = argValue('--dir') ?? join(ROOT, 'target/release/bundle');
const latestPath = argValue('--latest');

let keyId;
try {
  keyId = pubkeyId(conf.plugins.updater.pubkey);
} catch (e) {
  console.error(`读不出 plugins.updater.pubkey 的 key id：${e.message}`);
  process.exit(1);
}

// --- 1. 版本号三处同值（发版第一步就是手工改这三处） --------------------------------

const others = {
  'src-tauri/Cargo.toml': /^version\s*=\s*"([^"]+)"/m.exec(readText(join(ROOT, 'src-tauri/Cargo.toml')))?.[1],
  'ui/package.json': JSON.parse(readText(join(ROOT, 'ui/package.json'))).version,
};
for (const [file, v] of Object.entries(others)) {
  if (v !== version) fail(`版本号不一致：${file} 是 ${v}，tauri.conf.json 是 ${version}`);
}
console.log(`版本 ${version} · 内嵌公钥 key id ${keyId}`);

// --- 2. 每个安装包有同名 .sig，签名 key id 与内嵌公钥一致、版本号一致 ------------------

let artifacts = [];
if (exists(bundleDir)) {
  artifacts = walk(bundleDir);
} else {
  console.log(`注意：${bundleDir} 不存在（没打过包？）——跳过产物检查`);
}

let checked = 0;
for (const artifact of artifacts) {
  const name = basename(artifact);
  if (!ARTIFACT_RE.test(name)) continue;
  const sigPath = `${artifact}.sig`;
  if (!exists(sigPath)) {
    fail(`${name}：没有同名 .sig（客户端没得验签 → 拒装）`);
    continue;
  }
  let sig;
  try {
    sig = parseSig(readText(sigPath));
  } catch (e) {
    fail(`${name}.sig：解析失败 —— ${e.message}`);
    continue;
  }
  let ok = true;
  if (sig.keyId !== keyId) {
    fail(`${name}.sig：签名 key id ${sig.keyId} ≠ 内嵌公钥 ${keyId}（私钥与公钥不是同一对 → 拒装）`);
    ok = false;
  }
  if (sig.version === undefined) {
    fail(`${name}.sig：trusted comment 里没有 version: 字段（requireSignedVersion 已开 → MissingSignedVersion）`);
    ok = false;
  } else if (sig.version.replace(/^v/, '') !== version) {
    fail(`${name}.sig：签名版本 ${sig.version} ≠ 配置版本 ${version}（→ SignedVersionMismatch）`);
    ok = false;
  }
  // 安装包文件名带版本号（Tauri 打包产物如 Kada_0.2.0_x64-setup.exe）；对不上就是打包配置或
  // 版本号抄错。deb/AppImage 的文件名形态不同（kadas_0.2.0_amd64.deb），正则对不上就跳过。
  const named = /_(\d+\.\d+\.\d+)[_-]/.exec(name);
  if (named && named[1] !== version) {
    fail(`${name}：文件名里的版本 ${named[1]} ≠ 配置版本 ${version}`);
    ok = false;
  }
  if (ok) console.log(`  ✓ ${name}`);
  checked += 1;
}
if (checked === 0) console.log('注意：产物目录里没有安装包，签名检查空跑');

// --- 3. 更新清单（tauri-action 生成，别手写）：版本一致 + 每个平台条目有对应的包与签名 ----

if (latestPath) {
  const latest = JSON.parse(readText(latestPath));
  if (latest.version !== version) fail(`latest.json 版本 ${latest.version} ≠ 配置版本 ${version}`);
  const names = new Set(artifacts.map((p) => basename(p)));
  for (const [platform, entry] of Object.entries(latest.platforms ?? {})) {
    const file = decodeURIComponent(entry.url.split('/').pop());
    // 清单是各平台条目并进去的；矩阵里别的 job 产的包不在本 job 的产物目录里，只核对「本 job 认得出的那条」。
    if (!names.has(file)) {
      console.log(`  - ${platform} → ${file}：本 job 没这个产物，跳过`);
      continue;
    }
    const sigPath = artifacts.find((p) => basename(p) === `${file}.sig`);
    if (!sigPath) {
      fail(`latest.json 的 ${platform} 条目指向 ${file}，但没有 ${file}.sig`);
    } else if (!entry.signature) {
      fail(`latest.json 的 ${platform} 条目没有 signature 字段`);
    } else if (entry.signature.trim() !== readText(sigPath).trim()) {
      fail(`latest.json 的 ${platform} 条目里 signature 与 ${file}.sig 内容不一致`);
    }
  }
  console.log(`  ✓ latest.json（${Object.keys(latest.platforms ?? {}).join(' / ') || '无平台条目'}）`);
} else {
  console.log('注意：没给 --latest，跳过更新清单核对');
}

if (problems.length === 0) {
  console.log('发布产物一致性校验通过。');
} else {
  console.error(`发布产物一致性校验失败（${problems.length} 项）：`);
  for (const p of problems) console.error(`  ✗ ${p}`);
}
process.exit(problems.length === 0 ? 0 : 1);