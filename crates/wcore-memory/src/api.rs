// M3 — Public MemoryApi trait.
//
// PartitionDispatcher (partition/mod.rs) is the only production implementor.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::Result;
use crate::v2_types::{
    AccessToken, CompactReport, DreamReport, Episode, EpisodeId, Fact, FactId, Hit, Partition,
    Procedure, ProcedureId, Query, Tier, UserModel,
};

#[async_trait]
pub trait MemoryApi: Send + Sync {
    async fn record_episode(&self, ep: Episode, tok: AccessToken) -> Result<EpisodeId>;
    async fn assert_fact(&self, f: Fact, tok: AccessToken) -> Result<FactId>;
    async fn upsert_procedure(&self, p: Procedure, tok: AccessToken) -> Result<ProcedureId>;
    /// W9 F11: enumerate P4 rows at the given tier. Returns the full set
    /// (no pagination — P4 is small by design: tens to hundreds of skills).
    async fn list_procedures(&self, tier: Tier, tok: AccessToken) -> Result<Vec<Procedure>>;
    async fn update_user_model(&self, key: &str, val: Value, tok: AccessToken) -> Result<()>;
    async fn search(&self, q: Query, tok: AccessToken) -> Result<Vec<Hit>>;
    async fn get_episode(&self, id: &EpisodeId, tok: AccessToken) -> Result<Episode>;
    async fn user_model(&self, tok: AccessToken) -> Result<UserModel>;
    async fn dream_now(&self) -> Result<DreamReport>;
    async fn compact(&self, target_tokens: u64) -> Result<CompactReport>;

    /// M3.5 — record one skill invocation outcome. Upserts a row named
    /// `skill:<skill_name>` in the procedural partition at `Tier::Project`
    /// and updates Thompson stats via `ProceduralPartition::record_use`.
    /// `latency_ms` is accepted for future use (per-invocation latency
    /// logging is a future schema extension); current impls accept-and-ignore.
    async fn record_skill_use(
        &self,
        skill_name: &str,
        succeeded: bool,
        latency_ms: u64,
    ) -> Result<()>;

    /// M3.5/M3.6 — top-K skills in the procedural partition ranked by a
    /// Beta-mean score (`alpha / (alpha + beta)`), with a `min_uses` filter
    /// so brand-new rows do not dominate the ranking. Consumed by the
    /// session-start prioritizer.
    async fn top_procedures(
        &self,
        tier: Tier,
        k: usize,
        min_uses: u64,
        tok: AccessToken,
    ) -> Result<Vec<Procedure>>;

    /// W5 — extract facts from a session/turn transcript and upsert them
    /// into the knowledge graph (`fact_extractor::ingest_facts_to_kg`).
    /// Returns the number of facts ingested. Callers gate this on
    /// [`crate::kg::kg_enabled`]; the no-op impls (`NullMemory`) return
    /// `Ok(0)`.
    async fn kg_ingest_facts(&self, transcript: &str) -> Result<usize>;

    /// v0.8.0 N.1 — bulk-clear every row in a single partition at the
    /// given tier. Returns the number of rows deleted. Wired into the
    /// `/memory clear <partition>` slash command (`MemoryHandler::Runtime`).
    ///
    /// The default impl returns `Ok(0)` so test fixtures and `NullMemory`
    /// keep their no-op semantics. `PartitionDispatcher` overrides this
    /// with a real SQL DELETE against the underlying SQLite table at the
    /// requested tier connection.
    ///
    /// Tiers honor the partition's design defaults:
    /// - Working: Session only
    /// - Episodic / Semantic / Procedural: Session / Project / Global
    /// - Core: Global only
    ///
    /// Invalid (partition, tier) combinations return
    /// `MemoryError::AccessDenied` via the existing gate check on the
    /// `PartitionDispatcher` impl.
    async fn clear_partition(
        &self,
        _partition: Partition,
        _tier: Tier,
        _tok: AccessToken,
    ) -> Result<usize> {
        Ok(0)
    }

    /// Rebind the session-tier store onto the real per-session DB file.
    ///
    /// Production bootstrap opens memory under a synthetic `"boot"` session id
    /// because the real id isn't known until `init_session` runs. Calling this
    /// afterward moves session-tier reads/writes onto `sessions/<id>.db`,
    /// giving each session its own isolated, cleanable file instead of one
    /// ever-growing shared `boot.db`. Tier::Project/Global are unaffected
    /// (Project is keyed by project_root, Global is fixed).
    ///
    /// The default impl is a no-op (`NullMemory`, test fixtures); only the
    /// real `PartitionDispatcher`/`Memory` perform the swap.
    async fn rebind_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
}
