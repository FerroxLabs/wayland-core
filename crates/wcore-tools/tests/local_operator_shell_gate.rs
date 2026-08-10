//! The exec-time shell gate, driven as a truth table over the REAL Windows
//! relaxed default backend.
//!
//! Before this lane, `bash.rs` refused every shell whose `WorkspacePolicy`
//! required OS secret-read-deny on a backend that cannot enforce it. On the
//! Windows shipping default (`WindowsJobObjectBackend`, which deliberately
//! keeps `enforces_read_deny() == false`) that is EVERY untrusted workspace —
//! i.e. every fresh clone — so the product had no shell at all on first run.
//!
//! The gate predicate is now
//! `WorkspacePolicy::shell_requires_os_read_deny()`, which is
//! `secret_read_deny_required && !local_operator_principal`. These tests pin
//! both directions and both of `bash.rs`'s two gate sites (buffered and
//! streaming — a fix applied to only one of them must not pass).
//!
//! **Why these tests are the Linux/macOS bit-identity proof.** Nothing here is
//! `cfg`-gated: `WindowsJobObjectBackend` compiles on every target and really
//! spawns (it delegates to `NoSandboxBackend`), so the same file runs the same
//! assertions on Linux, macOS and Windows. `read_deny_enforcing_backend_makes_
//! the_local_principal_inert` then shows the new flag changes NOTHING once the
//! backend claims read-deny enforcement — and Linux (bwrap) and macOS
//! (sandbox_exec) both claim it at their shipping default, which
//! `platform_default_backends_that_enforce_read_deny_never_reach_the_relaxation`
//! asserts against the real platform cascade.
//!
//! Every verdict is graded from the FILESYSTEM, not from `is_error`: a refusal
//! must leave zero bytes, and an admitted command must leave a real file.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::json;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::no_sandbox::NoSandboxBackend;
use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;
use wcore_sandbox::{
    SandboxChunk, SandboxCommand, SandboxManifest, SandboxOutput, SandboxRegistry,
};
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const REFUSAL: &str = "Refused: shell is unavailable because the active sandbox";

/// A backend that really executes (same `NoSandboxBackend` delegation the
/// Windows relaxed default uses) but whose `enforces_read_deny()` claim is a
/// constructor argument. It exists so the ONLY variable between the two arms of
/// the positive control is that one claim — not the execution mechanism.
struct ClaimingBackend {
    enforces: bool,
    inner: NoSandboxBackend,
    calls: Arc<AtomicUsize>,
}

impl ClaimingBackend {
    fn new(enforces: bool) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                enforces,
                inner: NoSandboxBackend::new(),
                calls: Arc::clone(&calls),
            }),
            calls,
        )
    }
}

#[async_trait]
impl SandboxBackend for ClaimingBackend {
    fn name(&self) -> &'static str {
        "claiming_test_backend"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn enforces_read_deny(&self) -> bool {
        self.enforces
    }
    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> wcore_sandbox::Result<SandboxOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.execute(manifest, cmd).await
    }
    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> wcore_sandbox::Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Arc::new(NoSandboxBackend::new()).execute_streaming(manifest, cmd)
    }
}

/// `echo` and `>` are builtins of both `sh` and `cmd`, so this needs nothing on
/// PATH inside the scrubbed child environment. Deliberately SPACE-FREE: the
/// Windows `cmd /C` path has a separate, unrelated argument-splitting defect
/// (see the working-system scoreboard), and a shell gate test must not be able
/// to fail for that reason.
const WRITE_MARKER: &str = "echo>shell_ran.txt";

fn ctx_for(policy: WorkspacePolicy, backend: Arc<dyn SandboxBackend>) -> ToolContext {
    ToolContext::test_default()
        .with_sandbox(Arc::new(SandboxRegistry::new(backend)))
        .with_workspace(Arc::new(policy))
}

fn marker_bytes(root: &std::path::Path) -> Option<u64> {
    std::fs::metadata(root.join("shell_ran.txt"))
        .ok()
        .map(|m| m.len())
}

/// THE fix. An untrusted local workspace — a fresh clone — on the real Windows
/// relaxed default backend gets a shell that actually runs and actually writes.
#[tokio::test]
async fn local_operator_keeps_a_working_shell_on_the_relaxed_windows_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let backend = Arc::new(WindowsJobObjectBackend::new());
    assert!(
        !backend.enforces_read_deny(),
        "precondition: the Windows relaxed default must NOT claim OS read-deny, \
         otherwise this test proves nothing about the gate"
    );

    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    assert!(
        policy.secret_read_deny_required(),
        "precondition: the policy still WANTS OS read-deny — the relaxation is \
         about the principal, not about dropping the requirement"
    );
    assert!(!policy.shell_requires_os_read_deny());

    let result = BashTool
        .execute_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(policy, backend),
        )
        .await;

    assert!(
        !result.content.contains(REFUSAL),
        "a local-operator session must not be refused: {}",
        result.content
    );
    assert!(!result.is_error, "{}", result.content);
    let bytes = marker_bytes(&root).expect("the shell must leave a real file on disk");
    assert!(bytes > 0, "marker file exists but is empty");
}

/// The safety net, unchanged. A channel/remote policy carries no local-operator
/// principal, so the SAME backend and the SAME command are refused — with zero
/// bytes written, which is what makes this a real denial rather than a reported
/// one.
#[tokio::test]
async fn channel_scoped_policy_is_still_refused_on_the_relaxed_windows_backend() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let policy = WorkspacePolicy::contained(&root);
    assert!(!policy.local_operator_principal());
    assert!(policy.shell_requires_os_read_deny());

    let result = BashTool
        .execute_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(policy, Arc::new(WindowsJobObjectBackend::new())),
        )
        .await;

    assert!(result.is_error, "{}", result.content);
    assert!(
        result.content.contains(REFUSAL),
        "expected the read-deny refusal, got: {}",
        result.content
    );
    assert!(
        marker_bytes(&root).is_none(),
        "a refused shell must write nothing; found {:?}",
        marker_bytes(&root)
    );
}

/// `bash.rs` has TWO gate sites. The streaming path must reach the identical
/// verdict, or a one-site change would leave the other half of the product
/// behaving the old way.
#[tokio::test]
async fn the_streaming_path_reaches_the_same_two_verdicts() {
    struct Sink;
    impl wcore_tools::ToolOutputSink for Sink {
        fn emit_chunk(&self, _chunk: &str) {}
    }

    let allowed = tempfile::tempdir().unwrap();
    let allowed_root = std::fs::canonicalize(allowed.path()).unwrap();
    let ok = BashTool
        .execute_streaming_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(
                WorkspacePolicy::contained(&allowed_root).with_local_operator_principal(),
                Arc::new(WindowsJobObjectBackend::new()),
            ),
            &Sink,
        )
        .await;
    assert!(!ok.content.contains(REFUSAL), "{}", ok.content);
    assert!(
        marker_bytes(&allowed_root).is_some_and(|n| n > 0),
        "streaming local-operator shell must leave a real file"
    );

    let refused_dir = tempfile::tempdir().unwrap();
    let refused_root = std::fs::canonicalize(refused_dir.path()).unwrap();
    let refused = BashTool
        .execute_streaming_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(
                WorkspacePolicy::contained(&refused_root),
                Arc::new(WindowsJobObjectBackend::new()),
            ),
            &Sink,
        )
        .await;
    assert!(refused.is_error, "{}", refused.content);
    assert!(refused.content.contains(REFUSAL), "{}", refused.content);
    assert!(
        marker_bytes(&refused_root).is_none(),
        "a refused streaming shell must write nothing"
    );
}

/// Positive control AND the Linux/macOS bit-identity argument in one test.
///
/// With a backend that DOES claim `enforces_read_deny()`, all four
/// (principal × policy) combinations run and none is refused — the new
/// `local_operator_principal` flag is behaviourally inert. Linux (bwrap) and
/// macOS (sandbox_exec) claim it at their shipping default, so this is the
/// state those two platforms are in.
///
/// Without this control, the refusal above would also "pass" if `contained`
/// simply never had a shell.
#[tokio::test]
async fn read_deny_enforcing_backend_makes_the_local_principal_inert() {
    for local_principal in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let base = WorkspacePolicy::contained(&root);
        let policy = if local_principal {
            base.with_local_operator_principal()
        } else {
            base
        };
        let (backend, calls) = ClaimingBackend::new(true);

        let result = BashTool
            .execute_with_ctx(
                json!({ "command": WRITE_MARKER }),
                &ctx_for(policy, backend),
            )
            .await;

        assert!(
            !result.content.contains(REFUSAL),
            "local_principal={local_principal}: an enforcing backend must never be \
             refused, got: {}",
            result.content
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "local_principal={local_principal}: the command must reach the backend"
        );
        assert!(
            marker_bytes(&root).is_some_and(|n| n > 0),
            "local_principal={local_principal}: enforcing backend must still run the shell"
        );
    }
}

/// The other two policy shapes that carry a non-operator principal stay refused
/// on a non-enforcing backend, and the pre-existing genuinely-local Trusted
/// policy is untouched (it never required OS read-deny in the first place).
#[tokio::test]
async fn non_operator_principals_stay_refused_and_trusted_local_is_untouched() {
    // Delegated orchestrator mutation — issued BY an orchestrator, never typed.
    let txn = tempfile::tempdir().unwrap();
    let checkout = txn.path().join("checkout");
    let scratch = txn.path().join("scratch");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::create_dir(&scratch).unwrap();
    let delegated = WorkspacePolicy::delegated_mutation(&checkout, &scratch, []).unwrap();
    assert!(!delegated.local_operator_principal());
    assert!(delegated.shell_requires_os_read_deny());
    let checkout = std::fs::canonicalize(&checkout).unwrap();
    let refused = BashTool
        .execute_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(delegated, Arc::new(WindowsJobObjectBackend::new())),
        )
        .await;
    assert!(refused.content.contains(REFUSAL), "{}", refused.content);
    assert!(marker_bytes(&checkout).is_none());

    // A Full/remote channel session mints a Trusted policy and opts INTO project
    // secret denial. It must not be able to reach the local branch either.
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_root = std::fs::canonicalize(remote_dir.path()).unwrap();
    let remote = WorkspacePolicy::trusted_local(&remote_root).with_project_secret_deny();
    assert!(remote.shell_requires_os_read_deny());
    let refused = BashTool
        .execute_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(remote, Arc::new(WindowsJobObjectBackend::new())),
        )
        .await;
    assert!(refused.content.contains(REFUSAL), "{}", refused.content);
    assert!(marker_bytes(&remote_root).is_none());

    // Genuinely-local Trusted: no change, it never required OS read-deny.
    let trusted_dir = tempfile::tempdir().unwrap();
    let trusted_root = std::fs::canonicalize(trusted_dir.path()).unwrap();
    let trusted = WorkspacePolicy::trusted_local(&trusted_root);
    assert!(!trusted.shell_requires_os_read_deny());
    let ok = BashTool
        .execute_with_ctx(
            json!({ "command": WRITE_MARKER }),
            &ctx_for(trusted, Arc::new(WindowsJobObjectBackend::new())),
        )
        .await;
    assert!(!ok.content.contains(REFUSAL), "{}", ok.content);
    assert!(marker_bytes(&trusted_root).is_some_and(|n| n > 0));
}

/// Linux and macOS reach the relaxation only if their shipping-default backend
/// stops enforcing read-deny. Asserted against the real platform cascade, so a
/// future backend swap on either platform fails HERE rather than silently
/// changing what an operator's shell is allowed to read.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn platform_default_backends_that_enforce_read_deny_never_reach_the_relaxation() {
    assert!(
        wcore_tools::bash::platform_enforces_read_deny(),
        "the {} shipping default must enforce OS secret-read-deny; if it no longer \
         does, the local-operator relaxation becomes reachable here and that is a \
         deliberate decision, not a side effect",
        std::env::consts::OS
    );
}
