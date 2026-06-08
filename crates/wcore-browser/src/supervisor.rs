//! `BrowserSupervisor` — lifecycle + orphan reaper for backend processes.
//!
//! Wave BR ships the real implementation. Responsibilities:
//!
//!   * **Launch:** `launch_camoufox` spawns the sidecar binary with
//!     `kill_on_drop(true)` so a panic in the host kills the child.
//!   * **PID tracking:** `register` records the live child + parent PID.
//!   * **Healthcheck:** `healthcheck` issues `GET /health` and returns
//!     `Ok(true)` on 2xx.
//!   * **Orphan reaper:** `start_reaper` spawns a tokio task that polls at
//!     [`SupervisorConfig::reaper_interval`] cadence. Each tick checks the
//!     recorded parent-PID via [`process_alive`] — when the parent dies
//!     (host crashed without running drop), the supervisor SIGTERMs the
//!     tracked child so it doesn't loiter as a zombie.
//!   * **Shutdown:** `on_session_end` SIGTERMs the matching child + drops
//!     the tracking entry.
//!
//! Cross-platform PID watching uses `kill(pid, 0)` semantics:
//!   * Unix: send signal 0 via `libc::kill` → returns 0 if process exists.
//!   * Windows: open the process handle via `OpenProcess`; alive iff handle
//!     non-null. (At the moment we delegate to a simpler approach using
//!     `std::process::Command` w/ `tasklist /FI` — see [`process_alive`].)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub pid_dir: PathBuf,
    /// Reaper polling interval. Default 1Hz; tests use 100ms.
    pub reaper_interval: Duration,
    /// Healthcheck interval. Default 30s.
    pub healthcheck_interval: Duration,
    /// HTTP healthcheck endpoint (Camoufox sidecar `/health`).
    pub healthcheck_url: String,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            pid_dir: home_pid_dir(),
            reaper_interval: Duration::from_secs(1),
            healthcheck_interval: Duration::from_secs(30),
            healthcheck_url: "http://localhost:9377/health".to_string(),
        }
    }
}

fn home_pid_dir() -> PathBuf {
    if let Some(home) = dirs_home() {
        home.join(".wayland-core").join("browser").join("pids")
    } else {
        std::env::temp_dir()
            .join("wayland-core")
            .join("browser")
            .join("pids")
    }
}

fn dirs_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("HOME") {
        return Some(PathBuf::from(h));
    }
    if let Some(h) = std::env::var_os("USERPROFILE") {
        return Some(PathBuf::from(h));
    }
    None
}

/// Tracked backend handle — session id + child PID + parent (host) PID. The
/// reaper SIGTERMs the child when the parent process dies.
#[derive(Debug, Clone)]
pub struct BackendHandle {
    pub session_id: String,
    pub pid: u32,
    pub parent_pid: u32,
}

#[derive(Default)]
pub struct BrowserSupervisor {
    config: SupervisorConfig,
    /// Live sessions tracked by this supervisor. Used by `on_session_end`
    /// to SIGTERM the matching backend and by the reaper to find orphans.
    sessions: Arc<Mutex<Vec<BackendHandle>>>,
    /// Cancellation handle for the reaper task (when started). The handle
    /// is dropped on `Drop` so an unstarted supervisor leaks nothing.
    reaper_cancel: Mutex<Option<CancellationToken>>,
}

impl BrowserSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: SupervisorConfig) -> Self {
        Self {
            config,
            sessions: Arc::new(Mutex::new(Vec::new())),
            reaper_cancel: Mutex::new(None),
        }
    }

    /// Record a backend handle. Writes a PID-file under `config.pid_dir` so a
    /// post-crash recovery can find orphans on the next boot.
    pub fn register(&self, handle: BackendHandle) {
        let _ = std::fs::create_dir_all(&self.config.pid_dir);
        let pid_path = self
            .config
            .pid_dir
            .join(format!("{}.pid", handle.session_id));
        let body = format!("{}\n{}\n", handle.pid, handle.parent_pid);
        // Best-effort: failure to persist is not fatal (we still track in-memory).
        // Ephemeral pid file — plain write is fine; loss on crash is acceptable.
        let _ = std::fs::write(&pid_path, body);
        self.sessions.lock().push(handle);
    }

    /// Close the backend for a given session. SIGTERMs the child and drops
    /// the in-memory + on-disk tracking entries. Returns `true` if the
    /// session was known.
    pub fn on_session_end(&self, session_id: &str) -> bool {
        let mut guard = self.sessions.lock();
        let mut removed: Option<BackendHandle> = None;
        guard.retain(|h| {
            if h.session_id == session_id {
                removed = Some(h.clone());
                false
            } else {
                true
            }
        });
        drop(guard);
        if let Some(h) = removed {
            terminate_pid(h.pid);
            let pid_path = self.config.pid_dir.join(format!("{session_id}.pid"));
            let _ = std::fs::remove_file(&pid_path);
            true
        } else {
            false
        }
    }

    pub fn live_sessions(&self) -> Vec<BackendHandle> {
        self.sessions.lock().clone()
    }

    pub fn pid_dir(&self) -> &std::path::Path {
        &self.config.pid_dir
    }

    /// Start the orphan reaper as a background tokio task. Returns the
    /// cancellation token so callers can stop it explicitly.
    ///
    /// The reaper polls at `config.reaper_interval` cadence. Each tick:
    ///   1. Snapshot the tracked handles.
    ///   2. For each handle, check `process_alive(parent_pid)`.
    ///   3. If the parent is dead, SIGTERM the child + remove the entry.
    pub fn start_reaper(self: &Arc<Self>) -> CancellationToken {
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let interval = self.config.reaper_interval;
        let sessions = Arc::clone(&self.sessions);
        let pid_dir = self.config.pid_dir.clone();
        *self.reaper_cancel.lock() = Some(cancel.clone());
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = cancel_for_task.cancelled() => break,
                    _ = ticker.tick() => {
                        let snapshot: Vec<BackendHandle> = sessions.lock().clone();
                        let mut orphan_sessions: Vec<String> = Vec::new();
                        for h in &snapshot {
                            if !process_alive(h.parent_pid) {
                                terminate_pid(h.pid);
                                orphan_sessions.push(h.session_id.clone());
                            }
                        }
                        if !orphan_sessions.is_empty() {
                            let mut guard = sessions.lock();
                            guard.retain(|h| !orphan_sessions.contains(&h.session_id));
                            drop(guard);
                            for sid in &orphan_sessions {
                                let p = pid_dir.join(format!("{sid}.pid"));
                                let _ = std::fs::remove_file(&p);
                            }
                        }
                    }
                }
            }
        });
        cancel
    }

    /// HTTP healthcheck against `config.healthcheck_url`. Returns `Ok(true)`
    /// when a 2xx response is observed within `timeout`.
    pub async fn healthcheck(&self, timeout: Duration) -> Result<bool, String> {
        let client = wcore_egress::EgressClient::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| e.to_string())?;
        match client.get(&self.config.healthcheck_url).send().await {
            Ok(r) => Ok(r.status().is_success()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Launch the Camoufox sidecar binary at `path`. The child is spawned
    /// with `kill_on_drop(true)` and tracked via `register`. The returned
    /// child can be retained by the caller for `wait()` semantics, or
    /// dropped — in which case the kill-on-drop guard fires when the
    /// supervisor drops.
    ///
    /// Args: `["--port", "9377"]` by default. Callers can override.
    pub async fn launch_camoufox(
        self: &Arc<Self>,
        binary_path: &std::path::Path,
        args: &[&str],
        session_id: impl Into<String>,
    ) -> Result<u32, String> {
        let session = session_id.into();
        let mut cmd = tokio::process::Command::new(binary_path);
        cmd.args(args).kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| format!("spawn camoufox: {e}"))?;
        let pid = child.id().ok_or_else(|| "no child PID".to_string())?;
        // Forget the child handle in-memory; kill_on_drop fired when this
        // Child struct drops, so we have to stash it (or accept SIGKILL when
        // the local goes out of scope). We stash via a static map keyed by
        // session_id so multiple sidecars can coexist.
        retain_child(&session, child);
        self.register(BackendHandle {
            session_id: session,
            pid,
            parent_pid: std::process::id(),
        });
        Ok(pid)
    }
}

impl Drop for BrowserSupervisor {
    fn drop(&mut self) {
        if let Some(c) = self.reaper_cancel.lock().take() {
            c.cancel();
        }
    }
}

/// In-process child-handle storage. We can't move the tokio Child onto the
/// `BrowserSupervisor` because Drop fires before the reaper sees the parent
/// die — we'd kill children before the orphan-reaper logic gets to run.
/// Stashing here means the child outlives the supervisor and the reaper
/// owns the SIGTERM path.
fn retain_child(session: &str, child: tokio::process::Child) {
    use parking_lot::Mutex as PM;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static CHILDREN: OnceLock<PM<HashMap<String, tokio::process::Child>>> = OnceLock::new();
    let m = CHILDREN.get_or_init(|| PM::new(HashMap::new()));
    m.lock().insert(session.to_string(), child);
}

/// Returns `true` if the process with `pid` is alive. Implementation:
///   * Unix: `kill(pid, 0)` returns 0 on success → alive.
///   * Windows: spawn `tasklist /FI "PID eq <pid>" /NH /FO CSV`; alive iff
///     output contains the pid.
#[cfg(unix)]
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // SAFETY: `libc::kill` with signal 0 is the standard liveness probe.
    // Returns 0 on success (process exists + signal could have been sent).
    // ESRCH (3) means no such process; EPERM (1) means process exists but
    // we don't have permission — treat as alive.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error();
        matches!(err.raw_os_error(), Some(libc::EPERM))
    }
}

#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use std::process::Command;
    if pid == 0 {
        return false;
    }
    // tasklist is shipped with Windows and is the safest way to probe
    // without pulling the `windows-sys` crate in just for OpenProcess.
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.contains(&pid.to_string())
        }
        Err(_) => false,
    }
}

/// Send SIGTERM to `pid`. On Windows uses `taskkill /PID <pid> /T` (no /F
/// — graceful first). Returns silently on no-such-process.
#[cfg(unix)]
fn terminate_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    // SAFETY: standard libc signal API. ESRCH is silently fine.
    let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
}

#[cfg(windows)]
fn terminate_pid(pid: u32) {
    use std::process::Command;
    if pid == 0 {
        return;
    }
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T"])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn register_and_on_session_end_drop_handle() {
        let sup = BrowserSupervisor::new();
        // Fake out-of-range PID — `on_session_end` will call
        // `terminate_pid(pid)` and we only care that the handle gets
        // dropped from the in-memory map. PID 1 looks safe on a normal
        // host (init / launchd, EPERM for unprivileged callers) but
        // inside a Docker container the test process IS root inside
        // its own PID namespace, so `kill(1, SIGTERM)` SUCCEEDS and
        // signals the container's init — which is the cargo nextest
        // runner itself, killing the whole job. Reproduced
        // deterministically at ~test #1302 in CI runs 26389443795,
        // 26391504902, 26393733929 (Linux containerized).
        // The orphan-reaper test (line ~423) already uses this
        // out-of-range pattern; mirror it here.
        sup.register(BackendHandle {
            session_id: "s1".into(),
            pid: 0x7fff_fffd,
            parent_pid: 0x7fff_fffe,
        });
        assert_eq!(sup.live_sessions().len(), 1);
        assert!(sup.on_session_end("s1"));
        assert!(sup.live_sessions().is_empty());
        // Idempotent on unknown sessions.
        assert!(!sup.on_session_end("s1"));
    }

    #[test]
    fn supervisor_default_uses_user_pid_dir() {
        let sup = BrowserSupervisor::new();
        let p = sup.pid_dir();
        let s = p.to_string_lossy();
        assert!(
            s.contains("wayland-core") && s.contains("browser") && s.contains("pids"),
            "unexpected pid dir: {s}"
        );
    }

    #[test]
    fn process_alive_detects_self_and_rejects_dead_pid() {
        let me = std::process::id();
        assert!(process_alive(me), "self process must be detected alive");
        // PID 0 is the kernel scheduler on Unix; treated as not-alive by our probe.
        assert!(!process_alive(0));
        // A wildly large PID is virtually guaranteed not to exist.
        assert!(!process_alive(0x7fff_fffe));
    }

    #[tokio::test]
    async fn healthcheck_returns_ok_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let cfg = SupervisorConfig {
            healthcheck_url: format!("{}/health", server.uri()),
            ..Default::default()
        };
        let sup = BrowserSupervisor::with_config(cfg);
        let ok = sup.healthcheck(Duration::from_millis(500)).await.unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn healthcheck_returns_false_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let cfg = SupervisorConfig {
            healthcheck_url: format!("{}/health", server.uri()),
            ..Default::default()
        };
        let sup = BrowserSupervisor::with_config(cfg);
        let ok = sup.healthcheck(Duration::from_millis(500)).await.unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn reaper_terminates_orphans_with_dead_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_millis(50),
            healthcheck_interval: Duration::from_secs(30),
            healthcheck_url: "http://unused.invalid/".into(),
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        // Register a fake handle whose parent_pid is dead (very large PID)
        // and whose child_pid is also fake (0xfffffe — would never exist).
        sup.register(BackendHandle {
            session_id: "orphan-1".into(),
            pid: 0x7fff_fffd,
            parent_pid: 0x7fff_fffe,
        });
        assert_eq!(sup.live_sessions().len(), 1);
        let cancel = sup.start_reaper();
        // Wait a few reaper cycles.
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        assert!(
            sup.live_sessions().is_empty(),
            "reaper should have cleaned up the orphan: {:?}",
            sup.live_sessions()
        );
    }

    #[tokio::test]
    async fn reaper_leaves_alive_parents_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_millis(50),
            healthcheck_interval: Duration::from_secs(30),
            healthcheck_url: "http://unused.invalid/".into(),
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        // The current process is the "parent" — definitely alive.
        sup.register(BackendHandle {
            session_id: "live-1".into(),
            pid: 1, // PID 1 is init/launchd on Unix — terminate_pid will return
            // EPERM but the reaper only triggers when parent is dead.
            parent_pid: std::process::id(),
        });
        let cancel = sup.start_reaper();
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        assert_eq!(
            sup.live_sessions().len(),
            1,
            "reaper should have left the live-parent session alone"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_camoufox_spawns_real_child_and_tracks_pid() {
        // Use `sleep 60` as a stand-in for the Camoufox sidecar — we only need
        // a real long-running process to assert PID tracking.
        let sup = Arc::new(BrowserSupervisor::new());
        let bin = std::path::Path::new("/bin/sleep");
        // Some build hosts use /usr/bin/sleep — try both.
        let bin = if bin.exists() {
            bin
        } else {
            std::path::Path::new("/usr/bin/sleep")
        };
        if !bin.exists() {
            return; // skip if no `sleep`
        }
        let pid = sup
            .launch_camoufox(bin, &["60"], "spawn-test")
            .await
            .unwrap();
        assert!(pid > 0);
        assert!(process_alive(pid), "spawned process should be alive");
        // Cleanup.
        assert!(sup.on_session_end("spawn-test"));
        // Give the OS a tick to reap.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
