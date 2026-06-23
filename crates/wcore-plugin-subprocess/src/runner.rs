//! v0.6.5 Task 3.2 — `SubprocessPluginRunner`.
//!
//! Spawns a native plugin binary, talks to it over JSON-Lines on stdin/stdout
//! ([`crate::rpc`]), and exposes a channel-based async API to the engine.
//!
//! ## Lifecycle
//!
//! 1. [`SubprocessPluginRunner::load`] spawns the binary from
//!    `manifest.runtime.subprocess.binary_path` (resolved against the
//!    plugin directory containing `plugin.toml`).
//! 2. Sends [`SubprocessVerb::Init`] → expects
//!    [`SubprocessResponseBody::InitResult`].
//! 3. Sends [`SubprocessVerb::ListTools`] → expects
//!    [`SubprocessResponseBody::ToolsList`]; the resulting
//!    [`LoadedSubprocessPlugin`] carries the tool list for the host
//!    apply-pipeline.
//! 4. While the runner lives, the engine calls
//!    [`SubprocessPluginRunner::call_tool`] for each tool invocation.
//! 5. [`SubprocessPluginRunner::shutdown`] sends
//!    [`SubprocessVerb::Shutdown`], awaits [`SubprocessResponseBody::Ack`],
//!    then waits up to 5s for the OS process to exit before SIGKILL.
//!
//! ## Security
//!
//! **Subprocess plugins inherit the engine's process privileges by default**
//! (cross-audit finding A7 / Q5). They are NOT a sandbox boundary — the OS
//! process has the same uid, env vars, file-system access, and network
//! reach as the host. The [`PluginAccessGate`] passed at load time still
//! gates host-callback invocations (memory, MCP, etc.), but anything the
//! plugin chooses to do *on its own* (raw syscalls, outbound HTTP, file
//! writes via its own `std::fs`) is fully privileged.
//!
//! Operators who want true isolation should run the engine itself inside a
//! container/sandbox. Plugin-level cgroups / namespaces are out of scope
//! for v0.6.5 and tracked as v0.7.x work.
//!
//! ## v0.6.5 Task 3.3 — Crash budget + restart policy
//!
//! Each [`LoadedSubprocessPlugin`] carries an `Arc<AtomicU8>` consecutive-
//! failure counter (mirroring the [`wcore_agent::plugins::PluginRunner`]
//! pattern from v0.6.5 Task 1.2). The following subprocess-level events
//! count as **one strike**:
//!
//! - Non-zero subprocess exit (or any broken-pipe / EOF on stdio).
//! - JSON-RPC parse error from the subprocess (malformed envelope).
//! - Tool-call timeout (`DEFAULT_RPC_TIMEOUT`, currently 30s).
//! - Subprocess-side protocol error reply (`SubprocessResponseBody::Error`).
//!
//! On strike, [`SubprocessPluginRunner::call_tool`] consults the counter:
//!
//! - If `counter < CRASH_THRESHOLD` (3), the runner restarts the subprocess
//!   in-place — backoff `100ms → 500ms → 2s` (one per strike), respawns the
//!   binary via the stored [`TransportFactory`], replays the Init + ListTools
//!   handshake, and **retries the failed call once** on the fresh instance.
//! - If `counter >= CRASH_THRESHOLD`, the call returns
//!   [`SubprocessPluginError::PermissionDenied`] with
//!   `"auto-disabled after 3 crashes"` and **does not respawn**.
//!
//! A successful `call_tool` resets the counter (consecutive-only semantics —
//! matches Task 1.2).
//!
//! ### State preservation across restart
//!
//! The engine-side registered tool set survives restart: the host apply
//! pipeline registered `LoadedSubprocessPlugin::tools` once at load time;
//! the runner restart only swaps the underlying transport + child handle,
//! and the engine continues routing `call_tool` requests to the same
//! `SubprocessPluginRunner` handle. The list of tools is **not** re-validated
//! across restart (we trust the plugin to honor its declared schema); a
//! divergent post-restart `ToolsList` is logged at `warn!` but does not
//! fail the restart. Schema-pin validation is a v0.6.7 follow-up.
//!
//! ### Engine-level crash counter integration
//!
//! For v0.6.5, crash tracking lives **inside** the `SubprocessPluginRunner`
//! (per-plugin `Arc<AtomicU8>`). The engine-level counter on
//! [`wcore_agent::plugins::PluginRunner`] from Task 1.2 is NOT wired into
//! this — the wiring lives in the engine adapter (Task 2.7's territory) and
//! lands as a v0.6.7 follow-up. Telemetry callers can read the current
//! count via [`SubprocessPluginRunner::crash_count`].
//!
//! ## Testing
//!
//! Real subprocess spawn is exercised in `tests/subprocess_e2e.rs` via
//! [`SubprocessPluginRunner::load_with_transport`] driven by a
//! `tokio::io::duplex()` pair — avoiding a real binary keeps CI fast and
//! cross-platform-safe. Wave 4 (Task 4.5) ships a real example plugin
//! that exercises actual process spawning end-to-end.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep, timeout};
use tracing::{debug, error, info, warn};
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_api::manifest::PluginManifest;

use crate::error::{Result, SubprocessPluginError};
use crate::rpc::{
    SubprocessRequest, SubprocessResponse, SubprocessResponseBody, SubprocessVerb, ToolDescriptor,
};

/// Default per-RPC timeout for subprocess plugin calls. Init/list happen on
/// the load path so this also bounds load time; tool calls reuse the same
/// budget. Override planned for v0.6.6 (per-manifest).
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Grace period for the subprocess to exit cleanly after `Shutdown`.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Security/reliability cap on a single newline-delimited line read from an
/// untrusted plugin subprocess's stdout (audit `rel-panic-68`/M-21). A
/// third-party plugin streaming an endless newline-free payload would
/// otherwise grow the read buffer until the host OOMs. On overflow the reader
/// drops the connection (drains pending callers → `WorkerTerminated`); the
/// crash budget then restarts or auto-disables the plugin. 8 MiB is far above
/// any legitimate JSON-RPC envelope.
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// v0.6.5 Task 3.3 — consecutive-crash threshold for auto-disable, matching
/// the [`wcore_agent::plugins::PluginRunner`] pattern from Task 1.2.
pub const CRASH_THRESHOLD: u8 = 3;

/// Minimal env vars forwarded to subprocess plugin child processes after
/// [`std::process::Command::env_clear`]. Everything else — including
/// `OPENAI_API_KEY`, `WAYLAND_VAULT_*`, `ANTHROPIC_*`, etc. — is withheld
/// (`WAYLAND_HOME` is an intentional exception below, forwarded so the child
/// resolves the same isolated profile — C3; the vault secret stays withheld).
/// Kept minimal: just enough for CLI tools to locate executables and
/// behave correctly under different locales on every supported OS.
///
/// Windows entries are mandatory — without `SYSTEMROOT`/`WINDIR`/etc. the
/// spawned child cannot initialise (CreateProcess returns
/// `ERROR_ENVVAR_NOT_FOUND` / 0xcb, observed v0.8.6 round 17). Mirrors
/// the same list in `wcore_mcp::transport::stdio::FORWARDED_ENV_VARS` and
/// `wcore_plugin_subprocess::mcp_bridge::FORWARDED_ENV_VARS` — keep all
/// three in sync when adding vars.
pub(crate) const FORWARDED_ENV_VARS: &[&str] = &[
    // Unix essentials
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "TZ",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "TMPDIR",
    // C3: the isolated-profile home. Engine-controlled children must resolve the
    // SAME profile as the parent — without this they fall back to the default
    // ~/.wayland (cross-profile leak). Non-secret path; the vault passphrase
    // (WAYLAND_VAULT_*) is never forwarded.
    "WAYLAND_HOME",
    // Windows essentials
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PSMODULEPATH",
    "TEMP",
    "TMP",
];

/// v0.6.5 Task 3.3 — backoff schedule per restart attempt, indexed by the
/// current strike count (1 → 100ms, 2 → 500ms, 3+ never restarts).
const RESTART_BACKOFF: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(500),
    Duration::from_millis(2000),
];

/// Output of a successful [`SubprocessPluginRunner::call_tool`].
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub stdout: String,
    pub structured: Option<serde_json::Value>,
    pub is_error: bool,
}

/// Result of [`SubprocessPluginRunner::load`] — the runner itself plus the
/// tools the plugin declared during init/list. The caller (host apply
/// pipeline in `wcore-agent`) registers these tools into the engine's
/// tool registry and keeps the runner alive for the plugin's lifetime.
pub struct LoadedSubprocessPlugin {
    pub runner: SubprocessPluginRunner,
    pub manifest_version: String,
    pub capabilities: Vec<String>,
    pub tools: Vec<ToolDescriptor>,
}

/// v0.6.5 Task 3.3 — pluggable transport spawner. Production
/// [`SubprocessPluginRunner::load`] supplies a factory that re-spawns the
/// configured binary; tests supply a factory that returns a fresh
/// [`tokio::io::duplex`] pair.
///
/// The factory is invoked once at initial load and once per restart.
/// Returning `Err` from inside `call_tool`'s restart loop counts as the same
/// strike that triggered restart (the runner does NOT increment a second
/// strike for a respawn failure) and yields
/// [`SubprocessPluginError::WorkerTerminated`] to the caller.
pub type TransportFactory =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<TransportSpawn>> + Send>> + Send + Sync>;

/// One spawn output from a [`TransportFactory`]: the stdin/stdout pair plus
/// the OS child handle (if any). `child = None` is used by duplex-based
/// tests.
pub struct TransportSpawn {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub child: Option<Child>,
}

/// Channel-based async handle to a running subprocess plugin.
///
/// The runner owns:
/// - one background tokio task that reads stdout line-by-line and routes
///   responses to in-flight request senders,
/// - a `Mutex` over the stdin half (writes serialize),
/// - the [`Child`] handle (for shutdown / kill),
/// - the access gate (for future host-callback validation),
/// - a per-runner [`Arc<AtomicU8>`] crash counter + a
///   [`TransportFactory`] for in-place restart (v0.6.5 Task 3.3).
pub struct SubprocessPluginRunner {
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SubprocessResponse>>>>,
    stdin: Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    child: Mutex<Option<Child>>,
    _gate: Arc<PluginAccessGate>,
    /// v0.6.5 Task 3.3 — consecutive-crash counter. Incremented on every
    /// transport/timeout/parse/protocol failure inside `call_tool`; reset to
    /// zero on the next successful tool call. When `>= CRASH_THRESHOLD` the
    /// runner refuses further calls until explicitly reset.
    crash_count: Arc<AtomicU8>,
    /// v0.6.5 Task 3.3 — invoked by `call_tool` after a strike to spawn a
    /// fresh transport + child for in-place restart. `None` indicates a
    /// non-restartable runner (legacy callers); such runners surface the
    /// original error without restarting.
    factory: Option<TransportFactory>,
    /// v0.6.5 Task 3.3 — restart serialization. Held for the duration of a
    /// respawn so concurrent `call_tool`s queue behind one another instead
    /// of each spawning their own replacement subprocess.
    restart_lock: Mutex<()>,
    /// Plugin name (for diagnostics + the auto-disabled error message).
    plugin_name: String,
}

/// Aud-18: resolve the binary path to spawn, binding execute to the
/// already-verified path when the loader provides one.
///
/// - `verified_binary == Some(p)`: the loader signature-verified `p` (a
///   canonicalized entry path). Re-derive `plugin_dir.join(binary_rel)`,
///   canonicalize it, and refuse to spawn unless it resolves to the same path.
///   Then spawn the VERIFIED path `p` (not the re-derived one), so the bytes
///   that run are the ones that were checked. A mismatch (e.g. a symlink swap
///   after verification) fails closed with `PermissionDenied`.
/// - `verified_binary == None`: legacy behavior — derive from the manifest.
pub(crate) fn resolve_verified_binary(
    plugin_name: &str,
    plugin_dir: &Path,
    binary_rel: &str,
    verified_binary: Option<&Path>,
) -> Result<std::path::PathBuf> {
    let derived = plugin_dir.join(binary_rel);
    let Some(verified) = verified_binary else {
        return Ok(derived);
    };
    // Canonicalize both sides and require equality. If the re-derived path no
    // longer canonicalizes to the verified path, the artifact was moved/swapped
    // between verify and spawn — refuse rather than execute unverified bytes.
    let derived_canon = derived.canonicalize().map_err(|e| {
        SubprocessPluginError::PermissionDenied(format!(
            "plugin {plugin_name}: cannot canonicalize spawn path {}: {e}",
            derived.display()
        ))
    })?;
    let verified_canon = verified
        .canonicalize()
        .unwrap_or_else(|_| verified.to_path_buf());
    if derived_canon != verified_canon {
        return Err(SubprocessPluginError::PermissionDenied(format!(
            "plugin {plugin_name}: spawn path {} does not match the \
             signature-verified path {} (artifact swapped after verification?)",
            derived_canon.display(),
            verified_canon.display()
        )));
    }
    Ok(verified_canon)
}

impl SubprocessPluginRunner {
    /// Spawn `manifest.runtime.subprocess.binary_path` (resolved against
    /// the directory containing `manifest_path`), perform the init →
    /// list-tools handshake, and return a [`LoadedSubprocessPlugin`].
    ///
    /// Aud-18 (verify-vs-execute TOCTOU): when the loader has already
    /// signature-verified a canonicalized entry path, it passes it as
    /// `verified_binary`. The runner then spawns THAT exact path instead of
    /// independently re-deriving `plugin_dir.join(binary_rel)`, and asserts the
    /// re-derived path canonicalizes to the verified one. This eliminates the
    /// gap where the bytes that were signed and the path that was spawned came
    /// from two separate resolutions (a swapped symlink/file in between would
    /// otherwise run unverified). `None` preserves the legacy
    /// derive-from-manifest behavior for callers without a verified path.
    pub async fn load(
        manifest_path: &Path,
        manifest: &PluginManifest,
        gate: Arc<PluginAccessGate>,
        verified_binary: Option<&Path>,
    ) -> Result<LoadedSubprocessPlugin> {
        let binary_rel = manifest
            .runtime
            .as_ref()
            .and_then(|r| r.subprocess.as_ref())
            .and_then(|s| s.binary_path.as_ref())
            .ok_or_else(|| {
                SubprocessPluginError::RpcParse(format!(
                    "plugin {}: manifest is missing [runtime.subprocess].binary_path",
                    manifest.plugin.name
                ))
            })?;
        let plugin_dir = manifest_path.parent().ok_or_else(|| {
            SubprocessPluginError::RpcParse(format!(
                "plugin {}: manifest_path has no parent directory",
                manifest.plugin.name
            ))
        })?;
        let binary = resolve_verified_binary(
            &manifest.plugin.name,
            plugin_dir,
            binary_rel,
            verified_binary,
        )?;
        let args: Vec<String> = manifest
            .runtime
            .as_ref()
            .and_then(|r| r.subprocess.as_ref())
            .map(|s| s.args.clone())
            .unwrap_or_default();

        // v0.6.5 Task 3.3 — build a restart factory that re-spawns the same
        // binary with the same args. Captured by Arc so call_tool can clone.
        let binary_for_factory = binary.clone();
        let args_for_factory = args.clone();
        let factory: TransportFactory = Arc::new(move || {
            let binary = binary_for_factory.clone();
            let args = args_for_factory.clone();
            Box::pin(async move { spawn_binary(&binary, &args).await })
        });

        let spawn = factory().await?;
        Self::handshake_over_transport(
            spawn,
            gate,
            &binary,
            Some(factory),
            manifest.plugin.name.clone(),
        )
        .await
    }

    /// Test seam — drive the runner over an arbitrary async transport
    /// (e.g. `tokio::io::duplex()`). Used by `tests/subprocess_e2e.rs` to
    /// exercise the RPC state machine without spawning a real process.
    ///
    /// Production code uses [`SubprocessPluginRunner::load`]; this entry
    /// point is `pub` so integration tests in sibling crates can reach it,
    /// but it is documented as test-only.
    #[doc(hidden)]
    pub async fn load_with_transport<W, R>(
        stdin: W,
        stdout: R,
        gate: Arc<PluginAccessGate>,
    ) -> Result<LoadedSubprocessPlugin>
    where
        W: AsyncWrite + Send + Unpin + 'static,
        R: AsyncRead + Send + Unpin + 'static,
    {
        let spawn = TransportSpawn {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            child: None,
        };
        Self::handshake_over_transport(
            spawn,
            gate,
            Path::new("<duplex>"),
            None,
            "<duplex>".to_string(),
        )
        .await
    }

    /// v0.6.5 Task 3.3 — test seam that wires up the restart factory. The
    /// factory is invoked at load *and* on each restart. Used by the new
    /// restart tests in `tests/subprocess_e2e.rs`.
    #[doc(hidden)]
    pub async fn load_with_factory(
        factory: TransportFactory,
        gate: Arc<PluginAccessGate>,
        plugin_name: impl Into<String>,
    ) -> Result<LoadedSubprocessPlugin> {
        let spawn = factory().await?;
        Self::handshake_over_transport(
            spawn,
            gate,
            Path::new("<factory>"),
            Some(factory),
            plugin_name.into(),
        )
        .await
    }

    async fn handshake_over_transport(
        spawn: TransportSpawn,
        gate: Arc<PluginAccessGate>,
        binary_for_logs: &Path,
        factory: Option<TransportFactory>,
        plugin_name: String,
    ) -> Result<LoadedSubprocessPlugin> {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SubprocessResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let binary_display = binary_for_logs.display().to_string();
        let reader_task = spawn_reader(spawn.stdout, Arc::clone(&pending), binary_display);

        let runner = SubprocessPluginRunner {
            next_id: AtomicU64::new(1),
            pending,
            stdin: Mutex::new(spawn.stdin),
            reader_task: Mutex::new(Some(reader_task)),
            child: Mutex::new(spawn.child),
            _gate: gate,
            crash_count: Arc::new(AtomicU8::new(0)),
            factory,
            restart_lock: Mutex::new(()),
            plugin_name,
        };

        let (manifest_version, capabilities, tools) = runner.handshake().await?;

        Ok(LoadedSubprocessPlugin {
            runner,
            manifest_version,
            capabilities,
            tools,
        })
    }

    /// Run the Init + ListTools handshake on the *current* transport. Used
    /// both at initial load and after a restart.
    async fn handshake(&self) -> Result<(String, Vec<String>, Vec<ToolDescriptor>)> {
        let init_resp = self.request(SubprocessVerb::Init).await?;
        let (manifest_version, capabilities) = match init_resp.body {
            SubprocessResponseBody::InitResult {
                manifest_version,
                capabilities,
            } => (manifest_version, capabilities),
            SubprocessResponseBody::Error { code, message, .. } => {
                return Err(SubprocessPluginError::ProtocolError { code, message });
            }
            other => {
                return Err(SubprocessPluginError::ResponseMismatch(format!(
                    "expected init_result, got {other:?}"
                )));
            }
        };

        let list_resp = self.request(SubprocessVerb::ListTools).await?;
        let tools = match list_resp.body {
            SubprocessResponseBody::ToolsList { tools } => tools,
            SubprocessResponseBody::Error { code, message, .. } => {
                return Err(SubprocessPluginError::ProtocolError { code, message });
            }
            other => {
                return Err(SubprocessPluginError::ResponseMismatch(format!(
                    "expected tools_list, got {other:?}"
                )));
            }
        };

        Ok((manifest_version, capabilities, tools))
    }

    /// Send a verb, await the matching response. Holds a slot in `pending`
    /// keyed by the monotonic request id.
    async fn request(&self, verb: SubprocessVerb) -> Result<SubprocessResponse> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let req = SubprocessRequest::new(id, verb);
        let line = serde_json::to_string(&req)
            .map_err(|e| SubprocessPluginError::RpcParse(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(id, tx);
        }

        // Write the request line + newline atomically under the stdin mutex.
        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
            if let Err(e) = stdin.write_all(b"\n").await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
            if let Err(e) = stdin.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(map_io_err(e));
            }
        }

        match timeout(DEFAULT_RPC_TIMEOUT, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                // Sender dropped before sending — reader task died or
                // pending map was cleared on EOF.
                Err(SubprocessPluginError::WorkerTerminated)
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(SubprocessPluginError::Timeout)
            }
        }
    }

    /// Invoke a tool by name. Returns the plugin's [`ToolOutput`] or a
    /// typed error.
    ///
    /// **v0.6.5 Task 3.3** — on a subprocess-level crash (broken pipe,
    /// timeout, parse error, protocol error, worker termination) the runner
    /// consults the crash budget:
    ///
    /// - `crash_count < CRASH_THRESHOLD`: restart the subprocess in-place
    ///   with a `100ms → 500ms → 2s` backoff (indexed by the new strike
    ///   count), then retry the call exactly once on the fresh instance.
    /// - `crash_count >= CRASH_THRESHOLD`: return
    ///   [`SubprocessPluginError::PermissionDenied`] with `"auto-disabled
    ///   after 3 crashes"` — the plugin stays disabled for the lifetime of
    ///   this `SubprocessPluginRunner`.
    ///
    /// A successful call resets the counter to zero (consecutive-only
    /// semantics — matches v0.6.5 Task 1.2's `PluginRunner` pattern).
    pub async fn call_tool(&self, name: &str, input: serde_json::Value) -> Result<ToolOutput> {
        // Hard short-circuit: once disabled, every call fails fast.
        if self.crash_count.load(Ordering::Acquire) >= CRASH_THRESHOLD {
            return Err(SubprocessPluginError::PermissionDenied(format!(
                "plugin {}: auto-disabled after {} crashes",
                self.plugin_name, CRASH_THRESHOLD
            )));
        }

        match self.call_tool_once(name, &input).await {
            Ok(out) => {
                // Reset on success — consecutive-only semantics.
                self.crash_count.store(0, Ordering::Release);
                Ok(out)
            }
            Err(err) if is_crash(&err) => {
                self.record_crash_and_maybe_restart(name, input, err).await
            }
            Err(other) => Err(other),
        }
    }

    /// Single attempt at `call_tool` — no restart logic. Factored out so
    /// the outer `call_tool` can wrap it with crash classification.
    async fn call_tool_once(&self, name: &str, input: &serde_json::Value) -> Result<ToolOutput> {
        let resp = self
            .request(SubprocessVerb::CallTool {
                name: name.to_string(),
                input: input.clone(),
            })
            .await?;
        match resp.body {
            SubprocessResponseBody::CallToolResult {
                stdout,
                structured,
                is_error,
            } => Ok(ToolOutput {
                stdout,
                structured,
                is_error,
            }),
            SubprocessResponseBody::Error { code, message, .. } => {
                Err(SubprocessPluginError::ProtocolError { code, message })
            }
            other => Err(SubprocessPluginError::ResponseMismatch(format!(
                "expected call_tool_result, got {other:?}"
            ))),
        }
    }

    /// Increment the crash counter; if still under threshold and we have a
    /// factory, restart the subprocess and retry exactly once. If at or
    /// over the threshold (or no factory), return the original error.
    async fn record_crash_and_maybe_restart(
        &self,
        name: &str,
        input: serde_json::Value,
        original_err: SubprocessPluginError,
    ) -> Result<ToolOutput> {
        let prev = self.crash_count.fetch_add(1, Ordering::AcqRel);
        let new = prev.saturating_add(1);
        warn!(
            plugin = %self.plugin_name,
            strike = new,
            threshold = CRASH_THRESHOLD,
            error = %original_err,
            "subprocess plugin crash"
        );

        // Threshold crossed — auto-disable. Original error is returned for
        // diagnostic value; the dedicated PermissionDenied lands on the
        // *next* call (per `call_tool`'s short-circuit).
        if new >= CRASH_THRESHOLD {
            warn!(
                plugin = %self.plugin_name,
                "auto-disabled after {} crashes",
                CRASH_THRESHOLD
            );
            return Err(original_err);
        }

        // No factory → not restartable (e.g. legacy `load_with_transport`
        // duplex callers). Bubble the original error up; subsequent calls
        // will keep incrementing until threshold.
        let Some(factory) = self.factory.clone() else {
            return Err(original_err);
        };

        // Serialize concurrent restart attempts — only one respawn per
        // strike, regardless of how many in-flight calls trip simultaneously.
        let _restart_guard = self.restart_lock.lock().await;

        // Backoff indexed by the new strike count (1 → 100ms, 2 → 500ms).
        let backoff_idx = (new as usize)
            .saturating_sub(1)
            .min(RESTART_BACKOFF.len() - 1);
        let backoff = RESTART_BACKOFF[backoff_idx];
        debug!(
            plugin = %self.plugin_name,
            strike = new,
            backoff_ms = backoff.as_millis() as u64,
            "subprocess restart backoff"
        );
        sleep(backoff).await;

        // Tear down the dead transport: kill old child + drain pending
        // senders so any racing in-flight requests get WorkerTerminated
        // instead of hanging.
        self.tear_down_transport().await;

        // Spawn a fresh transport. A respawn failure does NOT increment a
        // second strike (the strike was already taken for the underlying
        // crash); we just surface the original error to the caller.
        let spawn = match factory().await {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    plugin = %self.plugin_name,
                    error = %e,
                    "subprocess restart spawn failed"
                );
                return Err(original_err);
            }
        };

        // Swap in the new transport + reader task.
        let binary_display = format!("<restart:{}>", self.plugin_name);
        let new_reader = spawn_reader(spawn.stdout, Arc::clone(&self.pending), binary_display);
        {
            let mut stdin_guard = self.stdin.lock().await;
            *stdin_guard = spawn.stdin;
        }
        {
            let mut child_guard = self.child.lock().await;
            *child_guard = spawn.child;
        }
        {
            let mut reader_guard = self.reader_task.lock().await;
            *reader_guard = Some(new_reader);
        }

        // Replay the handshake. Failure here = the fresh subprocess died
        // immediately; surface the original error.
        if let Err(e) = self.handshake().await {
            warn!(
                plugin = %self.plugin_name,
                error = %e,
                "subprocess restart handshake failed"
            );
            return Err(original_err);
        }

        // Retry the original call exactly once on the new instance.
        match self.call_tool_once(name, &input).await {
            Ok(out) => {
                // Successful retry resets the counter — the prior strikes
                // are forgiven once forward progress resumes.
                self.crash_count.store(0, Ordering::Release);
                Ok(out)
            }
            Err(e) => {
                warn!(
                    plugin = %self.plugin_name,
                    error = %e,
                    "subprocess retry after restart still failed"
                );
                Err(e)
            }
        }
    }

    /// Kill the current child + drain pending senders. Safe to call when
    /// the transport is already dead. Used by the restart path *before*
    /// swapping in a new transport.
    async fn tear_down_transport(&self) {
        {
            let mut child_guard = self.child.lock().await;
            if let Some(mut child) = child_guard.take() {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }
        // Drain any in-flight pending senders so racing requests fail with
        // WorkerTerminated instead of hanging on a closed reader.
        {
            let mut pending = self.pending.lock().await;
            pending.clear();
        }
        // The old reader task will exit naturally on stdout EOF; we don't
        // need to await it here (would deadlock if it's already taken).
        let mut reader_guard = self.reader_task.lock().await;
        let _old = reader_guard.take();
    }

    /// v0.6.5 Task 3.3 — current consecutive-crash count. Exposed for
    /// telemetry; the engine-level counter integration is a v0.6.7 follow-up.
    pub fn crash_count(&self) -> u8 {
        self.crash_count.load(Ordering::Acquire)
    }

    /// Send `Shutdown`, await `Ack`, wait up to [`SHUTDOWN_GRACE`] for the
    /// OS process to exit; SIGKILL on timeout. Idempotent — safe to call
    /// multiple times; second call is a no-op.
    pub async fn shutdown(&self) -> Result<()> {
        // Best-effort Shutdown verb. If the plugin already died this errors,
        // but we still need to reap the child and join the reader.
        let shutdown_result = self.request(SubprocessVerb::Shutdown).await;
        match &shutdown_result {
            Ok(resp) => {
                if !matches!(resp.body, SubprocessResponseBody::Ack) {
                    warn!(
                        body = ?resp.body,
                        "subprocess plugin shutdown reply was not Ack — proceeding to kill"
                    );
                }
            }
            Err(e) => {
                warn!(error = %e, "subprocess plugin shutdown verb failed — proceeding to kill");
            }
        }

        // Reap child with grace period.
        let mut child_guard = self.child.lock().await;
        if let Some(mut child) = child_guard.take() {
            match timeout(SHUTDOWN_GRACE, child.wait()).await {
                Ok(Ok(_status)) => {}
                Ok(Err(e)) => {
                    warn!(error = %e, "subprocess wait() failed after shutdown");
                }
                Err(_) => {
                    warn!("subprocess did not exit within grace period — SIGKILL");
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
        }

        // Join the reader task; it should exit naturally on stdout close.
        let mut reader_guard = self.reader_task.lock().await;
        if let Some(handle) = reader_guard.take() {
            match timeout(Duration::from_secs(1), handle).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!(error = %e, "subprocess reader task join error"),
                Err(_) => warn!("subprocess reader task did not finish — leaking"),
            }
        }

        // Propagate the original shutdown error AFTER cleanup so callers
        // get diagnostic info but cleanup still happens.
        shutdown_result.map(|_| ())
    }
}

impl Drop for SubprocessPluginRunner {
    fn drop(&mut self) {
        // `kill_on_drop(true)` on the Command takes care of the OS process
        // when this runner is dropped without an explicit `shutdown()`.
        // Reader task is tied to the stdout pipe lifecycle and exits on EOF.
    }
}

/// v0.6.5 Task 3.3 — true iff this error class counts as a subprocess
/// "crash" for crash-budget purposes:
///
/// - [`SubprocessPluginError::BrokenPipe`] — stdin/stdout dropped (subprocess
///   exited or closed its pipes).
/// - [`SubprocessPluginError::WorkerTerminated`] — reader task died / pending
///   senders were drained on EOF.
/// - [`SubprocessPluginError::Timeout`] — per-call deadline exceeded.
/// - [`SubprocessPluginError::RpcParse`] — malformed envelope from the
///   subprocess.
/// - [`SubprocessPluginError::UnexpectedExit`] — non-zero subprocess exit.
/// - [`SubprocessPluginError::ProtocolError`] — subprocess-side domain error.
///   We treat this as a strike too: a plugin that consistently errors out
///   is wedged whether the cause is transport-level or schema-level.
///
/// Explicitly NOT a crash: [`SubprocessPluginError::PermissionDenied`]
/// (already disabled), [`SubprocessPluginError::ResponseMismatch`] (a host
/// bug, not a plugin crash), [`SubprocessPluginError::SpawnFailed`] (load
/// path; restart loop doesn't see this).
fn is_crash(err: &SubprocessPluginError) -> bool {
    matches!(
        err,
        SubprocessPluginError::BrokenPipe
            | SubprocessPluginError::WorkerTerminated
            | SubprocessPluginError::Timeout
            | SubprocessPluginError::RpcParse(_)
            | SubprocessPluginError::UnexpectedExit(_)
            | SubprocessPluginError::ProtocolError { .. }
    )
}

/// Spawn the stdout reader task. Lines are parsed as
/// [`SubprocessResponse`]s and routed to the matching `pending` sender
/// keyed by request id; on EOF/error, pending senders are drained so
/// in-flight `request` callers get [`SubprocessPluginError::WorkerTerminated`].
fn spawn_reader(
    stdout: Box<dyn AsyncRead + Send + Unpin>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SubprocessResponse>>>>,
    binary_display: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line: Vec<u8> = Vec::new();
        loop {
            line.clear();
            // Byte-capped read (audit rel-panic-68/M-21): a hostile/buggy
            // plugin streaming an endless newline-free payload would grow an
            // unbounded `read_line` buffer until the host OOMs. Cap the read at
            // MAX_LINE_BYTES; if we hit the cap without a terminating newline
            // the line is over-length — tear the transport down (drain pending
            // → WorkerTerminated) so the crash budget restarts/disables it.
            match (&mut reader)
                .take(MAX_LINE_BYTES)
                .read_until(b'\n', &mut line)
                .await
            {
                Ok(0) => {
                    debug!(binary = %binary_display, "subprocess stdout closed");
                    break;
                }
                Ok(_) => {
                    let over_length =
                        line.len() as u64 >= MAX_LINE_BYTES && line.last() != Some(&b'\n');
                    if over_length {
                        error!(
                            binary = %binary_display,
                            cap_bytes = MAX_LINE_BYTES,
                            "subprocess plugin sent over-length line (no newline within cap) — \
                             dropping transport"
                        );
                        break;
                    }
                    let trimmed = String::from_utf8_lossy(&line);
                    let trimmed = trimmed.trim_end_matches(['\n', '\r']);
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<SubprocessResponse>(trimmed) {
                        Ok(resp) => {
                            let id = resp.id;
                            let mut guard = pending.lock().await;
                            match guard.remove(&id) {
                                Some(tx) => {
                                    let _ = tx.send(resp);
                                }
                                None => {
                                    warn!(
                                        id,
                                        "subprocess plugin response for unknown id (dropping)"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            error!(error = %e, line = %trimmed, "subprocess plugin sent unparseable line");
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "subprocess stdout read error");
                    break;
                }
            }
        }
        // On EOF / error / over-length: drain pending senders so callers get a
        // typed error.
        let mut guard = pending.lock().await;
        guard.clear();
    })
}

/// Spawn the configured binary and return a [`TransportSpawn`] suitable
/// for [`SubprocessPluginRunner::handshake_over_transport`]. Used both at
/// initial load and (via the captured [`TransportFactory`]) on restart.
async fn spawn_binary(binary: &Path, args: &[String]) -> Result<TransportSpawn> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for var in FORWARDED_ENV_VARS {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
    info!(
        binary = %binary.display(),
        forwarded_env = ?FORWARDED_ENV_VARS,
        "subprocess plugin spawning with cleared env"
    );
    let mut child = cmd.spawn().map_err(SubprocessPluginError::SpawnFailed)?;

    let stdin = child
        .stdin
        .take()
        .ok_or(SubprocessPluginError::BrokenPipe)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(SubprocessPluginError::BrokenPipe)?;

    Ok(TransportSpawn {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        child: Some(child),
    })
}

fn map_io_err(e: std::io::Error) -> SubprocessPluginError {
    match e.kind() {
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof => {
            SubprocessPluginError::BrokenPipe
        }
        _ => SubprocessPluginError::RpcParse(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::FORWARDED_ENV_VARS;

    /// env_clear() — secret vars must NOT reach the child process.
    ///
    /// Spawns a real `/bin/sh` child and prints the value of
    /// `WAYLAND_TEST_SECRET_VAR`. Because `spawn_binary` calls
    /// `Command::env_clear()` before forwarding the allowlist, the child
    /// sees an empty string for any var not in `FORWARDED_ENV_VARS`.
    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_env_clear_blocks_secret_vars() {
        use std::path::Path;
        use tokio::io::AsyncReadExt;

        // Plant a secret in the engine's env.
        let secret_var = "WAYLAND_TEST_SECRET_VAR";
        let secret_val = "should-not-leak";
        // SAFETY: single-threaded test context; no concurrent env mutation.
        unsafe { std::env::set_var(secret_var, secret_val) };

        // Spawn `sh -c 'printf "%s" "$WAYLAND_TEST_SECRET_VAR"'` via
        // spawn_binary so the env_clear path is exercised.
        // Note: spawn_binary moves stdout into TransportSpawn.stdout, not the
        // child handle — read from result.stdout, not result.child.stdout.
        let mut result = super::spawn_binary(
            Path::new("/bin/sh"),
            &["-c".to_string(), format!("printf '%s' \"${secret_var}\"")],
        )
        .await
        .expect("spawn /bin/sh failed");

        let mut stdout_buf = String::new();
        result.stdout.read_to_string(&mut stdout_buf).await.unwrap();
        if let Some(mut child) = result.child {
            let _ = child.wait().await;
        }

        assert!(
            stdout_buf.is_empty(),
            "child process saw secret var: {stdout_buf:?}"
        );

        // SAFETY: single-threaded test context; no concurrent env mutation.
        unsafe { std::env::remove_var(secret_var) };
    }

    /// env_clear() — secret vars not in allowlist must be absent; PATH is in
    /// the allowlist so it is forwarded. Verified via `spawn_binary`'s stdout
    /// transport (stdout is moved into TransportSpawn, not the child handle).
    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_env_clear_blocks_non_allowlist_var_openai() {
        use std::path::Path;
        use tokio::io::AsyncReadExt;

        let secret_var = "OPENAI_API_KEY";
        let secret_val = "sk-test-should-not-leak";
        // SAFETY: single-threaded test context; no concurrent env mutation.
        unsafe { std::env::set_var(secret_var, secret_val) };

        let mut result = super::spawn_binary(
            Path::new("/bin/sh"),
            &["-c".to_string(), format!("printf '%s' \"${secret_var}\"")],
        )
        .await
        .expect("spawn /bin/sh failed");

        let mut stdout_buf = String::new();
        result.stdout.read_to_string(&mut stdout_buf).await.unwrap();
        if let Some(mut child) = result.child {
            let _ = child.wait().await;
        }

        assert!(
            stdout_buf.is_empty(),
            "child process saw OPENAI_API_KEY: {stdout_buf:?}"
        );

        // SAFETY: single-threaded test context; no concurrent env mutation.
        unsafe { std::env::remove_var(secret_var) };
    }

    /// C3: WAYLAND_HOME is forwarded so an engine-spawned child resolves the
    /// SAME isolated profile as the parent. Spawns a real `/bin/sh` that prints
    /// $WAYLAND_HOME and asserts the child sees the parent's value.
    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_forwards_wayland_home() {
        use std::path::Path;
        use tokio::io::AsyncReadExt;

        // SAFETY: single-threaded serial test context; no concurrent env mutation.
        unsafe { std::env::set_var("WAYLAND_HOME", "/tmp/isolated-profile-c3") };

        let mut result = super::spawn_binary(
            Path::new("/bin/sh"),
            &[
                "-c".to_string(),
                "printf '%s' \"$WAYLAND_HOME\"".to_string(),
            ],
        )
        .await
        .expect("spawn /bin/sh failed");

        let mut stdout_buf = String::new();
        result.stdout.read_to_string(&mut stdout_buf).await.unwrap();
        if let Some(mut child) = result.child {
            let _ = child.wait().await;
        }

        // SAFETY: single-threaded serial test context.
        unsafe { std::env::remove_var("WAYLAND_HOME") };

        assert_eq!(
            stdout_buf, "/tmp/isolated-profile-c3",
            "child must inherit the parent's WAYLAND_HOME (C3)"
        );
    }

    #[test]
    fn forwarded_env_vars_contains_expected_minimum() {
        assert!(FORWARDED_ENV_VARS.contains(&"PATH"));
        assert!(FORWARDED_ENV_VARS.contains(&"HOME"));
        assert!(FORWARDED_ENV_VARS.contains(&"USER"));
        assert!(FORWARDED_ENV_VARS.contains(&"LANG"));
        // C3 profile propagation.
        assert!(FORWARDED_ENV_VARS.contains(&"WAYLAND_HOME"));
    }

    /// Audit rel-panic-68/M-21: the stdout reader must NOT buffer an unbounded
    /// newline-free stream (OOM DoS from a third-party plugin). When a line
    /// exceeds `MAX_LINE_BYTES` with no terminating newline, the reader tears
    /// the transport down — draining pending senders so in-flight callers get
    /// `WorkerTerminated` instead of the host OOMing.
    #[tokio::test]
    async fn reader_drops_transport_on_overlong_line() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::io::AsyncWriteExt;
        use tokio::sync::{Mutex, oneshot};

        let (client, mut server) = tokio::io::duplex(64 * 1024);

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<super::SubprocessResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Register an in-flight request whose sender must be dropped on overflow.
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(1, tx);

        let reader = super::spawn_reader(
            Box::new(client),
            Arc::clone(&pending),
            "<overlong-test>".to_string(),
        );

        // Stream > MAX_LINE_BYTES of newline-free bytes from the "plugin".
        let writer = tokio::spawn(async move {
            let chunk = vec![b'a'; 256 * 1024];
            let mut written: u64 = 0;
            while written <= super::MAX_LINE_BYTES {
                if server.write_all(&chunk).await.is_err() {
                    break;
                }
                written += chunk.len() as u64;
            }
            // Keep the write half alive briefly so the reader hits the cap
            // before EOF rather than after a clean close.
            let _ = server.flush().await;
        });

        // The reader must finish (broke out of the loop) and the pending
        // sender must have been dropped → recv() errors.
        let join = tokio::time::timeout(std::time::Duration::from_secs(10), reader).await;
        assert!(join.is_ok(), "reader task did not terminate on overflow");
        assert!(
            rx.await.is_err(),
            "pending sender should have been dropped (caller gets WorkerTerminated)"
        );
        let _ = writer.await;
        assert!(
            pending.lock().await.is_empty(),
            "pending map should be drained on overflow"
        );
    }

    /// Sanity: a normal newline-terminated response under the cap is parsed and
    /// routed to the matching pending sender (the safe path is unchanged).
    #[tokio::test]
    async fn reader_routes_normal_response() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use tokio::io::AsyncWriteExt;
        use tokio::sync::{Mutex, oneshot};

        use crate::rpc::{SubprocessResponse, SubprocessResponseBody};

        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<SubprocessResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        let _reader = super::spawn_reader(
            Box::new(client),
            Arc::clone(&pending),
            "<normal-test>".to_string(),
        );

        // Minimal valid SubprocessResponse for id=7 (Ack body). The body is
        // `#[serde(flatten)]` + `tag = "kind"`, so this serializes flat as
        // `{"id":7,"kind":"ack"}`.
        let resp = SubprocessResponse::new(7, SubprocessResponseBody::Ack);
        let line = serde_json::to_string(&resp).unwrap();
        server
            .write_all(format!("{line}\n").as_bytes())
            .await
            .unwrap();
        server.flush().await.unwrap();

        let got = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await
            .expect("timed out waiting for routed response")
            .expect("sender dropped unexpectedly");
        assert_eq!(got.id, 7);
    }

    // -------------------------------------------------------------------------
    // Windows sibling for env_clear tests (Audit W-4 fix).
    // E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2: the two env_clear tests above are
    // #[cfg(unix)]-gated and use /bin/sh. On Windows, spawn_binary must also
    // env_clear() correctly — verified here using cmd.exe.
    //
    // These tests are #[cfg(windows)]-gated so macOS/Linux CI skips them.
    // -------------------------------------------------------------------------

    /// W-4: env_clear blocks secret vars on Windows via cmd.exe.
    /// spawn_binary must strip env vars not in FORWARDED_ENV_VARS on Windows.
    #[cfg(windows)]
    #[tokio::test]
    async fn subprocess_env_clear_blocks_secret_vars_windows() {
        use std::path::Path;
        use tokio::io::AsyncReadExt;

        let secret_var = "WAYLAND_TEST_SECRET_VAR_WIN";
        let secret_val = "should-not-leak-windows";
        // SAFETY: single-threaded test context.
        unsafe { std::env::set_var(secret_var, secret_val) };

        // `cmd.exe /C echo %VAR%` prints the var value or the literal "%VAR%"
        // if the var is unset. We check that the output does NOT contain the
        // secret value.
        let mut result = super::spawn_binary(
            Path::new("cmd.exe"),
            &["/C".to_string(), format!("echo %{secret_var}%")],
        )
        .await
        .expect("spawn cmd.exe failed");

        let mut stdout_buf = String::new();
        result.stdout.read_to_string(&mut stdout_buf).await.unwrap();
        if let Some(mut child) = result.child {
            let _ = child.wait().await;
        }

        assert!(
            !stdout_buf.contains(secret_val),
            "child process saw secret var on Windows: {stdout_buf:?}"
        );

        // SAFETY: single-threaded test context.
        unsafe { std::env::remove_var(secret_var) };
    }

    /// W-4: env_clear also blocks OPENAI_API_KEY on Windows.
    #[cfg(windows)]
    #[tokio::test]
    async fn subprocess_env_clear_blocks_openai_key_windows() {
        use std::path::Path;
        use tokio::io::AsyncReadExt;

        let secret_var = "OPENAI_API_KEY";
        let secret_val = "sk-windows-test-should-not-leak";
        // SAFETY: single-threaded test context.
        unsafe { std::env::set_var(secret_var, secret_val) };

        let mut result = super::spawn_binary(
            Path::new("cmd.exe"),
            &["/C".to_string(), format!("echo %{secret_var}%")],
        )
        .await
        .expect("spawn cmd.exe failed");

        let mut stdout_buf = String::new();
        result.stdout.read_to_string(&mut stdout_buf).await.unwrap();
        if let Some(mut child) = result.child {
            let _ = child.wait().await;
        }

        assert!(
            !stdout_buf.contains(secret_val),
            "child process saw OPENAI_API_KEY on Windows: {stdout_buf:?}"
        );

        // SAFETY: single-threaded test context.
        unsafe { std::env::remove_var(secret_var) };
    }
}
