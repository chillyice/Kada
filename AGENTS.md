# AGENTS.md — Kada 项目规则

> 本文件供 ZCode 跨会话加载：**只留规则 + 导航索引**，目标 **≤20KB**（超出会被截断，尾部先丢）。
> 完整功能需求见 `docs/需求设计说明书.md`（活文档，未实现规划在 §7）；实现历史归档见 `docs/变更归档.md`（只追加，不写回本文件）。

> **⚠ Git 规定**：非人工指令，不得主动提交代码（`git commit`/`git push` 一律禁止）；`git add` 只能指定具体文件路径（禁 `git add -A` / `git add .`）。归档文档走 `/archive` 技能。**例外（用户已授权）：归档自动提交**——归档流程写完全部文档即自动 `git commit` + `git push`（不必再等指令）；`git add` 仍只给具体路径。

> **⚠ 编码/路径警示（遵守以防误写）**：含中文的源码/配置/文档一律走本工具的 Read/Write/Edit 读写，**禁止用 PowerShell `Get-Content`/`Set-Content` 读写**（PS 5.1 默认 GBK 会破坏 UTF-8 中文；PS 仅用于 cargo/npm 构建与运行 demo）。Git Bash 调 Windows 原生命令时 POSIX 路径会被自动转换，注意参数。

## 项目概述

- **咔哒 Kada**：轻快跨平台快捷键 / 改键 / 宏工具，托盘常驻（关窗不退出），配置 JSON 落盘、可跨平台同步。
- **技术栈**：Rust workspace（`kada-core` 纯逻辑 + `kada-hook` 平台钩子 + `kada-actions` 动作引擎）+ Tauri 2 桌面壳（crate `kada`）+ Vite/TypeScript UI（`kada-ui`）。配置落盘 `app_data_dir()/config.json`（`%APPDATA%\com.kada\`）。

## 命名约定

| 项 | 值 |
|----|----|
| workspace members | `crates/kada-core` / `crates/kada-hook` / `crates/kada-actions` / `src-tauri` |
| Tauri crate 名（`src-tauri/Cargo.toml`） | `kada` |
| npm 包名（`ui/package.json`） | `kada-ui` |
| Tauri `identifier` | `com.kada` |
| `productName` / 窗口标题 / 应用显示名 | `Kada` / `咔哒 Kada` |

- **键模型使用平台无关中性名**：`Enter/Escape`（不是 `Return`）、修饰键 `Ctrl/Alt/Shift/Meta`（macOS 的 ⌘ 由 `Meta` 承载，平台层负责把 OS 键映射成 `Key`）。键名渲染回 `"Ctrl+Alt+K"` 形式。

## 代码结构

代码地图见 `docs/README.md`「代码地图」；结构规则：members / crate 名见「命名约定」，平台代码 `#[cfg]` 门控、三后端同一套接口。

## 核心概念与铁律

> 逐字段定义见 `docs/需求设计说明书.md` §2~§5；下面只列**动手必须遵守的约束**。

- **配置模型**：`Config { folders, layers, shortcuts, remaps, expansions, settings }`；逐字段定义见 §2 / §3，只留易踩的约束。`Layer` 可选 `mark`（色值或 emoji，纯展示；颜色本机各层唯一）。`Remap` = to / tap-hold（`tap`+`hold`）/ 切层键（`hold_layer` 长按进入、`lock_layer` 长按锁定，**互斥、同设保留 lock**）/ 进阶修饰（`oneshot`/`sticky`/`tap2`/`tap3`，形态互斥，优先级 `sticky > oneshot > tap-hold > to`，单次/粘滞值域任意键）。`ShortcutItem`/`Remap` 均可带触发侧条件门控 `when`——不成立等价于「该条不存在」（落到同触发键下一条、不吞键），判重指纹含 `when`（不同条件不算重复）。
- **吞键铁律（序列 / 和弦 / tap-hold 必须遵守）**：被吞掉的键补不回来，「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口内；**没组成快捷键的按键必须按用户输入顺序原样回放**——和弦成员全抬起仍未凑齐、序列断链或超时、tap-hold 两条无输出路径（短按没设 `tap`、长按到底没设 `hold`，含切层键与 momentary 层单按）。反例与机制见 `架构设计.md` §3.1。
- **配置落盘铁律**（`config_io.rs`，见 `需求设计说明书.md` §5.6）：保存 = **原子写**（临时文件 → `fsync` → `rename` 覆盖，留 `.bak`），**不要退回 `fs::write`**；加载解析失败**绝不静默清空**——留档 → `.bak` 恢复，全失败才空配置 + 告警。加载只 `migrate()` **不** `sanitize_config()`（清洗只在保存/导入路径）。**导入必须选方式**：`merge` 只增不删、不造新冲突、**设置保留本机**；`replace` 覆盖前必须 `archive_copy` 留档。**删除已保存条目要先确认**（`confirmDelete`），未落盘新增项可不问。**外部修改监听**（`config_watch.rs`）：**能解析才采纳、解析不动就别碰磁盘**（只读 `read_only`）；写盘与 `adopt_own_write` 同持 `cfg_watch` 锁；界面收 `config-changed` 有未保存草稿必须问用户。**`config-updated`（应用内写盘）与 `config-changed`（外部改动）语义分离，不可混用**。**同步存档**（`settings.sync_dir`）：每条写盘路径后 `push_sync` 一份到该目录（尽力而为）；**新增写盘路径别漏**。
- **层语义**：激活层条目优先、基础层条目兜底（不归属任何层的条目始终生效）；**momentary 层只在 roll 或按住切层键期间按别的键时真正进入**。
- **平台能力缺失**（`PlatformCaps`，见钩子引擎段）与**层可达性**（层有条目却无切层键指向 → 警告）同属软冲突。
- **执行记录（消息页「执行记录」标签，规格见说明书 §5.4）**：一次触发 = 一条 `RunRecord`（`fire` 先落 `running`、收尾定 `ok`/`error`/`abort`），**每个动作**都上报一步 `ActionStep`——文本/按键/延迟这类不产命令结果的动作也必须有，否则「按了没反应」查不到是哪一步。逐步 `emit("run-update")` 推整张卡（`get_run_records` 兜底）；两种展示与上限见 §5.4。
- **普通消息**（`run_id === null` 才进列表；内存态，重启清空）：命令/脚本结果、**中止/超时**（`abort`）、**动作失败**（`error`）、**配置事件**（`config`）都记入；`show_output` 开则弹结果弹窗，否则托盘红点；进「消息」页标记已读。**定长上限**：200 条、单条 stdout/stderr 各 16KB（`MAX_RESULTS` / `truncate_text`）。
- **动作执行有界**（`kada-actions/`，见 `架构设计.md` §3.17 / 说明书 §5.7）：命令/脚本**不许用 `Command::output()` 无限等**——走 `run_with_limits` 边读边等，超时（`settings.action_timeout_ms`，默认 30 秒、0=不限）/ 中止即**杀整棵进程树**（平台手法见 §3.17）；中止 = **代数计数器** `abort::request()`。**并行动作** `Parallel`（7.1-㉞）= 成员各起一线程、全部结束才进下一步，用**链内注入锁**维持「注入串行」（注入类动作持锁，只并发非注入步骤），成员 `vars` 私有快照、按下标顺序合并与提交。
- **配置锦囊（分享）铁律**（`snippet.rs` + `export_fragment`）：锦囊**必须仍是合法的 `Config` JSON**（顶层多个被忽略的 `kada_snippet` 键）——对方靠现成的「导入 → 合并」吃下，**不要为锦囊另立一套导入格式 / 解析器**。摘取时**剥掉 `folder`**、**带上被引用的层**、**不带 `settings`**（理由见 §5.6）。**锦囊市场没有服务端**：索引就是一份能下载到的静态 JSON（默认仓库 `snippets/index.json`，设置里可改 URL），浏览与搜索在本地；**装之前必须先预览**（锦囊能带命令 / 脚本动作 = 装进来就会跑），装走的是「导入 → 合并」同一条路径。
- **feature 门控**（三个 crate 均 `default` 开启的 `automation` feature）：关掉（`--no-default-features`）得**基础版** = 改键全形态 / 层 / 序列 / 和弦 / 文本扩展 / Text·Keys·Mouse·PauseMs 注入，裁掉 Command / Os / System / App / OpenUrl / If / Script / Condition / Vars。手工配置允许缺字段/未知字段（`#[serde(default)]`）。**门控要覆盖测试目标**：用到 `Condition` / `Action::If` / `Parallel` 的测试与辅助函数（如 `linux_caps()`）必须自己也 `#[cfg(feature = "automation")]`，否则 `cargo test -p kada-core --no-default-features` 编译失败（`cargo check` 抓不到这类漏网）。
- **前端键表须与 core 同源校验**：`ui/src/main.ts` 的 `KEY_OPTIONS` 须与 `kada-core` 的 `Key::ALL` 一致且**保持显式字面量**（壳测试 `frontend_key_table_matches_core` 守着，Rust 增键而前端漏改即红）。

## 钩子引擎（动手前必读的约束）

- **Windows**（`kada-hook::win`，见 §4.1）：`SetWindowsHookEx(WH_KEYBOARD_LL + WH_MOUSE_LL)`——**回调里绝不能做耗时操作**（超时被系统摘钩）。自动重复按「该键已按下且未抬起」判定（**不用时间窗**，见 §4.1）；NumLock 关时小键盘按扫描码还原（见 §4.1）。`Block`/`Replace` 登记 `SWALLOWED`，后续 keyup 一并吞掉防幽灵按键，**但所有 keyup（含被吞掉的）仍以观察者身份回调 handler**。注入事件带 `LLKHF_INJECTED` 一律放行防回环；`Replace` 保持「按下-抬起」配对；`Replace`/`Keys` 注入走**扫描码 + 前台布局换算**（媒体键保留 VK）。鼠标钩子只翻译中键/侧键且**按需安装**（配置引用鼠标键才挂，`set_mouse_enabled` + `config_uses_mouse`），新增鼠标键用途要同步该判定。
- **macOS**（`kada-hook::macos`，见 `架构设计.md` §3.19 / `需求设计说明书.md` §4.3）：`CGEventTap` 独立线程 CFRunLoop，**回调里同样不许做耗时操作**。三条硬约束：① tap 必须会话级 `kCGSessionEventTap`；② 「辅助功能」是**运行时授权**（`hooks_supported()` = `AXIsProcessTrusted()`），未授权 `start` 报指引 + 弹授权框；③ 注入识别两判据取或（Private 事件源 ∪ 创建者 pid == 本进程）。自愈走下面那条看门狗。
- **输入层起不来的处置**：Windows 失败退出（异常）；macOS / Linux（权限没配好）**照常启动**，`start` 的错误串（带可照抄指引）经消息中心推给用户。
- **回调里不许建窗口 / 碰 IO 网络**：唤窗（主窗口 / 气泡 / 状态指示）、动作执行、注入一律 `std::thread::spawn`，回调里只留判定（读配置、比时间、状态机）——理由与代价见 `架构设计.md` §3.6。判定与副作用分开：判定用不持有 `AppHandle` 的纯函数。
- **透明浮窗（`toast` / `hud` / `hints`）必须 `focusable(false)`**（能被激活就抢前台 → `ResetWatch` 误判复位，见 §3.21）。**`hints`（7.1-㊳）是唯一可交互浮窗**：默认不 `set_ignore_cursor_events`、`zoom_hotkeys_enabled(false)`、**一律 `shadow(false)`**（见 `变更归档.md` V0.39）；唯一例外是下面那条悬停让位的临时穿透。
- **界面外链必须由壳层接住（主窗口 `on_new_window`）**：新窗口请求没人接就被 wry 吞掉、链接点了没反应；一律 `Deny`，只把 http(s) 交 `kada_actions::open_url` 用系统浏览器打开（`external_link_to_open` 过滤，见 §5）。**新增界面外链 / 改建窗代码时别把这段弄丢**；真机验收见 `人工验证清单.md` §21。
- **提示框悬停让位（`spawn_hints_hover_watch`，见 §3.21）**：光标进框即穿透 + 变淡，停满 2s 恢复可交互直到挪出，再进照样让位。三条硬约束：**穿透之后收不到任何鼠标事件，只能轮询全局光标位置**（见 §3.21）；**隐藏时必须真撤掉穿透标记**；穿透状态要广播 `hints-peek` 让界面变淡。
- **Linux**（`kada-hook::linux`，见 §4.2）：evdev + uinput，`EVIOCGRAB` 抓键盘与**真鼠标**（触摸板/指点杆**不抓**），`/dev/uinput` 建键鼠合一虚拟设备；权限 = `input` 组 + udev 放行 uinput（起不来分诊见 §4.2）。**设备拔出就地复位三张表**并递增 `reinstall_count()`（壳层 `ResetWatch` 轮询接通）；热插拔仍无（重启应用）。自动重复 `value==2`。**改完 Linux 分支必跑**：`cargo check -p kada-hook -p kada-actions --target x86_64-unknown-linux-gnu --all-targets`——**查不到 Tauri 壳**（要 gtk/webkit，本机交叉不了），壳分支只有 CI 或容器构建（`scripts/linux-package/`）拦得住。
- **注入**（`simulate`）：文本走剪贴板 + `Ctrl+V` / macOS 的 ⌘V（中文最稳）；组合键「全部按下 → 稍停 → 逆序松开」。**动作执行前必须 `wait_modifiers_released`**（≤300ms 轮询）等物理修饰键释放——否则仍按住的触发修饰键会把注入污染成 `Ctrl+Alt+V`（Linux 空实现）；**新增注入类动作要同步壳层 `injects_keys`**（否则不多等这一下）与链内注入锁。
- **提权窗口注入**（7.1-㉑a，见 `架构设计.md` §3.23）：**新增注入路径必须经 `deliver()`**（前台是提权窗口且开关开、通道在才交提权进程重放，失败回落本地）；钩子线程标 `local_only`——回调里不许有带等待的 IPC；提权通道只放行本用户 + 双向身份校验、**只许有注入类 op**（别开「执行命令」口子）、断开或父进程退出即退。
- **状态机集中在 `src-tauri/src/engine.rs`**（Tauri 无关、`Inject` 假注入可单测，接线在 `lib.rs`）：顺序为 `taphold_step`（tap-hold / 切层 / oneshot / sticky / 连击）→ `chord_step` → `sequence_step` → `decide` → `hotstring`。**超时类等待态必须由定时器线程落地**（`spawn_engine_ticker` 每 60ms 调 `Engine::tick`，传 `cfg`——**配置读锁必须先于 `Engine` 锁**，同序否则死锁；事件路径另有懒判定兜底）。热串命中后吞掉后缀键，由**后台线程**回删 + 注入 + 补回后缀。
- **录入暂停是限时租约**：`set_paused(true)` 只暂停 60 秒（`PAUSE_LEASE_MS`）后自动失效——前端被中断时可能来不及解除，布尔量永久卡在暂停态会让整个应用静默失效；前端失焦/隐藏/关详情时也主动解除。
- **自愈看门狗**（`win.rs`/`macos.rs` 各一套；判据见 `架构设计.md` §3.15）：低层钩子/tap 会**静默失效**，`start` 另起 1s 巡检线程 + 指数退避。Windows **只投递 `WM_APP_REINSTALL`**（`SetWindowsHookEx` 必须在有消息循环的钩子线程上调用）、**必须先卸旧再装新**并复位运行时状态表（不卸旧 = 按键被处理两遍）；**判据缺观察者就不判**（没鼠标钩子时归因不到键盘，硬判会无限重装）。
- **输入状态复位**（`Engine::reset`，三平台，机制见 `架构设计.md` §3.16）：钩子重装 / 设备拔出 / 前台切换 / **配置整份换掉** = 「与物理键盘断过一次线」。口径：**注入的收回来**（补 up + 退出 momentary 层）、**缓冲里的丢弃不回放**、**锁定层保留**。触发通道：`reinstall_count()` 变化（Linux 同通道计设备拔出）、`foreground_window()` 连续两轮同一新窗口（Linux 无这路）、配置 `cfg_stale` 旗标（均在定时器线程）。
- **前台 / 设备上下文**（`frontmost_context` / `current_device`，取法与限制见 `架构设计.md` §3.22）：**Linux 恒 `None`**（Wayland）、macOS 设备恒 `None`。带 `when` 的条目用 `TriggerContext`，前台**惰性查询**（只在真被判定时才查）。**`get_conflicts` 按 `PlatformCaps` 标出平台能力缺失**（`Warn` 不拦保存），改钩子能力时同步 `platform_caps()` 填表。**输入层环境诊断**（7.1-㊽，见 §4.6）同属软冲突，但**只在配置用到鼠标键时才出**、`settings.input_env_diag` 关掉**连探测都不跑**；`DIVERTERS` 只收真会 divert / 独占抓取的软件，乱加即误报。
- **已知天花板（升级路径）**：见 `需求设计说明书.md` §4.5 / §7.2。

## 软件更新

- **软件更新红线**（§5.5 / `release.yml` 顶部）：**私钥绝不入库**（公钥内嵌 `plugins.updater.pubkey`，私钥只在 `~/.tauri/kada.key` 与 CI Secret）；`latest.json` 由 `tauri-action` 生成，**不手写、不入库**；端点只认**已发布** Release。**前端碰不到更新**（全在 Rust，capabilities 不开 `updater:*`）。**安装会退出应用**；其它平台装完 `app.restart()`。
- **三步分开**：检查永远自动（启动 8 秒 + 每 24h）；下载/安装由 `Settings.auto_update`（**默认关**）决定——勾了即自动下载、空闲 ≥10 分钟（`kada_hook::idle_seconds()`；Linux 恒 `None`，**绝不据此安装**）或便携版下次启动时装，没勾只能手动「下载」→「安装」。下载落盘 `updates/pending.json`（**形态不符即丢弃**），重启恢复 `ready`；安装前重查清单版本。
- **更新源与形态**（§5.5）：`Settings.update_source` 可切 `github`；镜像只改 URL、`.sig` 原样；检查走 `build_updater`，别用 `app.updater()`。**不改安装形态**：exe 目录 == 注册表 `HKCU/HKLM\Software\kada\Kada` 才是安装版（`installMode: quiet` = NSIS `/S /R`）；否则便携版读清单 `windows-x86_64-portable` 裸 exe 原位替换（改名→落新→重启，失败回滚）。**绝不交叉**；CI 用 `signer sign --app-version` 签便携包。

## 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试
cargo check -p kada         # Tauri 壳编译（完整版）
cargo check -p kada --no-default-features  # 基础版编译（裁掉 automation）
npm --prefix ui run build   # 前端构建（tsc + vite build）
cargo run -p kada-hook --example demo   # 冒烟 demo（三平台真人按键）
cargo check -p kada-hook --target aarch64-apple-darwin --all-targets  # macOS 侧唯一可用的本地验证（见下）
```

- **打包**：产物在**仓库根** `target/release/bundle/`；**打包前先退出 Kada**。须签名（缺则拒装）：`npm run tauri -- build --bundles nsis` + env `TAURI_SIGNING_PRIVATE_KEY`=密钥**文件全文**、密码空串（PS 传不了空串）。**三条渠道（GitHub / ihomy 直链 / macOS dmg）各有清单，发什么文件与放行口径见 §5.5**。**Linux 包只能靠容器出**（`scripts/linux-package/`）、**macOS 包只能靠 CI**。
- **发布链路版本红线**：`requireSignedVersion = true` 要求签名里带版本号，**tauri-cli < 2.12.1 不写**（客户端全拒装 `MissingSignedVersion`，流水线还不报错），根 `@tauri-apps/cli` 锁 `^2.12.1`；**动 CLI 版本/版本号/打包 targets 后必跑 `node scripts/verify-release.mjs`**（流水线最后一步也跑）。
- **CI**：`ci.yml`（三平台：前端构建 + `cargo test --workspace` + `cargo check -p kada` 两版）；`release.yml`（打 `v*` tag 或手动触发；矩阵出 Windows NSIS + 便携 zip + Linux deb/AppImage + macOS dmg，`max-parallel: 1` 串行、**根目录 `npm ci` 不能省**、产物目录自适应，其余见顶部注释）。**macOS 本地验证边界**：darwin 交叉 check 只能真编 `kada-hook`（`-p kada` 必失败，`ring` 要真 clang）——macOS 壳层分支只能靠 CI / 真机验。
## 里程碑与规划

- **未实现规划唯一清单 + 已落地事项编号索引见 `docs/需求设计说明书.md` §7**（M0–M5 已完成）——立项、排期一律以那里为准（勿重复立项）；实现历史见 `docs/变更归档.md`。

## 文档清单

- `README.md`；`docs/README.md`（索引 + 阅读顺序 + 全清单）。
- **落点分工**：规则/约定/命名 → 本文件；功能需求与规划 → `需求设计说明书.md`；已实现归档 → `变更归档.md`。代码事实以源码为准，先 `grep` 再动手。本机已装 Rust / Node / Tauri CLI；前端依赖 `npm --prefix ui ci`。
- **`需求设计说明书.md` 按功能模块组织，不按版本**：顶部 16 模块导航表；**版本号只写进 `变更归档.md`**，正文引**模块小节号**（如 §5.7）而非版本号。§N 是稳定 ID，**只增不改**，新功能写进所属模块小节。

