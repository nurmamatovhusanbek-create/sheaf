# Windows smoke test of the real executable (CI; also runnable by hand).
#
#   ./tools/smoke/windows.ps1 -Exe target\release\sheaf.exe -Fixture some.pdf
#   (the console twin, sheafc.exe beside it, is used for the headless steps)
#
# 1. A headless quick action and a saved preset run through the real worker and write their output.
# 2. The window opens with a file, stays up, renders thumbnails through a worker,
#    and neither it nor anything it started listens on a TCP port (TEST-SEC).
# 3. The tray helper starts and stays up (no window of its own).
# 4. Three processes started for one selection make one action; the right-click menu installs and goes.
#
# Every section runs even when an earlier one fails, so one run shows everything that is wrong.
param(
  [Parameter(Mandatory = $true)][string]$Exe,
  [Parameter(Mandatory = $true)][string]$Fixture
)
$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path
$Cli = Join-Path (Split-Path $Exe) "sheafc.exe"
$Fixture = (Resolve-Path $Fixture).Path
$failures = New-Object System.Collections.Generic.List[string]

function Step([string]$Name, [scriptblock]$Body) {
  Write-Host "== $Name"
  try { & $Body } catch { Write-Host "FAIL $Name : $_"; $script:failures.Add($Name) }
}

function Check-Output([string]$What, $Output) {
  Write-Host "printed: $Output"
  $first = $Output | Select-Object -First 1
  if ($first -match '^\\\\\?\\') { throw "$What printed a path in the verbatim form, which most programs cannot take: $first" }
  if (-not $first -or -not (Test-Path -LiteralPath $first)) { throw "$What wrote no file: $Output" }
  Write-Host "ok   $first"
}

# Stop a program and everything it started. The window may be a second copy of itself (the program
# starts itself again when OpenGL is unusable), and a copy left running would keep its files locked.
function Stop-Tree([int]$RootId) {
  & taskkill.exe /PID $RootId /T /F 2>&1 | Out-Null
  Stop-Process -Id $RootId -Force -ErrorAction SilentlyContinue
}

function Family([int]$RootId) {
  $family = [System.Collections.Generic.HashSet[int]]::new()
  [void]$family.Add($RootId)
  for ($i = 0; $i -lt 5; $i++) {
    Get-CimInstance Win32_Process | Where-Object { $family.Contains([int]$_.ParentProcessId) } | ForEach-Object { [void]$family.Add([int]$_.ProcessId) }
  }
  return $family
}

# What the console twin says when it is asked to open a window: the GUI program has no stderr to show.
function Console-Twin-Says() {
  $log = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-window-stderr.txt"
  $out = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-window-stdout.txt"
  $p = Start-Process -FilePath $Cli -ArgumentList @("open", $Fixture) -PassThru -RedirectStandardError $log -RedirectStandardOutput $out
  Start-Sleep -Seconds 12
  $state = if ($p.HasExited) { "exited with code $($p.ExitCode)" } else { "still running" }
  Stop-Tree $p.Id
  Write-Host "console twin $state"
  foreach ($f in @($log, $out)) { if ((Test-Path $f) -and (Get-Item $f).Length -gt 0) { Write-Host "--- $f"; Get-Content $f | Select-Object -First 30 } }
}

Step "headless: sheaf run compress" {
  $out = & $Cli run compress $Fixture
  if ($LASTEXITCODE -ne 0) { throw "run compress exited with $LASTEXITCODE" }
  Check-Output "run compress" $out
}

Step "headless: sheaf preset" {
  $presets = Join-Path $env:APPDATA "Sheaf\presets"
  New-Item -ItemType Directory -Force -Path $presets | Out-Null
  '{"compress":"Recommended","namePattern":"{source}.demo"}' | Set-Content -Path (Join-Path $presets "Demo scan.json") -Encoding ascii
  $list = @(& $Cli presets)
  if ($list -notcontains "Demo scan") { throw "the preset is not listed: $list" }
  $made = & $Cli preset "Demo scan" $Fixture
  if ($LASTEXITCODE -ne 0) { throw "preset run exited with $LASTEXITCODE" }
  Check-Output "the preset run" $made
}

Step "repair: a PDF with its tail cut off" {
  $bytes = [System.IO.File]::ReadAllBytes($Fixture)
  $cut = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-cut.pdf"
  [System.IO.File]::WriteAllBytes($cut, $bytes[0..($bytes.Length - 81)])
  $made = & $Cli repair $cut 2>$null
  if ($LASTEXITCODE -ne 0) { throw "repair exited with $LASTEXITCODE" }
  Check-Output "repair" $made
}

Step "info: set, read back, strip" {
  $edited = & $Cli info $Fixture --set "title=Smoke test" --set "author=Sheaf" 2>$null
  if ($LASTEXITCODE -ne 0) { throw "info --set exited with $LASTEXITCODE" }
  Check-Output "info --set" $edited
  $said = (& $Cli info ($edited | Select-Object -First 1) 2>&1 | Out-String)
  if ($said -notmatch 'author: Sheaf') { throw "the author was not read back: $said" }
  $cleaned = & $Cli info ($edited | Select-Object -First 1) --strip 2>$null
  Check-Output "info --strip" $cleaned
  if ((Get-Content -LiteralPath ($cleaned | Select-Object -First 1) -Raw -Encoding Latin1) -match 'Smoke test') { throw "the title is still in the stripped file" }
}

Step "Explorer-style selection: three processes, one merge" {
  $agg = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-agg"
  Remove-Item -Recurse -Force $agg -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force -Path $agg | Out-Null
  1..3 | ForEach-Object { Copy-Item -LiteralPath $Fixture -Destination (Join-Path $agg "m$_.pdf") }
  $procs = 1..3 | ForEach-Object {
    Start-Process -FilePath $Cli -ArgumentList @("merge", "--collect", "`"$(Join-Path $agg "m$_.pdf")`"") `
      -RedirectStandardOutput (Join-Path $agg "out$_.txt") -RedirectStandardError (Join-Path $agg "err$_.txt") -PassThru
  }
  $procs | Wait-Process -Timeout 120
  $printed = @(Get-ChildItem $agg -Filter "out*.txt" | Get-Content | Where-Object { $_ })
  if ($printed.Count -ne 1) {
    Get-ChildItem $agg -Filter "err*.txt" | Get-Content | Write-Host
    throw "expected exactly one process to act, but $($printed.Count) paths were printed: $printed"
  }
  Check-Output "the collected merge" $printed
}

Step "Explorer right-click menu: install, check, remove" {
  $base = "HKCU:\Software\Classes\SystemFileAssociations"
  & $Cli install-verbs
  if ($LASTEXITCODE -ne 0) { throw "install-verbs exited with $LASTEXITCODE" }
  $cmd = (Get-ItemProperty -LiteralPath "$base\.pdf\shell\Sheaf\shell\21-compress\command").'(default)'
  Write-Host "compress verb: $cmd"
  if ($cmd -notmatch '--collect' -or $cmd -notmatch '--card' -or $cmd -notmatch 'sheaf\.exe') { throw "the compress verb is not what was meant: $cmd" }
  $preset = (Get-ItemProperty -LiteralPath "$base\.pdf\shell\Sheaf\shell\50-preset0\command").'(default)'
  if ($preset -notmatch 'preset "Demo scan"') { throw "the preset verb is missing: $preset" }
  if (-not (Test-Path -LiteralPath "$base\.png\shell\Sheaf\shell\10-open\command")) { throw "no entry for pictures" }
  & $Cli uninstall-verbs
  if ($LASTEXITCODE -ne 0) { throw "uninstall-verbs exited with $LASTEXITCODE" }
  foreach ($ext in "pdf", "png", "jpg") {
    if (Test-Path -LiteralPath "$base\.$ext\shell\Sheaf") { throw "the entry for .$ext is still there" }
  }
  Write-Host "ok   installed, checked and removed"
}

Step "window with a file open" {
  $proc = Start-Process -FilePath $Exe -ArgumentList @("open", $Fixture) -PassThru
  try {
    Start-Sleep -Seconds 15
    if ($proc.HasExited) {
      $code = $proc.ExitCode
      Console-Twin-Says
      throw "the window exited early with code $code"
    }
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
    Stop-Tree $proc.Id
    Get-Process -Name "sheaf-worker" -ErrorAction SilentlyContinue | Stop-Process -Force
  }
}

Step "tray helper" {
  $tray = Start-Process -FilePath $Exe -ArgumentList @("tray") -PassThru
  try {
    Start-Sleep -Seconds 5
    if ($tray.HasExited) { throw "the tray helper exited early with code $($tray.ExitCode)" }
    Write-Host "ok   tray helper is running"
  }
  finally {
    Stop-Tree $tray.Id
  }
}

if ($failures.Count -gt 0) {
  Write-Host "`nsmoke test FAILED: $($failures -join '; ')"
  exit 1
}
Write-Host "`nsmoke test passed"
