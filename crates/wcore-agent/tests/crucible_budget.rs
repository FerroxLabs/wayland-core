//! Crucible cost-governance wiring — proves the per-member CHARGE path actually
//! records real spend, and the END-TO-END MOAT: a first council's real spend
//! makes a second council's soft daily pre-check refuse before spawning.
//!
//! The council `BudgetTracker` is CAP-LESS by design (the daily bound is the
//! soft pre-flight check in `run_council`; per-member charging must always
//! commit so the next council's pre-check is accurate). These tests drive the
//! real `run_council` with deterministic mock providers that report NONZERO
//! `TokenUsage`, so the charge path is exercised exactly.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex as PlMutex;
use tokio::sync::mpsc;
use wcore_agent::orchestration::council::{
    CouncilError, CouncilSpend, ProposerSpec, ProviderResolver, ResolveError, Roster, run_council,
};
use wcore_agent::spawner::AgentSpawner;
use wcore_budget::{BudgetCap, BudgetTracker};
use wcore_pricing::DEFAULT_CATALOG;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

/// The priceable provider/model used across these tests (a native catalog row,
/// as the spend.rs pricing tests rely on). Members differ only by spec string so
/// the resolver can hand back distinct mocks; pricing keys off this single model.
const PROVIDER: &str = "deepseek";
const MODEL: &str = "deepseek-v4-pro";

/// Fixed nonzero usage every mock member reports — so the real charge path
/// records a concrete, priceable amount.
const FIXED_USAGE: TokenUsage = TokenUsage {
    input_tokens: 100,
    output_tokens: 200,
    cache_creation_tokens: 0,
    cache_read_tokens: 0,
};

/// A proposer/aggregator mock that emits a text reply then a `Done` carrying a
/// fixed NONZERO usage. (`common::CapturingProvider` reports `TokenUsage::default()`
/// — zero — which would never exercise the charge path, so we need our own.)
struct UsageProvider {
    reply: String,
    usage: TokenUsage,
}

impl UsageProvider {
    fn new(reply: &str) -> Arc<Self> {
        Arc::new(Self {
            reply: reply.to_string(),
            usage: FIXED_USAGE,
        })
    }

    /// A member that produces NO text (empty) but still burns `FIXED_USAGE`
    /// tokens — `is_error == false`, `is_usable() == false` (blank text). This
    /// is the "billed but not usable" case the charge guard must still record.
    fn blank() -> Arc<Self> {
        Arc::new(Self {
            reply: String::new(),
            usage: FIXED_USAGE,
        })
    }
}

#[async_trait]
impl LlmProvider for UsageProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let (tx, rx) = mpsc::channel(8);
        let reply = self.reply.clone();
        let usage = self.usage.clone();
        tokio::spawn(async move {
            if !reply.is_empty() {
                let _ = tx.send(LlmEvent::TextDelta(reply)).await;
            }
            let _ = tx
                .send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage,
                })
                .await;
        });
        Ok(rx)
    }
}

/// The spawner's unused default provider — never invoked (every member pinned).
struct NeverProvider;

#[async_trait]
impl LlmProvider for NeverProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        Err(ProviderError::Connection("never".into()))
    }
}

/// Resolver mapping a full spec → a fixed mock provider. It always resolves the
/// model to the single priceable `MODEL`, regardless of the (distinct) spec
/// strings used as map keys, so EVERY proposal and the judge's spend provenance
/// price through the catalog. (If it returned the spec's own post-`:` model, a
/// member keyed `deepseek:proposer-a` would resolve to the unpriceable model
/// `proposer-a` and its charge USD would be 0.)
struct MapResolver {
    map: HashMap<String, Arc<dyn LlmProvider>>,
}

impl ProviderResolver for MapResolver {
    fn resolve_provider(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        match self.map.get(spec) {
            // Always the priceable MODEL — see the doc comment above.
            Some(p) => Ok((p.clone(), Some(MODEL.to_string()))),
            None => Err(ResolveError::Unknown(spec.to_string())),
        }
    }
}

/// Build a spawner with the given resolver map + a cap-less council tracker and
/// the `("s","u")` charge identity. Returns the spawner and a handle to the
/// shared tracker so the test can read `user_daily_usd("u")`.
fn spawner_with(
    map: HashMap<String, Arc<dyn LlmProvider>>,
) -> (AgentSpawner, Arc<PlMutex<BudgetTracker>>) {
    let tracker = Arc::new(PlMutex::new(BudgetTracker::new(BudgetCap::default())));
    let spawner = AgentSpawner::new(Arc::new(NeverProvider), test_config())
        .with_provider_resolver(Arc::new(MapResolver { map }))
        .with_budget_tracker(Arc::clone(&tracker))
        .with_budget_identity("s", "u");
    (spawner, tracker)
}

/// A `ProposerSpec` whose `spec` keys the resolver map (the resolver fixes the
/// resolved model to `MODEL`, so the proposal prices through the catalog
/// regardless of the spec's own post-`:` text).
fn pspec(spec: &str) -> ProposerSpec {
    let provider = spec.split(':').next().unwrap().to_string();
    let model = spec.split_once(':').map(|(_, m)| m.to_string());
    ProposerSpec {
        spec: spec.to_string(),
        provider,
        model,
    }
}

/// The canonical aggregator spec — exactly `provider:MODEL`, because run_council's
/// PRE-FLIGHT estimate prices the judge by splitting THIS string (not via the
/// resolver), so a non-canonical suffix would make the judge unpriceable and the
/// certified ceiling `None` (the moat would then never trip).
fn judge_spec() -> String {
    format!("{PROVIDER}:{MODEL}")
}

fn roster(
    proposers: Vec<ProposerSpec>,
    aggregator: Option<&str>,
    daily_cap_usd: Option<f64>,
) -> Roster {
    Roster {
        proposers,
        aggregator: aggregator.map(|s| s.to_string()),
        min_proposers: 1,
        proposer_max_turns: 1,
        proposer_concurrency: 0,
        proposer_deadline_s: 90,
        global_deadline_s: 25,
        max_cost_usd: None,
        flux_markup: 1.0,
        daily_cap_usd,
        proposer_temperature: 0.6,
        aggregator_temperature: 0.4,
    }
}

/// USD a single member charges for `FIXED_USAGE` at the catalog price (markup 1.0).
fn member_usd() -> f64 {
    CouncilSpend::usd_for_usage(PROVIDER, Some(MODEL), &FIXED_USAGE, 1.0)
}

/// Two distinct proposer specs + the canonical judge spec, each mapped to a
/// `UsageProvider`. The proposer keys are distinct (so the resolver hands back
/// distinct mocks); the judge key is the canonical `provider:MODEL` (so the
/// pre-flight estimate prices it). All resolve to `MODEL` for charging.
fn three_member_map() -> HashMap<String, Arc<dyn LlmProvider>> {
    let mut map: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    map.insert(format!("{PROVIDER}:p-a"), UsageProvider::new("answer A"));
    map.insert(format!("{PROVIDER}:p-b"), UsageProvider::new("answer B"));
    map.insert(judge_spec(), UsageProvider::new("FUSED"));
    map
}

/// The 2-proposer + judge roster that pairs with `three_member_map`.
fn build_roster_3(daily_cap_usd: Option<f64>) -> Roster {
    roster(
        vec![
            pspec(&format!("{PROVIDER}:p-a")),
            pspec(&format!("{PROVIDER}:p-b")),
        ],
        Some(&judge_spec()),
        daily_cap_usd,
    )
}

// ---- Test 1: every member is charged against the envelope -----------------

#[tokio::test]
async fn council_charges_each_member_against_the_envelope() {
    let (spawner, tracker) = spawner_with(three_member_map());

    let outcome = run_council("task", &build_roster_3(None), &spawner, &test_config())
        .await
        .expect("council runs");

    assert_eq!(outcome.final_text, "FUSED");

    // The two proposers + the judge each burned FIXED_USAGE and were charged.
    let charged = tracker.lock().user_daily_usd("u");
    assert!(charged > 0.0, "the envelope must record real spend");

    // It equals the sum over the 2 proposers + the judge (all the same price).
    let expected = 3.0 * member_usd();
    assert!(
        (charged - expected).abs() < 1e-9,
        "charged {charged} != expected {expected} (2 proposers + judge)"
    );
}

// ---- Test 2: END-TO-END MOAT — second council blocked after the first ------

#[tokio::test]
async fn second_council_blocked_after_first_real_council_charges() {
    // Compute the certified judge-inclusive ceiling for this 2-proposer + judge
    // roster, priced identically to how run_council's preflight does it
    // (DEFAULT_PROPOSER_MAX_TOKENS = 4096, proposer_max_turns = 1, markup 1.0).
    let proposers: Vec<(&str, Option<&str>)> =
        vec![(PROVIDER, Some(MODEL)), (PROVIDER, Some(MODEL))];
    let ceiling_microcents = CouncilSpend::estimate_preflight_microcents(
        &DEFAULT_CATALOG,
        &proposers,
        Some((PROVIDER, Some(MODEL))),
        1,    // proposer_max_turns — must match the roster below
        4096, // DEFAULT_PROPOSER_MAX_TOKENS (run_council's preflight constant)
        1.0,
    )
    .certified_microcents()
    .expect("a fully-priced roster certifies a ceiling");
    let ceiling = ceiling_microcents as f64 / 100_000_000.0;

    // Set the daily cap EXACTLY at the ceiling. With prior_spent == 0, council A
    // passes (`0 + ceiling > ceiling` is false). After A charges any real spend
    // (> 0), council B's pre-check (`spent + ceiling > ceiling`) trips. This is
    // the tightest, most robust tuning — it relies only on A's spend being > 0,
    // not on a hand-derived fraction of the (much larger) worst-case ceiling.
    // The preflight in run_council recomputes the SAME ceiling with identical
    // inputs, so `c == ceiling` exactly and the `> cap` comparison is exact.
    let cap = ceiling;

    let (spawner, tracker) = spawner_with(three_member_map());

    // Council A: prior_spent == 0 → passes, then charges real spend.
    let a = run_council(
        "task A",
        &build_roster_3(Some(cap)),
        &spawner,
        &test_config(),
    )
    .await;
    assert!(
        a.is_ok(),
        "first council must run (ceiling == cap, spent 0): {a:?}"
    );
    let after_a = tracker.lock().user_daily_usd("u");
    assert!(after_a > 0.0, "council A must have charged real spend");

    // Council B: on the SAME spawner (shared tracker). prior_spent > 0 →
    // `spent + ceiling > cap` → refused before spawning.
    let b = run_council(
        "task B",
        &build_roster_3(Some(cap)),
        &spawner,
        &test_config(),
    )
    .await;
    match b {
        Err(CouncilError::DailyBudgetExhausted { spent_usd, cap_usd }) => {
            assert!(spent_usd > 0.0, "the moat must report prior real spend");
            assert!((cap_usd - cap).abs() < 1e-9, "cap echoed back");
        }
        other => panic!("second council must be daily-budget-exhausted, got {other:?}"),
    }
}

// ---- Test 3: a billed-but-not-usable member is still charged ---------------

#[tokio::test]
async fn errored_proposer_with_usage_is_still_charged() {
    // One member returns NONZERO usage but EMPTY text → `is_error == false`,
    // `is_usable() == false`. The old guard (`if proposal.is_usable()`) would
    // skip the charge; the new guard (`if charged_tokens > 0`) records it.
    //
    // NOTE: the spawner's `spawn_one` zeroes usage on the engine-`Err` path
    // (`usage: TokenUsage::default()` when `is_error == true`), so a TRUE errored
    // proposer cannot carry usage through this mock infra. The blank-text member
    // is the faithful, deterministic stand-in for "billed but not usable": same
    // `is_usable() == false`, but with the real tokens the council was billed.
    let mut map: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    map.insert(format!("{PROVIDER}:blank"), UsageProvider::blank()); // billed, not usable
    map.insert(
        format!("{PROVIDER}:ok"),
        UsageProvider::new("usable answer"),
    ); // keeps quorum
    let (spawner, tracker) = spawner_with(map);

    let outcome = run_council(
        "task",
        &roster(
            vec![
                pspec(&format!("{PROVIDER}:blank")),
                pspec(&format!("{PROVIDER}:ok")),
            ],
            None, // no aggregator → first-usable fallback (the ok member)
            None,
        ),
        &spawner,
        &test_config(),
    )
    .await
    .expect("the usable member forms a quorum");

    assert_eq!(outcome.final_text, "usable answer");

    // The daily total includes BOTH members — the not-usable one was still
    // charged because it burned tokens.
    let charged = tracker.lock().user_daily_usd("u");
    let expected = 2.0 * member_usd();
    assert!(
        (charged - expected).abs() < 1e-9,
        "charged {charged} must include the billed-but-not-usable member (expected {expected})"
    );
}
