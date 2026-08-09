//! B2 — the Bash result the model is shown must not lie about what the
//! command printed.
//!
//! `compact_bash` runs over the ALREADY-RENDERED BashTool envelope
//! (`Exit code: N\nSTDOUT:\n…\nSTDERR:\n…`). Three independently observed
//! defects live there, all reproduced against the shipped binary by the
//! conformance probes `pr04_interleave`, `pr05b_stream_labels`:
//!
//!   1. a ~39-line cliff: a 200-line, 1.4 KiB command output lost 82% of its
//!      lines even though it costs almost no tokens;
//!   2. the `STDERR:` delimiter was elided, so stderr diagnostics were
//!      presented to the model under the `STDOUT:` heading;
//!   3. the `--- last N lines ---` appendix re-stated lines the body already
//!      ended with, so the model saw output the command printed only once
//!      twice.
//!
//! Each test below is graded on the returned string only — no product claim
//! about itself is trusted.

use wcore_tools::bash_compact::compact_bash;

/// Build a BashTool result envelope exactly as `bash.rs` renders one.
fn envelope(stdout: &str, stderr: &str) -> String {
    format!("Exit code: 0\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}")
}

fn joined(prefix: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Leg 1 — the line cliff. 100 stdout + 100 stderr lines is ~1.4 KiB: far
/// below the byte budget compaction exists to defend, and every line matters
/// (this is what `git status`, a test run or a build log looks like). It must
/// arrive intact.
#[test]
fn a_thousand_bytes_of_output_survives_intact() {
    let raw = envelope(&joined("O", 100), &joined("E", 100));
    assert!(
        raw.len() < 8 * 1024,
        "precondition: this fixture must be small in bytes, got {}",
        raw.len()
    );

    let got = compact_bash("some-command", &raw, 0);

    for i in 1..=100 {
        assert!(
            got.content.contains(&format!("O{i}\n")) || got.content.ends_with(&format!("O{i}")),
            "stdout line O{i} was dropped from a {}-byte result:\n{}",
            raw.len(),
            got.content
        );
        assert!(
            got.content.contains(&format!("E{i}\n")) || got.content.ends_with(&format!("E{i}")),
            "stderr line E{i} was dropped from a {}-byte result:\n{}",
            raw.len(),
            got.content
        );
    }
}

/// Leg 2 — the stream delimiter. Whatever compaction does to a genuinely
/// large result, it may never delete the `STDERR:` boundary, because a model
/// reading stderr under the `STDOUT:` heading attributes diagnostics to the
/// wrong stream.
#[test]
fn large_output_keeps_the_stderr_boundary() {
    // ~11 KiB: over the byte budget, so compaction really does engage.
    let raw = envelope(&joined("OUTMARK", 500), &joined("ERRMARK", 500));
    assert!(
        raw.len() > 8 * 1024,
        "precondition: this fixture must clear the size gate, got {}",
        raw.len()
    );

    let got = compact_bash("some-command", &raw, 0);
    assert!(
        got.compacted_bytes < got.raw_bytes,
        "precondition: this fixture must actually be compacted"
    );

    let delim = got.content.find("\nSTDERR:\n");
    assert!(
        delim.is_some(),
        "the STDERR delimiter was eaten by compaction:\n{}",
        got.content
    );
    let first_err = got.content.find("ERRMARK");
    if let Some(err_at) = first_err {
        assert!(
            err_at > delim.expect("delimiter present"),
            "a stderr line is presented before the STDERR delimiter \
             (STDERR: at {:?}, first ERRMARK at {err_at}):\n{}",
            delim,
            got.content
        );
    }
}

/// Leg 3 — duplication. The `--- last N lines ---` appendix is insurance that
/// the final error survived; it must not re-state lines the compacted body
/// already ends with. A line the command printed once must appear once.
#[test]
fn compaction_never_repeats_a_line_the_command_printed_once() {
    let raw = envelope(&joined("OUTMARK", 500), &joined("ERRMARK", 500));
    let got = compact_bash("some-command", &raw, 0);
    assert!(
        got.compacted_bytes < got.raw_bytes,
        "precondition: this fixture must actually be compacted"
    );

    let mut seen: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for line in got.content.lines() {
        let t = line.trim();
        if t.starts_with("OUTMARK") || t.starts_with("ERRMARK") {
            *seen.entry(t).or_default() += 1;
        }
    }
    let dupes: Vec<_> = seen.iter().filter(|(_, n)| **n > 1).collect();
    assert!(
        dupes.is_empty(),
        "compaction duplicated {} line(s) {:?} in:\n{}",
        dupes.len(),
        dupes,
        got.content
    );
}

/// Direction check for the two large-output tests: a body large enough to be
/// compacted really is shrunk, so the assertions above are not passing simply
/// because compaction never ran.
#[test]
fn large_unstructured_output_is_still_compacted() {
    let raw = envelope(&joined("OUTMARK", 500), &joined("ERRMARK", 500));
    let got = compact_bash("some-command", &raw, 0);
    assert!(
        got.compacted_bytes < got.raw_bytes,
        "large output must still be compacted: {} -> {}",
        got.raw_bytes,
        got.compacted_bytes
    );
    assert!(
        got.content.contains("omitted"),
        "compaction must disclose that it dropped lines:\n{}",
        got.content
    );
}
