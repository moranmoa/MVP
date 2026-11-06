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
  [string[]]$Roots = @(),
  [string]$Repo = 'moranmoa/MVP'
)
$ErrorActionPreference = 'Stop'
$version = $Tag.Substring(1)

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
  $ext = if ($p.Name -like 'windows-*') { 'zip' } else { 'tar.gz' }
  $name = "mvs-$version-$($p.Name).$ext"
  gh release download $Tag --repo $Repo --pattern $name --dir $work --clobber
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
gh release view $rel --repo $Repo *> $null
if ($LASTEXITCODE -ne 0) {
  gh release create $rel --repo $Repo --prerelease --title "Update channel: $Channel" `
    --notes "Signed update manifest for the $Channel channel, polled by the app. Not an installer."
}
$manifestCopy = Join-Path $work 'manifest.json'
Copy-Item $Manifest $manifestCopy -Force
$upload = @($manifestCopy) + $files + $Roots
gh release upload $rel --repo $Repo --clobber @upload
"published to $rel : $(($upload | ForEach-Object { Split-Path -Leaf $_ }) -join ', ')"
