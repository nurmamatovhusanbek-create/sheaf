//! Store half of TEST-SEC: path traversal, device names, ADS, oversized
//! inputs, no overwrite, session dir removed on exit.

use sheaf_core::ErrorCode;
use sheaf_store::paths::{check_windows_path_rules, sanitize_file_name};
use sheaf_store::*;
use std::fs;
use std::path::{Path, PathBuf};

/// Fresh scratch directory under the system temp dir, removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sheaf-store-test-{}-{tag}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(sheaf_store::paths::simplify_verbatim(
            dir.canonicalize().expect("canonical scratch dir"),
        ))
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn session_is_created_private_and_removed_on_close_and_drop() {
    let base = Scratch::new("session");
    let session = Session::create(base.path()).expect("create session");
    let dir = session.dir().to_path_buf();
    assert!(dir.join("thumbs").is_dir() && dir.join("cache").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&dir).expect("stat").permissions().mode();
        assert_eq!(mode & 0o077, 0, "session dir must not be readable by others");
    }
    fs::write(dir.join("cache").join("x"), b"data").expect("write");
    session.close().expect("close");
    assert!(!dir.exists());

    let dropped = Session::create(base.path()).expect("create session");
    let dir = dropped.dir().to_path_buf();
    drop(dropped);
    assert!(!dir.exists(), "dropping the session removes it too");
}

#[test]
fn session_resolve_rejects_escapes() {
    let base = Scratch::new("resolve");
    let session = Session::create(base.path()).expect("create session");
    assert_eq!(
        session.resolve("thumbs/a.png").expect("plain path"),
        session.dir().join("thumbs/a.png")
    );
    for bad in [
        "",
        "../x",
        "thumbs/../../x",
        "/etc/passwd",
        "C:\\x",
        "thumbs\\..\\x",
        "a/./b",
        "x\0y",
        "\\\\srv\\share\\x",
    ] {
        let err = session.resolve(bad).expect_err(bad);
        assert_eq!(err.code, ErrorCode::PathRejected, "{bad:?}");
    }
}

#[test]
fn attach_only_accepts_a_session_dir_and_does_not_delete_it() {
    let base = Scratch::new("attach");
    let owner = Session::create(base.path()).expect("create session");
    let worker_view = Session::attach(owner.dir()).expect("attach");
    drop(worker_view);
    assert!(owner.dir().exists(), "a worker must not delete its parent's session");
    assert!(Session::attach(base.path()).is_err());
}

#[test]
fn sweep_removes_dead_sessions_but_not_live_ones() {
    let base = Scratch::new("sweep");
    let live = Session::create(base.path()).expect("create session");

    // A session whose owner is gone: the lock file exists but nobody holds it.
    let dead = base.path().join("session-1-deadbeef");
    fs::create_dir_all(dead.join("cache")).expect("mkdir");
    fs::write(dead.join(".lock"), b"").expect("lock file");
    fs::write(dead.join("cache").join("leftover.png"), b"x").expect("leftover");
    // Crashed before the lock file was written.
    let half = base.path().join("session-2-half");
    fs::create_dir_all(&half).expect("mkdir");
    // Not ours: must be left alone.
    let other = base.path().join("settings-backup");
    fs::create_dir_all(&other).expect("mkdir");

    assert_eq!(sweep_stale(base.path()), 2);
    assert!(!dead.exists() && !half.exists());
    assert!(other.exists());
    assert!(live.dir().exists(), "a live session survives the sweep");
}

#[test]
fn windows_path_rules() {
    for ok in [
        r"C:\Users\me\Documents\scan.pdf",
        r"\\server\share\dir\scan.pdf",
        r"\\?\C:\very\long\path\scan.pdf",
        r"\\?\UNC\server\share\scan.pdf",
        "D:/mixed/slashes.pdf",
        r"C:\dir\console.pdf",
        r"C:\dir\com10.pdf",
        r"relative\name.pdf",
    ] {
        check_windows_path_rules(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in [
        "",
        "a\0b",
        r"\\.\PhysicalDrive0",
        r"\\.\pipe\name",
        r"\\?\GLOBALROOT\Device\Harddisk0",
        r"\??\C:\x",
        r"C:\dir\file.pdf:stream",
        r"C:\dir\file.pdf::$DATA",
        r"C:\dir:ads\file.pdf",
        r"C:\dir\NUL",
        r"C:\dir\nul.pdf",
        r"C:\con\file.pdf",
        r"C:\dir\COM1.txt",
        r"C:\dir\lpt9",
        r"C:\dir\aux .pdf",
        "//./COM1",
    ] {
        let err = check_windows_path_rules(bad).expect_err(bad);
        assert_eq!(err.code, ErrorCode::PathRejected, "{bad:?}");
    }
}

#[test]
fn allow_input_accepts_files_and_rejects_everything_else() {
    let dir = Scratch::new("input");
    let file = dir.path().join("doc.pdf");
    fs::write(&file, b"%PDF-1.4").expect("write");

    // The path that comes back is the canonical one (on Windows the verbatim form).
    let canon = file.canonicalize().expect("canonical");
    let ok = allow_input(&file).expect("regular file");
    assert_eq!(ok.path, canon);
    assert_eq!(ok.size, 8);
    assert!(!ok.is_unc);

    // Traversal is resolved, not trusted: the canonical path is what comes back.
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).expect("mkdir");
    let via_dotdot = sub.join("..").join("doc.pdf");
    assert_eq!(allow_input(&via_dotdot).expect("resolves").path, canon);

    assert_eq!(
        allow_input(&dir.path().join("missing.pdf")).expect_err("missing").code,
        ErrorCode::NotFound
    );
    assert_eq!(
        allow_input(dir.path()).expect_err("directory").code,
        ErrorCode::PathRejected
    );
    assert_eq!(
        allow_input(Path::new("")).expect_err("empty").code,
        ErrorCode::PathRejected
    );
    #[cfg(unix)]
    assert_eq!(
        allow_input(Path::new("/dev/null")).expect_err("device").code,
        ErrorCode::PathRejected
    );
}

#[test]
fn oversized_input_is_rejected_before_anything_reads_it() {
    let dir = Scratch::new("big");
    let file = dir.path().join("big.pdf");
    let f = fs::File::create(&file).expect("create");
    // Sparse: sets the length without writing 2 GiB.
    if f.set_len(sheaf_core::consts::MAX_FILE_BYTES + 1).is_err() {
        return; // file system without sparse files: nothing to test here
    }
    assert_eq!(allow_input(&file).expect_err("too big").code, ErrorCode::LimitExceeded);
}

#[test]
fn verbatim_paths_are_simplified_only_when_safe() {
    use sheaf_store::paths::simplify_verbatim as simplify;
    let p = |s: &str| PathBuf::from(s);
    assert_eq!(
        simplify(p(r"\\?\C:\Users\me\AppData\Local\Sheaf")),
        p(r"C:\Users\me\AppData\Local\Sheaf")
    );
    for unchanged in [
        r"C:\plain",
        r"\\?\UNC\server\share\x",
        r"\\?\GLOBALROOT\Device\x",
        r"\\?\C:\dir\NUL",
        "/unix/path",
        r"\\?\C:",
    ] {
        assert_eq!(simplify(p(unchanged)), p(unchanged), "{unchanged}");
    }
    let long = format!(r"\\?\C:\{}", "d".repeat(300));
    assert_eq!(
        simplify(p(&long)),
        p(&long),
        "long paths keep the prefix that makes them work"
    );
}

#[test]
fn file_names_are_sanitized() {
    assert_eq!(sanitize_file_name("report.pdf"), "report.pdf");
    assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
    assert_eq!(sanitize_file_name("a<b>c:d\"e|f?g*.pdf"), "a_b_c_d_e_f_g_.pdf");
    assert_eq!(sanitize_file_name("NUL.pdf"), "_NUL.pdf");
    assert_eq!(sanitize_file_name("com1"), "_com1");
    assert_eq!(sanitize_file_name("name. "), "name");
    assert_eq!(sanitize_file_name(""), "_");
    assert_eq!(sanitize_file_name("..."), "_");
    assert_eq!(sanitize_file_name("Shartnoma №12 ўқғҳ.pdf"), "Shartnoma №12 ўқғҳ.pdf");
    let long = sanitize_file_name(&format!("{}.pdf", "x".repeat(500)));
    assert_eq!(long.chars().count(), 120);
    assert!(long.ends_with(".pdf"));
}

#[test]
fn outputs_never_overwrite() {
    let session_base = Scratch::new("pub-session");
    let out = Scratch::new("pub-out");
    let session = Session::create(session_base.path()).expect("create session");
    let staged = session.dir().join("results").join("r.pdf");

    fs::write(out.path().join("merged.pdf"), b"the user's file").expect("existing file");
    assert_eq!(
        allow_output(out.path(), "merged.pdf").expect("free name"),
        out.path().join("merged-2.pdf")
    );

    fs::write(&staged, b"first").expect("stage");
    assert_eq!(
        publish(&staged, out.path(), "merged.pdf").expect("publish"),
        out.path().join("merged-2.pdf")
    );
    fs::write(&staged, b"second").expect("stage");
    assert_eq!(
        publish(&staged, out.path(), "merged.pdf").expect("publish"),
        out.path().join("merged-3.pdf")
    );

    assert_eq!(
        fs::read(out.path().join("merged.pdf")).expect("read"),
        b"the user's file"
    );
    assert_eq!(fs::read(out.path().join("merged-2.pdf")).expect("read"), b"first");
    assert_eq!(fs::read(out.path().join("merged-3.pdf")).expect("read"), b"second");
    let names: Vec<String> = fs::read_dir(out.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 3, "no temp files left behind: {names:?}");

    // A hostile name cannot steer the output elsewhere.
    let escaped = publish(&staged, out.path(), "../escape.pdf").expect("publish");
    assert_eq!(escaped.parent(), Some(out.path()));
    assert!(publish(&staged, &out.path().join("missing-dir"), "x.pdf").is_err());
    assert!(publish(&session.dir().join("nope"), out.path(), "y.pdf").is_err());
    assert!(!out.path().join("y.pdf").exists(), "a failed publish leaves no file");
}

#[test]
fn atomic_write_replaces_whole_files_and_hash_matches() {
    let dir = Scratch::new("atomic");
    let file = dir.path().join("settings.json");
    write_atomic(&file, b"one").expect("write");
    write_atomic(&file, b"two").expect("rewrite");
    assert_eq!(fs::read(&file).expect("read"), b"two");
    assert_eq!(
        fs::read_dir(dir.path()).expect("list").count(),
        1,
        "no temp file left behind"
    );

    fs::write(&file, b"abc").expect("write");
    let hex: String = hash_file(&file)
        .expect("hash")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn settings_round_trip_and_tolerate_garbage() {
    let dir = Scratch::new("settings");
    assert_eq!(Settings::load(dir.path()), Settings::default());

    let s = Settings {
        language: "uz".into(),
        workers: Some(3),
        theme: "dark".into(),
        ..Default::default()
    };
    s.save(dir.path()).expect("save");
    assert_eq!(Settings::load(dir.path()), s);
    assert_eq!(s.worker_count(), 3);

    fs::write(
        dir.path().join("settings.json"),
        br#"{"language":"xx","workers":9999,"theme":"neon","extra":1}"#,
    )
    .expect("write");
    let loaded = Settings::load(dir.path());
    assert_eq!(loaded.language, "en");
    assert_eq!(loaded.theme, "system");
    assert_eq!(loaded.workers, Some(settings::MAX_WORKERS));

    fs::write(dir.path().join("settings.json"), b"not json").expect("write");
    assert_eq!(Settings::load(dir.path()), Settings::default());
    assert!(Settings::default().worker_count() >= 1);
}
