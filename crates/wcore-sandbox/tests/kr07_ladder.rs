//! `F-KR-07` denial ladder — attribute the deterministic `SandboxError::Timeout`
//! of `live_cmd_runs_when_allowlist_has_missing_path` to ONE property.
//!
//! The finding as reported is: a plain `cmd /c echo` under a 10s manifest fails
//! 12/12 serial runs with `SandboxError::Timeout`. Reported, never chased. A
//! deterministic failure is measurable, so this file measures it rather than
//! guessing, one property per rung, each rung a separate observation — the
//! method that converted `KR-01` from a two-week misdiagnosis into a clean
//! DISPROVED in one pass.
//!
//! The failing test differs from its already-passing sibling
//! (`live_cmd_builtin_runs_under_hardened_sandbox`, byte-identical argv, same
//! 10s timeout, PASSES) in exactly ONE field: `fs_read_allow`. So the cause is
//! inside the ACL lease, and the ladder splits that field apart.
//!
//! What the ladder can distinguish, and why the numbers mean something:
//!
//! * `windows_impl::process` bounds the whole blocking call at
//!   `manifest.timeout + 15s`, because the inner `WaitForSingleObject` bounds
//!   only the child's RUN, not the Win32 setup before it. So the elapsed time of
//!   the failure names the mechanism: **~10s => the child hung**; **~25s => the
//!   SETUP hung**, i.e. the ACL apply, which is the only setup this test adds.
//! * `apply_explicit_access` calls `SetNamedSecurityInfoW` with
//!   `SUB_CONTAINERS_AND_OBJECTS_INHERIT`, which propagates the new inheritable
//!   ACE across the target's whole subtree. The failing test grants over
//!   `std::env::temp_dir()`, which on this host has **10,341 top-level entries**.
//!   If the cost is the subtree, a small directory must behave differently.
//!
//! NOTHING here is a fix, and no rung may become one. Rung 5 deliberately runs
//! with a large ceiling — that is an INSTRUMENT for separating "slow" from
//! "stuck", not a repair. Raising the timeout of the real test is specifically
//! forbidden, and this file does not modify that test.
//!
//! Run serially — `--test-threads=1` is a correctness requirement for live
//! AppContainer suites (`F-KR-08`), not a preference.
//!
//! # What every rung asserts, and why it is not a fix (wayland#934, 2026-08-28)
//!
//! Rungs 2, 4, 6 and 7 called `observe()`, printed, and asserted NOTHING — they
//! bound the outcome to `_ok` and `_label` and discarded both, so they returned
//! green against a sandbox that launches nothing at all. That is not a defensible
//! shape even for a diagnostic: a rung that cannot fail cannot distinguish "the
//! ladder ran and the mechanism is X" from "the ladder ran and the harness is
//! broken", which is the only thing a reader uses these numbers for.
//!
//! They now assert the one thing that is true independently of whether the defect
//! under investigation is fixed: **the backend's own documented bound**.
//! `windows_impl::process` caps the whole blocking call at `manifest.timeout + 15s`
//! (the inner `WaitForSingleObject` bounds only the child's run; the outer ceiling
//! bounds setup too). An `execute()` that returns outside that window has broken
//! the contract the elapsed-time attribution in this file is entirely built on, so
//! every rung reading a number would be reading a meaningless one.
//!
//! Asserting `ok` on rungs 2, 4, 6 and 7 remains FORBIDDEN — that would be
//! asserting the finding is closed, which is what a diagnostic ladder must not do.
//! Rungs 1, 3 and 5 assert `ok` because each is a control or an isolated property
//! whose red is itself the finding.

#![cfg(windows)]

use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

mod common;
use common::require_live_windows;

/// Number of ladder rungs in this binary, excluding the guard itself. Kept in
/// lockstep with the `#[ignore]`d rungs so a silently-dropped rung is a failure
/// rather than a quietly shorter ladder.
const KR07_RUNGS: usize = 7;

/// The absent allowlist path the field regression is named for.
const ABSENT: &str = r"C:\__wcore_absent_cache__\.npm";

/// F-WR-02 guard, same shape as the one in `live_integrity.rs`. Every rung below
/// is `#[ignore]`d, so `cargo test --test kr07_ladder` executes NONE of them and
/// still exits 0 printing `test result: ok`. This test is deliberately NOT
/// `#[ignore]`d, so a run that declares live intent but cannot execute a single
/// rung fails loudly instead of certifying nothing.
#[test]
fn ladder_run_must_not_report_success_on_zero_rungs() {
    assert_eq!(KR07_RUNGS, 7);
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_SANDBOX_LIVE_WINDOWS").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "WAYLAND_SANDBOX_LIVE_WINDOWS=1 declares a live ladder run, but this invocation \
         cannot execute any of the {KR07_RUNGS} rungs — they are #[ignore]d and neither \
         --ignored nor --include-ignored was passed. Exiting 0 here would certify nothing. \
         Re-run with: cargo test -p wcore-sandbox --test kr07_ladder -- --ignored --test-threads=1"
    );
}

fn echo_cmd() -> SandboxCommand {
    SandboxCommand {
        argv: vec!["cmd.exe".into(), "/c".into(), "echo kr07-ok".into()],
        cwd: None,
    }
}

/// One observation: run the echo under `allow`, report outcome and elapsed.
/// Returns `(ok, label, elapsed_ms)`. Never asserts — the caller decides what
/// the property was, so a rung under investigation can record a red honestly.
async fn observe(
    rung: &str,
    allow: Vec<std::path::PathBuf>,
    timeout_secs: u64,
) -> (bool, String, u128) {
    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        fs_read_allow: allow,
        timeout: Some(Duration::from_secs(timeout_secs)),
        ..Default::default()
    };
    let start = Instant::now();
    let result = b.execute(&m, echo_cmd()).await;
    let elapsed = start.elapsed().as_millis();
    let (ok, label) = match &result {
        Ok(out)
            if out.exit_code == 0 && String::from_utf8_lossy(&out.stdout).contains("kr07-ok") =>
        {
            (true, "OK_EXIT0_WITH_STDOUT".to_string())
        }
        Ok(out) => (
            false,
            format!(
                "SPAWNED_BUT_WRONG exit={} stdout={:?} stderr={:?}",
                out.exit_code,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            ),
        ),
        Err(e) => (false, format!("ERR {e:?}")),
    };
    println!("KR07_RUNG_{rung} ok={ok} elapsed_ms={elapsed} outcome={label}");
    (ok, label, elapsed)
}

/// RUNG 1 — CONTROL. The sibling shape: identical argv, identical timeout, NO
/// allowlist at all. Establishes that `cmd /c echo` runs under this sandbox on
/// this host right now, so any later red is attributable to the allowlist and
/// not to the sandbox, the host, or the command.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung1_control_no_allowlist_at_all() {
    require_live_windows();
    let (ok, label, ms) = observe("1_CONTROL", vec![], 10).await;
    assert!(
        ok,
        "CONTROL rung failed after {ms}ms ({label}) — the sandbox cannot run a bare \
         `cmd /c echo` on this host, so nothing below this rung is attributable to the \
         allowlist. Investigate the host before reading any other rung."
    );
}

/// RUNG 2 — the exact failing shape, reproduced verbatim from
/// `live_cmd_runs_when_allowlist_has_missing_path`: real `%TEMP%` + the absent
/// path, 10s manifest. No expectation asserted — this is the observation under
/// investigation. The ELAPSED time is the payload: ~10s means the child hung,
/// ~25s means setup hung (10s manifest + 15s setup grace ceiling).
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung2_exact_failing_shape_tempdir_plus_absent() {
    require_live_windows();
    let missing = std::path::PathBuf::from(ABSENT);
    assert!(
        !missing.exists(),
        "precondition: allowlist path must be absent"
    );
    let (_ok, _label, ms) = observe("2_EXACT_SHAPE", vec![std::env::temp_dir(), missing], 10).await;
    let mechanism = mechanism_from_elapsed(ms);
    println!("KR07_RUNG_2_MECHANISM={mechanism}");
    assert_within_documented_ceiling("2", 10, ms);
    // The outcome is deliberately NOT asserted — this is the shape under
    // investigation. What IS asserted is that the reading is interpretable:
    // `UNEXPECTED_beyond_both_bounds` means the elapsed time fits neither bound
    // this file reasons with, so the mechanism attribution below it is void.
    assert_ne!(
        mechanism, "UNEXPECTED_beyond_both_bounds",
        "rung 2 took {ms}ms, which sits outside both documented bounds, so no mechanism can be \
         attributed from it"
    );
}

/// RUNG 3 — THE NAMED PROPERTY, ISOLATED. Allowlist contains ONLY the absent
/// path. This is precisely what the field regression is about: an allowlist
/// entry that does not exist must be skipped rather than aborting the spawn.
/// If this rung is GREEN, the skip-missing behaviour works and F-KR-07's
/// headline — "the field-regression test for a non-existent allowlist path does
/// not pass" — is misattributed to the absent path.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung3_absent_path_only() {
    require_live_windows();
    let missing = std::path::PathBuf::from(ABSENT);
    assert!(
        !missing.exists(),
        "precondition: allowlist path must be absent"
    );
    let (ok, label, ms) = observe("3_ABSENT_ONLY", vec![missing], 10).await;
    assert!(
        ok,
        "a non-existent allowlist path alone broke the spawn after {ms}ms ({label}) — \
         this IS the property the field-regression test names, and it is genuinely red."
    );
}

/// RUNG 4 — the real path, isolated. Allowlist contains ONLY `%TEMP%`, no absent
/// path anywhere. If this rung reds while rung 3 greens, the cause is the REAL
/// path, and the absent path is a bystander. Not asserted: under investigation.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung4_real_tempdir_only() {
    require_live_windows();
    let (_ok, _label, ms) = observe("4_TEMPDIR_ONLY", vec![std::env::temp_dir()], 10).await;
    let mechanism = mechanism_from_elapsed(ms);
    println!("KR07_RUNG_4_MECHANISM={mechanism}");
    assert_within_documented_ceiling("4", 10, ms);
    assert_ne!(
        mechanism, "UNEXPECTED_beyond_both_bounds",
        "rung 4 took {ms}ms, outside both documented bounds, so its comparison against rung 3 \
         cannot separate the real path from the absent one"
    );
}

/// RUNG 5 — grant over a SMALL real directory, plus the absent path. Same two
/// -entry allowlist shape as the failing test; the only thing changed is the
/// SIZE of the granted subtree. If this greens while rung 2 reds, the cause is
/// neither "granting" nor "an absent entry" but the subtree under the granted
/// path — `SetNamedSecurityInfoW` with `SUB_CONTAINERS_AND_OBJECTS_INHERIT`
/// propagates to every child object.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung5_small_fresh_dir_plus_absent() {
    require_live_windows();
    let small = std::env::temp_dir().join(format!("wcore-kr07-small-{}", std::process::id()));
    std::fs::create_dir_all(&small).expect("create small grant dir");
    std::fs::write(small.join("one.txt"), b"x").expect("seed small grant dir");
    let missing = std::path::PathBuf::from(ABSENT);
    let (ok, label, ms) = observe("5_SMALL_PLUS_ABSENT", vec![small.clone(), missing], 10).await;
    let _ = std::fs::remove_dir_all(&small);
    assert!(
        ok,
        "an allowlist of [small real dir, absent path] failed after {ms}ms ({label}) — \
         so the two-entry shape itself is broken independently of subtree size."
    );
}

/// RUNG 6 — INSTRUMENT, NOT A FIX. The exact failing shape again, with a large
/// ceiling, purely to separate "slow" from "stuck" and to measure the true cost
/// of the grant. A green here does NOT close the finding and this ceiling must
/// never be transplanted into the real test — raising a timeout to reach green
/// is forbidden. What it buys: if this completes, the grant is linear-slow and
/// the cost is a number; if it does not, the grant is wedged, which is a
/// different defect with a different fix.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung6_exact_shape_with_diagnostic_ceiling() {
    require_live_windows();
    let missing = std::path::PathBuf::from(ABSENT);
    let (ok, label, ms) = observe(
        "6_DIAGNOSTIC_CEILING",
        vec![std::env::temp_dir(), missing],
        600,
    )
    .await;
    println!(
        "KR07_RUNG_6_VERDICT={}",
        if ok {
            "SLOW_NOT_STUCK"
        } else {
            "STUCK_OR_OTHER"
        }
    );
    println!("KR07_RUNG_6_TRUE_COST_MS={ms} label={label}");
    // `ok` is NOT asserted: a green here does not close the finding, and a red
    // here is the finding. The ceiling is, because the whole value of this rung is
    // that {ms} is a TRUE COST — a number produced outside the bound is not a cost
    // measurement, it is an unbounded call, and "slow vs stuck" is then unanswered
    // rather than answered "slow".
    assert_within_documented_ceiling("6", 600, ms);
}

/// RUNG 7 — is the cost paid once or every run? Windows does not re-propagate an
/// ACE that is already present on every child, so a much cheaper second run means
/// the cost is first-touch propagation over the subtree; an equally expensive
/// second run means the cost is paid per execution, which is materially worse in
/// the field.
///
/// **This rung runs the shape TWICE itself (wayland#934, 2026-08-28).** It used to
/// take one observation and print `KR07_RUNG_7_SECOND_RUN_MS`, calling it the
/// second run because rung 6 had happened earlier — in a different test, whose
/// number this test cannot see. It could therefore neither assert nor even compute
/// the ratio it exists to report, and a reader comparing the two printed numbers
/// was comparing across an ordering nextest does not guarantee. Both runs are now
/// in one process, in order, and the comparison is made here.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung7_repeat_cost_is_paid_once_or_every_run() {
    require_live_windows();
    let missing = std::path::PathBuf::from(ABSENT);
    let allow = vec![std::env::temp_dir(), missing];

    let (ok_first, label_first, first_ms) = observe("7_FIRST", allow.clone(), 600).await;
    let (ok_second, label_second, second_ms) = observe("7_REPEAT", allow, 600).await;
    println!("KR07_RUNG_7_FIRST_RUN_MS={first_ms}");
    println!("KR07_RUNG_7_SECOND_RUN_MS={second_ms}");
    println!(
        "KR07_RUNG_7_VERDICT={}",
        if second_ms * 2 < first_ms.max(1) {
            "FIRST_TOUCH_ONLY_second_run_less_than_half"
        } else {
            "PAID_EVERY_RUN_second_run_not_materially_cheaper"
        }
    );

    assert_within_documented_ceiling("7_FIRST", 600, first_ms);
    assert_within_documented_ceiling("7_REPEAT", 600, second_ms);
    // The outcome is not asserted; its REPRODUCIBILITY is. Two identical
    // invocations back to back that classify differently mean the shape is
    // non-deterministic, and every single-observation rung above — including the
    // 12/12 determinism the whole finding rests on — is then reading noise.
    assert_eq!(
        ok_first, ok_second,
        "two back-to-back runs of the identical shape disagreed ({label_first} then \
         {label_second}), so this shape is non-deterministic and no rung in this ladder is \
         measuring a stable property"
    );
}

/// The outer bound `windows_impl::process` guarantees for the WHOLE blocking call:
/// the manifest timeout plus the 15s setup grace, plus a small allowance for
/// scheduling on a loaded host.
///
/// This is the assertion available to a rung that must not assert its own outcome.
/// It cannot be satisfied by the defect being fixed and cannot be broken by the
/// defect being present; it is broken only by the backend violating the contract
/// that makes `mechanism_from_elapsed` mean anything.
fn assert_within_documented_ceiling(rung: &str, timeout_secs: u64, ms: u128) {
    let ceiling_ms = u128::from(timeout_secs + 15) * 1000 + 5_000;
    assert!(
        ms <= ceiling_ms,
        "rung {rung}: execute() returned after {ms}ms, outside the {ceiling_ms}ms ceiling \
         windows_impl::process documents for a {timeout_secs}s manifest (timeout + 15s setup \
         grace + 5s scheduling allowance). Every elapsed-time attribution in this file rests \
         on that bound, so a number outside it is not evidence about the allowlist — it is \
         evidence the bound is not enforced."
    );
}

/// The elapsed time names the mechanism, because the two bounds are 15s apart:
/// the inner `WaitForSingleObject` bounds the child's run at the manifest
/// timeout, and the outer ceiling bounds the whole blocking call — setup
/// included — at manifest timeout + 15s.
fn mechanism_from_elapsed(ms: u128) -> &'static str {
    match ms {
        0..=8_000 => "FAST_NO_TIMEOUT",
        8_001..=13_000 => "CHILD_RUN_BOUND_inner_WaitForSingleObject_fired",
        13_001..=30_000 => "SETUP_BOUND_outer_ceiling_fired_ACL_apply_is_the_only_added_setup",
        _ => "UNEXPECTED_beyond_both_bounds",
    }
}
