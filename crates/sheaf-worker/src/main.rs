//! MOD-WORKER binary: see the library for what the worker does.

fn main() -> std::process::ExitCode {
    sheaf_worker::run(std::env::args_os().skip(1))
}
