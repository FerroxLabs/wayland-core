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

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test otlp_local_test`
/// executes 0 of 1 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// case were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1` to declare a run of the
/// ignored case while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 1 #[ignore]d case, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-observability --test otlp_local_test -- --ignored --test-threads=1"
    );
}
