//! #1139 wiring: a per-call cost the PROVIDER reports on the wire must reach
//! the cost ledger.
//!
//! The bug was not that `TokenUsage` could not hold the figure — it was that
//! `usage.cost_usd` was never read off the SSE stream at all
//! (`git grep -in cost_usd origin/main -- crates/wcore-providers/` returned
//! zero across 30+ provider files), so a real spend arrived at the ledger as a
//! pure-pricing estimate, or — with no catalog row for the model — as
//! `$0.000000`.
//!
//! So this drives the WHOLE path: a real `OpenAIProvider` against a wiremock
//! SSE body, through `process_sse_stream`, into `AgentEngine::run`, out to the
//! ledger JSON on disk, and asserts on `LedgerSummary`. A test that built a
//! `TokenUsage` by hand would pass with the provider-side parse deleted and
//! would therefore grade nothing.

mod common;

use std::sync::Arc;

use serde_json::{Value, json};
use wcore_agent::cache_ledger::{CacheLedger, CostSource, CostTruth};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::ProviderType;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::openai::OpenAIProvider;
use wcore_tools::registry::ToolRegistry;
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::test_config;

/// The figure the mock provider bills. Deliberately not a round number and not
/// derivable from any token count in the stream, so it can only have come off
/// the wire.
const BILLED_USD: f64 = 0.041_337;

/// A model no `wcore-pricing` catalog row knows. Without the provider's own
/// figure this session is `CostTruth::Unpriced` at `$0.000000` — which is the
/// exact failure #1139 reports.
const UNPRICED_MODEL: &str = "mystery-router-model-1139";

/// One OpenAI-wire chat stream: some text, a `finish_reason`, then the trailing
/// usage-only chunk. `cost_usd` rides on the usage object when `billed` is set.
fn openai_sse(text: &str, billed: Option<f64>) -> String {
    let chunk = json!({
        "id": "chatcmpl-1139", "object": "chat.completion.chunk",
        "created": 0, "model": UNPRICED_MODEL,
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": Value::Null }]
    });
    let stop = json!({
        "id": "chatcmpl-1139", "object": "chat.completion.chunk",
        "created": 0, "model": UNPRICED_MODEL,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    });
    let mut usage = json!({
        "prompt_tokens": 1_200,
        "completion_tokens": 340,
        "prompt_tokens_details": { "cached_tokens": 200 }
    });
    if let Some(usd) = billed {
        usage["cost_usd"] = json!(usd);
    }
    let final_chunk = json!({
        "id": "chatcmpl-1139", "object": "chat.completion.chunk",
        "created": 0, "model": UNPRICED_MODEL,
        "choices": [],
        "usage": usage
    });
    format!("data: {chunk}\n\ndata: {stop}\n\ndata: {final_chunk}\n\ndata: [DONE]\n\n")
}

async fn start_mock(body: String) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wm_path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    server
}

fn read_only_ledger(dir: &std::path::Path) -> CacheLedger {
    let files: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("no ledger directory at {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert_eq!(
        files.len(),
        1,
        "expected exactly one ledger, found {files:?}"
    );
    serde_json::from_slice(&std::fs::read(&files[0]).unwrap())
        .unwrap_or_else(|e| panic!("ledger is not decodable: {e}"))
}

/// Drive one real turn against `body` and return the ledger the engine wrote.
async fn run_turn_and_read_ledger(body: String) -> CacheLedger {
    let server = start_mock(body).await;
    let dir = tempfile::tempdir().unwrap();

    let compat = ProviderCompat::openai_defaults();
    let provider: Arc<dyn LlmProvider> = Arc::new(OpenAIProvider::new(
        "cost-wiring-test-key",
        &server.uri(),
        compat.clone(),
        DebugConfig::default(),
    ));

    let mut config = test_config();
    config.provider = ProviderType::OpenAI;
    config.provider_label = "openai".to_string();
    config.base_url = server.uri();
    config.model = UNPRICED_MODEL.to_string();
    config.compat = compat;

    let output: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    let mut engine = AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), output);
    engine.set_cache_ledger_dir(dir.path());
    engine
        .run("go", "msg-1139")
        .await
        .expect("the run succeeds");

    read_only_ledger(dir.path())
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn a_provider_reported_cost_on_the_sse_stream_reaches_the_ledger() {
    let ledger = run_turn_and_read_ledger(openai_sse("ok", Some(BILLED_USD))).await;
    assert_eq!(ledger.turns.len(), 1, "one round-trip: {:#?}", ledger.turns);

    let turn = &ledger.turns[0];
    assert!(
        (turn.cost_usd - BILLED_USD).abs() < 1e-9,
        "the round-trip must be recorded at the figure the provider billed, got {}",
        turn.cost_usd
    );
    assert_eq!(
        turn.cost_source,
        CostSource::ProviderReported,
        "provenance must say the PROVIDER supplied this, not a catalog row"
    );

    let s = ledger.summarize();
    assert!(
        (s.cost_usd - BILLED_USD).abs() < 1e-9,
        "LedgerSummary.cost_usd must carry the wire figure, got {}",
        s.cost_usd
    );
    assert_eq!(s.provider_reported_round_trips, 1);
    assert_eq!(
        s.cost_truth(),
        CostTruth::Priced,
        "a figure the provider billed is spend, so the total is a fact"
    );
    assert!(s.cost_truth().is_trustworthy());
}

/// THE CONTROL. Same model, same tokens, same everything — only `cost_usd` is
/// absent from the usage object. It must come back unpriced at `$0.00`, which
/// proves (a) the assertion above can fail, and (b) the figure it asserts on
/// really did travel from the wire rather than from any local pricing.
#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn the_control_without_a_reported_cost_stays_unpriced() {
    let ledger = run_turn_and_read_ledger(openai_sse("ok", None)).await;
    assert_eq!(ledger.turns.len(), 1);

    let turn = &ledger.turns[0];
    assert_ne!(
        turn.cost_source,
        CostSource::ProviderReported,
        "nothing was reported, so nothing may claim provider provenance"
    );
    assert_eq!(
        turn.cost_usd, 0.0,
        "no catalog row and no reported figure means no number at all"
    );

    let s = ledger.summarize();
    assert_eq!(s.provider_reported_round_trips, 0);
    assert_eq!(
        s.cost_truth(),
        CostTruth::Unpriced,
        "an unknown cost must grade as unpriced — and must NOT render as spend"
    );
}
