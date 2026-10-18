#!/usr/bin/env bash
# Kada 更新镜像：GitHub Releases → ihomy（由 kada-mirror.timer 每天触发，也可手动跑）。
#
# 只镜像「已发布」的 Release：GitHub 的 /releases/latest 天然排除 draft / prerelease，
# 与客户端选 GitHub 源时的口径一致——草稿不会被镜像出去。
#
# 清单里的 URL 改写成 ihomy 的地址，`signature` 原样保留：minisign 签的是安装包本身，
# 客户端仍用内嵌公钥校验，镜像不参与信任链（改不了包，也伪造不了签名）。
#
# 顺序：先落安装包与 .sig，最后原子替换 latest.json——客户端任何时刻拿到的清单，指向的
# 文件都已经就位。
#
# 环境变量（一般不用改）：
#   KADA_MIRROR_DEST         落盘目录（默认 /opt/ihomy/uploads/kada/updates）
#   KADA_MIRROR_PUBLIC_BASE  对外地址前缀（默认 https://ihomy.top/files/kada/updates）
#   KADA_MIRROR_REPO         GitHub 仓库（默认 chillyice/Kada）
#   KADA_MIRROR_API          测试用：替换 GitHub API 地址
#   KADA_MIRROR_FORCE=1      忽略「已同步」状态，强制重拉
set -euo pipefail

REPO="${KADA_MIRROR_REPO:-chillyice/Kada}"
DEST="${KADA_MIRROR_DEST:-/opt/ihomy/uploads/kada/updates}"
PUBLIC_BASE="${KADA_MIRROR_PUBLIC_BASE:-https://ihomy.top/files/kada/updates}"
FORCE="${KADA_MIRROR_FORCE:-0}"
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
"$PY" - "$tmp/release.json" "$tmp/assets.tsv" <<'PY'
import json, sys
rel = json.load(open(sys.argv[1], encoding="utf-8"))
with open(sys.argv[2], "w", encoding="utf-8", newline="\n") as f:
    for a in rel.get("assets", []):
        name, url = a["name"], a["browser_download_url"]
        if not name or name in (".", "..") or "/" in name or "\\" in name:
            raise SystemExit("非法资产名：" + repr(name))
        f.write(name + "\t" + url + "\n")
PY

# 2. GitHub 生成的更新清单（tauri-action 产出，别手写）。
latest_url="$(awk -F'\t' '$1 == "latest.json" { print $2 }' "$tmp/assets.tsv")"
[ -n "$latest_url" ] || die "Release 里没有 latest.json"
curl_json "$latest_url" > "$tmp/github-latest.json" || die "下载 latest.json 失败"

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
