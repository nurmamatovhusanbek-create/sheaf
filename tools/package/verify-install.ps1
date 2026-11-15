# Runs on a clean Windows runner: installs the package silently into a temporary folder, runs the real
# smoke test against the installed program, uninstalls, and checks that nothing is left behind
# (PH-7: clean-VM install).
#
#   ./tools/package/verify-install.ps1 -Setup dist\Sheaf-Setup-0.1.0.exe -Fixture some.pdf
param(
  [Parameter(Mandatory = $true)][string]$Setup,
  [Parameter(Mandatory = $true)][string]$Fixture
)
$ErrorActionPreference = "Stop"
$Setup = (Resolve-Path $Setup).Path
$here = $PSScriptRoot
$target = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-installed"
if (Test-Path $target) { Remove-Item -Recurse -Force $target }
$log = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-setup.log"

Write-Host "== install"
$p = Start-Process -FilePath $Setup -ArgumentList @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/DIR=`"$target`"", "/LOG=`"$log`"") -PassThru -Wait
if ($p.ExitCode -ne 0) { Get-Content $log -Tail 40 | Write-Host; throw "the installer exited with $($p.ExitCode)" }
foreach ($f in "sheaf.exe", "sheafc.exe", "sheaf-worker.exe", "THIRD_PARTY_LICENSES", "vendor\SHA256SUMS", "vendor\pdfium\win-x64\pdfium.dll", "unins000.exe") {
  if (-not (Test-Path (Join-Path $target $f))) { throw "the installed folder lacks $f" }
}
$verb = "HKCU:\Software\Classes\SystemFileAssociations\.pdf\shell\Sheaf\shell\21-compress\command"
if (-not (Test-Path -LiteralPath $verb)) { throw "the installer did not add the right-click menu" }
Write-Host "ok   installed to $target, right-click menu present"

Write-Host "== the installed program, through the smoke test"
& (Join-Path $here "..\smoke\windows.ps1") -Exe (Join-Path $target "sheaf.exe") -Fixture $Fixture
if ($LASTEXITCODE -ne 0) { throw "the smoke test failed on the installed program" }

Write-Host "== uninstall"
$u = Start-Process -FilePath (Join-Path $target "unins000.exe") -ArgumentList @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART") -PassThru -Wait
if ($u.ExitCode -ne 0) { throw "the uninstaller exited with $($u.ExitCode)" }
# The uninstaller finishes in a helper process; give it a moment to delete itself.
$deadline = (Get-Date).AddSeconds(30)
while ((Test-Path (Join-Path $target "sheaf.exe")) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
if (Test-Path (Join-Path $target "sheaf.exe")) { throw "sheaf.exe is still installed after uninstalling" }
if (Test-Path -LiteralPath "HKCU:\Software\Classes\SystemFileAssociations\.pdf\shell\Sheaf") { throw "the right-click menu is still there after uninstalling" }
Write-Host "ok   removed, right-click menu gone"
Write-Host "`ninstall test passed"
