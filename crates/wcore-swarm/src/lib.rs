//! `wcore-swarm` — productized worktree-isolated multi-agent dispatch.
//!
//! Foundation for M5.6 (consensus) + M5.7 (memory propagation). The
//! public surface below is SPEC-LOCKED — downstream M5.6/M5.7 dispatch
//! briefs match against these exact signatures. Do not extend without
//! updating the roadmap.
//!
//! # Quick start
//!
//! ```ignore
//! use std::time::Duration;
//! use wcore_swarm::{Swarm, SwarmBrief};
//!
//! # async fn demo() -> wcore_swarm::Result<()> {
//! let swarm = Swarm::new(std::path::Path::new("/path/to/repo"))?;
//! let brief = SwarmBrief {
//!     task: "implement W7 fixture builder".into(),
//!     base_branch: "main".into(),
//!     worker_branch_prefix: "swarm/w7".into(),
//!     worker_command: vec!["bash".into(), "-c".into(), "echo hi".into()],
//!     timeout: Duration::from_secs(3600),
//!     env: vec![],
//! };
//! let handles = swarm.dispatch(brief, 4).await?;
//! let results = swarm.collect(handles).await?;
//! swarm.cleanup().await?;
//! # Ok(()) }
//! ```
//!
//! # Lifecycle invariants
//!
//! - `dispatch` REFUSES if the base repo is dirty (collision detection).
//! - Each worker gets a fresh worktree at `<repo>/.swarm-worktrees/<id>`.
//! - `collect` waits for all workers (already-finished handles in the
//!   v0.6 implementation; future versions may aggregate streaming output).
//! - `cleanup` removes ALL worker worktrees. Idempotent.
//! - Workers run as subprocesses of the orchestrator (process boundary;
//!   no shared memory). All git ops use argv mode (no shell interp).
//!
//! # What's NOT in v0.6
//!
//! - Cross-host dispatch.
//! - Encrypted channels (workers trust the orchestrator's UID).
//! - Live stdout streaming. Final stdout/stderr are returned by `collect`.
//!   For hung-worker detection, workers may opt into a minimal heartbeat
//!   via [`heartbeat::HeartbeatWriter`]; the orchestrator polls it via
//!   [`Swarm::worker_status`].

pub mod audit;
pub mod bridge;
pub mod collect;
pub mod consensus;
pub mod debate;
pub mod dispatch;
pub mod error;
pub mod fleet;
pub mod heartbeat;
pub mod mesh;
pub mod scorer;
pub mod topology;
pub mod worktree;

pub use bridge::SwarmMemoryBridge;
pub use consensus::{Consensus, ConsensusOutcome};
pub use debate::{Debate, DebateOutcome, DebateRound};
pub use error::{Result, SwarmError};
pub use fleet::{
    DEFAULT_SHARD_SIZE, FleetDispatcher, FleetError, FleetReducer, ShardReducer, ShardSummary,
};
pub use heartbeat::WorkerStatusFile;
pub use mesh::{AgentReport, BlackboardCtx, MeshAgent, MeshDispatcher, MeshError, Reducer};
pub use scorer::{RuleBasedScorer, Scorer};
pub use topology::{BlackboardScope, ParentVisibility, Topology, TopologyConfig, TopologyError};

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::worktree::WorktreeManager;

/// Brief describing what each worker should run. Wire-friendly:
/// `timeout` uses humantime so TOML briefs can write `timeout = "30s"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmBrief {
    /// Free-form human label for telemetry (e.g. "implement W7 fixture
    /// builder"). Not interpreted by `wcore-swarm`.
    pub task: String,
    /// Branch the worker worktrees are created from.
    pub base_branch: String,
    /// Branch prefix for each worker; the final branch is
    /// `<worker_branch_prefix>/<worker_id>`.
    pub worker_branch_prefix: String,
    /// argv to spawn for each worker (no shell interpretation). The first
    /// element is the program; the rest are arguments. Resolved against
    /// the OS PATH (and PATHEXT on Windows).
    pub worker_command: Vec<String>,
    /// Per-worker wall-clock timeout. On expiry the worker is reported as
    /// [`WorkerStatus::TimedOut`] and the child is SIGKILLed via
    /// `kill_on_drop`.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    /// Extra environment variables passed to each worker subprocess.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// Terminal state of a worker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkerStatus {
    Succeeded,
    Failed(String),
    TimedOut,
    Cancelled,
}

/// Live handle returned by [`Swarm::dispatch`]. Carries the worker's
/// final stdout/stderr/duration alongside the status (so the orchestrator
/// can poll heartbeats via [`Swarm::worker_status`] and then drain into
/// [`SwarmResult`] via [`Swarm::collect`]).
///
/// `duration` is intentionally NOT serialized — it's a runtime-only
/// `Instant`-derived value. The wire-friendly twin is [`SwarmResult`].
#[derive(Debug, Clone)]
pub struct WorkerHandle {
    pub worker_id: String,
    pub branch: String,
    pub status: WorkerStatus,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

/// Wire-friendly result aggregated from a [`WorkerHandle`]. Distinct from
/// the handle so future versions can attach extra collect-time fields
/// (e.g. commit SHAs touched) without changing the dispatch path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmResult {
    pub worker_id: String,
    pub branch: String,
    pub status: WorkerStatus,
    pub stdout: String,
    pub stderr: String,
    #[serde(with = "humantime_serde")]
    pub duration: Duration,
}

/// Top-level swarm orchestrator. Owns the repo root + the worktree
/// manager. One `Swarm` per orchestrator; `dispatch` may be called
/// multiple times in sequence (each call asserts clean checkout first).
pub struct Swarm {
    repo_root: PathBuf,
    manager: WorktreeManager,
}

impl Swarm {
    /// Construct a new swarm rooted at `repo_root`. Creates
    /// `<repo_root>/.swarm-worktrees/` if it does not exist.
    pub fn new(repo_root: &Path) -> Result<Self> {
        let manager = WorktreeManager::new(repo_root)?;
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            manager,
        })
    }

    /// Underlying repo root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Dispatch `count` workers in parallel using the same `brief`. Each
    /// gets a unique worker id (`<uuid>-<index>`), a fresh worktree, and a
    /// branch named `<brief.worker_branch_prefix>/<worker_id>`. Returns
    /// the handles in the order the workers complete (race-order may
    /// differ from index order — the caller should not assume).
    ///
    /// Refuses with [`SwarmError::DirtyCheckout`] if `repo_root` has any
    /// uncommitted changes (collision detection).
    pub async fn dispatch(&self, brief: SwarmBrief, count: usize) -> Result<Vec<WorkerHandle>> {
        self.manager.assert_clean().await?;
        let mut futs = Vec::with_capacity(count);
        for i in 0..count {
            let worker_id = format!("{}-{}", uuid::Uuid::new_v4().simple(), i);
            let manager_ref = &self.manager;
            let brief_ref = &brief;
            futs.push(dispatch::run_worker(manager_ref, worker_id, brief_ref));
        }
        // Concurrent poll of all worker futures via futures::join_all.
        // This is true parallelism for the await-points inside each
        // worker (worktree creation, subprocess output), all driven by
        // the current tokio runtime.
        let handles = futures::future::join_all(futs).await;
        Ok(handles)
    }

    /// Finalize the worker handles into wire-friendly results. In v0.6
    /// this is a synchronous transform; async-on-the-surface is reserved
    /// for future aggregation work without breaking M5.6/M5.7 callers.
    pub async fn collect(&self, handles: Vec<WorkerHandle>) -> Result<Vec<SwarmResult>> {
        collect::ResultCollector::finalize(handles)
    }

    /// Remove every worker worktree under `.swarm-worktrees/` via
    /// `git worktree remove --force`. Idempotent — safe to call twice.
    pub async fn cleanup(&self) -> Result<()> {
        self.manager.cleanup_all().await?;
        Ok(())
    }

    /// Read the worker's heartbeat file
    /// (`<worktree>/.swarm-status.json`). Returns `Ok(None)` if the
    /// worker has not yet written one (or never will — heartbeat is opt-in).
    ///
    /// Use this to detect hung workers WITHOUT consuming final
    /// stdout/stderr; those are only available after [`Self::collect`].
    pub fn worker_status(&self, handle: &WorkerHandle) -> Result<Option<WorkerStatusFile>> {
        let worktree = self.manager.swarm_root().join(&handle.worker_id);
        heartbeat::read_status(&worktree)
    }
}
