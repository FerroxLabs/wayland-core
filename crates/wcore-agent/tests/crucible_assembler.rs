//! Crucible auto-assembly — integration of the deterministic Assembler with the
//! council executor: assemble a roster from a priced candidate pool, then run it
//! through `run_council` with mock providers and prove it fuses.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::orchestration::council::{
    AssemblyPolicy, CouncilDecision, ProposerSpec, ProviderResolver, ResolveError, Roster, Stakes,
    assemble, family, run_council,
};
use wcore_agent::spawner::AgentSpawner;
use wcore_pricing::PricingCatalog;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};

use common::{MockLlmProvider, test_config};

/// Provider that is never called (the spawner's unused default).
struct NeverProvider;

#[async_trait]
impl LlmProvider for NeverProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        Err(ProviderError::Connection("never".into()))
    }
}

fn clone_err(e: &ResolveError) -> ResolveError {
    match e {
        ResolveError::Unknown(s) => ResolveError::Unknown(s.clone()),
        ResolveError::Keyless(s) => ResolveError::Keyless(s.clone()),
        ResolveError::Build(a, b) => ResolveError::Build(a.clone(), b.clone()),
    }
}

struct MapResolver {
    map: HashMap<String, Result<Arc<dyn LlmProvider>, ResolveError>>,
}

impl ProviderResolver for MapResolver {
    fn resolve_provider(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        match self.map.get(spec) {
            Some(Ok(p)) => Ok((p.clone(), None)),
            Some(Err(e)) => Err(clone_err(e)),
            None => Err(ResolveError::Unknown(spec.to_string())),
        }
    }
}

fn ok_text(text: &str) -> Result<Arc<dyn LlmProvider>, ResolveError> {
    Ok(Arc::new(MockLlmProvider::with_text_response(text)))
}

fn spawner_with(map: HashMap<String, Result<Arc<dyn LlmProvider>, ResolveError>>) -> AgentSpawner {
    AgentSpawner::new(Arc::new(NeverProvider), test_config())
        .with_provider_resolver(Arc::new(MapResolver { map }))
}

/// Five catalog-priced specs across five distinct families.
fn pool() -> Vec<String> {
    [
        "openai:gpt-5",
        "anthropic:claude-opus-4-7",
        "deepseek:deepseek-v4-pro",
        "gemini:gemini-2-5-pro",
        "xai:grok-3",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn policy() -> AssemblyPolicy {
    AssemblyPolicy {
        deny_families: vec![],
        max_proposers: 5,
        markup: 1.0,
        cap_low_usd: 0.02,
        cap_med_usd: 100.0, // generous → always fits, no downshift
        cap_high_usd: 100.0,
        price_floor_frac: 0.25,
        proposer_max_turns: 1,
        proposer_max_tokens: 1000,
    }
}

fn roster_from_plan(members: &[String], aggregator: Option<String>) -> Roster {
    Roster {
        proposers: members
            .iter()
            .map(|s| ProposerSpec {
                spec: s.clone(),
                provider: s.split(':').next().unwrap().to_string(),
                model: s.split_once(':').map(|(_, m)| m.to_string()),
            })
            .collect(),
        aggregator,
        min_proposers: 1,
        proposer_max_turns: 1,
        proposer_concurrency: 0,
        proposer_deadline_s: 90,
        global_deadline_s: 25,
        max_cost_usd: None,
        flux_markup: 1.0,
        daily_cap_usd: None,
        proposer_temperature: 0.6,
        aggregator_temperature: 0.4,
    }
}

#[tokio::test]
async fn auto_med_assembles_diverse_council_and_fuses() {
    let cat = PricingCatalog::load_default().unwrap();
    let gate = CouncilDecision::Council {
        reason: "med".into(),
        stakes: Stakes::Med,
    };
    let plan = assemble("design a system", &pool(), &cat, &gate, &policy());

    // The Assembler chose a 3-proposer, 3-distinct-family roster with a decoupled
    // judge.
    assert!(plan.convene, "Med over a 5-family pool must convene");
    assert_eq!(plan.members.len(), 3);
    let fams: HashSet<String> = plan.members.iter().map(|m| family(m)).collect();
    assert_eq!(fams.len(), 3, "proposers span 3 distinct families");
    let agg = plan.aggregator.clone().expect("council has a judge");
    assert!(!plan.members.contains(&agg), "judge must be decoupled");

    // Run the assembled roster through the executor with mocks for every spec.
    let mut map: HashMap<String, Result<Arc<dyn LlmProvider>, ResolveError>> = HashMap::new();
    for s in pool() {
        map.insert(s.clone(), ok_text(&format!("ans-{s}")));
    }
    let spawner = spawner_with(map);
    let roster = roster_from_plan(&plan.members, plan.aggregator.clone());
    let outcome = run_council("design a system", &roster, &spawner, &test_config())
        .await
        .expect("assembled council runs");

    assert_eq!(
        outcome.proposals.len(),
        3,
        "exactly the 3 chosen proposers ran"
    );
    assert!(!outcome.final_text.is_empty(), "the judge fused an answer");
    let provs: HashSet<&str> = outcome.chosen_from.iter().map(|s| s.as_str()).collect();
    assert_eq!(provs.len(), 3, "fused from 3 distinct providers");
    assert!(outcome.skipped.is_empty());
}

#[test]
fn auto_low_routes_direct_not_council() {
    let cat = PricingCatalog::load_default().unwrap();
    let gate = CouncilDecision::Direct {
        reason: "trivial".into(),
    };
    let plan = assemble("hi", &pool(), &cat, &gate, &policy());
    assert!(
        !plan.convene,
        "a Low/Direct gate must not convene a council"
    );
    assert_eq!(plan.members.len(), 1, "Direct answers with a single model");
    assert!(plan.aggregator.is_none());
}

#[test]
fn auto_deny_family_excludes_it_from_roster_and_judge() {
    let cat = PricingCatalog::load_default().unwrap();
    let gate = CouncilDecision::Council {
        reason: "med".into(),
        stakes: Stakes::Med,
    };
    let mut p = policy();
    p.deny_families = vec!["openai".to_string()];
    let plan = assemble("design a system", &pool(), &cat, &gate, &p);
    assert!(plan.convene);
    assert!(
        plan.members.iter().all(|m| family(m) != "openai"),
        "denied family must not propose"
    );
    assert_ne!(
        plan.aggregator.as_deref().map(family),
        Some("openai".to_string()),
        "denied family must not judge"
    );
}
