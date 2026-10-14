# 在 Windows 上产 Linux 安装包用的构建镜像（本机没有 Linux 机器，也不打算为它跑 CI 时才用得上）。
#
# 为什么是 ubuntu:22.04：Tauri 的 .deb 要与发行版 glibc 版本对齐，在越老的发行版上构建，
# 产物能在越多新发行版上装（22.04 = glibc 2.35，也是 release.yml 里跑的同一个版本）。
#
# 镜像源全部换成国内可达镜像（本机网络下 archive.ubuntu.com / static.crates.io /
# static.rust-lang.org / nodejs 官方源的可达性差别很大，换掉最省事）：
#   apt  → 阿里云   node → nodejs.org 官方（实测可达）   rustup → 清华
FROM docker.m.daocloud.io/library/ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive LANG=C.UTF-8

# Tauri Linux 前置依赖：与 .github/workflows/release.yml 里那份保持一致（少一个就会在
# 链接或打包阶段炸）。libfuse2 是 AppImage 工具链要用的，deb 用不到。
#
# **必须 http**：ubuntu 基础镜像里没装 ca-certificates，https 直连镜像会
# 「Certificate verification failed」（第一次就栽在这）；换成 http 秒过，装完
# ca-certificates 之后 rustup / node 那些 https 才有证书链可用。
RUN sed -i 's|http://archive.ubuntu.com/ubuntu|http://mirrors.tuna.tsinghua.edu.cn/ubuntu|g; s|http://security.ubuntu.com/ubuntu|http://mirrors.tuna.tsinghua.edu.cn/ubuntu|g' /etc/apt/sources.list \
 && apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates build-essential pkg-config curl wget file libssl-dev \
      libwebkit2gtk-4.1-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev \
      libfuse2 xz-utils \
      patchelf desktop-file-utils libglib2.0-bin rsync \
 && rm -rf /var/lib/apt/lists/*

# Node 20：前端构建（tsc + vite）要用。官方 tar 包直接铺到 /usr/local。
RUN curl -fsSL https://nodejs.org/dist/v20.20.2/node-v20.20.2-linux-x64.tar.xz -o /tmp/node.tar.xz \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 \
 && rm /tmp/node.tar.xz \
 && node -v && npm -v

# Rust：rustup 走清华镜像（static.rust-lang.org 本机不可达）。镜像只同步 stable/beta/nightly
# 三个频道、不保留带版本号的旧频道，所以钉的是 stable（当前 1.99.0，比本机 1.98.1 新一版）。
ENV RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup \
    RUSTUP_UPDATE_ROOT=https://mirrors.tuna.tsinghua.edu.cn/rustup/rustup
RUN curl -fsSL "$RUSTUP_UPDATE_ROOT/dist/x86_64-unknown-linux-gnu/rustup-init" -o /tmp/rustup-init \
 && chmod +x /tmp/rustup-init \
 && /tmp/rustup-init -y --profile minimal --default-toolchain stable \
 && rm /tmp/rustup-init
ENV PATH="/root/.cargo/bin:$PATH"
RUN cargo --version && rustc --version

# AppImage 打包要的三个工具**tauri 是现从 GitHub 拉的**——本机 GitHub 不通，会以
# `failed to bundle project: io: Connection refused` 收场。用 ghfast.top 代理提前下好、
# 按 tauri 缓存目录里的**文件名**塞进 ~/.cache/tauri（打包时只判文件在不在，不校验哈希）：
#   ~/.cache/tauri/AppRun-x86_64                        ← apprun-old
#   ~/.cache/tauri/linuxdeploy-07333c6-x86_64.AppImage  ← 名字里带 tauri 钉的 commit hash，
#                                                         照抄 tauri-bundler 的
#                                                         `linuxdeploy-{LINUXDEPLOY_COMMIT_HASH}-{arch}.AppImage`；
#                                                         写错名字它照样重下（踩过）
#   ~/.cache/tauri/runtime-x86_64                       ← AppImage 的 type2 runtime。**这个得另外喂**：
#                                                         linuxdeploy 里的 appimagetool 运行时会自己去
#                                                         GitHub 下（`type2-runtime` 仓库），失败就
#                                                         「Failed to download runtime」收场。运行时传
#                                                         `-e LDAI_RUNTIME_FILE=/root/.cache/tauri/runtime-x86_64`
#                                                         把 `LDAI_*` 家族里的这个钩子喂给它（见 build.sh 注释）。
# 两个 gtk/gstreamer 插件脚本是 include_bytes! 编进去的，不用下。
ENV TAURI_CACHE=/root/.cache/tauri
RUN mkdir -p "$TAURI_CACHE" \
 && curl -fsSL "https://ghfast.top/https://github.com/tauri-apps/binary-releases/releases/download/apprun-old/AppRun-x86_64" -o "$TAURI_CACHE/AppRun-x86_64" \
 && curl -fsSL "https://ghfast.top/https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-x86_64.AppImage" -o "$TAURI_CACHE/linuxdeploy-07333c6-x86_64.AppImage" \
 && curl -fsSL "https://ghfast.top/https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64" -o "$TAURI_CACHE/runtime-x86_64" \
 && chmod +x "$TAURI_CACHE"/* \
 && ls -la "$TAURI_CACHE"
