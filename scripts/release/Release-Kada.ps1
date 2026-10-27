#requires -Version 5.1
<#
Kada one-key release (bump -> commit -> tag -> push -> wait CI -> publish -> sync ihomy).

Wire it as a Kada shortcut:
  action type : Script
  path        : <repo>\scripts\release\Release-Kada.ps1
  interpreter : powershell.exe
  hide_window : false   (dialog + progress live in the console window)
  show_output : true
  Settings -> action timeout must be 0 (unlimited): a release takes ~40 min.

Env:
  GITHUB_TOKEN  fine-grained PAT, Contents: Read and write (+ Actions: Read).
                With it, publishing the draft release is automatic.
                Without it, the script opens the browser and waits for you to click "Publish release".

Args: -Version 1.2.2   -DryRun   -NoChecks   -NoPause
#>
param(
  [string]$Version,
  [switch]$DryRun,
  [switch]$NoChecks,
  [switch]$NoPause
)

$ErrorActionPreference = "Stop"
$Repo = "chillyice/Kada"
$SshHost = "root@ihomy.top"
$SshPort = "19068"
$LatestUrl = "https://ihomy.top/files/kada/updates/latest.json"

function Fail($msg) { Write-Host "`n[release] FAILED: $msg" -ForegroundColor Red; if (-not $NoPause) { Read-Host "Press Enter to close" }; exit 1 }
function Step($msg) { Write-Host "`n== $msg" -ForegroundColor Cyan }
function Ok($msg) { Write-Host "   $msg" -ForegroundColor DarkGray }

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $Root
Write-Host "repo: $Root"

if (-not $Version) {
  Add-Type -AssemblyName Microsoft.VisualBasic
  $Version = [Microsoft.VisualBasic.Interaction]::InputBox(
    "New version (semver, e.g. 1.2.2).`nBumps 5 files, commits, tags v<version> and pushes.",
    "Release Kada",
    ""
  )
}
$Version = ($Version -as [string]).Trim().TrimStart("v")
if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "version must look like 1.2.2 (got '$Version')" }

$conf = "src-tauri/tauri.conf.json"
$current = [regex]::Match((Get-Content $conf -Raw), '"version":\s*"([^"]+)"').Groups[1].Value
if ($Version -eq $current) { Fail "version $Version is already the current version" }
Write-Host "version: $current -> $Version"

Step "preflight"
$dirty = git status --porcelain --untracked-files=no
if ($dirty) {
  $dirty | ForEach-Object { Ok $_ }
  if ($DryRun) { Ok "tracked changes present (real run would refuse)" } else { Fail "tracked changes are not committed; commit or stash them first" }
} else {
  Ok "working tree clean (untracked files ignored)"
}

$files = @("src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock", "ui/package.json", "ui/package-lock.json")
foreach ($f in $files) { if (-not (Test-Path $f)) { Fail "missing $f" } }

if ($DryRun) {
  Write-Host "`n[dry-run] would bump, commit, tag v$Version, push, wait for CI, publish, sync ihomy."
  if (-not $NoPause) { Read-Host "Press Enter to close" }
  exit 0
}

Step "bump versions"
function Set-Text($path, $old, $new, $label) {
  $text = [IO.File]::ReadAllText((Join-Path $Root $path))
  if (-not $text.Contains($old)) { Fail "${label}: pattern not found in $path" }
  [IO.File]::WriteAllText((Join-Path $Root $path), $text.Replace($old, $new), (New-Object Text.UTF8Encoding($false)))
}
Set-Text "src-tauri/tauri.conf.json" ('"version": "' + $current + '"') ('"version": "' + $Version + '"') "tauri.conf"
Set-Text "src-tauri/Cargo.toml" ('version = "' + $current + '"') ('version = "' + $Version + '"') "Cargo.toml"
Set-Text "ui/package.json" ('"version": "' + $current + '"') ('"version": "' + $Version + '"') "package.json"

$lock = [IO.File]::ReadAllText((Join-Path $Root "ui/package-lock.json"))
$lock = $lock.Replace(('"version": "' + $current + '"'), ('"version": "' + $Version + '"'))
[IO.File]::WriteAllText((Join-Path $Root "ui/package-lock.json"), $lock, (New-Object Text.UTF8Encoding($false)))

$cargoLock = [IO.File]::ReadAllText((Join-Path $Root "Cargo.lock"))
$at = $cargoLock.IndexOf('name = "kada"')
if ($at -lt 0) { Fail "kada package not found in Cargo.lock" }
$verAt = $cargoLock.IndexOf('version = "' + $current + '"', $at)
if ($verAt -lt 0) { Fail "kada version not found in Cargo.lock" }
$cargoLock = $cargoLock.Substring(0, $verAt) + 'version = "' + $Version + '"' + $cargoLock.Substring($verAt + ('version = "' + $current + '"').Length)
[IO.File]::WriteAllText((Join-Path $Root "Cargo.lock"), $cargoLock, (New-Object Text.UTF8Encoding($false)))
Ok "5 files bumped"

if (-not $NoChecks) {
  Step "checks"
  foreach ($cmd in @(@("node", @("scripts/check-docs.mjs")), @("node", @("scripts/check-i18n.mjs")))) {
    & $cmd[0] @($cmd[1]) | Out-Host
    if ($LASTEXITCODE -ne 0) { Fail ("check failed: " + ($cmd[1] -join " ")) }
  }
  Ok "docs / i18n ok"
}

Step "commit + tag + push"
git add -- $files
if ($LASTEXITCODE -ne 0) { Fail "git add failed" }
git commit -m "chore: release $Version"
if ($LASTEXITCODE -ne 0) { Fail "git commit failed" }
git tag -a "v$Version" -m "v$Version"
if ($LASTEXITCODE -ne 0) { Fail "git tag failed (does v$Version already exist?)" }
git push origin HEAD
if ($LASTEXITCODE -ne 0) { Fail "git push failed" }
git push origin "v$Version"
if ($LASTEXITCODE -ne 0) { Fail "tag push failed; delete the local tag before retrying" }
Ok "pushed v$Version (Release workflow starts now)"

$headers = @{ "User-Agent" = "kada-release"; "Accept" = "application/vnd.github+json" }
$token = $env:GITHUB_TOKEN
if ($token) { $headers["Authorization"] = "Bearer $token" }

function Get-Json($url) {
  try { return Invoke-RestMethod -Headers $headers -Uri $url -TimeoutSec 30 -ErrorAction Stop } catch { return $null }
}

if ($token) {
  Step "wait for Release workflow (~40 min)"
  $run = $null
  for ($i = 0; $i -lt 120; $i++) {
    $runs = Get-Json "https://api.github.com/repos/$Repo/actions/runs?head_branch=v$Version&per_page=10"
    if ($runs) { $run = $runs.workflow_runs | Where-Object { $_.name -like "Release*" } | Select-Object -First 1 }
    if ($run -and $run.status -eq "completed") { break }
    if ($i % 4 -eq 0) { Ok ($(if ($run) { "status: " + $run.status } else { "waiting for run to appear..." })) }
    Start-Sleep -Seconds 30
  }
  if (-not $run -or $run.status -ne "completed") { Fail "Release workflow did not complete in 60 min: https://github.com/$Repo/actions" }
  if ($run.conclusion -ne "success") { Fail ("Release workflow " + $run.conclusion + ": " + $run.html_url) }
  Ok "workflow success"

  Step "publish draft release"
  $rel = $null
  for ($i = 0; $i -lt 20; $i++) {
    $rel = Get-Json "https://api.github.com/repos/$Repo/releases/tags/v$Version"
    if ($rel) { break }
    Start-Sleep -Seconds 15
  }
  if (-not $rel) { Fail "draft release v$Version not found" }
  if ($rel.draft) {
    $body = @{ draft = $false } | ConvertTo-Json
    Invoke-RestMethod -Method Patch -Headers $headers -ContentType "application/json" -Body $body -Uri ("https://api.github.com/repos/$Repo/releases/" + $rel.id) -TimeoutSec 30 | Out-Null
  }
  Ok "published https://github.com/$Repo/releases/tag/v$Version"
} else {
  Step "publish the draft (no GITHUB_TOKEN: this one step is manual)"
  Start-Process ("https://github.com/$Repo/releases")
  Write-Host "   1) wait for the green Release workflow (Actions tab)" -ForegroundColor Yellow
  Write-Host "   2) on the v$Version draft: Publish release" -ForegroundColor Yellow
  Write-Host "   this script keeps polling https://github.com/$Repo/releases/latest ..." -ForegroundColor Yellow
  $published = $false
  for ($i = 0; $i -lt 240; $i++) {
    try {
      $resp = Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -TimeoutSec 20 -ErrorAction Stop
    } catch {
      $loc = $_.Exception.Response.Headers["Location"]
      if ($loc -and ($loc -join "") -match ("v" + [regex]::Escape($Version) + "$")) { $published = $true; break }
    }
    Start-Sleep -Seconds 30
  }
  if (-not $published) { Fail "release was not published within 120 min" }
  Ok "release v$Version is live"
}

Step "sync ihomy mirror"
ssh -p $SshPort -o BatchMode=yes -o ConnectTimeout=15 $SshHost "systemctl start kada-mirror.service"
if ($LASTEXITCODE -ne 0) { Fail "ihomy mirror failed; run manually: ssh -p $SshPort $SshHost systemctl start kada-mirror.service" }
$latest = Get-Json $LatestUrl
if (-not $latest -or $latest.version -ne $Version) { Fail ("ihomy latest.json is '" + $(if ($latest) { $latest.version } else { "unreachable" }) + "', expected $Version") }
Ok "ihomy latest.json = $($latest.version)"

Write-Host "`n[release] DONE: v$Version published and mirrored." -ForegroundColor Green
Write-Host "Installed clients see it on next launch (8 s) or within 24 h; auto_update is off by default."
if (-not $NoPause) { Read-Host "Press Enter to close" }
