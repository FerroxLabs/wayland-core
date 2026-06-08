// W5 Group E acceptance: dream_now produces episodes + facts within 30s.

use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::partition::working::WorkingEntry;
use wcore_memory::v2_types::Tier;

async fn fresh_dispatcher() -> PartitionDispatcher {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    PartitionDispatcher::new(gate, db, embedder, cdc, Some("sess".into()))
}

async fn seed_session(d: &PartitionDispatcher) {
    // 10 tool calls + a couple of summary turns.
    for i in 0..10 {
        d.working
            .push(WorkingEntry::ToolCall {
                ts: i,
                tool: "bash".into(),
                summary: format!("ran command {i}"),
            })
            .await
            .unwrap();
    }
    d.working
        .push(WorkingEntry::Turn {
            ts: 11,
            role: "user".into(),
            text: "User prefers imperative commit messages".into(),
        })
        .await
        .unwrap();
    d.working
        .push(WorkingEntry::Turn {
            ts: 12,
            role: "assistant".into(),
            text: "Acknowledged.".into(),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn compress_emits_episode_per_session() {
    let d = fresh_dispatcher().await;
    seed_session(&d).await;
    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    let n = engine.compress().await.unwrap();
    assert!(n >= 1, "compress should emit ≥1 episode, got {n}");
}

#[tokio::test]
async fn consolidate_emits_fact_from_user_prefers() {
    let d = fresh_dispatcher().await;
    // Seed a P2 episode whose summary contains the "User prefers X" pattern.
    d.record_episode(
        wcore_memory::v2_types::Episode {
            id: wcore_memory::v2_types::EpisodeId::new(),
            tier: Tier::Project,
            ts: 1,
            episode_type: "summary".into(),
            summary: "User prefers imperative commit messages".into(),
            atomic_facts: vec![],
            source: "consolidate".into(),
            source_product: "wcore-consolidate".into(),
            session_id: Some("s1".into()),
            project_root: None,
            decay_score: 1.0,
            status: wcore_memory::v2_types::EpisodeStatus::Active,
        },
        wcore_memory::v2_types::AccessToken::System,
    )
    .await
    .unwrap();
    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    let facts = engine.consolidate().await.unwrap();
    assert!(facts >= 1, "expected ≥1 fact, got {facts}");
}

#[tokio::test]
async fn dream_now_runs_under_30s() {
    let d = fresh_dispatcher().await;
    seed_session(&d).await;
    let report = d.dream_now().await.unwrap();
    assert!(
        report.elapsed_ms < 30_000,
        "dream took {}ms",
        report.elapsed_ms
    );
    assert!(report.episodes_compressed >= 1);
}

#[tokio::test]
async fn decay_ages_episodes() {
    let d = fresh_dispatcher().await;
    let now = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()) as i64;
    // Insert episodes spanning 0..40 days ago.
    for i in 0..40 {
        let ts = now - (i as i64) * 86400;
        d.record_episode(
            wcore_memory::v2_types::Episode {
                id: wcore_memory::v2_types::EpisodeId::new(),
                tier: Tier::Project,
                ts,
                episode_type: "decay_test".into(),
                summary: format!("day {i}"),
                atomic_facts: vec![],
                source: "main-agent".into(),
                source_product: "wcore-agent".into(),
                session_id: None,
                project_root: None,
                decay_score: 1.0,
                status: wcore_memory::v2_types::EpisodeStatus::Active,
            },
            wcore_memory::v2_types::AccessToken::MainAgent,
        )
        .await
        .unwrap();
    }
    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    engine.decay().await.unwrap();

    let tc = d.db.tier_or_global(Tier::Project);
    let conn = tc.conn.lock();
    let archived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE status = 'archived'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        archived >= 10,
        "expected ≥10 archived (everything 30+ days old), got {archived}"
    );
}
