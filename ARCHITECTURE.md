# Sheaf: architecture and agent build spec

**Status** draft 0.3 · 2026-11-15 · **Target** Windows 10/11 x64 · **Stack** Rust, native egui UI (no web view) · **Network** none at runtime

> "Sheaf" is a placeholder name. Rename in one place: `APP_NAME` in [`MOD-CORE`](#mod-core) and the bundle identifier in [`MOD-SHELL`](#mod-shell).

**Contents**

- [0. Reading guide (agents start here)](#doc-read)
- [1. Goals and non-goals](#doc-goals)
- [2. Decisions](#doc-adr)
- [3. System shape](#doc-shape)
- [4. Repository layout](#doc-layout)
- [5. Data model](#doc-data)
- [6. Modules](#doc-modules)
- [7. IPC: commands and events](#doc-ipc)
- [8. Tools (iLovePDF-style, all local)](#doc-tools)
- [9. Presets (one-click pipelines)](#preset-format)
- [10. Command line and Explorer verbs](#cli-grammar)
- [11. Flows](#doc-flows)
- [12. OCR subsystem](#doc-ocr)
- [13. Scanner subsystem](#doc-scan)
- [14. Security](#doc-security)
- [15. Performance](#doc-perf)
- [16. UI](#doc-ui)
- [17. Testing](#doc-tests)
- [18. Build, packaging, CI](#doc-build)
- [19. Build plan](#ph-plan)
- [20. Risks and open questions](#doc-risks)
- [21. Traceability matrix (tool → module → command → UI → test → phase)](#doc-trace)
- [22. Rules for the agent that builds this](#doc-agent-rules)
- [23. External references (checked 2026-11-13)](#doc-refs)
- [Appendix A. Reverse index](#appendix-a)
- [Appendix B. Open VERIFY items](#appendix-b)

## <a id="doc-read"></a>0. Reading guide (agents start here)

1. Read sections 1 to 3 first (goals, decisions, shape), then jump by ID.
2. Every concept has a stable ID. IDs are never renamed or reused; new ones are appended.
3. Every ID mention is a link. Appendix A lists, for each ID, everything that references it. [`DOC-TRACE`](#doc-trace) is the traceability matrix (tool, module, command, UI, test, phase).
4. **VERIFY** marks a claim that is not yet proven. Resolve it by testing, then edit the text and remove the marker. Appendix B lists all open ones.
5. Rust snippets are sketches of intent, not compiled code.
6. Hard rules for whoever writes code are in [`DOC-AGENT-RULES`](#doc-agent-rules). Start there before touching anything.

| Prefix | Kind |
|---|---|
| GOAL / NG | goals, non-goals |
| ADR | architecture decision |
| PROC | runtime process |
| MOD | code module (crate or folder) |
| DATA | data structure |
| CMD / EVT | UI-to-core command, core-to-UI event |
| TOOL | iLovePDF-style tool |
| FLOW | end-to-end user or system flow |
| OCR / SCAN | subsystem detail |
| SEC / PERF | security control, performance budget |
| UI | user-interface component |
| TEST | test group |
| PH | build phase |
| RISK | known risk or open question |
| CLI / PRESET | command-line grammar, preset format |

## <a id="doc-goals"></a>1. Goals and non-goals

| ID | Goal | Checked by |
|---|---|---|
| <a id="goal-secure"></a> | No network listener, no telemetry, hostile files tolerated | [`SEC-NONET`](#sec-nonet), [`SEC-ISOLATE`](#sec-isolate), [`TEST-SEC`](#test-sec) |
| <a id="goal-fast"></a> | Instant UI, parallel work, lossless page copying | [`PERF-BUDGETS`](#perf-budgets) |
| <a id="goal-compact"></a> | One window, one tools drawer, no accounts, small installer | [`UI-DRAWER`](#ui-drawer), [`DOC-NONGOALS`](#doc-nongoals) |
| <a id="goal-liveocr"></a> | OCR overlaps scanning, pages searchable as they arrive | [`FLOW-SCAN-LIVE`](#flow-scan-live) |
| <a id="goal-oncmd-ocr"></a> | OCR on command for any existing PDF or image | [`FLOW-OCR-MANUAL`](#flow-ocr-manual) |
| <a id="goal-ilovepdf"></a> | iLovePDF-style tool coverage, locally | [`DOC-TOOLS`](#doc-tools) |
| <a id="goal-agent"></a> | An autonomous agent can build it phase by phase | [`PH-PLAN`](#ph-plan), [`DOC-AGENT-RULES`](#doc-agent-rules) |
| <a id="goal-wheel"></a> | Work on files from anywhere: an action wheel at the cursor, on the desktop or in Explorer, acting on what is selected | [`ADR-WHEEL`](#adr-wheel), [`FLOW-WHEEL`](#flow-wheel), [`TEST-WHEEL`](#test-wheel) |

### <a id="doc-nongoals"></a>Non-goals

| ID | Non-goal | Why |
|---|---|---|
| <a id="ng-cloud"></a> | Cloud, accounts, sync, telemetry, auto-update | Contradicts [`GOAL-SECURE`](#goal-secure) |
| <a id="ng-office"></a> | PDF to or from Word, Excel, PowerPoint | Needs LibreOffice or Office; revisit after v1 |
| <a id="ng-edittext"></a> | Free-form editing of existing PDF text or layout | Large surface, low value for scan workflows |
| <a id="ng-ai"></a> | Chat, summarizing, "AI features" | Not the job |
| <a id="ng-xplat"></a> | macOS or Linux GUI in v1 (core crates still build there) | Core crates still compile and test on Linux ([`TEST-MATRIX`](#test-matrix)) |
| <a id="ng-crack"></a> | Password recovery or cracking | [`TOOL-UNLOCK`](#tool-unlock) requires the correct password |

## <a id="doc-adr"></a>2. Decisions

| ID | Decision | Why | Rejected | Revisit when |
|---|---|---|---|---|
| <a id="adr-tauri"></a> | Rust core and a native Rust UI; no web view. (Superseded the earlier Tauri 2 + WebView2 decision: see [`ADR-NATIVE-UI`](#adr-native-ui).) | One language, one process, small and quick to start, nothing to render HTML with, no IPC boundary inside the app | .NET (easier WIA), Python + pywebview (size, AV false positives), Electron (size), Tauri 2 (a web page in a native frame) | [`RISK-WIA`](#risk-wia) proves unworkable in Rust |
| <a id="adr-nohttp"></a> | No localhost server and no network code at all; the UI is in-process | Any local process or browser tab can reach a port | Flask/axum on 127.0.0.1 | never |
| <a id="adr-nondestructive"></a> | Pages are records; edits change data, pixels are written only at export or tool run | Fast, cheap undo, lossless merges ([`DATA-PAGE`](#data-page)) | Rewrite PDF on every edit | never |
| <a id="adr-pdfium-lopdf"></a> | `pdfium-render` for rendering, text extraction, page import, saving, and for overlays (the OCR text layer, stamps); `lopdf` for encryption, metadata, optimization, embedded-image extraction | PDFium renders reliably and imports pages; lopdf gives object-level control | Single library for everything | [`RISK-LOPDF-ENC`](#risk-lopdf-enc) |
| <a id="adr-tess-sidecar"></a> | Bundled `tesseract.exe` called per job; `OcrEngine` trait | Avoids C++/Leptonica build on Windows, isolates crashes | libtesseract bindings | per-page spawn cost shows up in [`PERF-BUDGETS`](#perf-budgets) |
| <a id="adr-no-vlm-default"></a> | Unlimited-OCR (3B VLM) is not the default engine | GPU-oriented, needs `trust_remote_code`, weak on Uzbek Cyrillic in one third-party benchmark, hallucination risk | VLM as default | a CPU-viable build passes [`TEST-OCR`](#test-ocr) on real documents ([`OCR-ENGINES`](#ocr-engines)) |
| <a id="adr-wia"></a> | Scanner support via WIA first; TWAIN and ESCL later | Native on Windows, no vendor SDK | TWAIN first | [`RISK-WIA`](#risk-wia) |
| <a id="adr-worker-procs"></a> | Heavy PDF and image work runs in separate worker processes ([`PROC-WORKER`](#proc-worker)) | PDFium is not thread-safe (the `pdfium-render` documentation says to assume so, and that the PDFium authors recommend parallel processes over threads), crash and hang containment, memory caps | Thread pool in main process | never |
| <a id="adr-no-overwrite"></a> | Outputs never overwrite inputs; `name.suffix.pdf` next to the source or in the output folder | Safety | In-place save | never |
| <a id="adr-explorer-verbs"></a> | Explorer integration is per-user registry verbs that launch the exe with arguments ([`CLI-GRAMMAR`](#cli-grammar)); the program is `sheaf.exe`, not the console twin, so no console window flashes, and the card gives the feedback the console would have | No admin, no shell-extension DLL | COM IExplorerCommand package | Windows 11 top-level menu is required |
| <a id="adr-i18n"></a> | Key-based strings: English, Uzbek (Latin), Russian | Target user base | Hard-coded text | never |
| <a id="adr-vanilla-ui"></a> | (Superseded by [`ADR-NATIVE-UI`](#adr-native-ui).) The UI had been plain HTML/CSS/ES modules with no framework | Compact, auditable | React/Svelte | n/a |
| <a id="adr-offline-assets"></a> | pdfium, tesseract, tessdata and fonts are bundled and hash-pinned; fetched only at build time in CI | Zero runtime network ([`SEC-NONET`](#sec-nonet)) | Download on first run | never |
| <a id="adr-app-crate"></a> | The main-process logic (workspace, jobs, every [`DOC-IPC`](#doc-ipc) command and its validation) lives in a toolkit-free crate, [`MOD-APP`](#mod-app); the window, the wheel, the command line and the harness all call it | The command layer and the quick actions are tested against the real worker with no window; the same code runs in every mode | Logic inside the UI crate (every mode would need the toolkit) | a second front end appears |
| <a id="adr-ui-reorder"></a> | Page reordering is a pointer drag inside the tray, with edge scrolling and an insertion line | Works the same for one page or a selection | A "move to…" dialog only | users ask for another way |
| <a id="adr-native-ui"></a> | The window is native Rust UI (egui over OpenGL), embedded in `sheaf.exe`; text is set in the bundled Noto Sans | The product is an app, not a web page: no HTML, JavaScript, browser engine or WebView2 runtime to install, patch or attack; the UI shares the process with [`MOD-APP`](#mod-app) so there is no IPC boundary to defend; one language; software GL makes it testable on any machine | Tauri/WebView2 (the first PH-4 build, replaced), Slint (licence terms), WinUI/Win32 (largest rewrite, untestable off Windows) | the widget set cannot do what a screen needs, or immediate-mode drawing shows up in [`PERF-BUDGETS`](#perf-budgets) |
| <a id="adr-wheel"></a> | The action wheel is a separate short-lived overlay (`sheaf wheel`) started by a resident tray helper (`sheaf tray`); it reads the selection from Explorer or the desktop through the shell's COM interfaces, runs [`MOD-APP`](#mod-app) quick actions with no main window, and shows the result in the same place | The wheel must appear at the cursor and work on what is selected where the person already is; a helper that only waits in the message loop costs nothing; a fresh overlay process has no stale window state and cannot take the main window down | One long-lived process with a hidden window (hidden windows get no redraws), a shell-extension DLL (installer, admin, crash risk inside Explorer), a global mouse hook | Windows 11 changes how Explorer exposes its selection |

## <a id="doc-shape"></a>3. System shape

```
 Explorer verb / app icon ──► sheaf.exe (PROC-MAIN)
 tray icon + hotkey ──► sheaf tray (PROC-TRAY) ──► sheaf wheel (PROC-WHEEL) ──► quick action
                               │ in-process calls (no port, no IPC)   ┌───────────────────────┐
                               │  MOD-UI egui window and wheel         │ session dir (temp)    │
                               │  MOD-APP workspace, commands, jobs    │ cache, thumbs, results│
                               │  MOD-CORE workspace + undo            │ MOD-STORE             │
                               └──────┬────────────────────────────────┴───────────────────────┘
                                      │ stdio JSON lines (DATA-WORKERPROTO)
                        ┌─────────────┴──────────────┐
                        ▼                            ▼
             PROC-WORKER × N            PROC-WORKER (scan thread: MOD-SCAN)
             MOD-PDF MOD-IMAGE                  WIA COM
                        │ spawn
                        ▼
                  PROC-TESS tesseract.exe (MOD-OCR)
```

| ID | Process | Role | Trust |
|---|---|---|---|
| <a id="proc-main"></a> | `sheaf.exe` | The native window ([`MOD-UI`](#mod-ui)), workspace, job scheduling, all in one process. Never parses untrusted file content. | trusted |
| <a id="proc-ui"></a> | (in `sheaf.exe`) | The egui views of [`MOD-UI`](#mod-ui). Part of the main process: no web view, no network code. The command layer still validates what the views send, so the same rules serve the command line and the wheel ([`SEC-IPC`](#sec-ipc)). | trusted |
| <a id="proc-tray"></a> | `sheaf tray` | Resident helper: tray icon, the wheel hotkey, reading the selection of the window in front. Draws nothing; waits in the Windows message loop. Writes the file list to a private file and starts the wheel. | trusted; the paths it reads are untrusted input ([`SEC-PATHS`](#sec-paths)) |
| <a id="proc-wheel"></a> | `sheaf wheel` | The wheel overlay: a transparent window at the cursor, then a result card; runs the chosen quick action with its own [`MOD-APP`](#mod-app) and workers. Lives seconds. | trusted |
| <a id="proc-worker"></a> | `sheaf-worker.exe` × N | Opens PDFs and images, renders, cleans, builds output ([`MOD-WORKER`](#mod-worker)). Memory-capped, killed on timeout. | handles hostile files |
| <a id="proc-tess"></a> | `tesseract.exe` per job | OCR. Fed pre-cleaned PNG, returns TSV/PDF. | handles hostile pixels |

Trust boundaries: <a id="sec-tb1"></a> UI to main (IPC), <a id="sec-tb2"></a> main to worker (stdio), <a id="sec-tb3"></a> worker to tesseract (process spawn), <a id="sec-tb4"></a> app to disk (files chosen by the user or passed by Explorer). Controls per boundary are in [`DOC-SECURITY`](#doc-security).

## <a id="doc-layout"></a>4. Repository layout

```
sheaf/
  Cargo.toml                    workspace
  crates/
    sheaf-core/     MOD-CORE     workspace model, ops, undo, errors, consts (no I/O)
    sheaf-store/    MOD-STORE    session dir, cache, settings, presets, path policy
    sheaf-pdf/      MOD-PDF      pdfium + lopdf operations
    sheaf-image/    MOD-IMAGE    decode, deskew, binarize, blank detect, encode
    sheaf-ocr/      MOD-OCR      engine trait, tesseract driver, langs, text layer, search
    sheaf-scan/     MOD-SCAN     WIA backend + fake backend (cfg(windows))
    sheaf-jobs/     MOD-JOBS     scheduler, worker pool, limits, protocol
    sheaf-worker/   MOD-WORKER   worker binary: job handlers
    sheaf-cli/      MOD-CLI      argument grammar, single-instance aggregation
    sheaf-app/      MOD-APP      main-process core: workspace, jobs, DOC-IPC commands (no window toolkit)
    sheaf-ui/       MOD-UI       egui views: main window, action wheel, i18n (en/uz/ru JSON embedded)
    sheaf-shell/    MOD-SHELL    the `sheaf` binary: modes, tray helper, hotkey, Explorer/desktop selection (Windows)
  tools/smoke/                   GUI and Windows smoke scripts
  vendor/           pdfium.dll, tesseract/, tessdata/, fonts/, SHA256SUMS (CI-fetched)
  fixtures/         test documents (TEST-FIXTURES)
  tools/            link-check, fetch-vendor, fixture-gen, harness (headless CLI + bench)
  .github/workflows/ build + test + package (DOC-BUILD)
```

## <a id="doc-data"></a>5. Data model

All types live in [`MOD-CORE`](#mod-core) unless noted. Serialization is `serde`; IPC uses JSON ([`DOC-IPC`](#doc-ipc)).

### <a id="data-source"></a>Source and <a id="data-page"></a>Page

```rust
struct Source { id: SourceId, path: PathBuf, kind: SourceKind /*Pdf|Image*/, sha256: [u8;32],
                page_count: u32, encrypted: bool, unlocked: bool }
struct Page   { id: PageId, content: PageContent /*Source{src, index} | Blank{width_pt, height_pt}*/,
                rot: Rot /*0|90|180|270*/,
                crop: Option<NormRect>, filter: Filter, ocr: OcrState, has_text: Option<bool> }
```

A page is a pointer into a source plus edits ([`ADR-NONDESTRUCTIVE`](#adr-nondestructive)); a blank page has a size instead of a source. `rot` is added to the source page's own rotation. `crop` is a rectangle in fractions of the page, in the page's frame before `rot`, so rotating a page never changes its crop. Page ids are never reused, not even after undo. Scanned pages become image Sources written to the session dir ([`MOD-STORE`](#mod-store)) the moment they arrive ([`FLOW-SCAN-LIVE`](#flow-scan-live)).

### <a id="data-filter"></a>Filter

`mode: Color|Gray|BW`, `threshold: Auto|u8`, `contrast: i8`, `deskew: Off|Auto|Angle(f32)`, `despeckle: bool`. Applied by [`MOD-IMAGE`](#mod-image) at render/export time only.

### <a id="data-ocrstate"></a>OcrState

`None | Queued | Running | Done{engine, langs, key} | SkippedHasText | Failed{reason}`. The `key` is a [`DATA-CACHEKEY`](#data-cachekey).

### <a id="data-workspace"></a>Workspace

`pages: Vec<Page>` in a persistent (structure-sharing) vector so each undo snapshot is cheap (the `imbl` crate, the maintained fork of `im`); `rev: u64` increments on every change; undo and redo stacks capped at 100. OCR state and the text-layer flag are recorded on pages outside the undo history: they describe the pixels, they are not edits. Selection lives in the UI ([`UI-TRAY`](#ui-tray)) and is passed to commands by `PageId`.

### <a id="data-op"></a>Op (the only way to change a workspace)

`Insert{at, pages} | Delete{ids} | Move{ids, to} | Rotate{ids, delta} | Crop{ids, rect|None} | SetFilter{ids, filter} | Duplicate{ids} | InsertBlank{at, size}`. Every Op is invertible; [`TEST-OPS`](#test-ops) property-tests `apply(op); undo() == identity`.

### <a id="data-cachekey"></a>OCR cache key

`sha256(image size ‖ engine_id ‖ engine_version ‖ langs ‖ params ‖ pixels_sent_to_engine)` as hex (SHA-256 because the store already carries it; hashing is a small fraction of recognition time). `langs` is the setting, `auto` or the fixed list, so automatic and fixed-language runs are separate entries. Results live at `<session>/cache/<key>/{result.tsv, page.pdf, meta.json}` ([`OCR-CACHE`](#ocr-cache)).

### <a id="data-ocrresult"></a>OcrResult

`words: Vec<{text, bbox, conf, line}>`, `orientation_deg`, `script`, `langs_used`, `mean_conf`. Parsed from Tesseract TSV ([`OCR-TESS-CLI`](#ocr-tess-cli)).

### <a id="data-scanprofile"></a>ScanProfile

`device_id`, `source: Flatbed|Feeder`, `duplex: bool`, `dpi: 150|200|300|600`, `color: Color|Gray|BW`, `paper: A4|A3|Letter|Auto`.

### <a id="data-ocrpolicy"></a>OcrPolicy

`mode: Auto|Off|Manual`, `engine: tesseract|<future>`, `langs: Auto|Fixed(Vec<Lang>)`, `quality: Fast|Accurate`, `existing: KeepText|ReplaceText`.

### <a id="data-splitrule"></a>SplitRule

`None | EveryN(n) | BlankSheet | TextMatch(regex) | Ranges("1-3,5")`. Used by [`TOOL-SPLIT`](#tool-split) and presets ([`PRESET-FORMAT`](#preset-format)).

### <a id="data-namepattern"></a>NamePattern

Template string with tokens `{date}`, `{n}`, `{source}`, `{match:<regex>}` (first regex match found in the OCR text of the first page of each output), `{counter}`. `{n}` is the number of the output within the run and `{counter}` the same number padded to three digits; `{date}` is today's UTC date; a `{match:…}` with no match expands to nothing. Unsafe characters are replaced; length capped at 120; collisions get `-2`, `-3`.

### <a id="data-exportspec"></a>ExportSpec

`pages: Vec<PageId>|All`, `format: Pdf|Images{png|jpg,dpi}`, `ocr: OcrPolicy`, `split: SplitRule`, `name: NamePattern`, `out_dir`, `compress: Option<Level>`, `protect: Option<{user_pw, owner_pw, perms}>`, `metadata: Keep|Strip|Set`.

### <a id="data-job"></a>Job

`id`, `kind` (a TOOL or EXPORT or OCR or THUMB or RENDER), `args`, `state: Queued|Running|Done|Failed|Cancelled`, `progress: f32`, `priority`, `cancel: token`, `limits: {timeout, rss_mb}`.

### <a id="data-settings"></a>Settings

`%APPDATA%\Sheaf\settings.json`: UI language, output dir, default preset, worker count (default cores minus 1, min 1), OCR quality and mode, theme, the wheel hotkey (default `ctrl+alt+space`; the tray helper notices a change by itself), whether files on network shares may be opened, recent folders. No document content, no passwords, no history of file names.

### <a id="data-workerproto"></a>Worker protocol

Newline-delimited JSON over the worker's stdin/stdout.

```json
{"id":17,"op":"ocr_page","args":{"src":"S3","index":4,"filter":{...},"langs":["eng","rus"],"out":"cache/ab12"}}
{"id":17,"progress":0.4}
{"id":17,"ok":true,"result":{"key":"ab12","mean_conf":91.2}}
{"id":17,"ok":false,"error":{"code":"ParseError","msg":"..."}}
```

Rules: no binary on the pipe, large results are files under the session dir, and every path in a request must resolve inside the session dir or be an explicitly allowed input ([`SEC-PATHS`](#sec-paths)). Paths a worker writes are session-relative; the main process then publishes a finished file to the user's folder. The pool does not trust worker output either: lines are capped at 1 MiB, a malformed line counts as a crash, and a worker cannot claim a timeout, cancellation or limit on its own behalf. Worker ops are listed per module in [`MOD-WORKER`](#mod-worker).

## <a id="doc-modules"></a>6. Modules

Each entry: purpose, API sketch, depends on, used by. A module may only depend on those listed ([`DOC-AGENT-RULES`](#doc-agent-rules) enforces this with `cargo-deny` and a dependency test).

### <a id="mod-core"></a>MOD-CORE (`sheaf-core`)

Pure logic, no I/O, no threads. `Workspace`, [`DATA-OP`](#data-op) application, undo/redo, ids, error enum, constants (`APP_NAME`, limits).

```rust
impl Workspace { fn apply(&mut self, op: Op) -> Result<Rev>; fn undo(&mut self)->Option<Rev>; fn redo(&mut self)->Option<Rev>;
                 fn pages(&self, range: Range<usize>) -> &[Page]; fn find(&self, id: PageId) -> Option<&Page>; }
```

Also owns `QuickAction`, `QuickRequest` and the file-kind rules the wheel and the command line share ([`ADR-WHEEL`](#adr-wheel)). Owns the types [`DATA-SOURCE`](#data-source), [`DATA-WORKSPACE`](#data-workspace), [`DATA-JOB`](#data-job) and [`DATA-OCRRESULT`](#data-ocrresult) (the last two are produced by [`MOD-JOBS`](#mod-jobs) and [`MOD-OCR`](#mod-ocr) but defined here). Depends on: nothing. Used by: [`MOD-SHELL`](#mod-shell), [`MOD-JOBS`](#mod-jobs), [`MOD-PDF`](#mod-pdf) (types), [`MOD-OCR`](#mod-ocr) (types). Tests: [`TEST-OPS`](#test-ops), [`TEST-UNDO`](#test-undo).

### <a id="mod-store"></a>MOD-STORE (`sheaf-store`)

Session dir `%LOCALAPPDATA%\Sheaf\session-<pid>-<rand>\` created with a per-user ACL; thumbnail and OCR caches; atomic writes (temp file then rename); settings and presets I/O; stale-session sweep at startup; the path policy ([`SEC-PATHS`](#sec-paths)); temp-data rules ([`SEC-TEMP`](#sec-temp)); content hashing.

```rust
fn session() -> &Session; fn cache_path(key:&CacheKey)->PathBuf; fn write_atomic(p:&Path, bytes:&[u8])->Result<()>;
fn allow_input(p:&Path)->Result<AllowedPath>; fn allow_output(dir:&Path, name:&str)->Result<AllowedPath>;
```

Depends on: [`MOD-CORE`](#mod-core). Used by: [`MOD-SHELL`](#mod-shell), [`MOD-JOBS`](#mod-jobs), [`MOD-WORKER`](#mod-worker), [`MOD-OCR`](#mod-ocr). Tests: [`TEST-SEC`](#test-sec).

### <a id="mod-pdf"></a>MOD-PDF (`sheaf-pdf`)

PDFium and lopdf operations. Runs only inside [`PROC-WORKER`](#proc-worker).

```rust
fn open(src:&Source, pw:Option<&str>) -> Result<Doc>;
fn render(doc:&Doc, page:&Page, max_px:Size) -> Result<RgbaImage>;        // thumbs and OCR input
fn has_text_layer(doc:&Doc, idx:u32) -> bool;                            // OCR-PIPELINE step 1
fn build_pdf(spec:&[PageRef], opts:&BuildOpts) -> Result<PathBuf>;       // import pages, set /Rotate, crop box
fn overlay_text(pdf:&Path, layers:&[TextLayer]) -> Result<PathBuf>;      // OCR-TEXTLAYER, TOOL-WATERMARK, TOOL-PAGENUM
fn compress(pdf:&Path, lvl:Level) -> Result<PathBuf>;                    // TOOL-COMPRESS
fn encrypt(pdf:&Path, o:&Protect) -> Result<PathBuf>; fn decrypt(pdf:&Path, pw:&str) -> Result<PathBuf>;
fn images_to_pdf(imgs:&[ImageRef], o:&ImgPdfOpts) -> Result<PathBuf>;   // TOOL-IMG2PDF
fn extract_images(...)/render_pages_to_files(...);                       // TOOL-PDF2IMG
```

Depends on: [`MOD-CORE`](#mod-core), [`MOD-IMAGE`](#mod-image). Used by: [`MOD-WORKER`](#mod-worker). Notes: untouched pages are imported, never re-encoded ([`PERF-LOSSLESS`](#perf-lossless)); cropping and PDF encryption specifics in [`TOOL-CROP`](#tool-crop) and [`TOOL-PROTECT`](#tool-protect); risks [`RISK-PDFIUM-THREADS`](#risk-pdfium-threads), [`RISK-LOPDF-ENC`](#risk-lopdf-enc), [`RISK-FORMS-MERGE`](#risk-forms-merge).

### <a id="mod-image"></a>MOD-IMAGE (`sheaf-image`)

Pure image functions: decode (JPEG, PNG, TIFF incl. multipage, BMP, WebP), EXIF orientation, crop, deskew (projection-profile search, small-angle range), binarize (Otsu, Sauvola for uneven lighting), despeckle, blank-page detection (ink coverage under threshold), downsample, encode (PNG, JPEG; CCITT G4 for bilevel is a later option). Multipage TIFF decodes its first page only until [`TOOL-IMG2PDF`](#tool-img2pdf) lands.

```rust
fn apply_filter(img:RgbaImage, f:&Filter) -> RgbaImage; fn is_blank(img:&RgbaImage, sens:f32)->bool;
fn estimate_skew(img:&GrayImage)->f32; fn downsample(img, target_dpi, src_dpi)->RgbaImage;
```

Depends on: [`MOD-CORE`](#mod-core). Used by: [`MOD-PDF`](#mod-pdf), [`MOD-OCR`](#mod-ocr), [`MOD-WORKER`](#mod-worker). Tests: [`TEST-IMAGE`](#test-image).

### <a id="mod-ocr"></a>MOD-OCR (`sheaf-ocr`)

```rust
trait OcrEngine { fn id(&self)->&str; fn version(&self)->String; fn detect(&self, img:&Path)->Result<Osd>;
                  fn recognize(&self, img:&Path, o:&RecOpts)->Result<OcrResult>; }
```

Contains: Tesseract driver ([`OCR-TESS-CLI`](#ocr-tess-cli)), language chooser ([`OCR-LANGS`](#ocr-langs)), cache lookup ([`OCR-CACHE`](#ocr-cache)), TSV parser, text-layer builder ([`OCR-TEXTLAYER`](#ocr-textlayer)), and a small in-memory full-text index for [`CMD-SEARCH`](#cmd-search).

Depends on: [`MOD-CORE`](#mod-core), [`MOD-IMAGE`](#mod-image), [`MOD-STORE`](#mod-store). Used by: [`MOD-WORKER`](#mod-worker) (recognition), [`MOD-SHELL`](#mod-shell) (search, queue policy). Tests: [`TEST-OCR`](#test-ocr).

### <a id="mod-scan"></a>MOD-SCAN (`sheaf-scan`)

```rust
trait ScanBackend: Send + Sync {
    fn devices(&self) -> Result<Vec<Device>>;                      // local scanners only
    fn scan(&self, p: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()>; }
struct WiaBackend;   // cfg(windows): COM on its own short-lived STA thread per call (SCAN-WIA, SCAN-THREADING)
struct FakeBackend;  // generated pages with delays, a jam, an empty feeder, a busy device: tests and development
```

A page leaves a backend as PNG or JPEG bytes (`normalize` converts the bitmap a driver may give and refuses anything that is not an image); the app writes it into the session folder and imports it like any file, so a hostile driver gets no further than a hostile image would ([`SEC-LIMITS`](#sec-limits)). A receiver that went away counts as a cancel.

Depends on: [`MOD-CORE`](#mod-core), [`MOD-IMAGE`](#mod-image). Used by: [`MOD-APP`](#mod-app) (`CMD-SCAN-DEVICES`, `CMD-SCAN-START`: [`CMD-SCAN-START`](#cmd-scan-start)). Decision [`ADR-WIA`](#adr-wia); details [`SCAN-WIA`](#scan-wia), [`SCAN-THREADING`](#scan-threading), [`SCAN-BACKEND`](#scan-backend), [`SCAN-FAILURES`](#scan-failures); risk [`RISK-WIA`](#risk-wia). Tests: [`TEST-SCAN`](#test-scan).

### <a id="mod-jobs"></a>MOD-JOBS (`sheaf-jobs`)

Priority scheduler (visible page first, then selection, then background), worker pool lifecycle (spawn, health-check, restart on crash), cancellation, limits (timeout, RSS cap via Windows Job Object), progress aggregation, backpressure.

```rust
Pool::new(cfg, on_event: impl Fn(JobEvent)) -> Pool;
fn submit(&self, job:JobSpec)->JobId; fn run(&self, job:JobSpec)->Result<Value>; fn cancel(&self, id:JobId);
```

Cancelling a running job kills its worker; so do a timeout, a crash, a protocol violation and exceeding the memory cap. The next job gets a fresh process. The memory cap is enforced by polling the worker's resident size (`/proc` on Linux, the working set on Windows), and on Windows by the operating system itself: each worker sits in its own Job Object with a per-process commit cap of twice the default limit, kill-on-close (so a crashed or killed main process leaves no worker or tesseract behind) and no unhandled-exception dialog ([`SEC-ISOLATE`](#sec-isolate)). A restricted token for the worker is not built yet.

Depends on: [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store). Spawns: [`PROC-WORKER`](#proc-worker). Used by: [`MOD-SHELL`](#mod-shell). Tests: [`TEST-JOBS`](#test-jobs).

### <a id="mod-worker"></a>MOD-WORKER (`sheaf-worker`, binary)

Request handlers speaking [`DATA-WORKERPROTO`](#data-workerproto).

| op | does | modules |
|---|---|---|
| `thumb` | render page at thumbnail size to cache | [`MOD-PDF`](#mod-pdf) |
| `render` | render page for preview | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) |
| `import_probe` | open a file, report pages, encryption, text-layer flags | [`MOD-PDF`](#mod-pdf) |
| `ocr_page` | render, clean, run engine, write cache | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image), [`MOD-OCR`](#mod-ocr) |
| `build` | assemble output PDF from page list, apply overlays | [`MOD-PDF`](#mod-pdf) |
| `tool` | run a TOOL by id | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) |

Depends on: [`MOD-CORE`](#mod-core) (protocol types), [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image), [`MOD-OCR`](#mod-ocr), [`MOD-STORE`](#mod-store). Started as `sheaf-worker --session <dir> --pdfium <dir> [--tesseract <exe> --tessdata <dir>]`; it refuses to start outside a session dir and loads PDFium on the first job that needs it. A further op, `page_text`, extracts the text of a page (optionally of a region of it); `tool` arrives in [`PH-3`](#ph-3).

### <a id="mod-cli"></a>MOD-CLI (`sheaf-cli`)

The argument grammar ([`CLI-GRAMMAR`](#cli-grammar)): arguments in, a command out (window, headless quick action, saved preset, preset list, tray helper, wheel overlay), with usage errors instead of panics. Pure. The two wrappers `--collect` and `--card` that Explorer verbs use ([`FLOW-EXPLORER`](#flow-explorer), [`RISK-EXPLORER-MULTI`](#risk-explorer-multi)). Depends on: [`MOD-CORE`](#mod-core) (the quick-action types).

### <a id="mod-app"></a>MOD-APP (`sheaf-app`)

The main process without a window toolkit ([`ADR-APP-CRATE`](#adr-app-crate)). `App` is the single-threaded core: the workspace, unlocked-source passwords ([`SEC-SECRETS`](#sec-secrets)), the search index, and blocking operations (import, thumbnails, OCR, export, tools). Every long operation is split into a *plan* made under the workspace lock (pages become self-contained worker requests) and an *execution* against `Shared` (worker pool and session dir, safe from any thread), so an export never blocks the tray. `Backend` is the thread-safe command layer the shell calls: it implements [`DOC-IPC`](#doc-ipc), treats every argument as hostile ([`SEC-IPC`](#sec-ipc)), runs long work as app jobs with progress, cancellation and events ([`EVT-JOB`](#evt-job)), serves page pictures, checks the bundled files' hashes at startup ([`SEC-SUPPLY`](#sec-supply)) and shuts down cleanly ([`FLOW-LIFECYCLE`](#flow-lifecycle)). A scan is an app job ([`FLOW-SCAN-LIVE`](#flow-scan-live)): the scanner runs on its own thread, each page is imported as it arrives, recognition is queued for it when asked, and a cancel or a failure keeps the pages already received. `quick` runs the wheel's actions with no window: merge, compress, OCR, split, PDF to pictures, pictures to PDF, rotate, each through the normal import and export so the path policy, limits and worker isolation apply, never overwriting, one bad file not stopping the others ([`FLOW-WHEEL`](#flow-wheel)).

```rust
Backend::start(cfg, emit) -> Arc<Backend>;       // engines discovery, session, settings, hash check
backend.apply(UiOp) / .import(paths) / .export(dto) / .tool_run(id, params, pages) / .ocr_run(pages, policy) ...
trait Emit { fn emit(&self, Event); }            // the shell sends events to the window, tests collect them
```

Depends on: [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store), [`MOD-JOBS`](#mod-jobs), [`MOD-OCR`](#mod-ocr), [`MOD-SCAN`](#mod-scan). Used by: [`MOD-SHELL`](#mod-shell), the harness. Tests: [`TEST-SEC`](#test-sec) (IPC checks), [`TEST-PAGEOPS`](#test-pageops), [`TEST-PROTECT`](#test-protect) and the job flow, through the real worker.

### <a id="mod-shell"></a>MOD-SHELL (`sheaf-shell`)

The `sheaf` executables: `sheaf.exe` (a Windows GUI program) and `sheafc.exe` (the same code built for the console, for scripts and Explorer verbs). Four modes chosen by [`MOD-CLI`](#mod-cli): the main window, headless quick actions (`sheaf run`), the resident tray helper (`sheaf tray`) and the wheel overlay (`sheaf wheel`). The tray helper (Windows only, [`NG-XPLAT`](#ng-xplat)) owns the tray icon and menu (open, start with Windows, quit), the global hotkey from [`DATA-SETTINGS`](#data-settings) (default Ctrl+Alt+Space) and the selection reader: the shell's COM interfaces (`IShellWindows`, the active view's `IFolderView2`) answer which items are selected in the Explorer window in front or on the desktop. That is the only `unsafe` code in the app ([`DOC-AGENT-RULES`](#doc-agent-rules) rule 5), in `selection.rs`, `tray.rs` and nowhere else. Single-instance aggregation (`aggregate.rs`, pure file operations, tested on Linux with threads and in the smoke with real processes) and the right-click menu (`verbs.rs`: the entries are data tested everywhere; the registry writes are Windows only) are in [`PH-6`](#ph-6). Not built yet: the installer ([`PH-7`](#ph-7)), the window icon and window state.

Depends on: [`MOD-UI`](#mod-ui), [`MOD-APP`](#mod-app), [`MOD-CLI`](#mod-cli), [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store).

### <a id="mod-ui"></a>MOD-UI (`sheaf-ui`)

Native views on egui ([`ADR-NATIVE-UI`](#adr-native-ui)): `app` (state, message pump, toolbar, job bar, toasts), `tray` (the page list, virtual), `preview` (zoom, rotate, filters, crop frame, text overlay), `side` (tools drawer, forms, OCR panel), `dialogs` (export, settings, password), `search`, `jobs` (result sentences), `textures` (pictures loaded off the UI thread), `wheel` (the radial menu and its overlay app), `i18n` ([`ADR-I18N`](#adr-i18n): the three language files are embedded), and the pure `geometry` and `format`. Anything that can block runs on a helper thread and returns as a message; the UI thread only draws ([`PERF-BUDGETS`](#perf-budgets)). The views call [`MOD-APP`](#mod-app) directly. Components are specified in [`DOC-UI`](#doc-ui).

## <a id="doc-ipc"></a>7. IPC: commands and events

The command surface is `Backend` in [`MOD-APP`](#mod-app), called in-process by [`MOD-UI`](#mod-ui) (and by the headless modes): commands return `Result<T, AppError>`, run long work as jobs, and validate every argument ([`SEC-IPC`](#sec-ipc)). Paths never come from the UI as free text: they come from native dialogs, drag-drop events, CLI args or the wheel's selection reader ([`SEC-PATHS`](#sec-paths)). As built: [`CMD-IMPORT`](#cmd-import) is the Open dialog plus the drop, `open` and wheel paths, all ending in `Backend::import`; [`CMD-APPLY`](#cmd-apply) takes only the ops a person can ask for (no `Insert`: pages enter through import alone); [`CMD-RENDER`](#cmd-render) and [`CMD-THUMB`](#cmd-thumb) are one call, `Backend::thumb(page, size)`, which returns PNG bytes (sizes up to 256 px are thumbnails, larger ones previews); long commands return a job id at once and report through [`EVT-JOB`](#evt-job). The scan commands ([`PH-5`](#ph-5)) and [`CMD-PRESETS`](#cmd-presets) ([`PH-6`](#ph-6)) are built; without a scanner (or a config folder) they answer `Unsupported`. Extra commands, appended: `snapshot`, `jobs`, `set_output_dir` (the folder is picked in a native dialog) and `register_pick` (a stamp picture, named by token afterwards). Events reach the window as messages on a channel; there is no transport to secure.

| ID | Args → returns | Calls | UI caller |
|---|---|---|---|
| <a id="cmd-import"></a> | `paths[]` (from dialog or drop) → `ImportReport` | [`MOD-JOBS`](#mod-jobs) `import_probe`, [`MOD-CORE`](#mod-core) Insert | [`UI-TOOLBAR`](#ui-toolbar), [`UI-TRAY`](#ui-tray) |
| <a id="cmd-pages"></a> | `range` → `Page[]`, `rev` | [`MOD-CORE`](#mod-core) | [`UI-TRAY`](#ui-tray) |
| <a id="cmd-thumb"></a> | `pageId, size` → PNG bytes (`Backend::thumb`; the page's edit fingerprint, `ver`, keys the texture cache) | [`MOD-JOBS`](#mod-jobs) `thumb` | [`UI-TRAY`](#ui-tray) |
| <a id="cmd-apply"></a> | `Op` → `rev` | [`MOD-CORE`](#mod-core) | [`UI-TRAY`](#ui-tray), [`UI-PREVIEW`](#ui-preview) |
| <a id="cmd-undo"></a> | `undo` / `redo` → `rev` | [`MOD-CORE`](#mod-core) | [`UI-TOOLBAR`](#ui-toolbar) |
| <a id="cmd-render"></a> | `pageId, zoom` → the picture size to ask [`CMD-THUMB`](#cmd-thumb) for | [`MOD-JOBS`](#mod-jobs) `render` | [`UI-PREVIEW`](#ui-preview) |
| <a id="cmd-scan-devices"></a> | none → `Device[]` (asks the system; call it off the UI thread) | [`MOD-SCAN`](#mod-scan) | [`UI-SCANDLG`](#ui-scandlg) |
| <a id="cmd-scan-start"></a> | `ScanProfile` (closed sets as words, plus whether to recognize pages as they arrive) → `JobId`; the job's `detail.pages` is the live count | [`MOD-SCAN`](#mod-scan), [`MOD-JOBS`](#mod-jobs) | [`UI-SCANDLG`](#ui-scandlg) |
| <a id="cmd-scan-cancel"></a> | `JobId` (the same as [`CMD-JOB-CANCEL`](#cmd-job-cancel)) | [`MOD-SCAN`](#mod-scan) | [`UI-JOBBAR`](#ui-jobbar) |
| <a id="cmd-ocr-run"></a> | `pageIds or All, OcrPolicy` → `JobId` | [`MOD-JOBS`](#mod-jobs) `ocr_page` | [`UI-TOOLBAR`](#ui-toolbar), [`UI-TRAY`](#ui-tray), [`UI-OCRPANEL`](#ui-ocrpanel) |
| <a id="cmd-ocr-result"></a> | `pageId` → `OcrResult` (words, conf) | [`MOD-OCR`](#mod-ocr) cache | [`UI-OCRPANEL`](#ui-ocrpanel), [`UI-PREVIEW`](#ui-preview) |
| <a id="cmd-search"></a> | `query` → `Hit[]` | [`MOD-OCR`](#mod-ocr) index | [`UI-SEARCH`](#ui-search) |
| <a id="cmd-tool-run"></a> | `toolId, params, pageIds or All` → `JobId` | [`MOD-JOBS`](#mod-jobs) `tool` | [`UI-DRAWER`](#ui-drawer) |
| <a id="cmd-export"></a> | `ExportSpec` → `JobId` | [`MOD-JOBS`](#mod-jobs) `build` | [`UI-EXPORTDLG`](#ui-exportdlg) |
| <a id="cmd-presets"></a> | list / get / save / delete / run(`name, files`) → `JobId`; a run uses a scratch session of its own, so the open document is untouched | [`MOD-STORE`](#mod-store), [`MOD-APP`](#mod-app) | [`UI-PRESETS`](#ui-presets) |
| <a id="cmd-settings"></a> | get / set | [`MOD-STORE`](#mod-store) | [`UI-SETTINGS`](#ui-settings) |
| <a id="cmd-job-cancel"></a> | `JobId` | [`MOD-JOBS`](#mod-jobs) | [`UI-JOBBAR`](#ui-jobbar) |
| <a id="cmd-reveal"></a> | `JobId` output → opens Explorer at that file (only paths produced by this app) | [`MOD-SHELL`](#mod-shell) | [`UI-JOBBAR`](#ui-jobbar) |
| <a id="cmd-unlock-pw"></a> | `sourceId, password` → ok / wrong (password zeroized after use) | [`MOD-JOBS`](#mod-jobs) `import_probe` | [`UI-DRAWER`](#ui-drawer) |

| ID | Event | Payload |
|---|---|---|
| <a id="evt-pages"></a> | pages added / removed / changed | `rev`, affected ids |
| <a id="evt-page-ready"></a> | thumbnail or OCR state changed for one page | `pageId`, `ocrState` |
| <a id="evt-scan-page"></a> | a page left the scanner | `pageId`, index |
| <a id="evt-job"></a> | progress, done, failed, cancelled | `JobId`, `progress`, result paths |
| <a id="evt-error"></a> | user-presentable error | code, i18n key, no file contents |
| <a id="evt-import"></a> | files came in through a dialog, a drop or the command line | per file: source, name, page ids, `encrypted`, `restricted`, `hasForms`, `isUnc` (the window asks for passwords from this) |
| <a id="evt-engines"></a> | the startup hash check of bundled files finished ([`SEC-SUPPLY`](#sec-supply)) | verified or not, tesseract and font present |

## <a id="doc-tools"></a>8. Tools (iLovePDF-style, all local)

Most tools are views of the same page model ([`ADR-NONDESTRUCTIVE`](#adr-nondestructive)): select pages in [`UI-TRAY`](#ui-tray), pick a tool in [`UI-DRAWER`](#ui-drawer), the tool runs on the selection (or on all pages if none is selected) and writes a new file ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)). The UI shows about ten entries because several tools below share one dialog.

| ID | iLovePDF analogue | Ver | UI | Command | Modules | Test | Phase |
|---|---|---|---|---|---|---|---|
| <a id="tool-merge"></a> | Merge PDF | v1 | import more files, then export | [`CMD-IMPORT`](#cmd-import), [`CMD-EXPORT`](#cmd-export) | [`MOD-PDF`](#mod-pdf) | [`TEST-MERGE`](#test-merge) | [`PH-3`](#ph-3) |
| <a id="tool-split"></a> | Split PDF | v1 | drawer: Split | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image), [`MOD-OCR`](#mod-ocr) | [`TEST-SPLIT`](#test-split) | [`PH-3`](#ph-3) |
| <a id="tool-extract"></a> | Extract pages | v1 | tray: selection, export | [`CMD-EXPORT`](#cmd-export) | [`MOD-PDF`](#mod-pdf) | [`TEST-PAGEOPS`](#test-pageops) | [`PH-3`](#ph-3) |
| <a id="tool-remove"></a> | Remove pages | v1 | tray: Delete key | [`CMD-APPLY`](#cmd-apply) | [`MOD-CORE`](#mod-core) | [`TEST-PAGEOPS`](#test-pageops) | [`PH-1`](#ph-1) |
| <a id="tool-organize"></a> | Organize PDF | v1 | tray: drag, duplicate, insert blank | [`CMD-APPLY`](#cmd-apply) | [`MOD-CORE`](#mod-core) | [`TEST-PAGEOPS`](#test-pageops) | [`PH-1`](#ph-1) |
| <a id="tool-rotate"></a> | Rotate PDF | v1 | tray / preview: R, Shift+R | [`CMD-APPLY`](#cmd-apply) | [`MOD-CORE`](#mod-core), [`MOD-PDF`](#mod-pdf) | [`TEST-PAGEOPS`](#test-pageops) | [`PH-1`](#ph-1) |
| <a id="tool-crop"></a> | Crop PDF | v1 | preview: crop handles | [`CMD-APPLY`](#cmd-apply) | [`MOD-CORE`](#mod-core), [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) | [`TEST-CROP`](#test-crop) | [`PH-3`](#ph-3) |
| <a id="tool-compress"></a> | Compress PDF | v1 | drawer: Compress | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) | [`TEST-COMPRESS`](#test-compress) | [`PH-3`](#ph-3) |
| <a id="tool-img2pdf"></a> | JPG to PDF | v1 | import images, export | [`CMD-IMPORT`](#cmd-import), [`CMD-EXPORT`](#cmd-export) | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) | [`TEST-IMG2PDF`](#test-img2pdf) | [`PH-3`](#ph-3) |
| <a id="tool-pdf2img"></a> | PDF to JPG | v1 | export dialog: Images | [`CMD-EXPORT`](#cmd-export) | [`MOD-PDF`](#mod-pdf) | [`TEST-PDF2IMG`](#test-pdf2img) | [`PH-3`](#ph-3) |
| <a id="tool-watermark"></a> | Add watermark | v1 | drawer: Stamp | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-STAMP`](#test-stamp) | [`PH-3`](#ph-3) |
| <a id="tool-pagenum"></a> | Add page numbers | v1 | drawer: Stamp | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-STAMP`](#test-stamp) | [`PH-3`](#ph-3) |
| <a id="tool-protect"></a> | Protect PDF | v1 | drawer: Lock | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-PROTECT`](#test-protect) | [`PH-3`](#ph-3) |
| <a id="tool-unlock"></a> | Unlock PDF | v1 | drawer: Lock | [`CMD-UNLOCK-PW`](#cmd-unlock-pw), [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-PROTECT`](#test-protect) | [`PH-3`](#ph-3) |
| <a id="tool-ocr"></a> | OCR PDF | v1 | toolbar: OCR | [`CMD-OCR-RUN`](#cmd-ocr-run) | [`MOD-OCR`](#mod-ocr), [`MOD-WORKER`](#mod-worker) | [`TEST-OCR`](#test-ocr) | [`PH-2`](#ph-2) |
| <a id="tool-scan"></a> | Scan to PDF | v1 | toolbar: Scan | [`CMD-SCAN-START`](#cmd-scan-start) | [`MOD-SCAN`](#mod-scan) | [`TEST-SCAN`](#test-scan) | [`PH-5`](#ph-5) |
| <a id="tool-repair"></a> | Repair PDF | v1.1 | drawer: Repair | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-REPAIR`](#test-repair) | [`PH-7`](#ph-7) |
| <a id="tool-metadata"></a> | (not in iLovePDF) view, edit, strip metadata | v1.1 | drawer: Info | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | [`TEST-METADATA`](#test-metadata) | [`PH-7`](#ph-7) |
| <a id="tool-sign-visual"></a> | Sign PDF (image stamp only) | v2 | drawer | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | not yet | none |
| <a id="tool-redact"></a> | Redact PDF | v2 | drawer | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`MOD-PDF`](#mod-pdf) | not yet | none |
| <a id="tool-html2pdf"></a> | HTML to PDF | v2 | n/a | n/a | needs an HTML renderer, which the app deliberately does not contain ([`ADR-NATIVE-UI`](#adr-native-ui)); a decision for v2 | not yet | none |
| <a id="tool-compare"></a> | Compare PDF | v3 | n/a | n/a | n/a | not yet | none |

Later versions: [`TOOL-SIGN-VISUAL`](#tool-sign-visual), [`TOOL-REDACT`](#tool-redact), [`TOOL-HTML2PDF`](#tool-html2pdf), [`TOOL-COMPARE`](#tool-compare). Excluded: Office conversions ([`NG-OFFICE`](#ng-office)), PDF/A (ICC profiles and font-embedding validation make it a project of its own), editing existing text ([`NG-EDITTEXT`](#ng-edittext)).

### Per-tool specs

**[`TOOL-MERGE`](#tool-merge).** Implemented: import reports `has_forms`, and export has a "flatten forms" option that draws field values into the page. Without it the page is copied but the result is no longer a form. Pages from several Sources are exported in workspace order. Method: `import_pages` from each source into one new document, never re-encoding untouched pages ([`PERF-LOSSLESS`](#perf-lossless)). Limits: bookmarks are rebuilt from page labels only if present; form fields (AcroForm) may not survive a pure page import, so document it in the export log and offer "flatten forms" ([`RISK-FORMS-MERGE`](#risk-forms-merge)).

**[`TOOL-SPLIT`](#tool-split).** Input [`DATA-SPLITRULE`](#data-splitrule). Output: many files named by [`DATA-NAMEPATTERN`](#data-namepattern). `BlankSheet` uses [`MOD-IMAGE`](#mod-image) `is_blank` on a low-resolution render and drops the separator page; `TextMatch` uses cached OCR or the PDF text layer of the first page of each candidate group ([`OCR-CACHE`](#ocr-cache)). Preview the split points in [`UI-TRAY`](#ui-tray) before running. `TextMatch` starts a new output at each page whose text matches; `Ranges` takes `1-3,5,8-` and leaves unlisted pages out. User regexes are compiled with the linear-time `regex` crate under size limits, so a hostile pattern cannot hang the app.

**[`TOOL-EXTRACT`](#tool-extract) / [`TOOL-REMOVE`](#tool-remove) / [`TOOL-ORGANIZE`](#tool-organize) / [`TOOL-ROTATE`](#tool-rotate).** Workspace [`DATA-OP`](#data-op)s; no file is written until export. Rotation sets the page `/Rotate` value at build time (pixels untouched).

**[`TOOL-CROP`](#tool-crop).** Security note: setting a CropBox hides content but does not remove it; anyone can read it back. Default for every page: re-encode only the kept pixels so the cropped-away area is really gone (a cropped PDF page is rasterized, so its text outside the crop goes too). The CropBox mode is offered for PDF pages only, keeps them lossless, and is labelled "hide only".

**[`TOOL-COMPRESS`](#tool-compress).** Levels: Low (lossless re-pack, object streams, drop unused objects), Recommended (images to 150 dpi, JPEG q≈70), Extreme (images to 96 dpi, q≈50, gray option). Rules: never upscale, skip images that are already small, never return a bigger file (keep the original and say so), show before/after size. Will not match Ghostscript on every file ([`RISK-COMPRESS`](#risk-compress)). Implemented with lopdf: JPEG and 8-bit flate gray/RGB images are resampled and stored as JPEG; masks, indexed, CMYK, CCITT and JBIG2 images are left as they are. An image's resolution is judged against the width of the page it sits on. The "gray option" for Extreme is not implemented.

**[`TOOL-IMG2PDF`](#tool-img2pdf).** JPEG passes through without re-encoding when no cleanup is requested (8-bit gray or RGB only; EXIF orientation becomes the page's `/Rotate`, and a mirrored orientation forces a re-encode); honors EXIF orientation; page size: fit image / A4 / Letter with margin; multi-page TIFF supported; order follows the tray.

**[`TOOL-PDF2IMG`](#tool-pdf2img).** PNG or JPG at chosen dpi, page range, one file per page into a new folder; alternative mode extracts embedded images unchanged (JPEG data byte for byte, flate images as PNG; other encodings are counted as skipped). The folder is always new: an existing one is never reused.

**[`TOOL-WATERMARK`](#tool-watermark) / [`TOOL-PAGENUM`](#tool-pagenum).** Both are stamps drawn with PDFium text and image objects at build time ([`MOD-PDF`](#mod-pdf)), positioned on the page as displayed whatever its `/Rotate`; the pages themselves stay untouched. Text, size, opacity, rotation, position, page range; page numbers support `{n}`, `{N}`, start offset, skip first. Fonts: bundled Noto Sans (SIL OFL; `NotoSans-Regular.ttf` v2.015 and its `OFL.txt` are vendored and hash-pinned) so Cyrillic and Uzbek letters (ў қ ғ ҳ) render. Image watermark: PNG with alpha.

**[`TOOL-PROTECT`](#tool-protect) / [`TOOL-UNLOCK`](#tool-unlock).** Protect: user and owner passwords, permission flags (print, copy, edit); AES-256, the PDF 2.0 security handler at revision 6 ([`RISK-LOPDF-ENC`](#risk-lopdf-enc)); with no owner password given, a random one is used so the opening password cannot lift the restrictions. Passwords are held in `zeroize`-d buffers, never logged, never written to settings or presets ([`SEC-SECRETS`](#sec-secrets)). Unlocking is done by PDFium: with the password it copies the pages into a new, unprotected file. Unlock: requires the correct password ([`NG-CRACK`](#ng-crack)), or removes owner-only restrictions from a file that opens with an empty user password.

**[`TOOL-OCR`](#tool-ocr).** See [`DOC-OCR`](#doc-ocr). Modes: add missing text (default), redo all, extract text only (`.txt`).

**[`TOOL-SCAN`](#tool-scan).** See [`DOC-SCAN`](#doc-scan) and [`FLOW-SCAN-LIVE`](#flow-scan-live).

**[`TOOL-REPAIR`](#tool-repair).** Best effort: reopen with PDFium, then lopdf; rebuild the xref by re-saving every readable page; report what was lost.

**[`TOOL-METADATA`](#tool-metadata).** Show author, producer, dates, XMP; strip all (default on for exports headed outside the company is a user setting, off by default); set title and author.

**[`TOOL-REDACT`](#tool-redact) (v2 warning).** Drawing black boxes is not redaction. Real redaction must remove the underlying text and image content. Do not ship a box-drawing tool under this name.

## <a id="preset-format"></a>9. Presets (one-click pipelines)

A preset is a saved chain: scan profile, cleanup, OCR policy, split rule, naming, compress, protect, output folder. Stored as JSON in `%APPDATA%\Sheaf\presets\` ([`MOD-STORE`](#mod-store)); never contains passwords ([`SEC-SECRETS`](#sec-secrets)).

```json
{
  "name": "Contract scan",
  "scan":    { "source": "feeder", "duplex": true, "dpi": 300, "color": "gray", "paper": "a4" },
  "cleanup": { "mode": "gray", "deskew": true, "despeckle": true, "dropBlank": true },
  "ocr":     { "mode": "Auto" },
  "split":   { "rule": "BlankSheet" },
  "namePattern": "{date}_{match:Shartnoma\\s*№\\s*(\\S+)}_{counter}",
  "compress": "Recommended",
  "outDir":  "D:\\Scans\\Contracts"
}
```

The file name is the preset's name (`Contract scan.json`); a file that is too large (64 KB), not valid, or has an unknown field or value is skipped in the list and refused on load, never partly used. Closed sets are words (`Off`/`Auto`, `Low`/`Recommended`/`Extreme`, `None`/`EveryN`/`BlankSheet`/`TextMatch`/`Ranges`). `cleanup.mode` is `color`, `gray`, `bw` or empty (leave as is). Run paths: [`UI-PRESETS`](#ui-presets) → [`CMD-PRESETS`](#cmd-presets) → [`FLOW-PRESET`](#flow-preset); from Explorer or a script through [`CLI-GRAMMAR`](#cli-grammar) (`sheaf preset NAME FILES...`). A preset run on existing files skips the scan stage; the scan stage is stored and validated but nothing starts a scan from a preset yet.

## <a id="cli-grammar"></a>10. Command line and Explorer verbs

```
sheaf.exe                                        the window, empty
sheaf.exe open <files...>                        the window, files imported (also what "Open with" passes)
sheaf.exe run <action> <files...> [--out DIR] [--level low|recommended|extreme] [--langs auto|eng+rus]
sheaf.exe <action> <files...> ...                the same without `run`, as an Explorer verb writes it
sheaf.exe tray                                   the resident helper: tray icon, wheel hotkey
sheaf.exe preset <name> <files...> [--out DIR]   a saved preset on files, no window; output as for `run`
sheaf.exe install-verbs | uninstall-verbs        add or remove the Explorer right-click menu (current user, no admin)
--card (with an action or preset)                the progress and result show in a small card at the pointer, not on the console
--collect (with open, an action or preset)       the first process gathers the files of the others Explorer started for one selection, then acts once
sheaf.exe presets                                the names of the saved presets, one per line
sheaf.exe wheel --at X,Y --files LIST            the wheel overlay (started by the helper)
actions: merge compress ocr split pdf2img img2pdf rotate
run prints one output path per line; exit code 0 all done / 2 partly / 1 nothing; never overwrites
scripts and Explorer verbs call sheafc.exe, the console twin of sheaf.exe (a GUI program has no stdout to script and no exit code to wait for)
```

Explorer verbs ([`ADR-EXPLORER-VERBS`](#adr-explorer-verbs)) are written by `sheaf install-verbs` (the installer calls it; so can a person) under `HKCU\Software\Classes\SystemFileAssociations\.<ext>\shell\Sheaf` for `.pdf` and the picture types: one cascading "Sheaf" entry holding Open in Sheaf, the actions that fit the type, and the saved presets (at most twelve; a name with `%` or a quote is left out because the shell would misread it). Each runs `sheaf.exe <action> --card --collect "%1"` (the GUI program, so no console window flashes). `uninstall-verbs` removes exactly those keys. All arguments are untrusted input ([`SEC-PATHS`](#sec-paths)). On Windows 11 these appear under "Show more options". The wheel ([`ADR-WHEEL`](#adr-wheel)) is the same quick actions reached from a hotkey instead of a menu. Grammar: [`MOD-CLI`](#mod-cli); aggregation: [`MOD-CLI`](#mod-cli); flow: [`FLOW-EXPLORER`](#flow-explorer).

## <a id="doc-flows"></a>11. Flows

### <a id="flow-import"></a>FLOW-IMPORT (open files, drag-drop, Explorer)

1. [`UI-TOOLBAR`](#ui-toolbar) dialog or drop → paths arrive at the shell, not as UI strings ([`SEC-PATHS`](#sec-paths)).
2. [`CMD-IMPORT`](#cmd-import) validates each path with [`MOD-STORE`](#mod-store) `allow_input`, size caps ([`SEC-LIMITS`](#sec-limits)).
3. [`MOD-JOBS`](#mod-jobs) sends `import_probe` to a [`PROC-WORKER`](#proc-worker): page count, encryption, per-page text-layer flag.
4. Encrypted and no password: UI asks via [`CMD-UNLOCK-PW`](#cmd-unlock-pw); wrong password never reveals page content.
5. [`MOD-CORE`](#mod-core) applies `Insert` ([`DATA-OP`](#data-op)); [`EVT-PAGES`](#evt-pages) fires; tray requests thumbnails lazily ([`CMD-THUMB`](#cmd-thumb)).
6. If the OCR policy is Auto, pages without text are queued ([`FLOW-OCR-AUTO`](#flow-ocr-auto)).

### <a id="flow-edit"></a>FLOW-EDIT (page operations and undo)

UI gesture → [`CMD-APPLY`](#cmd-apply) → [`MOD-CORE`](#mod-core) `apply` → new `rev` → [`EVT-PAGES`](#evt-pages) → tray invalidates only affected thumbnails (cache key includes `rev` of the page's edits). Undo and redo via [`CMD-UNDO`](#cmd-undo). Edits that change pixels (crop, filter, rotate) also invalidate the page's OCR key, so OCR re-runs automatically when the policy is Auto ([`OCR-CACHE`](#ocr-cache)).

### <a id="flow-scan-live"></a>FLOW-SCAN-LIVE (scan with OCR overlap)

```
UI            SHELL           MOD-SCAN         JOBS/WORKER            TESSERACT
 | start       |                |                 |                      |
 |─CMD-SCAN-START─►| scan(profile)──►| page 1 out      |                      |
 |             |◄──page1────────|                  |                      |
 |◄─EVT-SCAN-PAGE─|  save image Source, Insert   |                      |
 |  thumb      |────────────────────────────────►| ocr_page(1) ─────────►|
 |             |                | page 2 out ...   |◄──── tsv/pdf ────────|
 |◄─EVT-PAGE-READY─ text + confidence for page 1 while page 2 is scanning
 | end of feeder |◄─ done ──────|                  |                      |
 |◄─EVT-JOB─ split / name / export per preset (FLOW-PRESET)
```

Rules: scanning never waits for OCR; OCR jobs for scanned pages have priority below "visible page" and above background; if OCR falls behind, the UI shows the lag but export waits only for pages still running. Cancel stops the feeder loop and discards queued OCR for pages not yet kept ([`CMD-SCAN-CANCEL`](#cmd-scan-cancel)).

### <a id="flow-ocr-auto"></a>FLOW-OCR-AUTO

Trigger: import or scan with [`DATA-OCRPOLICY`](#data-ocrpolicy) `Auto`. Steps are [`OCR-PIPELINE`](#ocr-pipeline). Pages with a text layer are marked `SkippedHasText`.

### <a id="flow-ocr-manual"></a>FLOW-OCR-MANUAL (OCR on command, existing PDFs)

1. The user opens any PDF or image (or right-clicks it in Explorer: `ocr` verb, [`CLI-GRAMMAR`](#cli-grammar)).
2. Chooses scope: selected pages / all pages, and mode: **add missing text** (default), **redo all** (replace existing text layer), **extract text only** (`.txt`, PDF untouched).
3. [`CMD-OCR-RUN`](#cmd-ocr-run) queues the same `ocr_page` jobs as [`FLOW-OCR-AUTO`](#flow-ocr-auto); only the "has text?" check is overridden by the mode.
4. Results appear in [`UI-OCRPANEL`](#ui-ocrpanel); export writes `name.ocr.pdf` ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)) with the text layer overlaid on the original page ([`OCR-TEXTLAYER`](#ocr-textlayer)).
5. Headless: `sheaf.exe ocr file.pdf --headless` runs steps 2 to 4 with no window.

### <a id="flow-tool-run"></a>FLOW-TOOL-RUN (any TOOL)

[`UI-DRAWER`](#ui-drawer) form → [`CMD-TOOL-RUN`](#cmd-tool-run) → [`MOD-JOBS`](#mod-jobs) validates params and paths → worker `tool` op → output written atomically to the output dir ([`MOD-STORE`](#mod-store)) → [`EVT-JOB`](#evt-job) with the result path → [`UI-JOBBAR`](#ui-jobbar) shows "Reveal" ([`CMD-REVEAL`](#cmd-reveal)). Failure leaves no partial file.

### <a id="flow-export"></a>FLOW-EXPORT

[`UI-EXPORTDLG`](#ui-exportdlg) → [`CMD-EXPORT`](#cmd-export) with [`DATA-EXPORTSPEC`](#data-exportspec) → worker `build` (import untouched pages, re-render only pages with pixel edits, overlay OCR text layers from cache) → optional compress ([`TOOL-COMPRESS`](#tool-compress)) → optional protect ([`TOOL-PROTECT`](#tool-protect)) → split and name ([`TOOL-SPLIT`](#tool-split), [`DATA-NAMEPATTERN`](#data-namepattern)) → files written.

### <a id="flow-preset"></a>FLOW-PRESET

[`CMD-PRESETS`](#cmd-presets) run: for each input file, in its own import, the stages in order: cleanup (the filter, applied when the output is built), recognition (pages that have text are skipped; a failure is an error rather than a file that only looks searchable), then the export with split, naming, "leave out blank pages", compression and the output folder (the command line's `--out`, else the preset's, else beside the input). Stages are skipped when their input does not exist (no scanner for files from Explorer). A locked file is reported, not guessed at; failure of one file does not abort the others; the job report lists each result; nothing is overwritten ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)).

### <a id="flow-explorer"></a>FLOW-EXPLORER

Verb → `sheaf.exe <cmd> --card --collect "%1"` (one process per selected file) → every process drops its path into a private folder and tries to take an exclusive lock file; the winner is the *leader*: it waits until no new path has arrived for 500 ms (at most 5 s), collects them all in name order and acts once, while the others wait until the leader has taken their list and leave; a process whose list outlived the leader leads its own batch, so a late arrival is not lost, and a lock left by a crashed leader goes stale after a minute. The leader runs the action in a small card at the pointer: progress, then what was made, what failed and why, "Show in folder" ([`FLOW-WHEEL`](#flow-wheel) step 5). `open` with `--collect` opens one window with every file ([`FLOW-IMPORT`](#flow-import)).

### <a id="flow-wheel"></a>FLOW-WHEEL (the action wheel)

1. `sheaf tray` is running (started by the person or by Windows). Its hotkey ([`DATA-SETTINGS`](#data-settings)) is pressed while Explorer or the desktop is in front.
2. The helper reads the selected paths from that window through the shell ([`MOD-SHELL`](#mod-shell)), the cursor position and the screen scale, writes the paths to a private list file and starts `sheaf wheel --at X,Y --files LIST`. A wheel already up is not started twice.
3. The overlay reads and deletes the list, classifies each file by extension ([`ADR-WHEEL`](#adr-wheel)) and draws eight fixed slots around the cursor: Merge, Compress, OCR, Split, PDF to pictures, Pictures to PDF, Rotate, Open in Sheaf. Slots that make no sense for these files are dimmed and do nothing; the hub says how many files. Click, a digit key, or Esc; clicking elsewhere or leaving it alone closes it.
4. A quick action runs in the overlay process ([`MOD-APP`](#mod-app) `quick`): each file is imported under the path policy, the output is built by the normal export and published beside the first input under a new name ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)); a locked file is reported, not guessed at; one failing file does not stop the rest.
5. The window shrinks to a card: progress, then what was made, what failed and why (the same error messages as everywhere, [`FLOW-ERROR`](#flow-error)), with "Show in folder" and "Open result in Sheaf". It closes by itself after a while unless the pointer is on it.

### <a id="flow-lifecycle"></a>FLOW-LIFECYCLE

Startup: verify bundled binary hashes ([`SEC-SUPPLY`](#sec-supply)), sweep stale session dirs, start worker pool lazily, show window before anything is imported. Shutdown: cancel jobs, kill workers and tesseract, delete the session dir. Crash recovery: next start sweeps leftovers; no document data is kept elsewhere.

### <a id="flow-error"></a>FLOW-ERROR

Every error has a code and an i18n key ([`EVT-ERROR`](#evt-error)); no file contents or paths with personal data in messages sent to the UI beyond the file name. Worker crash or timeout: the job fails, the worker is restarted, the page is marked failed, the app continues. Out of disk space: stop and report before writing partial output.

## <a id="doc-ocr"></a>12. OCR subsystem

### <a id="ocr-pipeline"></a>OCR-PIPELINE (per page, runs in [`PROC-WORKER`](#proc-worker))

1. **Has text?** `has_text_layer` ([`MOD-PDF`](#mod-pdf)). Skip unless mode is redo ([`FLOW-OCR-MANUAL`](#flow-ocr-manual)).
2. **Render and clean:** render at 300 dpi (or the scan's dpi), apply [`DATA-FILTER`](#data-filter) ([`MOD-IMAGE`](#mod-image)); this cleaned image is what the engine sees and what the cache key hashes ([`DATA-CACHEKEY`](#data-cachekey)).
3. **Cache check** ([`OCR-CACHE`](#ocr-cache)). Hit means done.
4. **Detect:** orientation and script via Tesseract OSD ([`OCR-TESS-CLI`](#ocr-tess-cli)). If the page is sideways or upside down, the image is turned before recognition and the main process records the same turn on the Page as a Rotate [`DATA-OP`](#data-op), which can be undone. Skipped when languages are fixed.
5. **Choose languages** ([`OCR-LANGS`](#ocr-langs)).
6. **Recognize:** TSV (words, boxes, confidence).
7. **Store:** cache entry; set [`DATA-OCRSTATE`](#data-ocrstate) `Done`; [`EVT-PAGE-READY`](#evt-page-ready).
8. **Index:** words added to the search index ([`CMD-SEARCH`](#cmd-search)).

### <a id="ocr-langs"></a>OCR-LANGS

Bundled models: `eng`, `rus`, `uzb` (Latin), `uzb_cyrl`, `osd`. Default chooser: script Latin → `uzb+eng`; Cyrillic → `uzb_cyrl+rus`; uncertain → all four (slower). Order matters: with `rus` first, Tesseract flattened the Uzbek letters on the synthetic fixtures (ў→у, қ→к; 5.2% character error), while `uzb_cyrl+rus` read Uzbek Cyrillic at 0% and Russian at 0.7%. Preset or setting can fix languages and skip detection (faster, [`PERF-BUDGETS`](#perf-budgets)). The mapping is a hypothesis to validate on real documents ([`TEST-OCR`](#test-ocr)). Quality `Fast` uses `tessdata_fast`; `Accurate` uses `tessdata_best` if installed (optional download at build time only, [`ADR-OFFLINE-ASSETS`](#adr-offline-assets)).

### <a id="ocr-tess-cli"></a>OCR-TESS-CLI

```
tesseract <img> stdout --tessdata-dir <dir> --psm 0 -l osd            # OSD: orientation, script
tesseract <img> <out>  --tessdata-dir <dir> -l uzb+eng --oem 1 --psm 3 --dpi 300 \
          -c tessedit_create_tsv=1 -c tessedit_create_pdf=1 -c textonly_pdf=1
                                                                      # one run: words, boxes, conf + text-only PDF
```

The input image is an uncompressed PPM, and file arguments are relative to the working directory, so the engine never sees a Windows verbatim path. An OSD failure (too little text) means "unknown", not an error; orientation and script are acted on only above a confidence floor. Always invoked with an argument array (never a shell string), a timeout, a working dir inside the session dir, and `OMP_THREAD_LIMIT=1` so parallelism comes from [`MOD-JOBS`](#mod-jobs), not inside Tesseract (VERIFY best setting by benchmark, [`PERF-BUDGETS`](#perf-budgets)).

### <a id="ocr-cache"></a>OCR-CACHE

Keyed by [`DATA-CACHEKEY`](#data-cachekey); survives edits that do not change pixels; invalidated automatically when crop, rotate, or filter change. Lives in the session dir, deleted on exit ([`FLOW-LIFECYCLE`](#flow-lifecycle)). Lets "Save as searchable PDF" mostly merge cached results.

### <a id="ocr-textlayer"></a>OCR-TEXTLAYER

Goal: invisible, selectable, searchable text aligned with each word, overlaid on the original page so the scan stays lossless.

- Implemented: Tesseract writes a text-only PDF of the page (`textonly_pdf=1`: invisible text in render mode 3, its glyphless font and `ToUnicode` map, no image). At export, [`MOD-PDF`](#mod-pdf) imports that page as a form XObject with PDFium and places it over the original page, with a transform that accounts for the page's own `/Rotate` and for the orientation fix applied before recognition. The original page is copied untouched, so scans stay lossless, and Cyrillic and Uzbek text extract correctly ([`RISK-OVERLAY-FONT`](#risk-overlay-font)). Tested by extracting text from regions of the exported page.
- "Redo all" on a page that already has text rasterizes that page, so the old text layer does not survive under the new one.
- Not needed so far: the fallback of importing Tesseract's full `pdf` output (which re-embeds the image).

### <a id="ocr-engines"></a>OCR-ENGINES

Only Tesseract in v1 ([`ADR-TESS-SIDECAR`](#adr-tess-sidecar)). The trait leaves room for an optional "Accurate (GPU)" engine pointing at a user-run local model server. Constraints if added: off by default, address configured explicitly, localhost default, document pixels leave the machine only after an explicit confirmation, and any model requiring `trust_remote_code` runs outside this app ([`ADR-NO-VLM-DEFAULT`](#adr-no-vlm-default)).

Evidence behind [`ADR-NO-VLM-DEFAULT`](#adr-no-vlm-default) (checked 2026-11-13, see [`DOC-REFS`](#doc-refs)): Unlimited-OCR is a 3B-parameter, MIT-licensed model tested on NVIDIA GPUs (CUDA), with no CPU guidance, no stated Uzbek or Cyrillic support, and a `trust_remote_code` requirement. In one third-party Uzbek benchmark (published on a competing model's card, small samples, wide confidence intervals) it beat Tesseract 5 on Uzbek Latin, Russian, English and phone photos, but was worse on Uzbek Cyrillic (book scans 8.37% vs 3.61% character error rate; scanner pages 15.99% vs 1.22%) and read ҳ қ ғ ў correctly only about half the time. The same benchmark reports roughly 58 s per page on a T4 GPU versus roughly 7.5 s per page for Tesseract on CPU. Re-check with [`TEST-OCR`](#test-ocr) on real files before changing the default.

### <a id="ocr-conf-ui"></a>OCR-CONF-UI

Per-word confidence from TSV drives [`UI-OCRPANEL`](#ui-ocrpanel): low-confidence words highlighted, "next doubtful word" key, inline correction edits the stored text layer (not the pixels).

## <a id="doc-scan"></a>13. Scanner subsystem

### <a id="scan-wia"></a>SCAN-WIA

Windows Image Acquisition through the `windows` crate (COM). Steps: enumerate devices; connect; set properties from [`DATA-SCANPROFILE`](#data-scanprofile) (document handling: feeder/flatbed/duplex, x/y resolution, data type, page size; exact property IDs from `wiadef.h`, VERIFY on target driver); transfer in a loop, one image per page, until the driver reports the feeder empty; decode each image and emit `ScannedPage`. Each page is written as an image Source in the session dir immediately ([`FLOW-SCAN-LIVE`](#flow-scan-live)).

### <a id="scan-threading"></a>SCAN-THREADING

COM runs on one dedicated single-threaded-apartment thread owned by [`MOD-SCAN`](#mod-scan); pages cross to the rest of the app through a channel; cancellation is checked between transfers and, where the driver allows, aborts the current one.

### <a id="scan-backend"></a>SCAN-BACKEND

`ScanBackend` trait ([`MOD-SCAN`](#mod-scan)) lets everything above the scanner be tested with `FakeBackend` on Linux ([`TEST-MATRIX`](#test-matrix)). Planned backends after v1: TWAIN, ESCL (network scanners; network access must be explicit and opt-in because of [`SEC-NONET`](#sec-nonet)).

### <a id="scan-failures"></a>SCAN-FAILURES

Driver errors, paper jams, empty feeder at start, scanner busy, user cancel. They map to `DeviceError`, `NoPaper` and `DeviceBusy` and to `Cancelled` ([`EVT-ERROR`](#evt-error)); pages already received are kept and the job bar says how many. A feeder that runs dry after at least one page is the normal end of a batch, not an error. One scan keeps at most 500 pages ([`SEC-LIMITS`](#sec-limits)). [`RISK-WIA`](#risk-wia) covers driver variance.

## <a id="doc-security"></a>14. Security

Threat model: (a) hostile PDFs and images (the main attack surface), (b) other software on the same PC probing the app, (c) a compromised UI layer, (d) supply chain, (e) data left behind on disk. Out of scope: an attacker with admin rights on the machine.

| ID | Control | Boundary | Implementation | Verified by |
|---|---|---|---|---|
| <a id="sec-nonet"></a> | No listener, no outbound connections | all | No HTTP server ([`ADR-NOHTTP`](#adr-nohttp)) and no web view: the UI is native code in the main process ([`ADR-NATIVE-UI`](#adr-native-ui)), so there is no browser engine to reach out and no page to navigate. No network code is compiled in at all: `cargo-deny` bans the HTTP, TLS, socket-server and async-runtime crates by name, and fails CI if one appears in the dependency tree; no updater, no telemetry; any future network feature ([`OCR-ENGINES`](#ocr-engines), ESCL) is opt-in, logged, and a spec change first | [`TEST-SEC`](#test-sec) (port scan on Linux and Windows, the ban list) |
| <a id="sec-ipc"></a> | Validate every request, wherever it comes from | [`SEC-TB1`](#sec-tb1) | The window and the wheel are trusted code in the main process, but every action goes through the `Backend` command layer, which treats its arguments as hostile anyway (types, ranges, id existence, closed enums, unknown fields refused for tool parameters, no `Insert` of pages, no free-text paths, picked files named by token), so the command line, the wheel and the window share one set of rules; the layer is tested with bad ids, huge ranges and bad paths | [`TEST-SEC`](#test-sec) |
| <a id="sec-isolate"></a> | Hostile files only touch workers | [`SEC-TB2`](#sec-tb2), [`SEC-TB3`](#sec-tb3) | [`PROC-WORKER`](#proc-worker) per job class: a Windows Job Object per worker (memory cap, kill-on-close, tesseract inside it too), timeout, no handle inheritance; main process never parses documents ([`ADR-WORKER-PROCS`](#adr-worker-procs)). Every executable starts by restricting DLL loading to the app folder and System32 (never the current directory, which Explorer verbs and the wheel set to a document's folder) and refusing images from network shares; PDFium and Tesseract are loaded by absolute path. Not built yet: a restricted token or low integrity level for the worker | [`TEST-JOBS`](#test-jobs), [`TEST-SEC`](#test-sec) |
| <a id="sec-paths"></a> | Path policy | [`SEC-TB4`](#sec-tb4) | Inputs only from dialogs, drops, or CLI args, canonicalized; reject device paths and alternate data streams; outputs only into the chosen output dir or beside the source with a new name ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)); [`CMD-REVEAL`](#cmd-reveal) only for files this app produced; files on network shares (UNC) are a setting, on by default and flagged to the person each time because reading one makes Windows contact that server; turned off, they are refused in every mode, window, wheel and command line | [`TEST-SEC`](#test-sec) (traversal, reserved names, long paths) |
| <a id="sec-limits"></a> | Resource limits | [`SEC-TB2`](#sec-tb2) | Max file size, max pages, max pixel dimensions per page (zip-bomb style images), per-job timeout, per-worker RSS cap; reject early with a clear message ([`RISK-LARGE-SCANS`](#risk-large-scans)) | [`TEST-SEC`](#test-sec) |
| <a id="sec-secrets"></a> | Passwords | [`SEC-TB1`](#sec-tb1) | Held in `zeroize` buffers in the main process for the session, because every job that opens a locked file needs the password again; sent to the worker inside that job's request; never logged, never in settings, presets, history or any file. The password fields of the window are `Zeroizing` strings, cleared the moment a request is sent. Copies made while a request is serialized, and inside the text widget as it is edited, are short-lived but not wiped | [`TEST-PROTECT`](#test-protect), code review checklist |
| <a id="sec-temp"></a> | Temp data | [`SEC-TB4`](#sec-tb4) | Session dir with per-user ACL; everything deleted on exit; stale sweep on start. No claim of secure wipe (not reliable on SSDs) | [`TEST-SEC`](#test-sec) |
| <a id="sec-log"></a> | Logging | all | No log of document activity at all. A panic leaves a note (version, thread, the place in our own source, never the panic message) and a plain message instead of a vanished window; verbose logging, if it ever exists, is opt-in, with no document text, no passwords, no full paths beyond file name | review, `crash.rs` test |
| <a id="sec-supply"></a> | Supply chain | build, start | `cargo-deny` + `cargo-audit` in CI; pinned versions; vendored binaries (pdfium, tesseract, tessdata, fonts) have SHA-256 in `vendor/SHA256SUMS`, verified at build and again before any worker starts, in every mode: the window (in the background), the wheel and `sheaf run` ([`FLOW-LIFECYCLE`](#flow-lifecycle)); fails closed when the pinned hashes are missing; no build scripts that download at runtime | CI |
| <a id="sec-sign"></a> | Code signing | release | Sign installer and exes if a certificate is obtained; otherwise SmartScreen will warn ([`RISK-SMARTSCREEN`](#risk-smartscreen)); bundled executables can also trip antivirus ([`RISK-AV`](#risk-av)) | release checklist |
| <a id="sec-honesty"></a> | No misleading security UI | UI | "Crop" says "hide only" when content remains ([`TOOL-CROP`](#tool-crop)); no "redact" tool until real redaction exists ([`TOOL-REDACT`](#tool-redact)); encryption strength shown as what was actually applied | [`TEST-PROTECT`](#test-protect) |

## <a id="doc-perf"></a>15. Performance

Budgets are targets to be measured, not claims. Each has a benchmark in `tools/bench` ([`TEST-FIXTURES`](#test-fixtures)).

| ID | Budget (target) | Technique |
|---|---|---|
| <a id="perf-budgets"></a> | Window visible in under 1 s cold; first 20 thumbnails within 500 ms of opening a 100-page PDF; UI thread never blocked over 50 ms; idle memory under 150 MB; 1000-page tray scrolls smoothly; OCR keeps pace with a feeder at the target dpi on a mid-range CPU (measure; scan rate and OCR per-page time decide) | [`PERF-LOSSLESS`](#perf-lossless), [`PERF-LAZY`](#perf-lazy), [`PERF-PARALLEL`](#perf-parallel), [`PERF-OCR`](#perf-ocr), [`PERF-IO`](#perf-io) |
| <a id="perf-lossless"></a> | Untouched pages are copied, not re-encoded: merge or split of 100 pages in a few seconds | PDFium page import (`PdfPages::copy_page_from_document`, [`ADR-PDFIUM-LOPDF`](#adr-pdfium-lopdf)); re-render only pages with pixel edits. Measured in PH-1: 100 pages export in about 20 ms, 1000 pages in about 160 ms (`sheaf-harness bench`, synthetic text pages) |
| <a id="perf-lazy"></a> | Render only what is visible | virtualized tray ([`UI-TRAY`](#ui-tray)), thumbnails requested by viewport, disk cache ([`MOD-STORE`](#mod-store)), LRU memory cache |
| <a id="perf-parallel"></a> | Use the cores without stalling the UI | N worker processes (default cores minus 1, [`DATA-SETTINGS`](#data-settings)), because PDFium is not thread-safe ([`ADR-WORKER-PROCS`](#adr-worker-procs), [`RISK-PDFIUM-THREADS`](#risk-pdfium-threads)); priority queue ([`MOD-JOBS`](#mod-jobs)) |
| <a id="perf-ocr"></a> | OCR cost control | cache ([`OCR-CACHE`](#ocr-cache)), skip pages with text, fixed-language presets skip detection, Fast model default, one Tesseract thread per job, measure process-start cost before considering library binding ([`ADR-TESS-SIDECAR`](#adr-tess-sidecar)) |
| <a id="perf-io"></a> | Cheap I/O | atomic writes, pictures passed as bytes in-process ([`CMD-THUMB`](#cmd-thumb)), streaming progress as messages rather than polling |

## <a id="doc-ui"></a>16. UI

Single window. No tabs, no nested modals deeper than one level. Labels come from i18n keys ([`ADR-I18N`](#adr-i18n)).

```
┌ UI-TOOLBAR: Scan · Open · OCR · Export · Undo/Redo · Presets ▾ · Search ────────────┐
│ UI-TRAY (left, virtual)   │ UI-PREVIEW (center)                │ UI-DRAWER / UI-OCRPANEL (right) │
│  thumbnails, badges       │  page, zoom, crop, text overlay    │  tools grid, or OCR text + conf │
├───────────────────────────┴────────────────────────────────────┴─────────────────────────────────┤
│ UI-JOBBAR: progress, cancel, reveal result                                                       │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
```

| ID | Component | Behavior | Commands |
|---|---|---|---|
| <a id="ui-toolbar"></a> | Top bar | Scan, Open, OCR, Export, Undo/Redo, Presets menu, Search box | [`CMD-IMPORT`](#cmd-import), [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-UNDO`](#cmd-undo), [`CMD-PRESETS`](#cmd-presets) |
| <a id="ui-tray"></a> | Page tray | Virtual list; multi-select (click, Shift, Ctrl, rubber-band); drag to reorder; badges: OCR state, text present, blank suspect; context menu: rotate, delete, duplicate, OCR, extract | [`CMD-PAGES`](#cmd-pages), [`CMD-THUMB`](#cmd-thumb), [`CMD-APPLY`](#cmd-apply) |
| <a id="ui-preview"></a> | Preview and editor | Zoom/pan, rotate, crop handles, filter toggles (gray, B/W, deskew), text overlay toggle with confidence colors | [`CMD-RENDER`](#cmd-render), [`CMD-APPLY`](#cmd-apply), [`CMD-OCR-RESULT`](#cmd-ocr-result) |
| <a id="ui-drawer"></a> | Tools drawer | Grid of [`DOC-TOOLS`](#doc-tools) entries, searchable, each opens a small form; password prompt for locked files | [`CMD-TOOL-RUN`](#cmd-tool-run), [`CMD-UNLOCK-PW`](#cmd-unlock-pw) |
| <a id="ui-ocrpanel"></a> | OCR panel | Recognized text for the current page, doubtful words, "next doubtful" key ([`OCR-CONF-UI`](#ocr-conf-ui)), run/redo OCR for selection | [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-OCR-RESULT`](#cmd-ocr-result) |
| <a id="ui-scandlg"></a> | Scan dialog | Device (with refresh), source (flatbed/feeder), two-sided, dpi, colour, paper, recognize while scanning; remembers its choices for the run. Starting closes it: the pages appear in the tray and the job bar shows the live page count with Cancel | [`CMD-SCAN-DEVICES`](#cmd-scan-devices), [`CMD-SCAN-START`](#cmd-scan-start) |
| <a id="ui-exportdlg"></a> | Export dialog | Format, page scope, OCR mode, split rule, name pattern, compress, protect, output folder | [`CMD-EXPORT`](#cmd-export) |
| <a id="ui-presets"></a> | Presets | Pick, edit, save-as; "Run on files…" | [`CMD-PRESETS`](#cmd-presets) |
| <a id="ui-settings"></a> | Settings | Language, output dir, workers, OCR quality, theme | [`CMD-SETTINGS`](#cmd-settings) |
| <a id="ui-search"></a> | Search | Full-text over text layers and OCR cache; results jump to page | [`CMD-SEARCH`](#cmd-search) |
| <a id="ui-jobbar"></a> | Job bar | One row per running job, cancel, "Reveal in Explorer" | [`CMD-JOB-CANCEL`](#cmd-job-cancel), [`CMD-REVEAL`](#cmd-reveal), [`EVT-JOB`](#evt-job) |
| <a id="ui-shortcuts"></a> | Shortcuts | Ctrl+O open, Ctrl+Shift+S scan, Ctrl+E export, Ctrl+Z/Y undo/redo, R / Shift+R rotate, Del delete, Ctrl+D duplicate, F OCR selection, Ctrl+F search, Space preview | [`CMD-APPLY`](#cmd-apply), [`CMD-OCR-RUN`](#cmd-ocr-run) |
| <a id="ui-wheel"></a> | Action wheel | A radial overlay at the cursor; eight fixed slots, dimmed when they do not fit the selected files; click or a digit chooses; the same window then shows progress and the result ([`FLOW-WHEEL`](#flow-wheel)) | [`MOD-APP`](#mod-app) `quick` |

Keyboard map: [`UI-SHORTCUTS`](#ui-shortcuts). Accessibility: full keyboard path for every action, visible focus, no information by color alone (badges also carry text/icons), respects OS text scale and dark mode. On Windows the window exposes itself to screen readers through UI Automation (AccessKit); on other systems that part is not built, because its Linux backend would bring in a D-Bus stack with an async runtime ([`SEC-NONET`](#sec-nonet)).

As built in [`PH-4`](#ph-4): the tray reorders by pointer ([`ADR-UI-REORDER`](#adr-ui-reorder)) and its badges carry words, not only colors; the preview offers zoom, rotation, Color/Gray/B-W, straighten and clean, crop with handles (arrow keys move the frame, Shift resizes) and the recognized-text overlay with doubtful words marked by outline and a dashed underline and listed in the OCR panel; the drawer opens a form per tool and the export dialog states "hide only" in words when selected ([`SEC-HONESTY`](#sec-honesty)); locked files ask for their password in a dialog, and the page stays hidden until it is right. The scan dialog ([`UI-SCANDLG`](#ui-scandlg)) and the presets dialog ([`UI-PRESETS`](#ui-presets)) are built. Buttons carry drawn icons and words; the bundled font has no pictograms. Not built yet: inline correction of recognized text ([`OCR-CONF-UI`](#ocr-conf-ui)), the "blank suspect" tray badge, the split preview in the tray ([`TOOL-SPLIT`](#tool-split)), a window icon, and a shortcuts help screen. The Uzbek and Russian texts are first drafts by the builder and need a native reader.

## <a id="doc-tests"></a>17. Testing

### <a id="test-matrix"></a>What runs where

| Area | Linux CI | Windows CI | Manual (needs hardware) |
|---|---|---|---|
| [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store), [`MOD-IMAGE`](#mod-image), [`MOD-JOBS`](#mod-jobs) | yes | yes | |
| [`MOD-PDF`](#mod-pdf), [`MOD-OCR`](#mod-ocr), [`MOD-WORKER`](#mod-worker) | yes (pdfium and tesseract binaries per OS) | yes | |
| [`MOD-SCAN`](#mod-scan) with `FakeBackend` | yes | yes | |
| [`MOD-SCAN`](#mod-scan) with `WiaBackend` | no | build only | real scanner ([`RISK-WIA`](#risk-wia)) |
| [`MOD-APP`](#mod-app) (the command layer, quick actions) | yes | yes | |
| [`MOD-UI`](#mod-ui) | logic and language-file tests; the window and the wheel drawn under a virtual display with software GL ([`TEST-UI`](#test-ui)) | build, smoke | visual check |
| [`MOD-SHELL`](#mod-shell) | modes, headless run; the Windows-only parts type-check (`--target x86_64-pc-windows-msvc`) | build, smoke (window, worker, no listening port, tray helper starts) | the hotkey, Explorer and desktop selection, the overlay at the cursor, on a clean VM ([`RISK-WIN-SHELL`](#risk-win-shell)) |
| Installer, Explorer verbs, SmartScreen | no | build | clean Windows VM |

### <a id="test-fixtures"></a>Fixtures (`fixtures/`)

Born-digital PDF with text; scanned PDF without text (Uzbek Latin, Uzbek Cyrillic, Russian, English); phone photo of a page; skewed scan; blank-separator batch; encrypted PDF (user and owner password); corrupted PDF; 1000-page PDF; huge-dimension page; rotated pages; PDF with forms; multipage TIFF; images with EXIF rotation. Generated or synthetic where possible; real documents are never committed.

### Test groups

| ID | Checks |
|---|---|
| <a id="test-ops"></a> | Each [`DATA-OP`](#data-op) applied to random workspaces keeps invariants (unique ids, valid indices) |
| <a id="test-undo"></a> | `apply(op); undo()` equals original; redo equals applied; depth cap respected |
| <a id="test-image"></a> | Deskew within tolerance on rotated fixtures, blank detection precision/recall, EXIF orientation |
| <a id="test-ocr"></a> | Character error rate per fixture group stays under agreed thresholds (provisional: 2% English and Russian, 4% Uzbek, on synthetic 300 dpi pages; measured 0 to 0.7%); skip logic; cache hit rate; language chooser on mixed fixtures; compare engines before changing defaults ([`OCR-ENGINES`](#ocr-engines)); accuracy risk [`RISK-OCR-ACCURACY`](#risk-ocr-accuracy) |
| <a id="test-jobs"></a> | Worker crash, hang, and memory-limit recovery; cancellation; priority ordering |
| <a id="test-sec"></a> | No listening ports; non-local requests blocked; command-layer checks (bad ids, huge ranges); hostile inputs through the real import and render path (48 deterministic mutations of a valid PDF, crafted page trees and page sizes, PNG decompression bombs): an answer within a time bound, never a crash or hang, and the next ordinary file still opens; the Job Object kills what is in it; a changed bundled file stops every mode; path traversal, device names, ADS; oversized inputs rejected; session dir removed on exit |
| <a id="test-ui"></a> | The three language files have the same keys, placeholders and plural forms and every key the code asks for exists; crop geometry round-trips through every rotation; job result sentences; and `tools/smoke/gui-linux.sh` starts the real window with a file and the real wheel on two PDFs under Xvfb, checks that real content was drawn and that a headless `run` writes its output |
| <a id="test-wheel"></a> | The wheel: sector hit-testing, which slots apply to which files, the file list read once and removed; quick actions through the real worker (merge order and naming, compress, split, rotate, pictures, OCR, bad inputs, locked files, no overwrite); the argument grammar including every bad input; and the Windows smoke script ([`RISK-WIN-SHELL`](#risk-win-shell)) |
| <a id="test-merge"></a> | Page count and order; untouched pages byte-identical objects where possible; forms behavior documented |
| <a id="test-split"></a> | Each [`DATA-SPLITRULE`](#data-splitrule); names from [`DATA-NAMEPATTERN`](#data-namepattern); no page lost or duplicated |
| <a id="test-pageops"></a> | Extract, remove, reorder, rotate produce expected page sequence and `/Rotate` |
| <a id="test-crop"></a> | Image page crop removes pixels (cropped region not recoverable); "hide only" mode labeled |
| <a id="test-compress"></a> | Output never larger; text still extractable; dpi respected; quality floor |
| <a id="test-img2pdf"></a> | JPEG passthrough byte-identical stream; EXIF orientation; TIFF pages |
| <a id="test-pdf2img"></a> | Page count, dpi, naming |
| <a id="test-stamp"></a> | Text and page numbers present at the right place; Cyrillic and Uzbek letters extract correctly |
| <a id="test-protect"></a> | Encrypted output opens only with the password; reported strength equals actual; wrong password fails cleanly; no password in logs |
| <a id="test-scan"></a> | Fake feeder through the command layer: page order, flatbed and two-sided counts, cancel mid-batch keeping the pages, recognition queued before the scan ends, error mapping, refused requests; the Linux GUI smoke drives the dialog and reads "3 pages" off the job bar; on Windows CI the WIA backend lists devices with no scanner attached |
| <a id="test-preset"></a> | Presets: the example parses and round-trips, every bad value and unknown field is refused, hostile names stay in the folder, damaged or oversized files are skipped; runs through the real worker: output beside the input or in the chosen folder (`--out` first), blank pages dropped, split at blank sheets, nothing overwritten, a locked or missing file reported while the others run, cancel; as a job on a scratch session that leaves the open document alone; `sheaf presets` / `sheaf preset` in the Linux and Windows smoke; the dialog lists a saved preset |
| <a id="test-explorer"></a> | Aggregation: a lone launch leads; twelve staggered launches make one batch with one leader and all twelve files in name order (repeated runs, after a race was found and fixed: a process whose list was already taken must not lead); a late arrival becomes a new batch; kinds of work do not mix; a stale lock is taken over; a stuck leader does not trap a follower; odd keys and paths cannot escape. Verbs: the entries for each file type, the commands (parsed back through the grammar), presets and unsafe names, everything under the Sheaf key and so removable, labels in the language. Linux smoke: three real `merge --collect` processes print one path; a `--card` run reads Done off the screen. Windows smoke: the same three processes, and install, check and remove the menu in the registry |
| <a id="test-repair"></a> | Corrupted fixtures: recovered page count reported honestly |
| <a id="test-metadata"></a> | Strip removes Info and XMP; set writes both |

## <a id="doc-build"></a>18. Build, packaging, CI

- Pinned: PDFium build 8125 from pdfium-binaries (the newest the `pdfium-render` 0.9.5 binding supports), `tessdata_fast` 4.1.0 plus `pdf.ttf`, Tesseract 5.5.0 for Windows (the UB Mannheim installer, unpacked with 7-Zip at build time), Noto Sans Regular 2.015 with its licence. Linux CI tests against the distribution's Tesseract (5.3.4 on Ubuntu 24.04).
- Rust stable, Cargo workspace ([`DOC-LAYOUT`](#doc-layout)); `clippy -D warnings`, `rustfmt`, `cargo-deny`, `cargo-audit` ([`SEC-SUPPLY`](#sec-supply)).
- CI vendoring step: `tools/fetch-vendor` downloads pinned releases (pdfium binaries, Tesseract Windows build, tessdata, Noto fonts), verifies SHA-256 against `vendor/SHA256SUMS`, fails on mismatch ([`ADR-OFFLINE-ASSETS`](#adr-offline-assets)). The licences of everything bundled go into `THIRD_PARTY_LICENSES`, generated by `tools/licenses` from the lock file (the crates `cargo tree` says the Windows build compiles in, with their licence files; crates that ship none are listed with their declared licence and repository) and the vendor folder, and checked in CI; the licence texts of the libraries inside the Tesseract installer are still to be added from the unpacked build.
- The installer (NSIS or MSI, [`PH-7`](#ph-7)) writes Explorer verbs ([`ADR-EXPLORER-VERBS`](#adr-explorer-verbs), [`CLI-GRAMMAR`](#cli-grammar)) per user and offers "start with Windows" for the tray helper; there is no runtime to bundle besides the app itself ([`ADR-NATIVE-UI`](#adr-native-ui)), so the former WebView2 question ([`RISK-WEBVIEW2`](#risk-webview2)) no longer applies.
- Windows and Linux runners build, lint and test the whole workspace ([`TEST-MATRIX`](#test-matrix)); egui compiles on Linux, so the views are checked there too. Two more jobs: `gui-linux` (the window and the wheel under a virtual display, screenshots kept as artifacts) and `windows-smoke` (the real executable: a headless action, the window with a worker and no listening port, the tray helper; `continue-on-error` until it has run once on a real runner).
- The UI toolkit brings a dependency tree of its own; each crate in it is covered by `cargo-deny` (licences, advisories, the network-crate ban list) and `cargo-audit` like any other ([`SEC-SUPPLY`](#sec-supply), [`SEC-NONET`](#sec-nonet)).
- Signing step is present but inactive until a certificate exists ([`SEC-SIGN`](#sec-sign)).
- Release artifact checklist: hashes published, SBOM generated, installer scanned, clean-VM smoke test.

## <a id="ph-plan"></a>19. Build plan

Each phase ends with its definition of done. An agent completes phases in order and does not start the next until the current one is green.

| ID | Phase | Delivers | Done when | Testable on Linux |
|---|---|---|---|---|
| <a id="ph-0"></a> | Scaffold | Workspace, CI, vendoring, link-check tool, empty crates | CI green on both runners; this spec's link check passes | yes |
| <a id="ph-1"></a> | Core | [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store), [`MOD-IMAGE`](#mod-image), [`MOD-PDF`](#mod-pdf) basics (open, render, build), [`MOD-WORKER`](#mod-worker) + [`MOD-JOBS`](#mod-jobs); page ops [`TOOL-REMOVE`](#tool-remove), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-ROTATE`](#tool-rotate); a headless CLI harness | [`TEST-OPS`](#test-ops), [`TEST-UNDO`](#test-undo), [`TEST-IMAGE`](#test-image), [`TEST-JOBS`](#test-jobs) pass; 100-page open and export within [`PERF-BUDGETS`](#perf-budgets) (`sheaf-harness bench --enforce`, run in CI) | yes |
| <a id="ph-2"></a> | OCR | [`MOD-OCR`](#mod-ocr), [`OCR-PIPELINE`](#ocr-pipeline), cache, text layer (fallback first, overlay if feasible), [`TOOL-OCR`](#tool-ocr), [`FLOW-OCR-MANUAL`](#flow-ocr-manual) headless | [`TEST-OCR`](#test-ocr) passes on fixtures; searchable PDF verified by text extraction (whole page and by region) | yes |
| <a id="ph-3"></a> | Tools | [`TOOL-MERGE`](#tool-merge), [`TOOL-SPLIT`](#tool-split), [`TOOL-EXTRACT`](#tool-extract), [`TOOL-CROP`](#tool-crop), [`TOOL-COMPRESS`](#tool-compress), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-PDF2IMG`](#tool-pdf2img), [`TOOL-WATERMARK`](#tool-watermark), [`TOOL-PAGENUM`](#tool-pagenum), [`TOOL-PROTECT`](#tool-protect), [`TOOL-UNLOCK`](#tool-unlock) | all their TEST groups pass; [`RISK-LOPDF-ENC`](#risk-lopdf-enc) resolved or fallback chosen (resolved) | yes |
| <a id="ph-4"></a> | Shell and UI | [`MOD-SHELL`](#mod-shell), [`MOD-UI`](#mod-ui), the [`DOC-IPC`](#doc-ipc) command surface, [`DOC-UI`](#doc-ui), i18n ([`ADR-I18N`](#adr-i18n)), and (added at the owner's request) the action wheel ([`ADR-WHEEL`](#adr-wheel)) with its headless quick actions | The window and the wheel run against the real worker; [`TEST-SEC`](#test-sec) command-layer and port checks pass. Status: built; green on Linux including the GUI smoke under a virtual display; the Windows-only parts (tray helper, hotkey, Explorer and desktop selection, overlay placement, the Windows smoke job) type-check and lint clean for the Windows target and have not run on Windows ([`RISK-WIN-SHELL`](#risk-win-shell)) | partly |
| <a id="ph-5"></a> | Scan | [`MOD-SCAN`](#mod-scan) (`FakeBackend` first, then `WiaBackend`), [`FLOW-SCAN-LIVE`](#flow-scan-live), the scan dialog ([`UI-SCANDLG`](#ui-scandlg)) | [`TEST-SCAN`](#test-scan) passes; manual scan on the real device ([`RISK-WIA`](#risk-wia)). Status: built end to end against the fake scanner (tests, and the dialog under a virtual display); `WiaBackend` is written against the WIA 2 interfaces and type-checks and lints for Windows, and listing devices runs on Windows CI, but no page has come from a real scanner | fake only |
| <a id="ph-6"></a> | Automation | [`PRESET-FORMAT`](#preset-format), [`FLOW-PRESET`](#flow-preset), [`MOD-CLI`](#mod-cli), Explorer verbs ([`FLOW-EXPLORER`](#flow-explorer)) | preset runs headless and from the UI ([`TEST-PRESET`](#test-preset)); multi-select aggregation works ([`RISK-EXPLORER-MULTI`](#risk-explorer-multi)). Status: presets, aggregation and the verb entries are built and tested on Linux ([`TEST-PRESET`](#test-preset), [`TEST-EXPLORER`](#test-explorer)); the registry writes and the three-process merge are in the Windows smoke; what Explorer really does with the menu (Windows 11 "Show more options", multi-select with large selections) has not been seen | partly |
| <a id="ph-7"></a> | Hardening | [`TOOL-REPAIR`](#tool-repair), [`TOOL-METADATA`](#tool-metadata), limits, fuzzing, signing, installer, perf tuning against [`PERF-BUDGETS`](#perf-budgets) | all [`TEST-SEC`](#test-sec) items, clean-VM install, budgets measured and recorded | partly |

Suggested spike order inside phases: the WIA spike (first half of [`PH-5`](#ph-5)) can run early and in parallel because it needs the physical scanner; do it as soon as [`PH-4`](#ph-4) has a window.

## <a id="doc-risks"></a>20. Risks and open questions

| ID | Risk or unknown | Impact | Plan |
|---|---|---|---|
| <a id="risk-wia"></a> | WIA from Rust (COM, driver variance, feeder behavior) is unproven here and cannot be tested in the sandbox | Scanning delayed | Early spike with the real scanner ([`PH-5`](#ph-5)); fall back to .NET helper process if blocked ([`ADR-TAURI`](#adr-tauri)) |
| <a id="risk-lopdf-enc"></a> | lopdf encryption: examples exist, but docs warn some algorithms are not fully supported; AES-256 output and compatibility with common readers unproven | [`TOOL-PROTECT`](#tool-protect), [`TOOL-UNLOCK`](#tool-unlock) | Resolved: lopdf 0.47 writes AES-256 (V5, R6). Output checked by hand in PDFium, qpdf 11.9 (`R = 6`, permissions as set, no warnings) and Poppler 24.02 (`algorithm:AES-256`), with a non-ASCII password. The qpdf fallback is not needed. Acrobat and browser viewers are still to be tried |
| <a id="risk-overlay-font"></a> | Glyphless font + ToUnicode for Cyrillic/Uzbek text layers is fiddly | [`OCR-TEXTLAYER`](#ocr-textlayer) quality | Sidestepped: the glyphless font and ToUnicode map are Tesseract's own (text-only PDF), overlaid with PDFium. Verified on synthetic Russian, Uzbek Latin and Uzbek Cyrillic pages; still to confirm in third-party readers |
| <a id="risk-compress"></a> | Image re-encoding cannot match dedicated optimizers on every file | [`TOOL-COMPRESS`](#tool-compress) expectations | Honest before/after reporting; never grow the file |
| <a id="risk-pdfium-threads"></a> | PDFium is not thread-safe. Confirmed in the `pdfium-render` 0.9.5 documentation; in practice, loading or dropping the library on one thread while another uses it crashes, so each process holds exactly one engine | [`PERF-PARALLEL`](#perf-parallel) | Worker processes ([`ADR-WORKER-PROCS`](#adr-worker-procs)) |
| <a id="risk-forms-merge"></a> | Page import may drop AcroForm fields | [`TOOL-MERGE`](#tool-merge) | Done: detected at import, flatten offered at export, both tested |
| <a id="risk-smartscreen"></a> | Unsigned exes trigger warnings | Adoption | Obtain a code-signing certificate ([`SEC-SIGN`](#sec-sign)) |
| <a id="risk-webview2"></a> | (No longer applies: the app has no web view, [`ADR-NATIVE-UI`](#adr-native-ui).) | none | none |
| <a id="risk-av"></a> | Bundled third-party exes can trigger antivirus heuristics | Install blocked | Sign; keep hashes published; test with Defender |
| <a id="risk-large-scans"></a> | A3 at 600 dpi creates very large bitmaps | Memory | [`SEC-LIMITS`](#sec-limits), process in strips or downsample for OCR |
| <a id="risk-ocr-accuracy"></a> | Tesseract is weaker on photos, complex layouts, tables, handwriting; Uzbek models vary | User trust | [`TEST-OCR`](#test-ocr) on real samples, confidence UI ([`OCR-CONF-UI`](#ocr-conf-ui)), preprocessing, optional second engine ([`OCR-ENGINES`](#ocr-engines)) |
| <a id="risk-win-shell"></a> | The Windows-only parts were written and type-checked for the Windows target from a Linux machine and have not been run on Windows: the tray helper and its hotkey, reading the selection from Explorer (tabs, the desktop) and from an elevated window, placing a transparent overlay at the cursor with the right scale on mixed-DPI monitors and getting keyboard focus, the OpenGL renderer on Windows GPUs and over remote desktop, autostart in the Run key. The window and the wheel themselves are drawn and exercised under a virtual display on Linux | [`PH-4`](#ph-4) cannot be called finished; the wheel may not appear or may see no selection | First Windows CI run of `windows-smoke`, then a manual pass on a clean VM with Explorer tabs, the desktop, a second monitor and an elevated Explorer; fix what it finds before [`PH-5`](#ph-5) |
| <a id="risk-wheel-hotkey"></a> | A global hotkey can clash with another program's, and a non-elevated helper cannot read the selection of an elevated Explorer window (Windows blocks it) | The wheel does nothing or shows no files | The hotkey is a setting and the helper says when registering it failed ([`DATA-SETTINGS`](#data-settings)); documented limit for elevated windows; "Open in Sheaf" and the tray menu still work |
| <a id="risk-explorer-multi"></a> | Explorer launches one process per selected file for simple verbs; Windows 11 hides classic verbs under "Show more options" | [`FLOW-EXPLORER`](#flow-explorer) | Single-instance aggregation ([`MOD-CLI`](#mod-cli), built: `--collect`); document the menu location (done in [`CLI-GRAMMAR`](#cli-grammar)); only the Windows 11 top-level menu stays out of reach with registry verbs |

## <a id="doc-trace"></a>21. Traceability matrix (tool → module → command → UI → test → phase)

The same facts as the tool table ([`DOC-TOOLS`](#doc-tools)), grouped by the capability a person asks for.

| Capability | Tools | Modules | Commands | UI | Tests | Phase |
|---|---|---|---|---|---|---|
| Combine files | [`TOOL-MERGE`](#tool-merge), [`TOOL-IMG2PDF`](#tool-img2pdf) | [`MOD-PDF`](#mod-pdf) | [`CMD-IMPORT`](#cmd-import), [`CMD-EXPORT`](#cmd-export) | [`UI-TOOLBAR`](#ui-toolbar), [`UI-EXPORTDLG`](#ui-exportdlg) | [`TEST-MERGE`](#test-merge), [`TEST-IMG2PDF`](#test-img2pdf) | [`PH-3`](#ph-3) |
| Pick or reorder pages | [`TOOL-EXTRACT`](#tool-extract), [`TOOL-REMOVE`](#tool-remove), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-ROTATE`](#tool-rotate) | [`MOD-CORE`](#mod-core) | [`CMD-APPLY`](#cmd-apply), [`CMD-UNDO`](#cmd-undo) | [`UI-TRAY`](#ui-tray), [`UI-PREVIEW`](#ui-preview) | [`TEST-PAGEOPS`](#test-pageops), [`TEST-OPS`](#test-ops), [`TEST-UNDO`](#test-undo) | [`PH-1`](#ph-1) |
| Split and name | [`TOOL-SPLIT`](#tool-split) | [`MOD-PDF`](#mod-pdf), [`MOD-OCR`](#mod-ocr) | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`UI-DRAWER`](#ui-drawer) | [`TEST-SPLIT`](#test-split) | [`PH-3`](#ph-3) |
| Make smaller | [`TOOL-COMPRESS`](#tool-compress), [`TOOL-CROP`](#tool-crop) | [`MOD-PDF`](#mod-pdf), [`MOD-IMAGE`](#mod-image) | [`CMD-TOOL-RUN`](#cmd-tool-run), [`CMD-APPLY`](#cmd-apply) | [`UI-DRAWER`](#ui-drawer), [`UI-PREVIEW`](#ui-preview) | [`TEST-COMPRESS`](#test-compress), [`TEST-CROP`](#test-crop) | [`PH-3`](#ph-3) |
| Convert to images | [`TOOL-PDF2IMG`](#tool-pdf2img) | [`MOD-PDF`](#mod-pdf) | [`CMD-EXPORT`](#cmd-export) | [`UI-EXPORTDLG`](#ui-exportdlg) | [`TEST-PDF2IMG`](#test-pdf2img) | [`PH-3`](#ph-3) |
| Mark pages | [`TOOL-WATERMARK`](#tool-watermark), [`TOOL-PAGENUM`](#tool-pagenum) | [`MOD-PDF`](#mod-pdf) | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`UI-DRAWER`](#ui-drawer) | [`TEST-STAMP`](#test-stamp) | [`PH-3`](#ph-3) |
| Lock and unlock | [`TOOL-PROTECT`](#tool-protect), [`TOOL-UNLOCK`](#tool-unlock) | [`MOD-PDF`](#mod-pdf) | [`CMD-TOOL-RUN`](#cmd-tool-run), [`CMD-UNLOCK-PW`](#cmd-unlock-pw) | [`UI-DRAWER`](#ui-drawer) | [`TEST-PROTECT`](#test-protect) | [`PH-3`](#ph-3) |
| Make searchable (on command) | [`TOOL-OCR`](#tool-ocr) | [`MOD-OCR`](#mod-ocr), [`MOD-WORKER`](#mod-worker) | [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-OCR-RESULT`](#cmd-ocr-result), [`CMD-SEARCH`](#cmd-search) | [`UI-OCRPANEL`](#ui-ocrpanel), [`UI-SEARCH`](#ui-search) | [`TEST-OCR`](#test-ocr) | [`PH-2`](#ph-2) |
| Scan with live OCR | [`TOOL-SCAN`](#tool-scan), [`TOOL-OCR`](#tool-ocr) | [`MOD-SCAN`](#mod-scan), [`MOD-JOBS`](#mod-jobs), [`MOD-OCR`](#mod-ocr) | [`CMD-SCAN-START`](#cmd-scan-start), [`CMD-SCAN-CANCEL`](#cmd-scan-cancel) | [`UI-SCANDLG`](#ui-scandlg), [`UI-JOBBAR`](#ui-jobbar) | [`TEST-SCAN`](#test-scan), [`TEST-OCR`](#test-ocr) | [`PH-5`](#ph-5) |
| One-click workflows | all above | [`MOD-JOBS`](#mod-jobs), [`MOD-CLI`](#mod-cli) | [`CMD-PRESETS`](#cmd-presets) | [`UI-PRESETS`](#ui-presets) | [`TEST-JOBS`](#test-jobs) | [`PH-6`](#ph-6) |
| Fix and inspect | [`TOOL-REPAIR`](#tool-repair), [`TOOL-METADATA`](#tool-metadata) | [`MOD-PDF`](#mod-pdf) | [`CMD-TOOL-RUN`](#cmd-tool-run) | [`UI-DRAWER`](#ui-drawer) | [`TEST-REPAIR`](#test-repair), [`TEST-METADATA`](#test-metadata) | [`PH-7`](#ph-7) |

Goal traceability: [`GOAL-SECURE`](#goal-secure) → [`DOC-SECURITY`](#doc-security); [`GOAL-FAST`](#goal-fast) → [`DOC-PERF`](#doc-perf); [`GOAL-COMPACT`](#goal-compact) → [`DOC-UI`](#doc-ui); [`GOAL-LIVEOCR`](#goal-liveocr) → [`FLOW-SCAN-LIVE`](#flow-scan-live); [`GOAL-ONCMD-OCR`](#goal-oncmd-ocr) → [`FLOW-OCR-MANUAL`](#flow-ocr-manual); [`GOAL-ILOVEPDF`](#goal-ilovepdf) → [`DOC-TOOLS`](#doc-tools); [`GOAL-AGENT`](#goal-agent) → this section and [`DOC-AGENT-RULES`](#doc-agent-rules).

## <a id="doc-agent-rules"></a>22. Rules for the agent that builds this

1. **Order:** follow [`PH-PLAN`](#ph-plan). One phase at a time; finish its tests before the next.
2. **Never:** add a network listener or outbound call ([`SEC-NONET`](#sec-nonet)); add telemetry or an updater ([`NG-CLOUD`](#ng-cloud)); parse documents in the main process ([`SEC-ISOLATE`](#sec-isolate)); build shell command strings from input ([`OCR-TESS-CLI`](#ocr-tess-cli)); pass free-text paths from the UI ([`SEC-PATHS`](#sec-paths)); log passwords or document text ([`SEC-LOG`](#sec-log)); overwrite user files ([`ADR-NO-OVERWRITE`](#adr-no-overwrite)); implement password cracking ([`NG-CRACK`](#ng-crack)); ship a "redact" or "crop" that only hides ([`SEC-HONESTY`](#sec-honesty)).
3. **Dependencies:** a crate may depend only on what [`DOC-MODULES`](#doc-modules) lists; adding a third-party crate needs a one-line justification in the PR and must pass `cargo-deny` ([`SEC-SUPPLY`](#sec-supply)).
4. **Errors:** no `unwrap` or panics across IPC or worker boundaries; every failure maps to an [`EVT-ERROR`](#evt-error) code ([`FLOW-ERROR`](#flow-error)).
5. **Unsafe:** only in Win32 and COM wrapper modules, each block commented with its invariant: PDFium bindings, `sheaf-jobs` `winjob` (Job Objects), `sheaf-store` `harden` (process hardening), and in the shell `selection`, `tray` and `crash` (shell COM, tray, hotkey, message box).
6. **Honesty:** when a claim marked VERIFY is tested, update the text and remove the marker; when reality differs from this spec, change the spec in the same PR and keep IDs stable.
7. **Done means:** code, tests from the matching [`DOC-TESTS`](#doc-tests) group, docs updated, link check passes (`tools/link-check`), budgets measured where [`PERF-BUDGETS`](#perf-budgets) applies.
8. **Ask, don't guess,** when blocked by hardware (scanner), licensing, or a decision listed in [`DOC-RISKS`](#doc-risks).

### Kickoff prompt (paste to the agent)

> Read `ARCHITECTURE.md` completely, starting with [`DOC-READ`](#doc-read) and [`DOC-AGENT-RULES`](#doc-agent-rules). Build phase [`PH-0`](#ph-0), then [`PH-1`](#ph-1), following [`PH-PLAN`](#ph-plan). For each task name the IDs you implement. Do not start a phase until the previous phase's tests pass. Record every VERIFY item you resolve. Report blockers that need hardware or a decision instead of guessing.

## <a id="doc-refs"></a>23. External references (checked 2026-11-13)

- Unlimited-OCR model page: https://huggingface.co/baidu/Unlimited-OCR (parameters, license, GPU testing, `trust_remote_code`)
- Uzbek OCR benchmark including Tesseract 5 and Unlimited-OCR: https://huggingface.co/steppelabs/xattot (published by a competing model; treat as informative, not neutral)
- `pdfium-render` (render, edit, import pages, save): https://docs.rs/pdfium-render/latest/
- `lopdf` (object-level PDF editing, encryption support and caveats): https://github.com/j-f-liu/lopdf and https://docs.rs/crate/lopdf/latest
- Tesseract language data including `uzb` and `uzb_cyrl`: https://github.com/tesseract-ocr/tessdata (also `tessdata_fast`, `tessdata_best`)
- PDFium binaries used by Rust bindings: https://github.com/bblanchon/pdfium-binaries

---

## <a id="appendix-a"></a>Appendix A. Reverse index (generated)

Every ID, what it is, and everything that references it. Generated by `tools/link-check --write` from the cross-links above; do not edit by hand.

<!-- BEGIN GENERATED appendix-a -->
| ID | Kind | Referenced by |
|---|---|---|
| [`ADR-APP-CRATE`](#adr-app-crate) | ADR | [`MOD-APP`](#mod-app) |
| [`ADR-EXPLORER-VERBS`](#adr-explorer-verbs) | ADR | [`CLI-GRAMMAR`](#cli-grammar), [`DOC-BUILD`](#doc-build) |
| [`ADR-I18N`](#adr-i18n) | ADR | [`DOC-UI`](#doc-ui), [`MOD-UI`](#mod-ui), [`PH-4`](#ph-4) |
| [`ADR-NATIVE-UI`](#adr-native-ui) | ADR | [`ADR-TAURI`](#adr-tauri), [`ADR-VANILLA-UI`](#adr-vanilla-ui), [`DOC-BUILD`](#doc-build), [`MOD-UI`](#mod-ui), [`RISK-WEBVIEW2`](#risk-webview2), [`SEC-NONET`](#sec-nonet), [`TOOL-HTML2PDF`](#tool-html2pdf) |
| [`ADR-NO-OVERWRITE`](#adr-no-overwrite) | ADR | [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-TOOLS`](#doc-tools), [`FLOW-OCR-MANUAL`](#flow-ocr-manual), [`FLOW-PRESET`](#flow-preset), [`FLOW-WHEEL`](#flow-wheel), [`SEC-PATHS`](#sec-paths) |
| [`ADR-NO-VLM-DEFAULT`](#adr-no-vlm-default) | ADR | [`OCR-ENGINES`](#ocr-engines) |
| [`ADR-NOHTTP`](#adr-nohttp) | ADR | [`SEC-NONET`](#sec-nonet) |
| [`ADR-NONDESTRUCTIVE`](#adr-nondestructive) | ADR | [`DATA-PAGE`](#data-page), [`DATA-SOURCE`](#data-source), [`DOC-TOOLS`](#doc-tools) |
| [`ADR-OFFLINE-ASSETS`](#adr-offline-assets) | ADR | [`DOC-BUILD`](#doc-build), [`OCR-LANGS`](#ocr-langs) |
| [`ADR-PDFIUM-LOPDF`](#adr-pdfium-lopdf) | ADR | [`PERF-LOSSLESS`](#perf-lossless) |
| [`ADR-TAURI`](#adr-tauri) | ADR | [`RISK-WIA`](#risk-wia) |
| [`ADR-TESS-SIDECAR`](#adr-tess-sidecar) | ADR | [`OCR-ENGINES`](#ocr-engines), [`PERF-OCR`](#perf-ocr) |
| [`ADR-UI-REORDER`](#adr-ui-reorder) | ADR | [`DOC-UI`](#doc-ui) |
| [`ADR-VANILLA-UI`](#adr-vanilla-ui) | ADR | (nothing; entry point or leaf) |
| [`ADR-WHEEL`](#adr-wheel) | ADR | [`CLI-GRAMMAR`](#cli-grammar), [`FLOW-WHEEL`](#flow-wheel), [`GOAL-WHEEL`](#goal-wheel), [`MOD-CORE`](#mod-core), [`PH-4`](#ph-4) |
| [`ADR-WIA`](#adr-wia) | ADR | [`MOD-SCAN`](#mod-scan) |
| [`ADR-WORKER-PROCS`](#adr-worker-procs) | ADR | [`PERF-PARALLEL`](#perf-parallel), [`RISK-PDFIUM-THREADS`](#risk-pdfium-threads), [`SEC-ISOLATE`](#sec-isolate) |
| [`CLI-GRAMMAR`](#cli-grammar) | CLI | [`ADR-EXPLORER-VERBS`](#adr-explorer-verbs), [`DOC-BUILD`](#doc-build), [`FLOW-OCR-MANUAL`](#flow-ocr-manual), [`MOD-CLI`](#mod-cli), [`PRESET-FORMAT`](#preset-format), [`RISK-EXPLORER-MULTI`](#risk-explorer-multi) |
| [`CMD-APPLY`](#cmd-apply) | CMD | [`DOC-IPC`](#doc-ipc), [`DOC-TRACE`](#doc-trace), [`FLOW-EDIT`](#flow-edit), [`TOOL-CROP`](#tool-crop), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-REMOVE`](#tool-remove), [`TOOL-ROTATE`](#tool-rotate), [`UI-PREVIEW`](#ui-preview), [`UI-SHORTCUTS`](#ui-shortcuts), [`UI-TRAY`](#ui-tray) |
| [`CMD-EXPORT`](#cmd-export) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-EXPORT`](#flow-export), [`TOOL-EXTRACT`](#tool-extract), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-MERGE`](#tool-merge), [`TOOL-PDF2IMG`](#tool-pdf2img), [`UI-EXPORTDLG`](#ui-exportdlg) |
| [`CMD-IMPORT`](#cmd-import) | CMD | [`DOC-IPC`](#doc-ipc), [`DOC-TRACE`](#doc-trace), [`FLOW-IMPORT`](#flow-import), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-MERGE`](#tool-merge), [`UI-TOOLBAR`](#ui-toolbar) |
| [`CMD-JOB-CANCEL`](#cmd-job-cancel) | CMD | [`CMD-SCAN-CANCEL`](#cmd-scan-cancel), [`UI-JOBBAR`](#ui-jobbar) |
| [`CMD-OCR-RESULT`](#cmd-ocr-result) | CMD | [`DOC-TRACE`](#doc-trace), [`UI-OCRPANEL`](#ui-ocrpanel), [`UI-PREVIEW`](#ui-preview) |
| [`CMD-OCR-RUN`](#cmd-ocr-run) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-OCR-MANUAL`](#flow-ocr-manual), [`TOOL-OCR`](#tool-ocr), [`UI-OCRPANEL`](#ui-ocrpanel), [`UI-SHORTCUTS`](#ui-shortcuts), [`UI-TOOLBAR`](#ui-toolbar) |
| [`CMD-PAGES`](#cmd-pages) | CMD | [`UI-TRAY`](#ui-tray) |
| [`CMD-PRESETS`](#cmd-presets) | CMD | [`DOC-IPC`](#doc-ipc), [`DOC-TRACE`](#doc-trace), [`FLOW-PRESET`](#flow-preset), [`PRESET-FORMAT`](#preset-format), [`UI-PRESETS`](#ui-presets), [`UI-TOOLBAR`](#ui-toolbar) |
| [`CMD-RENDER`](#cmd-render) | CMD | [`DOC-IPC`](#doc-ipc), [`UI-PREVIEW`](#ui-preview) |
| [`CMD-REVEAL`](#cmd-reveal) | CMD | [`FLOW-TOOL-RUN`](#flow-tool-run), [`SEC-PATHS`](#sec-paths), [`UI-JOBBAR`](#ui-jobbar) |
| [`CMD-SCAN-CANCEL`](#cmd-scan-cancel) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-SCAN-LIVE`](#flow-scan-live) |
| [`CMD-SCAN-DEVICES`](#cmd-scan-devices) | CMD | [`UI-SCANDLG`](#ui-scandlg) |
| [`CMD-SCAN-START`](#cmd-scan-start) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-SCAN-LIVE`](#flow-scan-live), [`MOD-SCAN`](#mod-scan), [`TOOL-SCAN`](#tool-scan), [`UI-SCANDLG`](#ui-scandlg) |
| [`CMD-SEARCH`](#cmd-search) | CMD | [`DOC-TRACE`](#doc-trace), [`MOD-OCR`](#mod-ocr), [`OCR-PIPELINE`](#ocr-pipeline), [`UI-SEARCH`](#ui-search) |
| [`CMD-SETTINGS`](#cmd-settings) | CMD | [`UI-SETTINGS`](#ui-settings) |
| [`CMD-THUMB`](#cmd-thumb) | CMD | [`CMD-RENDER`](#cmd-render), [`DOC-IPC`](#doc-ipc), [`FLOW-IMPORT`](#flow-import), [`PERF-IO`](#perf-io), [`UI-TRAY`](#ui-tray) |
| [`CMD-TOOL-RUN`](#cmd-tool-run) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-TOOL-RUN`](#flow-tool-run), [`TOOL-COMPRESS`](#tool-compress), [`TOOL-METADATA`](#tool-metadata), [`TOOL-PAGENUM`](#tool-pagenum), [`TOOL-PROTECT`](#tool-protect), [`TOOL-REDACT`](#tool-redact), [`TOOL-REPAIR`](#tool-repair), [`TOOL-SIGN-VISUAL`](#tool-sign-visual), [`TOOL-SPLIT`](#tool-split), [`TOOL-UNLOCK`](#tool-unlock), [`TOOL-WATERMARK`](#tool-watermark), [`UI-DRAWER`](#ui-drawer) |
| [`CMD-UNDO`](#cmd-undo) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-EDIT`](#flow-edit), [`UI-TOOLBAR`](#ui-toolbar) |
| [`CMD-UNLOCK-PW`](#cmd-unlock-pw) | CMD | [`DOC-TRACE`](#doc-trace), [`FLOW-IMPORT`](#flow-import), [`TOOL-UNLOCK`](#tool-unlock), [`UI-DRAWER`](#ui-drawer) |
| [`DATA-CACHEKEY`](#data-cachekey) | DATA | [`DATA-OCRSTATE`](#data-ocrstate), [`OCR-CACHE`](#ocr-cache), [`OCR-PIPELINE`](#ocr-pipeline) |
| [`DATA-EXPORTSPEC`](#data-exportspec) | DATA | [`FLOW-EXPORT`](#flow-export) |
| [`DATA-FILTER`](#data-filter) | DATA | [`OCR-PIPELINE`](#ocr-pipeline) |
| [`DATA-JOB`](#data-job) | DATA | [`MOD-CORE`](#mod-core) |
| [`DATA-NAMEPATTERN`](#data-namepattern) | DATA | [`DOC-TOOLS`](#doc-tools), [`FLOW-EXPORT`](#flow-export), [`TEST-SPLIT`](#test-split) |
| [`DATA-OCRPOLICY`](#data-ocrpolicy) | DATA | [`FLOW-OCR-AUTO`](#flow-ocr-auto) |
| [`DATA-OCRRESULT`](#data-ocrresult) | DATA | [`MOD-CORE`](#mod-core) |
| [`DATA-OCRSTATE`](#data-ocrstate) | DATA | [`OCR-PIPELINE`](#ocr-pipeline) |
| [`DATA-OP`](#data-op) | DATA | [`DOC-TOOLS`](#doc-tools), [`FLOW-IMPORT`](#flow-import), [`MOD-CORE`](#mod-core), [`OCR-PIPELINE`](#ocr-pipeline), [`TEST-OPS`](#test-ops) |
| [`DATA-PAGE`](#data-page) | DATA | [`ADR-NONDESTRUCTIVE`](#adr-nondestructive) |
| [`DATA-SCANPROFILE`](#data-scanprofile) | DATA | [`SCAN-WIA`](#scan-wia) |
| [`DATA-SETTINGS`](#data-settings) | DATA | [`FLOW-WHEEL`](#flow-wheel), [`MOD-SHELL`](#mod-shell), [`PERF-PARALLEL`](#perf-parallel), [`RISK-WHEEL-HOTKEY`](#risk-wheel-hotkey) |
| [`DATA-SOURCE`](#data-source) | DATA | [`MOD-CORE`](#mod-core) |
| [`DATA-SPLITRULE`](#data-splitrule) | DATA | [`DOC-TOOLS`](#doc-tools), [`TEST-SPLIT`](#test-split) |
| [`DATA-WORKERPROTO`](#data-workerproto) | DATA | [`DOC-SHAPE`](#doc-shape), [`MOD-WORKER`](#mod-worker) |
| [`DATA-WORKSPACE`](#data-workspace) | DATA | [`MOD-CORE`](#mod-core) |
| [`DOC-ADR`](#doc-adr) | DOC | (nothing; entry point or leaf) |
| [`DOC-AGENT-RULES`](#doc-agent-rules) | DOC | [`DOC-MODULES`](#doc-modules), [`DOC-READ`](#doc-read), [`DOC-TRACE`](#doc-trace), [`GOAL-AGENT`](#goal-agent), [`MOD-SHELL`](#mod-shell) |
| [`DOC-BUILD`](#doc-build) | DOC | [`DOC-LAYOUT`](#doc-layout) |
| [`DOC-DATA`](#doc-data) | DOC | (nothing; entry point or leaf) |
| [`DOC-FLOWS`](#doc-flows) | DOC | (nothing; entry point or leaf) |
| [`DOC-GOALS`](#doc-goals) | DOC | (nothing; entry point or leaf) |
| [`DOC-IPC`](#doc-ipc) | DOC | [`ADR-APP-CRATE`](#adr-app-crate), [`DOC-DATA`](#doc-data), [`DOC-LAYOUT`](#doc-layout), [`MOD-APP`](#mod-app), [`PH-4`](#ph-4) |
| [`DOC-LAYOUT`](#doc-layout) | DOC | [`DOC-BUILD`](#doc-build) |
| [`DOC-MODULES`](#doc-modules) | DOC | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`DOC-NONGOALS`](#doc-nongoals) | DOC | [`GOAL-COMPACT`](#goal-compact) |
| [`DOC-OCR`](#doc-ocr) | DOC | [`DOC-TOOLS`](#doc-tools) |
| [`DOC-PERF`](#doc-perf) | DOC | [`DOC-TRACE`](#doc-trace) |
| [`DOC-READ`](#doc-read) | DOC | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`DOC-REFS`](#doc-refs) | DOC | [`OCR-ENGINES`](#ocr-engines) |
| [`DOC-RISKS`](#doc-risks) | DOC | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`DOC-SCAN`](#doc-scan) | DOC | [`DOC-TOOLS`](#doc-tools) |
| [`DOC-SECURITY`](#doc-security) | DOC | [`DOC-TRACE`](#doc-trace), [`SEC-TB1`](#sec-tb1), [`SEC-TB2`](#sec-tb2), [`SEC-TB3`](#sec-tb3), [`SEC-TB4`](#sec-tb4) |
| [`DOC-SHAPE`](#doc-shape) | DOC | (nothing; entry point or leaf) |
| [`DOC-TESTS`](#doc-tests) | DOC | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`DOC-TOOLS`](#doc-tools) | DOC | [`DOC-TRACE`](#doc-trace), [`GOAL-ILOVEPDF`](#goal-ilovepdf), [`UI-DRAWER`](#ui-drawer) |
| [`DOC-TRACE`](#doc-trace) | DOC | [`DOC-READ`](#doc-read) |
| [`DOC-UI`](#doc-ui) | DOC | [`DOC-TRACE`](#doc-trace), [`MOD-UI`](#mod-ui), [`PH-4`](#ph-4) |
| [`EVT-ENGINES`](#evt-engines) | EVT | (nothing; entry point or leaf) |
| [`EVT-ERROR`](#evt-error) | EVT | [`DOC-AGENT-RULES`](#doc-agent-rules), [`FLOW-ERROR`](#flow-error), [`SCAN-FAILURES`](#scan-failures) |
| [`EVT-IMPORT`](#evt-import) | EVT | (nothing; entry point or leaf) |
| [`EVT-JOB`](#evt-job) | EVT | [`DOC-IPC`](#doc-ipc), [`FLOW-SCAN-LIVE`](#flow-scan-live), [`FLOW-TOOL-RUN`](#flow-tool-run), [`MOD-APP`](#mod-app), [`UI-JOBBAR`](#ui-jobbar) |
| [`EVT-PAGE-READY`](#evt-page-ready) | EVT | [`FLOW-SCAN-LIVE`](#flow-scan-live), [`OCR-PIPELINE`](#ocr-pipeline) |
| [`EVT-PAGES`](#evt-pages) | EVT | [`FLOW-EDIT`](#flow-edit), [`FLOW-IMPORT`](#flow-import) |
| [`EVT-SCAN-PAGE`](#evt-scan-page) | EVT | [`FLOW-SCAN-LIVE`](#flow-scan-live) |
| [`FLOW-EDIT`](#flow-edit) | FLOW | (nothing; entry point or leaf) |
| [`FLOW-ERROR`](#flow-error) | FLOW | [`DOC-AGENT-RULES`](#doc-agent-rules), [`FLOW-WHEEL`](#flow-wheel) |
| [`FLOW-EXPLORER`](#flow-explorer) | FLOW | [`CLI-GRAMMAR`](#cli-grammar), [`MOD-CLI`](#mod-cli), [`PH-6`](#ph-6), [`RISK-EXPLORER-MULTI`](#risk-explorer-multi) |
| [`FLOW-EXPORT`](#flow-export) | FLOW | (nothing; entry point or leaf) |
| [`FLOW-IMPORT`](#flow-import) | FLOW | [`FLOW-EXPLORER`](#flow-explorer) |
| [`FLOW-LIFECYCLE`](#flow-lifecycle) | FLOW | [`MOD-APP`](#mod-app), [`OCR-CACHE`](#ocr-cache), [`SEC-SUPPLY`](#sec-supply) |
| [`FLOW-OCR-AUTO`](#flow-ocr-auto) | FLOW | [`FLOW-IMPORT`](#flow-import), [`FLOW-OCR-MANUAL`](#flow-ocr-manual) |
| [`FLOW-OCR-MANUAL`](#flow-ocr-manual) | FLOW | [`DOC-TRACE`](#doc-trace), [`GOAL-ONCMD-OCR`](#goal-oncmd-ocr), [`OCR-PIPELINE`](#ocr-pipeline), [`PH-2`](#ph-2) |
| [`FLOW-PRESET`](#flow-preset) | FLOW | [`FLOW-SCAN-LIVE`](#flow-scan-live), [`PH-6`](#ph-6), [`PRESET-FORMAT`](#preset-format) |
| [`FLOW-SCAN-LIVE`](#flow-scan-live) | FLOW | [`DATA-PAGE`](#data-page), [`DATA-SOURCE`](#data-source), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`GOAL-LIVEOCR`](#goal-liveocr), [`MOD-APP`](#mod-app), [`PH-5`](#ph-5), [`SCAN-WIA`](#scan-wia) |
| [`FLOW-TOOL-RUN`](#flow-tool-run) | FLOW | (nothing; entry point or leaf) |
| [`FLOW-WHEEL`](#flow-wheel) | FLOW | [`FLOW-EXPLORER`](#flow-explorer), [`GOAL-WHEEL`](#goal-wheel), [`MOD-APP`](#mod-app), [`UI-WHEEL`](#ui-wheel) |
| [`GOAL-AGENT`](#goal-agent) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-COMPACT`](#goal-compact) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-FAST`](#goal-fast) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-ILOVEPDF`](#goal-ilovepdf) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-LIVEOCR`](#goal-liveocr) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-ONCMD-OCR`](#goal-oncmd-ocr) | GOAL | [`DOC-TRACE`](#doc-trace) |
| [`GOAL-SECURE`](#goal-secure) | GOAL | [`DOC-TRACE`](#doc-trace), [`NG-CLOUD`](#ng-cloud) |
| [`GOAL-WHEEL`](#goal-wheel) | GOAL | (nothing; entry point or leaf) |
| [`MOD-APP`](#mod-app) | MOD | [`ADR-APP-CRATE`](#adr-app-crate), [`ADR-NATIVE-UI`](#adr-native-ui), [`ADR-WHEEL`](#adr-wheel), [`CMD-PRESETS`](#cmd-presets), [`DOC-IPC`](#doc-ipc), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`FLOW-WHEEL`](#flow-wheel), [`MOD-SCAN`](#mod-scan), [`MOD-SHELL`](#mod-shell), [`MOD-UI`](#mod-ui), [`PROC-WHEEL`](#proc-wheel), [`TEST-MATRIX`](#test-matrix), [`UI-WHEEL`](#ui-wheel) |
| [`MOD-CLI`](#mod-cli) | MOD | [`CLI-GRAMMAR`](#cli-grammar), [`DOC-LAYOUT`](#doc-layout), [`DOC-TRACE`](#doc-trace), [`MOD-SHELL`](#mod-shell), [`PH-6`](#ph-6), [`RISK-EXPLORER-MULTI`](#risk-explorer-multi) |
| [`MOD-CORE`](#mod-core) | MOD | (preamble), [`CMD-APPLY`](#cmd-apply), [`CMD-IMPORT`](#cmd-import), [`CMD-PAGES`](#cmd-pages), [`CMD-UNDO`](#cmd-undo), [`DOC-DATA`](#doc-data), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`DOC-TRACE`](#doc-trace), [`FLOW-EDIT`](#flow-edit), [`FLOW-IMPORT`](#flow-import), [`MOD-APP`](#mod-app), [`MOD-CLI`](#mod-cli), [`MOD-IMAGE`](#mod-image), [`MOD-JOBS`](#mod-jobs), [`MOD-OCR`](#mod-ocr), [`MOD-PDF`](#mod-pdf), [`MOD-SCAN`](#mod-scan), [`MOD-SHELL`](#mod-shell), [`MOD-STORE`](#mod-store), [`MOD-WORKER`](#mod-worker), [`PH-1`](#ph-1), [`TEST-MATRIX`](#test-matrix), [`TOOL-CROP`](#tool-crop), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-REMOVE`](#tool-remove), [`TOOL-ROTATE`](#tool-rotate) |
| [`MOD-IMAGE`](#mod-image) | MOD | [`DATA-FILTER`](#data-filter), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-OCR`](#mod-ocr), [`MOD-PDF`](#mod-pdf), [`MOD-SCAN`](#mod-scan), [`MOD-WORKER`](#mod-worker), [`OCR-PIPELINE`](#ocr-pipeline), [`PH-1`](#ph-1), [`TEST-MATRIX`](#test-matrix), [`TOOL-COMPRESS`](#tool-compress), [`TOOL-CROP`](#tool-crop), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-SPLIT`](#tool-split) |
| [`MOD-JOBS`](#mod-jobs) | MOD | [`CMD-EXPORT`](#cmd-export), [`CMD-IMPORT`](#cmd-import), [`CMD-JOB-CANCEL`](#cmd-job-cancel), [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-RENDER`](#cmd-render), [`CMD-SCAN-START`](#cmd-scan-start), [`CMD-THUMB`](#cmd-thumb), [`CMD-TOOL-RUN`](#cmd-tool-run), [`CMD-UNLOCK-PW`](#cmd-unlock-pw), [`DOC-LAYOUT`](#doc-layout), [`DOC-TRACE`](#doc-trace), [`FLOW-IMPORT`](#flow-import), [`FLOW-TOOL-RUN`](#flow-tool-run), [`MOD-APP`](#mod-app), [`MOD-CORE`](#mod-core), [`MOD-STORE`](#mod-store), [`OCR-TESS-CLI`](#ocr-tess-cli), [`PERF-PARALLEL`](#perf-parallel), [`PH-1`](#ph-1), [`TEST-MATRIX`](#test-matrix) |
| [`MOD-OCR`](#mod-ocr) | MOD | [`CMD-OCR-RESULT`](#cmd-ocr-result), [`CMD-SEARCH`](#cmd-search), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`DOC-TRACE`](#doc-trace), [`MOD-APP`](#mod-app), [`MOD-CORE`](#mod-core), [`MOD-IMAGE`](#mod-image), [`MOD-STORE`](#mod-store), [`MOD-WORKER`](#mod-worker), [`PH-2`](#ph-2), [`TEST-MATRIX`](#test-matrix), [`TOOL-OCR`](#tool-ocr), [`TOOL-SPLIT`](#tool-split) |
| [`MOD-PDF`](#mod-pdf) | MOD | [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-CORE`](#mod-core), [`MOD-IMAGE`](#mod-image), [`MOD-WORKER`](#mod-worker), [`OCR-PIPELINE`](#ocr-pipeline), [`OCR-TEXTLAYER`](#ocr-textlayer), [`PH-1`](#ph-1), [`TEST-MATRIX`](#test-matrix), [`TOOL-COMPRESS`](#tool-compress), [`TOOL-CROP`](#tool-crop), [`TOOL-EXTRACT`](#tool-extract), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-MERGE`](#tool-merge), [`TOOL-METADATA`](#tool-metadata), [`TOOL-PAGENUM`](#tool-pagenum), [`TOOL-PDF2IMG`](#tool-pdf2img), [`TOOL-PROTECT`](#tool-protect), [`TOOL-REDACT`](#tool-redact), [`TOOL-REPAIR`](#tool-repair), [`TOOL-ROTATE`](#tool-rotate), [`TOOL-SIGN-VISUAL`](#tool-sign-visual), [`TOOL-SPLIT`](#tool-split), [`TOOL-UNLOCK`](#tool-unlock), [`TOOL-WATERMARK`](#tool-watermark) |
| [`MOD-SCAN`](#mod-scan) | MOD | [`CMD-SCAN-CANCEL`](#cmd-scan-cancel), [`CMD-SCAN-DEVICES`](#cmd-scan-devices), [`CMD-SCAN-START`](#cmd-scan-start), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`DOC-TRACE`](#doc-trace), [`FLOW-SCAN-LIVE`](#flow-scan-live), [`MOD-APP`](#mod-app), [`PH-5`](#ph-5), [`SCAN-BACKEND`](#scan-backend), [`SCAN-THREADING`](#scan-threading), [`TEST-MATRIX`](#test-matrix), [`TOOL-SCAN`](#tool-scan) |
| [`MOD-SHELL`](#mod-shell) | MOD | (preamble), [`CMD-REVEAL`](#cmd-reveal), [`DOC-LAYOUT`](#doc-layout), [`FLOW-WHEEL`](#flow-wheel), [`MOD-APP`](#mod-app), [`MOD-CORE`](#mod-core), [`MOD-JOBS`](#mod-jobs), [`MOD-OCR`](#mod-ocr), [`MOD-STORE`](#mod-store), [`PH-4`](#ph-4), [`TEST-MATRIX`](#test-matrix) |
| [`MOD-STORE`](#mod-store) | MOD | [`CMD-PRESETS`](#cmd-presets), [`CMD-SETTINGS`](#cmd-settings), [`DATA-PAGE`](#data-page), [`DATA-SOURCE`](#data-source), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`FLOW-IMPORT`](#flow-import), [`FLOW-TOOL-RUN`](#flow-tool-run), [`MOD-APP`](#mod-app), [`MOD-JOBS`](#mod-jobs), [`MOD-OCR`](#mod-ocr), [`MOD-SHELL`](#mod-shell), [`MOD-WORKER`](#mod-worker), [`PERF-LAZY`](#perf-lazy), [`PH-1`](#ph-1), [`PRESET-FORMAT`](#preset-format), [`TEST-MATRIX`](#test-matrix) |
| [`MOD-UI`](#mod-ui) | MOD | [`DOC-IPC`](#doc-ipc), [`DOC-LAYOUT`](#doc-layout), [`DOC-SHAPE`](#doc-shape), [`MOD-SHELL`](#mod-shell), [`PH-4`](#ph-4), [`PROC-MAIN`](#proc-main), [`PROC-UI`](#proc-ui), [`TEST-MATRIX`](#test-matrix) |
| [`MOD-WORKER`](#mod-worker) | MOD | [`DATA-WORKERPROTO`](#data-workerproto), [`DOC-LAYOUT`](#doc-layout), [`DOC-TRACE`](#doc-trace), [`MOD-IMAGE`](#mod-image), [`MOD-OCR`](#mod-ocr), [`MOD-PDF`](#mod-pdf), [`MOD-STORE`](#mod-store), [`PH-1`](#ph-1), [`PROC-WORKER`](#proc-worker), [`TEST-MATRIX`](#test-matrix), [`TOOL-OCR`](#tool-ocr) |
| [`NG-AI`](#ng-ai) | NG | (nothing; entry point or leaf) |
| [`NG-CLOUD`](#ng-cloud) | NG | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`NG-CRACK`](#ng-crack) | NG | [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-TOOLS`](#doc-tools) |
| [`NG-EDITTEXT`](#ng-edittext) | NG | [`DOC-TOOLS`](#doc-tools) |
| [`NG-OFFICE`](#ng-office) | NG | [`DOC-TOOLS`](#doc-tools) |
| [`NG-XPLAT`](#ng-xplat) | NG | [`MOD-SHELL`](#mod-shell) |
| [`OCR-CACHE`](#ocr-cache) | OCR | [`DATA-CACHEKEY`](#data-cachekey), [`DOC-TOOLS`](#doc-tools), [`FLOW-EDIT`](#flow-edit), [`MOD-OCR`](#mod-ocr), [`OCR-PIPELINE`](#ocr-pipeline), [`PERF-OCR`](#perf-ocr) |
| [`OCR-CONF-UI`](#ocr-conf-ui) | OCR | [`DOC-UI`](#doc-ui), [`RISK-OCR-ACCURACY`](#risk-ocr-accuracy), [`UI-OCRPANEL`](#ui-ocrpanel) |
| [`OCR-ENGINES`](#ocr-engines) | OCR | [`ADR-NO-VLM-DEFAULT`](#adr-no-vlm-default), [`RISK-OCR-ACCURACY`](#risk-ocr-accuracy), [`SEC-NONET`](#sec-nonet), [`TEST-OCR`](#test-ocr) |
| [`OCR-LANGS`](#ocr-langs) | OCR | [`MOD-OCR`](#mod-ocr), [`OCR-PIPELINE`](#ocr-pipeline) |
| [`OCR-PIPELINE`](#ocr-pipeline) | OCR | [`FLOW-OCR-AUTO`](#flow-ocr-auto), [`MOD-PDF`](#mod-pdf), [`PH-2`](#ph-2) |
| [`OCR-TESS-CLI`](#ocr-tess-cli) | OCR | [`DATA-OCRRESULT`](#data-ocrresult), [`DOC-AGENT-RULES`](#doc-agent-rules), [`MOD-OCR`](#mod-ocr), [`OCR-PIPELINE`](#ocr-pipeline) |
| [`OCR-TEXTLAYER`](#ocr-textlayer) | OCR | [`FLOW-OCR-MANUAL`](#flow-ocr-manual), [`MOD-OCR`](#mod-ocr), [`MOD-PDF`](#mod-pdf), [`RISK-OVERLAY-FONT`](#risk-overlay-font) |
| [`PERF-BUDGETS`](#perf-budgets) | PERF | [`ADR-NATIVE-UI`](#adr-native-ui), [`ADR-TESS-SIDECAR`](#adr-tess-sidecar), [`DOC-AGENT-RULES`](#doc-agent-rules), [`GOAL-FAST`](#goal-fast), [`MOD-UI`](#mod-ui), [`OCR-LANGS`](#ocr-langs), [`OCR-TESS-CLI`](#ocr-tess-cli), [`PH-1`](#ph-1), [`PH-7`](#ph-7) |
| [`PERF-IO`](#perf-io) | PERF | [`PERF-BUDGETS`](#perf-budgets) |
| [`PERF-LAZY`](#perf-lazy) | PERF | [`PERF-BUDGETS`](#perf-budgets) |
| [`PERF-LOSSLESS`](#perf-lossless) | PERF | [`DOC-TOOLS`](#doc-tools), [`MOD-PDF`](#mod-pdf), [`PERF-BUDGETS`](#perf-budgets) |
| [`PERF-OCR`](#perf-ocr) | PERF | [`PERF-BUDGETS`](#perf-budgets) |
| [`PERF-PARALLEL`](#perf-parallel) | PERF | [`PERF-BUDGETS`](#perf-budgets), [`RISK-PDFIUM-THREADS`](#risk-pdfium-threads) |
| [`PH-0`](#ph-0) | PH | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`PH-1`](#ph-1) | PH | [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-TRACE`](#doc-trace), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-REMOVE`](#tool-remove), [`TOOL-ROTATE`](#tool-rotate) |
| [`PH-2`](#ph-2) | PH | [`DOC-TRACE`](#doc-trace), [`TOOL-OCR`](#tool-ocr) |
| [`PH-3`](#ph-3) | PH | [`DOC-TRACE`](#doc-trace), [`MOD-WORKER`](#mod-worker), [`TOOL-COMPRESS`](#tool-compress), [`TOOL-CROP`](#tool-crop), [`TOOL-EXTRACT`](#tool-extract), [`TOOL-IMG2PDF`](#tool-img2pdf), [`TOOL-MERGE`](#tool-merge), [`TOOL-PAGENUM`](#tool-pagenum), [`TOOL-PDF2IMG`](#tool-pdf2img), [`TOOL-PROTECT`](#tool-protect), [`TOOL-SPLIT`](#tool-split), [`TOOL-UNLOCK`](#tool-unlock), [`TOOL-WATERMARK`](#tool-watermark) |
| [`PH-4`](#ph-4) | PH | [`DOC-UI`](#doc-ui), [`PH-PLAN`](#ph-plan), [`RISK-WIN-SHELL`](#risk-win-shell) |
| [`PH-5`](#ph-5) | PH | [`DOC-IPC`](#doc-ipc), [`DOC-TRACE`](#doc-trace), [`PH-PLAN`](#ph-plan), [`RISK-WIA`](#risk-wia), [`RISK-WIN-SHELL`](#risk-win-shell), [`TOOL-SCAN`](#tool-scan) |
| [`PH-6`](#ph-6) | PH | [`DOC-IPC`](#doc-ipc), [`DOC-TRACE`](#doc-trace), [`MOD-SHELL`](#mod-shell) |
| [`PH-7`](#ph-7) | PH | [`DOC-BUILD`](#doc-build), [`DOC-TRACE`](#doc-trace), [`MOD-SHELL`](#mod-shell), [`TOOL-METADATA`](#tool-metadata), [`TOOL-REPAIR`](#tool-repair) |
| [`PH-PLAN`](#ph-plan) | PH | [`DOC-AGENT-RULES`](#doc-agent-rules), [`GOAL-AGENT`](#goal-agent) |
| [`PRESET-FORMAT`](#preset-format) | PRESET | [`DATA-SPLITRULE`](#data-splitrule), [`PH-6`](#ph-6) |
| [`PROC-MAIN`](#proc-main) | PROC | [`DOC-SHAPE`](#doc-shape) |
| [`PROC-TESS`](#proc-tess) | PROC | [`DOC-SHAPE`](#doc-shape) |
| [`PROC-TRAY`](#proc-tray) | PROC | [`DOC-SHAPE`](#doc-shape) |
| [`PROC-UI`](#proc-ui) | PROC | (nothing; entry point or leaf) |
| [`PROC-WHEEL`](#proc-wheel) | PROC | [`DOC-SHAPE`](#doc-shape) |
| [`PROC-WORKER`](#proc-worker) | PROC | [`ADR-WORKER-PROCS`](#adr-worker-procs), [`DOC-SHAPE`](#doc-shape), [`FLOW-IMPORT`](#flow-import), [`MOD-JOBS`](#mod-jobs), [`MOD-PDF`](#mod-pdf), [`OCR-PIPELINE`](#ocr-pipeline), [`SEC-ISOLATE`](#sec-isolate) |
| [`RISK-AV`](#risk-av) | RISK | [`SEC-SIGN`](#sec-sign) |
| [`RISK-COMPRESS`](#risk-compress) | RISK | [`DOC-TOOLS`](#doc-tools) |
| [`RISK-EXPLORER-MULTI`](#risk-explorer-multi) | RISK | [`MOD-CLI`](#mod-cli), [`PH-6`](#ph-6) |
| [`RISK-FORMS-MERGE`](#risk-forms-merge) | RISK | [`DOC-TOOLS`](#doc-tools), [`MOD-PDF`](#mod-pdf) |
| [`RISK-LARGE-SCANS`](#risk-large-scans) | RISK | [`SEC-LIMITS`](#sec-limits) |
| [`RISK-LOPDF-ENC`](#risk-lopdf-enc) | RISK | [`ADR-PDFIUM-LOPDF`](#adr-pdfium-lopdf), [`DOC-TOOLS`](#doc-tools), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3) |
| [`RISK-OCR-ACCURACY`](#risk-ocr-accuracy) | RISK | [`TEST-OCR`](#test-ocr) |
| [`RISK-OVERLAY-FONT`](#risk-overlay-font) | RISK | [`OCR-TEXTLAYER`](#ocr-textlayer) |
| [`RISK-PDFIUM-THREADS`](#risk-pdfium-threads) | RISK | [`MOD-PDF`](#mod-pdf), [`PERF-PARALLEL`](#perf-parallel) |
| [`RISK-SMARTSCREEN`](#risk-smartscreen) | RISK | [`SEC-SIGN`](#sec-sign) |
| [`RISK-WEBVIEW2`](#risk-webview2) | RISK | [`DOC-BUILD`](#doc-build) |
| [`RISK-WHEEL-HOTKEY`](#risk-wheel-hotkey) | RISK | (nothing; entry point or leaf) |
| [`RISK-WIA`](#risk-wia) | RISK | [`ADR-TAURI`](#adr-tauri), [`ADR-WIA`](#adr-wia), [`MOD-SCAN`](#mod-scan), [`PH-5`](#ph-5), [`SCAN-FAILURES`](#scan-failures), [`TEST-MATRIX`](#test-matrix) |
| [`RISK-WIN-SHELL`](#risk-win-shell) | RISK | [`PH-4`](#ph-4), [`TEST-MATRIX`](#test-matrix), [`TEST-WHEEL`](#test-wheel) |
| [`SCAN-BACKEND`](#scan-backend) | SCAN | [`MOD-SCAN`](#mod-scan) |
| [`SCAN-FAILURES`](#scan-failures) | SCAN | [`MOD-SCAN`](#mod-scan) |
| [`SCAN-THREADING`](#scan-threading) | SCAN | [`MOD-SCAN`](#mod-scan) |
| [`SCAN-WIA`](#scan-wia) | SCAN | [`MOD-SCAN`](#mod-scan) |
| [`SEC-HONESTY`](#sec-honesty) | SEC | [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-UI`](#doc-ui) |
| [`SEC-IPC`](#sec-ipc) | SEC | [`DOC-IPC`](#doc-ipc), [`MOD-APP`](#mod-app), [`PROC-UI`](#proc-ui) |
| [`SEC-ISOLATE`](#sec-isolate) | SEC | [`DOC-AGENT-RULES`](#doc-agent-rules), [`GOAL-SECURE`](#goal-secure), [`MOD-JOBS`](#mod-jobs) |
| [`SEC-LIMITS`](#sec-limits) | SEC | [`FLOW-IMPORT`](#flow-import), [`MOD-SCAN`](#mod-scan), [`RISK-LARGE-SCANS`](#risk-large-scans), [`SCAN-FAILURES`](#scan-failures) |
| [`SEC-LOG`](#sec-log) | SEC | [`DOC-AGENT-RULES`](#doc-agent-rules) |
| [`SEC-NONET`](#sec-nonet) | SEC | [`ADR-OFFLINE-ASSETS`](#adr-offline-assets), [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-BUILD`](#doc-build), [`DOC-UI`](#doc-ui), [`GOAL-SECURE`](#goal-secure), [`SCAN-BACKEND`](#scan-backend) |
| [`SEC-PATHS`](#sec-paths) | SEC | [`CLI-GRAMMAR`](#cli-grammar), [`DATA-WORKERPROTO`](#data-workerproto), [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-IPC`](#doc-ipc), [`FLOW-IMPORT`](#flow-import), [`MOD-STORE`](#mod-store), [`PROC-TRAY`](#proc-tray) |
| [`SEC-SECRETS`](#sec-secrets) | SEC | [`DOC-TOOLS`](#doc-tools), [`MOD-APP`](#mod-app), [`PRESET-FORMAT`](#preset-format) |
| [`SEC-SIGN`](#sec-sign) | SEC | [`DOC-BUILD`](#doc-build), [`RISK-SMARTSCREEN`](#risk-smartscreen) |
| [`SEC-SUPPLY`](#sec-supply) | SEC | [`DOC-AGENT-RULES`](#doc-agent-rules), [`DOC-BUILD`](#doc-build), [`EVT-ENGINES`](#evt-engines), [`FLOW-LIFECYCLE`](#flow-lifecycle), [`MOD-APP`](#mod-app) |
| [`SEC-TB1`](#sec-tb1) | SEC | [`SEC-IPC`](#sec-ipc), [`SEC-SECRETS`](#sec-secrets) |
| [`SEC-TB2`](#sec-tb2) | SEC | [`SEC-ISOLATE`](#sec-isolate), [`SEC-LIMITS`](#sec-limits) |
| [`SEC-TB3`](#sec-tb3) | SEC | [`SEC-ISOLATE`](#sec-isolate) |
| [`SEC-TB4`](#sec-tb4) | SEC | [`SEC-PATHS`](#sec-paths), [`SEC-TEMP`](#sec-temp) |
| [`SEC-TEMP`](#sec-temp) | SEC | [`MOD-STORE`](#mod-store) |
| [`TEST-COMPRESS`](#test-compress) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-COMPRESS`](#tool-compress) |
| [`TEST-CROP`](#test-crop) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-CROP`](#tool-crop) |
| [`TEST-EXPLORER`](#test-explorer) | TEST | [`PH-6`](#ph-6) |
| [`TEST-FIXTURES`](#test-fixtures) | TEST | [`DOC-LAYOUT`](#doc-layout), [`DOC-PERF`](#doc-perf) |
| [`TEST-IMAGE`](#test-image) | TEST | [`MOD-IMAGE`](#mod-image), [`PH-1`](#ph-1) |
| [`TEST-IMG2PDF`](#test-img2pdf) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-IMG2PDF`](#tool-img2pdf) |
| [`TEST-JOBS`](#test-jobs) | TEST | [`DOC-TRACE`](#doc-trace), [`MOD-JOBS`](#mod-jobs), [`PH-1`](#ph-1), [`SEC-ISOLATE`](#sec-isolate) |
| [`TEST-MATRIX`](#test-matrix) | TEST | [`DOC-BUILD`](#doc-build), [`NG-XPLAT`](#ng-xplat), [`SCAN-BACKEND`](#scan-backend) |
| [`TEST-MERGE`](#test-merge) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-MERGE`](#tool-merge) |
| [`TEST-METADATA`](#test-metadata) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-METADATA`](#tool-metadata) |
| [`TEST-OCR`](#test-ocr) | TEST | [`ADR-NO-VLM-DEFAULT`](#adr-no-vlm-default), [`DOC-TRACE`](#doc-trace), [`MOD-OCR`](#mod-ocr), [`OCR-ENGINES`](#ocr-engines), [`OCR-LANGS`](#ocr-langs), [`PH-2`](#ph-2), [`RISK-OCR-ACCURACY`](#risk-ocr-accuracy), [`TOOL-OCR`](#tool-ocr) |
| [`TEST-OPS`](#test-ops) | TEST | [`DATA-OP`](#data-op), [`DOC-TRACE`](#doc-trace), [`MOD-CORE`](#mod-core), [`PH-1`](#ph-1) |
| [`TEST-PAGEOPS`](#test-pageops) | TEST | [`DOC-TRACE`](#doc-trace), [`MOD-APP`](#mod-app), [`TOOL-EXTRACT`](#tool-extract), [`TOOL-ORGANIZE`](#tool-organize), [`TOOL-REMOVE`](#tool-remove), [`TOOL-ROTATE`](#tool-rotate) |
| [`TEST-PDF2IMG`](#test-pdf2img) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-PDF2IMG`](#tool-pdf2img) |
| [`TEST-PRESET`](#test-preset) | TEST | [`PH-6`](#ph-6) |
| [`TEST-PROTECT`](#test-protect) | TEST | [`DOC-TRACE`](#doc-trace), [`MOD-APP`](#mod-app), [`SEC-HONESTY`](#sec-honesty), [`SEC-SECRETS`](#sec-secrets), [`TOOL-PROTECT`](#tool-protect), [`TOOL-UNLOCK`](#tool-unlock) |
| [`TEST-REPAIR`](#test-repair) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-REPAIR`](#tool-repair) |
| [`TEST-SCAN`](#test-scan) | TEST | [`DOC-TRACE`](#doc-trace), [`MOD-SCAN`](#mod-scan), [`PH-5`](#ph-5), [`TOOL-SCAN`](#tool-scan) |
| [`TEST-SEC`](#test-sec) | TEST | [`GOAL-SECURE`](#goal-secure), [`MOD-APP`](#mod-app), [`MOD-STORE`](#mod-store), [`PH-4`](#ph-4), [`PH-7`](#ph-7), [`SEC-IPC`](#sec-ipc), [`SEC-ISOLATE`](#sec-isolate), [`SEC-LIMITS`](#sec-limits), [`SEC-NONET`](#sec-nonet), [`SEC-PATHS`](#sec-paths), [`SEC-TEMP`](#sec-temp) |
| [`TEST-SPLIT`](#test-split) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-SPLIT`](#tool-split) |
| [`TEST-STAMP`](#test-stamp) | TEST | [`DOC-TRACE`](#doc-trace), [`TOOL-PAGENUM`](#tool-pagenum), [`TOOL-WATERMARK`](#tool-watermark) |
| [`TEST-UI`](#test-ui) | TEST | [`TEST-MATRIX`](#test-matrix) |
| [`TEST-UNDO`](#test-undo) | TEST | [`DOC-TRACE`](#doc-trace), [`MOD-CORE`](#mod-core), [`PH-1`](#ph-1) |
| [`TEST-WHEEL`](#test-wheel) | TEST | [`GOAL-WHEEL`](#goal-wheel) |
| [`TOOL-COMPARE`](#tool-compare) | TOOL | [`DOC-TOOLS`](#doc-tools) |
| [`TOOL-COMPRESS`](#tool-compress) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`FLOW-EXPORT`](#flow-export), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3), [`RISK-COMPRESS`](#risk-compress) |
| [`TOOL-CROP`](#tool-crop) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3), [`SEC-HONESTY`](#sec-honesty) |
| [`TOOL-EXTRACT`](#tool-extract) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-3`](#ph-3) |
| [`TOOL-HTML2PDF`](#tool-html2pdf) | TOOL | [`DOC-TOOLS`](#doc-tools) |
| [`TOOL-IMG2PDF`](#tool-img2pdf) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-IMAGE`](#mod-image), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3) |
| [`TOOL-MERGE`](#tool-merge) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-3`](#ph-3), [`RISK-FORMS-MERGE`](#risk-forms-merge) |
| [`TOOL-METADATA`](#tool-metadata) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-7`](#ph-7) |
| [`TOOL-OCR`](#tool-ocr) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-2`](#ph-2) |
| [`TOOL-ORGANIZE`](#tool-organize) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-1`](#ph-1) |
| [`TOOL-PAGENUM`](#tool-pagenum) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3) |
| [`TOOL-PDF2IMG`](#tool-pdf2img) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3) |
| [`TOOL-PROTECT`](#tool-protect) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`FLOW-EXPORT`](#flow-export), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3), [`RISK-LOPDF-ENC`](#risk-lopdf-enc) |
| [`TOOL-REDACT`](#tool-redact) | TOOL | [`DOC-TOOLS`](#doc-tools), [`SEC-HONESTY`](#sec-honesty) |
| [`TOOL-REMOVE`](#tool-remove) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-1`](#ph-1) |
| [`TOOL-REPAIR`](#tool-repair) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-7`](#ph-7) |
| [`TOOL-ROTATE`](#tool-rotate) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`PH-1`](#ph-1) |
| [`TOOL-SCAN`](#tool-scan) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace) |
| [`TOOL-SIGN-VISUAL`](#tool-sign-visual) | TOOL | [`DOC-TOOLS`](#doc-tools) |
| [`TOOL-SPLIT`](#tool-split) | TOOL | [`DATA-SPLITRULE`](#data-splitrule), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`FLOW-EXPORT`](#flow-export), [`PH-3`](#ph-3) |
| [`TOOL-UNLOCK`](#tool-unlock) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`NG-CRACK`](#ng-crack), [`PH-3`](#ph-3), [`RISK-LOPDF-ENC`](#risk-lopdf-enc) |
| [`TOOL-WATERMARK`](#tool-watermark) | TOOL | [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`MOD-PDF`](#mod-pdf), [`PH-3`](#ph-3) |
| [`UI-DRAWER`](#ui-drawer) | UI | [`CMD-TOOL-RUN`](#cmd-tool-run), [`CMD-UNLOCK-PW`](#cmd-unlock-pw), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`FLOW-TOOL-RUN`](#flow-tool-run), [`GOAL-COMPACT`](#goal-compact) |
| [`UI-EXPORTDLG`](#ui-exportdlg) | UI | [`CMD-EXPORT`](#cmd-export), [`DOC-TRACE`](#doc-trace), [`FLOW-EXPORT`](#flow-export) |
| [`UI-JOBBAR`](#ui-jobbar) | UI | [`CMD-JOB-CANCEL`](#cmd-job-cancel), [`CMD-REVEAL`](#cmd-reveal), [`CMD-SCAN-CANCEL`](#cmd-scan-cancel), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`FLOW-TOOL-RUN`](#flow-tool-run) |
| [`UI-OCRPANEL`](#ui-ocrpanel) | UI | [`CMD-OCR-RESULT`](#cmd-ocr-result), [`CMD-OCR-RUN`](#cmd-ocr-run), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`FLOW-OCR-MANUAL`](#flow-ocr-manual), [`OCR-CONF-UI`](#ocr-conf-ui) |
| [`UI-PRESETS`](#ui-presets) | UI | [`CMD-PRESETS`](#cmd-presets), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`PRESET-FORMAT`](#preset-format) |
| [`UI-PREVIEW`](#ui-preview) | UI | [`CMD-APPLY`](#cmd-apply), [`CMD-OCR-RESULT`](#cmd-ocr-result), [`CMD-RENDER`](#cmd-render), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui) |
| [`UI-SCANDLG`](#ui-scandlg) | UI | [`CMD-SCAN-DEVICES`](#cmd-scan-devices), [`CMD-SCAN-START`](#cmd-scan-start), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`PH-5`](#ph-5) |
| [`UI-SEARCH`](#ui-search) | UI | [`CMD-SEARCH`](#cmd-search), [`DOC-TRACE`](#doc-trace) |
| [`UI-SETTINGS`](#ui-settings) | UI | [`CMD-SETTINGS`](#cmd-settings) |
| [`UI-SHORTCUTS`](#ui-shortcuts) | UI | [`DOC-UI`](#doc-ui) |
| [`UI-TOOLBAR`](#ui-toolbar) | UI | [`CMD-IMPORT`](#cmd-import), [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-UNDO`](#cmd-undo), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`FLOW-IMPORT`](#flow-import) |
| [`UI-TRAY`](#ui-tray) | UI | [`CMD-APPLY`](#cmd-apply), [`CMD-IMPORT`](#cmd-import), [`CMD-OCR-RUN`](#cmd-ocr-run), [`CMD-PAGES`](#cmd-pages), [`CMD-THUMB`](#cmd-thumb), [`DATA-WORKSPACE`](#data-workspace), [`DOC-TOOLS`](#doc-tools), [`DOC-TRACE`](#doc-trace), [`DOC-UI`](#doc-ui), [`PERF-LAZY`](#perf-lazy) |
| [`UI-WHEEL`](#ui-wheel) | UI | (nothing; entry point or leaf) |
<!-- END GENERATED appendix-a -->

## <a id="appendix-b"></a>Appendix B. Open VERIFY items (generated)

Each is a claim not yet proven. Resolve by testing, edit the text, remove the marker. Generated by `tools/link-check --write`.

<!-- BEGIN GENERATED appendix-b -->
| Where | Item |
|---|---|
| [`OCR-TESS-CLI`](#ocr-tess-cli) | VERIFY best setting by benchmark, PERF-BUDGETS |
| [`SCAN-WIA`](#scan-wia) | document handling: feeder/flatbed/duplex, x/y resolution, data type, page size; exact property IDs from `wiadef.h`, VERIFY on target driver |
<!-- END GENERATED appendix-b -->
