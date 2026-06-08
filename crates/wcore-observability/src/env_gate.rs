//! D.2 (v0.6.3) — canonical disable-vocabulary for `WAYLAND_*` env gates.
//!
//! Round 2 audit found the "mirror" family of opt-out env gates accepted
//! inconsistent disable values: `WAYLAND_TRACE_RESULT_SNIPPETS` honoured
//! `off`/`0`/`false`, while `WAYLAND_KG` and `WAYLAND_MEMORY_STALENESS`
//! honoured only the literal `off`. An operator who learns one vocabulary
//! reasonably expects it to work for the rest.
//!
//! This module is the single source of truth for the disable vocabulary so
//! every `WAYLAND_*` opt-out gate accepts the same set. `wcore-observability`
//! is depended on by every gate-owning crate (`wcore-memory`,
//! `wcore-agent`), so the helper is reachable everywhere a gate lives.

/// Returns `true` if `value` is a recognized "disable" token: any of
/// `off`, `0`, `false`, `no` (case-insensitive, surrounding whitespace
/// trimmed). This is the canonical disable vocabulary for `WAYLAND_*`
/// opt-out env gates — every gate should route through this so the vocab
/// is uniform.
pub fn is_disable_value(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "off" | "0" | "false" | "no"
    )
}

/// Reads the env var `name` and returns `true` unless it is set to a
/// recognized disable token (see [`is_disable_value`]). Unset or any
/// non-disable value → `true` (gate enabled). A fresh `std::env::var`
/// read each call so tests can flip the var at runtime.
///
/// This is the canonical implementation for the "ON by default, opt out
/// via WAYLAND_*" gate pattern.
pub fn enabled_unless_disabled(name: &str) -> bool {
    std::env::var(name)
        .map(|v| !is_disable_value(&v))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disable_vocabulary_accepts_all_canonical_tokens() {
        for tok in ["off", "0", "false", "no"] {
            assert!(is_disable_value(tok), "{tok} must disable");
            assert!(
                is_disable_value(&tok.to_uppercase()),
                "{tok} must disable case-insensitively"
            );
        }
    }

    #[test]
    fn disable_vocabulary_trims_whitespace() {
        assert!(is_disable_value("  off  "));
        assert!(is_disable_value("\t0\n"));
    }

    #[test]
    fn non_disable_values_keep_gate_enabled() {
        for tok in ["on", "1", "true", "yes", "", "garbage"] {
            assert!(!is_disable_value(tok), "{tok} must NOT disable");
        }
    }

    #[test]
    fn enabled_unless_disabled_defaults_on_when_unset() {
        // SAFETY: single-threaded test, var removed before read.
        unsafe {
            std::env::remove_var("WAYLAND_TEST_GATE_UNSET");
        }
        assert!(enabled_unless_disabled("WAYLAND_TEST_GATE_UNSET"));
    }

    #[test]
    fn enabled_unless_disabled_honors_every_disable_token() {
        for tok in ["off", "0", "false", "no", "OFF", "False"] {
            // SAFETY: single-threaded test, var set then read then removed.
            unsafe {
                std::env::set_var("WAYLAND_TEST_GATE_OFF", tok);
            }
            assert!(
                !enabled_unless_disabled("WAYLAND_TEST_GATE_OFF"),
                "{tok} must disable the gate"
            );
        }
        unsafe {
            std::env::remove_var("WAYLAND_TEST_GATE_OFF");
        }
    }
}
