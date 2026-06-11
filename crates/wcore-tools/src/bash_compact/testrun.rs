//! Block-aware compaction for non-cargo test-runner output
//! (pytest / jest / vitest / go test).
//!
//! Verbose test runs are mostly passing lines (`… PASSED`, `✓ …`,
//! `--- PASS: …`) that carry no signal once the run is over. We keep the
//! pass/fail SUMMARY line, the names of the FAILING tests, the `FAILURES`
//! header, and each failure's full detail block (assertion lines starting with
//! `>` or `E `, indented tracebacks, `thread '…' panicked` lines), and drop the
//! passing lines in between.
//!
//! Returns `None` when the output does not contain any recognizable
//! test-runner summary or failure marker, so the caller can fall through to a
//! generic classifier.

/// Hard cap on retained lines so a pathological run (thousands of failures)
/// can't defeat the point of compaction.
const MAX_KEPT_LINES: usize = 200;

/// Compact verbose test-runner output, preserving the summary and failures.
///
/// Returns `Some(compacted)` when the output is a recognizable test summary
/// (pytest / jest / vitest / go test) and was parsed; `None` when no test
/// marker is present.
pub(super) fn compact(raw: &str, exit_code: i32) -> Option<String> {
    // `exit_code` is part of the stable signature; a non-zero exit only
    // corroborates the failure markers we already detect from text, so we do
    // not branch on it here.
    let _ = exit_code;

    if !looks_like_test_output(raw) {
        return None;
    }

    let mut kept: Vec<&str> = Vec::new();

    for line in raw.lines() {
        if kept.len() >= MAX_KEPT_LINES {
            break;
        }
        if is_passing_line(line) {
            continue;
        }
        if is_kept_line(line) {
            kept.push(line);
        }
    }

    if kept.is_empty() {
        return None;
    }

    Some(kept.join("\n"))
}

/// True when the output contains at least one test-runner summary or failure
/// marker. Arbitrary noise text returns false.
fn looks_like_test_output(raw: &str) -> bool {
    raw.lines().any(|line| {
        is_summary_line(line)
            || is_failures_header(line)
            || line.contains("--- FAIL:")
            || line.contains("FAILED")
            || line.contains("PASSED")
    })
}

/// A pass/fail summary line: pytest's `N failed, M passed` banner, jest/vitest's
/// `Tests:` line, or any line pairing a digit with `passed`/`failed`.
fn is_summary_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if lower.contains("tests:") {
        return true;
    }
    let has_digit = line.chars().any(|c| c.is_ascii_digit());
    has_digit && (lower.contains("passed") || lower.contains("failed"))
}

/// The pytest `=== FAILURES ===` section header (with or without the `=` rule).
fn is_failures_header(line: &str) -> bool {
    let trimmed = line.trim_matches('=').trim();
    trimmed == "FAILURES"
}

/// Lines we keep: summaries, failure markers, the FAILURES header, per-failure
/// detail/traceback lines, and pytest `___ test_name ___` failure headers.
fn is_kept_line(line: &str) -> bool {
    if is_summary_line(line) || is_failures_header(line) {
        return true;
    }

    // Failure markers across runners.
    if line.contains("FAILED")
        || line.contains("--- FAIL:")
        || line.contains('✗')
        || line.starts_with("FAIL")
    {
        return true;
    }

    // pytest per-test failure header, e.g. `___ test_boom ___`.
    if is_underscore_header(line) {
        return true;
    }

    // Assertion / traceback detail lines.
    let trimmed = line.trim_start();
    if trimmed.starts_with('>') || trimmed.starts_with("E ") {
        return true;
    }
    if line.contains("panicked") {
        return true;
    }

    false
}

/// pytest underscore-delimited failure header: `___ test_name ___`.
fn is_underscore_header(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('_') && trimmed.ends_with('_') && trimmed.contains(char::is_alphabetic)
}

/// Lines that indicate a passing test and should be dropped.
fn is_passing_line(line: &str) -> bool {
    line.contains("PASSED")
        || line.contains("--- PASS:")
        || line.contains('✓')
        || line.ends_with(" ok")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pytest_keeps_failures_and_traceback() {
        let mut raw = String::new();
        for i in 0..80 {
            raw.push_str(&format!("tests/test_{i}.py::ok PASSED\n"));
        }
        raw.push_str("tests/test_x.py::boom FAILED\n");
        raw.push_str("=================== FAILURES ===================\n");
        raw.push_str("___ test_boom ___\n>   assert x == y\nE   assert 1 == 2\n");
        raw.push_str("=========== 1 failed, 80 passed in 2.0s ===========\n");
        let out = compact(&raw, 1).expect("pytest compacts");
        assert!(out.contains("1 failed, 80 passed"));
        assert!(out.contains("test_boom"));
        assert!(out.contains("assert 1 == 2"));
        assert!(!out.contains("test_40.py::ok PASSED"), "drop passing tests");
        assert!(out.len() < raw.len());
    }

    #[test]
    fn returns_none_without_summary() {
        assert!(compact(&"noise\n".repeat(60), 0).is_none());
    }
}
