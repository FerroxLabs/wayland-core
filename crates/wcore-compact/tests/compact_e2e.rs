//! E5 scenario 3 — compact_output end-to-end.
//!
//! Simulates a long multi-turn transcript (tool calls + assistant turns +
//! tool results). Passes it through `compact_output(Full)`. Asserts:
//!   - Output is strictly smaller than input.
//!   - The most recent assistant turn text is preserved.
//!   - Key decision text embedded in the transcript survives.
//!   - ANSI escapes are stripped.
//!   - Repeated "Compiling ..." lines are folded.

use wcore_compact::{CompactionLevel, compact_output};

fn build_transcript() -> String {
    let mut parts: Vec<String> = Vec::new();

    // System-level header (should survive — it's short and unique).
    parts.push("DECISION: use async Rust for all I/O".to_string());

    // Simulate 20 "Compiling" lines — compact Full should fold these.
    // Lines must share >50% prefix with lines[0] to satisfy fold algorithm.
    for i in 0..20 {
        parts.push(format!("Compiling crate-{i} v0.1.0"));
    }

    // ANSI-decorated status output.
    parts.push("\x1b[32mBuild OK\x1b[0m".to_string());

    // A tool result block (large JSON — Full will compact it).
    parts.push(
        "{\n    \"id\": 1,\n    \"name\": \"Alice Wonderland\",\n    \"email\": \"alice@example.com\",\n    \"age\": 30,\n    \"address\": \"123 Main Street, Anytown, USA 12345\"\n}"
            .to_string(),
    );

    // Blank-line spam.
    for _ in 0..10 {
        parts.push(String::new());
    }

    // Most recent assistant turn — must survive compaction.
    parts.push("FINAL_ANSWER: the implementation is complete".to_string());

    parts.join("\n")
}

#[test]
fn compact_full_reduces_size_and_preserves_key_content() {
    let transcript = build_transcript();
    let compacted = compact_output(&transcript, CompactionLevel::Full);

    // Output must be strictly smaller.
    assert!(
        compacted.len() < transcript.len(),
        "compact Full must reduce size: {} → {}",
        transcript.len(),
        compacted.len(),
    );

    // Most recent turn preserved.
    assert!(
        compacted.contains("FINAL_ANSWER: the implementation is complete"),
        "most recent assistant turn must survive compaction"
    );

    // Key decision preserved (short unique text is not folded).
    assert!(
        compacted.contains("DECISION: use async Rust"),
        "key decision text must survive compaction"
    );

    // ANSI escapes stripped.
    assert!(
        !compacted.contains("\x1b"),
        "ANSI escapes must be stripped by Full compaction"
    );

    // Repeated Compiling lines folded.
    assert!(
        compacted.contains("[..."),
        "repeated Compiling lines must be folded"
    );
}

#[test]
fn compact_safe_strips_ansi_but_keeps_all_lines() {
    let transcript = build_transcript();
    let compacted = compact_output(&transcript, CompactionLevel::Safe);

    assert!(!compacted.contains("\x1b"), "Safe must strip ANSI escapes");
    // Safe does not fold repeated lines — all Compiling lines still present.
    assert!(
        compacted.contains("Compiling crate-19"),
        "Safe must not fold repeated Compiling lines"
    );
    assert!(
        compacted.contains("FINAL_ANSWER"),
        "Safe must preserve all content"
    );
}

#[test]
fn compact_off_preserves_input_exactly() {
    let transcript = build_transcript();
    let compacted = compact_output(&transcript, CompactionLevel::Off);
    assert_eq!(compacted, transcript, "Off must return input unchanged");
}
