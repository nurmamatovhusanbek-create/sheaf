# Signs executables with a code-signing certificate (SEC-SIGN). Without a certificate it does nothing and
# says so: an unsigned build is still a valid build (SmartScreen will warn, RISK-SMARTSCREEN).
#
# The certificate comes from the environment so that it never sits in the repository or in a log:
#   SIGNING_PFX_BASE64    the .pfx file, base64-encoded (a repository secret)
#   SIGNING_PFX_PASSWORD  its password (a repository secret)
# or, for a local run, -PfxPath and -PfxPassword.
#
#   ./tools/package/sign.ps1 -Files dist\stage\sheaf.exe, dist\stage\sheafc.exe
param(
  [Parameter(Mandatory = $true)][string[]]$Files,
  [string]$PfxPath = "",
  [string]$PfxPassword = $env:SIGNING_PFX_PASSWORD,
  # Timestamps keep a signature valid after the certificate expires. Empty means none (offline tests).
  [string]$TimestampUrl = "http://timestamp.digicert.com"
)
$ErrorActionPreference = "Stop"

$temp = $null
if (-not $PfxPath) {
  if (-not $env:SIGNING_PFX_BASE64) {
    Write-Host "sign: no certificate given (SIGNING_PFX_BASE64 is not set); nothing is signed"
    exit 0
  }
  $temp = Join-Path ([System.IO.Path]::GetTempPath()) ("sheaf-sign-" + [guid]::NewGuid().ToString("N") + ".pfx")
  [System.IO.File]::WriteAllBytes($temp, [Convert]::FromBase64String($env:SIGNING_PFX_BASE64))
  $PfxPath = $temp
}

try {
  $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $signtool) { throw "signtool.exe was not found (Windows SDK)" }
  $targets = @($Files | ForEach-Object { Resolve-Path $_ } | ForEach-Object { $_.Path })
  $args = @("sign", "/fd", "SHA256", "/f", $PfxPath)
  if ($PfxPassword) { $args += @("/p", $PfxPassword) }
  if ($TimestampUrl) { $args += @("/tr", $TimestampUrl, "/td", "SHA256") }
  & $signtool.FullName @args @targets
  if ($LASTEXITCODE -ne 0) { throw "signtool exited with $LASTEXITCODE" }
  foreach ($t in $targets) {
    $sig = Get-AuthenticodeSignature -LiteralPath $t
    if (-not $sig.SignerCertificate) { throw "$t has no signature after signing" }
    Write-Host ("signed {0}: {1}" -f (Split-Path $t -Leaf), $sig.SignerCertificate.Subject)
  }
}
finally {
  if ($temp -and (Test-Path $temp)) { Remove-Item -Force $temp }
}
