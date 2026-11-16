# Gathers exactly what ships into a staging folder (the installer packs this folder, and the CI test
# runs the programs from it too). Windows, after `cargo build --release` and `bash tools/fetch-vendor`.
#
#   ./tools/package/stage.ps1 -Out dist\stage
param(
  [string]$Release = "target\release",
  [string]$Out = "dist\stage"
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$Release = Join-Path $root $Release
$Out = Join-Path $root $Out
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

foreach ($exe in "sheaf.exe", "sheafc.exe", "sheaf-worker.exe") {
  $src = Join-Path $Release $exe
  if (-not (Test-Path $src)) { throw "missing $src (cargo build --release -p sheaf-shell -p sheaf-worker)" }
  Copy-Item $src $Out
}
foreach ($doc in "THIRD_PARTY_LICENSES") {
  Copy-Item (Join-Path $root $doc) $Out
}

# The bundled engines: Windows files only, no download archives, no Linux builds.
$vendor = Join-Path $root "vendor"
$dest = Join-Path $Out "vendor"
function Take([string]$Relative) {
  $from = Join-Path $vendor $Relative
  if (-not (Test-Path $from)) { throw "missing vendor\$Relative (bash tools/fetch-vendor)" }
  $to = Join-Path $dest $Relative
  New-Item -ItemType Directory -Force -Path (Split-Path $to) | Out-Null
  Copy-Item -Recurse $from $to
}
Take "pdfium\win-x64"
Take "pdfium\LICENSE"
Take "tessdata"
Take "fonts"
Take "SHA256SUMS"
if (Test-Path (Join-Path $vendor "tesseract\win-x64")) { Take "tesseract\win-x64" }

# What must not be in the package.
foreach ($bad in "vendor\dl", "vendor\pdfium\linux-x64") {
  if (Test-Path (Join-Path $Out $bad)) { throw "$bad must not ship" }
}
$files = Get-ChildItem -Recurse -File $Out
"{0} files, {1:N1} MB staged in {2}" -f $files.Count, (($files | Measure-Object Length -Sum).Sum / 1MB), $Out
