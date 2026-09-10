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
//!
//! gh#1284, 2026-09-10: the timeout test no longer grades a wall-clock ratio —
//! see its own doc block. The v0.13.4 red arm is UNCHANGED in kind but is now
//! carried by two different assertions: that table's "timeout message names a
//! cause: no" row is the first of them, and at v0.13.4 the child ran after the
//! walk, so the token assertion reds there too. NOT RE-RUN at v0.13.4 in this
//! pass — that re-verification is owed, and is recorded as owed rather than
//! claimed.

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

/// A token only the CHILD can put into the result, so "the command never ran"
/// is graded by an observation rather than by a duration.
///
/// `echo`ing it is the whole child. If it is absent the child produced no
/// output; if it is present the child ran. Deliberately not a word that occurs
/// anywhere in the timeout message — "hi" would have matched `while`.
const CHILD_TOKEN: &str = "MANIFEST_BOUND_CHILD_RAN_a1b2";

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
///
/// REWRITTEN for gh#1284. The previous version decided on
/// `bounded * 3 < walk`: a RATIO between two wall-clock samples taken at
/// different moments — the walk measured inside the growth helper, and the
/// timed call afterwards. That ratio only holds if load is comparable across
/// the two samples, and on a hosted macOS runner it is not: the allowlist
/// records one attempt at 3.434s where the Linux control ran the same bytes at
/// ~4.5ms against a ~42ms walk, 0/60 at `--retries 0`. The ratio was therefore
/// grading the runner, and it flaked on macOS in two consecutive runs
/// (33437649161, 33462025536).
///
/// Nothing here is timed any more. The bound is graded as an EVENT and the
/// "no child ran" half as an OBSERVATION:
///
/// * the manifest-named timeout string is emitted at exactly ONE site — the
///   `Err(_)` arm of `timeout_at(deadline, build)` in `bash.rs` — so its
///   presence IS the proof that the deadline fired while the manifest build was
///   still outstanding, which is the property this test is named for. A bare
///   "Command timed out after Nms" is byte-identical to the CHILD-timeout
///   return, which is why `manifest` and not just `timed out` is required.
/// * `CHILD_TOKEN` can only reach the result through the child's stdout, so its
///   ABSENCE is "the command never ran" without reference to any clock — and
///   the second call below is its POSITIVE CONTROL: the same command in the
///   same posture on the same policy does put the token in the result once the
///   budget is not the constraint, so the absence above is attributable to the
///   timeout and not to a token that never appears.
/// * `secret_deny_walk_count` is the injected counter `workspace_policy.rs`
///   documents for exactly this ("a wall clock cannot tell a skipped walk from
///   a fast one"), and it keeps the grade from passing vacuously on a policy
///   that never walked at all.
///
/// Load can still only make the walk SLOWER, which makes the deadline fire
/// during the build MORE reliably. The remaining failure direction is one-sided
/// and is the one that means the product regressed.
#[tokio::test]
async fn the_live_backend_timeout_bounds_the_manifest_build_and_names_it() {
    if !enforcing_host() {
        return;
    }
    warm_process_init().await;
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (policy, walk) = workspace_whose_walk_costs_at_least(&root, TARGET);

    let ctx = ctx_for(Arc::clone(&policy));
    // Derived from the walk just measured, not pinned: a literal that is small
    // against today's walk becomes large against a faster one, and the
    // assertion would then pass for the wrong reason. The derived number sets
    // up the condition; it is no longer part of what is asserted.
    let timeout_ms = (walk / 10).as_millis().max(1) as u64;
    let command = format!("echo {CHILD_TOKEN}");

    let result = BashTool
        .execute_with_ctx(
            json!({"command": command.as_str(), "timeout": timeout_ms}),
            &ctx,
        )
        .await;

    println!(
        "timeout: budget={timeout_ms}ms walk={walk:?} msg={:?}",
        result.content
    );
    // THE BOUND, as an event. `contains("timed out")` alone is satisfied by the
    // byte-identical string the CHILD-timeout path returns, so it would grade
    // nothing here; `manifest` is what pins the return to the build's own arm.
    assert!(
        result.content.contains("timed out") && result.content.contains("manifest"),
        "the caller was not told the workspace secret-scan ate the budget and \
         that no child ran; got: {}",
        result.content
    );
    // NO CHILD RAN, as an observation. Positive-controlled below.
    assert!(
        !result.content.contains(CHILD_TOKEN),
        "the child produced output after a timeout that claims it never ran, so \
         the manifest build was not what the deadline cut; got: {}",
        result.content
    );

    // POSITIVE CONTROL for the assertion above, and the anti-vacuity check for
    // the whole test: the same command, same policy, same posture, with the
    // default budget instead of a tenth of the walk.
    let ran = BashTool
        .execute_with_ctx(json!({"command": command.as_str()}), &ctx)
        .await;
    println!("control: msg={:?}", ran.content);
    assert!(
        ran.content.contains(CHILD_TOKEN),
        "control is broken: the child never emits {CHILD_TOKEN} even without a \
         tight budget, so its absence above graded nothing; got: {}",
        ran.content
    );
    // Read AFTER the control call, never after the timed one: the timed call's
    // build is detached on the blocking pool and may not have entered the walk
    // by the time the deadline fires, which would be a load-sensitive read of
    // exactly the kind this rewrite removes. The control returned, so its
    // manifest build completed, so the deny set was produced — by a fresh walk
    // or from the memo a completed walk left behind. Either way this is >= 1.
    assert!(
        policy.secret_deny_walk_count() >= 1,
        "no deny walk was ever entered on this policy, so the timeout above \
         bounded a manifest build that had nothing to bound"
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
