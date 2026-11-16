//! `sheaf-harness`: the development build of the command line (see `harness::cli`).

fn main() -> std::process::ExitCode {
    harness::cli::run(harness::Engines::vendored)
}
