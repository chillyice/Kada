#!/usr/bin/env bash
# Kada 更新镜像：GitHub Releases + 卡带市场 → ihomy（由 kada-mirror.timer 每天触发，也可手动跑）。
#
# 只镜像「已发布」的 Release：GitHub 的 /releases/latest 天然排除 draft / prerelease，
# 与客户端选 GitHub 源时的口径一致——草稿不会被镜像出去。
#
# 清单里的 URL 改写成 ihomy 的地址，`signature` 原样保留：minisign 签的是安装包本身，
# 客户端仍用内嵌公钥校验，镜像不参与信任链（改不了包，也伪造不了签名）。
#
# 顺序：先落安装包与 .sig，最后原子替换 latest.json——客户端任何时刻拿到的清单，指向的
# 文件都已经就位。卡带索引同理：先落卡带文件、最后替换 index.json。
#
# 环境变量（一般不用改）：
#   KADA_MIRROR_DEST         落盘目录（默认 /opt/ihomy/uploads/kada/updates）
#   KADA_MIRROR_PUBLIC_BASE  对外地址前缀（默认 https://ihomy.top/files/kada/updates）
#   KADA_MIRROR_SNIPPET_DEST 卡带落盘目录（默认 /opt/ihomy/uploads/kada/snippets）
#   KADA_MIRROR_SNIPPET_PUBLIC_BASE 卡带对外地址前缀（默认 https://ihomy.top/files/kada/snippets）
#   KADA_MIRROR_RAW_BASE     卡带源目录（默认 https://raw.githubusercontent.com/<repo>/main/snippets）
#   KADA_MIRROR_REPO         GitHub 仓库（默认 chillyice/Kada）
#   KADA_MIRROR_API          测试用：替换 GitHub API 地址
#   KADA_MIRROR_GH_BASE      可选下载代理前缀（如 https://ghfast.top/）。服务器直连
#                            github.com/releases/download 会挂住 / 只有几十 KB/s 时用
#   KADA_MIRROR_FORCE=1      忽略「已同步」状态，强制重拉
set -euo pipefail

REPO="${KADA_MIRROR_REPO:-chillyice/Kada}"
DEST="${KADA_MIRROR_DEST:-/opt/ihomy/uploads/kada/updates}"
PUBLIC_BASE="${KADA_MIRROR_PUBLIC_BASE:-https://ihomy.top/files/kada/updates}"
SNIPPET_DEST="${KADA_MIRROR_SNIPPET_DEST:-/opt/ihomy/uploads/kada/snippets}"
SNIPPET_PUBLIC_BASE="${KADA_MIRROR_SNIPPET_PUBLIC_BASE:-https://ihomy.top/files/kada/snippets}"
RAW_BASE="${KADA_MIRROR_RAW_BASE:-https://raw.githubusercontent.com/${REPO}/main/snippets}"
FORCE="${KADA_MIRROR_FORCE:-0}"
GH_BASE="${KADA_MIRROR_GH_BASE:-}"
API="${KADA_MIRROR_API:-https://api.github.com/repos/${REPO}/releases/latest}"
STATE="$DEST/.synced-tag"

log() { printf '[kada-mirror] %s\n' "$*"; }
die() { printf '[kada-mirror] 错误：%s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null || die "缺少 curl"
# Ubuntu 上叫 python3；本机 Git Bash 只有 python——两者都接受。
PY="$(command -v python3 || command -v python || true)"
[ -n "$PY" ] || die "缺少 python3（用来改写 latest.json）"

curl_json() {
  curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 15 \
    -H 'Accept: application/vnd.github+json' \
    -H 'User-Agent: kada-mirror' "$1"
}

# 卡带同步用的下载器：第一个参数是源 URL，可走 GH_BASE 代理前缀（其余参数原样交给 curl）。
dl() {
  local url="$1"
  shift
  curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 15 \
    -H 'User-Agent: kada-mirror' "$GH_BASE$url" "$@"
}

# 卡带市场（snippets/）走同一个镜像：客户端默认索引地址永远是 ihomy，所以索引与卡带文件
# 必须在这里就位（否则市场一打开就是 404，用户只会觉得「卡带市场是坏的」）。
# 索引里的条目地址改写成 ihomy——客户端只认条目里的 url，去 GitHub 拉在国内会直接卡住。
# 每天全量重拉一遍：源文件只有几 KB，比维护增量状态省心。
sync_snippets() {
  local tmp_s
  tmp_s="$(mktemp -d)"
  trap 'rm -rf "$tmp_s"' RETURN
  log "同步卡带索引：$RAW_BASE/index.json"
  dl "$RAW_BASE/index.json" -o "$tmp_s/index-src.json" || die "下载卡带索引失败"

  # 只接受 v1 索引：镜像不认识新格式就保持旧文件不动，别按老字段猜着改写。
  "$PY" - "$tmp_s/index-src.json" "$tmp_s/index.json" "$tmp_s/files.txt" "$SNIPPET_PUBLIC_BASE" <<'PY' || die "卡带索引格式不对"
import json, sys
src, out, files, base = sys.argv[1:5]
idx = json.load(open(src, encoding="utf-8"))
if int(idx.get("version", 1)) != 1:
    raise SystemExit("卡带索引版本过新（镜像只会改写 v1）")
names = []
entries = []
for e in idx.get("snippets", []):
    name = (e.get("name") or "").strip()
    url = (e.get("url") or "").strip()
    if not name or not url:
        continue
    f = url.rstrip("/").split("/")[-1]
    if not f.endswith(".json") or f.startswith(".") or "/" in f or "\\" in f:
        raise SystemExit("非法卡带文件名：" + repr(f))
    if f in names:
        raise SystemExit("卡带文件名重复：" + f)
    names.append(f)
    entries.append({**e, "url": base.rstrip("/") + "/" + f})
if not entries:
    raise SystemExit("索引里没有可用条目")
with open(files, "w", encoding="utf-8", newline="\n") as fh:
    for f in names:
        fh.write(f + "\n")
with open(out, "w", encoding="utf-8", newline="\n") as fh:
    json.dump({"version": 1, "snippets": entries}, fh, ensure_ascii=False, indent=2)
    fh.write("\n")
PY

  mkdir -p "$tmp_s/files"
  local f
  while IFS= read -r f; do
    dl "$RAW_BASE/$f" -o "$tmp_s/files/$f" || die "下载卡带 $f 失败"
  done < "$tmp_s/files.txt"

  # 每份都要是「能装出东西」的卡带：JSON 合法、带 kada_snippet.name、至少一条条目。
  "$PY" - "$tmp_s/files" "$tmp_s/files.txt" <<'PY' || die "卡带内容校验失败"
import json, os, sys
d, files = sys.argv[1:3]
for f in open(files, encoding="utf-8").read().split():
    part = json.load(open(os.path.join(d, f), encoding="utf-8"))
    if not (part.get("kada_snippet") or {}).get("name"):
        raise SystemExit(f + " 缺 kada_snippet.name")
    n = sum(len(part.get(k) or []) for k in
            ("shortcuts", "remaps", "expansions", "gestures", "hot_corners", "wheel_edges"))
    if n == 0:
        raise SystemExit(f + " 一条条目也没有")
PY

  # 落盘：先卡带文件、最后替换 index.json（客户端任何时刻拿到的索引，指向的文件都已就位）。
  # 从索引里撤下的旧文件不删——删了反而让缓存里的旧索引指向 404。
  mkdir -p "$SNIPPET_DEST"
  while IFS= read -r f; do
    install -m 644 "$tmp_s/files/$f" "$SNIPPET_DEST/$f" || die "写入卡带 $f 失败"
  done < "$tmp_s/files.txt"
  install -m 644 "$tmp_s/index.json" "$SNIPPET_DEST/.index.json.$$" || die "写入卡带索引失败"
  mv -f "$SNIPPET_DEST/.index.json.$$" "$SNIPPET_DEST/index.json"
  log "卡带同步完成：$(wc -l < "$tmp_s/files.txt") 份 → $SNIPPET_DEST（对外 $SNIPPET_PUBLIC_BASE/index.json）"
}

# 卡带与更新清单互不牵连：卡带失败只记一行，别把当天的更新同步一起带崩。
if ! ( sync_snippets ); then
  log "卡带同步失败（更新清单照常继续）"
fi

release="$(curl_json "$API")" || die "拉取 GitHub Release 失败（网络不通或被限流）"
tag="$(printf '%s' "$release" | "$PY" -c 'import json,sys; print(json.load(sys.stdin).get("tag_name",""))')"
[ -n "$tag" ] || die "Release 里没有 tag_name"

if [ "$FORCE" != "1" ] && [ -f "$STATE" ] && [ -f "$DEST/latest.json" ] && [ "$(cat "$STATE")" = "$tag" ]; then
  log "已是最新（$tag），跳过"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. Release 资产表：name<TAB>url。
printf '%s' "$release" > "$tmp/release.json"
"$PY" - "$tmp/release.json" "$tmp/assets.tsv" "$GH_BASE" <<'PY'
import json, sys
rel = json.load(open(sys.argv[1], encoding="utf-8"))
base = sys.argv[3]
with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as f:
    for a in rel.get("assets", []):
        name, url = a["name"], a.get("browser_download_url") or ""
        if not url:
            continue
        if not name or name in (".", "..") or "/" in name or "\\" in name:
            raise SystemExit("非法资产名：" + repr(name))
        f.write(name + "\t" + base + url + "\n")
PY

# 2. GitHub 生成的更新清单（tauri-action 产出，别手写）。
latest_url="$(awk -F'\t' '$1 == "latest.json" { print $2 }' "$tmp/assets.tsv")"
[ -n "$latest_url" ] || die "Release 里没有 latest.json"
curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 15 \
  -H 'User-Agent: kada-mirror' "$latest_url" > "$tmp/github-latest.json" || die "下载 latest.json 失败"

# 3. 清单引用的安装包与 .sig 都必须在资产里，否则不出半套。
"$PY" - "$tmp/assets.tsv" "$tmp/github-latest.json" "$tmp/files.txt" <<'PY'
import json, sys

assets = {}
with open(sys.argv[1], encoding="utf-8") as f:
    for line in f:
        name, url = line.rstrip("\r\n").split("\t", 1)
        assets[name] = url

manifest = json.load(open(sys.argv[2], encoding="utf-8"))
platforms = manifest.get("platforms", {})
if not platforms:
    raise SystemExit("latest.json 里没有 platforms 条目")

need = set()
for platform, entry in platforms.items():
    if not entry.get("signature"):
        raise SystemExit(f"{platform} 条目没有 signature")
    name = entry.get("url", "").split("/")[-1]
    if not name:
        raise SystemExit(f"{platform} 条目没有 url")
    need.add(name)

missing = sorted(n for n in need if n not in assets)
if missing:
    raise SystemExit("清单引用的资产不存在：" + ", ".join(missing))
missing_sig = sorted(n for n in need if n + ".sig" not in assets)
if missing_sig:
    raise SystemExit("缺少签名文件：" + ", ".join(missing_sig))

with open(sys.argv[3], "w", encoding="utf-8", newline="\n") as f:
    for name in sorted(need):
        f.write(name + "\n" + name + ".sig\n")
PY

# 4. 下载（curl 逐项失败即停；文件名已在上一步限定为单段路径）。
while IFS= read -r name; do
  url="$(awk -F'\t' -v n="$name" '$1 == n { print $2 }' "$tmp/assets.tsv")"
  [ -n "$url" ] || die "资产表里找不到 $name"
  log "下载 $name"
  curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 15 \
    -H 'User-Agent: kada-mirror' -o "$tmp/$name" "$url" || die "下载 $name 失败"
done < "$tmp/files.txt"

# 5. 把清单里的下载地址改到 ihomy；签名与版本原样保留。
"$PY" - "$tmp/github-latest.json" "$PUBLIC_BASE" "$tmp/latest.json" <<'PY'
import json, sys
manifest = json.load(open(sys.argv[1], encoding="utf-8"))
base = sys.argv[2].rstrip("/")
for entry in manifest.get("platforms", {}).values():
    entry["url"] = base + "/" + entry["url"].split("/")[-1]
with open(sys.argv[3], "w", encoding="utf-8", newline="\n") as f:
    json.dump(manifest, f, ensure_ascii=False, indent=2)
    f.write("\n")
PY

# 6. 落盘：安装包 → 最后 latest.json（同目录内 rename，原子）。
mkdir -p "$DEST"
while IFS= read -r name; do
  install -m 644 "$tmp/$name" "$DEST/$name"
done < "$tmp/files.txt"
install -m 644 "$tmp/latest.json" "$DEST/.latest.json.$$"
mv -f "$DEST/.latest.json.$$" "$DEST/latest.json"
printf '%s\n' "$tag" > "$DEST/.synced-tag.$$"
mv -f "$DEST/.synced-tag.$$" "$DEST/.synced-tag"

log "完成：$tag → $DEST（对外 $PUBLIC_BASE/latest.json）"
