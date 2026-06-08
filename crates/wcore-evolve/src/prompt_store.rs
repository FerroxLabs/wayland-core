//! M4.4 — PromptStore.
//!
//! Persists evolved skill variants (the winners the GEPA loop emits each
//! run) into the `evolved_prompts` table on the wcore-memory global tier.
//! The point is to give the learning loop *memory across runs*: each
//! `wcore-evolve-bench` invocation can read past winners, bootstrap the
//! seed pool, and observe convergence per skill over time.
//!
//! Schema lives in `wcore-memory/src/schema/v2_evolved_prompts.sql`
//! (migration v2). PromptStore reuses the Db handle directly — no
//! parallel sqlite connection — and writes to the global tier only,
//! since the evolved_prompts table is operational memory for the
//! learning loop and is naturally cross-run / cross-project.

use std::sync::Arc;

use rusqlite::params;
use wcore_memory::db::Db;
use wcore_memory::error::MemoryError;

use crate::error::EvolveError;
use crate::evolve::EvolveOutcome;

/// One row in the `evolved_prompts` table.
#[derive(Debug, Clone, PartialEq)]
pub struct EvolvedPrompt {
    /// UUID v4 — generate via `uuid::Uuid::new_v4()` at the call site.
    pub id: String,
    pub skill_name: String,
    /// Optional pointer to the prior winner that seeded this variant.
    pub parent_id: Option<String>,
    pub prompt_body: String,
    /// `pass_ratio` for BenchScorer, `dimensions.combined` for DefaultScorer.
    pub score: f64,
    /// Stable scorer identifier — currently `"bench"` or `"default"`.
    pub scorer: String,
    /// Zero-based generation index at which this variant was emitted.
    pub generation: u32,
    /// Unix seconds (UTC) when the row was recorded.
    pub created_at: i64,
    /// Optional JSON blob for arbitrary extras (termination reason,
    /// mutator kind, etc.). Stored as TEXT in sqlite.
    pub metadata: Option<String>,
}

/// Persists evolved variants into the wcore-memory `evolved_prompts` table.
///
/// Cheap to clone (Arc inside). Construct once per binary run and pass
/// references where needed.
#[derive(Clone)]
pub struct PromptStore {
    db: Arc<Db>,
}

impl PromptStore {
    /// Construct against an existing wcore-memory `Db`. Callers are
    /// expected to have already run migrations (any `Memory::open` path
    /// does this on first use).
    pub fn new(db: Arc<Db>) -> Self {
        Self { db }
    }

    /// Insert one evolved variant. Returns an error if the row already
    /// exists (`UNIQUE (skill_name, generation, id)` violation), so call
    /// sites should generate fresh `id`s rather than retrying with the
    /// same uuid.
    pub fn record_variant(&self, v: &EvolvedPrompt) -> Result<(), EvolveError> {
        let tc = self.db.global.clone();
        let conn = tc.conn.lock();
        conn.execute(
            "INSERT INTO evolved_prompts \
             (id, skill_name, parent_id, prompt_body, score, scorer, generation, created_at, metadata) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                v.id,
                v.skill_name,
                v.parent_id,
                v.prompt_body,
                v.score,
                v.scorer,
                v.generation,
                v.created_at,
                v.metadata,
            ],
        )
        .map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?;
        Ok(())
    }

    /// Convenience: record the winning variant from an `EvolveOutcome`.
    ///
    /// No-op when the run produced no winner (`best_candidate == None`).
    /// Returns the generated row id when a row was written, `None`
    /// otherwise.
    pub fn record_outcome(
        &self,
        skill_name: &str,
        scorer: &str,
        outcome: &EvolveOutcome,
    ) -> Result<Option<String>, EvolveError> {
        let Some(winner) = outcome.best_candidate.as_ref() else {
            return Ok(None);
        };
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp();
        let row = EvolvedPrompt {
            id: id.clone(),
            skill_name: skill_name.to_string(),
            parent_id: None, // M5+ will thread parent chains
            prompt_body: winner.mutation.body.clone(),
            score: winner.score.dimensions.combined,
            scorer: scorer.to_string(),
            generation: outcome.generations_run.saturating_sub(1),
            created_at,
            metadata: None,
        };
        self.record_variant(&row)?;
        Ok(Some(id))
    }

    /// Read the top-N winners for `skill_name` filtered by `scorer`,
    /// ordered by score DESC. Bounded by the index
    /// `idx_evolved_prompts_skill_scorer_score`.
    pub fn best_for_skill(
        &self,
        skill_name: &str,
        scorer: &str,
        limit: usize,
    ) -> Result<Vec<EvolvedPrompt>, EvolveError> {
        let tc = self.db.global.clone();
        let conn = tc.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, skill_name, parent_id, prompt_body, score, scorer, generation, created_at, metadata \
                 FROM evolved_prompts \
                 WHERE skill_name = ?1 AND scorer = ?2 \
                 ORDER BY score DESC, created_at DESC \
                 LIMIT ?3",
            )
            .map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?;
        let rows = stmt
            .query_map(
                params![skill_name, scorer, limit as i64],
                row_to_evolved_prompt,
            )
            .map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?);
        }
        Ok(out)
    }

    /// Compute Beta-scorer seed pairs for a candidate list of skill
    /// names from this store. Caller passes the result to e.g.
    /// `wcore-skills::SkillRouter::restore_seeds`. For each name we look
    /// up the top-scored winner via [`PromptStore::best_for_skill`] and
    /// map `clamp(score, 0.0..=1.0)` × 5, rounded, to a simulated-success
    /// count. Names with no winner, or a winner whose scaled value is 0,
    /// are skipped. This is the inter-crate seam: `wcore-skills` cannot
    /// depend on `wcore-evolve` (the dep already runs the other way), so
    /// callers (e.g. agent bootstrap) bridge the two via this helper.
    pub fn seed_pairs_for(
        &self,
        candidates: &[String],
        scorer: &str,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, EvolveError> {
        let mut out = Vec::new();
        for name in candidates {
            let winners = self.best_for_skill(name, scorer, limit)?;
            let Some(top) = winners.first() else {
                continue;
            };
            // 0.0..=1.0 → 0..=5 simulated successes.
            let scaled = (top.score.clamp(0.0, 1.0) * 5.0).round() as u64;
            if scaled > 0 {
                out.push((name.clone(), scaled));
            }
        }
        Ok(out)
    }

    /// Read every winner for `skill_name` across all scorers, ordered by
    /// (generation DESC, score DESC).
    pub fn all_for_skill(&self, skill_name: &str) -> Result<Vec<EvolvedPrompt>, EvolveError> {
        let tc = self.db.global.clone();
        let conn = tc.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, skill_name, parent_id, prompt_body, score, scorer, generation, created_at, metadata \
                 FROM evolved_prompts \
                 WHERE skill_name = ?1 \
                 ORDER BY generation DESC, score DESC",
            )
            .map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?;
        let rows = stmt
            .query_map(params![skill_name], row_to_evolved_prompt)
            .map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| EvolveError::PromptStore(MemoryError::Db(e).to_string()))?);
        }
        Ok(out)
    }
}

fn row_to_evolved_prompt(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvolvedPrompt> {
    Ok(EvolvedPrompt {
        id: row.get(0)?,
        skill_name: row.get(1)?,
        parent_id: row.get(2)?,
        prompt_body: row.get(3)?,
        score: row.get(4)?,
        scorer: row.get(5)?,
        generation: row.get::<_, i64>(6)? as u32,
        created_at: row.get(7)?,
        metadata: row.get(8)?,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    fn fresh_store() -> PromptStore {
        let db = Arc::new(Db::open_memory().expect("open_memory"));
        PromptStore::new(db)
    }

    fn sample(id: &str, skill: &str, generation: u32, score: f64, scorer: &str) -> EvolvedPrompt {
        EvolvedPrompt {
            id: id.to_string(),
            skill_name: skill.to_string(),
            parent_id: None,
            prompt_body: format!("body for {id}"),
            score,
            scorer: scorer.to_string(),
            generation,
            created_at: 1_000 + generation as i64,
            metadata: None,
        }
    }

    #[test]
    fn record_variant_then_read_roundtrip() {
        let s = fresh_store();
        let row = sample("aaa", "skill-x", 0, 0.9, "bench");
        s.record_variant(&row).unwrap();

        let got = s.best_for_skill("skill-x", "bench", 10).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0], row);
    }

    #[test]
    fn best_for_skill_orders_by_score_desc() {
        let s = fresh_store();
        s.record_variant(&sample("a", "s", 0, 0.5, "bench"))
            .unwrap();
        s.record_variant(&sample("b", "s", 1, 0.9, "bench"))
            .unwrap();
        s.record_variant(&sample("c", "s", 2, 0.7, "bench"))
            .unwrap();

        let got = s.best_for_skill("s", "bench", 10).unwrap();
        assert_eq!(
            got.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["b", "c", "a"]
        );
    }

    #[test]
    fn best_for_skill_filters_by_scorer() {
        let s = fresh_store();
        s.record_variant(&sample("a", "s", 0, 0.9, "bench"))
            .unwrap();
        s.record_variant(&sample("b", "s", 0, 0.95, "default"))
            .unwrap();

        let bench = s.best_for_skill("s", "bench", 10).unwrap();
        assert_eq!(bench.len(), 1);
        assert_eq!(bench[0].id, "a");

        let default = s.best_for_skill("s", "default", 10).unwrap();
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].id, "b");
    }

    #[test]
    fn best_for_skill_respects_limit() {
        let s = fresh_store();
        for i in 0..5 {
            s.record_variant(&sample(
                &format!("id-{i}"),
                "s",
                i,
                0.5 + (i as f64) * 0.05,
                "bench",
            ))
            .unwrap();
        }
        let got = s.best_for_skill("s", "bench", 3).unwrap();
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn all_for_skill_returns_both_scorers() {
        let s = fresh_store();
        s.record_variant(&sample("a", "s", 0, 0.9, "bench"))
            .unwrap();
        s.record_variant(&sample("b", "s", 1, 0.8, "default"))
            .unwrap();
        s.record_variant(&sample("c", "other-skill", 0, 0.7, "bench"))
            .unwrap();

        let got = s.all_for_skill("s").unwrap();
        assert_eq!(got.len(), 2);
        // generation DESC, then score DESC — so "b" (gen=1) precedes "a" (gen=0).
        assert_eq!(got[0].id, "b");
        assert_eq!(got[1].id, "a");
    }

    #[test]
    fn record_variant_rejects_duplicate_id_at_same_skill_generation() {
        let s = fresh_store();
        let row = sample("dup", "s", 0, 0.5, "bench");
        s.record_variant(&row).unwrap();
        // UNIQUE (skill_name, generation, id) — second insert with same
        // tuple must error.
        let err = s.record_variant(&row).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("UNIQUE") || msg.contains("constraint") || msg.contains("PRIMARY KEY"),
            "expected uniqueness violation, got: {msg}"
        );
    }

    #[test]
    fn seed_pairs_for_empty_store_returns_empty() {
        let s = fresh_store();
        let pairs = s
            .seed_pairs_for(&["a".to_string(), "b".to_string()], "bench", 1)
            .unwrap();
        assert!(pairs.is_empty());
    }

    #[test]
    fn seed_pairs_for_maps_score_to_scaled_successes() {
        let s = fresh_store();
        s.record_variant(&sample("a", "alpha", 0, 0.9, "bench"))
            .unwrap();
        s.record_variant(&sample("b", "beta", 0, 0.5, "bench"))
            .unwrap();
        s.record_variant(&sample("c", "weak", 0, 0.1, "bench"))
            .unwrap();
        let pairs = s
            .seed_pairs_for(
                &[
                    "alpha".to_string(),
                    "beta".to_string(),
                    "weak".to_string(),
                    "missing".to_string(),
                ],
                "bench",
                1,
            )
            .unwrap();
        // 0.9 → 5 (rounds up from 4.5), 0.5 → 3 (rounds up from 2.5), 0.1
        // → 1 (rounds up from 0.5), missing → skipped (no row).
        let map: std::collections::HashMap<String, u64> = pairs.into_iter().collect();
        assert_eq!(map.get("alpha").copied(), Some(5));
        assert_eq!(map.get("beta").copied(), Some(3));
        assert_eq!(map.get("weak").copied(), Some(1));
        assert!(!map.contains_key("missing"));
    }

    #[test]
    fn seed_pairs_for_filters_by_scorer() {
        let s = fresh_store();
        s.record_variant(&sample("a", "alpha", 0, 0.9, "bench"))
            .unwrap();
        s.record_variant(&sample("b", "alpha", 0, 0.95, "default"))
            .unwrap();
        let bench = s
            .seed_pairs_for(&["alpha".to_string()], "bench", 1)
            .unwrap();
        assert_eq!(bench, vec![("alpha".to_string(), 5)]);
    }

    #[test]
    fn seed_pairs_picks_top_winner_only() {
        // limit=1 means we only consume the top row for scaling.
        let s = fresh_store();
        s.record_variant(&sample("low", "alpha", 0, 0.2, "bench"))
            .unwrap();
        s.record_variant(&sample("high", "alpha", 1, 0.95, "bench"))
            .unwrap();
        let pairs = s
            .seed_pairs_for(&["alpha".to_string()], "bench", 1)
            .unwrap();
        assert_eq!(pairs, vec![("alpha".to_string(), 5)]);
    }

    #[test]
    fn empty_skill_returns_empty_vec() {
        let s = fresh_store();
        assert!(
            s.best_for_skill("never-evolved", "bench", 10)
                .unwrap()
                .is_empty()
        );
        assert!(s.all_for_skill("never-evolved").unwrap().is_empty());
    }
}
