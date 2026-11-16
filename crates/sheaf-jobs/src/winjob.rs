//! Windows Job Objects for worker processes (SEC-ISOLATE).
//!
//! Each worker goes into a job of its own with these limits: every process in
//! the job dies when the job handle closes (so a crashed or killed main
//! process leaves no orphaned worker or tesseract behind), no process may
//! commit more than the hard cap (a backstop under the polling check in
//! `child.rs`), an unhandled exception ends a process instead of showing
//! a dialog nobody can answer, at most [`MAX_PROCESSES`] processes live in it
//! (a hostile file cannot fork-bomb the machine), and its processes may not
//! touch the clipboard, the desktop, the display settings, system parameters or
//! window handles of other programs (a worker that renders documents has no
//! business with any of them). Children of the worker (tesseract) join the
//! job automatically.
//!
//! Win32 wrapper module: the only `unsafe` in this crate (DOC-AGENT-RULES rule 5).
//! Written against the Windows API docs and exercised by the Windows CI run.

#![allow(unsafe_code)]

use std::os::windows::io::AsRawHandle;
use std::process::Child;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS,
    JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
    JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

/// Most processes a worker's job may hold: the worker itself and the Tesseract it starts, with room
/// to spare, and no room for anything a document could talk a parser into starting.
pub(crate) const MAX_PROCESSES: u32 = 6;

/// A job holding one worker. Dropping it ends every process in it.
pub(crate) struct Contained {
    job: HANDLE,
}

// SAFETY: a job handle is a kernel object identifier, valid from any thread.
unsafe impl Send for Contained {}

impl Drop for Contained {
    fn drop(&mut self) {
        // SAFETY: `job` came from CreateJobObjectW and is closed exactly once, here.
        // Closing it triggers KILL_ON_JOB_CLOSE for whatever is still running.
        unsafe {
            let _ = CloseHandle(self.job);
        }
    }
}

/// Put `child` into a new job with a per-process memory cap of `cap_mb`.
/// `None` if that is not possible (for example the parent already sits in a job
/// that forbids nesting); the worker then runs with the polling check alone.
pub(crate) fn contain(child: &Child, cap_mb: u64) -> Option<Contained> {
    // SAFETY: plain Win32 calls. `info` is a fully initialised struct whose size is
    // passed; the process handle is borrowed from `child`, which outlives the call.
    unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = MAX_PROCESSES;
        info.ProcessMemoryLimit = usize::try_from(cap_mb.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX);
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        // No clipboard, desktop, display or system-parameter access, no handles to other programs' windows.
        // Best effort: the limits above are what matter most, and a refusal here must not stop work.
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_EXITWINDOWS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        };
        let _ = SetInformationJobObject(
            job,
            JobObjectBasicUIRestrictions,
            (&raw const ui).cast(),
            std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        );
        let assigned = set.is_ok() && AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())).is_ok();
        if assigned {
            Some(Contained { job })
        } else {
            let _ = CloseHandle(job);
            None
        }
    }
}

/// Working set of `child` in MiB: what the polling check compares to the job's limit.
pub(crate) fn working_set_mb(child: &Child) -> Option<u64> {
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    // SAFETY: `counters` is a valid out-struct and its size is passed; the handle is borrowed.
    unsafe {
        GetProcessMemoryInfo(
            HANDLE(child.as_raw_handle()),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .ok()?;
    }
    Some(counters.WorkingSetSize as u64 / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Ask the system what the job really enforces: the process cap, the memory cap, kill on close,
    /// and the UI restrictions.
    #[test]
    fn the_job_enforces_the_limits_that_were_asked_for() {
        use windows::Win32::System::JobObjects::{
            QueryInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOB_OBJECT_UILIMIT_READCLIPBOARD,
        };
        let child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("start ping");
        let mut child = child;
        let job = contain(&child, 512).expect("a process can join a job on a CI runner");
        // SAFETY: out-structs of the sizes passed; the job handle is open.
        unsafe {
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            QueryInformationJobObject(
                Some(job.job),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                None,
            )
            .expect("query limits");
            let flags = info.BasicLimitInformation.LimitFlags;
            assert!(flags.0 & JOB_OBJECT_LIMIT_ACTIVE_PROCESS.0 != 0, "process cap on");
            assert_eq!(info.BasicLimitInformation.ActiveProcessLimit, MAX_PROCESSES);
            assert!(flags.0 & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.0 != 0);
            assert!(flags.0 & JOB_OBJECT_LIMIT_PROCESS_MEMORY.0 != 0);
            assert_eq!(info.ProcessMemoryLimit, 512 * 1024 * 1024);

            let mut ui = JOBOBJECT_BASIC_UI_RESTRICTIONS::default();
            QueryInformationJobObject(
                Some(job.job),
                JobObjectBasicUIRestrictions,
                (&raw mut ui).cast(),
                std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
                None,
            )
            .expect("query UI restrictions");
            assert!(
                ui.UIRestrictionsClass.0 & JOB_OBJECT_UILIMIT_READCLIPBOARD.0 != 0,
                "no clipboard"
            );
            assert!(
                ui.UIRestrictionsClass.0 & JOB_OBJECT_UILIMIT_HANDLES.0 != 0,
                "no foreign window handles"
            );
        }
        let _ = child.kill();
        drop(job);
    }

    /// Closing the job ends the process in it: what keeps a crashed main
    /// process from leaving workers behind.
    #[test]
    fn closing_the_job_kills_what_is_inside() {
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .expect("start ping");
        let job = contain(&child, 512).expect("a process can join a job on a CI runner");
        assert!(
            child.try_wait().expect("poll").is_none(),
            "still running while the job is open"
        );
        assert!(working_set_mb(&child).is_some());
        drop(job);
        let end = Instant::now() + Duration::from_secs(5);
        while child.try_wait().expect("poll").is_none() {
            assert!(Instant::now() < end, "the process outlived its job");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
