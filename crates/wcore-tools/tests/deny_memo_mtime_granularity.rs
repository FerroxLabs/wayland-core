//! FerroxLabs/wayland#1145 — the deny-walk memo takes its staleness slack from
//! the granularity of the stamp it is actually holding.
//!
//! `deny_cache_hit` decides a memoised deny set is still current by re-stat-ing
//! every stamped directory and requiring an unchanged mtime that is further
//! behind the walk's own start instant than one filesystem timestamp tick. That
//! tick is not a constant: ext4, xfs, btrfs, tmpfs and APFS stamp sub-second
//! mtimes, while HFS+, FAT/exFAT and older NFS servers resolve whole seconds
//! only. On a whole-second filesystem a write made a fraction of a second after
//! the walk lands in the SAME tick and leaves the directory mtime byte-identical
//! — exactly the #1145 leak — so a stamp from such a filesystem has to earn a
//! far longer quiescence before it may be trusted.
//!
//! The build hosts all use sub-second filesystems, so the whole-second branch is
//! unreachable here without forcing the stamp. `filetime` forces it.

use std::time::{Duration, SystemTime};

use filetime::FileTime;
use wcore_tools::workspace_policy::{WorkspacePolicy, walk_calls};

fn fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("main.rs"), b"fn main() {}").unwrap();
    std::fs::write(root.join(".env"), b"TOKEN=redacted\n").unwrap();
}

#[test]
fn a_whole_second_mtime_earns_a_whole_second_of_quiescence() {
    // POSITIVE CONTROL: an ordinary sub-second stamp. 60 ms of quiescence is
    // past the sub-second tick, so the second exec is served from the memo —
    // without this the assertion below could pass against a memo that never
    // hits at all.
    let quiet = tempfile::tempdir().unwrap();
    fixture(quiet.path());
    let policy = WorkspacePolicy::contained(quiet.path());
    std::thread::sleep(Duration::from_millis(60));

    let before = walk_calls();
    let deny = policy.secret_deny_paths_for_backend(true);
    assert!(
        deny.iter().any(|p| p.ends_with(".env")),
        "control: the walk must find the planted secret, or this test grades \
         nothing; got {deny:?}"
    );
    let _ = policy.secret_deny_paths_for_backend(true);
    assert_eq!(
        walk_calls() - before,
        1,
        "control: a sub-second stamp 60 ms old is settled and must be memoised"
    );

    // The same tree, the same 60 ms, but every directory stamped on a whole
    // second the way a coarse filesystem stamps them. 60 ms proves nothing
    // there, so the memo must refuse to answer.
    let coarse = tempfile::tempdir().unwrap();
    fixture(coarse.path());
    let whole_second = FileTime::from_unix_time(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs() as i64,
        0,
    );
    for dir in [coarse.path().to_path_buf(), coarse.path().join("src")] {
        filetime::set_file_mtime(&dir, whole_second).unwrap();
    }
    let policy = WorkspacePolicy::contained(coarse.path());
    std::thread::sleep(Duration::from_millis(60));

    let before = walk_calls();
    let deny = policy.secret_deny_paths_for_backend(true);
    assert!(
        deny.iter().any(|p| p.ends_with(".env")),
        "the forced stamp must not have broken the walk itself; got {deny:?}"
    );
    let _ = policy.secret_deny_paths_for_backend(true);
    assert_eq!(
        walk_calls() - before,
        2,
        "a whole-second mtime can hide a write for a whole second, so a memo \
         stamped with one must not be served after 60 ms of quiescence"
    );
}
