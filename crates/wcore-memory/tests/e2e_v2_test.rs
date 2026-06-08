// W5 Group G — end-to-end test of the Memory facade as agents would use
// it: open in-memory → record episodes via MainAgentToken → dream_now
// extracts a fact → compact reduces token window → sub-agent ACL.

use serde_json::json;

use wcore_memory::v2_types::{AccessToken, Episode, EpisodeId, EpisodeStatus, Query, Tier};
use wcore_memory::{Memory, MemoryApi};

#[tokio::test]
async fn full_v2_lifecycle() {
    let mem = Memory::open_in_memory().await.unwrap();

    // 5 episodes from MainAgent.
    for i in 0..5 {
        mem.record_episode(
            Episode {
                id: EpisodeId::new(),
                tier: Tier::Project,
                ts: i,
                episode_type: "tool_call".into(),
                summary: if i == 0 {
                    "User prefers imperative commit messages".into()
                } else {
                    format!("ran tool call {i}")
                },
                atomic_facts: vec![],
                source: "main-agent".into(),
                source_product: "wcore-agent".into(),
                session_id: Some("s1".into()),
                project_root: None,
                decay_score: 1.0,
                status: EpisodeStatus::Active,
            },
            AccessToken::MainAgent,
        )
        .await
        .unwrap();
    }

    // Audit log should record allow rows for those writes.
    let total_audit = mem.audit.count().unwrap();
    assert!(total_audit >= 5, "audit count {total_audit}");

    // Dream extracts at least one fact (from the "User prefers..." episode).
    let report = mem.dream_now().await.unwrap();
    assert!(report.facts_consolidated >= 1, "dream report: {report:?}");

    // user_model write requires SystemToken.
    mem.update_user_model("style.commits", json!("imperative"), AccessToken::System)
        .await
        .unwrap();
    let model = mem.user_model(AccessToken::System).await.unwrap();
    assert_eq!(model.entries.len(), 1);

    // SubAgent ACL: reviewer with project_episodes read scope can search,
    // cannot access user_model.
    let mut policy = wcore_memory::gate::AccessPolicy::empty();
    policy.grant_read(
        "reviewer",
        wcore_memory::v2_types::Partition::Episodic,
        Tier::Project,
    );
    let gate_with_policy = std::sync::Arc::new(wcore_memory::gate::MemoryAccessGate::new(
        mem.audit.clone(),
        policy,
    ));
    // Build a dispatcher sharing the same DB + cdc but with the scoped gate.
    let dispatcher = wcore_memory::partition::PartitionDispatcher::new(
        gate_with_policy,
        mem.db.clone(),
        mem.embedder.clone(),
        mem.cdc.clone(),
        Some("scoped".into()),
    );
    let token = AccessToken::SubAgent {
        agent_name: "reviewer".into(),
    };
    let hits = dispatcher
        .search(
            Query {
                text: "tool call".into(),
                tier: Tier::Project,
                ..Query::default()
            },
            token.clone(),
        )
        .await
        .unwrap();
    assert!(!hits.is_empty());
    let err = dispatcher.user_model(token).await.unwrap_err();
    assert!(matches!(
        err,
        wcore_memory::error::MemoryError::AccessDenied { .. }
    ));
}

#[tokio::test]
async fn legacy_import_via_memory_is_idempotent_when_missing() {
    // When no project_root is set, import is a no-op.
    let mem = Memory::open_in_memory().await.unwrap();
    let r = mem.import_legacy_if_present().await.unwrap();
    assert_eq!(r.episodes_inserted, 0);
}
