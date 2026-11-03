# T12 install / uninstall check for one Windows package (NSIS, per-user).
#
#   pwsh packaging/install-tests/windows.ps1 -Shell tauri -Installer out\shell-tauri-windows-x64-setup.exe
#
# Takes a snapshot of everything an installer can touch (registry, shortcuts,
# program and data folders, mvs:// handler, autostart), then:
#   1. silent install, compare: exactly the expected entries appear
#   2. first launch: still running after 10 s, data folder created
#   3. install again over the existing install: nothing new appears
#   4. silent uninstall: only the data folder is left, no process left
#   5. reinstall + uninstall again
#   6. install + uninstall as a standard (non-admin) user
# Every snapshot diff is printed, so the log doubles as QA's before/after list
# (INS-01, 04, 05, 08, 10, 11 in qa/TEST_PLAN.md).
param(
  [Parameter(Mandatory)][ValidateSet('tauri', 'electron')][string]$Shell,
  [Parameter(Mandatory)][string]$Installer
)
$ErrorActionPreference = 'Stop'
$Installer = (Resolve-Path $Installer).Path

# What each spike package creates (spikes/RESULTS.md, "Install and uninstall").
# Paths are relative to the environment variable named first.
$expect = @{
  tauri    = @{
    InstallDir = 'LOCALAPPDATA\ShellSpikeTauri'
    Uninstall  = { param($dir) @("$dir\uninstall.exe", '/S') }
    Data       = @('file:LOCALAPPDATA\dev.mvp.shellspike.tauri')
    Installed  = @(
      'file:LOCALAPPDATA\ShellSpikeTauri',
      'reg:HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShellSpikeTauri',
      'file:STARTMENU\ShellSpikeTauri.lnk',
      'file:DESKTOP\ShellSpikeTauri.lnk'
    )
  }
  electron = @{
    InstallDir = 'LOCALAPPDATA\Programs\shell-spike-electron'
    Uninstall  = { param($dir) @("$dir\Uninstall ShellSpikeElectron.exe", '/currentuser', '/S') }
    Data       = @('file:APPDATA\shell-spike-electron')
    Installed  = @(
      'file:LOCALAPPDATA\Programs\shell-spike-electron',
      'reg:HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\169df8e2-50f9-5fea-b18c-9805ad553a53',
      'reg:HKCU\Software\169df8e2-50f9-5fea-b18c-9805ad553a53',
      'file:STARTMENU\ShellSpikeElectron.lnk',
      'file:DESKTOP\ShellSpikeElectron.lnk'
    )
  }
}[$Shell]

$failures = [System.Collections.Generic.List[string]]::new()
function Check($ok, $what) {
  if ($ok) { Write-Host "  ok    $what" } else { Write-Host "  FAIL  $what"; $failures.Add($what) }
}
function Expand($rel) {
  $name, $rest = $rel -split '\\', 2
  $base = switch ($name) {
    'LOCALAPPDATA' { $env:LOCALAPPDATA }
    'APPDATA' { $env:APPDATA }
    default { throw "unknown base $name" }
  }
  if ($rest) { Join-Path $base $rest } else { $base }
}

# One line per thing an installer could add. Folders are listed one level
# deep (plus LOCALAPPDATA\Programs), registry keys by name.
function Snapshot {
  $dirs = @{
    LOCALAPPDATA            = $env:LOCALAPPDATA
    'LOCALAPPDATA\Programs' = Join-Path $env:LOCALAPPDATA 'Programs'
    APPDATA                 = $env:APPDATA
    STARTMENU               = [Environment]::GetFolderPath('Programs')
    COMMONSTARTMENU         = [Environment]::GetFolderPath('CommonPrograms')
    DESKTOP                 = [Environment]::GetFolderPath('Desktop')
    PUBLICDESKTOP           = [Environment]::GetFolderPath('CommonDesktopDirectory')
    PROGRAMFILES            = $env:ProgramFiles
    STARTUP                 = [Environment]::GetFolderPath('Startup')
  }
  $out = foreach ($k in $dirs.Keys) {
    if ($dirs[$k] -and (Test-Path $dirs[$k])) {
      Get-ChildItem -Force $dirs[$k] -ErrorAction SilentlyContinue | ForEach-Object { "file:$k\$($_.Name)" }
    }
  }
  $keys = @(
    'HKCU:\Software',
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall',
    'HKCU:\Software\Classes'
  )
  $out += foreach ($k in $keys) {
    Get-ChildItem $k -ErrorAction SilentlyContinue | ForEach-Object { "reg:$($_.Name -replace '^HKEY_CURRENT_USER','HKCU' -replace '^HKEY_LOCAL_MACHINE','HKLM')" }
  }
  # Autostart values and the mvs:// handler (explicitly, even if absent).
  foreach ($run in 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run', 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run') {
    $p = Get-ItemProperty $run -ErrorAction SilentlyContinue
    if ($p) { $out += $p.PSObject.Properties | Where-Object Name -NotLike 'PS*' | ForEach-Object { "run:$run\$($_.Name)" } }
  }
  if (Test-Path 'Registry::HKEY_CLASSES_ROOT\mvs') { $out += 'proto:mvs' }
  $out | Sort-Object -Unique
}
function Show-Diff($before, $after, $label) {
  $added = @($after | Where-Object { $_ -notin $before })
  $removed = @($before | Where-Object { $_ -notin $after })
  Write-Host "  diff $label"
  $added | ForEach-Object { Write-Host "    + $_" }
  $removed | ForEach-Object { Write-Host "    - $_" }
  , @($added, $removed)
}
function SameSet($a, $b) {
  $a = @($a); $b = @($b)
  ($a.Count -eq $b.Count) -and -not ($a | Where-Object { $_ -notin $b })
}
function Run($exe, $argv) {
  $p = Start-Process -FilePath $exe -ArgumentList $argv -Wait -PassThru
  $p.ExitCode
}
function Install { Run $Installer @('/S') }
function Uninstall {
  $cmd = & $expect.Uninstall (Expand $expect.InstallDir)
  $code = Run $cmd[0] $cmd[1..($cmd.Count - 1)]
  # NSIS uninstallers copy themselves to %TEMP% and return before they finish.
  for ($i = 0; $i -lt 60 -and (Test-Path (Expand $expect.InstallDir)); $i++) { Start-Sleep 1 }
  $code
}
function Leftover {
  $dir = Expand $expect.InstallDir
  @(Get-Process | Where-Object { $_.Path -and $_.Path.StartsWith($dir, 'OrdinalIgnoreCase') })
}

Write-Host "== $Shell : $(Split-Path -Leaf $Installer)"
$clean = Snapshot

Write-Host "1. silent install"
Check ((Install) -eq 0) 'installer exit code 0'
$installed = Snapshot
$added, $removed = Show-Diff $clean $installed 'clean -> installed'
Check (SameSet $added $expect.Installed) 'exactly the expected entries were added'
Check ($removed.Count -eq 0) 'nothing was removed'
Check (-not ($added | Where-Object { $_ -like 'reg:HKLM*' -or $_ -like 'file:PROGRAMFILES*' -or $_ -like 'file:COMMON*' -or $_ -like 'file:PUBLIC*' })) 'per-user only: no HKLM, Program Files or all-users entries'
Check (-not ($added | Where-Object { $_ -like 'run:*' -or $_ -like 'file:STARTUP*' })) 'no autostart'
$exe = Get-ChildItem (Expand $expect.InstallDir) -Filter *.exe | Where-Object Name -NotLike "Uninstall*" | Select-Object -First 1
Check ($null -ne $exe) "program exe found in $($expect.InstallDir): $($exe.Name)"

Write-Host "2. first launch"
$app = Start-Process -FilePath $exe.FullName -PassThru
Start-Sleep 10
Check (-not $app.HasExited) 'app still running after 10 s'
Leftover | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep 2
$launched = Snapshot
$added, $null = Show-Diff $installed $launched 'installed -> after first launch'
Check (SameSet $added $expect.Data) "first launch created only the data folder ($($expect.Data -join ', '))"

Write-Host "3. install over the existing install"
Check ((Install) -eq 0) 'installer exit code 0'
$again = Snapshot
$added, $removed = Show-Diff $launched $again 'installed -> installed again'
Check ($added.Count -eq 0 -and $removed.Count -eq 0) 'no duplicate entries, nothing lost'

Write-Host "4. silent uninstall"
Uninstall | Out-Null
Start-Sleep 2
$gone = Snapshot
$added, $removed = Show-Diff $clean $gone 'clean -> uninstalled'
Check (SameSet $added $expect.Data) 'only the data folder is left'
Check ($removed.Count -eq 0) 'nothing that existed before was removed'
Check ((Leftover).Count -eq 0) 'no process left running'

Write-Host "5. reinstall after uninstall"
Check ((Install) -eq 0) 'installer exit code 0'
$re = Snapshot
$added, $null = Show-Diff $gone $re 'uninstalled -> reinstalled'
Check (SameSet $added $expect.Installed) 'same entries as the first install'
Uninstall | Out-Null

Write-Host "6. standard user (no admin rights)"
$user = 'mvsinstalltest'
$pw = -join ((48..57) + (65..90) + (97..122) | Get-Random -Count 24 | ForEach-Object { [char]$_ }) + '!a1'
$secure = ConvertTo-SecureString $pw -AsPlainText -Force
New-LocalUser -Name $user -Password $secure -AccountNeverExpires | Out-Null
try {
  $cred = [pscredential]::new("$env:COMPUTERNAME\$user", $secure)
  Check (-not (Get-LocalGroupMember Administrators | Where-Object Name -like "*\$user")) 'test user is not an administrator'
  $p = Start-Process -FilePath $Installer -ArgumentList '/S' -Credential $cred -LoadUserProfile -Wait -PassThru -WorkingDirectory $env:TEMP
  Check ($p.ExitCode -eq 0) 'installer exit code 0 as standard user'
  $profileDir = Get-CimInstance Win32_UserProfile | Where-Object { $_.LocalPath -like "*\$user" } | Select-Object -First 1 -ExpandProperty LocalPath
  $userDir = ($expect.InstallDir -replace '^LOCALAPPDATA', "$profileDir\AppData\Local")
  Check (Test-Path (Join-Path $userDir $exe.Name)) "installed into the user's profile: $userDir"
  $cmd = & $expect.Uninstall $userDir
  $p = Start-Process -FilePath $cmd[0] -ArgumentList $cmd[1..($cmd.Count - 1)] -Credential $cred -LoadUserProfile -Wait -PassThru -WorkingDirectory $env:TEMP
  for ($i = 0; $i -lt 60 -and (Test-Path $userDir); $i++) { Start-Sleep 1 }
  Check (-not (Test-Path $userDir)) 'uninstalled as standard user'
} finally {
  Remove-LocalUser -Name $user -ErrorAction SilentlyContinue
}

Write-Host ''
if ($failures.Count) {
  Write-Host "$Shell : $($failures.Count) check(s) failed"
  $failures | ForEach-Object { Write-Host "  - $_" }
  exit 1
}
Write-Host "$Shell : all checks passed"
