# One command for the OWNER to sign and publish a release to installed apps:
# steps 3-6 of docs/release/RELEASING.md. Run locally, never in CI: it is the
# only place the update root key is used.
#
#   .\.github\scripts\publish-release.ps1 -Tag v0.3.1 [-Channel beta|alpha] [-Key <path>] [-WhatIf]
#
# The key comes from -Key or $env:MVS_ROOT_KEY, on an encrypted removable
# drive that is connected only while signing (core/updater/SECURITY.md). This
# script never prints, copies or uploads it, and refuses a key inside the
# repository or in a synced folder.
#
# 1. Downloads the tag's draft or pre-release and checks every file against
#    SHA256SUMS, and the channel's manifest body against both.
# 2. Restamps the body when it is older than -RestampAfterHours.
# 3. Builds `updater` from the tag's own source (git archive of the tag, which
#    must be on origin/main). Never a downloaded binary.
# 4. Shows a summary and asks you to type the version to go on.
# 5. Only now: asks you to connect the key drive, checks the key, signs, and
#    runs `updater verify` against the tag's core/updater/trust/root.json,
#    the payloads included. Then asks you to remove the drive.
# 6. Publishes the release (--draft=false --prerelease), then runs
#    publish-update.ps1 for the channel.
# Any failure stops it before anything is signed or uploaded.
#
# -WhatIf does all of the checks and the build, then stops before signing
# and uploading (no key needed, no confirmation asked).
param(
  [Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+$')][string]$Tag,
  [ValidateSet('beta', 'alpha')][string]$Channel = 'beta',
  [string]$Key,
  # Root rotations (root-2.json, ...), oldest first; passed to verify and
  # uploaded by publish-update.ps1. Read while the key drive is connected.
  [string[]]$Roots = @(),
  [int]$RestampAfterHours = 48,
  [switch]$WhatIf,
  [string]$Repo = 'moranmoa/MVP'
)
# Read the key's path once, then drop it from the environment before any
# native command runs: git, gh, cargo (build scripts) and node never see it.
if (-not $Key) { $Key = $env:MVS_ROOT_KEY }
Remove-Item Env:\MVS_ROOT_KEY -ErrorAction SilentlyContinue
$ErrorActionPreference = 'Stop'
# Hebrew notes in the summary.
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$version = $Tag.Substring(1)

function Fail([string]$why) {
  Write-Host "STOPPED: $why" -ForegroundColor Red
  Write-Host 'No step after this one ran.' -ForegroundColor Red
  exit 1
}
function Step([string]$what) { Write-Host "`n== $what" -ForegroundColor Cyan }
function Ok([string]$what) { Write-Host "ok: $what" }
# A native command that must succeed.
function Run([string]$what, [scriptblock]$cmd) {
  & $cmd
  if ($LASTEXITCODE -ne 0) { Fail "$what (exit code $LASTEXITCODE)" }
}

# --- 0. Preconditions -------------------------------------------------------
Step 'Checks before starting'
foreach ($tool in 'git', 'gh', 'cargo', 'node', 'tar') {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Fail "'$tool' is not installed or not on PATH" }
}
if ($env:CI -or $env:GITHUB_ACTIONS) { Fail 'this script is for the owner''s own PC, never CI' }
$repoRoot = (git -C $PSScriptRoot rev-parse --show-toplevel)
if ($LASTEXITCODE -ne 0 -or -not $repoRoot) { Fail 'run it from a checkout of the repository' }
$repoRoot = [IO.Path]::GetFullPath($repoRoot)

Run 'gh is not logged in (gh auth login)' { gh auth status *> $null }
# Only that a key is named; the drive is connected later, for signing only.
if (-not $WhatIf -and -not $Key) { Fail 'no signing key: pass -Key <path> or set $env:MVS_ROOT_KEY' }

$work = Join-Path ([IO.Path]::GetTempPath()) ("mvs-release-$version-" + [Guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory $work | Out-Null
$dl = Join-Path $work 'release'
$src = Join-Path $work 'src'
New-Item -ItemType Directory $dl, $src | Out-Null
Ok "working folder $work"

# --- 1. The tag's source ----------------------------------------------------
Step "Source of $Tag"
Run "git fetch of $Tag and main from origin (is the tag pushed?)" { git -C $repoRoot fetch --quiet origin main "refs/tags/${Tag}:refs/tags/$Tag" }
$commit = (git -C $repoRoot rev-parse --verify --quiet "$Tag^{commit}")
if ($LASTEXITCODE -ne 0 -or -not $commit) { Fail "tag $Tag not found" }
git -C $repoRoot merge-base --is-ancestor $commit origin/main
if ($LASTEXITCODE -ne 0) { Fail "$Tag ($commit) is not on origin/main: only a reviewed commit is released" }
$tarball = Join-Path $work 'src.tar'
Run 'git archive of the tag' { git -C $repoRoot archive --format=tar -o $tarball $commit core .github/scripts }
Run 'unpacking the tag''s source' { tar -xf $tarball -C $src }
Ok "$Tag is $commit, on origin/main"

# --- 2. Release files and checksums -----------------------------------------
Step "Release $Tag"
$relJson = (gh release view $Tag --repo $Repo --json isDraft,isPrerelease,tagName)
if ($LASTEXITCODE -ne 0) { Fail "no release for $Tag on $Repo" }
$rel = $relJson | ConvertFrom-Json
if (-not $rel.isDraft -and -not $rel.isPrerelease) { Fail "$Tag is already published as a full release; this script only handles drafts and pre-releases" }
Run 'downloading the release files' { gh release download $Tag --repo $Repo --dir $dl }

$sumsFile = Join-Path $dl 'SHA256SUMS'
if (-not (Test-Path $sumsFile)) { Fail 'the release has no SHA256SUMS' }
$sums = @{}
foreach ($line in Get-Content $sumsFile) {
  if (-not $line.Trim()) { continue }
  if ($line -notmatch '^([0-9a-f]{64}) [ *]?(\S+)$') { Fail "bad SHA256SUMS line: $line" }
  $sum = $Matches[1]; $file = $Matches[2]
  if ($file -match '[\\/]') { Fail "SHA256SUMS names a path, not a file: $file" }
  $path = Join-Path $dl $file
  if (-not (Test-Path -PathType Leaf $path)) { Fail "$file is in SHA256SUMS but not in the release" }
  if ((Get-FileHash $path -Algorithm SHA256).Hash.ToLower() -ne $sum) { Fail "$file does not match SHA256SUMS" }
  $sums[$file] = $sum
}
Ok "$($sums.Count) files match SHA256SUMS"

# --- 3. The channel's manifest body -----------------------------------------
Step "Manifest body for $Channel"
$bodyFile = Join-Path $dl "manifest-body-$Channel.json"
if (-not (Test-Path $bodyFile)) { Fail "the release has no manifest-body-$Channel.json" }
$body = Get-Content -Raw -Encoding UTF8 $bodyFile | ConvertFrom-Json
if ($body.version -ne $version) { Fail "body version is '$($body.version)', not $version" }
if ($body.channel -ne $Channel) { Fail "body channel is '$($body.channel)', not $Channel" }
$platforms = @($body.artifacts.PSObject.Properties)
if ($platforms.Count -eq 0) { Fail 'the body lists no artifacts' }
$payloads = @()
foreach ($p in $platforms) {
  # The updater fetches only mvs-<version>-<platform>.zip (publish-update.ps1).
  $name = "mvs-$version-$($p.Name).zip"
  if (-not $sums.ContainsKey($name)) { Fail "$name is not in SHA256SUMS" }
  if ($p.Value.sha256 -ne $sums[$name]) { Fail "the body's sha256 for $name differs from SHA256SUMS" }
  if ((Get-Item (Join-Path $dl $name)).Length -ne $p.Value.size) { Fail "the body's size for $name differs from the file" }
  $payloads += [pscustomobject]@{ Platform = $p.Name; File = (Join-Path $dl $name); Name = $name; Sha256 = $p.Value.sha256; Size = $p.Value.size }
}
Ok "the body's artifacts match SHA256SUMS and the files"

$now = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
$restamped = $false
if ($now - [int64]$body.issued_at -gt $RestampAfterHours * 3600) {
  Run 'restamping the body' { node (Join-Path $src '.github/scripts/manifest-body.mjs') --restamp $bodyFile --days 14 }
  $body = Get-Content -Raw -Encoding UTF8 $bodyFile | ConvertFrom-Json
  $restamped = $true
  Ok 'body restamped: valid 14 days from now (hashes untouched)'
}
if ([int64]$body.expires_at -le $now) { Fail 'the body has already expired' }

# --- 4. The updater, built from the tag -------------------------------------
Step "Building updater from $Tag (not a downloaded binary)"
$target = Join-Path $work 'target'
Run 'cargo build of updater' { cargo build --release --locked -p updater --manifest-path (Join-Path $src 'core/Cargo.toml') --target-dir $target }
$exeName = 'updater'
if ($env:OS -eq 'Windows_NT') { $exeName = 'updater.exe' }
$updater = Join-Path $target "release/$exeName"
if (-not (Test-Path $updater)) { Fail "the build produced no $exeName" }
$root = Join-Path $src 'core/updater/trust/root.json'
if (-not (Test-Path $root)) { Fail "$Tag has no core/updater/trust/root.json" }
Ok "updater built; root.json from $Tag"

# --- 5. Summary and the one confirmation ------------------------------------
Step 'Summary'
$utc = { param($t) [DateTimeOffset]::FromUnixTimeSeconds([int64]$t).UtcDateTime.ToString('yyyy-MM-dd HH:mm') + ' UTC' }
$state = 'pre-release'
if ($rel.isDraft) { $state = 'draft' }
Write-Host "  version   $version   ($Tag, $state, commit $($commit.Substring(0, 10)))"
Write-Host "  channel   $Channel  -> update-$Channel"
Write-Host "  issued    $(& $utc $body.issued_at)$(if ($restamped) { '  (restamped now)' })"
Write-Host "  expires   $(& $utc $body.expires_at)"
foreach ($p in $payloads) { Write-Host ("  {0,-16} {1}  {2} bytes  {3}" -f $p.Platform, $p.Sha256, $p.Size, $p.Name) }
if ($body.revoked -and @($body.revoked).Count) { Write-Host "  revokes   $(@($body.revoked) -join ', ')" -ForegroundColor Yellow }
if ($body.notes) {
  foreach ($n in $body.notes.PSObject.Properties) {
    $lines = @(($n.Value -split "`n") | Where-Object { $_.Trim() } | Select-Object -First 3)
    Write-Host "  notes [$($n.Name)]"
    foreach ($l in $lines) { if ($l.Length -gt 110) { $l = $l.Substring(0, 110) + '...' }; Write-Host "    | $l" }
  }
} else { Write-Host '  notes     (none)' -ForegroundColor Yellow }
if ($Roots.Count) { Write-Host "  rotations $(($Roots | ForEach-Object { Split-Path -Leaf $_ }) -join ', ')" }

if ($WhatIf) {
  Step 'WhatIf: stopping before signing and uploading'
  Write-Host "  would ask you to connect the key drive, check the key, and sign manifest-body-$Channel.json"
  Write-Host "  would run: updater verify --root <tag>/core/updater/trust/root.json, each payload included"
  if ($rel.isDraft) { Write-Host "  would run: gh release edit $Tag --draft=false --prerelease" }
  Write-Host "  would run: publish-update.ps1 -Tag $Tag -Channel $Channel"
  Write-Host "  checked files are in $work"
  exit 0
}

$answer = Read-Host "`nType the version ($version) to sign and publish it to $Channel, anything else to stop"
if ($answer.Trim() -ne $version) { Fail 'not confirmed' }

# --- 6. Key drive, sign and verify ------------------------------------------
# Everything that needs the network or runs third-party code is done. The
# key's path is never printed: messages say "the key" only.
Step 'Key drive'
if (-not $Key) { Fail 'no signing key: pass -Key <path> or set $env:MVS_ROOT_KEY' }
[void](Read-Host 'Connect the key drive now, then press Enter')
if (-not (Test-Path -PathType Leaf -LiteralPath $Key)) { Fail 'the signing key (-Key / MVS_ROOT_KEY) is not a file that exists: is the drive connected?' }
$keyPath = (Resolve-Path -LiteralPath $Key).ProviderPath
$sep = [IO.Path]::DirectorySeparatorChar
if ($keyPath.StartsWith($repoRoot + $sep, [StringComparison]::OrdinalIgnoreCase)) {
  Fail 'the signing key is inside the repository checkout; keep it on an encrypted removable drive'
}
# Synced folders upload the key: OneDrive (any of its variables), Dropbox,
# Google Drive.
$synced = @($env:OneDrive, $env:OneDriveConsumer, $env:OneDriveCommercial) | Where-Object { $_ }
foreach ($s in $synced) {
  if ($keyPath.StartsWith([IO.Path]::GetFullPath($s).TrimEnd($sep) + $sep, [StringComparison]::OrdinalIgnoreCase)) {
    Fail 'the signing key is in a OneDrive folder; keep it on an encrypted removable drive'
  }
}
if ($keyPath -match '(?i)[\\/](OneDrive[^\\/]*|Dropbox|Google Drive|GoogleDrive|My Drive)[\\/]') {
  Fail 'the signing key is in a synced folder (OneDrive, Dropbox or Google Drive); keep it on an encrypted removable drive'
}
$keyRoot = [IO.Path]::GetPathRoot($keyPath)
$driveType = $null
try { $driveType = ([IO.DriveInfo]::new($keyRoot)).DriveType } catch { }
if ("$driveType" -ne 'Removable') {
  Write-Host "warning: the key is not on a removable drive ($(if ($driveType) { $driveType } else { 'unknown type' })). SECURITY.md wants an encrypted removable drive, connected only while signing." -ForegroundColor Yellow
}
Ok 'signing key found (path not shown)'
# Root rotations are public; copy them now so the drive can go before the upload.
$rotations = @()
foreach ($r in $Roots) {
  if (-not (Test-Path -PathType Leaf -LiteralPath $r)) { Fail "root rotation file not found: $r" }
  $copy = Join-Path $work (Split-Path -Leaf $r)
  Copy-Item -LiteralPath $r $copy
  $rotations += $copy
}
$Roots = $rotations

Step 'Signing'
$manifest = Join-Path $work "manifest-$Channel.json"
$prev = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$out = & $updater sign-manifest --key $keyPath --body $bodyFile --out $manifest 2>&1 | Out-String
$code = $LASTEXITCODE
$ErrorActionPreference = $prev
# The updater names the key file in its errors; never echo the path.
Write-Host ($out.Replace($keyPath, '<key>'))
if ($code -ne 0) { Fail 'updater sign-manifest failed' }

Step 'Verifying'
foreach ($p in $payloads) {
  $verifyArgs = @('verify', '--manifest', $manifest, '--root', $root, '--artifact', $p.File, '--platform', $p.Platform)
  foreach ($r in $Roots) { $verifyArgs += @('--root-update', $r) }
  Run "updater verify ($($p.Platform))" { & $updater @verifyArgs }
}
Ok 'signature, root chain, expiry and payload hashes verified'
$keyPath = $null
$Key = $null
[void](Read-Host "`nRemove the key drive now, then press Enter to upload")

# --- 7. Publish -------------------------------------------------------------
Step 'Publishing'
if ($rel.isDraft) {
  Run 'publishing the release' { gh release edit $Tag --repo $Repo --draft=false --prerelease }
  Ok "$Tag published as a pre-release"
}
$publishArgs = @{ Tag = $Tag; Channel = $Channel; Manifest = $manifest; Root = $root; Updater = $updater; Repo = $Repo }
if ($Roots.Count) { $publishArgs.Roots = $Roots }
try { & (Join-Path $PSScriptRoot 'publish-update.ps1') @publishArgs }
catch { Fail "publish-update.ps1: $_ (the release itself may already be published; re-run to retry the channel upload)" }
Write-Host "`nDone: $version is on update-$Channel. Installed apps on $Channel pick it up at their next check (6 h ± 30 min, or on restart)." -ForegroundColor Green
Write-Host "Signed manifest kept at $manifest"
