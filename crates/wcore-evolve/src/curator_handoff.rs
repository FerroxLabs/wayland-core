//! Curator hand-off boundary. Winners NEVER write to the live catalog
//! directly — they pass through a `CuratorPort` trait so the W9 F11 curator
//! (or any future replacement) can dedupe, judge, archive, or promote them.
//!
//! `wcore-evolve` deliberately does NOT depend on `wcore-skills`. The
//! adapter that wires this trait to `wcore_skills::curate::Curator` lives in
//! the binary (`crates/wcore-evolve/src/bin/wcore-evolve.rs`).

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::EvolveError;
use crate::generation::ScoredCandidate;

/// What the curator decided about a candidate. Mirrors the W9 F11 shape but
/// stays local to wcore-evolve so we don't take a `wcore-skills` dep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Promote,
    Archive,
}

/// Lineage carried with every hand-off so the curator can record where the
/// candidate came from for post-mortem analysis.
#[derive(Debug, Clone)]
pub struct Lineage {
    pub run_id: String,
    pub generation: u32,
    pub child_index: u32,
    pub parent_id: String,
    pub mutation_kind: String,
    pub score: f64,
}

#[async_trait]
pub trait CuratorPort: Send + Sync {
    async fn submit(&self, body: &str, lineage: &Lineage) -> Result<Decision, String>;
}

pub struct Handoff {
    curator: Arc<dyn CuratorPort>,
}

impl Handoff {
    pub fn new(curator: Arc<dyn CuratorPort>) -> Self {
        Self { curator }
    }

    pub async fn promote(
        &self,
        w: &ScoredCandidate,
        parent_id: &str,
        run_id: &str,
    ) -> Result<Decision, EvolveError> {
        let lineage = Lineage {
            run_id: run_id.to_string(),
            generation: 0, // Generation index lives on the ScoredCandidate's
            // enclosing context; this hand-off API is per-winner so we
            // accept the caller-supplied run_id and parent_id and stamp
            // generation = 0 (the curator looks up the full lineage from
            // the trace event stream if it cares).
            child_index: w.child_index,
            parent_id: parent_id.to_string(),
            mutation_kind: format!("{:?}", w.mutation.kind),
            score: w.score.dimensions.combined,
        };
        self.curator
            .submit(&w.mutation.body, &lineage)
            .await
            .map_err(EvolveError::CuratorRejected)
    }
}
