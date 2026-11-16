//! SEC-ISOLATE: containing a worker and everything it starts.
//!
//! A worker handles hostile files and starts Tesseract for OCR. Killing the
//! worker alone (cancel, timeout, memory cap) would leave Tesseract running
//! with a page-sized bitmap in the session dir, and on Windows would keep that
//! folder from being deleted. So every worker gets a container that holds its
//! whole process tree and can end all of it at once:
//!
//! - Unix: its own process group, ended with `SIGKILL` to the group.
//! - Windows: a Job Object with kill-on-close, a hard memory cap per process,
//!   a cap on the number of processes and no access to the clipboard, the
//!   desktop or other windows' handles.
//!
//! Not done: a reduced token / low integrity level on Windows (PH-7). A
//! worker that is exploited still runs with the user's rights, minus the above.

use sheaf_core::consts::DEFAULT_WORKER_RSS_MB;
use sheaf_core::{AppError, ErrorCode, Result};
use std::process::Child;

/// Hard per-process memory cap in a Windows Job Object, in MiB. The soft cap
/// polled by the pool (`Limits::rss_mb`) kills a runaway worker first; this
/// is the backstop for one that allocates faster than the pool polls, and it
/// counts committed memory, which runs above resident size.
#[cfg_attr(not(windows), allow(dead_code))]
const HARD_MEMORY_CAP_MB: u64 = 2 * DEFAULT_WORKER_RSS_MB;
/// Worker, Tesseract and a little headroom.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_PROCESSES: u32 = 8;

fn not_contained() -> AppError {
    AppError::new(ErrorCode::EngineMissing, "worker process could not be contained")
}

#[cfg(unix)]
mod imp {
    use super::*;
    use rustix::process::{kill_process_group, Pid, Signal};
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub(crate) struct Containment {
        pgid: i32,
    }

    /// Before spawning: the child leads a new process group, and so do its children.
    pub(crate) fn prepare(cmd: &mut Command) {
        cmd.process_group(0);
    }

    impl Containment {
        pub(crate) fn attach(child: &Child) -> Result<Containment> {
            let pgid = i32::try_from(child.id()).map_err(|_| not_contained())?;
            Ok(Containment { pgid })
        }

        /// End the whole tree. Call before the child is reaped, while its pid
        /// (and so the group id) still cannot belong to anybody else.
        pub(crate) fn kill_tree(&self) {
            if let Some(pid) = Pid::from_raw(self.pgid) {
                let _ = kill_process_group(pid, Signal::KILL);
            }
        }

        pub(crate) fn rss_mb(&self, child: &Child) -> Option<u64> {
            #[cfg(target_os = "linux")]
            {
                let status = std::fs::read_to_string(format!("/proc/{}/status", child.id())).ok()?;
                let kb: u64 = status
                    .lines()
                    .find_map(|l| l.strip_prefix("VmRSS:"))?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()?;
                Some(kb / 1024)
            }
            #[cfg(not(target_os = "linux"))]
            {
                let _ = child;
                None
            }
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)] // Win32 Job Object calls; each block states its invariant.
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Command;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP,
        JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS, JOB_OBJECT_UILIMIT_GLOBALATOMS,
        JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};

    pub(crate) struct Containment {
        job: HANDLE,
    }

    // SAFETY: a Job Object handle is just a kernel handle value; the kernel
    // synchronizes access, and this type only closes it once, in `Drop`.
    unsafe impl Send for Containment {}

    pub(crate) fn prepare(_cmd: &mut Command) {}

    impl Containment {
        /// Put the freshly spawned child in a new job. The worker reads its
        /// first request only after this returns, so nothing hostile has been
        /// parsed while it was still outside the job. Any failure is an
        /// error: a worker that cannot be contained is not used.
        pub(crate) fn attach(child: &Child) -> Result<Containment> {
            // SAFETY: both arguments may be null (default security, no name).
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(not_contained());
            }
            let contained = Containment { job }; // closes the handle on any early return

            // SAFETY: zeroed is a valid value for this plain-data struct.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
            limits.BasicLimitInformation.ActiveProcessLimit = MAX_PROCESSES;
            limits.ProcessMemoryLimit = usize::try_from(HARD_MEMORY_CAP_MB << 20).unwrap_or(usize::MAX);
            // SAFETY: `job` is a live handle; the pointer and size describe `limits`.
            let ok = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast::<c_void>(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(not_contained());
            }

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
            // SAFETY: as above, for `ui`.
            let ok = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectBasicUIRestrictions,
                    std::ptr::from_ref(&ui).cast::<c_void>(),
                    std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
                )
            };
            if ok == 0 {
                return Err(not_contained());
            }

            // SAFETY: `job` is live, and the child's process handle is owned by `child`.
            let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
            if ok == 0 {
                return Err(not_contained());
            }
            Ok(contained)
        }

        /// Closing the handle ends every process in the job (kill-on-close).
        pub(crate) fn kill_tree(&self) {}

        /// Working set of the worker process, in MiB.
        pub(crate) fn rss_mb(&self, child: &Child) -> Option<u64> {
            // SAFETY: zeroed is valid for this plain-data struct; the handle is
            // the child's own and `cb` is the struct's size.
            let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
            let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = unsafe { K32GetProcessMemoryInfo(child.as_raw_handle() as HANDLE, &mut counters, cb) };
            (ok != 0).then(|| (counters.WorkingSetSize as u64) >> 20)
        }
    }

    impl Drop for Containment {
        fn drop(&mut self) {
            // SAFETY: the handle was created in `attach` and is closed only here.
            unsafe { CloseHandle(self.job) };
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::*;
    use std::process::Command;

    pub(crate) struct Containment;
    pub(crate) fn prepare(_cmd: &mut Command) {}
    impl Containment {
        pub(crate) fn attach(_child: &Child) -> Result<Containment> {
            Ok(Containment)
        }
        pub(crate) fn kill_tree(&self) {}
        pub(crate) fn rss_mb(&self, _child: &Child) -> Option<u64> {
            None
        }
    }
}

pub(crate) use imp::{prepare, Containment};
