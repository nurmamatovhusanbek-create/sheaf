//! "Did the last run end properly?" A program that dies on the spot (an access
//! violation in a driver, the system killing it) cannot say so itself, so the
//! next start asks the file system: a mark is written while the program runs
//! and cleared when it ends cleanly. A mark that is still there, with nobody
//! holding its lock, means the last run never got to clear it.
//!
//! The lock tells a crash from a second copy of the program running at the
//! same time: a held lock means "someone is running", and that copy owns the
//! mark.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const FILE: &str = "running.mark";
const MARK: &[u8] = b"running";

/// Held for the life of a window. Dropping it without [`RunMark::finish`] leaves the mark behind.
#[derive(Debug)]
pub struct RunMark {
    file: Option<File>,
    previous_run_ended_badly: bool,
}

impl RunMark {
    /// Look at what the last run left, then mark this one. Never fails: with
    /// no usable folder it just reports nothing.
    pub fn begin(dir: &Path) -> RunMark {
        let none = RunMark {
            file: None,
            previous_run_ended_badly: false,
        };
        if std::fs::create_dir_all(dir).is_err() {
            return none;
        }
        let Ok(mut file) = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(FILE))
        else {
            return none;
        };
        // Someone else is running: they own the mark, and this run adds nothing.
        if file.try_lock().is_err() {
            return none;
        }
        let mut old = Vec::new();
        let _ = file.read_to_end(&mut old);
        let bad = old == MARK;
        let written = file
            .seek(SeekFrom::Start(0))
            .and_then(|_| file.set_len(0))
            .and_then(|()| file.write_all(MARK))
            .and_then(|()| file.flush());
        if written.is_err() {
            return none;
        }
        RunMark {
            file: Some(file),
            previous_run_ended_badly: bad,
        }
    }

    pub fn previous_run_ended_badly(&self) -> bool {
        self.previous_run_ended_badly
    }

    /// The run ended properly: clear the mark.
    pub fn finish(mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.set_len(0);
        }
    }
}
