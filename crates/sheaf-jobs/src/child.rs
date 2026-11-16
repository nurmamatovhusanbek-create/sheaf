//! One worker process and its pipes.

use crate::PoolConfig;
use sheaf_core::{AppError, ErrorCode, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// Longest protocol line accepted from a worker. A worker handles hostile
/// files, so its output is not trusted either (SEC-TB2).
const MAX_LINE: u64 = 1 << 20;

/// Per-process memory the operating system itself refuses to exceed: twice the
/// default worker limit, a backstop under the polling check (SEC-LIMITS).
#[cfg(windows)]
const HARD_CAP_MB: u64 = 2 * sheaf_core::consts::DEFAULT_WORKER_RSS_MB;

pub(crate) enum Line {
    Text(String),
    Nothing,
    /// The worker exited, closed its pipe or sent an oversized line.
    Closed,
}

pub(crate) struct WorkerProc {
    // Declared before `child` so the job (and everything in it) goes first on drop.
    #[cfg(windows)]
    _job: Option<crate::winjob::Contained>,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl WorkerProc {
    pub(crate) fn spawn(cfg: &PoolConfig) -> Result<WorkerProc> {
        // Argument array, never a shell string. Only the three stdio handles
        // are inherited by the child.
        let mut command = Command::new(&cfg.program);
        command
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // No console window for a worker that is a console program (the app has none of its own).
        sheaf_store::hide_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| AppError::new(ErrorCode::EngineMissing, "worker process could not be started"))?;
        // SEC-ISOLATE: contain the worker (and tesseract below it) before it reads a byte of a document.
        #[cfg(windows)]
        let job = crate::winjob::contain(&child, HARD_CAP_MB);
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(i), Some(o)) => (i, o),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::internal("worker pipes missing"));
            }
        };
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut buf = Vec::new();
                match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut buf) {
                    Ok(n) if n > 0 && buf.ends_with(b"\n") => {
                        let text = String::from_utf8_lossy(&buf).trim_end().to_string();
                        if tx.send(text).is_err() {
                            break;
                        }
                    }
                    // EOF, read error, or a line longer than the cap: stop.
                    // Dropping `tx` tells the pool the worker is gone.
                    _ => break,
                }
            }
        });
        Ok(WorkerProc {
            #[cfg(windows)]
            _job: job,
            child,
            stdin,
            lines,
        })
    }

    pub(crate) fn send(&mut self, line: &str) -> std::io::Result<()> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    pub(crate) fn recv(&mut self, wait: Duration) -> Line {
        match self.lines.recv_timeout(wait) {
            Ok(text) => Line::Text(text),
            Err(RecvTimeoutError::Timeout) => Line::Nothing,
            Err(RecvTimeoutError::Disconnected) => Line::Closed,
        }
    }

    /// Resident memory in MiB, where the platform lets us read it.
    pub(crate) fn rss_mb(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            let status = std::fs::read_to_string(format!("/proc/{}/status", self.child.id())).ok()?;
            let kb: u64 = status
                .lines()
                .find_map(|l| l.strip_prefix("VmRSS:"))?
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            Some(kb / 1024)
        }
        #[cfg(windows)]
        {
            crate::winjob::working_set_mb(&self.child)
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            None
        }
    }
}

impl Drop for WorkerProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
