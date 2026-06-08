//! M5.2 — session trace replay + diff for wayland-core.
//!
//! Three primitives that close the "what did the session actually do?"
//! debugging loop:
//!
//! - [`Trace`] / [`TraceEvent`] — agent-flow events serialized to JSON
//! - [`Replayer`] — load + version-skew-guarded dry-run of a trace
//! - [`Differ`] — side-by-side compare of two traces with
//!   [`DiffKind::Added`] / [`Removed`] / [`Changed`] / [`Unchanged`]
//!
//! In-process LLM rehydration (re-running a trace against a real
//! provider) is intentionally out of scope for v0.6 — that surface lives
//! in `wcore-agent` and is gated behind a future feature flag. The value
//! this crate ships is the schema + the version-skew guard + the diff
//! entrypoint, which is the load-bearing piece for debugging.

pub mod diff;
pub mod error;
pub mod trace;

pub use diff::{DiffEntry, DiffKind, Differ};
pub use error::{ReplayError, Result};
pub use trace::{Trace, TraceEvent};

/// Loads + version-checks a trace before exposing its events for
/// further processing. The version-skew guard refuses to replay a
/// trace recorded by a different `wcore-core` build unless the caller
/// explicitly opts in via [`Replayer::force_version_skew`].
pub struct Replayer {
    pub force_version_skew: bool,
}

impl Replayer {
    pub fn new() -> Self {
        Self {
            force_version_skew: false,
        }
    }

    /// Echo the event stream after the version-skew gate. In v0.6 this
    /// is the identity operation — the value is in the gate + the load.
    /// `wcore-agent` will consume this surface later for live rehydration.
    pub fn dry_run(&self, trace: &Trace, runtime_version: &str) -> Result<Vec<TraceEvent>> {
        if trace.wcore_version != runtime_version && !self.force_version_skew {
            return Err(ReplayError::VersionSkew {
                trace: trace.wcore_version.clone(),
                runtime: runtime_version.to_string(),
            });
        }
        Ok(trace.events.clone())
    }
}

impl Default for Replayer {
    fn default() -> Self {
        Self::new()
    }
}
