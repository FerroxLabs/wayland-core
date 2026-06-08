//! W4 (Task 8): ScriptTool is registered iff config.builtin_tools.script.enabled
//! is true AND the engine flips advertised_capabilities.rpc_tool_script.
//! Default off; turning Script on is a deliberate wcore-config decision.

use std::sync::Arc;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

#[tokio::test]
async fn script_tool_absent_by_default() {
    let config = minimal_config();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap");
    let names = result.engine.tool_names();
    assert!(
        !names.contains(&"Script".to_string()),
        "Script must be absent by default; got {names:?}"
    );
    assert!(
        !result.engine.advertised_capabilities().rpc_tool_script,
        "rpc_tool_script must be false by default"
    );
}

#[tokio::test]
async fn script_tool_present_when_config_flag_on() {
    let mut config = minimal_config();
    config.builtin_tools.script.enabled = true;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap");
    let names = result.engine.tool_names();
    assert!(
        names.contains(&"Script".to_string()),
        "Script must be present when enabled; got {names:?}"
    );
}

#[tokio::test]
async fn capability_advertises_rpc_tool_script_when_enabled() {
    let mut config = minimal_config();
    config.builtin_tools.script.enabled = true;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .build()
        .await
        .expect("bootstrap");
    assert!(
        result.engine.advertised_capabilities().rpc_tool_script,
        "advertised_capabilities.rpc_tool_script must be true when Script enabled"
    );
}
