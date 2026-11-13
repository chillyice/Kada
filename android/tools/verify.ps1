# Kada Android S0 自动化验收（adb）
#
#   powershell android\tools\verify.ps1                 # 构建 + 装机 + 全部检查
#   powershell android\tools\verify.ps1 -SkipBuild      # 只跑检查
#   powershell android\tools\verify.ps1 -SkipSlowChecks # 跳过 30s 心跳等待
#   powershell android\tools\verify.ps1 -KeyDevice /dev/input/event1
#
# 判据来自 docs/安卓版规划.md §7（S0-1~S0-5）；任何一条失败退出码非 0。
# 注意：按键用 sendevent 走真实输入设备——`adb shell input keyevent` 的注入事件
# 不进无障碍过滤，用它测「吞键」会得到假阴性。
param(
    [string]$Serial = "",
    [switch]$SkipBuild,
    [switch]$SkipSlowChecks,
    [string]$KeyDevice = ""
)

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$androidDir = Split-Path -Parent $PSScriptRoot
$apkPath = Join-Path $androidDir "app\build\outputs\apk\debug\app-debug.apk"
$package = "com.kada.spike"
$receiver = "com.kada.spike/.ControlReceiver"
$reportRemote = "/sdcard/Android/data/com.kada.spike/files/kada-s0-report.json"

# ---------------------------------------------------------------- adb
function Resolve-Adb {
    $candidates = @()
    if ($env:ANDROID_HOME) { $candidates += (Join-Path $env:ANDROID_HOME "platform-tools\adb.exe") }
    if ($env:ANDROID_SDK_ROOT) { $candidates += (Join-Path $env:ANDROID_SDK_ROOT "platform-tools\adb.exe") }
    $localProps = Join-Path $androidDir "local.properties"
    if (Test-Path $localProps) {
        $line = (Select-String -Path $localProps -Pattern '^sdk.dir=').Line
        if ($line) {
            $sdk = $line.Substring(8).Replace('\:', ':').Replace('\\', '\')
            $candidates += (Join-Path $sdk "platform-tools\adb.exe")
        }
    }
    foreach ($c in $candidates) { if ($c -and (Test-Path $c)) { return $c } }
    $cmd = Get-Command adb.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    throw "找不到 adb：设 ANDROID_HOME 或在 android/local.properties 里写 sdk.dir"
}

$adb = Resolve-Adb
$adbBase = @()
if ($Serial) { $adbBase += @("-s", $Serial) }

function Adb {
    param([string[]]$Arguments)
    # adb 会把进度写到 stderr（pull 一行），PS 5.1 在 ErrorActionPreference=Stop 下会把它
    # 当成终止性错误。这里临时放宽再合并输出——否则脚本在第一次 pull 就死。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $adb @adbBase @Arguments 2>&1 } finally { $ErrorActionPreference = $prev }
}

function Shell {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try { & $adb @adbBase shell $Command 2>&1 } finally { $ErrorActionPreference = $prev }
}

function Bcast {
    param([string]$Op, [string[]]$Extra = @())
    $args = @("shell", "am", "broadcast", "-a", "com.kada.spike.CONTROL", "-n", $receiver, "--es", "op", $Op) + $Extra
    Adb $args | Out-Null
    Start-Sleep -Milliseconds 350
}

# ---------------------------------------------------------------- 录制 / 判定
$script:results = @()

function Check {
    param([string]$Name, [bool]$Ok, [string]$Detail = "")
    $script:results += [pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail }
    $tag = if ($Ok) { "[PASS]" } else { "[FAIL]" }
    Write-Host ("{0} {1} {2}" -f $tag, $Name, $Detail)
}

function SendKey {
    param([int]$Code, [int]$HoldMs = 30)
    $sec = [math]::Round($HoldMs / 1000.0, 3)
    Shell "sendevent $script:keyDev 1 $Code 1; sendevent $script:keyDev 0 0 0; sleep $sec; sendevent $script:keyDev 1 $Code 0; sendevent $script:keyDev 0 0 0" | Out-Null
}

function KeyDown([int]$Code) {
    Shell "sendevent $script:keyDev 1 $Code 1; sendevent $script:keyDev 0 0 0" | Out-Null
}

function KeyUp([int]$Code) {
    Shell "sendevent $script:keyDev 1 $Code 0; sendevent $script:keyDev 0 0 0" | Out-Null
}

# 一次 adb shell 里把整段按键按真实时序打完。
#
# 分开发（每次一个 `adb shell`）的问题：Windows 上起一次 adb 有 50~200ms 抖动，偶尔卡到
# 1~2 秒；和弦 / 序列的「成员间隔」于是会漂到超时窗之外（`chord_timeout_ms` 默认 1000ms），
# 表现成偶发不命中。判定逻辑没问题，是脚本自己太慢——所以时序必须一次发完。
#
# 参数是 `"键码:1"`（按下）/ `"键码:0"`（抬起）/ `"sleep:毫秒"`。
function Send-Sequence {
    param([string[]]$Steps)
    $parts = @()
    foreach ($s in $Steps) {
        if ($s -like "sleep:*") {
            $ms = [int]($s.Substring(6))
            $parts += ("sleep {0}" -f [math]::Round($ms / 1000.0, 3))
        } else {
            $code, $value = $s.Split(':')
            $parts += "sendevent $script:keyDev 1 $code $value; sendevent $script:keyDev 0 0 0"
        }
    }
    Shell ($parts -join "; ") | Out-Null
}

# 按键类断言的统一流程：拉回前台 → 清事件 → 发键 → 等 → 取报告 → 断言，失败重试一次。
#
# 为什么要重试：模拟器上的 `sendevent` 偶发丢一次（输入队列 / adb 进程抖动），而真回归
# 会两次都失败——重试不会掩盖问题，但成功时会打一行 `[warn]` 留痕。
function Test-KeyFlow {
    param([scriptblock]$Send, [scriptblock]$Assert, [string]$Label, [int]$SettleMs = 800)
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        Bcast "clear"
        & $Send
        Start-Sleep -Milliseconds $SettleMs
        $script:lastReport = Get-Report
        if (& $Assert) {
            if ($try -gt 1) { Write-Host "  [warn] $Label 第 $try 次才命中（前一次被丢了）" }
            return $true
        }
        Write-Host ("  [warn] {0} 第 {1} 次没命中：{2}" -f $Label, $try, (Event-Digest $script:lastReport))
    }
    return $false
}

function Get-Report {
    Bcast "export"
    Start-Sleep -Milliseconds 900
    $dest = Join-Path $env:TEMP "kada-s0-report.json"
    if (Test-Path $dest) { Remove-Item $dest -Force }
    Adb @("pull", $reportRemote, $dest) | Out-Null
    if (-not (Test-Path $dest)) { throw "报告没拉下来：$reportRemote" }
    Get-Content $dest -Raw -Encoding UTF8 | ConvertFrom-Json
}

# 当前是否有窗口焦点（`dumpsys window` 的 mCurrentFocus 是不是本应用）。
# 通知栏 / 其它 app 抢到前台时，Activity 会丢窗口焦点：此时 EditText 的 view 焦点还在，
# 但按键不会再交给它——「打字全是空」就是这么来的。
function Test-WindowFocused {
    $lines = @(Shell "dumpsys window" | Select-String -Pattern "mCurrentFocus=")
    if ($lines.Count -eq 0) { return $false }
    $last = "$($lines[-1])"
    return ($last -match [regex]::Escape(" $package/"))
}

# 输入框聚焦走 app 自己的入口（`op focusField`）：
# 本界面每 500ms 刷新文本，窗口不空闲，uiautomator 的 dump / tap 会间歇性失败，
# 导致「点了没焦点、打字全丢」的假阴性。
#
# 光广播一次不够，三个前提都要等：
# 1. 本应用在前台（先收通知栏，再 am start 把任务拉到前面）；
# 2. Activity 窗口有焦点（`hasWindowFocus`）；
# 3. 输入法真的把焦点交给了 EditText（`mServedView=android.widget.EditText`）——
#    没有输入连接时 `ViewRootImpl` 不会把按键送给输入法（`mLastWasImTarget=false`），
#    可打印字符会被直接丢掉。
function Test-FieldServed {
    $out = (Shell "dumpsys input_method") -join "`n"
    return ($out -match "mServedView=android\.widget\.EditText")
}

function Focus-Field {
    for ($i = 1; $i -le 4; $i++) {
        Shell "cmd statusbar collapse" | Out-Null
        if (-not (Test-WindowFocused)) {
            Shell "am start -n $package/.MainActivity" | Out-Null
            Start-Sleep -Milliseconds 800
        }
        Bcast "focusField"
        Start-Sleep -Milliseconds 600
        if (Test-FieldServed) { return $true }
    }
    return $false
}

function Clear-Field {
    Shell "input keyevent KEYCODE_MOVE_END" | Out-Null
    1..20 | ForEach-Object { Shell "input keyevent KEYCODE_DEL" | Out-Null }
    Start-Sleep -Milliseconds 300
}

# 把本应用拉回前台，并收掉系统面板（通知栏 / 快捷设置 / 音量面板）。
#
# 为什么每段按键前都要做：
# - `open_url` 会把 Chrome 拉到前台；通知栏 / 快捷设置也是「有焦点的窗口」；
# - 前台一变，无障碍服务要重新绑输入连接（几十到几百毫秒），这段窗口里的按键会被丢掉，
#   现象是「同一段脚本有时命中有时不命中」——不是判定逻辑的问题，别去改判定。
function Restore-App {
    Shell "cmd statusbar collapse" | Out-Null
    Shell "am start -n $package/.MainActivity" | Out-Null
    Start-Sleep -Milliseconds 900
}

# 失败诊断：区分「键根本没进服务」（焦点 / 输入连接问题）与「进了但没命中」（判定问题）。
function Event-Digest {
    param($Report, [string[]]$Keys = @("F8", "F6", "F7", "F", "J", "K", "F9", "Space", "Ctrl", "Alt"))
    $parts = @()
    foreach ($k in $Keys) {
        $n = @($Report.events | Where-Object { $_.key -eq $k }).Count
        if ($n -gt 0) { $parts += "$k=$n" }
    }
    return ("keys[{0}] a11y={1} useRust={2}" -f ($parts -join ","), [bool]$Report.accessibility.connected, $Report.rust.useRustEngine)
}

$script:keyCodes = @{
    'a' = 30; 'b' = 48; 'c' = 46; 'd' = 32; 'e' = 18; 'f' = 33; 'g' = 34; 'h' = 35; 'i' = 23
    'j' = 36; 'k' = 37; 'l' = 38; 'm' = 50; 'n' = 49; 'o' = 24; 'p' = 25; 'q' = 16; 'r' = 19
    's' = 31; 't' = 20; 'u' = 22; 'v' = 47; 'w' = 17; 'x' = 45; 'y' = 21; 'z' = 44
}

function Type-Text([string]$Text) {
    foreach ($ch in $Text.ToLowerInvariant().ToCharArray()) {
        $code = $script:keyCodes["$ch"]
        if ($null -ne $code) { SendKey $code 20 }
    }
    Start-Sleep -Milliseconds 500
}

# 聚焦 → 清空 → 打字 → 读报告；一次不中重试一次（焦点 / 输入法动画的时序抖动）。
# `-Expect` 是期望的最终文本（吞键用例要期望「少了某个字母」），不传就是原文。
# 返回最后一次的报告（调用方自己判），失败时把焦点状态打出来，别让「没聚焦」冒充「吞键生效」。
function Type-Into-Field {
    param([string]$Text, [string]$Expect = "")
    if (-not $Expect) { $Expect = $Text }
    $report = $null
    for ($try = 1; $try -le 2; $try++) {
        $focused = Focus-Field
        if (-not $focused) { Write-Host "  [warn] focusField 没拿到焦点（第 $try 次）" }
        Clear-Field
        Type-Text $Text
        Start-Sleep -Milliseconds 400
        $report = Get-Report
        if ($report.stats.inputFieldText -eq $Expect) { return $report }
        Write-Host ("  [warn] 第 {0} 次没打进：field=[{1}] 期望=[{2}] focused={3} window={4} {5}" -f `
            $try, $report.stats.inputFieldText, $Expect, $report.stats.inputFieldFocused, `
            $report.stats.windowFocused, $report.stats.lastFocusAttempt)
    }
    return $report
}

function Get-Volume {
    $out = (Shell "cmd media_session volume --stream 3 --get") -join "`n"
    $m = [regex]::Match($out, 'volume is (\d+)')
    if ($m.Success) { return [int]$m.Groups[1].Value }
    return -1
}

function Resolve-KeyDevice {
    if ($KeyDevice) { return $KeyDevice }
    $dump = (Shell "getevent -pl") -join "`n"
    $found = [regex]::Matches($dump, 'add device \d+: (?<path>/dev/input/event\d+)\r?\n\s*name:\s*"(?<name>[^"]*)"')
    $keyboards = @()
    foreach ($m in $found) {
        if ($m.Groups["name"].Value -match '(?i)keyboard|kbd') { $keyboards += $m }
    }
    if ($keyboards.Count -eq 0) {
        throw "找不到键盘输入设备；用 -KeyDevice /dev/input/eventX 指定（先跑 adb shell getevent -pl 看一眼）"
    }
    return $keyboards[0].Groups["path"].Value
}

# ---------------------------------------------------------------- 开跑
Write-Host "== Kada Android S0 验收 =="
Write-Host "adb: $adb"

# sendevent 走真实输入设备需要 root（模拟器 `adb root` 即可；重启后 root 会丢）。
# 真机没 root 时这一步失败属正常，按键类检查只能手测——脚本继续跑。
Adb @("root") | Out-Null
Start-Sleep -Milliseconds 1500
Adb @("wait-for-device") | Out-Null

if (-not $SkipBuild) {
    # 先把 Rust 动态库编出来（判定逻辑在 crates/kada-android，Gradle 不编它）。
    # 子进程的 stderr 要收下来：cargo 的进度写 stderr，本脚本是 Stop 语义，直接调会被当成错误。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $buildLog = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "build-rust.ps1") 2>&1
        $buildCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
    $buildLog | ForEach-Object { Write-Host $_ }
    if ($buildCode -ne 0) { throw "Rust 库构建失败（$buildCode）" }
    # Gradle 的输出落到临时文件再回显，**不要**直接接到本进程的 stdout。
    #
    # 原因：Gradle 会留一个常驻 daemon（空闲 3 小时才退），它继承父进程的 stdout 句柄。
    # 只要本脚本被管道接走（`powershell verify.ps1 | Tee-Object ...`），那个管道就永远等不到
    # EOF——脚本跑完、汇总都打完了，外层却挂到超时才被杀。句柄落到文件就没这个循环等待。
    $gradleLog = Join-Path $env:TEMP "kada-build-gradle.log"
    Push-Location $androidDir
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cmd /c ".\gradlew.bat --offline assembleDebug > `"$gradleLog`" 2>&1"
        $gradleCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
        Pop-Location
    }
    # daemon 可能还持有这个文件的写句柄：用 FileShare.ReadWrite 打开，别让回显本身失败。
    if (Test-Path $gradleLog) {
        try {
            $fs = [System.IO.File]::Open($gradleLog, 'Open', 'Read', 'ReadWrite')
            $sr = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8)
            $sr.ReadToEnd().Split("`n") | Where-Object { $_.Trim() } | ForEach-Object { Write-Host $_ }
            $sr.Dispose()
            $fs.Dispose()
        } catch {
            Write-Host "  [warn] 构建日志读不出来（daemon 占着），见 $gradleLog"
        }
    }
    if ($gradleCode -ne 0) { throw "构建失败（$gradleCode，日志 $gradleLog）" }
}
if (-not (Test-Path $apkPath)) { throw "APK 不存在：$apkPath" }

Adb @("install", "-r", $apkPath) | Out-Null
Shell "pm grant $package android.permission.POST_NOTIFICATIONS" | Out-Null
Shell "settings put secure enabled_accessibility_services $package/$package.KeyFilterService" | Out-Null
Shell "settings put secure accessibility_enabled 1" | Out-Null
# 屏幕可能已休眠 / 通知栏被上一轮「全局动作」拉开：都关掉，否则音量键会落给通知栏。
Shell "input keyevent KEYCODE_WAKEUP" | Out-Null
Shell "cmd statusbar collapse" | Out-Null
Shell "am start -n $package/.MainActivity" | Out-Null
Start-Sleep -Seconds 3

$script:keyDev = Resolve-KeyDevice
Write-Host "key device: $script:keyDev"

Bcast "swallow" @("--ez", "on", "true")
Bcast "demoRules" @("--ez", "on", "true")
Bcast "swallowA" @("--ez", "on", "false")
Bcast "swallowVolume" @("--ez", "on", "false")
Bcast "chordSwallowAll" @("--ez", "on", "false")
Bcast "volumeChord" @("--ez", "on", "false")
Bcast "tapHoldMs" @("--ei", "ms", "500")
Bcast "clear"

# S0-1 无障碍已连接
$report = Get-Report
Check "S0-1 无障碍已连接" ([bool]$report.accessibility.connected) "filterKeyEvents=$($report.accessibility.filterKeyEventsFlagged)"

# S0 段是「硬编码演示规则」的证伪：先把判定切回 S0 的 KeyRouter，
# 否则请求会先被 S1 的 Rust 判定接走（S1 段末尾会切回 true）。
Bcast "useRust" @("--ez", "on", "false")

# S0-1 / S0-3 单键吞键 + 动作
Bcast "clear"
SendKey 66
Start-Sleep -Milliseconds 700
$report = Get-Report
Check "S0-1 F8 单键 → 动作" (($report.stats.down -ge 1) -and ($report.actions.toast.okCount -ge 1)) "down=$($report.stats.down) toastOk=$($report.actions.toast.okCount)"
Check "S0-1 F8 down/up 都吞" (($report.stats.up -ge 1) -and ($report.stats.unmatchedUp -eq 0)) "up=$($report.stats.up) unmatchedUp=$($report.stats.unmatchedUp)"

# S0-2 tap-hold：短按 / 长按
Bcast "clear"
SendKey 67 30
Start-Sleep -Milliseconds 700
$report = Get-Report
$tapHit = @($report.events | Where-Object { $_.note -eq "taphold:tap" }).Count -ge 1
Check "S0-2 F9 短按 → tap" $tapHit "note=taphold:tap"

Bcast "clear"
KeyDown 67
Start-Sleep -Milliseconds 900
KeyUp 67
Start-Sleep -Milliseconds 700
$report = Get-Report
Check "S0-2 F9 长按 → hold" ($report.actions.recents.okCount -ge 1) "recentsOk=$($report.actions.recents.okCount)"

# S0-2 和弦（非破坏式：修饰键放行、只吞凑齐那一下）
Bcast "clear"
KeyDown 29
KeyDown 56
SendKey 37 30
KeyUp 29
KeyUp 56
Start-Sleep -Milliseconds 700
$report = Get-Report
Check "S0-2 Ctrl+Alt+K 和弦" ($report.actions.notifications.okCount -ge 1) "notificationsOk=$($report.actions.notifications.okCount)"
$modPassed = @($report.events | Where-Object { $_.key -eq "Ctrl" -and $_.kind -eq "down" -and -not $_.swallowed }).Count -ge 1
Check "S0-2 非破坏式：修饰键放行" $modPassed ""
# 通知栏被上面这条动作拉下来了：它是有焦点的窗口，会把主窗口的窗口焦点抢走，
# 后面「往输入框打字」就再也送不到 EditText。收回去是测试卫生，不是产品行为。
Shell "cmd statusbar collapse" | Out-Null
Start-Sleep -Milliseconds 600

# S0-2 全吞式的代价：单按 Ctrl 被白吞
Bcast "chordSwallowAll" @("--ez", "on", "true")
Bcast "clear"
SendKey 29 40
Start-Sleep -Milliseconds 600
$report = Get-Report
$missCost = @($report.events | Where-Object { $_.note -like "chord-miss*" }).Count -ge 1
Check "S0-2 全吞式：成员键被吞且记代价" $missCost "note=chord-miss:*"
Bcast "chordSwallowAll" @("--ez", "on", "false")

# S0-1 端到端吞键（输入框里打字）
$report = Type-Into-Field "hello"
Check "S0-1 吞键关闭：hello 落进输入框" ($report.stats.inputFieldText -eq "hello") "field=[$($report.stats.inputFieldText)] focused=$($report.stats.inputFieldFocused) window=$($report.stats.windowFocused)"

Bcast "swallowA" @("--ez", "on", "true")
$report = Type-Into-Field -Text "hallo" -Expect "hllo"
Check "S0-1 吞 A：a 不落进输入框" ($report.stats.inputFieldText -eq "hllo") "field=[$($report.stats.inputFieldText)] focused=$($report.stats.inputFieldFocused) window=$($report.stats.windowFocused)"
Bcast "swallowA" @("--ez", "on", "false")

# S0-1 音量键可吞（手机档的物理触发源）
#
# 放在节点动作之前：节点动作是跨进程 binder 调用，跑在主线程上，连做几次会吃掉输入过滤的
# 500ms 判定预算（超时只能 fail-open 放行），音量键检查就会偶发假阴性。
Restore-App
$volBefore = Get-Volume
if ($volBefore -ge 0) {
    Bcast "swallowVolume" @("--ez", "on", "true")
    Shell "cmd media_session volume --stream 3 --set $volBefore" | Out-Null
    Start-Sleep -Milliseconds 500
    # 单次按键在这台模拟器上偶发被音量面板动画吃掉，连按 3 次再判（吞掉时三次都不该生效）。
    1..3 | ForEach-Object { SendKey 115 40; Start-Sleep -Milliseconds 150 }
    Start-Sleep -Milliseconds 700
    $volSwallowed = Get-Volume
    Check "S0-1 吞音量键：系统音量不变" ($volSwallowed -eq $volBefore) "$volBefore -> $volSwallowed（连按 3 次）"

    Bcast "swallowVolume" @("--ez", "on", "false")
    1..3 | ForEach-Object { SendKey 115 40; Start-Sleep -Milliseconds 150 }
    Start-Sleep -Milliseconds 700
    $volPassed = Get-Volume
    Check "S0-1 放行音量键：系统音量上升" ($volPassed -gt $volBefore) "$volBefore -> $volPassed（连按 3 次）"
    Shell "cmd media_session volume --stream 3 --set $volBefore" | Out-Null
} else {
    Check "S0-1 音量键吞键" $false "cmd media_session 不可用（ROM 差异）"
}

# S0-5 节点动作（先把光标放进输入框）
# 顺序按依赖走：先写入文本 → 再全选 → 最后复制（复制需要有效选区）。
Bcast "clear"
$focusOk = Focus-Field
foreach ($a in @("setText", "selectAll", "copy")) {
    Bcast "fire" @("--es", "action", $a)
    Start-Sleep -Milliseconds 500
}
$report = Get-Report
$nodeOk = (($report.actions.selectAll.lastOk) -and ($report.actions.copy.lastOk) -and ($report.actions.setText.lastOk))
Check "S0-5 节点动作（写入 / 全选 / 复制）" $nodeOk "setText=$($report.actions.setText.lastOk) selectAll=$($report.actions.selectAll.lastOk) copy=$($report.actions.copy.lastOk) focused=$focusOk"
if (-not $nodeOk) { Bcast "focusInfo" }

# S0-5 全局动作
Bcast "clear"
foreach ($a in @("home", "back", "recents", "notifications", "quickSettings", "toast")) {
    Bcast "fire" @("--es", "action", $a)
    Start-Sleep -Milliseconds 400
}
$report = Get-Report
$globalOk = $true
foreach ($a in @("home", "back", "recents", "notifications", "quickSettings", "toast")) {
    if (-not $report.actions.$a.lastOk) { $globalOk = $false }
}
Check "S0-5 GLOBAL_ACTION_* / toast" $globalOk "lockScreen / screenshot 单独手测"
# 上面拉开了通知栏 / 快捷设置，收回来（否则后续音量键落给通知栏）
Shell "cmd statusbar collapse" | Out-Null

# S0-4 前台服务 + 心跳
if (-not $SkipSlowChecks) {
    # 上一段的全局动作把应用丢到后台（home）并拉开了系统面板。Android 12+ 不许后台启动
    # 前台服务（ForegroundServiceStartNotAllowedException），先把它拉回前台再起服务。
    Restore-App
    Bcast "startService"
    Start-Sleep -Seconds 35
    $report = Get-Report
    Check "S0-4 前台服务常驻（30s 心跳）" (([bool]$report.foregroundService.running) -and ($report.foregroundService.ticks -ge 1)) "ticks=$($report.foregroundService.ticks)"
    Bcast "stopService"
} else {
    Write-Host "[SKIP] S0-4 前台服务 30s 心跳（-SkipSlowChecks）"
}

# S0-3 延迟（样本来自前面的单键触发）
$report = Get-Report
Check "S0-3 延迟样本" ($report.latency.count -ge 1) "n=$($report.latency.count) p50=$($report.latency.p50ms)ms p95=$($report.latency.p95ms)ms"

# ---------------------------------------------------------------- S1（Rust 判定）
Write-Host ""
Write-Host "== S1 · 配置驱动判定（Rust / kada-core）=="
Bcast "useRust" @("--ez", "on", "true")
# 从「出厂示例配置」起跑：上一轮的合并导入会把 F6 留在设备上，条数就不确定了，
# 「合并导入后 +1 条」这类断言会随上一轮残留漂移。删配置 + 重新 init = 恢复出厂。
Bcast "resetConfig"
Restore-App
Bcast "rust"
Start-Sleep -Milliseconds 500
$report = Get-Report
$rust = $report.rust
Check "S1-1 Rust 库已加载（libkada_android.so）" ([bool]$rust.loaded) "version=$($rust.version)"
Check "S1-1 判定默认走 Rust" ([bool]$rust.useRustEngine) ""
Check "S1-1 首启自动生成安卓示例配置" ($rust.report.counts.shortcuts -eq 5) "shortcuts=$($rust.report.counts.shortcuts) path=$($rust.report.path)"
Check "S1-1 触发键分类（组合 / 序列 / 和弦）" (($rust.report.triggers.combo -ge 1) -and ($rust.report.triggers.sequence -ge 1) -and ($rust.report.triggers.chord -ge 1)) "combo=$($rust.report.triggers.combo) seq=$($rust.report.triggers.sequence) chord=$($rust.report.triggers.chord)"

function Rust-Fired([string]$Trigger) {
    return @($script:lastReport.rust.fires | Where-Object { $_.trigger -eq $Trigger }).Count -ge 1
}

# 单键 F8：命中动作 + down/up 都吞（桌面同款「吞掉的键连 keyup 一起吞」）
$f8Ok = Test-KeyFlow -Label "F8" -Send { SendKey 66 40 } -Assert { Rust-Fired "F8" }
Check "S1-1 单键 F8 → 动作成功" $f8Ok "fires=$(@($script:lastReport.rust.fires).Count) $(Event-Digest $script:lastReport)"
$swallowedBoth = @($script:lastReport.events | Where-Object { $_.note -like "rust:*" -and $_.swallowed }).Count -ge 2
Check "S1-1 命中的键 down/up 都吞" $swallowedBoth "note=rust:shortcut:F8 $(Event-Digest $script:lastReport)"

# 和弦 F&J：非破坏式——成员放行，只吞凑齐那一下
$chordOk = Test-KeyFlow -Label "F&J" -Send {
    Send-Sequence @("33:1", "sleep:60", "36:1", "sleep:80", "36:0", "sleep:20", "33:0")
} -Assert { Rust-Fired "F&J" }
Check "S1-1 和弦 F&J → 动作" $chordOk "$(Event-Digest $script:lastReport)"
$fPassed = @($script:lastReport.events | Where-Object { $_.key -eq "F" -and $_.kind -eq "down" -and -not $_.swallowed }).Count -ge 1
Check "S1-1 和弦非破坏：成员 F 放行" $fPassed "只有凑齐的 J 被吞 $(Event-Digest $script:lastReport)"

# 序列 F9 Space：前导键放行 + 等待窗内凑齐（等待窗由 60ms ticker 推进）
$seqOk = Test-KeyFlow -Label "F9 Space" -Send {
    Send-Sequence @("67:1", "sleep:30", "67:0", "sleep:150", "57:1", "sleep:40", "57:0")
} -Assert { Rust-Fired "F9 Space" }
Check "S1-1 序列 F9 Space → 动作" $seqOk "$(Event-Digest $script:lastReport)"

# 组合 Ctrl+Alt+K（修饰键放行，K 带 metaState 命中）
$comboOk = Test-KeyFlow -Label "Ctrl+Alt+K" -Send {
    Send-Sequence @("29:1", "sleep:40", "56:1", "sleep:40", "37:1", "sleep:40", "37:0", "sleep:30", "56:0", "sleep:20", "29:0")
} -Assert { Rust-Fired "Ctrl+Alt+K" }
Check "S1-1 组合 Ctrl+Alt+K → 动作" $comboOk "$(Event-Digest $script:lastReport)"

# S1-2 导出：SAF 写文件之前的那一段（Rust 侧序列化 + 原子写后的磁盘内容）
$extDir = "/sdcard/Android/data/com.kada.spike/files"
$exportLocal = Join-Path $env:TEMP "kada-export-config.json"
Bcast "exportConfig"
Start-Sleep -Milliseconds 700
if (Test-Path $exportLocal) { Remove-Item $exportLocal -Force }
Adb @("pull", "$extDir/exported-config.json", $exportLocal) | Out-Null
$exported = if (Test-Path $exportLocal) { Get-Content $exportLocal -Raw -Encoding UTF8 | ConvertFrom-Json } else { $null }
$baselineCount = if ($exported) { @($exported.shortcuts).Count } else { 0 }
Check "S1-2 导出配置" ($baselineCount -ge 5) "shortcuts=$baselineCount"

# 合并导入：在原配置上加一条 F6（只增不删）
if ($exported) {
    $merged = Get-Content $exportLocal -Raw -Encoding UTF8 | ConvertFrom-Json
    $merged.shortcuts += [pscustomobject]@{
        name     = "S1 导入测试"
        triggers = @("F6")
        actions  = @([pscustomobject]@{ type = "open_url"; url = "https://example.com/import" })
        enabled  = $true
    }
    $mergedPath = Join-Path $env:TEMP "kada-import-merged.json"
    # PS 5.1 的 Set-Content -Encoding UTF8 会写 BOM，Rust 侧 serde_json 解析会失败
    [System.IO.File]::WriteAllText($mergedPath, ($merged | ConvertTo-Json -Depth 20), (New-Object System.Text.UTF8Encoding($false)))
    Adb @("push", $mergedPath, "$extDir/incoming.json") | Out-Null
    Bcast "import" @("--es", "file", "incoming.json", "--ez", "merge", "true")
    Start-Sleep -Milliseconds 700
    $script:lastReport = Get-Report
    Check "S1-2 合并导入（只增不删）" ($script:lastReport.rust.report.counts.shortcuts -eq ($baselineCount + 1)) "shortcuts=$($script:lastReport.rust.report.counts.shortcuts)"

    $f6Ok = Test-KeyFlow -Label "F6" -Send { SendKey 64 40 } -Assert { Rust-Fired "F6" }
    Check "S1-2 导入的 F6 立即生效（无需重启）" $f6Ok "$(Event-Digest $script:lastReport)"

    # 替换导入：换一份带 remaps 的配置 → 整份换掉 + 报告标「本机不支持改键」+ 覆盖前留档
    $replaced = Get-Content $exportLocal -Raw -Encoding UTF8 | ConvertFrom-Json
    $replaced.remaps = @([pscustomobject]@{ from = "A"; to = "B"; enabled = $true })
    $replacedPath = Join-Path $env:TEMP "kada-import-replace.json"
    [System.IO.File]::WriteAllText($replacedPath, ($replaced | ConvertTo-Json -Depth 20), (New-Object System.Text.UTF8Encoding($false)))
    Adb @("push", $replacedPath, "$extDir/incoming.json") | Out-Null
    Bcast "import" @("--es", "file", "incoming.json", "--ez", "merge", "false")
    Start-Sleep -Milliseconds 700
    $script:lastReport = Get-Report
    $notes = (@($script:lastReport.rust.report.notes) -join " / ")
    Check "S1-2 替换导入（整份换掉）" ($script:lastReport.rust.report.counts.shortcuts -eq $baselineCount) "shortcuts=$($script:lastReport.rust.report.counts.shortcuts)（F6 已随旧配置消失）"
    Check "S1-2 remaps 标为「本机不支持」（软冲突，不拦保存）" (($script:lastReport.rust.report.unsupported.remaps -eq 1) -and ($notes -like "*改键*")) "unsupported.remaps=$($script:lastReport.rust.report.unsupported.remaps)"
    $files = (Shell "run-as $package ls files") -join "`n"
    Check "S1-2 替换导入前留档" ($files -match "pre-import-") "files=$(($files -split "`n") -join ",")"

    # 收尾：把基线配置换回来，别把测试条目留在设备上
    Adb @("push", $exportLocal, "$extDir/restore.json") | Out-Null
    Bcast "import" @("--es", "file", "restore.json", "--ez", "merge", "false")
} else {
    Check "S1-2 合并导入" $false "导出文件没拿到"
}

Write-Host ""
Write-Host "报告：$(Join-Path $env:TEMP 'kada-s0-report.json')"
Write-Host "设备侧：$reportRemote"

$failed = @($script:results | Where-Object { -not $_.Ok })
Write-Host ""
Write-Host ("== 汇总：{0}/{1} 通过 ==" -f ($script:results.Count - $failed.Count), $script:results.Count)
if ($failed.Count -gt 0) {
    $failed | ForEach-Object { Write-Host ("[FAIL] {0} {1}" -f $_.Name, $_.Detail) }
    exit 1
}
exit 0
