//! On-disk live-task registry.
//!
//! `wayland-core backend cancel --task-id <id>` runs in a DIFFERENT process
//! from `wayland-core backend run`, so cancellation cannot be an in-memory
//! handle. Each backend records just enough to terminate the far end — a pid,
//! a container id, a remote host plus nonce, a machine id — and removes the
//! entry when the task reaches a terminal state.
//!
//! The same file is what makes an orphan scan meaningful: an entry that
//! outlives its work IS the orphan, and plan 25-04 prosecutes exactly that.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::contract::{BackendKind, validate_identifier};
use crate::error::{ExecError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTask {
    pub task_id: String,
    pub nonce: String,
    pub backend_id: String,
    pub kind: BackendKind,
    /// Local pid of the direct child, when there is one.
    pub pid: Option<u32>,
    /// Container id / remote host / cloud machine id, per backend kind.
    pub handle: Option<String>,
    pub started_unix_ms: u64,
}

thread_local! {
    /// Per-thread override of [`state_dir`], installed by [`StateDirGuard`].
    ///
    /// Deliberately a thread-local and NOT a process global. `cargo test` runs
    /// every test of a binary on its own thread inside ONE SHARED PROCESS, so a
    /// process-global override (which is what `WAYLAND_EXEC_BACKEND_STATE_DIR`
    /// is) is visible to every other concurrently-running test. That is the
    /// exact defect this replaces: two tests each pointed the env var at their
    /// own `TempDir`, so whichever wrote last silently redirected the other's
    /// `record`/`load`/`list` calls into a directory that was about to be
    /// deleted. Injecting per-thread makes the tests independent without
    /// serializing them, and without weakening the assertions.
    static STATE_DIR_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// RAII override of [`state_dir`] scoped to the CURRENT THREAD.
///
/// Restores the previous value on drop, so nesting and early returns are safe.
/// Exposed (rather than `#[cfg(test)]`-gated) so integration tests in
/// `tests/`, which link this crate compiled WITHOUT `cfg(test)`, can use the
/// same isolation as the unit tests.
#[doc(hidden)]
pub struct StateDirGuard {
    prev: Option<PathBuf>,
}

impl StateDirGuard {
    /// Point [`state_dir`] at `dir` for this thread until the guard drops.
    pub fn set(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let prev = STATE_DIR_OVERRIDE.with(|cell| cell.replace(Some(dir)));
        Self { prev }
    }
}

impl Drop for StateDirGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        STATE_DIR_OVERRIDE.with(|cell| *cell.borrow_mut() = prev);
    }
}

/// Where live-task state lives. Overridable so tests never touch a real
/// operator's state directory.
///
/// Resolution order: the thread-local test override ([`StateDirGuard`]), then
/// the `WAYLAND_EXEC_BACKEND_STATE_DIR` operator override, then the default
/// under the Wayland config dir.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = STATE_DIR_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return dir;
    }
    if let Ok(dir) = std::env::var("WAYLAND_EXEC_BACKEND_STATE_DIR") {
        return PathBuf::from(dir);
    }
    wcore_config::config::wayland_config_dir().join("exec-backend")
}

fn tasks_dir() -> PathBuf {
    state_dir().join("tasks")
}

fn task_path(task_id: &str) -> Result<PathBuf> {
    // The task id becomes a FILENAME, so it is validated as an identifier
    // before it is ever joined onto a path. `..` and separators are refused by
    // the identifier rule, so this cannot traverse.
    validate_identifier("task_id", task_id)?;
    Ok(tasks_dir().join(format!("{task_id}.json")))
}

pub fn record(entry: &LiveTask) -> Result<()> {
    let dir = tasks_dir();
    std::fs::create_dir_all(&dir)?;
    let path = task_path(&entry.task_id)?;
    let bytes = serde_json::to_vec_pretty(entry)?;
    // Write-then-rename: a cancel racing a run must never read a half file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn load(task_id: &str) -> Result<LiveTask> {
    let path = task_path(task_id)?;
    let bytes = std::fs::read(&path).map_err(|_| ExecError::UnknownTask {
        backend_id: "<any>".into(),
        task_id: task_id.into(),
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn forget(task_id: &str) -> Result<()> {
    let path = task_path(task_id)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Every live task this host currently believes it owns.
pub fn list() -> Vec<LiveTask> {
    read_all(&tasks_dir())
}

fn read_all(dir: &Path) -> Vec<LiveTask> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(task) = serde_json::from_slice::<LiveTask>(&bytes)
        {
            out.push(task);
        }
    }
    out
}

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_state<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().expect("tempdir");
        // Per-thread injection, NOT a process-global env var. Under `cargo
        // test` all tests of this binary share one process, so the previous
        // `set_var` leaked into every concurrently-running test. `dir` is held
        // for the whole of `f()`, so the directory cannot be deleted while the
        // override still points at it.
        let _guard = StateDirGuard::set(dir.path());
        f()
    }

    fn entry(id: &str) -> LiveTask {
        LiveTask {
            task_id: id.into(),
            nonce: "n-1".into(),
            backend_id: "local".into(),
            kind: BackendKind::Local,
            pid: Some(4242),
            handle: None,
            started_unix_ms: 1,
        }
    }

    #[test]
    fn a_recorded_task_is_readable_by_another_caller_and_removable() {
        with_temp_state(|| {
            record(&entry("t-1")).unwrap();
            let loaded = load("t-1").unwrap();
            assert_eq!(loaded.pid, Some(4242));
            assert_eq!(list().len(), 1);
            forget("t-1").unwrap();
            assert!(load("t-1").is_err());
            assert!(list().is_empty());
        });
    }

    #[test]
    fn a_task_id_cannot_traverse_out_of_the_state_directory() {
        with_temp_state(|| {
            assert!(task_path("../../etc/passwd").is_err());
            assert!(task_path("a/b").is_err());
            assert!(load("../../etc/passwd").is_err());
        });
    }
}
