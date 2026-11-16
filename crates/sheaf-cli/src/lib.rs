//! MOD-CLI: the argument grammar (CLI-GRAMMAR). Pure: arguments in, a
//! [`Command`] out. Every argument is untrusted input (SEC-PATHS); paths are
//! checked by the path policy when they are used, not here.
//!
//! ```text
//! sheaf                              the window, empty
//! sheaf open <files...>              the window, files imported
//! sheaf run <action> <files...> [--out DIR] [--level low|recommended|extreme] [--langs auto|eng+rus]
//! sheaf <action> <files...> ...      the same, `run` left out (what an Explorer verb writes)
//! sheaf preset <name> <files...> [--out DIR]   run a saved preset on files, no window
//! sheaf presets                      list the saved presets, one per line
//! sheaf info <file> [--strip | --set key=value ...] [--out DIR]   show, change or remove a PDF's information
//! sheaf repair <files...> [--out DIR]   rebuild damaged PDFs into NAME.repaired.pdf, never overwriting
//! sheaf install-verbs | uninstall-verbs   add or remove the Explorer right-click menu (current user)
//! --card (with an action or preset): the progress and result show in a small card at the cursor
//! --collect (with open, an action or preset): what an Explorer verb adds. The first process gathers the
//!                                    files of the processes Explorer starts for the same selection and
//!                                    acts once for all of them; the others leave
//! sheaf tray                         the resident helper: tray icon and the wheel hotkey
//! sheaf wheel --at X,Y --files LIST  the wheel overlay (started by the helper)
//! ```
//! Actions: `merge compress ocr split pdf2img img2pdf rotate`.

use sheaf_core::{CompressLevel, QuickAction, QuickRequest};
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Window {
        files: Vec<PathBuf>,
    },
    Run(Run),
    /// A saved preset on files, no window (FLOW-PRESET).
    Preset(PresetRun),
    /// The names of the saved presets.
    Presets,
    /// Rebuild damaged PDFs into new files (TOOL-REPAIR).
    Repair(RepairRun),
    /// Show, change or remove what a PDF says about itself (TOOL-METADATA).
    Info(InfoRun),
    /// Write or remove Explorer's right-click entries (FLOW-EXPLORER).
    InstallVerbs,
    UninstallVerbs,
    Tray,
    Wheel {
        at: (i32, i32),
        list: PathBuf,
    },
    /// The command, to run once for every process Explorer started for one selection (`--collect`).
    Collected(Box<Command>),
    /// An action or preset whose progress and result show in a small card at the cursor instead of
    /// on the console (`--card`, what Explorer verbs use).
    Card(Box<Command>),
}

impl Command {
    /// What kind of work a collected command is, so that only like batches with like.
    pub fn collect_key(&self) -> String {
        match self {
            Command::Window { .. } => "open".into(),
            Command::Run(r) => r.action.id().into(),
            Command::Preset(p) => format!("preset-{}", p.name),
            Command::Card(inner) | Command::Collected(inner) => inner.collect_key(),
            _ => "other".into(),
        }
    }

    /// The files this command works on.
    pub fn files(&self) -> &[PathBuf] {
        match self {
            Command::Window { files } => files,
            Command::Run(r) => &r.files,
            Command::Preset(p) => &p.files,
            Command::Card(inner) | Command::Collected(inner) => inner.files(),
            _ => &[],
        }
    }

    /// The arguments that make [`parse`] return this command again, for starting the program a
    /// second time (the renderer fallback). Only the commands that open a window have any;
    /// `--collect` is left out because by then the files have been collected.
    pub fn to_args(&self) -> Option<Vec<OsString>> {
        let os = |s: &str| OsString::from(s);
        let files = |v: &[PathBuf]| v.iter().map(|f| f.as_os_str().to_owned()).collect::<Vec<_>>();
        match self {
            Command::Window { files: f } => Some([vec![os("open")], files(f)].concat()),
            Command::Run(r) => {
                let mut a = vec![os(r.action.id())];
                a.extend(files(&r.files));
                if let Some(out) = &r.out {
                    a.extend([os("--out"), out.as_os_str().to_owned()]);
                }
                a.extend([
                    os("--level"),
                    os(match r.level {
                        CompressLevel::Low => "low",
                        CompressLevel::Recommended => "recommended",
                        CompressLevel::Extreme => "extreme",
                    }),
                ]);
                if let Some(langs) = &r.langs {
                    a.extend([os("--langs"), os(&langs.join("+"))]);
                }
                Some(a)
            }
            Command::Preset(p) => {
                let mut a = vec![os("preset"), os(&p.name)];
                a.extend(files(&p.files));
                if let Some(out) = &p.out {
                    a.extend([os("--out"), out.as_os_str().to_owned()]);
                }
                Some(a)
            }
            Command::Card(inner) => {
                let mut a = inner.to_args()?;
                a.push(os("--card"));
                Some(a)
            }
            _ => None,
        }
    }

    /// The same command on other files.
    pub fn with_files(mut self, new: Vec<PathBuf>) -> Command {
        match &mut self {
            Command::Window { files } => *files = new,
            Command::Run(r) => r.files = new,
            Command::Preset(p) => p.files = new,
            Command::Card(inner) | Command::Collected(inner) => {
                let moved = std::mem::replace(&mut **inner, Command::Tray);
                **inner = moved.with_files(new);
            }
            _ => {}
        }
        self
    }
}

/// A headless quick action.
pub type Run = QuickRequest;

/// Damaged PDFs to rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairRun {
    pub files: Vec<PathBuf>,
    pub out: Option<PathBuf>,
}

/// The fields `sheaf info --set` knows.
pub const INFO_KEYS: [&str; 6] = ["title", "author", "subject", "keywords", "creator", "producer"];

/// One PDF whose information is shown, changed or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoRun {
    pub file: PathBuf,
    /// Write a copy with all of it removed.
    pub strip: bool,
    /// Write a copy with these fields changed (the others stay as they are).
    pub set: Vec<(String, String)>,
    pub out: Option<PathBuf>,
}

/// A saved preset applied to files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetRun {
    pub name: String,
    pub files: Vec<PathBuf>,
    /// Replaces the preset's own output folder.
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub const USAGE: &str =
    "usage: sheaf [open FILES... | run ACTION FILES... | ACTION FILES... | preset NAME FILES... | presets | tray]\n\
actions: merge compress ocr split pdf2img img2pdf rotate\n\
run flags: --out DIR  --level low|recommended|extreme  --langs auto|eng+rus  --card  --collect (Explorer verbs)";

fn err<T>(msg: impl Into<String>) -> Result<T, UsageError> {
    Err(UsageError(msg.into()))
}

/// Parse the arguments after the program name.
pub fn parse(args: &[OsString]) -> Result<Command, UsageError> {
    let (collect, card) = (
        args.iter().any(|a| a == "--collect"),
        args.iter().any(|a| a == "--card"),
    );
    if !collect && !card {
        return parse_one(args);
    }
    let rest: Vec<OsString> = args
        .iter()
        .filter(|a| *a != "--collect" && *a != "--card")
        .cloned()
        .collect();
    let mut command = parse_one(&rest)?;
    if card {
        command = match command {
            c @ (Command::Run(_) | Command::Preset(_)) => Command::Card(Box::new(c)),
            _ => return err("--card goes with an action or a preset"),
        };
    }
    if collect {
        command = match command {
            c @ (Command::Window { .. } | Command::Run(_) | Command::Preset(_) | Command::Card(_)) => {
                Command::Collected(Box::new(c))
            }
            _ => return err("--collect goes with open, an action or a preset"),
        };
    }
    Ok(command)
}

fn parse_one(args: &[OsString]) -> Result<Command, UsageError> {
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
        "presets" => match it.next() {
            None => Ok(Command::Presets),
            Some(_) => err("presets takes no arguments"),
        },
        "preset" => parse_preset(it.collect()),
        "repair" => parse_repair(it.collect()),
        "info" => parse_info(it.collect()),
        "install-verbs" | "uninstall-verbs" => match it.next() {
            None if first_str == "install-verbs" => Ok(Command::InstallVerbs),
            None => Ok(Command::UninstallVerbs),
            Some(_) => err(format!("{first_str} takes no arguments")),
        },
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

fn parse_preset(rest: Vec<&OsString>) -> Result<Command, UsageError> {
    let mut it = rest.into_iter();
    let name = it
        .next()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty() && !n.starts_with("--"))
        .ok_or(UsageError("preset needs a preset name".into()))?
        .to_string();
    let (mut files, mut out) = (Vec::new(), None);
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--out") => {
                out = Some(PathBuf::from(
                    it.next().ok_or(UsageError("--out needs a folder".into()))?,
                ))
            }
            Some(flag) if flag.starts_with("--") => return err(format!("unknown option {flag}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if files.is_empty() {
        return err("preset needs at least one file");
    }
    Ok(Command::Preset(PresetRun { name, files, out }))
}

fn parse_repair(rest: Vec<&OsString>) -> Result<Command, UsageError> {
    let (mut files, mut out) = (Vec::new(), None);
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--out") => {
                out = Some(PathBuf::from(
                    it.next().ok_or(UsageError("--out needs a folder".into()))?,
                ))
            }
            Some(flag) if flag.starts_with("--") => return err(format!("unknown option {flag}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if files.is_empty() {
        return err("repair needs at least one file");
    }
    Ok(Command::Repair(RepairRun { files, out }))
}

fn parse_info(rest: Vec<&OsString>) -> Result<Command, UsageError> {
    let mut run = InfoRun {
        file: PathBuf::new(),
        strip: false,
        set: Vec::new(),
        out: None,
    };
    let mut have_file = false;
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--strip") => run.strip = true,
            Some("--out") => {
                run.out = Some(PathBuf::from(
                    it.next().ok_or(UsageError("--out needs a folder".into()))?,
                ))
            }
            Some("--set") => {
                let pair = it.next().and_then(|v| v.to_str()).unwrap_or("");
                let (key, value) = pair.split_once('=').ok_or(UsageError("--set needs key=value".into()))?;
                if !INFO_KEYS.contains(&key) {
                    return err(format!("--set knows {}", INFO_KEYS.join(", ")));
                }
                run.set.push((key.to_string(), value.to_string()));
            }
            Some(flag) if flag.starts_with("--") => return err(format!("unknown option {flag}")),
            _ if !have_file => {
                run.file = PathBuf::from(arg);
                have_file = true;
            }
            _ => return err("info takes one file"),
        }
    }
    if !have_file {
        return err("info needs a file");
    }
    if run.strip && !run.set.is_empty() {
        return err("--strip and --set cannot be used together");
    }
    Ok(Command::Info(run))
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
    fn presets_are_listed_and_run_on_files() {
        assert_eq!(p(&["presets"]), Ok(Command::Presets));
        assert_eq!(
            p(&["preset", "Contract scan", "a.pdf", "b.pdf", "--out", "out"]),
            Ok(Command::Preset(PresetRun {
                name: "Contract scan".into(),
                files: vec!["a.pdf".into(), "b.pdf".into()],
                out: Some("out".into()),
            }))
        );
        for bad in [
            &["preset"][..],
            &["preset", "Name"],
            &["preset", "--out", "x", "a.pdf"],
            &["preset", "Name", "a.pdf", "--frobnicate"],
            &["preset", "Name", "a.pdf", "--out"],
            &["presets", "extra"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn collect_wraps_the_command_and_names_the_kind_of_work() {
        let c = p(&["merge", "--collect", "a.pdf"]).expect("parses");
        let Command::Collected(inner) = c else { panic!("{c:?}") };
        assert_eq!(inner.collect_key(), "merge");
        assert_eq!(inner.files(), [PathBuf::from("a.pdf")]);
        let both = inner.with_files(vec!["a.pdf".into(), "b.pdf".into()]);
        assert_eq!(both.files().len(), 2);
        match p(&["preset", "Contract scan", "x.pdf", "--collect"]) {
            Ok(Command::Collected(i)) => assert_eq!(i.collect_key(), "preset-Contract scan"),
            other => panic!("{other:?}"),
        }
        match p(&["open", "--collect", "x.pdf"]) {
            Ok(Command::Collected(i)) => assert_eq!(i.collect_key(), "open"),
            other => panic!("{other:?}"),
        }
        // A verb is both: gather the selection, then show a card.
        match p(&["compress", "--collect", "--card", "a.pdf"]) {
            Ok(Command::Collected(i)) => match *i {
                Command::Card(run) => assert!(matches!(*run, Command::Run(_))),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        let both = p(&["preset", "X", "--card", "--collect", "a.pdf"]).expect("parses");
        assert_eq!(both.files(), [PathBuf::from("a.pdf")]);
        let more = both.with_files(vec!["a.pdf".into(), "b.pdf".into()]);
        assert_eq!(
            more.files().len(),
            2,
            "the files reach the command inside both wrappers"
        );
        assert_eq!(p(&["install-verbs"]), Ok(Command::InstallVerbs));
        assert_eq!(p(&["uninstall-verbs"]), Ok(Command::UninstallVerbs));
        assert!(p(&["install-verbs", "x"]).is_err() && p(&["install-verbs", "--collect"]).is_err());
        assert!(p(&["tray", "--card"]).is_err() && p(&["open", "--card"]).is_err());
        assert!(p(&["tray", "--collect"]).is_err());
        assert!(p(&["presets", "--collect"]).is_err());
        assert!(
            p(&["--collect"]).is_ok_and(|c| matches!(c, Command::Collected(_))),
            "an empty window, once"
        );
        // Without the flag nothing changes.
        assert!(matches!(p(&["merge", "a.pdf"]), Ok(Command::Run(_))));
    }

    #[test]
    fn repair_takes_files_and_an_optional_folder() {
        assert_eq!(
            p(&["repair", "a.pdf", "b.pdf", "--out", "fixed"]),
            Ok(Command::Repair(RepairRun {
                files: vec!["a.pdf".into(), "b.pdf".into()],
                out: Some("fixed".into()),
            }))
        );
        for bad in [
            &["repair"][..],
            &["repair", "--out", "x"],
            &["repair", "a.pdf", "--frobnicate"],
            &["repair", "a.pdf", "--out"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn info_shows_by_default_and_changes_a_copy_with_strip_or_set() {
        let show = p(&["info", "a.pdf"]).expect("parses");
        assert_eq!(
            show,
            Command::Info(InfoRun {
                file: "a.pdf".into(),
                strip: false,
                set: vec![],
                out: None
            })
        );
        match p(&[
            "info",
            "a.pdf",
            "--set",
            "title=My file",
            "--set",
            "author=A=B",
            "--out",
            "o",
        ]) {
            Ok(Command::Info(r)) => {
                assert_eq!(
                    r.set,
                    [
                        ("title".to_string(), "My file".to_string()),
                        ("author".to_string(), "A=B".to_string())
                    ]
                );
                assert_eq!(r.out, Some("o".into()));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(p(&["info", "a.pdf", "--strip"]), Ok(Command::Info(r)) if r.strip));
        for bad in [
            &["info"][..],
            &["info", "a", "b"],
            &["info", "a", "--strip", "--set", "title=x"],
            &["info", "a", "--set", "colour=red"],
            &["info", "a", "--set", "novalue"],
            &["info", "a", "--set"],
            &["info", "a", "--frobnicate"],
        ] {
            assert!(p(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_command_can_be_written_back_as_arguments_that_parse_to_the_same_command() {
        for args in [
            &["open", "a.pdf", "b.png"][..],
            &["compress", "a.pdf", "--out", "o", "--level", "extreme"],
            &["ocr", "s.pdf", "--langs", "eng+rus", "--level", "low"],
            &["preset", "Contract scan", "a.pdf", "--out", "o"],
            &["merge", "a.pdf", "b.pdf", "--card"],
            &["preset", "X", "a.pdf", "--card"],
        ] {
            let command = p(args).expect("parses");
            let back = command.to_args().expect("has arguments");
            assert_eq!(parse(&back), Ok(command.clone()), "{args:?} -> {back:?}");
        }
        assert!(Command::Tray.to_args().is_none() && Command::Presets.to_args().is_none());
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
