//! UI-UPDATE against the real GitHub release page. Needs the network and a published release, so it is
//! ignored by default: `cargo test -p sheaf-app --test update_live -- --ignored --nocapture`.

use sheaf_app::update::{apply, check, side_file, Check, Curl, Version};

#[test]
#[ignore = "needs the network and a published release"]
fn the_real_release_is_found_downloaded_checked_and_put_in_place() {
    // Pretend to be an older program, so the newest release is an update.
    let found = check(&Curl, "0.0.1").expect("the release page answers");
    let Check::Available(release) = found else {
        panic!("expected an update from 0.0.1, got {found:?}")
    };
    eprintln!(
        "found {} ({} bytes, sha256 {})",
        release.tag, release.asset_size, release.sha256
    );
    assert!(release.version >= Version(0, 1, 0));
    assert!(release
        .asset_url
        .starts_with("https://github.com/nurmamatovhusanbek-create/sheaf/releases/download/"));
    assert!(release.asset_size > 10_000_000, "a whole program, not a stub");

    // The newest release is not an update for itself.
    let same = check(&Curl, &release.version.to_string()).expect("check again");
    assert!(matches!(same, Check::UpToDate(_)), "{same:?}");

    // Download it over HTTPS (through GitHub's redirect), check it, and swap it in for a stand-in program.
    let dir = std::env::temp_dir().join(format!("sheaf-update-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let exe = dir.join("sheaf.exe");
    std::fs::write(&exe, b"the old program").expect("old");
    apply(&Curl, &release, &exe).expect("downloaded, verified and swapped in");

    let got = sheaf_store::hash_file(&exe).expect("hash");
    assert_eq!(
        sheaf_store::hex(&got),
        release.sha256,
        "the new program is exactly the released file"
    );
    assert_eq!(
        std::fs::read(side_file(&exe, "old")).expect("old kept"),
        b"the old program"
    );
    assert!(
        std::fs::read(&exe).expect("new").starts_with(b"MZ"),
        "a Windows program"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
