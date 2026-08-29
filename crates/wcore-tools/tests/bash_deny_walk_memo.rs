//! #1111 acceptance 1 — the per-exec secret-deny workspace walk must not be
//! repeated for every `Bash` execution.
//!
//! Graded end-to-end through `BashTool`, at BOTH ctx-aware call sites
//! (`execute_with_ctx` and `execute_streaming_with_ctx`), because the defect is
//! per-call-site and a fix applied to one of them leaves the other live.
//!
//! Deliberately NOT measured in a fresh temp dir: an empty tree walks in ~0.1 ms
//! and hides the defect completely. Every test here first GROWS a workspace
//! until its cold walk is expensive enough to be measurable, and asserts that
//! growth succeeded before grading anything.

use std::sync::Arc;
#[cfg(unix)]
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::json;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::no_sandbox::NoSandboxBackend;
use wcore_sandbox::{
    SandboxChunk, SandboxCommand, SandboxManifest, SandboxOutput, SandboxRegistry,
};
use wcore_tools::NullToolOutputSink;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

/// WHY THE WALL-CLOCK ARMS SUBTRACT A BASELINE AND SAMPLE MINIMA
///
/// They used to assert `second_exec * 3 < cold_walk` on ONE sample of ONE
/// execution. That conflates two things: `second_exec` is
/// `exec overhead + (a walk, if it was repeated)`, and the overhead — process
/// spawn, policy construction, sandbox plumbing — is none of it the walk. On a
/// contended host the overhead alone reaches tens of milliseconds and the
/// assertion fails while the walk has been performed exactly once.
///
/// MEASURED, `macos-latest`, CI run 33240249894: `second exec 53.414ms` against
/// `cold walk 105.839125ms` — a red gate over 53 ms of process spawn. REPRODUCED
/// on hetzner-dsm by pinning six concurrent copies onto four CPUs: 7 of 72
/// executions failed, both variants, e.g. `second exec 28.316531ms against a
/// cold walk measured at 67.81672ms`.
///
/// So the verdict is now taken on the part of the cost the big tree can
/// account for — steady-state minus an identical exec over an EMPTY workspace —
/// and both arms are sampled interleaved and reduced to MINIMA, because a
/// single descheduled sample is exactly what used to decide it. The claim
/// itself is additionally graded by `secret_deny_walk_count()`, which is
/// deterministic and cannot flake at all.
///
/// The cold walk a fixture must reach before any latency verdict is taken.
#[cfg(unix)]
const TARGET_WALK: Duration = Duration::from_millis(60);

/// A backend that really runs the command and CLAIMS OS read-deny enforcement,
/// so `secret_deny_paths_for_backend` actually performs the walk under test.
struct EnforcingBackend(NoSandboxBackend);

#[async_trait]
impl SandboxBackend for EnforcingBackend {
    fn name(&self) -> &'static str {
        "memo_test_enforcing_backend"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn enforces_read_deny(&self) -> bool {
        true
    }
    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput, wcore_sandbox::SandboxError> {
        self.0.execute(manifest, cmd).await
    }
    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>, wcore_sandbox::SandboxError> {
        Arc::new(NoSandboxBackend::new()).execute_streaming(manifest, cmd)
    }
}

fn ctx_for(policy: Arc<WorkspacePolicy>) -> ToolContext {
    ToolContext::test_default()
        .with_sandbox(Arc::new(SandboxRegistry::new(Arc::new(EnforcingBackend(
            NoSandboxBackend::new(),
        )))))
        .with_workspace(policy)
}

/// Pay `BashTool`'s one-time process-wide lazy init before any clock starts.
async fn warm_bash_process_init(root: &std::path::Path) {
    let policy = Arc::new(WorkspacePolicy::trusted_local(root));
    let _ = BashTool
        .execute_with_ctx(json!({"command": "echo warm"}), &ctx_for(policy))
        .await;
}

/// One `BashTool` execution at the requested call site, asserted successful and
/// timed.
#[cfg(unix)]
async fn timed_exec(streaming: bool, ctx: &ToolContext, command: &str) -> Duration {
    let started = Instant::now();
    let result = if streaming {
        BashTool
            .execute_streaming_with_ctx(json!({ "command": command }), ctx, &NullToolOutputSink)
            .await
    } else {
        BashTool
            .execute_with_ctx(json!({ "command": command }), ctx)
            .await
    };
    let elapsed = started.elapsed();
    assert!(!result.is_error, "{}", result.content);
    elapsed
}

/// A second, EMPTY workspace under the same contained policy — the walk-free
/// arm the verdict below is taken against.
///
/// The `TempDir` is returned so the caller keeps it alive; dropping it would
/// delete the workspace under the running policy.
#[cfg(unix)]
async fn walk_free_arm() -> (tempfile::TempDir, ToolContext) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_bash_process_init(&root).await;
    let ctx = ctx_for(Arc::new(WorkspacePolicy::contained(&root)));
    (tmp, ctx)
}

/// How many times each arm is sampled once the memo is warm.
#[cfg(unix)]
const STEADY_SAMPLES: usize = 5;

/// Grow `root` until a COLD walk of it costs at least `target`, and return the
/// smallest cold-walk sample observed (the honest floor).
///
/// Each sample uses a FRESH policy: a memoised policy would report ~0 for every
/// sample after the first, the loop would grow the tree for ever, and the
/// latency verdicts would be taken against a walk cost that does not exist.
#[cfg(unix)]
fn workspace_whose_cold_walk_costs_at_least(root: &std::path::Path, target: Duration) -> Duration {
    std::fs::write(root.join(".env"), b"TOKEN=redacted\n").unwrap();
    for batch in 0..24usize {
        let mut floor = Duration::from_secs(3600);
        for _ in 0..3 {
            let probe = WorkspacePolicy::contained(root);
            let started = Instant::now();
            let deny = probe.secret_deny_paths_for_backend(true);
            floor = floor.min(started.elapsed());
            // Known-positive control on the instrument: a "cheap" walk that
            // stopped finding the planted secret would mean the walk was
            // SKIPPED, not fast, and every verdict below would pass for the
            // wrong reason.
            assert!(
                deny.iter().any(|p| p.ends_with(".env")),
                "instrument control: the contained walk must find the planted .env; got {deny:?}"
            );
        }
        if floor >= target {
            return floor;
        }
        // File-heavy on purpose, ~50 files per directory: that is the shape of
        // a real workspace, and it is the shape the memo is FOR. A
        // directory-dominated tree is the memo's worst case (revalidation costs
        // one `stat` per directory, so a tree with two files per directory
        // saves almost nothing) — see the note on the issue.
        for d in 0..200 {
            let dir = root.join(format!("b{batch}")).join(format!("d{d}"));
            std::fs::create_dir_all(&dir).unwrap();
            for f in 0..50 {
                std::fs::write(dir.join(format!("f{f}.txt")), b"x").unwrap();
            }
        }
    }
    panic!("could not grow a workspace whose cold walk costs {target:?}");
}

/// Wall-clock arms are Unix-only, and deliberately so. Growing a fixture whose
/// cold walk costs 60 ms takes ~240k entries, and creating that many files on
/// NTFS is minutes rather than seconds. The two verdicts these arms take are
/// ALSO taken by `two_execs_perform_exactly_one_walk`, which uses the injected
/// counter, needs no fixture at all and DOES run on Windows — so nothing about
/// this acceptance goes ungraded on any platform. What is lost here is only the
/// wall-clock reproduction of the reported symptom, and that reproduction is
/// recorded against a Linux host.
#[cfg(unix)]
/// #1111 acceptance 1, buffered call site (`execute_with_ctx`).
#[tokio::test]
async fn a_second_bash_exec_does_not_repeat_the_walk_buffered() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_bash_process_init(&root).await;
    let (_walk_free_dir, walk_free_ctx) = walk_free_arm().await;
    let walk = workspace_whose_cold_walk_costs_at_least(&root, TARGET_WALK);

    // #1145: the memo may not answer for a tree that changed within one
    // filesystem timestamp tick of the walk. Let the grown fixture settle past
    // that tick, as the counter-driven tests below already do.
    std::thread::sleep(Duration::from_millis(60));

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let ctx = ctx_for(Arc::clone(&policy));

    // The walk happens here, on the first execution over the grown tree.
    let first_elapsed = timed_exec(false, &ctx, "echo one").await;

    // Steady state, sampled INTERLEAVED against the walk-free arm so both
    // minima are drawn from the same contention window, and taken as MINIMA so
    // one descheduled sample cannot decide the verdict.
    let mut steady = Duration::from_secs(3600);
    let mut walk_free = Duration::from_secs(3600);
    for _ in 0..STEADY_SAMPLES {
        steady = steady.min(timed_exec(false, &ctx, "echo two").await);
        walk_free = walk_free.min(timed_exec(false, &walk_free_ctx, "echo base").await);
    }

    // THE CLAIM, GRADED DETERMINISTICALLY. A wall clock cannot tell a skipped
    // walk from a fast one and reads badly under contention; this counter can,
    // and it cannot flake. It also makes the wall-clock arm below impossible to
    // pass vacuously.
    assert_eq!(
        policy.secret_deny_walk_count(),
        1,
        "buffered executions over one policy must walk the workspace once, not once per exec"
    );

    let attributable = steady.saturating_sub(walk_free);
    println!(
        "#1111 buffered: cold walk {walk:?}, first exec {first_elapsed:?}, \
         steady {steady:?}, walk-free {walk_free:?}, attributable {attributable:?}"
    );
    assert!(
        attributable * 2 < walk,
        "the steady-state exec costs {steady:?}, {attributable:?} of it above the \
         {walk_free:?} an identical exec costs over an EMPTY workspace, against a cold \
         walk of {walk:?} (first exec {first_elapsed:?}) — a walk repeated per exec \
         would put the attributable cost at a full walk"
    );
}

/// #1111 acceptance 1, streaming call site (`execute_streaming_with_ctx`).
/// See the note above on why the wall-clock arms are Unix-only.
#[cfg(unix)]
#[tokio::test]
async fn a_second_bash_exec_does_not_repeat_the_walk_streaming() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_bash_process_init(&root).await;
    let (_walk_free_dir, walk_free_ctx) = walk_free_arm().await;
    let walk = workspace_whose_cold_walk_costs_at_least(&root, TARGET_WALK);

    // #1145: the memo may not answer for a tree that changed within one
    // filesystem timestamp tick of the walk. Let the grown fixture settle past
    // that tick, as the counter-driven tests below already do.
    std::thread::sleep(Duration::from_millis(60));

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let ctx = ctx_for(Arc::clone(&policy));

    // The walk happens here, on the first execution over the grown tree.
    let first_elapsed = timed_exec(true, &ctx, "echo one").await;

    // Steady state, sampled INTERLEAVED against the walk-free arm so both
    // minima are drawn from the same contention window, and taken as MINIMA so
    // one descheduled sample cannot decide the verdict.
    let mut steady = Duration::from_secs(3600);
    let mut walk_free = Duration::from_secs(3600);
    for _ in 0..STEADY_SAMPLES {
        steady = steady.min(timed_exec(true, &ctx, "echo two").await);
        walk_free = walk_free.min(timed_exec(true, &walk_free_ctx, "echo base").await);
    }

    // THE CLAIM, GRADED DETERMINISTICALLY. A wall clock cannot tell a skipped
    // walk from a fast one and reads badly under contention; this counter can,
    // and it cannot flake. It also makes the wall-clock arm below impossible to
    // pass vacuously.
    assert_eq!(
        policy.secret_deny_walk_count(),
        1,
        "streaming executions over one policy must walk the workspace once, not once per exec"
    );

    let attributable = steady.saturating_sub(walk_free);
    println!(
        "#1111 streaming: cold walk {walk:?}, first exec {first_elapsed:?}, \
         steady {steady:?}, walk-free {walk_free:?}, attributable {attributable:?}"
    );
    assert!(
        attributable * 2 < walk,
        "the steady-state streaming exec costs {steady:?}, {attributable:?} of it above the \
         {walk_free:?} an identical exec costs over an EMPTY workspace, against a cold \
         walk of {walk:?} (first exec {first_elapsed:?}) — a walk repeated per exec \
         would put the attributable cost at a full walk"
    );
}

/// #1111 acceptance 1, graded with the INJECTED COUNTER the issue asks for
/// rather than a wall clock: a clock cannot tell a skipped walk from a fast one.
///
/// Both ctx call sites in one test, driven over the SAME policy the way a
/// session drives it.
#[tokio::test]
async fn two_execs_perform_exactly_one_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_bash_process_init(&root).await;
    std::fs::write(root.join(".env"), b"TOKEN=redacted\n").unwrap();
    for d in 0..40 {
        let dir = root.join(format!("d{d}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
    }

    // #1145: the memo may not answer for a tree that changed within one
    // filesystem timestamp tick of the walk - a change that recent leaves an
    // mtime identical to the recorded one, so it cannot be witnessed. Let the
    // fixture settle past that tick before asking for a HIT; the walk counts
    // asserted below are unchanged.
    std::thread::sleep(std::time::Duration::from_millis(60));

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    assert_eq!(
        policy.secret_deny_walk_count(),
        0,
        "instrument control: a freshly constructed policy has walked nothing"
    );
    let ctx = ctx_for(Arc::clone(&policy));

    for _ in 0..2 {
        let r = BashTool
            .execute_with_ctx(json!({"command": "echo hi"}), &ctx)
            .await;
        assert!(!r.is_error, "{}", r.content);
    }
    assert_eq!(
        policy.secret_deny_walk_count(),
        1,
        "two buffered executions must walk the workspace once, not twice"
    );

    for _ in 0..2 {
        let r = BashTool
            .execute_streaming_with_ctx(json!({"command": "echo hi"}), &ctx, &NullToolOutputSink)
            .await;
        assert!(!r.is_error, "{}", r.content);
    }
    assert_eq!(
        policy.secret_deny_walk_count(),
        1,
        "two more executions on the streaming call site must not walk again \
         either — a fix applied to one call site leaves the other live"
    );
}

/// #1111 acceptance 4, the half a memo can break: a secret that appears AFTER
/// the memo was taken must still be denied.
///
/// The secret is planted with plain `std::fs`, deliberately NOT through the
/// tool VFS — that is the case an invalidate-on-VFS-write memo would miss, and
/// it is the ordinary one: the operator's own editor, a `git checkout`, an
/// unrelated program.
#[tokio::test]
async fn a_secret_created_after_the_memo_is_still_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    warm_bash_process_init(&root).await;
    let nested = root.join("packages").join("api");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("main.rs"), b"fn main() {}").unwrap();

    // #1145: the memo may not answer for a tree that changed within one
    // filesystem timestamp tick of the walk - a change that recent leaves an
    // mtime identical to the recorded one, so it cannot be witnessed. Let the
    // fixture settle past that tick before asking for a HIT; the walk counts
    // asserted below are unchanged.
    std::thread::sleep(std::time::Duration::from_millis(60));

    let policy = Arc::new(WorkspacePolicy::contained(&root));
    let ctx = ctx_for(Arc::clone(&policy));

    let first = BashTool
        .execute_with_ctx(json!({"command": "echo one"}), &ctx)
        .await;
    assert!(!first.is_error, "{}", first.content);
    assert_eq!(policy.secret_deny_walk_count(), 1);

    // NEGATIVE CONTROL: nothing secret exists yet, so nothing under the
    // workspace is denied. Without this the positive assertion below could pass
    // against an implementation that denied the whole tree.
    let planted = nested.join(".env");
    assert!(
        !policy
            .secret_deny_paths_for_backend(true)
            .contains(&planted),
        "control: the secret is not planted yet and must not be denied yet"
    );
    assert_eq!(
        policy.secret_deny_walk_count(),
        1,
        "control: that query was served from the memo, so the test below really \
         is asking a memo to notice a change"
    );

    std::fs::write(&planted, b"TOKEN=redacted\n").unwrap();

    let deny = policy.secret_deny_paths_for_backend(true);
    assert!(
        deny.contains(&planted),
        "a secret created after the memo was taken is not denied: {deny:?}"
    );
    assert_eq!(
        policy.secret_deny_walk_count(),
        2,
        "the memo must have been invalidated and the walk redone"
    );
}

/// #1111 acceptance 4: the memo is keyed on the policy too, so a read grant
/// minted mid-session widens the deny set on the very next execution.
#[test]
fn a_new_read_grant_invalidates_the_memo() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_root = std::fs::canonicalize(outside.path()).unwrap();
    std::fs::write(root.join("a.txt"), b"x").unwrap();
    let granted_secret = outside_root.join(".env");
    std::fs::write(&granted_secret, b"TOKEN=redacted\n").unwrap();

    // A grant is only mintable by a local-operator session; the deny walk it
    // widens is the same one either way.
    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    assert!(policy.secret_read_deny_required());
    assert!(
        !policy
            .secret_deny_paths_for_backend(true)
            .contains(&granted_secret),
        "control: an ungranted root is not walked, so its secret is not denied"
    );
    let walks = policy.secret_deny_walk_count();

    policy
        .grant_session_read_root(&outside_root, false)
        .expect("granting a read root");

    let deny = policy.secret_deny_paths_for_backend(true);
    assert!(
        deny.contains(&granted_secret),
        "a secret under a newly granted read root must be denied: {deny:?}"
    );
    assert!(
        policy.secret_deny_walk_count() > walks,
        "the grant must have missed the memo and forced a fresh walk"
    );
}
