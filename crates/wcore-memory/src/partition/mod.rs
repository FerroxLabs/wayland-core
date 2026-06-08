// Partition module: per-partition stores + the PartitionDispatcher that
// implements MemoryApi by routing each call through the gate to the right
// store.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use wcore_observability::sink::MemoryTraceSink;

use crate::api::MemoryApi;
use crate::cdc::CdcWriter;
use crate::db::Db;
use crate::embed::Embedder;
use crate::error::{MemoryError, Result};
use crate::gate::MemoryAccessGate;
use crate::v2_types::{
    AccessToken, CompactReport, DreamReport, Episode, EpisodeId, Fact, FactId, Hit, Partition,
    Procedure, ProcedureId, ProcedureStatus, Query, Tier, UserModel,
};

/// Common shape every partition store implements. Concrete types live in
/// each submodule (working, episodic, semantic, procedural, core).
#[async_trait]
pub trait PartitionStore: Send + Sync {
    type Item: Send + Sync;
    type Id: Send + Sync;

    async fn write(&self, item: Self::Item, tier: Tier) -> Result<Self::Id>;
    async fn read(&self, id: &Self::Id, tier: Tier) -> Result<Self::Item>;
    async fn delete_or_archive(&self, id: &Self::Id, tier: Tier) -> Result<()>;
}

pub mod collaboration;
pub mod core;
pub mod core_inference;
pub mod episodic;
pub mod prefixspan;
pub mod procedural;
pub mod semantic;
pub mod thompson;
pub mod working;

pub use collaboration::{
    AuditRecord, Blackboard, BlackboardEntry, BlackboardPredicate, Subscription,
};
pub use core::CorePartition;
pub use core_inference::UserModelInferencer;
pub use episodic::EpisodicPartition;
pub use prefixspan::{FrequentPattern, PrefixSpan, ToolSequence};
pub use procedural::ProceduralPartition;
pub use semantic::SemanticPartition;
pub use thompson::{ThompsonSampler, ToolCandidate, ToolSelectionResult};
pub use working::WorkingPartition;

/// M3 — PartitionDispatcher.
///
/// Holds one store per partition and dispatches MemoryApi calls through
/// the gate. Cheap to clone (everything inside is Arc-wrapped).
#[derive(Clone)]
pub struct PartitionDispatcher {
    pub gate: Arc<MemoryAccessGate>,
    pub db: Arc<Db>,
    pub embedder: Arc<dyn Embedder>,
    pub cdc: Arc<CdcWriter>,
    pub working: Arc<WorkingPartition>,
    pub episodic: Arc<EpisodicPartition>,
    pub semantic: Arc<SemanticPartition>,
    pub procedural: Arc<ProceduralPartition>,
    pub core: Arc<CorePartition>,
    /// M3.3 — optional observability sink. `None` by default so dispatcher
    /// construction sites that don't care about traces stay free of an
    /// explicit `Arc::new(NullSink)` wrapping.
    pub trace_sink: Option<Arc<dyn MemoryTraceSink>>,
}

impl PartitionDispatcher {
    pub fn new(
        gate: Arc<MemoryAccessGate>,
        db: Arc<Db>,
        embedder: Arc<dyn Embedder>,
        cdc: Arc<CdcWriter>,
        session_id: Option<String>,
    ) -> Self {
        let working = Arc::new(WorkingPartition::new(db.clone(), cdc.clone(), session_id));
        let episodic = Arc::new(EpisodicPartition::new(
            db.clone(),
            embedder.clone(),
            cdc.clone(),
        ));
        let semantic = Arc::new(SemanticPartition::new(
            db.clone(),
            embedder.clone(),
            cdc.clone(),
        ));
        let procedural = Arc::new(ProceduralPartition::new(db.clone(), cdc.clone()));
        let core = Arc::new(CorePartition::new(db.clone(), cdc.clone()));
        Self {
            gate,
            db,
            embedder,
            cdc,
            working,
            episodic,
            semantic,
            procedural,
            core,
            trace_sink: None,
        }
    }

    /// M3.3 — attach a `MemoryTraceSink`. Every `MemoryApi` call routed
    /// through this dispatcher will emit one event around the gated work.
    /// Cheap: replaces the existing `trace_sink` field; cloning the
    /// dispatcher still shares the same `Arc`.
    pub fn with_trace_sink(mut self, sink: Arc<dyn MemoryTraceSink>) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// M3.3 — emit one trace if a sink is attached. Private helper so the
    /// MemoryApi impls stay readable.
    fn emit_trace(&self, op: &str, partition: &str, tier: &str, latency_ms: u64, success: bool) {
        if let Some(sink) = self.trace_sink.as_ref() {
            sink.emit(op, partition, tier, latency_ms, success);
        }
    }
}

/// v0.6.4 Task 6.6b — Thompson-sample-rank a procedural row pool in place,
/// then truncate to the top `k`. Factored out of
/// `PartitionDispatcher::top_procedures` so deterministic tests can pass a
/// seeded sampler (`ThompsonSampler::with_seed(42)`) while production uses
/// `ThompsonSampler::new()`.
///
/// One Beta(`alpha`, `beta`) sample is drawn per candidate; higher samples
/// rank first. Replaces the v0.5 deterministic mean-rank `α/(α+β)` scorer.
pub fn rank_procedures_with_sampler(
    procs: &mut Vec<Procedure>,
    k: usize,
    sampler: &mut ThompsonSampler,
) {
    // Pre-compute one sample per row so the `sort_by` comparator stays
    // pure (sorts compare pairs multiple times; resampling inside the
    // comparator would re-randomize and break ordering).
    let scored: Vec<(usize, f64)> = procs
        .iter()
        .enumerate()
        .map(|(i, p)| (i, sampler.sample_beta(p.thompson_alpha, p.thompson_beta)))
        .collect();
    let mut order: Vec<usize> = scored.iter().map(|(i, _)| *i).collect();
    order.sort_by(|&a, &b| {
        scored[b]
            .1
            .partial_cmp(&scored[a].1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let reordered: Vec<Procedure> = order.into_iter().map(|i| procs[i].clone()).collect();
    *procs = reordered;
    procs.truncate(k);
}

#[async_trait]
impl MemoryApi for PartitionDispatcher {
    async fn record_episode(&self, ep: Episode, tok: AccessToken) -> Result<EpisodeId> {
        let start = std::time::Instant::now();
        let tier = ep.tier;
        let result = async {
            self.gate.check_write(&tok, Partition::Episodic, tier)?;
            self.episodic.record(ep).await
        }
        .await;
        self.emit_trace(
            "record_episode",
            Partition::Episodic.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn assert_fact(&self, f: Fact, tok: AccessToken) -> Result<FactId> {
        let start = std::time::Instant::now();
        let tier = f.tier;
        let result = async {
            self.gate.check_write(&tok, Partition::Semantic, tier)?;
            self.semantic.assert(f).await
        }
        .await;
        self.emit_trace(
            "assert_fact",
            Partition::Semantic.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn upsert_procedure(&self, p: Procedure, tok: AccessToken) -> Result<ProcedureId> {
        let start = std::time::Instant::now();
        let tier = p.tier;
        let result = async {
            self.gate.check_write(&tok, Partition::Procedural, tier)?;
            self.procedural.upsert(p).await
        }
        .await;
        self.emit_trace(
            "upsert_procedure",
            Partition::Procedural.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn list_procedures(&self, tier: Tier, tok: AccessToken) -> Result<Vec<Procedure>> {
        let start = std::time::Instant::now();
        let result = async {
            self.gate.check_read(&tok, Partition::Procedural, tier)?;
            self.procedural.list(tier).await
        }
        .await;
        self.emit_trace(
            "list_procedures",
            Partition::Procedural.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn update_user_model(&self, key: &str, val: Value, tok: AccessToken) -> Result<()> {
        let start = std::time::Instant::now();
        let result = async {
            self.gate.check_write(&tok, Partition::Core, Tier::Global)?;
            self.core.update(key, val, &tok).await
        }
        .await;
        self.emit_trace(
            "update_user_model",
            Partition::Core.as_str(),
            Tier::Global.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn search(&self, q: Query, tok: AccessToken) -> Result<Vec<Hit>> {
        // Default partition for search is Episodic; if pinned, gate against
        // it. Group D wires the real hybrid retriever; this stub returns a
        // BM25-only result so C-level dispatcher tests can pass.
        let start = std::time::Instant::now();
        let partition = q.partition.unwrap_or(Partition::Episodic);
        let tier = q.tier;
        let result: Result<Vec<Hit>> = async {
            self.gate.check_read(&tok, partition, tier)?;
            let mut hits =
                crate::retrieve::search_basic(&self.db, self.embedder.as_ref(), &q).await?;
            // Semantic-fact recall. `assert_fact` persists facts into the P3
            // Semantic `facts` table, which `search_basic` (episodic-only)
            // never reads — the cross-session recall gap. Append the durable
            // facts most relevant to the query so `session_search` and the
            // engine's session-start recall surface them. Gated separately so
            // a sub-agent without Semantic read scope still only gets episodes;
            // a denied check simply skips the fact pass (episodic hits stand).
            if self
                .gate
                .check_read(&tok, Partition::Semantic, tier)
                .is_ok()
            {
                let facts =
                    crate::retrieve::facts_search(&self.db, self.embedder.as_ref(), &q).await?;
                hits.extend(facts);
            }
            Ok(hits)
        }
        .await;
        self.emit_trace(
            "search",
            partition.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn get_episode(&self, id: &EpisodeId, tok: AccessToken) -> Result<Episode> {
        // We don't know the tier yet — let the caller hint via the Query
        // API for hot paths; for direct gets, try project then global.
        let start = std::time::Instant::now();
        let mut found_tier: Option<Tier> = None;
        let result: Result<Episode> = async {
            for tier in [Tier::Project, Tier::Global, Tier::Session] {
                self.gate.check_read(&tok, Partition::Episodic, tier)?;
                if let Ok(ep) = self.episodic.get(id, tier).await {
                    found_tier = Some(tier);
                    return Ok(ep);
                }
            }
            Err(MemoryError::Consolidation(format!(
                "episode {} not found in any tier",
                id.0
            )))
        }
        .await;
        // Report the tier we found the row in if any; otherwise "-" to
        // signal "tried all".
        let tier_str = found_tier.map(|t| t.as_str()).unwrap_or("-");
        self.emit_trace(
            "get_episode",
            Partition::Episodic.as_str(),
            tier_str,
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn user_model(&self, tok: AccessToken) -> Result<UserModel> {
        let start = std::time::Instant::now();
        let result = async {
            self.gate.check_read(&tok, Partition::Core, Tier::Global)?;
            self.core.read_all().await
        }
        .await;
        self.emit_trace(
            "user_model",
            Partition::Core.as_str(),
            Tier::Global.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn dream_now(&self) -> Result<DreamReport> {
        let start = std::time::Instant::now();
        let result = crate::consolidate::ConsolidationEngine::new(self.clone())
            .run()
            .await;
        // Cross-partition op; "-" partition + tier.
        self.emit_trace(
            "dream_now",
            "-",
            "-",
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn compact(&self, target_tokens: u64) -> Result<CompactReport> {
        let start = std::time::Instant::now();
        let result = crate::compact::compact(self, target_tokens).await;
        self.emit_trace(
            "compact",
            "-",
            "-",
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn record_skill_use(
        &self,
        skill_name: &str,
        succeeded: bool,
        _latency_ms: u64,
    ) -> Result<()> {
        let row_name = format!("skill:{skill_name}");
        let tier = Tier::Project;

        // 1. Find the existing row by name, or create a new one. Skill
        // telemetry rows always live at Tier::Project (procedural is a
        // partition, not a tier — writes target Project).
        let procs = self.procedural.list(tier).await?;
        let id = if let Some(p) = procs.iter().find(|p| p.name == row_name) {
            p.id
        } else {
            let p = Procedure {
                id: ProcedureId::new(),
                tier,
                ts: 0,
                name: row_name,
                description: format!("Auto-created on first invocation of skill {skill_name}"),
                artifact: String::new(),
                status: ProcedureStatus::Active,
                created_by: "skill_telemetry".into(),
                thompson_alpha: 1.0,
                thompson_beta: 1.0,
                use_count: 0,
                success_count: 0,
            };
            let id = p.id;
            self.procedural.upsert(p).await?;
            id
        };

        // 2. Record the use (updates Thompson stats + counts).
        self.procedural.record_use(&id, tier, succeeded).await
    }

    async fn top_procedures(
        &self,
        tier: Tier,
        k: usize,
        min_uses: u64,
        tok: AccessToken,
    ) -> Result<Vec<Procedure>> {
        self.gate.check_read(&tok, Partition::Procedural, tier)?;
        let mut all = self.procedural.list(tier).await?;
        all.retain(|p| p.use_count >= min_uses);
        // v0.6.4 Task 6.6b: replace deterministic Beta-mean exploit-only
        // scorer with one Thompson sample per row (Forge parity). The
        // production sampler seeds from the OS RNG; tests use the
        // `rank_procedures_with_sampler` helper with `with_seed(42)`.
        let mut sampler = ThompsonSampler::new();
        rank_procedures_with_sampler(&mut all, k, &mut sampler);
        Ok(all)
    }

    async fn kg_ingest_facts(&self, transcript: &str) -> Result<usize> {
        // W5 — upsert extracted facts into the KG against the raw session-
        // tier `Connection` (the canonical KG owner; W2 runs `init_kg` on
        // the same tier under the `kg_enabled()` gate).
        let tier_conn =
            self.db
                .tier(Tier::Session)
                .ok_or_else(|| crate::error::MemoryError::AccessDenied {
                    partition: "kg".into(),
                    tier: "session".into(),
                    reason: "no session tier configured for KG ingest".into(),
                })?;
        let conn = tier_conn.conn.lock();
        crate::fact_extractor::ingest_facts_to_kg(&conn, transcript)
    }

    /// v0.8.0 N.1 — bulk-clear every row in the given partition at the
    /// given tier. Used by the `/memory clear <partition>` slash command.
    ///
    /// Implementation: gate-check the write, resolve the tier connection,
    /// and run a tier-scoped `DELETE FROM <table> WHERE tier = ?1` against
    /// the underlying SQLite. The Core partition's `user_model` table is
    /// tier-less (Global-only by design) so the WHERE clause is omitted
    /// there. The Working partition's `p1_working` table is also tier-less
    /// (Session DB only by design).
    async fn clear_partition(
        &self,
        partition: Partition,
        tier: Tier,
        tok: AccessToken,
    ) -> Result<usize> {
        let start = std::time::Instant::now();
        let result = async {
            self.gate.check_write(&tok, partition, tier)?;

            // Resolve the connection for the requested tier. If the tier
            // wasn't configured (e.g. no session DB on a project-only
            // handle) there is nothing to delete — return 0.
            let tier_conn = match self.db.tier(tier) {
                Some(tc) => tc,
                None => return Ok::<usize, MemoryError>(0),
            };
            let conn = tier_conn.conn.lock();

            // Per-partition table mapping (mirrors schema/v1.sql). The
            // FTS triggers + sqlite-vec virtual tables keep themselves in
            // sync via the `episodes_ad` AFTER DELETE trigger for FTS and
            // via the per-row delete path in the partition stores; bulk
            // DELETE on the parent table fires the FTS trigger and the
            // vec0 mirror tables are reconciled at next consolidation.
            let deleted: usize = match partition {
                Partition::Working => {
                    // p1_working is tier-less (session DB only by design).
                    if tier != Tier::Session {
                        return Ok(0);
                    }
                    conn.execute("DELETE FROM p1_working", [])
                        .map_err(MemoryError::Db)?
                }
                Partition::Episodic => conn
                    .execute("DELETE FROM episodes WHERE tier = ?1", [tier.as_str()])
                    .map_err(MemoryError::Db)?,
                Partition::Semantic => conn
                    .execute("DELETE FROM facts WHERE tier = ?1", [tier.as_str()])
                    .map_err(MemoryError::Db)?,
                Partition::Procedural => conn
                    .execute("DELETE FROM procedures WHERE tier = ?1", [tier.as_str()])
                    .map_err(MemoryError::Db)?,
                Partition::Core => {
                    // Core / user_model is Global-only and tier-less.
                    if tier != Tier::Global {
                        return Ok(0);
                    }
                    conn.execute("DELETE FROM user_model", [])
                        .map_err(MemoryError::Db)?
                }
            };
            Ok(deleted)
        }
        .await;
        self.emit_trace(
            "clear_partition",
            partition.as_str(),
            tier.as_str(),
            start.elapsed().as_millis() as u64,
            result.is_ok(),
        );
        result
    }

    async fn rebind_session(&self, session_id: &str) -> Result<()> {
        // Resolve the real per-session DB path and swap the session-tier
        // connection in place. The dispatcher's attached trace sink + decay
        // scheduler are untouched (they ride the dispatcher Arcs, not the DB
        // handle). Also update the working partition's CDC tag so spillover
        // events carry the real id.
        let path = crate::paths::session_db_path(session_id).ok_or_else(|| {
            MemoryError::PathValidation("no session memory DB path resolvable".into())
        })?;
        self.db.rebind_session(path)?;
        self.working.set_session_id(Some(session_id.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcedureStatus, ThompsonSampler, rank_procedures_with_sampler};
    use crate::v2_types::{Partition, Procedure, ProcedureId, Tier, valid_combinations};

    fn mk_proc(name: &str, alpha: f64, beta: f64) -> Procedure {
        Procedure {
            id: ProcedureId::new(),
            tier: Tier::Project,
            ts: 0,
            name: name.into(),
            description: String::new(),
            artifact: String::new(),
            status: ProcedureStatus::Active,
            created_by: "test".into(),
            thompson_alpha: alpha,
            thompson_beta: beta,
            use_count: ((alpha + beta) as u64).saturating_sub(2),
            success_count: (alpha as u64).saturating_sub(1),
        }
    }

    /// Task 6.6b statistical assertion: over N=1000 reseeded selections, the
    /// strong arm (Beta(51, 2), mean ≈ 0.962) must win the top slot ≥ 950
    /// times against the weak arm (Beta(2, 51), mean ≈ 0.038). Mirrors the
    /// `dominant_arm_wins` golden in `partition::thompson::tests`.
    #[test]
    fn strong_arm_wins_top_slot_at_least_95pct() {
        let n: u32 = 1_000;
        let mut strong_wins: u32 = 0;
        for i in 0..n {
            let mut procs = vec![mk_proc("strong", 51.0, 2.0), mk_proc("weak", 2.0, 51.0)];
            // Reseed per trial so the same seed base reproduces the run.
            let mut sampler = ThompsonSampler::with_seed(42 + i as u64);
            rank_procedures_with_sampler(&mut procs, 2, &mut sampler);
            assert_eq!(procs.len(), 2);
            if procs[0].name == "strong" {
                strong_wins += 1;
            }
        }
        assert!(
            strong_wins >= 950,
            "strong arm should win >=950/1000 top slots, got {strong_wins}"
        );
    }

    /// `k` truncates the returned pool after sampling.
    #[test]
    fn rank_procedures_truncates_to_k() {
        let mut procs = vec![
            mk_proc("a", 5.0, 1.0),
            mk_proc("b", 4.0, 1.0),
            mk_proc("c", 3.0, 1.0),
        ];
        let mut sampler = ThompsonSampler::with_seed(42);
        rank_procedures_with_sampler(&mut procs, 2, &mut sampler);
        assert_eq!(procs.len(), 2, "k=2 truncates to two rows");
    }

    #[test]
    fn all_storage_targets_has_nine() {
        // Sanity: 9 valid combos across 5 partitions × 3 tiers.
        assert_eq!(valid_combinations().len(), 9);
        // P1 has only one slot, P5 has only one slot.
        let p1_slots = valid_combinations()
            .iter()
            .filter(|(p, _)| matches!(p, Partition::Working))
            .count();
        let p5_slots = valid_combinations()
            .iter()
            .filter(|(p, _)| matches!(p, Partition::Core))
            .count();
        assert_eq!(p1_slots, 1);
        assert_eq!(p5_slots, 1);
    }
}
