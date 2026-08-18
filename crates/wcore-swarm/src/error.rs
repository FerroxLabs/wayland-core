//! Error type for wcore-swarm.
//!
//! Surface is SPEC-LOCKED per M5.5. M5.6 (consensus) + M5.7 (memory
//! propagation) match against these variants — do not rename or remove
//! without updating the downstream dispatch briefs.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwarmError {
    /// Refused before creating worktrees or starting worker processes because
    /// the request would exceed the dispatch resource envelope.
    #[error("dispatch admission refused: {0}")]
    DispatchAdmission(String),

    /// Refused to dispatch because the base checkout has uncommitted changes.
    /// The string is the raw `git status --porcelain` output.
    #[error("dirty checkout — refused dispatch: {0}")]
    DirtyCheckout(String),

    /// Failure while creating or removing a worker worktree (git subprocess
    /// error, filesystem error wrapped as a message).
    #[error("worktree io: {0}")]
    WorktreeIo(String),

    /// Failure while spawning a worker subprocess (executable not found,
    /// permission error, etc.).
    #[error("worker spawn: {0}")]
    WorkerSpawn(String),

    /// Failure while collecting results for a specific worker.
    #[error("collect: worker {worker_id} {reason}")]
    Collect { worker_id: String, reason: String },

    /// Failure during cleanup of the swarm-worktrees directory.
    #[error("cleanup: {0}")]
    Cleanup(String),

    /// Generic IO error (filesystem operations on `.swarm-worktrees`).
    #[error("io: {0}")]
    Io(std::io::Error),

    /// Failure inside the Auto-Mode audit trail (sqlite open / record /
    /// query). T3-2. Carries the human-readable diagnostic; callers route
    /// it into the engine's telemetry rather than dropping silently.
    #[error("audit: {0}")]
    Audit(String),
}

pub type Result<T> = std::result::Result<T, SwarmError>;

// gh#1025 INSTRUMENTATION (temporary, lane-only). `#[from]` was replaced with
// this hand-written impl purely so the ONE funnel every bare `?` on an
// `io::Error` passes through can print where it came from. The returned value
// is byte-for-byte what `#[from]` produced, so no control flow changes and the
// race cannot move under observation. Gated on an env var so it is inert unless
// the repro workflow asks for it.
impl From<std::io::Error> for SwarmError {
    fn from(error: std::io::Error) -> Self {
        if std::env::var_os("WCORE_GH1025_TRACE").is_some() {
            eprintln!(
                "gh1025 bare-io kind={:?} err={error}\n{}",
                error.kind(),
                std::backtrace::Backtrace::force_capture()
            );
        }
        SwarmError::Io(error)
    }
}
