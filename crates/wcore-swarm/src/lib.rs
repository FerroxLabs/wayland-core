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
//! - Dispatch admission caps worker processes, retained worktrees, and total
//!   captured output before any worker is created.
//! - Each worker gets a child-owned standalone repository under
//!   `<repo>/.swarm-worktrees/<id>/checkout`.
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
pub mod reduce;
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
pub use reduce::{ReduceMode, ReduceOutput, reduce};
pub use scorer::{RuleBasedScorer, Scorer};
pub use topology::{BlackboardScope, ParentVisibility, Topology, TopologyConfig, TopologyError};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::worktree::WorktreeManager;

/// Maximum number of workers scheduled by one Swarm dispatch.
/// This is also the canonical cap exposed by [`Topology::Swarm`].
pub const MAX_DISPATCH_WORKERS: usize = 100;

/// Maximum workers creating worktrees or executing processes at once. The
/// remaining admitted workers stay as bounded futures without host processes.
pub const MAX_CONCURRENT_WORKERS: usize = 20;

/// Separate evidence quota for worktrees retained across dispatches. It is
/// deliberately larger than one full wave so preserving a failed wave does
/// not prevent the next frontier-capacity dispatch.
pub const MAX_RETAINED_WORKTREES: usize = 256;

/// Maximum aggregate stdout + stderr bytes retained by one dispatch,
/// independent of worker count.
pub const MAX_DISPATCH_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

const MAX_WORKER_STREAM_BYTES: usize = 8 * 1024 * 1024;

type DispatchGate = tokio::sync::Mutex<()>;

fn dispatch_gate_for(repo_root: &Path) -> Result<Arc<DispatchGate>> {
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Weak<DispatchGate>>>> = OnceLock::new();

    let mut gates = GATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| {
            SwarmError::DispatchAdmission("repository dispatch registry is unavailable".into())
        })?;
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(repo_root).and_then(Weak::upgrade) {
        return Ok(gate);
    }

    let gate = Arc::new(DispatchGate::new(()));
    gates.insert(repo_root.to_path_buf(), Arc::downgrade(&gate));
    Ok(gate)
}

#[derive(Debug, Clone, Copy)]
struct DispatchLimits {
    worker_stream_bytes: usize,
}

impl DispatchLimits {
    fn admit(requested: usize, retained: usize) -> Result<Self> {
        let occupied = retained
            .checked_add(requested)
            .ok_or_else(|| SwarmError::DispatchAdmission("worker count overflowed".into()))?;
        if requested > MAX_DISPATCH_WORKERS {
            return Err(SwarmError::DispatchAdmission(format!(
                "requested {requested} worker(s); scheduled-worker cap is {MAX_DISPATCH_WORKERS}"
            )));
        }
        if occupied > MAX_RETAINED_WORKTREES {
            return Err(SwarmError::DispatchAdmission(format!(
                "requested {requested} worker(s) with {retained} retained worktree(s); evidence quota is {MAX_RETAINED_WORKTREES}"
            )));
        }

        let worker_stream_bytes = if requested == 0 {
            MAX_WORKER_STREAM_BYTES
        } else {
            let streams = requested.checked_mul(2).ok_or_else(|| {
                SwarmError::DispatchAdmission("worker stream count overflowed".into())
            })?;
            (MAX_DISPATCH_OUTPUT_BYTES / streams).min(MAX_WORKER_STREAM_BYTES)
        };
        if worker_stream_bytes == 0 {
            return Err(SwarmError::DispatchAdmission(
                "dispatch output budget cannot provide a bounded worker stream".into(),
            ));
        }
        Ok(Self {
            worker_stream_bytes,
        })
    }
}

/// Brief describing what each worker should run. Wire-friendly:
/// `timeout` uses humantime so TOML briefs can write `timeout = "30s"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmBrief {
    /// Free-form human label for telemetry (e.g. "implement W7 fixture
    /// builder"). Not interpreted by `wcore-swarm`.
    pub task: String,
    /// Ref that must resolve to the current clean checkout's exact HEAD.
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
    dispatch_gate: Arc<DispatchGate>,
    terminal_heartbeats: Mutex<HashMap<String, WorkerStatusFile>>,
}

impl Swarm {
    /// Construct a new swarm rooted at `repo_root`. Creates
    /// `<repo_root>/.swarm-worktrees/` if it does not exist.
    pub fn new(repo_root: &Path) -> Result<Self> {
        let manager = WorktreeManager::new(repo_root)?;
        let repo_root = manager.repo_root().to_path_buf();
        let dispatch_gate = dispatch_gate_for(&repo_root)?;
        Ok(Self {
            repo_root,
            manager,
            dispatch_gate,
            terminal_heartbeats: Mutex::new(HashMap::new()),
        })
    }

    /// Underlying repo root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Dispatch `count` workers in parallel using the same `brief`. Each
    /// gets a unique worker id (`<uuid>-<index>`), a standalone checkout, and a
    /// branch named `<brief.worker_branch_prefix>/<worker_id>`. Returns
    /// the handles in the order the workers complete (race-order may
    /// differ from index order — the caller should not assume).
    ///
    /// Refuses with [`SwarmError::DirtyCheckout`] if `repo_root` has any
    /// uncommitted changes (collision detection), or
    /// [`SwarmError::DispatchAdmission`] if this request would exceed the
    /// worker/worktree/output envelope.
    pub async fn dispatch(&self, brief: SwarmBrief, count: usize) -> Result<Vec<WorkerHandle>> {
        self.dispatch_with_cancel(brief, count, CancellationToken::new())
            .await
    }

    /// Dispatch workers under a cooperative cancellation token.
    ///
    /// Cancellation terminates each worker's owned process group or Windows
    /// Job and returns [`WorkerStatus::Cancelled`] for unfinished workers.
    pub async fn dispatch_with_cancel(
        &self,
        brief: SwarmBrief,
        count: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<WorkerHandle>> {
        let _dispatch_guard = self.dispatch_gate.try_lock().map_err(|_| {
            SwarmError::DispatchAdmission(
                "another dispatch is already active for this repository in this process".into(),
            )
        })?;
        let retained = self.manager.retained_worker_count(MAX_RETAINED_WORKTREES)?;
        let limits = DispatchLimits::admit(count, retained)?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(Vec::new()),
            result = self.manager.assert_clean() => result?,
        }
        let pinned_head = self
            .manager
            .pinned_dispatch_base(&brief.base_branch)
            .await?;
        let capacity = self
            .manager
            .workspace_capacity(count.min(MAX_CONCURRENT_WORKERS))
            .await?;
        self.manager
            .assert_dispatch_checkout_fits(&pinned_head, capacity)
            .await?;
        let mut futs = Vec::with_capacity(count);
        for i in 0..count {
            let worker_id = format!("{}-{}", uuid::Uuid::new_v4().simple(), i);
            let manager_ref = &self.manager;
            let brief_ref = &brief;
            // Keep each worker state machine on the heap. The dispatch future
            // otherwise contains one large `run_worker` future inline and can
            // overflow a small Tokio test/runtime stack before its first
            // suspension point.
            futs.push(Box::pin(dispatch::run_worker(
                manager_ref,
                worker_id,
                brief_ref,
                limits.worker_stream_bytes,
                &pinned_head,
                capacity,
                cancel.clone(),
            )));
        }
        // Keep all admitted workers scheduled while bounding simultaneous
        // worktree creation and subprocess ownership. `buffer_unordered`
        // also makes the documented completion-order result explicit.
        let terminals: Vec<dispatch::WorkerTerminal> = futures::stream::iter(futs)
            .buffer_unordered(MAX_CONCURRENT_WORKERS)
            .collect()
            .await;
        let mut heartbeats = self.terminal_heartbeats.lock().map_err(|_| {
            SwarmError::DispatchAdmission("terminal heartbeat registry is unavailable".into())
        })?;
        let mut handles = Vec::with_capacity(terminals.len());
        for terminal in terminals {
            if let Some(heartbeat) = terminal.heartbeat {
                heartbeats.insert(terminal.handle.worker_id.clone(), heartbeat);
            }
            handles.push(terminal.handle);
        }
        Ok(handles)
    }

    /// Finalize the worker handles into wire-friendly results. In v0.6
    /// this is a synchronous transform; async-on-the-surface is reserved
    /// for future aggregation work without breaking M5.6/M5.7 callers.
    pub async fn collect(&self, handles: Vec<WorkerHandle>) -> Result<Vec<SwarmResult>> {
        let mut heartbeats = self.terminal_heartbeats.lock().map_err(|_| {
            SwarmError::WorktreeIo("terminal heartbeat registry is unavailable".into())
        })?;
        for handle in &handles {
            heartbeats.remove(&handle.worker_id);
        }
        drop(heartbeats);
        collect::ResultCollector::finalize(handles)
    }

    /// Remove every worker worktree under `.swarm-worktrees/` via
    /// `git worktree remove --force`. Idempotent — safe to call twice.
    pub async fn cleanup(&self) -> Result<()> {
        self.cleanup_with_cancel(CancellationToken::new()).await
    }

    /// Run bounded cleanup, aborting the active Git subprocess if cancelled.
    /// Incomplete cleanup returns every residual path observed before return.
    pub async fn cleanup_with_cancel(&self, cancel: CancellationToken) -> Result<()> {
        self.manager.cleanup_all(&cancel).await?;
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
        if let Some(status) = heartbeat::read_status(&worktree)? {
            return Ok(Some(status));
        }
        let heartbeats = self.terminal_heartbeats.lock().map_err(|_| {
            SwarmError::WorktreeIo("terminal heartbeat registry is unavailable".into())
        })?;
        Ok(heartbeats.get(&handle.worker_id).cloned())
    }
}

#[cfg(test)]
mod dispatch_limit_tests {
    use super::*;
    use std::time::Duration;

    fn brief() -> SwarmBrief {
        SwarmBrief {
            task: "admission test".into(),
            base_branch: "main".into(),
            worker_branch_prefix: "swarm/admission".into(),
            worker_command: vec!["unused".into()],
            timeout: Duration::from_secs(1),
            env: vec![],
        }
    }

    #[test]
    fn aggregate_output_budget_is_fixed_across_worker_counts() {
        for workers in 1..=MAX_DISPATCH_WORKERS {
            let limits = DispatchLimits::admit(workers, 0).expect("admit bounded dispatch");
            let aggregate = workers
                .checked_mul(2)
                .and_then(|streams| streams.checked_mul(limits.worker_stream_bytes))
                .expect("bounded aggregate");
            assert!(aggregate <= MAX_DISPATCH_OUTPUT_BYTES);
        }
        assert_eq!(
            DispatchLimits::admit(1, 0).unwrap().worker_stream_bytes,
            MAX_WORKER_STREAM_BYTES
        );
        assert!(
            DispatchLimits::admit(MAX_DISPATCH_WORKERS, 0)
                .unwrap()
                .worker_stream_bytes
                < MAX_WORKER_STREAM_BYTES
        );
    }

    #[test]
    fn admission_bounds_requested_and_retained_workers() {
        assert!(DispatchLimits::admit(MAX_DISPATCH_WORKERS, 0).is_ok());
        assert!(DispatchLimits::admit(MAX_DISPATCH_WORKERS, MAX_DISPATCH_WORKERS).is_ok());
        assert!(matches!(
            DispatchLimits::admit(MAX_DISPATCH_WORKERS + 1, 0),
            Err(SwarmError::DispatchAdmission(_))
        ));
        assert!(matches!(
            DispatchLimits::admit(1, MAX_RETAINED_WORKTREES),
            Err(SwarmError::DispatchAdmission(_))
        ));
        assert!(matches!(
            DispatchLimits::admit(usize::MAX, 1),
            Err(SwarmError::DispatchAdmission(_))
        ));
    }

    #[tokio::test]
    async fn count_cap_fails_before_git_or_worker_side_effects() {
        let repo = tempfile::tempdir().unwrap();
        let swarm = Swarm::new(repo.path()).unwrap();
        let error = swarm
            .dispatch(brief(), MAX_DISPATCH_WORKERS + 1)
            .await
            .unwrap_err();
        assert!(matches!(error, SwarmError::DispatchAdmission(_)));
        assert_eq!(swarm.manager.retained_worker_count(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn retained_worktrees_consume_admission_slots() {
        let repo = tempfile::tempdir().unwrap();
        let swarm = Swarm::new(repo.path()).unwrap();
        for index in 0..MAX_RETAINED_WORKTREES {
            std::fs::create_dir(swarm.manager.swarm_root().join(format!("worker-{index}")))
                .unwrap();
        }
        let error = swarm.dispatch(brief(), 1).await.unwrap_err();
        assert!(matches!(error, SwarmError::DispatchAdmission(_)));
    }

    #[tokio::test]
    async fn overlapping_dispatch_across_same_repo_instances_fails_closed() {
        let repo = tempfile::tempdir().unwrap();
        let first = Swarm::new(repo.path()).unwrap();
        let second = Swarm::new(repo.path()).unwrap();
        assert!(Arc::ptr_eq(&first.dispatch_gate, &second.dispatch_gate));

        let _active = first.dispatch_gate.try_lock().unwrap();
        let error = second.dispatch(brief(), 1).await.unwrap_err();
        assert!(matches!(error, SwarmError::DispatchAdmission(_)));
        assert_eq!(second.manager.retained_worker_count(0).unwrap(), 0);
    }
}
