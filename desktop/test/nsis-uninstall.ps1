# Windows installer / uninstaller check (B1-3, T12). Installs per user,
# launches once (that registers mvs://), then:
#   0. install again (an update)  -> nothing deleted
#   1. silent uninstall           -> local data kept, mvs:// gone, no installer
#                                    cache, no empty %LOCALAPPDATA%\Programs (#94)
#   2. reinstall, /S /DELETEDATA  -> local data deleted
# and in both cases a marker in Windows Credential Manager is untouched.
#
#   powershell -ExecutionPolicy Bypass -File test\nsis-uninstall.ps1 -Installer "dist\MVS Setup 0.1.0.exe"
param([Parameter(Mandatory)] [string] $Installer)
$ErrorActionPreference = 'Stop'

$data = Join-Path $env:APPDATA 'MVS'
$dir = Join-Path $env:LOCALAPPDATA 'Programs\MVS'
$cred = 'mvs-uninstall-test-marker'
# electron-builder's installer cache, named after package.json "name".
$updater = Join-Path $env:LOCALAPPDATA 'mvs-desktop-updater'
$programs = Join-Path $env:LOCALAPPDATA 'Programs'
# If Programs already holds other software, it must survive; if not, the
# uninstaller must leave no empty Programs folder behind.
$programsExisted = (Test-Path $programs) -and @(Get-ChildItem $programs -Force | Where-Object { $_.Name -ne 'MVS' }).Count -gt 0
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

# An update: the installer again over the installed app. The old uninstaller
# runs in update mode and must delete nothing.
Install
Check (Test-Path "$dir\mvs.exe") 'update: the program is still there'
Check (Test-Path "$data\core\marker.txt") 'update: the local data is kept'

Uninstall @()
Check (-not (Test-Path "$dir\mvs.exe")) 'silent uninstall removes the program'
Check (Test-Path "$data\core\marker.txt") 'silent uninstall keeps the local data'
Check (-not (Test-Path 'HKCU:\Software\Classes\mvs')) 'mvs:// registration removed'
Check ([bool](cmdkey /list:$cred | Select-String $cred)) 'Credential Manager entry untouched'
Check (-not (Test-Path $updater)) 'installer cache (<package>-updater) removed (#94)'
Check (-not (Test-Path $dir)) 'program folder removed'
if ($programsExisted) {
  Check (Test-Path $programs) '%LOCALAPPDATA%\Programs kept: it held other programs before'
} else {
  Check (-not (Test-Path $programs)) 'empty %LOCALAPPDATA%\Programs removed (#94)'
}

Install
Uninstall @('/DELETEDATA')
Check (-not (Test-Path $data)) '/DELETEDATA deletes the local data'
Check ([bool](cmdkey /list:$cred | Select-String $cred)) 'Credential Manager entry still untouched'
Check (-not (Test-Path $updater)) 'installer cache removed again'

cmdkey /delete:$cred | Out-Null
if (Test-Path $data) { Remove-Item $data -Recurse -Force -Confirm:$false }
if ($fail.Count) { exit 1 }
