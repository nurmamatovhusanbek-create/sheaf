//! `sheafc.exe`: the same program for the console, for scripts and Explorer verbs.

fn main() -> std::process::ExitCode {
    sheaf_shell::run()
}
