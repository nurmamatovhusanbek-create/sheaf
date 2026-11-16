# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust + Tauri 2). No network at runtime.

The design and the phase-by-phase build plan are in [ARCHITECTURE.md](ARCHITECTURE.md);
start with its reading guide and the rules for whoever writes code.

**Status:** PH-3 (tools) is done on top of the core and OCR: merge, extract, split with name
patterns, crop, compress, images to PDF, PDF to images, watermarks and page numbers,
AES-256 protection and unlocking. There is no window yet (PH-4); the tools are exercised
through a headless harness and its tests.

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

## One-file build

```bash
cargo build --release -p sheaf-portable        # target/release/sheaf (sheaf.exe on Windows)
```

`sheaf` is the harness, the worker and the engines (PDFium, Tesseract on Windows, language data, font)
in one executable. The worker is the same file started with `--session` first. On first run it unpacks
the engines into the per-user data folder and checks them against hashes compiled into the executable
on every run; changed, missing or extra files are replaced. It needs `vendor/` filled in at build time
(`bash tools/fetch-vendor`, and the Tesseract files unpacked for a Windows target).

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
