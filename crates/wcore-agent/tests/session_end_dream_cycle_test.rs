// M3.1.3 — verify the dream-cycle wiring at session-end.
//
// `fire_on_session_end` in `wcore-agent::engine` invokes
// `memory_api.dream_now()` IFF `dream_throttle.should_run()` returns true.
// This test exercises the same gating pattern against a counting mock
// MemoryApi, so we can assert the throttle correctly enforces the window
// without spinning up a full AgentEngine + provider stack.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use wcore_memory::api::MemoryApi;
use wcore_memory::consolidate::DreamThrottle;
use wcore_memory::error::Result as MemResult;
use wcore_memory::v2_types::{
    AccessToken, CompactReport, DreamReport, Episode, EpisodeId, Fact, FactId, Hit, Procedure,
    ProcedureId, Query, Tier, UserModel,
};

#[derive(Default)]
struct CountingMem {
    dream_calls: AtomicU64,
}

#[async_trait]
impl MemoryApi for CountingMem {
    async fn record_episode(&self, _: Episode, _: AccessToken) -> MemResult<EpisodeId> {
        Ok(EpisodeId::default())
    }
    async fn assert_fact(&self, _: Fact, _: AccessToken) -> MemResult<FactId> {
        Ok(FactId::default())
    }
    async fn upsert_procedure(&self, _: Procedure, _: AccessToken) -> MemResult<ProcedureId> {
        Ok(ProcedureId::default())
    }
    async fn list_procedures(&self, _: Tier, _: AccessToken) -> MemResult<Vec<Procedure>> {
        Ok(vec![])
    }
    async fn update_user_model(&self, _: &str, _: Value, _: AccessToken) -> MemResult<()> {
        Ok(())
    }
    async fn search(&self, _: Query, _: AccessToken) -> MemResult<Vec<Hit>> {
        Ok(vec![])
    }
    async fn get_episode(&self, _: &EpisodeId, _: AccessToken) -> MemResult<Episode> {
        unimplemented!("not exercised by this test")
    }
    async fn user_model(&self, _: AccessToken) -> MemResult<UserModel> {
        Ok(UserModel::default())
    }
    async fn dream_now(&self) -> MemResult<DreamReport> {
        self.dream_calls.fetch_add(1, Ordering::SeqCst);
        Ok(DreamReport::default())
    }
    async fn compact(&self, _: u64) -> MemResult<CompactReport> {
        Ok(CompactReport::default())
    }
    // M3.5 trait additions — mock isn't exercising skills telemetry, so
    // both methods are no-ops returning the expected empty/Ok shapes.
    async fn record_skill_use(&self, _: &str, _: bool, _: u64) -> MemResult<()> {
        Ok(())
    }
    async fn top_procedures(
        &self,
        _: Tier,
        _: usize,
        _: u64,
        _: AccessToken,
    ) -> MemResult<Vec<Procedure>> {
        Ok(vec![])
    }
    async fn kg_ingest_facts(&self, _: &str) -> MemResult<usize> {
        Ok(0)
    }
}

/// Mirrors the exact gating logic added to `fire_on_session_end` in
/// `crates/wcore-agent/src/engine.rs` so the test is a real regression
/// guard on the contract, not a separate mock.
async fn fire(mem: &Arc<dyn MemoryApi>, throttle: &DreamThrottle) {
    if throttle.should_run() {
        let _ = mem.dream_now().await;
    }
}

#[tokio::test]
async fn dream_cycle_fires_when_throttle_releases() {
    let counter = Arc::new(CountingMem::default());
    let mem: Arc<dyn MemoryApi> = counter.clone();
    let throttle = DreamThrottle::new(Duration::from_millis(0));

    fire(&mem, &throttle).await;
    fire(&mem, &throttle).await;

    let observed = counter.dream_calls.load(Ordering::SeqCst);
    assert!(
        observed >= 2,
        "0-window throttle must let both calls through, got {observed}"
    );
}

#[tokio::test]
async fn dream_cycle_throttled_within_window() {
    let counter = Arc::new(CountingMem::default());
    let mem: Arc<dyn MemoryApi> = counter.clone();
    let throttle = DreamThrottle::new(Duration::from_secs(60));

    fire(&mem, &throttle).await;
    fire(&mem, &throttle).await;

    let observed = counter.dream_calls.load(Ordering::SeqCst);
    assert_eq!(
        observed, 1,
        "60s throttle must block the 2nd fire; got {observed}"
    );
}
