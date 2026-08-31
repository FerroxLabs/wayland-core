//! FerroxLabs/wayland#1218 — the ask that actually reaches the wire.
//!
//! #1218's arithmetic is about "the `max_tokens` core actually sends", and
//! `size_output_cap` cannot send anything: the value on the wire is whatever
//! the turn loop passes it and whatever it does with the result. Every test
//! written for this ticket so far drives the helper DIRECTLY with an explicit
//! `Some(window)`, so all of them stay green if the one production call site
//! (`engine.rs`, `request.max_tokens = size_output_cap(.., window_in_force)`)
//! ever passes `None` again — which is exactly the state the ticket was filed
//! against.
//!
//! This test drives the REAL `AgentEngine` turn loop with a capturing provider
//! and reads `LlmRequest::max_tokens` off the request the engine hands the
//! provider, so the wiring is graded rather than assumed.
//!
//! RED ARM: passing `None` for `window_in_force` at that call site — the
//! pre-#1179 behaviour — sends `UNKNOWN_CAP` = 8,192 on an 8,192-token window,
//! and this test fails while the in-crate unit tests all still pass.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::test_utils::TestSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// A model id no arm of `model_output_ceiling` matches — the unlisted arm
/// #1172 measured, served over an OpenAI-compatible endpoint (which is NOT
/// omit-safe, so the field IS sent).
const UNLISTED_MODEL: &str = "issue-1218-local-8k-unlisted";
const PROVIDER_TYPE: &str = "openai-compat";
/// The window in force for the session, via `[compact] context_window` — the
/// operator-set arm of the same reconciliation #1172's learned window feeds.
const WINDOW: usize = 8_192;
/// `size_output_cap`'s own headroom constant, kept free for prompt growth.
const WINDOW_BUFFER: usize = 512;
/// The conservative floor an unlisted model is sized to before any window is
/// taken into account. This is the value the ticket measured on the wire.
const UNKNOWN_CAP: u32 = 8_192;

/// Records the `max_tokens` on every request the engine sends.
struct CapturingProvider {
    asks: Arc<Mutex<Vec<u32>>>,
}

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.asks.lock().unwrap().push(request.max_tokens);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _ = tx.send(LlmEvent::TextDelta("ok".to_string())).await;
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: TokenUsage {
                        input_tokens: 40,
                        output_tokens: 2,
                        ..Default::default()
                    },
                })
                .await;
        });
        Ok(rx)
    }
}

fn config() -> Config {
    let mut config = Config {
        provider_label: PROVIDER_TYPE.into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: UNLISTED_MODEL.into(),
        // Generous user cap: the CAP must not be what bounds the ask here, or
        // the test would pass without the window ever being consulted.
        max_tokens: 64_000,
        max_turns: Some(1),
        compat: ProviderCompat {
            provider_type: Some(PROVIDER_TYPE.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    config.compact.context_window = Some(WINDOW);
    config
}

#[tokio::test]
async fn the_ask_on_the_wire_fits_the_window_in_force() {
    // Control: this test is about the UNLISTED arm. If the model is ever
    // catalogued the case it grades has moved, and the test says so rather
    // than passing on the wrong arm.
    assert!(
        wcore_config::limits::model_output_ceiling(PROVIDER_TYPE, UNLISTED_MODEL).is_none(),
        "control: {UNLISTED_MODEL} must have no catalogued output ceiling"
    );

    let asks = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider {
        asks: Arc::clone(&asks),
    });
    let sink = Arc::new(TestSink::new());
    let output: Arc<dyn OutputSink> = sink;
    let mut engine =
        AgentEngine::new_with_provider(provider, config(), ToolRegistry::new(), output);
    engine
        .run("hello", "")
        .await
        .expect("the turn runs: an 8,192-token window is workable");

    let asks = asks.lock().unwrap().clone();
    // Control: silence must not read as success. If the turn never reached the
    // provider there is no ask to grade and the assertions below are vacuous.
    assert!(
        !asks.is_empty(),
        "control: the engine must have sent at least one request"
    );
    for ask in asks {
        assert!(
            ask < UNKNOWN_CAP,
            "the ask that reached the wire is {ask}, the conservative unlisted \
             floor, on a {WINDOW}-token window: the turn loop is sizing the \
             output without the window in force"
        );
        assert!(
            ask as usize <= WINDOW - WINDOW_BUFFER,
            "the ask that reached the wire is {ask}, which leaves no room in \
             the {WINDOW}-token window for the input it is sent with"
        );
    }
}
