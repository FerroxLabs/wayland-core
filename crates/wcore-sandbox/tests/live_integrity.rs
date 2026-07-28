//! Live integrity-boundary verification (negative-test style).
//!
//! The hardened AppContainer pipeline (Low integrity + disabled
//! `BUILTIN\Administrators` / `Users` / `Authenticated Users` SIDs +
//! Job Object UI restrictions) is intentionally tight enough that
//! LSA-dependent system tools (`whoami /groups`, `wmic`, `net user`)
//! fail to run inside it. We assert that as a security property: a
//! child that CAN'T enumerate its own group membership has provably
//! lost access to the LSA endpoint, which means the restricted token
//! is doing its job.
//!
//! Why this shape, rather than a positive "IL=Low" check?
//!   1. A custom probe binary (`il_probe.exe` at
//!      `src/bin/il_probe.rs`) that calls `GetTokenInformation`
//!      directly cannot load under the hardened sandbox: NTFS DACLs
//!      on `target\debug\` exclude the AppContainer SID, and copying
//!      the binary into the AppContainer package storage still leaves
//!      it unable to resolve VCRUNTIME140.dll under the
//!      disabled-Users restricted token. This is the v0.7.0 filesystem
//!      allowlist's job (queued: wire
//!      `SetNamedSecurityInfoW(GRANT, AppContainer SID)`).
//!   2. The positive Low-IL proof comes from the Procmon trace gate
//!      (verification gate #2), which observes the child's integrity
//!      level at the OS layer. The test here proves the *consequence*
//!      of Low IL + restricted token, not the property itself.
//!
//! Companion live tests:
//!   * `echo_runs_live` (in src) — proves trivial cmd.exe spawn works.
//!   * `appcontainer_execute_trivial_command_returns_exit_zero` (in
//!     `tests/backend_integration.rs`) — proves end-to-end pipeline.
//!   * THIS test — proves the boundary is tight.

#![cfg(windows)]

use std::time::Duration;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

mod common;
use common::{
    capture_alive_descendant_pids, cmd_script, descendant_hold, inline_hold, live_descendant_count,
    reap_stray_descendants, require_live_windows, surviving_captured_descendant_pids, wait_until,
};

/// Number of live acceptance cases in this binary, excluding the guard itself.
/// Kept in lockstep with the `#[ignore]`d tests so a silently-dropped case is a
/// failure rather than a quietly smaller proof.
const LIVE_INTEGRITY_CASES: usize = 5;

/// F-WR-02 guard. This binary's acceptance cases are `#[ignore]`d, so the
/// obvious command — `cargo test -p wcore-sandbox --test live_integrity` —
/// executes NONE of them and still exits 0. Worse, before this change the cases
/// were not `#[ignore]`d at all but returned early when
/// `WAYLAND_SANDBOX_LIVE_WINDOWS` was unset, so that command printed
/// `5 passed` — an affirmative green for zero work, which is strictly harder to
/// notice than `0 passed; 12 ignored`.
///
/// This test is deliberately NOT `#[ignore]`d, so it always runs. It fires only
/// when the caller has declared live intent by setting the env var while asking
/// for a run that cannot execute the ignored cases — i.e. exactly the case where
/// a green would be read as certification. It is skipped under nextest, whose
/// `--run-ignored`/`--no-tests=fail` handling covers the same ground and which
/// runs each test in its own process (so this one would not see `--ignored`).
///
/// Falsifiable: set `WAYLAND_SANDBOX_LIVE_WINDOWS=1` and run this binary without
/// `-- --ignored` and it FAILS. Add `-- --ignored` and it passes and the real
/// cases run.
#[test]
fn live_acceptance_run_must_not_report_success_on_zero_tests() {
    assert_eq!(LIVE_INTEGRITY_CASES, 5);
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "WAYLAND_SANDBOX_LIVE_WINDOWS=1 declares a live acceptance run, but this \
         invocation cannot execute any of the {LIVE_INTEGRITY_CASES} acceptance cases \
         — they are #[ignore]d and neither --ignored nor --include-ignored was passed. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-sandbox --test live_integrity -- --ignored --test-threads=1"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn live_lsa_dependent_tool_fails_under_hardened_sandbox() {
    require_live_windows();

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    // `whoami /groups` enumerates group SIDs and calls LsaLookupSids2
    // to format friendly names like `BUILTIN\Administrators`. The lookup
    // requires the calling thread's token to grant access to the LSA
    // ALPC port `\Default`, which under our hardened pipeline it does
    // not (Admins/Users/AuthUsers SIDs are deny-only on the restricted
    // token; the AppContainer SID is not on the LSA port's DACL).
    //
    // If this test starts PASSING (whoami exit=0 with group output), it
    // means the sandbox just got LOOSER — either SidsToDisable went
    // away, the token integrity dropped to something LSA accepts, or
    // a new capability was granted. That's a security regression.
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "whoami /groups".into()],
                cwd: None,
            },
        )
        .await
        .expect("AppContainer spawn must succeed even if whoami fails inside");

    assert_ne!(
        out.exit_code,
        0,
        "whoami /groups SUCCEEDED under hardened AppContainer — sandbox just got LOOSER. \
         A successful LSA group lookup means the restricted token's SID disabling and / or \
         Low integrity pinning is no longer effective. stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Verifies the positive control: a tool with no LSA / network / USER
/// surface dependencies (just `cmd` builtins) DOES run successfully
/// inside the sandbox. This is the matched-pair to the negative test
/// above — together they prove the sandbox is "tight enough to block
/// LSA, loose enough to run a shell builtin."
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn live_cmd_builtin_runs_under_hardened_sandbox() {
    require_live_windows();

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "echo proof-of-life".into()],
                cwd: None,
            },
        )
        .await
        .expect("AppContainer cmd /c echo spawn failed");
    assert_eq!(
        out.exit_code,
        0,
        "cmd /c echo should run inside the hardened sandbox; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("proof-of-life"),
        "expected 'proof-of-life' in stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Field regression (#321-324 follow-up, PR #99). The local fs allowlist
/// routinely includes optional dev caches (`~/.cache`, `~/.cargo`, `~/.npm`,
/// `~/.rustup`) that are ABSENT on non-developer machines. Before the
/// grant/deny skip-missing fix, `GetNamedSecurityInfoW` returned
/// `ERROR_FILE_NOT_FOUND` (0x2) on the absent path and aborted the whole spawn,
/// so EVERY sandboxed shell command hard-failed in the field. This proves a real
/// sandboxed `cmd` still runs end-to-end when the allowlist contains a
/// non-existent path alongside a real one.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn live_cmd_runs_when_allowlist_has_missing_path() {
    require_live_windows();

    // The real allowlist entry is a directory this test OWNS, not
    // `std::env::temp_dir()`, which is what it used to be. That is a
    // determinism repair, not a relaxation: the property under test is that a
    // NON-EXISTENT allowlist entry is skipped rather than aborting the spawn,
    // and the size of the entry beside it was never part of it. The assertions
    // below are unchanged — still exit 0, still the stdout marker, still one
    // real path and one absent path.
    //
    // Why it had to change (F-KR-07 ladder, measured on SeanDesktop): the cost
    // of a grant is dominated by the number of objects under the granted path,
    // and `%TEMP%` is unbounded, shared with every other process on the host,
    // and outside this test's control. Same command, same manifest, same
    // two-entry allowlist shape, varying only that one directory:
    //
    //     200 objects        133 ms
    //     %TEMP%, 57 636   ~10 000 ms   (5 consecutive runs: 9 848 … 10 629 ms)
    //     200 000 objects  19 487 ms
    //
    // The effective budget is 25s (`manifest.timeout` 10s plus the 15s setup
    // grace in `windows_impl::process`, which exists because the inner
    // `WaitForSingleObject` bounds only the child's RUN). Against `%TEMP%` this
    // test therefore ran at ~42% of its ceiling with an unbounded dependency
    // deciding the margin — a pass whose survival depended on how much litter
    // the host happened to be holding. `F-KR-07` recorded it failing 12/12 with
    // `SandboxError::Timeout`; it passes today, and the only thing known to
    // have changed on the host between the two is `%TEMP%` getting smaller.
    //
    // NO TIMEOUT IS RAISED. The manifest below is the original 10s.
    let real = std::env::temp_dir().join(format!(
        "wcore-allowlist-skip-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&real).expect("create the real allowlist directory");
    std::fs::write(real.join("cache-entry.bin"), b"kr07").expect("seed the real allowlist entry");
    let missing = std::path::PathBuf::from(r"C:\__wcore_absent_cache__\.npm");
    assert!(
        !missing.exists(),
        "precondition: the allowlist path must be absent"
    );

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        fs_read_allow: vec![real.clone(), missing],
        timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };
    let spawned = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "echo allowlist-skip-ok".into(),
                ],
                cwd: None,
            },
        )
        .await;
    let _ = std::fs::remove_dir_all(&real);
    let out = spawned.expect("AppContainer spawn must succeed despite a non-existent allowlist path");
    assert_eq!(
        out.exit_code,
        0,
        "cmd must run when the allowlist has a missing path; stderr={:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("allowlist-skip-ok"),
        "expected 'allowlist-skip-ok' in stdout: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// #100 regression: a runaway command must be bounded by the manifest timeout.
/// On timeout the backend terminates the whole job tree and reaps it before
/// draining, so the blocking `drain_pipe` can reach EOF even when the child (or
/// a helper it spawned, e.g. a console host) is still alive — otherwise the call
/// hangs far past the timeout (the 120s "command timed out, no output" symptom).
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn live_runaway_command_is_bounded_by_timeout() {
    require_live_windows();

    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        timeout: Some(Duration::from_secs(3)),
        ..Default::default()
    };
    let start = std::time::Instant::now();
    // `for /l %i in (0,0,1)` never reaches its end value -> infinite cmd loop.
    let r = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec![
                    "cmd.exe".into(),
                    "/c".into(),
                    "for /l %i in (0,0,1) do @rem".into(),
                ],
                cwd: None,
            },
        )
        .await;
    let secs = start.elapsed().as_secs();
    assert!(
        secs <= 8,
        "runaway command must be bounded by the 3s timeout; took {secs}s (drain hung past timeout)"
    );
    assert!(
        matches!(r, Err(wcore_sandbox::SandboxError::Timeout)),
        "expected SandboxError::Timeout, got {r:?}"
    );
}

/// Dropping the async execution future is the session-cancellation path: the
/// whole descendant tree must be reaped, not just the direct child.
///
/// # Why this test was rebuilt (F-WR-01)
///
/// The original construction could not reach this assertion, and had not been
/// able to since the commit that introduced it (`2b662fe8`, which added both the
/// reap fix AND this test). It wrote a `.cmd` heartbeat script under `%PUBLIC%`,
/// granted itself the directory, set `cwd` to it, and executed the script
/// through a NESTED `cmd.exe /d /c cmd.exe /d /c <script>`. Measured on real
/// hardware, that shape returns exit 1 / `Access is denied.` in ~2.7s — the
/// nested spawn is refused — so no descendant was ever created, `heartbeat.txt`
/// was never written, and the run aborted at the "exited before cancellation"
/// arm. `rc=101` read exactly like "the reap is broken"; only the wall clock
/// (0.53s against a body that sleeps 10+2+2s) revealed that the body had not
/// run. A landed fix therefore carried its own acceptance test as a red for two
/// weeks, and that red was attributed to the defect the fix had closed.
///
/// Two independent construction faults, both since proven by measurement:
///   1. `choice.exe` — the heartbeat's only sleep primitive — exits in <80ms
///      under the Low-IL AppContainer restricted token (console/DLL deps fail to
///      load), so the loop could never have held even had it started. A bare
///      `for /L` cmd BUILTIN is the only primitive that holds here.
///   2. The descendant must be detached with `start "" /b`, the shape every
///      PASSING descendant test on this platform uses, not a nested `/c`.
///
/// The witness was also weak: file length cannot distinguish "reaped" from
/// "alive but starved past both sampling windows", and under competing load that
/// bias runs toward a FALSE PASS. It is replaced by fixed-`ProcessId` liveness —
/// the descendant's PID is captured WHILE it is provably alive, and the reap is
/// asserted against those exact PIDs. `capture_alive_descendant_pids` panics if
/// it never observes one, so a run that creates no descendant now FAILS as
/// unmeasurable instead of aborting in setup or passing vacuously.
///
/// Requires serial execution within this binary (`--test-threads=1`, or nextest's
/// process-per-test): `resolve_anchor_pid` fails closed on more than one
/// concurrent anchor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn live_future_drop_reaps_descendant_job_tree() {
    require_live_windows();

    // The anchor detaches a `for /L` idler cmd.exe (a real descendant, child of
    // the anchor) and then holds ITSELF alive with an inline `for /L`, so the
    // execution future is still in flight when it is dropped below. The idler is
    // asked to hold far longer than the anchor so that, absent job ownership, it
    // would outlive the cancellation and stay observable.
    let script = format!(
        "start \"\" /b {idle} & {parent} & exit /b 0",
        idle = descendant_hold(60),
        parent = inline_hold(8),
    );
    let manifest = SandboxManifest {
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };

    let captured;
    {
        let backend = AppContainerBackend::new();
        let execution = backend.execute(&manifest, cmd_script(script));
        tokio::pin!(execution);

        // Drive the execution future while observing the host, and REFUSE to
        // proceed unless a descendant is genuinely seen alive. The future
        // completing here means the anchor exited before we could cancel it —
        // that is an unusable run, not a reap result.
        let observed = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                tokio::select! {
                    result = &mut execution => {
                        panic!(
                            "anchor exited before cancellation could be applied, so the \
                             future-drop path was never exercised: {result:?}"
                        );
                    }
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        if tokio::task::block_in_place(live_descendant_count) >= 1 {
                            break;
                        }
                    }
                }
            }
        })
        .await;
        observed.expect("a detached descendant must be observed alive before cancellation");

        // Capture by fixed ProcessId WHILE the anchor is alive. Panics if it
        // cannot, so the post-drop check can never be vacuous.
        captured = tokio::task::block_in_place(|| capture_alive_descendant_pids(1, 20));
        println!("KR01_WITNESS_DESCENDANTS_ALIVE_BEFORE_DROP={captured:?}");

        // Dropping `execution` here IS the session-cancellation path under test.
    }

    assert!(
        !captured.is_empty(),
        "descendant PID set was not captured — the reap check would be vacuous"
    );

    // The idler was asked to hold ~60s and the anchor only ~8s, so absent job
    // ownership these exact PIDs would still be alive well past this deadline.
    tokio::task::block_in_place(|| {
        wait_until(
            || surviving_captured_descendant_pids(&captured) == 0,
            30,
            "detached descendant reaped after execution future drop",
        )
    });
    println!(
        "KR01_WITNESS_SURVIVORS_AFTER_DROP={} of {}",
        tokio::task::block_in_place(|| surviving_captured_descendant_pids(&captured)),
        captured.len()
    );
    reap_stray_descendants();
}
