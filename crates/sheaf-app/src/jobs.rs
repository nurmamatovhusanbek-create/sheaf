//! App-level jobs (DATA-JOB, EVT-JOB): what the job bar shows. One app job is
//! a thread that may run many worker jobs (an export is a build per output
//! file, OCR is one worker job per page). Cancelling an app job stops it
//! before its next worker job and kills the worker job that is running.

use crate::api::{Emit, ErrorDto, Event};
use crate::plan::{CancelFlag, JobCtl};
use crate::Shared;
use serde::Serialize;
use serde_json::Value;
use sheaf_core::{AppError, JobId, JobState};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// What a job bar row shows (EVT-JOB payload). Paths are never sent: the UI
/// gets file names, and reveals through the job id (CMD-REVEAL).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub id: u64,
    pub kind: String,
    pub state: &'static str,
    pub progress: f32,
    /// File names written so far.
    pub outputs: Vec<String>,
    pub error: Option<ErrorDto>,
    /// Kind-specific numbers: sizes, counts of pages recognized, skipped, failed.
    pub detail: Value,
}

fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

struct Status {
    state: JobState,
    progress: f32,
    outputs: Vec<PathBuf>,
    error: Option<AppError>,
    detail: Value,
    last_emitted: f32,
}

/// One running or finished app job.
pub struct JobEntry {
    pub id: u64,
    pub kind: String,
    cancel: CancelFlag,
    /// The worker job currently queued or running for it; 0 when none.
    current: AtomicU64,
    status: Mutex<Status>,
}

impl JobEntry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn view(&self) -> JobView {
        let st = self.lock();
        JobView {
            id: self.id,
            kind: self.kind.clone(),
            state: state_name(st.state),
            progress: st.progress,
            outputs: st
                .outputs
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect(),
            error: st.error.as_ref().map(ErrorDto::from),
            detail: st.detail.clone(),
        }
    }

    pub fn state(&self) -> JobState {
        self.lock().state
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.state(), JobState::Done | JobState::Failed | JobState::Cancelled)
    }

    /// Files this job produced, in order. The only paths CMD-REVEAL will open.
    pub fn outputs(&self) -> Vec<PathBuf> {
        self.lock().outputs.clone()
    }

    pub(crate) fn finish(&self, result: Result<JobOutput, AppError>) {
        let mut st = self.lock();
        match result {
            Ok(out) => {
                st.state = JobState::Done;
                st.progress = 1.0;
                st.outputs = out.outputs;
                st.detail = out.detail;
            }
            Err(e) if e.code == sheaf_core::ErrorCode::Cancelled => st.state = JobState::Cancelled,
            Err(e) => {
                st.state = JobState::Failed;
                st.error = Some(e);
            }
        }
    }

    /// Report progress without emitting (the caller emits when it wants to).
    pub(crate) fn set_progress(&self, fraction: f32) {
        let mut st = self.lock();
        st.progress = fraction.clamp(0.0, 1.0);
        st.last_emitted = st.progress;
    }

    pub(crate) fn set_running(&self) {
        self.lock().state = JobState::Running;
    }
}

/// What a finished job hands back.
#[derive(Debug, Default)]
pub struct JobOutput {
    pub outputs: Vec<PathBuf>,
    pub detail: Value,
}

/// Every app job of this run.
#[derive(Default)]
pub struct Registry {
    next: AtomicU64,
    jobs: Mutex<BTreeMap<u64, Arc<JobEntry>>>,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u64, Arc<JobEntry>>> {
        self.jobs.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn create(&self, kind: &str) -> Arc<JobEntry> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = Arc::new(JobEntry {
            id,
            kind: kind.to_string(),
            cancel: CancelFlag::default(),
            current: AtomicU64::new(0),
            status: Mutex::new(Status {
                state: JobState::Queued,
                progress: 0.0,
                outputs: Vec::new(),
                error: None,
                detail: Value::Null,
                last_emitted: 0.0,
            }),
        });
        self.lock().insert(id, Arc::clone(&entry));
        entry
    }

    pub fn get(&self, id: u64) -> Option<Arc<JobEntry>> {
        self.lock().get(&id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<JobEntry>> {
        self.lock().values().cloned().collect()
    }

    /// Ask every unfinished job to stop. Returns how many were asked.
    pub fn cancel_all(&self, shared: &Shared) -> usize {
        let mut n = 0;
        for entry in self.all().into_iter().filter(|e| !e.is_finished()) {
            entry.request_cancel(shared);
            n += 1;
        }
        n
    }

    /// Drop finished jobs beyond the newest `keep`, so a long session does not grow without bound.
    pub fn trim(&self, keep: usize) {
        let mut jobs = self.lock();
        let finished: Vec<u64> = jobs.values().filter(|e| e.is_finished()).map(|e| e.id).collect();
        if finished.len() > keep {
            for id in &finished[..finished.len() - keep] {
                jobs.remove(id);
            }
        }
    }
}

impl JobEntry {
    /// CMD-JOB-CANCEL: stop before the next worker job, and kill the running one.
    pub fn request_cancel(&self, shared: &Shared) {
        self.cancel.set();
        let current = self.current.load(Ordering::SeqCst);
        if current != 0 {
            shared.pool.cancel(JobId(current));
        }
    }
}

/// The [`JobCtl`] of one app job: cancellation in, progress and events out.
pub struct Ctx {
    pub entry: Arc<JobEntry>,
    pub shared: Arc<Shared>,
    pub emit: Arc<dyn Emit>,
}

impl Ctx {
    pub fn emit_view(&self) {
        self.emit.emit(Event::Job(self.entry.view()));
    }

    /// Record a file the job has finished writing, so Reveal works before the job ends.
    pub fn add_output(&self, path: &Path) {
        self.entry.lock().outputs.push(path.to_path_buf());
    }
}

impl JobCtl for Ctx {
    fn cancelled(&self) -> bool {
        self.entry.cancel.is_set()
    }

    fn progress(&self, fraction: f32) {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let emit = {
            let mut st = self.entry.lock();
            st.progress = fraction;
            // About a hundred updates per job is plenty for a progress bar.
            let due = fraction >= 1.0 || fraction - st.last_emitted >= 0.01;
            if due {
                st.last_emitted = fraction;
            }
            due
        };
        if emit {
            self.emit_view();
        }
    }

    fn running(&self, id: JobId) {
        self.entry.current.store(id.0, Ordering::SeqCst);
        // Cancelled between the check and the queueing: stop it now.
        if self.entry.cancel.is_set() {
            self.shared.pool.cancel(id);
        }
    }
}
