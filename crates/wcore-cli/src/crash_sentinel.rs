//! T1-E2: Dirty-death flag for crash detection.
//!
//! Writes a file at `$WAYLAND_HOME/.dirty-death` on startup and removes it on
//! clean shutdown via `Drop`. If the flag is present at next startup, the
//! previous run crashed or was killed without unwinding — surface a warning so
//! observability/telemetry can correlate. During a panic the `Drop` guard
//! intentionally leaves the flag behind so the next run detects the unclean
//! exit.
//!
//! Source pattern: Forge Apache-2.0 `SessionCheckpointService.ts` (dirty-death
//! flag write-on-start / clear-on-clean-exit). The Forge version also persists
//! a checkpoint payload + history; we lift only the flag mechanic here. The
//! richer checkpoint payload is out of scope for T1-E2.

use std::path::{Path, PathBuf};

/// Filename of the flag written under `$WAYLAND_HOME`.
const FLAG_FILE: &str = ".dirty-death";

/// Environment variable that overrides the default wayland home directory.
const WAYLAND_HOME_ENV: &str = "WAYLAND_HOME";

/// Subdirectory of `$HOME` used when `WAYLAND_HOME` is unset.
const WAYLAND_HOME_DIRNAME: &str = ".wayland";

/// RAII guard for the dirty-death flag. Holding a `CrashSentinel` means the
/// flag is on disk. Dropping it (cleanly, not during a panic) removes the flag.
pub struct CrashSentinel {
    flag_path: PathBuf,
    armed: bool,
}

impl CrashSentinel {
    /// Resolve the default flag path, honoring `WAYLAND_HOME` when set, else
    /// `$HOME/.wayland/.dirty-death`, with a final fallback to `./.wayland/`
    /// when no home directory can be determined.
    pub fn default_path() -> PathBuf {
        let home = std::env::var_os(WAYLAND_HOME_ENV)
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(WAYLAND_HOME_DIRNAME)))
            .unwrap_or_else(|| PathBuf::from("./.wayland"));
        home.join(FLAG_FILE)
    }

    /// Write the flag. Returns whether the flag was already present from a
    /// prior incomplete shutdown.
    ///
    /// This is split out from [`new`] so callers can probe + warn before
    /// constructing the RAII guard.
    ///
    /// Most callers should NOT use this directly — use [`check_dirty`] to
    /// probe + [`new`] to arm, avoiding the double-write described in R2
    /// fix A5. `arm` remains public for legacy callers and test code.
    pub fn arm(flag_path: &Path) -> std::io::Result<bool> {
        let was_dirty = flag_path.exists();
        if let Some(parent) = flag_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(flag_path, b"armed")?;
        Ok(was_dirty)
    }

    /// Side-effect-free probe: returns whether the flag is already on
    /// disk (left behind by a prior dirty death). Use this BEFORE
    /// constructing a sentinel via [`new`] so the construction call
    /// performs a single write.
    ///
    /// R2 fix A5 — previously callers paired `arm()` (writes + reports
    /// dirty) with `new()` (writes again), resulting in two `fs::write`
    /// calls per startup and a guard that could end up None on the
    /// second-write failure even though the first succeeded.
    pub fn check_dirty(flag_path: &Path) -> bool {
        flag_path.exists()
    }

    /// Construct a guard that owns the flag at `flag_path`. Writes the flag
    /// as a side-effect; the previous-run dirtiness signal is discarded here
    /// (callers who need it should use [`check_dirty`] first).
    pub fn new(flag_path: PathBuf) -> std::io::Result<Self> {
        let _was_dirty = Self::arm(&flag_path)?;
        Ok(Self {
            flag_path,
            armed: true,
        })
    }

    /// Explicitly mark a clean shutdown. After this, [`Drop`] is a no-op.
    /// Idempotent: safe to call twice.
    pub fn disarm(&mut self) -> std::io::Result<()> {
        if self.armed && self.flag_path.exists() {
            std::fs::remove_file(&self.flag_path)?;
        }
        self.armed = false;
        Ok(())
    }

    /// Path of the flag file this sentinel owns.
    #[allow(dead_code)] // exposed for diagnostics / future telemetry wiring
    pub fn flag_path(&self) -> &Path {
        &self.flag_path
    }
}

impl Drop for CrashSentinel {
    fn drop(&mut self) {
        // If Drop fires because the stack is unwinding from a panic, leave the
        // flag behind so the next run can detect the unclean exit. Any other
        // Drop path (clean fall-through, explicit `disarm()` already called)
        // removes the flag best-effort.
        if std::thread::panicking() {
            return;
        }
        let _ = self.disarm();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn flag_in(dir: &TempDir) -> PathBuf {
        dir.path().join("subdir").join(FLAG_FILE)
    }

    #[test]
    fn arm_creates_flag_in_fresh_dir() {
        let dir = TempDir::new().unwrap();
        let path = flag_in(&dir);
        assert!(!path.exists(), "precondition: flag should not exist");

        let was_dirty_first = CrashSentinel::arm(&path).unwrap();
        assert!(!was_dirty_first, "first arm in fresh dir = clean");
        assert!(path.exists(), "arm should create the flag file");

        let was_dirty_second = CrashSentinel::arm(&path).unwrap();
        assert!(was_dirty_second, "second arm should report dirty");
    }

    #[test]
    fn disarm_removes_flag() {
        let dir = TempDir::new().unwrap();
        let path = flag_in(&dir);
        let mut sentinel = CrashSentinel::new(path.clone()).unwrap();
        assert!(path.exists(), "new() should arm");

        sentinel.disarm().unwrap();
        assert!(!path.exists(), "disarm should remove flag");

        // Idempotent
        sentinel.disarm().unwrap();
    }

    #[test]
    fn drop_disarms_on_clean_exit() {
        let dir = TempDir::new().unwrap();
        let path = flag_in(&dir);

        {
            let _sentinel = CrashSentinel::new(path.clone()).unwrap();
            assert!(path.exists(), "flag exists while sentinel is live");
        }

        assert!(
            !path.exists(),
            "flag should be removed when sentinel drops on clean exit"
        );
    }

    #[test]
    fn dirty_persists_across_simulated_runs() {
        let dir = TempDir::new().unwrap();
        let path = flag_in(&dir);

        // Simulate run 1: arm, then a "crash" — leak the sentinel so Drop
        // can't fire (mirrors a SIGKILL / segfault that bypasses unwinding).
        let sentinel = CrashSentinel::new(path.clone()).unwrap();
        std::mem::forget(sentinel);
        assert!(path.exists(), "flag still on disk after simulated crash");

        // Simulate run 2: probe via arm() — should report dirty.
        let was_dirty = CrashSentinel::arm(&path).unwrap();
        assert!(was_dirty, "second run must observe the dirty flag");

        // Cleanup
        std::fs::remove_file(&path).ok();
    }

    /// B3 regression guard: the TUI's normal-exit path (q key / Ctrl+C chord)
    /// explicitly calls `sentinel.disarm()` rather than relying solely on
    /// `Drop`, closing the window between TUI exit and MCP shutdown where a
    /// signal-based `process::exit` could bypass Drop and leave the flag
    /// behind. This test verifies the `disarm` → flag-gone contract that
    /// the explicit call relies on.
    #[test]
    fn explicit_disarm_before_drop_removes_flag() {
        let dir = TempDir::new().unwrap();
        let path = flag_in(&dir);

        let mut sentinel = CrashSentinel::new(path.clone()).unwrap();
        assert!(path.exists(), "flag must be on disk after arm");

        // Simulate the explicit disarm the TUI normal-exit path now calls.
        sentinel.disarm().unwrap();
        assert!(!path.exists(), "flag must be gone after explicit disarm");

        // A subsequent Drop (from going out of scope) must be a no-op —
        // not an error and not a re-creation of the flag.
        drop(sentinel);
        assert!(
            !path.exists(),
            "flag must remain absent after Drop follows explicit disarm"
        );
    }

    #[test]
    #[serial]
    fn default_path_honors_wayland_home_env() {
        let dir = TempDir::new().unwrap();
        // R3-B2: the default `#[serial]` group serializes this against every
        // other env-mutating test in the binary.
        // SAFETY: #[serial] serializes every env-mutating test in this binary.
        unsafe {
            std::env::set_var(WAYLAND_HOME_ENV, dir.path());
        }
        let resolved = CrashSentinel::default_path();
        unsafe {
            std::env::remove_var(WAYLAND_HOME_ENV);
        }

        assert_eq!(resolved, dir.path().join(FLAG_FILE));
    }
}
