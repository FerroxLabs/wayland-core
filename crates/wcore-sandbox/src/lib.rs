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
pub mod directory_authority;
pub mod error;
pub mod manifest;
pub mod process_capture;

pub use backends::HardContainmentMechanism;
pub use directory_authority::{
    DirectoryAuthority, DirectoryAuthorityIdentity, DirectoryHandleLoan, RegularFileAuthority,
    RetainedWorkspaceAuthority,
};
pub use error::{Result, SandboxError};
pub use manifest::{
    ContainmentPolicyIdentity, HardContainmentFilesystem, NetworkPolicy, SandboxManifest,
    SyscallPolicy,
};

use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use wcore_types::execution_policy::DangerousSessionGrant;

/// Operator opt-in that permits running model-driven commands with NO
/// isolation when the platform's real sandbox is unavailable. Without it
/// the sandbox layer fails CLOSED (refuses execution) rather than silently
/// degrading to host-permission execution (audit M-2 / rel-concurrency-70).
const ALLOW_NO_SANDBOX_ENV: &str = "WAYLAND_ALLOW_NO_SANDBOX";

/// Env-var name selecting the sandbox backend (`none` / `docker`).
const SANDBOX_ENV: &str = "WAYLAND_SANDBOX";

/// Resolve the process-level compatibility backend selection. Hosted sessions
/// never call this path; they resolve config into an immutable
/// [`SandboxRegistry`] through [`SandboxRegistry::required_for_session`].
fn resolved_sandbox_choice() -> Option<String> {
    std::env::var(SANDBOX_ENV).ok()
}

/// True iff the operator has explicitly opted in to unsandboxed execution.
///
/// The compatibility path accepts only the process-start environment. Hosted
/// config cannot mutate this value; explicit local Dangerous authority is
/// carried by a per-session [`DangerousSessionGrant`].
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

#[derive(Clone)]
pub struct SandboxRegistry {
    backend: Arc<dyn backends::SandboxBackend>,
    /// Authority state, not a backend capability. Only `dangerous()` can set
    /// this after receiving an opaque resolver-issued session grant.
    bypasses_containment: bool,
    /// Immutable environment-variable passthrough authority for this
    /// session. Tool manifests read this snapshot instead of mutable
    /// process-global configuration.
    env_passthrough: Arc<HashSet<String>>,
}

impl SandboxRegistry {
    pub fn new(backend: Arc<dyn backends::SandboxBackend>) -> Self {
        Self {
            backend,
            bypasses_containment: false,
            env_passthrough: Arc::new(HashSet::new()),
        }
    }

    /// Attach the resolved environment passthrough allowlist to this session.
    pub fn with_env_passthrough<I, S>(mut self, var_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let names = var_names
            .into_iter()
            .filter_map(|name| {
                let trimmed = name.as_ref().trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect();
        self.env_passthrough = Arc::new(names);
        self
    }

    pub fn env_passthrough(&self) -> &HashSet<String> {
        &self.env_passthrough
    }
    pub async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        self.backend.execute(manifest, cmd).await
    }

    /// Validate external filesystem authority at the final registry boundary,
    /// immediately before the backend receives path-based grants.
    pub async fn execute_authorized<F>(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        authorize: F,
    ) -> Result<SandboxOutput>
    where
        F: FnOnce() -> Result<()>,
    {
        authorize()?;
        self.backend.execute(manifest, cmd).await
    }

    /// Execute against an owner-bound retained workspace with a hard import
    /// bound. The command's declared cwd must equal the retained checkout's
    /// display path, external authority is revalidated, and the retained
    /// workspace identity is re-proven before the backend receives it. Refuses
    /// unless the selected backend actually binds workspace authority — there
    /// is no path-based fallback for delegated mutation.
    pub async fn execute_with_workspace_authority<F>(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        workspace: RetainedWorkspaceAuthority,
        max_workspace_bytes: u64,
        authorize: F,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<SandboxOutput>
    where
        F: Fn() -> Result<()> + Send + Sync,
    {
        if cmd.cwd.as_deref() != Some(workspace.workspace().display_path()) {
            return Err(SandboxError::PathDenied(
                "sandbox command cwd does not match retained workspace authority".to_owned(),
            ));
        }
        authorize()?;
        workspace.validate()?;
        if !self.backend.binds_workspace_authority() {
            return Err(SandboxError::PolicyNotSupported(format!(
                "sandbox backend {} cannot bind retained workspace authority",
                self.backend.name()
            )));
        }
        self.backend
            .execute_with_workspace_authority(
                manifest,
                cmd,
                workspace,
                max_workspace_bytes,
                &authorize,
                cancel,
            )
            .await
    }

    /// Streaming execution — see [`backends::SandboxBackend::execute_streaming`].
    pub fn execute_streaming(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        Arc::clone(&self.backend).execute_streaming(manifest, cmd)
    }

    /// Streaming counterpart to [`Self::execute_authorized`]. Authority is
    /// checked before the backend receives the manifest or starts its task.
    pub fn execute_streaming_authorized<F>(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
        authorize: F,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>>
    where
        F: FnOnce() -> Result<()>,
    {
        authorize()?;
        Arc::clone(&self.backend).execute_streaming(manifest, cmd)
    }
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }
    pub fn is_available(&self) -> bool {
        self.backend.is_available()
    }
    pub fn enforces_read_deny(&self) -> bool {
        self.backend.enforces_read_deny()
    }
    pub fn owns_descendants_hard(&self) -> bool {
        self.backend.owns_descendants_hard()
    }
    pub fn binds_cwd_authority(&self) -> bool {
        self.backend.binds_cwd_authority()
    }
    pub fn binds_workspace_authority(&self) -> bool {
        self.backend.binds_workspace_authority()
    }
    pub fn bypasses_containment(&self) -> bool {
        self.bypasses_containment
    }
    pub fn blocks_powershell(&self) -> bool {
        self.backend.blocks_powershell()
    }

    /// Resolve one immutable, containment-required backend for an agent
    /// session. Environment may select another real backend (Docker), but
    /// neither environment nor persisted config may select `none`.
    pub fn required_for_session(config_backend: Option<&str>) -> Result<Self> {
        let choice = std::env::var(SANDBOX_ENV)
            .ok()
            .or_else(|| config_backend.map(str::to_owned));
        let normalized = choice.as_deref().map(str::trim).filter(|s| !s.is_empty());

        let backend: Box<dyn backends::SandboxBackend> = match normalized {
            None => real_platform_backend().unwrap_or_else(|| Box::new(FailClosedBackend::new())),
            Some("docker") => {
                use backends::SandboxBackend as _;
                let docker = backends::docker::DockerBackend::new();
                if docker.is_available() {
                    Box::new(docker)
                } else {
                    tracing::error!(
                        target: "wcore_sandbox",
                        "Docker was selected for this session but is unavailable; failing closed"
                    );
                    Box::new(FailClosedBackend::new())
                }
            }
            Some("none") => return Err(SandboxError::UnsafeBypassSource),
            Some(other) => return Err(SandboxError::UnknownBackend(other.to_string())),
        };

        if no_sandbox_opt_in() {
            tracing::warn!(
                target: "wcore_sandbox",
                "WAYLAND_ALLOW_NO_SANDBOX/config allow_no_sandbox is ignored for hosted sessions; \
                 containment bypass requires an explicit local Dangerous launch"
            );
        }
        Ok(Self::new(Arc::from(backend)))
    }

    /// Construct a production session runtime that deliberately has no OS
    /// sandbox. The private fields on `DangerousSessionGrant` and its lack of
    /// deserialization keep config/wire inputs away from this authority path.
    /// [`Self::new`] remains public for trusted host integration and tests;
    /// production launch code must use a validated policy constructor.
    pub fn dangerous(grant: &DangerousSessionGrant) -> Self {
        backends::no_sandbox::warn_once_sandbox_disabled();
        tracing::warn!(
            target: "wcore_sandbox",
            activation_id = grant.activation_id(),
            ttl_millis = grant.ttl_millis(),
            "Dangerous session runtime selected: OS sandbox is disabled"
        );
        Self {
            backend: Arc::new(backends::no_sandbox::NoSandboxBackend::new()),
            bypasses_containment: true,
            env_passthrough: Arc::new(HashSet::new()),
        }
    }

    /// Mint a one-use [`HardContainmentAuthority`] for a hard-contained
    /// execution.
    ///
    /// This is the ONLY constructor of the authority. It fails closed unless:
    /// 1. this registry does not bypass containment (a Dangerous / no-sandbox
    ///    runtime can never mint), AND
    /// 2. the selected backend passes a semantic LIVE probe of its EXACT
    ///    hard-containment mechanism under `fs`'s normalized policy — only the
    ///    qualifying bubblewrap / docker / AppContainer backends can, because
    ///    only they can construct the crate-private probe proof.
    ///
    /// The minted authority privately binds the backend, executable / runtime
    /// identity, mechanism, process-tree mechanism, normalized policy identity,
    /// and the exact spawn parameters. Any later drift refuses execution.
    pub async fn establish_hard_containment(
        &self,
        fs: &HardContainmentFilesystem,
        cmd: &SandboxCommand,
    ) -> Result<HardContainmentAuthority> {
        // An authority runtime that bypasses containment can NEVER mint hard
        // containment — a boolean/bypass source does not qualify.
        if self.bypasses_containment {
            return Err(SandboxError::UnsafeBypassSource);
        }
        // Live probe of the exact backend + normalized policy. Non-qualifying
        // backends fail closed here (default `PolicyNotSupported`).
        let probe = self.backend.probe_hard_containment(fs).await?;
        // Cross-check the live probe's identity against the backend's cheap
        // stable identity, so a backend that probes one mechanism cannot report
        // another. Absence of a stable identity after a probe fails closed.
        let cheap = self.backend.hard_containment_identity().ok_or_else(|| {
            SandboxError::ExecFailed(
                "backend produced a hard-containment probe but no stable identity".into(),
            )
        })?;
        if cheap != probe.identity {
            return Err(SandboxError::ExecFailed(
                "hard-containment probe identity disagreed with the backend identity".into(),
            ));
        }
        Ok(HardContainmentAuthority::mint(
            self.backend.name(),
            fs,
            cmd,
            probe,
        ))
    }

    /// Consume a [`HardContainmentAuthority`] and verify it still binds THIS
    /// registry's backend, the given normalized policy, and the exact spawn
    /// parameters. Any drift (backend, executable, runtime, mechanism, policy,
    /// or spawn parameters) refuses. Consuming the authority makes it one-use.
    pub fn verify_hard_containment(
        &self,
        authority: HardContainmentAuthority,
        fs: &HardContainmentFilesystem,
        cmd: &SandboxCommand,
    ) -> Result<()> {
        authority.verify_no_drift(&*self.backend, fs, cmd)
    }
}

/// Opaque, one-use proof that a specific backend live-probed its exact
/// hard-containment mechanism under an exact normalized policy, and that the
/// upcoming spawn matches what was probed.
///
/// Structural properties (all load-bearing):
/// - **Not serializable** (no `serde`) and **not cloneable / copyable** (no
///   `Clone` / `Copy`): it cannot be persisted, transported, or duplicated.
/// - **No public constructor:** the only mint is the crate-private [`mint`] fn,
///   reachable only through [`SandboxRegistry::establish_hard_containment`].
/// - **One-use:** [`Self::verify_no_drift`] takes `self` by value, so the
///   authority is consumed on use and cannot be checked (or reused) twice.
///
/// It privately binds the backend, executable / runtime identity, mechanism,
/// process-tree mechanism, normalized policy identity, and exact spawn
/// parameters captured at mint. This type makes NO gate-result, receipt,
/// candidate-acceptance, or landing claim — it is solely the containment
/// authority.
///
/// [`mint`]: HardContainmentAuthority::mint
pub struct HardContainmentAuthority {
    backend_name: &'static str,
    mechanism: backends::HardContainmentMechanism,
    executable_identity: String,
    runtime_identity: String,
    process_tree_mechanism: backends::process_tree::ProcessTreeMechanism,
    policy_identity: manifest::ContainmentPolicyIdentity,
    spawn_identity: SpawnIdentity,
}

impl std::fmt::Debug for HardContainmentAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redacted: this authority binds the executable/runtime identity, the
        // candidate + writable-root paths, and the exact spawn argv/cwd of a
        // contained execution. Only the non-sensitive backend name and
        // mechanism are shown so the capability's plan never leaks into logs.
        f.debug_struct("HardContainmentAuthority")
            .field("backend", &self.backend_name)
            .field("mechanism", &self.mechanism)
            .field("bound", &"<redacted>")
            .finish()
    }
}

/// The exact argv + cwd bound at mint. Compared by value at spawn.
#[derive(Debug, PartialEq, Eq)]
struct SpawnIdentity {
    argv: Vec<String>,
    cwd: Option<std::path::PathBuf>,
}

impl SpawnIdentity {
    fn from_command(cmd: &SandboxCommand) -> Self {
        Self {
            argv: cmd.argv.clone(),
            cwd: cmd.cwd.clone(),
        }
    }
}

impl HardContainmentAuthority {
    /// Crate-private mint. Not `pub`, so only [`SandboxRegistry`] (in this
    /// module) can construct the authority. Callers cannot fabricate one.
    fn mint(
        backend_name: &'static str,
        fs: &HardContainmentFilesystem,
        cmd: &SandboxCommand,
        probe: backends::HardContainmentProbe,
    ) -> Self {
        let identity = probe.identity;
        Self {
            backend_name,
            mechanism: identity.mechanism,
            executable_identity: identity.executable_identity,
            runtime_identity: identity.runtime_identity,
            process_tree_mechanism: identity.process_tree_mechanism,
            policy_identity: fs.policy_identity(),
            spawn_identity: SpawnIdentity::from_command(cmd),
        }
    }

    /// The hard-containment mechanism this authority is bound to.
    pub fn mechanism(&self) -> backends::HardContainmentMechanism {
        self.mechanism
    }

    /// Consume the authority and refuse on ANY drift between mint and spawn.
    ///
    /// Re-derives the backend's cheap identity (no spawn) and the policy /
    /// spawn identities, comparing each bound field. A mismatch — including a
    /// backend that no longer offers hard containment — returns a fail-closed
    /// error naming the field that drifted.
    pub fn verify_no_drift(
        self,
        backend: &dyn backends::SandboxBackend,
        fs: &HardContainmentFilesystem,
        cmd: &SandboxCommand,
    ) -> Result<()> {
        let refuse = |field: &str| {
            Err(SandboxError::ExecFailed(format!(
                "hard containment refused: {field} changed between mint and spawn"
            )))
        };
        if self.backend_name != backend.name() {
            return refuse("backend");
        }
        let identity = backend.hard_containment_identity().ok_or_else(|| {
            SandboxError::ExecFailed(
                "hard containment refused: backend no longer offers hard containment".into(),
            )
        })?;
        if identity.mechanism != self.mechanism {
            return refuse("mechanism");
        }
        if identity.executable_identity != self.executable_identity {
            return refuse("executable identity");
        }
        if identity.runtime_identity != self.runtime_identity {
            return refuse("runtime identity");
        }
        if identity.process_tree_mechanism != self.process_tree_mechanism {
            return refuse("process-tree mechanism");
        }
        if fs.policy_identity() != self.policy_identity {
            return refuse("normalized policy");
        }
        if SpawnIdentity::from_command(cmd) != self.spawn_identity {
            return refuse("spawn parameters");
        }
        Ok(())
    }
}

/// Return the real native backend when one is available. This helper never
/// consults process-global configuration and never falls back to NoSandbox.
fn real_platform_backend() -> Option<Box<dyn backends::SandboxBackend>> {
    #[cfg(target_os = "linux")]
    {
        use backends::SandboxBackend as _;
        let bwrap = backends::bwrap::BubblewrapBackend::new();
        if bwrap.is_available() {
            return Some(Box::new(bwrap));
        }
    }
    #[cfg(target_os = "macos")]
    {
        use backends::SandboxBackend as _;
        let sbx = backends::sandbox_exec::SandboxExecBackend::new();
        if sbx.is_available() {
            return Some(Box::new(sbx));
        }
    }
    #[cfg(target_os = "windows")]
    {
        use backends::SandboxBackend as _;
        let appc = backends::appcontainer::AppContainerBackend::new();
        if appc.is_available() {
            return Some(Box::new(appc));
        }
    }
    None
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
                return unsandboxed_fallback();
            }
            _ => {}
        }
    }
    real_platform_backend().unwrap_or_else(unsandboxed_fallback)
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
    fn required_session_rejects_environment_bypass_pair() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_sandbox(Some("none"));
        EnvGuard::set_allow(Some("1"));

        assert!(matches!(
            SandboxRegistry::required_for_session(None),
            Err(SandboxError::UnsafeBypassSource)
        ));
    }

    #[test]
    fn required_session_rejects_persisted_none() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_sandbox(None);
        EnvGuard::set_allow(None);

        assert!(matches!(
            SandboxRegistry::required_for_session(Some("none")),
            Err(SandboxError::UnsafeBypassSource)
        ));
    }

    #[test]
    fn session_runtimes_do_not_follow_later_global_changes() {
        use wcore_types::execution_policy::{
            ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
            resolve_dangerous_launch,
        };

        let _lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let _g = EnvGuard::capture();
        EnvGuard::set_sandbox(None);
        EnvGuard::set_allow(None);

        let required = SandboxRegistry::required_for_session(None).unwrap();
        let required_name = required.backend_name();
        assert_ne!(required_name, "no_sandbox");

        let baseline =
            BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default);
        let grant = resolve_dangerous_launch(
            &baseline,
            DangerousLaunchRequest::cli(60, "isolation-test"),
            10_000,
        )
        .unwrap();
        let dangerous = SandboxRegistry::dangerous(&grant);
        let unauthorised_no_sandbox =
            SandboxRegistry::new(Arc::new(backends::no_sandbox::NoSandboxBackend::new()));
        assert_eq!(dangerous.backend_name(), "no_sandbox");
        assert!(dangerous.bypasses_containment());
        assert!(!required.bypasses_containment());
        assert!(!unauthorised_no_sandbox.bypasses_containment());

        EnvGuard::set_sandbox(Some("none"));
        EnvGuard::set_allow(Some("1"));

        assert_eq!(required.backend_name(), required_name);
        assert_ne!(required.backend_name(), dangerous.backend_name());
        assert_eq!(dangerous.backend_name(), "no_sandbox");
    }

    #[test]
    fn environment_passthrough_is_owned_by_each_session_runtime() {
        let session_a = SandboxRegistry::new(Arc::new(FailClosedBackend::new()))
            .with_env_passthrough(["SESSION_A_ONLY", " SHARED "]);
        let session_b = SandboxRegistry::new(Arc::new(FailClosedBackend::new()))
            .with_env_passthrough(["SESSION_B_ONLY", "SHARED"]);

        assert!(session_a.env_passthrough().contains("SESSION_A_ONLY"));
        assert!(!session_a.env_passthrough().contains("SESSION_B_ONLY"));
        assert!(session_b.env_passthrough().contains("SESSION_B_ONLY"));
        assert!(!session_b.env_passthrough().contains("SESSION_A_ONLY"));
        assert!(session_a.env_passthrough().contains("SHARED"));
        assert!(session_b.env_passthrough().contains("SHARED"));
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
mod authority_boundary_tests {
    use super::*;
    use crate::backends::SandboxBackend;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingBackend(AtomicUsize);

    #[async_trait]
    impl SandboxBackend for CountingBackend {
        async fn execute(
            &self,
            _manifest: &SandboxManifest,
            _cmd: SandboxCommand,
        ) -> Result<SandboxOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(SandboxOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }

        fn name(&self) -> &'static str {
            "authority-counting"
        }

        fn is_available(&self) -> bool {
            true
        }
    }

    fn command() -> SandboxCommand {
        SandboxCommand {
            argv: vec!["must-not-run".to_owned()],
            cwd: None,
        }
    }

    fn replace_directory(path: &std::path::Path) {
        let original = path.with_extension("original");
        std::fs::rename(path, original).unwrap();
        std::fs::create_dir(path).unwrap();
    }

    #[tokio::test]
    async fn buffered_authority_rejects_same_path_replacement_before_backend() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let authority = DirectoryAuthority::open(&root).unwrap();
        replace_directory(&root);
        let backend = Arc::new(CountingBackend(AtomicUsize::new(0)));
        let registry = SandboxRegistry::new(backend.clone());

        let error = registry
            .execute_authorized(&SandboxManifest::default(), command(), || {
                authority.validate_path(&root)
            })
            .await
            .expect_err("same-path replacement reached buffered backend");

        assert!(error.to_string().contains("identity changed"), "{error}");
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn streaming_authority_rejects_same_path_replacement_before_backend() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let authority = DirectoryAuthority::open(&root).unwrap();
        replace_directory(&root);
        let backend = Arc::new(CountingBackend(AtomicUsize::new(0)));
        let registry = SandboxRegistry::new(backend.clone());

        let error = registry
            .execute_streaming_authorized(&SandboxManifest::default(), command(), || {
                authority.validate_path(&root)
            })
            .expect_err("same-path replacement reached streaming backend");

        assert!(error.to_string().contains("identity changed"), "{error}");
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod hard_containment_tests {
    use super::*;
    use crate::backends::process_tree::ProcessTreeMechanism;
    use crate::backends::{HardContainmentIdentity, HardContainmentProbe, SandboxBackend};

    /// A crate-private test double standing in for a qualifying backend. It can
    /// build the crate-private probe/identity types precisely BECAUSE it lives
    /// inside the crate — an external backend cannot, which is the structural
    /// seal. This double is never exported and grants no bypass outside the
    /// crate.
    struct QualBackend {
        name: &'static str,
        exec: String,
        mechanism: HardContainmentMechanism,
    }

    impl QualBackend {
        fn new(name: &'static str, exec: &str) -> Self {
            Self {
                name,
                exec: exec.to_owned(),
                mechanism: HardContainmentMechanism::BubblewrapPidNamespace,
            }
        }
    }

    #[async_trait]
    impl SandboxBackend for QualBackend {
        async fn execute(&self, _m: &SandboxManifest, _c: SandboxCommand) -> Result<SandboxOutput> {
            Ok(SandboxOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn is_available(&self) -> bool {
            true
        }
        fn hard_containment_identity(&self) -> Option<HardContainmentIdentity> {
            Some(HardContainmentIdentity {
                mechanism: self.mechanism,
                executable_identity: self.exec.clone(),
                runtime_identity: format!("runtime:{}", self.exec),
                process_tree_mechanism: ProcessTreeMechanism::LinuxPidNamespaceReap,
            })
        }
        async fn probe_hard_containment(
            &self,
            _fs: &HardContainmentFilesystem,
        ) -> Result<HardContainmentProbe> {
            Ok(HardContainmentProbe {
                identity: self.hard_containment_identity().unwrap(),
            })
        }
    }

    /// A backend that keeps a stable `name` but no longer offers hard
    /// containment (identity `None`) — models a backend whose live mechanism
    /// vanished between mint and spawn (e.g. the `bwrap` binary was removed). The
    /// name matches the minted authority, so the backend-name check passes and
    /// the identity-`None` fail-closed branch is what must refuse.
    struct VanishedBackend {
        name: &'static str,
    }

    #[async_trait]
    impl SandboxBackend for VanishedBackend {
        async fn execute(&self, _m: &SandboxManifest, _c: SandboxCommand) -> Result<SandboxOutput> {
            Ok(SandboxOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn is_available(&self) -> bool {
            true
        }
        fn hard_containment_identity(&self) -> Option<HardContainmentIdentity> {
            None
        }
    }

    /// A qualifying-looking backend whose live probe fails at a named stage. It
    /// reports a stable identity, so the failure is the PROBE, not the identity
    /// cross-check — modeling a process-tree failure stage that must fail closed.
    struct FailingProbe {
        stage: &'static str,
    }

    #[async_trait]
    impl SandboxBackend for FailingProbe {
        async fn execute(&self, _m: &SandboxManifest, _c: SandboxCommand) -> Result<SandboxOutput> {
            Ok(SandboxOutput {
                exit_code: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
                resource_limits: ResourceLimitEnforcement::Enforced,
            })
        }
        fn name(&self) -> &'static str {
            "failing-probe"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn hard_containment_identity(&self) -> Option<HardContainmentIdentity> {
            Some(HardContainmentIdentity {
                mechanism: HardContainmentMechanism::BubblewrapPidNamespace,
                executable_identity: "/probe".to_owned(),
                runtime_identity: "runtime:/probe".to_owned(),
                process_tree_mechanism: ProcessTreeMechanism::LinuxPidNamespaceReap,
            })
        }
        async fn probe_hard_containment(
            &self,
            _fs: &HardContainmentFilesystem,
        ) -> Result<HardContainmentProbe> {
            // Each stage maps to the fail-closed error the real backends return
            // after killing the owned process tree.
            Err(match self.stage {
                "timeout" => SandboxError::Timeout,
                "overflow" => SandboxError::OutputLimitExceeded { limit_bytes: 8 },
                other => SandboxError::ExecFailed(format!("hard-containment probe {other} failed")),
            })
        }
    }

    fn fs_fixture() -> HardContainmentFilesystem {
        HardContainmentFilesystem::new(
            std::path::PathBuf::from("/srv/wl-hard/candidate"),
            vec![std::path::PathBuf::from("/srv/wl-hard/scratch")],
        )
        .expect("fixture policy validates")
    }

    fn cmd_fixture() -> SandboxCommand {
        SandboxCommand {
            argv: vec!["/bin/echo".into(), "hi".into()],
            cwd: Some(std::path::PathBuf::from("/srv/wl-hard/candidate")),
        }
    }

    async fn mint(name: &'static str, exec: &str) -> (SandboxRegistry, HardContainmentAuthority) {
        let registry = SandboxRegistry::new(Arc::new(QualBackend::new(name, exec)));
        let authority = registry
            .establish_hard_containment(&fs_fixture(), &cmd_fixture())
            .await
            .expect("qualifying backend must mint");
        (registry, authority)
    }

    #[tokio::test]
    async fn qualifying_backend_mints_and_verifies_with_no_drift() {
        let (registry, authority) = mint("q", "/a").await;
        assert_eq!(
            authority.mechanism(),
            HardContainmentMechanism::BubblewrapPidNamespace
        );
        registry
            .verify_hard_containment(authority, &fs_fixture(), &cmd_fixture())
            .expect("no drift must verify");
    }

    #[tokio::test]
    async fn spawn_parameter_drift_refuses() {
        let (registry, authority) = mint("q", "/a").await;
        let drifted = SandboxCommand {
            argv: vec!["/bin/echo".into(), "TAMPERED".into()],
            cwd: Some(std::path::PathBuf::from("/srv/wl-hard/candidate")),
        };
        let err = registry
            .verify_hard_containment(authority, &fs_fixture(), &drifted)
            .expect_err("argv drift must refuse");
        assert!(err.to_string().contains("spawn parameters"), "{err}");
    }

    #[tokio::test]
    async fn policy_drift_refuses() {
        let (registry, authority) = mint("q", "/a").await;
        let other_fs = HardContainmentFilesystem::new(
            std::path::PathBuf::from("/srv/wl-hard/candidate"),
            vec![std::path::PathBuf::from("/srv/wl-hard/other-scratch")],
        )
        .unwrap();
        let err = registry
            .verify_hard_containment(authority, &other_fs, &cmd_fixture())
            .expect_err("policy drift must refuse");
        assert!(err.to_string().contains("normalized policy"), "{err}");
    }

    #[tokio::test]
    async fn executable_and_runtime_drift_refuses() {
        // Mint against exec "/a"; verify against a same-named backend whose
        // executable identity changed to "/b".
        let (_registry, authority) = mint("q", "/a").await;
        let drifted_backend = QualBackend::new("q", "/b");
        let err = authority
            .verify_no_drift(&drifted_backend, &fs_fixture(), &cmd_fixture())
            .expect_err("executable drift must refuse");
        assert!(err.to_string().contains("executable identity"), "{err}");
    }

    #[tokio::test]
    async fn backend_drift_refuses() {
        let (_registry, authority) = mint("q", "/a").await;
        let other = QualBackend::new("other", "/a");
        let err = authority
            .verify_no_drift(&other, &fs_fixture(), &cmd_fixture())
            .expect_err("backend drift must refuse");
        assert!(err.to_string().contains("backend"), "{err}");
    }

    #[tokio::test]
    async fn non_qualifying_backend_at_spawn_refuses() {
        // A backend that keeps the minted name but no longer offers hard
        // containment (identity None) must refuse — a probe-time success cannot
        // be spent against it. The same name passes the backend-name check, so
        // the identity-None branch is the one under test.
        let (_registry, authority) = mint("q", "/a").await;
        let vanished = VanishedBackend { name: "q" };
        let err = authority
            .verify_no_drift(&vanished, &fs_fixture(), &cmd_fixture())
            .expect_err("non-qualifying spawn backend must refuse");
        assert!(
            err.to_string()
                .contains("no longer offers hard containment"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn authority_debug_is_redacted() {
        // The opaque authority's Debug must not leak the contained execution's
        // plan (executable/runtime identity, bound paths, spawn argv/cwd).
        let (_registry, authority) = mint("q", "/secret/bwrap-path").await;
        let shown = format!("{authority:?}");
        assert!(shown.contains("<redacted>"), "{shown}");
        assert!(
            !shown.contains("/secret/bwrap-path"),
            "executable identity leaked: {shown}"
        );
        assert!(
            !shown.contains("runtime:"),
            "runtime identity leaked: {shown}"
        );
        assert!(!shown.contains("/bin/echo"), "spawn argv leaked: {shown}");
        assert!(!shown.contains("scratch"), "writable root leaked: {shown}");
        // Non-sensitive discriminants remain for diagnostics.
        assert!(shown.contains("HardContainmentAuthority"), "{shown}");
    }

    #[tokio::test]
    async fn non_qualifying_backends_cannot_mint() {
        // FailClosed and NoSandbox keep the trait default and structurally
        // cannot mint.
        for registry in [
            SandboxRegistry::new(Arc::new(FailClosedBackend::new())),
            SandboxRegistry::new(Arc::new(backends::no_sandbox::NoSandboxBackend::new())),
        ] {
            let err = registry
                .establish_hard_containment(&fs_fixture(), &cmd_fixture())
                .await
                .expect_err("non-qualifying backend must not mint");
            assert!(
                matches!(err, SandboxError::PolicyNotSupported(_)),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn bypass_registry_cannot_mint() {
        use wcore_types::execution_policy::{
            ApprovalPolicy, BaselineExecutionPolicy, DangerousLaunchRequest, PolicySource,
            resolve_dangerous_launch,
        };
        let baseline =
            BaselineExecutionPolicy::smart(ApprovalPolicy::Prompt, PolicySource::Default);
        let grant = resolve_dangerous_launch(
            &baseline,
            DangerousLaunchRequest::cli(60, "hard-containment-test"),
            10_000,
        )
        .unwrap();
        let dangerous = SandboxRegistry::dangerous(&grant);
        assert!(dangerous.bypasses_containment());
        let err = dangerous
            .establish_hard_containment(&fs_fixture(), &cmd_fixture())
            .await
            .expect_err("a containment-bypassing runtime must never mint");
        assert!(matches!(err, SandboxError::UnsafeBypassSource), "{err:?}");
    }

    #[tokio::test]
    async fn probe_failure_at_every_stage_fails_closed() {
        // Each stage models a process-tree failure point that must kill the
        // owned tree and fail closed; the boundary surfaces the fail-closed
        // error rather than a mint. (The real owned-tree teardown is covered by
        // the `required_live_*` tests in process_tree.rs.)
        for stage in [
            "spawn",
            "identity",
            "containment",
            "cancellation",
            "timeout",
            "overflow",
            "capture",
            "wait",
            "descendant-cleanup",
        ] {
            let registry = SandboxRegistry::new(Arc::new(FailingProbe { stage }));
            let err = registry
                .establish_hard_containment(&fs_fixture(), &cmd_fixture())
                .await
                .expect_err("a failed probe stage must fail closed");
            assert!(
                matches!(
                    err,
                    SandboxError::ExecFailed(_)
                        | SandboxError::Timeout
                        | SandboxError::OutputLimitExceeded { .. }
                ),
                "stage {stage} produced unexpected error: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn probe_identity_disagreement_fails_closed() {
        // A backend whose probe proof disagrees with its cheap identity cannot
        // mint — the registry cross-checks them.
        struct Disagree;
        #[async_trait]
        impl SandboxBackend for Disagree {
            async fn execute(
                &self,
                _m: &SandboxManifest,
                _c: SandboxCommand,
            ) -> Result<SandboxOutput> {
                unreachable!("execute is never reached for a rejected mint")
            }
            fn name(&self) -> &'static str {
                "disagree"
            }
            fn is_available(&self) -> bool {
                true
            }
            fn hard_containment_identity(&self) -> Option<HardContainmentIdentity> {
                Some(HardContainmentIdentity {
                    mechanism: HardContainmentMechanism::BubblewrapPidNamespace,
                    executable_identity: "/cheap".to_owned(),
                    runtime_identity: "runtime:/cheap".to_owned(),
                    process_tree_mechanism: ProcessTreeMechanism::LinuxPidNamespaceReap,
                })
            }
            async fn probe_hard_containment(
                &self,
                _fs: &HardContainmentFilesystem,
            ) -> Result<HardContainmentProbe> {
                Ok(HardContainmentProbe {
                    identity: HardContainmentIdentity {
                        mechanism: HardContainmentMechanism::DockerContainer,
                        executable_identity: "/probe".to_owned(),
                        runtime_identity: "runtime:/probe".to_owned(),
                        process_tree_mechanism: ProcessTreeMechanism::DockerContainerReap,
                    },
                })
            }
        }
        let registry = SandboxRegistry::new(Arc::new(Disagree));
        let err = registry
            .establish_hard_containment(&fs_fixture(), &cmd_fixture())
            .await
            .expect_err("probe/identity disagreement must fail closed");
        assert!(err.to_string().contains("disagreed"), "{err}");
    }
}
