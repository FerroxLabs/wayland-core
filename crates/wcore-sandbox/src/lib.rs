//! wcore-sandbox — process-isolated tool execution.
//!
//! v0.6.3 introduces a multi-backend trait: each platform's preferred
//! sandbox (bubblewrap on Linux, sandbox-exec on macOS, AppContainer on
//! Windows, Docker as an opt-in cross-platform option) implements the
//! same `SandboxBackend::execute` API. Callers pass a `SandboxManifest`
//! plus a `SandboxCommand` and receive a `SandboxOutput` that includes
//! a `ResourceLimitEnforcement` flag so they can warn the operator when
//! limits are advisory rather than enforced.
//!
//! `default_for_platform` selects the platform's real backend by `cfg`:
//! bubblewrap on Linux, sandbox-exec on macOS, AppContainer on Windows
//! (Docker is an opt-in via `WAYLAND_SANDBOX=docker`). There is no
//! unsandboxed default — when no real backend is available the dispatcher
//! fails closed via `FailClosedBackend` (refusing execution), and only
//! falls back to `NoSandboxBackend` under the explicit
//! `WAYLAND_ALLOW_NO_SANDBOX=1` opt-in.

pub mod backends;
pub mod error;
pub mod manifest;

pub use error::{Result, SandboxError};
pub use manifest::{NetworkPolicy, SandboxManifest, SyscallPolicy};

use async_trait::async_trait;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Operator opt-in that permits running model-driven commands with NO
/// isolation when the platform's real sandbox is unavailable. Without it
/// the sandbox layer fails CLOSED (refuses execution) rather than silently
/// degrading to host-permission execution (audit M-2 / rel-concurrency-70).
const ALLOW_NO_SANDBOX_ENV: &str = "WAYLAND_ALLOW_NO_SANDBOX";

/// Env-var name selecting the sandbox backend (`none` / `docker`).
const SANDBOX_ENV: &str = "WAYLAND_SANDBOX";

/// #327: config-sourced sandbox toggle, installed once at bootstrap from
/// `[tools] sandbox` / `[tools] allow_no_sandbox`.
///
/// This is a process-global fallback consulted only when the corresponding
/// env var is unset — the `WAYLAND_SANDBOX` / `WAYLAND_ALLOW_NO_SANDBOX`
/// env vars keep precedence for back-compat. Mirrors the process-global
/// pattern used by `wcore_tools::env_passthrough::set_config_passthrough`.
#[derive(Clone, Default)]
struct SandboxConfigOverride {
    /// Backend selection (`Some("none")` / `Some("docker")`), or `None` to
    /// leave the platform default.
    backend: Option<String>,
    /// Operator opt-in to unsandboxed execution; `None` = unset.
    allow_no_sandbox: Option<bool>,
}

fn sandbox_config_override() -> &'static RwLock<SandboxConfigOverride> {
    static CFG: OnceLock<RwLock<SandboxConfigOverride>> = OnceLock::new();
    CFG.get_or_init(|| RwLock::new(SandboxConfigOverride::default()))
}

/// Install the config-sourced sandbox toggle (#327).
///
/// Called once at host bootstrap with the resolved `[tools] sandbox` /
/// `[tools] allow_no_sandbox` values. The `WAYLAND_SANDBOX` /
/// `WAYLAND_ALLOW_NO_SANDBOX` env vars still take precedence — config is
/// only the fallback when the env var is absent. Subsequent calls replace
/// the previous config (the host owns the source of truth).
pub fn set_config_sandbox(backend: Option<String>, allow_no_sandbox: Option<bool>) {
    let normalized = backend.and_then(|b| {
        let t = b.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    });
    let mut guard = match sandbox_config_override().write() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    *guard = SandboxConfigOverride {
        backend: normalized,
        allow_no_sandbox,
    };
}

/// Resolve the effective sandbox backend selection: the `WAYLAND_SANDBOX`
/// env var wins; otherwise the config-installed value (#327); otherwise
/// `None` (platform default).
fn resolved_sandbox_choice() -> Option<String> {
    if let Ok(v) = std::env::var(SANDBOX_ENV) {
        return Some(v);
    }
    sandbox_config_override()
        .read()
        .ok()
        .and_then(|g| g.backend.clone())
}

/// True iff the operator has explicitly opted in to unsandboxed execution.
///
/// The `WAYLAND_ALLOW_NO_SANDBOX=1` (or `=true`) env var wins; otherwise
/// the config-installed `[tools] allow_no_sandbox` value is consulted
/// (#327).
pub fn no_sandbox_opt_in() -> bool {
    if let Ok(v) = std::env::var(ALLOW_NO_SANDBOX_ENV) {
        return v == "1" || v.eq_ignore_ascii_case("true");
    }
    sandbox_config_override()
        .read()
        .ok()
        .and_then(|g| g.allow_no_sandbox)
        .unwrap_or(false)
}

/// Minimum gap between repeated "sandbox degraded" warnings.
const DEGRADED_WARN_INTERVAL: Duration = Duration::from_secs(60);

/// Emit a warn-level log on EVERY unsandboxed selection, rate-limited to at
/// most once per [`DEGRADED_WARN_INTERVAL`]. Unlike the process-global
/// warn-once used for the explicit `WAYLAND_SANDBOX=none` path, this keeps
/// the degraded-isolation state visible for the life of a long-running
/// agent process instead of logging it exactly once at startup (audit M-2 /
/// rel-concurrency-70).
fn warn_sandbox_degraded_rate_limited() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut guard = match LAST.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let now = Instant::now();
    let due = match *guard {
        None => true,
        Some(prev) => now.duration_since(prev) >= DEGRADED_WARN_INTERVAL,
    };
    if due {
        *guard = Some(now);
        drop(guard);
        tracing::warn!(
            target: "wcore_sandbox",
            "sandbox UNAVAILABLE — running model-driven command with NO isolation \
             (WAYLAND_ALLOW_NO_SANDBOX opt-in is set). Filesystem and network are \
             unconfined. Install bubblewrap (Linux) or set WAYLAND_SANDBOX=docker.",
        );
    }
}

/// Emit a warn-level log on EVERY unsandboxed selection made because the
/// session is TRUSTED (the user's own machine) rather than via the
/// `WAYLAND_ALLOW_NO_SANDBOX` opt-in. Rate-limited like
/// [`warn_sandbox_degraded_rate_limited`]. Names the exact bypass so the
/// degraded state is never silent (#657 Part 3 / audit #656).
fn warn_sandbox_degraded_trusted_rate_limited() {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut guard = match LAST.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let now = Instant::now();
    let due = match *guard {
        None => true,
        Some(prev) => now.duration_since(prev) >= DEGRADED_WARN_INTERVAL,
    };
    if due {
        *guard = Some(now);
        drop(guard);
        tracing::warn!(
            target: "wcore_sandbox",
            "no OS sandbox backend available on this host — running model-driven \
             commands with NO isolation because the workspace is TRUSTED (your own \
             machine). Filesystem and network are unconfined. Install bubblewrap \
             (Linux) or set WAYLAND_SANDBOX=docker to restore isolation; a \
             Contained/remote workspace would refuse to run instead.",
        );
    }
}

/// Fail-closed backend selected when no real sandbox is available and the
/// operator has NOT opted in to unsandboxed execution via
/// `WAYLAND_ALLOW_NO_SANDBOX=1`.
///
/// Every `execute` call is refused with an error that names the remediation.
/// This is the default-safe behavior: rather than silently substituting
/// [`backends::no_sandbox::NoSandboxBackend`] (which runs with full host
/// permissions), the sandbox layer refuses model-driven execution outright
/// (audit M-2 / rel-concurrency-70).
///
/// `is_available()` returns `true` so callers that probe a constructed
/// backend treat selection as resolved; the refusal surfaces at execution
/// time with an actionable message instead.
pub struct FailClosedBackend;

impl FailClosedBackend {
    pub fn new() -> Self {
        Self
    }

    fn refusal() -> SandboxError {
        SandboxError::ExecFailed(
            "sandbox UNAVAILABLE and unsandboxed execution is not permitted — \
             refusing to run with host permissions. Install bubblewrap (Linux), \
             set WAYLAND_SANDBOX=docker, or explicitly opt in with \
             WAYLAND_ALLOW_NO_SANDBOX=1 to accept running with NO isolation."
                .into(),
        )
    }
}

impl Default for FailClosedBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl backends::SandboxBackend for FailClosedBackend {
    fn name(&self) -> &'static str {
        "fail_closed"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _manifest: &SandboxManifest,
        _cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        // Surface on every refused command so the degraded state is visible.
        tracing::error!(
            target: "wcore_sandbox",
            "refused unsandboxed command — no real sandbox backend available \
             and WAYLAND_ALLOW_NO_SANDBOX is not set",
        );
        Err(Self::refusal())
    }
}

/// Select the unsandboxed fallback backend, failing CLOSED by default.
///
/// - If `WAYLAND_ALLOW_NO_SANDBOX=1` (or `=true`): warn (rate-limited, on
///   every selection) and return [`backends::no_sandbox::NoSandboxBackend`]
///   so execution proceeds with NO isolation per explicit operator opt-in.
/// - Else if `trusted` (the session's workspace is `Trusted` — the user's
///   own machine, #657 Part 3): degrade to a LOUD [`NoSandboxBackend`]
///   (rate-limited warn naming the bypass) so a hands-on-keyboard user on a
///   host without a real sandbox can still run installs/builds, rather than
///   hitting a hard refusal that looks broken next to other agents.
/// - Otherwise (Contained / untrusted / trust unknown): return
///   [`FailClosedBackend`], which refuses execution.
///
/// Single chokepoint for the silent-degradation paths in
/// `default_for_platform` (audit M-2 / rel-concurrency-70).
fn unsandboxed_fallback(trusted: bool) -> Box<dyn backends::SandboxBackend> {
    if no_sandbox_opt_in() {
        warn_sandbox_degraded_rate_limited();
        Box::new(backends::no_sandbox::NoSandboxBackend::new())
    } else if trusted {
        warn_sandbox_degraded_trusted_rate_limited();
        Box::new(backends::no_sandbox::NoSandboxBackend::new())
    } else {
        tracing::error!(
            target: "wcore_sandbox",
            "no real sandbox backend available and WAYLAND_ALLOW_NO_SANDBOX is not \
             set — sandbox FAILS CLOSED; model-driven commands will be refused. \
             Install bubblewrap (Linux), set WAYLAND_SANDBOX=docker, or set \
             WAYLAND_ALLOW_NO_SANDBOX=1 to run with NO isolation.",
        );
        Box::new(FailClosedBackend::new())
    }
}

/// The argv + cwd a backend executes inside a sandboxed child.
#[derive(Debug, Clone)]
pub struct SandboxCommand {
    pub argv: Vec<String>,
    pub cwd: Option<std::path::PathBuf>,
}

/// A single streamed unit of output from a sandboxed child process.
///
/// Emitted on the `mpsc::Receiver` returned by
/// [`backends::SandboxBackend::execute_streaming`]. A streaming run yields
/// zero or more `Stdout`/`Stderr` chunks followed by exactly one terminal
/// `Exit` chunk. Backends that cannot stream natively (the default trait
/// impl) emit one `Stdout` chunk, one `Stderr` chunk, then `Exit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxChunk {
    /// Raw bytes read from the child's stdout.
    Stdout(Vec<u8>),
    /// Raw bytes read from the child's stderr.
    Stderr(Vec<u8>),
    /// Terminal chunk — the child has exited. Carries the exit code and
    /// the resource-limit-enforcement metadata for the run.
    Exit {
        exit_code: i32,
        resource_limits: ResourceLimitEnforcement,
    },
}

/// What `SandboxBackend::execute` returns.
#[derive(Debug, Clone)]
pub struct SandboxOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Whether the backend mechanism actually enforced resource limits.
    pub resource_limits: ResourceLimitEnforcement,
}

/// Whether the backend was able to enforce the manifest's resource limits.
/// Callers (BashTool, etc.) can warn the user if a class of limit is not
/// real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceLimitEnforcement {
    /// Backend has no rlimit mechanism for this platform (e.g.
    /// sandbox-exec).
    None,
    /// Backend tries via `setrlimit` pre-exec; subject to OOM-killer races.
    BestEffort,
    /// Backend enforces via OS/hypervisor (Docker, AppContainer Job
    /// Objects).
    Enforced,
}

pub struct SandboxRegistry {
    backend: Arc<dyn backends::SandboxBackend>,
}

impl SandboxRegistry {
    pub fn new(backend: Arc<dyn backends::SandboxBackend>) -> Self {
        Self { backend }
    }
    pub async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        self.backend.execute(manifest, cmd).await
    }
    /// Streaming execution — see [`backends::SandboxBackend::execute_streaming`].
    pub fn execute_streaming(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        Arc::clone(&self.backend).execute_streaming(manifest, cmd)
    }
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }
    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }
}

/// Choose the default backend for the current platform.
///
/// Each platform's real backend is selected by a `cfg` branch below:
/// bubblewrap (Linux), sandbox-exec (macOS), AppContainer (Windows), each
/// used when its `is_available()` holds. There is no unsandboxed default —
/// when no real backend is available the dispatcher fails closed (see below).
///
/// `WAYLAND_SANDBOX=none` forces the no-op backend, but ONLY when the
/// operator has also opted in via `WAYLAND_ALLOW_NO_SANDBOX=1`; otherwise it
/// fails closed (audit M-2). `WAYLAND_SANDBOX=docker` opts in to the Docker
/// backend; when Docker is unreachable it fails closed rather than silently
/// substituting NoSandbox.
///
/// Whenever no real sandbox backend is available, this routes through
/// [`unsandboxed_fallback`]: it returns a [`FailClosedBackend`] (refuses
/// execution) unless `WAYLAND_ALLOW_NO_SANDBOX=1` is set, in which case it
/// returns [`backends::no_sandbox::NoSandboxBackend`] with a rate-limited
/// warning on every selection.
///
/// This entry point carries NO trust signal, so it fails closed on the
/// no-backend path. Callers that know the session is `Trusted` (the user's
/// own machine) should use [`default_for_platform_trusted`] instead, which
/// degrades to a loud NoSandbox rather than refusing (#657 Part 3).
pub fn default_for_platform() -> Box<dyn backends::SandboxBackend> {
    default_for_platform_trusted(false)
}

/// Trust-aware sandbox selection (#657 Part 3). Identical to
/// [`default_for_platform`] except that when NO real sandbox backend is
/// available and the operator has not set `WAYLAND_ALLOW_NO_SANDBOX`, a
/// `trusted` session (the user's own machine) degrades to a LOUD
/// [`backends::no_sandbox::NoSandboxBackend`] instead of failing closed — so
/// installs/builds still run on a host without bubblewrap. Contained /
/// untrusted sessions (`trusted = false`) keep the fail-closed behavior.
///
/// The explicit `WAYLAND_SANDBOX=none`-without-opt-in path still fails
/// closed regardless of trust: that is a deliberate operator misconfiguration
/// signal, not the "no sandbox installed" case this flag addresses.
pub fn default_for_platform_trusted(trusted: bool) -> Box<dyn backends::SandboxBackend> {
    // #327: env var wins; otherwise the config-installed `[tools] sandbox`.
    if let Some(choice) = resolved_sandbox_choice() {
        match choice.as_str() {
            "none" => {
                // Explicit operator request for no sandbox. Honor it only
                // when the unsandboxed opt-in is ALSO set; otherwise fail
                // closed so a stray `WAYLAND_SANDBOX=none` cannot silently
                // strip isolation (audit M-2).
                if no_sandbox_opt_in() {
                    backends::no_sandbox::warn_once_sandbox_disabled();
                    return Box::new(backends::no_sandbox::NoSandboxBackend::new());
                }
                tracing::error!(
                    target: "wcore_sandbox",
                    "WAYLAND_SANDBOX=none requested but WAYLAND_ALLOW_NO_SANDBOX \
                     is not set — refusing to disable the sandbox. Set \
                     WAYLAND_ALLOW_NO_SANDBOX=1 to run with NO isolation."
                );
                return Box::new(FailClosedBackend::new());
            }
            "docker" => {
                use backends::SandboxBackend as _;
                let docker = backends::docker::DockerBackend::new();
                if docker.is_available() {
                    return Box::new(docker);
                }
                // Docker requested but unreachable. Surface the misconfig
                // loud-and-early and fail closed rather than silently
                // running unsandboxed under the host's full permissions.
                tracing::error!(
                    target: "wcore_sandbox",
                    "WAYLAND_SANDBOX=docker but Docker socket not reachable; \
                     failing closed (set WAYLAND_ALLOW_NO_SANDBOX=1 to run \
                     unsandboxed instead)"
                );
                return unsandboxed_fallback(trusted);
            }
            _ => {}
        }
    }
    #[cfg(target_os = "linux")]
    {
        use backends::SandboxBackend as _;
        let bwrap = backends::bwrap::BubblewrapBackend::new();
        if bwrap.is_available() {
            return Box::new(bwrap);
        }
        // S7 may add Docker fallback here; for now, fail closed (or
        // NoSandbox under explicit opt-in).
    }
    #[cfg(target_os = "macos")]
    {
        use backends::SandboxBackend as _;
        let sbx = backends::sandbox_exec::SandboxExecBackend::new();
        if sbx.is_available() {
            return Box::new(sbx);
        }
    }
    #[cfg(target_os = "windows")]
    {
        use backends::SandboxBackend as _;
        let appc = backends::appcontainer::AppContainerBackend::new();
        if appc.is_available() {
            return Box::new(appc);
        }
    }
    unsandboxed_fallback(trusted)
}

/// Crate-wide serialization lock for tests that mutate the process-global
/// sandbox state (`WAYLAND_SANDBOX` / `WAYLAND_ALLOW_NO_SANDBOX` env vars and
/// the `#327` config override). Both `fail_closed_tests` and
/// `config_toggle_tests` touch the SAME globals, so they must share one lock —
/// per-module locks would let env mutations from one module race the reads of
/// the other.
#[cfg(test)]
static SANDBOX_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use backends::SandboxBackend as _;

    /// Serialize the env-mutating tests in this module — `WAYLAND_SANDBOX`
    /// and `WAYLAND_ALLOW_NO_SANDBOX` are process-global. Shared with
    /// `config_toggle_tests` (same globals).
    use super::SANDBOX_TEST_LOCK as ENV_LOCK;

    /// RAII guard that snapshots and restores both sandbox env vars so a
    /// test never leaks state into a sibling.
    ///
    /// These tests assert pure-env behavior, so `capture()` also clears the
    /// `#327` config override (snapshotting it for restore) — otherwise a
    /// config value left by a sibling `config_toggle_tests` case would bleed
    /// into `no_sandbox_opt_in` / `default_for_platform`.
    struct EnvGuard {
        sandbox: Option<String>,
        allow: Option<String>,
        cfg: SandboxConfigOverride,
    }
    impl EnvGuard {
        fn capture() -> Self {
            let cfg = sandbox_config_override()
                .read()
                .map(|g| g.clone())
                .unwrap_or_default();
            // Clear config so these tests observe env-only behavior.
            set_config_sandbox(None, None);
            Self {
                sandbox: std::env::var("WAYLAND_SANDBOX").ok(),
                allow: std::env::var(ALLOW_NO_SANDBOX_ENV).ok(),
                cfg,
            }
        }
        fn set_sandbox(v: Option<&str>) {
            // SAFETY: tests are serialized via ENV_LOCK; no other thread in
            // this binary reads these vars concurrently during the test.
            unsafe {
                match v {
                    Some(val) => std::env::set_var("WAYLAND_SANDBOX", val),
                    None => std::env::remove_var("WAYLAND_SANDBOX"),
                }
            }
        }
        fn set_allow(v: Option<&str>) {
            unsafe {
                match v {
                    Some(val) => std::env::set_var(ALLOW_NO_SANDBOX_ENV, val),
                    None => std::env::remove_var(ALLOW_NO_SANDBOX_ENV),
                }
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            Self::set_sandbox(self.sandbox.as_deref());
            Self::set_allow(self.allow.as_deref());
            set_config_sandbox(self.cfg.backend.clone(), self.cfg.allow_no_sandbox);
        }
    }

    #[tokio::test]
    async fn fail_closed_backend_refuses_execution() {
        let backend = FailClosedBackend::new();
        assert_eq!(backend.name(), "fail_closed");
        // Reports available so selection resolves, but execution is refused.
        assert!(backend.is_available());
        let err = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["/bin/echo".into(), "hi".into()],
                    cwd: None,
                },
            )
            .await
            .unwrap_err();
        match err {
            SandboxError::ExecFailed(msg) => {
                assert!(
                    msg.contains("WAYLAND_ALLOW_NO_SANDBOX"),
                    "refusal must name the opt-in env: {msg}"
                );
            }
            other => panic!("expected ExecFailed, got {other:?}"),
        }
    }

    #[test]
    fn unsandboxed_fallback_fails_closed_without_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(None);
        // Untrusted / trust-unknown fallback must fail closed.
        let backend = unsandboxed_fallback(false);
        assert_eq!(
            backend.name(),
            "fail_closed",
            "without WAYLAND_ALLOW_NO_SANDBOX the untrusted fallback must fail closed"
        );
    }

    #[test]
    fn unsandboxed_fallback_runs_no_sandbox_with_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(Some("1"));
        let backend = unsandboxed_fallback(false);
        assert_eq!(
            backend.name(),
            "no_sandbox",
            "WAYLAND_ALLOW_NO_SANDBOX=1 must opt in to NoSandbox"
        );
    }

    /// #657 Part 3: with no real sandbox and NO opt-in, a TRUSTED session
    /// degrades to a loud NoSandbox (installs work on the user's own box),
    /// while an untrusted session in the identical env still fails closed.
    #[test]
    fn unsandboxed_fallback_trusted_degrades_untrusted_refuses() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(None);
        assert_eq!(
            unsandboxed_fallback(true).name(),
            "no_sandbox",
            "Trusted session must degrade to NoSandbox instead of failing closed"
        );
        assert_eq!(
            unsandboxed_fallback(false).name(),
            "fail_closed",
            "Untrusted session must still fail closed in the same env"
        );
    }

    #[test]
    fn sandbox_none_fails_closed_without_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_sandbox(Some("none"));
        EnvGuard::set_allow(None);
        // A stray WAYLAND_SANDBOX=none must NOT silently strip isolation.
        let backend = default_for_platform();
        assert_eq!(
            backend.name(),
            "fail_closed",
            "WAYLAND_SANDBOX=none without the opt-in must fail closed"
        );
    }

    #[test]
    fn sandbox_none_honored_with_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_sandbox(Some("none"));
        EnvGuard::set_allow(Some("1"));
        let backend = default_for_platform();
        assert_eq!(
            backend.name(),
            "no_sandbox",
            "WAYLAND_SANDBOX=none + opt-in must honor the no-op backend"
        );
    }

    #[test]
    fn fail_closed_backend_does_not_enforce_read_deny() {
        // FailClosedBackend never enforces deny rules (it refuses all
        // execution), so enforces_read_deny() must stay on the trait default
        // of false. The Bash capability gate depends on this being truthful.
        let backend = FailClosedBackend::new();
        assert!(
            !backend.enforces_read_deny(),
            "FailClosedBackend must not claim to enforce secret-read-deny"
        );
    }

    #[test]
    fn opt_in_parsing_accepts_1_and_true() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(Some("1"));
        assert!(no_sandbox_opt_in());
        EnvGuard::set_allow(Some("true"));
        assert!(no_sandbox_opt_in());
        EnvGuard::set_allow(Some("TRUE"));
        assert!(no_sandbox_opt_in());
        EnvGuard::set_allow(Some("0"));
        assert!(!no_sandbox_opt_in());
        EnvGuard::set_allow(Some("yes"));
        assert!(!no_sandbox_opt_in());
        EnvGuard::set_allow(None);
        assert!(!no_sandbox_opt_in());
    }
}

#[cfg(test)]
mod config_toggle_tests {
    //! #327: `[tools] sandbox` / `[tools] allow_no_sandbox` config toggle.
    //!
    //! The config-installed values are a fallback consulted only when the
    //! corresponding env var is unset; the env var keeps precedence. Both
    //! the env vars and the config override are process-global, so these
    //! tests serialize on the shared SANDBOX_TEST_LOCK (the same lock
    //! fail_closed_tests uses) and restore all state on drop.
    use super::*;

    use super::SANDBOX_TEST_LOCK as ENV_LOCK;

    /// Snapshot + restore both sandbox env vars AND the config override so a
    /// test never leaks state into a sibling (config override is process-global).
    struct StateGuard {
        sandbox: Option<String>,
        allow: Option<String>,
        cfg: SandboxConfigOverride,
    }
    impl StateGuard {
        fn capture() -> Self {
            let cfg = sandbox_config_override()
                .read()
                .map(|g| g.clone())
                .unwrap_or_default();
            Self {
                sandbox: std::env::var(SANDBOX_ENV).ok(),
                allow: std::env::var(ALLOW_NO_SANDBOX_ENV).ok(),
                cfg,
            }
        }
        fn clear_env() {
            // SAFETY: serialized via ENV_LOCK.
            unsafe {
                std::env::remove_var(SANDBOX_ENV);
                std::env::remove_var(ALLOW_NO_SANDBOX_ENV);
            }
        }
    }
    impl Drop for StateGuard {
        fn drop(&mut self) {
            // SAFETY: serialized via ENV_LOCK.
            unsafe {
                match &self.sandbox {
                    Some(v) => std::env::set_var(SANDBOX_ENV, v),
                    None => std::env::remove_var(SANDBOX_ENV),
                }
                match &self.allow {
                    Some(v) => std::env::set_var(ALLOW_NO_SANDBOX_ENV, v),
                    None => std::env::remove_var(ALLOW_NO_SANDBOX_ENV),
                }
            }
            set_config_sandbox(self.cfg.backend.clone(), self.cfg.allow_no_sandbox);
        }
    }

    #[test]
    fn config_allow_no_sandbox_opt_in_honored_when_env_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        set_config_sandbox(None, None);
        assert!(!no_sandbox_opt_in(), "default must be off");
        set_config_sandbox(None, Some(true));
        assert!(
            no_sandbox_opt_in(),
            "[tools] allow_no_sandbox = true must opt in when env is unset"
        );
    }

    #[test]
    fn env_allow_no_sandbox_wins_over_config() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        // Config says opt-in, env explicitly says off → env wins (back-compat).
        set_config_sandbox(None, Some(true));
        // SAFETY: serialized via ENV_LOCK.
        unsafe { std::env::set_var(ALLOW_NO_SANDBOX_ENV, "0") };
        assert!(
            !no_sandbox_opt_in(),
            "WAYLAND_ALLOW_NO_SANDBOX must take precedence over config"
        );
    }

    #[test]
    fn config_sandbox_none_honored_with_config_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        // Pure-config path: sandbox=none + allow_no_sandbox=true, no env vars.
        set_config_sandbox(Some("none".into()), Some(true));
        let backend = default_for_platform();
        assert_eq!(
            backend.name(),
            "no_sandbox",
            "[tools] sandbox=\"none\" + allow_no_sandbox=true must select NoSandbox"
        );
    }

    #[test]
    fn config_sandbox_none_fails_closed_without_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        // sandbox=none from config but no opt-in → must fail closed, exactly
        // like the env-var path (audit M-2 invariant preserved).
        set_config_sandbox(Some("none".into()), Some(false));
        let backend = default_for_platform();
        assert_eq!(
            backend.name(),
            "fail_closed",
            "config sandbox=none without the opt-in must fail closed"
        );
    }

    #[test]
    fn env_sandbox_backend_wins_over_config_backend() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        // Config selects docker; env forces none. The env WAYLAND_SANDBOX
        // backend selection wins. With the env opt-in set, none resolves to
        // NoSandbox (proving env's `none` overrode config's `docker`).
        set_config_sandbox(Some("docker".into()), Some(false));
        // SAFETY: serialized via ENV_LOCK.
        unsafe {
            std::env::set_var(SANDBOX_ENV, "none");
            std::env::set_var(ALLOW_NO_SANDBOX_ENV, "1");
        }
        let backend = default_for_platform();
        assert_eq!(
            backend.name(),
            "no_sandbox",
            "env WAYLAND_SANDBOX must take precedence over config sandbox backend"
        );
    }

    #[test]
    fn empty_config_backend_is_treated_as_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = StateGuard::capture();
        StateGuard::clear_env();
        // A whitespace-only [tools] sandbox must normalize to None (platform
        // default), not a bogus backend name.
        set_config_sandbox(Some("   ".into()), None);
        assert!(
            resolved_sandbox_choice().is_none(),
            "blank config sandbox must resolve to None"
        );
    }
}
