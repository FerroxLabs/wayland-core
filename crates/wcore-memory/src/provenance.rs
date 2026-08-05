//! F23-03 — recall provenance and the operator controls over it.
//!
//! # Why this module exists
//!
//! The retriever in [`crate::retrieve`] already computes, for every item it
//! places in a prompt, which modality selected it, at what rank, and with what
//! fused score — and then discards all three. A user asking "why is this in my
//! context window?" has had no answer. This module keeps those facts and adds
//! the controls a user needs over them: correct, forget, privacy-scope and
//! retention-bound.
//!
//! # The two invariants that make this honest
//!
//! 1. **Provenance is produced by the retrieval it describes.**
//!    [`crate::retrieve::search_basic_with_provenance`] is the ONLY fusion
//!    path; [`crate::retrieve::search_basic`] delegates to it and drops the
//!    provenance. So a provenance record cannot describe a different selection
//!    than the one that happened — there is no second ranking to drift from.
//!
//! 2. **Every operation runs through the unmodified [`crate::gate`].**
//!    Nothing here widens the 5x3 grid. A correction to the user model (P5)
//!    needs a `SystemToken` exactly as the gate already requires, and a
//!    main-agent token is refused, audited, and told so.
//!
//! # What is NOT here
//!
//! Nudge *delivery* — scheduling a proactive message — is Phase 24's
//! persistent runtime. [`NudgeBudget`] implements only the bound F23-03 asks
//! for: a per-session cap and an off switch, both observable when they refuse.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEntry, AuditLog, now_secs};
use crate::cdc::CdcWriter;
use crate::db::Db;
use crate::error::{MemoryError, Result};
use crate::gate::MemoryAccessGate;
use crate::staleness::{StalenessVerdict, verdict_for_age};
use crate::v2_types::{AccessToken, Hit, Partition, Tier};

// ---------------------------------------------------------------------------
// Provenance records
// ---------------------------------------------------------------------------

/// Which retrieval pass selected an item. `Fused` means more than one did —
/// the case reciprocal-rank fusion exists to reward, and the one a user is
/// most likely to ask about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecallModality {
    /// FTS5 BM25 over `episodes_fts`.
    Lexical,
    /// Embedding nearest-neighbour (sqlite-vec KNN, or the legacy cosine
    /// fallback for rows written before the per-dim index existed).
    Vector,
    /// Depth-bounded BFS over the knowledge graph, projected onto episodes.
    Graph,
}

impl RecallModality {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Vector => "vector",
            Self::Graph => "graph",
        }
    }
}

/// One item's provenance, captured at the point of fusion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallProvenance {
    pub id: String,
    pub partition: Partition,
    pub tier: Tier,
    /// Every modality that contributed to this item's fused score, with the
    /// rank it held inside that modality's own result list.
    pub contributions: Vec<ModalityContribution>,
    /// Rank in the FUSED list, zero-based. This is the rank that decided
    /// whether the item reached the prompt at all.
    pub rank: usize,
    /// The reciprocal-rank-fusion score the ordering was taken from.
    pub fused_score: f64,
    /// Seconds between the item's recorded timestamp and the moment of
    /// recall. Negative ages are clamped to zero rather than reported, since
    /// a future timestamp is a clock fault, not an age.
    pub age_secs: i64,
    pub staleness: StalenessVerdict,
}

impl RecallProvenance {
    /// `fused` when more than one modality contributed, otherwise the single
    /// contributing modality's name. Rendered by `/memory provenance`.
    #[must_use]
    pub fn modality_label(&self) -> String {
        match self.contributions.len() {
            0 => "none".to_owned(),
            1 => self.contributions[0].modality.as_str().to_owned(),
            _ => "fused".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModalityContribution {
    pub modality: RecallModality,
    /// Zero-based rank WITHIN that modality's own result list.
    pub rank: usize,
}

/// Why an item that would otherwise have been recalled was not.
///
/// Exclusions are reported, never silent: a user who scoped a partition out
/// must be able to see that the scope did something, and an expired item must
/// be distinguishable from an item that never existed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cause", rename_all = "snake_case")]
pub enum ExclusionCause {
    PrivacyScope { reason: String },
    RetentionExpired { max_age_secs: i64, age_secs: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallExclusion {
    pub partition: Partition,
    pub tier: Tier,
    /// Absent for a whole-cell privacy exclusion, which is decided before any
    /// row is read; present for a per-item retention expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub cause: ExclusionCause,
}

/// The complete answer to "what is in my context window and why".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecallReport {
    pub provenance: Vec<RecallProvenance>,
    pub exclusions: Vec<RecallExclusion>,
}

// ---------------------------------------------------------------------------
// Receipts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrectionReceipt {
    pub id: String,
    pub partition: Partition,
    pub tier: Tier,
    pub actor: String,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForgetReceipt {
    pub id: String,
    pub partition: Partition,
    pub tier: Tier,
    pub actor: String,
    pub at: i64,
    /// True once the deletion is represented in the change-data-capture
    /// changelog, so a downstream consumer sees a deletion rather than a row
    /// that quietly vanished.
    pub in_changelog: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyScope {
    pub partition: Partition,
    pub tier: Tier,
    pub reason: String,
    pub excluded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionBound {
    pub partition: Partition,
    pub tier: Tier,
    pub max_age_secs: i64,
    pub set_at: i64,
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// Operator control surface over recall.
///
/// Holds the same [`Db`], [`MemoryAccessGate`] and [`CdcWriter`] the rest of
/// the memory system uses. It owns no store of its own and reimplements no
/// retrieval.
pub struct MemoryControls {
    db: Arc<Db>,
    gate: Arc<MemoryAccessGate>,
    cdc: CdcWriter,
}

impl MemoryControls {
    #[must_use]
    pub fn new(db: Arc<Db>, gate: Arc<MemoryAccessGate>, cdc: CdcWriter) -> Self {
        Self { db, gate, cdc }
    }

    fn audit(&self) -> Arc<AuditLog> {
        self.gate.audit()
    }

    /// Record an operator-initiated mutation in the audit log. The gate
    /// already writes its own allow/deny row; this row names WHAT was done
    /// and by WHOM, which a gate decision alone does not carry.
    fn record_operation(&self, p: Partition, t: Tier, op: &str, actor: &str, detail: &str) {
        let entry = AuditEntry {
            ts: now_secs(),
            token_kind: "operator".to_owned(),
            agent_name: Some(actor.to_owned()),
            partition: p,
            tier: t,
            op: op.to_owned(),
            decision: "allow".to_owned(),
            reason: detail.to_owned(),
        };
        // An audit write failure must not silently swallow the operation's
        // own result; it is logged and the operation still reports honestly.
        if let Err(e) = self.audit().record(entry) {
            tracing::warn!(
                target: "wcore_memory::provenance",
                "audit write failed for {op} on {}/{}: {e}",
                p.as_str(),
                t.as_str()
            );
        }
    }

    // ----- correction -------------------------------------------------------

    /// Replace a recalled episode's summary. The item keeps its identity, so
    /// a later recall of the same id shows the corrected text rather than a
    /// second, competing copy.
    pub fn correct_episode(
        &self,
        tok: &AccessToken,
        tier: Tier,
        id: &str,
        corrected_summary: &str,
        actor: &str,
    ) -> Result<CorrectionReceipt> {
        self.gate.check_write(tok, Partition::Episodic, tier)?;
        let tc = self.db.tier_or_global(tier);
        let changed = {
            let conn = tc.conn.lock();
            conn.execute(
                "UPDATE episodes SET summary = ?1 WHERE id = ?2 AND tier = ?3",
                rusqlite::params![corrected_summary, id, tier.as_str()],
            )
            .map_err(MemoryError::Db)?
        };
        if changed == 0 {
            return Err(MemoryError::NotFound {
                partition: Partition::Episodic.to_string(),
                tier: tier.to_string(),
                id: id.to_owned(),
            });
        }
        self.record_operation(Partition::Episodic, tier, "correct", actor, id);
        self.cdc
            .append_correction(tier, Partition::Episodic, id, actor)?;
        Ok(CorrectionReceipt {
            id: id.to_owned(),
            partition: Partition::Episodic,
            tier,
            actor: actor.to_owned(),
            at: now_secs(),
        })
    }

    // ----- forgetting -------------------------------------------------------

    /// Remove an episode so no later retrieval can surface it, and represent
    /// the removal in the change-data-capture changelog.
    ///
    /// The row is DELETEd rather than status-flagged: a status flag is a
    /// filter every future query has to remember to apply, and one forgotten
    /// filter re-exposes the content. The FTS5 delete trigger in schema v1
    /// keeps the lexical index consistent with the delete.
    pub fn forget_episode(
        &self,
        tok: &AccessToken,
        tier: Tier,
        id: &str,
        actor: &str,
    ) -> Result<ForgetReceipt> {
        self.gate.check_write(tok, Partition::Episodic, tier)?;
        let tc = self.db.tier_or_global(tier);
        let removed = {
            let conn = tc.conn.lock();
            conn.execute(
                "DELETE FROM episodes WHERE id = ?1 AND tier = ?2",
                rusqlite::params![id, tier.as_str()],
            )
            .map_err(MemoryError::Db)?
        };
        if removed == 0 {
            return Err(MemoryError::NotFound {
                partition: Partition::Episodic.to_string(),
                tier: tier.to_string(),
                id: id.to_owned(),
            });
        }
        self.record_operation(Partition::Episodic, tier, "forget", actor, id);
        self.cdc
            .append_forget(tier, Partition::Episodic, id, actor)?;
        Ok(ForgetReceipt {
            id: id.to_owned(),
            partition: Partition::Episodic,
            tier,
            actor: actor.to_owned(),
            at: now_secs(),
            in_changelog: true,
        })
    }

    // ----- semantic facts (23B-C3) -----------------------------------------

    /// Remove a semantic fact so no later retrieval can surface it, and
    /// represent the removal in the changelog.
    ///
    /// This exists because [`Self::forget_episode`] hardcodes
    /// `Partition::Episodic` while the semantic partition is the only one the
    /// engine auto-injects into the outbound provider request body
    /// (`AgentEngine::recall_relevant_facts` keeps `Partition::Semantic` hits
    /// and discards every other partition). Without this, the single class of
    /// remembered content a user can actually see in their prompt was the one
    /// class `/memory forget` could not remove: the `DELETE` matched zero rows
    /// in `episodes` and the control returned `NotFound` while the fact stayed
    /// in every subsequent prompt.
    ///
    /// The row is DELETEd rather than superseded. `superseded_by` is a filter
    /// every future query has to remember to apply — `facts_cosine_pass` does
    /// apply it, but a forget must not depend on that discipline holding in a
    /// query nobody has written yet.
    pub fn forget_fact(
        &self,
        tok: &AccessToken,
        tier: Tier,
        id: &str,
        actor: &str,
    ) -> Result<ForgetReceipt> {
        self.gate.check_write(tok, Partition::Semantic, tier)?;
        let tc = self.db.tier_or_global(tier);
        let removed = {
            let conn = tc.conn.lock();
            conn.execute(
                "DELETE FROM facts WHERE id = ?1 AND tier = ?2",
                rusqlite::params![id, tier.as_str()],
            )
            .map_err(MemoryError::Db)?
        };
        if removed == 0 {
            return Err(MemoryError::NotFound {
                partition: Partition::Semantic.to_string(),
                tier: tier.to_string(),
                id: id.to_owned(),
            });
        }
        self.record_operation(Partition::Semantic, tier, "forget", actor, id);
        self.cdc
            .append_forget(tier, Partition::Semantic, id, actor)?;
        Ok(ForgetReceipt {
            id: id.to_owned(),
            partition: Partition::Semantic,
            tier,
            actor: actor.to_owned(),
            at: now_secs(),
            in_changelog: true,
        })
    }

    /// Replace a semantic fact's `object` — the part of the triple that
    /// carries the claim — and REWRITE ITS EMBEDDING in the same statement.
    ///
    /// The embedding is a required argument rather than something this method
    /// could omit, and that is the whole design. `facts_cosine_pass` ranks on
    /// `embedding` and skips rows where it is NULL, so the two lazy
    /// alternatives are both silent lies: leaving the old vector in place
    /// means a corrected fact keeps being recalled by the query that matched
    /// the *wrong* text, and nulling it means "correct" silently performs a
    /// forget. `MemoryControls` owns no embedder, so the caller that has one
    /// must supply it; [`crate::api::MemoryApi::correct_recalled`] is the
    /// production path and the dispatcher's override is where the re-embed
    /// happens.
    pub fn correct_fact(
        &self,
        tok: &AccessToken,
        tier: Tier,
        id: &str,
        corrected_object: &str,
        embedding: &[f32],
        actor: &str,
    ) -> Result<CorrectionReceipt> {
        self.gate.check_write(tok, Partition::Semantic, tier)?;
        let blob = crate::embed::encode_blob(embedding);
        let tc = self.db.tier_or_global(tier);
        let changed = {
            let conn = tc.conn.lock();
            conn.execute(
                "UPDATE facts SET object = ?1, embedding = ?2 WHERE id = ?3 AND tier = ?4",
                rusqlite::params![corrected_object, blob, id, tier.as_str()],
            )
            .map_err(MemoryError::Db)?
        };
        if changed == 0 {
            return Err(MemoryError::NotFound {
                partition: Partition::Semantic.to_string(),
                tier: tier.to_string(),
                id: id.to_owned(),
            });
        }
        self.record_operation(Partition::Semantic, tier, "correct", actor, id);
        self.cdc
            .append_correction(tier, Partition::Semantic, id, actor)?;
        Ok(CorrectionReceipt {
            id: id.to_owned(),
            partition: Partition::Semantic,
            tier,
            actor: actor.to_owned(),
            at: now_secs(),
        })
    }

    /// The triple text a corrected fact will be re-embedded from, so a caller
    /// embeds exactly the string `facts_cosine_pass` will later render as the
    /// hit preview. Returns `NotFound` rather than a default, because
    /// embedding a guessed triple would put a wrong vector on a real row.
    pub fn fact_triple_after_correction(
        &self,
        tier: Tier,
        id: &str,
        corrected_object: &str,
    ) -> Result<String> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT subject, predicate FROM facts WHERE id = ?1 AND tier = ?2",
                rusqlite::params![id, tier.as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(MemoryError::Db)?;
        match row {
            Some((subject, predicate)) => Ok(format!("{subject} {predicate} {corrected_object}")),
            None => Err(MemoryError::NotFound {
                partition: Partition::Semantic.to_string(),
                tier: tier.to_string(),
                id: id.to_owned(),
            }),
        }
    }

    // ----- privacy ----------------------------------------------------------

    /// Exclude a grid cell from retrieval. Idempotent: re-scoping an already
    /// scoped cell replaces the reason rather than erroring, so an operator
    /// can refine the reason without first clearing.
    pub fn set_privacy_scope(
        &self,
        tok: &AccessToken,
        partition: Partition,
        tier: Tier,
        reason: &str,
        actor: &str,
    ) -> Result<PrivacyScope> {
        self.gate.check_write(tok, partition, tier)?;
        let at = now_secs();
        let tc = self.db.tier_or_global(tier);
        {
            let conn = tc.conn.lock();
            conn.execute(
                "INSERT INTO memory_privacy_scope (partition, tier, excluded_at, reason)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (partition, tier) DO UPDATE SET
                     excluded_at = excluded.excluded_at, reason = excluded.reason",
                rusqlite::params![partition.as_str(), tier.as_str(), at, reason],
            )
            .map_err(MemoryError::Db)?;
        }
        self.record_operation(partition, tier, "privacy_scope", actor, reason);
        Ok(PrivacyScope {
            partition,
            tier,
            reason: reason.to_owned(),
            excluded_at: at,
        })
    }

    pub fn clear_privacy_scope(
        &self,
        tok: &AccessToken,
        partition: Partition,
        tier: Tier,
        actor: &str,
    ) -> Result<bool> {
        self.gate.check_write(tok, partition, tier)?;
        let tc = self.db.tier_or_global(tier);
        let removed = {
            let conn = tc.conn.lock();
            conn.execute(
                "DELETE FROM memory_privacy_scope WHERE partition = ?1 AND tier = ?2",
                rusqlite::params![partition.as_str(), tier.as_str()],
            )
            .map_err(MemoryError::Db)?
        };
        self.record_operation(partition, tier, "privacy_scope_clear", actor, "");
        Ok(removed > 0)
    }

    /// Read the scope for one cell. Returns `None` when the cell is not
    /// excluded.
    pub fn privacy_scope(&self, partition: Partition, tier: Tier) -> Result<Option<PrivacyScope>> {
        read_privacy_scope(&self.db, partition, tier)
    }

    // ----- retention --------------------------------------------------------

    /// Bound how old an item in a cell may be before it is reported as
    /// expired. Expiry excludes from retrieval; it does not delete.
    pub fn set_retention(
        &self,
        tok: &AccessToken,
        partition: Partition,
        tier: Tier,
        max_age_secs: i64,
        actor: &str,
    ) -> Result<RetentionBound> {
        self.gate.check_write(tok, partition, tier)?;
        if max_age_secs < 0 {
            return Err(MemoryError::InvalidControl(
                "retention max age must not be negative".to_owned(),
            ));
        }
        let at = now_secs();
        let tc = self.db.tier_or_global(tier);
        {
            let conn = tc.conn.lock();
            conn.execute(
                "INSERT INTO memory_retention (partition, tier, max_age_secs, set_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (partition, tier) DO UPDATE SET
                     max_age_secs = excluded.max_age_secs, set_at = excluded.set_at",
                rusqlite::params![partition.as_str(), tier.as_str(), max_age_secs, at],
            )
            .map_err(MemoryError::Db)?;
        }
        self.record_operation(
            partition,
            tier,
            "retention",
            actor,
            &max_age_secs.to_string(),
        );
        Ok(RetentionBound {
            partition,
            tier,
            max_age_secs,
            set_at: at,
        })
    }

    pub fn retention(&self, partition: Partition, tier: Tier) -> Result<Option<RetentionBound>> {
        read_retention(&self.db, partition, tier)
    }
}

// ---------------------------------------------------------------------------
// Free readers — used by the retrieval path, which holds no MemoryControls
// ---------------------------------------------------------------------------

pub fn read_privacy_scope(
    db: &Db,
    partition: Partition,
    tier: Tier,
) -> Result<Option<PrivacyScope>> {
    let tc = db.tier_or_global(tier);
    let conn = tc.conn.lock();
    let row = conn
        .query_row(
            "SELECT excluded_at, reason FROM memory_privacy_scope
             WHERE partition = ?1 AND tier = ?2",
            rusqlite::params![partition.as_str(), tier.as_str()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(MemoryError::Db)?;
    Ok(row.map(|(excluded_at, reason)| PrivacyScope {
        partition,
        tier,
        reason,
        excluded_at,
    }))
}

pub fn read_retention(db: &Db, partition: Partition, tier: Tier) -> Result<Option<RetentionBound>> {
    let tc = db.tier_or_global(tier);
    let conn = tc.conn.lock();
    let row = conn
        .query_row(
            "SELECT max_age_secs, set_at FROM memory_retention
             WHERE partition = ?1 AND tier = ?2",
            rusqlite::params![partition.as_str(), tier.as_str()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(MemoryError::Db)?;
    Ok(row.map(|(max_age_secs, set_at)| RetentionBound {
        partition,
        tier,
        max_age_secs,
        set_at,
    }))
}

/// Age of an item, clamped at zero. A timestamp in the future is a clock
/// fault; reporting a negative age would let it masquerade as extra freshness.
#[must_use]
pub fn age_secs_at(now: i64, ts: i64) -> i64 {
    (now - ts).max(0)
}

/// The inputs that are the same for every item in one recall, separated from
/// the per-item ones so the record builder takes a signature a reader can hold
/// in their head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecallContext {
    /// The instant the recall happened. Passed in rather than read here so
    /// every item in one recall is aged against the SAME clock reading — two
    /// items in one prompt reporting ages a second apart would be a lie about
    /// which is older.
    pub now: i64,
    /// The operator's retention bound for the cell, if one is set.
    pub max_age_secs: Option<i64>,
}

/// Build the provenance record for one fused hit.
#[must_use]
pub fn provenance_for(
    hit: &Hit,
    contributions: Vec<ModalityContribution>,
    rank: usize,
    ts: i64,
    ctx: RecallContext,
) -> RecallProvenance {
    let age_secs = age_secs_at(ctx.now, ts);
    RecallProvenance {
        id: hit.id.clone(),
        partition: hit.partition,
        tier: hit.tier,
        contributions,
        rank,
        fused_score: hit.score,
        age_secs,
        staleness: verdict_for_age(age_secs, ctx.max_age_secs),
    }
}

// ---------------------------------------------------------------------------
// Nudge bound
// ---------------------------------------------------------------------------

/// The bound F23-03 requires on proactive nudges: an explicit per-session cap
/// and an off switch.
///
/// An unbounded nudge path is a background actor that spends tokens on a turn
/// the user did not ask for, so the bound is enforced here rather than left to
/// a caller's discipline. Delivery scheduling is deliberately NOT implemented:
/// that is Phase 24's persistent runtime, and shipping half of it here would
/// mean shipping an unbounded actor with a bound bolted on.
/// 23B-C3: `cap` and `enabled` are atomics rather than plain fields so the
/// bound is a control a user can **change** at runtime (`/memory nudge off`,
/// `/memory nudge cap N`) and not merely a constant they can read. A bound
/// nobody can move is a configuration decision, not a control, and the
/// criterion asks for control.
#[derive(Debug)]
pub struct NudgeBudget {
    cap: AtomicU32,
    enabled: AtomicBool,
    used: AtomicU32,
}

/// Why a nudge request was refused. Both variants are observable by the
/// caller, so the bound can be proved by driving past it rather than by
/// reading the constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeRefusal {
    /// The off switch is set.
    Disabled,
    /// The per-session cap has been reached.
    CapReached { cap: u32 },
}

impl std::fmt::Display for NudgeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "proactive nudges are switched off"),
            Self::CapReached { cap } => {
                write!(f, "per-session nudge cap of {cap} already reached")
            }
        }
    }
}

/// The default per-session cap when nothing sets one explicitly. Chosen, not
/// measured: three is small enough that an unnoticed nudge path cannot spend a
/// session's budget, and the user can raise it.
pub const DEFAULT_NUDGE_CAP: u32 = 3;

impl NudgeBudget {
    #[must_use]
    pub fn new(cap: u32, enabled: bool) -> Self {
        Self {
            cap: AtomicU32::new(cap),
            enabled: AtomicBool::new(enabled),
            used: AtomicU32::new(0),
        }
    }

    #[must_use]
    pub fn cap(&self) -> u32 {
        self.cap.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Turn the nudge path on or off for the rest of the session. Returns the
    /// previous state so a caller can report what actually changed rather
    /// than echoing the request back.
    pub fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.swap(enabled, Ordering::SeqCst)
    }

    /// Move the per-session cap. Returns the previous cap.
    ///
    /// Lowering the cap below `used` does NOT retroactively refuse claims that
    /// already succeeded — `remaining()` saturates at zero and every
    /// subsequent `request()` is refused. A control that pretended to unspend
    /// an already-spent nudge would be lying about the past.
    pub fn set_cap(&self, cap: u32) -> u32 {
        self.cap.swap(cap, Ordering::SeqCst)
    }

    #[must_use]
    pub fn used(&self) -> u32 {
        self.used.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.cap().saturating_sub(self.used())
    }

    /// Claim one nudge. The claim is atomic against concurrent callers: the
    /// compare-and-swap loop means two threads racing at `cap - 1` cannot both
    /// succeed, which a read-then-increment would allow.
    pub fn request(&self) -> std::result::Result<u32, NudgeRefusal> {
        if !self.enabled() {
            return Err(NudgeRefusal::Disabled);
        }
        let mut current = self.used.load(Ordering::SeqCst);
        loop {
            // Re-read the cap on every loop turn: `set_cap` may land between
            // the load and the CAS, and honouring the newest bound is the
            // point of making it settable.
            let cap = self.cap();
            if current >= cap {
                return Err(NudgeRefusal::CapReached { cap });
            }
            match self.used.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(current + 1),
                Err(observed) => current = observed,
            }
        }
    }

    /// Reset for a new session. Nudge budget is per-session by definition.
    pub fn reset(&self) {
        self.used.store(0, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::AccessPolicy;
    use crate::v2_types::{Episode, EpisodeId, EpisodeStatus};

    fn controls() -> (MemoryControls, Arc<Db>) {
        let db = Arc::new(Db::open_memory().unwrap());
        let audit = Arc::new(AuditLog::open_memory().unwrap());
        let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
        let cdc = CdcWriter::new_stub();
        (MemoryControls::new(db.clone(), gate, cdc), db)
    }

    fn seed_episode(db: &Db, tier: Tier, id: &str, summary: &str, ts: i64) {
        let tc = db.tier_or_global(tier);
        let conn = tc.conn.lock();
        conn.execute(
            "INSERT INTO episodes (id, tier, ts, episode_type, summary, atomic_facts, source, \
             source_product, session_id, project_root, decay_score, status) \
             VALUES (?1, ?2, ?3, 'note', ?4, '[]', 'test', 'test', NULL, NULL, 1.0, 'active')",
            rusqlite::params![id, tier.as_str(), ts, summary],
        )
        .unwrap();
    }

    fn summary_of(db: &Db, tier: Tier, id: &str) -> Option<String> {
        let tc = db.tier_or_global(tier);
        let conn = tc.conn.lock();
        conn.query_row(
            "SELECT summary FROM episodes WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .unwrap()
    }

    #[test]
    fn correction_updates_the_item_and_is_audited() {
        let (ctl, db) = controls();
        seed_episode(
            &db,
            Tier::Project,
            "ep-1",
            "the aardvark is blue",
            now_secs(),
        );
        let before = ctl.audit().count().unwrap();
        let receipt = ctl
            .correct_episode(
                &AccessToken::MainAgent,
                Tier::Project,
                "ep-1",
                "the aardvark is brown",
                "operator",
            )
            .unwrap();
        assert_eq!(receipt.id, "ep-1");
        assert_eq!(
            summary_of(&db, Tier::Project, "ep-1").as_deref(),
            Some("the aardvark is brown")
        );
        assert!(
            ctl.audit().count().unwrap() > before,
            "the correction must leave an audit trail"
        );
    }

    #[test]
    fn correcting_an_absent_item_is_not_found_not_a_silent_success() {
        let (ctl, _db) = controls();
        let err = ctl
            .correct_episode(
                &AccessToken::MainAgent,
                Tier::Project,
                "absent",
                "x",
                "operator",
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::NotFound { .. }), "got {err:?}");
    }

    #[test]
    fn forget_removes_the_item_and_reaches_the_changelog() {
        let (ctl, db) = controls();
        seed_episode(&db, Tier::Project, "ep-2", "forget me", now_secs());
        let receipt = ctl
            .forget_episode(&AccessToken::MainAgent, Tier::Project, "ep-2", "operator")
            .unwrap();
        assert!(receipt.in_changelog);
        assert_eq!(summary_of(&db, Tier::Project, "ep-2"), None);
    }

    #[test]
    fn forget_is_represented_in_the_cdc_changelog() {
        let db = Arc::new(Db::open_memory().unwrap());
        let audit = Arc::new(AuditLog::open_memory().unwrap());
        let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
        let cdc = CdcWriter::new_stub();
        let ctl = MemoryControls::new(db.clone(), gate, cdc.clone());
        seed_episode(&db, Tier::Project, "ep-3", "forget me too", now_secs());
        ctl.forget_episode(&AccessToken::MainAgent, Tier::Project, "ep-3", "operator")
            .unwrap();
        let entries = cdc.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.op == "forget" && e.target_id.as_deref() == Some("ep-3")),
            "a downstream consumer must see a deletion, not a vanished row: {entries:?}"
        );
    }

    #[test]
    fn core_partition_write_is_refused_for_a_main_agent_token() {
        // P5 is system-only. The control surface must not widen that.
        let (ctl, _db) = controls();
        let err = ctl
            .set_privacy_scope(
                &AccessToken::MainAgent,
                Partition::Core,
                Tier::Global,
                "user asked",
                "operator",
            )
            .unwrap_err();
        assert!(
            matches!(err, MemoryError::AccessDenied { .. }),
            "P5 must stay system-only, got {err:?}"
        );
    }

    #[test]
    fn refusals_are_audited() {
        let (ctl, _db) = controls();
        let before = ctl.audit().count_denials().unwrap();
        let _ = ctl.set_privacy_scope(
            &AccessToken::MainAgent,
            Partition::Core,
            Tier::Global,
            "x",
            "operator",
        );
        assert!(
            ctl.audit().count_denials().unwrap() > before,
            "a refused control operation must be recorded as a denial"
        );
    }

    #[test]
    fn sub_agent_without_scope_is_denied_every_control() {
        let (ctl, db) = controls();
        seed_episode(&db, Tier::Project, "ep-4", "x", now_secs());
        let tok = AccessToken::SubAgent {
            agent_name: "unscoped".into(),
        };
        assert!(matches!(
            ctl.forget_episode(&tok, Tier::Project, "ep-4", "operator"),
            Err(MemoryError::AccessDenied { .. })
        ));
        assert!(matches!(
            ctl.correct_episode(&tok, Tier::Project, "ep-4", "y", "operator"),
            Err(MemoryError::AccessDenied { .. })
        ));
        // and the item survived the refusal
        assert!(summary_of(&db, Tier::Project, "ep-4").is_some());
    }

    #[test]
    fn privacy_scope_round_trips_and_clears() {
        let (ctl, _db) = controls();
        assert!(
            ctl.privacy_scope(Partition::Episodic, Tier::Project)
                .unwrap()
                .is_none()
        );
        ctl.set_privacy_scope(
            &AccessToken::MainAgent,
            Partition::Episodic,
            Tier::Project,
            "medical notes",
            "operator",
        )
        .unwrap();
        let scope = ctl
            .privacy_scope(Partition::Episodic, Tier::Project)
            .unwrap()
            .expect("scope must be readable back");
        assert_eq!(scope.reason, "medical notes");
        assert!(
            ctl.clear_privacy_scope(
                &AccessToken::MainAgent,
                Partition::Episodic,
                Tier::Project,
                "operator"
            )
            .unwrap()
        );
        assert!(
            ctl.privacy_scope(Partition::Episodic, Tier::Project)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn retention_rejects_a_negative_bound() {
        let (ctl, _db) = controls();
        let err = ctl
            .set_retention(
                &AccessToken::MainAgent,
                Partition::Episodic,
                Tier::Project,
                -1,
                "operator",
            )
            .unwrap_err();
        assert!(matches!(err, MemoryError::InvalidControl(_)), "got {err:?}");
    }

    #[test]
    fn retention_round_trips() {
        let (ctl, _db) = controls();
        ctl.set_retention(
            &AccessToken::MainAgent,
            Partition::Episodic,
            Tier::Project,
            86_400,
            "operator",
        )
        .unwrap();
        let bound = ctl
            .retention(Partition::Episodic, Tier::Project)
            .unwrap()
            .unwrap();
        assert_eq!(bound.max_age_secs, 86_400);
    }

    #[test]
    fn age_is_clamped_at_zero_for_a_future_timestamp() {
        assert_eq!(age_secs_at(100, 500), 0);
        assert_eq!(age_secs_at(500, 100), 400);
    }

    #[test]
    fn provenance_labels_a_single_modality_and_a_fusion_differently() {
        let hit = |id: &str, score: f64| Hit {
            partition: Partition::Episodic,
            tier: Tier::Project,
            id: id.to_owned(),
            score,
            session_id: None,
            preview: String::new(),
        };
        let ctx = RecallContext {
            now: 1_000,
            max_age_secs: None,
        };

        let single = provenance_for(
            &hit("a", 0.016),
            vec![ModalityContribution {
                modality: RecallModality::Lexical,
                rank: 0,
            }],
            0,
            900,
            ctx,
        );
        assert_eq!(single.modality_label(), "lexical");
        assert_eq!(single.age_secs, 100);
        assert!((single.fused_score - 0.016).abs() < f64::EPSILON);

        let fused = provenance_for(
            &hit("b", 0.032),
            vec![
                ModalityContribution {
                    modality: RecallModality::Lexical,
                    rank: 0,
                },
                ModalityContribution {
                    modality: RecallModality::Vector,
                    rank: 2,
                },
            ],
            1,
            900,
            ctx,
        );
        assert_eq!(fused.modality_label(), "fused");
        assert_eq!(fused.contributions.len(), 2);
    }

    #[test]
    fn nudge_cap_refuses_past_its_limit_and_says_why() {
        let budget = NudgeBudget::new(2, true);
        assert_eq!(budget.request().unwrap(), 1);
        assert_eq!(budget.request().unwrap(), 2);
        let refusal = budget.request().unwrap_err();
        assert_eq!(refusal, NudgeRefusal::CapReached { cap: 2 });
        assert!(refusal.to_string().contains('2'));
        assert_eq!(budget.remaining(), 0);
        budget.reset();
        assert_eq!(budget.request().unwrap(), 1);
    }

    #[test]
    fn nudge_off_switch_refuses_the_first_request() {
        let budget = NudgeBudget::new(100, false);
        assert_eq!(budget.request().unwrap_err(), NudgeRefusal::Disabled);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn nudge_cap_holds_against_concurrent_claimants() {
        // A read-then-increment would let two threads both pass at cap-1.
        let budget = Arc::new(NudgeBudget::new(50, true));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let b = budget.clone();
            handles.push(std::thread::spawn(move || {
                let mut granted = 0;
                for _ in 0..20 {
                    if b.request().is_ok() {
                        granted += 1;
                    }
                }
                granted
            }));
        }
        let total: u32 = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, 50, "exactly cap grants, never more");
        assert_eq!(budget.used(), 50);
    }

    #[test]
    fn episode_type_import_is_exercised() {
        // Keeps the v2_types import honest: the seed helper writes the same
        // row shape `Episode` describes.
        let ep = Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: now_secs(),
            episode_type: "note".into(),
            summary: "x".into(),
            atomic_facts: vec![],
            source: "test".into(),
            source_product: "test".into(),
            session_id: None,
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        };
        assert_eq!(ep.tier, Tier::Project);
    }
}
