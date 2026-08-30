//! core#244 c4 — the SCOPE of "a Bash subprocess cannot read the store", pinned
//! by measurement instead of asserted by omission.
//!
//! `bash_vcs_store_deny_linux.rs` proves the property where the OS sandbox
//! enforces read-deny (bwrap, sandbox-exec). It is `cfg(target_os = "linux")`
//! and it SKIPS where `platform_enforces_read_deny()` is false — so on the
//! Windows shipping default it asserts nothing at all, and c4's unqualified
//! text quietly covered a platform on which it is FALSE.
//!
//! The exec-time gate is `shell_requires_os_read_deny()` =
//! `secret_read_deny_required && !local_operator_principal`. On a backend that
//! cannot enforce read-deny — `WindowsJobObjectBackend`, the Windows shipping
//! default — that yields two different answers:
//!
//! * NOT the local operator: the shell is REFUSED. Fail-closed, and the first
//!   test below reproduces that arm before the second asserts the open one.
//! * The local operator (the ordinary interactive CLI user): the shell RUNS,
//!   with no OS read-deny, and the VCS content store is readable. That is
//!   deliberate — `workspace_policy.rs:1204-1223` records why, and the
//!   alternative was no shell at all on every fresh Windows clone — but it
//!   means the store is NOT unreachable to a Bash subprocess there.
//!
//! **This file is not `cfg(windows)`.** `WindowsJobObjectBackend` compiles on
//! every target and really spawns (it delegates to `NoSandboxBackend`), which
//! is the same property `local_operator_shell_gate.rs` relies on. So the gap is
//! a STANDING gate on the Linux build host rather than a named-host run that
//! nothing re-checks.
//!
//! The second test asserts the gap IS THERE. If it ever fails, the gap has been
//! CLOSED: re-grade core#244 c4 and FerroxLabs/wayland-core#391 — do not delete
//! the test.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const ROOT_OBJECT: &str = "ROOT-OBJECT-BYTES-244";
const PLAIN: &str = "ordinary-working-tree-contents";
const REFUSAL: &str = "Refused: shell is unavailable because the active sandbox";

/// `cat` on unix, `type` on `cmd` — the one external difference this file has,
/// branched rather than scattered, per the cross-platform rule.
fn read_cmd(rel_unix: &str) -> String {
    if cfg!(windows) {
        format!("type {}", rel_unix.replace('/', "\\"))
    } else {
        format!("cat {rel_unix}")
    }
}

fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join(".git/objects/ab")).unwrap();
    std::fs::write(root.join(".git/objects/ab/cdef"), ROOT_OBJECT).unwrap();
    std::fs::write(root.join("readme.txt"), PLAIN).unwrap();
    (dir, root)
}

/// The Windows shipping default, whose `enforces_read_deny()` is deliberately
/// false. Returned with the precondition already asserted, so no test can
/// silently run against a backend that would make it vacuous.
fn non_enforcing_backend() -> Arc<dyn SandboxBackend> {
    let backend = Arc::new(WindowsJobObjectBackend::new());
    assert!(
        !backend.enforces_read_deny(),
        "precondition: the Windows relaxed default must NOT claim OS read-deny, \
         or neither test below is about anything"
    );
    backend
}

fn ctx_for(policy: WorkspacePolicy) -> ToolContext {
    ToolContext::test_default()
        .with_sandbox(Arc::new(SandboxRegistry::new(non_enforcing_backend())))
        .with_workspace(Arc::new(policy))
}

async fn run(ctx: &ToolContext, root: &Path, command: &str) -> String {
    BashTool
        .execute_with_ctx(
            json!({ "command": command, "cwd": root.to_string_lossy() }),
            ctx,
        )
        .await
        .content
}

/// THE FAIL-CLOSED ARM, reproduced first. Every principal that is not the local
/// operator still loses the shell entirely on a backend that cannot enforce
/// read-deny, so the store is unreachable to it for the strongest possible
/// reason.
#[tokio::test]
async fn a_non_local_principal_gets_no_shell_at_all_on_a_non_enforcing_backend() {
    let (_dir, root) = workspace();
    let policy = WorkspacePolicy::contained(&root);
    assert!(
        policy.shell_requires_os_read_deny(),
        "precondition: a non-local-operator contained policy must still demand \
         OS read-deny, or this arm is not the fail-closed one"
    );
    let ctx = ctx_for(policy);

    let out = run(&ctx, &root, &read_cmd(".git/objects/ab/cdef")).await;
    assert!(
        out.contains(REFUSAL),
        "the fail-closed arm must refuse the shell outright, got:\n{out}"
    );
    assert!(
        !out.contains(ROOT_OBJECT),
        "the object store's bytes reached a refused shell:\n{out}"
    );
}

/// THE OPEN ARM. c4's text says "a Bash subprocess cannot read the store"; for
/// the local operator on a non-enforcing backend it can, and this pins that so
/// it cannot silently change in either direction.
#[tokio::test]
async fn a_local_operator_shell_reads_the_store_on_a_non_enforcing_backend() {
    let (_dir, root) = workspace();
    let policy = WorkspacePolicy::contained(&root).with_local_operator_principal();
    assert!(
        policy.secret_read_deny_required(),
        "precondition: the policy still WANTS OS read-deny — the relaxation is \
         about the principal, not about dropping the requirement"
    );
    assert!(
        !policy.shell_requires_os_read_deny(),
        "precondition: the local-operator exemption is what puts a shell on this \
         backend at all"
    );
    let ctx = ctx_for(policy);

    // POSITIVE CONTROL. Without it, a clean result below would be satisfied by
    // a shell that ran nothing.
    let plain = run(&ctx, &root, &read_cmd("readme.txt")).await;
    assert!(
        plain.contains(PLAIN),
        "control failed: an ordinary working-tree read must succeed, got:\n{plain}"
    );

    let store = run(&ctx, &root, &read_cmd(".git/objects/ab/cdef")).await;
    eprintln!("LOCAL_OPERATOR_STORE_READ:\n{store}");
    assert!(
        store.contains(ROOT_OBJECT),
        "core#244 c4 scope pin FAILED — which means the gap is CLOSED, not that \
         this test is wrong. A local-operator shell on a backend that cannot \
         enforce OS read-deny no longer reads the VCS content store. Re-grade \
         core#244 c4 and FerroxLabs/wayland-core#391 and then update this file; \
         do not delete it. Got:\n{store}"
    );
}

/// THE THIRD ARM OF THE SAME GATE, which neither test above reaches and which
/// c4's rewritten text now names explicitly rather than leaving to a note.
///
/// `bash.rs` refuses on `shell_requires_os_read_deny() &&
/// !enforces_read_deny() && !bypasses_containment()`. The first two conjuncts
/// are what the two tests above split. The THIRD is `SandboxRegistry::dangerous`
/// -- the operator's explicit no-sandbox launch, session AUTHORITY rather than a
/// backend capability -- and it admits the shell for a NON-local principal, the
/// very principal the fail-closed arm refuses.
///
/// Without this measurement, a c4 clause reading "the shell is REFUSED for every
/// principal but the local operator" would be FALSE and nothing in the suite
/// would say so. It is measured here so the exclusion carried by the field is a
/// fact and not a narration.
#[tokio::test]
async fn a_dangerous_no_sandbox_session_admits_the_shell_for_a_non_local_principal() {
    use wcore_types::execution_policy::{
        ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
        resolve_dangerous_launch,
    };

    let (_dir, root) = workspace();
    let policy = WorkspacePolicy::contained(&root);
    assert!(
        policy.shell_requires_os_read_deny(),
        "precondition: the SAME non-local-operator policy the fail-closed arm \
         uses, or this is not the same gate"
    );

    let baseline = BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default);
    let grant = resolve_dangerous_launch(
        &baseline,
        DangerousLaunchRequest::cli(60, "core-244-c4-third-arm"),
        10_000,
    )
    .unwrap();
    let registry = SandboxRegistry::dangerous(&grant);
    assert!(
        registry.bypasses_containment(),
        "precondition: the Dangerous launch must be the containment-bypass \
         session, or this test is about the wrong arm"
    );
    assert!(
        !registry.enforces_read_deny(),
        "precondition: the no-sandbox backend must not claim read-deny, or the \
         refusal would be skipped for the OTHER conjunct and this arm is vacuous"
    );

    let ctx = ToolContext::test_default()
        .with_sandbox(Arc::new(registry))
        .with_workspace(Arc::new(policy));

    let store = run(&ctx, &root, &read_cmd(".git/objects/ab/cdef")).await;
    eprintln!("DANGEROUS_NON_LOCAL_STORE_READ:\n{store}");
    assert!(
        !store.contains(REFUSAL),
        "the Dangerous bypass arm must NOT refuse -- if it now does, c4's \
         Dangerous exclusion is stale and must be DELETED from the field. \
         Got:\n{store}"
    );
    assert!(
        store.contains(ROOT_OBJECT),
        "the Dangerous bypass arm is expected to read the store outright; if it \
         no longer does, re-grade c4 rather than editing this assertion. \
         Got:\n{store}"
    );
}
