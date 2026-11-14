# vendor/

Third-party binaries, fetched at build time only and hash-pinned (ADR-OFFLINE-ASSETS, SEC-SUPPLY).
Nothing here is downloaded at runtime.

- `manifest.tsv`: destination path and source URL of each asset.
- `SHA256SUMS`: expected SHA-256 of each asset (`sha256sum` format, paths relative to this folder).
- `extract.tsv`: files taken out of downloaded archives; each has its own hash.
- `bash tools/fetch-vendor` downloads what is missing and verifies everything; `--check` verifies without downloading.

| Asset | Status |
|---|---|
| tessdata_fast 4.1.0: `eng`, `rus`, `uzb`, `uzb_cyrl`, `osd` (Apache-2.0) | pinned |
| PDFium build 8125 (bblanchon/pdfium-binaries `chromium/8125`), Windows x64 + Linux x64; the newest build the `pdfium-render` 0.9.5 binding supports | pinned |
| Tesseract 5.5.0 for Windows (UB Mannheim installer from the tesseract-ocr releases, Apache-2.0): `tesseract.exe` and its DLLs, unpacked with 7-Zip on Windows only | pinned |
| `pdf.ttf` (Tesseract 5.5.0), needed for text-layer output | pinned |
| Noto Sans Regular 2.015 and `OFL.txt` (SIL OFL) | pinned |

Still open: the licence texts of the DLLs that ship inside the Tesseract installer have to be
collected into `THIRD_PARTY_LICENSES` before a release.
