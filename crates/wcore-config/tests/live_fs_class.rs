//! Live filesystem-classification test: run the real `statfs`/`GetDriveTypeW`
//! probe against a REAL network mount and a REAL local disk on the same host.
//!
//! # Why this exists
//!
//! `sqlite_journal`'s classifier was unit-tested against its *pure* arms —
//! `classify_linux_magic`, `classify_macos`, `is_windows_unc` — all fed
//! hand-written inputs. Only the Linux path was ever exercised end-to-end on a
//! real mount. That left the two arms that cannot be checked by inspection
//! completely unproven:
//!
//! - **macOS** — that `imp::classify_existing` really reads `f_flags` and
//!   `f_fstypename` out of a live `statfs`, that the `c_char`-array-to-`str`
//!   conversion produces the mount's actual type name, and that a genuine
//!   remote mount really does lack `MNT_LOCAL`.
//! - **Windows** — that `GetVolumePathNameW` + `GetDriveTypeW` really return
//!   `DRIVE_REMOTE` for a mapped network drive. This arm is unreachable by
//!   string inspection: `Z:\` looks exactly like a local drive letter, so the
//!   syntactic UNC check cannot see it and only the live API call can.
//!
//! # Why it is `#[ignore]` and why that is not a self-passing gate
//!
//! It needs mounts CI does not have. The failure mode the lane brief warns
//! about is a suite that exits 0 having run zero tests, so:
//!
//! - it is `#[ignore]`d and must be run explicitly with `-- --ignored`, and the
//!   caller must read back the `N passed` count;
//! - when the required environment is absent it **panics** rather than
//!   returning early. An env-gated early `return` is the measured
//!   "printed `5 passed` for zero work" defect; this cannot do that.
//!
//! # Running it
//!
//! ```text
//! WL_FS_NETWORK_PATH=/Users/me/smb-mount \
//! WL_FS_LOCAL_PATH=/tmp \
//!   cargo test -p wcore-config --test live_fs_class -- --ignored --exact \
//!     live_classifier_discriminates_network_from_local
//! ```
//!
//! Both variables are mandatory. A one-sided run is not a result: a classifier
//! that answered `Network` for everything would pass the network arm alone, and
//! the pre-fix code (which answered `WAL` for everything) would pass the local
//! arm alone.

use std::path::PathBuf;

use wcore_config::sqlite_journal::{FsClass, JOURNAL_MODE_ENV, SqliteJournalMode, classify_path};

/// Read a mandatory path variable. Panics when unset — see module docs: an
/// early `return` here would turn a zero-work run into a reported pass.
fn required_path(var: &str) -> PathBuf {
    let raw = std::env::var(var).unwrap_or_else(|_| {
        panic!(
            "{var} is not set. This test measures a live mount and cannot be \
             run without one; it must FAIL rather than silently pass having \
             measured nothing."
        )
    });
    assert!(!raw.trim().is_empty(), "{var} is set but empty");
    let path = PathBuf::from(raw);
    assert!(
        path.exists(),
        "{var}={} does not exist — the mount is not present, so any answer \
         from the classifier would come from the walk-up-to-an-ancestor path \
         and would be about a different filesystem entirely.",
        path.display()
    );
    path
}

#[test]
#[ignore = "requires a real network mount; run with --ignored and both WL_FS_* vars set"]
fn live_classifier_discriminates_network_from_local() {
    // A forced mode would make the journal-mode half of this test report the
    // kill-switch's answer rather than the classifier's.
    assert!(
        std::env::var(JOURNAL_MODE_ENV).is_err(),
        "{JOURNAL_MODE_ENV} is set; unset it or this test measures the \
         override instead of the detector"
    );

    let network = required_path("WL_FS_NETWORK_PATH");
    let local = required_path("WL_FS_LOCAL_PATH");

    let network_class = classify_path(&network);
    let local_class = classify_path(&local);
    let network_mode = SqliteJournalMode::for_db_path(&network.join("probe.sqlite"));
    let local_mode = SqliteJournalMode::for_db_path(&local.join("probe.sqlite"));

    println!(
        "LIVEFS os={} network={} -> {:?} / {:?}",
        std::env::consts::OS,
        network.display(),
        network_class,
        network_mode
    );
    println!(
        "LIVEFS os={} local={} -> {:?} / {:?}",
        std::env::consts::OS,
        local.display(),
        local_class,
        local_mode
    );

    // (1) KNOWN-NEGATIVE — the real network mount must not select WAL.
    assert_eq!(
        network_class,
        FsClass::Network,
        "the live probe failed to recognise {} as a network filesystem",
        network.display()
    );
    assert_ne!(
        network_mode,
        SqliteJournalMode::Wal,
        "a network mount selected WAL — this is the defect, live"
    );

    // (2) KNOWN-POSITIVE — and the local disk must still get WAL, or the
    //     classifier is just answering "Network" to everything and the
    //     assertion above proves nothing.
    assert_eq!(
        local_class,
        FsClass::Local,
        "the live probe failed to recognise {} as a local filesystem; a \
         classifier that says Network to everything passes the network arm \
         for free",
        local.display()
    );
    assert_eq!(
        local_mode,
        SqliteJournalMode::Wal,
        "local disk must keep WAL — the fix must not be a blanket disable"
    );

    // (3) THE OLD CODE COULD NOT TELL THESE APART. Without this, both arms
    //     above still pass on an instrument that never consulted the
    //     filesystem: the pre-fix value was a constant.
    let legacy = SqliteJournalMode::legacy_unconditional();
    assert_eq!(
        legacy, local_mode,
        "legacy agreed with the selector on local disk, so the local arm on \
         its own cannot demonstrate the repair"
    );
    assert_ne!(
        legacy, network_mode,
        "on this live network mount the legacy code chose {legacy:?} where the \
         selector chose {network_mode:?} — that difference IS the fix, measured \
         rather than argued"
    );
}
