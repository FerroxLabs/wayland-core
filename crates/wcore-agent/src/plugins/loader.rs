//! `PluginLoader` — discovers built-in plugins via the api crate's inventory
//! slot and reconciles each against the host's `PluginsConfig`.
//!
//! ## v0.6.5 Task 3.2 — subprocess plugin identity
//!
//! Subprocess plugins are discovered from on-disk manifests (NOT inventory).
//! The canonical construction path is [`PluginIdentity::from_subprocess_path`]
//! — it reuses [`PluginIdentity::from_path_prefix`]'s allowed-roots check
//! and lifts the result into [`PluginIdentity::Subprocess`].
//!
//! The actual host wire-up that translates a `PluginIdentity::Subprocess`
//! into a running [`wcore_plugin_subprocess::SubprocessPluginRunner`] is
//! the responsibility of Task 2.7 (host runtime dispatch table) — that
//! table also covers `PluginIdentity::Wasm`. This file's discovery loop
//! still operates only on the `inventory` slot for v0.6.5; on-disk
//! subprocess/WASM plugin discovery lands with 2.7.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::VerifyingKey;
use wcore_config::plugins_config::PluginsConfig;
use wcore_plugin_api::McpServerSpec;
use wcore_plugin_api::manifest::PluginIdentity;
use wcore_plugin_api::registry::tools::NamespaceLedger;
use wcore_plugin_api::{
    Plugin, PluginAccessGate, PluginError, PluginManifest, PluginResult, plugin_inventory,
};
use wcore_plugin_subprocess::{LoadedMcpBridgePlugin, LoadedSubprocessPlugin};
use wcore_plugin_wasm::LoadedWasmPlugin;

use super::runner::PluginHook;

use crate::plugins::sig_verifier::{
    ENV_TRUST_UNSIGNED, KeySource, load_filesystem_keys, parse_verifying_key_b64, trusted_keys_dir,
    verify_path_plugin_signature,
};

/// v0.6.5 Task 2.7b — env override for the on-disk plugins root.
/// Mirrors the `WAYLAND_PLUGIN_TRUST_UNSIGNED` pattern from Task 1.3:
/// unset = use [`PluginIdentity::default_plugin_root`]; set = use the
/// override directory. Always honoured (including in tests) so fixtures
/// can point at a tempdir.
pub const ENV_PLUGINS_DIR: &str = "WAYLAND_PLUGINS_DIR";

/// A plugin that survived discovery + permission validation, ready to be
/// `initialize()`d by `PluginRunner`.
pub struct DiscoveredPlugin {
    pub name: String,
    pub manifest: PluginManifest,
    pub plugin: Box<dyn Plugin>,
}

impl DiscoveredPlugin {
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct PluginLoader<'a> {
    config: &'a PluginsConfig,
    discovered: Vec<DiscoveredPlugin>,
    /// v0.6.5 Task 2.7b — observable record of each on-disk plugin's
    /// dispatch outcome. Populated by [`PluginLoader::discover_on_disk`].
    /// Tests assert against this; production callers can inspect it for
    /// diagnostics. Empty when no on-disk plugins were found / when
    /// on-disk discovery hasn't run yet.
    on_disk_dispatches: Vec<OnDiskDispatchRecord>,
}

/// v0.6.5 Task 2.7b — outcome of routing one on-disk manifest through
/// the runtime-dispatch table. Carries enough provenance for tests to
/// assert that the right `*PluginRunner::load` was invoked AND for
/// host diagnostics to surface which manifest succeeded vs failed.
///
/// Wave 6A.1 — on successful load, `handle` carries the loaded runtime
/// handle so the bootstrap caller can pass it to the matching
/// `synthesize_initialize_outcome_*` adapter. On failure, `handle` is
/// `LoadedRuntimeHandle::None`.
pub struct OnDiskDispatchRecord {
    /// Manifest path that produced this dispatch.
    pub manifest_path: PathBuf,
    /// Plugin name as declared in the manifest.
    pub plugin_name: String,
    /// `tool_namespace` declared by the manifest, or the plugin name when
    /// none was declared. Used by the synthesizers to compute
    /// `<namespace>::<tool>` fq-names.
    pub tool_namespace: String,
    /// Which runner the dispatch went to.
    pub dispatch: RuntimeDispatch,
    /// `Ok(())` if the runner accepted the load; `Err(reason)` otherwise.
    /// The Err arm fires the crash-budget counter on the [`PluginRunner`]
    /// when the caller passes one in.
    pub load_result: Result<(), String>,
    /// Loaded runtime handle on success; `None` on failure or when the
    /// dispatch landed on `Static` (handled by the inventory path).
    pub handle: LoadedRuntimeHandle,
}

impl std::fmt::Debug for OnDiskDispatchRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnDiskDispatchRecord")
            .field("manifest_path", &self.manifest_path)
            .field("plugin_name", &self.plugin_name)
            .field("tool_namespace", &self.tool_namespace)
            .field("dispatch", &self.dispatch)
            .field("load_result", &self.load_result)
            .field("handle", &self.handle.kind_str())
            .finish()
    }
}

/// Wave 6A.1 — loaded runtime handle carried out of `discover_on_disk` so
/// the bootstrap caller can thread it into the matching
/// `synthesize_initialize_outcome_*` adapter. WASM + Subprocess use `Arc`
/// (closures clone the handle on every tool call); the MCP-bridge handle
/// is taken by value into the synthesizer.
pub enum LoadedRuntimeHandle {
    None,
    Wasm(Arc<LoadedWasmPlugin>),
    Subprocess(Arc<LoadedSubprocessPlugin>),
    McpBridge(LoadedMcpBridgePlugin),
    /// Path B step 1 — declarative on-disk plugin. Plain data: the parsed
    /// lifecycle hooks and optional MCP server spec, threaded into
    /// `plugin_outcome.hooks` / `plugin_outcome.mcp_servers` by bootstrap so
    /// the existing C1 dispatcher binds + fires them. No Arc — there is no
    /// running runtime to keep alive.
    Declarative {
        hooks: Vec<PluginHook>,
        mcp_server: Option<McpServerSpec>,
    },
}

impl std::fmt::Debug for LoadedRuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.kind_str())
    }
}

impl LoadedRuntimeHandle {
    fn kind_str(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Wasm(_) => "Wasm",
            Self::Subprocess(_) => "Subprocess",
            Self::McpBridge(_) => "McpBridge",
            Self::Declarative { .. } => "Declarative",
        }
    }
}

impl<'a> PluginLoader<'a> {
    /// Walk the inventory slot, instantiate each plugin via its factory, and
    /// filter by `PluginsConfig.is_enabled(name)`.
    ///
    /// v0.6.5 Task 1.3 (fixup): when
    /// `config.plugin_signature_verification` is `true`, each path-based
    /// plugin's artifact is verified against the UNION of filesystem-side
    /// trusted keys (`*.pub` files in the trust-anchor directory) and
    /// config-side trusted keys (`config.trusted_plugin_keys`). Static
    /// plugins (`plugin_path() == None`) always skip — the engine binary
    /// is their trust anchor.
    ///
    /// Returns `Err` from `try_discover` only when verification is on AND
    /// no key sources of any kind are available AND the
    /// `WAYLAND_PLUGIN_TRUST_UNSIGNED` escape is unset — that config
    /// blocks every path-based plugin with no indication why.
    pub fn try_discover(config: &'a PluginsConfig) -> PluginResult<Self> {
        if config.plugin_signature_verification {
            let trust_unsigned = std::env::var(ENV_TRUST_UNSIGNED)
                .map(|v| !v.is_empty() && v != "0")
                .unwrap_or(false);
            if !trust_unsigned && config.trusted_plugin_keys.is_empty() {
                let has_fs_keys = match trusted_keys_dir() {
                    Some(d) => !load_filesystem_keys(&d).is_empty(),
                    None => false,
                };
                if !has_fs_keys {
                    return Err(PluginError::ConfigError(
                        "plugin_signature_verification is enabled but no trusted keys \
                         are available — add at least one base64 ed25519 public key to \
                         trusted_plugin_keys in plugins.toml, drop *.pub files into \
                         the trust-anchor directory (default ~/.wayland/trusted-keys, \
                         override via WAYLAND_TRUSTED_KEYS_DIR), disable \
                         plugin_signature_verification, or set \
                         WAYLAND_PLUGIN_TRUST_UNSIGNED=1 (DEV ONLY)."
                            .to_string(),
                    ));
                }
            }
        }
        Ok(Self::discover(config))
    }

    pub fn discover(config: &'a PluginsConfig) -> Self {
        // Build the union of trusted keys once per discovery cycle.
        let union_keys = build_trusted_key_union(config);

        // v0.6.5 Task 1.3 (fixup): path-based plugins are verified when the
        // master flag is on. Static plugins (plugin_path() == None) ALWAYS
        // skip — the engine binary itself is their trust anchor.
        let trust_unsigned = std::env::var(ENV_TRUST_UNSIGNED)
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);
        if trust_unsigned {
            tracing::warn!(
                env = ENV_TRUST_UNSIGNED,
                "WAYLAND_PLUGIN_TRUST_UNSIGNED is set — unsigned path-based plugins will be loaded \
                 (DEV ONLY; do not enable in production)"
            );
        }

        let mut discovered = Vec::new();
        for factory in plugin_inventory::iter() {
            let name = factory.name();
            if !config.is_enabled(name) {
                tracing::info!(plugin = name, "plugin disabled by plugins.toml — skipping");
                continue;
            }

            if config.plugin_signature_verification
                && let Err(e) =
                    enforce_path_signing(name, factory.plugin_path(), trust_unsigned, &union_keys)
            {
                tracing::error!(plugin = name, error = %e, "plugin rejected: signature verification failed");
                continue;
            }

            let plugin = factory.build();
            let manifest = plugin.manifest().clone();
            discovered.push(DiscoveredPlugin {
                name: name.to_string(),
                manifest,
                plugin,
            });
        }
        Self {
            config,
            discovered,
            on_disk_dispatches: Vec::new(),
        }
    }

    /// v0.6.5 Task 2.7b — walk the on-disk plugins root and route each
    /// manifest through the runtime-dispatch table built in Task 2.7.
    ///
    /// Root resolution (see [`resolved_plugins_roots`]):
    /// 1. `$WAYLAND_PLUGINS_DIR` (if set + non-empty) → that dir ONLY (override).
    /// 2. Otherwise → both `<data_dir>/wayland/plugins` AND
    ///    `profile_home()/plugins` (the C3 profile home), scanned in order.
    ///
    /// Each root is its own security anchor: `allowed_roots` for the
    /// path-prefix gate is built from THAT root's canonicalization, and the
    /// per-entry symlink defense applies per root. A non-existent root is
    /// skipped with a debug log.
    ///
    /// For each `<root>/<plugin-name>/plugin.toml`:
    /// 1. Parse the manifest via [`PluginManifest::from_toml_str`] (which
    ///    runs the `plugin_api_version` + `[runtime].kind` validation).
    /// 2. Build a [`PluginIdentity`] from the manifest path
    ///    ([`PluginIdentity::from_subprocess_path`] for subprocess /
    ///    mcp-bridge; [`PluginIdentity::Wasm`] for wasm).
    /// 3. Call [`classify_runtime`] to pick the dispatch variant.
    /// 4. Reuse Task 1.3's [`enforce_path_signing`] against the entry
    ///    binary/component for any path-based variant.
    /// 5. Invoke the matching `*PluginRunner::load`. Failures are recorded
    ///    in [`Self::on_disk_dispatches`] AND increment the runner's
    ///    crash-budget counter (Task 1.2).
    ///
    /// Path-based plugins discovered here are NOT instantiated as `Plugin`
    /// trait objects; the static-plugin path (inventory factory →
    /// `Plugin::initialize`) does not apply. Synthesizing
    /// `InitializeOutcome` for the loaded handle is the wasm_adapter /
    /// subprocess_adapter / mcp_bridge_adapter responsibility — which
    /// the host wires in via a follow-up call after this method returns.
    pub async fn discover_on_disk(
        &mut self,
        runner: &super::runner::PluginRunner,
        wasm_runner: Option<&wcore_plugin_wasm::WasmPluginRunner>,
        gate: Arc<PluginAccessGate>,
    ) {
        let roots = resolved_plugins_roots();
        if roots.is_empty() {
            tracing::debug!("on-disk plugins discovery skipped: no plugins root resolved");
            return;
        }

        let union_keys = build_trusted_key_union(self.config);
        let trust_unsigned = std::env::var(ENV_TRUST_UNSIGNED)
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false);

        // Track plugin names seen across roots so a duplicate (same plugin in
        // two roots) is observable. Dedup/last-wins downstream is unchanged.
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        for root in roots {
            if !root.exists() {
                tracing::debug!(root = %root.display(), "on-disk plugins root does not exist — skipping");
                continue;
            }

            // `allowed_roots` is anchored to THIS root only. Each root is its
            // own security boundary: we never widen the path-prefix check
            // across roots. We canonicalise when possible so that
            // `from_path_prefix`'s starts_with check accepts manifests below
            // `root`.
            let allowed_roots = vec![root.canonicalize().unwrap_or_else(|_| root.clone())];

            let entries = match std::fs::read_dir(&root) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        root = %root.display(),
                        error = %e,
                        "on-disk plugins discovery: read_dir failed"
                    );
                    continue;
                }
            };

            for entry in entries.flatten() {
                let plugin_dir = entry.path();

                // v0.6.5 Task 6B.4 — symlink defense: skip any entry that is a
                // symlink. A malicious symlink in the plugins root could point
                // at an attacker-controlled directory outside `allowed_roots`
                // and bypass the path-prefix canonicalization check below.
                // Fail-closed on file_type errors (treat as suspect → skip).
                match entry.file_type() {
                    Ok(ft) if ft.is_symlink() => {
                        tracing::warn!(
                            path = %plugin_dir.display(),
                            "skipping symlink in plugins dir for security"
                        );
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %plugin_dir.display(),
                            error = %e,
                            "skipping plugins-dir entry: file_type() failed"
                        );
                        continue;
                    }
                    _ => {}
                }

                if !plugin_dir.is_dir() {
                    continue;
                }
                let manifest_path = plugin_dir.join("plugin.toml");
                if !manifest_path.is_file() {
                    continue;
                }

                let record = self
                    .dispatch_one_on_disk(
                        &manifest_path,
                        &allowed_roots,
                        trust_unsigned,
                        &union_keys,
                        runner,
                        wasm_runner,
                        gate.clone(),
                    )
                    .await;

                if !seen_names.insert(record.plugin_name.clone()) {
                    tracing::debug!(
                        plugin = %record.plugin_name,
                        root = %root.display(),
                        "duplicate plugin name discovered across plugins roots — last wins"
                    );
                }

                if record.load_result.is_err() {
                    runner.record_failure(&record.plugin_name);
                }
                self.on_disk_dispatches.push(record);
            }
        }
    }

    /// Helper: route a single on-disk manifest. Extracted so the caller
    /// loop stays readable. Always returns an [`OnDiskDispatchRecord`]
    /// (errors are recorded in `load_result`, never raised).
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_one_on_disk(
        &self,
        manifest_path: &Path,
        allowed_roots: &[PathBuf],
        trust_unsigned: bool,
        union_keys: &[(VerifyingKey, KeySource)],
        runner: &super::runner::PluginRunner,
        wasm_runner: Option<&wcore_plugin_wasm::WasmPluginRunner>,
        gate: Arc<PluginAccessGate>,
    ) -> OnDiskDispatchRecord {
        let _ = runner; // crash-budget increment happens in the caller.

        let toml_src = match std::fs::read_to_string(manifest_path) {
            Ok(s) => s,
            Err(e) => {
                return OnDiskDispatchRecord {
                    manifest_path: manifest_path.to_path_buf(),
                    plugin_name: "<unparsed>".to_string(),
                    tool_namespace: "<unparsed>".to_string(),
                    dispatch: RuntimeDispatch::Static,
                    load_result: Err(format!("read manifest: {e}")),
                    handle: LoadedRuntimeHandle::None,
                };
            }
        };
        let manifest = match PluginManifest::from_toml_str(&toml_src) {
            Ok(m) => m,
            Err(e) => {
                return OnDiskDispatchRecord {
                    manifest_path: manifest_path.to_path_buf(),
                    plugin_name: "<unparsed>".to_string(),
                    tool_namespace: "<unparsed>".to_string(),
                    dispatch: RuntimeDispatch::Static,
                    load_result: Err(format!("parse manifest: {e}")),
                    handle: LoadedRuntimeHandle::None,
                };
            }
        };
        let plugin_name = manifest.plugin.name.clone();
        let tool_namespace = manifest
            .permissions
            .tool_namespace
            .clone()
            .unwrap_or_else(|| plugin_name.clone());

        // Build PluginIdentity. For wasm, construct directly; for
        // subprocess + mcp-bridge, route through `from_subprocess_path`
        // so the path-prefix check fires.
        let runtime_kind_is_wasm = manifest
            .runtime
            .as_ref()
            .map(|r| r.kind.eq_ignore_ascii_case("wasm"))
            .unwrap_or(false);
        let identity = if runtime_kind_is_wasm {
            // Wasm: enforce allowed_roots manually (no helper exists yet).
            let canonical = match manifest_path.canonicalize() {
                Ok(c) => c,
                Err(e) => {
                    return OnDiskDispatchRecord {
                        manifest_path: manifest_path.to_path_buf(),
                        plugin_name,
                        tool_namespace,
                        dispatch: RuntimeDispatch::Wasm,
                        load_result: Err(format!("canonicalize manifest path: {e}")),
                        handle: LoadedRuntimeHandle::None,
                    };
                }
            };
            let allowed = allowed_roots.iter().any(|r| canonical.starts_with(r));
            if !allowed {
                return OnDiskDispatchRecord {
                    manifest_path: manifest_path.to_path_buf(),
                    plugin_name,
                    tool_namespace,
                    dispatch: RuntimeDispatch::Wasm,
                    load_result: Err(format!(
                        "manifest path {canonical:?} outside allowed roots {allowed_roots:?}"
                    )),
                    handle: LoadedRuntimeHandle::None,
                };
            }
            PluginIdentity::Wasm {
                manifest_path: canonical,
            }
        } else {
            match PluginIdentity::from_subprocess_path(manifest_path, allowed_roots) {
                Ok(id) => id,
                Err(e) => {
                    return OnDiskDispatchRecord {
                        manifest_path: manifest_path.to_path_buf(),
                        plugin_name,
                        tool_namespace,
                        dispatch: RuntimeDispatch::Subprocess,
                        load_result: Err(format!("build identity: {e}")),
                        handle: LoadedRuntimeHandle::None,
                    };
                }
            }
        };

        let dispatch = classify_runtime(&identity, &manifest);

        // Enforce config-level enable flag.
        if !self.config.is_enabled(&plugin_name) {
            return OnDiskDispatchRecord {
                manifest_path: manifest_path.to_path_buf(),
                plugin_name,
                tool_namespace,
                dispatch,
                load_result: Err("disabled by plugins.toml".to_string()),
                handle: LoadedRuntimeHandle::None,
            };
        }

        // Compute the entry-binary/component path for sig verification.
        let plugin_dir = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        // v0.6.5 Task 6B.4 — gather the raw declared relative path so we can
        // run the path-traversal check before resolving to an absolute path
        // and before signature verification.
        let declared_rel: Option<String> = match dispatch {
            RuntimeDispatch::Wasm => Some(
                manifest
                    .runtime
                    .as_ref()
                    .and_then(|r| r.wasm.as_ref())
                    .and_then(|w| w.component_path.clone())
                    .unwrap_or_else(|| "plugin.wasm".to_string()),
            ),
            RuntimeDispatch::Subprocess | RuntimeDispatch::McpBridge => manifest
                .runtime
                .as_ref()
                .and_then(|r| r.subprocess.as_ref())
                .and_then(|s| s.binary_path.clone()),
            // Path B step 1 — declarative plugins have no entry binary, so
            // there is nothing to resolve or signature-verify. The
            // `entry_path` guard below short-circuits on `None`.
            RuntimeDispatch::Static | RuntimeDispatch::Declarative => None,
        };

        let entry_path: Option<PathBuf> = match declared_rel.as_deref() {
            Some(rel) => match resolve_entry_path(&plugin_dir, rel) {
                Ok(p) => Some(p),
                Err(e) => {
                    return OnDiskDispatchRecord {
                        manifest_path: manifest_path.to_path_buf(),
                        plugin_name,
                        tool_namespace,
                        dispatch,
                        handle: LoadedRuntimeHandle::None,
                        load_result: Err(format!("entry path rejected: {e}")),
                    };
                }
            },
            None => None,
        };

        // Reuse Task 1.3's signing flow when verification is on.
        //
        // Aud-14 (verify-vs-execute TOCTOU): the WASM arm performs its OWN
        // byte-based verify-then-execute over a single `std::fs::read`, so the
        // bytes that are signature-checked are byte-identical to the bytes that
        // wasmtime compiles. Skip the path-based `enforce_path_signing` here for
        // WASM to avoid a redundant SECOND read (the old double-read was the
        // TOCTOU gap). Subprocess/McpBridge still verify here and additionally
        // pass the verified `entry_path` down so the runner executes the same
        // resolved path (Aud-18).
        if self.config.plugin_signature_verification
            && !matches!(dispatch, RuntimeDispatch::Wasm)
            && let Some(p) = entry_path.as_deref()
            && let Err(e) = enforce_path_signing(&plugin_name, Some(p), trust_unsigned, union_keys)
        {
            return OnDiskDispatchRecord {
                manifest_path: manifest_path.to_path_buf(),
                plugin_name,
                tool_namespace,
                dispatch,
                load_result: Err(format!("signature verification: {e}")),
                handle: LoadedRuntimeHandle::None,
            };
        }

        // Dispatch to the runtime-specific runner. Wave 6A.1 — capture the
        // loaded handle (when load succeeds) so the bootstrap caller can
        // pass it to the matching `synthesize_initialize_outcome_*` adapter.
        let (load_result, handle): (Result<(), String>, LoadedRuntimeHandle) = match dispatch {
            RuntimeDispatch::Wasm => {
                let Some(wasm) = wasm_runner else {
                    return OnDiskDispatchRecord {
                        manifest_path: manifest_path.to_path_buf(),
                        plugin_name,
                        tool_namespace,
                        dispatch,
                        load_result: Err(
                            "wasm plugin discovered but no WasmPluginRunner provided".to_string()
                        ),
                        handle: LoadedRuntimeHandle::None,
                    };
                };
                let component_path = entry_path
                    .clone()
                    .unwrap_or_else(|| plugin_dir.join("plugin.wasm"));
                // Aud-14: read the component bytes EXACTLY ONCE, verify the
                // signature over those bytes, then compile the SAME buffer via
                // `load_from_bytes`. No second `open()` of `component_path`
                // occurs between verify and execute, so a file swapped after
                // verification can never be the bytes that run.
                let verify_and_load = || -> Result<LoadedWasmPlugin, String> {
                    let bytes = std::fs::read(&component_path).map_err(|e| {
                        format!(
                            "WasmPluginRunner: read component bytes from {}: {e}",
                            component_path.display()
                        )
                    })?;
                    if self.config.plugin_signature_verification && !trust_unsigned {
                        let sig_dir = component_path.parent().unwrap_or(&plugin_dir);
                        crate::plugins::sig_verifier::verify_plugin_signature_bytes(
                            &plugin_name,
                            sig_dir,
                            &bytes,
                            union_keys,
                        )
                        .map_err(|e| format!("signature verification: {e}"))?;
                    }
                    wasm.load_from_bytes(&bytes, &manifest, gate.clone())
                        .map_err(|e| format!("WasmPluginRunner::load: {e}"))
                };
                match verify_and_load() {
                    Ok(loaded) => (Ok(()), LoadedRuntimeHandle::Wasm(Arc::new(loaded))),
                    Err(e) => (Err(e), LoadedRuntimeHandle::None),
                }
            }
            RuntimeDispatch::Subprocess => {
                match wcore_plugin_subprocess::SubprocessPluginRunner::load(
                    manifest_path,
                    &manifest,
                    gate.clone(),
                    entry_path.as_deref(),
                )
                .await
                {
                    Ok(loaded) => (Ok(()), LoadedRuntimeHandle::Subprocess(Arc::new(loaded))),
                    Err(e) => (
                        Err(format!("SubprocessPluginRunner::load: {e}")),
                        LoadedRuntimeHandle::None,
                    ),
                }
            }
            RuntimeDispatch::McpBridge => {
                match wcore_plugin_subprocess::McpBridgePluginRunner::load(
                    manifest_path,
                    &manifest,
                    gate.clone(),
                    entry_path.as_deref(),
                )
                .await
                {
                    Ok(loaded) => (Ok(()), LoadedRuntimeHandle::McpBridge(loaded)),
                    Err(e) => (
                        Err(format!("McpBridgePluginRunner::load: {e}")),
                        LoadedRuntimeHandle::None,
                    ),
                }
            }
            RuntimeDispatch::Static => {
                tracing::warn!(
                    plugin = %plugin_name,
                    manifest = %manifest_path.display(),
                    "on-disk manifest classified as Static — skipping (static plugins live in inventory)"
                );
                (
                    Err("on-disk manifest classified as Static".to_string()),
                    LoadedRuntimeHandle::None,
                )
            }
            RuntimeDispatch::Declarative => {
                // Path B step 1 — no binary to spawn or verify. Translate the
                // manifest's declared `[[hooks]]` into `PluginHook`s (tagged
                // with this plugin's name) and carry the optional `[mcp_server]`
                // spec out on the handle. Path-prefix trust is already enforced
                // (identity was built via `from_subprocess_path` above);
                // binary signature verification does not apply.
                let hooks: Vec<PluginHook> = manifest
                    .hooks
                    .iter()
                    .map(|h| PluginHook {
                        plugin: plugin_name.clone(),
                        phase: h.phase,
                        name: h.tool.clone(),
                    })
                    .collect();
                (
                    Ok(()),
                    LoadedRuntimeHandle::Declarative {
                        hooks,
                        mcp_server: manifest.mcp_server.clone(),
                    },
                )
            }
        };

        OnDiskDispatchRecord {
            manifest_path: manifest_path.to_path_buf(),
            plugin_name,
            tool_namespace,
            dispatch,
            load_result,
            handle,
        }
    }

    /// v0.6.5 Task 2.7b — read-only view of the on-disk dispatch ledger
    /// populated by [`Self::discover_on_disk`]. Tests assert against this.
    pub fn on_disk_dispatches(&self) -> &[OnDiskDispatchRecord] {
        &self.on_disk_dispatches
    }

    /// Wave 6A.1 — take ownership of the on-disk dispatch records so the
    /// bootstrap caller can move each loaded runtime `handle` into the
    /// matching synthesizer. Leaves the loader's internal ledger empty.
    pub fn take_on_disk_dispatches(&mut self) -> Vec<OnDiskDispatchRecord> {
        std::mem::take(&mut self.on_disk_dispatches)
    }

    /// Run cross-plugin validation (namespace ledger). Returns the validated
    /// `Vec<DiscoveredPlugin>` so the runner can take ownership. Validation
    /// failures are returned as `PluginError`; the caller decides whether to
    /// log-and-continue or abort.
    pub fn validate_all(&mut self) -> PluginResult<Vec<DiscoveredPlugin>> {
        let mut ledger = NamespaceLedger::default();
        for d in &self.discovered {
            if d.manifest.permissions.register_tools
                && let Some(ns) = &d.manifest.permissions.tool_namespace
            {
                ledger.claim(ns, &d.name)?;
            }
        }
        let _ = self.config; // suppress unused for now; W4 will read grants here
        Ok(std::mem::take(&mut self.discovered))
    }

    pub fn discovered(&self) -> &[DiscoveredPlugin] {
        &self.discovered
    }
}

#[allow(dead_code)]
fn _hint_error_type() -> Option<PluginError> {
    None
}

/// Build the union of `(VerifyingKey, KeySource)` from the config's
/// `trusted_plugin_keys` (tagged `Config(i)`) and the resolved filesystem
/// trust-anchor directory (tagged `Filesystem(path)`). Order: config first,
/// filesystem second; the verifier accepts on first match, so callers
/// shouldn't depend on a specific iteration order for correctness.
pub(crate) fn build_trusted_key_union(config: &PluginsConfig) -> Vec<(VerifyingKey, KeySource)> {
    let mut union: Vec<(VerifyingKey, KeySource)> = config
        .trusted_plugin_keys
        .iter()
        .enumerate()
        .filter_map(|(i, b64)| {
            let key = parse_verifying_key_b64(b64);
            if key.is_none() {
                tracing::warn!(
                    index = i,
                    key = b64,
                    "plugins.toml: malformed trusted_plugin_key — skipping"
                );
            }
            key.map(|k| (k, KeySource::Config(i)))
        })
        .collect();
    if let Some(dir) = trusted_keys_dir() {
        union.extend(load_filesystem_keys(&dir));
    }
    union
}

/// v0.6.5 Task 1.3 (fixup): signing decision for a single plugin.
///
/// Pure-function wrapper around the unified signing policy applied inside
/// [`PluginLoader::discover`]. Extracted so tests can exercise the
/// static-skips / unsigned-env-allows / valid-sig-accepted / wrong-key-rejected
/// matrix without needing a global `plugin_inventory` factory.
///
/// `path` is the plugin's artifact path (i.e. `PluginFactory::plugin_path()`);
/// `None` means a static plugin and is ALWAYS accepted (engine binary = trust
/// anchor).
pub(crate) fn enforce_path_signing(
    plugin_name: &str,
    path: Option<&std::path::Path>,
    trust_unsigned: bool,
    union_keys: &[(VerifyingKey, KeySource)],
) -> PluginResult<()> {
    let Some(path) = path else {
        // Static plugin: engine binary is the trust anchor.
        return Ok(());
    };
    if trust_unsigned {
        tracing::warn!(
            plugin = plugin_name,
            path = %path.display(),
            "loading path-based plugin WITHOUT signature verification (WAYLAND_PLUGIN_TRUST_UNSIGNED=1)"
        );
        return Ok(());
    }
    verify_path_plugin_signature(plugin_name, path, union_keys)
}

/// v0.6.5 Task 6B.4 — resolve a manifest-declared entry path (a relative
/// `binary_path` / `component_path`) against `plugin_dir` and enforce
/// containment so a malicious manifest cannot point signature verification
/// or loader handoff at an arbitrary file on disk.
///
/// Rejects:
/// - Absolute paths (`"/etc/passwd"`, Windows drive prefixes).
/// - Any path containing a `..` component (traversal).
/// - Symlinks anywhere along the resolved path that escape `plugin_dir`
///   (caught by [`Path::canonicalize`] + `starts_with` once the file
///   exists on disk — pre-existence canonicalization happens when both
///   `plugin_dir` and the joined path resolve).
///
/// When the entry file doesn't exist yet (e.g. test fixtures pointing at
/// a binary that will only ever fail to spawn) we fall back to the
/// lexical absolute/`..` check on the declared relative path — that
/// still defeats the path-traversal class. Symlink escape is only
/// reachable via on-disk files, so the canonicalize path catches it.
pub(crate) fn resolve_entry_path(plugin_dir: &Path, declared_rel: &str) -> Result<PathBuf, String> {
    let rel = Path::new(declared_rel);
    if rel.is_absolute() {
        return Err(format!("absolute path not allowed: {declared_rel}"));
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(format!("parent-dir component not allowed: {declared_rel}"));
    }

    let joined = plugin_dir.join(rel);

    // When the entry exists on disk, canonicalize both sides and enforce
    // prefix. This catches symlink-escape attacks. When it doesn't exist
    // (test fixtures, lazy-built binaries), the lexical check above is
    // the only line of defense — which is fine: the absolute-path /
    // `..` cases are already rejected, and a non-existent file can't
    // be a symlink target.
    if let Ok(canon_entry) = joined.canonicalize() {
        let canon_dir = plugin_dir
            .canonicalize()
            .map_err(|e| format!("canonicalize plugin_dir: {e}"))?;
        if !canon_entry.starts_with(&canon_dir) {
            return Err(format!(
                "entry path {canon_entry:?} escapes plugin_dir {canon_dir:?}"
            ));
        }
        Ok(canon_entry)
    } else {
        Ok(joined)
    }
}

// ---------------------------------------------------------------------------
// v0.6.5 Task 2.7 — Runtime dispatch table.
//
// Routes a `(PluginIdentity, manifest.runtime.kind)` pair to the correct
// runner (static/path/signed via inventory; Wasm via wcore-plugin-wasm;
// Subprocess via wcore-plugin-subprocess; mcp-bridge via the same crate).
//
// The actual host wire-up (loading from disk + InitializeOutcome synthesis
// + apply pipeline integration) happens in the per-variant adapters
// (`wasm_adapter`, `subprocess_adapter`, `mcp_bridge_adapter`). This
// classifier is pure data + zero I/O; the caller owns the I/O.
// ---------------------------------------------------------------------------

/// v0.6.5 Task 2.7 — classification of how a discovered manifest should
/// be loaded. The four variants map 1:1 onto the host's four plugin
/// runtimes. `runtime.kind = "mcp-bridge"` takes precedence over the
/// identity variant — an MCP-bridge plugin can be either Subprocess or
/// PathPrefix-discovered, but in both cases the LOADER hands off to
/// `McpBridgePluginRunner`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeDispatch {
    /// Static / path-prefix / signed — discover via `plugin_inventory`,
    /// hand off to the existing static-plugin path.
    Static,
    /// WASM component — load via `WasmPluginRunner::load(component_path)`.
    Wasm,
    /// Generic subprocess plugin — load via `SubprocessPluginRunner::load`.
    Subprocess,
    /// MCP-server-as-plugin bridge — load via `McpBridgePluginRunner::load`.
    McpBridge,
    /// Path B step 1 — declarative plugin (`runtime.kind = "declarative"`):
    /// no executable. Contributes `[[hooks]]` + an optional `[mcp_server]`
    /// straight into the host's plugin outcome.
    Declarative,
}

/// v0.6.5 Task 2.7 — classify a manifest into its target runtime.
///
/// Precedence:
/// 1. `runtime.kind == "mcp-bridge"` → [`RuntimeDispatch::McpBridge`]
///    regardless of identity (PathPrefix or Subprocess can both carry
///    an MCP bridge payload; the host treats them identically).
/// 2. `PluginIdentity::Wasm { .. }` → [`RuntimeDispatch::Wasm`].
/// 3. `PluginIdentity::Subprocess { .. }` → [`RuntimeDispatch::Subprocess`].
/// 4. Everything else (Static / PathPrefix / Signed) → [`RuntimeDispatch::Static`].
pub fn classify_runtime(
    identity: &wcore_plugin_api::manifest::PluginIdentity,
    manifest: &PluginManifest,
) -> RuntimeDispatch {
    use wcore_plugin_api::manifest::PluginIdentity;

    // Path B step 1 — declarative kind wins over identity: a declarative
    // plugin has no binary, so it never routes to a binary-backed runner.
    if let Some(rt) = manifest.runtime.as_ref()
        && rt.kind.eq_ignore_ascii_case("declarative")
    {
        return RuntimeDispatch::Declarative;
    }
    if let Some(rt) = manifest.runtime.as_ref()
        && rt.kind.eq_ignore_ascii_case("mcp-bridge")
    {
        return RuntimeDispatch::McpBridge;
    }
    match identity {
        PluginIdentity::Wasm { .. } => RuntimeDispatch::Wasm,
        PluginIdentity::Subprocess { .. } => RuntimeDispatch::Subprocess,
        _ => RuntimeDispatch::Static,
    }
}

/// v0.6.5 Task 2.7b — resolve the on-disk plugins root(s).
///
/// Resolution:
/// 1. If `$WAYLAND_PLUGINS_DIR` is set + non-empty → return ONLY that dir.
///    This is an explicit operator override: it REPLACES the default roots
///    (preserving the single-dir test isolation the on-disk discovery tests
///    rely on, and letting an operator pin discovery to one directory).
/// 2. Otherwise → resolve `default_plugin_root()` then `profile_home()/plugins`,
///    de-duplicated. Post-isolation-sweep [`PluginIdentity::default_plugin_root`]
///    is itself `<WAYLAND_HOME or ~/.wayland>/plugins`, so it coincides with
///    `profile_home()/plugins` and the two collapse to a single root (where a
///    declarative plugin installed by IJFW's installer lands).
///
/// Each returned root is its OWN security anchor in [`PluginLoader::discover_on_disk`]:
/// the per-root path-prefix gate + symlink defense apply identically to every
/// root (all are the same user-controlled-home trust class).
///
/// May return an empty `Vec` only in the vanishingly rare case where the env
/// var is unset AND `default_plugin_root()` somehow coincides after dedup with
/// nothing else — covered by the no-root branches in `discover_on_disk`.
pub(crate) fn resolved_plugins_roots() -> Vec<PathBuf> {
    if let Ok(v) = std::env::var(ENV_PLUGINS_DIR)
        && !v.is_empty()
    {
        // Operator override: replace the defaults with exactly this dir.
        return vec![PathBuf::from(v)];
    }

    let mut roots = Vec::with_capacity(2);
    let mut push_unique = |p: PathBuf| {
        if !roots.contains(&p) {
            roots.push(p);
        }
    };
    push_unique(PluginIdentity::default_plugin_root());
    push_unique(wcore_config::config::profile_home().join("plugins"));
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::sig_verifier::PLUGIN_SIG_FILENAME;
    use ed25519_dalek::{SigningKey, ed25519::signature::Signer};
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    fn make_path_plugin(
        sign_with: Option<&SigningKey>,
        fs_trusted: &[&SigningKey],
    ) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let plugin_path = tmp.path().join("plugin.bin");
        let body = b"plugin body";
        std::fs::write(&plugin_path, body).unwrap();
        if let Some(sk) = sign_with {
            let sig = sk.sign(body);
            std::fs::write(tmp.path().join(PLUGIN_SIG_FILENAME), sig.to_bytes()).unwrap();
        }
        let keys = tmp.path().join("keys");
        std::fs::create_dir_all(&keys).unwrap();
        for (i, sk) in fs_trusted.iter().enumerate() {
            std::fs::write(
                keys.join(format!("k-{i}.pub")),
                sk.verifying_key().as_bytes(),
            )
            .unwrap();
        }
        (tmp, plugin_path, keys)
    }

    fn union_from_fs(keys_dir: &std::path::Path) -> Vec<(VerifyingKey, KeySource)> {
        load_filesystem_keys(keys_dir)
    }

    #[test]
    fn static_plugin_skips_signing() {
        let result = enforce_path_signing("static-plugin", None, false, &[]);
        assert!(
            result.is_ok(),
            "static plugins must skip signing even with empty union: {result:?}"
        );
        let result = enforce_path_signing("static-plugin", None, true, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn unsigned_path_plugin_rejected() {
        let key = SigningKey::generate(&mut OsRng);
        let (_tmp, plugin_path, keys_dir) = make_path_plugin(None, &[&key]);
        let union = union_from_fs(&keys_dir);
        let result = enforce_path_signing("p", Some(&plugin_path), false, &union);
        assert!(
            matches!(result, Err(PluginError::SignatureMissing { .. })),
            "expected SignatureMissing, got {result:?}"
        );
    }

    #[test]
    fn unsigned_path_plugin_allowed_with_env() {
        let key = SigningKey::generate(&mut OsRng);
        let (_tmp, plugin_path, keys_dir) = make_path_plugin(None, &[&key]);
        let union = union_from_fs(&keys_dir);
        let result = enforce_path_signing("p", Some(&plugin_path), true, &union);
        assert!(result.is_ok(), "trust-unsigned must allow: {result:?}");
    }

    #[test]
    fn valid_signature_accepted() {
        let key = SigningKey::generate(&mut OsRng);
        let (_tmp, plugin_path, keys_dir) = make_path_plugin(Some(&key), &[&key]);
        let union = union_from_fs(&keys_dir);
        let result = enforce_path_signing("p", Some(&plugin_path), false, &union);
        assert!(result.is_ok(), "valid sig must be accepted: {result:?}");
    }

    #[test]
    fn wrong_key_rejected() {
        let signer = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let (_tmp, plugin_path, keys_dir) = make_path_plugin(Some(&signer), &[&other]);
        let union = union_from_fs(&keys_dir);
        let result = enforce_path_signing("p", Some(&plugin_path), false, &union);
        assert!(
            matches!(result, Err(PluginError::SignatureVerificationFailed { .. })),
            "expected SignatureVerificationFailed, got {result:?}"
        );
    }

    #[test]
    fn empty_union_is_config_error_for_path_plugin() {
        let key = SigningKey::generate(&mut OsRng);
        let (_tmp, plugin_path, _keys) = make_path_plugin(Some(&key), &[]);
        let result = enforce_path_signing("p", Some(&plugin_path), false, &[]);
        assert!(
            matches!(result, Err(PluginError::ConfigError(_))),
            "expected ConfigError, got {result:?}"
        );
    }

    #[test]
    fn try_discover_rejects_verification_enabled_with_no_trusted_keys() {
        // Point filesystem trust dir at an empty temp dir so the union is
        // unambiguously empty regardless of the host's ~/.wayland setup.
        let tmp = TempDir::new().unwrap();
        let empty_dir = tmp.path().join("empty-keys");
        std::fs::create_dir_all(&empty_dir).unwrap();
        // Serialize against other tests that touch the same env vars.
        // Set only inside this scope; clean up at end.
        // SAFETY: tests run single-threaded per process by default for env;
        // wcore-agent uses #[test] which runs concurrent threads, so we
        // explicitly clear the unsigned-trust escape too.
        // SAFETY: env mutation in tests is acceptable here; this test
        // group is the only one in this module that touches these vars.
        unsafe {
            std::env::set_var("WAYLAND_TRUSTED_KEYS_DIR", &empty_dir);
            std::env::remove_var(ENV_TRUST_UNSIGNED);
        }

        let config = PluginsConfig {
            plugin_signature_verification: true,
            trusted_plugin_keys: vec![],
            ..Default::default()
        };
        let result = PluginLoader::try_discover(&config);

        // Always restore env even on assertion failure.
        unsafe {
            std::env::remove_var("WAYLAND_TRUSTED_KEYS_DIR");
        }

        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("must fail when verification on but union is empty"),
        };
        assert!(
            matches!(err, PluginError::ConfigError(_)),
            "expected ConfigError, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("trusted_plugin_keys") || msg.contains("trust-anchor"),
            "error must mention trusted_plugin_keys or trust-anchor; got: {msg}"
        );
    }

    #[test]
    fn try_discover_ok_when_verification_disabled() {
        let config = PluginsConfig {
            plugin_signature_verification: false,
            trusted_plugin_keys: vec![],
            ..Default::default()
        };
        let _ = PluginLoader::try_discover(&config)
            .expect("must succeed when plugin_signature_verification=false");
    }

    // -----------------------------------------------------------------
    // C3 — multi-root plugins discovery resolution.
    //
    // `resolved_plugins_roots()` must:
    // - return ONLY the override dir when `$WAYLAND_PLUGINS_DIR` is set
    //   (preserving single-dir test isolation), and
    // - resolve `default_plugin_root()` + `profile_home()/plugins` when the
    //   override is unset; post-isolation-sweep these coincide and dedup to a
    //   single `<WAYLAND_HOME or ~/.wayland>/plugins` root (so an IJFW-installed
    //   declarative plugin under `~/.wayland/plugins` is reachable).
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::serial]
    fn resolved_plugins_roots_override_is_single_dir() {
        // SAFETY: env mutation is serialized by `#[serial_test::serial]` and
        // restored at the end of this test.
        unsafe {
            std::env::set_var(ENV_PLUGINS_DIR, "/tmp/some-pinned-plugins");
        }
        let roots = resolved_plugins_roots();
        unsafe {
            std::env::remove_var(ENV_PLUGINS_DIR);
        }
        assert_eq!(
            roots,
            vec![PathBuf::from("/tmp/some-pinned-plugins")],
            "override must REPLACE the defaults with exactly one dir"
        );
    }

    #[test]
    #[serial_test::serial]
    fn resolved_plugins_roots_default_is_single_profile_home_root() {
        // After the isolation sweep `default_plugin_root()` == `profile_home()/plugins`,
        // so the two pushes in `resolved_plugins_roots()` collapse via `push_unique`
        // to a SINGLE root rooted under WAYLAND_HOME.
        // SAFETY: env mutation is serialized by `#[serial_test::serial]`; both vars
        // are saved + restored around the body.
        let saved_plugins = std::env::var_os(ENV_PLUGINS_DIR);
        let saved_home = std::env::var_os("WAYLAND_HOME");
        let home = TempDir::new().unwrap();
        unsafe {
            std::env::remove_var(ENV_PLUGINS_DIR);
            // Pin profile_home() to a deterministic location.
            std::env::set_var("WAYLAND_HOME", home.path());
        }

        let roots = resolved_plugins_roots();

        unsafe {
            match saved_plugins {
                Some(v) => std::env::set_var(ENV_PLUGINS_DIR, v),
                None => std::env::remove_var(ENV_PLUGINS_DIR),
            }
            match saved_home {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        // Compute the expectation from the pinned home (NOT from
        // default_plugin_root() after restore — its result now depends on the
        // restored WAYLAND_HOME).
        let expected = home.path().join("plugins");
        assert!(
            roots.contains(&expected),
            "default roots must include the profile-home plugins root {expected:?}; got {roots:?}"
        );
        // default_plugin_root() and profile_home()/plugins now coincide → deduped.
        assert_eq!(
            roots.iter().filter(|r| *r == &expected).count(),
            1,
            "the two former-distinct roots must dedup to ONE entry; got {roots:?}"
        );
    }

    // -----------------------------------------------------------------
    // v0.6.5 Task 2.7 — RuntimeDispatch classifier tests.
    //
    // Builds fixtures that don't touch disk and verifies each of the four
    // dispatch routes is reachable.
    // -----------------------------------------------------------------

    use std::path::PathBuf;
    use wcore_plugin_api::manifest::PluginIdentity;

    fn fixture_manifest(kind: Option<&str>) -> PluginManifest {
        // Build via TOML to keep the path identical to the production
        // parser — same pattern used by `wcore-plugin-wasm::runner::tests`.
        let runtime_block = kind
            .map(|k| format!("[runtime]\nkind = \"{k}\"\n"))
            .unwrap_or_default();
        let toml_str = format!(
            r#"
[plugin]
name = "fixture"
version = "0.0.0"
description = "fixture"
entry = "fixture"
license = "Apache-2.0"

[permissions]

{runtime_block}
"#
        );
        PluginManifest::from_toml_str(&toml_str).expect("fixture toml parses")
    }

    #[test]
    fn classify_runtime_routes_wasm_identity_to_wasm() {
        let id = PluginIdentity::Wasm {
            manifest_path: PathBuf::from("/tmp/x.toml"),
        };
        let m = fixture_manifest(None);
        assert_eq!(classify_runtime(&id, &m), RuntimeDispatch::Wasm);
    }

    #[test]
    fn classify_runtime_routes_subprocess_identity_to_subprocess() {
        let id = PluginIdentity::Subprocess {
            manifest_path: PathBuf::from("/tmp/x.toml"),
        };
        let m = fixture_manifest(None);
        assert_eq!(classify_runtime(&id, &m), RuntimeDispatch::Subprocess);
    }

    #[test]
    fn classify_runtime_routes_mcp_bridge_kind_to_mcp_bridge_regardless_of_identity() {
        // MCP-bridge precedence over identity: a Subprocess identity
        // carrying runtime.kind = "mcp-bridge" routes to McpBridge.
        let id = PluginIdentity::Subprocess {
            manifest_path: PathBuf::from("/tmp/x.toml"),
        };
        let m = fixture_manifest(Some("mcp-bridge"));
        assert_eq!(classify_runtime(&id, &m), RuntimeDispatch::McpBridge);
    }

    #[test]
    fn classify_runtime_routes_static_default() {
        let id = PluginIdentity::from_static("fixture");
        let m = fixture_manifest(None);
        assert_eq!(classify_runtime(&id, &m), RuntimeDispatch::Static);
    }

    // Path B step 1 — T8: a manifest with runtime.kind = "declarative" routes
    // to RuntimeDispatch::Declarative regardless of the (path-prefix) identity.
    #[test]
    fn classify_runtime_routes_declarative_kind_to_declarative() {
        let id = PluginIdentity::from_static("fixture");
        let m = fixture_manifest(Some("declarative"));
        assert_eq!(classify_runtime(&id, &m), RuntimeDispatch::Declarative);
    }
}
