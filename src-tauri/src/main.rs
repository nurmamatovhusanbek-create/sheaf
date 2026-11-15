//! MOD-SHELL: the Tauri 2 shell around MOD-APP (ADR-TAURI).
//!
//! This crate only translates: window and dialog toolkit calls in, `Backend`
//! commands out. Every rule about what the UI may ask lives in `sheaf-app`
//! (SEC-IPC) and in `build.rs` plus `capabilities/main.json` (which commands
//! the UI may call at all).

// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ipc;
mod shell;
mod thumb;

fn main() {
    shell::run();
}
