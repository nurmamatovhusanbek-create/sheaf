# Windows smoke test of the real executable (CI; also runnable by hand).
#
#   ./tools/smoke/windows.ps1 -Exe target\release\sheaf.exe -Fixture some.pdf
#
# 1. A headless quick action runs through the real worker and writes its output.
# 2. The window opens with a file, stays up, renders thumbnails through a worker,
#    and neither it nor anything it started listens on a TCP port (TEST-SEC).
# 3. The tray helper starts and stays up (no window of its own).
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Fixture
)
$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
$Fixture = (Resolve-Path $Fixture).Path

Write-Host "== headless: sheaf run compress"
$out = & $Exe run compress $Fixture
if ($LASTEXITCODE -ne 0) { throw "run compress exited with $LASTEXITCODE" }
if (-not $out -or -not (Test-Path ($out | Select-Object -First 1))) { throw "no output file was written: $out" }
Write-Host "ok   $out"

function Family([int]$RootId) {
  $family = [System.Collections.Generic.HashSet[int]]::new()
  [void]$family.Add($RootId)
  for ($i = 0; $i -lt 5; $i++) {
    Get-CimInstance Win32_Process | Where-Object { $family.Contains([int]$_.ParentProcessId) } | ForEach-Object { [void]$family.Add([int]$_.ProcessId) }
  }
  return $family
}

Write-Host "== window with a file open"
$proc = Start-Process -FilePath $Exe -ArgumentList @("open", $Fixture) -PassThru
try {
  Start-Sleep -Seconds 15
  if ($proc.HasExited) { throw "the window exited early with code $($proc.ExitCode)" }
  $family = Family $proc.Id
  $names = $family | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName } | Sort-Object -Unique
  Write-Host ("processes: " + ($names -join ", "))
  if ($names -notcontains "sheaf-worker") { throw "no worker was started, so nothing was rendered" }
  $listening = Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $family.Contains([int]$_.OwningProcess) }
  if ($listening) {
    $listening | Format-Table LocalAddress, LocalPort, OwningProcess | Out-String | Write-Host
    throw "the app or a child is listening on a TCP port"
  }
  Write-Host "ok   window up, worker running, no listening TCP sockets"
}
finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Get-Process -Name "sheaf-worker" -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host "== tray helper"
$tray = Start-Process -FilePath $Exe -ArgumentList @("tray") -PassThru
try {
  Start-Sleep -Seconds 5
  if ($tray.HasExited) { throw "the tray helper exited early with code $($tray.ExitCode)" }
  Write-Host "ok   tray helper is running"
}
finally {
  Stop-Process -Id $tray.Id -Force -ErrorAction SilentlyContinue
}
Write-Host "`nsmoke test passed"
