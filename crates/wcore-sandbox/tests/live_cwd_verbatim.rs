//! Live Windows acceptance for the `lpCurrentDirectory` verbatim-prefix fix.
//!
//! The unit guards in `windows_impl/tests.rs::resolve_cwd_*` assert the UTF-16
//! buffer production hands to `CreateProcessAsUserW`. These assert the thing
//! that actually matters: **where the child process ends up**, observed from
//! inside the child, through the `wcore-sandbox` PUBLIC surface only.
//!
//! The defect (found by frankforges, wayland-core #254): `std::fs::canonicalize`
//! returns the verbatim `\\?\C:\…` spelling for every local path on Windows.
//! Passed to `lpCurrentDirectory` unmodified, the command processor reads the
//! leading `\\` as UNC, refuses it as a current directory, prints *"CMD.EXE was
//! started with the above path as the current directory. UNC paths are not
//! supported. Defaulting to Windows directory."* and runs in `C:\Windows`. The
//! caller is never told; the child simply executes somewhere else.
//!
//! Falsifiability: [`verbatim_cwd_lands_in_the_requested_directory`] asks the
//! child to print its own cwd and asserts it is the requested directory AND is
//! not the Windows directory. Against the unfixed backend the child reports
//! `C:\Windows`, so the assertion fails — this is a measured red, recorded in
//! `.planning/intel/CORE-254-TAKEN.md`, not an assumed one. The companion case
//! pins the ordinary `C:\…` spelling so a fix that mangled every path (rather
//! than only the verbatim one) could not pass both.
//!
//! Gating matches `live_fs_acl.rs`: `#![cfg(windows)]` + `#[ignore]`, and
//! `require_live_acceptance()` ASSERTS on `WAYLAND_SANDBOX_LIVE_WINDOWS=1`
//! rather than returning early — an unset variable makes these fail, never
//! silently pass.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// The number of authored cwd acceptance cases, kept in lockstep with the
/// `#[ignore]`d tests below so a silently-dropped case fails the zero-execution
/// guard rather than shrinking the proof unnoticed.
const NATIVE_CWD_CASES: usize = 2;

fn require_live_acceptance() {
    assert_eq!(
        std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref(),
        Ok("1"),
        "native cwd acceptance requires WAYLAND_SANDBOX_LIVE_WINDOWS=1"
    );
    assert!(
        AppContainerBackend::new().is_available(),
        "explicit native cwd acceptance requires an available AppContainer backend"
    );
}

#[test]
#[ignore = "zero-execution guard for explicit native Windows acceptance"]
fn native_cwd_gate_marker() {
    require_live_acceptance();
    assert_eq!(NATIVE_CWD_CASES, 2);
}

/// A unique directory under `%PUBLIC%` — the same seeding `live_fs_acl.rs` uses,
/// because an AppContainer package SID can actually be granted there.
fn seed_dir(tag: &str) -> PathBuf {
    let public = std::env::var("PUBLIC").unwrap_or_else(|_| r"C:\Users\Public".into());
    let dir = PathBuf::from(public).join(format!("wcore-cwd-{}-{}", std::process::id(), tag));
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// `cmd /d /s /c cd` — with no argument `cd` prints the process's current
/// directory, i.e. the child reports where the OS actually put it.
fn print_cwd(cwd: PathBuf) -> SandboxCommand {
    SandboxCommand {
        argv: vec![
            "cmd.exe".into(),
            "/d".into(),
            "/s".into(),
            "/c".into(),
            "cd".into(),
        ],
        cwd: Some(cwd),
    }
}

async fn child_reported_cwd(grant: &Path, cwd: PathBuf) -> String {
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        fs_read_allow: vec![grant.to_path_buf()],
        ..Default::default()
    };
    let out = AppContainerBackend::new()
        .execute(&manifest, print_cwd(cwd))
        .await
        .expect("spawn must succeed");
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        !stdout.is_empty(),
        "child produced no cwd; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// THE REGRESSION GUARD. Hand the backend the canonicalized (verbatim) spelling
/// and require the child to land in that directory.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn verbatim_cwd_lands_in_the_requested_directory() {
    require_live_acceptance();
    let dir = seed_dir("verbatim");

    // This is how the path reaches the backend in production: canonicalize
    // returns `\\?\C:\Users\Public\...` on Windows, always.
    let verbatim = std::fs::canonicalize(&dir).expect("canonicalize");
    assert!(
        verbatim.to_string_lossy().starts_with(r"\\?\"),
        "precondition: canonicalize must yield the verbatim spelling, got {verbatim:?}"
    );

    let reported = child_reported_cwd(&dir, verbatim).await;

    // The bug's signature, named explicitly so a failure reads as itself.
    let windows_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    assert!(
        !reported.eq_ignore_ascii_case(&windows_dir),
        "child fell back to the Windows directory ({reported}) -- the verbatim \
         prefix reached lpCurrentDirectory and cmd.exe rejected it as UNC"
    );
    assert!(
        reported.eq_ignore_ascii_case(&dir.to_string_lossy()),
        "child cwd {reported} is not the requested directory {}",
        dir.display()
    );
}

/// The ordinary spelling must keep working untouched — so a "fix" that
/// rewrote every path, rather than only the verbatim form, fails here.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn plain_cwd_is_unaffected() {
    require_live_acceptance();
    let dir = seed_dir("plain");
    let reported = child_reported_cwd(&dir, dir.clone()).await;
    assert!(
        reported.eq_ignore_ascii_case(&dir.to_string_lossy()),
        "child cwd {reported} is not the requested directory {}",
        dir.display()
    );
}
