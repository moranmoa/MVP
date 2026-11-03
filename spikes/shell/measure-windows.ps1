# Footprint on Windows (experiment 6): installs one shell per user with its
# NSIS installer, measures it, and uninstalls it again.
#
#   powershell -ExecutionPolicy Bypass -File measure-windows.ps1 -Installer <setup.exe> -Exe <installed exe name> [-Runs 5]
#
# Prints one JSON line: installer and installed size, start time of every
# run (ms from launch to the first message rows on screen; run 1 is the
# first launch after install), private working set of the whole process
# tree (what Task Manager shows) at idle and after scrolling 10k messages.
param(
  [Parameter(Mandatory)] [string] $Installer,
  [Parameter(Mandatory)] [string] $Exe,
  [int] $Runs = 5
)
$ErrorActionPreference = 'Stop'

function Get-Tree([int] $Root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name
  $ids = @($Root)
  do {
    $more = $all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId }
    $ids += $more
  } while ($more)
  $ids
}

function Get-PrivateMiB([int] $Root) {
  $ids = Get-Tree $Root
  $perf = Get-CimInstance Win32_PerfRawData_PerfProc_Process | Where-Object { $ids -contains [int]$_.IDProcess }
  [math]::Round((($perf | Measure-Object WorkingSetPrivate -Sum).Sum) / 1MB)
}

function Wait-Mark([string] $File, [string] $Label, [int] $Seconds) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while ((Get-Date) -lt $deadline) {
    if (Test-Path $File) {
      $line = Get-Content $File | Where-Object { $_ -like "$Label *" } | Select-Object -First 1
      if ($line) { return [int64]($line.Split(' ')[1]) }
    }
    Start-Sleep -Milliseconds 20
  }
  throw "timeout waiting for $Label"
}

$now = { [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds() }
$installerMiB = [math]::Round((Get-Item $Installer).Length / 1MB, 1)

# Per-user, silent install. Find the exe it created under LOCALAPPDATA.
Start-Process -FilePath $Installer -ArgumentList '/S' -Wait
Start-Sleep -Seconds 2
$exePath = Get-ChildItem -Path $env:LOCALAPPDATA -Filter $Exe -Recurse -ErrorAction SilentlyContinue -Depth 3 | Select-Object -First 1 -ExpandProperty FullName
if (-not $exePath) { throw "installed $Exe not found" }
$installDir = Split-Path $exePath
$installedMiB = [math]::Round(((Get-ChildItem $installDir -Recurse -File | Measure-Object Length -Sum).Sum) / 1MB, 1)

$starts = @(); $idle = 0; $scrolled = 0
for ($run = 1; $run -le $Runs; $run++) {
  $out = Join-Path $env:TEMP "shell-bench-$run.txt"
  Remove-Item $out -ErrorAction SilentlyContinue
  $env:SHELL_BENCH_OUT = $out
  $t0 = & $now
  $p = Start-Process -FilePath $exePath -PassThru
  $starts += (Wait-Mark $out 'rendered' 60) - $t0
  if ($run -eq 1) {
    Start-Sleep -Seconds 10
    $idle = Get-PrivateMiB $p.Id
    [void](Wait-Mark $out 'scrolled' 180)
    Start-Sleep -Seconds 3
    $scrolled = Get-PrivateMiB $p.Id
  }
  Get-Tree $p.Id | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Seconds 3
}
Remove-Item Env:\SHELL_BENCH_OUT

# Uninstall again.
$uninstaller = Get-ChildItem $installDir -Filter 'Uninstall*.exe' | Select-Object -First 1 -ExpandProperty FullName
if ($uninstaller) { Start-Process -FilePath $uninstaller -ArgumentList '/S' -Wait }

[ordered]@{
  shell = $Exe; installerMiB = $installerMiB; installedMiB = $installedMiB; installDir = $installDir
  startMs = $starts; idlePrivateMiB = $idle; scrolled10kPrivateMiB = $scrolled
} | ConvertTo-Json -Compress
