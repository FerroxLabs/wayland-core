//! `BrowserSupervisor` — lifecycle + orphan reaper for backend processes.
//!
//! Wave BR ships the real implementation. Responsibilities:
//!
//!   * **Launch:** `launch_camoufox` spawns the sidecar binary with
//!     `kill_on_drop(true)` so a panic in the host kills the child.
//!   * **Provisioning:** when the configured sidecar program does not resolve
//!     and the operator opted in via `[browser.camoufox_download]`,
//!     `ensure_ready` calls
//!     `BrowserBinaryManager::provision_camoufox` to download + SHA-verify +
//!     unpack it. Off by default; fail-closed without a pinned digest.
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
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::egress_proxy::PolicyEgressProxy;
use crate::policy::BrowserPolicy;

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub pid_dir: PathBuf,
    /// Reaper polling interval. Default 1Hz; tests use 100ms.
    pub reaper_interval: Duration,
    /// Healthcheck interval. Default 30s.
    pub healthcheck_interval: Duration,
    /// HTTP healthcheck endpoint (Camoufox sidecar `/health`).
    pub healthcheck_url: String,
    /// Installed Camoufox sidecar command. `None` keeps the supervisor in
    /// observe-only mode, which is useful for externally managed providers.
    pub sidecar_program: Option<String>,
    /// Maximum time to wait for a newly spawned sidecar to become healthy.
    pub startup_timeout: Duration,
    /// Opt-in auto-provisioning of the Camoufox binary. Default: **disabled**
    /// (`CamoufoxDownloadConfig::default()`), so the supervisor fetches
    /// nothing unless an operator turns it on in `[browser.camoufox_download]`
    /// and pins a SHA-256 for their platform.
    pub camoufox_download: wcore_config::browser::CamoufoxDownloadConfig,
    /// Install root for auto-provisioned browser binaries.
    pub binary_install_root: PathBuf,
    /// gh#1117 — the policy Core enforces on the sidecar's OWN egress.
    ///
    /// `Some(policy)` makes containment a precondition: `ensure_ready` starts
    /// a loopback [`PolicyEgressProxy`] carrying
    /// [`BrowserPolicy::address_gate_only`] of this policy, points every
    /// sidecar it launches at it, and refuses a sidecar it did not launch
    /// (see [`SupervisorConfig::allow_unproxied_sidecar`]).
    ///
    /// `None` is the pre-gh#1117 behaviour and stays the default, so an
    /// observe-only supervisor and every existing construction site are
    /// unchanged. The production Camoufox path sets it in
    /// [`crate::adapter::from_spec`].
    pub egress_policy: Option<BrowserPolicy>,
    /// gh#1117 opt-out. `false` (default) refuses a sidecar Core did not
    /// launch behind its egress proxy; `true` proceeds with a warning and no
    /// address screening on the sidecar's own requests.
    pub allow_unproxied_sidecar: bool,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            pid_dir: home_pid_dir(),
            reaper_interval: Duration::from_secs(1),
            healthcheck_interval: Duration::from_secs(30),
            healthcheck_url: "http://localhost:9377/health".to_string(),
            sidecar_program: None,
            startup_timeout: Duration::from_secs(15),
            camoufox_download: wcore_config::browser::CamoufoxDownloadConfig::default(),
            binary_install_root: home_bin_dir(),
            egress_policy: None,
            allow_unproxied_sidecar: false,
        }
    }
}

impl SupervisorConfig {
    /// Production configuration for the locally managed Camoufox sidecar.
    /// The command may be overridden by Desktop or an operator.
    ///
    /// Core still invokes no package manager. It downloads executable code
    /// only when the operator has explicitly enabled
    /// `[browser.camoufox_download]` AND pinned a SHA-256 for their platform;
    /// the default config leaves that switch off, which is the pre-existing
    /// "never downloads" behaviour verbatim.
    pub fn local_camoufox(base_url: &str) -> Self {
        let base_url = base_url.trim_end_matches('/');
        Self {
            healthcheck_url: format!("{base_url}/health"),
            sidecar_program: Some(
                std::env::var("WAYLAND_CAMOUFOX_BIN")
                    .unwrap_or_else(|_| "camofox-browser".to_string()),
            ),
            camoufox_download: configured_camoufox_download(),
            allow_unproxied_sidecar: configured_allow_unproxied_sidecar(),
            ..Self::default()
        }
    }
}

/// The operator's gh#1117 opt-out, read from the environment first and the
/// config file second.
///
/// The env var wins because it is the escape hatch an operator reaches for
/// when the refusal is already in front of them. Absent both, the answer is
/// `false` — refuse — and a config file that cannot be read yields `false`
/// too, so an unreadable config can never silently disable containment.
fn configured_allow_unproxied_sidecar() -> bool {
    if let Ok(raw) = std::env::var("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR") {
        let value = raw.trim().to_ascii_lowercase();
        if !value.is_empty() {
            return matches!(value.as_str(), "1" | "true" | "yes" | "on");
        }
    }
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        wcore_config::config::load_merged_config_file(None)
            .map(|file| file.browser.allow_unproxied_sidecar)
            .unwrap_or(false)
    })
}

/// gh#1117 refusal text.
///
/// Names the protection that is missing AND the exact opt-out with exactly
/// what it costs. A refusal that names neither is a dead end, and a silent
/// downgrade is the bug class this whole change is about.
pub fn unproxied_sidecar_refusal(healthcheck_url: &str) -> String {
    format!(
        "Camoufox is already running at {healthcheck_url} and this browser tool did not start it \
behind its own egress proxy — either nothing in Core started it, or another browser tool in this \
process did, which means it is wired to THAT tool's policy gate and not to this one. Either way it \
is not contained by this policy. An unproxied sidecar resolves its own DNS, so Core cannot see or \
screen the addresses the browser actually dials: the policy would apply to the NAME and not to the \
destination (gh#1117). Refusing, rather than reporting a protection that does not apply.\n\
Fix: stop that sidecar and let Core start it — Core passes PROXY_HOST/PROXY_PORT to the sidecar \
it launches.\n\
Opt out: WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR=1, or `allow_unproxied_sidecar = true` under \
[browser] in config. That gives up, for every request the sidecar makes: the DNS resolution gate \
(a public name pointing at 169.254.169.254 or into RFC 1918 reaches the browser), TTL=0 \
intra-navigation rebinding, and any screening at all of sub-resource loads. The navigation URL \
string checks still apply."
    )
}

/// The operator's `[browser.camoufox_download]` block, read once per process.
///
/// `local_camoufox` is the only production constructor of the Camoufox
/// supervisor (`adapter::from_spec`), and it already resolves the sidecar
/// program from the environment, so config resolution belongs here too. A
/// config that cannot be read yields the default - which is *disabled*, so
/// the failure mode is "no auto-download", never "unverified download".
fn configured_camoufox_download() -> wcore_config::browser::CamoufoxDownloadConfig {
    static CACHE: OnceLock<wcore_config::browser::CamoufoxDownloadConfig> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            wcore_config::config::load_merged_config_file(None)
                .map(|file| file.browser.camoufox_download)
                .unwrap_or_default()
        })
        .clone()
}

fn home_bin_dir() -> PathBuf {
    wcore_config::config::profile_home()
        .join("browser")
        .join("bin")
}

fn home_pid_dir() -> PathBuf {
    // isolation: route through profile_home() so browser PID tracking follows
    // WAYLAND_HOME. PIDs are ephemeral; stale entries at the old location are
    // harmless (the reaper only acts on PIDs it registered this session).
    wcore_config::config::profile_home()
        .join("browser")
        .join("pids")
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
    /// gh#1117 egress proxy, started lazily by `ensure_ready` when
    /// `config.egress_policy` is set. Shut down on `Drop`.
    egress_proxy: Mutex<Option<Arc<PolicyEgressProxy>>>,
}

fn sidecar_start_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
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
            egress_proxy: Mutex::new(None),
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
            // F25: kill through the stashed Child handle when present (race-free
            // vs PID reuse), falling back to the raw PID for orphan recovery.
            // `terminate_session` also removes the stashed handle, releasing its
            // fds + zombie slot instead of holding them for the host lifetime.
            terminate_session(session_id, h.pid);
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
        // F24: a second `start_reaper` would otherwise overwrite the stored
        // token, orphaning the prior reaper + healthcheck tasks (they hold the
        // OLD token and never get cancelled). Cancel and replace atomically so
        // the previous task pair shuts down before the new one starts.
        if let Some(prev) = self.reaper_cancel.lock().replace(cancel.clone()) {
            prev.cancel();
        }

        // Schedule the healthcheck loop on the same cancellation token. A
        // zero interval means "disabled" — `tokio::time::interval` panics on a
        // zero period, so we skip scheduling entirely in that case.
        if !self.config.healthcheck_interval.is_zero() {
            let cancel_for_health = cancel.clone();
            let hc_interval = self.config.healthcheck_interval;
            // F23: capture a `Weak<Self>` (not a strong `Arc`). A strong ref
            // here forms a refcount cycle that keeps the supervisor alive
            // forever, so `Drop` (which cancels the reaper) never runs. With a
            // Weak we `upgrade()` per tick and stop the loop the moment the
            // supervisor is dropped — breaking the cycle.
            let sup = Arc::downgrade(self);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(hc_interval);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // Drop the immediate first tick so we wait one full interval
                // before the initial probe, matching the reaper cadence.
                ticker.tick().await;
                loop {
                    tokio::select! {
                        _ = cancel_for_health.cancelled() => break,
                        _ = ticker.tick() => {
                            // Stop probing once the supervisor is gone.
                            let Some(sup) = sup.upgrade() else { break };
                            // Best-effort liveness probe; errors are non-fatal
                            // (sidecar may be starting/restarting).
                            let _ = sup.healthcheck(hc_interval).await;
                        }
                    }
                }
            });
        }

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
                                // F25: prefer the stashed Child handle (race-free
                                // vs PID reuse); fall back to the raw PID for
                                // cross-boot orphans with no handle.
                                terminate_session(&h.session_id, h.pid);
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

    /// Ensure the configured local Camoufox service is reachable. A healthy
    /// externally managed service is reused. Otherwise Core starts only the
    /// already-installed command and waits under a fixed deadline.
    pub async fn ensure_ready(self: &Arc<Self>) -> Result<(), String> {
        let Some(configured_program) = self.config.sidecar_program.clone() else {
            return Ok(());
        };

        let _startup_guard = sidecar_start_lock().lock().await;

        // gh#1117: containment is established BEFORE anything is reused or
        // started, so "this sidecar is not behind the proxy" can never be
        // discovered after a navigation has already gone out.
        let containment_required = self.ensure_egress_proxy().await?;
        // The ownership key carries the egress proxy PORT, so "Core launched
        // it" means "THIS supervisor launched it behind ITS OWN gate".
        //
        // `children_map()` is process-global and a process can hold several
        // supervisors: `HostBrowserRegistrar::reify_all` mints one per
        // registered browser tool spec, each with its own `BrowserPolicy` and
        // its own proxy, and they all point at the same sidecar URL. Keyed on
        // the pid alone, the SECOND supervisor finds the first one's retained
        // child, concludes it owns the sidecar, and reuses one that is pointed
        // at the FIRST policy's proxy - its own `denied_origins` and its own
        // gh#911 port grant silently not applied to anything the browser
        // dials, with no refusal and no warning. Including the port makes that
        // sidecar exactly as unownable as an externally started one, which is
        // what it is.
        let session_id = match self.egress_proxy.lock().as_ref() {
            Some(proxy) => format!(
                "camoufox-sidecar-{}-egress-{}",
                std::process::id(),
                proxy.port()
            ),
            None => format!("camoufox-sidecar-{}", std::process::id()),
        };

        if self
            .healthcheck(Duration::from_millis(500))
            .await
            .unwrap_or(false)
        {
            if containment_required && !owns_live_sidecar(&session_id) {
                if !self.config.allow_unproxied_sidecar {
                    return Err(unproxied_sidecar_refusal(&self.config.healthcheck_url));
                }
                tracing::warn!(
                    healthcheck_url = %self.config.healthcheck_url,
                    "using a Camoufox sidecar Core did not start: it resolves its own DNS, so \
                     the browser policy is NOT enforced on the addresses it dials (gh#1117, \
                     allowed by allow_unproxied_sidecar)"
                );
            }
            return Ok(());
        }

        let resolved_program = self.resolve_sidecar_program(&configured_program).await?;
        let program = resolved_program.as_str();

        // A prior owned sidecar may be alive but unhealthy. Remove it before
        // reusing the stable ownership key so inserting the replacement can
        // never detach the old Child handle.
        if children_map().lock().contains_key(&session_id) {
            let _ = self.on_session_end(&session_id);
        }
        let pid = self
            .launch_camoufox_program(program, &[], &session_id)
            .await
            .map_err(|error| {
                format!(
                    "Camoufox is unavailable at {} and Core could not start `{program}`: {error}. \
Install @askjo/camofox-browser or set WAYLAND_CAMOUFOX_BIN to its executable",
                    self.config.healthcheck_url
                )
            })?;

        let deadline = tokio::time::Instant::now() + self.config.startup_timeout;
        loop {
            if self
                .healthcheck(Duration::from_millis(500))
                .await
                .unwrap_or(false)
            {
                return Ok(());
            }
            if let Some(status) = owned_child_status(&session_id) {
                let _ = self.on_session_end(&session_id);
                return Err(format!(
                    "Camoufox process {pid} exited before becoming healthy ({status}); run `{program}` directly for diagnostics"
                ));
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = self.on_session_end(&session_id);
                return Err(format!(
                    "Camoufox process {pid} did not become healthy at {} within {}ms",
                    self.config.healthcheck_url,
                    self.config.startup_timeout.as_millis()
                ));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Start the gh#1117 egress proxy if this supervisor is configured to
    /// contain its sidecar. Returns whether containment is required at all.
    ///
    /// Fail-closed: a proxy that cannot bind is an error, never a quiet
    /// fallback to an uncontained sidecar.
    async fn ensure_egress_proxy(self: &Arc<Self>) -> Result<bool, String> {
        let Some(policy) = self.config.egress_policy.clone() else {
            return Ok(false);
        };
        let already = self.egress_proxy.lock().is_some();
        if already {
            return Ok(true);
        }
        let proxy = PolicyEgressProxy::start(policy.address_gate_only())
            .await
            .map_err(|error| {
                format!(
                    "could not start the browser egress proxy on loopback: {error}. Refusing to \
                     use a sidecar that would resolve its own DNS (gh#1117)"
                )
            })?;
        *self.egress_proxy.lock() = Some(proxy);
        Ok(true)
    }

    /// The running egress proxy, if any. Test / introspection helper — a test
    /// asserting "the sidecar could not reach it" reads the proxy's refusal
    /// counter, not just the client-side error.
    pub fn egress_proxy(&self) -> Option<Arc<PolicyEgressProxy>> {
        self.egress_proxy.lock().clone()
    }

    /// The executable [`Self::ensure_ready`] will actually spawn.
    ///
    /// Returns the configured program unchanged when it already resolves
    /// (PATHEXT-aware via `which`, resolve-only - nothing is executed), and
    /// unchanged when auto-download is off, which is the default. Only when
    /// the program is missing AND the operator enabled
    /// `[browser.camoufox_download]` does this reach the network, and then
    /// only through the fail-closed
    /// [`crate::binary::BrowserBinaryManager::provision_camoufox`]: an
    /// unconfigured platform or an unpinned digest is an error here, never a
    /// silent unverified fetch.
    async fn resolve_sidecar_program(&self, program: &str) -> Result<String, String> {
        if which::which(program).is_ok() {
            return Ok(program.to_string());
        }
        if !self.config.camoufox_download.enabled {
            // Unchanged pre-existing behaviour: let the spawn below fail with
            // the actionable "install it / set WAYLAND_CAMOUFOX_BIN" message.
            return Ok(program.to_string());
        }
        let manager = crate::binary::BrowserBinaryManager::new(
            self.config.binary_install_root.clone(),
            false,
        );
        match manager
            .provision_camoufox(&self.config.camoufox_download)
            .await
        {
            Ok(Some(path)) => path
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| "provisioned Camoufox path is not valid UTF-8".to_string()),
            Ok(None) => Ok(program.to_string()),
            Err(error) => Err(format!(
                "Camoufox auto-download failed while provisioning `{program}`: {error}"
            )),
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
        let program = binary_path
            .to_str()
            .ok_or_else(|| "Camoufox executable path is not valid UTF-8".to_string())?;
        self.launch_camoufox_program(program, args, session_id)
            .await
    }

    async fn launch_camoufox_program(
        self: &Arc<Self>,
        program: &str,
        args: &[&str],
        session_id: impl Into<String>,
    ) -> Result<u32, String> {
        let session = session_id.into();
        let mut cmd = wcore_config::shell::shell_command_argv(program, args);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // gh#1117 — every sidecar Core launches goes out through Core's gate.
        if let Some(proxy) = self.egress_proxy.lock().clone() {
            apply_egress_env(&mut cmd, &proxy);
        }
        wcore_sandbox::backends::process_tree::isolate(&mut cmd);
        let child = cmd.spawn().map_err(|e| format!("spawn camoufox: {e}"))?;
        let pid = child.id().ok_or_else(|| "no child PID".to_string())?;
        let tree_guard = wcore_sandbox::backends::process_tree::ProcessTreeGuard::new(Some(pid))
            .map_err(|error| format!("own camoufox process tree: {error}"))?;
        // Forget the child handle in-memory; kill_on_drop fired when this
        // Child struct drops, so we have to stash it (or accept SIGKILL when
        // the local goes out of scope). We stash via a static map keyed by
        // session_id so multiple sidecars can coexist.
        retain_child(&session, child, tree_guard);
        self.register(BackendHandle {
            session_id: session,
            pid,
            parent_pid: std::process::id(),
        });
        Ok(pid)
    }
}

/// Point a sidecar command at Core's egress proxy.
///
/// `@askjo/camofox-browser` reads these in `lib/config.js` and hands the
/// resulting `http://host:port` to the browser as its launch proxy.
/// VERIFIED live 2026-08-23 against `@askjo/camofox-browser@1.13.1` on real
/// Camoufox: with these set, Firefox sent `CONNECT api.ipify.org:443` and
/// `GET http://example.com/ HTTP/1.1` to the proxy — the HOSTNAME, resolved
/// by nobody but the proxy.
///
/// `PROXY_PORTS` is pinned as well as `PROXY_PORT` because the sidecar's own
/// parser lets `PROXY_PORTS` WIN; an ambient `PROXY_PORTS` in the operator's
/// environment would otherwise send the browser's egress to a port Core does
/// not serve. `PROXY_STRATEGY` is pinned for the same reason: an ambient
/// `backconnect` ignores host/port entirely and would route the browser
/// through a third party.
pub fn apply_egress_env(cmd: &mut tokio::process::Command, proxy: &PolicyEgressProxy) {
    let port = proxy.port().to_string();
    cmd.env("PROXY_STRATEGY", "round_robin")
        .env("PROXY_HOST", proxy.host())
        .env("PROXY_PORT", &port)
        .env("PROXY_PORTS", &port)
        .env("PROXY_USERNAME", "")
        .env("PROXY_PASSWORD", "")
        .env_remove("PROXY_BACKCONNECT_HOST")
        .env_remove("PROXY_BACKCONNECT_PORT");
}

impl Drop for BrowserSupervisor {
    fn drop(&mut self) {
        if let Some(c) = self.reaper_cancel.lock().take() {
            c.cancel();
        }
        if let Some(proxy) = self.egress_proxy.lock().take() {
            proxy.shutdown();
        }
        // Kill only processes for which this process retained a Child handle.
        // Recovered PID-file entries are not safe to signal here because the
        // numeric PID may have been reused.
        for handle in self.sessions.lock().iter() {
            terminate_owned_session(&handle.session_id);
            let pid_path = self
                .config
                .pid_dir
                .join(format!("{}.pid", handle.session_id));
            let _ = std::fs::remove_file(pid_path);
        }
    }
}

/// In-process child-handle storage. We can't move the tokio Child onto the
/// `BrowserSupervisor` because Drop fires before the reaper sees the parent
/// die — we'd kill children before the orphan-reaper logic gets to run.
/// Stashing here means the child outlives the supervisor and the reaper
/// owns the SIGTERM path.
fn children_map() -> &'static parking_lot::Mutex<std::collections::HashMap<String, OwnedChild>> {
    use parking_lot::Mutex as PM;
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static CHILDREN: OnceLock<PM<HashMap<String, OwnedChild>>> = OnceLock::new();
    CHILDREN.get_or_init(|| PM::new(HashMap::new()))
}

struct OwnedChild {
    child: tokio::process::Child,
    tree_guard: wcore_sandbox::backends::process_tree::ProcessTreeGuard,
}

fn retain_child(
    session: &str,
    child: tokio::process::Child,
    tree_guard: wcore_sandbox::backends::process_tree::ProcessTreeGuard,
) {
    children_map()
        .lock()
        .insert(session.to_string(), OwnedChild { child, tree_guard });
}

/// Whether THIS PROCESS launched the sidecar that is answering now, and that
/// child is still alive.
///
/// Derived from the retained `Child` handle rather than from a flag on the
/// supervisor, because a flag gets both edges wrong:
///
///   * a flag is per-supervisor, so the SECOND `ensure_ready` call on the same
///     supervisor - `BrowserTool` makes one before every op - would have to
///     re-derive it, and a flag that is set once cannot;
///   * a flag stays set after the child exits, so an externally started
///     sidecar appearing afterwards would be reused with no refusal at all.
///
/// A sidecar Core did not launch cannot be shown to be contained: `/health`
/// does not report the browser's proxy configuration, so there is nothing to
/// ask it.
fn owns_live_sidecar(session: &str) -> bool {
    let retained = children_map().lock().contains_key(session);
    if !retained {
        return false;
    }
    // `Some(status)` means the child has already exited.
    owned_child_status(session).is_none()
}

fn owned_child_status(session: &str) -> Option<std::process::ExitStatus> {
    children_map()
        .lock()
        .get_mut(session)
        .and_then(|owned| owned.child.try_wait().ok().flatten())
}

fn terminate_owned_session(session: &str) {
    if let Some(mut owned) = children_map().lock().remove(session) {
        let _ = owned.child.start_kill();
        drop(owned.tree_guard);
    }
}

/// Terminate the backend for `session` race-free. When a stashed
/// [`tokio::process::Child`] handle exists (the in-process spawn path) we kill
/// THROUGH it — the kernel guarantees the signal targets that exact child even
/// if the recorded numeric PID has since been recycled by the OS (F25). Only
/// when no handle exists (cross-boot orphan recovery, where the child was
/// spawned by a previous host process) do we fall back to signalling the raw
/// `pid`.
fn terminate_session(session: &str, pid: u32) {
    let mut map = children_map().lock();
    if let Some(mut owned) = map.remove(session) {
        // start_kill targets the Child by handle — immune to PID reuse.
        let _ = owned.child.start_kill();
        drop(owned.tree_guard);
    } else {
        drop(map);
        terminate_pid(pid);
    }
}

/// Returns `true` if the process with `pid` is still RUNNING.
///
/// PRODUCTION path: the browser supervisor uses this to decide whether the
/// parent it is supervising is still there, and therefore whether to tear the
/// browser down. The unix arm was `kill(pid, 0)`, which a **zombie**
/// satisfies, so a parent that had exited but not been reaped kept the
/// supervisor waiting on a corpse and the browser alive. The Windows arm
/// shelled out to `tasklist` and substring-matched the pid anywhere in the
/// output, so `PID eq 42` also matched a row whose memory column contained
/// "42".
///
/// Both are now the one zombie-aware probe; see `.planning/ZOMBIE-PROBE.md`.
pub fn process_alive(pid: u32) -> bool {
    wcore_types::process_liveness::process_is_alive(pid)
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
            s.contains("browser") && s.contains("pids"),
            "unexpected pid dir: {s}"
        );
    }

    #[test]
    #[serial_test::serial]
    fn pid_dir_roots_under_wayland_home() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        // SAFETY: serialized via serial_test; env restored below.
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        let dir = super::home_pid_dir();
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert_eq!(dir, tmp.path().join("browser").join("pids"));
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
    async fn ensure_ready_reuses_healthy_external_sidecar() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let cfg = SupervisorConfig {
            healthcheck_url: format!("{}/health", server.uri()),
            sidecar_program: Some("definitely-not-a-real-camoufox-command".into()),
            ..Default::default()
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        sup.ensure_ready().await.unwrap();
        assert!(sup.live_sessions().is_empty());
    }

    #[tokio::test]
    async fn ensure_ready_reports_actionable_missing_sidecar() {
        let cfg = SupervisorConfig {
            healthcheck_url: "http://127.0.0.1:9/health".into(),
            sidecar_program: Some("wcore-camoufox-command-that-does-not-exist".into()),
            startup_timeout: Duration::from_millis(100),
            ..Default::default()
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        let error = sup.ensure_ready().await.unwrap_err();
        assert!(error.contains("Install @askjo/camofox-browser"), "{error}");
        assert!(error.contains("WAYLAND_CAMOUFOX_BIN"), "{error}");
        assert!(sup.live_sessions().is_empty());
    }

    #[tokio::test]
    async fn reaper_terminates_orphans_with_dead_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_millis(50),
            healthcheck_interval: Duration::from_secs(30),
            healthcheck_url: "http://unused.invalid/".into(),
            ..Default::default()
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
            ..Default::default()
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

    // Spawns a real `true` process as the stashed child; `true` is a Unix
    // builtin/binary with no Windows equivalent on PATH, so gate to unix.
    // `#[tokio::test]`: dropping a `tokio::process::Child` reaps via pidfd,
    // which requires a running reactor.
    #[cfg(unix)]
    #[tokio::test]
    async fn on_session_end_releases_stashed_child_handle() {
        // R64: `on_session_end` must drop the stashed `Child` handle so its
        // fds + zombie slot are released instead of being held for the host
        // lifetime. Use a real short-lived child as the stashed handle and
        // assert the CHILDREN entry is gone after the session ends.
        let sup = BrowserSupervisor::new();
        let sid = "release-child-test";
        // A trivially-short child stands in for the Camoufox sidecar; we only
        // need a real `tokio::process::Child` to stash and then drop.
        let mut command =
            tokio::process::Command::new(if std::path::Path::new("/bin/true").exists() {
                "/bin/true"
            } else {
                "true"
            });
        command.kill_on_drop(true);
        wcore_sandbox::backends::process_tree::isolate(&mut command);
        let child = command.spawn().expect("spawn /bin/true");
        let guard = wcore_sandbox::backends::process_tree::ProcessTreeGuard::new(child.id())
            .expect("own /bin/true process tree");
        retain_child(sid, child, guard);
        assert!(
            children_map().lock().contains_key(sid),
            "child handle should be stashed before session end"
        );
        sup.register(BackendHandle {
            session_id: sid.into(),
            pid: 0x7fff_fffd,
            parent_pid: 0x7fff_fffe,
        });
        assert!(sup.on_session_end(sid));
        assert!(
            !children_map().lock().contains_key(sid),
            "on_session_end must remove the stashed child handle"
        );
    }

    #[tokio::test]
    async fn start_reaper_twice_cancels_prior_task_pair() {
        // F24: a second `start_reaper` must cancel the first token so the prior
        // reaper + healthcheck tasks shut down instead of leaking. Assert the
        // first-returned token is cancelled once the second call runs.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_secs(3600),
            healthcheck_interval: Duration::from_secs(3600),
            healthcheck_url: "http://unused.invalid/".into(),
            ..Default::default()
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        let first = sup.start_reaper();
        assert!(
            !first.is_cancelled(),
            "first token live before second start"
        );
        let second = sup.start_reaper();
        assert!(
            first.is_cancelled(),
            "second start_reaper must cancel the first token (F24)"
        );
        assert!(!second.is_cancelled(), "second token must be live");
        second.cancel();
    }

    #[tokio::test]
    async fn start_reaper_skips_healthcheck_when_interval_zero() {
        // R63: a zero healthcheck_interval means "disabled". The scheduler
        // must skip it entirely — `tokio::time::interval(0)` would otherwise
        // panic. Reaching the assertion proves no panic occurred.
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_millis(50),
            healthcheck_interval: Duration::ZERO,
            healthcheck_url: "http://unused.invalid/".into(),
            ..Default::default()
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        let cancel = sup.start_reaper();
        tokio::time::sleep(Duration::from_millis(120)).await;
        cancel.cancel();
    }

    #[tokio::test]
    async fn start_reaper_schedules_healthcheck_probe() {
        // R63: a non-zero healthcheck_interval must auto-schedule periodic
        // probes against `healthcheck_url`. Drive a mock server and assert it
        // receives at least one request from the scheduled loop.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let tmp = tempfile::tempdir().unwrap();
        let cfg = SupervisorConfig {
            pid_dir: tmp.path().to_path_buf(),
            reaper_interval: Duration::from_secs(3600),
            healthcheck_interval: Duration::from_millis(50),
            healthcheck_url: format!("{}/health", server.uri()),
            ..Default::default()
        };
        let sup = Arc::new(BrowserSupervisor::with_config(cfg));
        let cancel = sup.start_reaper();
        // First probe fires one full interval in; wait a few cycles.
        tokio::time::sleep(Duration::from_millis(250)).await;
        cancel.cancel();
        let hits = server.received_requests().await.unwrap_or_default();
        assert!(
            !hits.is_empty(),
            "scheduled healthcheck loop should have probed /health at least once"
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

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_supervisor_kills_owned_sidecar() {
        let sup = Arc::new(BrowserSupervisor::new());
        let bin = if std::path::Path::new("/bin/sleep").exists() {
            std::path::Path::new("/bin/sleep")
        } else {
            std::path::Path::new("/usr/bin/sleep")
        };
        if !bin.exists() {
            return;
        }
        let pid = sup
            .launch_camoufox(bin, &["60"], "drop-owned-sidecar")
            .await
            .unwrap();
        assert!(process_alive(pid));
        drop(sup);
        for _ in 0..20 {
            if !process_alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("owned sidecar process {pid} survived supervisor drop");
    }
}
