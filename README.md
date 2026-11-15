# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust + Tauri 2). No network at runtime.

The design and the phase-by-phase build plan are in [ARCHITECTURE.md](ARCHITECTURE.md);
start with its reading guide and the rules for whoever writes code.

**Status:** PH-4 (shell and UI) is built on top of the core, OCR and tools: a Tauri 2 window for Windows
with a page tray, preview, tools drawer, OCR panel, export dialog, settings and job bar, in English, Uzbek
and Russian. It has been type-checked for Windows from Linux and its UI tested in Chromium; it has **not
yet been run on Windows** (see `RISK-WIN-SHELL` in the spec). Scanning (PH-5) and presets (PH-6) are not
there yet. The headless harness still works.

## Working on it

```bash
bash tools/fetch-vendor                 # download and verify pinned third-party binaries (once)
```

```bash
cargo build --workspace --exclude sheaf-shell   # the tests drive the real worker binary
```

```bash
cargo test --workspace --exclude sheaf-shell    # needs the vendored PDFium and a `tesseract` on PATH (Linux)
```

```bash
cargo check --target x86_64-pc-windows-msvc -p sheaf-shell   # type-check the Windows shell from Linux
```

```bash
cd tools/ui-test && npm ci && node --test "unit/*.test.mjs" && node e2e/run.mjs   # UI logic and browser tests
```

```bash
cargo run -p link-check                 # spec cross-links and generated appendices
```

```bash
cargo run -p link-check -- --write      # regenerate the appendices after editing the spec
```

## Running the app (Windows)

```powershell
bash tools/fetch-vendor
cargo build --release -p sheaf-shell -p sheaf-worker
.\target\release\sheaf.exe open some.pdf     # `open` is optional; files can also be dropped on the window
```

The app finds `vendor/` next to the executable or in the checkout above it (or `SHEAF_VENDOR_DIR`).
Settings live in `%APPDATA%\Sheaf`, the session in `%LOCALAPPDATA%\Sheaf` and is deleted on exit.

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
