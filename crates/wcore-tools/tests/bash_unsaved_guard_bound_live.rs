//! #1142 — the P2b unsaved-work guard is bounded, and REFUSES when its bound
//! expires.
//!
//! `unsaved_shell_refusal` used to run ahead of the point where the caller's
//! timeout was armed (`bash.rs`: guard at :684 / :842, deadline at :741 /
//! :881 at 7066118a), so the guard's own cost sat outside the only clock that
//! could stop it. The same shape behind #1000/#921/#1002 turned one `echo`
//! into an apparent hang.
//!
//! The constraint that makes this non-trivial is that the guard must NOT
//! become skippable by timing out: a version that gives up under time pressure
//! and lets a destructive command through is strictly worse than an unbounded
//! one. So both halves are graded here, and the second is the one that matters:
//!
//! 1. the guard's cost is bounded by the caller's budget;
//! 2. when that bound expires the command is REFUSED and nothing runs — proven
//!    on a tree where the guard, allowed to finish, would have ALLOWED.
//!
//! This grades the WIRING, through `BashTool::execute_with_ctx` and
//! `execute_streaming_with_ctx` on a real `ToolContext`, for the reason
//! `bash_manifest_bound_live_backend.rs` states: a canned unit test of the
//! bounding helper cannot notice a call site that never calls it.

use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// The guard cost the fixture is grown to.
///
/// Large enough that the budget below cannot be confused with tokio timer
/// granularity, small enough that the fixture stays a few hundred files.
const TARGET: Duration = Duration::from_millis(600);

/// The caller budget the guard is graded against.
///
/// Sized from BOTH sides, and both matter:
///
/// * well under `TARGET`, so a guard that runs to completion cannot come in
///   under it — that is what makes expiry certain;
/// * comfortably ABOVE what the manifest build and the child itself cost on a
///   `trusted_local` workspace (0.18 ms and a few ms for `rm` + `touch`), so
///   that a guard which passed on expiry instead of refusing would go on to
///   RUN the command and leave the marker file. Without that headroom the
///   fail-closed test would pass against a pass-on-expiry implementation for
///   the unrelated reason that the child ran out of budget too.
const BUDGET_MS: u64 = 200;

fn git(root: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be on PATH for this test");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A git work tree with `files` committed files under `sub/`, and NOTHING
/// uncommitted anywhere in it.
///
/// Clean is the point. `rm -rf sub` against it makes the guard do all of its
/// work — a walk of every file underneath the operand, then a `check-ignore`
/// and a recorded-blob read for each — and then answer `None`: there is
/// nothing on disk that is in no commit, so the command is allowed. That is
/// what makes it the fail-closed instrument: the ONLY reason a refusal can
/// appear here is that the guard did not get to finish.
fn clean_repo_with(root: &Path, files: usize) {
    std::fs::create_dir_all(root.join("sub")).unwrap();
    git(root, &["init", "-q", "."]);
    git(root, &["config", "user.email", "ci@example.invalid"]);
    git(root, &["config", "user.name", "ci"]);
    for f in 0..files {
        std::fs::write(
            root.join("sub").join(format!("f{f}.txt")),
            format!("committed line {f}\n"),
        )
        .unwrap();
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-qm", "base"]);
}

/// Grow a clean work tree under `root` until ONE guard call on it costs at
/// least `target`, and report that cost.
///
/// Measured warm, which is the conservative direction: warm is the CHEAPEST
/// the guard ever is, so a budget beaten on a warm tree is beaten on a cold
/// one too.
fn tree_whose_guard_costs_at_least(root: &Path, target: Duration) -> (PathBuf, Duration) {
    for files in [128usize, 256, 512, 1024, 2048] {
        let tree = root.join(format!("t{files}"));
        clean_repo_with(&tree, files);
        // KNOWN-POSITIVE CONTROL on the instrument, in the same call as the
        // measurement: the guard must SEE this tree and answer "allowed". If
        // it started returning early — a marker probe that stopped finding
        // `.git`, an `rm` parse that stopped recognising the operand — a cheap
        // number would mean "the guard was skipped", not "the guard is fast",
        // and every latency claim below would be vacuous. The dirty twin
        // pins the other polarity.
        let started = Instant::now();
        let verdict = wcore_tools::unsaved_work::shell_refusal("rm -rf sub", &tree);
        let cost = started.elapsed();
        assert!(
            verdict.is_none(),
            "control: a clean tree must be ALLOWED by the guard, else the \
             fail-closed test below cannot tell a bound from a finding; got {verdict:?}"
        );
        if cost >= target {
            return (tree, cost);
        }
    }
    panic!("could not grow a work tree whose guard costs {target:?} within 2048 files");
}

/// The opposite polarity of the control above: on the SAME shape of tree with
/// one uncommitted line, the guard must refuse. Without this, "clean trees are
/// allowed" is equally consistent with a guard that allows everything.
#[test]
fn control_the_guard_refuses_the_same_command_when_a_line_is_unsaved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    clean_repo_with(&root, 8);
    assert!(
        wcore_tools::unsaved_work::shell_refusal("rm -rf sub", &root).is_none(),
        "clean tree must be allowed"
    );
    std::fs::write(root.join("sub/f0.txt"), "committed line 0\nuser typed this\n").unwrap();
    let verdict = wcore_tools::unsaved_work::shell_refusal("rm -rf sub", &root);
    assert!(
        verdict.is_some(),
        "control: an unsaved line under the operand must be refused"
    );
}

fn ctx_for(root: &Path) -> ToolContext {
    // `trusted_local` keeps the #922 secret-deny walk out of the measurement:
    // it is the other pre-child cost, it is already bounded by #1111, and a
    // posture that pays it would put its cost inside numbers that claim to be
    // about the guard.
    let mut ctx = ToolContext::test_default();
    ctx.workspace = Some(Arc::new(WorkspacePolicy::trusted_local(root)));
    ctx
}

/// Pay one-time process initialisation before anything is timed. nextest runs
/// each test in its own process, so cold init would otherwise land inside the
/// measured window — the effect `bash_manifest_bound_live_backend.rs` measured
/// at 24.18 ms with nothing whatsoever to walk.
async fn warm_process_init(root: &Path) {
    let ctx = ctx_for(root);
    let _ = BashTool
        .execute_with_ctx(json!({"command": "echo warm"}), &ctx)
        .await;
}

/// #1142 acceptance 1 — the guard's cost is bounded by the caller's budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_caller_budget_bounds_the_unsaved_work_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_process_init(&root).await;
    let (tree, cost) = tree_whose_guard_costs_at_least(&root, TARGET);

    let ctx = ctx_for(&tree);
    let started = Instant::now();
    let result = BashTool
        .execute_with_ctx(
            json!({"command": "rm -rf sub", "timeout": BUDGET_MS}),
            &ctx,
        )
        .await;
    let elapsed = started.elapsed();

    println!("bound: guard cost={cost:?} elapsed={elapsed:?} msg={:?}", result.content);
    assert!(
        elapsed * 2 < cost,
        "the guard cost {cost:?} and the call took {elapsed:?} against a {BUDGET_MS}ms \
         budget — the guard is running outside the caller's clock"
    );
    assert!(result.is_error, "an expired guard must be an error result");
}

/// #1142 acceptance 2, THE one that matters — expiry REFUSES.
///
/// The tree is clean, so a guard allowed to finish returns "allowed" and the
/// command runs. Under a budget it cannot meet, the command must be refused
/// and must not have run. A fix that bounded the guard by letting it pass on
/// expiry would turn this test red, which is the whole reason it exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_expired_unsaved_work_guard_refuses_instead_of_passing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_process_init(&root).await;
    let (tree, cost) = tree_whose_guard_costs_at_least(&root, TARGET);

    let ctx = ctx_for(&tree);
    let result = BashTool
        .execute_with_ctx(
            // The `rm` is what the guard is slow about; the `touch` is the
            // side effect that says whether the shell ever ran. Nothing is
            // asserted from the command's own output, which a refusal shapes.
            json!({"command": "rm -rf sub && touch it-ran", "timeout": BUDGET_MS}),
            &ctx,
        )
        .await;

    println!("fail-closed: guard cost={cost:?} msg={:?}", result.content);
    assert!(
        !tree.join("it-ran").exists(),
        "the shell RAN despite the unsaved-work guard not finishing: {}",
        result.content
    );
    assert!(
        tree.join("sub").exists(),
        "the shell removed the work tree despite the guard not finishing"
    );
    assert!(
        result.is_error,
        "a command that did not run must be reported as an error, got: {}",
        result.content
    );
    assert!(
        result.content.contains("unsaved-work check did not finish"),
        "the refusal must name the guard as the cause, not read like a child \
         timeout; got: {}",
        result.content
    );
}

/// The streaming ctx path is the second live call site. #1111's own note says
/// fixing one and leaving the other is the mistake available here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_streaming_path_refuses_on_expiry_too() {
    struct Silent;
    impl wcore_tools::ToolOutputSink for Silent {
        fn emit_chunk(&self, _: &str) {}
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_process_init(&root).await;
    let (tree, cost) = tree_whose_guard_costs_at_least(&root, TARGET);

    let ctx = ctx_for(&tree);
    let result = BashTool
        .execute_streaming_with_ctx(
            json!({"command": "rm -rf sub && touch it-ran", "timeout": BUDGET_MS}),
            &ctx,
            &Silent,
        )
        .await;

    println!("fail-closed (streaming): guard cost={cost:?} msg={:?}", result.content);
    assert!(
        !tree.join("it-ran").exists(),
        "the streaming path RAN the shell despite the guard not finishing: {}",
        result.content
    );
    assert!(
        result.content.contains("unsaved-work check did not finish"),
        "the streaming refusal must name the guard; got: {}",
        result.content
    );
}
