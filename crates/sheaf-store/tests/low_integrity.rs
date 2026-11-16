//! A process that lowers itself to low integrity can write in a session folder and nowhere else the user's
//! normal programs own. Windows only; one test, because lowering is permanent for the process.
#![cfg(windows)]

use sheaf_store::{integrity_level_of, lower_worker, Session};

#[test]
fn a_lowered_process_writes_in_the_session_folder_and_nowhere_else() {
    let base = std::env::temp_dir().join(format!("sheaf-low-{}", std::process::id()));
    std::fs::create_dir_all(&base).expect("base");
    let session = Session::create(&base).expect("session");
    // Somewhere an ordinary program of this user owns.
    let elsewhere = base.join("elsewhere.txt");
    std::fs::write(&elsewhere, b"user's file").expect("a normal file");

    assert_ne!(
        integrity_level_of(std::process::id()),
        Some(0x1000),
        "starts at the user's level"
    );
    lower_worker(session.dir());
    assert_eq!(integrity_level_of(std::process::id()), Some(0x1000), "now low");

    // Reading what the user owns still works (the worker has to read documents)...
    assert_eq!(std::fs::read(&elsewhere).expect("read"), b"user's file");
    // ...writing it does not, neither new files next to it nor changes to it...
    assert!(
        std::fs::write(base.join("new.txt"), b"x").is_err(),
        "no new files outside the session"
    );
    assert!(
        std::fs::write(&elsewhere, b"changed").is_err(),
        "no changes to the user's files"
    );
    // ...while the session folder works, files and sub-folders, including the temporary folder it was pointed at.
    std::fs::write(session.dir().join("results").join("out.bin"), b"ok").expect("write in the session");
    std::fs::create_dir(session.dir().join("results").join("sub")).expect("folders in the session");
    assert_eq!(
        std::env::temp_dir(),
        session.dir(),
        "temporary files go to the session folder"
    );
    std::fs::write(std::env::temp_dir().join("tmp.bin"), b"ok").expect("temporary file");

    // (Cleaning up is the parent's job in the app; here the lowered process just leaves the scratch folder.)
}
