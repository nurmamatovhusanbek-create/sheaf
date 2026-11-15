# TEST-SEC, Windows: the app and everything it started listen on no TCP port.
# Starts sheaf.exe with a document, lets it run real work, then lists listening
# TCP sockets owned by it or by any process below it (workers, tesseract,
# WebView2). Run without WebDriver: msedgedriver opens a debugging port of its own.
param(
  [Parameter(Mandatory = $true)][string]$App,
  [Parameter(Mandatory = $true)][string]$Fixture
)
$ErrorActionPreference = "Stop"
$proc = Start-Process -FilePath $App -ArgumentList @("open", $Fixture) -PassThru
try {
  Start-Sleep -Seconds 15   # window up, file imported, thumbnails rendered by a worker
  $family = [System.Collections.Generic.HashSet[int]]::new()
  [void]$family.Add($proc.Id)
  for ($i = 0; $i -lt 5; $i++) {
    Get-CimInstance Win32_Process | Where-Object { $family.Contains([int]$_.ParentProcessId) } | ForEach-Object { [void]$family.Add([int]$_.ProcessId) }
  }
  Write-Host ("processes checked: " + (($family | Sort-Object) -join ", "))
  $names = $family | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).ProcessName } | Sort-Object -Unique
  Write-Host ("process names: " + ($names -join ", "))
  if ($names -notcontains "sheaf-worker") { throw "no worker was running, so the check proved nothing" }
  $listening = Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $family.Contains([int]$_.OwningProcess) }
  if ($listening) {
    $listening | Format-Table LocalAddress, LocalPort, OwningProcess | Out-String | Write-Host
    throw "the app or a child is listening on a TCP port"
  }
  Write-Host "ok   no listening TCP sockets"
}
finally {
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  Get-Process -Name "sheaf-worker" -ErrorAction SilentlyContinue | Stop-Process -Force
}
