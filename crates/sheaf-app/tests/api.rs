//! TEST-SEC (IPC checks), TEST-PAGEOPS and the job flow through the command
//! layer: what the Tauri shell calls, driven without a window. Needs the
//! worker binary (`cargo build --workspace`) and the vendored PDFium.

use fixture_gen::{noto_font, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, Scratch};
use serde_json::{json, Value};
use sheaf_app::api::{Backend, BackendConfig, Emit, Event, ExportSpecDto, OcrPolicyDto, PagesDto, SettingsPatch, UiOp};
use sheaf_app::{sibling_worker, Engines};
use sheaf_core::{ErrorCode, NormRect};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Collector(Mutex<Vec<Event>>);

impl Emit for Collector {
    fn emit(&self, event: Event) {
        self.0.lock().expect("events").push(event);
    }
}

impl Collector {
    fn all(&self) -> Vec<Event> {
        self.0.lock().expect("events").clone()
    }
    fn count(&self, name: &str) -> usize {
        self.all().iter().filter(|e| e.name() == name).count()
    }
}

struct Rig {
    backend: Arc<Backend>,
    events: Arc<Collector>,
    // Declared last so it is dropped last: the session lives in `base`.
    base: Scratch,
    work: Scratch,
}

fn engines() -> Engines {
    Engines {
        worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
        pdfium_dir: vendored_pdfium_dir(),
        tesseract: Some((tesseract_exe(), tessdata_dir())),
        font: Some(noto_font()),
    }
}

fn rig_with(workers: Option<usize>, verify: bool, engines: Engines) -> Rig {
    let base = Scratch::new("api-base");
    let work = Scratch::new("api-work");
    let config = base.path().join("config");
    if let Some(n) = workers {
        std::fs::create_dir_all(&config).expect("config dir");
        std::fs::write(config.join("settings.json"), format!("{{\"workers\":{n}}}")).expect("settings");
    }
    let events = Arc::new(Collector::default());
    let backend = Backend::start(
        BackendConfig {
            engines,
            base_dir: base.path().join("sessions"),
            config_dir: Some(config),
            verify_vendor: verify,
        },
        events.clone(),
    )
    .expect("backend starts");
    Rig {
        backend,
        events,
        base,
        work,
    }
}

fn rig() -> Rig {
    rig_with(Some(2), false, engines())
}

impl Rig {
    fn import_pdf(&self, name: &str, pages: u32) -> (PathBuf, Vec<u64>) {
        let path = self.work.file(name, &text_pdf(pages));
        let reports = self.backend.import(vec![path.clone()]).expect("import");
        (path, reports[0].pages.clone())
    }

    fn page_list(&self) -> PagesDto {
        self.backend.pages(0, 1_000).expect("pages")
    }

    /// Wait for a job to finish and return its last view.
    fn wait(&self, job: u64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let view = self
                .backend
                .jobs()
                .into_iter()
                .find(|j| j.id == job)
                .map(|j| serde_json::to_value(j).expect("view"))
                .expect("job exists");
            if matches!(view["state"].as_str(), Some("done" | "failed" | "cancelled")) {
                return view;
            }
            assert!(Instant::now() < deadline, "job {job} did not finish: {view}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G'])
}

fn sha(path: &Path) -> [u8; 32] {
    sheaf_store::hash_file(path).expect("hash")
}

#[test]
fn import_lists_pages_and_serves_thumbnails_and_previews() {
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 3);
    let list = rig.page_list();
    assert_eq!((list.total, list.pages.len()), (3, 3));
    assert_eq!(list.pages.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
    assert!(list
        .pages
        .iter()
        .all(|p| p.kind == "source" && p.rot == 0 && p.ocr.state == "none"));
    assert!(rig.events.count("pages") >= 1 && rig.events.count("import") == 1);

    let thumb = rig.backend.thumb(ids[0], 256).expect("thumbnail");
    assert!(is_png(&thumb));
    let preview = rig.backend.thumb(ids[0], 900).expect("preview");
    assert!(
        is_png(&preview) && preview.len() > thumb.len(),
        "a preview is a bigger picture"
    );

    // A rotation changes that page's version and no other page's.
    let before = rig.page_list();
    rig.backend
        .apply(UiOp::Rotate {
            ids: vec![ids[1]],
            deg: 90,
        })
        .expect("rotate");
    let after = rig.page_list();
    assert_ne!(before.pages[1].ver, after.pages[1].ver);
    assert_eq!(before.pages[0].ver, after.pages[0].ver);
    assert_eq!(after.pages[1].rot, 90);
    let key = rig.backend.render_key(ids[1], 0.8).expect("render key");
    assert_eq!(key, format!("{}/{}/800", ids[1], after.pages[1].ver));
}

#[test]
fn page_ops_undo_and_events() {
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 4);
    rig.backend.apply(UiOp::Delete { ids: vec![ids[0]] }).expect("delete");
    rig.backend
        .apply(UiOp::Move {
            ids: vec![ids[3]],
            to: 0,
        })
        .expect("move");
    rig.backend
        .apply(UiOp::Duplicate { ids: vec![ids[1]] })
        .expect("duplicate");
    rig.backend
        .apply(UiOp::InsertBlank { at: 1, size: None })
        .expect("blank");
    let list = rig.page_list();
    assert_eq!(list.total, 5);
    assert_eq!(list.pages[1].kind, "blank");
    assert!(list.can_undo && !list.can_redo);
    let events_before = rig.events.count("pages");
    for _ in 0..4 {
        assert!(rig.backend.undo(false).expect("undo").is_some());
    }
    assert_eq!(rig.page_list().total, 4);
    // The import itself is an edit too: one more undo takes the file's pages out again.
    assert!(rig.backend.undo(false).expect("undo import").is_some());
    assert_eq!(rig.page_list().total, 0);
    assert!(rig.backend.undo(false).expect("nothing left").is_none());
    assert!(rig.backend.undo(true).expect("redo").is_some());
    assert_eq!(rig.page_list().total, 4);
    assert!(rig.events.count("pages") > events_before, "undo announces itself");
}

#[test]
fn bad_requests_are_refused_and_change_nothing() {
    // TEST-SEC: bad ids, huge ranges, out-of-range values.
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 3);
    let rev = rig.page_list().rev;
    let code = |r: Result<u64, sheaf_core::AppError>| r.expect_err("must be refused").code;

    assert_eq!(
        code(rig.backend.apply(UiOp::Delete { ids: vec![9_999] })),
        ErrorCode::NotFound
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::Delete { ids: vec![] })),
        ErrorCode::InvalidArg
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::Delete {
            ids: vec![ids[0], ids[0]]
        })),
        ErrorCode::InvalidArg,
        "a page listed twice"
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::Rotate {
            ids: vec![ids[0]],
            deg: 45
        })),
        ErrorCode::InvalidArg
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::Move {
            ids: vec![ids[0]],
            to: usize::MAX
        })),
        ErrorCode::InvalidArg
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::InsertBlank {
            at: usize::MAX,
            size: None
        })),
        ErrorCode::InvalidArg
    );
    assert_eq!(
        code(rig.backend.apply(UiOp::InsertBlank {
            at: 0,
            size: Some((f32::INFINITY, 100.0))
        })),
        ErrorCode::InvalidArg
    );
    let rect = |x0, y0, x1, y1| NormRect { x0, y0, x1, y1 };
    for bad in [
        rect(0.5, 0.0, 0.2, 1.0),
        rect(-1.0, 0.0, 1.0, 1.0),
        rect(0.0, 0.0, f32::NAN, 1.0),
    ] {
        assert_eq!(
            code(rig.backend.apply(UiOp::Crop {
                ids: vec![ids[0]],
                rect: Some(bad)
            })),
            ErrorCode::InvalidArg
        );
    }
    let huge: Vec<u64> = (0..60_000).collect();
    assert_eq!(
        code(rig.backend.apply(UiOp::Delete { ids: huge })),
        ErrorCode::LimitExceeded
    );
    assert_eq!(rig.page_list().rev, rev, "nothing above changed the workspace");

    // The wire format has no way to say "insert": pages come in through import only.
    let insert: Result<UiOp, _> = serde_json::from_value(json!({
        "op": "insert", "at": 0, "pages": [{"content": {"Source": {"src": 0, "index": 0}}, "has_text": null}]
    }));
    assert!(insert.is_err());
    assert!(serde_json::from_value::<UiOp>(json!({"op": "rotate", "ids": [1], "deg": "x"})).is_err());

    // Ranges and sizes.
    let all = rig
        .backend
        .pages(usize::MAX, usize::MAX)
        .expect("range past the end is empty");
    assert!(all.pages.is_empty() && all.total == 3);
    assert_eq!(rig.backend.pages(0, usize::MAX).expect("capped").pages.len(), 3);
    assert_eq!(
        rig.backend.thumb(9_999, 256).expect_err("unknown page").code,
        ErrorCode::NotFound
    );
    for size in [0, 31, 4_097, u32::MAX] {
        assert_eq!(
            rig.backend.thumb(ids[0], size).expect_err("size").code,
            ErrorCode::InvalidArg,
            "{size}"
        );
    }
    for zoom in [f32::NAN, -1.0, 0.0, f32::INFINITY] {
        assert!(rig.backend.render_key(ids[0], zoom).is_err(), "{zoom}");
    }
    assert!(rig.backend.search(&"x".repeat(1_000)).is_err());
    assert!(rig.backend.search("").expect("empty query").is_empty());
}

#[test]
fn jobs_and_tools_reject_bad_input() {
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 2);
    let err = |r: Result<u64, sheaf_core::AppError>| r.expect_err("must be refused");

    assert_eq!(
        err(rig.backend.tool_run("rm-rf", json!({}), None)).code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        err(rig.backend.tool_run("repair", json!({}), None)).code,
        ErrorCode::Unsupported
    );
    assert_eq!(
        err(rig.backend.tool_run("compress", json!({"level": "Ludicrous"}), None)).code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig
            .backend
            .tool_run("compress", json!({"level": "Low", "outDir": "C:\\"}), None))
        .code,
        ErrorCode::InvalidArg,
        "unknown parameters, such as a path, are not accepted"
    );
    assert_eq!(
        err(rig
            .backend
            .tool_run("pdf2img", json!({"format": "png", "dpi": 100_000}), None))
        .code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig.backend.tool_run("pdf2img", json!({"format": "bmp"}), None)).code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig
            .backend
            .tool_run("pdf2img", json!({"mode": "extract"}), Some(ids.clone())))
        .code,
        ErrorCode::InvalidArg,
        "extract takes exactly one page"
    );
    // A stamp can name an image only by a token this app issued.
    let mut stamp = serde_json::to_value(sheaf_core::Stamp::watermark("DRAFT")).expect("stamp");
    stamp["image"] = json!("../../etc/passwd");
    assert_eq!(
        err(rig.backend.tool_run("stamp", json!({"stamps": [stamp]}), None)).code,
        ErrorCode::NotFound
    );
    let spec = |f: &dyn Fn(&mut ExportSpecDto)| {
        let mut s = ExportSpecDto::default();
        f(&mut s);
        s
    };
    assert_eq!(
        err(rig.backend.export(spec(&|s| s.format = "docx".into()))).code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig.backend.export(spec(&|s| s.name = "x".repeat(500)))).code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig.backend.export(spec(&|s| s.name = "a\0b".into()))).code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig.backend.export(spec(&|s| s.pages = Some(vec![123_456])))).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        err(rig.backend.ocr_run(
            None,
            OcrPolicyDto {
                mode: "magic".into(),
                langs: None
            }
        ))
        .code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        err(rig.backend.ocr_run(
            Some(ids.clone()),
            OcrPolicyDto {
                mode: "add".into(),
                langs: Some(vec!["eng; rm -rf /".into()])
            }
        ))
        .code,
        ErrorCode::InvalidArg,
        "language names end up on a command line"
    );
    assert_eq!(
        rig.backend.ocr_result(ids[0]).expect_err("not recognized").code,
        ErrorCode::NotFound
    );
    assert_eq!(
        rig.backend.unlock(77, "x".into()).expect_err("no such source").code,
        ErrorCode::NotFound
    );
    assert_eq!(
        rig.backend.unlock(0, "x".into()).expect_err("not locked").code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        rig.backend.reveal_path(424_242, 0).expect_err("no job").code,
        ErrorCode::NotFound
    );
    rig.backend.job_cancel(424_242); // unknown ids are ignored

    // Scanning and presets arrive in later phases and say so.
    assert_eq!(rig.backend.scan_unavailable().code, ErrorCode::Unsupported);
    assert_eq!(rig.backend.presets_unavailable().code, ErrorCode::Unsupported);
    assert!(rig.backend.presets_list().is_empty());
}

#[test]
fn import_refuses_what_the_path_policy_refuses() {
    // TEST-SEC: paths. (Windows device names and streams are covered in sheaf-store.)
    let rig = rig();
    let missing = rig.work.path().join("missing.pdf");
    let dir = rig.work.path().to_path_buf();
    for path in [missing, dir, PathBuf::new(), PathBuf::from("a\0b.pdf")] {
        let e = rig.backend.import(vec![path]).expect_err("refused");
        assert!(
            matches!(e.code, ErrorCode::NotFound | ErrorCode::PathRejected | ErrorCode::Io),
            "{e}"
        );
    }
    // Not a PDF: the worker says so and the app carries on.
    let junk = rig.work.file("junk.pdf", b"this is not a pdf");
    assert!(rig.backend.import(vec![junk]).is_err());
    assert_eq!(rig.page_list().total, 0);
    // One bad file among good ones: the good ones come in, the bad one is announced.
    let good = rig.work.file("good.pdf", &text_pdf(2));
    let junk = rig.work.file("junk2.pdf", b"nope");
    let reports = rig.backend.import(vec![junk, good]).expect("partial success");
    assert_eq!(reports.len(), 1);
    assert_eq!(rig.page_list().total, 2);
    assert!(rig.events.count("error") >= 1);
}

#[test]
fn export_runs_as_a_job_and_never_overwrites() {
    let rig = rig();
    let (src, ids) = rig.import_pdf("report.pdf", 3);
    let original = sha(&src);
    rig.backend.apply(UiOp::Delete { ids: vec![ids[1]] }).expect("delete");

    let job = rig
        .backend
        .export(ExportSpecDto {
            name: "{source}".into(), // would be the source's own name
            ..Default::default()
        })
        .expect("export starts");
    let done = rig.wait(job);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["progress"], 1.0);
    let out = rig.backend.reveal_path(job, 0).expect("reveal the output");
    assert_ne!(out, src, "the source is never the target");
    assert_eq!(out.parent(), src.parent(), "beside the source by default");
    assert_eq!(sha(&src), original, "the source is untouched");
    assert_eq!(done["detail"]["files"][0]["pages"], 2);
    assert!(rig.backend.reveal_path(job, 7).is_err());

    // The events told the story: queued/running, then done.
    let states: Vec<String> = rig
        .events
        .all()
        .iter()
        .filter_map(|e| match e {
            Event::Job(v) if v.id == job => Some(v.state.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(states.first().map(String::as_str), Some("queued"));
    assert_eq!(states.last().map(String::as_str), Some("done"));

    // Only paths this app produced can be revealed: a job that wrote nothing has none.
    let img = rig
        .backend
        .tool_run("pdf2img", json!({"format": "png", "dpi": 50}), Some(vec![ids[0]]))
        .expect("images");
    let done = rig.wait(img);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["detail"]["count"], 1);
    let file = rig.backend.reveal_path(img, 0).expect("image");
    assert!(file.is_file() && is_png(&std::fs::read(file).expect("read")));
}

#[test]
fn output_folder_is_chosen_by_the_shell_and_checked() {
    let rig = rig();
    let (src, _) = rig.import_pdf("doc.pdf", 1);
    let out = rig.work.path().join("out");
    std::fs::create_dir(&out).expect("out dir");
    assert!(
        rig.backend.set_output_dir(Some(&src)).is_err(),
        "a file is not a folder"
    );
    assert!(
        rig.backend.set_output_dir(Some(&out.join("nope"))).is_err(),
        "must exist"
    );
    let dto = rig.backend.set_output_dir(Some(&out)).expect("folder accepted");
    assert!(dto.output_dir.is_some());

    let job = rig.backend.export(ExportSpecDto::default()).expect("export");
    let done = rig.wait(job);
    assert_eq!(done["state"], "done", "{done}");
    let file = rig.backend.reveal_path(job, 0).expect("output");
    assert_eq!(
        file.parent().map(|p| p.canonicalize().expect("canon")),
        Some(out.canonicalize().expect("canon"))
    );

    // Settings survive a restart; out-of-range values are refused.
    assert!(rig
        .backend
        .set_settings(SettingsPatch {
            language: Some("de".into()),
            ..Default::default()
        })
        .is_err());
    assert!(rig
        .backend
        .set_settings(SettingsPatch {
            theme: Some("dark".into()),
            language: Some("uz".into()),
            ..Default::default()
        })
        .is_ok());
    let saved = std::fs::read_to_string(rig.base.path().join("config").join("settings.json")).expect("settings.json");
    assert!(saved.contains("\"uz\"") && saved.contains("\"dark\""), "{saved}");
    assert!(!saved.contains("password"), "settings never hold secrets");
}

#[test]
fn protect_then_unlock_through_commands() {
    // TEST-PROTECT through the IPC surface: the password goes in, never comes back out.
    let rig = rig();
    let (_, _) = rig.import_pdf("secret.pdf", 2);
    let secret = "Tr0ub4dor-қулф";
    // The wire spelling is the core's own (snake_case); camelCase is refused, not guessed at.
    assert_eq!(
        rig.backend
            .tool_run(
                "protect",
                json!({"protect": {"userPw": secret, "allowPrint": true}}),
                None
            )
            .expect_err("wrong spelling")
            .code,
        ErrorCode::InvalidArg
    );

    let job = rig
        .backend
        .tool_run(
            "protect",
            json!({"protect": {"user_pw": secret, "owner_pw": "", "allow_print": true, "allow_copy": false, "allow_edit": false}}),
            None,
        )
        .expect("protect starts");
    let done = rig.wait(job);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["detail"]["files"][0]["encryption"], "AES-256");
    let locked = rig.backend.reveal_path(job, 0).expect("locked file");
    let text = serde_json::to_string(&rig.backend.jobs()).expect("jobs");
    assert!(!text.contains(secret), "job views never echo the password");

    let reports = rig.backend.import(vec![locked]).expect("import locked");
    assert!(reports[0].encrypted && reports[0].pages.is_empty());
    let source = reports[0].source;
    let wrong = rig
        .backend
        .unlock(source, "wrong".into())
        .expect("a wrong password is an answer, not an error");
    assert!(!wrong.ok && wrong.pages.is_empty());
    assert_eq!(rig.page_list().total, 2, "nothing was added by the failed attempt");
    let right = rig.backend.unlock(source, secret.into()).expect("unlock");
    assert!(right.ok);
    assert_eq!(right.pages.len(), 2);
    assert_eq!(rig.page_list().total, 4);
    assert!(rig.backend.thumb(right.pages[0], 128).is_ok(), "unlocked pages render");

    for entry in walk(rig.base.path())
        .into_iter()
        .chain(walk(&rig.backend.session_dir()))
    {
        let bytes = std::fs::read(&entry).unwrap_or_default();
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "password found in {}",
            entry.display()
        );
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn ocr_job_reports_each_page_and_can_be_cancelled() {
    // One worker, many pages: the first finishes while the rest wait, so a
    // cancel lands mid-job. Recognition is forced ("redo") on pages that have text.
    let rig = rig_with(Some(1), false, engines());
    let (_, ids) = rig.import_pdf("many.pdf", 8);
    let job = rig
        .backend
        .ocr_run(
            Some(ids.clone()),
            OcrPolicyDto {
                mode: "redo".into(),
                langs: Some(vec!["eng".into()]),
            },
        )
        .expect("ocr starts");
    let deadline = Instant::now() + Duration::from_secs(120);
    while rig.events.count("page-ready") == 0 {
        assert!(Instant::now() < deadline, "no page finished");
        std::thread::sleep(Duration::from_millis(10));
    }
    rig.backend.job_cancel(job);
    let done = rig.wait(job);
    assert_eq!(done["state"], "cancelled", "{done}");
    let list = rig.page_list();
    assert!(
        list.pages
            .iter()
            .all(|p| p.ocr.state == "done" || p.ocr.state == "none"),
        "no page is left waiting: {:?}",
        list.pages.iter().map(|p| p.ocr.state).collect::<Vec<_>>()
    );
    let recognized = list.pages.iter().filter(|p| p.ocr.state == "done").count();
    assert!(
        (1..8).contains(&recognized),
        "some pages finished before the cancel, not all: {recognized}"
    );

    // A finished page has words and is searchable.
    let page = list
        .pages
        .iter()
        .find(|p| p.ocr.state == "done")
        .expect("a recognized page");
    let words = rig.backend.ocr_result(page.id).expect("words");
    assert!(words.text.to_lowercase().contains("page"));
    assert!(
        words.image_px.0 > 1_000 && words.image_px.1 > 1_000,
        "an A4 page at 300 dpi: {:?}",
        words.image_px
    );
    assert!(!rig.backend.search("page").expect("search").is_empty());

    // Cropping changes the pixels, so the text read from them is dropped (FLOW-EDIT).
    rig.backend
        .apply(UiOp::Crop {
            ids: vec![page.id],
            rect: Some(NormRect {
                x0: 0.0,
                y0: 0.0,
                x1: 0.5,
                y1: 0.5,
            }),
        })
        .expect("crop");
    let after = rig.page_list();
    assert_eq!(
        after.pages.iter().find(|p| p.id == page.id).map(|p| p.ocr.state),
        Some("none")
    );
}

#[test]
fn text_only_ocr_writes_a_txt_beside_the_source() {
    let rig = rig();
    let (src, _) = rig.import_pdf("born-digital.pdf", 2);
    let job = rig
        .backend
        .ocr_run(
            None,
            OcrPolicyDto {
                mode: "text".into(),
                langs: Some(vec!["eng".into()]),
            },
        )
        .expect("start");
    let done = rig.wait(job);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(
        done["detail"]["skipped"], 2,
        "pages that have text are not recognized again"
    );
    let txt = rig.backend.reveal_path(job, 0).expect("txt");
    assert_eq!(txt.extension().and_then(|e| e.to_str()), Some("txt"));
    assert_eq!(txt.parent(), src.parent());
    assert!(std::fs::read_to_string(&txt).expect("read").contains("Page 1"));
}

#[test]
fn failed_hash_check_blocks_document_work() {
    // SEC-SUPPLY at startup: a bundled file that does not match its pinned hash stops the workers.
    #[cfg(unix)]
    {
        let vendor = Scratch::new("api-vendor");
        std::os::unix::fs::symlink(
            vendored_pdfium_dir().parent().expect("pdfium parent"),
            vendor.path().join("pdfium"),
        )
        .expect("link pdfium");
        std::fs::create_dir(vendor.path().join("tessdata")).expect("tessdata");
        std::fs::write(vendor.path().join("tessdata/eng.traineddata"), b"tampered").expect("file");
        std::fs::write(
            vendor.path().join("SHA256SUMS"),
            format!("{}  tessdata/eng.traineddata\n", "0".repeat(64)),
        )
        .expect("sums");
        let mut e = engines();
        e.pdfium_dir = vendor
            .path()
            .join("pdfium")
            .join(vendored_pdfium_dir().file_name().expect("platform"));
        let rig = rig_with(Some(1), true, e);
        let path = rig.work.file("doc.pdf", &text_pdf(1));
        let err = rig.backend.import(vec![path]).expect_err("blocked");
        assert_eq!(err.code, ErrorCode::EngineMissing);
        assert!(err.msg.contains("eng.traineddata"), "{err}");
        assert!(rig.events.count("error") >= 1 && rig.events.count("engines") == 1);
        assert_eq!(rig.backend.snapshot().expect("snapshot").engines.verified, Some(false));
    }
}

#[test]
fn shutdown_removes_the_session_dir_and_stops_commands() {
    // FLOW-LIFECYCLE, SEC-TEMP.
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 2);
    rig.backend.thumb(ids[0], 128).expect("thumbnail");
    let session = rig.backend.session_dir();
    assert!(session.is_dir());
    rig.backend.shutdown();
    drop(rig.backend);
    assert!(!session.exists(), "session dir is gone after shutdown");
}

#[cfg(target_os = "linux")]
#[test]
fn nothing_listens_on_a_port() {
    // TEST-SEC: no listening socket in this process or in any process it started
    // (workers, tesseract), after real work has run.
    use std::collections::HashSet;
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 2);
    rig.backend.thumb(ids[0], 128).expect("thumbnail");
    let job = rig
        .backend
        .ocr_run(
            Some(vec![ids[0]]),
            OcrPolicyDto {
                mode: "redo".into(),
                langs: Some(vec!["eng".into()]),
            },
        )
        .expect("ocr");
    rig.wait(job);

    let mut listening: HashSet<String> = HashSet::new();
    for table in ["tcp", "tcp6", "udp", "udp6"] {
        let text = std::fs::read_to_string(format!("/proc/net/{table}")).unwrap_or_default();
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            // tcp: state 0A is LISTEN. udp has no listen state: any bound socket counts.
            if cols.len() > 9 && (table.starts_with("udp") || cols[3] == "0A") {
                listening.insert(cols[9].to_string());
            }
        }
    }
    let me = std::process::id();
    let mut family = HashSet::from([me]);
    for _ in 0..4 {
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
                continue;
            };
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
            let ppid = stat
                .rsplit(')')
                .next()
                .and_then(|r| r.split_whitespace().nth(1))
                .and_then(|p| p.parse::<u32>().ok());
            if ppid.is_some_and(|p| family.contains(&p)) {
                family.insert(pid);
            }
        }
    }
    for pid in family {
        for fd in std::fs::read_dir(format!("/proc/{pid}/fd"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                let t = target.to_string_lossy();
                if let Some(inode) = t.strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
                    assert!(!listening.contains(inode), "process {pid} listens on socket {inode}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The wire contract with MOD-UI.
//
// `tools/ui-test/contract/shapes.json` records the *shape* of every payload the
// UI receives (field names and kinds, not values). This test builds the real
// payloads and compares; the UI's own tests (tools/ui-test/unit/contract.test.mjs)
// hold the fake backend, which the browser tests run against, to the same file.
// So a changed DTO fails here until the file and the UI are updated together.
//
//   SHEAF_WRITE_CONTRACT=1 cargo test -p sheaf-app --test api wire_shapes
// rewrites the file after an intended change.

fn shape(v: &Value) -> Value {
    match v {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("bool"),
        Value::Number(_) => json!("number"),
        Value::String(_) => json!("string"),
        Value::Array(a) => Value::Array(a.first().map(|x| vec![shape(x)]).unwrap_or_default()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, x)| (k.clone(), shape(x))).collect()),
    }
}

#[test]
fn wire_shapes_match_the_ui_contract() {
    let rig = rig();
    let (_, ids) = rig.import_pdf("doc.pdf", 2);
    let to = |v: &dyn erased::Ser| v.value();
    let mut samples = serde_json::Map::new();
    let mut put = |name: &str, v: Value| {
        samples.insert(name.to_string(), shape(&v));
    };

    put("snapshot", to(&rig.backend.snapshot().expect("snapshot")));
    put("pages", to(&rig.backend.pages(0, 10).expect("pages")));
    let more = rig.work.file("more.pdf", &text_pdf(1));
    put("import", to(&rig.backend.import(vec![more]).expect("import")));
    put("settings", to(&rig.backend.settings()));

    let job = rig.backend.export(ExportSpecDto::default()).expect("export");
    rig.wait(job);
    let view = rig.backend.jobs().into_iter().find(|j| j.id == job).expect("job");
    put("job.export", to(&view));

    let job = rig
        .backend
        .ocr_run(Some(vec![ids[0]]), OcrPolicyDto { mode: "redo".into(), langs: Some(vec!["eng".into()]) })
        .expect("ocr");
    rig.wait(job);
    put("job.ocr", to(&rig.backend.jobs().into_iter().find(|j| j.id == job).expect("job")));
    put("ocr_result", to(&rig.backend.ocr_result(ids[0]).expect("result")));
    put("search", to(&rig.backend.search("page").expect("search")));

    let events = rig.events.all();
    let payload = |name: &str| events.iter().find(|e| e.name() == name).map(|e| e.payload()).unwrap_or(Value::Null);
    put("event.pages", payload("pages"));
    put("event.page-ready", payload("page-ready"));
    put("event.import", payload("import"));
    put("event.job", payload("job"));

    let png = rig.work.file("logo.png", &[0x89, b'P', b'N', b'G']);
    put("pick", to(&rig.backend.register_pick(&png).expect("pick")));
    put(
        "error",
        to(&sheaf_app::api::ErrorDto::from(&sheaf_core::AppError::not_found("page"))),
    );
    put("engines", to(&rig.backend.snapshot().expect("snapshot").engines));

    let actual = Value::Object(samples);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/ui-test/contract/shapes.json");
    if std::env::var_os("SHEAF_WRITE_CONTRACT").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&actual).expect("json") + "\n").expect("write contract");
        return;
    }
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("shapes.json is committed")).expect("valid json");
    assert_eq!(
        actual,
        committed,
        "the payloads the UI receives changed: update tools/ui-test/e2e/fake-backend.js and the UI, then rerun with SHEAF_WRITE_CONTRACT=1"
    );
}

/// `serde_json::to_value` for anything serializable, through one trait object.
mod erased {
    pub trait Ser {
        fn value(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn value(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
        }
    }
}
