//! E5 scenario 4 — memory concurrent writes.
//!
//! Spawn 10 concurrent tokio tasks each calling
//! `EpisodicPartition::record_with_embedding` with distinct content.
//! After all join:
//!   - 10 episode rows present in the DB (no lost writes).
//!   - No orphan rows (episodes count == vec0 mirror count).
//!   - vec0 mirror is in sync (leverages A6 transactional writes).

use std::sync::Arc;

use wcore_memory::Memory;
use wcore_memory::db::vec_table_name_for_dim;
use wcore_memory::partition::episodic::EpisodicPartition;
use wcore_memory::v2_types::{Episode, EpisodeId, EpisodeStatus, Tier};

#[tokio::test]
async fn ten_concurrent_record_with_embedding_no_orphans() {
    let mem = Memory::open_in_memory().await.unwrap();

    // Build EpisodicPartition directly — same DB + embedder the Memory facade uses.
    let partition = Arc::new(EpisodicPartition::new(
        mem.db.clone(),
        mem.embedder.clone(),
        mem.cdc.clone(),
    ));

    let n = 10usize;
    let mut handles = Vec::with_capacity(n);

    for i in 0..n {
        let ep_partition = partition.clone();
        handles.push(tokio::spawn(async move {
            let ep = Episode {
                id: EpisodeId::new(),
                tier: Tier::Global,
                ts: 0, // auto-filled by record_with_embedding
                episode_type: "concurrent_write".into(),
                summary: format!("concurrent task {i} unique content marker"),
                atomic_facts: vec![],
                source: format!("task-{i}"),
                source_product: "e2e-test".into(),
                session_id: Some(format!("session-{i}")),
                project_root: None,
                decay_score: 1.0,
                status: EpisodeStatus::Active,
            };
            ep_partition.record_with_embedding(ep).await
        }));
    }

    // All tasks must complete without error.
    for (i, handle) in handles.into_iter().enumerate() {
        handle
            .await
            .unwrap_or_else(|e| panic!("task {i} panicked: {e}"))
            .unwrap_or_else(|e| panic!("task {i} returned error: {e}"));
    }

    // Inspect DB directly — must have exactly n episodes.
    let global_tc = mem.db.tier_or_global(Tier::Global);
    let conn = global_tc.conn.lock();

    let episode_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        episode_count, n as i64,
        "expected {n} episode rows, got {episode_count}"
    );

    // vec0 mirror must match: no orphan rows.
    let dim = mem.embedder.dim();
    let vec_table = vec_table_name_for_dim(dim);
    let vec_count: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM {vec_table}"), [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        vec_count, episode_count,
        "vec0 mirror must be in sync with episodes: episodes={episode_count}, vec={vec_count}"
    );
}
