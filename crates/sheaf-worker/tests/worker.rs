//! The real worker binary, driven over its pipes exactly as MOD-JOBS does.
//! Needs `bash tools/fetch-vendor` to have run.

use fixture_gen::{PageDef, Scratch};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    /// Kept so the session outlives the worker.
    session: PathBuf,
}

impl Worker {
    fn start(base: &Scratch) -> Worker {
        // A session dir as MOD-STORE makes it: the marker is its lock file.
        let session = base.path().join("session-test");
        for sub in ["thumbs", "results"] {
            std::fs::create_dir_all(session.join(sub)).expect("session dirs");
        }
        std::fs::write(session.join(".lock"), b"").expect("lock file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_sheaf-worker"))
            .arg("--session")
            .arg(&session)
            .arg("--pdfium")
            .arg(fixture_gen::vendored_pdfium_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("worker starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Worker {
            child,
            stdin,
            stdout,
            next: 0,
            session,
        }
    }

    fn send_raw(&mut self, line: &str) -> Value {
        writeln!(self.stdin, "{line}").expect("write request");
        self.stdin.flush().expect("flush");
        loop {
            let mut reply = String::new();
            assert!(
                self.stdout.read_line(&mut reply).expect("read reply") > 0,
                "worker closed its pipe"
            );
            let v: Value = serde_json::from_str(&reply).expect("worker speaks JSON");
            if v.get("ok").is_some() {
                return v;
            }
            assert!(v.get("progress").is_some(), "unexpected line: {reply}");
        }
    }

    fn call(&mut self, op: &str, args: Value) -> Value {
        self.next += 1;
        let id = self.next;
        let reply = self.send_raw(&json!({"id": id, "op": op, "args": args}).to_string());
        assert_eq!(reply["id"], json!(id));
        reply
    }

    fn ok(&mut self, op: &str, args: Value) -> Value {
        let reply = self.call(op, args);
        assert_eq!(reply["ok"], json!(true), "{op} failed: {reply}");
        reply["result"].clone()
    }

    fn err(&mut self, op: &str, args: Value) -> String {
        let reply = self.call(op, args);
        assert_eq!(reply["ok"], json!(false), "{op} should fail: {reply}");
        reply["error"]["code"]
            .as_str()
            .expect("error code")
            .to_string()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pdf_page(path: &Path, index: u32) -> Value {
    json!({"source": {"path": path, "kind": "Pdf"}, "index": index})
}

fn png(dir: &Scratch, name: &str, w: u32, h: u32, rgb: [u8; 3]) -> PathBuf {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let path = dir.path().join(name);
    img.save(&path).expect("write png");
    path
}

#[test]
fn probe_classifies_inputs() {
    let dir = Scratch::new("worker-probe");
    let mut w = Worker::start(&dir);
    assert!(w.ok("ping", json!({}))["pid"].as_u64().is_some());

    let pdf = dir.file(
        "a.pdf",
        &fixture_gen::pdf(&[PageDef::text("Hi"), PageDef::no_text()]),
    );
    let r = w.ok("import_probe", json!({"path": pdf}));
    assert_eq!(
        (r["kind"].as_str(), r["page_count"].as_u64()),
        (Some("Pdf"), Some(2))
    );
    assert_eq!(r["has_text"], json!([true, false]));

    let image = png(&dir, "scan.png", 1240, 1754, [250, 250, 250]);
    let r = w.ok("import_probe", json!({"path": image}));
    assert_eq!(
        (r["kind"].as_str(), r["page_count"].as_u64()),
        (Some("Image"), Some(1))
    );
    assert_eq!(r["has_text"], json!([false]));

    let locked = dir.file("locked.pdf", &fixture_gen::locked_pdf());
    let r = w.ok("import_probe", json!({"path": locked}));
    assert_eq!(
        (r["encrypted"].as_bool(), r["page_count"].as_u64()),
        (Some(true), Some(0))
    );

    let corrupt = dir.file("corrupt.pdf", &fixture_gen::corrupt_pdf());
    assert_eq!(
        w.err("import_probe", json!({"path": corrupt})),
        "ParseError"
    );
    let text = dir.file("notes.txt", b"just text");
    assert_eq!(w.err("import_probe", json!({"path": text})), "Unsupported");
    assert_eq!(
        w.err(
            "import_probe",
            json!({"path": dir.path().join("missing.pdf")})
        ),
        "NotFound"
    );
    assert_eq!(
        w.err("import_probe", json!({"path": "relative.pdf"})),
        "PathRejected"
    );
    assert_eq!(
        w.err("import_probe", json!({"path": dir.path()})),
        "PathRejected"
    );

    // Bad requests are answered, not fatal.
    assert_eq!(w.err("import_probe", json!({"wrong": 1})), "InvalidArg");
    assert_eq!(w.err("format_disk", json!({})), "InvalidArg");
    assert_eq!(
        w.send_raw("this is not json")["error"]["code"],
        json!("InvalidArg")
    );
    assert!(
        w.ok("ping", json!({}))["pid"].as_u64().is_some(),
        "still alive after all of that"
    );
}

#[test]
fn thumbnails_land_in_the_session_and_nowhere_else() {
    let dir = Scratch::new("worker-thumb");
    let mut w = Worker::start(&dir);
    let pdf = dir.file("a.pdf", &fixture_gen::pdf(&[PageDef::no_text()]));

    let r = w.ok(
        "thumb",
        json!({"page": pdf_page(&pdf, 0), "max_px": [256, 256], "out": "thumbs/p0.png"}),
    );
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(181), Some(256))
    );
    let thumb = image::open(w.session.join("thumbs/p0.png"))
        .expect("thumbnail is a PNG")
        .into_rgb8();
    assert_eq!(thumb.dimensions(), (181, 256));
    assert!(thumb.get_pixel(90, 128)[0] < 100 && thumb.get_pixel(5, 5)[0] > 240);

    // Rotation and crop are applied to what the user sees.
    let mut page = pdf_page(&pdf, 0);
    page["rot"] = json!("R90");
    let r = w.ok(
        "render",
        json!({"page": page, "max_px": [256, 256], "out": "thumbs/turned.png"}),
    );
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(256), Some(181))
    );
    let mut page = pdf_page(&pdf, 0);
    page["crop"] = json!({"x0": 0.0, "y0": 0.0, "x1": 0.2, "y1": 0.2});
    w.ok(
        "render",
        json!({"page": page, "max_px": [256, 256], "out": "thumbs/corner.png"}),
    );
    let corner = image::open(w.session.join("thumbs/corner.png"))
        .expect("png")
        .into_rgb8();
    assert!(
        corner.pixels().all(|p| p[0] > 240),
        "the cropped corner holds only paper"
    );

    let blank = json!({"source": null, "blank_pt": [595.0, 842.0]});
    let r = w.ok(
        "thumb",
        json!({"page": blank, "max_px": [100, 100], "out": "thumbs/blank.png"}),
    );
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(70), Some(100))
    );

    let image = png(&dir, "photo.png", 400, 200, [10, 200, 10]);
    let page = json!({"source": {"path": image, "kind": "Image"}, "filter": {"mode": "Gray", "threshold": "Auto", "contrast": 0, "deskew": "Off", "despeckle": false}});
    let r = w.ok(
        "thumb",
        json!({"page": page, "max_px": [100, 100], "out": "thumbs/gray.png"}),
    );
    assert_eq!(
        (r["width"].as_u64(), r["height"].as_u64()),
        (Some(100), Some(50))
    );
    let gray = image::open(w.session.join("thumbs/gray.png"))
        .expect("png")
        .into_rgb8();
    assert!(gray.pixels().all(|p| p[0] == p[1] && p[1] == p[2]));

    // SEC-PATHS: the worker writes inside the session dir only.
    let outside = dir.path().join("escaped.png");
    for out in [
        "../escaped.png",
        outside.to_str().expect("utf-8 path"),
        "thumbs/../../escaped.png",
        "",
    ] {
        let code = w.err(
            "thumb",
            json!({"page": pdf_page(&pdf, 0), "max_px": [64, 64], "out": out}),
        );
        assert_eq!(code, "PathRejected", "{out:?}");
    }
    assert!(!outside.exists());
    assert_eq!(
        w.err(
            "thumb",
            json!({"page": pdf_page(&pdf, 7), "max_px": [64, 64], "out": "thumbs/x.png"})
        ),
        "InvalidArg"
    );
}

#[test]
fn build_assembles_a_mixed_document() {
    let dir = Scratch::new("worker-build");
    let mut w = Worker::start(&dir);
    let a = dir.file("a.pdf", &fixture_gen::text_pdf(3));
    let scan = png(&dir, "scan.png", 1240, 1754, [245, 245, 245]);

    let mut turned = pdf_page(&a, 0);
    turned["rot"] = json!("R180");
    let mut cropped = pdf_page(&a, 1);
    cropped["crop"] = json!({"x0": 0.0, "y0": 0.0, "x1": 0.5, "y1": 0.5});
    let pages = json!([
        pdf_page(&a, 2),
        turned,
        {"source": {"path": scan, "kind": "Image"}},
        {"source": null, "blank_pt": [300.0, 300.0]},
        cropped,
    ]);
    let r = w.ok("build", json!({"pages": pages, "out": "results/out.pdf"}));
    assert_eq!(
        r,
        json!({"out": "results/out.pdf", "page_count": 5, "copied": 2, "rasterized": 2})
    );
    let out = w.session.join("results/out.pdf");
    assert!(
        !w.session.join("results/out.part").exists(),
        "no temp file left behind"
    );

    let probe = w.ok("import_probe", json!({"path": out}));
    assert_eq!(probe["page_count"], json!(5));
    assert_eq!(
        probe["has_text"],
        json!([true, true, false, false, false]),
        "copied pages keep text; rasterized ones do not"
    );
    let sizes: Vec<(i64, i64)> = probe["sizes_pt"]
        .as_array()
        .expect("sizes")
        .iter()
        .map(|s| {
            (
                s[0].as_f64().expect("w").round() as i64,
                s[1].as_f64().expect("h").round() as i64,
            )
        })
        .collect();
    assert_eq!(
        sizes,
        [(595, 842), (595, 842), (298, 421), (300, 300), (298, 421)]
    );

    // Failures leave nothing behind.
    assert_eq!(
        w.err("build", json!({"pages": [], "out": "results/empty.pdf"})),
        "InvalidArg"
    );
    assert_eq!(
        w.err(
            "build",
            json!({"pages": [pdf_page(&a, 9)], "out": "results/bad.pdf"})
        ),
        "InvalidArg"
    );
    let corrupt = dir.file("corrupt.pdf", &fixture_gen::corrupt_pdf());
    assert_eq!(
        w.err(
            "build",
            json!({"pages": [pdf_page(&a, 0), pdf_page(&corrupt, 0)], "out": "results/bad.pdf"})
        ),
        "ParseError"
    );
    assert_eq!(
        w.err(
            "build",
            json!({"pages": [pdf_page(&a, 0)], "out": "../out.pdf"})
        ),
        "PathRejected"
    );
    let left: Vec<String> = std::fs::read_dir(w.session.join("results"))
        .expect("list results")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["out.pdf"]);
}

#[test]
fn the_worker_refuses_to_start_outside_a_session() {
    let dir = Scratch::new("worker-nosession");
    let status = Command::new(env!("CARGO_BIN_EXE_sheaf-worker"))
        .arg("--session")
        .arg(dir.path())
        .arg("--pdfium")
        .arg(fixture_gen::vendored_pdfium_dir())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(2));
    let status = Command::new(env!("CARGO_BIN_EXE_sheaf-worker"))
        .stderr(Stdio::null())
        .status()
        .expect("runs");
    assert_eq!(status.code(), Some(2));
}
