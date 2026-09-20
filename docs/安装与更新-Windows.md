# Kada Windows 版安装与软件更新

> 本文档覆盖 Windows 版的**安装步骤**（当前可用）与**软件更新策略**（选型已定、代码未落地）。
> 状态标注：✅ 当前可用；🔧 设计已定、待实现。
> 配套：功能需求见 `docs/需求设计说明书.md`；架构与跨平台策略见 `docs/架构设计.md`。

## 一、安装（✅ 当前可用）

### 1.1 构建安装包（开发者）

在仓库根目录执行（`beforeBuildCommand` 会自动先跑 `npm --prefix ui run build`）：

```powershell
npm run tauri build
# 或 npx tauri build / cargo tauri build
```

产物输出到 `src-tauri/target/release/bundle/`，Windows 下含：

| 格式 | 路径 | 说明 |
|------|------|------|
| NSIS 安装包 | `nsis/Kada_<版本>_x64-setup.exe` | 向导式安装，**推荐分发用** |
| WiX MSI | `msi/Kada_<版本>_x64_<语言>.msi` | 企业/组策略静默部署用 |
| MSIX | `msix/Kada_<版本>_x64.msix` | Microsoft Store / 旁加载 |

当前 `tauri.conf.json` 的 `bundle.targets = "all"`，用 Tauri 默认打包参数（未做自定义安装配置）。

### 1.2 用户安装步骤

1. 下载 `Kada_<版本>_x64-setup.exe` 安装包。
2. 双击运行，按向导完成安装（默认 NSIS 安装到系统程序目录，可能触发 UAC 提权确认）。
3. 安装完成后启动「咔哒 Kada」，程序以**托盘图标**常驻后台；主窗口可关闭（关闭 = 隐藏到托盘，不退出）。
4. 首次使用在界面里配置快捷键 / 改键 / 宏，保存后即时生效（无需重启）。

> 权限说明：全局快捷键/改键用 Windows 低层钩子（`WH_KEYBOARD_LL`），**无需管理员权限**；但低层钩子拦不住 UAC 提权进程与部分游戏（见 `docs/架构设计.md` §5）。

### 1.3 首次运行与数据位置

- 配置（快捷键/改键/宏）以 JSON 落盘在 Tauri `app_data_dir()` 下的 `config.json`，Windows 上约 `%APPDATA%\com.kada\config.json`。
- 配置是纯 JSON 文本，可直接跨平台同步/备份（键模型与配置结构见 `docs/需求设计说明书.md` §2/§3）。

### 1.4 卸载

- 图形界面：Windows 设置 → 应用 → 已安装的应用 → 咔哒 Kada → 卸载。
- 或运行安装目录下的卸载程序（NSIS 生成的 `uninstall.exe`）。
- 卸载**不清除** `%APPDATA%\com.kada\` 下的配置文件；如需彻底清除请手动删除该目录。

## 二、软件更新策略（🔧 设计已定、待实现）

### 2.1 选型结论

采用 **Tauri 官方 `tauri-plugin-updater` 插件 + GitHub Releases 作更新源 + Ed25519 签名**。理由：

- 官方插件，与 Tauri 2 打包/签名链路天然打通，支持后台静默下载、退出时安装。
- 更新源用 GitHub Releases（仓库即 `chillyice/Kada`），无需自建服务器。
- Ed25519 签名保证更新包完整性与来源可信（防篡改/防中间人）。

### 2.2 更新源与发布流程

- 每次发布在 GitHub Releases 上传安装包（`*-setup.exe`），并附带一份 `latest.json` 更新清单。
- `latest.json` 结构（固定 URL `.../releases/latest/download/latest.json`）：

```json
{
  "version": "0.2.0",
  "notes": "更新说明",
  "pub_date": "2026-09-17T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "（安装包内容的 Ed25519 签名）",
      "url": "https://github.com/chillyice/Kada/releases/download/v0.2.0/Kada_0.2.0_x64-setup.exe"
    }
  }
}
```

- 渠道：单 `stable` 渠道起步；后续需要灰度再拆 `beta`/`stable` 双渠道（各一份 `latest.json`）。

### 2.3 签名与安全

- 用 `tauri signer generate` 生成 Ed25519 密钥对：**公钥**内嵌进 `tauri.conf.json` 的 `plugins.updater.pubkey`（随仓库）；**私钥**只进 CI Secret（`TAURI_SIGNING_PRIVATE_KEY`，可加密码 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`），绝不入库。
- 发布流水线用 `tauri-apps/tauri-action` 自动完成：构建 → 用私钥对产物签名 → 生成 `latest.json` → 上传 Release。
- 客户端只认内嵌公钥校验签名，私钥泄露需轮换公钥并随新版客户端下发。
- （可选，与更新机制正交）对安装包做 **Windows 代码签名证书**签名，可降低 SmartScreen 拦截；正式对外分发时建议补。

### 2.4 客户端更新流程（规划）

1. 启动时 + 托盘菜单「检查更新」手动入口，调 `check()` 拉取 `latest.json`。
2. 有新版本 → 后台静默 `downloadAndInstall()`（下载到临时目录）。
3. 用户下次退出/重启应用时自动安装，安装完自动拉起新版本。
4. 失败回退：网络/校验失败不打断使用，仅提示；保留当前版本。

### 2.5 落地清单（roadmap）

| 步骤 | 内容 | 涉及 |
|------|------|------|
| 1 | 加依赖 `tauri-plugin-updater` + `@tauri-apps/plugin-updater` | `src-tauri/Cargo.toml`、`ui/package.json` |
| 2 | 配置 `plugins.updater.pubkey` + `endpoints` | `src-tauri/tauri.conf.json` |
| 3 | 壳内注册插件 + 托盘加「检查更新」 | `src-tauri/src/lib.rs` |
| 4 | 加能力 `updater:default` | `src-tauri/capabilities/default.json` |
| 5 | 生成签名密钥对，公钥入库、私钥进 CI Secret | 本地 `tauri signer generate` + GitHub Secrets |
| 6 | 加发布流水线（构建→签名→生成 latest.json→Release） | `.github/workflows/release.yml`（`tauri-apps/tauri-action`） |
| 7 | （可选）Windows 代码签名证书 | 证书采购 + `tauri-action` 签名参数 |

> 版本号规则：`latest.json` 的 `version` 与 `tauri.conf.json`/`Cargo.toml`/`package.json` 的版本保持一致；发版时先改版本号再打流水线。
