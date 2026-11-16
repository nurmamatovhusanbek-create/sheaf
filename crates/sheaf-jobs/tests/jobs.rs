//! TEST-JOBS: worker crash, hang and memory-limit recovery; cancellation;
//! priority ordering. Runs against the fake worker.

use serde_json::{json, Value};
use sheaf_core::{ErrorCode, JobId, Limits, Priority};
use sheaf_jobs::{JobEvent, JobSpec, Pool, PoolConfig};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

fn pool(workers: usize) -> (Pool, Receiver<JobEvent>) {
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let cfg = PoolConfig {
        program: env!("CARGO_BIN_EXE_sheaf-fake-worker").into(),
        args: vec![],
        workers,
    };
    let pool = Pool::new(cfg, move |e| {
        let _ = tx.lock().expect("event sender").send(e);
    });
    (pool, rx)
}

fn job(op: &str, args: Value) -> JobSpec {
    JobSpec::new(op, args, Priority::Background)
}

fn with_limits(op: &str, args: Value, timeout_ms: u64, rss_mb: u64) -> JobSpec {
    JobSpec {
        limits: Limits { timeout_ms, rss_mb },
        ..job(op, args)
    }
}

/// The pool still works: a fresh worker answers.
fn assert_alive(pool: &Pool) -> u64 {
    pool.run(job("pid", json!({})))
        .expect("pool recovered")
        .as_u64()
        .expect("pid")
}

fn wait_for(rx: &Receiver<JobEvent>, mut pred: impl FnMut(&JobEvent) -> bool) -> JobEvent {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = rx.recv_timeout(left).expect("expected event did not arrive");
        if pred(&event) {
            return event;
        }
    }
}

#[test]
fn jobs_run_and_report_progress() {
    let (pool, rx) = pool(2);
    assert_eq!(
        pool.run(job("echo", json!({"a": [1, 2]}))).expect("echo"),
        json!({"a": [1, 2]})
    );

    let id = pool.submit(job("sleep", json!({"ms": 40})));
    let mut progress = Vec::new();
    let mut started = false;
    let done = wait_for(&rx, |e| match e {
        JobEvent::Started { id: i } if *i == id => {
            started = true;
            false
        }
        JobEvent::Progress { id: i, progress: p } if *i == id => {
            progress.push(*p);
            false
        }
        JobEvent::Done { id: i, .. } => *i == id,
        _ => false,
    });
    assert!(started);
    assert_eq!(progress, [0.25, 0.5, 0.75, 1.0]);
    assert_eq!(
        done,
        JobEvent::Done {
            id,
            result: json!("slept")
        }
    );
}

#[test]
fn worker_errors_pass_through_and_the_worker_is_kept() {
    let (pool, _rx) = pool(1);
    let before = assert_alive(&pool);
    assert_eq!(
        pool.run(job("fail", json!({}))).expect_err("fail").code,
        ErrorCode::ParseError
    );
    assert_eq!(
        pool.run(job("nope", json!({}))).expect_err("unknown op").code,
        ErrorCode::InvalidArg
    );
    assert_eq!(
        assert_alive(&pool),
        before,
        "an ordinary failure does not cost a process"
    );
    // A worker cannot make the pool believe it timed out or was cancelled.
    assert_eq!(
        pool.run(job("claim_timeout", json!({}))).expect_err("claim").code,
        ErrorCode::Internal
    );
    // Lines for other jobs are ignored rather than misattributed.
    assert_eq!(
        pool.run(job("stale_then_done", json!({}))).expect("stale"),
        json!("mine")
    );
}

#[test]
fn a_crashed_worker_is_replaced() {
    let (pool, _rx) = pool(1);
    let before = assert_alive(&pool);
    assert_eq!(
        pool.run(job("crash", json!({}))).expect_err("crash").code,
        ErrorCode::WorkerCrashed
    );
    let after = assert_alive(&pool);
    assert_ne!(before, after, "a new process took over");
    assert_eq!(
        pool.run(job("garbage", json!({}))).expect_err("garbage").code,
        ErrorCode::WorkerCrashed
    );
    assert_eq!(
        pool.run(job("flood", json!({}))).expect_err("flood").code,
        ErrorCode::WorkerCrashed
    );
    assert_ne!(assert_alive(&pool), after);
}

#[test]
fn a_hung_worker_is_killed_at_the_timeout() {
    let (pool, _rx) = pool(1);
    let before = assert_alive(&pool);
    let t = Instant::now();
    assert_eq!(
        pool.run(with_limits("hang", json!({}), 200, 4096))
            .expect_err("hang")
            .code,
        ErrorCode::Timeout
    );
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "killed promptly, took {:?}",
        t.elapsed()
    );
    assert_ne!(assert_alive(&pool), before);
}

#[cfg(target_os = "linux")]
#[test]
fn a_worker_over_its_memory_limit_is_killed() {
    let (pool, _rx) = pool(1);
    let before = assert_alive(&pool);
    let err = pool
        .run(with_limits("alloc", json!({"mb": 300, "ms": 5000}), 20_000, 100))
        .expect_err("alloc");
    assert_eq!(err.code, ErrorCode::LimitExceeded);
    assert_ne!(assert_alive(&pool), before);
    // Under the cap the same op is fine.
    pool.run(with_limits("alloc", json!({"mb": 20, "ms": 0}), 20_000, 400))
        .expect("small allocation");
}

#[test]
fn running_and_queued_jobs_can_be_cancelled() {
    let (pool, rx) = pool(1);
    let running = pool.submit(job("hang", json!({})));
    wait_for(&rx, |e| *e == JobEvent::Started { id: running });
    let queued = pool.submit(job("echo", json!(1)));
    let survivor = pool.submit(job("echo", json!(2)));

    pool.cancel(queued);
    wait_for(&rx, |e| *e == JobEvent::Cancelled { id: queued });
    let t = Instant::now();
    pool.cancel(running);
    wait_for(&rx, |e| *e == JobEvent::Cancelled { id: running });
    assert!(t.elapsed() < Duration::from_secs(5));

    // The cancelled queued job never ran; the one behind it did.
    let mut seen_queued_start = false;
    wait_for(&rx, |e| {
        seen_queued_start |= *e == JobEvent::Started { id: queued };
        *e == JobEvent::Done {
            id: survivor,
            result: json!(2),
        }
    });
    assert!(!seen_queued_start);
    pool.cancel(JobId(9_999)); // unknown ids are ignored
    pool.cancel(survivor); // so are finished ones
    assert_alive(&pool);
}

#[test]
fn higher_priority_jobs_start_first() {
    let (pool, rx) = pool(1);
    let blocker = pool.submit(job("sleep", json!({"ms": 200})));
    wait_for(&rx, |e| *e == JobEvent::Started { id: blocker });
    let submit = |p| pool.submit(JobSpec::new("echo", json!(null), p));
    let background = submit(Priority::Background);
    let background2 = submit(Priority::Background);
    let scan = submit(Priority::Scan);
    let visible = submit(Priority::Visible);
    let selection = submit(Priority::Selection);
    let visible2 = submit(Priority::Visible);

    let mut order = Vec::new();
    while order.len() < 6 {
        if let JobEvent::Started { id } = wait_for(&rx, |e| matches!(e, JobEvent::Started { .. })) {
            order.push(id);
        }
    }
    assert_eq!(order, [visible, visible2, selection, scan, background, background2]);
}

#[test]
fn work_is_spread_over_several_workers() {
    let (pool, rx) = pool(3);
    let t = Instant::now();
    let ids: Vec<JobId> = (0..3).map(|_| pool.submit(job("sleep", json!({"ms": 600})))).collect();
    let mut done = 0;
    while done < ids.len() {
        if matches!(
            wait_for(&rx, |e| matches!(e, JobEvent::Done { .. })),
            JobEvent::Done { .. }
        ) {
            done += 1;
        }
    }
    assert!(
        t.elapsed() < Duration::from_millis(1500),
        "ran in parallel, took {:?}",
        t.elapsed()
    );
}

#[test]
fn shutdown_cancels_everything_and_kills_workers() {
    let (mut pool, rx) = pool(1);
    let running = pool.submit(job("hang", json!({})));
    wait_for(&rx, |e| *e == JobEvent::Started { id: running });
    let queued = pool.submit(job("echo", json!(1)));
    let t = Instant::now();
    pool.shutdown();
    assert!(t.elapsed() < Duration::from_secs(5));
    let events: Vec<JobEvent> = rx.try_iter().collect();
    assert!(events.contains(&JobEvent::Cancelled { id: running }));
    assert!(events.contains(&JobEvent::Cancelled { id: queued }));
    assert_eq!(
        pool.run(job("echo", json!(1))).expect_err("after shutdown").code,
        ErrorCode::Cancelled
    );
}

#[test]
fn a_missing_worker_binary_fails_the_job_not_the_app() {
    let (tx, rx) = channel();
    let tx = std::sync::Mutex::new(tx);
    let cfg = PoolConfig {
        program: "/no/such/sheaf-worker".into(),
        args: vec![],
        workers: 1,
    };
    let pool = Pool::new(cfg, move |e| {
        let _ = tx.lock().expect("event sender").send(e);
    });
    assert_eq!(
        pool.run(job("echo", json!(1))).expect_err("no binary").code,
        ErrorCode::EngineMissing
    );
    assert_eq!(
        pool.run(job("echo", json!(1))).expect_err("still none").code,
        ErrorCode::EngineMissing
    );
    drop(rx);
}
