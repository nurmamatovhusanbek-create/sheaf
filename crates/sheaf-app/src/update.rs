//! UPDATE: look up the newest release on GitHub, download it, check it, and put it in place of the
//! running program.
//!
//! This is the one place the program talks to a network, and only when the person presses
//! "Check for updates" or "Update" (SEC-NONET): nothing runs in the background, nothing is sent but
//! the request for a public page, and no network crate is compiled in. The system's own `curl`
//! (it ships with Windows 10 and later) does the talking, restricted to HTTPS.
//!
//! What makes it safe to run what was downloaded: the release must carry a SHA-256 for the file (GitHub
//! records one for every asset, and the release workflow adds a `.sha256` file next to it), the
//! download must match it exactly, and the file must come from this project's own release page.
//!
//! Putting it in place: Windows lets a running program be renamed but not overwritten, so the new file
//! is written beside the program, the running file becomes `<name>.old`, the new one takes its name, and the
//! new program is started; it removes the `.old` file when it starts. Everything before the first rename
//! can fail without changing anything, and a failed second rename puts the old program back.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Where releases are published.
pub const REPO: &str = "nurmamatovhusanbek-create/sheaf";
/// The release file that is the whole program.
pub const ASSET: &str = "sheaf.exe";
/// Largest file the updater will accept: far more than the program and far less than a disk.
pub const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;
const MAX_PAGE: u64 = 2 * 1024 * 1024;

/// What can go wrong, in words a person can act on (the window picks the text from the variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateError {
    /// No answer: offline, a firewall, or `curl` missing.
    Network(String),
    /// GitHub answered, but not with something a release can be read from.
    BadAnswer(String),
    /// Nothing has been released yet.
    NoRelease,
    /// The release has no file for this program, or no checksum for it.
    NoFile(String),
    /// The download is not the file the release describes.
    Mismatch,
    /// The program's folder cannot be written (a system-wide install, say): use the installer or the release page.
    NotWritable(String),
    Other(String),
}

impl UpdateError {
    pub fn key(&self) -> &'static str {
        match self {
            UpdateError::Network(_) => "update.err_network",
            UpdateError::BadAnswer(_) => "update.err_answer",
            UpdateError::NoRelease => "update.err_norelease",
            UpdateError::NoFile(_) => "update.err_nofile",
            UpdateError::Mismatch => "update.err_mismatch",
            UpdateError::NotWritable(_) => "update.err_readonly",
            UpdateError::Other(_) => "update.err_other",
        }
    }
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::Mismatch => write!(f, "the download does not match its checksum"),
            UpdateError::NoRelease => write!(f, "no release has been published yet"),
            UpdateError::Network(m)
            | UpdateError::BadAnswer(m)
            | UpdateError::NoFile(m)
            | UpdateError::NotWritable(m)
            | UpdateError::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for UpdateError {}

type Result<T> = std::result::Result<T, UpdateError>;

/// A release as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    /// The release notes, as written on the release page.
    pub notes: String,
    /// The release's page, for a person who would rather download it themselves.
    pub page: String,
    pub asset_url: String,
    pub asset_size: u64,
    /// Lower-case hex SHA-256 of the program file.
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    /// "v1.2.3", "1.2" and "1.2.3-beta" → 1.2.3 style numbers; anything that is not that → `None`.
    pub fn parse(text: &str) -> Option<Version> {
        let core = text.trim().trim_start_matches(['v', 'V']);
        let core = core.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let mut next = || -> Option<u64> {
            match parts.next() {
                None => Some(0),
                Some(p) if !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
                Some(_) => None,
            }
        };
        let v = Version(next()?, next()?, next()?);
        parts.next().is_none().then_some(v)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The network, as far as the updater needs it. The real one is [`Curl`]; tests bring their own.
pub trait Net: Send + Sync {
    /// The body of an HTTPS page, at most `max` bytes.
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>>;
    /// A file, saved at `to`, at most `max` bytes.
    fn download(&self, url: &str, to: &Path, max: u64) -> Result<()>;
}

/// The system's `curl`, HTTPS only, redirects only to HTTPS, with time limits.
pub struct Curl;

impl Curl {
    fn command(&self) -> std::process::Command {
        // On Windows the system one, by absolute path: never whatever is first on the search path.
        #[cfg(windows)]
        let program = {
            let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            PathBuf::from(root).join("System32").join("curl.exe")
        };
        #[cfg(not(windows))]
        let program = PathBuf::from("curl");
        let mut cmd = std::process::Command::new(program);
        sheaf_store::hide_console(&mut cmd);
        cmd.args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--max-redirs",
            "5",
            "--connect-timeout",
            "15",
            "--max-time",
            "900",
            "--user-agent",
            concat!("Sheaf/", env!("CARGO_PKG_VERSION")),
            "--header",
            "Accept: application/vnd.github+json",
        ]);
        cmd
    }
}

impl Net for Curl {
    fn get(&self, url: &str, max: u64) -> Result<Vec<u8>> {
        let out = self
            .command()
            .args(["--max-filesize", &max.to_string(), "--", url])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| UpdateError::Network(format!("cannot run curl: {e}")))?;
        if !out.status.success() {
            return Err(UpdateError::Network(first_line(&String::from_utf8_lossy(&out.stderr))));
        }
        if out.stdout.len() as u64 > max {
            return Err(UpdateError::BadAnswer("the answer is too large".into()));
        }
        Ok(out.stdout)
    }

    fn download(&self, url: &str, to: &Path, max: u64) -> Result<()> {
        let out = self
            .command()
            .args(["--max-filesize", &max.to_string(), "--output"])
            .arg(to)
            .args(["--", url])
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| UpdateError::Network(format!("cannot run curl: {e}")))?;
        if !out.status.success() {
            let _ = std::fs::remove_file(to);
            return Err(UpdateError::Network(first_line(&String::from_utf8_lossy(&out.stderr))));
        }
        Ok(())
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("no answer")
        .trim()
        .chars()
        .take(200)
        .collect()
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    UpToDate(Version),
    Available(Box<Release>),
}

/// Ask GitHub for the newest release and say whether it is newer than `current`.
pub fn check(net: &dyn Net, current: &str) -> Result<Check> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = net.get(&url, MAX_PAGE).map_err(|e| match e {
        // The API answers "not found" for a project with no release.
        UpdateError::Network(m) if m.contains("404") => UpdateError::NoRelease,
        other => other,
    })?;
    let release = parse_release(&body, |u| net.get(u, 4096))?;
    let now = Version::parse(current).unwrap_or(Version(0, 0, 0));
    Ok(if release.version > now {
        Check::Available(Box::new(release))
    } else {
        Check::UpToDate(now)
    })
}

/// Read a release from the API's JSON. `fetch_text` gets a small file by URL (the `.sha256` beside the program).
pub fn parse_release(json: &[u8], fetch_text: impl Fn(&str) -> Result<Vec<u8>>) -> Result<Release> {
    let v: Value = serde_json::from_slice(json).map_err(|_| UpdateError::BadAnswer("not a release".into()))?;
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let tag = text("tag_name");
    let version =
        Version::parse(&tag).ok_or_else(|| UpdateError::BadAnswer(format!("version '{tag}' not understood")))?;
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    let empty = Vec::new();
    let assets = v.get("assets").and_then(Value::as_array).unwrap_or(&empty);
    let find = |name: &str| {
        assets.iter().find(|a| {
            a.get("name").and_then(Value::as_str) == Some(name)
                && a.get("browser_download_url")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u.starts_with(&prefix))
        })
    };
    let asset = find(ASSET).ok_or_else(|| UpdateError::NoFile(format!("release {tag} has no {ASSET}")))?;
    let url_of = |a: &Value| {
        a.get("browser_download_url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    // The checksum: the one GitHub itself records for the asset, else the file the workflow put beside it.
    let digest = asset
        .get("digest")
        .and_then(Value::as_str)
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_string);
    let sha256 = match digest {
        Some(d) => d,
        None => {
            let side = find(&format!("{ASSET}.sha256"))
                .ok_or_else(|| UpdateError::NoFile(format!("release {tag} has no checksum for {ASSET}")))?;
            let bytes = fetch_text(&url_of(side))?;
            String::from_utf8_lossy(&bytes)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        }
    }
    .to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpdateError::NoFile(format!(
            "the checksum of release {tag} is not a SHA-256"
        )));
    }
    Ok(Release {
        page: text("html_url"),
        notes: text("body").chars().take(4000).collect(),
        asset_url: url_of(asset),
        asset_size: asset.get("size").and_then(Value::as_u64).unwrap_or(0),
        sha256,
        version,
        tag,
    })
}

/// Download the release's program beside `exe` and check it. The file is left in place only if it is the one described.
pub fn download(net: &dyn Net, release: &Release, exe: &Path) -> Result<PathBuf> {
    let new = side_file(exe, "new");
    if release.asset_size > MAX_DOWNLOAD {
        return Err(UpdateError::BadAnswer("the file is larger than expected".into()));
    }
    let prefix = format!("https://github.com/{REPO}/releases/download/");
    if !release.asset_url.starts_with(&prefix) {
        return Err(UpdateError::BadAnswer("the file is not from the release page".into()));
    }
    // Writing next to the program is also the test that it can be replaced at all.
    if let Err(e) = std::fs::write(&new, b"") {
        return Err(UpdateError::NotWritable(format!("{}: {e}", new.display())));
    }
    let outcome = net
        .download(&release.asset_url, &new, MAX_DOWNLOAD)
        .and_then(|()| check_file(&new, release));
    if outcome.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    outcome.map(|()| new)
}

fn check_file(file: &Path, release: &Release) -> Result<()> {
    let meta = std::fs::metadata(file).map_err(|e| UpdateError::Other(format!("downloaded file: {e}")))?;
    if release.asset_size != 0 && meta.len() != release.asset_size {
        return Err(UpdateError::Mismatch);
    }
    let hash = sheaf_store::hash_file(file).map_err(|e| UpdateError::Other(e.to_string()))?;
    if sheaf_store::hex(&hash) != release.sha256 {
        return Err(UpdateError::Mismatch);
    }
    Ok(())
}

/// `<exe>.<suffix>` in the program's own folder.
pub fn side_file(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    exe.with_file_name(name)
}

/// Make `new` the program at `exe`: the old one is kept as `<exe>.old` until the new one has started.
pub fn swap_in(exe: &Path, new: &Path) -> Result<()> {
    let old = side_file(exe, "old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| UpdateError::NotWritable(format!("{}: {e}", exe.display())))?;
    if let Err(e) = std::fs::rename(new, exe) {
        // Put the program back; failing that, there is nothing more to do than say so.
        let back = std::fs::rename(&old, exe);
        return Err(UpdateError::Other(format!(
            "cannot put the new program in place: {e}{}",
            if back.is_err() {
                " (and the old one could not be restored)"
            } else {
                ""
            }
        )));
    }
    Ok(())
}

/// At startup: remove what an update left behind (`<exe>.old`, `<exe>.new`). Best effort.
pub fn clean_up_after_update() {
    if let Ok(exe) = std::env::current_exe() {
        for suffix in ["old", "new"] {
            let _ = std::fs::remove_file(side_file(&exe, suffix));
        }
    }
}

/// Download, check, replace and start the new program; returns once it has been started and the caller
/// should exit. `current_exe` is the running program.
pub fn apply(net: &dyn Net, release: &Release, exe: &Path) -> Result<()> {
    let new = download(net, release, exe)?;
    if let Err(e) = swap_in(exe, &new) {
        let _ = std::fs::remove_file(&new);
        return Err(e);
    }
    Ok(())
}

/// Start `exe` again (after an update) as a separate program.
pub fn restart(exe: &Path) -> Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null());
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| UpdateError::Other(format!("cannot start the new program: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn hex_of(bytes: &[u8]) -> String {
        sheaf_store::hex(&sheaf_store::hash_bytes(&[bytes]))
    }

    fn release_json(tag: &str, digest: Option<&str>, with_side: bool) -> String {
        let base = format!("https://github.com/{REPO}/releases/download/{tag}");
        let digest = digest.map_or(String::new(), |d| format!(r#","digest":"sha256:{d}""#));
        let side = if with_side {
            format!(r#",{{"name":"sheaf.exe.sha256","browser_download_url":"{base}/sheaf.exe.sha256","size":80}}"#)
        } else {
            String::new()
        };
        format!(
            r#"{{"tag_name":"{tag}","html_url":"https://github.com/{REPO}/releases/tag/{tag}","body":"notes",
            "assets":[{{"name":"sheaf.exe","browser_download_url":"{base}/sheaf.exe","size":5{digest}}}{side}]}}"#
        )
    }

    struct Fake {
        pages: Vec<(String, Vec<u8>)>,
        file: Vec<u8>,
        asked: Mutex<Vec<String>>,
    }

    impl Net for Fake {
        fn get(&self, url: &str, _max: u64) -> Result<Vec<u8>> {
            self.asked.lock().expect("ok").push(url.to_string());
            self.pages
                .iter()
                .find(|(u, _)| url.ends_with(u.as_str()))
                .map(|(_, b)| b.clone())
                .ok_or_else(|| UpdateError::Network("no route".into()))
        }
        fn download(&self, url: &str, to: &Path, _max: u64) -> Result<()> {
            self.asked.lock().expect("ok").push(url.to_string());
            std::fs::write(to, &self.file).map_err(|e| UpdateError::Other(e.to_string()))
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sheaf-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("ok");
        dir
    }

    #[test]
    fn versions_are_read_from_tags_and_compared_as_numbers() {
        assert_eq!(Version::parse("v1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(Version::parse("0.1"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("2.0.0-beta.1"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("latest"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version::parse("1..3"), None);
        assert!(Version::parse("0.10.0") > Version::parse("0.9.9"), "numbers, not text");
        assert!(Version::parse("1.0.0") > Version::parse("0.99.99"));
    }

    #[test]
    fn a_newer_release_is_offered_and_the_same_one_is_not() {
        let digest = hex_of(b"hello");
        let net = Fake {
            pages: vec![(
                "/releases/latest".into(),
                release_json("v0.2.0", Some(&digest), false).into_bytes(),
            )],
            file: b"hello".to_vec(),
            asked: Mutex::default(),
        };
        match check(&net, "0.1.0").expect("check") {
            Check::Available(r) => {
                assert_eq!(r.version, Version(0, 2, 0));
                assert_eq!(r.sha256, digest);
                assert_eq!(r.notes, "notes");
            }
            other => panic!("expected an update, got {other:?}"),
        }
        assert_eq!(check(&net, "0.2.0").expect("ok"), Check::UpToDate(Version(0, 2, 0)));
        assert_eq!(check(&net, "0.3.0").expect("ok"), Check::UpToDate(Version(0, 3, 0)));
    }

    #[test]
    fn the_checksum_may_come_from_the_file_beside_the_program() {
        let digest = hex_of(b"hello");
        let net = Fake {
            pages: vec![
                (
                    "/releases/latest".into(),
                    release_json("v1.0.0", None, true).into_bytes(),
                ),
                ("sheaf.exe.sha256".into(), format!("{digest}  sheaf.exe\n").into_bytes()),
            ],
            file: b"hello".to_vec(),
            asked: Mutex::default(),
        };
        let Check::Available(r) = check(&net, "0.1.0").expect("ok") else {
            panic!("expected an update")
        };
        assert_eq!(r.sha256, digest);
    }

    #[test]
    fn a_release_without_a_checksum_or_without_the_program_is_refused() {
        let none = release_json("v1.0.0", None, false);
        assert!(matches!(
            parse_release(none.as_bytes(), |_| Ok(Vec::new())),
            Err(UpdateError::NoFile(_))
        ));
        let junk_side = release_json("v1.0.0", None, true);
        assert!(matches!(
            parse_release(junk_side.as_bytes(), |_| Ok(b"not a hash".to_vec())),
            Err(UpdateError::NoFile(_))
        ));
        let nothing = br#"{"tag_name":"v1.0.0","assets":[]}"#;
        assert!(matches!(
            parse_release(nothing, |_| Ok(Vec::new())),
            Err(UpdateError::NoFile(_))
        ));
        assert!(matches!(
            parse_release(b"<html>rate limited</html>", |_| Ok(Vec::new())),
            Err(UpdateError::BadAnswer(_))
        ));
    }

    #[test]
    fn a_file_from_somewhere_else_is_not_trusted() {
        let digest = hex_of(b"hello");
        let evil = release_json("v1.0.0", Some(&digest), false).replace(
            &format!("https://github.com/{REPO}/releases/download"),
            "https://example.com/download",
        );
        assert!(matches!(
            parse_release(evil.as_bytes(), |_| Ok(Vec::new())),
            Err(UpdateError::NoFile(_))
        ));
    }

    fn a_release(digest: &str) -> Release {
        let Ok(r) = parse_release(release_json("v9.9.9", Some(digest), false).as_bytes(), |_| {
            Ok(Vec::new())
        }) else {
            panic!("release")
        };
        r
    }

    #[test]
    fn the_download_replaces_the_program_and_keeps_the_old_one_until_the_new_one_runs() {
        let dir = temp("swap");
        let exe = dir.join("sheaf.exe");
        std::fs::write(&exe, b"old program").expect("ok");
        let net = Fake {
            pages: vec![],
            file: b"hello".to_vec(),
            asked: Mutex::default(),
        };
        let rel = a_release(&hex_of(b"hello"));
        apply(&net, &rel, &exe).expect("update");
        assert_eq!(std::fs::read(&exe).expect("ok"), b"hello");
        assert_eq!(std::fs::read(side_file(&exe, "old")).expect("ok"), b"old program");
        assert!(!side_file(&exe, "new").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_download_that_does_not_match_changes_nothing_and_leaves_nothing() {
        let dir = temp("mismatch");
        let exe = dir.join("sheaf.exe");
        std::fs::write(&exe, b"old program").expect("ok");
        let net = Fake {
            pages: vec![],
            file: b"tampered".to_vec(),
            asked: Mutex::default(),
        };
        let rel = a_release(&hex_of(b"hello"));
        assert_eq!(apply(&net, &rel, &exe), Err(UpdateError::Mismatch));
        assert_eq!(std::fs::read(&exe).expect("ok"), b"old program");
        let left: Vec<_> = std::fs::read_dir(&dir).expect("ok").flatten().collect();
        assert_eq!(left.len(), 1, "no .new or .old file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_swap_puts_the_old_program_back() {
        let dir = temp("rollback");
        let exe = dir.join("sheaf.exe");
        std::fs::write(&exe, b"old program").expect("ok");
        // The new file is missing, so the second rename fails after the first has happened.
        let err = swap_in(&exe, &dir.join("does-not-exist")).expect_err("an error");
        assert!(matches!(err, UpdateError::Other(_)), "{err}");
        assert_eq!(std::fs::read(&exe).expect("ok"), b"old program");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_be_written_is_reported_before_anything_is_downloaded() {
        let net = Fake {
            pages: vec![],
            file: b"hello".to_vec(),
            asked: Mutex::default(),
        };
        let rel = a_release(&hex_of(b"hello"));
        let exe = Path::new("/nonexistent-folder-for-sheaf-tests/sheaf.exe");
        assert!(matches!(apply(&net, &rel, exe), Err(UpdateError::NotWritable(_))));
        assert!(net.asked.lock().expect("ok").is_empty(), "nothing was fetched");
    }

    #[test]
    fn every_error_has_a_text_in_the_window() {
        for e in [
            UpdateError::Network(String::new()),
            UpdateError::BadAnswer(String::new()),
            UpdateError::NoRelease,
            UpdateError::NoFile(String::new()),
            UpdateError::Mismatch,
            UpdateError::NotWritable(String::new()),
            UpdateError::Other(String::new()),
        ] {
            assert!(e.key().starts_with("update.err_"));
        }
    }
}
