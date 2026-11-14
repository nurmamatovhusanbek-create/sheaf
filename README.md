# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust + Tauri 2). No network at runtime.

The design and the phase-by-phase build plan are in [ARCHITECTURE.md](ARCHITECTURE.md);
start with its reading guide and the rules for whoever writes code.

**Status:** PH-2 (OCR) is done on top of the PH-1 core: pages without text are recognized
with Tesseract (English, Russian, Uzbek Latin and Cyrillic), results are cached by pixel
content, and export lays an invisible text layer over the untouched page. There is no window
yet (PH-4); a headless harness drives the core.

## Working on it

```bash
bash tools/fetch-vendor                 # download and verify pinned third-party binaries (once)
```

```bash
cargo build --workspace                 # the tests drive the real worker binary
```

```bash
cargo test --workspace                  # needs the vendored PDFium and a `tesseract` on PATH (Linux)
```

```bash
cargo run -p link-check                 # spec cross-links and generated appendices
```

```bash
cargo run -p link-check -- --write      # regenerate the appendices after editing the spec
```

## Trying it

```bash
cargo build --workspace --release
```

```bash
./target/release/sheaf-harness probe some.pdf scan.png
```

```bash
./target/release/sheaf-harness export --out . --name merged.pdf --drop 2 --rotate 90 a.pdf b.pdf
```

```bash
./target/release/sheaf-harness ocr --out . scan.pdf            # writes scan.ocr.pdf; --text-only for .txt
```

```bash
./target/release/sheaf-harness bench --enforce   # PERF-BUDGETS: 100-page open, thumbnails, export
```
