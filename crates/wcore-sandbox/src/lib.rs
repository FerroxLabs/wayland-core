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
use std::time::{Duration, Instant};

/// Operator opt-in that permits running model-driven commands with NO
/// isolation when the platform's real sandbox is unavailable. Without it
/// the sandbox layer fails CLOSED (refuses execution) rather than silently
/// degrading to host-permission execution (audit M-2 / rel-concurrency-70).
const ALLOW_NO_SANDBOX_ENV: &str = "WAYLAND_ALLOW_NO_SANDBOX";

/// True iff the operator has explicitly opted in to unsandboxed execution
/// via `WAYLAND_ALLOW_NO_SANDBOX=1` (or `=true`).
pub fn no_sandbox_opt_in() -> bool {
    std::env::var(ALLOW_NO_SANDBOX_ENV)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
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
/// - Otherwise: return [`FailClosedBackend`], which refuses execution.
///
/// Single chokepoint for the silent-degradation paths in
/// `default_for_platform` (audit M-2 / rel-concurrency-70).
fn unsandboxed_fallback() -> Box<dyn backends::SandboxBackend> {
    if no_sandbox_opt_in() {
        warn_sandbox_degraded_rate_limited();
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
pub fn default_for_platform() -> Box<dyn backends::SandboxBackend> {
    if let Ok(choice) = std::env::var("WAYLAND_SANDBOX") {
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
                return unsandboxed_fallback();
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
    unsandboxed_fallback()
}

#[cfg(test)]
mod fail_closed_tests {
    use super::*;
    use backends::SandboxBackend as _;

    /// Serialize the env-mutating tests in this module — `WAYLAND_SANDBOX`
    /// and `WAYLAND_ALLOW_NO_SANDBOX` are process-global.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// RAII guard that snapshots and restores both sandbox env vars so a
    /// test never leaks state into a sibling.
    struct EnvGuard {
        sandbox: Option<String>,
        allow: Option<String>,
    }
    impl EnvGuard {
        fn capture() -> Self {
            Self {
                sandbox: std::env::var("WAYLAND_SANDBOX").ok(),
                allow: std::env::var(ALLOW_NO_SANDBOX_ENV).ok(),
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
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(None);
        let backend = unsandboxed_fallback();
        assert_eq!(
            backend.name(),
            "fail_closed",
            "without WAYLAND_ALLOW_NO_SANDBOX the fallback must fail closed"
        );
    }

    #[test]
    fn unsandboxed_fallback_runs_no_sandbox_with_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::capture();
        EnvGuard::set_allow(Some("1"));
        let backend = unsandboxed_fallback();
        assert_eq!(
            backend.name(),
            "no_sandbox",
            "WAYLAND_ALLOW_NO_SANDBOX=1 must opt in to NoSandbox"
        );
    }

    #[test]
    fn sandbox_none_fails_closed_without_opt_in() {
        let _lock = ENV_LOCK.lock().unwrap();
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
        let _lock = ENV_LOCK.lock().unwrap();
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
    fn opt_in_parsing_accepts_1_and_true() {
        let _lock = ENV_LOCK.lock().unwrap();
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
