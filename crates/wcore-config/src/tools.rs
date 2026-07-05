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

/// Layer D1 (token-opt): defer cold built-ins out of the tools[] array.
///
/// ~30 built-in tool schemas (~7k tokens) are re-serialized on every model
/// round-trip. With deferral on, only the tools on `hot_allowlist` ship
/// their full schema. Everything else (cold built-ins + MCP tools) is —
/// with `catalog` on (default) — folded into a single compact,
/// name-only inventory line inside ToolSearch's own description (the
/// openclaw pattern: no per-tool stub entries at all). With `catalog`
/// off, cold tools fall back to individual name + truncated-description
/// stub entries. Either way the model hydrates on demand via `ToolSearch`.
///
/// CRITICAL caching constraint: the hot/deferred split is a pure function
/// of this static config — never of per-turn state — so the serialized
/// `tools[]` array (including the catalog line) stays byte-identical across
/// the turns of a conversation (the cached-prefix guard is
/// `tools_array_byte_stable_across_roundtrips` in `wcore-providers`); a
/// ToolSearch hydration changes it once.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeferColdConfig {
    /// Default ON. `false` restores full schemas for every tool.
    pub enabled: bool,
    /// Tools that always ship their full schema. `ToolSearch` is never
    /// deferred regardless of this list (it is the hydration path).
    pub hot_allowlist: Vec<String>,
    /// Default ON: deferred tools ship as ONE sorted name-only catalog line
    /// in ToolSearch's description instead of per-tool stub entries
    /// (measured: 43 stubs cost ~2.5k tokens/request — more than the hot
    /// schemas). `false` restores per-tool stub entries.
    pub catalog: bool,
    /// HARD cap on the catalog line's name-list length in chars — even a
    /// single name is dropped when it alone exceeds the budget (the suffix
    /// "+N more — search to discover" replaces the overflow, so an MCP
    /// swarm or a pathological name cannot balloon the prompt). Applies
    /// strictly to the names portion; the fixed prefix and the
    /// constant-size suffix sit outside it.
    pub catalog_max_chars: usize,
}

impl DeferColdConfig {
    /// The high-frequency core loop tools plus the hydration tool.
    pub fn default_hot_allowlist() -> Vec<String> {
        [
            "Read",
            "Edit",
            "Write",
            "Bash",
            "Grep",
            "Glob",
            "ToolSearch",
        ]
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
            catalog: true,
            catalog_max_chars: 4096,
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
