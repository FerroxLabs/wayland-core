//! Crucible T4 — cross-provider diversity guard (the core regression test).
//!
//! Each council member is pinned to a *distinct* provider whose mock returns a
//! *distinct* text. After spawning, every result must carry its pinned
//! provider's text — never the parent's. This proves the resolved provider
//! actually drives the child engine end-to-end, on BOTH the relay
//! (`spawn_parallel`) and fleet (`spawn_via_fleet`) paths. If the resolver
//! failed to propagate through `clone_for_spawn`, every child would run on the
//! parent and the assertions would see "PARENT" instead.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use wcore_agent::orchestration::council::{ProviderResolver, ResolveError};
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::LlmProvider;
use wcore_types::spawner::{SubAgentConfig, SubAgentResult};

use common::{MockLlmProvider, test_config};

/// Resolver mapping a spec string to a specific mock provider `Arc`.
struct MapResolver {
    map: HashMap<String, Arc<dyn LlmProvider>>,
}

impl ProviderResolver for MapResolver {
    fn resolve_provider(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        self.map
            .get(spec)
            .cloned()
            .map(|p| (p, None))
            .ok_or_else(|| ResolveError::Unknown(spec.to_string()))
    }
}

/// Build `n` pinned sub-agent configs + a resolver where each spec resolves to
/// a mock provider whose response text is `RESP-<i>` — so the result text
/// identifies which provider ran.
fn pinned_roster(n: usize) -> (Vec<SubAgentConfig>, Arc<dyn ProviderResolver>) {
    let mut map: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
    let mut configs = Vec::with_capacity(n);
    for i in 0..n {
        let spec = format!("prov-{i}");
        let provider: Arc<dyn LlmProvider> =
            Arc::new(MockLlmProvider::with_text_response(&format!("RESP-{i}")));
        map.insert(spec.clone(), provider);
        configs.push(SubAgentConfig {
            name: format!("member-{i}"),
            prompt: "answer".into(),
            max_turns: 1,
            max_tokens: 64,
            system_prompt: None,
            provider: Some(spec),
            model: None,
            temperature: None,
        });
    }
    (configs, Arc::new(MapResolver { map }))
}

/// Assert every member ran on its own pinned provider (text `RESP-<i>`), and
/// none fell back to the parent (`PARENT`).
fn assert_diverse(results: &[SubAgentResult], n: usize) {
    let by_name: HashMap<&str, &str> = results
        .iter()
        .map(|r| (r.name.as_str(), r.text.as_str()))
        .collect();
    assert_eq!(by_name.len(), n, "expected {n} distinct member results");
    for text in by_name.values() {
        assert_ne!(*text, "PARENT", "a member fell back to the parent provider");
    }
    for i in 0..n {
        let name = format!("member-{i}");
        assert_eq!(
            by_name.get(name.as_str()).copied(),
            Some(format!("RESP-{i}").as_str()),
            "member-{i} must run on its pinned provider"
        );
    }
}

#[tokio::test]
async fn relay_path_honors_pinned_providers() {
    // 4 members through the relay (spawn_parallel → clone_for_spawn) path.
    let parent = Arc::new(MockLlmProvider::with_text_response("PARENT"));
    let (configs, resolver) = pinned_roster(4);
    let spawner = AgentSpawner::new(parent, test_config()).with_provider_resolver(resolver);

    let results = spawner.spawn_parallel(configs).await;
    assert_diverse(&results, 4);
}

#[tokio::test]
async fn fleet_path_honors_pinned_providers() {
    // 12 members (> DEFAULT_SHARD_SIZE = 10) forces multi-shard fleet dispatch.
    // This is the path most likely to drop the resolver in clone_for_spawn.
    let parent = Arc::new(MockLlmProvider::with_text_response("PARENT"));
    let (configs, resolver) = pinned_roster(12);
    let spawner = AgentSpawner::new(parent, test_config()).with_provider_resolver(resolver);

    let results = spawner.spawn_via_fleet(configs, "diversity-test").await;
    assert_diverse(&results, 12);
}

#[tokio::test]
async fn unpinned_member_uses_parent_provider() {
    // Sanity counter-test: with no pin, the member runs on the parent.
    let parent = Arc::new(MockLlmProvider::with_text_response("PARENT"));
    let (_configs, resolver) = pinned_roster(1);
    let spawner = AgentSpawner::new(parent, test_config()).with_provider_resolver(resolver);

    let result = spawner
        .spawn_one(SubAgentConfig {
            name: "lone".into(),
            prompt: "answer".into(),
            max_turns: 1,
            max_tokens: 64,
            system_prompt: None,
            provider: None,
            model: None,
            temperature: None,
        })
        .await;
    assert_eq!(result.text, "PARENT");
}
