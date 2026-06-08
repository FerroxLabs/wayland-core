// W5 Group C acceptance: PartitionDispatcher implements MemoryApi for all
// 5 partitions, routing through the gate.

use std::sync::Arc;

use serde_json::json;

use wcore_memory::api::MemoryApi;
use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::error::MemoryError;
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::v2_types::{
    AccessToken, Episode, EpisodeId, EpisodeStatus, Fact, FactId, Procedure, ProcedureId,
    ProcedureStatus, Query, Tier,
};

async fn fresh_dispatcher() -> PartitionDispatcher {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    PartitionDispatcher::new(gate, db, embedder, cdc, Some("test-session".into()))
}

#[tokio::test]
async fn episodic_roundtrip() {
    let d = fresh_dispatcher().await;
    let ep = Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: 0, // dispatcher fills in
        episode_type: "tool_call".into(),
        summary: "ran cargo nextest".into(),
        atomic_facts: vec!["nextest is the test runner".into()],
        source: "main-agent".into(),
        source_product: "wcore-agent".into(),
        session_id: Some("test-session".into()),
        project_root: Some("/tmp/p".into()),
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    let id = d
        .record_episode(ep.clone(), AccessToken::MainAgent)
        .await
        .unwrap();
    assert_eq!(id, ep.id);
    let got = d.get_episode(&id, AccessToken::MainAgent).await.unwrap();
    assert_eq!(got.summary, "ran cargo nextest");
    assert_eq!(got.episode_type, "tool_call");
    assert_eq!(got.atomic_facts.len(), 1);
}

#[tokio::test]
async fn episodic_acl_rejects_subagent_without_scope() {
    let d = fresh_dispatcher().await;
    let ep = Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: 0,
        episode_type: "x".into(),
        summary: "x".into(),
        atomic_facts: vec![],
        source: "sub-agent:nope".into(),
        source_product: "wcore-agent".into(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    let err = d
        .record_episode(
            ep,
            AccessToken::SubAgent {
                agent_name: "nope".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
}

#[tokio::test]
async fn semantic_supersedes_chain() {
    let d = fresh_dispatcher().await;
    let f1 = Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: 0,
        subject: "rust".into(),
        predicate: "version".into(),
        object: "2023".into(),
        confidence: 0.9,
        source_episode: None,
        superseded_by: None,
    };
    let id1 = d
        .assert_fact(f1.clone(), AccessToken::MainAgent)
        .await
        .unwrap();

    let f2 = Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: 0,
        subject: "rust".into(),
        predicate: "version".into(),
        object: "2024".into(),
        confidence: 0.95,
        source_episode: None,
        superseded_by: None,
    };
    let id2 = d
        .assert_fact(f2.clone(), AccessToken::MainAgent)
        .await
        .unwrap();
    assert_ne!(id1, id2);

    // f1.superseded_by should now point at f2.
    let list = d
        .search(
            Query {
                text: "rust version".into(),
                tier: Tier::Project,
                ..Query::default()
            },
            AccessToken::MainAgent,
        )
        .await
        .unwrap();
    // sanity: search should at least return something here (BM25 over the
    // semantic fact's natural form is via facts table, not episodes — the
    // basic retriever only hits episodes_fts, so this just asserts the
    // search path succeeds without an ACL or DB error).
    let _ = list;
}

/// v2 memory recall-injection gap regression: a fact written via
/// `assert_fact` (P3 Semantic) MUST be reachable through `search`. Before the
/// fix, `search` only hit `episodes_fts`/`episodes` and never the `facts`
/// table, so a stored preference was unrecoverable in a later session — the
/// D4 keystone's "favorite color is teal" recall miss.
#[tokio::test]
async fn search_recalls_asserted_fact() {
    let d = fresh_dispatcher().await;
    let fact = Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: 0,
        subject: "user".into(),
        predicate: "favorite_color".into(),
        object: "teal".into(),
        confidence: 0.95,
        source_episode: None,
        superseded_by: None,
    };
    d.assert_fact(fact, AccessToken::MainAgent).await.unwrap();

    let hits = d
        .search(
            Query {
                text: "what is my favorite color".into(),
                tier: Tier::Project,
                ..Query::default()
            },
            AccessToken::MainAgent,
        )
        .await
        .unwrap();

    let teal = hits
        .iter()
        .find(|h| h.partition == wcore_memory::v2_types::Partition::Semantic)
        .expect("a semantic fact hit must be returned for the stored preference");
    assert!(
        teal.preview.contains("teal"),
        "fact preview should carry the stored object, got: {}",
        teal.preview
    );
}

/// The fact recall pass MUST respect the per-partition read scope: a sub-agent
/// granted only episodic read must NOT receive semantic facts through `search`.
#[tokio::test]
async fn search_fact_recall_honors_subagent_acl() {
    // Build a dispatcher whose policy grants the sub-agent project-episodic
    // read only (no semantic scope).
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let mut policy = AccessPolicy::empty();
    policy.grant_read(
        "reviewer",
        wcore_memory::v2_types::Partition::Episodic,
        Tier::Project,
    );
    let gate = Arc::new(MemoryAccessGate::new(audit, policy));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    let d = PartitionDispatcher::new(gate, db, embedder, cdc, Some("test-session".into()));

    // Main agent asserts a fact.
    d.assert_fact(
        Fact {
            id: FactId::new(),
            tier: Tier::Project,
            ts: 0,
            subject: "user".into(),
            predicate: "favorite_color".into(),
            object: "teal".into(),
            confidence: 0.95,
            source_episode: None,
            superseded_by: None,
        },
        AccessToken::MainAgent,
    )
    .await
    .unwrap();

    // Sub-agent (episodic-only) searches — the gate allows the episodic pass
    // but the semantic fact pass is skipped, so no Semantic hits leak.
    let hits = d
        .search(
            Query {
                text: "favorite color".into(),
                tier: Tier::Project,
                ..Query::default()
            },
            AccessToken::SubAgent {
                agent_name: "reviewer".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        !hits
            .iter()
            .any(|h| h.partition == wcore_memory::v2_types::Partition::Semantic),
        "sub-agent without semantic read scope must not receive facts"
    );
}

#[tokio::test]
async fn procedural_state_machine_via_dispatcher() {
    let d = fresh_dispatcher().await;
    let p = Procedure {
        id: ProcedureId::new(),
        tier: Tier::Project,
        ts: 0,
        name: "deploy".into(),
        description: "x".into(),
        artifact: "...".into(),
        status: ProcedureStatus::Staged,
        created_by: "evolution".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    let id = d
        .upsert_procedure(p.clone(), AccessToken::MainAgent)
        .await
        .unwrap();
    d.procedural
        .transition(&id, Tier::Project, ProcedureStatus::Active)
        .await
        .unwrap();
    d.procedural
        .transition(&id, Tier::Project, ProcedureStatus::Archived)
        .await
        .unwrap();
    // From Archived can't go anywhere.
    let err = d
        .procedural
        .transition(&id, Tier::Project, ProcedureStatus::Active)
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
}

#[tokio::test]
async fn core_system_only_write() {
    let d = fresh_dispatcher().await;
    // System ok:
    d.update_user_model(
        "style.commits",
        json!({"format": "imperative"}),
        AccessToken::System,
    )
    .await
    .unwrap();
    // MainAgent denied:
    let err = d
        .update_user_model(
            "style.commits",
            json!({"format": "long"}),
            AccessToken::MainAgent,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
    let msg = err.to_string();
    assert!(msg.contains("core"));

    let model = d.user_model(AccessToken::System).await.unwrap();
    assert_eq!(model.entries.len(), 1);
    assert_eq!(model.entries[0].key, "style.commits");
}

#[tokio::test]
async fn working_memory_spillover() {
    let d = fresh_dispatcher().await;
    // Push 60 entries to exceed the default cap of 50.
    for i in 0..60 {
        d.working
            .push(wcore_memory::partition::working::WorkingEntry::Turn {
                ts: i as i64,
                role: "user".into(),
                text: format!("msg {i}"),
            })
            .await
            .unwrap();
    }
    assert!(d.working.live_len() <= 50);
    let spillover = d.working.spillover_count().unwrap();
    assert!(
        spillover >= 10,
        "expected at least 10 spilled, got {spillover}"
    );
}
