//! Fixture-replay tests for `LlmParaphraseProvider` — Wave PA / W10B.1.
//!
//! These tests feed a recorded sequence of `LlmEvent`s through a hand-rolled
//! `LlmProvider` mock and assert the adapter extracts the expected paraphrase
//! text byte-identically. They exercise the streaming-collapse logic, the
//! per-call timeout, and the error-event boundary that maps a provider
//! failure into `ParaphraseProvider::paraphrase_blocking` returning `Err(_)`.
//!
//! Live-provider integration is gated behind `--features network-tests` and
//! `#[ignore]`; see `llm_paraphrase_live_integration_smoke` at the bottom of
//! this file.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_evolve::mutator::{
    AsyncParaphrase, LlmParaphraseError, LlmParaphraseProvider, Mutator, Paraphrase,
    ParaphraseProvider,
};
use wcore_evolve::mutator::{MutationKind, MutationSeed};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Hand-rolled mock `LlmProvider` that replays a recorded sequence of events.
/// Each `stream()` call pops the next script off the queue; callers can preload
/// multiple scripts to drive several requests in sequence.
struct ScriptedProvider {
    scripts: Mutex<Vec<Vec<LlmEvent>>>,
}

impl ScriptedProvider {
    fn new(scripts: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            scripts: Mutex::new(scripts),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let script = {
            let mut q = self.scripts.lock().expect("scripts mutex poisoned");
            if q.is_empty() {
                return Err(ProviderError::Connection(
                    "scripted provider exhausted".to_string(),
                ));
            }
            q.remove(0)
        };
        let (tx, rx) = mpsc::channel(script.len().max(1));
        tokio::spawn(async move {
            for event in script {
                let _ = tx.send(event).await;
            }
            // Closing the sender signals EOF; if the script omitted a Done
            // event, the adapter will surface `StreamEndedEarly`.
        });
        Ok(rx)
    }
}

/// Provider that never produces any event — exercises the request timeout.
struct StalledProvider;

#[async_trait]
impl LlmProvider for StalledProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // Channel is created but no sender task is spawned, so `recv()` on
        // the receiver hangs forever until the adapter's timeout fires.
        let (tx, rx) = mpsc::channel::<LlmEvent>(1);
        // Keep the sender alive so the channel doesn't close.
        std::mem::forget(tx);
        Ok(rx)
    }
}

fn done_event() -> LlmEvent {
    LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: TokenUsage::default(),
    }
}

#[tokio::test]
async fn llm_paraphrase_extracts_text_from_streamed_response() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("Hello".into()),
        LlmEvent::TextDelta(", world".into()),
        LlmEvent::TextDelta("!".into()),
        done_event(),
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let result = pp
        .paraphrase_async("input text")
        .await
        .expect("paraphrase returned an error");
    assert_eq!(result, "Hello, world!");
}

#[tokio::test]
async fn llm_paraphrase_concatenates_many_text_deltas_in_order() {
    let mut events: Vec<LlmEvent> = (0..20)
        .map(|i| LlmEvent::TextDelta(format!("chunk-{i:02} ")))
        .collect();
    events.push(done_event());
    let provider = Arc::new(ScriptedProvider::new(vec![events]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let result = pp.paraphrase_async("x").await.expect("ok");
    let expected: String = (0..20).map(|i| format!("chunk-{i:02} ")).collect();
    // Adapter trims trailing whitespace.
    assert_eq!(result, expected.trim());
}

#[tokio::test]
async fn llm_paraphrase_ignores_thinking_and_tool_use_events() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::ThinkingDelta("internal reasoning".into()),
        LlmEvent::TextDelta("real ".into()),
        LlmEvent::ToolUse {
            id: "x".into(),
            name: "bash".into(),
            input: serde_json::json!({"cmd": "echo"}),
            extra: None,
        },
        LlmEvent::TextDelta("output".into()),
        done_event(),
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let result = pp.paraphrase_async("x").await.expect("ok");
    assert_eq!(result, "real output");
}

#[tokio::test]
async fn llm_paraphrase_returns_error_on_provider_error_event() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("partial".into()),
        LlmEvent::Error("rate limited".into()),
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let err = pp.paraphrase_async("x").await.expect_err("must error");
    assert!(
        matches!(err, LlmParaphraseError::ProviderEvent(ref m) if m.contains("rate limited")),
        "unexpected error variant: {err:?}",
    );
}

#[tokio::test]
async fn llm_paraphrase_returns_stream_ended_early_when_done_missing() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("dangling".into()),
        // No Done event — channel closes after the last send.
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let err = pp.paraphrase_async("x").await.expect_err("must error");
    assert!(
        matches!(err, LlmParaphraseError::StreamEndedEarly),
        "unexpected error variant: {err:?}",
    );
}

#[tokio::test]
async fn llm_paraphrase_returns_empty_error_on_whitespace_only_response() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("   ".into()),
        LlmEvent::TextDelta("\n\n".into()),
        done_event(),
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture");
    let err = pp.paraphrase_async("x").await.expect_err("must error");
    assert!(
        matches!(err, LlmParaphraseError::Empty),
        "unexpected error variant: {err:?}",
    );
}

#[tokio::test]
async fn llm_paraphrase_times_out_on_stalled_provider() {
    let provider: Arc<dyn LlmProvider> = Arc::new(StalledProvider);
    let pp = LlmParaphraseProvider::new(provider, "claude-haiku-fixture")
        .with_request_timeout(Duration::from_millis(50));
    let err = pp.paraphrase_async("x").await.expect_err("must time out");
    assert!(
        matches!(err, LlmParaphraseError::Timeout(_)),
        "unexpected error variant: {err:?}",
    );
}

#[tokio::test]
async fn llm_paraphrase_surfaces_provider_stream_error_as_provider_variant() {
    // Empty script queue → ScriptedProvider returns ProviderError::Connection
    // on the first call. The adapter must surface it as
    // `LlmParaphraseError::Provider(_)` (via `#[from]`).
    let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider::new(Vec::new()));
    let pp = LlmParaphraseProvider::new(provider, "claude-haiku-fixture");
    let err = pp.paraphrase_async("x").await.expect_err("must error");
    assert!(
        matches!(err, LlmParaphraseError::Provider(_)),
        "unexpected error variant: {err:?}",
    );
}

#[tokio::test]
async fn llm_paraphrase_respects_custom_system_prompt() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("ok".into()),
        done_event(),
    ]]));
    let pp = LlmParaphraseProvider::new(provider as Arc<dyn LlmProvider>, "claude-haiku-fixture")
        .with_system_prompt("CUSTOM PROMPT")
        .with_max_tokens(64);
    // Smoke: builder doesn't crash and runs end-to-end.
    let result = pp.paraphrase_async("x").await.expect("ok");
    assert_eq!(result, "ok");
}

/// Round-trips through the **synchronous** `ParaphraseProvider::paraphrase_blocking`
/// surface that `Generation::run` actually calls. We deliberately invoke it
/// from inside a `spawn_blocking` task — that's the runtime context the loop
/// produces, and the adapter's `Handle::block_on` bridge has to work there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paraphrase_blocking_runs_inside_spawn_blocking() {
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("rewritten body".into()),
        done_event(),
    ]]));
    let pp: Arc<dyn ParaphraseProvider> = Arc::new(LlmParaphraseProvider::new(
        provider as Arc<dyn LlmProvider>,
        "claude-haiku-fixture",
    ));
    let result = tokio::task::spawn_blocking(move || pp.paraphrase_blocking("input", "seed-token"))
        .await
        .expect("blocking task panicked")
        .expect("paraphrase_blocking returned an error");
    assert_eq!(result, "rewritten body");
}

/// End-to-end through the `Paraphrase` mutator. Verifies that a non-identity
/// `LlmParaphraseProvider` produces non-identity Paraphrase children — the
/// acceptance criterion from Wave PA: "GEPA evolution loop, when configured
/// with `LlmParaphraseProvider`, produces non-identity Paraphrase children."
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paraphrase_mutator_produces_non_identity_child_with_real_adapter() {
    let parent_body = "## Preconditions\n- a\n\n## Steps\n- one\n- two\n";
    let provider = Arc::new(ScriptedProvider::new(vec![vec![
        LlmEvent::TextDelta("## Preconditions\n- a\n\n## Steps\n- ONE (rewritten)\n- TWO\n".into()),
        done_event(),
    ]]));
    let real_pp: Arc<dyn ParaphraseProvider> = Arc::new(LlmParaphraseProvider::new(
        provider as Arc<dyn LlmProvider>,
        "claude-haiku-fixture",
    ));
    let parent_body_owned = parent_body.to_string();
    let mutation = tokio::task::spawn_blocking(move || {
        let mutator = Paraphrase {
            provider: real_pp,
            temperature: 0.0,
        };
        mutator.mutate(&parent_body_owned, MutationSeed::new("parent-hash", 0, 0))
    })
    .await
    .expect("blocking task panicked")
    .expect("mutate returned an error");
    assert_eq!(mutation.kind, MutationKind::Paraphrase);
    assert_ne!(
        mutation.body, parent_body,
        "real provider must produce non-identity child"
    );
    assert!(mutation.body.contains("ONE (rewritten)"));
}

// ---------------------------------------------------------------------------
// Live-LLM smoke test (network-tests feature, #[ignore] by default).
// ---------------------------------------------------------------------------
//
// Run manually with:
//   ANTHROPIC_API_KEY=... \
//     cargo nextest run -p wcore-evolve --features network-tests \
//       --run-ignored only -- llm_paraphrase_live_integration_smoke
//
// Skips itself if `ANTHROPIC_API_KEY` is unset so CI matrices that flip the
// feature on by accident don't fail with auth errors.

#[cfg(feature = "network-tests")]
#[tokio::test]
#[ignore = "live-network test; requires ANTHROPIC_API_KEY"]
async fn llm_paraphrase_live_integration_smoke() {
    use wcore_config::compat::ProviderCompat;
    use wcore_config::debug::DebugConfig;
    use wcore_providers::anthropic::AnthropicProvider;

    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        eprintln!("skipping: ANTHROPIC_API_KEY not set");
        return;
    };

    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
        &api_key,
        "https://api.anthropic.com",
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ));
    let pp = LlmParaphraseProvider::new(provider, "claude-3-5-haiku-latest")
        .with_request_timeout(Duration::from_secs(30));
    let result = pp
        .paraphrase_async("Read the file before editing it.")
        .await
        .expect("live paraphrase failed");
    assert!(!result.trim().is_empty(), "live paraphrase returned empty");
    assert_ne!(
        result.trim(),
        "Read the file before editing it.",
        "live paraphrase should not be byte-identical to input"
    );
}
