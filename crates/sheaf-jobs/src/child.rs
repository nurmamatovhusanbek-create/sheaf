//! One worker process and its pipes.

use crate::contain::{self, Containment};
use crate::PoolConfig;
use sheaf_core::{AppError, ErrorCode, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

/// Longest protocol line accepted from a worker. A worker handles hostile
/// files, so its output is not trusted either (SEC-TB2).
const MAX_LINE: u64 = 1 << 20;

pub(crate) enum Line {
    Text(String),
    Nothing,
    /// The worker exited, closed its pipe or sent an oversized line.
    Closed,
}

pub(crate) struct WorkerProc {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// Holds the worker's whole process tree (SEC-ISOLATE).
    contained: Containment,
}

impl WorkerProc {
    pub(crate) fn spawn(cfg: &PoolConfig) -> Result<WorkerProc> {
        // Argument array, never a shell string. Only the three stdio handles
        // are inherited by the child.
        let mut cmd = Command::new(&cfg.program);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        contain::prepare(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|_| AppError::new(ErrorCode::EngineMissing, "worker process could not be started"))?;
        // Contain it before it is sent anything to parse.
        let contained = match Containment::attach(&child) {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(i), Some(o)) => (i, o),
            _ => {
                contained.kill_tree();
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
            child,
            stdin,
            lines,
            contained,
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
        self.contained.rss_mb(&self.child)
    }
}

impl Drop for WorkerProc {
    fn drop(&mut self) {
        // The tree first, while the worker's pid is still ours (not yet reaped).
        self.contained.kill_tree();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
