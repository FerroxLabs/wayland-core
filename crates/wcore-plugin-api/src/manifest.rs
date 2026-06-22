//! `PluginManifest` — the declarative permission / metadata block every plugin
//! ships. Parsed from TOML; validated against schema invariants per design
//! spec §5.17.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{PluginError, PluginResult};
use crate::mcp_server_spec::McpServerSpec;
use crate::registry::hooks::HookPhase;

/// Wave SC SECURITY MAJOR fix — verified identity for a loaded plugin.
///
/// **The threat.** `PluginCapabilitySet::from_loaded(...)` previously
/// flipped engine-level capability flags (`browser_suite`,
/// `computer_use`) based on string matching against
/// `PluginManifest.plugin.name`. A malicious crate could set
/// `name = "wayland-browser"` in its manifest and impersonate the
/// real browser plugin, gaining the host's UI capability badge
/// without owning the underlying surface.
///
/// **The fix (v0.2.1 path-prefix / static-link).** Identity is now
/// either:
///
/// - [`PluginIdentity::Static`]: the plugin was discovered via
///   `inventory::submit!` at compile time. The symbol name is
///   immutable — the plugin's identity is anchored to the binary's
///   own link table, so impersonation is impossible without
///   recompiling the engine.
///
/// - [`PluginIdentity::PathPrefix`]: the plugin was loaded from disk
///   under `<data_dir>/wayland/plugins/`. The host validates the path
///   prefix at load time; a manifest claiming
///   `name = "wayland-browser"` from anywhere else is refused.
///
/// **The future (v0.3.0 path).** [`PluginIdentity::Signed`] is
/// reserved for ed25519-signed manifests with a hardcoded
/// public-key allowlist in `wcore-cli`. Out of scope for this
/// wave — Static + PathPrefix close the audit finding.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PluginIdentity {
    /// Compile-time static identity — the plugin was discovered via
    /// `inventory::submit!`. The `symbol` field is the
    /// `static MANIFEST: PluginManifest` symbol name as exposed to the
    /// `inventory` registry; immutable at link time.
    Static { symbol: String },
    /// Path-prefix-verified identity — the plugin was loaded from
    /// disk under the host-controlled plugins directory. The
    /// `manifest_path` is the on-disk path; the host validates that
    /// it starts with `<data_dir>/wayland/plugins/` at load time.
    PathPrefix { manifest_path: PathBuf },
    /// Reserved for v0.3.0 — ed25519-signed manifest with a host
    /// allowlist. Holding the door open in the type system so the
    /// v0.3.0 transition doesn't need a breaking enum change.
    Signed {
        public_key: String,
        signature: String,
    },
    /// v0.6.5 Task 3.2 — subprocess plugin identity. The plugin is a native
    /// binary that the engine spawns and talks to over JSON-Lines on stdio.
    /// `manifest_path` points at the on-disk manifest (`<plugin_dir>/plugin.toml`);
    /// the binary path comes from the manifest's `[runtime.subprocess]` block.
    /// Path-prefix validation (`from_path_prefix`-style) is applied at load
    /// time by `wcore-agent`'s plugin loader; signature verification of the
    /// binary is delegated to `loader::enforce_path_signing` from Task 1.3.
    ///
    /// Subprocess plugins inherit the engine's process privileges by default
    /// (Q5/A7 from the cross-audit) — they are NOT a sandbox boundary.
    Subprocess { manifest_path: PathBuf },
    /// v0.6.5 Task 2.6 — WASM-component plugin identity. The plugin is a
    /// `.wasm` Component-Model module that the engine instantiates fresh
    /// per tool call (Ironclaw pattern) inside a wasmtime sandbox.
    /// `manifest_path` points at the on-disk manifest (`<plugin_dir>/plugin.toml`);
    /// the component path comes from the manifest's `[runtime.wasm]` block
    /// (or `<plugin_dir>/plugin.wasm` by convention).
    ///
    /// Unlike `Subprocess`, WASM plugins ARE a sandbox boundary — host
    /// capabilities are opt-in via [`PluginPermissions::allow_network`] /
    /// `allow_workspace_*` / `allow_tool_invoke` / `permitted_secrets`.
    /// Composition is fail-closed: omitted permissions yield `Deny*`
    /// adapters at link time (see `wcore_plugin_wasm::runner`).
    Wasm { manifest_path: PathBuf },
}

impl PluginIdentity {
    /// Construct the static-link identity for a plugin discovered via
    /// `inventory::submit!`. The `symbol` MUST come from the engine's
    /// inventory enumeration — never from a manifest field.
    pub fn from_static(symbol: impl Into<String>) -> Self {
        Self::Static {
            symbol: symbol.into(),
        }
    }

    /// Validate that `manifest_path` lives under one of the
    /// host-controlled plugin roots, then return the verified
    /// [`PluginIdentity::PathPrefix`] identity. Returns
    /// `PluginError::ManifestSchema` when the prefix doesn't match —
    /// a manifest from `~/Downloads/evil-browser/manifest.toml`
    /// CANNOT claim to be the real `wayland-browser`.
    ///
    /// `allowed_roots` is the set of host-trusted prefixes; in
    /// production this is `[dirs::data_dir()/wayland/plugins/]` plus
    /// any test override.
    pub fn from_path_prefix(
        manifest_path: impl AsRef<Path>,
        allowed_roots: &[PathBuf],
    ) -> PluginResult<Self> {
        let p = manifest_path.as_ref();
        let canonical = match p.canonicalize() {
            Ok(c) => c,
            Err(e) => {
                return Err(PluginError::ManifestSchema {
                    reason: format!("plugin manifest path {p:?} could not be canonicalized: {e}"),
                });
            }
        };
        let allowed = allowed_roots.iter().any(|root| {
            let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            canonical.starts_with(&canon_root)
        });
        if !allowed {
            return Err(PluginError::ManifestSchema {
                reason: format!(
                    "plugin manifest path {canonical:?} is outside the host's allowed plugin roots {allowed_roots:?}"
                ),
            });
        }
        Ok(Self::PathPrefix {
            manifest_path: canonical,
        })
    }

    /// Default trusted root for dynamic plugins, rooted under the profile
    /// home so `WAYLAND_HOME` sandboxes it: `<profile_home>/plugins/`.
    ///
    /// SECURITY: this is a trust anchor — it becomes an `allowed_roots`
    /// entry for path-prefix manifest validation
    /// ([`PluginIdentity::from_path_prefix`]). `wcore-plugin-api` is the
    /// plugin-isolation boundary and MUST NOT depend on `wcore-config`
    /// (enforced by `build.rs` FORBIDDEN_CORE_IMPORTS), so the resolution
    /// is replicated inline in [`profile_home_local`]. It MUST stay
    /// byte-for-byte equivalent to `wcore_config::config::profile_home()`;
    /// the cross-crate test `default_plugin_root_matches_canonical_resolver`
    /// in `wcore-agent` pins that equality.
    pub fn default_plugin_root() -> PathBuf {
        profile_home_local().join("plugins")
    }

    /// True when this identity is anchored to the engine binary
    /// itself (impersonation-impossible class).
    pub fn is_static(&self) -> bool {
        matches!(self, Self::Static { .. })
    }

    /// v0.6.5 Task 3.2 — construct a subprocess-plugin identity from an
    /// on-disk manifest path. Like [`Self::from_path_prefix`] this enforces
    /// that `manifest_path` lives under one of the host's allowed plugin
    /// roots; unlike `from_path_prefix` the resulting identity records
    /// that the plugin's runtime is the subprocess host.
    ///
    /// The actual subprocess spawn lives in `wcore-plugin-subprocess`; this
    /// constructor is the canonical entry point for the loader to mint a
    /// verified identity before handing off to the runner.
    pub fn from_subprocess_path(
        manifest_path: impl AsRef<Path>,
        allowed_roots: &[PathBuf],
    ) -> PluginResult<Self> {
        let path_id = Self::from_path_prefix(manifest_path, allowed_roots)?;
        match path_id {
            Self::PathPrefix { manifest_path } => Ok(Self::Subprocess { manifest_path }),
            // from_path_prefix only returns PathPrefix on success; other
            // arms are unreachable but listed for the #[non_exhaustive] guard.
            other => Err(PluginError::ManifestSchema {
                reason: format!(
                    "internal: from_path_prefix returned non-PathPrefix variant {other:?}"
                ),
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub plugin: PluginInfo,
    #[serde(default)]
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub capabilities: PluginCapabilities,
    /// v0.6.5 Task 1.1 — declared plugin-API semver. Compared against
    /// [`crate::PLUGIN_API_VERSION`] by [`PluginManifest::require_api_version`].
    /// Optional for backward compatibility with v0.6.4 manifests.
    #[serde(default)]
    pub plugin_api_version: Option<String>,
    /// v0.6.5 Task 1.1 — runtime-kind selector for the plugin SDK. Default
    /// `static` (compile-time-linked Rust plugins; the only kind supported
    /// before v0.6.5). `wasm` / `subprocess` / `mcp-bridge` / `declarative`.
    #[serde(default)]
    pub runtime: Option<PluginRuntime>,
    /// Path B step 1 — declarative lifecycle hooks. Each entry binds a
    /// `HookPhase` to a tool NAME that the C1 hook→context dispatcher matches
    /// against an MCP server's advertised tool. Empty for non-declarative
    /// plugins. Requires `permissions.register_hooks` when non-empty.
    #[serde(default)]
    pub hooks: Vec<ManifestHook>,
    /// Path B step 1 — a declarative plugin's contributed MCP server spec.
    /// Carries the same operator-trust as a config `[mcp.servers.*]` entry.
    /// Requires `permissions.register_mcp_server` when present.
    #[serde(default)]
    pub mcp_server: Option<McpServerSpec>,
}

/// Path B step 1 — one declarative `[[hooks]]` entry. Binds a lifecycle
/// `phase` to a `tool` NAME. The tool name is matched against an MCP server's
/// advertised tool list by the C1 dispatcher (`resolve_server_for_plugin`).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManifestHook {
    /// Lifecycle phase. `HookPhase` is `serde(rename_all = "snake_case")`,
    /// so `phase = "session_start"` parses to `HookPhase::SessionStart`.
    pub phase: HookPhase,
    /// Tool name to dispatch in this phase.
    pub tool: String,
}

/// v0.6.5 Task 1.1 — `[runtime]` manifest block. All fields optional;
/// missing block ≡ `kind = "static"` with no per-kind config.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginRuntime {
    /// Runtime kind. Default `"static"`. Validated against the closed
    /// set {`static`, `wasm`, `subprocess`, `mcp-bridge`}; unknown
    /// kinds raise [`PluginError::UnknownRuntimeKind`].
    #[serde(default = "default_runtime_kind")]
    pub kind: String,
    /// Reserved for Wave 2 — wasm-specific config block.
    #[serde(default)]
    pub wasm: Option<PluginRuntimeWasm>,
    /// Reserved for Wave 3 — subprocess-specific config block.
    #[serde(default)]
    pub subprocess: Option<PluginRuntimeSubprocess>,
    /// Reserved for Wave 3 — mcp-bridge-specific config block.
    #[serde(default)]
    pub mcp_bridge: Option<PluginRuntimeMcpBridge>,
}

fn default_runtime_kind() -> String {
    "static".to_string()
}

/// Reserved for Wave 2. Optional fields only — schema stays open to
/// additive change without breaking existing manifests.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginRuntimeWasm {
    pub component_path: Option<String>,
    pub fuel_per_call: Option<u64>,
}

/// Reserved for Wave 3. Optional fields only.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginRuntimeSubprocess {
    pub binary_path: Option<String>,
    pub args: Vec<String>,
}

/// Reserved for Wave 3. Optional fields only.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginRuntimeMcpBridge {
    pub server_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Entry symbol / artifact reference. Optional: declarative on-disk
    /// plugins have no executable, so they omit it. Compiled-in plugins
    /// (and existing on-disk subprocess/wasm manifests) still set it.
    #[serde(default)]
    pub entry: Option<String>,
    #[serde(default)]
    pub authors: Vec<String>,
    pub license: String,
    /// When true, the plugin's `initialize()` is deferred until first use.
    #[serde(default)]
    pub deferred: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginPermissions {
    pub register_tools: bool,
    pub register_hooks: bool,
    pub register_providers: bool,
    pub register_agents: bool,
    pub register_skills: bool,
    pub register_rules: bool,
    pub register_mcp_server: bool,
    /// v0.6.4 Task 2.1 — gate for `ScopedUserModelRegistry`. Plugins that
    /// supply a user-model backend (e.g. `wayland-honcho`) must set this.
    pub register_user_models: bool,
    pub tool_namespace: Option<String>,
    pub memory_partitions_writable: Vec<String>,
    pub memory_partitions_readable: Vec<String>,
    /// `"self_only"` | `"all"` — default `"self_only"`.
    pub mcp_servers_visible: Option<String>,

    // ---- v0.6.5 Task 2.6 (WASM host capabilities — Path X) ----
    //
    // These flags drive the composition root in `wcore-plugin-wasm`: when
    // a plugin's [runtime.kind] is `"wasm"`, the runner reads these to pick
    // `Gated*` vs `Deny*` host adapters at component-link time. Composition
    // is fail-closed: every field defaults to `false` / empty, so existing
    // manifests stay byte-compatible AND opt into nothing by accident.
    //
    // Static/subprocess/mcp-bridge plugins ignore these fields — host
    // adapter gating for those runtimes is enforced differently (engine
    // privilege inheritance for subprocess per A7, no host-callbacks for
    // static).
    /// Permits the WASM host's `http-request` import. Default: deny.
    pub allow_network: bool,
    /// Allow-list of host glob patterns (matched against request URL host)
    /// when `allow_network = true`. Empty = no hosts reachable; every
    /// outbound request is permission-denied. Patterns use `glob` syntax
    /// (`api.example.com`, `*.example.com`). Default: empty.
    pub http_allowlist: Vec<String>,
    /// Permits the WASM host's `workspace-read` import. Default: deny.
    pub allow_workspace_read: bool,
    /// Permits the WASM host's `workspace-write` import. Default: deny.
    pub allow_workspace_write: bool,
    /// Permits the WASM host's `tool-invoke` import. Default: deny.
    pub allow_tool_invoke: bool,
    /// Allow-list of secret names visible to the plugin's `secret-exists`
    /// import. Existence-only — values are NEVER exposed to WASM (see
    /// `host_adapters::secrets` for the structural contract). Default: empty.
    pub permitted_secrets: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PluginCapabilities {
    pub required: Vec<String>,
    pub optional: Vec<String>,
}

impl PluginManifest {
    pub fn from_toml_str(s: &str) -> PluginResult<Self> {
        let manifest: Self = toml::from_str(s)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> PluginResult<()> {
        // register_tools requires tool_namespace.
        if self.permissions.register_tools && self.permissions.tool_namespace.is_none() {
            return Err(PluginError::ManifestSchema {
                reason: format!(
                    "{}: register_tools=true requires tool_namespace",
                    self.plugin.name
                ),
            });
        }

        // Defense-in-depth (security audit plugins-11): a wildcard http_allowlist
        // makes `allow_network` an open egress grant, which the host's runtime
        // SSRF guard then has to carry alone. Require explicit hosts so the
        // allowlist is a real control, not a rubber stamp. ("*" — or a bare
        // empty-host entry — is rejected; concrete hostnames/domains are fine.)
        if self.permissions.allow_network {
            for host in &self.permissions.http_allowlist {
                let h = host.trim();
                if h == "*" || h.is_empty() {
                    return Err(PluginError::ManifestSchema {
                        reason: format!(
                            "{}: http_allowlist may not contain a wildcard \"*\" (or empty) host \
                             when allow_network=true; list explicit hostnames/domains instead",
                            self.plugin.name
                        ),
                    });
                }
            }
        }

        // Every partition in writable/readable must be P1..=P5; P5 is never writable.
        for p in &self.permissions.memory_partitions_writable {
            validate_partition(p, "writable", &self.plugin.name)?;
            if p == "P5" {
                return Err(PluginError::ManifestSchema {
                    reason: format!(
                        "{}: P5 (user model) is system-write-only; plugins cannot list it as writable",
                        self.plugin.name
                    ),
                });
            }
        }
        for p in &self.permissions.memory_partitions_readable {
            validate_partition(p, "readable", &self.plugin.name)?;
        }

        // mcp_servers_visible must be self_only or all.
        if let Some(v) = &self.permissions.mcp_servers_visible
            && !matches!(v.as_str(), "self_only" | "all")
        {
            return Err(PluginError::ManifestSchema {
                reason: format!(
                    "{}: mcp_servers_visible must be \"self_only\" or \"all\" (got {v:?})",
                    self.plugin.name
                ),
            });
        }

        // v0.6.5 Task 1.1 — validate [runtime] kind if the block was
        // declared. Missing block ≡ "static" and is always accepted.
        if let Some(rt) = &self.runtime
            && !matches!(
                rt.kind.as_str(),
                "static" | "wasm" | "subprocess" | "mcp-bridge" | "declarative"
            )
        {
            return Err(PluginError::UnknownRuntimeKind {
                plugin: self.plugin.name.clone(),
                kind: rt.kind.clone(),
            });
        }

        // Path B step 1 — declarative-plugin permission gates. Declared hooks
        // require `register_hooks`; a declared MCP server requires
        // `register_mcp_server`. Hooks-without-mcp_server is allowed.
        if !self.hooks.is_empty() && !self.permissions.register_hooks {
            return Err(PluginError::ManifestSchema {
                reason: format!(
                    "{}: [[hooks]] declared but permissions.register_hooks is not granted",
                    self.plugin.name
                ),
            });
        }
        if self.mcp_server.is_some() && !self.permissions.register_mcp_server {
            return Err(PluginError::ManifestSchema {
                reason: format!(
                    "{}: [mcp_server] declared but permissions.register_mcp_server is not granted",
                    self.plugin.name
                ),
            });
        }

        Ok(())
    }

    /// v0.6.5 Task 1.1 — assert the manifest's declared `plugin_api_version`
    /// matches `expected` (normally [`crate::PLUGIN_API_VERSION`]).
    ///
    /// Returns `Ok(())` when:
    /// - the manifest omits the field (treated as compatible with the
    ///   current engine — older v0.6.4 manifests stay loadable), OR
    /// - the declared version equals `expected`.
    ///
    /// Returns [`PluginError::VersionMismatch`] when the declared version
    /// is present and does not equal `expected`.
    pub fn require_api_version(&self, expected: &str) -> PluginResult<()> {
        match &self.plugin_api_version {
            None => Ok(()),
            Some(v) if v == expected => Ok(()),
            Some(v) => Err(PluginError::VersionMismatch {
                plugin: self.plugin.name.clone(),
                expected: expected.to_string(),
                found: v.clone(),
            }),
        }
    }
}

fn validate_partition(p: &str, side: &str, plugin: &str) -> PluginResult<()> {
    match p {
        "P1" | "P2" | "P3" | "P4" | "P5" => Ok(()),
        _ => Err(PluginError::ManifestSchema {
            reason: format!(
                "{plugin}: memory_partitions_{side} contains invalid partition `{p}` (valid: P1..=P5)"
            ),
        }),
    }
}

/// Inline mirror of `wcore_config::config::profile_home()`. Kept in sync by
/// hand because the plugin-api isolation boundary forbids a `wcore-config`
/// dep (`build.rs` FORBIDDEN_CORE_IMPORTS). MUST stay byte-for-byte equal:
/// same env var, same control-char filter, same `~/.wayland` default, same
/// CWD-relative last-resort fallback. Pinned by the cross-crate equality test
/// `default_plugin_root_matches_canonical_resolver` in `wcore-agent`.
fn profile_home_local() -> PathBuf {
    // Reject an override carrying an ASCII control char (NUL/newline) — it
    // can't be passed safely to a child env. Fall through to the default.
    if let Ok(wh) = std::env::var("WAYLAND_HOME")
        && !wh.chars().any(|c| c.is_control())
    {
        return PathBuf::from(wh);
    }
    dirs::home_dir()
        .map(|h| h.join(".wayland"))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|d| d.join(".wayland"))
                .unwrap_or_else(|_| PathBuf::from(".wayland"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn default_plugin_root_honours_wayland_home() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        let root = PluginIdentity::default_plugin_root();
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert_eq!(root, tmp.path().join("plugins"));
    }

    #[test]
    #[serial_test::serial]
    fn default_plugin_root_rejects_control_char_override() {
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", "bad\nvalue") };
        let root = PluginIdentity::default_plugin_root();
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        // Control-char override rejected → falls back to ~/.wayland/plugins,
        // never the literal "bad\nvalue".
        assert!(root.ends_with("plugins"));
        assert!(!root.to_string_lossy().contains('\n'));
    }
}
