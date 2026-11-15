//! MOD-CLI: the argument grammar (CLI-GRAMMAR). Pure: arguments in, a
//! [`Command`] out. Every argument is untrusted input (SEC-PATHS); paths are
//! checked by the path policy when they are used, not here.
//!
//! ```text
//! sheaf                              the window, empty
//! sheaf open <files...>              the window, files imported
//! sheaf run <action> <files...> [--out DIR] [--level low|recommended|extreme] [--langs auto|eng+rus]
//! sheaf <action> <files...> ...      the same, `run` left out (what an Explorer verb writes)
//! sheaf tray                         the resident helper: tray icon and the wheel hotkey
//! sheaf wheel --at X,Y --files LIST  the wheel overlay (started by the helper)
//! ```
//! Actions: `merge compress ocr split pdf2img img2pdf rotate`.

use sheaf_core::{CompressLevel, QuickAction, QuickRequest};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Window { files: Vec<PathBuf> },
    Run(Run),
    Tray,
    Wheel { at: (i32, i32), list: PathBuf },
}

/// A headless quick action.
pub type Run = QuickRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub const USAGE: &str = "usage: sheaf [open FILES... | run ACTION FILES... | ACTION FILES... | tray]\n\
actions: merge compress ocr split pdf2img img2pdf rotate\n\
run flags: --out DIR  --level low|recommended|extreme  --langs auto|eng+rus";

fn err<T>(msg: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(msg.into()))
}

/// Parse the arguments after the program name.
pub fn parse(args: &[OsString]) -> Result<Command, UsageError> {
    let mut it = args.iter();
    let Some(first) = it.next() else {
        return Ok(Command::Window { files: vec![] });
    };
    let first_str = first.to_str().unwrap_or("");
    match first_str {
        "open" => Ok(Command::Window {
            files: it.map(PathBuf::from).collect(),
        }),
        "tray" => match it.next() {
            None => Ok(Command::Tray),
            Some(_) => err("tray takes no arguments"),
        },
        "wheel" => parse_wheel(it.collect()),
        "run" => match it.next().and_then(|w| w.to_str()).and_then(QuickAction::parse) {
            Some(action) => parse_run(action, it.collect()),
            None => err("run needs an action: merge compress ocr split pdf2img img2pdf rotate"),
        },
        word => match QuickAction::parse(word) {
            Some(action) => parse_run(action, it.collect()),
            // A bare path (double-click through "Open with"): open it.
            None if !word.starts_with("--") => Ok(Command::Window {
                files: args.iter().map(PathBuf::from).collect(),
            }),
            None => err(format!("unknown option {word}")),
        },
    }
}

fn parse_run(action: QuickAction, rest: Vec<&OsString>) -> Result<Command, UsageError> {
    let mut run = Run::new(action, vec![]);
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--out") => {
                run.out = Some(PathBuf::from(
                    it.next().ok_or(UsageError("--out needs a folder".into()))?,
                ))
            }
            Some("--level") => {
                run.level = match it.next().and_then(|v| v.to_str()) {
                    Some("low") => CompressLevel::Low,
                    Some("recommended") => CompressLevel::Recommended,
                    Some("extreme") => CompressLevel::Extreme,
                    _ => return err("--level needs low, recommended or extreme"),
                }
            }
            Some("--langs") => {
                let v = it
                    .next()
                    .and_then(|v| v.to_str())
                    .ok_or(UsageError("--langs needs a value".into()))?;
                run.langs = (v != "auto").then(|| v.split('+').map(str::to_string).collect());
            }
            Some(flag) if flag.starts_with("--") => return err(format!("unknown option {flag}")),
            _ => run.files.push(PathBuf::from(arg)),
        }
    }
    if run.files.is_empty() {
        return err(format!("{} needs at least one file", action.id()));
    }
    Ok(Command::Run(run))
}

fn parse_wheel(rest: Vec<&OsString>) -> Result<Command, UsageError> {
    let (mut at, mut list) = (None, None);
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--at") => {
                let v = it.next().and_then(|v| v.to_str()).unwrap_or("");
                let (x, y) = v.split_once(',').ok_or(UsageError("--at needs X,Y".into()))?;
                let num = |s: &str| {
                    s.trim()
                        .parse::<i32>()
                        .map_err(|_| UsageError("--at needs whole numbers".into()))
                };
                at = Some((num(x)?, num(y)?));
            }
            Some("--files") => list = it.next().map(PathBuf::from),
            _ => return err("wheel takes --at X,Y and --files LIST"),
        }
    }
    match (at, list) {
        (Some(at), Some(list)) => Ok(Command::Wheel { at, list }),
        _ => err("wheel takes --at X,Y and --files LIST"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Command, UsageError> {
        parse(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_open_an_empty_window() {
        assert_eq!(p(&[]), Ok(Command::Window { files: vec![] }));
    }

    #[test]
    fn open_and_bare_paths_open_the_window_with_files() {
        let want = Command::Window {
            files: vec!["a.pdf".into(), "b.png".into()],
        };
        assert_eq!(p(&["open", "a.pdf", "b.png"]), Ok(want.clone()));
        assert_eq!(p(&["a.pdf", "b.png"]), Ok(want), "'Open with' passes the path alone");
    }

    #[test]
    fn actions_run_headless_with_or_without_the_run_word() {
        let want = Command::Run(Run {
            action: QuickAction::Compress,
            files: vec!["a.pdf".into()],
            out: Some("out".into()),
            level: CompressLevel::Extreme,
            langs: None,
        });
        assert_eq!(
            p(&["run", "compress", "a.pdf", "--out", "out", "--level", "extreme"]),
            Ok(want.clone())
        );
        assert_eq!(
            p(&["compress", "a.pdf", "--level", "extreme", "--out", "out"]),
            Ok(want)
        );
        match p(&["ocr", "s.pdf", "--langs", "eng+rus"]) {
            Ok(Command::Run(r)) => assert_eq!(r.langs, Some(vec!["eng".into(), "rus".into()])),
            other => panic!("{other:?}"),
        }
        match p(&["ocr", "s.pdf", "--langs", "auto"]) {
            Ok(Command::Run(r)) => assert_eq!(r.langs, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bad_input_is_a_usage_error_not_a_panic() {
        for bad in [
            &["run"][..],
            &["run", "shred", "a.pdf"],
            &["merge"],
            &["merge", "--out"],
            &["compress", "a.pdf", "--level", "ludicrous"],
            &["compress", "a.pdf", "--frobnicate"],
            &["--help"],
            &["tray", "extra"],
            &["wheel", "--at", "x,y", "--files", "l"],
            &["wheel", "--at", "1,2"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_wheel_overlay_gets_a_position_and_a_list() {
        assert_eq!(
            p(&["wheel", "--at", "-20,300", "--files", "list.txt"]),
            Ok(Command::Wheel {
                at: (-20, 300),
                list: "list.txt".into()
            })
        );
        assert_eq!(p(&["tray"]), Ok(Command::Tray));
    }
}
