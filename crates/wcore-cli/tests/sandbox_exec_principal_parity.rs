//! `wayland-core sandbox exec` and a local session must answer ONE question the
//! same way: may this shell run on a backend that cannot enforce OS
//! secret-read-deny?
//!
//! They did not. Lane B gave a LOCAL interactive session a carve-out at
//! `AgentBootstrap::build` and left `sandbox_cmd.rs` alone, so on the Windows
//! relaxed default (`windows_job_object`, `enforces_read_deny() == false`) the
//! session shell worked and `sandbox exec` exited 1 with "Refused: shell is
//! unavailable because the active sandbox backend cannot enforce
//! secret-read-deny for this workspace." — from the very verb whose purpose is
//! to produce evidence ABOUT that session's containment.
//!
//! `sandbox exec` has exactly one route in: `TopCmd::Sandbox`, parsed from this
//! host's argv. No channel, no host protocol, no slash command, no MCP. Its
//! principal IS the local operator, so it takes the same carve-out — through
//! the same `WorkspacePolicy::with_shell_principal`, not a second copy of the
//! condition.
//!
//! THESE TESTS EXIST TO FAIL IF THE TWO PATHS EVER DISAGREE AGAIN. They read
//! the session answer off the policy the production `AgentBootstrap::build`
//! ACTUALLY installs and the exec answer off the production
//! `sandbox_cmd::sandbox_policy`, then assert equality — so re-introducing an
//! independent condition on either side breaks the build's tests, not a user's
//! Windows box. Nothing is `cfg`-gated: `WindowsJobObjectBackend` compiles and
//! executes on every target, so all three platforms run identical assertions.

use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_cli::sandbox_cmd::sandbox_policy;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, ManagedDangerousPolicy, PolicySource,
};

const REFUSAL: &str = "Refused: shell is unavailable because the active sandbox";
const WRITE_MARKER: &str = "echo>shell_ran.txt";

/// An untrusted workspace — the state of any fresh clone, and the state in
/// which the refusal actually bit. The dead URL is never dialled: `build()`
/// only constructs.
fn config_with(execution_policy: BaselineExecutionPolicy) -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        workspace_trust: wcore_types::workspace_trust::EffectiveWorkspaceTrust::untrusted(
            wcore_types::workspace_trust::AuthoritySource::Default,
            "test-fingerprint",
            "test: no local trust decision for this workspace",
        ),
        execution_policy,
        ..Default::default()
    }
}

fn unmanaged() -> BaselineExecutionPolicy {
    BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::UserConfig)
}

fn managed() -> BaselineExecutionPolicy {
    BaselineExecutionPolicy::managed(ApprovalPolicy::Prompt, ManagedDangerousPolicy::Deny)
}

/// The policy the PRODUCTION bootstrap installs for a local keyboard session —
/// read back off the engine, not off the branch that built it.
async fn session_policy(
    execution_policy: BaselineExecutionPolicy,
    workspace: &std::path::Path,
) -> Arc<WorkspacePolicy> {
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let boot = AgentBootstrap::new(
        config_with(execution_policy),
        workspace.to_str().unwrap().to_string(),
        sink,
    )
    .without_channels(true);
    boot.build()
        .await
        .expect("bootstrap")
        .engine
        .current_tool_context()
        .workspace
        .clone()
        .expect("bootstrap installs one workspace policy per session")
}

/// Execute the REAL shell tool against the REAL Windows relaxed backend under
/// `policy`. This is the backend whose `enforces_read_deny()` is false, i.e.
/// the one that triggers the gate.
async fn run_bash(policy: Arc<WorkspacePolicy>) -> wcore_types::tool::ToolResult {
    let backend: Arc<dyn SandboxBackend> = Arc::new(WindowsJobObjectBackend::new());
    assert!(
        !backend.enforces_read_deny(),
        "precondition: the relaxed Windows default must not claim OS read-deny"
    );
    let ctx = ToolContext::test_default()
        .with_sandbox(Arc::new(SandboxRegistry::new(backend)))
        .with_workspace(policy);
    BashTool
        .execute_with_ctx(json!({ "command": WRITE_MARKER }), &ctx)
        .await
}

/// THE parity property. For every execution floor, the session verb and the
/// operator verb must reach the same shell-principal answer. An independent
/// condition re-introduced on either side fails here.
#[tokio::test]
async fn sandbox_exec_and_a_local_session_agree_on_the_shell_principal() {
    for (label, policy) in [("unmanaged", unmanaged()), ("managed", managed())] {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();

        let session = session_policy(policy.clone(), &root).await;
        // `sandbox exec` supplies `managed_execution_floor` from the merged
        // config files; here the same fact is supplied directly.
        let exec = sandbox_policy(&root, policy.is_managed());

        assert_eq!(
            session.local_operator_principal(),
            exec.local_operator_principal(),
            "{label}: session and `sandbox exec` disagree on the shell principal"
        );
        assert_eq!(
            session.shell_requires_os_read_deny(),
            exec.shell_requires_os_read_deny(),
            "{label}: session and `sandbox exec` disagree on the exec gate"
        );
        // Both must still WANT OS read-deny, so the deny list is still built and
        // a backend that CAN enforce it still does. The carve-out moves the gate
        // predicate, never the deny list.
        assert!(session.secret_read_deny_required(), "{label}: session");
        assert!(exec.secret_read_deny_required(), "{label}: exec");
    }
}

/// The fix, at the product level: on the backend that cannot enforce read-deny,
/// `sandbox exec`'s own policy now runs a command and produces the artifact the
/// containment differential is read from.
#[tokio::test]
async fn sandbox_exec_runs_a_command_on_a_backend_without_os_read_deny() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let policy = Arc::new(sandbox_policy(&root, false));

    assert!(policy.local_operator_principal());
    assert!(!policy.shell_requires_os_read_deny());

    let result = run_bash(Arc::clone(&policy)).await;
    assert!(
        !result.content.contains(REFUSAL),
        "`sandbox exec` must not refuse the local operator: {}",
        result.content
    );
    let marker = root.join("shell_ran.txt");
    let bytes = std::fs::metadata(&marker)
        .unwrap_or_else(|e| panic!("no artifact at {}: {e}", marker.display()))
        .len();
    assert!(bytes > 0, "marker file exists but is empty");
}

/// The administrator's floor is NOT relaxed by this verb. Without this,
/// `sandbox exec` would be a documented one-command way to obtain the very
/// shell a Managed policy refuses in-session.
#[tokio::test]
async fn a_managed_floor_still_refuses_sandbox_exec() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let policy = Arc::new(sandbox_policy(&root, true));

    assert!(!policy.local_operator_principal());
    assert!(policy.shell_requires_os_read_deny());

    let result = run_bash(Arc::clone(&policy)).await;
    assert!(
        result.is_error && result.content.contains(REFUSAL),
        "a Managed floor must still refuse: {}",
        result.content
    );
    assert!(
        !root.join("shell_ran.txt").exists(),
        "a refused shell must write nothing"
    );
}

/// `with_shell_principal` is the whole decision, and it is a conjunction: a
/// channel principal is refused the carve-out even with no Managed floor, and a
/// Managed floor refuses it even with no channel principal. Pinned directly so
/// a future edit cannot quietly turn the `||` into an `&&` and pass the two
/// tests above.
#[test]
fn the_shared_predicate_relaxes_only_for_an_unmanaged_local_principal() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    for (channel, managed_floor, expected) in [
        (false, false, true),
        (false, true, false),
        (true, false, false),
        (true, true, false),
    ] {
        let policy = WorkspacePolicy::contained(&root).with_shell_principal(channel, managed_floor);
        assert_eq!(
            policy.local_operator_principal(),
            expected,
            "channel={channel} managed={managed_floor}"
        );
        assert_eq!(
            policy.shell_requires_os_read_deny(),
            !expected,
            "channel={channel} managed={managed_floor}"
        );
    }
}
