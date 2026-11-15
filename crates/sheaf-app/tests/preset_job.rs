//! CMD-PRESETS through the command layer: the preset files, and a run as an
//! app job on its own scratch session (the open document is not touched).

use fixture_gen::{noto_font, tessdata_dir, tesseract_exe, text_pdf, vendored_pdfium_dir, Scratch};
use serde_json::Value;
use sheaf_app::api::{Backend, BackendConfig, Emit, Event};
use sheaf_app::{sibling_worker, Engines};
use sheaf_core::{ErrorCode, Preset};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Quiet;
impl Emit for Quiet {
    fn emit(&self, _: Event) {}
}

fn backend(base: &Scratch) -> Arc<Backend> {
    Backend::start(
        BackendConfig {
            engines: Engines {
                worker: sibling_worker().expect("sheaf-worker is built (run `cargo build --workspace`)"),
                pdfium_dir: vendored_pdfium_dir(),
                tesseract: Some((tesseract_exe(), tessdata_dir())),
                font: Some(noto_font()),
            },
            base_dir: base.path().join("sessions"),
            config_dir: Some(base.path().join("config")),
            verify_vendor: false,
        },
        Arc::new(Quiet),
    )
    .expect("backend starts")
}

fn wait(b: &Backend, id: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let view = b
            .jobs()
            .into_iter()
            .find(|j| j.id == id)
            .map(|j| serde_json::to_value(j).expect("view"))
            .expect("job exists");
        if matches!(view["state"].as_str(), Some("done" | "failed" | "cancelled")) {
            return view;
        }
        assert!(Instant::now() < deadline, "preset job did not finish: {view}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn presets_are_saved_listed_loaded_and_deleted_through_the_backend() {
    let base = Scratch::new("pj-base");
    let b = backend(&base);
    assert!(b.presets_list().is_empty());
    let saved = b
        .preset_save(Preset {
            name: "Archive".into(),
            compress: Some("Extreme".into()),
            ..Preset::default()
        })
        .expect("save");
    assert_eq!(saved.name, "Archive");
    assert_eq!(b.presets_list(), ["Archive"]);
    assert_eq!(
        b.preset_get("Archive").expect("get").compress.as_deref(),
        Some("Extreme")
    );

    let bad = Preset {
        name: "Bad".into(),
        ocr: sheaf_core::PresetOcr {
            mode: "Maybe".into(),
            langs: None,
        },
        ..Preset::default()
    };
    assert_eq!(b.preset_save(bad).expect_err("invalid").code, ErrorCode::InvalidArg);

    // The default preset is forgotten when its preset goes.
    b.set_settings(sheaf_app::api::SettingsPatch {
        default_preset: Some("Archive".into()),
        ..Default::default()
    })
    .expect("default");
    b.preset_delete("Archive").expect("delete");
    assert!(b.presets_list().is_empty());
    assert_eq!(b.settings().default_preset, None);
    assert_eq!(b.preset_delete("Archive").expect_err("gone").code, ErrorCode::NotFound);
    assert_eq!(b.preset_get("../../x").expect_err("none").code, ErrorCode::NotFound);
}

#[test]
fn a_run_is_a_job_that_leaves_the_open_document_alone() {
    let base = Scratch::new("pj-base2");
    let work = Scratch::new("pj-work2");
    let b = backend(&base);
    let open = work.file("open.pdf", &text_pdf(2));
    b.import(vec![open]).expect("a document is open");
    let before = b.snapshot().expect("snapshot").total;

    b.preset_save(Preset {
        name: "Shrink".into(),
        compress: Some("Recommended".into()),
        ..Preset::default()
    })
    .expect("save");
    let a = work.file("a.pdf", &text_pdf(1));
    let c = work.file("c.pdf", &text_pdf(3));
    let job = b.preset_run("Shrink", vec![a, c]).expect("start");
    let view = wait(&b, job);
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!(view["detail"]["count"], 2);
    assert_eq!(view["detail"]["failed"], 0);
    assert!(work.path().join("a.processed.pdf").is_file() && work.path().join("c.processed.pdf").is_file());
    assert_eq!(
        b.snapshot().expect("snapshot").total,
        before,
        "the open document is untouched"
    );
    assert!(b.reveal_path(job, 0).is_ok(), "the outputs can be shown in the folder");

    // Refused up front.
    assert_eq!(
        b.preset_run("Shrink", vec![]).expect_err("no files").code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        b.preset_run("Nope", vec![work.path().join("a.pdf")])
            .expect_err("no preset")
            .code,
        ErrorCode::NotFound
    );

    // A run where nothing works is a failed job with the reason.
    let job = b
        .preset_run("Shrink", vec![work.path().join("missing.pdf")])
        .expect("start");
    let view = wait(&b, job);
    assert_eq!(view["state"], "failed", "{view}");
}
