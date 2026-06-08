//! Integration test: PiiScrubbingSink redacts credentials in emitted traces.

use std::sync::Arc;

use serde_json::{Value, json};
use wcore_observability::sink::{InMemorySink, PiiScrubbingSink, SpanSink};

/// Helper: emit `trace` through a `PiiScrubbingSink` backed by an
/// `InMemorySink`, then return the single captured value.
fn scrub_emit(trace: Value) -> Value {
    let inner = Arc::new(InMemorySink::new());
    let scrubbing: Arc<dyn SpanSink> = Arc::new(PiiScrubbingSink::wrap(inner.clone()));
    scrubbing.emit(&trace);
    inner
        .snapshot()
        .into_iter()
        .next()
        .expect("one trace must be emitted")
}

#[test]
fn anthropic_key_in_trace_is_redacted() {
    let raw_key = "sk-ant-api03-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
    let trace = json!({ "api_key": raw_key });
    let result = scrub_emit(trace);
    let result_str = serde_json::to_string(&result).unwrap();
    assert!(
        !result_str.contains(raw_key),
        "raw Anthropic key must not appear in output"
    );
    assert!(
        result_str.contains("[REDACTED:ANTHROPIC_API_KEY]"),
        "REDACTED marker must appear: {result_str}"
    );
}

#[test]
fn openai_key_in_trace_is_redacted() {
    let raw_key = "sk-abcdefghijklmnopqrstuvwxyz012345";
    let trace = json!({ "message": format!("got key: {raw_key}") });
    let result = scrub_emit(trace);
    let result_str = serde_json::to_string(&result).unwrap();
    assert!(
        !result_str.contains(raw_key),
        "raw OpenAI key must not appear in output"
    );
    assert!(
        result_str.contains("[REDACTED:"),
        "REDACTED marker must appear: {result_str}"
    );
}

#[test]
fn clean_trace_passes_through_unchanged() {
    let trace = json!({ "turn": 1, "model": "claude-3-5-sonnet", "cost_usd": 0.002 });
    let result = scrub_emit(trace.clone());
    assert_eq!(result, trace, "clean trace must be forwarded unchanged");
}

#[test]
fn bearer_token_in_trace_is_redacted() {
    let trace = json!({ "auth": "Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijk" });
    let result = scrub_emit(trace);
    let result_str = serde_json::to_string(&result).unwrap();
    assert!(
        !result_str.contains("eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijk"),
        "raw bearer token must not appear in output"
    );
    assert!(
        result_str.contains("[REDACTED:"),
        "REDACTED marker must appear: {result_str}"
    );
}

#[test]
fn scrubbing_sink_forwards_to_inner() {
    // Verify the inner sink actually receives the (scrubbed) trace — i.e.
    // nothing is silently dropped.
    let inner = Arc::new(InMemorySink::new());
    let scrubbing: Arc<dyn SpanSink> = Arc::new(PiiScrubbingSink::wrap(inner.clone()));
    scrubbing.emit(&json!({ "x": 1 }));
    scrubbing.emit(&json!({ "x": 2 }));
    assert_eq!(inner.len(), 2, "both traces must reach the inner sink");
}
