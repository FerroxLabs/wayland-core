//! W8b D.3: When `FileWatcher` records an external edit between turns,
//! the engine drains the events at the per-turn boundary and injects
//! the synthetic context message produced by `render_external_edit_message`
//! into `self.messages` as a synthetic User-role context block so the
//! next turn's LlmRequest carries it.
//!
//! **v0.9.1.1 F7**: the prior version of this hook also emitted the
//! same message as an `Info` event on the protocol stream, which the
//! TUI bridge translates into a transcript system turn. A `cargo fmt`
//! burst (or any IDE save-storm) could thereby pour hundreds of paths
//! into the user's transcript. The `emit_info` mirror has been
//! removed; bus events flow ONLY to `tracing::info!` on
//! `wcore_agent::watch`. The User-role inject continues unchanged so
//! the LLM still sees the edited-files context on its next turn.
//!
//! Test shape: a single-turn ScriptedProvider with a manually-attached
//! `FileWatcher`; an external edit performed before `run_synthetic_turn`
//! is invoked; we then assert the captured event stream does NOT
//! contain a stray transcript-leak Info event AND the engine's
//! message history contains a User-role block whose text matches the
//! `render_external_edit_message` output.

use std::sync::Arc;
use std::time::Duration;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::watch::FileWatcher;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(2),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn single_text_script(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

#[tokio::test]
async fn external_edit_does_not_leak_to_transcript_v0911() {
    // v0.9.1.1 F7: the engine used to mirror the synthetic
    // "User edited N files…" message via `emit_info`, which lands as
    // a transcript system turn in the TUI. A `cargo fmt` burst could
    // pour hundreds of paths into the user's view. The fix demotes
    // the mirror to `tracing::info!` only; the User-role inject into
    // `self.messages` still feeds the LLM the next-turn context.
    //
    // This test asserts the negative: no `Info` event escapes through
    // the protocol stream even when an external edit happens.
    let tmp = tempfile::tempdir().expect("tempdir");
    let watcher = Arc::new(FileWatcher::new(tmp.path()).expect("watcher"));

    let (mut engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(), single_text_script("ack"));
    engine.set_file_watcher(Arc::clone(&watcher));

    // Allow notify to finish arming (FSEvents on macOS is async).
    tokio::time::sleep(Duration::from_millis(80)).await;

    // External edit — happens BEFORE the engine drives a turn.
    let target = tmp.path().join("foo.txt");
    tokio::fs::write(&target, b"edited externally")
        .await
        .expect("external write");

    // Wait for notify to surface the fs event into the channel.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Drive one synthetic turn. The engine should drain the watcher at
    // the per-turn boundary and silently inject the User-role message
    // WITHOUT emitting a transcript-leaking `Info` event.
    let _out = engine
        .run_synthetic_turn("read foo.txt")
        .await
        .expect("synthetic turn should succeed");

    let events = engine.captured_protocol_events();
    let has_leak = events.iter().any(|e| {
        e["type"] == "info"
            && e["message"]
                .as_str()
                .map(|s| s.contains("re-read") || s.contains("while I was thinking"))
                .unwrap_or(false)
    });
    assert!(
        !has_leak,
        "external-edit notice must NOT escape as an Info event; events: {events:?}"
    );
}

#[tokio::test]
async fn no_external_edit_emits_no_synthetic_message() {
    // Negative case: a watcher attached with zero events between turns
    // must NOT emit a synthetic Info event. This pins the gating logic.
    let tmp = tempfile::tempdir().expect("tempdir");
    let watcher = Arc::new(FileWatcher::new(tmp.path()).expect("watcher"));

    let (mut engine, _handle) =
        AgentBootstrap::build_for_test(minimal_config(), single_text_script("ack"));
    engine.set_file_watcher(Arc::clone(&watcher));

    tokio::time::sleep(Duration::from_millis(80)).await;

    let _out = engine
        .run_synthetic_turn("hello")
        .await
        .expect("synthetic turn should succeed");

    let events = engine.captured_protocol_events();
    let has_synthetic_msg = events.iter().any(|e| {
        e["type"] == "info"
            && e["message"]
                .as_str()
                .map(|s| s.contains("re-read"))
                .unwrap_or(false)
    });
    assert!(
        !has_synthetic_msg,
        "no external edits → no synthetic Info; events: {events:?}"
    );
}
