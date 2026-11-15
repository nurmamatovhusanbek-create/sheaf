//! Single-instance aggregation for Explorer verbs (FLOW-EXPLORER,
//! RISK-EXPLORER-MULTI). Explorer starts one process per selected file; this
//! makes the first of them do the work for all of them.
//!
//! Every process drops its paths into a private folder as a list file, then
//! tries to take an exclusive lock file (an atomic "create new", the same on
//! every platform). The one that gets the lock is the *leader*: it waits until
//! no new list has arrived for the grace period (500 ms), collects every list,
//! removes the lock and returns the whole selection. The others are
//! *followers*: they wait until the leader has taken their list, then return.
//! A follower whose list is still there after the leader has gone becomes a
//! leader of its own batch, so a late arrival is never lost. A lock left by a
//! crashed leader goes stale and is removed.
//!
//! Files are ordered by name, because Explorer's processes race and their
//! start order says nothing about the selection (a merge follows name order).

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// How long things wait.
#[derive(Debug, Clone)]
pub struct Timing {
    /// The leader acts when no list has arrived for this long.
    pub grace: Duration,
    /// The longest the leader keeps collecting, however many keep arriving.
    pub collect_max: Duration,
    /// The longest a follower waits for a leader before doing its own files.
    pub follower_max: Duration,
    /// A lock older than this belongs to a leader that is gone.
    pub stale: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            grace: Duration::from_millis(500),
            collect_max: Duration::from_secs(5),
            follower_max: Duration::from_secs(15),
            stale: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Joined {
    /// Do the work on all of these.
    Leader(Vec<PathBuf>),
    /// A leader has taken this process's files: nothing to do.
    Follower,
}

const STEP: Duration = Duration::from_millis(20);

/// A name that no other process picks: process id, clock and a counter.
fn token() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{:x}-{nanos:x}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// The folder name for a kind of work, with nothing in it that is not a plain character.
fn folder_for(key: &str) -> String {
    let clean: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .take(60)
        .collect();
    if clean.is_empty() {
        "_".into()
    } else {
        clean
    }
}

fn lists_in(dir: &Path) -> BTreeSet<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("list"))
                .collect()
        })
        .unwrap_or_default()
}

fn write_list(dir: &Path, files: &[PathBuf]) -> io::Result<PathBuf> {
    let name = token();
    let tmp = dir.join(format!(".{name}.tmp"));
    let list = dir.join(format!("{name}.list"));
    let mut text = String::new();
    for file in files {
        // A path with a line break cannot be told apart from two paths; it cannot be a real selection.
        if let Some(s) = file.to_str().filter(|s| !s.contains(['\n', '\r'])) {
            text.push_str(s);
            text.push('\n');
        }
    }
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, &list)?;
    Ok(list)
}

fn read_all(lists: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for list in lists {
        if let Ok(text) = fs::read_to_string(list) {
            files.extend(text.lines().filter(|l| !l.is_empty()).map(PathBuf::from));
        }
        let _ = fs::remove_file(list);
    }
    // Name order, without regard to case; the same file named twice counts once.
    files.sort_by_key(|p| {
        (
            p.file_name().map(|n| n.to_string_lossy().to_lowercase()),
            p.to_string_lossy().into_owned(),
        )
    });
    files.dedup();
    files
}

fn lock_is_stale(lock: &Path, stale: Duration) -> bool {
    fs::metadata(lock)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age > stale)
}

/// Join the batch of `key` (a kind of work: an action, a preset) with these files. `root`
/// is a private folder of this user's; a folder per key is made inside it.
pub fn join(root: &Path, key: &str, files: &[PathBuf], timing: &Timing) -> io::Result<Joined> {
    let dir = root.join(folder_for(key));
    fs::create_dir_all(&dir)?;
    let mine = write_list(&dir, files)?;
    let lock = dir.join("leader.lock");
    let started = Instant::now();

    loop {
        // A leader that has come and gone took this process's files with it.
        if !mine.exists() {
            return Ok(Joined::Follower);
        }
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", std::process::id());
                drop(f);
                // The list may have been taken between the check above and the lock: then there is
                // nothing left to lead, and leading an empty batch would run the action twice.
                if !mine.exists() {
                    let _ = fs::remove_file(&lock);
                    return Ok(Joined::Follower);
                }
                return Ok(Joined::Leader(lead(&dir, &lock, timing)));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&lock, timing.stale) {
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                if started.elapsed() > timing.follower_max {
                    // A leader that never finishes: do our own files and leave its alone.
                    let own = read_all(&BTreeSet::from([mine]));
                    return Ok(Joined::Leader(own));
                }
                std::thread::sleep(STEP);
            }
            Err(e) => {
                let _ = fs::remove_file(&mine);
                return Err(e);
            }
        }
    }
}

/// The leader's part: wait for the lists to stop arriving, then take them all.
fn lead(dir: &Path, lock: &Path, timing: &Timing) -> Vec<PathBuf> {
    let began = Instant::now();
    let mut seen = lists_in(dir);
    let mut quiet_since = Instant::now();
    loop {
        std::thread::sleep(STEP);
        let now = lists_in(dir);
        if now != seen {
            seen = now;
            quiet_since = Instant::now();
        }
        if quiet_since.elapsed() >= timing.grace || began.elapsed() >= timing.collect_max {
            break;
        }
    }
    let files = read_all(&lists_in(dir));
    let _ = fs::remove_file(lock);
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sheaf-agg-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).expect("dir");
        d
    }

    fn quick() -> Timing {
        Timing {
            grace: Duration::from_millis(300),
            collect_max: Duration::from_secs(5),
            follower_max: Duration::from_secs(10),
            stale: Duration::from_secs(60),
        }
    }

    #[test]
    fn a_lone_launch_leads_with_its_own_files() {
        let root = scratch("lone");
        let got = join(&root, "compress", &["b.pdf".into(), "a.pdf".into()], &quick()).expect("join");
        assert_eq!(got, Joined::Leader(vec!["a.pdf".into(), "b.pdf".into()]), "by name");
        // The lock is gone, so the next batch starts clean.
        let again = join(&root, "compress", &["c.pdf".into()], &quick()).expect("join");
        assert_eq!(again, Joined::Leader(vec!["c.pdf".into()]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn many_launches_at_once_make_one_batch_with_one_leader() {
        let root = Arc::new(scratch("many"));
        let timing = Arc::new(quick());
        let handles: Vec<_> = (0..12)
            .map(|i| {
                let (root, timing) = (Arc::clone(&root), Arc::clone(&timing));
                std::thread::spawn(move || {
                    // Explorer's processes do not start at the same instant.
                    std::thread::sleep(Duration::from_millis(i * 7));
                    join(&root, "merge", &[PathBuf::from(format!("/in/file{i:02}.pdf"))], &timing).expect("join")
                })
            })
            .collect();
        let results: Vec<Joined> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
        let leaders: Vec<&Vec<PathBuf>> = results
            .iter()
            .filter_map(|r| match r {
                Joined::Leader(files) => Some(files),
                Joined::Follower => None,
            })
            .collect();
        assert_eq!(leaders.len(), 1, "exactly one process does the work");
        let want: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!("/in/file{i:02}.pdf"))).collect();
        assert_eq!(*leaders[0], want, "all twelve files, in name order");
        assert_eq!(results.iter().filter(|r| **r == Joined::Follower).count(), 11);
        let _ = fs::remove_dir_all(&*root);
    }

    #[test]
    fn a_late_arrival_is_not_lost() {
        let root = Arc::new(scratch("late"));
        let timing = Arc::new(quick());
        let first = {
            let (root, timing) = (Arc::clone(&root), Arc::clone(&timing));
            std::thread::spawn(move || join(&root, "ocr", &["a.pdf".into()], &timing).expect("join"))
        };
        // Long after the first batch closed.
        std::thread::sleep(Duration::from_millis(700));
        let second = join(&root, "ocr", &["z.pdf".into()], &timing).expect("join");
        assert_eq!(first.join().expect("thread"), Joined::Leader(vec!["a.pdf".into()]));
        assert_eq!(
            second,
            Joined::Leader(vec!["z.pdf".into()]),
            "a new batch, a new leader"
        );
        let _ = fs::remove_dir_all(&*root);
    }

    #[test]
    fn different_kinds_of_work_do_not_mix() {
        let root = Arc::new(scratch("kinds"));
        let timing = Arc::new(quick());
        let a = {
            let (root, timing) = (Arc::clone(&root), Arc::clone(&timing));
            std::thread::spawn(move || join(&root, "merge", &["m.pdf".into()], &timing).expect("join"))
        };
        let b = join(&root, "compress", &["c.pdf".into()], &timing).expect("join");
        assert_eq!(b, Joined::Leader(vec!["c.pdf".into()]));
        assert_eq!(a.join().expect("thread"), Joined::Leader(vec!["m.pdf".into()]));
        let _ = fs::remove_dir_all(&*root);
    }

    #[test]
    fn a_lock_left_by_a_crashed_leader_is_taken_over() {
        let root = scratch("stale");
        let dir = root.join("rotate");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("leader.lock"), b"99999\n").expect("lock");
        let timing = Timing {
            stale: Duration::ZERO,
            ..quick()
        };
        std::thread::sleep(Duration::from_millis(30));
        let got = join(&root, "rotate", &["r.pdf".into()], &timing).expect("join");
        assert_eq!(got, Joined::Leader(vec!["r.pdf".into()]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_leader_that_never_finishes_does_not_trap_a_follower() {
        let root = scratch("stuck");
        let dir = root.join("split");
        fs::create_dir_all(&dir).expect("dir");
        fs::write(dir.join("leader.lock"), b"1\n").expect("lock");
        let timing = Timing {
            follower_max: Duration::from_millis(200),
            ..quick()
        };
        let got = join(&root, "split", &["s.pdf".into()], &timing).expect("join");
        assert_eq!(
            got,
            Joined::Leader(vec!["s.pdf".into()]),
            "does its own files after a while"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keys_and_paths_cannot_escape_or_confuse() {
        assert_eq!(folder_for("preset-Contract scan"), "preset-Contract_scan");
        assert_eq!(folder_for("../../x"), "______x");
        assert_eq!(folder_for(""), "_");
        let root = scratch("odd");
        // A path with a line break is dropped, not split into two paths.
        let got = join(&root, "k", &["ok.pdf".into(), "bad\nname.pdf".into()], &quick()).expect("join");
        assert_eq!(got, Joined::Leader(vec!["ok.pdf".into()]));
        let _ = fs::remove_dir_all(root);
    }
}
