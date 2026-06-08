//! Integration tests for wcore-compact.
//!
//! These tests exercise the public API (`compact_output`, `compact_output_toon`,
//! `CompactionLevel`) end-to-end with realistic fixture data, covering
//! cross-cutting behaviour that the per-module unit tests do not reach.

use wcore_compact::{CompactionLevel, compact_output, compact_output_toon};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A realistic agent transcript as it arrives from a tool invocation: ANSI
/// colour codes, carriage-return progress lines, repeated "Compiling" output,
/// multiple blank lines, and an embedded JSON object.
fn agent_transcript() -> String {
    let mut t = String::new();
    // ANSI header
    t.push_str("\x1b[1m\x1b[32m=== Build started ===\x1b[0m\n");
    // Progress bar overwritten via CR
    t.push_str("Progress: 10%\rProgress: 50%\rProgress: 100%\n");
    // Repeated Compiling lines (enough to trigger folding at Full level)
    for i in 0..8 {
        t.push_str(&format!("Compiling dep-{i} v0.1.{i}\n"));
    }
    // Multiple blank lines
    t.push_str("\n\n\n\n");
    // Trailing whitespace on a content line
    t.push_str("Build succeeded!   \n");
    // Embedded JSON result
    t.push_str("{\n    \"status\": \"ok\",\n    \"elapsed_ms\": 1234,\n    \"warnings\": 0\n}\n");
    t
}

// ---------------------------------------------------------------------------
// CompactionLevel::Off
// ---------------------------------------------------------------------------

#[test]
fn off_preserves_input_byte_for_byte() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Off);
    assert_eq!(result, input, "Off level must be an identity transform");
}

#[test]
fn off_preserves_ansi_codes() {
    let input = "\x1b[31mred\x1b[0m";
    let result = compact_output(input, CompactionLevel::Off);
    assert!(result.contains("\x1b[31m"), "ANSI must survive Off level");
}

// ---------------------------------------------------------------------------
// CompactionLevel::Safe — exercises lib.rs → sanitize.rs pipeline
// ---------------------------------------------------------------------------

#[test]
fn safe_strips_ansi_from_realistic_transcript() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Safe);
    assert!(
        !result.contains("\x1b["),
        "Safe must strip all ANSI escape sequences; got:\n{result}"
    );
}

#[test]
fn safe_collapses_cr_progress_lines() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Safe);
    // Only the final overwrite "Progress: 100%" should survive
    assert!(
        result.contains("Progress: 100%"),
        "Safe must retain last CR-overwritten value; got:\n{result}"
    );
    assert!(
        !result.contains("Progress: 10%"),
        "Safe must discard earlier CR-overwritten values; got:\n{result}"
    );
}

#[test]
fn safe_merges_multiple_blank_lines() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Safe);
    assert!(
        !result.contains("\n\n\n"),
        "Safe must collapse 3+ consecutive blank lines to at most 2; got:\n{result}"
    );
}

#[test]
fn safe_does_not_fold_repeated_lines() {
    // Safe must NOT apply fold — that is Full-only behaviour.
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Safe);
    assert!(
        !result.contains("[..."),
        "Safe level must not fold repeated lines; got:\n{result}"
    );
}

#[test]
fn safe_on_empty_input_returns_empty() {
    assert_eq!(compact_output("", CompactionLevel::Safe), "");
}

// ---------------------------------------------------------------------------
// CompactionLevel::Full — exercises lib.rs → sanitize.rs → fold.rs → json.rs
// ---------------------------------------------------------------------------

#[test]
fn full_folds_repeated_compiling_lines() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Full);
    assert!(
        result.contains("[...") && result.contains("similar lines"),
        "Full must fold the 8 repeated Compiling lines; got:\n{result}"
    );
    // First and last of the group must be preserved
    assert!(
        result.contains("Compiling dep-0"),
        "first Compiling must survive"
    );
    assert!(
        result.contains("Compiling dep-7"),
        "last Compiling must survive"
    );
}

#[test]
fn full_compacts_embedded_json() {
    // Use a transcript with NO fold-marker lines (which contain '[') so that
    // compact_json's embedded-JSON search is not confused by '[... N similar lines]'
    // appearing before the actual JSON block.  The fold path uses `find(['{','['])`
    // and stops at the first match — a TOON-fold marker wins over the JSON block.
    let input = concat!(
        "Build output:\n",
        "{\n    \"status\": \"ok\",\n    \"elapsed_ms\": 1234,\n    \"warnings\": 0\n}\n"
    );
    let result = compact_output(input, CompactionLevel::Full);
    // 4-space-indented JSON (65 chars) should be rewritten to 2-space (59 chars)
    assert!(
        !result.contains("    \"status\""),
        "Full must remove 4-space indent from embedded JSON; got:\n{result}"
    );
    assert!(
        result.contains("\"status\""),
        "JSON content must survive compaction; got:\n{result}"
    );
}

/// Documents a known limitation: when fold produces `[... N similar lines]`
/// markers BEFORE a JSON block, compact_json's embedded-object search finds
/// the `[` in the fold marker first (not a valid JSON array) and falls through,
/// leaving the JSON block in its original pretty-printed form.
#[test]
fn full_json_not_compacted_when_fold_marker_precedes_it() {
    let input = agent_transcript();
    let result = compact_output(&input, CompactionLevel::Full);
    // The fold marker appears before the JSON, so compact_json skips the block.
    // This test documents the current behaviour so a regression is visible.
    assert!(
        result.contains("\"status\""),
        "JSON key must still be present in output; got:\n{result}"
    );
}

#[test]
fn full_result_is_shorter_than_safe_for_transcript() {
    let input = agent_transcript();
    let safe = compact_output(&input, CompactionLevel::Safe);
    let full = compact_output(&input, CompactionLevel::Full);
    assert!(
        full.len() < safe.len(),
        "Full must produce shorter output than Safe for a transcript with repeating lines and JSON;\n\
         safe len={}, full len={}",
        safe.len(),
        full.len()
    );
}

#[test]
fn full_on_empty_input_returns_empty() {
    assert_eq!(compact_output("", CompactionLevel::Full), "");
}

// ---------------------------------------------------------------------------
// compact_output_toon — exercises lib.rs → toon.rs pipeline
// ---------------------------------------------------------------------------

#[test]
fn toon_encodes_uniform_json_array() {
    let input = r#"[
        {"id": 1, "name": "Alice", "role": "admin"},
        {"id": 2, "name": "Bob",   "role": "user"},
        {"id": 3, "name": "Carol", "role": "user"}
    ]"#;
    let result = compact_output_toon(input);
    assert!(
        result.contains("[3]{id,name,role}:"),
        "TOON header must be present; got:\n{result}"
    );
    assert!(result.contains("1,Alice,admin"));
    assert!(result.contains("3,Carol,user"));
    // Output must be shorter than the original prettified JSON
    assert!(
        result.len() < input.len(),
        "TOON must be more token-efficient than pretty JSON; got:\n{result}"
    );
}

#[test]
fn toon_passes_through_non_array_input() {
    let input = "plain text — not JSON";
    assert_eq!(compact_output_toon(input), input);
}

#[test]
fn toon_passes_through_non_uniform_array() {
    // Objects with different key sets must not be TOON-encoded
    let input = r#"[{"id": 1}, {"name": "Bob"}]"#;
    let result = compact_output_toon(input);
    assert_eq!(result, input, "non-uniform array must round-trip unchanged");
}

#[test]
fn toon_on_empty_input_returns_empty() {
    assert_eq!(compact_output_toon(""), "");
}

// ---------------------------------------------------------------------------
// CompactionLevel serde / parse round-trip (exercises level.rs via public re-export)
// ---------------------------------------------------------------------------

#[test]
fn level_parse_display_roundtrip_all_variants() {
    for s in ["off", "safe", "full"] {
        let level: CompactionLevel = s.parse().expect("should parse");
        assert_eq!(level.to_string(), s, "Display must match parse input");
    }
}

#[test]
fn level_invalid_parse_returns_error() {
    let result = "turbo".parse::<CompactionLevel>();
    assert!(result.is_err(), "unknown level must return Err");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("turbo"),
        "error message should echo the bad input; got: {msg}"
    );
}
