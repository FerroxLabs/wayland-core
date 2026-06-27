// M2.6: Entire file is gated behind the `live-openai` cargo feature. Default
// CI does not compile or run any of these tests. Enable manually with:
//   cargo nextest run -p wcore-agent --features live-openai --test e2e
// With the feature ON, a missing `OPENAI_API_KEY` PANICS — the contract is
// "you asked for live tests; you must supply the key" — rather than the
// previous silent skip which masked credential-misconfiguration failures.
#![cfg(feature = "live-openai")]

use std::sync::Arc;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType, SessionConfig, ToolsConfig};
use wcore_providers::create_provider;
use wcore_tools::read::ReadTool;
use wcore_tools::registry::ToolRegistry;

fn openai_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
}

fn openai_config(api_key: &str) -> Config {
    Config {
        provider: ProviderType::OpenAI,
        provider_label: "openai".to_string(),
        api_key: api_key.to_string(),
        base_url: "https://api.openai.com".to_string(),
        model: crate::common::models::openai_gpt4o_mini(), // cheapest for e2e
        max_tokens: 256,
        max_turns: Some(3),
        system_prompt: Some("You are a helpful assistant. Be concise.".to_string()),
        compat: ProviderCompat::openai_defaults(),
        tools: ToolsConfig {
            auto_approve: true,
            allow_list: vec![],
            skills: wcore_config::config::SkillsPermissionConfig::default(),
            verify_edits: false,
            windows_shell: None,
            env_passthrough: Vec::new(),
            sandbox: None,
            allow_no_sandbox: None,
        },
        session: SessionConfig {
            enabled: false,
            directory: "/tmp".to_string(),
            max_sessions: 1,
        },
        ..Default::default()
    }
}

/// Smoke test: single-turn text completion.
#[tokio::test]
async fn test_openai_single_turn_completion() {
    let api_key = openai_api_key().expect(
        "[e2e] OPENAI_API_KEY required when --features live-openai is enabled \
         (wcore-agent/live-openai)",
    );

    let config = openai_config(&api_key);
    let provider = create_provider(&config);
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let registry = ToolRegistry::new();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Say 'hello world' and nothing else.", "")
        .await
        .expect("engine.run should not fail");

    assert!(!result.text.is_empty(), "response text should not be empty");
    assert!(result.usage.output_tokens > 0);

    eprintln!(
        "[e2e] openai single-turn: {} tokens in / {} out",
        result.usage.input_tokens, result.usage.output_tokens
    );
}

/// Tool-use smoke test: agent calls Read tool when asked to read a file.
#[tokio::test]
async fn test_openai_tool_use() {
    let api_key = openai_api_key().expect(
        "[e2e] OPENAI_API_KEY required when --features live-openai is enabled \
         (wcore-agent/live-openai)",
    );

    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    std::fs::write(tmp.path(), "e2e-openai-content-99").expect("write tempfile");
    let path = tmp.path().to_string_lossy().to_string();

    let config = openai_config(&api_key);
    let provider = create_provider(&config);
    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadTool::new(None)));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let prompt = format!(
        "Read the file at '{}' and tell me what it contains. Be brief.",
        path
    );
    let result = engine
        .run(&prompt, "")
        .await
        .expect("engine.run should not fail");

    assert!(!result.text.is_empty());
    assert!(
        result.text.contains("e2e-openai-content-99") || result.turns > 1,
        "model should echo the content or use multiple turns: {}",
        result.text
    );

    eprintln!(
        "[e2e] openai tool-use: {} turns, {} tokens out",
        result.turns, result.usage.output_tokens
    );
}
