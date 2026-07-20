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

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SwarmError};

/// Filename within each worker's worktree where the heartbeat lives.
pub const STATUS_FILE: &str = ".swarm-status.json";
const MAX_STATUS_BYTES: u64 = 4096;

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
    let bytes = match open_status_file(&path) {
        Ok(file) => read_status_bytes(file)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SwarmError::Io(e)),
    };
    decode_status(&bytes).map(Some)
}

/// Read a heartbeat through an already-retained checkout capability. This is
/// the orchestrator path: a worker cannot redirect it by replacing the
/// checkout pathname after admission.
pub fn read_status_authorized(
    checkout: &wcore_sandbox::DirectoryAuthority,
) -> Result<Option<WorkerStatusFile>> {
    let Some(bytes) = checkout
        .read_child_bounded(STATUS_FILE, MAX_STATUS_BYTES)
        .map_err(|error| SwarmError::WorktreeIo(format!("heartbeat authority read: {error}")))?
    else {
        return Ok(None);
    };
    decode_status(&bytes).map(Some)
}

fn decode_status(bytes: &[u8]) -> Result<WorkerStatusFile> {
    serde_json::from_slice(bytes).map_err(|e| {
        SwarmError::WorktreeIo(format!(
            "heartbeat decode: {e}; payload={:?}",
            String::from_utf8_lossy(bytes)
        ))
    })
}

fn open_status_file(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

fn read_status_bytes(file: File) -> Result<Vec<u8>> {
    let metadata = file.metadata()?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(SwarmError::WorktreeIo(
                "heartbeat file is a reparse point".to_owned(),
            ));
        }
    }
    if !metadata.is_file() {
        return Err(SwarmError::WorktreeIo(
            "heartbeat file must be a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_STATUS_BYTES {
        return Err(SwarmError::WorktreeIo(format!(
            "heartbeat file exceeds {MAX_STATUS_BYTES} bytes"
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STATUS_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATUS_BYTES {
        return Err(SwarmError::WorktreeIo(format!(
            "heartbeat file exceeds {MAX_STATUS_BYTES} bytes"
        )));
    }
    Ok(bytes)
}
