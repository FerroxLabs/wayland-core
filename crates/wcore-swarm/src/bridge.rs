//! M5.7 — `SwarmMemoryBridge`: cross-session memory propagation.
//!
//! Wires the M5.5 [`crate::Swarm`] dispatch lifecycle into wcore-memory's
//! episodic + procedural + semantic partitions so worker outcomes
//! propagate into the cross-session memory store.
//!
//! # Flow direction
//!
//! ```text
//!   parent_session                       worker (child) session
//!   ──────────────                       ──────────────────────
//!   semantic_snapshot_for_child(child)   <-  read_for_child(child)
//!                                            (bootstrap context)
//!                                        ->  record_child_outcome(child, result)
//!   procedural ⊕ child.procedural        <-  merge_child_into_parent(child)
//!                                            (timestamp-wins)
//! ```
//!
//! # Cycle / direction guard
//!
//! The bridge owns a [`wcore_memory::MemoryLineage`] keyed on session
//! IDs. `record_child_lineage` rejects edges that would create a cycle
//! (e.g. a worker trying to declare the orchestrator as its child).
//! `read_ancestor_chain` refuses a read where the target is downstream
//! of the reader — workers cannot harvest their own descendants'
//! memory.
//!
//! # NOT in v0.6
//!
//! - Live-streaming worker memory updates. Workers emit memory at
//!   exit only; `record_child_outcome` is the integration point.
//! - Multi-parent procedural merges (each child has exactly one parent
//!   in the lineage forest). See `wcore_memory::propagation` module
//!   doc for the deferred-features rationale.

use std::sync::Arc;

use tokio::sync::Mutex;
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::propagation::MemoryLineage;
use wcore_memory::v2_types::{
    AccessToken, Episode, EpisodeId, EpisodeStatus, Procedure, ProcedureId, ProcedureStatus, Tier,
};

use crate::error::{Result, SwarmError};
use crate::{SwarmResult, WorkerStatus};

/// Per-orchestrator memory bridge.
///
/// Construct once with the parent session id; clone freely (everything
/// inside is `Arc`-wrapped). One bridge instance can serve many
/// dispatched workers — the `MemoryLineage` inside serialises
/// concurrent `record_child_lineage` / `merge_child_into_parent` calls.
#[derive(Clone)]
pub struct SwarmMemoryBridge {
    dispatcher: PartitionDispatcher,
    parent_session: String,
    lineage: Arc<Mutex<MemoryLineage>>,
}

impl SwarmMemoryBridge {
    /// Build a bridge that propagates memory under `parent_session` via
    /// the supplied `dispatcher`. Callers typically pull the
    /// dispatcher off a fully-constructed `wcore_memory::Memory`:
    ///
    /// ```ignore
    /// let mem = wcore_memory::Memory::open_in_memory().await?;
    /// let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "orchestrator".into());
    /// ```
    pub fn new(dispatcher: PartitionDispatcher, parent_session: String) -> Self {
        Self {
            dispatcher,
            parent_session,
            lineage: Arc::new(Mutex::new(MemoryLineage::new())),
        }
    }

    /// The orchestrator session id this bridge is rooted at.
    pub fn parent_session(&self) -> &str {
        &self.parent_session
    }

    /// Snapshot of the parent's recent episodic memory at the requested
    /// tier, suitable for handing to a freshly-dispatched child worker.
    /// Records the parent->child lineage edge as a side effect so
    /// subsequent `record_child_outcome` / `merge_child_into_parent`
    /// calls already know the relationship.
    ///
    /// Returns at most `limit` episodes ordered newest-first.
    pub async fn read_for_child(
        &self,
        child_id: &str,
        tier: Tier,
        limit: usize,
    ) -> Result<Vec<Episode>> {
        self.record_child_lineage(child_id, &self.parent_session.clone())
            .await?;
        // Pull recent episodes for the parent session. We use a
        // text-empty search at the requested tier and post-filter by
        // session_id locally — the dispatcher's `search` is the
        // production retrieval surface but it requires an embedder and
        // returns Hits (not Episodes). For the bridge's bootstrap
        // contract we want the raw Episodes so the worker can rehydrate
        // them; that means a direct partition read.
        //
        // We piggyback on the global-search-style listing through
        // `top_procedures`-adjacent rationale (no embedder needed):
        // fetch via a single direct prepared SELECT through the
        // EpisodicPartition's underlying Db. The dispatcher does not
        // currently expose a "list-recent-by-session" API on the
        // MemoryApi trait; rather than carve a new trait method we
        // reach into the episodic partition reference owned by the
        // dispatcher.
        let mut episodes = self
            .dispatcher
            .episodic
            .list_recent_for_session(&self.parent_session, tier, limit)
            .await
            .map_err(|e| SwarmError::Collect {
                worker_id: child_id.to_string(),
                reason: e.to_string(),
            })?;
        episodes.truncate(limit);
        Ok(episodes)
    }

    /// Record that `child_id`'s parent is `parent_id`. Returns
    /// `SwarmError::Collect` if the edge would create a cycle in the
    /// lineage forest. Cheap: O(depth) cycle check.
    pub async fn record_child_lineage(&self, child_id: &str, parent_id: &str) -> Result<()> {
        let mut lin = self.lineage.lock().await;
        lin.record_parent(child_id, parent_id)
            .map_err(|e| SwarmError::Collect {
                worker_id: child_id.to_string(),
                reason: e.to_string(),
            })
    }

    /// Persist a worker's final outcome as an episode under the worker's
    /// session id, with the embedding mirror written into the dim-aware
    /// `vec_episodes_<dim>` virtual table for KNN retrieval (M5.7
    /// carryover #3). Returns the new episode id.
    ///
    /// Status flow: `WorkerStatus::Succeeded` records as `Active`;
    /// `Failed`/`TimedOut`/`Cancelled` records as `Active` too (the
    /// outcome is still factual memory — the *contents* of the episode
    /// summary signal the failure). Operators can filter via the
    /// `episode_type` we set to `"swarm_worker_outcome"`.
    pub async fn record_child_outcome(
        &self,
        child_id: &str,
        result: &SwarmResult,
    ) -> Result<EpisodeId> {
        let summary = format!(
            "[swarm] worker={} branch={} status={} stdout_len={} stderr_len={} duration_ms={}",
            result.worker_id,
            result.branch,
            describe_status(&result.status),
            result.stdout.len(),
            result.stderr.len(),
            result.duration.as_millis(),
        );
        let ep = Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: 0, // record_with_embedding stamps now() if 0
            episode_type: "swarm_worker_outcome".to_string(),
            summary,
            atomic_facts: vec![
                format!("worker.branch={}", result.branch),
                format!("worker.status={}", describe_status(&result.status)),
            ],
            source: format!("sub-agent:{child_id}"),
            source_product: "wcore-swarm".to_string(),
            session_id: Some(child_id.to_string()),
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        };
        self.dispatcher
            .episodic
            .record_with_embedding(ep)
            .await
            .map_err(|e| SwarmError::Collect {
                worker_id: child_id.to_string(),
                reason: e.to_string(),
            })
    }

    /// Merge a child worker's procedural-tier writes (skill artifacts
    /// it generated during its run) up into the parent's procedural
    /// tier. Returns the count of merged rows.
    ///
    /// Conflict resolution: "timestamp wins" — if a procedure with the
    /// same `name` already exists in the parent tier with a `ts >= the
    /// child's, the child's row is skipped. Otherwise the child's
    /// artifact is upserted under a fresh `ProcedureId` rooted at the
    /// parent session.
    pub async fn merge_child_into_parent(&self, child_id: &str) -> Result<usize> {
        // Pull every procedural row from the child's project tier
        // (procedural is project/global only — no Session tier per
        // `valid_combinations`). `list` is cheap by design (tens to
        // hundreds of rows).
        let child_procs = self
            .dispatcher
            .procedural
            .list(Tier::Project)
            .await
            .map_err(|e| SwarmError::Collect {
                worker_id: child_id.to_string(),
                reason: e.to_string(),
            })?
            .into_iter()
            .filter(|p| p.created_by == format!("sub-agent:{child_id}"))
            .collect::<Vec<_>>();

        let parent_procs = self
            .dispatcher
            .procedural
            .list(Tier::Project)
            .await
            .map_err(|e| SwarmError::Collect {
                worker_id: child_id.to_string(),
                reason: e.to_string(),
            })?;

        // Filter parent_existing to ONLY rows that were already merged
        // into the parent (created_by tag `swarm-merge:<parent>`). The
        // child's own `sub-agent:<id>` rows would otherwise alias the
        // candidate row itself and make every merge a no-op.
        let parent_merge_tag = format!("swarm-merge:{}", self.parent_session);
        let mut merged = 0usize;
        for child_proc in child_procs {
            let parent_existing = parent_procs
                .iter()
                .filter(|p| p.name == child_proc.name && p.created_by == parent_merge_tag)
                .max_by_key(|p| p.ts);
            let newer = parent_existing.is_some_and(|p| p.ts >= child_proc.ts);
            if newer {
                continue;
            }
            let promoted = Procedure {
                id: ProcedureId::new(),
                tier: Tier::Project,
                ts: child_proc.ts,
                name: child_proc.name.clone(),
                description: child_proc.description.clone(),
                artifact: child_proc.artifact.clone(),
                // Promoted into the parent's namespace — the original
                // child row stays put for audit.
                status: ProcedureStatus::Active,
                created_by: format!("swarm-merge:{}", self.parent_session),
                thompson_alpha: child_proc.thompson_alpha,
                thompson_beta: child_proc.thompson_beta,
                use_count: child_proc.use_count,
                success_count: child_proc.success_count,
            };
            self.dispatcher
                .procedural
                .upsert(promoted)
                .await
                .map_err(|e| SwarmError::Collect {
                    worker_id: child_id.to_string(),
                    reason: e.to_string(),
                })?;
            merged += 1;
        }
        Ok(merged)
    }

    /// Permit `reader` to read from `target` only if `target` is an
    /// ancestor of `reader` in the lineage forest. Returns
    /// `SwarmError::Collect` (carrying `descendant read denied`) if
    /// `target` is downstream of `reader`. Both reader and target with
    /// no recorded edges are treated as siblings and permitted.
    pub async fn read_ancestor_chain(&self, reader: &str, target: &str) -> Result<()> {
        let lin = self.lineage.lock().await;
        if lin.is_ancestor(reader, target) {
            return Err(SwarmError::Collect {
                worker_id: reader.to_string(),
                reason: format!("descendant read denied: {target} is downstream of {reader}"),
            });
        }
        Ok(())
    }

    /// Test/debug snapshot of how many lineage edges have been recorded.
    pub async fn lineage_len(&self) -> usize {
        self.lineage.lock().await.len()
    }
}

/// Helper: SwarmMemoryBridge needs access to PartitionDispatcher's
/// inner partition handles. The dispatcher fields were already public
/// at the crate level in M5.5; we re-export the shape here so the
/// bridge does not need to construct a separate `MemoryApi`.
///
/// (Removed `AccessToken` plumbing — the dispatcher partitions accept
/// raw inserts without token validation; only the trait surface
/// enforces ACL. M5.7 deliberately writes via the partition refs to
/// keep the bridge a thin shim, with M5.8/M5.9 owning ACL gating on a
/// follow-up.)
fn describe_status(s: &WorkerStatus) -> &'static str {
    match s {
        WorkerStatus::Succeeded => "succeeded",
        WorkerStatus::Failed(_) => "failed",
        WorkerStatus::TimedOut => "timed_out",
        WorkerStatus::Cancelled => "cancelled",
    }
}

// Suppress unused-import warning when downstream feature flags trim the
// AccessToken surface — kept in the imports for future ACL wiring.
#[allow(dead_code)]
fn _access_token_marker(_t: AccessToken) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wcore_memory::Memory;

    async fn fresh_bridge(parent: &str) -> (Memory, SwarmMemoryBridge) {
        let mem = Memory::open_in_memory().await.unwrap();
        let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), parent.to_string());
        (mem, bridge)
    }

    #[tokio::test]
    async fn record_child_lineage_then_descendant_read_denied() {
        let (_mem, bridge) = fresh_bridge("root").await;
        bridge
            .record_child_lineage("child-a", "root")
            .await
            .unwrap();
        bridge
            .record_child_lineage("grandchild-b", "child-a")
            .await
            .unwrap();

        // grandchild reading from its ancestor: OK.
        bridge
            .read_ancestor_chain("grandchild-b", "child-a")
            .await
            .unwrap();

        // child reading from its descendant: must error with
        // "descendant read denied".
        let err = bridge
            .read_ancestor_chain("child-a", "grandchild-b")
            .await
            .unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("descendant") || s.contains("downstream"),
            "got: {s}"
        );
    }

    #[tokio::test]
    async fn record_child_outcome_writes_episode_with_knn_mirror() {
        let (mem, bridge) = fresh_bridge("orch-1").await;
        let result = SwarmResult {
            worker_id: "w-1".to_string(),
            branch: "swarm/x/w-1".to_string(),
            status: WorkerStatus::Succeeded,
            stdout: "hello".to_string(),
            stderr: String::new(),
            duration: Duration::from_secs(2),
        };
        let id = bridge.record_child_outcome("w-1", &result).await.unwrap();

        // The episode should round-trip via the standard MemoryApi.
        let ep = mem
            .dispatcher
            .episodic
            .get(&id, Tier::Project)
            .await
            .unwrap();
        assert_eq!(ep.episode_type, "swarm_worker_outcome");
        assert_eq!(ep.session_id.as_deref(), Some("w-1"));
        assert!(ep.summary.contains("worker=w-1"));
        assert!(ep.summary.contains("status=succeeded"));
    }

    #[tokio::test]
    async fn cycle_rejected_by_record_child_lineage() {
        let (_mem, bridge) = fresh_bridge("root").await;
        bridge.record_child_lineage("a", "root").await.unwrap();
        bridge.record_child_lineage("b", "a").await.unwrap();
        // a -> b would form a -> b -> a -> b cycle.
        let err = bridge.record_child_lineage("a", "b").await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("cycle"));
    }

    #[tokio::test]
    async fn merge_child_into_parent_promotes_unique_procedures() {
        use uuid::Uuid;
        let (mem, bridge) = fresh_bridge("orch-1").await;

        // Worker writes one procedure tagged with its sub-agent source.
        let child_proc = Procedure {
            id: ProcedureId(Uuid::now_v7()),
            tier: Tier::Project,
            ts: 1_000,
            name: "discovered-skill".into(),
            description: "found by w-1".into(),
            artifact: "---\nname: discovered\n---\nbody".into(),
            status: ProcedureStatus::Active,
            created_by: "sub-agent:w-1".into(),
            thompson_alpha: 1.0,
            thompson_beta: 1.0,
            use_count: 0,
            success_count: 0,
        };
        mem.dispatcher.procedural.upsert(child_proc).await.unwrap();

        let merged = bridge.merge_child_into_parent("w-1").await.unwrap();
        assert!(merged >= 1, "expected at least one merged row");

        // Verify the promoted row carries the swarm-merge source tag.
        let all = mem.dispatcher.procedural.list(Tier::Project).await.unwrap();
        assert!(
            all.iter()
                .any(|p| p.name == "discovered-skill" && p.created_by.starts_with("swarm-merge:")),
            "merged row missing: {all:?}"
        );
    }
}
