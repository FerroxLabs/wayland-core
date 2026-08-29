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

use wcore_agent::cache_ledger::{CacheLedger, CostSource, CostTruth};
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
    /// Real HTTP boundary the persisted-session tests route through, so the
    /// journal receives an authoritative physical-attempt identity before the
    /// scripted stream becomes visible. `None` for the in-memory tests.
    physical_url: Option<String>,
}

impl ScriptedProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(VecDeque::from(turns)),
            physical_url: None,
        }
    }

    fn with_physical_url(mut self, url: impl Into<String>) -> Self {
        self.physical_url = Some(url.into());
        self
    }
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        if let Some(url) = self.physical_url.as_deref() {
            let client = wcore_egress::EgressClient::new()
                .with_policy(Arc::new(wcore_egress::AllowAllPolicy));
            let response = wcore_providers::retry::scope_max_retries(
                0,
                wcore_providers::retry::builder_send_with_retry(client.get(url)),
            )
            .await?;
            if !response.status().is_success() {
                return Err(ProviderError::Api {
                    status: response.status().as_u16(),
                    message: "fixture response".into(),
                });
            }
        }
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
    // #1179 / #353 — the compactor is LEFT ON here, deliberately.
    //
    // This fixture's scripted usage is not a physically possible sequence:
    // round trip 1 reports 40,000 total input (20,000 uncached + 20,000 cache
    // WRITE) and round trip 2 reports 20,500, i.e. the prompt SHRANK by half
    // while messages were appended. That is precisely the signature #1172's
    // `ServedWindowTracker` reads as truncation, so it learns a served window
    // of 40,000 from it. Until #1179 that cost a spurious notice and nothing
    // else; once the learned window also moved the autocompact trigger (to
    // 18,001 on a 40,000 window) the fixture's own round trip 2 crossed it and
    // round trip 3 arrived behind a real summarization — `HistoryRewritten`,
    // zero cache read, and this test measuring compaction instead of the cache.
    // #1179 turned the compactor off to isolate the subject; #353 is the
    // general defect that made that necessary.
    //
    // It is back on because this is the one place in the tree where the #353
    // false positive is REAL rather than constructed: the anomaly is a single
    // observation, `Regression` now requires corroboration before it may size
    // the session, and so the trigger no longer moves. If that corroboration
    // rule is ever weakened, this test fails — which is worth more than the
    // isolation the disable bought.

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
    assert!(ledger.turns[2].is_hit(), "{:#?}", ledger.turns);
    // A cold open that successfully WROTE cache is normal, not an
    // invalidation, and must not be mislabelled `no_marker`.
    assert_eq!(
        ledger.turns[0].invalidation_cause, None,
        "a cold open that wrote cache is not an invalidation"
    );

    let s = ledger.summarize();
    assert_eq!(s.round_trips, 3);
    assert_eq!(s.cache_read_tokens, 40_500);
    assert_eq!(s.cache_write_tokens, 20_000);
    assert_eq!(s.hit_round_trips, 2);
    assert_eq!(s.miss_round_trips, 1);
    // 40_500 cache reads over 81_500 total input — the 20_000 cache WRITES on
    // the cold open are in the denominator on purpose. A first draft asserted
    // `> 0.6` on the assumption writes were excluded, and failed at 0.497; the
    // assertion was wrong, not the ratio.
    assert!(
        (s.hit_ratio() - 40_500.0 / 81_500.0).abs() < 1e-6,
        "hit ratio {} does not match cache_read / total_input",
        s.hit_ratio()
    );
    // The warm window excludes the two cold opening round-trips, so it must
    // read much better than the session average.
    assert_eq!(s.warm_round_trips, 1);
    assert!(
        s.warm_hit_ratio() > 0.97 && s.warm_hit_ratio() > s.hit_ratio(),
        "warm={} all={}",
        s.warm_hit_ratio(),
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
    // costs.
    //
    // The `spend` shape is READ-dominated (one small cache write, then many
    // reads) so its counterfactual is genuinely more expensive. A first draft
    // used a write-heavy shape and asserted the cache always saves; it failed
    // at 11.2575 billed vs 10.0075 uncached, because Anthropic's cache-write
    // rate is 1.25x input and a session that writes far more than it reads
    // really does cost MORE than an uncached one. That is a true fact about
    // prompt caching, not a bug — so both directions are asserted below,
    // each on a shape that actually exhibits it.
    // EVERY counter scales with `scale`, including outputs. An earlier draft
    // held the output tokens and the small per-turn inputs fixed, so a 100x
    // workload only moved cost 48x and the run came back red at `> 50x`. Making
    // the shape purely linear turns a fuzzy "it moved a lot" into an exact
    // ratio, which is a strictly stronger guard against an invariant number.
    async fn spend(dir: &std::path::Path, scale: u64) -> (f64, f64) {
        let provider = Arc::new(ScriptedProvider::new(vec![
            tool_turn("t1", usage(scale, scale, 0, scale)),
            tool_turn("t2", usage(scale, scale, 10 * scale, 0)),
            text_turn("done", usage(scale, scale, 10 * scale, 0)),
        ]));
        let mut config = test_config();
        config.model = "claude-opus-4-7".to_string();
        // Compaction off: this test measures cost, and a compaction pass would
        // consume a scripted turn and change the token totals underneath it.
        config.compact.enabled = false;
        // …and the emergency hard stop is ALWAYS on, independent of
        // `compact.enabled`. At scale=100_000 the watermark reaches 1M and the
        // run aborts with ContextTooLong before the third round-trip, which is
        // how this first came back red. Widen the window so the cost arithmetic
        // is what is under test here; `token_pressure_*` below covers the
        // thresholds themselves.
        config.compact.context_window = Some(100_000_000);
        let mut engine = AgentEngine::new_with_provider(
            provider,
            config,
            registry_with_mock_tool(),
            silent_output(),
        );
        engine.set_cache_ledger_dir(dir);
        engine.run("go", "m").await.expect("run should succeed");
        let s = read_only_ledger(dir).summarize();
        assert_eq!(s.round_trips, 3, "the script must run to completion");
        (
            s.cost_usd,
            s.uncached_equivalent_usd
                .expect("claude-opus-4-7 is catalogued, so the counterfactual is priced"),
        )
    }

    let small_dir = tempfile::tempdir().unwrap();
    let large_dir = tempfile::tempdir().unwrap();
    let (small, small_uncached) = spend(small_dir.path(), 1_000).await;
    let (large, large_uncached) = spend(large_dir.path(), 100_000).await;

    assert!(small > 0.0, "a priced model must produce a non-zero cost");
    let ratio = large / small;
    assert!(
        (ratio - 100.0).abs() < 0.01,
        "a 100x workload must cost 100x: {small} -> {large} is {ratio}x \
         (a constant would give 1.0; a partly-fixed number something between)"
    );

    // Read-dominated: the counterfactual, priced through the same catalog with
    // every cached token re-billed as input, must cost strictly more.
    assert!(
        small_uncached > small && large_uncached > large,
        "a read-dominated session must beat its uncached counterfactual: \
         {small_uncached} vs {small}, {large_uncached} vs {large}"
    );

    // Write-dominated: the OTHER direction, on a shape that exhibits it. This
    // is the known-negative for the assertion above — without it, a saving
    // hardcoded positive would pass.
    let loss_dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(1_000, 300, 0, 100_000),
    )]));
    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    config.compact.enabled = false;
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), silent_output());
    engine.set_cache_ledger_dir(loss_dir.path());
    engine.run("go", "m").await.expect("run should succeed");
    let loss = read_only_ledger(loss_dir.path()).summarize();
    assert!(
        loss.cache_saving_usd().is_some_and(|saving| saving < 0.0),
        "a session that writes 100k of cache and reads none must report a \
         NEGATIVE saving, got {:?} (billed {}, uncached {:?})",
        loss.cache_saving_usd(),
        loss.cost_usd,
        loss.uncached_equivalent_usd
    );
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn an_uncatalogued_model_is_recorded_as_an_estimate_not_as_spend() {
    // MEASURED FINDING, and the reason `CostSource` exists.
    //
    // `test_config()`'s model `test-model` is not in the pricing catalog. The
    // first draft of this test asserted the engine would record it UNPRICED.
    // It came back PRICED — `resolve_turn_cost` falls through to the
    // `ProviderCompat` family rate, so an unknown model dispatched to Anthropic
    // is billed at Anthropic's generic rate and reported with `priced = true`.
    //
    // That number is not a lie, but it is not spend either, and it had been
    // rendering identically to a catalog price. The ledger therefore records
    // the SOURCE, and this test pins the honest answer: `provider_defaults`,
    // grading the session `Estimated` and NOT trustworthy.
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

    let ledger = read_only_ledger(dir.path());
    assert_eq!(ledger.turns[0].cost_source, CostSource::ProviderDefaults);
    let s = ledger.summarize();
    assert_eq!(s.round_trips, 1);
    assert_eq!(s.cost_truth(), CostTruth::Estimated);
    assert!(
        !s.cost_truth().is_trustworthy(),
        "a family-rate estimate must not be presented as spend"
    );
    assert_eq!(s.estimated_round_trips, 1);
    assert_eq!(s.catalog_priced_round_trips, 0);

    // Known-negative: the SAME engine on a catalogued model must grade
    // `Priced`, so the grade above is reading the source rather than always
    // returning Estimated.
    let dir2 = tempfile::tempdir().unwrap();
    let provider2 = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(50_000, 1_000, 0, 0),
    )]));
    let mut config2 = test_config();
    config2.model = "claude-opus-4-7".to_string();
    let mut engine2 =
        AgentEngine::new_with_provider(provider2, config2, ToolRegistry::new(), silent_output());
    engine2.set_cache_ledger_dir(dir2.path());
    engine2.run("go", "m").await.expect("run should succeed");
    let s2 = read_only_ledger(dir2.path()).summarize();
    assert_eq!(s2.cost_truth(), CostTruth::Priced);
    assert!(s2.cost_truth().is_trustworthy());
}

#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn a_provider_with_no_prompt_cache_at_all_is_attributed_no_marker() {
    // MEASURED on a live local-Ollama session: round-trip 1 arrived at the
    // ledger with NO invalidation cause. `CacheBreakDetector` returns
    // `Healthy { hit_rate: 0.0 }` for the first request because it has nothing
    // to compare against, which makes `CacheBreakCause::FirstRequest`
    // unreachable from the engine — so the one round-trip that is guaranteed
    // to be a miss was the one round-trip nothing explained.
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![text_turn(
        "done",
        usage(4_000, 10, 0, 0),
    )]));
    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), silent_output());
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "m").await.expect("run should succeed");

    let ledger = read_only_ledger(dir.path());
    assert_eq!(
        ledger.turns[0].invalidation_cause,
        Some(wcore_providers::cache_observation::InvalidationCause::NoMarker),
        "an opening round-trip that neither read nor wrote cache must say why"
    );
    let s = ledger.summarize();
    assert_eq!(s.invalidation_causes.get("no_marker"), Some(&1));
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
    config.compact.context_window = Some(200_000);
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
    //
    // GH#635: `context_window = Some(200_000)` above is now load-bearing — it
    // is an EXPLICIT operator setting, which outranks claude-opus-4-7's real
    // 1,000,000-token window. Drop it and these become 967_000 / 997_000 (see
    // `gh635_ledger_reports_the_active_models_boundaries_not_the_200k_default`).
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

/// GH#635 — with NO configured `context_window`, the ledger must report the
/// boundaries of the model that is actually running, and they must be the
/// SAME numbers the enforcement path computes. A reported 167k/197k next to an
/// enforced 967k/997k would send an operator hunting a compaction that is
/// never going to fire.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: change
/// `config.effective_context_window(provider, model)` back to
/// `config.context_window` in either `autocompact_threshold`
/// (crates/wcore-agent/src/compact/auto.rs) or `emergency_limit`
/// (crates/wcore-agent/src/compact/emergency.rs) — the ledger reports
/// 167_000 / 197_000 and the equality assertions against the enforcement
/// functions break.
#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn gh635_ledger_reports_the_active_models_boundaries_not_the_200k_default() {
    use wcore_agent::compact::auto::autocompact_threshold;
    use wcore_agent::compact::emergency::emergency_limit;

    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(ScriptedProvider::new(vec![
        tool_turn("t1", usage(220_000, 300, 0, 0)),
        text_turn("done", usage(230_000, 300, 0, 0)),
    ]));
    let mut config = test_config();
    // A registry-known 1,000,000-token model, with `context_window` left
    // UNCONFIGURED so the registry supplies the window.
    config.model = "claude-opus-4-7".to_string();
    config.compact.enabled = false;
    assert_eq!(
        config.compact.context_window, None,
        "this test is only meaningful with an unconfigured window"
    );

    // The buffers stay at their defaults; snapshot them for the expected math.
    let expected_threshold = autocompact_threshold(&config.compact, "anthropic", &config.model);
    let expected_limit = emergency_limit(&config.compact, "anthropic", &config.model);
    assert_eq!(expected_threshold, 967_000, "1_000_000 - 20_000 - 13_000");
    assert_eq!(expected_limit, 997_000, "1_000_000 - 3_000");

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        registry_with_mock_tool(),
        silent_output(),
    );
    engine.set_cache_ledger_dir(dir.path());
    // 220k input would have tripped the OLD 197k emergency hard stop before
    // the first tool call ever returned.
    engine.run("go", "m").await.expect("run should succeed");

    let ledger = read_only_ledger(dir.path());
    let t = &ledger.turns[0];
    assert_eq!(t.autocompact_threshold_tokens, expected_threshold as u64);
    assert_eq!(t.emergency_limit_tokens, expected_limit as u64);
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

// ── #1161: resume must not fragment the ledger ──────────────────────────────

/// #1161 — an ordinary `--resume` must keep the session's `conversation_id`,
/// so the ledger APPENDS to the record the first launch wrote.
///
/// The oracle is the ledger FILE COUNT in one hermetic directory. It is not
/// vacuous: the ledger is keyed by `conversation_id` (`ledger_path`), so a
/// re-minted id necessarily produces a second file, and a preserved one
/// necessarily produces a single file with both round-trips in it. Neither
/// outcome can be reached by any other route.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: put
/// `conversation_id: uuid::Uuid::new_v4().to_string()` back into
/// `AgentEngine::resume_with_provider_parts` — the resumed run writes a second
/// ledger under a fresh uuid and the count assertion breaks.
#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn resuming_a_session_appends_to_its_ledger_rather_than_starting_a_new_one() {
    use wcore_agent::session::SessionManager;

    let server = common::physical_attempt_server().await;
    let ledger_dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();

    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    config.compact.enabled = false;
    common::configure_persisted_test_session(&mut config, root.path());
    let session_dir = std::path::PathBuf::from(&config.session.directory);
    let workspace = root.path().to_string_lossy().into_owned();

    // Launch 1 — a fresh session with the id the USER chose.
    let provider = Arc::new(
        ScriptedProvider::new(vec![text_turn("one", usage(20_000, 300, 0, 20_000))])
            .with_physical_url(server.uri()),
    );
    let mut engine = AgentEngine::new_with_provider(
        provider,
        config.clone(),
        ToolRegistry::new(),
        silent_output(),
    );
    engine.set_cache_ledger_dir(ledger_dir.path());
    engine
        .init_session("test-provider", &workspace, Some("aa55aa55-0002"))
        .expect("init_session");
    engine.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    engine.run("go", "m1").await.expect("first run");
    drop(engine);

    let first: Vec<_> = ledger_files(ledger_dir.path());
    assert_eq!(first.len(), 1, "launch 1 must write exactly one ledger");

    // Launch 2 — the ordinary `--resume` path: load the persisted session and
    // hand it straight to the resume constructor, exactly as `main.rs` does.
    let manager = SessionManager::new(session_dir, 10);
    let active = manager
        .load_for_run("aa55aa55-0002")
        .expect("the session must be resumable");
    let provider = Arc::new(
        ScriptedProvider::new(vec![text_turn("two", usage(500, 300, 20_000, 0))])
            .with_physical_url(server.uri()),
    );
    let mut engine = AgentEngine::resume_active_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        silent_output(),
        active,
    );
    engine.set_cache_ledger_dir(ledger_dir.path());
    engine.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    engine.run("again", "m2").await.expect("resumed run");
    drop(engine);

    let after = ledger_files(ledger_dir.path());
    assert_eq!(
        after.len(),
        1,
        "resuming minted a new conversation id and fragmented the ledger: {after:?}"
    );
    assert_eq!(
        after[0], first[0],
        "the resumed run must append to the SAME ledger file"
    );
    let ledger = read_only_ledger(ledger_dir.path());
    assert_eq!(
        ledger.turns.len(),
        2,
        "both round-trips must land in one record: {:#?}",
        ledger.turns
    );
}

/// Every `.json` in `dir`, sorted, as bare file names.
fn ledger_files(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// #1161 back-compat: every session saved by an EARLIER build has no
/// `conversation_id` on disk. Resuming one must mint a fresh id and run, not
/// crash and not reuse another session's key.
///
/// This is the state every existing user's saved sessions are in, so it is the
/// case the fix is most likely to break.
#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn a_session_saved_before_the_field_existed_still_resumes() {
    use wcore_agent::session::SessionManager;

    let server = common::physical_attempt_server().await;
    let ledger_dir = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();

    let mut config = test_config();
    config.model = "claude-opus-4-7".to_string();
    config.compact.enabled = false;
    common::configure_persisted_test_session(&mut config, root.path());
    let session_dir = std::path::PathBuf::from(&config.session.directory);
    let workspace = root.path().to_string_lossy().into_owned();

    let provider = Arc::new(
        ScriptedProvider::new(vec![text_turn("one", usage(1_000, 100, 0, 0))])
            .with_physical_url(server.uri()),
    );
    let mut engine = AgentEngine::new_with_provider(
        provider,
        config.clone(),
        ToolRegistry::new(),
        silent_output(),
    );
    engine.set_cache_ledger_dir(ledger_dir.path());
    engine
        .init_session("test-provider", &workspace, Some("aa55aa55-0001"))
        .expect("init_session");
    engine.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    engine.run("go", "m1").await.expect("first run");
    drop(engine);

    // Rewrite the snapshot the way an older build left it: no such field.
    let manager = SessionManager::new(session_dir, 10);
    let path = manager.session_file_path("aa55aa55-0001");
    let raw = std::fs::read_to_string(&path).expect("session snapshot");
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    value
        .as_object_mut()
        .expect("session is an object")
        .remove("conversation_id");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let active = manager
        .load_for_run("aa55aa55-0001")
        .expect("a pre-field session must still load");
    let provider = Arc::new(
        ScriptedProvider::new(vec![text_turn("two", usage(500, 100, 400, 0))])
            .with_physical_url(server.uri()),
    );
    let mut engine = AgentEngine::resume_active_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        silent_output(),
        active,
    );
    engine.set_cache_ledger_dir(ledger_dir.path());
    engine.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    engine
        .run("again", "m2")
        .await
        .expect("a pre-field session must still resume and run");

    // Two ledgers, because the first launch's id is genuinely unrecoverable.
    // The point is that the resume WORKS and does not reuse the wrong key.
    let files = ledger_files(ledger_dir.path());
    assert_eq!(
        files.len(),
        2,
        "a legacy session has no id to restore, so a fresh one is minted: {files:?}"
    );
}

/// #1163 — a model the catalog cannot price must NOT record its uncached
/// counterfactual as `0.0`. Asserted on the raw JSON the engine wrote, so the
/// assertion holds across the type change rather than restating it.
///
/// The billed figure here comes from the PROVIDER, so this reproduces the
/// measured contradiction exactly: a real, trustworthy spend beside a
/// counterfactual nothing could compute.
#[tokio::test]
#[serial_test::serial(wayland_cache_ledger_env)]
async fn an_unpriceable_counterfactual_is_not_recorded_as_a_zero() {
    let dir = tempfile::tempdir().unwrap();
    let mut billed = usage(7_000, 500, 0, 0);
    billed.reported_cost_usd = Some(0.061389);
    let provider = Arc::new(ScriptedProvider::new(vec![text_turn("done", billed)]));

    let mut config = test_config();
    // An uncatalogued model, priced by the provider itself — `flux-reasoning`
    // in the live measurement.
    config.model = "flux-reasoning".to_string();

    let mut engine =
        AgentEngine::new_with_provider(provider, config, ToolRegistry::new(), silent_output());
    engine.set_cache_ledger_dir(dir.path());
    engine.run("go", "m").await.expect("run should succeed");

    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();
    assert_eq!(files.len(), 1);
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    let turn = &raw["turns"][0];
    assert_eq!(
        turn["cost_source"], "provider_reported",
        "the provider billed this call, so the spend figure is a fact: {turn}"
    );
    assert!(
        turn["cost_usd"].as_f64().unwrap_or(0.0) > 0.0,
        "the billed figure must survive: {turn}"
    );
    // `flux-reasoning` has no catalog row, and the only figure left is the
    // provider-FAMILY preset rate — which the resolver itself grades
    // `priced = false`. Subtracting a real provider-reported spend from a
    // preset ceiling is not a measurement, so the counterfactual is unknown.
    assert_eq!(
        turn["uncached_equivalent_usd"],
        serde_json::Value::Null,
        "no price exists for this model, so the counterfactual is unknown; \
         recording a number makes the saving read as -cost: {turn}"
    );
}
