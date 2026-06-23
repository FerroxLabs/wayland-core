//! Black-box integration tests for CompactConfig (TC-2.2-01 through TC-2.2-03, TC-2.2-07).
//!
//! These test the public API of CompactConfig from a config-file consumer's
//! perspective: default values, full TOML override, partial override, and
//! Config-level integration.

use wcore_config::compact::CompactConfig;
use wcore_config::config::ConfigFile;

/// TC-2.2-01: CompactConfig default values match spec.
#[test]
fn tc_2_2_01_compact_config_defaults() {
    let cfg = CompactConfig::default();
    assert_eq!(cfg.context_window, 200_000);
    assert_eq!(cfg.output_reserve, 20_000);
    assert_eq!(cfg.autocompact_buffer, 13_000);
    assert_eq!(cfg.emergency_buffer, 3_000);
    assert_eq!(cfg.max_failures, 3);
    assert_eq!(cfg.micro_keep_recent, 5);
    assert_eq!(cfg.micro_gap_seconds, 3600);
    assert!(cfg.enabled);
}

/// TC-2.2-02: CompactConfig full TOML parsing.
#[test]
fn tc_2_2_02_compact_config_toml_full() {
    let toml_str = r#"
[compact]
context_window = 128000
output_reserve = 15000
autocompact_buffer = 10000
emergency_buffer = 2000
max_failures = 5
micro_keep_recent = 3
micro_gap_seconds = 1800
compactable_tools = ["Read", "Bash"]
enabled = false
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    assert_eq!(config.compact.context_window, 128_000);
    assert_eq!(config.compact.output_reserve, 15_000);
    assert_eq!(config.compact.autocompact_buffer, 10_000);
    assert_eq!(config.compact.emergency_buffer, 2_000);
    assert_eq!(config.compact.max_failures, 5);
    assert_eq!(config.compact.micro_keep_recent, 3);
    assert_eq!(config.compact.micro_gap_seconds, 1800);
    assert_eq!(config.compact.compactable_tools, vec!["Read", "Bash"]);
    assert!(!config.compact.enabled);
}

/// TC-2.2-03: partial override — only context_window set, rest are defaults.
#[test]
fn tc_2_2_03_compact_config_partial_override() {
    let toml_str = r#"
[compact]
context_window = 128000
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    assert_eq!(config.compact.context_window, 128_000);
    // All other fields should be defaults
    assert_eq!(config.compact.output_reserve, 20_000);
    assert_eq!(config.compact.autocompact_buffer, 13_000);
    assert_eq!(config.compact.emergency_buffer, 3_000);
    assert_eq!(config.compact.max_failures, 3);
    assert_eq!(config.compact.micro_keep_recent, 5);
    assert_eq!(config.compact.micro_gap_seconds, 3600);
    assert!(config.compact.enabled);
}

/// #280: smart auto-compaction is default-OFF and back-compatible. A config
/// with no smart_* keys must leave the master gate false and the handoff on.
#[test]
fn smart_compaction_default_off_in_config_file() {
    let toml_str = r#"
[compact]
context_window = 100000
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    assert!(!config.compact.smart_enabled);
    assert_eq!(config.compact.smart_trigger_fraction, 0.65);
    assert!(config.compact.smart_handoff_to_memory);
}

/// #280: an out-of-band smart_trigger_fraction parses verbatim (the spec band
/// clamp is applied at the engine use site, not at deserialization, to keep
/// config back-compat — see advanced.md). This documents the raw-parse contract.
#[test]
fn smart_trigger_fraction_out_of_band_parses_verbatim() {
    let toml_str = r#"
[compact]
smart_enabled = true
smart_trigger_fraction = 0.95
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    // Raw value is preserved; the engine clamps it into 0.60..=0.70 when it
    // evaluates the trigger.
    assert_eq!(config.compact.smart_trigger_fraction, 0.95);
}

/// TC-2.2-07: Config TOML with [compact] section parses completely.
#[test]
fn tc_2_2_07_config_with_compact_section() {
    let toml_str = r#"
[default]
provider = "anthropic"

[compact]
context_window = 100000
enabled = true
"#;
    let config: ConfigFile = toml::from_str(toml_str).unwrap();
    assert_eq!(config.compact.context_window, 100_000);
    assert!(config.compact.enabled);
    // Other config sections should still parse
    assert_eq!(config.default.provider, "anthropic");
}
