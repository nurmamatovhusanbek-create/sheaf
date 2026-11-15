# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust + Tauri 2). No network at runtime.

The design and the phase-by-phase build plan are in [ARCHITECTURE.md](ARCHITECTURE.md);
start with its reading guide and the rules for whoever writes code.

**Status:** PH-4 (shell and UI) is built on top of the core, OCR and tools, as a **native Rust app** (egui,
no web view): a window with a page tray, preview, tools drawer, OCR panel, export dialog, settings and job
bar, in English, Uzbek and Russian; plus the **action wheel**: a tray helper with a global hotkey that, over
Explorer or the desktop, pops a radial menu at the cursor and merges, compresses, OCRs, splits, converts or
rotates the selected files with no window. The same actions run from the command line (`sheaf run ...`).
The window and the wheel are exercised on Linux under a virtual display; the Windows-only parts (tray helper,
hotkey, Explorer and desktop selection, overlay placement) type-check for Windows but **have not yet run on
Windows** (see `RISK-WIN-SHELL` in the spec).

**Scanning (PH-5)** works end to end against a fake scanner (dialog, live pages in the tray, optional
recognition while scanning, cancel keeps what was scanned). The Windows backend (WIA 2) is written and
lints for Windows, but no page has come from a real scanner yet (`RISK-WIA`). Presets (PH-6) are not there yet.

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
cargo clippy --target x86_64-pc-windows-msvc -p sheaf-shell -p sheaf-ui -- -D warnings   # the Windows-only code, from Linux
```

```bash
bash tools/smoke/gui-linux.sh           # window + wheel + headless action under Xvfb (needs xvfb, imagemagick, mesa)
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
.\target\release\sheaf.exe open some.pdf       # the window (files can also be dropped on it)
.\target\release\sheaf.exe tray                # the helper: tray icon + Ctrl+Alt+Space action wheel
.\target\release\sheaf.exe run compress a.pdf b.pdf    # headless; prints the outputs, exit 0 / 2 / 1
```

Select files in Explorer or on the desktop, press the hotkey, click an action. Actions: `merge compress ocr
split pdf2img img2pdf rotate`. Results land beside the first input under a new name; nothing is overwritten.
The app finds `vendor/` next to the executable or in the checkout above it (or `SHEAF_VENDOR_DIR`).
Settings live in `%APPDATA%\Sheaf` (the hotkey too), the session in `%LOCALAPPDATA%\Sheaf` and is deleted on exit.

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
