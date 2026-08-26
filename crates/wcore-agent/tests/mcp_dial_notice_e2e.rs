//! The boot MCP dial must not take a session's opening seconds in silence.
//!
//! End-to-end through the real `AgentBootstrap::build()`, against a real
//! stdio server that never speaks MCP — `sleep`, which is the reproducer that
//! produced the measurement this exists for: on 0.13.8 a `message` sent with
//! one such server configured put NOTHING on the wire for 30.3 s, then
//! `mcp_failed`, then `stream_start`.
//!
//! Not a unit test of the notice helper (that lives beside it in
//! `mcp_dial_notice.rs`). This is the WIRING: the surfaces that reach
//! `build()` — headless `-p`, the line REPL, ACP `session/new`, and the
//! per-conversation channel dispatch — all inherit whatever this call site
//! does, and none of them has a splash to hide behind.
//!
//! Real clock on purpose. `start_paused` would let the runtime skip the
//! deadline, and the thing under test is a race between a real subprocess
//! that never answers and a real timer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::{TestSink, TestSinkHandle};
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, McpServerConfig, ProviderType, TransportType};

/// A server that accepts the spawn and then never says anything. The dial
/// cannot distinguish this from a server still starting up, which is exactly
/// why it has a deadline and exactly why the deadline needs announcing.
fn config_with_a_server_that_never_speaks() -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    config.mcp.servers = HashMap::from([(
        "mute".to_string(),
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("sleep".to_string()),
            args: Some(vec!["3600".to_string()]),
            env: None,
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
            only_for_assistant: None,
        },
    )]);
    config
}

fn dial_notices(events: &TestSinkHandle) -> Vec<String> {
    events
        .snapshot()
        .iter()
        .filter(|event| event["type"].as_str() == Some("info"))
        .filter_map(|event| event["message"].as_str().map(str::to_string))
        .filter(|message| message.contains("Still waiting on MCP servers"))
        .collect()
}

#[tokio::test]
async fn a_boot_dial_that_hangs_announces_itself_instead_of_going_quiet() {
    let sink = TestSink::new();
    let events = sink.handle();
    let output: Arc<dyn OutputSink> = Arc::new(sink);
    let workdir = tempfile::TempDir::new().expect("workdir");

    let started = Instant::now();
    let result = AgentBootstrap::new(
        config_with_a_server_that_never_speaks(),
        workdir.path().to_str().expect("utf8 workdir"),
        output,
    )
    .build()
    .await;
    let took = started.elapsed();

    // The dial is non-fatal per server: a mute server is skipped, not a
    // launch failure. That is the pre-existing contract and is not what this
    // test is about — it is here so a regression to "the build now fails"
    // reads as itself rather than as a missing notice.
    assert!(
        result.is_ok(),
        "a server that never speaks must be skipped, not fail the launch: {:?}",
        result.err()
    );

    let notices = dial_notices(&events);
    assert_eq!(
        notices.len(),
        1,
        "the user must be told exactly once that the launch is waiting on MCP; \
         the dial took {took:?} and said {notices:?}"
    );
    assert!(
        notices[0].contains(&format!(
            "{}s",
            wcore_mcp::manager::CONNECT_TIMEOUT.as_secs()
        )),
        "the notice must name the deadline it is counting towards, got {:?}",
        notices[0]
    );

    // Bracketing the wait pins that the notice landed INSIDE the silent
    // window rather than after it, where it would be a receipt instead of a
    // warning. The dial must genuinely have outrun the notice budget, and it
    // must still be bounded by the per-server deadline.
    assert!(
        took >= wcore_agent::mcp_dial_notice::MCP_DIAL_NOTICE_AFTER,
        "the fixture did not actually produce a slow dial: {took:?}"
    );
    assert!(
        took < wcore_mcp::manager::CONNECT_TIMEOUT * 3,
        "the dial ran far past its own per-server deadline: {took:?}"
    );
}
