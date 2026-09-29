# AGENTS.md — Kada 项目规则

> 本文件供 ZCode 跨会话加载：**只留规则 + 导航索引**，目标 **≤20KB** 保证完整注入上下文（超出会被截断且尾部最先丢失）。
> 完整功能需求见 `docs/需求设计说明书.md`（活文档，未实现规划在 §7）；实现历史归档见 `docs/变更归档.md`（新归档只追加到该文件，不写回本文件）。

> **⚠ Git 规定（必须遵守）**：非人工指令，不得主动提交代码（`git commit`/`git add -A`/`git push` 一律禁止）。`git add` 只能指定具体文件路径，禁止 `git add -A`/`git add .`。归档文档走 `/archive` 技能（归档推送）。

> **⚠ 编码/路径警示（遵守以防误写）**：
> - 工作目录绝对路径：`C:\Users\chill\OneDrive\WorkStation\Projects\Kada`
> - 含中文的源码/配置/文档一律走本工具的 Read/Write/Edit 读写，禁止用 PowerShell `Get-Content`/`Set-Content`/`WriteAllText` 读写（PS 5.1 默认 GBK 会破坏 UTF-8 中文）。PowerShell 仅用于：cargo/npm 构建、运行 demo。
> - Git Bash（MSYS）调 Windows 原生命令时，POSIX 风格路径会被自动转换，涉及 Windows 工具时注意参数路径。

## 项目概述

- **咔哒 Kada**：轻快跨平台快捷键 / 改键 / 宏工具，桌面托盘常驻（关窗不退出），配置以 JSON 落盘、可跨平台同步。
- **技术栈**：Rust workspace（`kada-core` 纯逻辑 + `kada-hook` 平台钩子 + `kada-actions` 动作引擎）+ Tauri 2 桌面壳（crate `kada`）+ Vite/TypeScript UI（`kada-ui`）。无数据库 / 后端服务，纯本地。
- 运行时配置落盘在 Tauri `app_data_dir()/config.json`（`%APPDATA%\com.kada\`，不在仓库内）。

## 命名约定（务必遵守）

| 项 | 值 |
|----|----|
| workspace members | `crates/kada-core` / `crates/kada-hook` / `crates/kada-actions` / `src-tauri` |
| Tauri crate 名（`src-tauri/Cargo.toml`） | `kada` |
| npm 包名（`ui/package.json`） | `kada-ui` |
| Tauri `identifier` | `com.kada` |
| `productName` / 窗口标题 / 应用显示名 | `Kada` / `咔哒 Kada` |

- **键模型使用平台无关中性名**：`Enter/Escape`（不是 `Return`）、修饰键 `Ctrl/Alt/Shift/Meta`（macOS 的 ⌘ 由 `Meta` 承载，平台层负责把 OS 键映射成 `Key`）。键名渲染回 `"Ctrl+Alt+K"` 形式（`format_shortcut` / `key_name`）。

## 代码结构

```
crates/kada-core/           # 零重量纯逻辑：键模型、快捷键解析/匹配、配置模型、冲突检测
  src/lib.rs                # Key/Modifier/RawEvent/Shortcut + parse/format/matches + Config/Action/Trigger/sanitize
crates/kada-hook/           # 平台钩子引擎：全局键盘事件监听 + 拦截 + 注入
  src/win.rs                # Windows: WH_KEYBOARD_LL + WH_MOUSE_LL 钩子 + swallow/Replace 状态机 + key↔VK 映射 + 自愈看门狗
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate
  examples/demo.rs          # M1 冒烟 demo（仅 Windows，真人按键验证）
crates/kada-actions/        # 动作执行引擎（automation feature 门控）：run_actions/run_os/run_app/run_cmd/Script + CommandResult
  src/abort.rs              # 中止开关（代数计数器 + 运行计数 RunGuard，细节见 §3.17）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻 + 窗口懒创建（冷启动零 WebView）
  src/engine.rs             # 输入决策引擎（Tauri 无关、可单测）：tap-hold/层/和弦/键序列/热串状态机 + Inject 通道
  src/config_io.rs          # 配置读写：原子写 + .bak + 损坏留档与自愈；read_only 只读入口供监听
  src/config_watch.rs       # 配置外部修改监听（内容指纹比对），可单测
  src/update.rs             # 软件更新：启动静默检查/手动检查/下载安装状态机 + 进度节流，Tauri 无关部分可单测
  tauri.conf.json           # 窗口/图标/构建配置 + bundle.createUpdaterArtifacts + plugins.updater（公钥/端点/安装模式）
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键（含动作流程图编排）/改键/文本扩展/宏录制/消息中心/设置页；编辑草稿与 cfg 隔离 + 未保存改动守卫
  src/mock.ts               # 纯浏览器演示模式：无 Tauri 壳时注入模拟 IPC + 演示配置（dev server 直开浏览器调试）
  src/style.css             # 深色 UI 样式
.github/workflows/          # ci.yml（双平台构建+测试）；release.yml（打 v* tag 签名发版）
```

## 核心概念与铁律

> 模型逐字段定义见 `docs/需求设计说明书.md` §2（键模型）/ §3（配置模型）/ §4（钩子）/ §5（壳层）；下面只列**动手时必须遵守的约束**。

- **配置模型**：`Config { folders, layers, shortcuts, remaps, expansions, settings }`。`Key` 覆盖字母/数字/F1-F24/标点/功能键/方向键/媒体键/NumPad/NumLock/鼠标键（中键·侧键 MB4/MB5 可作触发键与改键两端）。触发键 `Trigger` = 单组合 `"Ctrl+K"` / 按键序列 `"F9 J K"` / 和弦 `"F&J"`（`&` 分隔，成员须非修饰裸键）。`Action` = Text / Command / Keys / PauseMs / Os / App / OpenUrl / If / Script。`Condition` = 路径 / 变量 / 时间 / 前台应用·窗口标题。`Remap` = 普通改键（`to`）/ tap-hold（`tap`+`hold`）/ 切层键（`hold_layer` 长按进入、`lock_layer` 长按锁定，**两者互斥、同设保留 lock**）/ 进阶修饰（`oneshot`/`sticky`/`tap2`/`tap3`，形态互斥，运行时优先级 `sticky > oneshot > tap-hold > 普通 to`，修饰值域只 `Ctrl/Alt/Shift/Meta`）。
- **吞键铁律（序列 / 和弦 / tap-hold 必须遵守）**：被吞掉的键补不回来，所以「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口内；**没组成快捷键的按键必须按用户输入顺序原样回放**——和弦成员全抬起仍未凑齐、序列断链或超时、tap-hold 两条无输出路径（短按没设 `tap`、长按到底没设 `hold`，含切层键与 momentary 层单按）。否则一旦配了 `F&J` / `F9 J K` / 只设「长按进入层」的切层键，这些键就彻底变哑。机制细节见 `docs/架构设计.md` §3.1。
- **配置落盘铁律**（`src-tauri/src/config_io.rs`）：保存必须走「唯一临时文件 → `fsync` → `rename` 覆盖」的原子替换（并留 `config.json.bak` 上次良好副本），**不要退回 `fs::write`**；加载解析失败**绝不静默清空**——留档 `config.json.corrupt-<时间戳>`、能从 `.bak` 恢复就恢复并回写主文件，都失败才以空配置启动 + 告警进消息中心。加载路径只 `migrate()` **不** `sanitize_config()`（清洗只在保存/导入路径）。**导入必须选方式**（`import_config(path, mode)`，合并见 `kada-core/src/merge.rs`）：`merge` 只增不删、不制造新冲突、**设置保留本机**；`replace` 覆盖前必须 `archive_copy` 留档 `pre-import-<时间戳>`。**删除已保存条目要先确认**（`ui/` `confirmDelete`），未落盘的新增项可以不问。**外部修改监听**（`config_watch.rs`，每秒比内容指纹）：**能解析才采纳、解析不动就别碰磁盘**（自愈只属启动 `load`，监听走只读 `read_only`）；写盘与 `adopt_own_write` 同持 `cfg_watch` 锁；界面收 `config-changed` 有未保存草稿必须问用户。
- **层语义**：激活层条目优先、基础层条目兜底（不归属任何层的条目始终生效）；**momentary 层只在 roll 或按住切层键期间按别的键时真正进入**。
- **冲突检测**（`detect_conflicts`）：硬冲突（重复触发键 / 序列 leader 遮蔽同层单组合）/ 软冲突（超集）/ 系统快捷键清单命中 / **层可达性**（层里有启用条目却没有任何切层键指向它 → 警告「层内条目永远不会生效」，这是「层配了但不触发」最难自查的原因）。
- **消息中心**（内存态，重启清空，「命令结果 / 冲突」两标签）：命令/脚本结果记入，**中止/超时记入**（`kind="abort"`）、**动作失败也记入**（`kind="error"`，标签带「失败」+ 动作摘要）、**配置事件也记入**（`kind="config"`）；`show_output` 开则弹结果弹窗，否则托盘红点；进「消息」页标记已读。
- **动作执行有界**（`kada-actions/`，详见 `需求设计说明书.md` §5.7）：命令/脚本**不许用 `Command::output()` 无限等**（挂住即永久占住该触发的执行线程）；走 `run_with_limits`（管道**边读边等** + 20ms 轮询 `try_wait`），超时（`settings.action_timeout_ms`，默认 30 秒、0=不限）/ 中止即 `taskkill /T /F` **杀整棵进程树**（`Child::kill` 杀不掉孙进程；Linux 只 `kill()` 为已知缺口）。中止 = **代数计数器** `abort::request()`（判定点：每步动作前 / 进程轮询 / `PauseMs` 分片 / `App::Status` 重试），`abort_actions` 回报正在执行数。
- **feature 门控**（`automation`，三个 crate 均 `default` 开启）：关掉（`--no-default-features`）得**基础版** = 改键全形态 / 层 / 序列 / 和弦 / 文本扩展 / Text·Keys·PauseMs 注入，裁掉 Command / Os / App / OpenUrl / If / Script / Condition / Vars。
- 手工编辑的配置允许缺字段、带未知字段（`#[serde(default)]`）。

## 钩子引擎（动手前必读的约束）

- **Windows**（`kada-hook::win`）：`SetWindowsHookEx(WH_KEYBOARD_LL + WH_MOUSE_LL)` 同一线程跑消息循环，处理函数直接跑在回调内（不跨线程调度，保证顺序与低延迟——**回调里绝不能做耗时操作**，超时会被系统摘掉钩子，之后全部功能静默失效）。自动重复按「该键已按下且未抬起」判定（`HELD_KEYS` 集合；**不用时间窗**——同键快速连打是正常输入，会被误判成重复丢掉第二击 → 热串缓冲缺字 / 连击不计击数）。`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键，**但所有 keyup（含被吞掉的，键盘与鼠标一致）仍以观察者身份回调 handler**——状态机靠抬起维护「按住集合」（和弦成员、tap-hold 短长按判定），漏掉会让该键与整个状态机永久卡死。注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持「按下-抬起」配对。鼠标钩子只翻译中键/侧键，左右键与滚轮放行；小键盘 Enter 与主 Enter 共用 `VK_RETURN`，靠 `LLKHF_EXTENDED` 扩展位区分。
- **回调里不许建窗口 / 碰 IO 网络**：唤窗（主窗口 / 气泡 / 状态指示）、动作执行、注入一律 `std::thread::spawn`，回调里只留判定（读配置、比时间、状态机）——同步建 WebView 会拖过系统约 300ms 的超时线，钩子被摘掉后全部功能静默失效（已踩两次，机制见 `docs/架构设计.md` §3.6）。判定与副作用分开写：判定用不持有 `AppHandle` 的纯函数。
- **透明浮窗（`toast` / `hud`）必须 `focusable(false)`**（Windows = `WS_EX_NOACTIVATE`）：`focused(false)` 只保证**首次** show 不抢焦点（tao 之后清标记、退回 `SW_SHOW`），而这两个窗会反复 show。一旦能被激活就成了 `GetForegroundWindow`——前台条件全读错，`ResetWatch` 还会误判「前台切换」→ 复位输入状态、踢掉按住的 momentary 层。
- **Linux**（`kada-hook::linux`）：evdev + uinput，`EVIOCGRAB` 独占抓取 `/dev/input/event*` + `/dev/uinput` 建虚拟键盘转发（X11 / Wayland 通用）；需 root 或 `input` 组 + udev 放行 `/dev/uinput`。修饰键按事件流维护（`MODS_DOWN`，与 Windows `GetKeyState` 语义对齐）；自动重复直接用 evdev 的 `value == 2`。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V`（中文最稳，会短暂占用并恢复剪贴板）；组合键「全部按下 → 稍停 → 逆序松开」。**动作执行前必须 `wait_modifiers_released`**（≤300ms 轮询 `GetKeyState`）等物理修饰键释放——触发带修饰键的快捷键时修饰键仍按住，直接注入会被污染成 `Ctrl+Alt+V` 而输不出文本（Linux 独占抓取 + 独立虚拟设备，同接口空实现）。
- **状态机集中在 `src-tauri/src/engine.rs`**（Tauri 无关、走 `Inject` trait 假注入可单测，`lib.rs` 只剩接线）：顺序为 `taphold_step`（tap-hold / 切层 / oneshot / sticky / 连击）→ `chord_step` → `sequence_step` → `decide` → `hotstring`。**超时类等待态必须由定时器线程落地**（`spawn_engine_ticker` 每 `ENGINE_TICK_MS=60ms` 调 `Engine::tick`：序列 leader 超时回放、和弦等待窗超时回放、连击等待窗提交，事件路径上另有懒判定兜底；两个窗口值取 `Settings.sequence_timeout_ms`/`chord_timeout_ms`（0=不限时），故 `tick` 传 `cfg`——**配置读锁必须先于 `Engine` 锁**，与钩子回调同序否则死锁）；只靠懒判定等于该键按了没反应。热串命中后吞掉后缀键，由**后台线程**回删触发词 + 注入 + 补回后缀（注入上百毫秒，绝不能跑在钩子回调里）。输入状态指示也由这条线程每拍比一次快照推送（见「透明浮窗」条）。
- **录入暂停是限时租约**：`set_paused(true)` 只暂停 `PAUSE_LEASE_MS`（60 秒）后自动失效——录入捕获被中断时前端可能来不及解除，布尔量会永久卡在暂停态让整个应用静默失效（快捷键/改键/文本扩展全无响应）；前端在失焦/隐藏/关详情时也主动解除。
- **自愈看门狗**（仅 Windows，`win.rs`）：低层钩子会**静默失效**（回调超时被摘除 / 休眠唤醒 / 解锁后再收不到事件，而托盘看着还活着）。`start` 另起 1s 巡检线程，三条触发路径（心跳 `LAST_HOOK_TICK` 与 `GetLastInputInfo` 对照、`GetTickCount` 间隔 >10s 判唤醒、`WTSInfoEx.SessionFlags` 判解锁——**反直觉字段：0=锁定、1=未锁定**；宽限与阈值细节见 §3.15）。命中后只投递 `WM_APP_REINSTALL`——**`SetWindowsHookEx` 必须在有消息循环的钩子线程上调用**、**必须先卸旧再装新**（不卸旧 = 每个按键被处理两遍），并**一并复位 `SWALLOWED`/`REPLACED_DOWN`/`HELD_KEYS`**（已注入的目标键补一个 up）。
- **输入状态复位**（`Engine::reset`，仅 Windows 有触发源，详见 `需求设计说明书.md` §5.1）：钩子重装 / 前台切换 / **配置整份换掉**（外部修改）= 「与物理键盘断过一次线」，靠「按键抬起」或时间窗终止的等待态会永远等不到终止。口径：**注入的收回来**（补 up + 退出 momentary 层）、**缓冲里的丢弃不回放**、**锁定层保留**。通道在状态机定时器线程上：`reinstall_count()` 变化 = 钩子已重装（不在钩子线程上跑壳层代码）；`foreground_window()` + `FocusTracker` **连续两轮同一新窗口**才算切换；配置走 `cfg_stale` 旗标。Linux 恒不触发。
- **前台上下文**（`frontmost_context`）：Windows 取 `GetForegroundWindow` 窗口标题 + 进程名；Linux 暂返回 `None`（Wayland 受限），前台类条件在该平台恒不成立。
- **已知天花板（升级路径）**：低层钩子拦不住 UAC 提权进程 / 部分游戏 → 驱动级拦截；Linux 热插拔键盘不在监听列表（重启应用即可）。

## 软件更新（Windows 已落地）

- **信任链**：`tauri-plugin-updater` + GitHub Releases + Ed25519 签名；端点 `https://github.com/chillyice/Kada/releases/latest/download/latest.json`（只解析**已发布**的 Release，故流水线默认发 draft）。**公钥内嵌 `plugins.updater.pubkey` 随仓库，私钥只在本机 `C:\Users\chill\.tauri\kada.key` 与 CI Secret（`TAURI_SIGNING_PRIVATE_KEY`），绝不入库**——私钥丢失 = 现有用户再也收不到更新，只能轮换公钥 + 让人手动装一次新版。`latest.json` 由 `tauri-action` 生成，**不要手写、不要入库**。
- **三种触发的打扰等级刻意不同**（`update.rs`，详见 §5.5）：启动后台检查静默（**只在发现新版本**时弹一次气泡）；托盘「检查更新」与设置页按钮（有无更新都弹窗，发现新版先确认再装）；设置页「下载并安装」（点击即确认）。**Windows 上「安装」会退出应用**（NSIS `passive` + `/R` 装完自动拉起新版）——所以启动检查只提示、不自动装。
- **状态机在后端**（`update-status` 事件推送 + `get_update_status` 兜底拉一次），前端只渲染、不维护瞬时态；下载进度**只在整数百分比变化时推事件**。更新逻辑全在 Rust（`UpdaterExt`），前端不直接调 → capabilities **不开** `updater:*`。
- 发版步骤、Secrets 配置、`requireSignedVersion` 加固、NSIS 安装模式见 `docs/安装与更新-Windows.md` §2。

## 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试（kada-core/kada-hook/kada-actions/src-tauri）
cargo check -p kada         # 验证 Tauri 壳编译（完整版，default features）
cargo check -p kada --no-default-features  # 基础版编译（裁掉 automation 自动化动作）
npm --prefix ui run build   # 前端构建（tsc + vite build）
cargo run -p kada-hook --example demo   # M1 冒烟 demo（仅 Windows）
```

- **打包**：产物在**仓库根** `target/release/bundle/`（workspace 的 `target/` 不在 `src-tauri/` 下）；**打包前先退出运行中的 Kada**（exe 被占用报「拒绝访问」，且 bundler 写「安装包类型」标记会失败）。发版要签名（比 `--no-bundle` 慢得多，日常打包别用；本机拉不到 NSIS 工具链，发版走 CI）：
  `$env:TAURI_SIGNING_PRIVATE_KEY_PATH="C:\Users\chill\.tauri\kada.key"; npx tauri build --bundles nsis` → 出 `Kada_<版本>_x64-setup.exe` 与 `.sig`（**缺签名则客户端拒装，是设计使然**）。`npx tauri bundle --bundles nsis` 可对已编译二进制单独出包。
- **CI**：`.github/workflows/ci.yml`（ubuntu + windows：前端构建 + `cargo test --workspace` + `cargo check -p kada`）；`release.yml`（打 `v*` tag 或手动触发）。
- 平台相关代码用 `#[cfg(windows)]` / `#[cfg(target_os = "linux")]` 门控；macOS 壳仍可编译运行（占位类型），但改键/快捷键/录制不可用。

## 里程碑与规划

M0–M4 已完成（工程骨架 / 键模型+Windows 钩子 / 配置模型与动作 / Tauri 壳+UI / Linux 钩子）；**M5 macOS 输入层规划中**。

- **完整里程碑进度与全部未实现规划（P0~P3 唯一权威清单）见 `docs/需求设计说明书.md` §7**——立项、排期、勾进度一律以那里为准（已落地项保留原编号就地标 ✅）；实现历史见 `docs/变更归档.md`。

## 文档清单

- `README.md`（简介）；`docs/README.md`（索引 + 阅读顺序）；`docs/架构设计.md`（分层 / 事件流 / 机制）；`docs/需求设计说明书.md`（功能需求 + 规划 §7）；`docs/安装与更新-Windows.md`（安装 / 更新 / 密钥 / 发版）；`docs/变量提取与引用指南.md`（变量 Q&A）；`docs/竞品分析与优化规划.md`；`docs/人工验证清单.md`（验收步骤 + 状态）；`docs/变更归档.md`（已实现变更 + 决策）。
- **落点分工**：规则/约定/命名 → 本文件；功能需求与规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`。代码事实以源码为准，先 `grep` 再动手。
- 本机已装：Rust toolchain、Node、Tauri CLI（`@tauri-apps/cli`）。前端依赖 `npm --prefix ui ci`；构建在 Windows 下进行。
