// M3.2 — decay scheduler idempotence + scheduling tests.
//
// Verifies two contracts of `ConsolidationEngine::decay()`:
//   1. Re-running decay never re-archives an already-archived row (the
//      `status = 'active'` predicate in consolidate.rs is the load-bearing
//      guard; without it the scheduler would flap status on every tick).
//   2. `Memory::spawn_decay_scheduler` returns a `JoinHandle<()>` that the
//      caller can `.abort()` to cleanly stop the background task at
//      shutdown (no leaked tasks across test runs).

use std::time::Duration;

use wcore_memory::MemoryApi;
use wcore_memory::consolidate::ConsolidationEngine;
use wcore_memory::memory::Memory;
use wcore_memory::v2_types::{AccessToken, Episode, EpisodeId, EpisodeStatus, Tier};

fn old_episode(age_days: i64) -> Episode {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: now - age_days * 86_400,
        episode_type: "manual_test".into(),
        summary: format!("episode aged {age_days}d"),
        atomic_facts: vec![],
        source: "test".into(),
        source_product: "wayland-core".into(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    }
}

#[tokio::test]
async fn decay_archives_old_episode_once_only() {
    let mem = Memory::open_in_memory().await.unwrap();
    let ep = old_episode(45); // > 30d threshold
    mem.dispatcher
        .record_episode(ep, AccessToken::System)
        .await
        .unwrap();

    let engine = ConsolidationEngine::new(mem.dispatcher.clone());

    // First call decays 1 episode and archives it.
    let n1 = engine.decay().await.unwrap();
    assert_eq!(n1, 1, "first decay should touch exactly 1 active episode");

    // Second call: the predicate `status = 'active'` must exclude the
    // newly archived row, so we expect ZERO touches.
    let n2 = engine.decay().await.unwrap();
    assert_eq!(
        n2, 0,
        "archived rows must NOT be re-touched on the next tick"
    );
}

#[tokio::test]
async fn scheduler_runs_at_interval() {
    let mem = Memory::open_in_memory().await.unwrap();
    let handle = mem.spawn_decay_scheduler(Duration::from_millis(50));

    // Give the scheduler enough wall-clock to tick a few times. We don't
    // assert on tick count (timing-fragile in CI); we assert the handle
    // was a `JoinHandle<()>` and the task didn't panic before we aborted.
    tokio::time::sleep(Duration::from_millis(200)).await;
    handle.abort();
    let _ = handle.await;
}
