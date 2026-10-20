# 咔哒 Kada

轻快跨平台快捷键工具：全局快捷键、改键、键盘层、文本扩展、宏，桌面托盘常驻（关窗不退出），配置以 JSON 落盘、可跨平台同步。

## 特性

- **全局快捷键**：三种触发形态——组合键（`Ctrl+Alt+K`）、按键序列（`F9 J K`，首步即 leader 键）、和弦（`F&J` 同时按住，成员可带修饰要求），任一命中即跑动作链。
- **动作链（宏）**：文本 / 命令（CMD、PowerShell）/ 脚本 / 应用 / 打开网址 / 文件动作 / 按键组合 / 鼠标操作 / 延迟 / 条件判断 / 并行执行；命令输出可存成变量供后续动作用 `{变量名}` 引用；命令与脚本都有超时与中止，超时或中止会杀掉整棵进程树，并行成员共用一把注入锁保证注入不交错。
- **改键**：任意键改发另一键，全形态——普通改键、tap-hold（短按 / 双击 / 三击 / 长按，阈值可调）、单次键 / 粘滞键、长按进入层 / 长按锁定层。
- **键盘层**：同一按键在层内与基础层干不同事（层内优先、基础层兜底），长按切层键进入；每层可设一个颜色或 emoji 标记，列表条目上一眼看出归属。
- **文本扩展**：输入触发词自动展开成一段文本（支持 `{date}` / `{time}` / `{clipboard}` 动态片段）。
- **触发条件**：同一按键可按前台应用 / 窗口标题 / 路径 / 变量 / 时间 / 设备（Linux）走不同分支。
- **执行记录与消息**：每次触发逐步记下每个动作的时间线 / 流水线（含条件分支走向与并行成员），命令结果、中止、动作失败、配置事件都会进消息页。
- **提示浮层**：输入状态指示（当前层 / 按住式层）、快捷键速查提示框（可拖整块面板，光标进框自动让位变淡）。
- **配置**：JSON 落盘（原子写 + 上次良好副本 + 损坏自愈，绝不静默清空）、导入导出（合并 / 替换）、卡带分享（把选中条目或整个目录导出为卡带，对方走导入合并吃下）、内置示例配置、外部修改监听。
- **界面**：合并列表栏 + 三类条目统一编辑页 + 动作流程图编排 + 宏录制；四主题（像素 CRT 深色 / 浅色、简约 深色 / 浅色，默认像素 CRT 深色）；托盘常驻、关窗不退出，另有快速唤醒键。
- **软件更新**：更新源可选（默认 ihomy 镜像，服务器每天从 GitHub 已发布版本同步；可切 GitHub Releases）+ Ed25519 签名校验，启动后每日自动检查。**检查、下载、安装分成三步**：默认手动点「下载」「安装」；勾「自动更新」则发现即下载，并在电脑长时间空闲时自动安装（免安装版另在下次启动时装，且原位替换 `kada.exe` 后重启）。
- **跨平台**：键模型平台无关。Windows（`WH_KEYBOARD_LL` + `WH_MOUSE_LL`）、Linux（evdev + uinput，X11 / Wayland 通用）、macOS（`CGEventTap`，需授予「辅助功能」权限）——三平台输入层均已实现。

## 技术栈

- Rust workspace：`kada-core`（纯逻辑：键模型 / 解析匹配 / 配置模型 / 冲突检测 / 卡带摘取）+ `kada-hook`（平台钩子引擎：全局监听 + 拦截 + 注入）+ `kada-actions`（动作执行引擎）
- Tauri 2 桌面壳（crate `kada`）+ Vite / TypeScript 前端（`kada-ui`）

## 构建与验证

```powershell
cargo test --workspace                     # 全 workspace 测试
cargo check -p kada                        # Tauri 壳（完整版）
cargo check -p kada --no-default-features  # 基础版（裁掉 automation：命令 / 脚本 / 条件 / 变量等）
npm --prefix ui run build                  # 前端构建（tsc + vite）
cargo run -p kada-hook --example demo      # 钩子冒烟 demo（三平台真人按键）
```

- 纯浏览器调试前端：`npm --prefix ui run dev` 后直开 `http://localhost:1420`（无 Tauri 壳时自动注入模拟 IPC 与演示配置）。
- 打包：`npm run tauri -- build --bundles nsis`，需 `TAURI_SIGNING_PRIVATE_KEY`（安装包签名带版本号，否则客户端拒装）；产物在仓库根 `target/release/bundle/`。**Linux 包**（deb / AppImage）在 Windows 上打不出来，借 docker 容器出（`scripts/linux-package/`）；**macOS 包**（universal dmg）由 CI 发布矩阵出（本机没有 Mac，也交叉不了）。

## 文档

- 项目规则与约定：`AGENTS.md`（AI 会话自动加载）
- 文档索引：`docs/README.md`
- 功能需求（活文档）：`docs/需求设计说明书.md`
- 架构与关键机制：`docs/架构设计.md`
- 实现归档：`docs/变更归档.md`

## License

MIT
