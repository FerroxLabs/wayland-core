//! v0.9.1.2 F-watermark regression: token watermark override stays in
//! tracing::debug, never the transcript.
//!
//! Background: `AgentEngine::run()` runs a per-turn loop. After each
//! provider response the engine compares `turn_usage.input_tokens` to a
//! local estimate over `self.messages` and uses the max as the
//! autocompact watermark. When the local estimate exceeds the
//! provider-reported count by more than 10k tokens (DeepSeek prefix
//! caching, anything that under-reports prompt_tokens), the engine
//! recorded a `"Token watermark override: provider=X, local_estimate=Y,
//! using=Y"` line via `self.output.emit_info(...)`.
//!
//! `emit_info` flows into `ProtocolEvent::Info`, which the TUI bridge
//! routes to `push_system` — a transcript message that forces a full
//! re-render. Under heavy tool use one user prompt can spawn 5-10
//! per-turn LLM round-trips, each re-tripping the >10k delta condition.
//! Sean's v0.9.1.1 screenshots showed 6+ watermark lines wedged between
//! a single user prompt and the assistant reply. The constant
//! re-renders were the dominant source of the "molasses" responsiveness
//! complaint.
//!
//! Fix (engine.rs ~ line 2524, see commit): route the watermark log
//! straight to `tracing::debug!` so the data still reaches `/doctor`
//! output and log files but never enters the transcript event stream.
//!
//! Same precedent as v0.9.1.2 F10's plugin-hook lifecycle classifier
//! (`run_post_tool_use` records into `hook_trace`, never `log_lines`):
//! filter at the SOURCE, not at the protocol bridge.

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_config::compact::CompactConfig;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Build a config that lets `run()` complete one turn cleanly. Disables
/// autocompact so the watermark check is reached but doesn't kick off a
/// summary call (which would emit additional Info events unrelated to
/// the watermark and complicate the assertion surface).
fn config_for_watermark_test() -> Config {
    let mut cfg = Config {
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
    // Keep the watermark logic live but make sure autocompact + emergency
    // never fire on this small synthetic conversation.
    cfg.compact = CompactConfig {
        enabled: false,
        ..CompactConfig::default()
    };
    cfg
}

/// Script: one Done turn that REPORTS only 500 input_tokens. With a
/// large user prompt (~50k chars = ~12.5k tokens) the local estimator
/// will see >10k delta over the provider count and trip the watermark
/// override condition. If the fix is in place no Info event with the
/// watermark string will be recorded.
fn underreporting_done(input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("ok".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                input_tokens,
                output_tokens: 1,
                ..Default::default()
            },
        },
    ]
}

/// Primary regression: trip the >10k delta condition end-to-end and
/// assert no system message (ProtocolEvent::Info) carries the watermark
/// override text.
#[tokio::test]
async fn token_watermark_does_not_leak_to_transcript_v0913() {
    let (mut engine, handle) =
        AgentBootstrap::build_for_test(config_for_watermark_test(), underreporting_done(500));

    // 50_000 characters of input → ~12_500 estimated tokens, comfortably
    // above provider_reported (500) + 10_000 buffer, so the condition
    // at engine.rs:2524 trips on this turn.
    let big_input = "a".repeat(50_000);
    let _ = engine
        .run_synthetic_turn(&big_input)
        .await
        .expect("synthetic turn should succeed");

    let events = handle.snapshot();

    // Cardinal assertion: no captured event of any type may carry the
    // watermark override string. If the fix regresses (someone reaches
    // for `self.output.emit_info(...)` again) this will catch it.
    let leaks: Vec<&serde_json::Value> = events
        .iter()
        .filter(|ev| {
            ev["message"]
                .as_str()
                .map(|m| m.contains("Token watermark override"))
                .unwrap_or(false)
        })
        .collect();

    assert!(
        leaks.is_empty(),
        "watermark override line leaked into the protocol event stream \
         (transcript channel) — should be tracing::debug! only. \
         Leaked events: {leaks:#?}\nFull captured stream: {events:#?}"
    );

    // Belt-and-suspenders: also check Info-typed events directly in
    // case a future refactor reshapes the leak into a non-`message`
    // field on a different variant.
    let info_leaks: Vec<&serde_json::Value> = events
        .iter()
        .filter(|ev| ev["type"].as_str() == Some("info"))
        .filter(|ev| {
            ev["message"]
                .as_str()
                .map(|m| m.contains("watermark"))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        info_leaks.is_empty(),
        "Info event mentioning watermark leaked to transcript: {info_leaks:#?}"
    );
}

/// Companion: the fix must KEEP recording the data for diagnostics. We
/// don't capture tracing output in-process (no `tracing-test` dep in
/// this workspace as of 2026-05-28), so we verify the fix indirectly:
/// the engine source MUST emit a `tracing::debug!` at the watermark
/// override site. A grep-style assertion is fragile but factual — if
/// the source ever loses the tracing emit, this test fails and forces
/// the author to reconcile.
///
/// Stronger end-to-end coverage would require wiring a
/// `tracing_subscriber::fmt::TestWriter` into the test harness and
/// asserting on captured log output; that's tracked as a v0.9.1.3
/// followup. For now this guards against the most plausible
/// regression (deleting the log entirely while migrating off emit_info).
#[test]
fn token_watermark_routed_to_tracing_debug_v0913() {
    let source = include_str!("../src/engine.rs");

    // The watermark override block must contain a tracing::debug! call,
    // and must NOT call self.output.emit_info with the watermark string.
    let watermark_idx = source
        .find("Token watermark override")
        .expect("engine.rs must still contain the watermark override log");

    // Inspect a window around the emit site large enough to cover the
    // whole if-block but tight enough to not bleed into unrelated code.
    let window_start = source[..watermark_idx]
        .rfind("if local_estimate > turn_usage.input_tokens")
        .expect("watermark guard if-block must precede the log line");
    let window_end = watermark_idx + 400;
    let window = &source[window_start..window_end.min(source.len())];

    assert!(
        window.contains("tracing::debug!"),
        "watermark override site must log via tracing::debug! (telemetry channel). \
         Found window:\n{window}"
    );
    assert!(
        !window.contains("self.output.emit_info"),
        "watermark override site must NOT call self.output.emit_info \
         (that channel feeds the transcript). Found window:\n{window}"
    );
}
