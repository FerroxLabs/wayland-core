//! W4 tools configuration: per-built-in-tool enable flags and the
//! engine-advertised capability surface.
//!
//! Created NEW (no existing `tools` module in `wcore-config/src/lib.rs`).
//! HIGH-3 audit fix. The existing `ToolsConfig` in `config.rs` covers
//! tool *permissions* (skills allow/deny, auto-approve); this new module
//! covers per-tool *registration gates* (Script on/off, RepoMap on/off)
//! and the W0 advertised-capabilities slot.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BuiltinToolsConfig {
    pub script: ScriptToolConfig,
    pub repomap: RepoMapToolConfig,
    pub defer_cold: DeferColdConfig,
}

/// Layer D1 (token-opt): defer cold built-ins to name-only stubs.
///
/// ~30 built-in tool schemas (~7k tokens) are re-serialized on every model
/// round-trip. With deferral on, only the tools on `hot_allowlist` ship
/// their full schema; everything else (cold built-ins + MCP tools) is sent
/// as a name + truncated-description stub that the model hydrates on demand
/// via `ToolSearch`.
///
/// CRITICAL caching constraint: the hot/stub split is a pure function of
/// this static config — never of per-turn state — so the serialized
/// `tools[]` array stays byte-identical across the turns of a conversation
/// (the cached-prefix guard is `tools_array_byte_stable_across_roundtrips`
/// in `wcore-providers`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeferColdConfig {
    /// Default ON. `false` restores full schemas for every tool.
    pub enabled: bool,
    /// Tools that always ship their full schema. `ToolSearch` is never
    /// deferred regardless of this list (it is the hydration path).
    pub hot_allowlist: Vec<String>,
}

impl DeferColdConfig {
    /// The high-frequency core loop tools plus the hydration tool.
    pub fn default_hot_allowlist() -> Vec<String> {
        ["Read", "Edit", "Write", "Bash", "Grep", "Glob", "ToolSearch"]
            .into_iter()
            .map(String::from)
            .collect()
    }
}

impl Default for DeferColdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hot_allowlist: Self::default_hot_allowlist(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptToolConfig {
    /// Default off. When true, `ScriptTool` is registered AND the engine
    /// flips `capabilities.rpc_tool_script` so hosts see it on Ready.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepoMapToolConfig {
    /// Read-only and shape-bounded — default ON. Hosts that don't want
    /// the tool flip this to `false` in `wcore.toml`.
    pub enabled: bool,
}

impl Default for RepoMapToolConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvertisedCapabilitiesConfig {
    /// Mirrored to `Capabilities.rpc_tool_script` (W0 slot at events.rs:139).
    /// The bootstrap path flips this true when `BuiltinToolsConfig.script.enabled`
    /// is on; flipping it in config directly is a no-op (the bootstrap is
    /// authoritative).
    pub rpc_tool_script: bool,

    /// W6 F7 — mirrored to `Capabilities.cost_attribution` (W0 slot).
    /// SINGLE source of truth (audit rev-2 finding 5): the bootstrap path
    /// flips this true when cost rows are present in the active
    /// `ProviderCompat`; `ProtocolSink::emit_session_cost` reads this field
    /// directly to decide whether to emit. There is NO parallel sink-builder
    /// flag.
    pub cost_attribution: bool,

    /// F-092 (W7-N): live-session online evolution capability advertisement.
    /// Mirrored to `Capabilities.online_evolution` on Ready.
    /// Set true when the user passes `--online-evolution` or sets
    /// `[observability] online_evolution = true` in config.
    pub online_evolution: bool,
    // Future W0-reserved flags land here, owned by their wave.
}
