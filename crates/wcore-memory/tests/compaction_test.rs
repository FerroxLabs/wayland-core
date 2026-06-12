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
use wcore_memory::v2_types::{EpisodeId, Tier};

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

/// Rank 67 — pin the `String -> EpisodeId` round-trip that the
/// "non-destructive" guarantee depends on. The bookmark that replaces the
/// offloaded turns stores `ep_id.0.to_string()`; this test proves that
/// string parses back into an `EpisodeId` and that `episodic.get()` returns
/// the very episode whose `summary`/`atomic_facts` absorbed the compacted
/// turns. If the stored format ever drifts away from what `get()` parses,
/// recovery breaks silently — this test fails loudly instead.
#[tokio::test]
async fn bookmark_episode_id_round_trips_to_get_episode() {
    let d = fresh_dispatcher().await;

    // Seed enough turns that compaction must offload some of them.
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

    let report = d.compact(5_000).await.unwrap();
    assert!(
        report.turns_offloaded > 0 && report.bookmarks_inserted == 1,
        "expected a bookmark to be inserted: {report:?}"
    );

    // Extract the bookmark's stored episode_id String from live P1.
    let snapshot = d.working.snapshot();
    let episode_id_str = snapshot
        .iter()
        .find_map(|e| match e {
            WorkingEntry::Bookmark { episode_id, .. } => Some(episode_id.clone()),
            _ => None,
        })
        .expect("compaction must leave a Bookmark in working memory");

    // The exact recovery path: parse the String back into an EpisodeId.
    let parsed = uuid::Uuid::parse_str(&episode_id_str)
        .map(EpisodeId)
        .expect("bookmark episode_id must parse back into an EpisodeId");

    // compact() persists the absorbing episode at Tier::Project.
    let recovered = d
        .episodic
        .get(&parsed, Tier::Project)
        .await
        .expect("episode referenced by the bookmark must be recoverable");

    // The recovered episode is the compaction episode, and its preview
    // matches the bookmark's summary_preview (both derived from the same
    // summary), proving the round-trip recovers the right episode.
    assert_eq!(recovered.id, parsed, "get() returned a different episode");
    assert_eq!(
        recovered.source_product, "wcore-compact-internal",
        "recovered episode is not the Letta compaction episode"
    );
    let preview: String = recovered.summary.chars().take(120).collect();
    let bookmark_preview = snapshot
        .iter()
        .find_map(|e| match e {
            WorkingEntry::Bookmark {
                summary_preview, ..
            } => Some(summary_preview.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        preview, bookmark_preview,
        "recovered episode summary does not match the bookmark preview"
    );

    // Non-destructive guarantee: every offloaded turn is recoverable from
    // the episode reached purely through the bookmark round-trip.
    for marker in ["turn 3:", "turn 11:", "turn 22:"] {
        assert!(
            recovered.atomic_facts.iter().any(|f| f.contains(marker)),
            "marker '{marker}' missing from the round-trip-recovered episode"
        );
    }
}
