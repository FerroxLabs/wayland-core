//! v0.6.5 Task 2.6 — `WasmPluginRunner`: the composition root for the WASM
//! plugin host.
//!
//! ## Lifecycle
//!
//! 1. [`WasmPluginRunner::new`] builds the shared wasmtime [`Engine`] via
//!    [`crate::runtime::build_engine`] and spawns the [`EpochTicker`].
//! 2. [`WasmPluginRunner::load`] reads the `.wasm` bytes, compiles them to
//!    a [`Component`], picks `Gated*` vs `Deny*` host adapters per the
//!    plugin's manifest permissions (Path X — see
//!    [`wcore_plugin_api::PluginPermissions`]), and returns a
//!    [`LoadedWasmPlugin`] holding both.
//! 3. [`LoadedWasmPlugin::call_tool`] **instantiates a fresh component
//!    per call** (Ironclaw pattern; see `ironclaw_wasm/src/runtime.rs:60-72`)
//!    so there is no shared mutable state between tool executions.
//!
//! ## Composition (Path X)
//!
//! The [`PluginAccessGate`] is currently a unit-struct holding only
//! associated `require_*(manifest)` helpers — it is NOT a stateful runtime
//! gate keyed by plugin name. Task 2.5 flagged this as a gap. Path X
//! (chosen here) closes the gap by adding five additive fields to
//! [`PluginPermissions`]:
//!
//! - `allow_network`
//! - `allow_workspace_read`
//! - `allow_workspace_write`
//! - `allow_tool_invoke`
//! - `permitted_secrets: Vec<String>`
//!
//! All default to `false` / empty via `#[serde(default)]` so every
//! existing manifest stays byte-compatible AND opts into nothing by
//! accident. The runner reads them at link time to pick `Gated*` vs
//! `Deny*` adapters. Default: fail-closed.
//!
//! ## Fresh-instance-per-call
//!
//! See [`LoadedWasmPlugin::call_tool`]. We hold the compiled
//! [`Component`] (cheap to clone — it's reference-counted internally
//! and shares JIT code through the [`Engine`]) plus the pre-wired
//! [`Linker`]; each call builds a new `Store<HostState>` + new instance.
//! No `Mutex<Store>`, no shared instance handles — by construction.
//!
//! ## Test fixture posture
//!
//! Task 4.4 ships an end-to-end `.wasm` fixture. For Task 2.6 we exercise
//! ONLY the composition logic via unit tests that compare which adapter
//! variants ended up on the [`HostState`] for a given manifest. Real
//! instantiation requires a real component; we don't ship one yet.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::debug;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker};
use wcore_plugin_api::access_gate::PluginAccessGate;
use wcore_plugin_api::manifest::PluginManifest;

use crate::error::{Result, WasmPluginError};
use crate::host_adapters::http::{
    DenyHostHttp, EmptySecretSource, GatedHostHttp, SecretSource, WaylandHostHttp,
};
use crate::host_adapters::log::{GatedHostLog, WaylandHostLog};
use crate::host_adapters::secrets::{DenyHostSecrets, GatedHostSecrets, WaylandHostSecrets};
use crate::host_adapters::tools::{DenyHostTools, GatedHostTools, ToolRegistry, WaylandHostTools};
use crate::host_adapters::workspace::{
    DenyHostWorkspace, GatedHostWorkspace, WaylandHostWorkspace,
};
use crate::limiter::{WasmPluginLimits, WasmResourceLimiter};
use crate::runtime::{EpochTicker, build_engine};

/// Output of a successful WASM tool invocation. Mirrors the
/// `tool.wit::response` record and the subprocess runner's
/// `ToolOutput` for cross-runtime uniformity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// Textual output. Maps onto the host-side `ToolResult::stdout`.
    pub stdout: String,
    /// Optional JSON-encoded structured result.
    pub structured: Option<String>,
    /// True when the plugin reported a tool-level error (distinct from
    /// a host-level trap — host traps surface as `WasmPluginError::*`).
    pub is_error: bool,
}

/// Composition-root state attached to every per-call `Store<HostState>`.
///
/// Holds the host-import adapter chosen for each capability at load time.
/// Adapters are `Arc`d because they outlive any single call and may be
/// shared across instances of the same plugin.
///
/// **Mutability boundary:** the runner clones `Arc<dyn …>` into a fresh
/// `HostState` for every `call_tool`. There is no shared mutable state
/// between calls — interior mutability lives inside each adapter, not in
/// this struct.
pub struct HostState {
    /// HTTP egress adapter.
    pub http: Box<dyn WaylandHostHttp>,
    /// Workspace filesystem adapter.
    pub workspace: Arc<dyn WaylandHostWorkspace>,
    /// Secret-existence adapter (existence-only by construction).
    pub secrets: Arc<dyn WaylandHostSecrets>,
    /// Tool-invocation adapter.
    pub tools: Arc<dyn WaylandHostTools>,
    /// Log adapter (always allowed; no `Deny*` variant).
    pub log: Arc<dyn WaylandHostLog>,
    /// Resource limiter attached to the per-call `Store` via
    /// `Store::limiter`. Wave 6B.2 — caps memory growth + instance/table
    /// counts per the manifest's plugin limits.
    pub limiter: WasmResourceLimiter,
}

impl std::fmt::Debug for HostState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostState").finish_non_exhaustive()
    }
}

/// Tag for which host-adapter variant was linked for a given capability.
/// Used by composition-only tests + tracing without exposing the trait
/// object's concrete type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdapterKind {
    /// Fail-closed `Deny*` adapter.
    Deny,
    /// Capability-gated `Gated*` adapter.
    Gated,
}

/// Record of which adapter variants the composition root selected for a
/// given plugin. Exposed on [`LoadedWasmPlugin`] so callers (and tests)
/// can verify the gating decision without inspecting trait-object types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterSelection {
    pub http: AdapterKind,
    pub workspace_read: AdapterKind,
    pub workspace_write: AdapterKind,
    pub secrets: AdapterKind,
    pub tools: AdapterKind,
}

/// Capability bundle passed to [`LoadedWasmPlugin::call_tool`]. Mirrors
/// `wcore_plugin_api::PluginToolCaps` semantically (cancellation,
/// call-id, source-agent) but is decoupled from the api crate's runtime
/// types so the wasm crate can stay free of `tokio_util::CancellationToken`
/// at the public seam. Task 2.7's loader translates `PluginToolCaps` into
/// this struct.
#[derive(Debug, Clone, Default)]
pub struct PluginToolCaps {
    /// Stable in-flight tool-call id (matches `ToolContext.call_id`).
    pub call_id: String,
    /// Originating sub-agent name; `None` = main agent.
    pub source_agent: Option<String>,
}

/// Metadata for a single tool exported by a loaded WASM plugin. Mirrors
/// `tool.wit::tool-metadata` so Task 2.7 can map directly to
/// `wcore_plugin_api::PluginTool` without information loss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmToolMetadata {
    pub name: String,
    pub description: String,
    pub input_schema: String,
    pub is_deferred: bool,
    pub max_result_size: u64,
}

/// Composition root for the WASM plugin host. Owns the shared
/// [`Engine`] + [`EpochTicker`]. Plugins are loaded into
/// [`LoadedWasmPlugin`] handles which carry the compiled
/// [`Component`] + per-plugin adapter selection.
pub struct WasmPluginRunner {
    engine: Engine,
    /// Kept alive for the lifetime of the runner so the engine's epoch
    /// keeps advancing; dropped (and joined) when the runner is dropped.
    _epoch_ticker: EpochTicker,
}

impl std::fmt::Debug for WasmPluginRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmPluginRunner").finish_non_exhaustive()
    }
}

impl WasmPluginRunner {
    /// Build a runner with a fresh wasmtime [`Engine`] and a running
    /// epoch ticker. Used by `wcore-agent` at engine bootstrap.
    pub fn new() -> Result<Self> {
        let engine = build_engine()?;
        let ticker = EpochTicker::start(engine.clone())?;
        Ok(Self {
            engine,
            _epoch_ticker: ticker,
        })
    }

    /// Engine handle (test/observability hook).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Load a WASM-component plugin from `component_path`. The manifest's
    /// `permissions` block drives the composition root — see the module
    /// docs for the Path X selection table.
    ///
    /// `gate` is the host-wide [`PluginAccessGate`]. Today it is a
    /// stateless unit struct; we hold an `Arc<PluginAccessGate>` on each
    /// adapter so the seam is in place for the day the gate becomes
    /// stateful (e.g. revocable tokens).
    pub fn load(
        &self,
        component_path: &Path,
        manifest: &PluginManifest,
        gate: Arc<PluginAccessGate>,
    ) -> Result<LoadedWasmPlugin> {
        let bytes = std::fs::read(component_path).map_err(|e| {
            WasmPluginError::LoadFailed(anyhow::Error::new(e).context(format!(
                "wasm plugin: read component bytes from {}",
                component_path.display()
            )))
        })?;
        self.load_from_bytes(&bytes, manifest, gate)
    }

    /// Variant for tests / in-memory components: compile directly from
    /// component bytes without touching the filesystem.
    pub fn load_from_bytes(
        &self,
        bytes: &[u8],
        manifest: &PluginManifest,
        gate: Arc<PluginAccessGate>,
    ) -> Result<LoadedWasmPlugin> {
        let component = Component::new(&self.engine, bytes).map_err(|e| {
            WasmPluginError::LoadFailed(e.context(format!(
                "wasm plugin: compile component '{}'",
                manifest.plugin.name
            )))
        })?;
        let workspace_root = component_workspace_root(manifest);
        Ok(LoadedWasmPlugin {
            engine: self.engine.clone(),
            component,
            plugin_name: manifest.plugin.name.clone(),
            gate,
            permissions: WasmPermissionsSnapshot::from_manifest(manifest),
            workspace_root,
            registry: Arc::new(ToolRegistry),
            secret_source: None,
            tools: Mutex::new(Vec::new()),
            limits: WasmPluginLimits::default(),
        })
    }
}

/// Snapshot of the WASM-relevant permission fields from
/// [`PluginPermissions`]. Held by [`LoadedWasmPlugin`] so adapter
/// selection is deterministic + testable without re-reading the manifest.
#[derive(Debug, Clone)]
pub struct WasmPermissionsSnapshot {
    pub allow_network: bool,
    pub allow_workspace_read: bool,
    pub allow_workspace_write: bool,
    pub allow_tool_invoke: bool,
    pub permitted_secrets: Vec<String>,
    pub http_allowlist: Vec<String>,
}

impl WasmPermissionsSnapshot {
    pub fn from_manifest(m: &PluginManifest) -> Self {
        Self {
            allow_network: m.permissions.allow_network,
            allow_workspace_read: m.permissions.allow_workspace_read,
            allow_workspace_write: m.permissions.allow_workspace_write,
            allow_tool_invoke: m.permissions.allow_tool_invoke,
            permitted_secrets: m.permissions.permitted_secrets.clone(),
            http_allowlist: m.permissions.http_allowlist.clone(),
        }
    }
}

/// A loaded WASM plugin. Holds the compiled [`Component`] (re-instantiated
/// per call — Ironclaw pattern) + the per-plugin adapter selection.
pub struct LoadedWasmPlugin {
    engine: Engine,
    component: Component,
    plugin_name: String,
    gate: Arc<PluginAccessGate>,
    permissions: WasmPermissionsSnapshot,
    workspace_root: PathBuf,
    registry: Arc<ToolRegistry>,
    /// Optional host-side secret store used by `GatedHostHttp` to expand
    /// `{{secret:NAME}}` tokens and to leak-scan response bodies. When
    /// `None`, an [`EmptySecretSource`] is used (no expansion, no values
    /// to scan for).
    secret_source: Option<Arc<dyn SecretSource>>,
    /// Cached tool metadata, populated lazily on first
    /// [`LoadedWasmPlugin::tools`] call (or by Task 2.7's loader after
    /// invoking the component's `metadata` export).
    tools: Mutex<Vec<WasmToolMetadata>>,
    /// Per-plugin resource limits (memory + fuel + epoch-deadline).
    /// Wave 6B.2 — fed into the per-call `Store` limiter + fuel + epoch.
    limits: WasmPluginLimits,
}

impl std::fmt::Debug for LoadedWasmPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedWasmPlugin")
            .field("plugin_name", &self.plugin_name)
            .field("permissions", &self.permissions)
            .finish_non_exhaustive()
    }
}

impl LoadedWasmPlugin {
    /// Plugin display name (matches `manifest.plugin.name`).
    pub fn name(&self) -> &str {
        &self.plugin_name
    }

    /// Attach a host-side [`SecretSource`] for `GatedHostHttp`. Wired by
    /// the engine bootstrap once the secret store is available. Default is
    /// [`EmptySecretSource`] (no values exposed in injection, no values to
    /// scan for in leak-scan).
    pub fn set_secret_source(&mut self, source: Arc<dyn SecretSource>) {
        self.secret_source = Some(source);
    }

    /// Snapshot of which adapter variants the composition root selected.
    /// Exposed for tests + observability; the actual trait objects are
    /// built fresh per call.
    pub fn adapter_selection(&self) -> AdapterSelection {
        AdapterSelection {
            http: if self.permissions.allow_network {
                AdapterKind::Gated
            } else {
                AdapterKind::Deny
            },
            workspace_read: if self.permissions.allow_workspace_read {
                AdapterKind::Gated
            } else {
                AdapterKind::Deny
            },
            workspace_write: if self.permissions.allow_workspace_write {
                AdapterKind::Gated
            } else {
                AdapterKind::Deny
            },
            // Secrets uses Gated when permitted_secrets is non-empty;
            // Deny (and therefore `secret_exists` returns false) otherwise.
            secrets: if self.permissions.permitted_secrets.is_empty() {
                AdapterKind::Deny
            } else {
                AdapterKind::Gated
            },
            tools: if self.permissions.allow_tool_invoke {
                AdapterKind::Gated
            } else {
                AdapterKind::Deny
            },
        }
    }

    /// Build a fresh [`HostState`] for one call. Composition root is
    /// fail-closed: every adapter defaults to its `Deny*` variant unless
    /// the manifest permission flag is true.
    pub fn build_host_state(&self) -> HostState {
        let plugin = self.plugin_name.clone();
        let gate = self.gate.clone();

        // ---- HTTP ----------------------------------------------------
        let http: Box<dyn WaylandHostHttp> = if self.permissions.allow_network {
            let secret_source: Arc<dyn SecretSource> = self
                .secret_source
                .clone()
                .unwrap_or_else(|| Arc::new(EmptySecretSource));
            Box::new(GatedHostHttp::new(
                gate.clone(),
                plugin.clone(),
                &self.permissions.http_allowlist,
                self.permissions.permitted_secrets.clone(),
                secret_source,
            ))
        } else {
            Box::new(DenyHostHttp)
        };

        // ---- Workspace ----------------------------------------------
        // GatedHostWorkspace owns one permission pair (read + write); we
        // attach it when EITHER flag is set so callers see the
        // appropriate per-op deny if only one is enabled.
        let workspace: Arc<dyn WaylandHostWorkspace> =
            if self.permissions.allow_workspace_read || self.permissions.allow_workspace_write {
                Arc::new(GatedHostWorkspace::new(
                    gate.clone(),
                    plugin.clone(),
                    self.workspace_root.clone(),
                    self.permissions.allow_workspace_read,
                    self.permissions.allow_workspace_write,
                ))
            } else {
                Arc::new(DenyHostWorkspace)
            };

        // ---- Secrets -------------------------------------------------
        let secrets: Arc<dyn WaylandHostSecrets> = if self.permissions.permitted_secrets.is_empty()
        {
            Arc::new(DenyHostSecrets)
        } else {
            Arc::new(GatedHostSecrets::new(
                gate.clone(),
                plugin.clone(),
                self.permissions.permitted_secrets.clone(),
            ))
        };

        // ---- Tool-invoke ---------------------------------------------
        let tools: Arc<dyn WaylandHostTools> = if self.permissions.allow_tool_invoke {
            Arc::new(GatedHostTools::new(
                gate.clone(),
                plugin.clone(),
                self.registry.clone(),
                true,
            ))
        } else {
            Arc::new(DenyHostTools)
        };

        // ---- Log -----------------------------------------------------
        let log: Arc<dyn WaylandHostLog> = Arc::new(GatedHostLog::new(plugin));

        HostState {
            http,
            workspace,
            secrets,
            tools,
            log,
            limiter: WasmResourceLimiter::from_limits(&self.limits),
        }
    }

    /// Invoke a tool by name.
    ///
    /// **Fresh-instance-per-call (Ironclaw lift).** Each call:
    /// 1. Builds a brand-new [`HostState`] from the selected adapters.
    /// 2. Builds a fresh `Store<HostState>` against the shared [`Engine`].
    /// 3. Compiles a new [`Linker`] and instantiates the [`Component`].
    /// 4. Calls the export, then drops the entire store.
    ///
    /// There is no shared mutable state between calls — by construction.
    ///
    /// **Wave 6B.1+6B.2 — full execute pipeline wired.** This method now:
    /// 1. Builds a fresh `HostState` + `Store<HostState>`.
    /// 2. Attaches `WasmResourceLimiter` via `store.limiter(...)`,
    ///    `store.set_fuel(...)`, and `store.set_epoch_deadline(...)` so
    ///    memory / fuel / wall-clock limits actually fire.
    /// 3. Builds a fresh [`Linker`] and registers every host import via
    ///    `WaylandTool::add_to_linker` (delegates each WIT method to the
    ///    `Gated*`/`Deny*` adapter held on `HostState`).
    /// 4. Instantiates the component asynchronously and dispatches
    ///    `tool.execute` with the WIT `Request` record.
    /// 5. Translates the response back to [`ToolOutput`].
    ///
    /// Closes v0.6.5 BLOCKER finding: the prior stub returned
    /// `ExecuteFailed("not yet wired (Task 2.7)")`. End-to-end execute
    /// now actually runs.
    pub async fn call_tool(
        &self,
        name: &str,
        input: &str,
        caps: PluginToolCaps,
    ) -> Result<ToolOutput> {
        // Fresh-instance-per-call: build a brand-new HostState + Store +
        // Linker + instance for every invocation. No shared mutable state.
        let host_state = self.build_host_state();
        let mut store = wasmtime::Store::new(&self.engine, host_state);

        // Wave 6B.2 — attach Store-level resource enforcement.
        store.limiter(|s: &mut HostState| &mut s.limiter);
        store
            .set_fuel(self.limits.fuel)
            .map_err(|e| WasmPluginError::ExecuteFailed(e.context("set_fuel")))?;
        // Epoch deadline counted in ticks of the engine's epoch. The
        // ticker increments once per EPOCH_TICK_INTERVAL (500 ms); convert
        // the wall-clock timeout to that unit, rounding up.
        let interval_ms = crate::runtime::EPOCH_TICK_INTERVAL.as_millis().max(1) as u64;
        let deadline_ticks = (self.limits.timeout_secs * 1000).div_ceil(interval_ms);
        store.set_epoch_deadline(deadline_ticks.max(1));

        // Linker: register every host import for the `wayland-tool` world.
        let mut linker: Linker<HostState> = Linker::new(&self.engine);
        crate::bindings::tool::WaylandTool::add_to_linker::<
            _,
            wasmtime::component::HasSelf<HostState>,
        >(&mut linker, |s: &mut HostState| s)
        .map_err(|e| WasmPluginError::ExecuteFailed(e.context("add_to_linker")))?;

        let bindings = crate::bindings::tool::WaylandTool::instantiate_async(
            &mut store,
            &self.component,
            &linker,
        )
        .await
        .map_err(|e| WasmPluginError::InstantiateFailed(e.context("instantiate_async")))?;

        let request = crate::bindings::tool::exports::wayland::host::tool::Request {
            input: input.to_string(),
            call_id: caps.call_id.clone(),
            source_agent: caps.source_agent.clone(),
        };

        debug!(
            plugin = %self.plugin_name,
            tool = %name,
            input_bytes = input.len(),
            "wasm call_tool: dispatching tool.execute"
        );

        let result = bindings
            .wayland_host_tool()
            .call_execute(&mut store, &request)
            .await
            .map_err(|e| WasmPluginError::ExecuteFailed(e.context("call_execute")))?;

        match result {
            Ok(response) => Ok(ToolOutput {
                stdout: response.stdout,
                structured: response.structured,
                is_error: response.is_error,
            }),
            Err(msg) => Err(WasmPluginError::ExecuteFailed(anyhow::anyhow!(
                "tool-level error: {msg}"
            ))),
        }
    }

    /// Tool metadata exported by this component.
    ///
    /// Populated by Task 2.7's loader after it invokes the component's
    /// `metadata` export. For Task 2.6 the cache starts empty; callers
    /// downstream (Task 2.7) `tools_set` to install the list once
    /// metadata has been pulled. We expose [`Self::set_tools`] so the
    /// loader can install the result without touching internals.
    pub async fn tools(&self) -> Vec<WasmToolMetadata> {
        self.tools.lock().await.clone()
    }

    /// Install the tool list pulled from the component's `metadata`
    /// export. Called by Task 2.7's loader after a successful load.
    pub async fn set_tools(&self, tools: Vec<WasmToolMetadata>) {
        let mut guard = self.tools.lock().await;
        *guard = tools;
    }
}

/// Pick a per-plugin workspace root, isolated from every other plugin.
///
/// Prior behavior (M-6/plugins-8): every plugin shared one fixed
/// `$TMPDIR/wayland/plugin-workspace` — no inter-plugin isolation and a
/// predictable world-reachable staging path that combined with the symlink
/// hole (M-5) to escape the sandbox.
///
/// Now we derive `<base>/wayland/plugin-workspace/<sanitized-name>-<hash>`,
/// where `<base>` prefers the user data dir and falls back to the temp dir.
/// The name component is sanitized + suffixed with a stable hash of the full
/// plugin name so two plugins cannot collide even after sanitization. The
/// directory tree is created with `0700` on unix. We refuse to use it if any
/// component is a pre-existing symlink or is owned by another uid (a local
/// attacker pre-planting a staging path).
fn component_workspace_root(manifest: &PluginManifest) -> PathBuf {
    let base = workspace_base_dir();
    let leaf = per_plugin_leaf(&manifest.plugin.name);
    let root = base.join("wayland").join("plugin-workspace").join(leaf);

    if let Err(e) = prepare_secure_dir(&root) {
        // Fail-soft on the path value but loudly: the workspace adapter still
        // canonicalizes + containment-checks every access (M-5), and an
        // unwritable/foreign root simply means reads/writes fail rather than
        // escaping. We surface the reason for operators.
        tracing::warn!(
            plugin = %manifest.plugin.name,
            root = %root.display(),
            error = %e,
            "wasm workspace: failed to prepare secure per-plugin root"
        );
    }
    root
}

/// Base directory for plugin workspaces. Prefers a per-user data dir over the
/// world-writable temp dir to shrink the local-attacker staging window.
fn workspace_base_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home).join(".local").join("share");
    }
    std::env::temp_dir()
}

/// Sanitize the plugin name into a single safe path component and append a
/// short stable hash of the full name so distinct plugins never collide.
fn per_plugin_leaf(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "plugin".to_string()
    } else {
        sanitized
    };
    format!("{sanitized}-{:016x}", stable_hash(name))
}

/// Stable, dependency-free hash (FNV-1a 64) of the full plugin name. Used only
/// for collision-avoidance of the directory leaf — not security-sensitive.
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Create `root` (and parents) with `0700`, refusing if any existing component
/// is a symlink or owned by a different uid.
fn prepare_secure_dir(root: &Path) -> std::io::Result<()> {
    // Reject if the final root already exists as a symlink.
    match std::fs::symlink_metadata(root) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "workspace root pre-exists as a symlink",
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    create_dir_all_secure(root)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::symlink_metadata(root)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::other("workspace root is a symlink"));
        }
        // Refuse a root owned by another uid (local-attacker pre-plant).
        let our_uid = unsafe { libc_getuid() };
        if meta.uid() != our_uid {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "workspace root owned by another uid",
            ));
        }
        // Tighten to 0700 in case it pre-existed wider.
        let mut perms = meta.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(root, perms)?;
    }
    Ok(())
}

/// `mkdir -p` with `0700` on the leaf-most directories we create. We create
/// parents permissively (they may be shared, e.g. `~/.local/share`) but lock
/// down the wayland-owned subtree.
fn create_dir_all_secure(root: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        // Create the public-ish base parents first without forcing 0700.
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Then create the final per-plugin root at 0700.
        match std::fs::DirBuilder::new().mode(0o700).create(root) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(root)
    }
}

/// Minimal libc `getuid` shim — avoids adding the `libc` crate dependency for
/// a single syscall. `getuid` never fails and has no errno semantics.
#[cfg(unix)]
unsafe fn libc_getuid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_plugin_api::manifest::{PluginInfo, PluginManifest, PluginPermissions};

    fn manifest_with(perms: PluginPermissions) -> PluginManifest {
        // Build via TOML to keep the path identical to the production
        // parser; if the schema ever rejects our test inputs, this
        // catches it earlier than runtime.
        let toml_str = format!(
            r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
description = "test"
entry = "test"
license = "Apache-2.0"

[permissions]
register_tools = {register_tools}
register_hooks = {register_hooks}
register_providers = {register_providers}
register_agents = {register_agents}
register_skills = {register_skills}
register_rules = {register_rules}
register_mcp_server = {register_mcp_server}
register_user_models = {register_user_models}
{tool_ns}
allow_network = {allow_network}
allow_workspace_read = {allow_workspace_read}
allow_workspace_write = {allow_workspace_write}
allow_tool_invoke = {allow_tool_invoke}
permitted_secrets = {permitted_secrets:?}
http_allowlist = {http_allowlist:?}
"#,
            register_tools = perms.register_tools,
            register_hooks = perms.register_hooks,
            register_providers = perms.register_providers,
            register_agents = perms.register_agents,
            register_skills = perms.register_skills,
            register_rules = perms.register_rules,
            register_mcp_server = perms.register_mcp_server,
            register_user_models = perms.register_user_models,
            tool_ns = perms
                .tool_namespace
                .as_ref()
                .map(|ns| format!("tool_namespace = \"{}\"", ns))
                .unwrap_or_default(),
            allow_network = perms.allow_network,
            allow_workspace_read = perms.allow_workspace_read,
            allow_workspace_write = perms.allow_workspace_write,
            allow_tool_invoke = perms.allow_tool_invoke,
            permitted_secrets = perms.permitted_secrets,
            http_allowlist = perms.http_allowlist,
        );
        PluginManifest::from_toml_str(&toml_str).expect("manifest must parse")
    }

    fn loaded(perms: PluginPermissions) -> LoadedWasmPlugin {
        // We don't have a real component here; for adapter-selection
        // tests we only need the manifest snapshot, not the wasm
        // compile. Build LoadedWasmPlugin directly.
        let engine = build_engine().expect("engine");
        let manifest = manifest_with(perms);
        // Empty WAT-style component is rejected without real bytes;
        // skip compile by constructing the inner state via the public
        // method that doesn't touch bytes — there is none, so for the
        // adapter-selection tests we cheat through a near-clone of the
        // load path that skips Component::new.
        //
        // Justification: `Component` cannot be constructed without a
        // valid wasm binary, but every test below only exercises
        // adapter selection + HostState construction — paths that do
        // not deref the Component. We therefore build the LoadedWasmPlugin
        // shell directly. If Component is ever needed by a test, swap to
        // load_from_bytes with a real fixture from Task 4.4.
        //
        // To keep this honest we exercise the real `load_from_bytes`
        // path with an obviously-invalid byte slice in `load_rejects_invalid_bytes`
        // below; the adapter-selection tests do NOT depend on a valid component.
        let minimal_component_bytes = wasm_empty_component_bytes();
        let runner = WasmPluginRunner {
            engine,
            _epoch_ticker: EpochTicker::start_with_interval(
                build_engine().expect("engine"),
                std::time::Duration::from_millis(20),
            )
            .expect("ticker"),
        };
        runner
            .load_from_bytes(
                &minimal_component_bytes,
                &manifest,
                Arc::new(PluginAccessGate),
            )
            .expect("load minimal component")
    }

    /// Minimal valid wasm-component bytes: the canonical 8-byte
    /// empty-component header. wasmtime 30.x accepts a component
    /// consisting of only the magic + version with zero sections.
    /// Verified against wasmtime 30.0.2 at task-2.6 implementation time.
    fn wasm_empty_component_bytes() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d, // \0asm magic
            0x0d, 0x00, 0x01, 0x00, // component model version: 0x000d0001
        ]
    }

    // --- Composition root: fail-closed default ---------------------

    #[test]
    fn defaults_all_deny_when_no_permissions() {
        let perms = PluginPermissions::default();
        let plugin = loaded(perms);
        let sel = plugin.adapter_selection();
        assert_eq!(sel.http, AdapterKind::Deny, "network defaults deny");
        assert_eq!(
            sel.workspace_read,
            AdapterKind::Deny,
            "workspace_read defaults deny"
        );
        assert_eq!(
            sel.workspace_write,
            AdapterKind::Deny,
            "workspace_write defaults deny"
        );
        assert_eq!(
            sel.secrets,
            AdapterKind::Deny,
            "secrets defaults deny (empty list)"
        );
        assert_eq!(sel.tools, AdapterKind::Deny, "tool_invoke defaults deny");
    }

    #[tokio::test]
    async fn host_state_is_deny_by_default() {
        let plugin = loaded(PluginPermissions::default());
        let state = plugin.build_host_state();
        // HTTP: deny.
        let err = state
            .http
            .http_request("https://example.com".into(), "GET".into(), vec![], vec![])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::host_adapters::http::HostHttpError::PermissionDenied
        ));
        // Workspace: deny on read and write.
        assert!(state.workspace.read("x").is_err());
        assert!(state.workspace.write("x", vec![1]).is_err());
        // Secrets: never exists.
        assert!(!state.secrets.secret_exists("ANY"));
        // Tools: deny.
        assert!(state.tools.tool_invoke("foo", "{}").is_err());
    }

    // --- Composition root: Gated when permission flag set ---------

    #[tokio::test]
    async fn network_permitted_selects_gated_http() {
        let p = PluginPermissions {
            allow_network: true,
            ..PluginPermissions::default()
        };
        let plugin = loaded(p);
        assert_eq!(plugin.adapter_selection().http, AdapterKind::Gated);
        // Gated http with an empty allowlist denies every request — the
        // gate is now real (no more "not implemented" stub).
        let state = plugin.build_host_state();
        let err = state
            .http
            .http_request("https://x.example/".into(), "GET".into(), vec![], vec![])
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            crate::host_adapters::http::HostHttpError::PermissionDenied
        ));
    }

    #[test]
    fn workspace_read_only_permission() {
        let p = PluginPermissions {
            allow_workspace_read: true,
            ..PluginPermissions::default()
        };
        let plugin = loaded(p);
        let sel = plugin.adapter_selection();
        assert_eq!(sel.workspace_read, AdapterKind::Gated);
        assert_eq!(sel.workspace_write, AdapterKind::Deny);
        // GatedHostWorkspace honors per-op denials even when attached.
        let state = plugin.build_host_state();
        assert!(state.workspace.write("x", vec![1]).is_err());
    }

    #[test]
    fn secrets_permitted_list_selects_gated() {
        let p = PluginPermissions {
            permitted_secrets: vec!["OPENAI_API_KEY".into()],
            ..PluginPermissions::default()
        };
        let plugin = loaded(p);
        assert_eq!(plugin.adapter_selection().secrets, AdapterKind::Gated);
        let state = plugin.build_host_state();
        assert!(state.secrets.secret_exists("OPENAI_API_KEY"));
        assert!(!state.secrets.secret_exists("OTHER"));
    }

    #[test]
    fn tool_invoke_permitted_selects_gated() {
        let p = PluginPermissions {
            allow_tool_invoke: true,
            ..PluginPermissions::default()
        };
        let plugin = loaded(p);
        assert_eq!(plugin.adapter_selection().tools, AdapterKind::Gated);
        let state = plugin.build_host_state();
        let err = state.tools.tool_invoke("alias", "{}").unwrap_err();
        assert!(err.contains("not yet wired"));
    }

    // --- Manifest backwards compatibility -------------------------

    #[test]
    fn existing_manifest_without_wasm_fields_parses() {
        // No allow_* or permitted_secrets fields: must still parse.
        let toml_str = r#"
[plugin]
name = "legacy"
version = "0.1.0"
description = "old"
entry = "x"
license = "MIT"

[permissions]
register_tools = true
tool_namespace = "legacy"
"#;
        let m = PluginManifest::from_toml_str(toml_str).expect("legacy manifest parses");
        assert!(!m.permissions.allow_network);
        assert!(!m.permissions.allow_workspace_read);
        assert!(!m.permissions.allow_workspace_write);
        assert!(!m.permissions.allow_tool_invoke);
        assert!(m.permissions.permitted_secrets.is_empty());
    }

    // --- Fresh-instance-per-call -----------------------------------

    #[tokio::test]
    async fn call_tool_constructs_fresh_state_per_call() {
        let plugin = loaded(PluginPermissions::default());
        // Two consecutive calls. The empty component used by this unit
        // test has no exports, so instantiate_async fails — what matters
        // is each invocation runs through fresh Store+Linker construction
        // without panicking, and both surface a typed `Err`.
        let caps = PluginToolCaps::default();
        let r1 = plugin.call_tool("noop", "{}", caps.clone()).await;
        let r2 = plugin.call_tool("noop", "{}", caps).await;
        assert!(
            matches!(
                r1,
                Err(WasmPluginError::InstantiateFailed(_) | WasmPluginError::ExecuteFailed(_))
            ),
            "expected typed err, got {r1:?}"
        );
        assert!(
            matches!(
                r2,
                Err(WasmPluginError::InstantiateFailed(_) | WasmPluginError::ExecuteFailed(_))
            ),
            "expected typed err, got {r2:?}"
        );
    }

    // --- PluginIdentity::Wasm round-trip --------------------------

    #[test]
    fn plugin_identity_wasm_variant_is_constructible() {
        use wcore_plugin_api::manifest::PluginIdentity;
        let id = PluginIdentity::Wasm {
            manifest_path: PathBuf::from("/tmp/plugin.toml"),
        };
        // Pattern-match to confirm the variant is exhaustive on its
        // public field set.
        match id {
            PluginIdentity::Wasm { manifest_path } => {
                assert_eq!(manifest_path, PathBuf::from("/tmp/plugin.toml"));
            }
            _ => panic!("expected Wasm variant"),
        }
    }

    // --- load_from_bytes rejects garbage --------------------------

    #[test]
    fn load_rejects_invalid_bytes() {
        let engine = build_engine().expect("engine");
        let runner = WasmPluginRunner {
            engine,
            _epoch_ticker: EpochTicker::start_with_interval(
                build_engine().expect("engine"),
                std::time::Duration::from_millis(20),
            )
            .expect("ticker"),
        };
        let manifest = manifest_with(PluginPermissions::default());
        let bad = b"not a wasm file";
        let res = runner.load_from_bytes(bad, &manifest, Arc::new(PluginAccessGate));
        assert!(matches!(res, Err(WasmPluginError::LoadFailed(_))));
    }

    // --- Per-plugin workspace isolation (M-6/plugins-8) -----------

    #[test]
    fn per_plugin_leaf_is_unique_per_name() {
        let a = per_plugin_leaf("vendor.alpha");
        let b = per_plugin_leaf("vendor.beta");
        assert_ne!(a, b, "distinct plugins must get distinct roots");
        // Sanitization: no path separators / dots leak into the leaf.
        assert!(!a.contains('.') && !a.contains('/'));
        // Deterministic for the same name.
        assert_eq!(per_plugin_leaf("vendor.alpha"), a);
    }

    #[test]
    fn per_plugin_leaf_distinguishes_after_sanitization() {
        // Two names that sanitize to the same prefix must NOT collide
        // (the hash suffix keeps them apart).
        let a = per_plugin_leaf("a.b");
        let b = per_plugin_leaf("a/b");
        assert_ne!(a, b);
    }

    #[test]
    #[cfg(unix)]
    fn prepare_secure_dir_creates_0700_and_rejects_symlink() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::fs::symlink;

        let base = tempfile::tempdir().unwrap();

        // Happy path: fresh dir is created 0700.
        let good = base
            .path()
            .join("wayland")
            .join("plugin-workspace")
            .join("p");
        prepare_secure_dir(&good).expect("secure dir");
        let mode = std::fs::symlink_metadata(&good)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "root must be 0700, got {mode:o}");

        // Pre-planted symlink root must be refused.
        let target = base.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        let evil = base.path().join("evil-root");
        symlink(&target, &evil).unwrap();
        let res = prepare_secure_dir(&evil);
        assert!(res.is_err(), "symlinked root must be refused");
    }

    // --- Smoke ---------------------------------------------------

    #[test]
    fn plugin_info_round_trip() {
        let info = PluginInfo {
            name: "x".into(),
            version: "0.0".into(),
            description: "y".into(),
            entry: "e".into(),
            authors: vec![],
            license: "MIT".into(),
            deferred: false,
        };
        assert_eq!(info.name, "x");
    }
}
