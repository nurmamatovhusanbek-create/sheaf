# vendor/

Third-party binaries, fetched at build time only and hash-pinned (ADR-OFFLINE-ASSETS, SEC-SUPPLY).
Nothing here is downloaded at runtime.

- `manifest.tsv`: destination path and source URL of each asset.
- `SHA256SUMS`: expected SHA-256 of each asset (`sha256sum` format, paths relative to this folder).
- `bash tools/fetch-vendor` downloads what is missing and verifies everything; `--check` verifies without downloading.

| Asset | Status |
|---|---|
| tessdata_fast 4.1.0: `eng`, `rus`, `uzb`, `uzb_cyrl`, `osd` (Apache-2.0) | pinned |
| PDFium binaries (bblanchon/pdfium-binaries), Windows x64 + Linux x64 | not pinned yet: needed in PH-1 |
| Tesseract Windows build | not pinned yet: source of the build is an open decision, needed in PH-2 |
| Noto Sans fonts (SIL OFL) | not pinned yet: exact files are an open VERIFY item, needed in PH-3 |
