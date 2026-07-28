//! `F-KR-07` denial ladder, part 3 — reproduce the reported failure on demand,
//! by varying ONE property.
//!
//! Parts 1 and 2 explained the finding; this part predicts it. Everything below
//! rests on measurements already taken on this host, not on argument:
//!
//! ```text
//! grant+revoke cost vs. subtree size (part 2, rung 8, fresh tree each time)
//!        0 files ->     95 ms
//!      500 files ->    228 ms
//!    2 000 files ->    299 ms
//!    8 000 files ->  1 610 ms          ~0.20 ms per object
//!
//! %TEMP% holds 57 636 recursive entries -> predicted ~10 s
//! measured, five consecutive runs      ->  9 848 / 9 797 / 9 914 / 10 525 / 10 629 ms
//! ```
//!
//! The curve predicts the observed `%TEMP%` cost to within a few percent, so the
//! model is: **the cost of a grant is linear in the number of objects under the
//! granted path, and it is paid on every execution** (`cleanup_locked` calls
//! `revoke_intents`, so the ACE does not persist between runs — the cost is not
//! a one-off).
//!
//! The reported test's effective budget is 25s: `manifest.timeout` of 10s plus
//! the 15s setup grace that `windows_impl::process` adds because the inner
//! `WaitForSingleObject` bounds only the child's RUN, not the Win32 setup before
//! it. At ~0.20 ms per object that budget is exhausted somewhere near 125 000
//! objects.
//!
//! So the model makes a falsifiable prediction: grant over a subtree of ~200 000
//! objects and the SAME command that passes today must fail with exactly
//! `SandboxError::Timeout` — the reported symptom — with nothing else changed.
//! Rung 11 demands that failure and FAILS if it does not arrive; rung 12 is its
//! matched pair, identical in every respect except size.
//!
//! If both rungs land, F-KR-07's red is attributed to the SIZE of the directory
//! the test chose to grant over (`std::env::temp_dir()`, unbounded and shared
//! with every other process on the host) and not to the non-existent allowlist
//! entry the test is named for — which part 1, rung 3 already cleared in 107 ms.

#![cfg(windows)]

use std::path::PathBuf;
use std::time::{Duration, Instant};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::appcontainer::AppContainerBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

mod common;
use common::require_live_windows;

const KR07C_RUNGS: usize = 2;
const ABSENT: &str = r"C:\__wcore_absent_cache__\.npm";

/// Objects in the oversized tree. Chosen from the measured ~0.20 ms/object so
/// the predicted cost (~40s) clears the 25s budget with margin, rather than
/// landing on it.
const OVERSIZED: usize = 200_000;

/// Objects in the matched-pair control.
const SMALL: usize = 200;

#[test]
fn ladder3_run_must_not_report_success_on_zero_rungs() {
    assert_eq!(KR07C_RUNGS, 2);
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
         cannot execute any of the {KR07C_RUNGS} rungs — they are #[ignore]d and neither \
         --ignored nor --include-ignored was passed. Exiting 0 here would certify nothing."
    );
}

fn build_tree(tag: &str, files: usize) -> PathBuf {
    let root = PathBuf::from(r"C:\wl-kr07-scratch").join(format!("tree3-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tree root");
    let start = Instant::now();
    for i in 0..files {
        let dir = root.join(format!("d{:05}", i / 200));
        if i % 200 == 0 {
            std::fs::create_dir_all(&dir).expect("create subdir");
        }
        std::fs::write(dir.join(format!("f{i:06}.bin")), b"kr07").expect("write file");
    }
    println!(
        "KR07C_BUILT tag={tag} files={files} build_ms={}",
        start.elapsed().as_millis()
    );
    root
}

/// Run the reported command shape verbatim — same argv, same 10s manifest, same
/// two-entry allowlist of one real path plus the absent one — over `real`.
async fn reported_shape(real: PathBuf) -> (Result<i32, String>, u128) {
    let b = AppContainerBackend::new();
    let m = SandboxManifest {
        fs_read_allow: vec![real, PathBuf::from(ABSENT)],
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
    let r = match out {
        Ok(o) => Ok(o.exit_code),
        Err(e) => Err(format!("{e:?}")),
    };
    (r, ms)
}

/// RUNG 11 — THE PREDICTION. Same command, same manifest, same allowlist shape
/// as the test reported failing; the ONLY thing changed from rung 12 below is
/// the number of objects under the real allowlist entry. The model says this
/// must fail with `SandboxError::Timeout`. This rung FAILS if it does not, so
/// the explanation is refutable rather than merely plausible.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung11_oversized_subtree_reproduces_the_reported_timeout() {
    require_live_windows();
    let tree = build_tree("oversized", OVERSIZED);
    let (result, ms) = reported_shape(tree.clone()).await;
    println!("KR07C_RUNG_11 files={OVERSIZED} elapsed_ms={ms} result={result:?}");
    let _ = std::fs::remove_dir_all(&tree);
    let err = result.err().unwrap_or_else(|| {
        panic!(
            "PREDICTION REFUTED: granting over {OVERSIZED} objects completed in {ms}ms \
             instead of exhausting the 25s budget. Subtree size does not explain \
             F-KR-07 and the attribution in parts 1-3 must be reconsidered."
        )
    });
    assert!(
        err.contains("Timeout"),
        "granting over {OVERSIZED} objects failed after {ms}ms, but with {err} rather than \
         the reported SandboxError::Timeout — same direction, different mechanism, so the \
         reproduction is NOT exact and must not be reported as one."
    );
}

/// RUNG 12 — THE MATCHED PAIR. Byte-identical to rung 11 except the real
/// allowlist entry holds 200 objects instead of 200 000. A green here with a red
/// above isolates subtree size as the single differing property; a red here
/// would mean something other than size is at work and rung 11 proves nothing.
#[tokio::test(flavor = "current_thread")]
#[ignore = "explicit native Windows AppContainer acceptance"]
async fn rung12_small_subtree_same_shape_passes() {
    require_live_windows();
    let tree = build_tree("small", SMALL);
    let (result, ms) = reported_shape(tree.clone()).await;
    println!("KR07C_RUNG_12 files={SMALL} elapsed_ms={ms} result={result:?}");
    let _ = std::fs::remove_dir_all(&tree);
    assert_eq!(
        result,
        Ok(0),
        "the control failed after {ms}ms — with the matched pair red, rung 11's failure \
         cannot be attributed to subtree size."
    );
}
