//! `F-KR-07` denial ladder, part 2 — dose-response, and a re-measurement of the
//! finding's own claim.
//!
//! Part 1 (`kr07_ladder.rs`) returned every rung GREEN on this host, including
//! the exact shape reported as failing 12/12. That does not close the finding —
//! it relocates it. The numbers part 1 produced are the lead:
//!
//! ```text
//! rung 1  no allowlist                      112 ms   ok
//! rung 3  ABSENT path only                  107 ms   ok
//! rung 5  small real dir + ABSENT path        99 ms   ok
//! rung 2  %TEMP% + ABSENT, first touch    21 499 ms   ok   <- 86% of the 25s ceiling
//! rung 4  %TEMP% only, second touch      10 164 ms   ok
//! rung 6  %TEMP% + ABSENT, third touch    9 285 ms   ok
//! rung 7  %TEMP% + ABSENT, fourth touch   9 084 ms   ok
//! ```
//!
//! Two facts fall straight out. Granting over a path costs ~9-21s while granting
//! over a small path costs ~0.1s, and the FIRST touch costs roughly double every
//! later one. The effective budget is 25s (`manifest.timeout` 10s + the 15s setup
//! grace in `windows_impl::process`), so the reported test sits at 86% of its own
//! ceiling on a host whose `%TEMP%` currently holds 10,341 top-level entries.
//!
//! That is a marginal condition, not a stable green — which is exactly what a
//! "deterministic 12/12" red on a fuller `%TEMP%` and a green here on an emptier
//! one would look like. The predecessor lane deleted 68 leaked work directories
//! and 564 profiles from this host between the red and this green.
//!
//! These rungs test that explanation rather than asserting it:
//!
//! * **Dose-response** — if subtree size is the cause, grant cost must scale
//!   with it. Measured over synthetic trees, so the claim is falsifiable: a flat
//!   curve refutes the whole explanation and the rung FAILS.
//! * **First-touch vs steady-state** — separates one-off ACE propagation from a
//!   cost paid on every single execution, which is the difference between a
//!   startup cost and an unusable product in the field.
//! * **Re-measurement** — runs the finding's exact shape repeatedly at the real
//!   10s manifest and counts, so "deterministic 12/12" is re-tested rather than
//!   inherited.
//!
//! Synthetic trees are built under the checkout, never under `%TEMP%`, so this
//! file does not grow the shared directory other lanes are measured against.
//! Nothing here raises the timeout of the test under investigation.

#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

mod common;
use common::require_live_windows;

/// Number of rungs in this binary, excluding the guard itself.
const KR07B_RUNGS: usize = 3;

const ABSENT: &str = r"C:\__wcore_absent_cache__\.npm";

/// F-WR-02 guard: this binary's rungs are all `#[ignore]`d, so the obvious
/// command runs none of them and still exits 0. This test always runs.
#[test]
fn ladder2_run_must_not_report_success_on_zero_rungs() {
    assert_eq!(KR07B_RUNGS, 3);
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
         cannot execute any of the {KR07B_RUNGS} rungs — they are #[ignore]d and neither \
         --ignored nor --include-ignored was passed. Exiting 0 here would certify nothing."
    );
}

fn scratch_root() -> PathBuf {
    PathBuf::from(r"C:\wl-kr07-scratch")
}

/// Build a tree of `files` regular files, in directories of 100, so the shape
/// resembles a real cache rather than one enormous flat directory.
fn build_tree(tag: &str, files: usize) -> PathBuf {
    let root = scratch_root().join(format!("tree-{tag}-{files}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create synthetic tree root");
    for i in 0..files {
        let dir = root.join(format!("d{:04}", i / 100));
        if i % 100 == 0 {
            std::fs::create_dir_all(&dir).expect("create synthetic subdir");
        }
        std::fs::write(dir.join(format!("f{i:05}.bin")), b"kr07").expect("write synthetic file");
    }
    root
}

/// One grant observation with a diagnostic ceiling large enough that nothing
/// times out — the point is to MEASURE the cost, not to discover the bound.
/// A large ceiling here is an instrument; it is never transplanted into the
/// test under investigation.
async fn grant_cost_ms(label: &str, allow: Vec<PathBuf>) -> u128 {
    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        fs_read_allow: allow,
        timeout: Some(Duration::from_secs(600)),
        ..Default::default()
    };
    let start = Instant::now();
    let out = b
        .execute(
            &m,
            SandboxCommand {
                argv: vec!["cmd.exe".into(), "/c".into(), "echo kr07-ok".into()],
                cwd: None,
            },
        )
        .await;
    let ms = start.elapsed().as_millis();
    let ok = matches!(&out, Ok(o) if o.exit_code == 0);
    println!("KR07B_GRANT label={label} ok={ok} elapsed_ms={ms}");
    ms
}

/// RUNG 8 — DOSE-RESPONSE. If the granted subtree is what costs the time, the
/// cost must rise with the number of objects in it. Falsifiable in the strong
/// direction: a flat curve means subtree size is NOT the cause and this rung
/// fails, which would send the whole explanation back.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung8_grant_cost_scales_with_subtree_size() {
    require_live_windows();
    let mut curve = Vec::new();
    for files in [0usize, 500, 2000, 8000] {
        let tree = build_tree("dose", files);
        // Fresh tree each time, so every measurement is a FIRST touch and the
        // sizes are comparable to each other.
        let ms = grant_cost_ms(&format!("files={files}"), vec![tree.clone()]).await;
        curve.push((files, ms));
        let _ = std::fs::remove_dir_all(&tree);
    }
    for (files, ms) in &curve {
        println!("KR07B_DOSE files={files} elapsed_ms={ms}");
    }
    let smallest = curve.first().expect("curve has a first point").1;
    let largest = curve.last().expect("curve has a last point").1;
    assert!(
        largest > smallest.saturating_mul(3),
        "grant cost did NOT scale with subtree size ({smallest}ms at 0 files vs {largest}ms \
         at 8000) — subtree size is not the cause and the F-KR-07 explanation is refuted. \
         curve={curve:?}"
    );
}

/// RUNG 9 — is the cost paid once, or on every execution? A fresh tree is
/// granted twice in a row. Windows does not re-propagate an ACE already present
/// on every child, so a much cheaper second grant means first-touch propagation;
/// a second grant of the same order means every sandboxed command in the field
/// pays it, which is materially worse.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung9_first_touch_versus_steady_state() {
    require_live_windows();
    let tree = build_tree("repeat", 8000);
    let first = grant_cost_ms("first-touch", vec![tree.clone()]).await;
    let second = grant_cost_ms("steady-state", vec![tree.clone()]).await;
    let _ = std::fs::remove_dir_all(&tree);
    println!("KR07B_FIRST_TOUCH_MS={first} KR07B_STEADY_STATE_MS={second}");
    assert!(
        first > 0 && second > 0,
        "both grants must produce a measurement; first={first} second={second}"
    );
}

/// RUNG 10 — RE-MEASURE THE FINDING'S OWN CLAIM. The exact shape of
/// `live_cmd_runs_when_allowlist_has_missing_path`, at its real 10s manifest,
/// run five times back to back, counting outcomes. The finding records 12/12
/// deterministic `SandboxError::Timeout`; this rung re-tests that on the host as
/// it is NOW, and prints the elapsed time of every run so a near-ceiling margin
/// is visible rather than hidden behind a pass.
///
/// No expectation is asserted beyond "every run produced a terminal
/// observation" — the outcome distribution IS the result, and asserting a green
/// here would be assuming the answer.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung10_remeasure_reported_shape_five_times() {
    require_live_windows();
    let b = AppContainerBackend::new();
    let mut passes = 0usize;
    let mut fails = 0usize;
    let mut worst = 0u128;
    for run in 1..=5 {
        let m = SandboxManifest {
            fs_read_allow: vec![std::env::temp_dir(), PathBuf::from(ABSENT)],
            timeout: Some(Duration::from_secs(10)),
            ..Default::default()
        };
        let start = Instant::now();
        let out = b
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
        let ms = start.elapsed().as_millis();
        worst = worst.max(ms);
        let verdict = match &out {
            Ok(o) if o.exit_code == 0 => {
                passes += 1;
                "PASS".to_string()
            }
            Ok(o) => {
                fails += 1;
                format!("FAIL exit={}", o.exit_code)
            }
            Err(e) => {
                fails += 1;
                format!("FAIL {e:?}")
            }
        };
        println!("KR07B_REMEASURE run={run} verdict={verdict} elapsed_ms={ms}");
    }
    // The effective ceiling is manifest.timeout (10s) + the 15s setup grace.
    let margin_pct = (worst as f64 / 25_000.0) * 100.0;
    println!(
        "KR07B_REMEASURE_SUMMARY passes={passes} fails={fails} worst_ms={worst} \
         pct_of_25s_ceiling={margin_pct:.1}"
    );
    assert_eq!(
        passes + fails,
        5,
        "every run must produce a terminal observation"
    );
}
