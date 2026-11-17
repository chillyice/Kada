# Kada Android 探针（S0 + S1 + S2）

Android 版前几个阶段的**探针工程**（docs/安卓版规划.md §6 / §7 的 S0 / S1 / S2 / S2b）：

- **S0**：纯 Kotlin + 无障碍服务 + 前台服务，把「安卓上到底能不能收键 / 吞键 / 判定 / 常驻」用真机数据证伪或证实。
- **S1**：判定搬进 Rust（`crates/kada-android` 复用 `kada-core`，与桌面同一份规则），配置落盘复用 `kada_core::config_io`，导入 / 导出走 SAF。
- **S2**：手机档——悬浮球 / 快捷面板 / 磁贴 / 通知 + 音量键，四类触发源共用一个总闸与一条动作执行链。
- **S2b**：画符号——长按悬浮球画一笔（17 个预设符号 + 自己录的符号），识别后执行绑定的快捷键。

目的仍然不是做产品，而是把每一条「安卓能不能」的结论钉在证据上。

## 目录

```
android/
  app/src/main/java/com/kada/spike/
    MainActivity.kt          S0 验收台（状态 / 开关 / 动作表 / 统计 / 延迟 / 事件流 / 报告）
    KeyFilterService.kt      无障碍服务：onKeyEvent → Rust 判定（S1）/ KeyRouter（S0）→ 吞 or 放行
    KeyRouter.kt             S0 的极小判定器（单键 / tap-hold / 和弦），只用于对照实验
    SemanticActions.kt       §3.2 语义动作表（ACTION_* / GLOBAL_ACTION_*）
    RustBridge.kt            JNI 桥（System.loadLibrary("kada_android") + 静态方法声明）
    RustPanel.kt             S1 面板：报告、触发键清单、导入 / 导出（SAF）、执行日志
    ShortcutEditor.kt        S1-d 最小配置编辑器：列表 + 编辑对话框 + 录入按键 + 删除确认
    ShortcutStore.kt         编辑器数据层：唯一落盘入口（先 Rust 校验触发键，再整份 save）
    TriggerCapture.kt        触发键录入：无障碍回调里截按键，录到的那个键连抬起一起吞
    Update.kt                S1-f 侧载自更新：清单 → 下载 + sha256 校验 → PackageInstaller 会话
    UpdateReceiver.kt        系统安装器回执（成功 / 被取消 / 失败原因）转给 Update
    ActionRunner.kt          把 Rust 返回的「命中条目」变成动作：open_url / text / clipboard / pause / 禁用
    KeyNames.kt              安卓 keycode ↔ core `Key` 名（必须与 core 逐字一致）
    BubbleService.kt         S2 悬浮球 + 快捷面板（+ S2b 长按开画板）
    DollarOne.kt             S2b $1 识别器（重采样 / 归一化 / 旋转 + 方向扇区 / 起点与笔顺不敏感）
    DrawStore.kt             S2b 符号表 / 绑定 / 执行 / 自检（预设 17 个符号，13 个形状类别；+ 自定义画法 max 24）
    DrawPadView.kt           S2b 画板（收手指轨迹，抬手归一化到 0~1）
    DrawBoardView.kt         S2b 画板浮层（长按悬浮球弹出，点空白取消）
    KadaTileService.kt       S2 快捷设置磁贴（亮 = 接管 / 灰 = 暂停）
    HeartbeatService.kt      前台服务 + 常驻通知 + 心跳日志（S0-4）
    TierProbe.kt             档位探测（root / /dev/uinput / 物理键盘）
    SpikeState.kt            进程内状态（开关 / 统计 / 事件环 / 延迟 / 动作结果 / 焦点）
    SpikeReport.kt           导出 kada-s0-report.json（S0 + S1 共用）
    ControlReceiver.kt       adb 控制入口（`android.permission.DUMP` 门控：只有 shell / system 能发）
  app/src/main/jniLibs/<abi>/libkada_android.so   cargo-ndk 产物（不进 Git）
  tools/build-rust.ps1       编 Rust 动态库（cargo-ndk，Gradle 不编它）
  tools/verify.ps1           自动化验收（构建 / 安装 / 开无障碍 / 逐条测 / 读报告）
  gradlew.bat                Gradle 9.1.0 wrapper（首次运行需要联网下载发行包）

crates/kada-android/       Rust 侧（cdylib + rlib，桌面也能 `cargo test`）
  src/lib.rs               JSON API + JNI 符号（Java_com_kada_spike_RustBridge_native*）
  src/engine.rs            安卓判定引擎：非破坏式吞键、序列 / 和弦等待窗、foreground 过滤
  src/action_support.rs    动作支持表（**唯一事实来源**，Kotlin 不许再维护第二张）
  src/report.rs            配置报告：条数 / 触发键分类 / 不支持项 / 软冲突 / 告警
```

## 构建

两步，**顺序不能反**（Gradle 不编 Rust，`jniLibs` 里的 `.so` 缺了就会 `System.loadLibrary` 失败）：

```powershell
# 1. Rust 动态库（需要 NDK；SDK 路径从 android/local.properties 读）
powershell android\tools\build-rust.ps1 -Abi x86_64          # 模拟器
powershell android\tools\build-rust.ps1 -Abi arm64-v8a       # 真机
powershell android\tools\build-rust.ps1 -Abi all             # 三个 ABI 全编

# 2. APK
cd android
.\gradlew.bat --offline assembleDebug
```

产物：

- `android/app/src/main/jniLibs/<abi>/libkada_android.so`
- `android/app/build/outputs/apk/debug/app-debug.apk`

> 本机（无外网）**只能**用缓存里有的 Gradle 9.1.0 + AGP 8.13.0 + Kotlin 2.2.20。
> 插件走 `buildscript classpath` 而不是 `plugins {}`：AGP / Kotlin 的插件 marker 不在
> 本机缓存里，离线解析会失败（见 `android/build.gradle.kts` 注释）。
> `verify.ps1` 默认会先跑 `build-rust.ps1` 再跑 Gradle；`-SkipBuild` 跳过这两步。

## 装到设备

```powershell
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
adb shell settings put secure enabled_accessibility_services com.kada.spike/com.kada.spike.KeyFilterService
adb shell settings put secure accessibility_enabled 1
adb shell am start -n com.kada.spike/.MainActivity
```

## 发布（侧载渠道）

1. **三个 ABI 全编**：`powershell android\tools\build-rust.ps1 -Abi all`（只编 `x86_64` 的包装到真机会
   `System.loadLibrary` 失败）；再 `cd android; .\gradlew.bat --offline assembleDebug`。
2. **记账**：`Get-FileHash app\build\outputs\apk\debug\app-debug.apk -Algorithm SHA256` 与字节数；
   用 `aapt2 dump badging` 确认 `versionCode` / `versionName` 与 `build.gradle.kts` 一致。
3. **传包**：`scp -P 19068 app\build\outputs\apk\debug\app-debug.apk root@ihomy.top:/opt/ihomy/uploads/kada/kada-android-<版本>-debug.apk`，
   远端 `chown ihomy:ihomy` + `chmod 644`，**旧的删掉**（同一地址只留最新，靠清单指版本）。
4. **写清单** `/opt/ihomy/uploads/kada/android/latest.json`（`/files/kada/**` 免登录）：
   `{"versionName","versionCode","url":"https://ihomy.top/files/kada/kada-android-<版本>-debug.apk","sha256","notes"}`。
   **清单与 APK 必须同批更新**——sha256 对不上，客户端下载完直接删包。
5. **验**：`HEAD` 直链 200 + 长度一致、匿名 `GET` 清单可读、旧版本 404；设备上「检查更新」应报
   `phase=available`（同版本则是 `latest`）。

> ihomy 落地页（`frontend/src/views/Kada.vue`）的 `androidVersion` 与卡片 `meta` 也要同步改；
> 工作区有并行改动时**别整包重发 dist**，按需定点替换线上 chunk（见 `docs/归档/03-界面与发布.md`）。

## 界面里能做什么

### S0 段（硬编码演示规则）

- **状态**：无障碍是否连上（含 `filterKeyEvents` 标志）、前台服务存活、档位探测、前台包名、输入设备。
- **触发实验**：F8 单键 → 气泡；F7 → 复制；F9 短按 → 气泡 / 长按 → 最近任务；
  `Ctrl+Alt+K` → 通知栏；音量上+音量下 → 返回（勾选后）。另有「吞 A」「吞音量键」
  「和弦全吞式」三个对照开关。
- **吞键端到端验证**：界面里有一个输入框，用 `adb shell input text` **测不出来**
  （注入事件不进无障碍过滤，见下），要用 `sendevent` 打物理键。
- **语义动作**：§3.2 的 16 条动作逐条点（14 个 core 的 `SemanticOperation` + `set_text` / `toast` 两条探针自用），
  结果写进报告（标注 ROM 可用性）。
- **统计 / 延迟 / 事件流**：按下 / 抬起 / 连发 / 未配对抬起、吞 / 放行、动作成功失败、
  P50/P95、最近 14 条事件。
- **导出报告**：`/sdcard/Android/data/com.kada.spike/files/kada-s0-report.json`（`adb pull` 拿走）。

### S1 段（配置驱动，在 S0 段下面）

- **用 Rust 判定**开关：关掉退回 S0 的硬编码演示规则（对照实验用，状态会持久化）。
- **配置**：首启自动生成安卓示例配置到 `/data/user/0/com.kada.spike/files/config.json`
  （5 条：单键 / 组合 / 序列 / 和弦）。落盘走 `kada_core::config_io`（原子写 + `.bak` + 损坏留档）。
- **导入 / 导出**：SAF 选文件。导入有两条路，缺一不可：
  - **导入·合并**：只增不删（往里加卡带），设置保留本机；
  - **导入·替换**：整份换掉（把桌面那一份原样搬过来，出厂示例 5 条随之消失），
    弹框二次确认，替换前 Rust 侧先 `archive_copy` 留 `config.json.pre-import-<时间戳>`。
  导入完成会在面板上写一行「快捷键 5 → 20 条 + 留档文件名」，不靠 Toast 猜。
- **恢复出厂**：删掉本机 `config.json`、重新落回示例 5 条。删之前壳层先留档
  `config.json.pre-reset-<时间戳>`（`resetToSample` 自己不归档，这一份由 `RustPanel` 补）。
- **报告**：条数、触发键分类（combo / sequence / chord）、**本机不支持项**
  （`remaps` / 层内条目 / `expansions` / 手势 / 桌角 / 边缘滚动）、逐条不支持的动作类型、
  软冲突、告警、`notes`（含吞键策略说明）。
- **执行日志**：最近命中的快捷键、动作结果、失败原因（报告里 `actions.<id>.lastFailDetail` /
  `lastFailAtMs` 只记失败、成功不覆盖——真机出现「成功过但失败过」时也能看到错在哪）。
- **快捷键编辑器（S1-d）**：面板末尾 + 顶部「快捷键编辑器」按钮（滚过去）。新建 / 编辑 / 删除一条，
  字段只有「名称 / 触发键 / 动作类型 / 参数 / 启用」；动作类型只给 `open_url` / `text` /
  `clipboard` / `semantic` / `pause_ms`，别的类型一律 `ACTION_KEEP`（**改触发键不动动作链**）。
  - 触发键解析与撞车判定在 **Rust**（`Trigger::parse_loose`：严格规则失败后只补正大小写，
    `ctrl+alt+k` → `Ctrl+Alt+K`；重复触发键是 core 的**硬冲突**，编辑器拦保存）；
  - 保存走 `ShortcutStore.save()` → `RustBridge.save` → `sanitize_config` + `config_io`：
    与桌面同一条落盘路径（原子写 + `.bak`），保存即生效、不用重启；
  - 「录入按键」在下一次按键时截（`TriggerCapture`），录入期间整条判定跳过、按键全吞——
    否则录 F8 会先把 F8 原来的快捷键触发一遍；录到后连它的抬起一起吞，安全阀是「新的非修饰键按下即放弃」；
  - 同一条目有多个触发键时**只改第一个**，其余原样保留（对话框里有提示）。
- **文本动作落地（S1-e）**：`text` 在**当前光标处插入**，原有文本保留——
  首选 `ACTION_SET_TEXT` + 选区参数（节点级写入，**不碰剪贴板、不依赖输入法**）；
  节点不接受时才退回「写剪贴板 + `ACTION_PASTE`」，并把原剪贴板**尽量恢复**
  （Android 10+ 后台读剪贴板受限，读不到就在结果里如实写「已被覆盖」，不假装成功）。
  `clipboard/tempstr`（临时文本）走同一条插入路径。非可编辑节点 / 没有输入焦点时退回粘贴。
- **自更新（S1-f，侧载）**：面板里「检查更新 / 安装已下载」两个按钮 + 一行状态。
  - **清单**是唯一事实来源：`https://ihomy.top/files/kada/android/latest.json`
    （`versionName` / `versionCode` / `url` / `sha256` / `notes`）；版本比较只用 `versionCode`，
    缺字段直接报错——**不猜默认值**（缺 `versionCode` 的清单会永远「没有新版本」）。
  - **下载必须校验**：清单给了 `sha256` 就逐字节算 SHA-256 比对，不匹配**删包 + 失败**；
    没给就在结果里写「未校验」，不假装验过。
  - **安装走 `PackageInstaller` 会话**（不自己拼 `file://` Intent，避开 `FileUriExposedException`）：
    提交后由系统安装器弹确认框，用户点「安装」才生效；没授权「安装未知应用」时回报
    `need_permission` 并引导去设置——**不许静默失败**。写会话在后台线程做（30 MB 拷进会话会 ANR）。
  - 回执（成功 / 被取消 / 失败原因）由 `UpdateReceiver` 收进状态行，报告里也有 `update` 段。

判定口径（S1-a）：

| 配置块 | 安卓无 root 档 | 说明 |
|---|---|---|
| `shortcuts`（combo / sequence / chord） | ✅ 成立 | 与桌面同一份 `Trigger::parse` + `match_shortcut` |
| `shortcuts` 里的 `open_url` / `text` / `clipboard` / `pause_ms` / `semantic` / `group` / `disabled` | ✅ 执行 | 支持表只在 `crates/kada-android/src/action_support.rs` |
| `remaps`（含 tap-hold / oneshot / sticky / 切层键） | ❌ 报告标记 | 要注入目标键，无 root 做不到；**绝不吞**，只留 `remap:unsupported:*` 痕迹 |
| `layers` / 层内快捷键 | ❌ 报告标记 | 层只能由改键激活 |
| `expansions`（打字触发） | ❌ 报告标记 | 软键盘输入不产生 `KeyEvent` |
| 其它动作（命令 / 脚本 / 按键注入 / 应用 / 变量 / 条件…） | ❌ 报告标记 | 整条跳过、不占执行记录 |

### S2 段（手机档：悬浮球 / 快捷面板 / 磁贴 / 通知）

手机没有物理键盘，「点一下」才是主入口。四类触发源都走**同一个执行器**（无障碍服务里的动作链），
磁贴 / 通知 / 面板 / adb 只改**同一个总闸**（`SpikeState.swallowMaster`）。

- **悬浮球 + 快捷面板**（`BubbleService`）：要「显示在其他应用上层」权限，没有就说明原因 + 跳设置页；
  悬浮球 `TYPE_APPLICATION_OVERLAY`，可拖动、位置持久化；点一下列出**启用中的快捷键**，
  点条目执行动作链（`text` 插进前台应用当前光标处、`open_url` 拉起浏览器）。
  面板与球都是 `FLAG_NOT_FOCUSABLE`——**不抢焦点**，目标输入框的输入法不会掉。
- **快捷设置磁贴**（`KadaTileService`）：亮 = 已接管、灰 = 已暂停；点磁贴翻转总闸。
- **常驻通知**（`HeartbeatService`）：标题显示「已接管 / 已暂停」，按钮「暂停接管 / 恢复接管」。
- **暂停 = 原样放行**：`KeyFilterService` 在 **Rust 判定之前**看总闸，关掉时按键一个都不吞、
  快捷键也不触发（以前总闸只挡 S0 的 `KeyRouter`，Rust 接管时按了没反应——那是最难解释的故障）。
- **音量键当触发源**：无障碍能收到音量键，配成 `shortcuts` 的触发键后**命中即吞**、音量不变；
  没绑条目时照常调音量。

### S2b 段（画符号：单笔画手势当触发源）

面板能列出的条目有限，手机上「想马上用一个不常用动作」还要滑面板。画符号是单手指能做到、
又能一眼记住的入口：**长按悬浮球 → 画一笔 → 抬手执行**。

- **识别器**（`DollarOne`）：$1 Unistroke Recognizer 的 Kotlin 版（纯计算、零依赖、单笔画）。
  与论文原版三处差异都是为「四个方向的三角」这类需求服务的：
  1. 旋转搜索放到**整圈**（原版只在 ±45° 内微调），再拿最优旋转角 θ 判方向；
  2. 起点 / 笔顺不敏感——**反向**对全部形状生效，**循环移位**只对闭合回路（圆 / 菱形 / 星 / 三角）
     生效（开放折线移位后是另一条乱线）；
  3. 方向按 **90° 扇区**量化，θ 落在扇区中心 ±35° 内才算数；落在两扇区中间（约 45°）
     直接返回「方向不明确」——**猜错方向比不识别更糟**。
- **符号表**（`DrawStore`）：17 个预设符号 / 13 个形状类别：○ 圆、◇ 菱形、☆ 五角星、✓ 勾、
  △▽◁▷ 四向三角、V、Z、S、L、C、U、M、W、N。**方向只在有多个符号的形状上有意义**
  （圆 / 菱形 / 星 / 勾的 θ 不稳定，不判方向）。
- **绑定**：符号 → **现有快捷键的条目名**（不新建动作链，复用面板 / 音量键那条执行链）。
  绑定落本机 `SharedPreferences("kada-draw")`，**暂不进 `Config`**（进配置要动 core 的模型与卡带格式）。
- **自定义画法**（P2，面板「录自定义」）：13 个预设不够用时，用户可以**教它认自己的符号**——
  画一次 + 起个名，模板就是他画的那一笔。三条取舍：
  1. 存**原始笔画**（0~1、四位小数）而不是归一化后的 32 点，以后改归一化口径老符号能跟着走；
  2. 自定义符号**不判方向**（θ 对随手画的形状不稳，也没必要），所以匹配只比形状；
  3. 与已有符号「几乎一样」（分 ≥ 0.90）的笔画**直接拒收**——两份模板都留着，每次识别都会在边界上抖。
  上限 24 个，id 前缀 `u:`（与 13 个预设类别名不撞）；删除时**连绑定一起删**，否则留下取不出来的键。
  自定义与绑定同一个 `SharedPreferences` 键空间（`custom` / `bindings`），**仍不进 `Config`**。
- **阈值**（都得真跑一遍才作数，所以自检在设备上跑）：匹配分 ≥ 0.80、前两名差距 ≥ 0.04、
  方向容差 ±35°。只有「该方向真的有预设符号」的候选才参与竞争——否则 Z 转 90° 会被 N 挤掉。
- **自检**（`op drawSelfTest` → 报告 `phone.draw.selfTest`）：31 项——17 个预设各自带 1.2% 抖动
  认回自己、±20° 歪画仍算同向、44° 必须拒绝、四向三角用**手写坐标**独立构造（不复用旋转函数，
  免得把符号约定错在同一个地方）、换起笔点 / 反着画的圆仍命中、乱涂与过短笔画拒绝；
  外加 3 项自定义画法（临时「楼梯」模板，不落盘：抖动后命中、转 30° 仍命中、乱涂不命中）。
  故意不引 Gradle 测试依赖（离线跑不了）：这些断言的正是「旋转方向 / 阈值」这种只有真跑才作数的东西。

## adb 控制入口（探针专用）

```powershell
$ctl = @("-a","com.kada.spike.CONTROL","-n","com.kada.spike/.ControlReceiver")
# S0 开关
adb shell am broadcast @ctl --es op swallow --ez on true
adb shell am broadcast @ctl --es op demoRules --ez on false
adb shell am broadcast @ctl --es op swallowA --ez on true
adb shell am broadcast @ctl --es op swallowVolume --ez on true
adb shell am broadcast @ctl --es op chordSwallowAll --ez on true
adb shell am broadcast @ctl --es op volumeChord --ez on true
adb shell am broadcast @ctl --es op tapHoldMs --ei ms 400
adb shell am broadcast @ctl --es op startService
adb shell am broadcast @ctl --es op stopService
# S1：配置 / 判定
adb shell am broadcast @ctl --es op useRust --ez on true
adb shell am broadcast @ctl --es op rust                       # 重载配置文件
adb shell am broadcast @ctl --es op resetConfig                # 删配置 + 重新落安卓示例（探针专用，等价恢复出厂）
adb shell am broadcast @ctl --es op import --es file incoming.json --ez merge true
adb shell am broadcast @ctl --es op exportConfig               # 导出到 files/exported-config.json
adb shell am broadcast @ctl --es op decide --es key F8 --ez down true --ez reset true --es mods "Ctrl,Alt"
# S1-d 编辑器（与界面同一份 ShortcutStore；--es 的值不能带空格，带空格会被 adb 拆成多个 token）
adb shell am broadcast @ctl --es op editorList
adb shell am broadcast @ctl --es op editorAdd --es name 名字 --es trigger f4 --es action open_url --es value "https://example.com/x"
adb shell am broadcast @ctl --es op editorAdd --es name 语义 --es trigger F3 --es action semantic --es value notifications
adb shell am broadcast @ctl --es op editorCheck --es trigger "ctrl+k"     # 校验 + 大小写补正 + 撞车
adb shell am broadcast @ctl --es op editorDelete --ei index 5
adb shell am broadcast @ctl --es op editorOpen                          # 主界面滚到编辑器
# S1-f 自更新（清单可用 file:// 覆盖，脚本没外网也能跑）
adb shell am broadcast @ctl --es op checkUpdate --es url "file:///sdcard/Android/data/com.kada.spike/files/latest-test.json"
adb shell am broadcast @ctl --es op downloadUpdate
adb shell am broadcast @ctl --es op installUpdate               # 未授权时会回报 need_permission
# S2 手机档（adb 只代替手指；真实拖动 / 遮挡 / ROM 差异要手点，见人工验证清单 §93）
adb shell appops set com.kada.spike SYSTEM_ALERT_WINDOW allow   # 悬浮球权限（脚本里也这么开）
adb shell am broadcast @ctl --es op master --ez on true         # 总闸：false = 全部原样放行
adb shell am broadcast @ctl --es op toggleMaster                # 翻转总闸（ui 口径）
adb shell am broadcast @ctl --es op bubble --ez on true         # 悬浮球开 / 关（关用 false）
adb shell am broadcast @ctl --es op bubbleShow                  # 打开快捷面板
adb shell am broadcast @ctl --es op bubbleHide
adb shell am broadcast @ctl --es op bubbleRun --es name panelTpl # 等价「点一下」；名字空时用 --ei index
adb shell am broadcast @ctl --es op tileToggle                  # 磁贴同一切换函数
adb shell am broadcast @ctl --es op notifyToggle                # 通知按钮同一切换函数
# S2b 画符号（`--es path` 里的分号会被 adb shell 当命令分隔符——远端要加引号，见「坑」）
adb shell am broadcast @ctl --es op drawSelfTest                # 识别器自检（31 项）→ 报告 phone.draw.selfTest
adb shell am broadcast @ctl --es op drawBind --es symbol triangle --es name panelTpl
adb shell am broadcast @ctl --es op drawUnbind --es symbol triangle
adb shell am broadcast @ctl --es op drawList                    # 把绑定表打进 logcat
adb shell "am broadcast -a com.kada.spike.CONTROL -n com.kada.spike/.ControlReceiver --es op drawRun --es path '0.5,0.05;0.95,0.95;0.05,0.95;0.5,0.05'"
# 自定义画法：drawLearn 与「录自定义」按钮调同一个 learn()，id 形如 u:mwk9tk1b
adb shell "am broadcast -a com.kada.spike.CONTROL -n com.kada.spike/.ControlReceiver --es op drawLearn --es label stairs --es path '0.10,0.85;0.10,0.50;0.45,0.50;0.45,0.15;0.90,0.15'"
adb shell am broadcast @ctl --es op drawCustom                  # 列出已录的自定义符号（id / 名字 / 闭合 / 点数）
adb shell am broadcast @ctl --es op drawForget --es symbol u:mwk9tk1b  # 删符号并带走绑定
adb shell am broadcast @ctl --es op drawBoard                   # 打开画板浮层（与长按悬浮球同一入口）
adb shell am broadcast @ctl --es op drawHide
# 探针杂项
adb shell am broadcast @ctl --es op fire --es action screenshot
adb shell am broadcast @ctl --es op focusField                 # 把光标放进「吞键验证输入框」（自动重试）
adb shell am broadcast @ctl --es op focusInfo                  # 打印 a11y 聚焦节点与动作列表
adb shell am broadcast @ctl --es op export
adb shell am broadcast @ctl --es op snapshot
adb shell am broadcast @ctl --es op clear
```

## 自动化验收

```powershell
powershell android\tools\verify.ps1            # 构建 Rust + Gradle、装机、全部检查
powershell android\tools\verify.ps1 -SkipBuild # 只跑检查
```

脚本会打出 `[PASS] / [FAIL]` 与最后的报告路径；任何一条失败退出码非 0。
按键一律用 `sendevent` 打真实输入设备（`input keyevent` 不进无障碍过滤），所以需要 `adb root`。

脚本自己也是「可重跑」的（否则第二轮会因为上一轮的残留而漂移）：

- S1 段开头先 `resetConfig`：删掉 `config.json` / `.bak` 再让 Rust 重新落示例配置，
  于是「示例 5 条」「合并导入后 6 条」这类断言每轮都从同一个基线起跑；
- 每段按键检查前 `Restore-App`（收状态栏 + `am start`）：`open_url` 会把 Chrome 拉到前台，
  前台一切换，无障碍服务要重建输入连接，这个窗口里的按键会被丢掉；
- 按键事件一次 `adb shell` 发完（`Send-Sequence`）：每次 adb 调用有 50~200ms 抖动，
  分开发会把和弦 / 序列的成员间隔拖到超时窗之外；
- `Test-KeyFlow` 对按键类断言**失败重试一次**（模拟器偶发丢一次 `sendevent`），
  真回归会两次都失败；重试成功时打 `[warn]` 留痕。

## 已验证（2026-11-16，emulator-5554 / ihomy34，Android 14）

`android/tools/verify.ps1` **62/62 通过**（s6 版本；S2-1~S2-5 + 收尾 13 条是新增，其余 49 条与 s5 一致）。
s5 那次是 49/49（S1-3 语义动作三条、S1-4 编辑器七条、S1-5 文本动作三条、S1-6 侧载自更新五条）。
编辑器另用 `uiautomator` 真点验过整条路径：顶部「快捷键编辑器」滚到面板末尾、新建对话框字段与下拉渲染、
「录入按键」按物理 `Ctrl+F6` 回填并显示「✓ 已录入」、切「语义动作」保存后列表变 6 条
（`#5 [Ctrl+F6] semantic`）、点「删除」弹确认框、确认后回到 5 条。

S0 段（32 项检查里的 S0 部分）：

| 项 | 结果 | 证据 |
|---|---|---|
| S0-1 收键 + 吞键 | ✅ 3 个 app 端到端（Kada 输入框 / 设置搜索 / Chrome 地址栏） | 吞掉 A 后字段是 `hllo`，不吞是 `hello` |
| S0-2 tap-hold | ✅ 短按 / 长按判定正确（阈值 200/400ms 都试过） | `taphold:tap` / `taphold:hold:400ms` |
| S0-2 和弦 | ✅ `Ctrl+Alt+K` 命中；非破坏式只吞凑齐那一下 | `chord:Ctrl+Alt+K`，Ctrl/Alt 的 down 是 `swallow=false` |
| S0-2 连发键 | ⚠️ 模拟器 AT 键盘**不产生** `repeat` 事件（按 1.5s 也没有） | 真机 USB 键盘待验 |
| S0-3 延迟（服务侧） | ✅ 空载 P50 < 2 ms / P95 < 3 ms；主线程忙（刷新 + 输入法动画）时 P50 可达 115 ms / P95 491 ms | 报告 `latency`；同进程主线程排队，见下「坑」 |
| S0-3 音量键流水线 | ⚠️ 系统收到 → 回调 **~160 ms**（字母键 1~8 ms） | 报告 `events[].pipelineMs`；触发手感要真机验 |
| S0-4 常驻 | ✅ 前台服务 + 常驻通知；`dumpsys deviceidle force-idle` 深睡时按键仍响应 | `hb tick` 30s 一次；8 小时待真机 |
| S0-5 语义动作 | ✅ `GLOBAL_ACTION_*` 全部返回 true；节点动作 `set_text` / `select_all` / `copy` 依次 true | 报告 `actions` |
| 音量键可吞 | ✅ 吞掉后系统音量不变（7→7 连按 3 次），放行则变（7→11） | `cmd media_session volume --stream 3 --get` |
| 档位探测 | ⚠️ 模拟器把触屏/电源键也报成 `SOURCE_KEYBOARD`；改用 `hasKeys(A,Z,Enter,Space)` 判键盘 | 报告 `tier.inputDevices` |

S1 段：

| 项 | 结果 | 证据 |
|---|---|---|
| S1-1 库加载 | ✅ `libkada_android.so` 2.85 MB，`rust lib loaded version=0.1.0` | logcat / 报告 `rust.loaded` |
| S1-1 首启配置 | ✅ 自动生成 5 条示例（combo 3 / sequence 1 / chord 1） | 报告 `rust.report.counts` |
| S1-1 单键 F8 | ✅ 命中并执行，down/up 都吞（`rust:shortcut:F8` / `rust:up:swallowed`） | 报告 `events` + `rust.fires` |
| S1-1 和弦 F&J | ✅ 非破坏式：F 放行、J（凑齐那一下）被吞 | `swallow=false` / `swallow=true` |
| S1-1 序列 F9 Space | ✅ 命中；F9 放行，Space 被吞 | `rust:sequence:F9 Space` |
| S1-1 组合 Ctrl+Alt+K | ✅ 命中（物理 Ctrl / Alt / K） | `rust:shortcut:Ctrl+Alt+K` |
| S1-2 导出 | ✅ `exported-config.json` 5 条，与配置一致 | 报告 shortcuts=5 |
| S1-2 合并导入 | ✅ 只增不删：5 → 6（新增 F6），导入后**立即生效**（无需重启） | `config.json` + `rust.fires` |
| S1-2 替换导入 | ✅ 整份换掉，F6 随旧配置消失（6 → 5） | 报告 shortcuts=5 |
| S1-2 导入留档 | ✅ `config.json.pre-import-<YYYYMMDD-HHMMSS>` | `run-as com.kada.spike ls files` |
| S1-2 软冲突 | ✅ 配置里的 `remaps` 标为「本机不支持」，不拦保存 | 报告 `unsupported.remaps` |
| S1-2 界面替换导入（SAF） | ✅ 5 → 20 条（20 条桌面卡带，带被忽略的 `kada_snippet` 顶层键），面板显示 `20/20 启用 · 组合 20` | `uiautomator` 点「导入·替换」→ 确认框 → 文件选择器 → `kada-20.json`；面板「5 → 20 条 + 留档 `config.json.pre-import-20261113-100135`」 |
| S1-2 界面合并导入（SAF） | ✅ 同一张卡带再合并一次：20 → 20（不重复） | 面板「合并导入完成：快捷键 20 → 20 条」 |
| S1-2 恢复出厂 | ✅ 20 → 5 条示例，旧配置留档 `config.json.pre-reset-20261113-100206`（5947 B = 那张 20 条卡带） | 面板「已恢复出厂示例：快捷键 5 条」+ `run-as ls files` |
| S0-5 失败原因留存 | ✅ 同一动作「先失败后成功」：`lastOk=true` / `failCount=2` 的同时仍留 `lastFailDetail` 与 `lastFailAtMs` | 报告 `actions.select_all`（真机报告里 `failCount=4` 看不到原因，这条就是补它） |
| S1-3 语义动作算「本机支持」 | ✅ `{"type":"semantic","operation":"notifications"}` 不进 `unsupported_actions` | 报告 `unsupported_actions=0` |
| S1-3 F5 → 语义动作 | ✅ 合并导入的 F5 命中，down/up 都吞 | `rust.fires` / `events` |
| S1-3 语义动作执行成功 | ✅ 通知栏真的拉下来（`ok=True`） | 报告 `actions['rust:F5'].lastOk` |
| S1-4 编辑器新建（大小写补正） | ✅ 广播带 `f4`（小写）→ 落盘成 `F4`，条数 +1；`sanitize_config` 不再把它当坏触发键丢掉 | 报告 `editor.shortcuts[].triggers` |
| S1-4 新建条目立即生效 | ✅ 新建的 `F4` 按下就命中（无需重启） | `rust.fires` / `editor.shortcuts` |
| S1-4 编辑器写语义动作 | ✅ `type=semantic` + `operation=notifications`，列表摘要「语义动作：通知栏」 | 报告 `editor.shortcuts[].actionType/actionSummary` |
| S1-4 语义动作执行成功 | ✅ F3 命中且 `lastOk=true` | 报告 `actions['rust:F3']` |
| S1-4 撞车校验 | ✅ `F4` 已被占用时 `duplicates=#6 编辑器测试`（core 硬冲突口径，编辑器拦保存） | 报告 `editor.lastCheck.duplicates` |
| S1-4 拒绝无效触发键 | ✅ `Foo+K` → `ok=false`，错误「未知按键: Foo」 | 报告 `editor.lastCheck.error` |
| S1-4 删除条目 | ✅ 条数 −1 且按名字确认已消失 | 报告 `editor.shortcuts` |
| S1-5 编辑器写入 text 动作 | ✅ `type=text` + `text=insertme`，摘要「输入文本「insertme」」 | 报告 `editor.shortcuts[].actionType/actionSummary` |
| S1-5 光标处插入（保留原文） | ✅ 输入框原有 `abc`，F2 后变 `abcinsertme`（`lastOk=true`） | 报告 `stats.inputFieldText` / `actions['rust:F2']` |
| S1-5 不覆盖剪贴板 | ✅ F1 写 `clipmark` → F2 插文本 → F12 粘贴得到 `abcinsertmeclipmark` | 报告 `stats.inputFieldText` |
| S1-6 检查更新（清单 + 版本比较） | ✅ 本机 `0.2.0-s5`/code 6，清单 `0.2.0-s99`/code 99 → `phase=available`、`newer=true` | 报告 `update.phase/newer/lastCheck` |
| S1-6 下载并校验 sha256 | ✅ 8,221,943 B 全部落盘（`apkBytes` 与源文件一致），结果「已下载 8029 KB（sha256 已校验）」 | 报告 `update.apkBytes/lastResult` |
| S1-6 未授权安装不假装成功 | ✅ `appops … deny` 后点安装 → `phase=need_permission`、`canInstall=false`，文案「需要先允许安装未知应用」 | 报告 `update.phase/canInstall` |
| S1-6 提交安装拉起系统安装器 | ✅ `phase=installing`，前台是 `com.google.android.packageinstaller/…PackageInstallerActivity`（不点「安装」不会真装） | `dumpsys window` 的 `mCurrentFocus` / 报告 `update.phase` |
| S1-6 坏 sha256 → 失败并删包 | ✅ 清单写全 0 → `phase=failed`、「sha256 不匹配（清单 000000000000… / 实际 c72250606764…），已删掉下载的文件」、`apkBytes=0` | 报告 `update.phase/lastResult/apkBytes` |

S2 段（手机档）：

| 项 | 结果 | 证据 |
|---|---|---|
| S2-1 悬浮球启动（权限 + 前台服务） | ✅ `appops … allow` 后 `bubble on` → `running=True` / `perm=True`，常驻通知出现 | 报告 `phone.bubbleRunning/bubblePermission` |
| S2-1 面板可打开且不抢焦点 | ✅ `panel=True`；面板弹出后 `mCurrentFocus` 仍是 `com.kada.spike/MainActivity` | 报告 `phone.bubblePanel` / `dumpsys window` |
| S2-2 面板执行：插入模板 | ✅ 输入框原 `abc` → 点条目后 `abcinsertok`（`ACTION_SET_TEXT` 光标处插入） | 报告 `stats.inputFieldText` / `phone.bubbleLastRun` |
| S2-2 面板执行：打开网址 | ✅ F11 条目 `lastOk=true`，前台切到 `com.android.chrome` | 报告 `actions['rust:F11']` / `accessibility.foregroundPackage` |
| S2-3 音量键触发（命中即吞） | ✅ `VolumeUp` 命中并执行语义动作，音量保持 7（没被调） | 报告 `rust.fires` / `cmd media_session volume` |
| S2-4 磁贴注册与切换 | ✅ `dumpsys package` 见 `KadaTileService` + `BIND_QUICK_SETTINGS_TILE`；`tileToggle` 后总闸翻转（`lastMasterToggle=tile:off@…`） | 报告 `phone.master/lastMasterToggle` |
| S2-5 通知动作暂停后按键不触发 | ✅ 通知按钮暂停后按 Enter：`fires` 数量不变（3→3），`lastMasterToggle=notify:off@…` | 报告 `rust.fires` / `phone.*` |
| S2 收尾 | ✅ 关悬浮球、恢复总闸、磁贴移出 QS、测试条目删干净 | 报告 `phone.bubbleRunning=False` / `phone.master=True` |

S2b 段（画符号）：

| 项 | 结果 | 证据 |
|---|---|---|
| S2b-1 识别器自检 31 项 | ✅ `failed=0/31`、`symbols=17`；含 17 个预设带 1.2% 抖动认回自己、±20° 歪画仍同向、44° 必须拒绝、四向三角用独立手写坐标、换起笔点 / 反着画的圆仍命中、乱涂与过短笔画拒绝、自定义符号抖动命中 / 转 30° 仍命中 / 乱涂不命中 | 报告 `phone.draw.selfTest.cases` |
| S2b-2 符号绑定到快捷键 | ✅ 建条目（`F6` → text `drawok`）后 `drawBind triangle drawTpl` → `bindings={"triangle":"drawTpl"}` | 报告 `phone.draw.bindings` / `editor.shortcuts` |
| S2b-3 画三角执行绑定条目 | ✅ 输入框原 `abc` → 画三角后 `abcdrawok`，`symbol=triangle score=1` | 报告 `stats.inputFieldText` / `phone.draw.lastSymbol` |
| S2b-4 44° 三角被拒绝 | ✅ `symbol=[]`、输入框前后一致（`abcdrawok` 未变）、`lastError=方向不明确（约 -44°…）` | 报告 `phone.draw.lastSymbol/lastError` |
| S2b-5 画板浮层可开 / 可收 | ✅ `board=True` 且 `mCurrentFocus` 仍是 `com.kada.spike/MainActivity`（不抢焦点）；`drawHide` 后 `board=False` | 报告 `phone.bubbleBoard` / `dumpsys window` |
| S2b-6 录自定义符号 | ✅ `drawLearn label=stairs` → `custom=0->1`、`new=u:mwk9tk1b`、`lastError` 空（原始笔画落盘 `prefs/custom`） | 报告 `phone.draw.customIds` / `run-as … cat shared_prefs/kada-draw.xml` |
| S2b-7 画自定义符号执行绑定 | ✅ 绑到新条目（`Ctrl+Alt+J` → text `stairsok`）后，输入框原 `xy` → 画楼梯（每条边挪 0.01）后 `xystairsok`，`lastRun=draw:stairs→drawCust` | 报告 `stats.inputFieldText` / `phone.draw.lastSymbol/lastRun` |
| S2b-8 与预设几乎一样的画法被拒收 | ✅ 再画标准三角 → `custom=1->1`（没多录），`lastError=和「三角」几乎一样（1.00），换个画法` | 报告 `phone.draw.customIds/lastError` |
| S2b-9 删自定义符号带走绑定 | ✅ `drawForget u:mwk9tk1b` → `customIds` 里没有了、`bindings` 里也没有这个键（`triangle → drawTpl` 不受影响） | 报告 `phone.draw.customIds/bindings` |
| S2b 收尾 | ✅ 显式解绑 `triangle`（绑定在 `SharedPreferences`，删条目带不走它）+ 删 `F6` / `Ctrl+Alt+J` 测试条目（自定义符号在 S2b-9 已删） | 报告 `phone.draw.bindings={}` |

> 可重跑：S1 段以 `resetConfig`（恢复出厂示例）起跑，结束时把基线配置替换回去；S2 段开头申请
> 悬浮窗权限、结尾关球 + 恢复总闸 + 把磁贴移出 QS 面板 + 删掉测试条目；S2b 段额外要显式解绑
> （绑定存在 `SharedPreferences`，删条目带不走它）与删掉当轮录的自定义符号。
> 本轮 s7 全量跑 **72/72 通过**（S2b 前一版 68/68，s6 是 62/62，s5 是 49/49）；旧的 44 项（S1-e）、41 项（S1-d）与 35 项（S1-3）此前连跑一致。
> S1-4 / S1-5 / S1-6 的十五条走的是**与界面同一份** `ShortcutStore`、执行器与 `Update`
> （广播只是代替点按），控件的点击手势、录入按键、系统安装器的「安装」按钮仍属人工验证
> （`docs/人工验证清单.md` §90 / §91 / §92）。
> S2 的十三条同理：`bubbleRun` / `tileToggle` / `notifyToggle` 走的是面板、磁贴、通知按钮**同一份**函数，
> 手指拖动、软键盘遮挡、ROM 差异归 `docs/人工验证清单.md` §93。
> S2b 的十条走的是长按悬浮球弹出的**那个**画板（`BubbleService.ACTION_SHOW_BOARD` → `DrawBoardView`）
> 与「录自定义」按钮背后**同一个** `DrawStore.learn`；`drawRun` / `drawLearn` 只是把坐标串喂给同一份逻辑，
> 真手指的笔顺、录符号时的手感与认错频率归 `docs/人工验证清单.md` §94。

## 关键结论（写进规划 §11 的就行）

1. **`onKeyEvent` 必须同步回答吞不吞**——没有桌面版「先吞下、凑不齐再回放」的窗口。
   所以安卓侧的吞键策略定成**非破坏式**：`remaps` 与层**一律不吞**（也做不到），
   `shortcuts` 只吞「凑齐的那一下」，成员键照常交给目标 app。
2. **只能吞不能注入**：吞错的键补不回来。和弦必须选「成员键被吞也无所谓」的组合
   （修饰键 + 非文本键），或接受全吞式的代价。
3. **软键盘打字不产生 KeyEvent**：手机档的「打字触发」只能走面板 / 粘贴（规划 §4 定案没变）。
4. **`adb shell input keyevent` 的注入事件不进无障碍过滤**：自动化测试必须用
   `sendevent` 走真实输入设备，否则测出来的「吞键无效」是假象。
5. **音量键能被无障碍收下并吞掉**（系统音量不跟着变），但系统侧流水线约 160ms——
   手机档拿它当触发源要接受这个延迟。
6. **配置可以整块复用桌面实现**：`Trigger::parse` / `match_shortcut` / `config_io` /
   `merge_configs` / `detect_conflicts` 一行没改就跑了；安卓独有的只有「吞不吞」和「能不能注入」。

## 坑（都踩过，别再踩）

- **窗口焦点 ≠ view 焦点**：通知栏被拉下来（比如测 `notifications` 动作）会抢走 Activity 的
  窗口焦点，`EditText` 的 `isFocused` 还是 true，但按键不会再交给它；自动化脚本里必须在
  打字前 `cmd statusbar collapse` + `am start` 把窗口拉回前台。
- **没有输入连接就没有打字**：`ViewRootImpl` 只在 `mLastWasImTarget`（输入法已绑定）时把
  可打印字符交给输入法；光 `requestFocus()` 不弹输入法时，字母键会被静默丢掉。
  所以 `Focus-Field` 判的是 `dumpsys input_method` 里的 `mServedView=...EditText`，不是 `isFocused`。
- **a11y 找焦点别用 `rootInActiveWindow`**：软键盘弹出后它常常是输入法窗口，
  里面没有输入焦点。用 `AccessibilityService.findFocus(FOCUS_INPUT)`，退回按 `windows`
  遍历时先看本应用的窗口（后面别的 app 可能留着焦点节点）。
- **PS 5.1 + adb stderr**：`adb pull` 的进度写 stderr，`$ErrorActionPreference="Stop"` 下会
  被当成终止性错误（脚本在第一次 pull 就死）。`Adb` / `Shell` 包装函数里临时放宽再 `2>&1`。
- **adb 调用本身是不可靠的时钟**：每次 `adb shell` 在 Windows 上有 50~200ms 启动抖动，偶发卡到
  1~2 秒。用它给按键定间隔（和弦成员之间、序列两步之间）会把间隔拖到 `chord_timeout_ms`
  （默认 1000ms）之外，现象是「同一段脚本有时命中有时不命中」——先怀疑脚本太慢，别改判定。
  定长时序一律塞进**一次** `adb shell`（`Send-Sequence`）。
- **前台服务不能在后台起**：Android 12+ 从后台 `startForegroundService` 会被拒
  （`ForegroundServiceStartNotAllowedException`）。S0-5 的 `home` 动作会把自己丢到后台，
  所以下一段起服务前必须先 `am start` 把应用拉回前台。
- **主线程是共享资源**：`onKeyEvent` 与动作执行 / 报告序列化在**同一条主线程**上。连续几次
  跨进程 binder 调用（节点动作）会把输入过滤的 500ms 判定预算吃光，此时系统直接 fail-open
  放行（吞键失效）。要测「吞键」，别把它排在重活后面。
- **模拟器打字偶发多出一个字母**（`hllo` → `hllllo`）：`sendevent` 的按下 / 抬起之间若被系统
  放大成一次连发就会多一个字符。`Type-Into-Field` 因此带一次重试，不是判定问题。
- **脚本文件要带 BOM**：`verify.ps1` / `build-rust.ps1` 里有中文注释，PS 5.1 没 BOM 会按 GBK 解析报错。
- **别把 Gradle 的输出接进管道**：Gradle 留的常驻 daemon（空闲 3 小时才退）会继承父进程的
  stdout 句柄，于是 `powershell verify.ps1 | Tee-Object ...` 永远等不到 EOF——脚本跑完、汇总都
  打完了，外层却挂到超时才被杀（日志是完整的，只有管道不收尾）。`verify.ps1` 里 Gradle 改走
  「输出落临时文件 → 用 `FileShare.ReadWrite` 读回来回显」，daemon 拿的是文件句柄而不是管道。
  自己手动跑时同理：要留日志就 `> 文件`，别 `| Tee-Object`。
- **这台机器的模拟器没有外网**：`adb shell ping ihomy.top` → `unknown host`，`ping 8.8.8.8` 也丢包，
  宿主机却正常（`Test-NetConnection ihomy.top -Port 443` 通）。加 `-dns-server 223.5.5.5` 与重启 AVD
  都无效，同一台 AVD 还会在开机后一两分钟内整体消失（`device offline` → 进程没了）。所以
  `Update` 的 **https 分支只能在宿主机 / 真机验**，自动化里用 `file://` 清单；设备上做「检查更新」
  只能看到失败路径（`phase=failed` + `Unable to resolve host`，这条本身是对的）。
- **`am broadcast --es` 的值不能带空格**：`--es name "S1 编辑器测试"` 会被 `adb shell` 拆成
  多个 token，后面所有 extra 跟着错位——现象是 `trigger` 变成空串、报「触发键不能为空」。
  要么用不带空格的名字，要么自己在远端加引号。
- **`view.top` 是相对父容器的**：把编辑器插在面板列里之后，`scrollTo(0, view.top)` 会滚到半路；
  必须 `getLocationInWindow` 拿两个绝对位置相减，换回内容坐标。
- **`wrap_content` 对话框里 `weight` 会塌成 0**：编辑对话框的高度要按屏高算一个确定值
  （`heightPixels * 0.62`），否则按钮行被挤没。软键盘弹出后对话框会整体上移，
  这时 `input tap` 打按屏幕绝对坐标会点到键盘上——先 `input keyevent 4` 收键盘再点按钮。
- **`--es` 的值里有 `;` 会被设备侧 shell 拆开**：`drawRun` 的 `path` 是 `x,y;x,y;…`，
  直接用 `Bcast` 拼参数只会送进第一段。远端必须带引号：
  `adb shell "am broadcast … --es path '0.5,0.05;0.95,0.95'"`（`verify.ps1` 里走 `Shell` 传整串）。
- **画符号的「循环移位」只能给闭合回路**：开放折线（V / Z / N / L / C / U / M）起笔换了就是另一条线，
  对它们做移位会让「乱涂」也命中模板。`DollarOne.Template.closed` 就是这道闸。
- **180° 旋转对称的形状（Z / N / S）必须并二次比较**：反着画与正着画距离几乎一样，
  取到哪个解全看浮点先后——分辨率不是「差 0.02」，而是「θ 差了 180°」，于是方向扇区落空、
  好好的 Z 被判成「这个方向没有预设符号」。`bestAlign` 因此在距离接近（相对 2%）时
  **取转角小的解**，让「正着画」稳定拿回 θ≈0。
- **程序调 `setSelection` 也会回调 `onItemSelected`**：面板重建 / 配置刷新时会把用户刚绑的项
  悄悄解绑（现象是 `bindings` 忽然变空、画符号没反应）。回调里先比较「选中的值 == 当时显示的
  值」，相同就 return——`Spinner` 没有「用户主动改」的独立信号。
- **自定义符号存原始笔画，不存归一化后的 32 点**：归一化口径（点数 / 缩放 / 重采样）以后还会调，
  存原始点意味着老符号能跟着新算法重新归一化；一条笔画几百点，落盘代价可以忽略。

## 已知限制 / 下一步

- `remaps` / 层 / 热串在无 root 档**不成立**，只做报告提示：想真的改键得等 root 档（规划 §3.1）或
  「输入法式」输入通道（规划 §4）。
- 语义动作已是 core 的 `Action::Semantic`（`{"type":"semantic","operation":"select_all"}`，14 个 op，
  见 `需求设计说明书.md` §3.7）：安卓探针的 `ActionCatalog` id 与 core 的 serde 名**逐字相同**，
  `ActionRunner` 直接查表、不留第二张映射表。桌面侧配置合法但执行明确报错 + 冲突 Warn。
  **真机逐条 op 的表现**（尤其厂商 ROM 对 `GLOBAL_ACTION_*` 的裁剪）见 `docs/人工验证清单.md` §89。
- `text` 动作已改成「`ACTION_SET_TEXT` 在光标处插入」优先（不动剪贴板）；退回剪贴板粘贴时
  **原剪贴板可能恢复不了**（Android 10+ 后台应用读不到剪贴板），结果里会写明「已被覆盖」。
  带格式文本（富文本 / 多段样式）只能退化成纯文本，输入框不接受 `SET_TEXT` 的 App（少见）走粘贴。
- **编辑器只做「够用」那一档**：动作类型只有五类（`open_url` / `text` / `clipboard` /
  `semantic` / `pause_ms`），别的类型保持原样不动；同一条目有多个触发键时只改第一个；
  没有目录 / 层 / `when` 的编辑（手机屏上做不了桌面的全量编辑器，但也不许「改个触发键把动作改没了」）。
- **画符号只做第一批**：单笔画、17 个**预设**符号 + **自己录的符号**（上限 24，不判方向）、
  绑定到**现有**快捷键；绑定与自定义笔画都存在本机 `SharedPreferences("kada-draw")`，
  **不进 `Config`、不同步、卡带里也没有**。还没做：把绑定与自定义符号搬进配置（P2b）、
  多笔画（如「圆 + 竖」）。圈选截图那种「画圈再点一下」的复合手势也不在这套里
  （那是桌面右键手势的家庭）。
- **自更新只覆盖侧载版**：清单地址是代码常量（`Update.MANIFEST_URL`，测试才用 adb 传 `file://` 覆盖），
  改发布域名要重编；Play / 国内商店渠道走商店自己的更新，不走这套。**安装必须用户在系统安装器里
  点「安装」**——应用不能替用户确认，这是系统限制，也不该绕过；「安装未知应用」得用户自己去设置里开。
  清单与 APK 都走 https，目前**没有签名校验**（只比 sha256），发布链路的 `latest.json` 与 APK 必须同批更新。
- `ControlReceiver` 是 `exported=true` 的 adb 控制入口，但配了 `android:permission="android.permission.DUMP"`：
  只有 shell / system 持有该权限，普通应用发不进来（`adb shell am broadcast` 照旧可用）。
- 模拟器无法验证的三项，2026-11-13 华为 MRO-W00（Android 12 / sdk 31）真机回过一次：
  **`GLOBAL_ACTION_*` 七项全通（没被裁）**、**两台物理键盘可同时接管且不丢键**（67 down / 67 up、`unmatchedUp=0`）；
  **仍缺**：命中即吞的真机验证（那次只打了字，5 条示例快捷键一次没按）、长按连发、锁屏 8 小时存活。
- `sendevent` 需要 root（模拟器 `adb root` 即可）；真机自动化要么 root，要么用外接键盘手测。
- **SAF 那两条（导入·替换 / 恢复出厂）只能人工点**：`ACTION_OPEN_DOCUMENT` 的 result 只回到发起它的
  Activity，adb 送不进去 → 没进 `verify.ps1`；要自动化得引 UiAutomator 测试工程（探针阶段不划算）。
  本轮是用 `uiautomator dump` + `input tap` 真点验的，证据在「验收结果」表里。
