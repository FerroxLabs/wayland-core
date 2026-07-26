//! F24-01 Task 1 — the hostile pid-lock cases.
//!
//! Written BEFORE `src/pidlock.rs` existed. Every case here is one of the
//! four Windows defect classes the 20A handoff names, or the stale-holder
//! case that turns a crash into a permanently unstartable runtime.
//!
//! These run on all three families deliberately. The Windows-specific
//! HAZARD is mandatory locking, but the CONTRACT (a reader is never
//! blocked; a live holder excludes a second launch; a dead holder is
//! reclaimed; a path stored in one representation still compares equal to
//! the same path in another) is the same everywhere, so the same
//! assertions must hold everywhere.

use wcore_gateway::pidlock::{PidLock, PidLockError, normalise_path, process_is_alive};

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// A second gateway launch against the same home is refused while the
/// first is alive, and the refusal NAMES the holder's process identity so
/// an operator can act on it.
#[test]
fn second_launch_against_a_live_holder_is_refused_with_the_holder_pid() {
    let dir = home();
    let first = PidLock::acquire(dir.path()).expect("first acquire succeeds");

    let err = PidLock::acquire(dir.path()).expect_err("second acquire must be refused");
    match err {
        PidLockError::AlreadyHeld { pid } => {
            assert_eq!(
                pid,
                std::process::id(),
                "the refusal must name the live holder's pid"
            );
        }
        other => panic!("expected AlreadyHeld, got {other:?}"),
    }

    drop(first);
}

/// Releasing the lock lets the next launch through. Without this the
/// first clean stop would make the runtime permanently unstartable.
#[test]
fn releasing_the_lock_admits_the_next_launch() {
    let dir = home();
    let first = PidLock::acquire(dir.path()).expect("first acquire");
    drop(first);
    let second = PidLock::acquire(dir.path()).expect("acquire after release");
    drop(second);
}

/// A pid file whose process is gone is stale and is reclaimed. This is the
/// crash case: the runtime died without removing its own pid file, and a
/// gateway that refuses to start after a crash is not a persistent
/// runtime.
#[test]
fn a_pid_file_whose_process_is_gone_is_reclaimed() {
    let dir = home();

    // Seed the home with a pid record for a process that cannot exist.
    // The lock file is deliberately NOT held, which is exactly the state a
    // crashed process leaves behind: the OS released its lock when the
    // process died, but the pid file it wrote is still on disk.
    PidLock::write_stale_record_for_test(dir.path(), 0x7FFF_FFFF);

    let reclaimed = PidLock::acquire(dir.path()).expect("stale holder must be reclaimable");
    let record = PidLock::read_record(dir.path()).expect("record readable after reclaim");
    assert_eq!(
        record.pid,
        std::process::id(),
        "the reclaiming process must own the record"
    );
    drop(reclaimed);
}

/// THE SENTINEL PROPERTY. While the lock is held, the crate's own status
/// reader still reads the pid record. On Windows a whole-file lock is
/// MANDATORY, so a lock taken over the readable record would exclude this
/// reader — which is why the lock lives on a one-byte sentinel file that
/// no reader ever opens.
#[test]
fn the_status_reader_is_not_blocked_by_a_held_lock() {
    let dir = home();
    let held = PidLock::acquire(dir.path()).expect("acquire");

    let record = PidLock::read_record(dir.path())
        .expect("the status reader must succeed while the lock is held");
    assert_eq!(record.pid, std::process::id());
    assert!(
        record.home == normalise_path(dir.path()),
        "the record must carry the normalised home it was taken against"
    );

    // And a plain byte read of the same file must also succeed — this is
    // the assertion that actually goes red under a mandatory whole-file
    // lock on the record.
    let raw = std::fs::read(PidLock::record_path(dir.path()))
        .expect("a raw read of the pid record must not be blocked by the lock");
    assert!(!raw.is_empty());

    drop(held);
}

/// The lock file and the readable record are DIFFERENT files. If they were
/// the same file the sentinel property above would be accidental rather
/// than designed, and a later refactor would silently lose it.
#[test]
fn the_lock_sentinel_is_not_the_readable_record() {
    let dir = home();
    let held = PidLock::acquire(dir.path()).expect("acquire");
    assert_ne!(
        PidLock::lock_path(dir.path()),
        PidLock::record_path(dir.path()),
        "the lock sentinel must be a separate file from the readable record"
    );
    let sentinel_len = std::fs::metadata(PidLock::lock_path(dir.path()))
        .expect("sentinel exists")
        .len();
    assert_eq!(
        sentinel_len, 1,
        "the sentinel must be exactly one byte, so the mandatory Windows lock \
         covers one byte and nothing an operator or the crate wants to read"
    );
    drop(held);
}

/// PATH REPRESENTATION. A home stored in one representation and compared
/// in another must still compare equal. Normalisation happens at the
/// COMPARISON boundary on BOTH operands: the simplification helpers are
/// documented as conditional and no-op on several real inputs, so
/// normalising only at storage time is a fail-open.
#[test]
fn a_home_compares_equal_across_representations() {
    let dir = home();
    let plain = dir.path().to_path_buf();

    // The same directory reached through a redundant `.` component and a
    // parent/child round-trip. Both are the same directory to the OS and
    // must be the same home to the lock.
    let via_dot = plain.join(".");
    let leaf = plain.file_name().expect("leaf").to_owned();
    let via_parent = plain
        .parent()
        .expect("parent")
        .join("..")
        .join(
            plain
                .parent()
                .expect("parent")
                .file_name()
                .expect("gp leaf"),
        )
        .join(&leaf);

    assert_eq!(
        normalise_path(&plain),
        normalise_path(&via_dot),
        "a `.` component must not change the identity of a home"
    );
    assert_eq!(
        normalise_path(&plain),
        normalise_path(&via_parent),
        "a parent round-trip must not change the identity of a home"
    );

    // And the lock itself must refuse a second acquire through the OTHER
    // representation, which is the property the normalisation exists for.
    let held = PidLock::acquire(&plain).expect("acquire via the plain path");
    let err = PidLock::acquire(&via_dot)
        .expect_err("a second acquire through a different representation must be refused");
    assert!(
        matches!(err, PidLockError::AlreadyHeld { .. }),
        "expected AlreadyHeld through the alternate representation, got {err:?}"
    );
    drop(held);
}

/// Two DIFFERENT homes do not exclude each other. Without this the lock
/// would be a global singleton and profile isolation would be broken by
/// the gateway rather than preserved by it.
#[test]
fn two_different_homes_do_not_exclude_each_other() {
    let a = home();
    let b = home();
    let held_a = PidLock::acquire(a.path()).expect("acquire home a");
    let held_b = PidLock::acquire(b.path()).expect("acquire home b");
    drop(held_a);
    drop(held_b);
}

/// The liveness probe is honest on both families: the running process
/// reads alive, and an identifier that cannot name a process reads dead.
/// This is the same assertion pair the existing cron probe carries, moved
/// rather than reinvented.
#[test]
fn liveness_probe_is_honest_in_both_directions() {
    assert!(
        process_is_alive(std::process::id()),
        "the running process must read alive"
    );
    assert!(
        !process_is_alive(0x7FFF_FFFF),
        "an identifier that cannot name a live process must read dead"
    );
}
