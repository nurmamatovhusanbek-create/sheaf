# Sheaf

Local, offline PDF and scan toolkit for Windows (Rust, native UI). No network at runtime.

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
lints for Windows, but no page has come from a real scanner yet (`RISK-WIA`). **Presets (PH-6)**: saved one-click pipelines (cleanup, OCR, split, naming, compress, output folder) run from
the Presets dialog or `sheaf preset NAME FILES...`. The Explorer right-click menu (`sheaf install-verbs`) gathers a multi-file selection into one action and shows
its progress and result in a small card at the pointer.

The scanner runs in a helper process of its own (`SCAN-ISOLATE`): a driver that crashes or hangs fails one scan with
an error and a "Scan again" button, never the app. If the program itself ever ends without closing properly, the next
start says so, with Details and "Report a problem".

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

## One file

```bash
cargo build --release -p sheaf-onefile      # target/release/sheaf-onefile (.exe on Windows): rename it sheaf.exe
```

The whole program in one executable: the window, the wheel, the scanner helper, the worker (the same file started
with `--session` first) and the engines (PDFium, Tesseract on Windows, language data, font, licence texts), which it
unpacks once into the per-user data folder and checks against hashes compiled into it on every start; a changed,
missing or extra file is replaced. Needs `vendor/` filled in at build time. CI builds it on the Windows runner and keeps
it as the `sheaf-onefile` artifact. It has no console: for scripts use `sheafc.exe` from the installer.

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

Help is built in (F1, or the ? in the top bar; English, Russian and Uzbek, searchable). Messages are kept under the bell;
an error has a Details window with what to try, and "Report a problem" builds a text report on screen that you copy or
save and send yourself: the program has no network, and the report holds no documents, file names or folder names.
The first start shows a Welcome window (start the action wheel, add the right-click menu). The installer is not signed
yet, so Windows may show "unknown publisher": choose More info, then Run anyway.

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
