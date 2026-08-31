//! Who may reach the local-shell branch, decided by the PRODUCTION bootstrap.
//!
//! `bash.rs` refuses a shell whose `WorkspacePolicy` requires OS secret-read-
//! deny on a backend that cannot enforce it. That refusal is correct for a
//! remote sender and catastrophic for a local operator in a fresh clone, where
//! it removed the shell entirely on the Windows relaxed default.
//!
//! The distinction is drawn at exactly one seam: `AgentBootstrap`'s
//! `channel_tool_posture`, which is `Some` for the engines
//! `ChannelTurnDispatcher` builds and `None` for the CLI / TUI / json-stream
//! engines. These tests drive the real `build()` — the same path a channel turn
//! uses — and read the flag off the policy the engine ACTUALLY holds, then
//! execute the real `BashTool` against the real Windows relaxed backend to
//! prove the flag decides the outcome.
//!
//! Nothing here is `cfg`-gated: `WindowsJobObjectBackend` compiles and executes
//! on every target, so Linux, macOS and Windows run identical assertions.

use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_sandbox::SandboxRegistry;
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend;
use wcore_tools::Tool;
use wcore_tools::bash::BashTool;
use wcore_tools::context::ToolContext;
use wcore_tools::workspace_policy::WorkspacePolicy;

const REFUSAL: &str = "Refused: shell is unavailable because the active sandbox";
const WRITE_MARKER: &str = "echo>shell_ran.txt";

/// Untrusted workspace, non-managed: the state of any fresh clone. The dead URL
/// is never dialled — `build()` only constructs.
fn untrusted_config() -> Config {
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
        ..Default::default()
    }
}

/// Records what the OPERATOR is told. Every other surface delegates to
/// `NullSink` so the test asserts on the notice channel alone.
#[derive(Default)]
struct NoticeSink {
    infos: std::sync::Mutex<Vec<String>>,
}

impl OutputSink for NoticeSink {
    fn emit_text_delta(&self, text: &str, msg_id: &str) {
        NullSink.emit_text_delta(text, msg_id);
    }
    fn emit_thinking(&self, text: &str, msg_id: &str) {
        NullSink.emit_thinking(text, msg_id);
    }
    fn emit_tool_call(&self, name: &str, input: &str) {
        NullSink.emit_tool_call(name, input);
    }
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        NullSink.emit_tool_result(name, is_error, content);
    }
    fn emit_stream_start(&self, msg_id: &str) {
        NullSink.emit_stream_start(msg_id);
    }
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input: u64,
        output: u64,
        cache_creation: u64,
        cache_read: u64,
        finish: wcore_types::message::FinishReason,
    ) {
        NullSink.emit_stream_end(
            msg_id,
            turns,
            input,
            output,
            cache_creation,
            cache_read,
            finish,
        );
    }
    fn emit_error(
        &self,
        msg: &str,
        retryable: bool,
        _category: wcore_protocol::events::FailureCategory,
    ) {
        NullSink.emit_error(
            msg,
            retryable,
            wcore_protocol::events::FailureCategory::Unknown,
        );
    }
    fn emit_info(&self, msg: &str) {
        self.infos.lock().unwrap().push(msg.to_string());
    }
}

async fn build_session(
    config: Config,
    workspace: &std::path::Path,
    posture: Option<ChannelToolPosture>,
) -> (Arc<WorkspacePolicy>, Vec<String>) {
    let ws = workspace.to_str().unwrap().to_string();
    let notices = Arc::new(NoticeSink::default());
    let sink: Arc<dyn OutputSink> = notices.clone();
    let mut boot = AgentBootstrap::new(config, ws.clone(), sink).without_channels(true);
    if let Some(posture) = posture {
        boot = boot.channel_tool_posture(ChannelToolScope {
            posture,
            workspace_root: workspace.to_path_buf(),
        });
    }
    let result = boot.build().await.expect("bootstrap");
    let policy = result
        .engine
        .current_tool_context()
        .workspace
        .clone()
        .expect("bootstrap installs one workspace policy per session");
    let infos = notices.infos.lock().unwrap().clone();
    (policy, infos)
}

async fn policy_for(
    config: Config,
    workspace: &std::path::Path,
    posture: Option<ChannelToolPosture>,
) -> Arc<WorkspacePolicy> {
    build_session(config, workspace, posture).await.0
}

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

/// THE fix, through the production bootstrap: a local keyboard session in an
/// UNTRUSTED workspace is a local-operator principal and keeps a working shell.
#[tokio::test]
async fn local_untrusted_session_is_a_local_operator_principal_and_keeps_its_shell() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let policy = policy_for(untrusted_config(), &root, None).await;

    assert_eq!(
        policy.trust(),
        wcore_tools::WorkspaceTrust::Contained,
        "an untrusted workspace must still get the Contained profile — the \
         relaxation is about the shell gate, not about widening the sandbox"
    );
    assert!(
        policy.secret_read_deny_required(),
        "the policy must still WANT OS read-deny, so the deny list is still built"
    );
    assert!(policy.local_operator_principal());
    assert!(!policy.shell_requires_os_read_deny());

    let result = run_bash(Arc::clone(&policy)).await;
    assert!(
        !result.content.contains(REFUSAL),
        "the local operator must get a shell: {}",
        result.content
    );
    let marker = policy.root().join("shell_ran.txt");
    let bytes = std::fs::metadata(&marker)
        .unwrap_or_else(|e| panic!("no artifact at {}: {e}", marker.display()))
        .len();
    assert!(bytes > 0, "marker file exists but is empty");
}

/// A channel session must NOT be able to reach the local branch — for every
/// posture, including `Full`, which keeps `Bash` in its registry and is
/// therefore the one that would actually execute.
#[tokio::test]
async fn no_channel_posture_can_reach_the_local_shell_branch() {
    for posture in [
        ChannelToolPosture::Full,
        ChannelToolPosture::Workspace,
        ChannelToolPosture::Conversational,
    ] {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let policy = policy_for(untrusted_config(), &root, Some(posture)).await;

        assert!(
            !policy.local_operator_principal(),
            "{posture:?}: a channel scope must never mint a local-operator principal"
        );
        assert!(
            policy.shell_requires_os_read_deny(),
            "{posture:?}: the shell gate must stay armed for a channel session"
        );

        let result = run_bash(Arc::clone(&policy)).await;
        assert!(
            result.is_error && result.content.contains(REFUSAL),
            "{posture:?}: a channel session's shell must still be refused, got: {}",
            result.content
        );
        assert!(
            !policy.root().join("shell_ran.txt").exists(),
            "{posture:?}: a refused channel shell must write nothing"
        );
    }
}

/// A trusted local workspace was never gated (`trusted_local` does not require
/// OS read-deny), so it must be unchanged — and it must NOT acquire the new
/// flag, which only ever rides on the Contained branch.
#[tokio::test]
async fn a_trusted_local_session_is_unchanged() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut config = untrusted_config();
    config.workspace_trust = wcore_types::workspace_trust::resolve_workspace_trust(
        "test-fingerprint",
        [wcore_types::workspace_trust::WorkspaceTrustInput::grant(
            wcore_types::workspace_trust::AuthoritySource::LocalSession,
        )],
    );
    assert!(config.workspace_trust.is_trusted());
    let policy = policy_for(config, &root, None).await;

    assert_eq!(policy.trust(), wcore_tools::WorkspaceTrust::Trusted);
    assert!(!policy.secret_read_deny_required());
    assert!(!policy.local_operator_principal());
    assert!(!policy.shell_requires_os_read_deny());
}

/// Managed execution policy is an administrator-imposed floor. This lane
/// relaxes the local operator's own session, not an administrator's.
#[tokio::test]
async fn a_managed_local_session_does_not_get_the_relaxation() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut config = untrusted_config();
    config.execution_policy = wcore_types::execution_policy::BaselineExecutionPolicy::managed(
        wcore_types::execution_policy::ApprovalPolicy::Prompt,
        wcore_types::execution_policy::ManagedDangerousPolicy::Deny,
    );
    assert!(config.execution_policy.is_managed());

    let policy = policy_for(config, &root, None).await;
    assert!(!policy.local_operator_principal());
    assert!(policy.shell_requires_os_read_deny());

    let result = run_bash(Arc::clone(&policy)).await;
    assert!(
        result.is_error && result.content.contains(REFUSAL),
        "a Managed session must stay refused, got: {}",
        result.content
    );
}

/// The activation notice: present exactly when a shell was KEPT that the gate
/// would otherwise have refused, absent otherwise, and truthful either way.
///
/// The expectation is keyed off the platform's own default backend, so this one
/// test is decisive on Windows (relaxed default → notice REQUIRED, and its
/// wording is checked) and non-vacuous on Linux and macOS (read-deny-enforcing
/// default → notice must be ABSENT, which is the byte-for-byte-unchanged claim
/// for those two platforms stated as an assertion instead of a comment).
#[tokio::test]
async fn the_activation_notice_matches_what_the_platform_actually_enforces() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (policy, infos) = build_session(untrusted_config(), &root, None).await;
    assert!(policy.local_operator_principal());

    let notices: Vec<&String> = infos
        .iter()
        .filter(|m| m.contains("Local shell enabled WITHOUT OS secret containment"))
        .collect();

    if wcore_tools::bash::platform_enforces_read_deny() {
        assert!(
            notices.is_empty(),
            "this platform's default backend enforces OS read-deny, so no shell was \
             ever at risk of refusal and the operator must be told nothing new; got: \
             {notices:?}"
        );
        return;
    }

    assert_eq!(
        notices.len(),
        1,
        "a kept-but-uncontained shell must be announced exactly once; got: {infos:?}"
    );
    let notice = notices[0];
    // It must name what is NOT enforced...
    assert!(
        notice.contains("cannot enforce filesystem read-deny"),
        "{notice}"
    );
    assert!(notice.contains("credential stores"), "{notice}");
    // ...and what IS, or it is a scare with no actionable content.
    assert!(notice.contains("process-tree ownership"), "{notice}");
    assert!(notice.contains("approval"), "{notice}");
    assert!(notice.contains("channel tool posture"), "{notice}");
    // ...and it must not overclaim scope.
    assert!(notice.contains("local keyboard session"), "{notice}");
    // The backend it blames must be the backend actually in force.
    let backend = wcore_sandbox::default_for_platform().name();
    assert!(
        notice.contains(backend),
        "the notice must name the live backend ({backend}): {notice}"
    );
}

/// A channel session never keeps the shell, so it must never be told it did.
#[tokio::test]
async fn a_channel_session_is_never_given_the_local_shell_notice() {
    for posture in [ChannelToolPosture::Full, ChannelToolPosture::Workspace] {
        let tmp = tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (policy, infos) = build_session(untrusted_config(), &root, Some(posture)).await;
        assert!(!policy.local_operator_principal());
        assert!(
            !infos
                .iter()
                .any(|m| m.contains("Local shell enabled WITHOUT OS secret containment")),
            "{posture:?}: a channel session must never receive the local-shell notice: {infos:?}"
        );
    }
}
