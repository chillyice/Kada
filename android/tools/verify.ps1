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

# 设备上的配置原文。走 `exec-out` 而不是 `adb shell`：后者经 pty 会把换行改成 CRLF，
# 中文也会多绕一层；`exec-out` 是原始字节，按脚本开头设的 UTF-8 解出来就能直接解析。
# `run-as` 的当前目录是应用数据目录，所以路径相对 `files/config.json`。
function Get-ConfigText {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        (& $adb @adbBase exec-out run-as $package cat files/config.json 2>$null) -join "`n"
    } finally {
        $ErrorActionPreference = $prev
    }
}

# 解析不出就返回 $null（被删 / 半截写入时，断言该失败而不是脚本崩）。
# 注意 PS 5.1 的 ConvertFrom-Json 把单元素数组解成对象，调用方一律用 @() 收。
function Get-ConfigJson {
    $text = Get-ConfigText
    if ([string]::IsNullOrWhiteSpace($text)) { return $null }
    try { return ($text | ConvertFrom-Json) } catch { return $null }
}

# JSON 对象的某个键（键名带 `u:` 这类字符时不能用 `.foo` 取，只能翻 PSObject.Properties）。
function Get-JsonField {
    param($Obj, [string]$Name)
    if (-not $Obj) { return $null }
    foreach ($p in @($Obj.PSObject.Properties)) {
        if ($p.Name -eq $Name) { return $p.Value }
    }
    return $null
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
foreach ($a in @("set_text", "select_all", "copy")) {
    Bcast "fire" @("--es", "action", $a)
    Start-Sleep -Milliseconds 500
}
$report = Get-Report
$nodeOk = (($report.actions.select_all.lastOk) -and ($report.actions.copy.lastOk) -and ($report.actions.set_text.lastOk))
Check "S0-5 节点动作（写入 / 全选 / 复制）" $nodeOk "set_text=$($report.actions.set_text.lastOk) select_all=$($report.actions.select_all.lastOk) copy=$($report.actions.copy.lastOk) focused=$focusOk"
if (-not $nodeOk) { Bcast "focusInfo" }

# S0-5 全局动作
Bcast "clear"
foreach ($a in @("home", "back", "recents", "notifications", "quick_settings", "toast")) {
    Bcast "fire" @("--es", "action", $a)
    Start-Sleep -Milliseconds 400
}
$report = Get-Report
$globalOk = $true
foreach ($a in @("home", "back", "recents", "notifications", "quick_settings", "toast")) {
    if (-not $report.actions.$a.lastOk) { $globalOk = $false }
}
Check "S0-5 GLOBAL_ACTION_* / toast" $globalOk "lock_screen / screenshot 单独手测"
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

    # S1-3 语义动作（core 的 `Action::Semantic` 变体）：合并导入一条 F5 → semantic(notifications)。
    # 选 notifications 不选 home / back：后两者会把应用踢到后台，毁掉后面所有按键投递；
    # 通知栏收尾 `cmd statusbar collapse` 一按就回去。
    $semPath = Join-Path $env:TEMP "kada-import-semantic.json"
    $sem = [pscustomobject]@{
        folders   = @()
        layers    = @()
        shortcuts = @([pscustomobject]@{
                name     = "S1 语义动作"
                triggers = @("F5")
                actions  = @([pscustomobject]@{ type = "semantic"; operation = "notifications" })
                enabled  = $true
            })
    }
    [System.IO.File]::WriteAllText($semPath, ($sem | ConvertTo-Json -Depth 20), (New-Object System.Text.UTF8Encoding($false)))
    Adb @("push", $semPath, "$extDir/incoming.json") | Out-Null
    Bcast "import" @("--es", "file", "incoming.json", "--ez", "merge", "true")
    Start-Sleep -Milliseconds 700
    $script:lastReport = Get-Report
    $semUnsupported = @($script:lastReport.rust.report.unsupported_actions | Where-Object { @($_.kinds) -contains "semantic" }).Count
    Check "S1-3 语义动作算「本机支持」（不进不支持清单）" ($semUnsupported -eq 0) "unsupported_actions=$(@($script:lastReport.rust.report.unsupported_actions).Count)"

    $semOk = Test-KeyFlow -Label "F5 → semantic" -Send { SendKey 63 40 } -Assert { Rust-Fired "F5" }
    Check "S1-3 F5 → 语义动作（notifications）" $semOk "$(Event-Digest $script:lastReport)"
    $semRec = $script:lastReport.actions.'rust:F5'
    Check "S1-3 语义动作执行成功" ($semRec -and $semRec.lastOk) "ok=$($semRec.lastOk) detail=$($semRec.lastDetail)"
    Shell "cmd statusbar collapse" | Out-Null

    # S1-4 编辑器（ShortcutStore）：新建 / 校验 / 生效 / 删除。脚本走的是界面同一份数据层，
    # 只是用广播代替点按——控件的点击手势由 uiautomator 手测（人工验证清单）。
    $beforeEditor = @($script:lastReport.editor.shortcuts).Count

    # 注意：`am broadcast --es` 的值**不能带空格**——adb shell 会把它拆成多个 token，
    # 后面所有 extra 都跟着错位（表现成「触发键不能为空」）。要带空格得自己加远端引号。
    $editorName = "编辑器测试"
    $semName = "编辑器语义"
    Bcast "editorAdd" @(
        "--es", "name", $editorName,
        "--es", "trigger", "f4",
        "--es", "action", "open_url",
        "--es", "value", "https://example.com/editor"
    )
    Start-Sleep -Milliseconds 500
    $script:lastReport = Get-Report
    $editorList = @($script:lastReport.editor.shortcuts)
    $added = @($editorList | Where-Object { $_.name -eq $editorName })
    $addedTrigger = if ($added.Count -eq 1) { @($added[0].triggers)[0] } else { "" }
    Check "S1-4 编辑器新建（小写 f4 补正成 F4）" (($added.Count -eq 1) -and ($addedTrigger -eq "F4") -and ($editorList.Count -eq $beforeEditor + 1)) "n=$($editorList.Count) trigger=$addedTrigger"

    $f4Ok = Test-KeyFlow -Label "F4 → 编辑器新建" -Send { SendKey 62 40 } -Assert { Rust-Fired "F4" }
    Check "S1-4 编辑器新建的 F4 立即生效" $f4Ok "$(Event-Digest $script:lastReport)"

    # 语义动作也走同一条路：编辑器写 `semantic` + 短名，执行器按 core 的 op 查表。
    Bcast "editorAdd" @(
        "--es", "name", $semName,
        "--es", "trigger", "F3",
        "--es", "action", "semantic",
        "--es", "value", "notifications"
    )
    Start-Sleep -Milliseconds 500
    $script:lastReport = Get-Report
    $semEntry = @($script:lastReport.editor.shortcuts | Where-Object { $_.name -eq $semName })
    Check "S1-4 编辑器写入语义动作（type=semantic）" (($semEntry.Count -eq 1) -and ($semEntry[0].actionType -eq "semantic")) "actionType=$($semEntry[0].actionType) summary=$($semEntry[0].actionSummary)"

    $f3Ok = Test-KeyFlow -Label "F3 → 编辑器语义动作" -Send { SendKey 61 40 } -Assert { Rust-Fired "F3" }
    $f3Rec = $script:lastReport.actions.'rust:F3'
    Check "S1-4 编辑器新建的语义动作执行成功" ($f3Ok -and $f3Rec -and $f3Rec.lastOk) "ok=$($f3Rec.lastOk) detail=$($f3Rec.lastDetail)"
    Shell "cmd statusbar collapse" | Out-Null

    # 撞车校验：core 把「同触发键」算硬冲突，编辑器据此拦保存（这里只验校验结果）。
    Bcast "editorCheck" @("--es", "trigger", "F4")
    Start-Sleep -Milliseconds 400
    $script:lastReport = Get-Report
    $dup = @($script:lastReport.editor.lastCheck.duplicates)
    Check "S1-4 编辑器撞车校验（F4 已被占用）" (([bool]$script:lastReport.editor.lastCheck.ok) -and ($dup.Count -ge 1)) "duplicates=$($dup -join ',')"

    Bcast "editorCheck" @("--es", "trigger", "Foo+K")
    Start-Sleep -Milliseconds 400
    $script:lastReport = Get-Report
    Check "S1-4 编辑器拒绝无效触发键" (-not [bool]$script:lastReport.editor.lastCheck.ok) "error=$($script:lastReport.editor.lastCheck.error)"

    # 删除：先删语义那条（后加的，下标在末尾），再按名字确认它不在了。
    $beforeDelete = @($script:lastReport.editor.shortcuts).Count
    $semIndex = if ($semEntry.Count -eq 1) { $semEntry[0].index } else { -1 }
    Bcast "editorDelete" @("--ei", "index", "$semIndex")
    Start-Sleep -Milliseconds 500
    $script:lastReport = Get-Report
    $afterDelete = @($script:lastReport.editor.shortcuts)
    Check "S1-4 编辑器删除条目" (($afterDelete.Count -eq $beforeDelete - 1) -and (@($afterDelete | Where-Object { $_.name -eq $semName }).Count -eq 0)) "n=$($afterDelete.Count)（删除前 $beforeDelete）"

    # S1-5 文本动作：`text` 走 ACTION_SET_TEXT 在光标处插入（不碰剪贴板），
    # 节点不接受才退回「写剪贴板 + 粘贴 + 尽量恢复」。三条短路：
    # F1 写剪贴板标记 / F2 插入文本 / F12 语义粘贴（用后两条验证剪贴板没被动过）。
    $textName = "编辑器文本"
    $clipName = "编辑器剪贴"
    $pasteName = "编辑器粘贴"
    Bcast "editorAdd" @("--es", "name", $clipName, "--es", "trigger", "F1", "--es", "action", "clipboard", "--es", "value", "clipmark")
    Bcast "editorAdd" @("--es", "name", $textName, "--es", "trigger", "F2", "--es", "action", "text", "--es", "value", "insertme")
    Bcast "editorAdd" @("--es", "name", $pasteName, "--es", "trigger", "F12", "--es", "action", "semantic", "--es", "value", "paste")
    Start-Sleep -Milliseconds 500
    $script:lastReport = Get-Report
    $textEntry = @($script:lastReport.editor.shortcuts | Where-Object { $_.name -eq $textName })
    Check "S1-5 编辑器写入 text 动作" (($textEntry.Count -eq 1) -and ($textEntry[0].actionType -eq "text")) "actionType=$($textEntry[0].actionType) summary=$($textEntry[0].actionSummary)"

    # 焦点 / 输入连接是这条链路唯一脆弱的地方（打字本身已由 S0 验过），失败重试一次。
    $textFlow = $false
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        if (-not (Focus-Field)) {
            Write-Host "  [warn] 文本插入用例没拿到焦点（第 $try 次）"
            continue
        }
        Clear-Field
        Type-Text "abc"
        SendKey 59 40            # F1：剪贴板写成 clipmark
        Start-Sleep -Milliseconds 300
        SendKey 60 40            # F2：text 动作
        Start-Sleep -Milliseconds 800
        $script:lastReport = Get-Report
        if ($script:lastReport.stats.inputFieldText -eq "abcinsertme") { $textFlow = $true; break }
        Write-Host ("  [warn] 文本插入第 {0} 次没中：field=[{1}] {2}" -f $try, $script:lastReport.stats.inputFieldText, (Event-Digest $script:lastReport))
    }
    $textRec = $script:lastReport.actions.'rust:F2'
    Check "S1-5 text 在光标处插入（保留原有文本）" ($textFlow -and $textRec -and $textRec.lastOk) "field=[$($script:lastReport.stats.inputFieldText)] ok=$($textRec.lastOk) detail=$($textRec.lastDetail)"

    # 剪贴板没被文本动作覆盖：随后按 F12 粘贴，得到的仍是 F1 写进去的标记。
    SendKey 88 40                # F12：semantic(paste)
    Start-Sleep -Milliseconds 800
    $script:lastReport = Get-Report
    $pasted = [string]$script:lastReport.stats.inputFieldText
    Check "S1-5 text 不覆盖剪贴板（随后粘贴仍是标记）" ($textFlow -and ($pasted -eq "abcinsertmeclipmark")) "field=[$pasted]"

    # S1-6 自更新（侧载）：清单 → 下载（sha256 校验）→ 交给系统安装器。
    # 清单走 `file://`：生产是 https，但解析 / 比较 / 下载 / 校验是同一条代码路径，
    # 把网络这一层摘掉是为了脚本不依赖外网与真实发布。
    $probeApkRemote = "update-probe.apk"
    $probeApkLocal = Join-Path $env:TEMP "kada-update-probe.apk"
    Adb @("push", $apkPath, "$extDir/$probeApkRemote") | Out-Null
    $apkBytes = (Get-Item $apkPath).Length
    $apkSha = (Get-FileHash $apkPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $probeApkUrl = "file://$extDir/$probeApkRemote"

    $manifestLocal = Join-Path $env:TEMP "kada-latest.json"
    $goodManifest = [pscustomobject]@{
        versionName = "0.2.0-s99"
        versionCode = 99
        url         = $probeApkUrl
        sha256      = $apkSha
        notes       = "verify.ps1 自更新用例"
    }
    [System.IO.File]::WriteAllText(
        $manifestLocal,
        ($goodManifest | ConvertTo-Json -Depth 5),
        (New-Object System.Text.UTF8Encoding($false))
    )
    Adb @("push", $manifestLocal, "$extDir/latest-test.json") | Out-Null

    Bcast "checkUpdate" @("--es", "url", "file://$extDir/latest-test.json")
    Start-Sleep -Milliseconds 1200
    $script:lastReport = Get-Report
    $upd = $script:lastReport.update
    Check "S1-6 检查更新（清单解析 + versionCode 比较）" (
        ($upd.phase -eq "available") -and [bool]($upd.newer) -and ($upd.lastCheck.versionCode -eq 99)
    ) "phase=$($upd.phase) current=$($upd.currentVersion)/$($upd.currentCode) manifest=$($upd.lastCheck.versionName)"

    Bcast "downloadUpdate"
    Start-Sleep -Milliseconds 4000
    $script:lastReport = Get-Report
    $upd = $script:lastReport.update
    Check "S1-6 下载并校验 sha256" (
        ($upd.phase -eq "ready") -and ($upd.apkBytes -eq $apkBytes) -and ($upd.lastResult -like "*已校验*")
    ) "phase=$($upd.phase) bytes=$($upd.apkBytes)/$apkBytes $($upd.lastResult)"

    # 没授权「安装未知应用」时必须明确回报，不许静默失败 / 假装已装
    Shell "appops set $package REQUEST_INSTALL_PACKAGES deny" | Out-Null
    Start-Sleep -Milliseconds 400
    Bcast "installUpdate"
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    $upd = $script:lastReport.update
    Check "S1-6 未授权安装时不假装安装" (($upd.phase -eq "need_permission") -and (-not [bool]($upd.canInstall))) "phase=$($upd.phase) canInstall=$($upd.canInstall) $($upd.lastResult)"

    # 授权后提交安装：系统安装器会接过前台（用户不点「安装」就不会真的装）
    Shell "appops set $package REQUEST_INSTALL_PACKAGES allow" | Out-Null
    Start-Sleep -Milliseconds 400
    Bcast "installUpdate"
    Start-Sleep -Milliseconds 1800
    $script:lastReport = Get-Report
    $upd = $script:lastReport.update
    $focusLine = @(Shell "dumpsys window" | Select-String -Pattern "mCurrentFocus=")
    $focus = if ($focusLine.Count -gt 0) { "$($focusLine[-1])" } else { "" }
    Check "S1-6 提交安装拉起系统安装器" (
        ($upd.phase -eq "installing") -and ($focus -match "packageinstaller|PackageInstaller")
    ) "phase=$($upd.phase) focus=$focus"

    # 取消安装（返回键），把应用拉回前台，别把后续步骤卡在系统对话框上
    Shell "input keyevent 4" | Out-Null
    Start-Sleep -Milliseconds 1500
    Restore-App

    # sha256 不匹配：必须失败，并且把下载的文件删掉（不许留个坏包在那儿）
    $badManifest = [pscustomobject]@{
        versionName = "0.2.0-s99"
        versionCode = 99
        url         = $probeApkUrl
        sha256      = ("0" * 64)
    }
    [System.IO.File]::WriteAllText(
        $manifestLocal,
        ($badManifest | ConvertTo-Json -Depth 5),
        (New-Object System.Text.UTF8Encoding($false))
    )
    Adb @("push", $manifestLocal, "$extDir/latest-bad.json") | Out-Null
    Bcast "checkUpdate" @("--es", "url", "file://$extDir/latest-bad.json")
    Start-Sleep -Milliseconds 1200
    Bcast "downloadUpdate"
    Start-Sleep -Milliseconds 4000
    $script:lastReport = Get-Report
    $upd = $script:lastReport.update
    Check "S1-6 sha256 不匹配 → 失败并删包" (
        ($upd.phase -eq "failed") -and ($upd.lastResult -like "*sha256 不匹配*") -and ($upd.apkBytes -eq 0)
    ) "phase=$($upd.phase) bytes=$($upd.apkBytes) $($upd.lastResult)"

    # S2 手机档：悬浮球 / 快捷面板 / 磁贴 / 通知动作 / 音量键触发。
    # 与界面同一份入口（BubbleService.runEntry / Keybind 执行器），adb 只代替点按。
    Shell "appops set $package SYSTEM_ALERT_WINDOW allow" | Out-Null
    Start-Sleep -Milliseconds 400
    Bcast "bubble" @("--ez", "on", "true")
    Start-Sleep -Milliseconds 1600
    $script:lastReport = Get-Report
    $ph = $script:lastReport.phone
    Check "S2-1 悬浮球启动（权限 + 前台服务）" (
        [bool]($ph.bubbleRunning) -and [bool]($ph.bubblePermission)
    ) "running=$($ph.bubbleRunning) perm=$($ph.bubblePermission) err=$($ph.bubbleLastError)"

    Bcast "bubbleShow"
    Start-Sleep -Milliseconds 800
    $script:lastReport = Get-Report
    Check "S2-1 快捷面板可打开（不抢焦点）" ([bool]$script:lastReport.phone.bubblePanel) "panel=$($script:lastReport.phone.bubblePanel)"
    Restore-App
    Start-Sleep -Milliseconds 300
    $focusLine = @(Shell "dumpsys window" | Select-String -Pattern "mCurrentFocus=")
    $focusNow = if ($focusLine.Count -gt 0) { "$($focusLine[-1])" } else { "" }
    Check "S2-1 面板不改变前台焦点" ($focusNow -match $package) "focus=$focusNow"
    Bcast "bubbleHide"

    # 面板执行 = 手机上「点一下」的主路径：插入模板（文本动作进前台输入框）。
    # 名字与文本一律 ASCII：PS 5.1 把中文参数按 GBK 交给 adb，设备侧按 UTF-8 解码就成了乱码。
    Bcast "editorAdd" @("--es", "name", "panelTpl", "--es", "trigger", "F10", "--es", "action", "text", "--es", "value", "insertok")
    Bcast "editorAdd" @("--es", "name", "panelUrl", "--es", "trigger", "F11", "--es", "action", "open_url", "--es", "value", "https://example.com/kada")
    Start-Sleep -Milliseconds 700

    $panelInsert = $false
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        if (-not (Focus-Field)) {
            Write-Host "  [warn] 面板插入用例没拿到焦点（第 $try 次）"
            continue
        }
        Clear-Field
        Type-Text "abc"
        Bcast "bubbleRun" @("--es", "name", "panelTpl")
        Start-Sleep -Milliseconds 1200
        $script:lastReport = Get-Report
        if ($script:lastReport.stats.inputFieldText -eq "abcinsertok") { $panelInsert = $true; break }
        Write-Host ("  [warn] 面板插入第 {0} 次没中：field=[{1}] last={2}" -f $try, $script:lastReport.stats.inputFieldText, $script:lastReport.phone.bubbleLastRun)
    }
    Check "S2-2 面板执行：插入模板（光标处）" $panelInsert "field=[$($script:lastReport.stats.inputFieldText)] last=$($script:lastReport.phone.bubbleLastRun)"
    $panelRec = $script:lastReport.actions.'rust:F10'
    Check "S2-2 面板执行计入执行记录" ($panelRec -and $panelRec.lastOk -and ($script:lastReport.phone.bubbleLastRun -like "*panelTpl*")) "ok=$($panelRec.lastOk) detail=$($panelRec.lastDetail)"

    Bcast "bubbleRun" @("--es", "name", "panelUrl")
    Start-Sleep -Milliseconds 2000
    $script:lastReport = Get-Report
    $webRec = $script:lastReport.actions.'rust:F11'
    # 前台包名在 `accessibility` 段（无障碍窗口事件更新），不是单独一段
    $fg = [string]$script:lastReport.accessibility.foregroundPackage
    Check "S2-2 面板执行：打开网址" ($webRec -and $webRec.lastOk -and ($fg -like "*chrome*")) "ok=$($webRec.lastOk) fg=$fg detail=$($webRec.lastDetail)"
    Restore-App

    # 音量键可作触发源：绑上 VolumeUp 后按下不该改音量（命中即吞）。
    Bcast "editorAdd" @("--es", "name", "volNotify", "--es", "trigger", "VolumeUp", "--es", "action", "semantic", "--es", "value", "notifications")
    Start-Sleep -Milliseconds 600
    Restore-App
    $volBeforeS2 = Get-Volume
    Shell "cmd media_session volume --stream 3 --set $volBeforeS2" | Out-Null
    Start-Sleep -Milliseconds 400
    SendKey 115 40
    Start-Sleep -Milliseconds 1200
    $script:lastReport = Get-Report
    $volAfterS2 = Get-Volume
    $volRec = $script:lastReport.actions.'rust:VolumeUp'
    $volFired = @($script:lastReport.rust.fires | Where-Object { $_.trigger -eq "VolumeUp" }).Count -ge 1
    Check "S2-3 音量键触发快捷键（命中即吞）" (
        $volFired -and $volRec -and $volRec.lastOk -and ($volAfterS2 -eq $volBeforeS2)
    ) "fired=$volFired ok=$($volRec.lastOk) volume=$volBeforeS2->$volAfterS2"
    Restore-App

    # 磁贴：服务必须带 BIND_QUICK_SETTINGS_TILE；`click-tile` 要求磁贴已在 QS 面板里，
    # 所以先 `add-tile` 再点——这样走的是 SystemUI → TileService.onClick 的真实路径。
    $tileDump = (Shell "dumpsys package $package") -join "`n"
    $tileRegistered = $tileDump -match "KadaTileService" -and $tileDump -match "BIND_QUICK_SETTINGS_TILE"
    Check "S2-4 磁贴已注册（BIND_QUICK_SETTINGS_TILE）" $tileRegistered ""
    $masterBeforeTile = [bool](Get-Report).phone.master
    Shell "cmd statusbar add-tile $package/.KadaTileService" | Out-Null
    Start-Sleep -Milliseconds 1200
    Shell "cmd statusbar click-tile $package/.KadaTileService" | Out-Null
    Start-Sleep -Milliseconds 1200
    $afterTile = Get-Report
    $tileFlipped = ([bool]$afterTile.phone.master -ne $masterBeforeTile)
    if (-not $tileFlipped) {
        # 个别模拟器 / ROM 没有 click-tile：退回同一切换函数的 adb 入口，并在 detail 里写明
        Bcast "tileToggle"
        Start-Sleep -Milliseconds 500
        $afterTile = Get-Report
        $tileFlipped = ([bool]$afterTile.phone.master -ne $masterBeforeTile)
        Write-Host "  [warn] cmd statusbar add-tile/click-tile 不可用，改走 tileToggle（同一函数）"
    }
    Check "S2-4 磁贴切换暂停 / 恢复" ($tileFlipped -and ($afterTile.phone.lastMasterToggle -like "tile:*")) "master=$($afterTile.phone.master) last=$($afterTile.phone.lastMasterToggle)"
    if (-not [bool]$afterTile.phone.master) { Bcast "master" @("--ez", "on", "true") }

    # 通知动作：与磁贴同一个总闸；暂停后按键必须原样放行（快捷键不再命中）。
    Shell "am start-foreground-service -n $package/.HeartbeatService" | Out-Null
    Start-Sleep -Milliseconds 800
    Bcast "notifyToggle"
    Start-Sleep -Milliseconds 900
    $pausedReport = Get-Report
    $firesBefore = @($pausedReport.rust.fires).Count
    SendKey 66 40
    Start-Sleep -Milliseconds 700
    $afterPause = Get-Report
    Check "S2-5 通知动作暂停后快捷键不触发" (
        (-not [bool]$afterPause.phone.master) -and (@($afterPause.rust.fires).Count -eq $firesBefore) -and
        ($afterPause.phone.lastMasterToggle -like "notify:*")
    ) "master=$($afterPause.phone.master) fires=$firesBefore->$(@($afterPause.rust.fires).Count) last=$($afterPause.phone.lastMasterToggle)"
    Bcast "notifyToggle"
    Start-Sleep -Milliseconds 600
    Check "S2-5 通知动作恢复后总闸打开" ([bool](Get-Report).phone.master) ""

    # S2b 画符号：手机上没有键，单手指画一个符号就是触发源。
    # 识别器是纯计算 + 几处「只有真跑一遍才作数」的阈值（旋转方向、扇区容差），
    # 所以自检跑在设备上（drawSelfTest → phone.draw.selfTest），脚本只断言结果。
    Bcast "drawSelfTest"
    Start-Sleep -Milliseconds 600
    $script:lastReport = Get-Report
    $drawTest = $script:lastReport.phone.draw.selfTest
    $drawCases = @($drawTest.cases)
    $drawBad = @($drawCases | Where-Object { -not $_.ok })
    Check "S2b-1 识别器自检全过（$($drawTest.total) 项）" (
        ($drawTest.failed -eq 0) -and ($drawCases.Count -eq $drawTest.total) -and ($drawCases.Count -ge 28)
    ) "failed=$($drawTest.failed)/$($drawTest.total) symbols=$($script:lastReport.phone.draw.symbols) bad=$(($drawBad | ForEach-Object { $_.name }) -join ',')"

    # 绑定：符号 → 现有快捷键。F6 在本段之前是空的（S1-2 那条随替换导入一起没了）；
    # 别用 F5——S1-3 合并进来的「S1 语义动作」一直留到最后，会撞车。
    Bcast "editorAdd" @("--es", "name", "drawTpl", "--es", "trigger", "F6", "--es", "action", "text", "--es", "value", "drawok")
    Start-Sleep -Milliseconds 700
    Bcast "drawBind" @("--es", "symbol", "triangle", "--es", "name", "drawTpl")
    Start-Sleep -Milliseconds 500
    $script:lastReport = Get-Report
    $drawEntry = @($script:lastReport.editor.shortcuts | Where-Object { $_.name -eq "drawTpl" })
    Check "S2b-2 符号绑定到快捷键（triangle → drawTpl）" (
        ($drawEntry.Count -eq 1) -and ([string]$script:lastReport.phone.draw.bindings.triangle -eq "drawTpl")
    ) "entry=$($drawEntry.Count) bindings=$($script:lastReport.phone.draw.bindings | ConvertTo-Json -Compress) err=$($script:lastReport.phone.draw.lastError)"

    # 画三角 = 触发那条快捷键：与前两段同一条执行链（KeyFilterService.runShortcut）。
    # 路径坐标是 0~1 归一化的折线，`--es path` 里的分号必须由设备侧 shell 当引号内容吃下，
    # 所以走 `Shell` 传整串（Bcast 直接拼参数会被 `;` 拆成两条命令）。
    $drawFlow = $false
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        if (-not (Focus-Field)) {
            Write-Host "  [warn] 画符号用例没拿到焦点（第 $try 次）"
            continue
        }
        Clear-Field
        Type-Text "abc"
        Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '0.5,0.05;0.95,0.95;0.05,0.95;0.5,0.05'" | Out-Null
        Start-Sleep -Milliseconds 1000
        $script:lastReport = Get-Report
        if ($script:lastReport.stats.inputFieldText -eq "abcdrawok") { $drawFlow = $true; break }
        Write-Host ("  [warn] 画三角第 {0} 次没命中：field=[{1}] symbol={2} err={3}" -f $try, $script:lastReport.stats.inputFieldText, $script:lastReport.phone.draw.lastSymbol, $script:lastReport.phone.draw.lastError)
    }
    Check "S2b-3 画三角执行绑定快捷键（插入文本）" (
        $drawFlow -and [string]$script:lastReport.phone.draw.lastSymbol -eq "triangle"
    ) "field=[$($script:lastReport.stats.inputFieldText)] symbol=$($script:lastReport.phone.draw.lastSymbol) score=$($script:lastReport.phone.draw.lastScore)"

    # 45° 附近不猜方向：宁可没反应，也不能执行错的快捷键。
    # 这条三角绕中心转了 44°，方向落在两个扇区中间——识别器必须拒绝，输入框一个字都不许多。
    $beforeEdge = [string]$script:lastReport.stats.inputFieldText
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '0.8126,0.1763;0.5111,1.1363;-0.1363,0.5111;0.8126,0.1763'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    Check "S2b-4 44 度三角不执行（方向不明确）" (
        [string]::IsNullOrEmpty([string]$script:lastReport.phone.draw.lastSymbol) -and
        ([string]$script:lastReport.stats.inputFieldText -eq $beforeEdge) -and
        (([string]$script:lastReport.phone.draw.lastError).Length -gt 0)
    ) "field=[$($script:lastReport.stats.inputFieldText)] before=[$beforeEdge] symbol=[$($script:lastReport.phone.draw.lastSymbol)] err=$($script:lastReport.phone.draw.lastError)"

    # 画板浮层：与「长按悬浮球」同一个入口（BubbleService.showBoard），必须不抢前台焦点。
    Restore-App
    Bcast "drawBoard"
    Start-Sleep -Milliseconds 1500
    $script:lastReport = Get-Report
    $boardOn = [bool]$script:lastReport.phone.bubbleBoard
    $boardFocus = @(Shell "dumpsys window" | Select-String -Pattern "mCurrentFocus=")
    $boardFocusNow = if ($boardFocus.Count -gt 0) { "$($boardFocus[-1])" } else { "" }
    Check "S2b-5 画板浮层可打开（不抢焦点）" ($boardOn -and ($boardFocusNow -match $package)) "board=$boardOn focus=$boardFocusNow"
    Bcast "drawHide"
    Start-Sleep -Milliseconds 500
    Check "S2b-5 画板可收起" (-not [bool](Get-Report).phone.bubbleBoard) ""

    # 自定义画法（P2）：13 个预设之外，让用户教它认自己的符号。
    # 形状挑「楼梯」——预设里没有，能被认出来只可能来自刚录的那一份模板。
    # 与 `drawRun` 同样：`--es path` 里的分号必须由设备侧 shell 当引号内容吃下，走 `Shell` 传整串。
    $stairsPath = "0.10,0.85;0.10,0.50;0.45,0.50;0.45,0.15;0.90,0.15"
    $customBefore = @((Get-Report).phone.draw.customIds)
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawLearn --es label stairs --es path '$stairsPath'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    $customAfter = @($script:lastReport.phone.draw.customIds)
    $newIds = @($customAfter | Where-Object { $customBefore -notcontains $_ })
    $newId = if ($newIds.Count -eq 1) { [string]$newIds[0] } else { "" }
    Check "S2b-6 录自定义符号（楼梯，落盘一条）" (
        ($newId -like "u:*") -and ($customAfter.Count -eq $customBefore.Count + 1) -and
        ([string]::IsNullOrEmpty([string]$script:lastReport.phone.draw.lastError))
    ) "custom=$($customBefore.Count)->$($customAfter.Count) new=$newId err=$($script:lastReport.phone.draw.lastError)"

    # P2b/P3：符号必须落在**配置**的 `draw.symbols` 里（P2a 那份 SharedPreferences 已不是存法），
    # 点坐标是 0~1000 的整数（core 的 `DRAW_POINTS_SCALE` 口径，与触摸板模板同一套）。
    # P3 起笔画是嵌套数组 `strokes: [[[x,y],…], …]`——一笔的符号就是只有一个元素的数组。
    $cfg = Get-ConfigJson
    $cfgSymbols = @()
    if ($cfg -and $cfg.draw) { $cfgSymbols = @($cfg.draw.symbols | Where-Object { $null -ne $_ }) }
    $cfgSymbol = @($cfgSymbols | Where-Object { $_.id -eq $newId })
    $cfgStrokes = @()
    if ($cfgSymbol.Count -eq 1) { $cfgStrokes = @($cfgSymbol[0].strokes | Where-Object { $null -ne $_ }) }
    $cfgPoints = @()
    $ptsBad = @()
    foreach ($st in $cfgStrokes) {
        $line = @($st)
        $cfgPoints += $line
        if ($line.Count -lt 2) { $ptsBad += "stroke<2" }
        $ptsBad += @($line | Where-Object { @($_).Count -ne 2 -or $_[0] -lt 0 -or $_[0] -gt 1000 -or $_[1] -lt 0 -or $_[1] -gt 1000 })
    }
    $ptsFirst = if ($cfgPoints.Count -gt 0) { "$($cfgPoints[0][0]),$($cfgPoints[0][1])" } else { "" }
    Check "S2b-6b 自定义符号写进配置 draw.symbols（strokes 嵌套）" (
        ($newId -ne "") -and ($cfgSymbol.Count -eq 1) -and ($cfgStrokes.Count -eq 1) -and
        ($cfgPoints.Count -ge 2) -and ($ptsBad.Count -eq 0) -and
        ([string]$script:lastReport.phone.draw.store -eq "config")
    ) "store=$($script:lastReport.phone.draw.store) symbol=$newId strokes=$($cfgStrokes.Count) points=$($cfgPoints.Count) first=$ptsFirst bad=$($ptsBad.Count) lastSave=$($script:lastReport.phone.draw.lastSave)"

    # 绑定 + 画它：走的是与预设完全相同的一条链（识别表里只是多了一份模板）。
    Bcast "editorAdd" @("--es", "name", "drawCust", "--es", "trigger", "Ctrl+Alt+J", "--es", "action", "text", "--es", "value", "stairsok")
    Start-Sleep -Milliseconds 700
    if ($newId -ne "") { Bcast "drawBind" @("--es", "symbol", $newId, "--es", "name", "drawCust") }
    $custFlow = $false
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        if (-not (Focus-Field)) {
            Write-Host "  [warn] 自定义画法用例没拿到焦点（第 $try 次）"
            continue
        }
        Clear-Field
        Type-Text "xy"
        # 与模板不完全重合（每条边挪 0.01）：验的是识别而不是字符串相等
        Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '0.11,0.86;0.11,0.51;0.46,0.51;0.46,0.16;0.91,0.16'" | Out-Null
        Start-Sleep -Milliseconds 1000
        $script:lastReport = Get-Report
        if ($script:lastReport.stats.inputFieldText -eq "xystairsok") { $custFlow = $true; break }
        Write-Host ("  [warn] 画自定义符号第 {0} 次没命中：field=[{1}] symbol={2} lastRun={3} err={4}" -f $try, $script:lastReport.stats.inputFieldText, $script:lastReport.phone.draw.lastSymbol, $script:lastReport.phone.draw.lastRun, $script:lastReport.phone.draw.lastError)
    }
    Check "S2b-7 画自定义符号执行绑定快捷键" (
        $custFlow -and ([string]$script:lastReport.phone.draw.lastSymbol -eq $newId) -and
        ($newId -ne "")
    ) "field=[$($script:lastReport.stats.inputFieldText)] symbol=$($script:lastReport.phone.draw.lastSymbol) lastRun=$($script:lastReport.phone.draw.lastRun) err=$($script:lastReport.phone.draw.lastError)"

    # 绑定也进配置：`draw.bindings[符号 id] = 条目名`。存名字不存下标——删一条条目不会串位。
    $cfg = Get-ConfigJson
    $cfgBind = if ($cfg -and $cfg.draw) { [string](Get-JsonField $cfg.draw.bindings $newId) } else { "" }
    Check "S2b-7b 绑定写进配置 draw.bindings（→ 条目名）" (
        ($newId -ne "") -and ($cfgBind -eq "drawCust")
    ) "bindings=$($script:lastReport.phone.draw.bindings | ConvertTo-Json -Compress) config=[$cfgBind] lastSave=$($script:lastReport.phone.draw.lastSave)"

    # 与已有符号「几乎一样」的笔画必须拒收：两份模板都留着，每次识别都会在边界上抖。
    # 这里画的就是标准三角，错误文案要点名「三角」。
    $customKeep = @((Get-Report).phone.draw.customIds)
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawLearn --es label duptri --es path '0.5,0.05;0.95,0.95;0.05,0.95;0.5,0.05'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    $customDup = @($script:lastReport.phone.draw.customIds)
    Check "S2b-8 与预设几乎一样的画法被拒收" (
        ($customDup.Count -eq $customKeep.Count) -and
        (([string]$script:lastReport.phone.draw.lastError) -like "*几乎一样*")
    ) "custom=$($customKeep.Count)->$($customDup.Count) err=$($script:lastReport.phone.draw.lastError)"

    # 删除自定义符号要连绑定一起带走：留下取不出来的键，下次导出就是脏数据。
    if ($newId -ne "") { Bcast "drawForget" @("--es", "symbol", $newId) }
    Start-Sleep -Milliseconds 600
    $script:lastReport = Get-Report
    $customLeft = @($script:lastReport.phone.draw.customIds)
    $boundNames = @($script:lastReport.phone.draw.bindings.PSObject.Properties.Name)
    Check "S2b-9 删自定义符号并带走绑定" (
        ($newId -ne "") -and ($customLeft -notcontains $newId) -and ($boundNames -notcontains $newId) -and
        ([string]::IsNullOrEmpty([string]$script:lastReport.phone.draw.lastError))
    ) "custom=$($customLeft.Count) bindings=$($script:lastReport.phone.draw.bindings | ConvertTo-Json -Compress) err=$($script:lastReport.phone.draw.lastError)"

    # 删符号与清绑定是**同一次写盘**：配置里该 id 一个字节都不该剩（`triangle` 的绑定不受影响）。
    $cfgText = Get-ConfigText
    $cfgHasId = if ($newId -ne "") { $cfgText -match [regex]::Escape($newId) } else { $true }
    $cfgStillTriangle = $cfgText -match '"triangle"'
    Check "S2b-9b 删除同时清掉配置里的符号与绑定" (
        ($newId -ne "") -and (-not $cfgHasId) -and $cfgStillTriangle
    ) "config_has_id=$cfgHasId triangle_kept=$cfgStillTriangle lastSave=$($script:lastReport.phone.draw.lastSave)"

    # P3 多笔画：一个符号可以是 1~3 笔，**笔数与画的先后顺序**都是身份（「圆 + 竖」≠「竖 + 圆」）。
    # 这里录「圆 + 竖」：两笔都不陌生，能被认出来且顺序敏感才说明多笔画真的通了。
    $circleStroke = "0.50,0.05;0.82,0.18;0.95,0.50;0.82,0.82;0.50,0.95;0.18,0.82;0.05,0.50;0.18,0.18;0.50,0.05"
    $barStroke = "0.50,0.10;0.50,0.90"
    Bcast "editorAdd" @("--es", "name", "drawMulti", "--es", "trigger", "Ctrl+Alt+M", "--es", "action", "text", "--es", "value", "multiok")
    Start-Sleep -Milliseconds 700
    $script:lastReport = Get-Report
    $multiEntry = @($script:lastReport.editor.shortcuts | Where-Object { $_.name -eq "drawMulti" })
    Check "S2b-12a 多笔画测试条目建好（Ctrl+Alt+M）" (
        ($multiEntry.Count -eq 1) -and (@($multiEntry[0].triggers) -contains "Ctrl+Alt+M")
    ) "entry=$($multiEntry.Count) triggers=$(@($multiEntry[0].triggers) -join ',')"
    $customBefore2 = @((Get-Report).phone.draw.customIds)
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawLearn --es label circbar --es path '$circleStroke|$barStroke'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    $customAfter2 = @($script:lastReport.phone.draw.customIds)
    $multiIds = @($customAfter2 | Where-Object { $customBefore2 -notcontains $_ })
    $multiId = if ($multiIds.Count -eq 1) { [string]$multiIds[0] } else { "" }
    Check "S2b-12 录多笔画符号（圆 + 竖）" (
        ($multiId -like "u:*") -and ($customAfter2.Count -eq $customBefore2.Count + 1) -and
        ([string]::IsNullOrEmpty([string]$script:lastReport.phone.draw.lastError))
    ) "custom=$($customBefore2.Count)->$($customAfter2.Count) new=$multiId err=$($script:lastReport.phone.draw.lastError)"

    # 落盘形态：`strokes` 必须是**两条**，且第二笔的 y 跨度明显大于 x（竖线）——
    # 只断言数量会被「两笔都截成同样形状」的 bug 骗过去。
    $cfg = Get-ConfigJson
    $cfgSymbols = @()
    if ($cfg -and $cfg.draw) { $cfgSymbols = @($cfg.draw.symbols | Where-Object { $null -ne $_ }) }
    $cfgMulti = @($cfgSymbols | Where-Object { $_.id -eq $multiId })
    $multiStrokes = @()
    if ($cfgMulti.Count -eq 1) { $multiStrokes = @($cfgMulti[0].strokes | Where-Object { $null -ne $_ }) }
    $multiBad = @()
    foreach ($st in $multiStrokes) {
        $line = @($st)
        if ($line.Count -lt 2) { $multiBad += "stroke<2" }
        $multiBad += @($line | Where-Object { @($_).Count -ne 2 -or $_[0] -lt 0 -or $_[0] -gt 1000 -or $_[1] -lt 0 -or $_[1] -gt 1000 })
    }
    $multiInfo = @($script:lastReport.phone.draw.customList | Where-Object { $_.id -eq $multiId })
    Check "S2b-12b 多笔画在配置里存成两笔（先圆后竖）" (
        ($multiId -ne "") -and ($multiStrokes.Count -eq 2) -and ($multiBad.Count -eq 0) -and
        ($multiInfo.Count -eq 1) -and ([int]$multiInfo[0].strokes -eq 2)
    ) "symbol=$multiId strokes=$($multiStrokes.Count) report_strokes=$(if ($multiInfo.Count -eq 1) { $multiInfo[0].strokes } else { '-' }) bad=$($multiBad.Count) lastSave=$($script:lastReport.phone.draw.lastSave)"

    # 绑定 + 画它：第二笔稍微错位（+0.01 / +0.02），验的是形状与顺序而不是字符串相等。
    if ($multiId -ne "") { Bcast "drawBind" @("--es", "symbol", $multiId, "--es", "name", "drawMulti") }
    $multiFlow = $false
    for ($try = 1; $try -le 2; $try++) {
        Restore-App
        if (-not (Focus-Field)) {
            Write-Host "  [warn] 多笔画用例没拿到焦点（第 $try 次）"
            continue
        }
        Clear-Field
        Type-Text "mn"
        Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '0.51,0.06;0.83,0.19;0.96,0.51;0.83,0.83;0.51,0.96;0.19,0.83;0.06,0.51;0.19,0.19;0.51,0.06|0.52,0.11;0.52,0.91'" | Out-Null
        Start-Sleep -Milliseconds 1000
        $script:lastReport = Get-Report
        if ($script:lastReport.stats.inputFieldText -eq "mnmultiok") { $multiFlow = $true; break }
        Write-Host ("  [warn] 画多笔画符号第 {0} 次没命中：field=[{1}] symbol={2} lastRun={3} err={4}" -f $try, $script:lastReport.stats.inputFieldText, $script:lastReport.phone.draw.lastSymbol, $script:lastReport.phone.draw.lastRun, $script:lastReport.phone.draw.lastError)
    }
    Check "S2b-13 画多笔画符号执行绑定快捷键" (
        $multiFlow -and ([string]$script:lastReport.phone.draw.lastSymbol -eq $multiId) -and ($multiId -ne "")
    ) "field=[$($script:lastReport.stats.inputFieldText)] symbol=$($script:lastReport.phone.draw.lastSymbol) lastRun=$($script:lastReport.phone.draw.lastRun) err=$($script:lastReport.phone.draw.lastError)"

    # 只画第一笔：多笔画符号不参与竞争，回到「圆」这个单笔画预设（**不能**抢着执行圆 + 竖）。
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '$circleStroke'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    Check "S2b-14 只画第一笔仍按单笔画「圆」识别" (
        ([string]$script:lastReport.phone.draw.lastSymbol -eq "circle")
    ) "symbol=$($script:lastReport.phone.draw.lastSymbol) score=$($script:lastReport.phone.draw.lastScore) err=$($script:lastReport.phone.draw.lastError)"

    # 顺序反了 = 另一个符号（不存在 → 不执行）。顺序就是身份，反着画必须认不出来。
    Shell "am broadcast -a com.kada.spike.CONTROL -n $receiver --es op drawRun --es path '$barStroke|$circleStroke'" | Out-Null
    Start-Sleep -Milliseconds 900
    $script:lastReport = Get-Report
    Check "S2b-15 笔画顺序反了不命中" (
        [string]::IsNullOrEmpty([string]$script:lastReport.phone.draw.lastSymbol) -and
        (([string]$script:lastReport.phone.draw.lastError).Length -gt 0)
    ) "symbol=[$($script:lastReport.phone.draw.lastSymbol)] err=$($script:lastReport.phone.draw.lastError)"

    # 收尾删掉，别让 S2b-11（整份替换配置后 custom=0）被这一轮新录的符号带崩。
    if ($multiId -ne "") { Bcast "drawForget" @("--es", "symbol", $multiId) }
    Start-Sleep -Milliseconds 600
    $script:lastReport = Get-Report
    $boundNames = @($script:lastReport.phone.draw.bindings.PSObject.Properties.Name)
    Check "S2b-16 删多笔画符号并带走绑定" (
        ($multiId -ne "") -and (@($script:lastReport.phone.draw.customIds) -notcontains $multiId) -and
        ($boundNames -notcontains $multiId)
    ) "custom=$(@($script:lastReport.phone.draw.customIds).Count) bindings=$($script:lastReport.phone.draw.bindings | ConvertTo-Json -Compress) err=$($script:lastReport.phone.draw.lastError)"

    # 升级迁移（P2a → P2b）：老版本的符号 / 绑定存在 `SharedPreferences("kada-draw")` 里。
    # 只往 prefs 里塞一份「P2a 格式」的数据再重启，断言搬到配置、旧键清空——否则老用户
    # 升级后自己录的符号凭空消失，那是**静默丢数据**。
    $legacyXml = @'
<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<map>
    <string name="bindings">{&quot;u:legacy1&quot;:&quot;drawTpl&quot;}</string>
    <string name="custom">[{&quot;id&quot;:&quot;u:legacy1&quot;,&quot;label&quot;:&quot;legacy&quot;,&quot;closed&quot;:false,&quot;points&quot;:[[0.1,0.85],[0.1,0.5],[0.45,0.5],[0.45,0.15],[0.9,0.15]]}]</string>
</map>
'@
    # 上一轮跑崩 / 手工测过可能留下同名符号：先清掉，才是「升级前什么都没有」的状态。
    $cfgText = Get-ConfigText
    if ($cfgText -match '"u:legacy1"') {
        Bcast "drawForget" @("--es", "symbol", "u:legacy1")
        Start-Sleep -Milliseconds 700
    }
    $legacyPath = Join-Path $env:TEMP "kada-legacy-draw.xml"
    [System.IO.File]::WriteAllText($legacyPath, $legacyXml, (New-Object System.Text.UTF8Encoding($false)))
    Adb @("push", $legacyPath, "/data/local/tmp/kada-legacy-draw.xml") | Out-Null
    Shell "run-as $package cp /data/local/tmp/kada-legacy-draw.xml shared_prefs/kada-draw.xml" | Out-Null
    Shell "rm -f /data/local/tmp/kada-legacy-draw.xml" | Out-Null
    # force-stop 会连无障碍服务一起杀，起来之后重新登记一次（别把设备留在「服务未连接」）
    Shell "am force-stop $package" | Out-Null
    Start-Sleep -Milliseconds 800
    Shell "am start -n $package/.MainActivity" | Out-Null
    Start-Sleep -Seconds 2
    Shell "settings put secure enabled_accessibility_services $package/$package.KeyFilterService" | Out-Null
    Shell "settings put secure accessibility_enabled 1" | Out-Null
    Start-Sleep -Milliseconds 800
    $script:lastReport = Get-Report
    $cfg = Get-ConfigJson
    $legacySymbol = @()
    $legacyBind = ""
    $legacyFirst = ""
    if ($cfg -and $cfg.draw) {
        $legacySymbol = @(@($cfg.draw.symbols) | Where-Object { $_ -and $_.id -eq "u:legacy1" })
        $legacyBind = [string](Get-JsonField $cfg.draw.bindings "u:legacy1")
        if ($legacySymbol.Count -eq 1) {
            # P3 起符号是 `strokes` 嵌套数组；P2a 的单笔画搬进来就是一层。
            $legacyStrokes = @($legacySymbol[0].strokes | Where-Object { $null -ne $_ })
            $legacyPts = if ($legacyStrokes.Count -ge 1) { @($legacyStrokes[0]) } else { @() }
            if ($legacyPts.Count -gt 0) { $legacyFirst = "$($legacyPts[0][0]),$($legacyPts[0][1])" }
        }
    }
    $prefsText = (Shell "run-as $package cat shared_prefs/kada-draw.xml") -join "`n"
    $prefsClean = ($prefsText -match "<map\s*/>") -and ($prefsText -notmatch "u:legacy1")
    Check "S2b-10 旧 prefs 迁移进配置（符号 + 绑定，旧键清空）" (
        ($legacySymbol.Count -eq 1) -and ($legacyBind -eq "drawTpl") -and
        ($legacyFirst -eq "100,850") -and $prefsClean -and
        ([string]$script:lastReport.phone.draw.store -eq "config")
    ) "symbol=$($legacySymbol.Count) first=$legacyFirst bind=[$legacyBind] prefs_clean=$prefsClean lastSave=$($script:lastReport.phone.draw.lastSave)"

    # 收尾：关悬浮球、收回权限、把磁贴从 QS 面板撤掉、删测试条目
    Bcast "bubble" @("--ez", "on", "false")
    Start-Sleep -Milliseconds 500
    Shell "cmd statusbar remove-tile $package/.KadaTileService" | Out-Null
    Shell "appops set $package SYSTEM_ALERT_WINDOW deny" | Out-Null
    # 画符号绑定现在存在配置的 `draw` 段里，但**清洗不会因为目标条目没了就删绑定**——
    # 那是「已失效」，面板要看得出来（由 `detect_conflicts` 报 Warn）。所以测试仍要显式解绑，
    # 别把「绑定的快捷键不在配置里」的告警留在设备上。
    Bcast "drawUnbind" @("--es", "symbol", "triangle")
    @("VolumeUp", "F10", "F11", "F6", "Ctrl+Alt+J", "Ctrl+Alt+M") | ForEach-Object {
        $trigger = $_
        $idx = -1
        $entries = @((Get-Report).editor.shortcuts)
        for ($i = 0; $i -lt $entries.Count; $i++) {
            if (@($entries[$i].triggers) -contains $trigger) { $idx = $i }
        }
        if ($idx -ge 0) { Bcast "editorDelete" @("--ei", "index", "$idx"); Start-Sleep -Milliseconds 400 }
    }
    $script:lastReport = Get-Report
    Check "S2 收尾：悬浮球已停、总闸已恢复" (
        (-not [bool]$script:lastReport.phone.bubbleRunning) -and [bool]$script:lastReport.phone.master
    ) "running=$($script:lastReport.phone.bubbleRunning) master=$($script:lastReport.phone.master)"

    # 收尾：把基线配置换回来，别把测试条目留在设备上
    Adb @("push", $exportLocal, "$extDir/restore.json") | Out-Null
    Bcast "import" @("--es", "file", "restore.json", "--ez", "merge", "false")
    Start-Sleep -Milliseconds 800

    # 这次整份替换导入不经过 `DrawStore`（导入走 `RustBridge.importFrom`），正好是
    # 「配置被别的路径换掉」的场景：符号表缓存不能还认旧值。曾经的现象是配置里已经没有
    # `draw` 段了，面板仍列着旧符号、报告里 `custom` 还是 1（陈旧缓存）。
    $script:lastReport = Get-Report
    $cfgText = Get-ConfigText
    $cfgHasDraw = $cfgText -match '"draw"'
    Check "S2b-11 整份替换配置后符号表跟着清（缓存不认旧值）" (
        ([int]$script:lastReport.phone.draw.custom -eq 0) -and (-not $cfgHasDraw) -and
        ($cfgText -notmatch "u:legacy1")
    ) "custom=$($script:lastReport.phone.draw.custom) config_draw=$cfgHasDraw bindings=$($script:lastReport.phone.draw.bindings | ConvertTo-Json -Compress)"
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
