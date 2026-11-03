# Voice bench on real Windows hardware: N Electron + M native str0m peers in
# one room over loopback, synthetic tone, CPU time of every participant's
# process tree. The signal server runs in Docker (see RESULTS.md).
#
#   powershell -ExecutionPolicy Bypass -File run-windows.ps1 -Electron <VoiceSpike.exe> -Str0m <str0m-peer.exe> -Results <dir>
param(
  [Parameter(Mandatory)] [string] $Electron,
  [Parameter(Mandatory)] [string] $Str0m,
  [Parameter(Mandatory)] [string] $Results,
  [int] $NElectron = 3,
  [int] $NStr0m = 3,
  [int] $Duration = 60,
  [string] $Room = 'win6'
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Results | Out-Null

function Get-Tree([int] $Root) {
  $all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId
  $ids = @($Root)
  do {
    $more = $all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId } | ForEach-Object { $_.ProcessId }
    $ids += $more
  } while ($more)
  $ids
}
function Sum-Cpu([int] $Root) {
  $t = 0.0
  foreach ($id in Get-Tree $Root) {
    $p = Get-Process -Id $id -ErrorAction SilentlyContinue
    if ($p) { $t += $p.TotalProcessorTime.TotalSeconds }
  }
  $t
}

$peers = @()
for ($i = 1; $i -le $NElectron; $i++) {
  $p = Start-Process -FilePath $Electron -PassThru -ArgumentList @("--signal=ws://localhost:8080", "--room=$Room", "--name=e$i", '--src=pulses', '--autojoin=1', "--duration=$Duration")
  $peers += [pscustomobject]@{ name = "e$i"; variant = 'chromium'; proc = $p }
  Start-Sleep -Seconds 2
}
for ($i = 1; $i -le $NStr0m; $i++) {
  $p = Start-Process -FilePath $Str0m -PassThru -WindowStyle Hidden -ArgumentList @('--signal', 'ws://localhost:8080', '--room', $Room, '--name', "s$i", '--src', 'pulses', '--duration', "$($Duration + 5)", '--results', "`"$Results`"")
  $peers += [pscustomobject]@{ name = "s$i"; variant = 'str0m'; proc = $p }
  Start-Sleep -Seconds 2
}

Start-Sleep -Seconds 15
$t0 = Get-Date
$c0 = @{}; foreach ($p in $peers) { $c0[$p.name] = Sum-Cpu $p.proc.Id }
Start-Sleep -Seconds 30
$t1 = Get-Date
$cpu = foreach ($p in $peers) {
  [pscustomobject]@{ name = $p.name; variant = $p.variant; cpuPctOfOneCore = [math]::Round(100 * ((Sum-Cpu $p.proc.Id) - $c0[$p.name]) / ($t1 - $t0).TotalSeconds, 1) }
}
Start-Sleep -Seconds ($Duration - 35)
foreach ($p in $peers | Where-Object variant -eq 'chromium') {
  Get-Tree $p.proc.Id | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
}
$cpu | ConvertTo-Json -Compress | Set-Content -Encoding utf8 (Join-Path $Results 'cpu.json')
$cpu | Format-Table -AutoSize
