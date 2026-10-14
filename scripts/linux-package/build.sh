#!/usr/bin/env bash
# 在容器里跑：把挂进来的源码同步到容器本地盘（drvfs 上编译太慢），装依赖 → tauri build
# 出 Linux 安装包 → 产物拷回挂载出去的 /build/out。
#
# BUNDLES 环境变量决定出什么包（默认 deb；`deb,appimage` 两个都要——AppImage 的工具链
# 已在镜像里预置，见 Dockerfile）。EXTRA_ARGS 可以追加 tauri build 的参数（如 --verbose）。
set -euo pipefail

SRC=/build/kada              # 挂载进来的仓库源码
WORK=/work                   # 容器本地盘（源码拷贝 + node_modules）
OUT=/build/out               # 挂载出去的产物目录
TARGET=/target               # **命名卷**：编译产物跨次保留，改一行不用重编 500 个 crate
BUNDLES="${BUNDLES:-deb}"

export CARGO_HOME=/root/.cargo
export CARGO_TARGET_DIR=$TARGET

mkdir -p "$CARGO_HOME" "$OUT"
# crates.io 本机不可达：换 USTC 镜像（index 的 config.json 里 dl 也指向它自己，能真下到 .crate）。
cat > "$CARGO_HOME/config.toml" <<'EOF'
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"

[net]
retry = 5
EOF

echo "== 同步源码到容器本地盘（rsync -a 保留 mtime → 没变的文件 cargo 不重编）=="
mkdir -p "$WORK/kada"
# 为什么不用 cp / tar：仓库根是直接挂进来的，`target/` 有几十 GB（排除掉），而且仓库在
# OneDrive 里、同步进程会边拷边动文件——tar 会以「file changed as we read it」整体失败，
# rsync 只当警告、照常把内容拷过去。
rsync -a --delete --exclude=target --exclude=node_modules \
  --exclude=ui/node_modules --exclude=ui/dist --exclude=.git --exclude='*.log' \
  "$SRC/" "$WORK/kada/"
cd "$WORK/kada"

if [ ! -d node_modules ]; then
  echo "== npm ci（根：Tauri CLI）=="
  npm ci --no-audit --no-fund
fi
if [ ! -d ui/node_modules ]; then
  echo "== npm ci（ui：前端）=="
  npm --prefix ui ci --no-audit --no-fund
fi

echo "== tauri build（--bundles $BUNDLES ${EXTRA_ARGS:-}）=="
# shellcheck disable=SC2086
npm run tauri -- build --bundles "$BUNDLES" ${EXTRA_ARGS:-}

echo "== 产物 =="
for kind in deb appimage rpm; do
  d="$TARGET/release/bundle/$kind"
  [ -d "$d" ] && ls -la "$d"
done
cp -v "$TARGET"/release/bundle/deb/*.deb "$OUT/" 2>/dev/null || true
cp -v "$TARGET"/release/bundle/appimage/*.AppImage "$OUT/" 2>/dev/null || true
cp -v "$TARGET"/release/kada "$OUT/kada-linux-x86_64" 2>/dev/null || true
echo "== 完成 =="
