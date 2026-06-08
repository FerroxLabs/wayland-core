//! v0.6.1 hardening (E1) — first integration tests for wcore-compact.
//!
//! Audit finding: wcore-compact had only inline `#[test]` units; no
//! `tests/` directory. Inline tests cover each module in isolation but
//! the end-to-end pipeline composition (`compact_output` →
//! sanitize → fold → json) had no integration coverage. A regression
//! at a module boundary could pass every unit test and still break the
//! public API contract.
//!
//! These tests exercise `compact_output()` at each `CompactionLevel`
//! with realistic adversarial inputs that touch every stage in the
//! Full pipeline.

use wcore_compact::{CompactionLevel, compact_output, compact_output_toon};

/// Realistic streamed tool output: terminal progress bars (ANSI +
/// carriage returns), repeated download lines, and a JSON tail. All
/// three pipeline stages should engage at CompactionLevel::Full.
const REALISTIC_TOOL_STREAM: &str = "\u{1b}[32m+\u{1b}[0m Installing dependencies\n\
    \u{1b}[36mloading...\u{1b}[0m\r\u{1b}[36mloading.\u{1b}[0m\r\u{1b}[36mloading..\u{1b}[0m\r\u{1b}[32mdone\u{1b}[0m\n\
    Downloading foo-1.0.0...\n\
    Downloading foo-1.0.0...\n\
    Downloading foo-1.0.0...\n\
    Downloading foo-1.0.0...\n\
    Downloading foo-1.0.0...\n\
    {\"name\":\"foo\",\"version\":\"1.0.0\",\"deps\":[\"a\",\"b\",\"c\"]}\n";

#[test]
fn level_off_returns_input_unchanged() {
    let input = REALISTIC_TOOL_STREAM;
    assert_eq!(compact_output(input, CompactionLevel::Off), input);
}

#[test]
fn level_safe_strips_ansi_but_preserves_lines() {
    let out = compact_output(REALISTIC_TOOL_STREAM, CompactionLevel::Safe);
    assert!(
        !out.contains('\u{1b}'),
        "Safe level must strip ESC characters"
    );
    // The Downloading line repeats 5x — Safe keeps them all (only
    // sanitization runs).
    assert_eq!(
        out.matches("Downloading foo-1.0.0").count(),
        5,
        "Safe level must NOT fold; got: {out}"
    );
}

#[test]
fn level_full_strips_ansi_and_folds_repeats() {
    let out = compact_output(REALISTIC_TOOL_STREAM, CompactionLevel::Full);

    // Stage 1: sanitize — no ANSI.
    assert!(
        !out.contains('\u{1b}'),
        "Full pipeline must strip ESC characters; got: {out}"
    );

    // Stage 2: fold — 5x identical lines must collapse.
    assert!(
        out.matches("Downloading foo-1.0.0").count() < 5,
        "Full level must fold repeated lines; got 5+ copies in: {out}"
    );

    // The JSON line should still be recognisable in output (we don't
    // pin its exact form because the compactor may reformat it).
    assert!(
        out.contains("foo") && out.contains("1.0.0"),
        "core JSON tokens must survive compaction; got: {out}"
    );
}

#[test]
fn level_full_is_idempotent() {
    // Running Full twice should produce the same result as once —
    // the pipeline must converge on a fixed point.
    let once = compact_output(REALISTIC_TOOL_STREAM, CompactionLevel::Full);
    let twice = compact_output(&once, CompactionLevel::Full);
    assert_eq!(once, twice, "Full compaction must be idempotent");
}

#[test]
fn empty_input_survives_every_level() {
    for level in [
        CompactionLevel::Off,
        CompactionLevel::Safe,
        CompactionLevel::Full,
    ] {
        let out = compact_output("", level);
        assert_eq!(
            out, "",
            "{:?} level must not synthesise content from empty",
            level
        );
    }
}

#[test]
fn ansi_only_input_collapses_to_empty_at_safe_or_higher() {
    let ansi_only = "\u{1b}[31m\u{1b}[1m\u{1b}[0m";
    assert_eq!(
        compact_output(ansi_only, CompactionLevel::Safe),
        "",
        "Safe must strip pure-ANSI to empty"
    );
    assert_eq!(
        compact_output(ansi_only, CompactionLevel::Full),
        "",
        "Full must strip pure-ANSI to empty"
    );
    // Off path leaves it intact.
    assert_eq!(compact_output(ansi_only, CompactionLevel::Off), ansi_only);
}

#[test]
fn carriage_return_progress_collapses_to_final_state() {
    // Common terminal-progress idiom: '\r' overwrites the same line.
    // Sanitize should keep only the post-last-CR substring per line.
    let progress = "loading 10%\rloading 50%\rloading 100%\ndone\n";
    let out = compact_output(progress, CompactionLevel::Safe);
    assert!(
        out.contains("loading 100%"),
        "must keep the final progress state; got: {out}"
    );
    assert!(
        !out.contains("loading 10%") && !out.contains("loading 50%"),
        "must drop the overwritten progress states; got: {out}"
    );
    assert!(
        out.contains("done"),
        "must keep subsequent lines; got: {out}"
    );
}

#[test]
fn full_level_preserves_structured_json_payload() {
    // A JSON-shaped payload should round-trip through compaction
    // semantically intact (formatting may change).
    let input = "{\"users\":[{\"id\":1,\"name\":\"alice\"},{\"id\":2,\"name\":\"bob\"}]}\n";
    let out = compact_output(input, CompactionLevel::Full);
    assert!(out.contains("alice") && out.contains("bob"));
    assert!(out.contains("\"id\""));
}

#[test]
fn toon_encoding_round_trips_simple_structures() {
    // The TOON path is a separate, opt-in encoding for token-efficient
    // structure transfer. Smoke that it does NOT panic and produces
    // non-empty output for a tabular payload (where TOON pays off).
    let tabular = "{\"users\":[{\"id\":1,\"name\":\"alice\"},{\"id\":2,\"name\":\"bob\"}]}";
    let out = compact_output_toon(tabular);
    assert!(!out.is_empty(), "TOON must not produce empty output");
    // Tokens still recognisable in the encoded output.
    assert!(out.contains("alice") || out.contains("bob"), "got: {out}");
}

#[test]
fn extreme_repetition_compresses_aggressively_at_full() {
    // 1000 identical lines is a worst-case for the fold pass — should
    // collapse to a 1-line-plus-count representation, not 1000 lines.
    let input = "noisy log line\n".repeat(1000);
    let safe_len = compact_output(&input, CompactionLevel::Safe).len();
    let full_len = compact_output(&input, CompactionLevel::Full).len();
    assert_eq!(
        safe_len,
        input.len(),
        "Safe must not change length on no-ANSI input"
    );
    assert!(
        full_len < input.len() / 10,
        "Full must collapse 1000x repetition to <10% of input; got {full_len}/{}",
        input.len()
    );
}

#[test]
fn mixed_workload_pipeline_smoke() {
    // The big one: ANSI + CR progress + repeats + JSON tail in one
    // input. Full level must produce something dramatically smaller
    // than the input without losing the structural payload.
    let input = REALISTIC_TOOL_STREAM;
    let full = compact_output(input, CompactionLevel::Full);
    assert!(
        full.len() < input.len(),
        "Full must reduce size; input={} full={}",
        input.len(),
        full.len()
    );
    // Core data survives.
    assert!(full.contains("Installing"));
    assert!(full.contains("done"));
    assert!(full.contains("foo"));
}
