//! F23-04: the ENGINE actually writes the cache/compaction ledger.
//!
//! `crates/wcore-cli/tests/cache_ledger_cli.rs` proves the operator surface
//! renders a ledger correctly, but it hands that surface a ledger written by
//! the test. That leaves the load-bearing half unproven: **does a real
//! `AgentEngine::run()` produce one at all, with the right numbers in it?**
//!
//! Everything here drives `AgentEngine::run()` end to end against a mock
//! provider that reports real `TokenUsage` (including cache read/write
//! counters), then reads the JSON file off disk and asserts on it. Nothing is
//! read out of the engine's memory — if the flush path is broken, these fail.
//!
//! The ledger directory is injected via `set_cache_ledger_dir` rather than
//! `WAYLAND_HOME`, because a process-global env var makes a parallel test
//! suite order-dependent (and this workspace runs its tests in parallel).

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use wcore_agent::cache_ledger::{CacheLedger, CostTruth};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// Mock provider replaying a fixed script of per-round-trip events.
struct ScriptedProvider {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
}

impl ScriptedProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(VecDeque::from(turns)),
        }
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                usage: TokenUsage::default(),
            }]
        });
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for e in events {
                let _ = tx.send(e).await;
            }
        });
        Ok(rx)
    }
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenUsage {
    TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_write,
        ..Default::default()
    }
}

/// A round-trip that calls a tool, so the agent loop continues to the next one.
fn tool_turn(call_id: &str, u: TokenUsage) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: call_id.to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: u,
        },
    ]
}

/// A terminal round-trip.
fn text_turn(text: &str, u: TokenUsage) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: u,
        },
    ]
}

fn registry_with_mock_tool() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Box::new(common::MockTool::new("mock_tool", "ok", false)));
    r
}

/// Read the single ledger the engine wrote into `dir`.
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
        "expected exactly one ledger in {}, found {files:?}",
        dir.display()
    );
    let raw = std::fs::read(&files[0]).unwrap();
    serde_json::from_slice(&raw).unwrap_or_else(|e| panic!("ledger is not decodable: {e}"))
}

// ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn a_real_run_writes_a_ledger_with_one_row_per_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    // Cold open (writes cache, reads nothing), then two warm round-trips that
    // read the prefix back.
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_turn("t1", usage(20_000, 300, 0, 20_000)),
        tool_turn("t2", usage(500, 300, 20_000, 0)),
        text_turn("done", usage(500, 300, 20_500, 0)),
    ]));

    let mut config = test_config();
    // A model the pricing catalog knows, so `priced` is true and the USD
    // figures are facts rather than floors.
    config.model = "claude-opus-4-7".to_string();

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        silent_output(),
    );
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "msg-1").await.expect("run should succeed");

    let ledger = read_only_ledger(dir.path());
    assert_eq!(
        ledger.turns.len(),
        3,
        "one row per LLM round-trip, not per agent turn: {:#?}",
        ledger.turns
    );
    assert!(
        ledger.session_complete,
        "the session ended cleanly, so the ledger must say so"
    );
    assert_eq!(
        ledger
            .turns
            .iter()
            .map(|t| t.round_trip)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    // Quality — read straight off the wire counters.
    assert_eq!(ledger.turns[0].cache_read_tokens, 0);
    assert_eq!(ledger.turns[0].cache_write_tokens, 20_000);
    assert!(!ledger.turns[0].is_hit());
    assert!(ledger.turns[1].is_hit());
    assert!(ledger.turns[2].is_hit());

    let s = ledger.summarize();
    assert_eq!(s.round_trips, 3);
    assert_eq!(s.cache_read_tokens, 40_500);
    assert_eq!(s.cache_write_tokens, 20_000);
    assert_eq!(s.hit_round_trips, 2);
    assert_eq!(s.miss_round_trips, 1);
    assert!(
        s.hit_ratio() > 0.6,
        "hit ratio should be dominated by the two warm reads, got {}",
        s.hit_ratio()
    );

    // The model actually dispatched is recorded, so cost is attributable.
    assert!(
        ledger.turns.iter().all(|t| t.model == "claude-opus-4-7"),
        "models: {:?}",
        ledger.turns.iter().map(|t| &t.model).collect::<Vec<_>>()
    );
    assert!(ledger.turns.iter().all(|t| t.provider == "anthropic"));
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn recorded_cost_varies_with_the_tokens_and_beats_the_uncached_counterfactual() {
    // The specific defect this guards: a cost observable that reports the same
    // number regardless of what happened. Two engines, identical in every way
    // except the token counts their provider reports, must produce different
    // costs — and the cached run must come out cheaper than its own uncached
    // counterfactual.
    async fn spend(dir: &std::path::Path, scale: u64) -> (f64, f64) {
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_turn("t1", usage(10 * scale, 300, 0, 10 * scale)),
            text_turn("done", usage(scale, 300, 10 * scale, 0)),
        ]));
        let mut config = test_config();
        config.model = "claude-opus-4-7".to_string();
        let mut engine = AgentEngine::new_with_provider(
            provider,
            config,
            registry_with_mock_tool(),
            silent_output(),
        );
        engine.set_cache_ledger_dir(dir);
        engine.run("go", "m").await.expect("run should succeed");
        let s = read_only_ledger(dir).summarize();
        (s.cost_usd, s.uncached_equivalent_usd)
    }

    let small_dir = tempfile::tempdir().unwrap();
    let large_dir = tempfile::tempdir().unwrap();
    let (small, small_uncached) = spend(small_dir.path(), 1_000).await;
    let (large, large_uncached) = spend(large_dir.path(), 100_000).await;

    assert!(small > 0.0, "a priced model must produce a non-zero cost");
    assert!(
        large > small * 50.0,
        "cost is INVARIANT or barely moving across a 100x workload: {small} -> {large}"
    );

    // The counterfactual is priced through the same catalog, and re-billing the
    // cache reads as ordinary input must cost strictly more.
    assert!(
        small_uncached > small && large_uncached > large,
        "the uncached counterfactual must exceed the billed cost: \
         {small_uncached} vs {small}, {large_uncached} vs {large}"
    );
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn an_uncatalogued_model_is_recorded_unpriced_rather_than_free() {
    // `test_config()`'s default model is not in the pricing catalog. A ledger
    // that recorded that as `$0.00 priced` would be indistinguishable from a
    // genuinely free session — the exact failure the CostTruth grade exists to
    // prevent, proved here through the engine rather than a fixture.
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(50_000, 1_000, 0, 0),
    )]));
    let config = test_config();
    assert_eq!(
        config.model, "test-model",
        "this test depends on the default model being uncatalogued"
    );

    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), silent_output());
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "m").await.expect("run should succeed");

    let s = read_only_ledger(dir.path()).summarize();
    assert_eq!(s.round_trips, 1);
    assert_eq!(s.cost_truth(), CostTruth::Unpriced);
    assert!(!s.cost_truth().is_trustworthy());
    assert_eq!(s.unpriced_round_trips, 1);
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn token_pressure_is_recorded_against_the_thresholds_the_engine_acts_on() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_turn("t1", usage(120_000, 300, 0, 0)),
        text_turn("done", usage(130_000, 300, 0, 0)),
    ]));
    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    // Compaction OFF so the watermark is free to climb and this test measures
    // the pressure recording rather than the compactor.
    config.compact.enabled = false;
    config.compact.context_window = 200_000;
    config.compact.output_reserve = 20_000;
    config.compact.autocompact_buffer = 13_000;
    config.compact.emergency_buffer = 3_000;

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        silent_output(),
    );
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "m").await.expect("run should succeed");

    let ledger = read_only_ledger(dir.path());
    let t = &ledger.turns[0];
    // 200_000 - 20_000 - 13_000 and 200_000 - 3_000: the SAME arithmetic
    // `should_autocompact` and `is_at_emergency_limit` test against.
    assert_eq!(t.autocompact_threshold_tokens, 167_000);
    assert_eq!(t.emergency_limit_tokens, 197_000);
    assert!(
        t.watermark_tokens >= 120_000,
        "the provider-reported watermark did not reach the ledger: {}",
        t.watermark_tokens
    );

    let s = ledger.summarize();
    assert!(
        s.peak_watermark_tokens >= 130_000,
        "peak watermark {} should reflect the larger of the two round-trips",
        s.peak_watermark_tokens
    );
    assert!(
        s.peak_pressure_ratio() > 0.7 && s.peak_pressure_ratio() < 1.0,
        "pressure {} should be a fraction of the threshold, not a raw count",
        s.peak_pressure_ratio()
    );

    // Known-negative for the pressure figure: a LOW-token session must report
    // a much smaller ratio through the same code path. Without this, a
    // constant would satisfy the bounds above.
    let quiet_dir = tempfile::tempdir().unwrap();
    let quiet = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(1_000, 100, 0, 0),
    )]));
    let mut qconfig = test_config();
    qconfig.model = "claude-opus-4-7".to_string();
    qconfig.compact.enabled = false;
    let mut qengine =
        AgentEngine::new_with_provider(quiet, qconfig, ToolRegistry::new(), silent_output());
    qengine.set_cache_ledger_dir(quiet_dir.path());
    qengine.run("go", "m").await.expect("run should succeed");
    let qs = read_only_ledger(quiet_dir.path()).summarize();
    assert!(
        qs.peak_pressure_ratio() < 0.1,
        "a 1k-token session reported pressure {} — the figure is not varying",
        qs.peak_pressure_ratio()
    );
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn the_kill_switch_stops_the_ledger_being_written_at_all() {
    // `WAYLAND_CACHE_LEDGER=0` must produce NO file. This is the assertion that
    // proves the other tests are observing a file the engine really wrote,
    // rather than one the harness or a stale run left behind.
    //
    // Uses a process-global env var, so it runs in its own test binary process
    // only if the suite is single-threaded; to stay safe it asserts on a
    // directory no other test in this file touches, and restores the variable.
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded within this test; the variable is restored below
    // and no other test in this binary reads it.
    unsafe { std::env::set_var("WAYLAND_CACHE_LEDGER", "0") };

    let provider = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(1_000, 100, 0, 0),
    )]));
    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), silent_output());
    engine.set_cache_ledger_dir(dir.path());
    let outcome = engine.run("go", "m").await;

    unsafe { std::env::remove_var("WAYLAND_CACHE_LEDGER") };
    outcome.expect("run should succeed with the ledger off");

    let files: Vec<_> = std::fs::read_dir(dir.path())
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    assert!(
        files.is_empty(),
        "the kill switch left files behind: {files:?}"
    );
}
