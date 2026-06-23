// M3.1.1 — verify MemoryConfig defaults + threading into ConfigFile.
//
// NOTE on plan deviation (Step 0.5): the plan's second test used
// `Config::default().memory.enabled` as a compile-check, but the
// resolved-runtime `Config` (config.rs:362) does not derive Default
// and has no `impl Default`. The actually-defaulting struct is
// `ConfigFile` (the serde TOML shape, line 98), which is what users
// configure. We compile-check the field on `ConfigFile::default()`
// instead — that exercises the same `#[serde(default)]` plumbing.
//
// F-091 (HIGH, D4 decision): `memory.enabled` default flipped from
// `false` → `true`. Memory is the core value proposition ("memory is
// the whole pitch"). Users who want to opt out
// set `memory.enabled = false` in wcore.toml or pass `--no-memory`.
// Test updated to reflect the new opt-out-by-config contract.

use wcore_config::config::{ConfigFile, MemoryConfig};

#[test]
fn memory_config_default_enabled() {
    let m = MemoryConfig::default();
    assert!(
        m.enabled,
        "F-091: memory must be enabled by default (opt-out via memory.enabled=false)"
    );
    assert_eq!(m.dream_cycle_throttle_secs, 1800, "30 min default throttle");
    assert_eq!(m.decay_interval_secs, 3600, "1 hour default decay interval");
}

#[test]
fn memory_config_threads_into_config_file() {
    // `ConfigFile.memory` is `Option<MemoryConfig>`: `None` means the on-disk
    // `[memory]` table was absent (the merge then inherits the other layer),
    // distinct from an explicit table that happens to match the default. A
    // default `ConfigFile` has no table.
    let cfg = ConfigFile::default();
    assert!(cfg.memory.is_none(), "absent [memory] table parses to None");

    // A present table threads every field through.
    let parsed: ConfigFile = toml::from_str("[memory]\nenabled = true\n").unwrap();
    let mem = parsed
        .memory
        .expect("present [memory] table parses to Some");
    let _ = mem.enabled;
    let _ = mem.dream_cycle_throttle_secs;
    let _ = mem.decay_interval_secs;
}
