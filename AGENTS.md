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
crates/kada-actions/        # 动作执行引擎（automation feature 门控）：run_actions/run_os/run_app/run_cmd/Script + CommandResult（从壳迁出，不依赖 Tauri）
  src/lib.rs                # 触发 → 执行动作串的执行逻辑（命令/文件/应用/条件/脚本）+ 消息中心结果结构
src-tauri/                  # Tauri 2 桌面壳（crate "kada"）
  src/lib.rs                # KadaState/decide/Recorder/消息中心/tauri commands/托盘常驻；动作执行调 kada_actions::run_actions；主窗口与气泡窗口按需懒创建（冷启动零 WebView）
  src/engine.rs             # 输入决策引擎（Tauri 无关、可单测）：tap-hold/层/和弦/键序列/热串状态机 + Inject 注入通道
  src/update.rs             # 软件更新（Tauri updater + GitHub Releases + Ed25519 签名）：启动静默检查/手动检查/下载安装状态机 + 进度节流，Tauri 无关部分可单测
  src/main.rs               # 入口
  tauri.conf.json           # 窗口/图标/构建配置 + bundle.createUpdaterArtifacts + plugins.updater（公钥/更新端点/安装模式）
ui/                         # Vite + TypeScript 前端（kada-ui）
  src/main.ts               # 配置界面：快捷键/改键/宏录制/消息中心/设置页软件更新
  src/style.css             # 深色 UI 样式
.github/workflows/ci.yml    # CI：ubuntu + windows 双平台构建 + 测试
.github/workflows/release.yml  # 发版：打 v* tag 触发，tauri-action 构建→签名→生成 latest.json→draft Release
```

## 键模型与配置（核心概念）

- **键模型**（`kada-core`）：`Key`（字母/数字/F1-F24/标点/功能键/方向键/媒体键/NumPad 区/NumLock/鼠标键）、`Modifier`（Ctrl/Alt/Shift/Meta）、`RawEvent { key, mods, pressed }`、`Shortcut { mods, key }`。鼠标键（`MouseMiddle/MouseBack/MouseForward`，即中键/侧键 MB4/MB5）可作触发键与改键 from/to。
- **解析**：`"Ctrl+Alt+K".parse::<Shortcut>()`，`+` 分隔，`Ctrl/Control/Cmd/Win`→Ctrl、`Alt/Option`→Alt、`Meta/Super`→Meta。裸修饰键名（如 `"Shift"`）按按键处理（改键场景）。
- **匹配**（`matches`）：主键一致 **且** 事件修饰键 ⊇ 快捷键修饰键——按下 `Ctrl+Shift+K` 也会命中 `Ctrl+K`（宽松规则，避免多按一个 Shift 就触发失败）。
- **配置模型**（JSON 落盘，跨平台同步介质）：
  - `Config { folders, layers, shortcuts, remaps, expansions, settings }`
  - `ShortcutItem { name?, description?, folder?, layer?, triggers, actions, enabled }`（`triggers` 任一组命中即触发，`actions` 按顺序执行；`folder` 目录分组、`layer` 归属层、`None`=基础层始终生效）
  - `Trigger`（`kada-core`）：触发键单元 = `Combo(Shortcut)` 单组合（`"Ctrl+K"`）/ `Sequence(Vec<Shortcut>)` 按键序列（`"F9 J K"` 依次按下 F9→J→K，首步即 leader 键进入等待态）/ `Chord(Vec<Shortcut>)` 和弦（`"F&J"` 同时按住 F 和 J，成员必须是非修饰的裸键）；`Trigger::parse` 按空白切分（1 token 含 `&`=Chord、否则 Combo，≥2=Sequence，每步用 `Shortcut::parse`，坏输入整条丢弃）；纯逻辑 `SequenceTracker`（时序状态机，支持共享前缀 `"F9 J K"`/`"F9 J L"` 并存）+ `ChordTracker`（无序「按住集合」凑齐判定，`press`/`release` 成对维护集合）+ `DEFAULT_SEQUENCE_TIMEOUT_MS = 1000`
  - **吞键铁律（序列/和弦/tap-hold 必须遵守）**：被吞掉的键补不回来，所以「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口内；**没组成快捷键的按键必须原样回放**（和弦成员全部抬起仍未凑齐 → 回放成员键；序列断链/超时 → 回放 leader 与已吞的中间步；tap-hold 短按没设 `tap`、长按到底没设 `hold`（切层键 / 只设短按 / 只设双击三击）→ 回放原键；待定期间按下的其它键一并吞掉并排在后面回放，保证回放顺序 = 用户输入顺序）。否则一旦配置 `F&J` / `F9 J K` / 只设「长按进入层」的切层键，F、J、F9、切层键就彻底变哑。
  - `Action`：`Text / Command / Keys / PauseMs / Os / App / OpenUrl / If / Script`（`Command` 带 `shell`(Cmd/Powershell)/`show_output` 是否弹结果/`var` 非空则把标准输出写入文本变量；`Os` 文件动作、`App` 应用动作、`OpenUrl` 用系统默认浏览器打开网址（Windows `explorer` / Linux `xdg-open`）、`If` 条件判断；`Script` 脚本动作带 `path`/`interpreter`(可选解释器)/`show_output`/`var`，执行时注入 `KADA_TRIGGER`/`KADA_NAME`/`KADA_VARS` 环境变量；旧版 `Cmd`/`Powershell`/`Launch`/`CloseProgram`/`OpenFolder` 加载时自动迁移）
  - `Condition`：路径/变量/时间（`Exists/NotExists/IsFile/IsDir/Equals/NotEquals/ModifiedWithin`）+ 前台应用/窗口（`FrontmostApp/NotFrontmostApp` 按进程名、`WindowTitleContains` 按窗口标题；含 `*`/`?` 走通配否则子串匹配，不区分大小写；Windows 取前台窗口、Linux 暂受限恒不成立）
  - `Remap { from, to, tap, hold, layer, hold_layer, lock_layer, tap_timeout_ms, oneshot, sticky, tap2, tap3, enabled }`：普通改键用 `to`；tap-hold 用 `tap`（短按）/`hold`（长按，常设修饰键）；切层键两形态——`hold_layer` 长按进入层（momentary，按住生效松开退回）/`lock_layer` 长按锁定层（长按切换、保持生效、再长按退出；**两者互斥，同设保留 lock_layer**；层内放和弦/序列只能用 lock_layer——momentary 要同时按住切层键凑 3~4 键）；`oneshot` 单次修饰（单击 `from` 武装、应用到下一个非修饰键后自动释放）/`sticky` 粘滞修饰（单击锁定、再击解锁，值域均只 `Ctrl/Alt/Shift/Meta`）/`tap2`/`tap3` 双击/三击（tap-dance 连击不同义，缺省回落上一级）；`tap_timeout_ms` 判定阈值默认 200ms。形态互斥，运行时优先级 `sticky > oneshot > tap-hold > 普通 to`
  - `Layer { id, name }` 键位层（快捷键/改键可归属某层，仅该层激活时生效）+ `TextExpansion { trigger, replace, enabled }` 文本扩展（输入触发词+后缀自动展开，`replace` 支持 `{date}`/`{time}`/`{clipboard}`）
  - `Settings { autostart, paused, wake_key }`；`detect_conflicts` 检测硬/软冲突（序列按完整字符串判重；序列 leader 遮蔽同层单组合=硬冲突；超集软冲突仅对单组合生效；**层可达性**：层里有启用的条目却没有任何「长按进入层/长按锁定层」的切层键指向它 → 警告「层内条目永远不会生效」，这是「层内快捷键配了但不触发」最难自查的原因）；启动显隐按来源区分（双击 exe 显示主窗口、开机自启带 `--autostart` 参数静默到托盘）
  - 变量占位符：`{变量名}` / `{变量名.字段}`，由 `substitute_vars` 替换（`Os::GetFileProps` 写 File、`App::Status` 写 Bool、命令动作 `var` 非空写 Text，Text 带退出码 `{变量名.exit_code}`，作用域=单次触发内的动作序列）；`sanitize_config` 逐条清洗坏条目（坏触发键/动作/改键单独忽略，不拖垮整份保存）
  - feature 门控（`automation`，`kada-core`/`kada-actions`/`src-tauri` 三 crate 均 `default = ["automation"]`）：`Command`/`Os`/`App`/`OpenUrl`/`If`/`Script`/`Condition`/`Vars` 属自动化动作，关闭 feature（`--no-default-features`）得「基础版」= 改键全形态/层/键序列/文本扩展/Text·Keys·PauseMs 注入；`Action::Script` 是脚本扩展的「逃生舱」（先指向脚本文件，不嵌 Rhai/Lua，生态起来再补 wasm/动态库）
- 手工编辑的配置允许缺字段、带未知字段（`#[serde(default)]`），加载时校验。
- **消息中心**（内存态，重启清空）：Cmd/PowerShell 结果一律记入消息中心；`show_output` 开则弹结果弹窗、关则托盘图标 + 应用内「消息」入口亮红点，进入「消息」页标记已读。「消息」页分「命令结果 / 冲突」两个标签（冲突标签常驻展示 `get_conflicts` 清单，`Conflict.name` 标识归属、severity 着色），快捷键列表顶部另有可点可消除的冲突气泡。

## 钩子引擎

- **Windows**（`kada-hook::win`）：`SetWindowsHookEx(WH_KEYBOARD_LL + WH_MOUSE_LL)`，同一线程跑消息循环；处理函数直接跑在回调内（不做跨线程调度，保证顺序与低延迟）。修饰键用 `GetKeyState` 实时读；**自动重复按「该键已按下且未抬起」判定**（`HELD_KEYS` 集合，不用时间窗——同键 250ms 内连打两次是正常输入，按时间窗会被误判成重复丢掉第二击，导致热串缓冲缺字 / 连击不计击数）；`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键，**但所有 keyup（含被吞掉的；键盘与鼠标都一样）仍以观察者身份回调 handler**——状态机靠抬起维护「按住集合」（和弦成员、tap-hold 短长按判定），漏掉就会让该键与整个状态机永久卡死；注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持"按下-抬起"配对（按住原键 = 按住目标键）。鼠标钩子只翻译中键/侧键（MB4/MB5），左右键与滚轮一律放行；小键盘 Enter 与主 Enter 共用 `VK_RETURN`，靠 `LLKHF_EXTENDED` 扩展位区分。
- **Linux**（`kada-hook::linux`）：evdev + uinput（内核输入层），`EVIOCGRAB` 独占抓取 `/dev/input/event*` 键盘设备、`/dev/uinput` 建虚拟键盘 `kada-virtual-keyboard` 转发；X11 / Wayland 通用；需要 root 或 `input` 组 + udev 放开 `/dev/uinput`。修饰键按事件流维护（`MODS_DOWN`），与 Windows `GetKeyState` 语义对齐；自动重复直接用 evdev 的 `value == 2` 标记。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V`（中文等 Unicode 最稳，会短暂占用并恢复剪贴板）；组合键全部按下 → 稍停 → 逆序松开。`wait_modifiers_released`：触发带修饰键的快捷键时修饰键仍物理按住，直接注入会被污染成 `Ctrl+Alt+<键>`（粘贴 Ctrl+V 变 Ctrl+Alt+V），故动作执行前轮询 `GetKeyState` 等修饰键释放（≤300ms）再注入（Windows 实现；Linux 因 evdev 独占抓取 + 独立虚拟设备，无需等待、提供同接口空实现）。
  - **壳层决策扩展**（状态机集中在 `src-tauri/src/engine.rs`，Tauri 无关、注入走 `Inject` trait 可单测，`lib.rs` 只剩接线）：`taphold_step` 状态机在 `decide` 前吞掉 tap-hold 键并延迟判定短按/长按/长按切层（momentary 与锁定两形态）/单次武装/粘滞切换/连击（roll 判定：待定期间按其它键立即判 hold；**短按没设 `tap`、长按到底没设 `hold`（含切层键）都回放原键**，否则这个键按下去什么都不发生）；层语义=激活层条目优先、基础层条目兜底（`active_layer` 由切层键驱动——`hold_layer` 按住期间生效，`lock_layer` 长按切换后保持生效，**momentary 层只在 roll 或按住期间按别的键时真正进入**，单独按住切层键再松开等于没用过、回放原键）；注入修饰统一走「物理注入 + mods 补全」通道（`ModsState` 分 `hold`/`sticky`/`oneshot` 三类，合并为 `hold ∪ sticky ∪ oneshot`；oneshot 下一个非修饰键释放、sticky 再击解锁、hold 松开 `from` 释放）；连击经 `TapDanceState` 等待窗累计击数、超时懒提交；**超时类等待态由定时器线程驱动**（`spawn_engine_ticker`，每 `ENGINE_TICK_MS=60ms` 调 `Engine::tick`：键序列 leader 超时回放、连击等待窗提交；事件路径上另有懒判定兜底）——只靠「下一个事件」懒判定的后果是单独按一下 leader 键（之后不按别的键）永远等不到回放，表现为该键按了没反应；`chord_step` 和弦状态机在 `taphold_step` 之后、`sequence_step` 之前——成员键按下先吞掉（不吞就漏字符、凑不成和弦），凑齐所有成员即 `fire` 触发、**没凑成则原样回放**（成员键全部抬起仍未凑齐 / 待定缓冲挤满 → 回放成员键与期间被吞的其它键，修饰键从不算其它键、直接放行）；`sequence_step` 键序列状态机在 `chord_step` 之后、`decide` 之前——leader 键吞掉进入等待、命中后 `fire` 触发、超时（1000ms，定时器 + 懒判定双路）/断链则回放 leader 与已吞的中间步（不再吞掉 `Escape`，它按普通断链处理）；`Engine::hotstring` 在放行事件上累积热串缓冲、命中触发词后吞掉后缀键并异步（后台线程）回删+注入+补回后缀，注入失败记入消息中心。
- **录入暂停是限时租约**：`set_paused(true)` 只暂停 `PAUSE_LEASE_MS`（60 秒）后自动失效——录入捕获（组合键/序列/和弦/宏录制）若被中断（窗口失焦、详情被关、收进托盘），前端可能来不及解除，布尔量会永久卡在暂停态让整个应用静默失效（快捷键/改键/文本扩展全无响应且看不出原因）；前端同时在失焦/隐藏/关详情时主动 `set_paused(false)`。
- **前台上下文**（`frontmost_context`）：Windows 取 `GetForegroundWindow` 窗口标题 + 进程名（`QueryFullProcessImageNameW`）；Linux 暂返回 None（Wayland 受限），前台类条件在该平台恒不成立。
- **已知天花板（升级路径）**：低层钩子拦不住 UAC 提权进程 / 部分游戏 → 驱动级拦截（Interception）；Linux 热插拔键盘不在监听列表（重启应用即可）。

## 软件更新（Windows 已落地）

- **信任链**：Tauri `tauri-plugin-updater` + GitHub Releases 作更新源 + Ed25519 签名。更新端点 `https://github.com/chillyice/Kada/releases/latest/download/latest.json`（配在 `tauri.conf.json` 的 `plugins.updater.endpoints`；`releases/latest` 只解析**已发布**的 Release，故流水线默认发 draft 由人 Publish）；**公钥内嵌 `plugins.updater.pubkey` 随仓库，私钥只在本机 `C:\Users\chill\.tauri\kada.key` 与 CI Secret（`TAURI_SIGNING_PRIVATE_KEY`），绝不入库**——私钥丢失 = 现有用户再也收不到更新，只能轮换公钥 + 让用户手动装一次新版。打包靠 `bundle.createUpdaterArtifacts = true` 产出 `.sig`；`latest.json` 由 `tauri-action` 生成，**不要手写、不要入库**。
- **三种触发的打扰等级不同**（`src-tauri/src/update.rs`）：启动后台检查（release 构建延迟 8 秒、**只在发现新版本**时弹一次气泡，网络失败/没发过 Release 全静默，dev 构建跳过）；托盘菜单「检查更新」与设置页按钮（无更新与出错都弹窗，发现新版本先确认再装）；设置页「下载并安装」（点击即确认）。
- **Windows 上「安装」这一步会退出应用**（安装器限制）：NSIS 以 `passive` + `/R` 跑，装完自动拉起新版本。**这就是启动检查只提示、不自动安装的原因**——自动安装等于替用户决定「现在关掉你正在用的应用」。安装失败保留当前版本并弹窗报错。
- **状态机在后端**：阶段 `idle/checking/up-to-date/available/downloading/installing/error` 由 Rust 持有、经 `update-status` 事件推送，前端只渲染（`get_update_status` 兜底拉取）；`busy()` 挡托盘连点。下载进度**只在整数百分比变化时推事件**（分片回调极密集，否则刷爆 IPC）。
- **权限面**：更新逻辑全在 Rust（`UpdaterExt`），前端不直接调 updater → capabilities **不开** `updater:*`，也不依赖 `@tauri-apps/plugin-updater`（最小权限）。
- 发版步骤、Secrets 配置、`requireSignedVersion` 加固选项、NSIS 安装模式（Tauri v2 默认 `currentUser`，即免提权）见 `docs/安装与更新-Windows.md` §2。

## 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试（kada-core/kada-hook/kada-actions/src-tauri）
cargo check -p kada         # 验证 Tauri 壳编译（完整版，default features）
cargo check -p kada --no-default-features  # 基础版编译（裁掉 automation 自动化动作）
npm --prefix ui run build   # 前端构建（tsc + vite build）
cargo run -p kada-hook --example demo   # M1 冒烟 demo（仅 Windows）
```

- **带签名的安装包（自动更新发版用；比 `--no-bundle` 慢得多，日常打包别用）**：
  `$env:TAURI_SIGNING_PRIVATE_KEY_PATH="C:\Users\chill\.tauri\kada.key"; npx tauri build --bundles nsis`
  → **仓库根**的 `target/release/bundle/nsis/` 下出 `Kada_<版本>_x64-setup.exe` **与其 `.sig`**（缺签名则客户端拒装，这是设计使然；workspace 的 `target/` 在根目录，不在 `src-tauri/` 下）。**打包前先退出正在运行的 Kada**：exe 被占用会报「拒绝访问」，且 bundler 写「安装包类型」标记失败会警告 `Updater plugin may not be able to update this package`。首次打包还需联网下载 NSIS 工具链（本机实测该 GitHub Release 资源拉不通，发版走 CI）。
- `npx tauri bundle --bundles nsis` 可对**已编译好**的二进制单独出安装包（不重编译），用于只改打包配置时省时间。
- **CI**：GitHub Actions（`.github/workflows/ci.yml`）在 `ubuntu-latest` + `windows-latest` 双平台执行：前端安装 + 构建 + `cargo test --workspace` + `cargo check -p kada`；发版流水线 `.github/workflows/release.yml`（打 `v*` tag 或手动触发）。
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

- `README.md`（项目简介，GitHub 展示）；`docs/README.md`（文档索引：每份文档一句话定位 + 新人阅读顺序）；`docs/架构设计.md`（分层 / 事件流转 / 关键机制 / 跨平台策略）；`docs/安装与更新-Windows.md`（Windows 版安装步骤 + 软件更新：客户端流程、签名密钥与 Secrets、发布流水线、发版步骤）；`docs/需求设计说明书.md`（功能需求唯一活文档 + 修订记录）；`docs/变量提取与引用指南.md`（变量提取/占位符引用 Q&A，含移动硬盘盘符实战）；`docs/竞品分析与优化规划.md`（竞品横向对比 + 差距清单 + P0~P3 优化路线图）；`docs/变更归档.md`（已实现变更归档，按里程碑的 文件-改动表 + 规则/决策）。
- 代码事实以 `crates/kada-core/src/lib.rs` + `crates/kada-hook/src/*.rs` + `src-tauri/src/lib.rs` 为准，如需检索先 `grep` 再动手。
- **文档规划借鉴 ihomy 与咖啡伴侣的结构化布局**：每个变更都有固定落点（规则 → 本文件；功能需求/规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`），保证多会话衔接、新会话可直接续接。

## 环境检查（参考）

本机已装：Rust toolchain、Node、Tauri CLI（`@tauri-apps/cli`）。前端依赖 `npm --prefix ui ci`；构建在 Windows 下进行。
