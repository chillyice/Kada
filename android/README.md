# Kada Android 探针（S0 + S1）

Android 版前两个阶段的**探针工程**（docs/安卓版规划.md §6 / §7 的 S0 / S1）：

- **S0**：纯 Kotlin + 无障碍服务 + 前台服务，把「安卓上到底能不能收键 / 吞键 / 判定 / 常驻」用真机数据证伪或证实。
- **S1**：判定搬进 Rust（`crates/kada-android` 复用 `kada-core`，与桌面同一份规则），配置落盘复用 `kada_core::config_io`，导入 / 导出走 SAF。

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
    ActionRunner.kt          把 Rust 返回的「命中条目」变成动作：open_url / text / clipboard / pause / 禁用
    KeyNames.kt              安卓 keycode ↔ core `Key` 名（必须与 core 逐字一致）
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

## 界面里能做什么

### S0 段（硬编码演示规则）

- **状态**：无障碍是否连上（含 `filterKeyEvents` 标志）、前台服务存活、档位探测、前台包名、输入设备。
- **触发实验**：F8 单键 → 气泡；F7 → 复制；F9 短按 → 气泡 / 长按 → 最近任务；
  `Ctrl+Alt+K` → 通知栏；音量上+音量下 → 返回（勾选后）。另有「吞 A」「吞音量键」
  「和弦全吞式」三个对照开关。
- **吞键端到端验证**：界面里有一个输入框，用 `adb shell input text` **测不出来**
  （注入事件不进无障碍过滤，见下），要用 `sendevent` 打物理键。
- **语义动作**：§3.2 的 15 条动作逐条点，结果写进报告（标注 ROM 可用性）。
- **统计 / 延迟 / 事件流**：按下 / 抬起 / 连发 / 未配对抬起、吞 / 放行、动作成功失败、
  P50/P95、最近 14 条事件。
- **导出报告**：`/sdcard/Android/data/com.kada.spike/files/kada-s0-report.json`（`adb pull` 拿走）。

### S1 段（配置驱动，在 S0 段下面）

- **用 Rust 判定**开关：关掉退回 S0 的硬编码演示规则（对照实验用，状态会持久化）。
- **配置**：首启自动生成安卓示例配置到 `/data/user/0/com.kada.spike/files/config.json`
  （5 条：单键 / 组合 / 序列 / 和弦）。落盘走 `kada_core::config_io`（原子写 + `.bak` + 损坏留档）。
- **导入 / 导出**：SAF 选文件。导入要点「合并 / 替换」二选一——合并只增不删，
  替换前先 `archive_copy` 留 `config.json.pre-import-<时间戳>`。
- **报告**：条数、触发键分类（combo / sequence / chord）、**本机不支持项**
  （`remaps` / 层内条目 / `expansions` / 手势 / 桌角 / 边缘滚动）、逐条不支持的动作类型、
  软冲突、告警、`notes`（含吞键策略说明）。
- **执行日志**：最近命中的快捷键、动作结果、失败原因。

判定口径（S1-a）：

| 配置块 | 安卓无 root 档 | 说明 |
|---|---|---|
| `shortcuts`（combo / sequence / chord） | ✅ 成立 | 与桌面同一份 `Trigger::parse` + `match_shortcut` |
| `shortcuts` 里的 `open_url` / `text` / `clipboard` / `pause_ms` / `group` / `disabled` | ✅ 执行 | 支持表只在 `crates/kada-android/src/action_support.rs` |
| `remaps`（含 tap-hold / oneshot / sticky / 切层键） | ❌ 报告标记 | 要注入目标键，无 root 做不到；**绝不吞**，只留 `remap:unsupported:*` 痕迹 |
| `layers` / 层内快捷键 | ❌ 报告标记 | 层只能由改键激活 |
| `expansions`（打字触发） | ❌ 报告标记 | 软键盘输入不产生 `KeyEvent` |
| 其它动作（命令 / 脚本 / 按键注入 / 应用 / 变量 / 条件…） | ❌ 报告标记 | 整条跳过、不占执行记录 |

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

## 已验证（2026-11-13，emulator-5554 / ihomy34，Android 14）

`android/tools/verify.ps1` **32/32 通过**，连跑两轮结果一致（第二轮无 `[warn]`）。

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
| S0-5 语义动作 | ✅ `GLOBAL_ACTION_*` 全部返回 true；节点动作 `setText` / `selectAll` / `copy` 依次 true | 报告 `actions` |
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

> 可重跑：S1 段以 `resetConfig`（恢复出厂示例）起跑，结束时把基线配置替换回去；
> 连跑两轮都是 32/32（第二轮无 `[warn]`），不再受上一轮残留条目影响。

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

## 已知限制 / 下一步

- `remaps` / 层 / 热串在无 root 档**不成立**，只做报告提示：想真的改键得等 root 档（规划 §3.1）或
  「输入法式」输入通道（规划 §4）。
- 语义动作（§3.2）目前是探针里的独立动作表，**还不是 core 的 `Action` 变体**：
  要进配置 / 界面 / i18n 得单独一轮（规划 §11 已记）。
- S1 的 `text` 动作走「写剪贴板 + `ACTION_PASTE`」：粘贴依赖目标输入框、会覆盖用户剪贴板，
  规划 §4 的方案 ③ 仍待定。
- `ControlReceiver` 是 `exported=true` 的 adb 控制入口，但配了 `android:permission="android.permission.DUMP"`：
  只有 shell / system 持有该权限，普通应用发不进来（`adb shell am broadcast` 照旧可用）。
- 模拟器无法验证：真机厂商 ROM 对 `GLOBAL_ACTION_*` 的裁剪、物理键盘连发、锁屏 8 小时存活。
- `sendevent` 需要 root（模拟器 `adb root` 即可）；真机自动化要么 root，要么用外接键盘手测。
