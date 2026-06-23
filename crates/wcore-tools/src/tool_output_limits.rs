//! T3-3.3.3: Configurable tool-output truncation limits.
//!
//! Ported from the prior Wayland Python engine (which in turn ports
//! `anomalyco/opencode` PR #23770 — *"feat(truncate): allow configuring tool
//! output truncation limits"*).
//!
//! ## Why a separate helper?
//!
//! `Tool::max_result_size()` in `lib.rs` already exposes a per-tool result
//! byte cap (50_000 default), and `truncate_utf8()` is the char-boundary-safe
//! truncation primitive. Neither of those is **user-configurable** at runtime,
//! and neither covers the *per-line length* cap that file-ops tools need.
//!
//! The predecessor centralises three user-tunable knobs behind a single
//! `tool_output` config section so power users can tune them without patching
//! the source:
//!
//! - `max_bytes` — terminal stdout/stderr cap (default 50_000)
//! - `max_lines` — read_file pagination + truncation cap (default 2000)
//! - `max_line_length` — per-line length cap before `... [truncated]`
//!   (default 2000)
//!
//! The prior Python module reads `tool_output` directly from
//! `wayland_cli.config.load_config()`. To avoid a circular `wcore-tools ↔
//! wcore-config` dependency (and to keep this helper trivially testable),
//! the Rust port accepts a `&serde_json::Value` config section pointer
//! supplied by the caller. Callers in `wcore-agent` / `wcore-cli` extract
//! `config["tool_output"]` from whatever config source they already own
//! and hand the slice to [`ToolOutputLimits::from_section`].
//!
//! ### Defensive fallback
//!
//! Like the Python original, this helper NEVER fails: missing config,
//! wrong types, non-positive values, and overflow all silently fall
//! through to the built-in defaults. Tools must not crash because of
//! malformed user config.
//!
//! ## Example
//!
//! ```rust
//! use wcore_tools::tool_output_limits::{ToolOutputLimits, DEFAULT_MAX_BYTES};
//! use serde_json::json;
//!
//! // Missing / absent config: defaults apply.
//! let limits = ToolOutputLimits::from_section(None);
//! assert_eq!(limits.max_bytes, DEFAULT_MAX_BYTES);
//!
//! // User override via parsed YAML/JSON:
//! let section = json!({"max_bytes": 100_000, "max_lines": 5000});
//! let limits = ToolOutputLimits::from_section(Some(&section));
//! assert_eq!(limits.max_bytes, 100_000);
//! assert_eq!(limits.max_lines, 5000);
//! // Unset key falls through to default:
//! assert_eq!(limits.max_line_length, 2000);
//! ```

use serde_json::Value;

/// Default terminal-tool byte cap (matches the prior engine's `terminal_tool.MAX_OUTPUT_CHARS`).
pub const DEFAULT_MAX_BYTES: usize = 50_000;

/// Default file-ops line cap (matches the prior engine's `file_operations.MAX_LINES`).
pub const DEFAULT_MAX_LINES: usize = 2_000;

/// Default per-line length cap (matches the prior engine's `file_operations.MAX_LINE_LENGTH`).
pub const DEFAULT_MAX_LINE_LENGTH: usize = 2_000;

/// Resolved tool-output truncation limits.
///
/// Construct via [`ToolOutputLimits::from_section`] (defensive parse from a
/// config slice) or [`ToolOutputLimits::default`] (built-in defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolOutputLimits {
    /// Terminal-tool stdout/stderr byte cap.
    pub max_bytes: usize,
    /// File-ops total-line cap.
    pub max_lines: usize,
    /// File-ops per-line length cap.
    pub max_line_length: usize,
}

impl Default for ToolOutputLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_lines: DEFAULT_MAX_LINES,
            max_line_length: DEFAULT_MAX_LINE_LENGTH,
        }
    }
}

impl ToolOutputLimits {
    /// Build limits from an optional `tool_output` config section.
    ///
    /// Mirrors the prior engine's `get_tool_output_limits()`:
    ///
    /// - `None` (no `tool_output` section in config) → all defaults.
    /// - A non-object section (e.g. a string mistakenly assigned by the user)
    ///   → all defaults.
    /// - Each individual key is coerced to a positive `usize`; any failure
    ///   (missing, wrong type, ≤ 0, > `usize::MAX`) falls back to that key's
    ///   default. **This function never panics or returns an error.**
    pub fn from_section(section: Option<&Value>) -> Self {
        let obj = match section {
            Some(Value::Object(o)) => o,
            _ => return Self::default(),
        };
        Self {
            max_bytes: coerce_positive_usize(obj.get("max_bytes"), DEFAULT_MAX_BYTES),
            max_lines: coerce_positive_usize(obj.get("max_lines"), DEFAULT_MAX_LINES),
            max_line_length: coerce_positive_usize(
                obj.get("max_line_length"),
                DEFAULT_MAX_LINE_LENGTH,
            ),
        }
    }
}

/// Coerce a JSON value to a positive `usize`, falling back to `default` on
/// any failure mode (missing, wrong type, ≤ 0, non-integral float, overflow).
///
/// Mirrors the prior engine's `_coerce_positive_int` but tightened for `usize`:
/// a negative JSON integer or a `>usize::MAX` integer both fall through to
/// `default`.
fn coerce_positive_usize(value: Option<&Value>, default: usize) -> usize {
    let v = match value {
        Some(v) => v,
        None => return default,
    };
    let parsed: Option<u64> = match v {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                Some(u)
            } else if let Some(i) = n.as_i64() {
                if i > 0 { Some(i as u64) } else { None }
            } else if let Some(f) = n.as_f64() {
                // Match the prior engine: `int(value)` on a float truncates toward zero;
                // values ≤ 0 fall through. Reject non-finite floats explicitly.
                if f.is_finite() && f >= 1.0 {
                    Some(f as u64)
                } else {
                    None
                }
            } else {
                None
            }
        }
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    };
    match parsed {
        Some(u) if u >= 1 && u as u128 <= usize::MAX as u128 => u as usize,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_when_section_missing() {
        let limits = ToolOutputLimits::from_section(None);
        assert_eq!(limits.max_bytes, DEFAULT_MAX_BYTES);
        assert_eq!(limits.max_lines, DEFAULT_MAX_LINES);
        assert_eq!(limits.max_line_length, DEFAULT_MAX_LINE_LENGTH);
        assert_eq!(limits, ToolOutputLimits::default());
    }

    #[test]
    fn defaults_when_section_is_not_object() {
        // Mirrors the prior engine: non-dict `tool_output` (e.g. a string) is ignored.
        let bad = json!("oops not an object");
        let limits = ToolOutputLimits::from_section(Some(&bad));
        assert_eq!(limits, ToolOutputLimits::default());
    }

    #[test]
    fn user_overrides_applied() {
        let section = json!({
            "max_bytes": 100_000,
            "max_lines": 5_000,
            "max_line_length": 4_096,
        });
        let limits = ToolOutputLimits::from_section(Some(&section));
        assert_eq!(limits.max_bytes, 100_000);
        assert_eq!(limits.max_lines, 5_000);
        assert_eq!(limits.max_line_length, 4_096);
    }

    #[test]
    fn partial_override_falls_through_per_key() {
        // Only max_bytes is overridden; other two keys must use defaults.
        let section = json!({"max_bytes": 12_345});
        let limits = ToolOutputLimits::from_section(Some(&section));
        assert_eq!(limits.max_bytes, 12_345);
        assert_eq!(limits.max_lines, DEFAULT_MAX_LINES);
        assert_eq!(limits.max_line_length, DEFAULT_MAX_LINE_LENGTH);
    }

    #[test]
    fn invalid_values_fall_back_to_defaults() {
        // Negative, zero, wrong-type, and unparseable-string all fall back.
        let section = json!({
            "max_bytes": -5,            // negative → default
            "max_lines": 0,             // non-positive → default
            "max_line_length": "abc",   // unparseable string → default
        });
        let limits = ToolOutputLimits::from_section(Some(&section));
        assert_eq!(limits.max_bytes, DEFAULT_MAX_BYTES);
        assert_eq!(limits.max_lines, DEFAULT_MAX_LINES);
        assert_eq!(limits.max_line_length, DEFAULT_MAX_LINE_LENGTH);
    }

    #[test]
    fn string_integers_are_coerced() {
        // The prior engine uses `int(value)` which accepts numeric strings; mirror that.
        let section = json!({
            "max_bytes": "65536",
            "max_lines": "  10000  ",   // whitespace trimmed
            "max_line_length": 1.0_f64, // float truncates to 1
        });
        let limits = ToolOutputLimits::from_section(Some(&section));
        assert_eq!(limits.max_bytes, 65_536);
        assert_eq!(limits.max_lines, 10_000);
        assert_eq!(limits.max_line_length, 1);
    }

    #[test]
    fn nonfinite_and_subunit_floats_fall_back() {
        // Floats < 1.0 (incl. 0.5, 0.0, -1.0) and NaN/Inf must fall back.
        let section = json!({
            "max_bytes": 0.5_f64,
            "max_lines": -1.5_f64,
        });
        let limits = ToolOutputLimits::from_section(Some(&section));
        assert_eq!(limits.max_bytes, DEFAULT_MAX_BYTES);
        assert_eq!(limits.max_lines, DEFAULT_MAX_LINES);
    }
}
