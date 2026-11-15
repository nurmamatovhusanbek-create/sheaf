# Proves the signing pipeline with a throw-away certificate, on a runner: sign copies of the programs,
# see the signature on each, and see that changing a byte breaks it. The certificate is not trusted
# (that is what a purchased one is for); this checks everything else.
param([Parameter(Mandatory = $true)][string[]]$Files)
$ErrorActionPreference = "Stop"
$work = Join-Path ([System.IO.Path]::GetTempPath()) "sheaf-sign-test"
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null

$cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Sheaf signing test" -CertStoreLocation Cert:\CurrentUser\My
$pfx = Join-Path $work "test.pfx"
$password = ConvertTo-SecureString -String "throw-away" -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $password | Out-Null

$copies = foreach ($f in $Files) { $c = Join-Path $work (Split-Path $f -Leaf); Copy-Item -LiteralPath $f $c; $c }
& (Join-Path $PSScriptRoot "sign.ps1") -Files $copies -PfxPath $pfx -PfxPassword "throw-away" -TimestampUrl ""
if ($LASTEXITCODE -ne 0) { throw "sign.ps1 failed" }
foreach ($c in $copies) {
  $sig = Get-AuthenticodeSignature -LiteralPath $c
  if ($sig.SignerCertificate.Subject -ne "CN=Sheaf signing test") { throw "$c is not signed by the test certificate" }
  # Not trusted (the certificate is self-issued) is expected; a damaged file is not.
  if ($sig.Status -eq "HashMismatch") { throw "$c fails its own signature straight after signing" }
}

# One changed byte must break the signature.
$victim = $copies[0]
$bytes = [System.IO.File]::ReadAllBytes($victim)
# A byte in the middle of the file is program code, which the signature covers. (The last bytes are the
# signature itself; damaging those gives "unknown error", not "changed file".)
$at = [int]($bytes.Length / 2)
$bytes[$at] = $bytes[$at] -bxor 0xFF
[System.IO.File]::WriteAllBytes($victim, $bytes)
$after = Get-AuthenticodeSignature -LiteralPath $victim
if ($after.Status -ne "HashMismatch") { throw "a changed file still verifies: $($after.Status)" }
Write-Host "ok   signed $($copies.Count) programs; a changed byte breaks the signature ($($after.Status))"

Get-ChildItem Cert:\CurrentUser\My | Where-Object { $_.Subject -eq "CN=Sheaf signing test" } | Remove-Item -Force
