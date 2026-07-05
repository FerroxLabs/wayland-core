//! T1-E2: Dirty-death flag for crash detection.
//!
//! Writes a file at `$WAYLAND_HOME/.dirty-death.<pid>` on startup and removes
//! it on clean shutdown via `Drop`. If a flag whose owning process is no
//! longer alive is present at next startup, that run crashed or was killed
//! without unwinding — surface a warning so observability/telemetry can
//! correlate. During a panic the `Drop` guard intentionally leaves the flag
//! behind so the next run detects the unclean exit.
//!
//! #181: the flag is scoped PER PROCESS. The original single un-scoped
//! `.dirty-death` was shared by every concurrent engine (chat + teammates +
//! subagents + doctor), so any sibling exiting uncleanly made every other
//! launch report "previous run did not shut down cleanly". Startup now scans
//! for `.dirty-death.<pid>` files, reports only those whose pid is dead,
//! reaps them after reporting, and migrates (report + delete) the legacy
//! un-scoped file once.
//!
//! Source pattern: Forge Apache-2.0 `SessionCheckpointService.ts` (dirty-death
//! flag write-on-start / clear-on-clean-exit). The Forge version also persists
//! a checkpoint payload + history; we lift only the flag mechanic here. The
//! richer checkpoint payload is out of scope for T1-E2.

use std::path::{Path, PathBuf};

/// Filename of the legacy un-scoped flag written under `$WAYLAND_HOME` by
/// builds before per-process scoping (#181). Read once at startup for
/// migration (report + delete), never written again.
const FLAG_FILE: &str = ".dirty-death";

/// Per-process flags are named `.dirty-death.<pid>` (#181).
const PID_FLAG_PREFIX: &str = ".dirty-death.";

/// Hard cap on sentinel files left behind after a startup scan. Pid reuse can
/// make an orphaned flag look "live" forever; the scan silently reaps the
/// oldest files beyond this cap so the directory can never accumulate
/// unboundedly (#181).
const MAX_SENTINEL_FILES: usize = 20;

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
    /// Resolve the directory sentinel flags live in, honoring `WAYLAND_HOME`
    /// when set, else `$HOME/.wayland`, with a final fallback to
    /// `./.wayland` when no home directory can be determined.
    pub fn default_dir() -> PathBuf {
        std::env::var_os(WAYLAND_HOME_ENV)
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(WAYLAND_HOME_DIRNAME)))
            .unwrap_or_else(|| PathBuf::from("./.wayland"))
    }

    /// Resolve THIS process's flag path:
    /// `<default_dir>/.dirty-death.<pid>` (#181 per-process scoping).
    pub fn default_path() -> PathBuf {
        Self::default_dir().join(format!("{PID_FLAG_PREFIX}{}", std::process::id()))
    }

    /// #181 startup scan: return the sentinel files in `dir` that signal a
    /// dirty death — per-pid flags whose owning process is no longer alive,
    /// plus the legacy un-scoped `.dirty-death` (one-time migration). Every
    /// returned file is deleted (reaped) before returning so it reports
    /// exactly once. Flags owned by LIVE sibling processes are left alone
    /// and NOT reported — a running sibling engine is not a crash.
    ///
    /// Scanning only the resolved `WAYLAND_HOME` directory inherently limits
    /// the report to sentinels of this same profile.
    pub fn scan_dead_sentinels(dir: &Path) -> Vec<PathBuf> {
        Self::scan_dead_sentinels_with(dir, crate::cron::process_is_alive)
    }

    /// Inner scan with an injectable liveness probe (unit tests exercise the
    /// cap without needing 20+ real live processes).
    fn scan_dead_sentinels_with(dir: &Path, is_alive: impl Fn(u32) -> bool) -> Vec<PathBuf> {
        let mut dirty = Vec::new();

        // Migration: a legacy un-scoped flag left by a pre-#181 build is
        // reported once and deleted so it can never fire again.
        let legacy = dir.join(FLAG_FILE);
        if legacy.is_file() {
            let _ = std::fs::remove_file(&legacy);
            dirty.push(legacy);
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            return dirty;
        };
        let mut live: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(pid_str) = name.strip_prefix(PID_FLAG_PREFIX) else {
                continue;
            };
            let Ok(pid) = pid_str.parse::<u32>() else {
                continue;
            };
            if is_alive(pid) {
                // A live sibling engine (or this process) owns this flag.
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                live.push((mtime, path));
            } else {
                // Owning process is gone but its flag survived: dirty death.
                // Reap after recording so it fires exactly once.
                let _ = std::fs::remove_file(&path);
                dirty.push(path);
            }
        }

        // Cap: pid reuse can make an orphaned flag pass the liveness probe
        // forever. Silently reap the oldest live-looking flags beyond the
        // cap so the directory never accumulates unboundedly.
        if live.len() > MAX_SENTINEL_FILES {
            live.sort_by_key(|(mtime, _)| *mtime);
            for (_, path) in live.drain(..live.len() - MAX_SENTINEL_FILES) {
                let _ = std::fs::remove_file(&path);
            }
        }

        dirty
    }

    /// Write the flag. Returns whether the flag was already present from a
    /// prior incomplete shutdown.
    ///
    /// This is split out from [`Self::new`] so callers can probe + warn
    /// before constructing the RAII guard.
    ///
    /// Most callers should NOT use this directly — use
    /// [`Self::scan_dead_sentinels`] to probe + [`Self::new`] to arm.
    /// `arm` remains public for legacy callers and test code.
    pub fn arm(flag_path: &Path) -> std::io::Result<bool> {
        let was_dirty = flag_path.exists();
        if let Some(parent) = flag_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(flag_path, b"armed")?;
        Ok(was_dirty)
    }

    /// Construct a guard that owns the flag at `flag_path`. Writes the flag
    /// as a side-effect; the previous-run dirtiness signal is discarded here
    /// (callers who need it should use [`Self::scan_dead_sentinels`] first).
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
    #[serial(env)]
    fn default_path_honors_wayland_home_env() {
        let dir = TempDir::new().unwrap();
        // R3-B2: gated under `serial_test`'s `env` group to prevent
        // racing with any other env-mutating test in the binary.
        // SAFETY: `set_var` is unsafe on 1.83+ per the new contract; this is a
        // test-only single-threaded usage (enforced by `#[serial(env)]`).
        unsafe {
            std::env::set_var(WAYLAND_HOME_ENV, dir.path());
        }
        let resolved = CrashSentinel::default_path();
        unsafe {
            std::env::remove_var(WAYLAND_HOME_ENV);
        }

        assert_eq!(
            resolved,
            dir.path()
                .join(format!("{PID_FLAG_PREFIX}{}", std::process::id())),
            "default_path must be scoped to this process's pid (#181)"
        );
    }

    // -----------------------------------------------------------------
    // #181 per-process scoping tests
    // -----------------------------------------------------------------

    /// Path of a per-pid flag for `pid` inside `dir`.
    fn pid_flag(dir: &TempDir, pid: u32) -> PathBuf {
        dir.path().join(format!("{PID_FLAG_PREFIX}{pid}"))
    }

    /// Spawn a trivial child and wait for it, returning a pid that is
    /// guaranteed dead (and reaped) at return time.
    fn dead_pid() -> u32 {
        #[cfg(unix)]
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("spawn `true`");
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .expect("spawn `cmd /C exit 0`");
        let pid = child.id();
        child.wait().expect("wait child");
        pid
    }

    #[test]
    fn own_pid_clean_exit_leaves_no_sentinel_and_scan_is_clean() {
        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, std::process::id());

        {
            let _sentinel = CrashSentinel::new(path.clone()).unwrap();
            assert!(path.exists(), "own flag on disk while sentinel is live");
        }

        assert!(!path.exists(), "own flag removed on clean exit");
        assert!(
            CrashSentinel::scan_dead_sentinels(dir.path()).is_empty(),
            "scan after a clean exit must report nothing"
        );
    }

    #[test]
    fn dead_sibling_pid_file_reports_once_then_reaped() {
        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, dead_pid());
        std::fs::write(&path, b"armed").unwrap();

        let dirty = CrashSentinel::scan_dead_sentinels(dir.path());
        assert_eq!(dirty, vec![path.clone()], "dead sibling must be reported");
        assert!(!path.exists(), "dead sibling flag must be reaped");

        assert!(
            CrashSentinel::scan_dead_sentinels(dir.path()).is_empty(),
            "second scan must be clean — a dead sibling fires exactly once"
        );
    }

    #[test]
    fn live_sibling_pid_file_does_not_report() {
        let dir = TempDir::new().unwrap();
        // The current test process is a guaranteed-alive "sibling".
        let path = pid_flag(&dir, std::process::id());
        std::fs::write(&path, b"armed").unwrap();

        let dirty = CrashSentinel::scan_dead_sentinels(dir.path());
        assert!(
            dirty.is_empty(),
            "a live sibling engine's flag is not a crash and must not be reported"
        );
        assert!(path.exists(), "live sibling flag must be left alone");
    }

    #[test]
    fn legacy_unscoped_flag_migrates_report_then_delete() {
        let dir = TempDir::new().unwrap();
        let legacy = dir.path().join(FLAG_FILE);
        std::fs::write(&legacy, b"armed").unwrap();

        let dirty = CrashSentinel::scan_dead_sentinels(dir.path());
        assert_eq!(
            dirty,
            vec![legacy.clone()],
            "legacy un-scoped flag must be reported once for migration"
        );
        assert!(!legacy.exists(), "legacy flag must be deleted after report");

        assert!(
            CrashSentinel::scan_dead_sentinels(dir.path()).is_empty(),
            "legacy flag must never fire a second time"
        );
    }

    #[test]
    fn live_looking_sentinels_capped_at_max() {
        let dir = TempDir::new().unwrap();
        for pid in 1..=(MAX_SENTINEL_FILES as u32 + 5) {
            std::fs::write(pid_flag(&dir, pid), b"armed").unwrap();
        }

        // Force every pid to look alive so only the cap can reap.
        let dirty = CrashSentinel::scan_dead_sentinels_with(dir.path(), |_| true);
        assert!(dirty.is_empty(), "live-looking flags are never reported");

        let remaining = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(
            remaining, MAX_SENTINEL_FILES,
            "scan must reap oldest flags beyond the accumulation cap"
        );
    }
}
