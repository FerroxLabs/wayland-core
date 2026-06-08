//! Local OTLP smoke test. Skipped unless `WCORE_OTLP_TEST_ENDPOINT` is set,
//! at which point it constructs an `OtlpSink`, emits one trace, and asserts
//! that construction + emission don't error.
//!
//! Run manually with:
//!     cargo nextest run -p wcore-observability --features otlp \
//!         --test otlp_local_test --run-ignored ignored-only
//!
//! Local Jaeger UI: `docker run --rm -p 16686:16686 -p 4318:4318 jaegertracing/all-in-one`
//! then export WCORE_OTLP_TEST_ENDPOINT=http://localhost:4318/v1/traces.

#![cfg(feature = "otlp")]

use serde_json::json;
use wcore_observability::sink::{OtlpSink, SpanSink};

#[tokio::test]
#[ignore = "requires WCORE_OTLP_TEST_ENDPOINT and a running OTLP collector"]
async fn otlp_sink_emits_against_local_collector() {
    let endpoint = std::env::var("WCORE_OTLP_TEST_ENDPOINT")
        .expect("set WCORE_OTLP_TEST_ENDPOINT=http://localhost:4318/v1/traces");
    let sink =
        OtlpSink::new(&endpoint).expect("sink must construct against a reachable local collector");
    sink.emit(&json!({
        "turn": 0,
        "model": "test",
        "provider": "test",
        "input_tokens": 100,
        "output_tokens": 50,
        "cache_read": 0,
        "cache_write": 0,
        "cache_hit_rate": 0.0,
        "cost_usd": 0.0,
        "tool_calls": [],
        "hook_actions": [],
        "source_product": "wayland-core"
    }));
}
