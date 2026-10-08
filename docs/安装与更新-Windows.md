# Kada Windows 版安装与软件更新

> 本文档覆盖 Windows 版的**安装步骤**与**软件更新**（客户端 + 发布流水线均已落地）。
> 状态标注：✅ 已落地；🔧 设计已定、待实现。
> 配套：功能需求见 `docs/需求设计说明书.md`；架构与跨平台策略见 `docs/架构设计.md`。

## 一、安装（✅ 当前可用）

### 1.1 构建安装包（开发者）

在仓库根目录执行（`beforeBuildCommand` 会自动先跑 `npm --prefix ui run build`）：

```powershell
npm run tauri build
# 或 npx tauri build / cargo tauri build
```

产物输出到**仓库根**的 `target/release/bundle/`（本仓库是 Cargo workspace，`target/` 落在 workspace 根，不在 `src-tauri/` 下），Windows 下含：

| 格式 | 路径 | 说明 |
|------|------|------|
| NSIS 安装包 | `nsis/Kada_<版本>_x64-setup.exe` | 向导式安装，**推荐分发用**；也是自动更新实际下载的包 |
| WiX MSI | `msi/Kada_<版本>_x64_<语言>.msi` | 企业/组策略静默部署用 |
| MSIX | `msix/Kada_<版本>_x64.msix` | Microsoft Store / 旁加载 |
| 更新签名 | 各安装包同目录的 `.sig` | 自动更新校验用（`bundle.createUpdaterArtifacts = true` 产出），随 Release 一起上传 |

当前 `tauri.conf.json` 的 `bundle.targets = "all"`，用 Tauri 默认打包参数（未做自定义安装配置）。

> **打包前必读两条**（都踩过）：
> 1. **先退出正在运行的 Kada**。exe 被占用时构建会在最后一步报「拒绝访问」，而且 bundler 给二进制写入「安装包类型」标记这一步会失败并警告 `Updater plugin may not be able to update this package`——这个标记决定已安装的客户端认不认 NSIS 更新包，**打包时它写不进去就可能导致更新装不上**。（`scripts/kada-package.ps1` 已自动先杀再构建。）
> 2. **首次打包要联网下载 NSIS 工具链**（`github.com/tauri-apps/binary-releases` 上的 `nsis-3.11.zip`）。本机实测该 GitHub Release 资源拉不通（`timeout: global`），本地产不出安装包；CI runner（GitHub 托管机器）网络正常，发版走流水线即可。

### 1.2 用户安装步骤

1. 下载 `Kada_<版本>_x64-setup.exe` 安装包。
2. 双击运行，按向导完成安装。
3. 安装完成后启动「咔哒 Kada」，程序以**托盘图标**常驻后台；主窗口可关闭（关闭 = 隐藏到托盘，不退出）。
4. 首次使用在界面里配置快捷键 / 改键 / 宏，保存后即时生效（无需重启）。

> 权限说明：全局快捷键/改键用 Windows 低层钩子（`WH_KEYBOARD_LL`），**无需管理员权限**；但低层钩子拦不住 UAC 提权进程与部分游戏（见 `docs/架构设计.md` §5）。
>
> 安装目录与提权：Tauri v2 的 NSIS 默认 `installMode = currentUser`（装到免管理员权限的目录，卸载信息写 `HKCU`），所以**默认安装过程不弹 UAC**。要让程序装进 `Program Files`、开机自启对所有用户生效，得在 `tauri.conf.json` 配 `bundle.windows.nsis.installMode = "perMachine"`（那样安装与更新都需要管理员）。本项目保持默认。

### 1.3 首次运行与数据位置

- 配置（快捷键/改键/宏）以 JSON 落盘在 Tauri `app_data_dir()` 下的 `config.json`，Windows 上约 `%APPDATA%\com.kada\config.json`。
- 配置是纯 JSON 文本，可直接跨平台同步/备份（键模型与配置结构见 `docs/需求设计说明书.md` §2/§3）。
- **更新不影响配置**：升级只替换程序文件，`%APPDATA%\com.kada\` 不动。

### 1.4 卸载

- 图形界面：Windows 设置 → 应用 → 已安装的应用 → 咔哒 Kada → 卸载。
- 或运行安装目录下的卸载程序（NSIS 生成的 `uninstall.exe`）。
- 卸载**不清除** `%APPDATA%\com.kada\` 下的配置文件；如需彻底清除请手动删除该目录。

## 二、软件更新（✅ 客户端与流水线已落地）

> 选型：**Tauri 官方 `tauri-plugin-updater` + GitHub Releases 作更新源 + Ed25519 签名**。
> 代码在 `src-tauri/src/update.rs`（状态机与流程）、`plugins.updater`（`src-tauri/tauri.conf.json`）、
> 发布流水线在 `.github/workflows/release.yml`。
> ⚠ 端到端升级需要**先配好仓库 Secrets 并发一次 Release**才能真跑通（见 §2.7）；本机只能验到「打包出签名」这一步。

### 2.1 更新源

- 端点（内嵌在 `tauri.conf.json` 的 `plugins.updater.endpoints`）：
  `https://github.com/chillyice/Kada/releases/latest/download/latest.json`
- 用 `releases/latest` 的含意：只解析**已发布**的最新 Release——draft 不算。所以流水线默认发 draft，由人确认后再 Publish，等于给发版留了一道闸。
- 单 `stable` 渠道起步；将来要灰度再拆 `beta`/`stable` 各一份 `latest.json`。

### 2.2 latest.json 由流水线生成，不要手写

`tauri-apps/tauri-action` 在发版时汇总各平台的 `.sig` 自动生成（含 `version` / `notes` / `pub_date` / `platforms`），Windows 条目指向该 Release 里的 NSIS 安装包，`signature` 是对应 `.sig` 文件的内容。手工维护这份清单只会跟产物对不上，**不要往仓库里放一份 latest.json**。

### 2.3 签名与安全

- 密钥对已生成（Ed25519）：**公钥**已内嵌进 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`（随仓库）；**私钥**在本机 `C:\Users\chill\.tauri\kada.key`（**不在仓库内，绝不入库**）。
- CI 用两个 GitHub Secrets（Settings → Secrets and variables → Actions）：
  - `TAURI_SIGNING_PRIVATE_KEY` = `kada.key` 的**文件全文**（本机这份是单行 348 字符、无换行）
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = 私钥密码；本项目生成时未设密码，该 secret 留空字符串即可
- 本地发版（不走 CI）也要签，**2026-10-08 实测的可用配方**（CLI 2.11.4）：
  - `build` 只认 `TAURI_SIGNING_PRIVATE_KEY`（= `kada.key` **文件全文**）；只设 `TAURI_SIGNING_PRIVATE_KEY_PATH` 会报 `A public key has been found, but no private key`（`..._PATH` 只有 `tauri signer sign -f` 认）。
  - 密码是**空串**，但 PowerShell `$env:X=""` 等于**删除**该变量 → 会卡在 `expect a prompt for password`（交互提示在管道下永远等不到）。用 Python 传「存在但为空」的环境变量即可无人值守：
    ```py
    env = dict(os.environ)
    env["TAURI_SIGNING_PRIVATE_KEY"] = Path(r"C:\Users\chill\.tauri\kada.key").read_text()
    env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = ""
    subprocess.run("npx tauri build --bundles nsis", shell=True, env=env)  # cwd = 仓库根
    ```
  - 产物：仓库根 `target/release/bundle/nsis/Kada_x.y.z_x64-setup.exe` + 同名 `.sig`；打包前先退出运行中的 Kada。
- 客户端只认内嵌公钥校验签名，**私钥泄露才需轮换公钥并随新版客户端下发**。
- ⚠ **私钥丢失 = 现有用户再也收不到更新**（只能轮换公钥 + 让用户手动装一次新版）。请把 `kada.key` 备份进密码管理器。
- **已开启加固**：`plugins.updater.requireSignedVersion = true` 要求签名里带版本号，挡「拿旧版有效签名冒充新版」的降级攻击（端点响应不走签名，`version` 字段可被篡改；校验签名里的版本号与端点声明是否一致）。**发版必须用会把版本号写进签名 `trusted comment` 的 Tauri CLI**（`version:X.Y.Z` 字段）：本地 CLI 2.11.4、CI 的 `tauri-action@v0` 都会写；不写版本号的老签名包会被客户端以 `MissingSignedVersion` 直接拒装。本项目此前从未发过版，无历史签名包袱，故可直接开启。
- （可选，与更新机制正交）对安装包做 **Windows 代码签名证书**签名，可降低 SmartScreen 拦截；正式对外分发时建议补。

### 2.4 客户端更新流程（已实现）

三个入口，状态机都在 `src-tauri/src/update.rs`：

| 入口 | 行为 |
|------|------|
| 启动后台检查 | release 构建启动 **8 秒后**静默查一次。**只有真发现新版本**才弹一个气泡「发现新版本 x.y.z / 托盘右键「检查更新」可安装」；网络不通、还没发过 Release、清单里没有本平台条目都完全静默（不打扰正在用的人）。dev 构建跳过（版本号还是开发中的号，查了也没意义）。 |
| 托盘菜单「检查更新」/ 设置页「检查更新」 | 结果一定给反馈：**已是最新**、**出错**都弹窗；**发现新版本**弹确认框（写明「安装时应用会退出，完成后自动打开新版本」），确认后下载并安装。 |
| 设置页「下载并安装」 | 已知有新版本时出现，点了直接装（不再二次确认——点击本身就是确认）。 |

下载与安装的要点：

- 下载**带进度**：按「整数百分比变化」才推 `update-status` 事件（分片回调极密集，每片都 emit 会把 IPC 和前端渲染刷爆），设置页状态行实时显示 `正在下载 v0.2.0… 42%`。
- **Windows 上「安装」这一步会退出应用**（Windows 安装器的限制，Tauri updater 明确如此）：NSIS 以 `passive` 模式带 `/R` 参数跑（小进度窗 + 装完自动重启）。**这正是「启动检查只提示、不自动安装」的原因**——自动安装等于替用户决定「现在关掉你正在用的应用」。
- 装完由安装器拉起新版本，不依赖旧进程做收尾（旧进程此时已经不在了）。
- 失败回退：网络或校验失败**保留当前版本**，弹窗报错并在设置页状态行留错误信息（启动检查的失败则完全静默）。

状态源在后端：更新阶段（`idle` / `checking` / `up-to-date` / `available` / `downloading` / `installing` / `error`）由 Rust 持有并经 `update-status` 事件推送，设置页另用 `get_update_status` 兜底拉一次（窗口晚于检查打开时才不会错过）。

### 2.5 依赖与权限

- Rust 依赖 `tauri-plugin-updater`；**不需要** `@tauri-apps/plugin-updater`，也**没有**给前端开 `updater:*` 权限——更新逻辑全在壳内（`UpdaterExt`），前端只调自己的三个命令（`check_update` / `install_update` / `get_update_status`）。capabilities 管的是 WebView 的 IPC，前端不直接碰 updater 就不开这个权限面。
- 打包新增 `bundle.createUpdaterArtifacts = true`（产出 `.sig`）。注意它只在**带 bundle 的 release 构建**里生效；日常快速打包用的 `--no-bundle` 不受影响。

### 2.6 落地清单

| 步骤 | 内容 | 状态 |
|------|------|------|
| 1 | 加依赖 `tauri-plugin-updater` | ✅ `src-tauri/Cargo.toml` |
| 2 | 配置 `plugins.updater.pubkey` + `endpoints` + `windows.installMode` | ✅ `src-tauri/tauri.conf.json` |
| 3 | 壳内注册插件 + 托盘「检查更新」+ 启动静默检查 + 3 个命令 | ✅ `src-tauri/src/lib.rs`、`src-tauri/src/update.rs` |
| 4 | 加能力 `updater:default` | ⛔ **不需要**：更新走 Rust 侧，前端不开 updater 权限（见 §2.5） |
| 5 | 生成签名密钥对，公钥入库、私钥进 CI Secret | ✅ 密钥已生成（公钥入库）；⚠ **私钥仍需你手动加进 GitHub Secrets** |
| 6 | 发布流水线（构建→签名→latest.json→Release） | ✅ `.github/workflows/release.yml` |
| 7 | （可选）Windows 代码签名证书 | 🔧 未做 |

### 2.7 发版步骤

1. 改版本号（三处同值）：`src-tauri/tauri.conf.json` 的 `version`、`src-tauri/Cargo.toml`、`ui/package.json`。
2. 归档本次变更（`/archive`）并提交。
3. 打 tag 推上去：`git tag v0.2.0 && git push origin v0.2.0`（或在 Actions 里手动跑 Release workflow）。
4. 等 `release (windows)` job 完成 → Releases 里出现一个 **draft**，Assets 含 `Kada_0.2.0_x64-setup.exe` 与 `latest.json`。
5. 下载 draft 里的安装包在本机验一次安装（首次发版建议再验一次「旧版点检查更新能否升上来」），确认无误后 **Publish**。
6. 之后：已装旧版的机器启动 8 秒后气泡提示，或托盘右键「检查更新」即可升级。

> 首次发版前必须先做 §2.3 的 Secrets 配置，否则流水线会因缺少签名私钥而失败——**未签名的更新包客户端一律拒绝**，这是设计使然，不是 bug。
