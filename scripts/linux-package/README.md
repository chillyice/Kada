# Linux 打包（本机是 Windows，靠 docker 容器出包）

Windows 上打不出 Linux 包（Tauri 的 deb / AppImage 要把壳链到 gtk / webkit，交叉编译不现实），
而 github.com 这台机器又连不上（CI 出的包也拿不下来）。所以走**容器构建**：在 ubuntu 22.04
容器里装齐 Tauri 前置依赖 + Rust + Node，编译并打包，产物从挂载目录取出来。

> 22.04 不是随手选的：`.deb` 的 glibc 下限跟着构建环境走，在越老的发行版上构建，越新的发行版也能装
> （这是 `.github/workflows/release.yml` 里那个 ubuntu-22.04 job 的同一个理由）。

## 用法

```bash
# 一次性：建镜像（apt + Node + Rust + AppImage 工具，约 5~8 分钟）
cd scripts/linux-package
docker build -t kada-linux:1 -f Dockerfile .

# 每次出包：先退出运行中的 Kada 不需要（这是 Linux 包），直接从仓库根跑
docker run --rm \
  -v "$PWD/../..:/build/kada" \
  -v "$PWD/out:/build/out" \
  -v "$PWD/build.sh:/build/build.sh" \
  -v "$USERPROFILE/.cargo/registry:/root/.cargo/registry" \
  -v kada-linux-target:/target \
  -v kada-linux-work:/work \
  -e BUNDLES=deb,appimage \
  -e LDAI_RUNTIME_FILE=/root/.cache/tauri/runtime-x86_64 \
  -e TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/kada.key)" \
  -e TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
  kada-linux:1 bash /build/build.sh
```

（Git Bash 里跑 docker 记得 `MSYS_NO_PATHCONV=1`，否则 `/build/xxx` 这种参数会被 MSYS 改写成 Windows 路径。）

产物落在 `scripts/linux-package/out/`：`Kada_<版本>_amd64.deb`、`Kada_<版本>_amd64.AppImage`，
外加一个裸 `kada-linux-x86_64`（排查用）。

- `kada-linux-target` / `kada-linux-work` 两个命名卷是**编译缓存**：留着，改一行重跑就只重编受影响的部分；
  不留的话每次全量重编（release profile 有 lto + codegen-units=1，全量约 20~30 分钟）。
- `BUNDLES` 只想要 deb 就填 `deb`（快）；AppImage 会额外跑 linuxdeploy，慢一些且依赖下面那套缓存工具。

## 为什么镜像里塞了那些「怪东西」

本机网络对 GitHub / crates.io / static.rust-lang.org 都不通，而构建链上有几处**会临时上网拉东西**，
全部提前换成可达镜像或预置缓存（细节都在 `Dockerfile` 注释里）：

| 要拉的东西 | 本机可达性 | 换成了 |
|------------|------------|--------|
| apt（Tauri 前置依赖） | archive.ubuntu.com 通但慢 | 清华 http 镜像（**必须 http**：基础镜像没有 ca-certificates，https 直接证书失败） |
| crates.io | 不通 | USTC sparse 镜像（`build.sh` 里写 `~/.cargo/config.toml`） |
| rustup | static.rust-lang.org 不通 | 清华 rustup 镜像（只同步 stable，钉不了具体版本） |
| Node 20 | 官方源通 | nodejs.org 官方 tar 包 |
| AppImage 工具链（AppRun / linuxdeploy / type2 runtime） | GitHub 不通 | `ghfast.top` 代理预下进 `~/.cache/tauri`，文件名必须与 tauri 期望的完全一致 |

AppImage 那条最容易踩：`linuxdeploy` 的文件名里带 tauri 钉的 commit hash
（`linuxdeploy-07333c6-x86_64.AppImage`），写错就静默重新下载然后失败；runtime 还要靠
`LDAI_RUNTIME_FILE` 环境变量喂给 linuxdeploy 里的 appimagetool（否则它自己去 GitHub 下）。

## 与 CI 的分工

`.github/workflows/release.yml` 里那个 ubuntu job 出的是一样的 deb / AppImage（GitHub 的 runner
不受这里的网络限制）。容器这条路是给**本机直接出包上 ihomy 发布页**用的——CI 的产物在 GitHub
Releases 上，本机下载也要过 GitHub。
