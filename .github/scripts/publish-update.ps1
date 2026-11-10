# Publishes an offline-signed update manifest to the fixed-tag release the
# app polls (B8-5, #211):
#   https://github.com/moranmoa/MVP/releases/download/update-<channel>/manifest.json
# Run by the OWNER on the online PC after signing on the offline machine.
# CI never holds a signing key.
#
#   .\.github\scripts\publish-update.ps1 -Tag v0.2.5 -Channel beta -Manifest .\manifest-beta.json [-Roots E:\mvs-keys\root-2.json]
#
# Checks before uploading anything: the signed body's channel and version
# match -Channel and -Tag, and the update zip from the tag's release matches
# the sha256 and size in the manifest.
param(
  [Parameter(Mandatory)][ValidatePattern('^v\d+\.\d+\.\d+$')][string]$Tag,
  [Parameter(Mandatory)][ValidateSet('alpha', 'beta')][string]$Channel,
  [Parameter(Mandatory)][string]$Manifest,
  # The root.json the app ships (or the newest root-N.json), to verify against.
  [Parameter(Mandatory)][string]$Root,
  [string[]]$Roots = @(),
  [string]$Updater = 'updater',
  [string]$Repo = 'moranmoa/MVP'
)
$ErrorActionPreference = 'Stop'

# Runs a native program (gh, the updater) and returns its exit code. Windows
# PowerShell 5.1 turns any stderr line of a native program into a terminating
# error under 'Stop', even when it exits 0 (gh writes "release not found" to
# stderr on a first run): so stderr is allowed here, and the exit code decides.
function Invoke-Native {
  param([Parameter(Mandatory)][string]$Exe, [string[]]$ArgList = @(), [switch]$Quiet)
  $old = $ErrorActionPreference
  $ErrorActionPreference = 'Continue'
  try {
    if ($Quiet) { & $Exe @ArgList *> $null }
    else { & $Exe @ArgList 2>&1 | ForEach-Object { Write-Host "$_" } }
  } finally { $ErrorActionPreference = $old }
  return $LASTEXITCODE
}
$version = $Tag.Substring(1)

# Verify the signature, root chain and expiry with the updater itself, so a
# bad or expired manifest is never published.
if (-not (Get-Command $Updater -ErrorAction SilentlyContinue)) {
  throw "updater CLI not found ('$Updater'): build it from a reviewed commit (cargo build --release -p updater) and pass -Updater <path>"
}
$verifyArgs = @('verify', '--manifest', $Manifest, '--root', $Root)
foreach ($r in $Roots) { $verifyArgs += @('--root-update', $r) }
if ((Invoke-Native $Updater $verifyArgs) -ne 0) { throw "updater verify failed: not publishing $Manifest" }
"ok: updater verify accepted the signature, root chain and expiry"

$signed = Get-Content -Raw $Manifest | ConvertFrom-Json
if (-not $signed.signed -or -not $signed.signatures) { throw "$Manifest is not a signed manifest (no 'signed' / 'signatures')" }
$body = $signed.signed | ConvertFrom-Json
if ($body.channel -ne $Channel) { throw "manifest channel is '$($body.channel)', not '$Channel': sign the manifest-body-$Channel.json" }
if ($body.version -ne $version) { throw "manifest version is '$($body.version)', not '$version'" }
"manifest: channel $($body.channel), version $($body.version), expires $([DateTimeOffset]::FromUnixTimeSeconds($body.expires_at).UtcDateTime.ToString('u'))"

# The payloads the manifest names, taken from the tag's own release, checked.
$work = Join-Path ([IO.Path]::GetTempPath()) "mvs-update-$Channel-$version"
New-Item -ItemType Directory -Force $work | Out-Null
$files = @()
foreach ($p in $body.artifacts.PSObject.Properties) {
  # The updater fetches and unpacks only mvs-<version>-<platform>.zip.
  $name = "mvs-$version-$($p.Name).zip"
  if ((Invoke-Native gh @('release', 'download', $Tag, '--repo', $Repo, '--pattern', $name, '--dir', $work, '--clobber')) -ne 0) {
    throw "could not download $name from release $Tag"
  }
  $path = Join-Path $work $name
  $hash = (Get-FileHash $path -Algorithm SHA256).Hash.ToLower()
  if ($hash -ne $p.Value.sha256) { throw "$name sha256 $hash does not match the manifest" }
  if ((Get-Item $path).Length -ne $p.Value.size) { throw "$name size does not match the manifest" }
  "ok: $name matches the manifest"
  $files += $path
}

# The fixed-tag channel release: created once, a pre-release so it never
# shows as "Latest".
$rel = "update-$Channel"
# A first run has no channel release yet: gh exits non-zero, and we create it.
if ((Invoke-Native gh @('release', 'view', $rel, '--repo', $Repo) -Quiet) -ne 0) {
  "$rel does not exist yet: creating it (pre-release)"
  $notes = "Signed update manifest for the $Channel channel, polled by the app. Not an installer."
  if ((Invoke-Native gh @('release', 'create', $rel, '--repo', $Repo, '--prerelease', '--title', "Update channel: $Channel", '--notes', $notes)) -ne 0) {
    throw "could not create release $rel"
  }
}
$manifestCopy = Join-Path $work 'manifest.json'
Copy-Item $Manifest $manifestCopy -Force
$upload = @($manifestCopy) + $files + $Roots
if ((Invoke-Native gh (@('release', 'upload', $rel, '--repo', $Repo, '--clobber') + $upload)) -ne 0) {
  throw "upload to $rel failed: check the release before telling anyone it is published"
}
"published to $rel : $(($upload | ForEach-Object { Split-Path -Leaf $_ }) -join ', ')"
