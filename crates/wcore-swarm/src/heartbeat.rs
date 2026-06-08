//! Minimal heartbeat — hung-worker detection mechanism.
//!
//! Each worker is invited (but not required) to write
//! `<worktree>/.swarm-status.json` every ~5 seconds while running. The
//! orchestrator polls it via [`crate::Swarm::worker_status`] to detect
//! stalled workers WITHOUT consuming final stdout/stderr (those are still
//! only available after [`crate::Swarm::collect`]).
//!
//! This is NOT live stdout streaming. Workers that never write a status
//! file always read back `Ok(None)` from `worker_status` — that's fine;
//! the orchestrator falls back to a "no heartbeat yet" interpretation.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SwarmError};

/// Filename within each worker's worktree where the heartbeat lives.
pub const STATUS_FILE: &str = ".swarm-status.json";

/// Wire-format heartbeat payload. Workers write this; orchestrator reads
/// it. `last_alive_at` is unix-epoch milliseconds. `step` is a free-form
/// human-readable label the worker may set (e.g. `"running tests"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStatusFile {
    pub last_alive_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

/// Helper for the worker side. The worker owns the worktree path and
/// decides when to call `write` (idiom: on entry into a new step, plus a
/// background ~5s tick).
pub struct HeartbeatWriter {
    path: PathBuf,
}

impl HeartbeatWriter {
    /// Build a writer that targets `<worktree>/<STATUS_FILE>`.
    pub fn new(worktree: &Path) -> Self {
        Self {
            path: worktree.join(STATUS_FILE),
        }
    }

    /// Write a heartbeat with the current wall-clock time and an optional
    /// step label. The write is atomic-ish: we write the file in place
    /// (small payload, single fs write) — partial reads return a serde
    /// error to the orchestrator, which interprets that as "no current
    /// heartbeat" (same as missing file).
    pub fn write(&self, step: Option<&str>) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| SwarmError::WorktreeIo(format!("clock: {e}")))?
            .as_millis() as u64;
        let payload = WorkerStatusFile {
            last_alive_at: now,
            step: step.map(str::to_owned),
        };
        let json = serde_json::to_string(&payload)
            .map_err(|e| SwarmError::WorktreeIo(format!("heartbeat encode: {e}")))?;
        wcore_config::atomic_write(&self.path, json.as_bytes()).map_err(SwarmError::Io)
    }

    /// Heartbeat file path (mainly useful for tests).
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Orchestrator-side accessor. Returns `Ok(None)` when the file does not
/// exist yet (worker hasn't written, or doesn't write at all), `Ok(Some)`
/// once a valid payload has been written. A malformed file is surfaced
/// as `Err(WorktreeIo(...))` so callers can distinguish "no heartbeat"
/// from "corrupt heartbeat".
pub fn read_status(worktree: &Path) -> Result<Option<WorkerStatusFile>> {
    let path = worktree.join(STATUS_FILE);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SwarmError::Io(e)),
    };
    let payload: WorkerStatusFile = serde_json::from_slice(&bytes)
        .map_err(|e| SwarmError::WorktreeIo(format!("heartbeat decode: {e}")))?;
    Ok(Some(payload))
}
