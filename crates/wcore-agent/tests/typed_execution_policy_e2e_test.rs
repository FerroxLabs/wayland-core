//! RED integration contract for typed Smart execution-policy wiring.
//!
//! These tests intentionally target the proposed
//! `AgentBootstrap::with_smart_execution_policy` builder API. Until production
//! bootstrap and confirmation paths consume the typed policy, this test target
//! must fail to compile or fail its behavioral assertions.

mod common;

use std::sync::{Arc, Mutex};

use common::MockLlmProvider;
use serde_json::json;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
    resolve_dangerous_launch,
};
use wcore_types::message::FinishReason;

fn bootstrap_config() -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 64,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    // Read is normally allow-listed. Clear the list so every successful tool
    // call below proves the typed posture reached the approval gate.
    config.tools.allow_list.clear();
    config
}

#[derive(Default)]
struct CapturingSink {
    tool_results: Mutex<Vec<(String, bool, String)>>,
}

impl CapturingSink {
    fn has_success_for(&self, tool: &str) -> bool {
        self.tool_results
            .lock()
            .unwrap()
            .iter()
            .any(|(name, is_error, _)| name == tool && !is_error)
    }

    fn has_success_containing(&self, tool: &str, needle: &str) -> bool {
        self.tool_results
            .lock()
            .unwrap()
            .iter()
            .any(|(name, is_error, content)| name == tool && !is_error && content.contains(needle))
    }

    fn has_denial_for(&self, tool: &str) -> bool {
        self.tool_results
            .lock()
            .unwrap()
            .iter()
            .any(|(name, is_error, content)| {
                name == tool && *is_error && content.to_ascii_lowercase().contains("denied")
            })
    }
}

impl OutputSink for CapturingSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str) {
        self.tool_results
            .lock()
            .unwrap()
            .push((name.to_string(), is_error, content.to_string()));
    }
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
}

#[tokio::test]
async fn typed_bypass_executes_read_without_bypassing_required_sandbox() {
    let workspace = tempfile::tempdir().unwrap();
    let proof_path = workspace.path().join("proof.txt");
    std::fs::write(&proof_path, "typed-bypass-read-succeeded").unwrap();

    let mut config = bootstrap_config();
    config.tools.auto_approve = false;
    let capture = Arc::new(CapturingSink::default());
    let sink: Arc<dyn OutputSink> = capture.clone();
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .with_smart_execution_policy(ApprovalPolicy::Bypass, PolicySource::LocalCliLaunch)
        .provider(Arc::new(MockLlmProvider::with_tool_use(
            "typed-read",
            "Read",
            json!({"file_path": proof_path.to_string_lossy()}),
        )))
        .without_channels(true)
        .build()
        .await
        .expect("typed approval bypass must retain an enforceable sandbox");

    assert_ne!(
        result.engine.tools().sandbox_runtime().backend_name(),
        "no_sandbox",
        "Smart/Bypass must not imply sandbox bypass"
    );

    result
        .engine
        .run("read the proof file", "typed-bypass-read")
        .await
        .expect("typed Bypass must execute a model-issued Read");

    assert!(
        capture.has_success_containing("Read", "typed-bypass-read-succeeded"),
        "the cleared allow-list makes this success dependent on typed Bypass"
    );
}

#[tokio::test]
async fn typed_bypass_executes_bash_inside_required_sandbox() {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = bootstrap_config();
    config.tools.auto_approve = false;
    let capture = Arc::new(CapturingSink::default());
    let sink: Arc<dyn OutputSink> = capture.clone();
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .with_smart_execution_policy(ApprovalPolicy::Bypass, PolicySource::LocalCliLaunch)
        .provider(Arc::new(MockLlmProvider::with_tool_use(
            "typed-bash",
            "Bash",
            json!({"command": "printf typed-bypass-bash-succeeded"}),
        )))
        .without_channels(true)
        .build()
        .await
        .expect("typed approval bypass must retain an enforceable sandbox");

    assert_ne!(
        result.engine.tools().sandbox_runtime().backend_name(),
        "no_sandbox",
        "Smart/Bypass must retain process containment"
    );
    result
        .engine
        .run("run the harmless proof command", "typed-bypass-bash")
        .await
        .expect("typed Bypass must execute sandboxed Bash");

    assert!(
        capture.has_success_containing("Bash", "typed-bypass-bash-succeeded"),
        "Bash must execute through the required sandbox runtime"
    );
}

#[tokio::test]
async fn typed_prompt_overrides_legacy_auto_approve_and_denies_write() {
    let workspace = tempfile::tempdir().unwrap();
    let write_path = workspace.path().join("must-not-exist.txt");

    let mut config = bootstrap_config();
    config.tools.auto_approve = true;
    let capture = Arc::new(CapturingSink::default());
    let sink: Arc<dyn OutputSink> = capture.clone();
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .with_smart_execution_policy(ApprovalPolicy::Prompt, PolicySource::LocalCliLaunch)
        .provider(Arc::new(MockLlmProvider::with_tool_use(
            "typed-prompt-write",
            "Write",
            json!({
                "file_path": write_path.to_string_lossy(),
                "content": "typed Prompt was ignored"
            }),
        )))
        .without_channels(true)
        .build()
        .await
        .expect("typed Prompt must override the legacy auto-approve boolean");

    result
        .engine
        .run("write the requested file", "typed-prompt-write")
        .await
        .expect("a denied Write should return a tool result, not abort the turn");

    assert!(
        !write_path.exists(),
        "typed Prompt must prevent the model-issued Write in noninteractive execution"
    );
    assert!(
        capture.has_denial_for("Write"),
        "the model must receive an explicit denial for the gated Write"
    );
}

#[tokio::test]
async fn typed_auto_edit_allows_write_but_denies_bash() {
    let workspace = tempfile::tempdir().unwrap();
    let write_path = workspace.path().join("auto-edit.txt");

    let mut write_config = bootstrap_config();
    write_config.tools.auto_approve = false;
    let write_capture = Arc::new(CapturingSink::default());
    let write_sink: Arc<dyn OutputSink> = write_capture.clone();
    let mut write_result =
        AgentBootstrap::new(write_config, workspace.path().to_string_lossy(), write_sink)
            .with_smart_execution_policy(ApprovalPolicy::AutoEdit, PolicySource::LocalCliLaunch)
            .provider(Arc::new(MockLlmProvider::with_tool_use(
                "typed-auto-edit-write",
                "Write",
                json!({
                    "file_path": write_path.to_string_lossy(),
                    "content": "typed-auto-edit-write-succeeded"
                }),
            )))
            .without_channels(true)
            .build()
            .await
            .expect("typed AutoEdit bootstrap must succeed");

    write_result
        .engine
        .run("write the requested file", "typed-auto-edit-write")
        .await
        .expect("AutoEdit must execute a model-issued Write");

    assert_eq!(
        std::fs::read_to_string(&write_path).unwrap(),
        "typed-auto-edit-write-succeeded",
        "typed AutoEdit must authorize edit-category tools"
    );
    assert!(
        write_capture.has_success_for("Write"),
        "the Write tool must report successful execution"
    );

    let mut bash_config = bootstrap_config();
    bash_config.tools.auto_approve = false;
    let bash_marker = workspace.path().join("auto-edit-bash-must-not-exist.txt");
    let bash_capture = Arc::new(CapturingSink::default());
    let bash_sink: Arc<dyn OutputSink> = bash_capture.clone();
    let mut bash_result =
        AgentBootstrap::new(bash_config, workspace.path().to_string_lossy(), bash_sink)
            .with_smart_execution_policy(ApprovalPolicy::AutoEdit, PolicySource::LocalCliLaunch)
            .provider(Arc::new(MockLlmProvider::with_tool_use(
                "typed-auto-edit-bash",
                "Bash",
                json!({
                    "command": format!(
                        "echo typed-auto-edit-bash-must-not-run > \"{}\"",
                        bash_marker.display()
                    )
                }),
            )))
            .without_channels(true)
            .build()
            .await
            .expect("typed AutoEdit bootstrap must succeed");

    bash_result
        .engine
        .run("run the requested command", "typed-auto-edit-bash")
        .await
        .expect("a denied Bash should return a tool result, not abort the turn");

    assert!(
        bash_capture.has_denial_for("Bash"),
        "typed AutoEdit must continue to gate exec-category tools"
    );
    assert!(
        !bash_marker.exists(),
        "a denied Bash must not create side effects"
    );
}

#[tokio::test]
async fn smart_and_dangerous_authority_cannot_be_combined() {
    let workspace = tempfile::tempdir().unwrap();
    let baseline = BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default);
    let grant = resolve_dangerous_launch(
        &baseline,
        DangerousLaunchRequest::cli(60, "mixed-authority-test"),
        0,
    )
    .unwrap();
    let sink: Arc<dyn OutputSink> = Arc::new(CapturingSink::default());

    let result = AgentBootstrap::new(bootstrap_config(), workspace.path().to_string_lossy(), sink)
        .with_smart_execution_policy(ApprovalPolicy::Prompt, PolicySource::LocalCliLaunch)
        .with_dangerous_grant(grant)
        .without_channels(true)
        .build()
        .await;
    let error = match result {
        Ok(_) => panic!("mixed Smart and Dangerous authority must fail closed"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("cannot combine Smart policy with a Dangerous grant")
    );
}
