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
| Tesseract Windows build | not pinned yet: source of the build is an open decision, needed in PH-2 |
| Noto Sans fonts (SIL OFL) | not pinned yet: exact files are an open VERIFY item, needed in PH-3 |
