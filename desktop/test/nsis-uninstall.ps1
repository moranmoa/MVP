# Windows installer / uninstaller check (B1-3, T12). Installs per user,
# launches once (that registers mvs://), then:
#   1. silent uninstall           -> local data kept, mvs:// gone
#   2. reinstall, /S /DELETEDATA  -> local data deleted
# and in both cases a marker in Windows Credential Manager is untouched.
#
#   powershell -ExecutionPolicy Bypass -File test\nsis-uninstall.ps1 -Installer "dist\MVS Setup 0.1.0.exe"
param([Parameter(Mandatory)] [string] $Installer)
$ErrorActionPreference = 'Stop'

$data = Join-Path $env:APPDATA 'MVS'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\MVS'
$cred = 'mvs-uninstall-test-marker'
$fail = @()
function Check([bool] $ok, [string] $what) { if ($ok) { "ok   $what" } else { "FAIL $what"; $script:fail += $what } }

function Install {
  Start-Process $Installer -ArgumentList '/S', '/currentuser' -Wait
  if (-not (Test-Path "$dir\mvs.exe")) { throw "not installed to $dir" }
}
function RunOnce {
  $p = Start-Process "$dir\mvs.exe" -PassThru
  Start-Sleep -Seconds 8
  Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -like "$dir*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 2
}
function Uninstall([string[]] $extra) {
  $u = Get-ChildItem $dir -Filter 'Uninstall*.exe' | Select-Object -First 1 -ExpandProperty FullName
  Start-Process $u -ArgumentList (@('/S', '/currentuser') + $extra) -Wait
  Start-Sleep -Seconds 3
}

# A stand-in for the identity entry the core keeps in Credential Manager.
cmdkey /generic:$cred /user:test /pass:not-a-secret | Out-Null

Install
RunOnce
Check (Test-Path 'HKCU:\Software\Classes\mvs') 'mvs:// registered for this user after the first run'
New-Item -ItemType Directory -Force "$data\core" | Out-Null
Set-Content "$data\core\marker.txt" 'keep me'
Check (-not (Test-Path 'HKLM:\Software\Classes\mvs')) 'nothing registered machine-wide'

Uninstall @()
Check (-not (Test-Path "$dir\mvs.exe")) 'silent uninstall removes the program'
Check (Test-Path "$data\core\marker.txt") 'silent uninstall keeps the local data'
Check (-not (Test-Path 'HKCU:\Software\Classes\mvs')) 'mvs:// registration removed'
Check ([bool](cmdkey /list:$cred | Select-String $cred)) 'Credential Manager entry untouched'

Install
Uninstall @('/DELETEDATA')
Check (-not (Test-Path $data)) '/DELETEDATA deletes the local data'
Check ([bool](cmdkey /list:$cred | Select-String $cred)) 'Credential Manager entry still untouched'

cmdkey /delete:$cred | Out-Null
if (Test-Path $data) { Remove-Item $data -Recurse -Force -Confirm:$false }
if ($fail.Count) { exit 1 }
