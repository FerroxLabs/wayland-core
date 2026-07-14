//! Cross-platform production proof that approval bypass and containment are
//! independent controls.

mod common;

use std::sync::{Arc, Mutex};

use common::MockLlmProvider;
use serde_json::json;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::message::FinishReason;

fn bootstrap_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 64,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

#[derive(Default)]
struct CapturingSink {
    tool_results: Mutex<Vec<(String, bool, String)>>,
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
async fn approval_bypass_retains_required_sandbox_and_tool_function() {
    let workspace = tempfile::tempdir().unwrap();
    let proof_path = workspace.path().join("proof.txt");
    std::fs::write(&proof_path, "contained-read-succeeded").unwrap();

    let mut config = bootstrap_config();
    config.tools.auto_approve = true;
    let capture = Arc::new(CapturingSink::default());
    let sink: Arc<dyn OutputSink> = capture.clone();
    let mut result = AgentBootstrap::new(config, workspace.path().to_string_lossy(), sink)
        .provider(Arc::new(MockLlmProvider::with_tool_use(
            "read-proof",
            "Read",
            json!({"file_path": proof_path}),
        )))
        .without_channels(true)
        .build()
        .await
        .expect("approval bypass must retain an enforceable sandbox");

    assert_ne!(
        result.engine.tools().sandbox_runtime().backend_name(),
        "no_sandbox",
        "approval bypass must not imply containment bypass"
    );

    result
        .engine
        .run("read the proof file", "approval-sandbox-proof")
        .await
        .expect("a harmless approved tool must remain functional under containment");

    let tool_results = capture.tool_results.lock().unwrap();
    assert!(
        tool_results.iter().any(|(name, is_error, content)| {
            name == "Read" && !is_error && content.contains("contained-read-succeeded")
        }),
        "the model-issued Read must execute successfully without disabling containment"
    );
}
