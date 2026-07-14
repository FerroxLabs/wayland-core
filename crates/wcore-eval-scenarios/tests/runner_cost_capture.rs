//! Regression test for #278 — R-009 vs S4 cost-event divergence.
//!
//! The bug: `drive_session` in `runner.rs` only captured `session_cost`
//! in the post-stop drain, but the engine emits `session_cost` before
//! `stream_end`. The per-turn event loop broke on `stream_end` and never
//! retained the earlier cost event, so `result.cost_usd` stayed `0.0`.

use std::time::Duration;

use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::run_with_binary;
use wcore_eval_scenarios::scenario::{Category, Scenario, Turn};

fn fixture() -> &'static std::path::Path {
    std::path::Path::new(env!("CARGO_BIN_EXE_wcore-eval-fixture"))
}

/// Drive the real runner against the compiled fixture, which emits
/// `session_cost` before `stream_end`, and verify the cost survives.
#[tokio::test]
async fn captures_session_cost_emitted_before_stream_end() {
    let scenario = Scenario::new(
        "captures_session_cost_emitted_before_stream_end",
        Category::Coverage,
    )
    .max_total_time(Duration::from_secs(10))
    .turn(
        Turn::new("anything")
            .max_time(Duration::from_secs(5))
            .max_steps(1),
    );

    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-cost")
        .with_api_key("test-key-never-used".to_string());

    let result = run_with_binary(&scenario, &provider, fixture())
        .await
        .expect("run completes");

    assert!(
        result.cost_usd > 0.0,
        "#278 regression: runner failed to capture `session_cost` event \
         emitted before `stream_end`. Got cost_usd={}; failures={:?}",
        result.cost_usd,
        result.failures
    );
    assert!(
        (result.cost_usd - 0.01).abs() < 1e-9,
        "expected $0.01, got ${}",
        result.cost_usd
    );
    let usage = result
        .execution
        .provider_usage
        .as_ref()
        .expect("stream_end usage must be retained");
    assert_eq!(usage.input_tokens, 1);
    assert_eq!(usage.output_tokens, 1);
    assert_eq!(usage.cache_read_tokens, 0);
    assert_eq!(usage.cache_write_tokens, 0);
}
