//! #1111 bullets 2 and 3, graded against the REAL platform sandbox backend.
//!
//! `bash::tests::{a_cancelled_bash_does_not_wait_for_the_secret_deny_walk,
//! the_bash_timeout_bounds_the_secret_deny_walk}` and their streaming twins
//! grade the same two bullets against `CannedBackend::enforcing()` — a test
//! double whose `enforces_read_deny()` returns a literal. That grades the
//! FUNCTION. It cannot notice if the wiring changes: if the platform default
//! stopped enforcing read-deny, or the exec-time capability gate above the
//! build started refusing, or `spawn_manifest_build` stopped being handed the
//! same backend handle that runs the child, every canned test stays green while
//! the shipped product does something else.
//!
//! This file grades the WIRING. It builds a `ToolContext` exactly as
//! `test_default()` does — `default_for_platform()` inside a real
//! `SandboxRegistry`, which on this host is bubblewrap — and asserts the
//! reported symptom is absent through that path.
//!
//! Provenance: these assertions are the reproduction used to observe the defect
//! before fixing anything. Same bytes, run at `0ccaa90b` (v0.13.4) and
//! `addb4f48` (v0.13.5) on hetzner-dsm, live bwrap:
//!
//! | measurement                    | v0.13.4        | v0.13.5   |
//! |--------------------------------|----------------|-----------|
//! | Esc during the manifest build  | 132.66 ms      | 238.85 us |
//! | timeout, elapsed               | 144.21 ms      | 5.43 ms   |
//! | ...against a walk of           | 68.77 ms       | 40.76 ms  |
//! | timeout message names a cause  | no             | yes       |
//!
//! Both assertions below FAIL at v0.13.4 and pass at v0.13.5, so they are a
//! real gate on a real regression rather than a description of today.

use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The walk cost the trees here are sized to.
///
/// RECALIBRATED once, and the first value is recorded rather than dropped: 150
/// ms was reached on the pre-fix SERIAL walk in ~2 growth batches but is NOT
/// reachable at all on the released PARALLEL walk inside the 240k-entry cap —
/// every test died in the helper on the released arm. A target only one arm can
/// reach is not one instrument, it is two.
const TARGET: Duration = Duration::from_millis(40);

/// Grow `root` until its secret-deny walk costs at least `target`; return the
/// policy and the SMALLEST of three warm samples.
///
/// Smallest-of-three, not first: one sample can hit the target because this
/// 96-core host stalled, and the tree then stays small while callers derive
/// their budget from a cost the walk does not really have.
/// Grow `root` until ONE COLD walk of it costs at least `target`, and hand back
/// a policy that has not walked yet.
///
/// EVERY measurement here is taken on a FRESH `WorkspacePolicy`, and the policy
/// returned is fresh too. That is not tidiness — it is what makes the number
/// mean anything now that #1111 memoises the deny walk
/// (`WorkspacePolicy::deny_cache_key` / `deny_cache_hit`).
///
/// This helper used to keep one policy for the whole loop and take the MINIMUM
/// of three back-to-back walks as its estimate. With the memo in the tree the
/// second and third of those are cache hits costing microseconds, so the
/// minimum collapsed to ~0, the target was never reached, and all three tests
/// in this file died on "could not grow a workspace whose walk costs 40ms
/// within 240k entries" — a calibration failure reported as a product failure.
/// A cheap-arm-of-a-repeat is the wrong instrument whenever a cache exists; the
/// cold arm is the one the product actually pays on a first exec, and it is the
/// one the timeout under test has to cut.
///
/// Returning a never-walked policy matters for the same reason: a warm policy
/// handed to the call under test would answer from the memo, and the test would
/// be timing a cache lookup instead of the walk it claims to bound.
fn workspace_whose_walk_costs_at_least(
    root: &std::path::Path,
    target: Duration,
) -> (Arc<WorkspacePolicy>, Duration) {
    std::fs::write(root.join(".env"), b"TOKEN=hunter2\n").unwrap();

    // One cold walk on a policy that has never walked before.
    let cold_walk = |root: &std::path::Path| -> Duration {
        let policy = WorkspacePolicy::contained(root);
        let started = Instant::now();
        let deny = policy.secret_deny_paths_for_backend(true);
        let walk = started.elapsed();
        // KNOWN-POSITIVE CONTROL on the instrument. If the walk stopped finding
        // the planted `.env`, a cheap `walk` would mean "the walk was skipped",
        // not "the walk is fast", and every latency claim here would be vacuous.
        assert!(
            deny.iter().any(|p| p.ends_with(".env")),
            "control: the contained walk must find the planted .env; got {deny:?}"
        );
        walk
    };

    for batch in 0..24usize {
        // Worst of three COLD walks, not the best of three warm ones. The
        // quantity is compared against a floor, so an under-estimate only ever
        // grows the tree further; taking the max keeps a single scheduling
        // hiccup from ending the loop early on a tree that is really too small.
        let mut best = Duration::ZERO;
        for _ in 0..3 {
            best = best.max(cold_walk(root));
        }
        if best >= target {
            return (Arc::new(WorkspacePolicy::contained(root)), best);
        }
        for d in 0..100 {
            let dir = root.join(format!("b{batch}")).join(format!("d{d}"));
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..100 {
                std::fs::write(dir.join(format!("f{f}.txt")), b"x").unwrap();
            }
        }
    }
    panic!("could not grow a workspace whose walk costs {target:?} within 240k entries");
}

fn ctx_for(policy: Arc<WorkspacePolicy>) -> ToolContext {
    let mut ctx = ToolContext::test_default();
    ctx.workspace = Some(policy);
    ctx
}

/// True when this host's platform default enforces OS read-deny, which is the
/// only configuration in which the walk under test runs at all.
///
/// Linux `bwrap` and macOS `sandbox_exec` hardcode `true`; the Windows session
/// default `windows_job_object` keeps the trait default `false` and the #922
/// gate skips the walk entirely there. Reported, never silently skipped: a test
/// that opts out without saying so reads exactly like a test that passed.
fn enforcing_host() -> bool {
    let ctx = ToolContext::test_default();
    if !ctx.sandbox.enforces_read_deny() {
        println!(
            "SKIP: backend {} does not enforce read-deny, so the #922 gate skips \
             the walk and there is no manifest-build cost to bound on this host",
            ctx.sandbox.backend_name()
        );
        return false;
    }
    println!(
        "host backend = {} (enforces read-deny)",
        ctx.sandbox.backend_name()
    );
    true
}

/// Pay one-time process initialisation before anything is timed.
///
/// nextest runs every test in its OWN process, so each test would otherwise pay
/// cold init inside the window it measures. Measured on hetzner-dsm: 24.18 ms
/// for a pre-cancelled `echo hi` against an EMPTY contained workspace — with
/// nothing whatsoever to walk. That is larger than the walk under test and
/// dominated every number here until it was moved outside the clock; it is the
/// reason the first run of this file reported a symptom that was not there.
/// `trusted_local` on an empty dir initialises the process without walking.
async fn warm_process_init() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let ctx = ctx_for(Arc::new(WorkspacePolicy::trusted_local(&root)));
    let _ = BashTool
        .execute_with_ctx(json!({"command": "echo warm"}), &ctx)
        .await;
}

/// The host's cost for one `tokio` timer wait of `ms`, with nothing else in
/// flight, so a few-millisecond budget is not graded against timer granularity.
async fn timer_allowance(ms: u64) -> Duration {
    let mut worst = Duration::ZERO;
    for _ in 0..5 {
        let s = Instant::now();
        let _ = tokio::time::timeout(Duration::from_millis(ms), std::future::pending::<()>()).await;
        worst = worst.max(s.elapsed().saturating_sub(Duration::from_millis(ms)));
    }
    worst
}

/// #1111 bullet 2 — "Esc cancels during manifest construction" — through the
/// real platform backend.
#[tokio::test]
async fn esc_during_the_live_backend_manifest_build_does_not_wait_for_the_walk() {
    if !enforcing_host() {
        return;
    }
    warm_process_init().await;
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (policy, walk) = workspace_whose_walk_costs_at_least(&root, TARGET);

    let ctx = ctx_for(policy);
    // Cancelled BEFORE the call, so a correct implementation has nothing to do
    // but return; any time spent is time the user could not interrupt.
    ctx.cancel.cancel();

    let started = Instant::now();
    let result = BashTool
        .execute_with_ctx(json!({"command": "echo hi"}), &ctx)
        .await;
    let elapsed = started.elapsed();

    println!(
        "esc: elapsed={elapsed:?} walk={walk:?} msg={:?}",
        result.content
    );
    assert!(
        result.content.contains("cancelled"),
        "a cancelled command must say so; got: {}",
        result.content
    );
    assert!(
        elapsed * 3 < walk,
        "Esc waited {elapsed:?} for a walk measured at {walk:?} on the live \
         {} backend — the manifest build is outside the cancellation scope",
        ToolContext::test_default().sandbox.backend_name()
    );
}

/// #1111 bullet 3 — the timeout bounds the manifest build AND names it — through
/// the real platform backend.
#[tokio::test]
async fn the_live_backend_timeout_bounds_the_manifest_build_and_names_it() {
    if !enforcing_host() {
        return;
    }
    warm_process_init().await;
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (policy, walk) = workspace_whose_walk_costs_at_least(&root, TARGET);

    let ctx = ctx_for(policy);
    // Derived from the walk just measured, not pinned: a literal that is small
    // against today's walk becomes large against a faster one, and the
    // assertion would then pass for the wrong reason.
    let timeout_ms = (walk / 10).as_millis().max(1) as u64;
    let allowance = timer_allowance(timeout_ms).await;

    let started = Instant::now();
    let result = BashTool
        .execute_with_ctx(json!({"command": "echo hi", "timeout": timeout_ms}), &ctx)
        .await;
    let elapsed = started.elapsed();
    let bounded = elapsed.saturating_sub(allowance);

    println!(
        "timeout: budget={timeout_ms}ms elapsed={elapsed:?} allowance={allowance:?} \
         bounded={bounded:?} walk={walk:?} msg={:?}",
        result.content
    );
    assert!(
        bounded * 3 < walk,
        "a {timeout_ms}ms timeout returned after {elapsed:?} ({bounded:?} above \
         this host's {allowance:?} allowance for one timer wait) against a walk \
         measured at {walk:?} — the manifest build is outside the timeout scope"
    );
    // `contains("timed out")` alone is satisfied by the byte-identical string
    // the CHILD-timeout path returns, so it would grade nothing here.
    assert!(
        result.content.contains("timed out") && result.content.contains("manifest"),
        "the caller was not told the workspace secret-scan ate the budget and \
         that no child ran; got: {}",
        result.content
    );
}

/// NEGATIVE CONTROL for both tests above.
///
/// Same host, same backend, same tree SIZE — but a `trusted_local` posture,
/// where `secret_read_deny_required()` is false and the manifest build never
/// walks. Without this, "the call returned quickly" would not be attributable
/// to the walk being escaped: it could equally mean the tree was too small to
/// cost anything, or that this host is simply fast.
///
/// Measured at v0.13.4 this control took 63.19 ms against a 68.20 ms contained
/// walk — i.e. the pre-fix defect was NOT confined to the secret-deny walk;
/// every part of the inline manifest build was uncancellable. At v0.13.5 it is
/// 204.9 us.
#[tokio::test]
async fn a_non_walking_posture_on_the_same_tree_is_the_negative_control() {
    if !enforcing_host() {
        return;
    }
    warm_process_init().await;
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (contained, walk) = workspace_whose_walk_costs_at_least(&root, TARGET);
    drop(contained);

    let policy = Arc::new(WorkspacePolicy::trusted_local(&root));
    // NOT `is_empty()`: SYSTEM_CREDENTIAL_STORES (/etc/docker, ...) are present
    // on every posture and have nothing to do with walking the project. The
    // question is whether the PROJECT TREE was walked — measured, this is what
    // the first draft of this control got wrong, and it failed for its own
    // reasons rather than the product's.
    let tl = policy.secret_deny_paths_for_backend(true);
    assert!(
        !tl.iter().any(|p| p.starts_with(&root)),
        "control is broken: trusted_local produced project paths ({tl:?}), so it \
         still walks the tree and discriminates nothing"
    );

    let ctx = ctx_for(policy);
    ctx.cancel.cancel();
    let started = Instant::now();
    let result = BashTool
        .execute_with_ctx(json!({"command": "echo hi"}), &ctx)
        .await;
    let elapsed = started.elapsed();

    println!(
        "control: elapsed={elapsed:?} contained_walk={walk:?} msg={:?}",
        result.content
    );
    assert!(
        result.content.contains("cancelled"),
        "a cancelled command must say so; got: {}",
        result.content
    );
    assert!(
        elapsed * 3 < walk,
        "a posture that never walks still took {elapsed:?} against a contained \
         walk of {walk:?} — the promptness the tests above assert would not be \
         attributable to the walk"
    );
}
