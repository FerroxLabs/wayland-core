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
//! #181 audit hardening: the flag file records its owner's OS process START
//! TIME. At scan, a pid-alive flag whose recorded start time matches the OS
//! start time is verified-live and never touched; a mismatch means the pid
//! was recycled and the original owner died dirty (report + reap). When a
//! start time cannot be determined the scan is conservative: the flag is
//! never treated as a crash and is only reaped silently once older than 30
//! days. Deleting a flag is the CLAIM on reporting it — concurrent starters
//! race on the delete, so one incident warns exactly once (TOCTOU fix).
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

/// Tolerance when comparing a flag's recorded owner start time against the
/// OS-reported start time for the same pid. Both sides are computed by the
/// same `sysinfo` probe so they should match exactly; ±2s absorbs any
/// rounding while remaining far below any plausible pid-recycle interval.
const START_TIME_TOLERANCE_SECS: u64 = 2;

/// Age past which a pid-alive flag whose ownership CANNOT be verified (no
/// readable start time on either side) is silently reaped. Conservative
/// hygiene only — such a flag is never reported as a crash and is never
/// evicted while fresh, so a genuinely-running engine's later crash report
/// is never destroyed (#181 audit).
const STALE_UNVERIFIED_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(30 * 24 * 60 * 60);

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
    /// dirty death — per-pid flags whose owner is verifiably gone (pid dead,
    /// or pid recycled per the recorded start time), plus the legacy
    /// un-scoped `.dirty-death` (one-time migration). Every returned file
    /// was deleted (reaped) by this scan — the delete is the CLAIM, so when
    /// concurrent starters race on the same flag exactly one reports it.
    /// Flags whose owner is verified-live (pid alive AND recorded start time
    /// matches the OS) are never touched — a running sibling engine is not a
    /// crash. Flags whose ownership cannot be verified are never reported
    /// and only silently reaped once older than 30 days.
    ///
    /// Scanning only the resolved `WAYLAND_HOME` directory inherently limits
    /// the report to sentinels of this same profile.
    pub fn scan_dead_sentinels(dir: &Path) -> Vec<PathBuf> {
        Self::scan_dead_sentinels_with(dir, crate::cron::process_is_alive, process_start_time)
    }

    /// Inner scan with injectable liveness + start-time probes (unit tests
    /// exercise pid-reuse and eviction rules without real process churn).
    fn scan_dead_sentinels_with(
        dir: &Path,
        is_alive: impl Fn(u32) -> bool,
        start_time_of: impl Fn(u32) -> Option<u64>,
    ) -> Vec<PathBuf> {
        let mut dirty = Vec::new();

        // Migration: a legacy un-scoped flag left by a pre-#181 build is
        // reported once and deleted so it can never fire again. The delete
        // is the claim — if a concurrent starter already removed it, that
        // starter owns the report.
        let legacy = dir.join(FLAG_FILE);
        if legacy.is_file() && claim_flag(&legacy) {
            dirty.push(legacy);
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            return dirty;
        };
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

            if !is_alive(pid) {
                // Owning process is gone but its flag survived: dirty death.
                // Claim (delete) before reporting so it fires exactly once
                // even when siblings scan concurrently.
                if claim_flag(&path) {
                    dirty.push(path);
                }
                continue;
            }

            // Pid is alive — verify it is the ORIGINAL owner and not a
            // recycled pid, via the start time recorded at arm().
            let recorded: Option<u64> = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse().ok());
            match (recorded, start_time_of(pid)) {
                (Some(rec), Some(os)) if rec.abs_diff(os) <= START_TIME_TOLERANCE_SECS => {
                    // Verified-live: the owner is genuinely running. Never
                    // touched, never evicted — its own clean exit or a later
                    // dirty-death scan will handle this flag.
                }
                (Some(_), Some(_)) => {
                    // Start-time mismatch: the pid was recycled by another
                    // process, so the original owner died dirty.
                    if claim_flag(&path) {
                        dirty.push(path);
                    }
                }
                _ => {
                    // Ownership indeterminable on this platform/flag (legacy
                    // "armed" contents, unreadable file, or no start time
                    // for the pid). Be conservative: never a crash report,
                    // never evicted while fresh — reap silently only once
                    // stale, so the directory cannot accumulate unboundedly.
                    let stale = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .map(|age| age > STALE_UNVERIFIED_MAX_AGE)
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                    }
                }
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
        // #181 audit: record the owner's OS process start time so the scan
        // can tell a genuinely-running owner from a pid-reuse orphan.
        // "armed" = start time indeterminable on this platform; the scan
        // treats such flags conservatively (never reported, never evicted
        // while fresh).
        let contents = process_start_time(std::process::id())
            .map(|t| t.to_string())
            .unwrap_or_else(|| "armed".to_string());
        std::fs::write(flag_path, contents)?;
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

/// OS start time (seconds since the epoch) of process `pid`, if it can be
/// determined. Uses `sysinfo` (already a dependency for the TUI header):
/// `/proc/<pid>/stat` on Linux, `sysctl` `kinfo_proc` on macOS,
/// `GetProcessTimes` on Windows. `None` when the process is gone or the
/// platform/permissions hide it — callers must treat `None` conservatively.
fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    let target = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        false,
        ProcessRefreshKind::nothing(),
    );
    sys.process(target).map(|p| p.start_time())
}

/// #181 audit (TOCTOU): deleting the flag IS the claim on reporting it.
/// Concurrent starters may both observe the same dirty flag; only the one
/// whose delete succeeds reports, so one incident warns exactly once. A
/// failed delete (typically `NotFound` — a sibling won the race) is skipped
/// silently; on any other error the flag survives for the next scan.
fn claim_flag(path: &Path) -> bool {
    std::fs::remove_file(path).is_ok()
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

    // -----------------------------------------------------------------
    // #181 audit: pid-reuse resistance + TOCTOU claim gating
    // -----------------------------------------------------------------

    /// Audit CRITICAL fix regression guard: verified-live flags (pid alive
    /// AND recorded start time matches the OS) are NEVER evicted, no matter
    /// how many concurrent engines exist. The old mtime-blind 20-cap evicted
    /// the oldest RUNNING engine's flag past 20 siblings, silencing its
    /// later crash.
    #[test]
    fn verified_live_flags_are_never_evicted_regardless_of_count() {
        let dir = TempDir::new().unwrap();
        let count = 25u32;
        for pid in 1..=count {
            // Recorded start time 1000 matches the injected OS start time.
            std::fs::write(pid_flag(&dir, pid), b"1000").unwrap();
        }

        let dirty = CrashSentinel::scan_dead_sentinels_with(dir.path(), |_| true, |_| Some(1000));
        assert!(dirty.is_empty(), "verified-live flags are never reported");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            count as usize,
            "all verified-live flags must survive the scan — no cap eviction"
        );
    }

    /// A pid-alive flag whose recorded start time MISMATCHES the OS start
    /// time is a pid-reuse orphan: the original owner died dirty. Report +
    /// reap.
    #[test]
    fn pid_reuse_start_time_mismatch_reports_as_dead() {
        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, 4242);
        // Owner recorded start time 1000; the process now holding pid 4242
        // started at 99999 — a different process entirely.
        std::fs::write(&path, b"1000").unwrap();

        let dirty = CrashSentinel::scan_dead_sentinels_with(dir.path(), |_| true, |_| Some(99_999));
        assert_eq!(
            dirty,
            vec![path.clone()],
            "pid-reuse orphan must be reported as a dirty death"
        );
        assert!(!path.exists(), "pid-reuse orphan flag must be reaped");
    }

    /// When ownership cannot be verified (no parseable recorded start time
    /// and/or no OS start time), the scan is conservative: never reported,
    /// never evicted while fresh, silently reaped only once stale.
    #[test]
    fn unverified_live_flag_kept_fresh_reaped_only_when_stale() {
        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, 4242);
        std::fs::write(&path, b"armed").unwrap(); // unparseable = unverified

        // Fresh: kept, not reported.
        let dirty = CrashSentinel::scan_dead_sentinels_with(dir.path(), |_| true, |_| None);
        assert!(dirty.is_empty(), "unverified flag must never be reported");
        assert!(path.exists(), "fresh unverified flag must not be evicted");

        // Backdate mtime past the stale threshold: reaped, still silent.
        let stale_mtime = std::time::SystemTime::now()
            - (STALE_UNVERIFIED_MAX_AGE + std::time::Duration::from_secs(60 * 60));
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(stale_mtime)
            .unwrap();

        let dirty = CrashSentinel::scan_dead_sentinels_with(dir.path(), |_| true, |_| None);
        assert!(dirty.is_empty(), "stale reap must be silent, not a report");
        assert!(!path.exists(), "stale unverified flag must be reaped");
    }

    /// Audit TOCTOU fix: the delete is the claim. If a concurrent sibling
    /// removes the flag between our listing and our remove_file, we must NOT
    /// also report it — one incident, one warning. Simulated by a liveness
    /// probe that deletes the file before answering "dead".
    #[test]
    fn toctou_flag_claimed_by_sibling_is_not_double_reported() {
        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, 4242);
        std::fs::write(&path, b"1000").unwrap();

        let sibling_path = path.clone();
        let dirty = CrashSentinel::scan_dead_sentinels_with(
            dir.path(),
            move |_| {
                // A concurrent starter claims (deletes + reports) the flag
                // in the window between our read_dir and our remove_file.
                std::fs::remove_file(&sibling_path).unwrap();
                false
            },
            |_| None,
        );
        assert!(
            dirty.is_empty(),
            "a flag already claimed by a sibling must not be reported again"
        );
    }

    /// Unit contract of the claim helper both scan branches (legacy + dead
    /// pid) gate on: first delete wins, second loses.
    #[test]
    fn claim_flag_first_claim_wins_second_loses() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(format!("{PID_FLAG_PREFIX}1"));
        std::fs::write(&path, b"1000").unwrap();

        assert!(claim_flag(&path), "first claim must succeed");
        assert!(!claim_flag(&path), "second claim must lose the race");
    }

    /// End-to-end with the REAL probes: a flag armed by this process records
    /// this process's actual start time, and the real scan verifies it live
    /// (kept, unreported). Also pins that `process_start_time` works for the
    /// current process on this platform.
    #[test]
    fn armed_own_flag_is_verified_live_by_real_scan() {
        let my_start = process_start_time(std::process::id());
        assert!(
            my_start.is_some(),
            "process_start_time must resolve for the current process"
        );

        let dir = TempDir::new().unwrap();
        let path = pid_flag(&dir, std::process::id());
        CrashSentinel::arm(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            my_start.unwrap().to_string(),
            "arm() must record the owner's start time in the flag"
        );

        let dirty = CrashSentinel::scan_dead_sentinels(dir.path());
        assert!(dirty.is_empty(), "own verified-live flag never reported");
        assert!(path.exists(), "own verified-live flag never evicted");
    }
}
