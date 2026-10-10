# AGENTS.md — Kada 项目规则

> 本文件供 ZCode 跨会话加载：**只留规则 + 导航索引**，目标 **≤20KB** 保证完整注入上下文（超出会被截断且尾部最先丢失）。
> 完整功能需求见 `docs/需求设计说明书.md`（活文档，未实现规划在 §7）；实现历史归档见 `docs/变更归档.md`（新归档只追加到该文件，不写回本文件）。

> **⚠ Git 规定（必须遵守）**：非人工指令，不得主动提交代码（`git commit`/`git add -A`/`git push` 一律禁止）。`git add` 只能指定具体文件路径，禁止 `git add -A`/`git add .`。归档文档走 `/archive` 技能（归档推送）。

> **⚠ 编码/路径警示（遵守以防误写）**：工作目录 `C:\Users\chill\OneDrive\WorkStation\Projects\Kada`。含中文的源码/配置/文档一律走本工具的 Read/Write/Edit 读写，**禁止用 PowerShell `Get-Content`/`Set-Content` 读写**（PS 5.1 默认 GBK 会破坏 UTF-8 中文；PS 仅用于 cargo/npm 构建与运行 demo）。Git Bash 调 Windows 原生命令时 POSIX 路径会被自动转换，注意参数。

## 项目概述

- **咔哒 Kada**：轻快跨平台快捷键 / 改键 / 宏工具，托盘常驻（关窗不退出），配置 JSON 落盘、可跨平台同步。
- **技术栈**：Rust workspace（`kada-core` 纯逻辑 + `kada-hook` 平台钩子 + `kada-actions` 动作引擎）+ Tauri 2 桌面壳（crate `kada`）+ Vite/TypeScript UI（`kada-ui`）。配置落盘在 Tauri `app_data_dir()/config.json`（`%APPDATA%\com.kada\`，不在仓库内）。

## 命名约定（务必遵守）

| 项 | 值 |
|----|----|
| workspace members | `crates/kada-core` / `crates/kada-hook` / `crates/kada-actions` / `src-tauri` |
| Tauri crate 名（`src-tauri/Cargo.toml`） | `kada` |
| npm 包名（`ui/package.json`） | `kada-ui` |
| Tauri `identifier` | `com.kada` |
| `productName` / 窗口标题 / 应用显示名 | `Kada` / `咔哒 Kada` |

- **键模型使用平台无关中性名**：`Enter/Escape`（不是 `Return`）、修饰键 `Ctrl/Alt/Shift/Meta`（macOS 的 ⌘ 由 `Meta` 承载，平台层负责把 OS 键映射成 `Key`）。键名渲染回 `"Ctrl+Alt+K"` 形式。

## 代码结构

```
crates/kada-core/           # 零重量纯逻辑：键模型、快捷键解析/匹配、配置模型、冲突检测、片段摘取
  src/lib.rs                # Key/Modifier/RawEvent/Shortcut + parse/format/matches + Config/Action/Trigger/sanitize
  src/merge.rs              # 配置合并（导入的「合并」方式：只增不删、不造新冲突、设置保留本机）
  src/snippet.rs            # 配置片段（分享用）：按「类别+下标」摘出条目 + 被引用的层，见下方铁律
crates/kada-hook/           # 平台钩子引擎：全局键盘事件监听 + 拦截 + 注入
  src/win.rs                # Windows: WH_KEYBOARD_LL + WH_MOUSE_LL + swallow/Replace 状态机 + key↔VK + 自愈看门狗
  src/win/simulate.rs       # Windows: SendInput 注入 + 剪贴板文本
  src/linux.rs              # Linux: evdev + uinput 钩子 + 键码映射 + simulate
  src/macos.rs              # macOS: CGEventTap 钩子 + key↔虚拟键码 + AX 前台查询 + simulate（需「辅助功能」权限）
  examples/demo.rs          # M1 冒烟 demo（三平台，真人按键验证）
crates/kada-actions/        # 动作执行引擎（automation feature 门控）：run_actions/run_os/run_app/run_cmd/Script + CommandResult
  src/abort.rs              # 中止开关（代数计数器 + 运行计数 RunGuard，细节见 §3.17）
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻 + 窗口懒创建
  src/engine.rs             # 输入决策引擎（Tauri 无关、可单测）：tap-hold/层/和弦/键序列/热串状态机 + Inject 通道
  src/config_io.rs          # 配置读写：原子写 + .bak + 损坏留档与自愈；read_only 只读入口供监听
  src/config_watch.rs       # 配置外部修改监听（内容指纹比对），可单测
  src/update.rs             # 软件更新：检查/下载/安装状态机 + 进度节流，Tauri 无关部分可单测
  tauri.conf.json           # 窗口/图标/构建配置 + bundle.createUpdaterArtifacts + plugins.updater（公钥/端点）
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键（动作流程图编排）/改键/文本扩展/宏录制/消息中心/设置页；草稿隔离 + 未保存守卫
  src/mock.ts               # 纯浏览器演示模式：无 Tauri 壳时注入模拟 IPC + 演示配置（含各类事件桩，可直开浏览器调试）
  src/style.css             # 深色 UI 样式
.github/workflows/          # ci.yml（三平台构建+测试）；release.yml（打 v* tag 签名发版）
```

## 核心概念与铁律

> 模型逐字段定义见 `docs/需求设计说明书.md` §2（键模型）/ §3（配置模型）/ §4（钩子）/ §5（壳层）；下面只列**动手时必须遵守的约束**。

- **配置模型**：`Config { folders, layers, shortcuts, remaps, expansions, settings }`；`Key` / `Trigger` / `Action` / `Condition` 的值域与逐字段定义见 `docs/需求设计说明书.md` §2 / §3，这里只留易踩的约束。`Remap` = 普通 `to` / tap-hold（`tap`+`hold`）/ 切层键（`hold_layer` 长按进入、`lock_layer` 长按锁定，**互斥、同设保留 lock**）/ 进阶修饰（`oneshot`/`sticky`/`tap2`/`tap3`，形态互斥，优先级 `sticky > oneshot > tap-hold > 普通 to`，单次/粘滞值域任意键）。`ShortcutItem`/`Remap` 均可带触发侧条件门控 `when`——不成立等价于「该条不存在」（落到下一条同触发键条目、不吞键），判重指纹含 `when`（同触发键不同条件不算重复）。
- **吞键铁律（序列 / 和弦 / tap-hold 必须遵守）**：被吞掉的键补不回来，「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口内；**没组成快捷键的按键必须按用户输入顺序原样回放**——和弦成员全抬起仍未凑齐、序列断链或超时、tap-hold 两条无输出路径（短按没设 `tap`、长按到底没设 `hold`，含切层键与 momentary 层单按）。否则配了 `F&J` / `F9 J K` / 只设「长按进入层」的切层键，这些键就彻底变哑。机制见 `docs/架构设计.md` §3.1。
- **配置落盘铁律**（`src-tauri/src/config_io.rs`）：保存走「唯一临时文件 → `fsync` → `rename` 覆盖」原子替换（留 `config.json.bak` 上次良好副本），**不要退回 `fs::write`**；加载解析失败**绝不静默清空**——留档、能从 `.bak` 恢复就恢复并回写主文件，都失败才空配置启动 + 告警。加载只 `migrate()` **不** `sanitize_config()`（清洗只在保存/导入路径）。**导入必须选方式**（`import_config(path, mode)`）：`merge` 只增不删、不制造新冲突、**设置保留本机**；`replace` 覆盖前必须 `archive_copy` 留档。**删除已保存条目要先确认**（`ui/` `confirmDelete`），未落盘新增项可不问。**外部修改监听**（`config_watch.rs`）：**能解析才采纳、解析不动就别碰磁盘**（自愈只属启动 `load`，监听走只读 `read_only`）；写盘与 `adopt_own_write` 同持 `cfg_watch` 锁；界面收 `config-changed` 有未保存草稿必须问用户。**应用内写盘广播 `config-updated`**（`set_config` / 导入 / `save_settings_patch`）与 `config-changed`（外部改动）语义分离，不可混用。
- **层语义**：激活层条目优先、基础层条目兜底（不归属任何层的条目始终生效）；**momentary 层只在 roll 或按住切层键期间按别的键时真正进入**。
- **平台能力缺失**（`PlatformCaps`，见钩子引擎段）与**层可达性**（层有条目却无切层键指向 → 警告）同属软冲突。
- **消息中心**（内存态，重启清空）：命令/脚本结果、**中止/超时**（`kind="abort"`）、**动作失败**（`kind="error"`）、**配置事件**（`kind="config"`）都记入；`show_output` 开则弹结果弹窗，否则托盘红点；进「消息」页标记已读。**定长上限**：200 条、单条 stdout/stderr 各 16KB（`MAX_RESULTS` / `truncate_text`）。
- **动作执行有界**（`kada-actions/`，机制见 `架构设计.md` §3.17 / `需求设计说明书.md` §5.7）：命令/脚本**不许用 `Command::output()` 无限等**——走 `run_with_limits` 边读边等，超时（`settings.action_timeout_ms`，默认 30 秒、0=不限）/ 中止即**杀整棵进程树**（Windows `taskkill /T /F`；Linux / macOS 自成进程组后 `kill(-pgid)`）；中止 = **代数计数器** `abort::request()`。**并行动作** `Parallel`（7.1-㉞）= 成员各起一线程、全部结束才进下一步，用**链内注入锁**维持「注入串行」（`Text`/`Keys`/`Mouse` 持锁，只并发非注入步骤），成员 `vars` 私有快照、按下标顺序合并与提交。
- **配置片段（分享）铁律**（`crates/kada-core/src/snippet.rs` + `export_fragment`）：片段**必须仍是合法的 `Config` JSON**（顶层可多一个被忽略的 `kada_snippet` 元信息键）——对方靠现成的「导入 → 合并」吃下，**不要为片段另立一套导入格式 / 解析器**。摘取时**剥掉 `folder`**（本机 UUID，带过去只会指向不存在的目录）、**带上被引用的层**（否则导入时层引用被清成基础层 = 静默改行为）、**不带 `settings`**（合并本就不碰它）。
- **feature 门控**（`automation`，三个 crate 均 `default` 开启）：关掉（`--no-default-features`）得**基础版** = 改键全形态 / 层 / 序列 / 和弦 / 文本扩展 / Text·Keys·Mouse·PauseMs 注入，裁掉 Command / Os / App / OpenUrl / If / Script / Condition / Vars。手工编辑的配置允许缺字段、带未知字段（`#[serde(default)]`）。
- **前端键表须与 core 同源校验**：`ui/src/main.ts` 的 `KEY_OPTIONS` 须与 `kada-core` 的 `Key::ALL` 一致且**保持显式字面量**（壳测试 `frontend_key_table_matches_core` 比对，Rust 新增键而前端漏改即红）。

## 钩子引擎（动手前必读的约束）

- **Windows**（`kada-hook::win`）：`SetWindowsHookEx(WH_KEYBOARD_LL + WH_MOUSE_LL)` 同一线程跑消息循环，处理函数直接跑在回调内（**回调里绝不能做耗时操作**，超时会被系统摘掉钩子、全部功能静默失效）。自动重复按「该键已按下且未抬起」判定（`HELD_KEYS`；**不用时间窗**）。`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键，**但所有 keyup（含被吞掉的）仍以观察者身份回调 handler**。注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持「按下-抬起」配对；`Replace`/`Keys` 注入走**扫描码**（按前台布局换算，媒体/音量键保留 VK，见 `需求设计说明书.md` §4.1）。鼠标钩子只翻译中键/侧键，且**按需安装**——配置引用鼠标键（触发/改键来源）才挂（`set_mouse_enabled` + 壳层 `config_uses_mouse`），新增鼠标键用途要同步该判定。
- **macOS**（`kada-hook::macos`，机制见 `架构设计.md` §3.19 / `需求设计说明书.md` §4.3）：`CGEventTap` 挂独立线程的 CFRunLoop，处理函数跑在回调内（**回调里绝不能做耗时操作**，同 Windows）。三条硬约束：① tap 位置必须会话级 `kCGSessionEventTap`（HID 位置普通用户拿 NULL，见 §4.3）；② 「辅助功能」权限是**运行时授权**（`hooks_supported()` = `AXIsProcessTrusted()`），未授权时 `start` 报授权指引 + 弹授权框；③ 注入识别两条判据取或（Private 事件源 ∪ 创建者 pid == 本进程）。自愈 = 系统停用回调当场重启用 + 1s 看门狗。
- **输入层起不来的处置**：Windows 失败退出（异常）；macOS / Linux（权限没配好）**照常启动**，`start` 的错误串（带可照抄指引）经消息中心推给用户——退出应用用户就连设置页都进不去。
- **回调里不许建窗口 / 碰 IO 网络**：唤窗（主窗口 / 气泡 / 状态指示）、动作执行、注入一律 `std::thread::spawn`，回调里只留判定（读配置、比时间、状态机）——同步建 WebView 会拖过系统约 300ms 的超时线，钩子被摘掉后全部功能静默失效（机制见 `docs/架构设计.md` §3.6）。判定与副作用分开写：判定用不持有 `AppHandle` 的纯函数。
- **透明浮窗（`toast` / `hud` / `hints`）必须 `focusable(false)`**：`focused(false)` 只保证**首次** show 不抢焦点，而这些窗会反复 show；一旦能被激活就成了 `GetForegroundWindow`——前台条件全读错，`ResetWatch` 还会误判「前台切换」→ 复位输入状态、踢掉按住的 momentary 层。**`hints`（快捷键提示框，7.1-㊳）是唯一可交互浮窗——不能 `set_ignore_cursor_events`**（靠鼠标事件，`WS_EX_NOACTIVATE` 只挡激活不挡鼠标消息），且 `zoom_hotkeys_enabled(false)` 防 Ctrl+wheel 被当成浏览器缩放；**一律 `shadow(false)`**（投影交 CSS，原因见 `变更归档.md` V0.39 条）。
- **Linux**（`kada-hook::linux`，细节见 §4.2）：evdev + uinput，`EVIOCGRAB` 抓键盘与**真鼠标**（触摸板/指点杆**不抓**），`/dev/uinput` 建键鼠合一虚拟设备（X11 / Wayland 通用）；鼠标键 `BTN_*`↔中键/侧键与键盘同走状态机，移动/滚轮原样转发。权限 = `input` 组 + udev 放行 uinput（起不来错误分诊见 §4.2）。**设备拔出就地复位三张表**并递增 `reinstall_count()`（壳层 `ResetWatch` 轮询接通）；热插拔仍无（重启应用）。修饰键按事件流维护（`MODS_DOWN`）、自动重复 `value==2`。**改完 Linux 分支必跑**：`cargo check -p kada-hook -p kada-actions --target x86_64-unknown-linux-gnu --all-targets`（evdev 纯 Rust，本机可交叉类型检查）。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V` / macOS 的 ⌘V（中文最稳）；组合键「全部按下 → 稍停 → 逆序松开」。**动作执行前必须 `wait_modifiers_released`**（≤300ms 轮询）等物理修饰键释放——否则仍按住的触发修饰键会把注入污染成 `Ctrl+Alt+V`（Linux 空实现；macOS 投递到 HID 位置不需要 root）。
- **状态机集中在 `src-tauri/src/engine.rs`**（Tauri 无关、走 `Inject` trait 假注入可单测，`lib.rs` 只剩接线）：顺序为 `taphold_step`（tap-hold / 切层 / oneshot / sticky / 连击）→ `chord_step` → `sequence_step` → `decide` → `hotstring`。**超时类等待态必须由定时器线程落地**（`spawn_engine_ticker` 每 60ms 调 `Engine::tick` 落地超时回放，事件路径另有懒判定兜底；`tick` 传 `cfg`——**配置读锁必须先于 `Engine` 锁**，同序否则死锁）。热串命中后吞掉后缀键，由**后台线程**回删 + 注入 + 补回后缀。输入状态指示也由这条线程每拍比快照推送。
- **录入暂停是限时租约**：`set_paused(true)` 只暂停 60 秒（`PAUSE_LEASE_MS`）后自动失效——前端被中断时可能来不及解除，布尔量永久卡在暂停态会让整个应用静默失效；前端失焦/隐藏/关详情时也主动解除。
- **自愈看门狗**（`win.rs` 与 `macos.rs` 各一套，机制见 `架构设计.md` §3.15）：低层钩子/tap 会**静默失效**（回调超时被摘 / 休眠唤醒 / 解锁，见 §3.15）。`start` 另起 1s 巡检线程 + 指数退避；Windows 命中后只投递 `WM_APP_REINSTALL`——**`SetWindowsHookEx` 必须在有消息循环的钩子线程上调用**、**必须先卸旧再装新**（不卸旧 = 按键被处理两遍），并复位 `SWALLOWED`/`REPLACED_DOWN`/`HELD_KEYS`（已注入目标键补 up）；解锁判据 `WTSInfoEx.SessionFlags` **反直觉：0=锁定、1=未锁定**。
- **输入状态复位**（`Engine::reset`，三平台，机制见 `架构设计.md` §3.16）：钩子重装 / 设备拔出 / 前台切换 / **配置整份换掉** = 「与物理键盘断过一次线」。口径：**注入的收回来**（补 up + 退出 momentary 层）、**缓冲里的丢弃不回放**、**锁定层保留**。通道在状态机定时器线程：`reinstall_count()` 变化（Linux 同通道计设备拔出）、`foreground_window()` 连续两轮同一新窗口（Linux 无这路）、配置走 `cfg_stale` 旗标。
- **前台上下文**（`frontmost_context`）：Windows 取 `GetForegroundWindow`，macOS 走 AX；Linux 恒 `None`（Wayland），前台条件恒不成立。**设备上下文**（`current_device`）：Linux evdev 设备名真生效、Windows/macOS 恒 `None`（设备条件恒走「否则」）。带 `when` 的条目判定用 `TriggerContext`——前台**惰性查询**（只在该条真被判定时才查，别让每次按键买单）。**`get_conflicts` 按 `PlatformCaps` 标出平台能力缺失**（`Warn` 不拦保存），改钩子能力时同步 `platform_caps()` 填表。
- **已知天花板（升级路径）**：逐条清单与升级项编号见 `docs/需求设计说明书.md` §4.5 与 §7.2（低层钩子拦不住 UAC 提权进程 / Linux 热插拔 / macOS 安全输入等）；触摸板 / 指点杆的中键·侧键不能作触发键是物理限制（不进 §7.2）。

## 软件更新（Windows 已落地）

- **信任链**：`tauri-plugin-updater` + GitHub Releases + Ed25519 签名；**公钥内嵌 `plugins.updater.pubkey` 随仓库，私钥只在本机 `C:\Users\chill\.tauri\kada.key` 与 CI Secret（`TAURI_SIGNING_PRIVATE_KEY`），绝不入库**。端点只解析**已发布**的 Release（流水线默认发 draft）；`latest.json` 由 `tauri-action` 生成，**不要手写、不要入库**。**已开 `requireSignedVersion`**——发版 CLI 必须把版本号写进签名 `trusted comment`，否则客户端以 `MissingSignedVersion` 拒装。
- **行为口径**（`update.rs`，详见 §5.5）：**Windows 上「安装」会退出应用**，故启动检查只提示不自动装。状态机在后端（`update-status` 推送 + `get_update_status` 兜底），前端只渲染；更新逻辑全在 Rust → capabilities **不开** `updater:*`。发版步骤见 `docs/安装与更新-Windows.md` §2。

## 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试（kada-core/kada-hook/kada-actions/src-tauri）
cargo check -p kada         # Tauri 壳编译（完整版）
cargo check -p kada --no-default-features  # 基础版编译（裁掉 automation）
npm --prefix ui run build   # 前端构建（tsc + vite build）
cargo run -p kada-hook --example demo   # 冒烟 demo（三平台真人按键）
cargo check -p kada-hook --target aarch64-apple-darwin --all-targets  # macOS 侧唯一可用的本地验证（见下）
```

- **打包**：产物在**仓库根** `target/release/bundle/`；**打包前先退出运行中的 Kada**。须签名（缺则拒装）：`npx tauri build --bundles nsis` + env `TAURI_SIGNING_PRIVATE_KEY`=密钥**文件全文**、密码空串（PS 传不了空串，见 `docs/安装与更新-Windows.md` §2.3）。**两条渠道发的不是同一个文件**：GitHub Releases 发 **NSIS 包**（自动更新用），ihomy 发布页 `https://ihomy.top/kada` 发**裸 `kada.exe`**——流程见 `安装与更新-Windows.md` §1.2。
- **CI**：`.github/workflows/ci.yml`（ubuntu + windows + **macos**：前端构建 + `cargo test --workspace` + `cargo check -p kada` 两版）；`release.yml`（打 `v*` tag 或手动触发）。平台代码用 `#[cfg]` 门控，三后端暴露同一套接口（清单见 `需求设计说明书.md` §4），壳层与动作层不为平台分叉。**macOS 本地验证边界**：darwin 交叉 check 只能真编 `kada-hook`（`-p kada` 必失败，`ring` 要真 clang）——macOS 壳层分支只能靠 CI / 真机验。
## 里程碑与规划

M0–M5 已完成（详见 §7）。

- **未实现规划唯一清单 + 已落地事项编号索引见 `docs/需求设计说明书.md` §7**——立项、排期一律以那里为准（已落地项收拢为编号索引，功能闭环写在 §1~§6，勿重复立项）；实现历史见 `docs/变更归档.md`。

## 文档清单

- `README.md`（简介）；`docs/README.md`（索引 + 阅读顺序 + 全清单）。
- **落点分工**：规则/约定/命名 → 本文件；功能需求与规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`。代码事实以源码为准，先 `grep` 再动手。本机已装 Rust toolchain、Node、Tauri CLI；前端依赖 `npm --prefix ui ci`。
- **`需求设计说明书.md` 按功能模块组织，不按版本**：顶部是「文档导航（按功能模块）」的 16 模块索引表（不是版本时间线）；**版本号只写进 `变更归档.md`**，说明书正文提到某功能时引**模块小节号**（`§5.7 动作执行边界`）而不是版本号。§N 是跨文档引用的稳定 ID（约 420 处），**只增不改**——重编号要连带重写六份文档。新功能写进所属模块小节，不新开「XX 版本改动」章节。

