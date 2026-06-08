//! M3.6 — Skill prioritizer.
//!
//! Reads the procedural partition's top-K rows at session start, and reorders
//! a flat list of skill names so successful skills surface before failing
//! ones. Names with no telemetry stay in input order, placed AFTER known-good
//! and BEFORE known-bad. One-shot, session-scoped — invoked from `bootstrap`
//! when `memory.enabled = true`.
//!
//! The score is the procedural row's Beta-mean `alpha / (alpha + beta)`,
//! supplied via `MemoryApi::top_procedures(min_uses=1)` so brand-new rows
//! with default `(1.0, 1.0)` priors (Beta-mean = 0.5) don't pollute the
//! ranking before they accumulate evidence.
//!
//! Procedural rows for skill telemetry are named `skill:<name>` per the
//! M3.5 convention in `wcore_memory::partition::record_skill_use`.

use std::collections::HashMap;
use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Tier};

pub struct SkillPrioritizer {
    memory: Arc<dyn MemoryApi>,
}

impl SkillPrioritizer {
    pub fn new(memory: Arc<dyn MemoryApi>) -> Self {
        Self { memory }
    }

    /// Reorder `input` by procedural-partition score:
    ///   1. Skills present in the procedural partition with Beta-mean >= 0.5,
    ///      sorted descending by score.
    ///   2. Skills with NO procedural row, in their original input order.
    ///   3. Skills present with Beta-mean < 0.5, sorted ascending (worst last).
    ///
    /// Falls back to the input order on any memory error or when no rows
    /// match — never panics, never blocks the bootstrap path.
    pub async fn priority_order(&self, input: &[String], k: usize) -> Vec<String> {
        // Ask for at least `input.len()` rows so we don't truncate before
        // we've scored every input name. `k` is the caller's hint; we treat
        // it as a lower bound on the candidate pool.
        let pool = k.max(input.len()).max(1);
        let top = match self
            .memory
            .top_procedures(Tier::Project, pool, 1, AccessToken::System)
            .await
        {
            Ok(v) => v,
            Err(_) => return input.to_vec(),
        };

        // Map skill name → Beta-mean. Procedural rows are stored as
        // `skill:<name>`; anything without that prefix is unrelated and
        // ignored.
        let mut scored: HashMap<String, f64> = HashMap::new();
        for p in top {
            if let Some(name) = p.name.strip_prefix("skill:") {
                let denom = p.thompson_alpha + p.thompson_beta;
                if denom > 0.0 {
                    let mean = p.thompson_alpha / denom;
                    scored.insert(name.to_string(), mean);
                }
            }
        }

        // Partition input into good (>=0.5) / unseen / bad (<0.5).
        let mut good: Vec<&String> = Vec::new();
        let mut unseen: Vec<&String> = Vec::new();
        let mut bad: Vec<&String> = Vec::new();
        for n in input {
            match scored.get(n.as_str()).copied() {
                Some(s) if s >= 0.5 => good.push(n),
                Some(_) => bad.push(n),
                None => unseen.push(n),
            }
        }

        good.sort_by(|a, b| {
            let sa = scored.get(a.as_str()).copied().unwrap_or(0.0);
            let sb = scored.get(b.as_str()).copied().unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        bad.sort_by(|a, b| {
            let sa = scored.get(a.as_str()).copied().unwrap_or(0.0);
            let sb = scored.get(b.as_str()).copied().unwrap_or(0.0);
            sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal)
        });

        good.into_iter().chain(unseen).chain(bad).cloned().collect()
    }
}
