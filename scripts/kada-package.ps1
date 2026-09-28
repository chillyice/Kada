# 打包 Kada（Tauri release）并在构建成功后自动重启新构建的程序。
#
# 给 Kada 自己的一条「执行命令」动作用（动作的 shell 选 PowerShell），命令填：
#
#   Start-Process powershell -WindowStyle Minimized -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File C:\Users\chill\OneDrive\WorkStation\Projects\Kada\scripts\kada-package.ps1'
#
# 为什么拆成脚本 + 这么一行：Kada 执行 PowerShell 命令走的是
# `powershell -NoProfile -Command "<命令原文>"`，而 Rust 起进程时会给命令里的双引号加反斜杠转义，
# Windows PowerShell 5.1 的原生命令行解析对嵌套双引号并不可靠（cmd 那条命令当年就是因此改用临时
# .bat）。所以动作命令里**一个双引号都不要出现**：单引号包参数 + 具体逻辑放脚本文件里。
#
# 流程：杀运行中实例（不杀 exe 被锁，构建报「拒绝访问」）→ npx tauri build → 成功则启动新 exe。
# 构建失败会把旧版本重新拉起来（否则这个动作会把 Kada 弄死，连打包键都没了），并留窗口给看报错。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\kada-package.ps1               # 只出裸 exe（最快）
#   powershell -ExecutionPolicy Bypass -File scripts\kada-package.ps1 -Bundle       # 另外出 MSI/NSIS
#   powershell -ExecutionPolicy Bypass -File scripts\kada-package.ps1 -DryRun       # 只打印步骤，不动进程
#   powershell -ExecutionPolicy Bypass -File scripts\kada-package.ps1 -RestartArgs '--autostart'  # 重启时静默到托盘

[CmdletBinding()]
param(
    # 项目根目录（含 tauri.conf.json 与 package.json）。
    [string]$ProjectDir = 'C:\Users\chill\OneDrive\WorkStation\Projects\Kada',
    # 出安装包（MSI/NSIS）。默认只出裸 exe，快得多。
    [switch]$Bundle,
    # 只打印将要做的步骤：不杀进程、不构建、不重启（改命令或排错时先跑这个）。
    [switch]$DryRun,
    # 重启新程序时附加的参数（如 '--autostart' 让它静默到托盘、不弹主窗口）。
    [string]$RestartArgs = ''
)

$ErrorActionPreference = 'Stop'

$ProjectDir = (Resolve-Path -LiteralPath $ProjectDir).Path
$exe = Join-Path $ProjectDir 'target\release\kada.exe'
$log = Join-Path $PSScriptRoot 'kada-package.log'

# 全程记一份日志：真正干活的是脱离出来的最小化窗口，出问题时日志比窗口好翻（*.log 已在 .gitignore）。
try { Start-Transcript -Path $log -Force | Out-Null } catch { Write-Warning "写日志失败：$_" }

function Write-Step([string]$Message) {
    Write-Host ("[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $Message)
}

Write-Step "Kada 打包开始：$ProjectDir"
if ($DryRun) { Write-Step '（DryRun：只打印步骤，不杀进程、不构建、不重启）' }

# 1) 关掉运行中的实例：exe 被运行中的进程锁住，不关构建会报「拒绝访问」。
#    必须只按进程名关——执行本脚本的进程是 powershell，不会被误伤（Kada 是它的父进程）。
$running = @(Get-Process -Name kada -ErrorAction SilentlyContinue)
if ($running.Count -gt 0) {
    Write-Step "关闭运行中的 Kada（PID $($running.Id -join ', ')）"
    if (-not $DryRun) {
        $running | Stop-Process -Force
        # 等进程真的退出、文件句柄释放，否则紧接着的构建仍可能撞上占用。
        $deadline = (Get-Date).AddSeconds(10)
        while ((Get-Process -Name kada -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 100
        }
        if (Get-Process -Name kada -ErrorAction SilentlyContinue) {
            Write-Warning 'Kada 进程 10 秒内没退出，构建可能因 exe 被占用而失败'
        }
    }
} else {
    Write-Step '没有运行中的 Kada 实例'
}

# 2) 构建（工作目录必须是项目根，npx 才找得到 tauri.conf.json / package.json）。
$npx = (Get-Command npx.cmd -ErrorAction SilentlyContinue).Source
if (-not $npx) { $npx = 'npx.cmd' }
$npxArgs = @('--yes', '@tauri-apps/cli', 'build')
if (-not $Bundle) { $npxArgs += '--no-bundle' }
Write-Step ("构建：{0} {1}" -f $npx, ($npxArgs -join ' '))

$code = 0
if (-not $DryRun) {
    Push-Location $ProjectDir
    try {
        & $npx @npxArgs
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
}

# 3) 成功 → 启动新 exe；失败 → 把旧版本拉回来（别让打包动作把工具弄死）+ 留窗口看报错。
$exists = Test-Path -LiteralPath $exe
if ($code -eq 0 -and -not $exists) {
    Write-Host "构建报成功但没找到 $exe" -ForegroundColor Red
    $code = 1
}

if ($code -ne 0) {
    Write-Host ("构建失败（exit {0}）：不重启新版本。" -f $code) -ForegroundColor Red
    if ($exists -and -not $DryRun) {
        Write-Step "恢复启动旧版本：$exe"
        Start-Process -FilePath $exe
    }
    Read-Host '按回车关闭此窗口'
    try { Stop-Transcript | Out-Null } catch {}
    exit $code
}

Write-Step ("启动新构建：{0}{1}" -f $exe, $(if ($RestartArgs) { " $RestartArgs" } else { '' }))
if (-not $DryRun) {
    if ($RestartArgs) { Start-Process -FilePath $exe -ArgumentList $RestartArgs }
    else { Start-Process -FilePath $exe }
}
Write-Step '打包完成'
try { Stop-Transcript | Out-Null } catch {}
