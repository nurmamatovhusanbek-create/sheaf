//! SCAN-ISOLATE: the scanner runs in a process of its own.
//!
//! Scanner drivers are other people's code, loaded into whoever talks to them,
//! and they are the part of a scan that fails in ways no `Result` reports: an
//! access violation, a corrupted heap, a hang. Run inside the app, one of those
//! ends the app with no message. So the app never talks to a driver itself:
//! [`ProcessBackend`] starts the program again in a helper mode, [`run_helper`]
//! there drives the real backend, and the two speak a small framed protocol on
//! the helper's pipes. If the helper dies, hangs or lies, the scan fails with
//! an error that names what happened, the pages already received stay, and
//! trying again starts a fresh helper.
//!
//! Frames, both directions in bytes: a tag byte, a `u32` little-endian length,
//! then the payload. Helper to app: `D` a device (`id`, newline, `name`), `P` a
//! page (`p` or `j`, then the image bytes), `E` a failure (JSON of the error),
//! `Z` the end of a successful run. App to helper: any byte means "cancel";
//! closing the pipe means the app is gone.

use crate::{
    CancelToken, Device, ScanBackend, ScanColor, ScanPaper, ScanProfile, ScanSource, ScannedPage, MAX_DEVICES,
    MAX_PAGE_BYTES,
};
use sheaf_core::{AppError, ErrorCode, Result};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// The argument that puts the program in helper mode (the shell handles it before anything else).
pub const HELPER_FLAG: &str = "--scan-helper";

/// Largest frame that is not a page.
const MAX_SMALL_FRAME: usize = 64 * 1024;
/// How long the helper may stay silent before the app gives up on it.
const SCAN_IDLE: Duration = Duration::from_secs(300);
const DEVICES_IDLE: Duration = Duration::from_secs(60);
/// After asking nicely, how long a cancelled helper has to stop before it is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(3);

fn crashed(what: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::DeviceCrashed, what)
}

// -- frames -------------------------------------------------------------------

fn write_frame(out: &mut impl Write, tag: u8, payload: &[u8]) -> std::io::Result<()> {
    let len = u32::try_from(payload.len()).map_err(|_| std::io::Error::other("frame too long"))?;
    out.write_all(&[tag])?;
    out.write_all(&len.to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

#[derive(Debug)]
enum Frame {
    Device(Device),
    Page {
        ext: &'static str,
        data: Vec<u8>,
    },
    Failed(AppError),
    /// A timed note from the helper (SCAN-TIMING).
    Note(String),
    Done,
    /// The helper broke the protocol; the text says how (no content from the helper).
    Bad(&'static str),
}

/// Read frames until the end of the run, the pipe closing or a bad frame.
fn read_frames(mut input: impl Read, tx: Sender<Frame>) {
    loop {
        let mut head = [0u8; 5];
        if input.read_exact(&mut head).is_err() {
            return; // closed: the receiver learns it from the disconnect
        }
        let tag = head[0];
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        let limit = if tag == b'P' {
            MAX_PAGE_BYTES + 1
        } else {
            MAX_SMALL_FRAME
        };
        if len > limit {
            let _ = tx.send(Frame::Bad("a frame was larger than allowed"));
            return;
        }
        // Do not trust the length for the allocation: grow as bytes really arrive.
        let mut payload = Vec::with_capacity(len.min(1 << 20));
        if (&mut input).take(len as u64).read_to_end(&mut payload).is_err() || payload.len() != len {
            return;
        }
        let frame = match tag {
            b'D' => match std::str::from_utf8(&payload).ok().and_then(|s| s.split_once('\n')) {
                Some((id, name)) => Frame::Device(Device {
                    id: id.to_string(),
                    name: name.to_string(),
                }),
                None => Frame::Bad("a device entry was malformed"),
            },
            b'P' => match payload.split_first() {
                Some((b'p', data)) => Frame::Page {
                    ext: "png",
                    data: data.to_vec(),
                },
                Some((b'j', data)) => Frame::Page {
                    ext: "jpg",
                    data: data.to_vec(),
                },
                _ => Frame::Bad("a page had no format"),
            },
            b'E' => match serde_json::from_slice::<AppError>(&payload) {
                Ok(e) => Frame::Failed(e),
                Err(_) => Frame::Bad("an error report was malformed"),
            },
            b'T' => Frame::Note(String::from_utf8_lossy(&payload).into_owned()),
            b'Z' => Frame::Done,
            _ => Frame::Bad("an unknown frame arrived"),
        };
        let last = matches!(frame, Frame::Done | Frame::Failed(_) | Frame::Bad(_));
        if tx.send(frame).is_err() || last {
            return;
        }
    }
}

// -- the app side --------------------------------------------------------------

/// A scanner reached through a helper process (see the module docs).
#[derive(Debug, Clone)]
pub struct ProcessBackend {
    program: PathBuf,
    /// Arguments that put the program in helper mode, before ours.
    prefix: Vec<OsString>,
    /// Extra environment for the helper (tests pick the fake's behaviour with it).
    env: Vec<(OsString, OsString)>,
    /// Timed notes about the last scan: the helper's, and ours about what reached the app (SCAN-TIMING).
    notes: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ProcessBackend {
    pub fn new(program: impl Into<PathBuf>, prefix: Vec<OsString>) -> ProcessBackend {
        ProcessBackend {
            program: program.into(),
            prefix,
            env: Vec::new(),
            notes: Default::default(),
        }
    }

    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> ProcessBackend {
        self.env.push((key.into(), value.into()));
        self
    }

    fn note(&self, line: String) {
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(line);
    }

    fn spawn(&self, args: &[OsString]) -> Result<Child> {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix)
            .args(args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.spawn()
            .map_err(|_| AppError::new(ErrorCode::DeviceError, "the scanner helper could not be started"))
    }

    /// Run one helper to its end. `on` gets each device or page and says whether
    /// to go on (`false` stops the helper, as a cancel does).
    fn drive(
        &self,
        args: &[OsString],
        cancel: &CancelToken,
        idle: Duration,
        mut on: impl FnMut(Frame) -> bool,
    ) -> Result<()> {
        let mut child = self.spawn(args)?;
        let mut stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::internal("scanner helper pipes missing"));
        };
        let (tx, rx): (Sender<Frame>, Receiver<Frame>) = mpsc::channel();
        let reader = std::thread::spawn(move || read_frames(stdout, tx));

        let (mut finished, mut failure, mut stopped_by_us) = (false, None::<AppError>, false);
        let (mut last_heard, mut asked_at) = (Instant::now(), None::<Instant>);
        loop {
            if (cancel.is_cancelled() || stopped_by_us) && asked_at.is_none() {
                // Ask first: a driver that is told to stop halts the feeder properly.
                if let Some(pipe) = stdin.as_mut() {
                    let _ = pipe.write_all(b"C").and_then(|()| pipe.flush());
                }
                asked_at = Some(Instant::now());
            }
            if asked_at.is_some_and(|t| t.elapsed() > CANCEL_GRACE) {
                let _ = child.kill();
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(Frame::Done) => finished = true,
                Ok(Frame::Note(line)) => self.note(line),
                Ok(Frame::Failed(e)) => failure = Some(e),
                Ok(Frame::Bad(why)) => {
                    failure = Some(crashed(format!("the scanner helper broke the protocol: {why}")));
                    let _ = child.kill();
                }
                Ok(item) => {
                    last_heard = Instant::now();
                    if !on(item) {
                        stopped_by_us = true;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if asked_at.is_none() && last_heard.elapsed() > idle {
                        failure = Some(AppError::new(
                            ErrorCode::DeviceError,
                            "the scanner stopped answering and was shut down",
                        ));
                        let _ = child.kill();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        drop(stdin); // closing the pipe also tells a live helper that we are done
        let status = child.wait().ok();
        let _ = reader.join();

        if cancel.is_cancelled() || stopped_by_us {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if finished {
            return Ok(());
        }
        // No end marker, no error report: the helper died under the driver.
        let how = match status.and_then(|s| s.code()) {
            Some(code) => format!("exit code {:#010x}", code as u32),
            None => "no exit code".to_string(),
        };
        Err(crashed(format!("the scanner helper stopped without finishing ({how})")))
    }
}

fn profile_args(p: &ScanProfile) -> Vec<OsString> {
    let source = match p.source {
        ScanSource::Flatbed => "flatbed",
        ScanSource::Feeder => "feeder",
    };
    let color = match p.color {
        ScanColor::Color => "color",
        ScanColor::Gray => "gray",
        ScanColor::Bw => "bw",
    };
    let paper = match p.paper {
        ScanPaper::A4 => "a4",
        ScanPaper::A3 => "a3",
        ScanPaper::Letter => "letter",
        ScanPaper::Auto => "auto",
    };
    let mut args: Vec<OsString> = vec!["scan".into()];
    for (flag, value) in [
        ("--device", p.device_id.clone()),
        ("--source", source.into()),
        ("--duplex", if p.duplex { "1" } else { "0" }.into()),
        ("--dpi", p.dpi.to_string()),
        ("--color", color.into()),
        ("--paper", paper.into()),
    ] {
        args.push(flag.into());
        args.push(value.into());
    }
    args
}

fn parse_profile(args: &[String]) -> Result<ScanProfile> {
    let bad = || AppError::invalid("bad scan arguments");
    let mut profile = ScanProfile::new("");
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(bad)?;
        match flag.as_str() {
            "--device" => profile.device_id = value.clone(),
            "--source" => {
                profile.source = match value.as_str() {
                    "flatbed" => ScanSource::Flatbed,
                    "feeder" => ScanSource::Feeder,
                    _ => return Err(bad()),
                }
            }
            "--duplex" => profile.duplex = value == "1",
            "--dpi" => profile.dpi = value.parse().map_err(|_| bad())?,
            "--color" => {
                profile.color = match value.as_str() {
                    "color" => ScanColor::Color,
                    "gray" => ScanColor::Gray,
                    "bw" => ScanColor::Bw,
                    _ => return Err(bad()),
                }
            }
            "--paper" => {
                profile.paper = match value.as_str() {
                    "a4" => ScanPaper::A4,
                    "a3" => ScanPaper::A3,
                    "letter" => ScanPaper::Letter,
                    "auto" => ScanPaper::Auto,
                    _ => return Err(bad()),
                }
            }
            _ => return Err(bad()),
        }
    }
    profile.validate()?;
    Ok(profile)
}

impl ScanBackend for ProcessBackend {
    fn devices(&self) -> Result<Vec<Device>> {
        let mut found = Vec::new();
        self.drive(&["devices".into()], &CancelToken::new(), DEVICES_IDLE, |frame| {
            if let Frame::Device(d) = frame {
                if found.len() < MAX_DEVICES {
                    found.push(d);
                }
            }
            true
        })?;
        Ok(found)
    }

    fn scan(&self, profile: &ScanProfile, pages: Sender<ScannedPage>, cancel: CancelToken) -> Result<()> {
        profile.validate()?;
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        let began = Instant::now();
        self.note("app: scanner helper starting".to_string());
        let mut number = 0usize;
        let result = self.drive(&profile_args(profile), &cancel, SCAN_IDLE, |frame| {
            let Frame::Page { ext, data } = frame else { return true };
            number += 1;
            self.note(format!(
                "app: page {number} reached the app after {} ms ({} bytes, {ext})",
                began.elapsed().as_millis(),
                data.len()
            ));
            // A receiver that has gone away counts as a cancel.
            pages.send(ScannedPage { number, data, ext }).is_ok()
        });
        self.note(format!(
            "app: scan ended after {} ms with {number} page(s): {}",
            began.elapsed().as_millis(),
            result
                .as_ref()
                .map_or_else(|e| format!("{:?}", e.code), |()| "ok".to_string())
        ));
        result
    }

    fn diagnostics(&self) -> Vec<String> {
        self.notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

// -- the helper side -------------------------------------------------------------

/// The helper's main: `args` are what follows the helper-mode flag. Drives
/// `backend` and writes frames to standard output. Returns the process exit
/// code: 0 when it ran to a verdict (success or a reported failure), 2 for
/// arguments it does not understand.
pub fn run_helper(args: &[OsString], backend: &dyn ScanBackend) -> i32 {
    let args: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut out = std::io::stdout().lock();
    let verdict = |out: &mut std::io::StdoutLock<'_>, result: Result<()>| -> i32 {
        let sent = match result {
            Ok(()) => write_frame(out, b'Z', &[]),
            Err(e) => write_frame(out, b'E', &serde_json::to_vec(&e).unwrap_or_default()),
        };
        i32::from(sent.is_err())
    };
    match args.first().map(String::as_str) {
        Some("devices") => {
            let result = backend.devices().and_then(|list| {
                for d in list.iter().take(MAX_DEVICES) {
                    let entry = format!("{}\n{}", d.id, d.name);
                    write_frame(&mut out, b'D', entry.as_bytes()).map_err(|_| AppError::internal("pipe closed"))?;
                }
                Ok(())
            });
            verdict(&mut out, result)
        }
        Some("scan") => {
            let profile = match parse_profile(&args[1..]) {
                Ok(p) => p,
                Err(_) => return 2,
            };
            let cancel = CancelToken::new();
            // Any byte from the app is a cancel; the pipe closing means the app is gone.
            let watcher = cancel.clone();
            std::thread::spawn(move || {
                let mut byte = [0u8; 1];
                let mut stdin = std::io::stdin();
                loop {
                    let n = stdin.read(&mut byte).unwrap_or(0);
                    watcher.cancel();
                    if n == 0 {
                        break;
                    }
                }
            });
            let (tx, rx) = mpsc::channel::<ScannedPage>();
            let result = std::thread::scope(|scope| {
                let scanning = scope.spawn(|| backend.scan(&profile, tx, cancel.clone()));
                for page in rx {
                    let mut payload = Vec::with_capacity(page.data.len() + 1);
                    payload.push(if page.ext == "jpg" { b'j' } else { b'p' });
                    payload.extend_from_slice(&page.data);
                    if write_frame(&mut out, b'P', &payload).is_err() {
                        cancel.cancel(); // the app is gone
                        break;
                    }
                }
                scanning
                    .join()
                    .unwrap_or_else(|_| Err(AppError::internal("the scanner backend panicked")))
            });
            // The timing log goes first, so it arrives even when the scan did not go well.
            for line in crate::diag::take() {
                if write_frame(&mut out, b'T', line.as_bytes()).is_err() {
                    break;
                }
            }
            verdict(&mut out, result)
        }
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_survives_the_command_line() {
        let mut p = ScanProfile::new("{6BDD1FC6-810F-11D0-BEC7-08002BE2092F}\\0001");
        p.source = ScanSource::Feeder;
        p.duplex = true;
        p.dpi = 600;
        p.color = ScanColor::Bw;
        p.paper = ScanPaper::Auto;
        let args: Vec<String> = profile_args(&p)
            .iter()
            .skip(1)
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(parse_profile(&args).expect("parse"), p);
        assert!(parse_profile(&["--dpi".into(), "99".into(), "--device".into(), "d".into()]).is_err());
        assert!(parse_profile(&["--nope".into(), "x".into()]).is_err());
        assert!(parse_profile(&["--device".into()]).is_err(), "a flag without a value");
    }

    #[test]
    fn frames_are_read_and_oversized_ones_refused() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b'D', b"id1\nMy scanner").expect("write");
        write_frame(&mut wire, b'P', b"pPNGDATA").expect("write");
        write_frame(&mut wire, b'Z', b"").expect("write");
        let (tx, rx) = mpsc::channel();
        read_frames(&wire[..], tx);
        let got: Vec<Frame> = rx.iter().collect();
        assert!(matches!(&got[0], Frame::Device(d) if d.id == "id1" && d.name == "My scanner"));
        assert!(matches!(&got[1], Frame::Page { ext: "png", data } if data == b"PNGDATA"));
        assert!(matches!(got[2], Frame::Done));

        // A length far beyond the limit is refused before anything is allocated for it.
        let mut lie = vec![b'P'];
        lie.extend((u32::MAX).to_le_bytes());
        let (tx, rx) = mpsc::channel();
        read_frames(&lie[..], tx);
        assert!(matches!(rx.recv().expect("a verdict"), Frame::Bad(_)));

        // A cut-off frame is a closed pipe, not a page.
        let mut cut = Vec::new();
        write_frame(&mut cut, b'P', &[b'p'; 100]).expect("write");
        cut.truncate(50);
        let (tx, rx) = mpsc::channel();
        read_frames(&cut[..], tx);
        assert!(rx.recv().is_err());
    }
}
