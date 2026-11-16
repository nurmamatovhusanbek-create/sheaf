//! Test double that speaks DATA-WORKERPROTO and misbehaves on request.
//! Used by TEST-JOBS only; never bundled with the app.

use serde_json::{json, Value};
use sheaf_core::proto::{Request, Response};
use sheaf_core::{AppError, ErrorCode};
use std::io::{BufRead, Write};
use std::time::Duration;

fn say(resp: &Response) {
    let mut out = std::io::stdout().lock();
    if let Ok(line) = serde_json::to_string(resp) {
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn main() {
    // Started by "spawn_sleeper": a grandchild that outlives its parent unless
    // somebody ends the whole tree (SEC-ISOLATE).
    if std::env::args().nth(1).as_deref() == Some("--sleeper") {
        std::thread::sleep(Duration::from_secs(600));
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            continue;
        };
        let ms = req.args.get("ms").and_then(Value::as_u64).unwrap_or(0);
        match req.op.as_str() {
            "echo" => say(&Response::done(req.id, req.args.clone())),
            "pid" => say(&Response::done(req.id, json!(std::process::id()))),
            "sleep" => {
                for step in 1..=4u64 {
                    std::thread::sleep(Duration::from_millis(ms / 4));
                    say(&Response::progress(req.id, step as f32 / 4.0));
                }
                say(&Response::done(req.id, json!("slept")));
            }
            "fail" => say(&Response::failed(
                req.id,
                AppError::new(ErrorCode::ParseError, "bad file"),
            )),
            "claim_timeout" => say(&Response::failed(
                req.id,
                AppError::new(ErrorCode::Timeout, "not really"),
            )),
            "spawn_sleeper" => {
                match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--sleeper").spawn()) {
                    Ok(child) => say(&Response::done(req.id, json!(child.id()))),
                    Err(_) => say(&Response::failed(req.id, AppError::internal("spawn failed"))),
                }
            }
            "crash" => std::process::exit(3),
            "hang" => loop {
                std::thread::sleep(Duration::from_secs(60));
            },
            "alloc" => {
                let mb = req.args.get("mb").and_then(Value::as_u64).unwrap_or(0) as usize;
                let mut hog = vec![0u8; mb << 20];
                for page in hog.chunks_mut(4096) {
                    page[0] = 1; // touch every page so it counts as resident
                }
                std::thread::sleep(Duration::from_millis(ms));
                say(&Response::done(req.id, json!(hog.len())));
            }
            "garbage" => {
                println!("this is not json");
                let _ = std::io::stdout().flush();
            }
            "flood" => {
                // One line far past the cap, with no newline in sight.
                let chunk = vec![b'x'; 1 << 16];
                let mut out = std::io::stdout().lock();
                for _ in 0..64 {
                    if out.write_all(&chunk).is_err() {
                        break;
                    }
                }
                let _ = out.flush();
                std::thread::sleep(Duration::from_secs(60));
            }
            "stale_then_done" => {
                say(&Response::done(req.id + 1000, json!("someone else's")));
                say(&Response::done(req.id, json!("mine")));
            }
            _ => say(&Response::failed(req.id, AppError::invalid("unknown op"))),
        }
    }
}
