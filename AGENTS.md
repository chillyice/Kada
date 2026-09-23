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
  src/win.rs                # Windows: WH_KEYBOARD_LL + WH_MOUSE_LL 钩子 + swallow/Replace 状态机 + key↔VK 映射
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate 子模块
  examples/demo.rs          # M1 冒烟 demo（仅 Windows，真人按键验证）
  tests/hook_smoke.rs       # 钩子安装/回收冒烟（仅 Windows）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻；主窗口与气泡窗口按需懒创建（冷启动零 WebView）
  src/main.rs               # 入口
  tauri.conf.json           # 窗口/图标/构建配置
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键/改键/宏录制/消息中心
  src/style.css             # 深色 UI 样式
.github/workflows/ci.yml    # CI：ubuntu + windows 双平台构建 + 测试
```

## 键模型与配置（核心概念）

- **键模型**（`kada-core`）：`Key`（字母/数字/F1-F24/标点/功能键/方向键/媒体键/NumPad 区/NumLock/鼠标键）、`Modifier`（Ctrl/Alt/Shift/Meta）、`RawEvent { key, mods, pressed }`、`Shortcut { mods, key }`。鼠标键（`MouseMiddle/MouseBack/MouseForward`，即中键/侧键 MB4/MB5）可作触发键与改键 from/to。
- **解析**：`"Ctrl+Alt+K".parse::<Shortcut>()`，`+` 分隔，`Ctrl/Control/Cmd/Win`→Ctrl、`Alt/Option`→Alt、`Meta/Super`→Meta。裸修饰键名（如 `"Shift"`）按按键处理（改键场景）。
- **匹配**（`matches`）：主键一致 **且** 事件修饰键 ⊇ 快捷键修饰键——按下 `Ctrl+Shift+K` 也会命中 `Ctrl+K`（宽松规则，避免多按一个 Shift 就触发失败）。
- **配置模型**（JSON 落盘，跨平台同步介质）：
  - `Config { folders, layers, shortcuts, remaps, expansions, settings }`
  - `ShortcutItem { name?, description?, folder?, layer?, triggers, actions, enabled }`（`triggers` 任一组命中即触发，`actions` 按顺序执行；`folder` 目录分组、`layer` 归属层、`None`=基层层始终生效）
  - `Trigger`（`kada-core`）：触发键单元 = `Combo(Shortcut)` 单组合（`"Ctrl+K"`）或 `Sequence(Vec<Shortcut>)` 按键序列（`"F9 J K"` 依次按下 F9→J→K，首步即 leader 键进入等待态）；`Trigger::parse` 按空白切分（1 token=Combo、≥2=Sequence，每步用 `Shortcut::parse`，坏步整条丢弃）；纯逻辑 `SequenceTracker`（时序状态机，支持共享前缀 `"F9 J K"`/`"F9 J L"` 并存）+ `DEFAULT_SEQUENCE_TIMEOUT_MS = 1000`
  - `Action`：`Text / Command / Keys / PauseMs / Os / App / If`（`Command` 带 `shell`(Cmd/Powershell)/`show_output` 是否弹结果/`var` 非空则把标准输出写入文本变量；`Os` 文件动作、`App` 应用动作、`If` 条件判断；旧版 `Cmd`/`Powershell`/`Launch`/`CloseProgram`/`OpenFolder` 加载时自动迁移）
  - `Condition`：路径/变量/时间（`Exists/NotExists/IsFile/IsDir/Equals/NotEquals/ModifiedWithin`）+ 前台应用/窗口（`FrontmostApp/NotFrontmostApp` 按进程名、`WindowTitleContains` 按窗口标题；含 `*`/`?` 走通配否则子串匹配，不区分大小写；Windows 取前台窗口、Linux 暂受限恒不成立）
  - `Remap { from, to, tap, hold, layer, hold_layer, tap_timeout_ms, oneshot, sticky, tap2, tap3, enabled }`：普通改键用 `to`；tap-hold 用 `tap`（短按）/`hold`（长按，常设修饰键）；`hold_layer` 长按进入层（momentary 切层）；`oneshot` 单次修饰（单击 `from` 武装、应用到下一个非修饰键后自动释放）/`sticky` 粘滞修饰（单击锁定、再击解锁，值域均只 `Ctrl/Alt/Shift/Meta`）/`tap2`/`tap3` 双击/三击（tap-dance 连击不同义，缺省回落上一级）；`tap_timeout_ms` 判定阈值默认 200ms。形态互斥，运行时优先级 `sticky > oneshot > tap-hold > 普通 to`
  - `Layer { id, name }` 键位层（快捷键/改键可归属某层，仅该层激活时生效）+ `TextExpansion { trigger, replace, enabled }` 文本扩展（输入触发词+后缀自动展开，`replace` 支持 `{date}`/`{time}`/`{clipboard}`）
  - `Settings { autostart, paused, wake_key }`；`detect_conflicts` 检测硬/软冲突（序列按完整字符串判重；序列 leader 遮蔽同层单组合=硬冲突；超集软冲突仅对单组合生效）；启动显隐按来源区分（双击 exe 显示主窗口、开机自启带 `--autostart` 参数静默到托盘）
  - 变量占位符：`{变量名}` / `{变量名.字段}`，由 `substitute_vars` 替换（`Os::GetFileProps` 写 File、`App::Status` 写 Bool、命令动作 `var` 非空写 Text，Text 带退出码 `{变量名.exit_code}`，作用域=单次触发内的动作序列）；`sanitize_config` 逐条清洗坏条目（坏触发键/动作/改键单独忽略，不拖垮整份保存）
- 手工编辑的配置允许缺字段、带未知字段（`#[serde(default)]`），加载时校验。
- **消息中心**（内存态，重启清空）：Cmd/PowerShell 结果一律记入消息中心；`show_output` 开则弹结果弹窗、关则托盘图标 + 应用内「消息」入口亮红点，进入「消息」页标记已读。

## 钩子引擎

- **Windows**（`kada-hook::win`）：`SetWindowsHookEx(WH_KEYBOARD_LL + WH_MOUSE_LL)`，同一线程跑消息循环；处理函数直接跑在回调内（不做跨线程调度，保证顺序与低延迟）。修饰键用 `GetKeyState` 实时读；自动重复按同键 250ms 内再次 down 识别；`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键；注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持"按下-抬起"配对（按住原键 = 按住目标键）。鼠标钩子只翻译中键/侧键（MB4/MB5），左右键与滚轮一律放行；小键盘 Enter 与主 Enter 共用 `VK_RETURN`，靠 `LLKHF_EXTENDED` 扩展位区分。
- **Linux**（`kada-hook::linux`）：evdev + uinput（内核输入层），`EVIOCGRAB` 独占抓取 `/dev/input/event*` 键盘设备、`/dev/uinput` 建虚拟键盘 `kada-virtual-keyboard` 转发；X11 / Wayland 通用；需要 root 或 `input` 组 + udev 放开 `/dev/uinput`。修饰键按事件流维护（`MODS_DOWN`），与 Windows `GetKeyState` 语义对齐。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V`（中文等 Unicode 最稳，会短暂占用并恢复剪贴板）；组合键全部按下 → 稍停 → 逆序松开。
- **壳层决策扩展**（`src-tauri`）：`taphold_step` 状态机在 `decide` 前吞掉 tap-hold 键并延迟判定短按/长按/长按切层/单次武装/粘滞切换/连击（roll 判定：待定期间按其它键立即判 hold）；层语义=激活层条目优先、基层层条目兜底（`active_layer` 由切层键驱动、momentary）；注入修饰统一走「物理注入 + mods 补全」通道（`ModsState` 分 `hold`/`sticky`/`oneshot` 三类，合并为 `hold ∪ sticky ∪ oneshot`；oneshot 下一个非修饰键释放、sticky 再击解锁、hold 松开 `from` 释放）；连击经 `TapDanceState` 等待窗累计击数、超时懒提交；`sequence_step` 键序列状态机在 `taphold_step` 之后、`decide` 之前——leader 键吞掉进入等待、命中后 `fire` 触发、`Escape` 取消、超时（1000ms）/断链重置且断链键继续流到 `decide`；`on_hotstring` 在放行事件上累积热串缓冲、命中触发词后异步回删+注入。
- **前台上下文**（`frontmost_context`）：Windows 取 `GetForegroundWindow` 窗口标题 + 进程名（`QueryFullProcessImageNameW`）；Linux 暂返回 None（Wayland 受限），前台类条件在该平台恒不成立。
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

- `README.md`（项目简介，GitHub 展示）；`docs/README.md`（文档索引：每份文档一句话定位 + 新人阅读顺序）；`docs/架构设计.md`（分层 / 事件流转 / 关键机制 / 跨平台策略）；`docs/安装与更新-Windows.md`（Windows 版安装步骤 + 软件更新策略，选型已定待实现）；`docs/需求设计说明书.md`（功能需求唯一活文档 + 修订记录）；`docs/变量提取与引用指南.md`（变量提取/占位符引用 Q&A，含移动硬盘盘符实战）；`docs/竞品分析与优化规划.md`（竞品横向对比 + 差距清单 + P0~P3 优化路线图）；`docs/变更归档.md`（已实现变更归档，按里程碑的 文件-改动表 + 规则/决策）。
- 代码事实以 `crates/kada-core/src/lib.rs` + `crates/kada-hook/src/*.rs` + `src-tauri/src/lib.rs` 为准，如需检索先 `grep` 再动手。
- **文档规划借鉴 ihomy 与咖啡伴侣的结构化布局**：每个变更都有固定落点（规则 → 本文件；功能需求/规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`），保证多会话衔接、新会话可直接续接。

## 环境检查（参考）

本机已装：Rust toolchain、Node、Tauri CLI（`@tauri-apps/cli`）。前端依赖 `npm --prefix ui ci`；构建在 Windows 下进行。
