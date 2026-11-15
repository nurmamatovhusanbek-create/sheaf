//! `sheaf.exe`: the Windows GUI program (no console window). See the library for the modes.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    sheaf_shell::run()
}
