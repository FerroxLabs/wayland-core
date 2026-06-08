//! M5.7 Part C — end-to-end integration test for the cross-task chain.
//!
//! Exercises:
//!   SwarmResult --(SwarmMemoryBridge.record_child_outcome)-->
//!   EpisodicPartition::record_with_embedding -->
//!   embedder produces 384-dim vec -->
//!   vec_episodes_384 virtual table stores it -->
//!   retrieve::search_basic KNN path finds it back.
//!
//! This test is the integration-level proof that the M4 carryovers
//! (vec0 KNN wiring + dim-aware substrate) actually plumb together
//! with the M5.7 bridge — not just that each step compiles in
//! isolation.

use std::time::Duration;

use wcore_memory::Memory;
use wcore_memory::db::vec_table_name_for_dim;
use wcore_memory::retrieve::search_basic;
use wcore_memory::v2_types::{Query, Tier};
use wcore_swarm::{SwarmMemoryBridge, SwarmResult, WorkerStatus};

fn make_result(worker_id: &str, summary_marker: &str) -> SwarmResult {
    SwarmResult {
        worker_id: worker_id.to_string(),
        branch: format!("swarm/e2e/{worker_id}"),
        status: WorkerStatus::Succeeded,
        stdout: summary_marker.to_string(),
        stderr: String::new(),
        duration: Duration::from_millis(123),
    }
}

#[tokio::test]
async fn dispatch_outcome_round_trips_via_knn_substrate() {
    let mem = Memory::open_in_memory().await.unwrap();
    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "orchestrator-e2e".to_string());

    // 3 worker outcomes — each becomes one Episode with one vec0 row.
    // We use distinct stdout markers so the retrieval text query has
    // something to discriminate on. The summaries themselves carry
    // worker={id} so the BM25 + cosine fusion can pick a winner.
    for i in 1..=3 {
        let r = make_result(&format!("w-{i}"), &format!("marker-{i}"));
        bridge
            .record_child_outcome(&format!("w-{i}"), &r)
            .await
            .unwrap();
    }

    // After 3 record_with_embedding calls, the dim-aware vec table
    // for 384 dims (the hashed embedder default) must exist on every
    // tier conn.
    let table = vec_table_name_for_dim(384);
    assert_eq!(table, "vec_episodes", "384-dim retains the legacy name");
    let project = mem.db.project.clone().unwrap();
    // Scoped lock — the parking_lot MutexGuard MUST NOT cross the
    // `search_basic` await below or clippy::await_holding_lock fires.
    {
        let conn = project.conn.lock();
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1 AND type = 'table'",
                [&table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "vec_episodes_384 must exist on project tier");
        let registered: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM vec_episodes_registry WHERE dim = 384",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(registered, 1, "registry must record dim=384");
        let mirror_rows: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(mirror_rows, 3, "expected 3 mirrored vec0 rows");
    }

    // Now the KNN-backed search should return at least one of the
    // three outcomes when we query with one of the embedded markers.
    let query = Query {
        text: "worker=w-2".to_string(),
        tier: Tier::Project,
        partition: None,
        entities: None,
        limit_per_modality: 5,
        kg_depth: 0,
        token_budget: None,
    };
    let hits = search_basic(&mem.db, mem.embedder.as_ref(), &query)
        .await
        .unwrap();
    assert!(
        !hits.is_empty(),
        "KNN-backed search returned no hits for marker query"
    );
    // The top hit should be one of the three episodes we wrote.
    let top_summary = &hits[0].preview;
    assert!(
        top_summary.contains("worker=w-1")
            || top_summary.contains("worker=w-2")
            || top_summary.contains("worker=w-3"),
        "top hit preview unexpected: {top_summary}"
    );
}

#[tokio::test]
async fn legacy_record_path_is_still_findable_via_cosine_fallback() {
    // Belt-and-suspenders: rows written via the LEGACY
    // EpisodicPartition::record path (no vec0 mirror) must still be
    // retrievable because retrieve.rs falls back to the O(n) cosine
    // pass when knn_pass returns empty.
    let mem = Memory::open_in_memory().await.unwrap();
    let ep = wcore_memory::v2_types::Episode {
        id: wcore_memory::v2_types::EpisodeId::new(),
        tier: Tier::Project,
        ts: 1_700_000_000,
        episode_type: "legacy_path".into(),
        summary: "legacy-marker-zzz unique-token".into(),
        atomic_facts: vec![],
        source: "main-agent".into(),
        source_product: "wcore-agent".into(),
        session_id: Some("legacy".into()),
        project_root: None,
        decay_score: 1.0,
        status: wcore_memory::v2_types::EpisodeStatus::Active,
    };
    mem.dispatcher.episodic.record(ep).await.unwrap();

    let q = Query {
        text: "legacy-marker-zzz unique-token".into(),
        tier: Tier::Project,
        partition: None,
        entities: None,
        limit_per_modality: 5,
        kg_depth: 0,
        token_budget: None,
    };
    let hits = search_basic(&mem.db, mem.embedder.as_ref(), &q)
        .await
        .unwrap();
    assert!(
        !hits.is_empty(),
        "legacy record path must remain findable via cosine fallback"
    );
}
