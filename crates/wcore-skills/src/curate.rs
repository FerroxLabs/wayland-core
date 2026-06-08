//! F11: skill corpus curator.
//!
//! Fires on `on_session_end` (W2 surface). Reads P4 via `MemoryApi`,
//! scores procedures, dedupes overlapping staged drafts, and archives
//! stale or low-quality entries via the P4 state-machine
//! (`ProcedureStatus::{Staged,Active} → Archived`).
//!
//! See design contract §5.3 (F11 acceptance) and the W9 plan.

use std::sync::Arc;

// W9 T5.0 cycle-break: Hook + HookAction + SessionEndSummary live in
// wcore-config::hooks (lifted out of wcore-agent).
use wcore_config::hooks::{Hook, HookAction, SessionEndSummary};
use wcore_memory::api::MemoryApi;
use wcore_memory::error::Result as MemResult;
use wcore_memory::v2_types::{AccessToken, Procedure, ProcedureStatus, Tier};

pub const MODULE_NAME: &str = "wcore-skills::curate";

#[derive(Debug, Default, Clone)]
pub struct CuratorReport {
    pub archived: Vec<String>,
    pub dedupes: Vec<(String, String)>, // (kept_name, archived_alias)
    pub kept_active: Vec<String>,
}

pub struct Curator {
    pub(crate) mem: Arc<dyn MemoryApi>,
    pub(crate) opts: CuratorOpts,
}

#[derive(Debug, Clone)]
pub struct CuratorOpts {
    /// Levenshtein threshold for treating two descriptions as duplicates.
    pub duplicate_description_distance: u32,
    /// Days since last use before an active procedure is archived.
    pub stale_after_days: u64,
    /// Minimum success/use ratio for an active procedure to stay active.
    pub min_success_ratio: f64,
}

impl Default for CuratorOpts {
    fn default() -> Self {
        Self {
            duplicate_description_distance: 5,
            stale_after_days: 30,
            min_success_ratio: 0.2,
        }
    }
}

impl Curator {
    pub fn new(mem: Arc<dyn MemoryApi>) -> Self {
        Self::with_opts(mem, CuratorOpts::default())
    }

    pub fn with_opts(mem: Arc<dyn MemoryApi>, opts: CuratorOpts) -> Self {
        Self { mem, opts }
    }

    pub async fn run(&self) -> MemResult<CuratorReport> {
        let mut report = CuratorReport::default();
        let procs = self
            .mem
            .list_procedures(Tier::Project, AccessToken::System)
            .await?;

        // 1. Candidates = Staged or Active (Pinned untouched, Archived
        //    terminal). Score and sort descending so the strongest entry
        //    survives any subsequent dedup collision.
        let mut candidates: Vec<Procedure> = procs
            .iter()
            .filter(|p| matches!(p.status, ProcedureStatus::Staged | ProcedureStatus::Active))
            .cloned()
            .collect();
        candidates.sort_by(|a, b| {
            score(b)
                .partial_cmp(&score(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 2. Dedupe overlapping descriptions (Levenshtein <= threshold).
        let mut keep: Vec<Procedure> = Vec::new();
        let thresh = self.opts.duplicate_description_distance as usize;
        for p in candidates.iter() {
            let collides = keep
                .iter()
                .any(|k| crate::audit::levenshtein(&k.description, &p.description) <= thresh);
            if collides {
                let winner = keep
                    .iter()
                    .find(|k| crate::audit::levenshtein(&k.description, &p.description) <= thresh)
                    .map(|k| k.name.clone())
                    .unwrap_or_default();
                report.dedupes.push((winner, p.name.clone()));
                self.archive(p).await?;
                report.archived.push(p.name.clone());
            } else {
                keep.push(p.clone());
            }
        }

        // 3. Active procs with use_count >= 5 and success_ratio < min get
        //    archived. Staged drafts haven't been used yet, so skip them.
        let mut final_keep: Vec<Procedure> = Vec::new();
        for p in keep {
            if matches!(p.status, ProcedureStatus::Active) && p.use_count >= 5 {
                let ratio = if p.use_count == 0 {
                    0.0
                } else {
                    p.success_count as f64 / p.use_count as f64
                };
                if ratio < self.opts.min_success_ratio {
                    self.archive(&p).await?;
                    report.archived.push(p.name.clone());
                    continue;
                }
            }
            final_keep.push(p);
        }

        report.kept_active = final_keep.iter().map(|p| p.name.clone()).collect();
        Ok(report)
    }

    async fn archive(&self, p: &Procedure) -> MemResult<()> {
        if matches!(
            p.status,
            ProcedureStatus::Pinned | ProcedureStatus::Archived
        ) {
            return Ok(());
        }
        let mut next = p.clone();
        next.status = ProcedureStatus::Archived;
        self.mem
            .upsert_procedure(next, AccessToken::System)
            .await
            .map(|_| ())
    }
}

/// Composite score: success_ratio · log(1 + use_count). Pure function.
/// Brand-new procs (use_count == 0) get a Bayesian prior of 0.5 so they
/// don't auto-archive on day 1.
fn score(p: &Procedure) -> f64 {
    let success_ratio = if p.use_count == 0 {
        0.5
    } else {
        p.success_count as f64 / p.use_count as f64
    };
    let use_factor = (1.0 + p.use_count as f64).ln();
    success_ratio * use_factor
}

#[async_trait::async_trait]
impl Hook for Curator {
    fn name(&self) -> &str {
        "wcore-skills::curate::Curator"
    }

    async fn on_session_end(&self, _summary: &SessionEndSummary) -> HookAction {
        match self.run().await {
            Ok(_report) => HookAction::Continue,
            Err(e) => {
                tracing::warn!("curator on_session_end failed: {e}");
                HookAction::Continue
            }
        }
    }
}
