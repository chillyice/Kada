# 构建安卓侧的 Rust 动态库（libkada_android.so）并放进 jniLibs。
#
# 为什么需要这一步：APK 里的 .so 不是 Gradle 编的——判定逻辑在 `crates/kada-android`
# （同一份 kada-core），必须用 cargo-ndk 交叉编译出各 ABI 的 .so，Gradle 只负责打包。
#
# 用法：
#   powershell android\tools\build-rust.ps1                  # 只编模拟器要的 x86_64
#   powershell android\tools\build-rust.ps1 -Abi arm64-v8a   # 真机（arm64）
#   powershell android\tools\build-rust.ps1 -Abi all         # 三个 ABI 全编
#   powershell android\tools\build-rust.ps1 -Profile dev     # 快速 debug 版（体积大、跑得慢）
#
# 前置：rustup target add x86_64-linux-android aarch64-linux-android armv7-linux-androideabi
#       cargo install cargo-ndk；NDK 已装在 %LOCALAPPDATA%\Android\Sdk\ndk\<ver>
param(
    [ValidateSet("x86_64", "arm64-v8a", "armeabi-v7a", "all")]
    [string]$Abi = "x86_64",
    [ValidateSet("release", "dev")]
    [string]$Profile = "release"
)

$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$out = Join-Path $root "android\app\src\main\jniLibs"

# cargo-ndk 自己找 NDK：优先 ANDROID_NDK_HOME / ANDROID_SDK_ROOT，其次 local.properties 里的 sdk.dir。
$localProps = Join-Path $root "android\local.properties"
if (-not $env:ANDROID_NDK_HOME -and -not $env:ANDROID_SDK_ROOT -and (Test-Path $localProps)) {
    $sdkLine = Select-String -Path $localProps -Pattern '^sdk\.dir=(.+)$' | Select-Object -First 1
    if ($sdkLine) {
        $sdk = $sdkLine.Matches[0].Groups[1].Value -replace '\\\\', '\' -replace '\\:', ':'
        $env:ANDROID_SDK_ROOT = $sdk
    }
}

$targets = switch ($Abi) {
    "x86_64" { @("x86_64-linux-android") }
    "arm64-v8a" { @("aarch64-linux-android") }
    "armeabi-v7a" { @("armv7-linux-androideabi") }
    "all" { @("x86_64-linux-android", "aarch64-linux-android", "armv7-linux-androideabi") }
}

$args = @("ndk")
foreach ($t in $targets) { $args += @("-t", $t) }
$args += @("-o", $out, "build", "-p", "kada-android")
if ($Profile -eq "release") { $args += "--release" }

Write-Host "cargo $($args -join ' ')" -ForegroundColor Cyan
Push-Location $root
try {
    # cargo 的进度写 stderr，PS 5.1 在 ErrorActionPreference=Stop 下会当成终止性错误。
    # 临时放宽并合并输出，靠退出码判成败。
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & cargo @args 2>&1 | ForEach-Object { Write-Host $_ }
    } finally {
        $ErrorActionPreference = $prev
    }
    if ($LASTEXITCODE -ne 0) { throw "cargo ndk 失败（$LASTEXITCODE）" }
} finally {
    Pop-Location
}

Get-ChildItem -Path $out -Recurse -Filter "libkada_android.so" |
    ForEach-Object { Write-Host ("[OK] {0}  {1:N0} 字节" -f $_.FullName, $_.Length) -ForegroundColor Green }
