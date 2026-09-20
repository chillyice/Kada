# AGENTS.md — Kada 项目规则

> 本文件供 ZCode 跨会话加载，记录项目关键规则、约定与当前事实。新会话启动时自动读取，无需重复说明背景；修改后立即对所有新会话生效。
> **分工**：本文件只留规则 + 导航索引，目标 **≤60KB 保证完整注入上下文**（超出会被截断且尾部最先丢失）；完整功能需求见 `docs/需求设计说明书.md`（活文档）；历史实现归档见 `docs/变更归档.md`（新归档追加到该文件，不写回本文件）。

> **⚠ Git 规定（必须遵守）**：非人工指令，不得主动提交代码（`git commit`/`git add -A`/`git push` 一律禁止）。`git add` 只能指定具体文件路径，禁止 `git add -A`/`git add .`。归档文档走 `/archive` 技能（归档推送）。

> **⚠ 编码/路径警示（遵守以防误写）**：
> - 工作目录绝对路径：`C:\Users\chill\OneDrive\WorkStation\Projects\Kada`
> - 含中文的源码/配置/文档一律走本工具的 Read/Write/Edit 读写，禁止用 PowerShell `Get-Content`/`Set-Content`/`WriteAllText` 读写（PS 5.1 默认 GBK 会破坏 UTF-8 中文）。PowerShell 仅用于：cargo/npm 构建、运行 demo。
> - Git Bash（MSYS）调 Windows 原生命令时，POSIX 风格路径会被自动转换，涉及 Windows 工具时注意参数路径。

## 项目概述

- **应用名**：咔哒 Kada —— 轻快跨平台快捷键工具。
- **定位**：全局键盘快捷键 / 改键 / 宏工具，桌面托盘常驻（关窗不退出）。配置以 JSON 落盘，可跨平台同步。
- **技术栈**：Rust workspace（`kada-core` 纯逻辑 + `kada-hook` 平台钩子）+ Tauri 2 桌面壳（crate `kada`）+ Vite/TypeScript UI（`kada-ui`）。无数据库、无后端服务，纯本地。

## 工作目录

项目位于单一目录：

| 目录 | 用途 |
|------|------|
| `C:\Users\chill\OneDrive\WorkStation\Projects\Kada` | 唯一工作目录（代码编辑 + 构建验证 + 文档） |

- 所有代码编辑、编译、构建验证都在此目录进行。
- 配置运行时落盘在 Tauri `app_data_dir()/config.json`（不在仓库内）。

## 命名约定（务必遵守）

| 项 | 值 |
|----|----|
| Rust workspace members | `crates/kada-core` / `crates/kada-hook` / `src-tauri` |
| Tauri crate 名（`src-tauri/Cargo.toml`） | `kada` |
| npm 包名（`ui/package.json`） | `kada-ui` |
| Tauri `identifier` | `com.kada` |
| `productName` / 窗口标题 | `Kada` / `咔哒 Kada` |
| 应用显示名 | 咔哒 Kada |

- **键模型使用平台无关中性名**：`Enter/Escape`（不是 `Return`）、修饰键 `Ctrl/Alt/Shift/Meta`（macOS 的 ⌘ 由 `Meta` 承载，平台层负责把 OS 键映射成 `Key`）。键名渲染回 `"Ctrl+Alt+K"` 形式（`format_shortcut` / `key_name`）。

## 代码结构

```
crates/kada-core/           # 零重量纯逻辑：键模型、快捷键解析/匹配、配置模型
  src/lib.rs                # Key/Modifier/RawEvent/Shortcut + parse/format/matches + Config/Action/冲突检测
crates/kada-hook/           # 平台钩子引擎：全局键盘事件监听 + 拦截 + 注入
  src/lib.rs                # 按平台 re-export win / linux
  src/win.rs                # Windows: WH_KEYBOARD_LL 钩子 + swallow/Replace 状态机 + key↔VK 映射
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate 子模块
  examples/demo.rs          # M1 冒烟 demo（仅 Windows，真人按键验证）
  tests/hook_smoke.rs       # 钩子安装/回收冒烟（仅 Windows）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻
  src/main.rs               # 入口
  tauri.conf.json           # 窗口/图标/构建配置
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键/改键/宏录制/消息中心
  src/style.css             # 深色 UI 样式
.github/workflows/ci.yml    # CI：ubuntu + windows 双平台构建 + 测试
```

## 键模型与配置（核心概念）

- **键模型**（`kada-core`）：`Key`（字母/数字/F1-F12/标点/功能键/方向键）、`Modifier`（Ctrl/Alt/Shift/Meta）、`RawEvent { key, mods, pressed }`、`Shortcut { mods, key }`。
- **解析**：`"Ctrl+Alt+K".parse::<Shortcut>()`，`+` 分隔，`Ctrl/Control/Cmd/Win`→Ctrl、`Alt/Option`→Alt、`Meta/Super`→Meta。裸修饰键名（如 `"Shift"`）按按键处理（改键场景）。
- **匹配**（`matches`）：主键一致 **且** 事件修饰键 ⊇ 快捷键修饰键——按下 `Ctrl+Shift+K` 也会命中 `Ctrl+K`（宽松规则，避免多按一个 Shift 就触发失败）。
- **配置模型**（JSON 落盘，跨平台同步介质）：
  - `Config { shortcuts: Vec<ShortcutItem>, remaps: Vec<Remap>, settings: Settings }`
  - `ShortcutItem { name?, triggers: Vec<String>, actions: Vec<Action>, enabled }`（`triggers` 任一组命中即触发，`actions` 按顺序执行）
  - `Action`：`Text / Cmd / Powershell / Launch / OpenFolder / Keys / PauseMs`（`Cmd`/`Powershell` 带 `show_output` 是否弹结果）
  - `Remap { from, to, enabled }`（`from` 键按下改发 `to` 键）
  - `Settings { autostart, paused, launch_minimized }`；`detect_conflicts` 检测硬/软冲突
- 手工编辑的配置允许缺字段、带未知字段（`#[serde(default)]`），加载时校验。
- **消息中心**（内存态，重启清空）：Cmd/PowerShell 结果一律记入消息中心；`show_output` 开则弹结果弹窗、关则托盘图标 + 应用内「消息」入口亮红点，进入「消息」页标记已读。

## 钩子引擎

- **Windows**（`kada-hook::win`）：`SetWindowsHookEx(WH_KEYBOARD_LL)`，独立线程跑消息循环；处理函数直接跑在回调内（不做跨线程调度，保证顺序与低延迟）。修饰键用 `GetKeyState` 实时读；自动重复按同键 250ms 内再次 down 识别；`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键；注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持"按下-抬起"配对（按住原键 = 按住目标键）。
- **Linux**（`kada-hook::linux`）：evdev + uinput（内核输入层），`EVIOCGRAB` 独占抓取 `/dev/input/event*` 键盘设备、`/dev/uinput` 建虚拟键盘 `kada-virtual-keyboard` 转发；X11 / Wayland 通用；需要 root 或 `input` 组 + udev 放开 `/dev/uinput`。修饰键按事件流维护（`MODS_DOWN`），与 Windows `GetKeyState` 语义对齐。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V`（中文等 Unicode 最稳，会短暂占用并恢复剪贴板）；组合键全部按下 → 稍停 → 逆序松开。
- **已知天花板（升级路径）**：低层钩子拦不住 UAC 提权进程 / 部分游戏 → 驱动级拦截（Interception）；Linux 热插拔键盘不在监听列表（重启应用即可）。

## 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试（kada-core/kada-hook/src-tauri）
cargo check -p kada         # 验证 Tauri 壳编译
npm --prefix ui run build   # 前端构建（tsc + vite build）
cargo run -p kada-hook --example demo   # M1 冒烟 demo（仅 Windows）
```

- **CI**：GitHub Actions（`.github/workflows/ci.yml`）在 `ubuntu-latest` + `windows-latest` 双平台执行：前端安装 + 构建 + `cargo test --workspace` + `cargo check -p kada`。
- 平台相关代码用 `#[cfg(windows)]` / `#[cfg(target_os = "linux")]` 门控；非 Windows/Linux 平台（macOS）壳仍可编译运行（占位类型），但改键/快捷键/录制不可用。

## 里程碑与规划

> 完整里程碑进度与未实现规划见 `docs/需求设计说明书.md` 第 7 章；实现归档见 `docs/变更归档.md`。

| 里程碑 | 内容 | 状态 |
|--------|------|------|
| M0 | 工程骨架：workspace + Tauri 壳 + UI + CI + 图标 | ✅ 完成 |
| M1 | 核心键模型 + Windows 全局钩子引擎（demo.rs 冒烟） | ✅ 完成 |
| M2 | 配置模型与动作/宏（Action/Config JSON） | ✅ 完成 |
| M3 | Tauri 桌面壳 + 宏录制 + 配置界面（快捷键/改键/宏） | ✅ 完成 |
| M4 | Linux 钩子引擎（evdev + uinput） | ✅ 完成（未提交） |
| M5 | macOS 输入层 | ⬜ 规划 |

- **M5 macOS**：输入层待实现（`src-tauri/src/lib.rs` 中 macOS 分支为占位类型，保证壳可编译；钩子/快捷键/改键/录制暂不可用，托盘与配置界面可用）。
- 其它规划项见 `docs/需求设计说明书.md` 第 7 章。

## 文档清单

- `README.md`（项目简介，GitHub 展示）；`docs/README.md`（文档索引：每份文档一句话定位 + 新人阅读顺序）；`docs/架构设计.md`（分层 / 事件流转 / 关键机制 / 跨平台策略）；`docs/安装与更新-Windows.md`（Windows 版安装步骤 + 软件更新策略，选型已定待实现）；`docs/需求设计说明书.md`（功能需求唯一活文档 + 修订记录）；`docs/变更归档.md`（已实现变更归档，按里程碑的 文件-改动表 + 规则/决策）。
- 代码事实以 `crates/kada-core/src/lib.rs` + `crates/kada-hook/src/*.rs` + `src-tauri/src/lib.rs` 为准，如需检索先 `grep` 再动手。
- **文档规划借鉴 ihomy 与咖啡伴侣的结构化布局**：每个变更都有固定落点（规则 → 本文件；功能需求/规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`），保证多会话衔接、新会话可直接续接。

## 环境检查（参考）

本机已装：Rust toolchain、Node、Tauri CLI（`@tauri-apps/cli`）。前端依赖 `npm --prefix ui ci`；构建在 Windows 下进行。
