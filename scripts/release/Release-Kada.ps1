#requires -Version 5.1
<#
Kada one-key release (bump -> commit -> tag -> push -> wait CI -> publish -> sync ihomy).

Wire it as a Kada shortcut (two actions):
  action 1 : Ask      variable release_version, prompt "new version"
  action 2 : Command  shell = powershell,
             & '<repo>\scripts\release\Release-Kada.ps1' -Version '{release_version}'
  show_output : false   (the script owns its console window)
  Settings -> action timeout must be 0 (unlimited): a release takes ~40 min.

Env:
  GITHUB_TOKEN  fine-grained PAT, Contents: Read and write (+ Actions: Read).
                Required: without it the script refuses to bump/commit/push,
                because it could never publish the draft (use -Manual to do
                that by hand).

Every run writes a transcript to scripts\release\logs\release-<stamp>.log.
When a release looks stuck, read that file first -- it shows the last step.

Args:
  -Version 1.2.2   target version (prompts when omitted)
  -Resume          finish an existing release: skip bump/commit/tag/push and go
                   straight to "wait for CI -> publish the draft -> sync ihomy"
  -Manual          no GITHUB_TOKEN: wait for CI, then open the Releases page and
                   poll until you click "Publish release"
  -DryRun          preflight + plan only
  -NoChecks        skip check-docs / check-i18n
  -NoPause         do not wait for Enter at the end
#>
param(
  [string]$Version,
  [switch]$Resume,
  [switch]$Manual,
  [switch]$DryRun,
  [switch]$NoChecks,
  [switch]$NoPause
)

$ErrorActionPreference = "Stop"
$Repo = "chillyice/Kada"
$SshHost = "root@ihomy.top"
$SshPort = "19068"
$LatestUrl = "https://ihomy.top/files/kada/updates/latest.json"
$CiTimeoutMin = 60
$script:Token = $env:GITHUB_TOKEN
$script:Logging = $false

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$logFile = $null

function Step($msg) { Write-Host "`n== $msg" -ForegroundColor Cyan }
function Ok($msg) { Write-Host "   $msg" -ForegroundColor DarkGray }
function Warn($msg) { Write-Host "   $msg" -ForegroundColor Yellow }
function Fail($msg) { throw ("[release] FAILED: " + $msg) }

function New-Headers {
  $h = @{ "User-Agent" = "kada-release"; "Accept" = "application/vnd.github+json" }
  if ($script:Token) { $h["Authorization"] = "Bearer " + $script:Token }
  return $h
}

function Get-Json($url, [int]$tries = 3) {
  for ($n = 1; $n -le $tries; $n++) {
    try { return Invoke-RestMethod -Headers (New-Headers) -Uri $url -TimeoutSec 30 -ErrorAction Stop }
    catch {
      Warn ("api {0}/{1}: {2}" -f $n, $tries, $_.Exception.Message)
      if ($n -lt $tries) { Start-Sleep -Seconds (3 * $n) }
    }
  }
  return $null
}

function Get-Release($tag) {
  # releases/tags/{tag} answers 404 for drafts -- always look the tag up in the list
  $raw = Get-Json ("https://api.github.com/repos/$Repo/releases?per_page=50")
  if (-not $raw) { return $null }
  # Invoke-RestMethod hands a JSON array back either flat or wrapped one level in
  # the function output; unwrap first, otherwise $_.tag_name is an array and
  # "-eq $tag" matches every release at once.
  $list = New-Object System.Collections.ArrayList
  foreach ($item in @($raw)) {
    if (($item -is [System.Collections.IEnumerable]) -and -not ($item -is [string])) {
      foreach ($sub in $item) { [void]$list.Add($sub) }
    } else {
      [void]$list.Add($item)
    }
  }
  foreach ($r in $list) { if ($r.tag_name -eq $tag) { return $r } }
  return $null
}

function Wait-Workflow($tag, [int]$intervalSec, [int]$timeoutMin) {
  $start = Get-Date
  $tick = 0
  while (((Get-Date) - $start).TotalMinutes -lt $timeoutMin) {
    $runs = Get-Json ("https://api.github.com/repos/$Repo/actions/runs?head_branch=$tag&per_page=10")
    $run = $null
    if ($runs) { $run = $runs.workflow_runs | Where-Object { $_.name -like "Release*" } | Select-Object -First 1 }
    if ($run -and $run.status -eq "completed") { return $run }
    if ($tick % 3 -eq 0) {
      $state = if ($run) { $run.status } else { "not listed yet" }
      Ok ("workflow {0} ({1} min elapsed)" -f $state, [int]((Get-Date) - $start).TotalMinutes)
    }
    $tick++
    Start-Sleep -Seconds $intervalSec
  }
  return $null
}

function Publish-Draft($rel, $tag) {
  # tag_name must be sent along: a PATCH on a draft that omits it re-tags the
  # release with an "untagged-..." placeholder (breaks every asset / updater url).
  $body = @{ draft = $false; tag_name = $tag } | ConvertTo-Json
  for ($n = 1; $n -le 4; $n++) {
    try {
      return Invoke-RestMethod -Method Patch -Headers (New-Headers) -ContentType "application/json" -Body $body -Uri ("https://api.github.com/repos/$Repo/releases/" + $rel.id) -TimeoutSec 60 -ErrorAction Stop
    } catch {
      $detail = $_.Exception.Message
      $code = 0
      $resp = $_.Exception.Response
      if ($resp) {
        try { $code = [int]$resp.StatusCode } catch { $code = 0 }
        try {
          $stream = $resp.GetResponseStream()
          if ($stream) {
            $text = (New-Object System.IO.StreamReader($stream)).ReadToEnd()
            if ($text) { $detail = "$code $text" }
          }
        } catch { }
      }
      Warn ("publish attempt {0}/{1}: {2}" -f $n, 4, $detail)
      if ($code -ge 400 -and $code -lt 500) { return $null }
      if ($n -lt 4) { Start-Sleep -Seconds (5 * $n) }
    }
  }
  return $null
}

function Invoke-Release {
  Set-Location $Root
  Write-Host "repo: $Root"

  $conf = "src-tauri/tauri.conf.json"
  if (-not (Test-Path $conf)) { Fail "missing $conf -- run from the repository" }
  $current = [regex]::Match((Get-Content $conf -Raw), '"version":\s*"([^"]+)"').Groups[1].Value
  if (-not $current) { Fail "cannot read the version out of $conf" }

  if ($Resume) {
    if (-not $Version) { $Version = $current }
  } elseif (-not $Version) {
    Add-Type -AssemblyName Microsoft.VisualBasic
    $Version = [Microsoft.VisualBasic.Interaction]::InputBox(
      "New version (semver, e.g. 1.2.2).`nBumps 5 files, commits, tags v<version> and pushes.",
      "Release Kada",
      ""
    )
  }
  $Version = ($Version -as [string]).Trim().TrimStart("v")
  if ($Version -notmatch '^\d+\.\d+\.\d+$') { Fail "version must look like 1.2.2 (got '$Version')" }
  $tag = "v$Version"
  if ($Resume) {
    Write-Host "resume: $tag (version in $conf is $current)"
  } else {
    if ($Version -eq $current) { Fail "version $Version is already the current version (use -Resume to finish it)" }
    Write-Host "version: $current -> $Version"
  }

  if (-not $script:Token -and -not $Manual) {
    Fail ("GITHUB_TOKEN is not set, so the draft could never be published.`n" +
      "   setx GITHUB_TOKEN 'github_pat_xxx'   then restart Kada (children inherit env only at start)`n" +
      "   or run with -Manual and click Publish release yourself")
  }
  if (-not $script:Token) { Warn "no GITHUB_TOKEN: -Manual mode, you will publish by hand" }

  if ($Resume) {
    Step "resume $tag"
    if (-not (git ls-remote --tags origin "refs/tags/$tag")) { Fail "tag $tag is not on the remote -- nothing was pushed, cannot resume" }
    Ok "tag $tag is on the remote"
  } else {
    Step "preflight"
    $dirty = git status --porcelain --untracked-files=no
    if ($dirty) {
      $dirty | ForEach-Object { Ok $_ }
      if ($DryRun) { Ok "tracked changes present (a real run would refuse)" } else { Fail "tracked changes are not committed; commit or stash them first" }
    } else {
      Ok "working tree clean (untracked files ignored)"
    }
    $files = @("src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "Cargo.lock", "ui/package.json", "ui/package-lock.json")
    foreach ($f in $files) { if (-not (Test-Path $f)) { Fail "missing $f" } }
    if ([version]$Version -lt [version]$current) {
      Warn "target $Version is lower than current $current -- only right if $current was never published"
    }
    if (git tag --list $tag) { Fail "local tag $tag already exists (abandoned attempt?). delete it first: git tag -d $tag" }
    if (git ls-remote --tags origin "refs/tags/$tag") { Fail "tag $tag already exists on the remote; pick a new version" }
  }

  if ($DryRun) {
    if ($Resume) { Write-Host "`n[dry-run] would wait for CI of $tag, publish the draft, sync ihomy." }
    else { Write-Host "`n[dry-run] would bump, commit, tag $tag, push, wait for CI, publish, sync ihomy." }
    return $null
  }

  if (-not $Resume) {
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
    git tag -a $tag -m $tag
    if ($LASTEXITCODE -ne 0) { Fail "git tag failed" }
    git push origin HEAD
    if ($LASTEXITCODE -ne 0) { Fail "git push failed; the local commit/tag are still there -- fix the push, then re-run with -Resume" }
    git push origin $tag
    if ($LASTEXITCODE -ne 0) { Fail "tag push failed; delete the local tag (git tag -d $tag) before retrying" }
    Ok "pushed $tag (Release workflow starts now)"
  }

  Step ("wait for the Release workflow (up to {0} min)" -f $CiTimeoutMin)
  # anonymous polling is rate limited to 60 requests/hour -- slow it down
  $interval = if ($script:Token -and -not $Manual) { 30 } else { 90 }
  $run = Wait-Workflow $tag $interval $CiTimeoutMin
  if (-not $run) { Fail "Release workflow did not finish in $CiTimeoutMin min: https://github.com/$Repo/actions" }
  if ($run.conclusion -ne "success") { Fail ("Release workflow " + $run.conclusion + ": " + $run.html_url) }
  Ok "workflow success"

  if ($script:Token -and -not $Manual) {
    Step "publish draft release"
    $rel = $null
    for ($i = 0; $i -lt 20; $i++) {
      $rel = Get-Release $tag
      if ($rel) { break }
      Start-Sleep -Seconds 15
    }
    if (-not $rel) { Fail "draft release $tag not found: https://github.com/$Repo/releases" }
    if ($rel -isnot [System.Management.Automation.PSCustomObject]) { Fail "unexpected release payload for $tag" }
    if ($rel.draft) {
      Ok ("draft id " + $rel.id + " found, publishing")
      $published = Publish-Draft $rel $tag
      if (-not $published) { Fail "publishing the draft failed (see the log above). Re-run: Release-Kada.ps1 -Resume" }
      Ok ("published " + $published.html_url)
    } else {
      Ok "release $tag was already published"
    }
    $check = Get-Json ("https://api.github.com/repos/$Repo/releases/" + $rel.id)
    if (-not $check) { Fail "cannot re-read the release to verify it" }
    if ($check.draft) { Fail "release $tag is still a draft after the PATCH" }
    if ($check.tag_name -ne $tag) { Fail ("release tag is '" + $check.tag_name + "', expected " + $tag + " -- fix it on the Releases page") }
    $assetCount = @($check.assets).Count
    if ($assetCount -lt 3) { Warn ("only {0} assets on the release -- check the workflow uploads" -f $assetCount) }
    Ok ("verified: published, tag $tag, $assetCount assets")
  } else {
    Step "publish the draft by hand (-Manual)"
    Start-Process ("https://github.com/$Repo/releases")
    Warn "1) the Release workflow is green"
    Warn "2) on the $tag draft: Publish release"
    $published = $false
    $start = Get-Date
    while (((Get-Date) - $start).TotalMinutes -lt 120) {
      try {
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/latest" -MaximumRedirection 0 -TimeoutSec 20 -ErrorAction Stop | Out-Null
      } catch {
        $loc = $_.Exception.Response.Headers["Location"]
        if ($loc -and (($loc -join "") -match ($tag + "$"))) { $published = $true; break }
      }
      Ok ("still waiting for you to publish $tag ({0} min)" -f [int]((Get-Date) - $start).TotalMinutes)
      Start-Sleep -Seconds 30
    }
    if (-not $published) { Fail "release was not published within 120 min -- publish the draft, then run with -Resume" }
    Ok "release $tag is live"
  }

  Step "sync ihomy mirror"
  & ssh -p $SshPort -o BatchMode=yes -o ConnectTimeout=15 $SshHost "systemctl start kada-mirror.service"
  if ($LASTEXITCODE -ne 0) { Fail "ihomy mirror failed; run manually: ssh -p $SshPort $SshHost `"systemctl start kada-mirror.service`"" }

  Step "wait for ihomy latest.json to report $Version"
  $latest = $null
  $start = Get-Date
  while (((Get-Date) - $start).TotalMinutes -lt 15) {
    $latest = Get-Json ($LatestUrl + "?t=" + [DateTime]::UtcNow.Ticks)
    if ($latest -and $latest.version -eq $Version) { break }
    Ok ("ihomy reports " + $(if ($latest) { $latest.version } else { "nothing (unreachable)" }) + ", waiting")
    Start-Sleep -Seconds 20
  }
  if (-not $latest -or $latest.version -ne $Version) {
    Fail ("ihomy latest.json is '" + $(if ($latest) { $latest.version } else { "unreachable" }) + "', expected $Version -- the mirror ran but did not pick the release up")
  }
  Ok "ihomy latest.json = $($latest.version)"

  return $tag
}

$exitCode = 0
try {
  $logDir = Join-Path $Root "scripts\release\logs"
  if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir -Force | Out-Null }
  $logFile = Join-Path $logDir ("release-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".log")
  try { Start-Transcript -Path $logFile -Force | Out-Null; $script:Logging = $true } catch { }
  Write-Host "log: $logFile"

  $doneTag = Invoke-Release

  if ($doneTag) {
    Write-Host ("`n[release] DONE: {0} published and mirrored." -f $doneTag) -ForegroundColor Green
    Write-Host "Installed clients see it on next launch (8 s) or within 24 h; auto_update is off by default."
  } else {
    Write-Host "`n[dry-run] nothing was changed." -ForegroundColor Green
  }
} catch {
  Write-Host ("`n" + $_.Exception.Message) -ForegroundColor Red
  if ($logFile) { Write-Host ("log: " + $logFile) -ForegroundColor Red }
  $exitCode = 1
} finally {
  if ($script:Logging) { try { Stop-Transcript | Out-Null } catch { } }
  if (-not $NoPause) { Read-Host "Press Enter to close" | Out-Null }
}
exit $exitCode
