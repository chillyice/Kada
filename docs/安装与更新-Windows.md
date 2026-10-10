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

当前 `tauri.conf.json` 的 `bundle.targets = "all"`，用 Tauri 默认打包参数（未做自定义安装配置）；**发版流水线另有取舍**（Windows 只出 NSIS、Linux 只出 deb + AppImage、另打一个便携版 zip，见 §1.2 与 §2.6）。

> **打包前必读三条**（前两条都踩过）：
> 1. **先退出正在运行的 Kada**。exe 被占用时构建会在最后一步报「拒绝访问」，而且 bundler 给二进制写入「安装包类型」标记这一步会失败并警告 `Updater plugin may not be able to update this package`——这个标记决定已安装的客户端认不认 NSIS 更新包，**打包时它写不进去就可能导致更新装不上**。（`scripts/kada-package.ps1` 已自动先杀再构建。）
> 2. **首次打包要联网下载 NSIS 工具链**（`github.com/tauri-apps/binary-releases` 上的 `nsis-3.11.zip`）。本机曾因网络拉不通（`timeout: global`）打不出安装包；CI runner（GitHub 托管机器）网络正常，发版走流水线即可。
> 3. **Tauri CLI 版本不能太老**（2026-10-10 实测，见 §2.3 的「版本号写不进签名」那条）：`requireSignedVersion = true` 要求安装包签名里带版本号，`tauri-cli < 2.12.1` 的打包签名**不带**，出来的包客户端一律拒装。根目录 `npm ci` 装的就是 `package.json` 里那个 CLI，别在老 node_modules 上打包；打完用 §2.8 的脚本核一遍。

### 1.2 分发渠道：三条路发的**不是同一个文件**

| 渠道 | 发的产物 | 面向 | 更新方式 |
|------|---------|------|---------|
| **GitHub Releases** | Windows：`Kada_<版本>_x64-setup.exe`（NSIS）+ `Kada_<版本>_x64-portable.zip`（便携版）+ `.sig` + `latest.json`；Linux：`kada_0.1.0_amd64.deb` / `kada_<版本>_amd64.AppImage` + `.sig` | 已装客户端的**自动更新**、CI 发版 | 客户端 updater 插件按 §2 走 |
| **ihomy 发布页** `https://ihomy.top/kada` | **裸 `kada.exe`**（`target/release/kada.exe`，不是 NSIS 包） | 公开下载、首次安装 | 每次手动 scp，见下 |
| **GitHub Releases（Linux 包）** | 同上那一行里的 deb / AppImage | Linux 发行版与桌面用户 | 走同一份 `latest.json`（`linux-x86_64` 条目） |

三条渠道各发各的，**不要把 NSIS 包传到 ihomy 页**（页面上写的是「绿色免安装、暂未做代码签名」，给的就是裸 exe）。

#### 便携版 zip（新增，2026-10-10）

- 流水线在 Windows job 里把**裸 `kada.exe`** 压成 `Kada_<版本>_x64-portable.zip` 上传到 Release。解压即用：没有安装器、不写注册表、不建开始菜单项，配置照样在 `%APPDATA%\com.kada\`。
- **前提是系统里有 WebView2 Runtime**（Win10 21H2 / Win11 自带；更老的 Win10 需先去微软官网装）。
- **不参与自动更新**：updater 只认 NSIS/MSI 这类安装器包，zip 就是给人手动替换 exe 用的。删了旧 exe、换成新 exe、重启即可（替换前先退出程序）。
- 与 ihomy 页的裸 exe 是同一种东西，区别只是 zip 省一次「下载完记得放哪」的麻烦；两条渠道都发。

#### Linux 包（新增，2026-10-10）

- 流水线新增 `ubuntu-22.04` job，出 **deb** 与 **AppImage**，产物同样签名并进 `latest.json`（`linux-x86_64`），所以 **Linux 用户也能在应用内点「检查更新」**。
- **deb 安装 / 更新会弹管理员密码**（Tauri 的 Linux updater 走 `pkexec`；AppImage 是原地替换自身）。AppImage 首次运行可能需要给它 FUSE 权限（`chmod +x` 后直接跑，或用 AppImageLauncher）。
- **输入层权限与打包无关、也不因装包自动解决**：evdev/uinput 那套要 `input` 组 + udev 规则，见 `docs/需求设计说明书.md` §4.2 与 `docs/人工验证清单.md` §19。本项目至今没有一台 Linux 真机，Linux 的安装与更新链路同样**未经真机验证**。
- macOS **不在发版矩阵里**：macOS 分发要代码签名 + 公证 + 分发证书，且输入层整条链路还没有真机跑过（§4.3 / 清单 §15），现在发 dmg 只会得到一个 Gatekeeper 直接拦下的包。等真机验过再加进矩阵。

#### ihomy 发布页的上传流程

页面源码在**另一个仓库** `Projects/ihomy`（`frontend/src/views/Kada.vue`），安装包托管在 ihomy 生产服务器的 nginx `/files/` 静态目录下。

```powershell
# 1) 先退出运行中的 Kada（否则 exe 被占用）
Get-Process -Name kada -ErrorAction SilentlyContinue | Stop-Process -Force

# 2) 备份线上旧包（覆盖前先留一份，便于回退）
ssh -p 19068 root@ihomy.top "cp -a /opt/ihomy/uploads/kada/kada.exe /opt/ihomy/uploads/kada/kada.exe.bak-<日期>"

# 3) 上传裸 exe（SSH 端口 19068，不是 22）
scp -P 19068 target\release\kada.exe root@ihomy.top:/opt/ihomy/uploads/kada/kada.exe
ssh -p 19068 root@ihomy.top "chown ihomy:ihomy /opt/ihomy/uploads/kada/kada.exe"

# 4) 校对：两端 md5 必须一致
(Get-FileHash target\release\kada.exe -Algorithm MD5).Hash.ToLower()
ssh -p 19068 root@ihomy.top "md5sum /opt/ihomy/uploads/kada/kada.exe"

# 5) 验证线上确实换成了新包（匿名可下就是对的）
(Invoke-WebRequest "https://ihomy.top/api/files/kada/kada.exe" -Method Head).Headers["Content-Length"]
```

- **这个目录不在 `deploy.ps1` 的同步范围内**，每次换版本都要**手动 scp**，跑 `deploy.ps1` 不会带上安装包。
- 换版本后还要改 `Projects/ihomy/frontend/src/views/Kada.vue` 顶部三个常量（`filename` / `version` / `fileSize`，版本与体积是**硬编码**的，不改页面就一直显示旧值），再走 `ihomy/scripts/deploy.ps1 -FrontendOnly` 才能生效。
- **`/files/kada/**` 在 ihomy 后端是免登录放行**（安装包要能匿名下载），且 nginx 的 `location /files/` 必须**反代**到后端而不是 alias 直出——直出会绕过整套访问判定。改 nginx 前先看 ihomy 仓的 `docs/踩坑速查.md` §7.8。
- ihomy 生产凭证与部署细节归 ihomy 仓管，**不写进本仓**。

### 1.3 用户安装步骤

1. 下载 `Kada_<版本>_x64-setup.exe` 安装包。
2. 双击运行，按向导完成安装。
3. 安装完成后启动「咔哒 Kada」，程序以**托盘图标**常驻后台；主窗口可关闭（关闭 = 隐藏到托盘，不退出）。
4. 首次使用在界面里配置快捷键 / 改键 / 宏，保存后即时生效（无需重启）。

> 权限说明：全局快捷键/改键用 Windows 低层钩子（`WH_KEYBOARD_LL`），**无需管理员权限**；但低层钩子拦不住 UAC 提权进程与部分游戏（见 `docs/架构设计.md` §5）。
>
> 安装目录与提权：Tauri v2 的 NSIS 默认 `installMode = currentUser`（装到免管理员权限的目录，卸载信息写 `HKCU`），所以**默认安装过程不弹 UAC**。要让程序装进 `Program Files`、开机自启对所有用户生效，得在 `tauri.conf.json` 配 `bundle.windows.nsis.installMode = "perMachine"`（那样安装与更新都需要管理员）。本项目保持默认。

### 1.4 首次运行与数据位置

- 配置（快捷键/改键/宏）以 JSON 落盘在 Tauri `app_data_dir()` 下的 `config.json`，Windows 上约 `%APPDATA%\com.kada\config.json`。
- 配置是纯 JSON 文本，可直接跨平台同步/备份（键模型与配置结构见 `docs/需求设计说明书.md` §2/§3）。
- **更新不影响配置**：升级只替换程序文件，`%APPDATA%\com.kada\` 不动。

### 1.5 卸载

- 图形界面：Windows 设置 → 应用 → 已安装的应用 → 咔哒 Kada → 卸载。
- 或运行安装目录下的卸载程序（NSIS 生成的 `uninstall.exe`）。
- 卸载**不清除** `%APPDATA%\com.kada\` 下的配置文件；如需彻底清除请手动删除该目录。

## 二、软件更新（✅ 客户端与流水线已落地）

> 选型：**Tauri 官方 `tauri-plugin-updater` + GitHub Releases 作更新源 + Ed25519 签名**。
> 代码在 `src-tauri/src/update.rs`（状态机与流程）、`plugins.updater`（`src-tauri/tauri.conf.json`）、
> 发布流水线在 `.github/workflows/release.yml`。
> ⚠ 端到端升级需要**先配好仓库 Secrets 并发一次 Release**才能真跑通（见 §2.7）。在此之前能自动验的只有「产物与清单自洽」（§2.8），客户端侧的真机验收见 `docs/人工验证清单.md` 附录。

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
- **已开启加固**：`plugins.updater.requireSignedVersion = true` 要求签名里带版本号，挡「拿旧版有效签名冒充新版」的降级攻击（端点响应不走签名，`version` 字段可被篡改；校验签名里的版本号与端点声明是否一致）。
- ⚠⚠ **版本号写不进签名 = 客户端一次更新都装不上（2026-10-10 实测并修复）**：`requireSignedVersion` 读的是签名 trusted comment 里的 `version:` 字段。实测 **tauri-cli 2.11.4 的 `tauri build`（`createUpdaterArtifacts`）签出来的 trusted comment 只有 `timestamp:…\tfile:…`，没有 version**——客户端会以 `MissingSignedVersion` 直接拒装，且**流水线一路绿灯**（签名步骤自己成功，产物也上传了），只有真用户点更新才炸。`2.12.1` 起 `tauri build` 才自动写上（`tauri signer sign` 则要手写 `--app-version` 才写）。根因是根目录 `package.json` 写的 `@tauri-apps/cli: "^2"` 曾把 lock 锁在 2.11.4。现已把依赖下限提到 `^2.12.1`、流水线补了根目录 `npm ci`，并加了 §2.8 的产物自检盯着这条不变量。**升级/降级 Tauri CLI 后一定要重跑 §2.8。**
- **（可选）Windows 代码签名证书**（对安装包做 Authenticode 签名，降低 SmartScreen「未知发布者」拦截）：流水线已备好导入证书的步骤——Secrets 里放 `WINDOWS_CERTIFICATE`（`.pfx` 的 base64）与 `WINDOWS_CERTIFICATE_PASSWORD`，再把证书指纹写进 `src-tauri/tauri.windows.conf.json`（Tauri 会自动把这个平台专属配置合并进 `tauri.conf.json`）：
  ```json
  { "bundle": { "windows": { "certificateThumbprint": "<指纹>", "digestAlgorithm": "sha256", "timestampUrl": "http://timestamp.digicert.com" } } }
  ```
  证书没配时那一步直接跳过，不影响发版。**与更新机制正交**：updater 验的是 Ed25519 安装包签名（`plugins.updater.pubkey`），Authenticode 解决的是 Windows 认不认这个发布者，两套各管各的。

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
- **其它平台装完由应用自己重启**（`app.restart()`，2026-10-10 补）：Windows 的安装器会带走进程，Linux（deb 走 `pkexec`、AppImage 原地替换）不会——不主动重启就一直跑着旧二进制，等于「更新了但没生效」。
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
| 6 | 发布流水线（构建→签名→latest.json→Release） | ✅ `.github/workflows/release.yml`（2026-10-10：补了根目录 `npm ci`，此前缺这步 `npm run tauri` 找不到 CLI、流水线必失败） |
| 7 | （可选）Windows 代码签名证书 | 🔧 未做，**但流水线已备好导入步骤 + 配置位置**（配好 Secrets 与 `tauri.windows.conf.json` 即生效，见 §2.3） |
| 8 | 便携版 zip / Linux 包（deb + AppImage） | ✅ 2026-10-10 流水线新增（见 §1.2） |
| 9 | 发布产物自检（版本 / 签名 / 清单一致性） | ✅ `scripts/verify-release.mjs`，流水线最后一步必跑（见 §2.8） |

### 2.7 发版步骤

1. 改版本号（三处同值）：`src-tauri/tauri.conf.json` 的 `version`、`src-tauri/Cargo.toml`、`ui/package.json`。
2. 归档本次变更（`/archive`）并提交。
3. 打 tag 推上去：`git tag v0.2.0 && git push origin v0.2.0`（或在 Actions 里手动跑 Release workflow）。
4. 等两个 job（`release (windows-latest)` → `release (ubuntu-22.04)`，串行）都完成 → Releases 里出现一个 **draft**，Assets 含 `Kada_0.2.0_x64-setup.exe`、`Kada_0.2.0_x64-portable.zip`、Linux 的 `.deb` / `.AppImage`、各自的 `.sig` 与 `latest.json`。
5. **看两个 job 的最后一步（Verify release artifacts）都是绿的**——它红了就说明产物与清单对不上，**别 Publish**（§2.8）。
6. 下载 draft 里的安装包在本机验一次安装（首次发版建议再验一次「旧版点检查更新能否升上来」），确认无误后 **Publish**。
7. 之后：已装旧版的机器启动 8 秒后气泡提示，或托盘右键「检查更新」即可升级。

> 首次发版前必须先做 §2.3 的 Secrets 配置，否则流水线会因缺少签名私钥而失败——**未签名的更新包客户端一律拒绝**，这是设计使然，不是 bug。

### 2.8 发布产物自检（`scripts/verify-release.mjs`）

发版流水线最后一步会跑它，本地也能跑（打完包就能查，不必等流水线）：

```bash
node scripts/verify-release.mjs                                  # 查 target/release/bundle
node scripts/verify-release.mjs --dir <产物目录> --latest <latest.json>   # 连更新清单一起查（流水线口径）
node scripts/verify-release.mjs --selftest                       # 解析器自检，不需要真产物
```

查的是**流水线自洽性**，六条：① 版本号三处同值；② 每个安装包有同名 `.sig`；③ 签名 key id 与 `tauri.conf.json` 内嵌公钥一致（对不上 = 私钥与公钥不是同一对，客户端一律拒装）；④ 签名 trusted comment 里有 `version:` 且等于配置版本（`requireSignedVersion` 的硬要求）；⑤ 文件名里的版本号与配置一致；⑥ `--latest` 时清单版本一致、清单条目指向的包与签名都在，且 `signature` 字段与 `.sig` 文件内容逐字相同。**任一条不过 `exit 1`，流水线红。**

**不查密码学**：minisign 是「预哈希 + 全局签名」两段，自校验要引入原生库；抗伪造的真校验在客户端 updater 插件里（`tauri-plugin-updater` 的 `verify_signature`，验不过直接拒装）。这里防的是「打包/发版时自己把事情搞错了」。

> 这套校验不是摆设：2026-10-10 第一次跑就抓出了 §2.3 那条——本机用老 CLI（2.11.4）打出来的包签名里没有版本号，客户端会以 `MissingSignedVersion` 拒装，而当时流水线对这种情况**毫无察觉**。
