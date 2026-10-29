# AGENTS.md — Kada 项目规则

> **本文件 = AI 会话必读入口**：只放规则 / 去哪查 / 怎么验证。机制细节外移到 `docs/`。
> **上限 20KB（按 CRLF）**——超了注入被截断、**尾部先丢**，故「命令 / 归档」放最后。改完跑 `node scripts/check-docs.mjs`。

## 0. 动手前 30 秒

| 我要做 | 去哪 |
|---|---|
| 找到该改哪个文件的哪个符号 | **`docs/代码地图.md`**（症状 → 文件 → grep 令牌 → 规则出处） |
| 查字段级定义 / 接口 | `docs/需求设计说明书.md`（活文档，只描述当前实现） |
| 查机制与「为什么这样设计」 | `docs/架构设计.md` |
| 查历史决策 | `docs/变更归档.md`（索引表）→ `docs/归档/`（分卷正文） |
| 查只能真机验的验收项 | `docs/人工验证清单.md` |
| 改完怎么归档 | 本文件「归档流程」 |

**顺序：先读规则 → 改代码 → 验证 → 归档。** 铁律是底线，细节去上表查。

> **⚠ 编码**：含中文的源码 / 配置 / 文档只用 Read/Write/Edit；**禁用 PS `Get-Content` / `Set-Content`**（PS 5.1 的 GBK 毁 UTF-8）；PS 只跑 cargo / npm / demo。

## 1. 铁律

> 每条违反了都是 bug，不是风格问题。括号里是细则所在小节，机制与「为什么」在那边，**别在本文件复述**。

### 1.1 配置与落盘（`src-tauri/src/config_io.rs` / `config_watch.rs`｜说明书 §5.6）

- **保存 = 原子写**（临时文件 → `fsync` → `rename`，留 `.bak`），**不要退回 `fs::write`**
- **解析失败绝不静默清空**：留档 → `.bak` 恢复；全失败才空配置 + 告警
- **加载只 `migrate()`，不 `sanitize_config()`**（清洗只在保存 / 导入路径）
- **导入必须选方式**：`merge` 只增不删、不造新冲突、**设置保留本机**；`replace` 覆盖前必须 `archive_copy` 留档
- **删除已保存条目要先确认**（`confirmDelete`）；未落盘的新增项可不问
- **外部修改监听**：**能解析才采纳、解析不动就别碰磁盘**（只读 `read_only`）；写盘与 `adopt_own_write` 同持 `cfg_watch` 锁；界面收到 `config-changed` 且有未保存草稿必须问用户
- **`config-updated`（应用内写盘）与 `config-changed`（外部改动）语义分离，不可混用**
- **同步存档**（`settings.sync_dir`）：每条写盘路径后 `push_sync` 一份——**新增写盘路径别漏**

### 1.2 模型与判重（`crates/kada-core/src/lib.rs`｜说明书 §2 / §3）

- `Remap` 四类形态（to / tap-hold / 切层键 / `oneshot`·`sticky`·`tap2`·`tap3`）**互斥**，优先级 `sticky > oneshot > tap-hold > to`；`hold_layer` 与 `lock_layer` **互斥**（同设保留 lock）
- `when` 不成立 = **该条不存在**（落同触发键下一条、不吞键）；**判重指纹含 `when`**
- `Layer.mark` 纯展示；**颜色本机唯一**
- **层语义**：激活层条目优先、基础层条目兜底；**momentary 层只在 roll 或按住切层键期间按别的键时真正进入**

### 1.3 吞键（不可逆｜架构设计 §3.1）

被吞掉的键补不回来。**「吞掉」只允许发生在「还在等下一个键来凑齐」的窗口内**；**没组成快捷键的按键必须按用户输入顺序原样回放**——和弦成员全抬起仍未凑齐、序列断链或超时、tap-hold 两条无输出路径（短按没设 `tap`、长按没设 `hold`，含切层键与 momentary 单按）。

### 1.4 钩子线程（`crates/kada-hook/`｜说明书 §4.1 / §4.2 / §4.3、架构设计 §3.6）

- **回调里绝不做耗时操作、不建窗口、不碰 IO 网络**：唤窗、动作执行、注入一律 `std::thread::spawn`；判定用**不持有 `AppHandle` 的纯函数**
- **注入事件一律放行**（Windows `LLKHF_INJECTED`）防回环；**`Block` / `Replace` 登记 `SWALLOWED`** 连 keyup 一起吞，但**所有 keyup 仍要回调 handler**
- **鼠标钩子按需安装**（`set_mouse_enabled` + `config_needs_mouse_hook`）：用到鼠标键（含滚轮）**或**有启用的手势 / 角动作 / 边缘滚动才挂；**手势不算鼠标键**，别混进 `config_uses_mouse`
- **滚轮只在绑了触发键时接管**（`config_uses_wheel` → `set_wheel_enabled`）：120 累积一格、换向清零；没绑就原样放行。**边缘滚动只在贴边时旁听、不吞滚轮**（`wheel_edge`）
- **macOS 辅助功能是运行时授权**（`AXIsProcessTrusted()`），未授权 `start` 报指引 + 弹授权框；**媒体键走独立 `NX_SYSDEFINED` tap**
- **Linux 不抓触摸板 / 指点杆**（抓了等于废掉整个触摸板）
- **输入层起不来的处置**：Windows 失败退出；macOS / Linux **照常启动**，把 `start` 的错误串经消息中心推给用户

### 1.5 注入（说明书 §4.4 / §4.7｜架构设计 §3.23）

- **动作执行前必须 `wait_modifiers_released`**（≤300ms 轮询）等物理修饰键释放，否则注入会被仍按住的触发修饰键污染；**新增注入类动作要同步壳层 `injects_keys`** 并接入链内注入锁
- **提权窗口注入**：**新增注入路径必须经 `deliver()`**（失败回落本地）；钩子线程标 `local_only`（回调里不许有带等待的 IPC）；提权通道只放行本用户 + 双向身份校验、**只许有注入类 op**、断开或父进程退出即退

### 1.5a 当前窗口动作（`App::CurrentWindow`，EXT-14｜说明书 §3）

- 目标 = **触发那一刻**的前台窗口（`TriggerCtx.window` 在 `fire()` 调用线程抓），**不按程序名匹配**；链里先切前台也不换目标
- 所有操作**不激活、不抢焦点**（`SWP_NOACTIVATE`、不调 `SetForegroundWindow`）；关闭发 `WM_CLOSE`，**不 taskkill**；macOS / Linux **报错 + `PlatformCaps.window_actions` 软冲突**，不许静默失败

### 1.6 状态机与复位（`src-tauri/src/engine.rs`｜架构设计 §3.2 / §3.15 / §3.16 / §3.22）

- 决策顺序固定：`taphold_step` → `chord_step` → `sequence_step` → `decide` → `hotstring`
- **超时类等待态必须由定时器线程落地**（`spawn_engine_ticker` 每 60ms 调 `Engine::tick`）——**配置读锁必须先于 `Engine` 锁**，同序否则死锁
- **录入暂停是限时租约**：`set_paused(true)` 只暂停 60 秒（`PAUSE_LEASE_MS`）后自动失效；前端失焦 / 隐藏 / 关详情时主动解除
- **全局全屏暂停**（`Settings.ignore_fullscreen`，默认关）：判定复用 `foreground_fullscreen`（盖满显示器且非最大化、排除本进程窗口与**桌面外壳 `Progman`/`WorkerW`**），与 `exclude_apps` 同层放在状态机之前，命中直接 `Allow`（不吞键）；**手势全屏让路无开关、恒生效**，两者别合并
- **输入状态复位**（`Engine::reset`）：钩子重装 / 设备拔出 / 前台切换 / **配置整份换掉** = 「与物理键盘断过一次线」。口径：**注入的收回来**、**缓冲里的丢弃不回放**、**锁定层保留**
- **自愈看门狗**：Windows **只投递 `WM_APP_REINSTALL`**、**必须先卸旧再装新**并复位运行时状态表（不卸旧 = 按键被处理两遍）；**判据缺观察者就不判**，但 **Raw Input 键盘旁路**（`LAST_RAW_KEY_TICK`）是独立观察者
- **前台 / 设备上下文**：**Linux 恒 `None`**（Wayland）、macOS 设备恒 `None`；带 `when` 的条目用 `TriggerContext`，前台**惰性查询**

### 1.7 浮窗与界面外链（架构设计 §3.21 / §3.6）

- 透明浮窗（`toast` / `hud` / `hints` / `clipboard-menu`）必须 `focusable(false)`（否则抢前台 → `ResetWatch` 误判复位）；**可交互浮窗只有 `hints` / `clipboard-menu`**：不 `set_ignore_cursor_events`、`zoom_hotkeys_enabled(false)`、**一律 `shadow(false)`**；复制菜单的触发（双击 Ctrl+C）只旁听、**不吞键**
- **提示框悬停让位**（`spawn_hints_hover_watch`）：光标进框即穿透 + 变淡，停满 2s 恢复。三条硬约束：**穿透后只能轮询全局光标位置**；**隐藏时必须真撤掉穿透标记**；穿透状态广播 `hints-peek`
- **界面外链必须由壳层接住**（主窗口 `.on_new_window`）：一律 `Deny`，只把 http(s) 交 `kada_actions::open_url` 用系统浏览器打开。**新增界面外链 / 改建窗代码时别把这段弄丢**（真机验收见人工验证清单 §21）

### 1.8 动作执行与执行记录（`crates/kada-actions/`｜说明书 §5.4 / §5.7、架构设计 §3.17）

- 命令 / 脚本**不许 `Command::output()` 无限等**——走 `run_with_limits` 边读边等；超时（`settings.action_timeout_ms`，默认 30 秒、0=不限）/ 中止即**杀整棵进程树**；**中止 = 代数计数器** `abort::request()`
- **并行动作** `Parallel`：成员各起一线程、全部结束才进下一步；**链内注入锁**保注入串行；`vars` 成员私有、按下标合并提交
- **禁用的动作**（`Action::Disabled`，删除键左侧）：内容保留，执行时**整条跳过、不占执行记录**，校验落到里面（说明书 §5.7）
- **每个动作都要上报一步 `ActionStep`**（文本 / 按键 / 延迟也必须有）。一次触发 = 一条 `RunRecord`（`fire` 先落 `running`，收尾定 `ok` / `error` / `abort`）；**「停止执行」只在 `running` 记录的「执行中」徽标右侧**
- **普通消息**（`run_id === null` 才进列表；内存态、重启清空）：命令结果 / 中止 / 失败 / 配置事件都记入；`show_output` 开则弹结果弹窗，否则托盘红点。**定长上限**：200 条、单条 stdout/stderr 各 16KB（`MAX_RESULTS` / `truncate_text`）

### 1.9 卡带 / 分享（`crates/kada-core/src/snippet.rs`｜说明书 §6.7）

- **卡带必须仍是合法的 `Config` JSON**（顶层多个被忽略的 `kada_snippet` 键）——对方靠现成的「导入 → 合并」吃下，**不要另立格式 / 解析器**
- 摘取时**剥掉 `folder`**、带上被引用的层、**不带 `settings`**
- **市场静态、发布双审**：索引 = 静态 JSON；发布本机只提示，ihomy 复检 + 人工通过；等级 0~3、法务只标记；社区索引独立文件，仅官方索引合并

### 1.10 feature 门控

- 三个 crate 都默认开启 `automation` feature；关掉（`--no-default-features`）得**基础版** = 改键全形态 / 层 / 序列 / 和弦 / 文本扩展 / Text·Keys·Mouse·PauseMs 注入，裁掉 Command / Os / System / App / OpenUrl / If / Script / Condition / Vars
- **门控要覆盖测试目标**：用到 `Condition` / `Action::If` / `Parallel` 的测试与辅助函数（如 `linux_caps()`）必须自己也 `#[cfg(feature = "automation")]`，否则 `cargo test -p kada-core --no-default-features` 编译失败

### 1.11 前端键表须与 core 同源

`ui/src/main.ts` 的 `KEY_OPTIONS` 须与 `kada-core` 的 `Key::ALL` 一致且**保持显式字面量**（`frontend_key_table_matches_core` 守着）。

### 1.12 软冲突（说明书 §3 / §4.6 / §4.8）

- **平台能力缺失**（`PlatformCaps`）与**层可达性**（层有条目却无切层键指向）都属软冲突：`get_conflicts` 标 `Warn`，**不拦保存**
- **输入层环境诊断**也属软冲突，但**只在配置用到鼠标键时才出**；`settings.input_env_diag` 关掉**连探测都不跑**；`settings.input_env_ignored` 按**单个接管者**掐提示
- **罗技设备可一键解锁**：只交回标准报文、**不接管事件流**

### 1.13 鼠标操作（总开关｜说明书 §5.9）

- 按住右键画轨迹 → 方向串。**右键按下先吞、抬起才判定**：命中执行动作；**未命中 / 没画出轨迹一律回放一次右键**（注入不回环）
- **右键画圈 ≠ 截图**：画圈后**右键不松再点左键**才确认（`on_left_down`，左右键都吞）→ 撤浮层后模拟 Snipaste `F1` 叫出它的截图界面（不抓屏、不写剪贴板；没装只提示）；只画圈回放右键；截图动作已删
- 识别在壳层做（钩子回调只记点），手势**不进 `Key` 模型、不进 `Engine::step`**；**轨迹出口只借用不取走**
- **让路**（都算「不接管、回放右键」）：排除程序 `gesture_exclude` 命中、**前台全屏**（占满显示器且非最大化）、`when` 不成立（落到下一条）
- **触摸板手势是独立分类**（`touchpad_gestures`，**不许塞回 `gestures`**；老 `trigger` 由 `migrate` 搬）：按住**键盘键**采集，**按下即吞、没命中不回放**（纯开关）；`Esc` / 停手 >3s / 双指 = 作废但抬起仍吞；「录一笔」**复用同一 sink + 强制开闸**、**只回填不执行**；松键后光标回起笔点
- 总闸 `Settings.experimental`（默认关）：只管鼠标类，关掉 = 入口隐藏、不生效、**配置保留**；触摸板：`Settings.touchpad`。

### 1.14 软件更新（说明书 §5.5）

- **私钥绝不入库**（公钥内嵌，私钥只在 `~/.tauri/kada.key` 与 CI Secret）；**`latest.json` 由 `tauri-action` 生成，不手写、不入库**；端点只认**已发布** Release；**前端碰不到更新**（capabilities 不开 `updater:*`）
- **三步分开**：检查永远自动（启动 8 秒 + 每 24h）；下载 / 安装由 `Settings.auto_update`（**默认关**）决定——勾了即自动下载、空闲 ≥10 分钟（`idle_seconds()`；Linux 恒 `None`，**绝不据此安装**）或便携版重启时装。落盘 `updates/pending.json`（形态不符即丢），重启恢复 `ready`；安装前重查清单版本
- **不改安装形态**：exe 目录 == 注册表 `HKCU/HKLM\Software\kada\Kada` 才是安装版；否则便携版原位替换（改名→落新→重启，失败回滚）。**绝不交叉**；检查走 `build_updater`，**别用 `app.updater()`**
- **安装会退出应用**；其它平台装完 `app.restart()`

### 1.15 界面文案（i18n｜说明书 §6.9）

- `ui/index.html` / `ui/src/main.ts` 里的中文**一律** `tr("…")` 或 `trt\`…${x}…\``（key = 中文原文／占位符 `{0}`）；新增文案**必须同时补 `ui/src/i18n.en.json`**，否则 `check-i18n` 红
- **不包**：拿后端中文文案做前缀匹配、与配置数据比较的字面量——行尾标 `// i18n:ignore`
- 语言只记本机 `localStorage["kada.locale"]`，**不进配置 / 不同步**；壳层（Rust）文案仍是中文（已知缺口，7.2-㊲）

### 1.16 主窗口标题栏（说明书 §6.10）

- **平台分两路**：Windows / Linux `decorations(false)` + **`shadow(true)`**（无投影会丢 DWM 缩放边 / 圆角 / 最大化）；macOS `TitleBarStyle::Overlay`，红绿灯不许自绘
- **拖动 / 双击最大化走 `data-tauri-drag-region`**；关闭键调 `close()` 别用 `hide()`
- **Linux 挂八条 `resize-zone`**（GTK 无边框窗无系统缩放边）；`core:window:allow-*` 不在 `core:default` 里，要显式放行

## 2. 命名与键模型

| 项 | 值 |
|----|----|
| workspace members | `crates/kada-core` / `crates/kada-hook` / `crates/kada-actions` / `src-tauri` |
| Tauri crate 名（`src-tauri/Cargo.toml`） | `kada` |
| npm 包名（`ui/package.json`） | `kada-ui` |
| Tauri `identifier` | `com.kada` |
| `productName` / 窗口标题 / 应用显示名 | `Kada` / `咔哒 Kada` |

- **键模型使用平台无关中性名**：`Enter/Escape`（不是 `Return`），修饰键 `Ctrl/Alt/Shift/Meta`（macOS ⌘ 由 `Meta` 承载，平台层映射）。键名渲染回 `"Ctrl+Alt+K"` 形式（说明书 §2）
- **滚轮是离散键**（`WheelUp`/`WheelDown`）：一格 = 一次按下+抬起，不能作和弦成员 / 按住类触发；高精度滚轮累积到 120 才算一格（说明书 §5.10）
- 平台代码一律 `#[cfg]` 门控，三后端同一套接口；目录树见 `docs/README.md`

## 3. 构建与验证命令

```powershell
cargo test --workspace      # 全 workspace 测试
cargo check -p kada         # Tauri 壳编译（完整版）
cargo check -p kada --no-default-features  # 基础版编译（裁掉 automation）
cargo build --release -p kada --features custom-protocol   # 手动打 exe：**必须带 custom-protocol**（漏了只嵌壳不嵌前端 → 白屏）
npm --prefix ui run build   # 前端构建
node scripts/check-docs.mjs # 文档一致性（路径引用 / § 消歧 / 代码地图令牌 / AGENTS 体积）
node scripts/check-i18n.mjs # 界面文案：中文必须包 tr()/trt()，i18n.en.json 覆盖全部 key
cargo run -p kada-hook --example demo   # 冒烟 demo
cargo check -p kada-hook --target aarch64-apple-darwin --all-targets  # macOS 侧唯一可用的本地验证
powershell scripts\clean-build.ps1 # 清 debug / 交叉产物（-Bundle 含安装包）
```

- **改完 Linux 分支必跑**：`cargo check -p kada-hook -p kada-actions --target x86_64-unknown-linux-gnu --all-targets`——壳分支只有 CI / 容器（`scripts/linux-package/`）拦得住
- **macOS 本地验证边界**：darwin 交叉 check 只能真编 `kada-hook`（`-p kada` 必失败，`ring` 要真 clang）——壳层分支只能靠 CI / 真机验
- **打包**：产物在**仓库根** `target/release/bundle/`（打包前先退出 Kada）。须签名（缺则拒装）：`npm run tauri -- build --bundles nsis` + env `TAURI_SIGNING_PRIVATE_KEY`=密钥**文件全文**、密码空串。**Linux 包只能靠容器**、**macOS 只能靠 CI**
- **发布链路版本红线**：`requireSignedVersion = true` 要求签名里带版本号，**tauri-cli < 2.12.1 不写**（客户端全拒装 `MissingSignedVersion`，流水线还不报错），根 `@tauri-apps/cli` 锁 `^2.12.1`；**动 CLI 版本 / 版本号 / targets 后必跑** `node scripts/verify-release.mjs`
- **CI**：`ci.yml`（三平台：前端构建 + `cargo test --workspace` + 两版 `cargo check -p kada` + 文档检查）；`release.yml`（`v*` tag 或手动触发；Windows NSIS + 便携 zip + Linux deb/AppImage + macOS dmg，串行、**根目录 `npm ci` 不能省**）

## 4. 归档流程

- **每次改完就归档**（验证通过后），别攒批——会话可能超长失败，**没归档的改动会随会话一起丢**；多会话并行时未提交的改动会被回退
- **分流**：`AGENTS.md`（新规则 / 踩坑 / 约定）｜ `docs/需求设计说明书.md`（功能现状，**只改模块小节、不写版本叙事**，正文引模块小节号而非版本号）｜ `docs/变更归档.md`（**索引表加一行**）+ `docs/归档/`（当前卷正文追加 `#####` 条目，标题带版本号）｜ `docs/架构设计.md`（结构变了才动）｜ `docs/人工验证清单.md`（只能真机验的项，跑完回填顶部状态表）
- 落点细则见 `.agents/skills/archive/SKILL.md`（`/archive` 技能）
- **⚠ Git 规定**：非人工指令**不得主动提交代码**（`git commit` / `git push` 一律禁止）；`git add` 只能指定具体文件路径（**禁 `git add -A` / `git add .`**）。**例外（用户已授权）：归档自动提交**——归档流程写完全部文档即自动 `git commit` + `git push`；`git add` 仍只给具体路径
- **交付范围 = 代码 + 本次文档更新**，别只提交代码

## 5. 文档导航

| 文档 | 一句话 |
|---|---|
| `docs/代码地图.md` | **症状 → 文件 → grep 令牌 → 规则出处**（定位代码先看它） |
| `docs/需求设计说明书.md` | 功能现状唯一活文档（字段级定义 / 接口 / 规划清单 说明书 §7） |
| `docs/架构设计.md` | 分层 / 事件流转 / 关键机制为什么这样设计 |
| `docs/变更归档.md` + `docs/归档/` | 已实现变更：索引表 + 分卷正文 |
| `docs/人工验证清单.md` | 只能真机 + 真人操作才能确认的验收项 |
| `docs/README.md` | 全部文档索引 + 目录树（哪个文件装什么） |
| 根 `README.md` | 公开门面：项目简介 / 特性 / 技术栈 |

- **§ 小节引用必须带文档名**（`说明书 §5.6` / `架构设计 §3.1` / `人工验证清单 §21`）——三份文档编号空间重叠，裸 `§N` 会指错；`scripts/check-docs.mjs` 盯着这件事
- **落点分工**：规则 / 约定 / 命名 → 本文件；功能需求与规划 → 需求设计说明书；实现历史 → 变更归档；代码事实以源码为准，**先 `grep` 再动手**
- 本机已装 Rust / Node / Tauri CLI；前端依赖 `npm --prefix ui ci`
