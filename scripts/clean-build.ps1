# 清理 Kada 的构建产物，把 target/ 从几十 GB 收回几 GB。
#
# 默认保留 target/release（裸 exe + deps，增量编译还靠它，且 Kada 运行时就是从那儿起的），
# 清掉的全是「删了只会变慢、不会丢东西」的：target/debug（增量缓存通常占大头）、
# target/tmp、其余 target 子目录（交叉编译产物，如 aarch64-apple-darwin / x86_64-unknown-linux-gnu）。
#
# 用法：
#   powershell -ExecutionPolicy Bypass -File scripts\clean-build.ps1            # 清 debug / 交叉产物
#   powershell -ExecutionPolicy Bypass -File scripts\clean-build.ps1 -Bundle    # 连安装包产物一起清
#   powershell -ExecutionPolicy Bypass -File scripts\clean-build.ps1 -DryRun    # 只列出会删什么

[CmdletBinding()]
param(
    # 项目根目录（含 target/）。留空则按脚本自身位置推断（scripts/ 的上一级）。
    [string]$ProjectDir,
    # 连安装包产物一起清：target/release/bundle、target/release/nsis。
    [switch]$Bundle,
    # 只打印将删除的目录与体积，不真删。
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'

# 不把 $PSScriptRoot 写进 param 默认值：Windows PowerShell 5.1 用相对路径 `-File` 调用时，
# 参数绑定阶段 $PSScriptRoot 还是空的（Split-Path 会直接报错）。放到函数体里再推断。
if (-not $ProjectDir) {
    $scriptPath = $MyInvocation.MyCommand.Path
    if (-not $scriptPath -and $PSScriptRoot) { $scriptPath = Join-Path $PSScriptRoot 'clean-build.ps1' }
    if ($scriptPath) { $ProjectDir = Split-Path -Parent (Split-Path -Parent (Convert-Path -LiteralPath $scriptPath)) }
    else { $ProjectDir = (Get-Location).Path }
}

$ProjectDir = (Resolve-Path -LiteralPath $ProjectDir).Path
$targetDir = Join-Path $ProjectDir 'target'

function Write-Step([string]$Message) {
    Write-Host ("[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $Message)
}

function Get-DirSize([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return $sum
}

if (-not (Test-Path -LiteralPath $targetDir)) {
    Write-Step "没有 target/ 目录，无需清理：$targetDir"
    exit 0
}

$doomed = @()

# 1) debug 缓存：占大头的就是它的 incremental 与 deps。
$doomed += Join-Path $targetDir 'debug'
$doomed += Join-Path $targetDir 'tmp'

# 2) 交叉编译产物：target/ 下除 debug / release / tmp 之外的目录（aarch64-apple-darwin 等）。
foreach ($dir in @(Get-ChildItem -LiteralPath $targetDir -Directory -Force -ErrorAction SilentlyContinue)) {
    if ($dir.Name -in @('debug', 'release', 'tmp')) { continue }
    $doomed += $dir.FullName
}

# 3) 安装包产物：默认留着（可能要发版），-Bundle 才清。
if ($Bundle) {
    $doomed += Join-Path $targetDir 'release\bundle'
    $doomed += Join-Path $targetDir 'release\nsis'
}

# 正在跑的 debug 版会锁住文件，删之前先说明（release 版不受影响，本脚本不动 release）。
$debugExe = Join-Path $targetDir 'debug\kada.exe'
if (Test-Path -LiteralPath $debugExe) {
    $running = @(Get-Process -Name kada -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path -eq $debugExe })
    if ($running.Count -gt 0) {
        Write-Warning "debug 版 Kada 正在运行（PID $($running.Id -join ', ')），先关掉它再清理"
    }
}

$total = 0
$removed = @()
foreach ($path in $doomed) {
    if (-not (Test-Path -LiteralPath $path)) { continue }
    $size = Get-DirSize $path
    $removed += [PSCustomObject]@{ Path = $path; GB = [math]::Round($size / 1GB, 2) }
    $total += $size
}

if ($removed.Count -eq 0) {
    Write-Step '没有可清理的构建产物'
    exit 0
}

Write-Step ("将清理 {0} 项，共 {1} GB" -f $removed.Count, [math]::Round($total / 1GB, 2))
$removed | Format-Table -AutoSize | Out-Host

if ($DryRun) {
    Write-Step '（DryRun：什么都没删）'
    exit 0
}

foreach ($item in $removed) {
    Write-Step "删除 $($item.Path)"
    try {
        Remove-Item -LiteralPath $item.Path -Recurse -Force -ErrorAction Stop
    } catch {
        Write-Warning "删除失败（可能被占用）：$($item.Path) —— $_"
    }
}

$left = Get-DirSize $targetDir
Write-Step ("清理完成，target/ 现为 {0} GB" -f [math]::Round($left / 1GB, 2))
