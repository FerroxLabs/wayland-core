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

/// The refusal a control returns when the backend has no store to act on.
/// Shared by the `*_recalled` defaults so a `NullMemory` cannot report a
/// control as applied.
fn no_controls() -> crate::error::MemoryError {
    crate::error::MemoryError::InvalidControl(
        "this memory backend exposes no operator controls".to_string(),
    )
}

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

    /// F23-03 — the operator control surface over recall, when this backend
    /// has one.
    ///
    /// Returns `None` for `NullMemory` and test fixtures, which have no store
    /// to correct, forget, scope or bound. A caller that gets `None` must say
    /// so rather than reporting a control as applied: the whole value of these
    /// controls is that a user can trust the answer.
    fn controls(&self) -> Option<crate::provenance::MemoryControls> {
        None
    }

    /// 23B-C3 — forget a recalled item by the id a user actually saw,
    /// whichever partition holds it.
    ///
    /// `/memory forget <id>` used to call `MemoryControls::forget_episode`
    /// directly, which hardcodes `Partition::Episodic`. Semantic facts are the
    /// only partition the engine auto-injects into the outbound provider
    /// request body, so a user could see a fact in their prompt, ask for it to
    /// be forgotten, and be told `NotFound` while it kept being sent. This
    /// tries episodic first (unchanged behaviour for every id that resolved
    /// before) and falls through to the fact store ONLY on `NotFound` — never
    /// on a denial, so a gate refusal is still surfaced as a refusal instead
    /// of being retried against another cell.
    async fn forget_recalled(
        &self,
        tier: Tier,
        id: &str,
        actor: &str,
        tok: AccessToken,
    ) -> Result<crate::provenance::ForgetReceipt> {
        let controls = self.controls().ok_or_else(no_controls)?;
        match controls.forget_episode(&tok, tier, id, actor) {
            Err(crate::error::MemoryError::NotFound { .. }) => {
                controls.forget_fact(&tok, tier, id, actor)
            }
            other => other,
        }
    }

    /// 23B-C3 — correct a recalled item by the id a user actually saw.
    ///
    /// The default impl handles episodes and REFUSES OUT LOUD for facts,
    /// because correcting a fact requires re-embedding the corrected triple
    /// and this trait's default has no embedder. A silent partial success —
    /// updating the text and leaving the old vector, or nulling the vector and
    /// thereby performing a forget — is exactly the "reported a control as
    /// applied" failure the F23-03 controls exist to avoid.
    /// `PartitionDispatcher` overrides this and does the re-embed.
    async fn correct_recalled(
        &self,
        tier: Tier,
        id: &str,
        corrected: &str,
        actor: &str,
        tok: AccessToken,
    ) -> Result<crate::provenance::CorrectionReceipt> {
        let controls = self.controls().ok_or_else(no_controls)?;
        match controls.correct_episode(&tok, tier, id, corrected, actor) {
            Err(crate::error::MemoryError::NotFound { .. }) => {
                Err(crate::error::MemoryError::InvalidControl(format!(
                    "no episode {id} at tier {}; correcting a semantic fact needs a re-embedding \
                     and this memory backend exposes no embedder",
                    tier.as_str()
                )))
            }
            other => other,
        }
    }

    /// 23B-C3 — the per-session nudge bound, when this backend has one.
    ///
    /// Returns `None` for backends with no nudge path. `NudgeBudget` shipped
    /// in F23-03 with a cap, an off switch and an atomic claim, and with no
    /// caller anywhere outside its own unit tests — so the criterion's
    /// "nudges" clause named a control no user could see or reach. This is the
    /// accessor `/memory nudge` reads.
    fn nudge_budget(&self) -> Option<std::sync::Arc<crate::provenance::NudgeBudget>> {
        None
    }

    /// 23B-C3 — the record of what memory was injected into the prompt on the
    /// user's behalf, and the switch that stops it being injected.
    ///
    /// Returns `None` for backends that never inject anything. See
    /// [`crate::activation`] for why this is a different question from
    /// provenance.
    fn activation_log(&self) -> Option<std::sync::Arc<crate::activation::ActivationLog>> {
        None
    }

    /// F23-03 — search, plus the provenance of everything it selected and
    /// everything it excluded.
    ///
    /// The default impl returns the ordinary hits with an EMPTY report, which
    /// reads as "this backend cannot tell you where these came from". It does
    /// not fabricate a provenance record, because a wrong answer to "why is
    /// this in my context window" is worse than no answer.
    async fn search_with_provenance(
        &self,
        q: Query,
        tok: AccessToken,
    ) -> Result<(Vec<Hit>, crate::provenance::RecallReport)> {
        Ok((
            self.search(q, tok).await?,
            crate::provenance::RecallReport::default(),
        ))
    }
}
