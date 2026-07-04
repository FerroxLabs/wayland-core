// P3 Semantic — subject/predicate/object triples with supersedes chain.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cdc::CdcWriter;
use crate::contradiction::{
    ContradictionCandidate, ContradictionResolution, ContradictionResolver,
};
use crate::db::Db;
use crate::embed::{Embedder, encode_blob};
use crate::error::{MemoryError, Result};
use crate::v2_types::{Fact, FactId, Tier};

/// Env var that gates the [`ContradictionResolver`] wiring in
/// [`SemanticPartition::assert`]. When unset, the legacy "any different
/// object supersedes" path runs unchanged.
///
/// See `.blackboard/v0.6.4-memory-depth-design.md` §4 (Task 6.6d) for the
/// roll-out plan. The default flips once dream-cycle CI is stable.
const CONTRADICTION_ENV: &str = "WAYLAND_CONTRADICTION";

pub struct SemanticPartition {
    pub(crate) db: Arc<Db>,
    pub(crate) embedder: Arc<dyn Embedder>,
    pub(crate) cdc: Arc<CdcWriter>,
}

impl SemanticPartition {
    pub fn new(db: Arc<Db>, embedder: Arc<dyn Embedder>, cdc: Arc<CdcWriter>) -> Self {
        Self { db, embedder, cdc }
    }

    /// Assert a fact. If a prior (subject, predicate) fact exists in the
    /// same tier with a different object, the prior fact's superseded_by
    /// is updated to point at the new one.
    ///
    /// When the `WAYLAND_CONTRADICTION` env var is set, the conflict is
    /// instead routed through [`ContradictionResolver::resolve`] and one
    /// of three outcomes is applied:
    /// - `Supersede` → existing marked superseded, new written at
    ///   `new_confidence`
    /// - `KeepExisting` → new fact discarded entirely, existing returned
    /// - `Coexist` → both rows present, new written at
    ///   `adjusted_confidence` (0.8× new), neither superseded
    pub async fn assert(&self, mut f: Fact) -> Result<FactId> {
        if f.ts == 0 {
            f.ts = now_secs();
        }
        let natural = format!("{} {} {}", f.subject, f.predicate, f.object);
        let embedding = self.embedder.embed(&natural).await?;
        let blob = encode_blob(&embedding);
        let tc = self.db.tier_or_global(f.tier);

        // Look for a prior fact in same tier with same subject+predicate
        // but different object. We also fetch confidence so the
        // resolver branch can compare without a second query.
        let prior: Option<(String, String, f64)> = {
            let conn = tc.conn.lock();
            let mut stmt = conn.prepare(
                "SELECT id, object, confidence FROM facts WHERE tier = ?1 AND subject = ?2 AND predicate = ?3 AND superseded_by IS NULL ORDER BY ts DESC LIMIT 1",
            )?;
            let r = stmt.query_row(
                rusqlite::params![f.tier.as_str(), f.subject, f.predicate],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            );
            match r {
                Ok(t) => Some(t),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(MemoryError::Db(e)),
            }
        };

        // Decide what to do with a different-object prior.
        //
        // * Default (env unset)            → Supersede (legacy behaviour).
        // * Env set + same object          → no contradiction, plain insert.
        // * Env set + different object     → consult resolver.
        // * No prior                       → plain insert.
        enum Action {
            Insert,                    // No prior, or prior with same object.
            Supersede(String),         // Mark `prior_id` as superseded.
            KeepExisting(String),      // Discard new; return existing `id`.
            Coexist { new_conf: f64 }, // Insert new at adjusted confidence.
        }

        let action = match prior.as_ref() {
            None => Action::Insert,
            Some((prior_id, prior_obj, prior_conf)) => {
                if prior_obj == &f.object {
                    Action::Insert
                } else if std::env::var(CONTRADICTION_ENV).is_ok() {
                    let verdict = ContradictionResolver::new().resolve(&ContradictionCandidate {
                        existing_relation_id: prior_id,
                        existing_fact: prior_obj,
                        existing_confidence: *prior_conf,
                        new_fact: &f.object,
                        new_confidence: f.confidence,
                    });
                    match verdict.resolution {
                        ContradictionResolution::Supersede => Action::Supersede(prior_id.clone()),
                        ContradictionResolution::KeepExisting => {
                            Action::KeepExisting(prior_id.clone())
                        }
                        ContradictionResolution::Coexist => Action::Coexist {
                            new_conf: verdict.adjusted_confidence,
                        },
                    }
                } else {
                    // Legacy path: any different-object prior is superseded.
                    // #664: this is the SHIPPING default (env unset), and it
                    // silently overwrites a conflicting fact with the crude
                    // supersede instead of consulting the nuanced resolver — a
                    // data-integrity effect on the default config. Log it so the
                    // overwrite is visible. (Flipping the default to the resolver
                    // is gated on dream-cycle CI stability per this module's
                    // docstring — a separate deliberate change, not this P2 pass.)
                    tracing::info!(
                        target: "wcore_memory::semantic",
                        subject = %f.subject,
                        predicate = %f.predicate,
                        "{CONTRADICTION_ENV} unset — superseding a conflicting fact via the \
                         legacy path (set {CONTRADICTION_ENV}=1 for the nuanced resolver)"
                    );
                    Action::Supersede(prior_id.clone())
                }
            }
        };

        // KeepExisting short-circuits before any write — no INSERT, no CDC.
        if let Action::KeepExisting(prior_id) = &action {
            let uuid = uuid::Uuid::parse_str(prior_id)
                .map_err(|_| MemoryError::Consolidation("non-uuid prior fact id".into()))?;
            return Ok(FactId(uuid));
        }

        // For Coexist, stamp the resolver-adjusted confidence onto the
        // outgoing Fact so CDC reflects what was actually persisted.
        if let Action::Coexist { new_conf } = &action {
            f.confidence = *new_conf;
        }

        // S2: INSERT new fact + optional UPDATE of prior's superseded_by
        // in a single transaction so a crash between the two writes
        // cannot leave a prior fact pointing at a non-existent new fact.
        let superseded_prior: Option<String> = {
            let conn = tc.conn.lock();
            let tx = conn.unchecked_transaction().map_err(MemoryError::Db)?;
            tx.execute(
                "INSERT INTO facts (id, tier, ts, subject, predicate, object, confidence, source_episode, superseded_by, embedding)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
                rusqlite::params![
                    f.id.0.to_string(),
                    f.tier.as_str(),
                    f.ts,
                    f.subject,
                    f.predicate,
                    f.object,
                    f.confidence,
                    f.source_episode.map(|e| e.0.to_string()),
                    blob,
                ],
            )?;
            let updated = if let Action::Supersede(prior_id) = &action {
                tx.execute(
                    "UPDATE facts SET superseded_by = ?1 WHERE id = ?2",
                    rusqlite::params![f.id.0.to_string(), prior_id],
                )?;
                Some(prior_id.clone())
            } else {
                None
            };
            tx.commit().map_err(MemoryError::Db)?;
            updated
        };

        // CDC writes happen after the transaction commits.
        self.cdc.append_fact(f.tier, &f)?;
        if let Some(prior_id) = superseded_prior {
            let old_uuid = uuid::Uuid::parse_str(&prior_id)
                .map_err(|_| MemoryError::Consolidation("non-uuid prior fact id".into()))?;
            self.cdc.append_fact_supersede(f.tier, &old_uuid, &f.id.0)?;
        }

        Ok(f.id)
    }

    pub async fn list_by_subject(&self, subject: &str, tier: Tier) -> Result<Vec<Fact>> {
        let tc = self.db.tier_or_global(tier);
        let conn = tc.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, tier, ts, subject, predicate, object, confidence, source_episode, superseded_by FROM facts WHERE subject = ?1",
        )?;
        let rows = stmt.query_map([subject], |r| {
            let id_s: String = r.get(0)?;
            let tier_s: String = r.get(1)?;
            let src_s: Option<String> = r.get(7)?;
            let sup_s: Option<String> = r.get(8)?;
            Ok(Fact {
                id: FactId(uuid::Uuid::parse_str(&id_s).unwrap_or_else(|_| uuid::Uuid::nil())),
                tier: tier_s.parse().unwrap_or(tier),
                ts: r.get(2)?,
                subject: r.get(3)?,
                predicate: r.get(4)?,
                object: r.get(5)?,
                confidence: r.get(6)?,
                source_episode: src_s
                    .and_then(|s| uuid::Uuid::parse_str(&s).ok())
                    .map(crate::v2_types::EpisodeId),
                superseded_by: sup_s
                    .and_then(|s| uuid::Uuid::parse_str(&s).ok())
                    .map(FactId),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(MemoryError::Db)?);
        }
        Ok(out)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
