//! Plugin API error type. Public, structured, `thiserror`-derived.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin manifest parse error: {0}")]
    ManifestParse(#[from] toml::de::Error),

    #[error("plugin manifest schema validation failed: {reason}")]
    ManifestSchema { reason: String },

    #[error("plugin {plugin} permission denied: {operation}")]
    PermissionDenied { plugin: String, operation: String },

    #[error(
        "plugin {plugin} attempted to register {name} outside its tool_namespace `{namespace}`"
    )]
    ToolNameOutsideNamespace {
        plugin: String,
        namespace: String,
        name: String,
    },

    #[error(
        "plugin {plugin}: tool namespace `{namespace}` is missing (register_tools=true requires it)"
    )]
    NamespaceMissing { plugin: String, namespace: String },

    #[error("plugin namespace collision: `{namespace}` claimed by both {first} and {second}")]
    NamespaceCollision {
        namespace: String,
        first: String,
        second: String,
    },

    #[error("plugin {plugin}: duplicate registration of {kind} `{name}`")]
    DuplicateRegistration {
        plugin: String,
        kind: &'static str,
        name: String,
    },

    #[error("plugin {plugin} initialize failed: {source}")]
    InitializeFailed {
        plugin: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("plugin {plugin} shutdown failed: {source}")]
    ShutdownFailed {
        plugin: String,
        #[source]
        source: anyhow::Error,
    },

    /// Wave RB STABILITY MINOR #13: host failed to populate a
    /// `PluginContext` field that the manifest declares as required
    /// (e.g. `register_tools = true` but `ctx.tools` is `None`). The
    /// host implementation is broken, not the plugin. This variant
    /// converts what used to be a panic into a structured error so
    /// plugin loading reports the misconfiguration through the
    /// normal `InitializeOutcome.errors` channel.
    #[error(
        "plugin {plugin} loaded into a misconfigured host: ctx.{surface} is None but manifest declares register_{surface}=true"
    )]
    HostMisconfiguration { plugin: String, surface: String },

    /// Sec6: signature verification is enabled but the plugin has no artifact
    /// path (compiled-in) or the .sig sidecar file is absent.
    #[error(
        "plugin {plugin}: signature file not found at `{sig_path}` (plugin_signature_verification=true)"
    )]
    SignatureMissing { plugin: String, sig_path: String },

    /// Sec6: the .sig sidecar was present but did not verify against any trusted key.
    #[error("plugin {plugin}: signature verification failed — binary may be tampered")]
    SignatureVerificationFailed { plugin: String },

    /// Bad host configuration caught before any plugin is loaded.
    #[error("plugin loader configuration error: {0}")]
    ConfigError(String),

    /// v0.6.5 Task 1.1 — manifest declares a `plugin_api_version` that
    /// does not match the engine's [`crate::PLUGIN_API_VERSION`].
    #[error("plugin {plugin}: plugin_api_version `{found}` does not match engine `{expected}`")]
    VersionMismatch {
        plugin: String,
        expected: String,
        found: String,
    },

    /// v0.6.5 Task 1.1 — `[runtime] kind = "..."` is not one of the
    /// recognised runtime kinds (`static`, `wasm`, `subprocess`, `mcp-bridge`).
    #[error(
        "plugin {plugin}: unknown runtime kind `{kind}` (valid: static, wasm, subprocess, mcp-bridge)"
    )]
    UnknownRuntimeKind { plugin: String, kind: String },

    /// v0.6.5 Task 1.5 — a plugin's `UserModelSpec.backend` tag did not
    /// match any known backend at reification time. Today only `"honcho"`
    /// reifies; unknown tags surface as a typed error rather than a panic
    /// or silent skip.
    #[error("plugin {plugin}: unknown user-model backend `{backend}` (known: honcho)")]
    UnknownUserModelBackend { plugin: String, backend: String },
}

pub type PluginResult<T> = Result<T, PluginError>;
