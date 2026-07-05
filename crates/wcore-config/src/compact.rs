use serde::{Deserialize, Serialize};

/// Configuration for the multi-level context compaction system.
///
/// All token-related fields are in tokens (not bytes or characters).
/// The defaults are tuned for Claude models with a 200k context window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    /// Context window size in tokens (e.g. 200_000 for Claude).
    #[serde(default = "default_context_window")]
    pub context_window: usize,

    /// Tokens reserved for output generation.
    /// Subtracted from `context_window` to get the effective input budget.
    #[serde(default = "default_output_reserve")]
    pub output_reserve: usize,

    /// Buffer below the effective window that triggers autocompact.
    /// `threshold = context_window - output_reserve - autocompact_buffer`
    #[serde(default = "default_autocompact_buffer")]
    pub autocompact_buffer: usize,

    /// Tokens from context_window limit to trigger emergency block.
    /// `emergency_limit = context_window - emergency_buffer`
    #[serde(default = "default_emergency_buffer")]
    pub emergency_buffer: usize,

    /// Max consecutive autocompact failures before the circuit breaker trips.
    #[serde(default = "default_max_failures")]
    pub max_failures: u32,

    /// Microcompact: keep the N most recent compactable tool results.
    #[serde(default = "default_micro_keep_recent")]
    pub micro_keep_recent: usize,

    /// Microcompact: gap threshold in seconds for time-based trigger.
    /// When the last assistant message is older than this, microcompact fires.
    #[serde(default = "default_micro_gap_seconds")]
    pub micro_gap_seconds: u64,

    /// Tool names whose results are eligible for microcompact content clearing.
    #[serde(default = "default_compactable_tools")]
    pub compactable_tools: Vec<String>,

    /// Whether the compaction system is enabled.
    /// When false, microcompact and autocompact are skipped
    /// (emergency truncation still applies).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Enable prompt cache diagnostics output to user.
    /// When true, cache hit/miss info is shown via OutputSink.
    /// Default: false.
    #[serde(default)]
    pub cache_diagnostics: bool,

    #[serde(default)]
    pub compaction: wcore_compact::CompactionLevel,

    #[serde(default)]
    pub toon: bool,

    /// Model id used for autocompact summarization.
    ///
    /// Summarization is a cheap-model task; running it on the live premium
    /// model costs ~15-20x more than necessary. When set, the autocompact
    /// LLM request targets this model instead of the live conversation model.
    /// The id is a plain provider-served model string (no provider assumed).
    ///
    /// Default: `None` — use the live model, preserving prior behavior.
    #[serde(default)]
    pub compaction_model: Option<String>,

    // --- #280 smart auto-compaction (default-OFF; soak before enabling) ---
    /// MASTER GATE for #280 smart auto-compaction. When false (the default),
    /// NOTHING in the smart path runs: the proactive pre-gate early-returns and
    /// `run_compaction` behaves byte-for-byte as before this feature landed.
    /// This is the default-OFF guarantee — flip to true only after a soak.
    #[serde(default)]
    pub smart_enabled: bool,

    /// High-water active-window share that ARMS a proactive compact (#280).
    /// Spec band 0.60–0.70; clamped to that band at the use site so an
    /// out-of-band TOML value is corrected rather than firing at 1% or never.
    #[serde(default = "default_smart_trigger_fraction")]
    pub smart_trigger_fraction: f64,

    /// Hysteresis low-water (#280). After a smart fire the trigger DISARMS and
    /// re-arms only once a later turn's fraction drops below this. Forced to
    /// `min(trigger - 0.05)` at the use site so it can never collapse hysteresis.
    #[serde(default = "default_smart_release_fraction")]
    pub smart_release_fraction: f64,

    /// Minimum completed turns between two smart fires (#280). Belt-and-
    /// suspenders for the post-stream watermark refresh lag.
    #[serde(default = "default_smart_cooldown_turns")]
    pub smart_cooldown_turns: u32,

    /// Cannot-shrink terminal latch (#280): if a smart-triggered compact frees
    /// fewer than this many tokens, smart compaction latches OFF for the rest
    /// of the session (guards against "frees ~nothing, fire forever").
    #[serde(default = "default_smart_min_shrink_tokens")]
    pub smart_min_shrink_tokens: u64,

    /// Write the non-destructive handoff Episode to long-term memory on a smart
    /// fire (#280). Default true, but only reachable when `smart_enabled`. Lets
    /// the memory write be soaked/disabled independently for NullMemory hosts.
    #[serde(default = "default_true")]
    pub smart_handoff_to_memory: bool,

    /// Continuous compaction of HISTORICAL assistant tool-call arguments
    /// (parity gap 2): large `tool_calls[].function.arguments` payloads (e.g.
    /// Write file bodies) older than the last N assistant turns are replaced
    /// with a compact stub. TOML table: `[compact.tool_call_args]`.
    #[serde(default)]
    pub tool_call_args: ToolCallArgsConfig,
}

/// Config for continuous tool-call-argument compaction (parity gap 2).
///
/// Unlike the tool-RESULT micro-compaction above (trigger-gated), this pass
/// runs on every compaction pipeline pass: an old Write body stops riding in
/// resent history at the first epoch tick after it leaves the protected tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallArgsConfig {
    /// Master gate for the pass. Default ON.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Assistant turns whose tool-call arguments stay verbatim, counted from
    /// the end of history — the model may still reference recent args.
    /// Floored to 1 at the use site.
    #[serde(default = "default_tca_keep_recent_turns")]
    pub keep_recent_turns: usize,

    /// Minimum serialized size (bytes) of an argument object before it is
    /// stubbed. Tiny args (Read paths, short Bash commands) are never touched.
    #[serde(default = "default_tca_min_args_bytes")]
    pub min_args_bytes: usize,

    /// Epoch quantization of the stub boundary (cache economics): the
    /// boundary advances only every `epoch_turns` assistant turns, stubbing a
    /// batch at once, instead of flipping one message per turn inside the
    /// provider's cached prefix (which would re-bill the byte-identical
    /// protected tail at full price every turn). Between ticks the boundary
    /// is frozen and the whole prefix stays cache-hittable. `1` = advance
    /// every turn (no quantization). Floored to 1 at the use site.
    #[serde(default = "default_tca_epoch_turns")]
    pub epoch_turns: usize,
}

impl Default for ToolCallArgsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            keep_recent_turns: default_tca_keep_recent_turns(),
            min_args_bytes: default_tca_min_args_bytes(),
            epoch_turns: default_tca_epoch_turns(),
        }
    }
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            context_window: default_context_window(),
            output_reserve: default_output_reserve(),
            autocompact_buffer: default_autocompact_buffer(),
            emergency_buffer: default_emergency_buffer(),
            max_failures: default_max_failures(),
            micro_keep_recent: default_micro_keep_recent(),
            micro_gap_seconds: default_micro_gap_seconds(),
            compactable_tools: default_compactable_tools(),
            enabled: default_true(),
            cache_diagnostics: false,
            compaction: wcore_compact::CompactionLevel::default(),
            toon: false,
            compaction_model: None,
            smart_enabled: false,
            smart_trigger_fraction: default_smart_trigger_fraction(),
            smart_release_fraction: default_smart_release_fraction(),
            smart_cooldown_turns: default_smart_cooldown_turns(),
            smart_min_shrink_tokens: default_smart_min_shrink_tokens(),
            smart_handoff_to_memory: true,
            tool_call_args: ToolCallArgsConfig::default(),
        }
    }
}

// --- Default value functions ---

fn default_context_window() -> usize {
    200_000
}
fn default_output_reserve() -> usize {
    20_000
}
fn default_autocompact_buffer() -> usize {
    13_000
}
fn default_emergency_buffer() -> usize {
    3_000
}
fn default_max_failures() -> u32 {
    3
}
fn default_micro_keep_recent() -> usize {
    5
}
fn default_micro_gap_seconds() -> u64 {
    3600
}
fn default_compactable_tools() -> Vec<String> {
    vec![
        "Read".into(),
        "Bash".into(),
        "Grep".into(),
        "Glob".into(),
        "Write".into(),
        "Edit".into(),
    ]
}
fn default_true() -> bool {
    true
}
fn default_smart_trigger_fraction() -> f64 {
    0.65
}
fn default_smart_release_fraction() -> f64 {
    0.50
}
fn default_smart_cooldown_turns() -> u32 {
    2
}
fn default_smart_min_shrink_tokens() -> u64 {
    2_000
}
fn default_tca_keep_recent_turns() -> usize {
    2
}
fn default_tca_min_args_bytes() -> usize {
    768
}
fn default_tca_epoch_turns() -> usize {
    4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_spec() {
        let cfg = CompactConfig::default();
        assert_eq!(cfg.context_window, 200_000);
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
        assert_eq!(
            cfg.compactable_tools,
            vec!["Read", "Bash", "Grep", "Glob", "Write", "Edit"]
        );
    }

    #[test]
    fn toml_full_override() {
        let toml_str = r#"
context_window = 128000
output_reserve = 10000
autocompact_buffer = 8000
emergency_buffer = 2000
max_failures = 5
micro_keep_recent = 3
micro_gap_seconds = 1800
compactable_tools = ["Read", "Bash"]
enabled = false
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, 128_000);
        assert_eq!(cfg.output_reserve, 10_000);
        assert_eq!(cfg.autocompact_buffer, 8_000);
        assert_eq!(cfg.emergency_buffer, 2_000);
        assert_eq!(cfg.max_failures, 5);
        assert_eq!(cfg.micro_keep_recent, 3);
        assert_eq!(cfg.micro_gap_seconds, 1800);
        assert_eq!(cfg.compactable_tools, vec!["Read", "Bash"]);
        assert!(!cfg.enabled);
    }

    #[test]
    fn toml_partial_override_uses_defaults() {
        let toml_str = r#"
context_window = 128000
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.context_window, 128_000);
        // Everything else should be default
        assert_eq!(cfg.output_reserve, 20_000);
        assert_eq!(cfg.autocompact_buffer, 13_000);
        assert_eq!(cfg.emergency_buffer, 3_000);
        assert_eq!(cfg.max_failures, 3);
        assert_eq!(cfg.micro_keep_recent, 5);
        assert_eq!(cfg.micro_gap_seconds, 3600);
        assert!(cfg.enabled);
    }

    #[test]
    fn toml_empty_uses_all_defaults() {
        let cfg: CompactConfig = toml::from_str("").unwrap();
        let default = CompactConfig::default();
        assert_eq!(cfg.context_window, default.context_window);
        assert_eq!(cfg.output_reserve, default.output_reserve);
        assert_eq!(cfg.autocompact_buffer, default.autocompact_buffer);
        assert_eq!(cfg.emergency_buffer, default.emergency_buffer);
        assert_eq!(cfg.max_failures, default.max_failures);
        assert_eq!(cfg.micro_keep_recent, default.micro_keep_recent);
        assert_eq!(cfg.micro_gap_seconds, default.micro_gap_seconds);
        assert_eq!(cfg.enabled, default.enabled);
    }

    #[test]
    fn cache_diagnostics_defaults_to_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.cache_diagnostics);
    }

    #[test]
    fn toml_cache_diagnostics_override() {
        let toml_str = r#"
cache_diagnostics = true
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.cache_diagnostics);
    }

    #[test]
    fn default_compaction_is_safe() {
        let cfg = CompactConfig::default();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Safe);
    }

    #[test]
    fn default_toon_is_false() {
        let cfg = CompactConfig::default();
        assert!(!cfg.toon);
    }

    #[test]
    fn toml_compaction_level_override() {
        let toml_str = r#"compaction = "full""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Full);
    }

    #[test]
    fn toml_compaction_off() {
        let toml_str = r#"compaction = "off""#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.compaction, wcore_compact::CompactionLevel::Off);
    }

    #[test]
    fn toml_toon_enabled() {
        let toml_str = r#"toon = true"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.toon);
    }

    #[test]
    fn smart_compaction_defaults_off() {
        // #280: the master gate is OFF by default and the band defaults sit in
        // the spec band so the use-site clamp is a no-op for the defaults.
        let cfg = CompactConfig::default();
        assert!(!cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.65);
        assert_eq!(cfg.smart_release_fraction, 0.50);
        assert_eq!(cfg.smart_cooldown_turns, 2);
        assert_eq!(cfg.smart_min_shrink_tokens, 2_000);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_empty_keeps_smart_off() {
        // An empty [compact] block must leave smart compaction default-OFF so
        // existing configs are byte-for-byte unaffected.
        let cfg: CompactConfig = toml::from_str("").unwrap();
        assert!(!cfg.smart_enabled);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_smart_partial_override_uses_defaults() {
        // Only the master gate set; every other smart field keeps its default.
        let cfg: CompactConfig = toml::from_str("smart_enabled = true").unwrap();
        assert!(cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.65);
        assert_eq!(cfg.smart_cooldown_turns, 2);
        assert_eq!(cfg.smart_min_shrink_tokens, 2_000);
        assert!(cfg.smart_handoff_to_memory);
    }

    #[test]
    fn toml_smart_full_override() {
        let toml_str = r#"
smart_enabled = true
smart_trigger_fraction = 0.68
smart_release_fraction = 0.45
smart_cooldown_turns = 4
smart_min_shrink_tokens = 5000
smart_handoff_to_memory = false
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.smart_enabled);
        assert_eq!(cfg.smart_trigger_fraction, 0.68);
        assert_eq!(cfg.smart_release_fraction, 0.45);
        assert_eq!(cfg.smart_cooldown_turns, 4);
        assert_eq!(cfg.smart_min_shrink_tokens, 5_000);
        assert!(!cfg.smart_handoff_to_memory);
    }

    #[test]
    fn tool_call_args_defaults() {
        // Parity gap 2: default ON, protect the last 2 assistant turns,
        // never stub argument payloads under 768 serialized bytes.
        let cfg = CompactConfig::default();
        assert!(cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 2);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 768);
        assert_eq!(cfg.tool_call_args.epoch_turns, 4);
    }

    #[test]
    fn toml_empty_keeps_tool_call_args_defaults() {
        let cfg: CompactConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.tool_call_args, ToolCallArgsConfig::default());
    }

    #[test]
    fn toml_tool_call_args_override() {
        let toml_str = r#"
[tool_call_args]
enabled = false
keep_recent_turns = 4
min_args_bytes = 2048
epoch_turns = 6
"#;
        let cfg: CompactConfig = toml::from_str(toml_str).unwrap();
        assert!(!cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 4);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 2048);
        assert_eq!(cfg.tool_call_args.epoch_turns, 6);
    }

    #[test]
    fn toml_tool_call_args_partial_override() {
        let cfg: CompactConfig =
            toml::from_str("[tool_call_args]\nkeep_recent_turns = 3\n").unwrap();
        assert!(cfg.tool_call_args.enabled);
        assert_eq!(cfg.tool_call_args.keep_recent_turns, 3);
        assert_eq!(cfg.tool_call_args.min_args_bytes, 768);
        assert_eq!(cfg.tool_call_args.epoch_turns, 4);
    }

    #[test]
    fn json_serialization_roundtrip() {
        let cfg = CompactConfig {
            context_window: 100_000,
            output_reserve: 15_000,
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CompactConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.context_window, 100_000);
        assert_eq!(back.output_reserve, 15_000);
        assert_eq!(back.autocompact_buffer, cfg.autocompact_buffer);
    }
}
