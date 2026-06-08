// W5 Group E acceptance: Letta compact ≥45% token reduction, non-destructive.

use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::partition::working::WorkingEntry;

async fn fresh_dispatcher() -> PartitionDispatcher {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    // Use a large in-memory cap so seeding 50 turns doesn't spill before
    // compaction runs (we want the compact to see them in P1).
    let mut dispatcher = PartitionDispatcher::new(gate, db, embedder, cdc, Some("s".into()));
    // Replace working partition with a higher cap.
    dispatcher.working = Arc::new(
        wcore_memory::partition::working::WorkingPartition::new(
            dispatcher.db.clone(),
            dispatcher.cdc.clone(),
            Some("s".into()),
        )
        .with_cap(200),
    );
    dispatcher
}

#[tokio::test]
async fn compact_reduces_tokens_and_remains_recoverable() {
    let d = fresh_dispatcher().await;

    // Seed ~50 turns, each ~200 words. Tokens-before ~ 10K.
    let big_words: String = "lorem ipsum dolor sit amet ".repeat(40);
    for i in 0..50 {
        d.working
            .push(WorkingEntry::Turn {
                ts: i,
                role: if i % 2 == 0 {
                    "user".into()
                } else {
                    "assistant".into()
                },
                text: format!("turn {i}: {big_words}"),
            })
            .await
            .unwrap();
    }
    // Sample three known fragments we'll look for post-compaction.
    // Pick three markers from the oldest half — they're the ones the
    // Letta-style compaction offloads (oldest-first until budget hit).
    let markers = ["turn 3:", "turn 11:", "turn 22:"];
    let total_tokens_before = d
        .working
        .snapshot()
        .iter()
        .map(wcore_memory::compact::entry_tokens)
        .sum::<u64>();
    assert!(
        total_tokens_before >= 10_000,
        "before {total_tokens_before}"
    );

    let report = d.compact(5_000).await.unwrap();
    assert_eq!(report.tokens_before, total_tokens_before);
    let ratio = report.tokens_after as f64 / report.tokens_before as f64;
    assert!(
        ratio <= 0.55,
        "compaction kept {ratio:.2} of original (need <= 0.55)"
    );

    // Recoverability: the absorbing P2 episode has every offloaded
    // turn in atomic_facts, so direct table inspection lets us prove
    // recoverability without depending on FTS5 token rules.
    let tc = d.db.tier_or_global(wcore_memory::v2_types::Tier::Project);
    let conn = tc.conn.lock();
    let count_each: Vec<i64> = markers
        .iter()
        .map(|m| {
            let pat = format!("%{m}%");
            conn.query_row(
                "SELECT COUNT(*) FROM episodes WHERE source_product = 'wcore-compact-internal' AND atomic_facts LIKE ?1",
                [&pat],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        })
        .collect();
    for (m, c) in markers.iter().zip(&count_each) {
        assert!(*c >= 1, "marker '{m}' not recoverable via atomic_facts");
    }
}
