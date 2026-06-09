//! E2E-HARNESS FOUNDATION — the mock-LLM backend + parser-conformance gate.
//!
//! Three deliverables, each compiled and tested here:
//!
//! 1. **base_url → mock-LLM spike.** Start the [`MockLlm`] HTTP server on
//!    `127.0.0.1:0`, point the *real* `wcore_providers::anthropic` provider at
//!    its `base_url`, drive one request, and assert the provider yields the
//!    expected `LlmEvent`s ending in a clean `Done { stop_reason }`. This
//!    proves the same mock can later drive the real `wayland-core` binary
//!    (which POSTs to `{base_url}/v1/messages`, anthropic.rs:176).
//!
//! 2. **Parser-conformance gate.** Feed the canonical text-turn and
//!    tool_use-turn fixtures through the *real* parser in-process and assert
//!    they are accepted without being flagged as truncated (which would
//!    trigger an engine retry). These assertions live in
//!    `support/mock_llm.rs`'s self-tests *and* are re-exercised here against
//!    the live-socket path.
//!
//! 3. **Reusable `MockLlm` builder.** Scriptable multi-turn (text / tool_use)
//!    Anthropic SSE, validated against the real parser so it can never drift.
//!
//! The mock self-tests in `support/mock_llm.rs` run as unit tests of that
//! module; this file covers the live HTTP integration path end to end.

#[path = "support/mod.rs"]
mod support;

use serde_json::json;
use support::mock_llm::{MockLlm, parse_with_real_parser, text_turn_sse, tool_use_turn_sse};

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason};

/// A minimal user-turn request. No tools, no thinking — the mock decides the
/// shape of the response, not the request.
fn minimal_request() -> LlmRequest {
    LlmRequest {
        model: "claude-mock".to_string(),
        system: "You are a test harness.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hi".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 256,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
    }
}

/// Point the real Anthropic provider at the mock's base_url with a dummy key.
fn provider_against(base_url: &str) -> AnthropicProvider {
    AnthropicProvider::new(
        "sk-ant-harness-not-real-key",
        base_url,
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    )
    // Caching off: the harness asserts on event shape, not cache headers.
    .with_cache(false)
}

/// Drain the provider's event channel to completion.
async fn collect(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    events
}

// ===========================================================================
// Deliverable 1 — base_url spike: real provider driven by the mock server.
// ===========================================================================

/// The core bet: a `MockLlm` bound to a real socket can drive the real
/// `AnthropicProvider` through one full text turn, producing a clean terminal
/// `Done`. If this passes, the same mock can later back the live binary.
#[tokio::test]
async fn spike_real_provider_consumes_mock_text_turn() {
    let mock = MockLlm::new().text("Hello from the mock!");
    let server = mock.start().await;

    let provider = provider_against(&server.uri());
    let rx = provider
        .stream(&minimal_request())
        .await
        .expect("provider.stream against mock base_url should succeed");
    let events = collect(rx).await;

    // The provider must emit the text delta the mock scripted.
    let text: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::TextDelta(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(
        text,
        vec![&"Hello from the mock!".to_string()],
        "provider should surface the mock's text; events: {events:?}"
    );

    // Exactly one clean terminal Done with end_turn — NOT an Error (which is
    // what truncation would produce).
    let dones: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::Done { .. }))
        .collect();
    assert_eq!(
        dones.len(),
        1,
        "expected exactly one Done; events: {events:?}"
    );
    match dones[0] {
        LlmEvent::Done { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::EndTurn),
        _ => unreachable!(),
    }
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "a well-formed turn must not surface an Error; events: {events:?}"
    );
}

/// The mock also drives a real **tool_use** turn end to end: the provider
/// accumulates the split `input_json_delta` fragments and emits one `ToolUse`
/// with the reassembled input, terminated by `Done(tool_use)`.
#[tokio::test]
async fn spike_real_provider_consumes_mock_tool_use_turn() {
    let input = json!({ "command": "echo hi && ls" });
    let mock = MockLlm::new().tool_use("Bash", input.clone());
    let server = mock.start().await;

    let provider = provider_against(&server.uri());
    let rx = provider
        .stream(&minimal_request())
        .await
        .expect("stream should succeed");
    let events = collect(rx).await;

    let tools: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, LlmEvent::ToolUse { .. }))
        .collect();
    assert_eq!(tools.len(), 1, "expected one ToolUse; events: {events:?}");
    match tools[0] {
        LlmEvent::ToolUse {
            name, input: got, ..
        } => {
            assert_eq!(name, "Bash");
            assert_eq!(got, &input, "provider must reassemble the split tool input");
        }
        _ => unreachable!(),
    }

    match events
        .iter()
        .find(|e| matches!(e, LlmEvent::Done { .. }))
        .expect("a Done event")
    {
        LlmEvent::Done { stop_reason, .. } => assert_eq!(*stop_reason, StopReason::ToolUse),
        _ => unreachable!(),
    }
}

/// Multi-turn script: the mock serves each queued turn in order across
/// successive POSTs, exactly as the agent loop would request them.
#[tokio::test]
async fn spike_mock_serves_multi_turn_script_in_order() {
    let mock = MockLlm::new()
        .text("turn one")
        .tool_use("Read", json!({ "file_path": "/etc/hosts" }))
        .text("turn three");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());

    // Turn 1: text.
    let e1 = collect(provider.stream(&minimal_request()).await.unwrap()).await;
    assert!(
        e1.iter()
            .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "turn one")),
        "first POST should serve the text turn; got {e1:?}"
    );

    // Turn 2: tool_use.
    let e2 = collect(provider.stream(&minimal_request()).await.unwrap()).await;
    assert!(
        e2.iter()
            .any(|e| matches!(e, LlmEvent::ToolUse { name, .. } if name == "Read")),
        "second POST should serve the tool_use turn; got {e2:?}"
    );

    // Turn 3: text again.
    let e3 = collect(provider.stream(&minimal_request()).await.unwrap()).await;
    assert!(
        e3.iter()
            .any(|e| matches!(e, LlmEvent::TextDelta(t) if t == "turn three")),
        "third POST should serve the final text turn; got {e3:?}"
    );
}

// ===========================================================================
// Deliverable 2 — parser-conformance gate against the LIVE provider path.
// ===========================================================================

/// The canonical text fixture, when streamed through the *real provider's*
/// SSE machinery (not just the in-process parser), produces no Error — i.e.
/// the provider does not treat it as truncated.
#[tokio::test]
async fn conformance_text_fixture_drives_clean_done_through_provider() {
    // In-process gate: the fixture is what the real parser accepts.
    let parsed = parse_with_real_parser(&text_turn_sse("conformance"));
    assert!(!parsed.is_truncated(), "text fixture must not be truncated");
    assert!(parsed.done().is_some(), "text fixture must yield one Done");

    // Live-socket gate: the same bytes through the real provider stream.
    let mock = MockLlm::new().text("conformance");
    let server = mock.start().await;
    let events = collect(
        provider_against(&server.uri())
            .stream(&minimal_request())
            .await
            .unwrap(),
    )
    .await;
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::Done { .. })),
        "text fixture must terminate in Done over the live path; got {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "text fixture must not surface an Error (no retry); got {events:?}"
    );
}

/// The canonical tool_use fixture is conformant both in-process and over the
/// live provider path.
#[tokio::test]
async fn conformance_tool_use_fixture_drives_clean_done_through_provider() {
    let input = json!({ "pattern": "fn main", "path": "src" });

    let parsed = parse_with_real_parser(&tool_use_turn_sse("Grep", &input));
    assert!(
        !parsed.is_truncated(),
        "tool_use fixture must not be truncated"
    );
    assert!(
        parsed.done().is_some(),
        "tool_use fixture must yield one Done"
    );

    let mock = MockLlm::new().tool_use("Grep", input);
    let server = mock.start().await;
    let events = collect(
        provider_against(&server.uri())
            .stream(&minimal_request())
            .await
            .unwrap(),
    )
    .await;
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::ToolUse { .. })),
        "tool_use fixture must surface a ToolUse over the live path; got {events:?}"
    );
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "tool_use fixture must not surface an Error; got {events:?}"
    );
}
