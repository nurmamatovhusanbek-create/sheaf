//! MOD-JOBS: scheduler, worker pool, limits, protocol.
//!
//! Jobs run in separate worker processes (ADR-WORKER-PROCS, SEC-ISOLATE). A
//! worker that crashes, hangs, outgrows its memory cap or breaks the protocol
//! is killed and replaced; the job fails and the app carries on (FLOW-ERROR).
//! See `ARCHITECTURE.md`, anchor `mod-jobs`.
//!
//! Memory caps are enforced by polling the worker's resident size, which is
//! implemented for Linux (`/proc`) and Windows (working set). On Windows each
//! worker also sits in a Job Object (hard cap, kill-on-close, see `winjob`).

mod child;
#[cfg(windows)]
mod winjob;

use child::{Line, WorkerProc};
use serde_json::Value;
use sheaf_core::proto::{Outcome, Request, Response};
use sheaf_core::{AppError, ErrorCode, JobId, Limits, Priority, Result};
use std::collections::{BinaryHeap, HashMap};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often a running job is checked for cancellation, timeout and memory.
const TICK: Duration = Duration::from_millis(25);

#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// The worker executable (`sheaf-worker`).
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Number of worker processes; they are started lazily (FLOW-LIFECYCLE).
    pub workers: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JobSpec {
    pub op: String,
    pub args: Value,
    pub priority: Priority,
    pub limits: Limits,
}

impl JobSpec {
    pub fn new(op: &str, args: Value, priority: Priority) -> Self {
        Self {
            op: op.to_string(),
            args,
            priority,
            limits: Limits::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Started { id: JobId },
    Progress { id: JobId, progress: f32 },
    Done { id: JobId, result: Value },
    Failed { id: JobId, error: AppError },
    Cancelled { id: JobId },
}

struct Queued {
    priority: Priority,
    seq: u64,
    id: JobId,
    spec: JobSpec,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    /// Highest priority first; within a priority, first come first served.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority.cmp(&other.priority).then(other.seq.cmp(&self.seq))
    }
}

#[derive(Default)]
struct State {
    queue: BinaryHeap<Queued>,
    running: HashMap<JobId, Arc<AtomicBool>>,
    waiters: HashMap<JobId, mpsc::Sender<Result<Value>>>,
    next: u64,
    shutdown: bool,
}

type Callback = Box<dyn Fn(JobEvent) + Send + Sync>;

struct Inner {
    cfg: PoolConfig,
    state: Mutex<State>,
    wake: Condvar,
    on_event: Callback,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking callback must not take the whole pool down with it.
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Report the end of a job to the callback and to whoever waits in `run`.
    fn finish(&self, id: JobId, outcome: std::result::Result<Value, AppError>) {
        let waiter = {
            let mut st = self.lock();
            st.running.remove(&id);
            st.waiters.remove(&id)
        };
        if let Some(tx) = waiter {
            let _ = tx.send(outcome.clone());
        }
        (self.on_event)(match outcome {
            Ok(result) => JobEvent::Done { id, result },
            Err(error) if error.code == ErrorCode::Cancelled => JobEvent::Cancelled { id },
            Err(error) => JobEvent::Failed { id, error },
        });
    }
}

/// Priority scheduler over a pool of worker processes.
pub struct Pool {
    inner: Arc<Inner>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    /// `on_event` is called from pool threads; keep it short.
    pub fn new(cfg: PoolConfig, on_event: impl Fn(JobEvent) + Send + Sync + 'static) -> Pool {
        let slots = cfg.workers.max(1);
        let inner = Arc::new(Inner {
            cfg,
            state: Mutex::default(),
            wake: Condvar::new(),
            on_event: Box::new(on_event),
        });
        let threads = (0..slots)
            .map(|_| {
                let inner = Arc::clone(&inner);
                std::thread::spawn(move || slot_loop(&inner))
            })
            .collect();
        Pool { inner, threads }
    }

    pub fn submit(&self, spec: JobSpec) -> JobId {
        self.enqueue(spec, None)
    }

    /// Submit and block until the job ends.
    pub fn run(&self, spec: JobSpec) -> Result<Value> {
        self.run_notify(spec, |_| {})
    }

    /// Like [`Pool::run`], and tells the caller the job's id as soon as it is
    /// queued, so another thread can [`Pool::cancel`] it.
    pub fn run_notify(&self, spec: JobSpec, queued: impl FnOnce(JobId)) -> Result<Value> {
        let (tx, rx) = mpsc::channel();
        let id = self.enqueue(spec, Some(tx));
        queued(id);
        rx.recv()
            .unwrap_or_else(|_| Err(AppError::new(ErrorCode::Cancelled, "pool shut down")))
    }

    fn enqueue(&self, spec: JobSpec, waiter: Option<mpsc::Sender<Result<Value>>>) -> JobId {
        let mut st = self.inner.lock();
        st.next += 1;
        let id = JobId(st.next);
        if st.shutdown {
            drop(st);
            if let Some(tx) = waiter {
                let _ = tx.send(Err(AppError::new(ErrorCode::Cancelled, "pool shut down")));
            }
            (self.inner.on_event)(JobEvent::Cancelled { id });
            return id;
        }
        if let Some(tx) = waiter {
            st.waiters.insert(id, tx);
        }
        let seq = st.next;
        st.queue.push(Queued {
            priority: spec.priority,
            seq,
            id,
            spec,
        });
        drop(st);
        self.inner.wake.notify_one();
        id
    }

    /// Cancel a queued or running job. A running job's worker is killed: the
    /// only way to stop third-party code promptly. Unknown ids are ignored.
    pub fn cancel(&self, id: JobId) {
        let mut st = self.inner.lock();
        if let Some(flag) = st.running.get(&id) {
            flag.store(true, Ordering::SeqCst);
            return;
        }
        let before = st.queue.len();
        let kept: Vec<Queued> = std::mem::take(&mut st.queue)
            .into_iter()
            .filter(|q| q.id != id)
            .collect();
        st.queue = kept.into();
        let was_queued = st.queue.len() != before;
        drop(st);
        if was_queued {
            self.inner
                .finish(id, Err(AppError::new(ErrorCode::Cancelled, "cancelled")));
        }
    }

    /// Cancel everything, kill the workers and wait for the pool threads.
    pub fn shutdown(&mut self) {
        let queued: Vec<JobId> = {
            let mut st = self.inner.lock();
            st.shutdown = true;
            for flag in st.running.values() {
                flag.store(true, Ordering::SeqCst);
            }
            std::mem::take(&mut st.queue).into_iter().map(|q| q.id).collect()
        };
        self.inner.wake.notify_all();
        for id in queued {
            self.inner
                .finish(id, Err(AppError::new(ErrorCode::Cancelled, "pool shut down")));
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One slot: owns at most one worker process and runs one job at a time.
fn slot_loop(inner: &Inner) {
    let mut worker: Option<WorkerProc> = None;
    loop {
        let (job, cancel) = {
            let mut st = inner.lock();
            loop {
                if st.shutdown {
                    return; // dropping `worker` kills the process
                }
                if let Some(job) = st.queue.pop() {
                    let cancel = Arc::new(AtomicBool::new(false));
                    st.running.insert(job.id, Arc::clone(&cancel));
                    break (job, cancel);
                }
                st = inner.wake.wait(st).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        (inner.on_event)(JobEvent::Started { id: job.id });
        let outcome = run_job(inner, &mut worker, &job, &cancel);
        if outcome.as_ref().is_err_and(|e| kills_worker(e.code)) {
            worker = None;
        }
        inner.finish(job.id, outcome);
    }
}

/// After these the worker is in an unknown state and is replaced.
fn kills_worker(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::WorkerCrashed | ErrorCode::Timeout | ErrorCode::Cancelled | ErrorCode::LimitExceeded
    )
}

fn run_job(inner: &Inner, worker: &mut Option<WorkerProc>, job: &Queued, cancel: &AtomicBool) -> Result<Value> {
    let request = Request {
        id: job.id.0,
        op: job.spec.op.clone(),
        args: job.spec.args.clone(),
    };
    let line = serde_json::to_string(&request).map_err(|e| AppError::internal(format!("encode request: {e}")))?;

    // A worker may have died while idle; one fresh process gets a second try.
    let mut sent = false;
    for _ in 0..2 {
        if worker.is_none() {
            *worker = Some(WorkerProc::spawn(&inner.cfg)?);
        }
        if worker.as_mut().is_some_and(|w| w.send(&line).is_ok()) {
            sent = true;
            break;
        }
        *worker = None;
    }
    let proc = match worker.as_mut() {
        Some(p) if sent => p,
        _ => {
            return Err(AppError::new(
                ErrorCode::WorkerCrashed,
                "worker did not accept the request",
            ))
        }
    };

    let deadline = Instant::now() + Duration::from_millis(job.spec.limits.timeout_ms);
    let mut ticks = 0u32;
    loop {
        match proc.recv(TICK) {
            Line::Text(text) => match serde_json::from_str::<Response>(&text) {
                Ok(resp) if resp.id == job.id.0 => match resp.outcome() {
                    Outcome::Progress(progress) => (inner.on_event)(JobEvent::Progress { id: job.id, progress }),
                    Outcome::Done(value) => return Ok(value),
                    // Whatever the worker claims, these codes are the pool's to give.
                    Outcome::Failed(e) if kills_worker(e.code) => return Err(AppError::internal(e.msg)),
                    Outcome::Failed(e) => return Err(e),
                },
                Ok(_) => {} // stale line from an earlier job
                Err(_) => return Err(AppError::new(ErrorCode::WorkerCrashed, "worker broke the protocol")),
            },
            Line::Closed => return Err(AppError::new(ErrorCode::WorkerCrashed, "worker exited during the job")),
            Line::Nothing => {}
        }
        if cancel.load(Ordering::SeqCst) {
            return Err(AppError::new(ErrorCode::Cancelled, "cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(AppError::new(ErrorCode::Timeout, "job exceeded its time limit"));
        }
        ticks += 1;
        if ticks.is_multiple_of(4) && proc.rss_mb().is_some_and(|mb| mb > job.spec.limits.rss_mb) {
            return Err(AppError::limit("worker exceeded its memory limit"));
        }
    }
}
