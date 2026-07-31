//! B2.3 — the shipped binary still runs when it cannot open its log, and SAYS
//! SO.
//!
//! # Why "it still exits 0" is not the assertion
//!
//! `lane/fix-tui-noise` listed the log-file-open-failure fallback as explicitly
//! unrun. The obvious test — make the log unopenable, run the binary, assert
//! success — passes just as happily if logging is dead, disabled, or was never
//! attempted in the first place. It demonstrates the absence of a crash, not
//! the presence of a fallback.
//!
//! So the degraded leg asserts an **observable**: the exact
//! [`LOG_FALLBACK_NOTICE`] the product prints when it gives up on the file and
//! continues on stderr. The constant is imported rather than retyped, so the
//! test cannot drift from the string the binary emits.
//!
//! # The positive control
//!
//! A notice-is-present assertion is worthless if the notice is always present.
//! [`a_writable_log_dir_produces_a_log_file_and_no_notice`] runs the same
//! binary with the same arguments against a WRITABLE home and requires the
//! opposite of every degraded-leg assertion: the log file exists, and the
//! notice is absent. Between them the two legs show the marker tracks the
//! condition rather than the run.
//!
//! # The negative control that cannot live here
//!
//! "Show the same assertion FAILS when the fallback is removed" needs a second,
//! differently-compiled binary, which a `#[test]` in this crate cannot produce.
//! It was run by hand instead: deleting the `eprintln!` in `main.rs` and
//! rebuilding makes `the_binary_survives_an_unopenable_log_dir` fail on the
//! missing notice while still exiting 0 — which is precisely the false green
//! this file exists to refuse. The transcript is in the lane report.

use std::path::Path;
use std::process::Command;

use wcore_cli::log_rotate::LOG_FALLBACK_NOTICE;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// `models list` is offline, terminates immediately, and runs AFTER the
/// tracing subscriber is installed in `main`, so it exercises the log-open
/// decision without needing a provider, a network or a credential.
fn run_with_home(home: &Path) -> std::process::Output {
    Command::new(bin())
        .args(["models", "list"])
        .env("WAYLAND_HOME", home)
        // `log_to_file` is `will_enter_tui || !rust_log_set`. Stdout is a pipe
        // here so the TUI half is already false; RUST_LOG must be UNSET for the
        // file path to be taken at all. Inheriting a developer's or CI's
        // RUST_LOG would silently route this test down the stderr-only branch
        // and make both legs vacuous.
        .env_remove("RUST_LOG")
        .output()
        .expect("the wayland-core binary must run")
}

fn log_path(home: &Path) -> std::path::PathBuf {
    home.join("logs").join("wayland-core.log")
}

/// POSITIVE CONTROL: a writable home logs to the file and prints no notice.
#[test]
fn a_writable_log_dir_produces_a_log_file_and_no_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_with_home(tmp.path());
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(
        out.status.success(),
        "the binary failed on a writable home: {stderr}"
    );
    assert!(
        log_path(tmp.path()).is_file(),
        "no log file was written to a WRITABLE home, so the degraded leg below would prove \
         nothing — it would be asserting a fallback for a feature that never works"
    );
    assert!(
        !stderr.contains(LOG_FALLBACK_NOTICE),
        "the degraded-mode notice was printed on a healthy run, so its presence in the other \
         leg would not distinguish anything:\n{stderr}"
    );
}

/// DEGRADED: the log directory cannot be created, and the run says so.
///
/// A plain FILE where the `logs/` directory belongs makes `create_dir_all`
/// fail on every platform. Chosen over a chmod so the leg is not
/// Unix-only and does not behave differently for a root CI user — `root`
/// ignores mode bits, which would have turned this into another silently
/// self-passing gate on the build host.
#[test]
fn the_binary_survives_an_unopenable_log_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("logs"), b"not a directory").unwrap();

    let out = run_with_home(tmp.path());
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);

    // 1. The product still works. A product that cannot open its log must
    //    still run — and must still do the thing it was asked to do, which is
    //    why stdout is checked and not only the exit status.
    assert!(
        out.status.success(),
        "the binary refused to run with an unopenable log dir:\n{stderr}"
    );
    assert!(
        !stdout.trim().is_empty(),
        "the binary exited 0 but produced no output, so it did not actually complete the \
         command; a silent success is not the fallback working"
    );

    // 2. The fallback PATH EXECUTED. This is the assertion the leg exists for.
    assert!(
        stderr.contains(LOG_FALLBACK_NOTICE),
        "the run survived but never announced that it had degraded. Exit status 0 is equally \
         consistent with logging being dead, disabled, or never attempted — the observable is \
         what distinguishes a working fallback from a missing feature.\nstderr was:\n{stderr}"
    );

    // 3. And it degraded for the stated reason, not some other one.
    assert!(
        !log_path(tmp.path()).is_file(),
        "a log file exists despite the notice claiming it could not be opened"
    );
}
