//! Block-aware compaction for `cargo build/check/clippy/test/nextest` output.
//!
//! The goal is to drop the high-volume, low-signal noise (`Compiling …`,
//! `Checking …`, passing `... ok` test lines) while PRESERVING the full
//! diagnostic signal. A compiler diagnostic is not a single line: an
//! `error[E…]:` / `error:` / `warning:` trigger is followed by an indented
//! continuation block (the `-->` location, the `|` code frame, and `= help`/
//! `= note` annotations). We keep the trigger together with its whole block so
//! the surviving text is still actionable, not just a bare headline.
//!
//! For test output we keep the `test result:` summary, `FAILED` test names, and
//! each failure's `failures:` / `---- … ----` panic block, dropping the passing
//! lines in between.
//!
//! Returns `None` when the output does not look like cargo at all, so the
//! caller can fall through to a generic classifier.

/// Hard cap on retained lines so a pathological build (thousands of errors)
/// can't defeat the point of compaction.
const MAX_KEPT_LINES: usize = 200;

/// Compact verbose cargo output, preserving error/test signal block-aware.
///
/// Returns `Some(compacted)` when the output is recognizably cargo and was
/// parsed; `None` when it does not look like cargo (no recognizable marker).
pub(super) fn compact(raw: &str, exit_code: i32) -> Option<String> {
    if !looks_like_cargo(raw) {
        return None;
    }

    // A non-zero exit biases us toward keeping diagnostics: it confirms the
    // build/test actually failed, so error and failure blocks are the signal.
    let failed = exit_code != 0;

    let lines: Vec<&str> = raw.lines().collect();
    let mut kept: Vec<&str> = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        if kept.len() >= MAX_KEPT_LINES {
            break;
        }

        let line = lines[idx];
        let trimmed = line.trim_start();

        // Always keep the terminal "could not compile" verdict.
        if line.contains("could not compile") {
            kept.push(line);
            idx += 1;
            continue;
        }

        // Diagnostic trigger: keep the line plus its full continuation block.
        if is_diagnostic_trigger(trimmed) {
            kept.push(line);
            idx += 1;
            while idx < lines.len() && kept.len() < MAX_KEPT_LINES {
                if is_continuation(lines[idx]) {
                    kept.push(lines[idx]);
                    idx += 1;
                } else {
                    break;
                }
            }
            continue;
        }

        // Test summary line.
        if trimmed.starts_with("test result:") {
            kept.push(line);
            idx += 1;
            continue;
        }

        // A FAILED test name or a panic line on its own.
        if trimmed.contains("FAILED") || trimmed.contains("panicked") {
            kept.push(line);
            idx += 1;
            continue;
        }

        // `failures:` header, then the per-failure `---- … ----` blocks and
        // their indented panic/assertion bodies.
        if trimmed.starts_with("failures:") {
            kept.push(line);
            idx += 1;
            continue;
        }

        // A `---- name stdout ----` failure header: keep it plus the indented /
        // non-empty body lines that follow (the captured panic output).
        if is_failure_header(trimmed) {
            kept.push(line);
            idx += 1;
            while idx < lines.len() && kept.len() < MAX_KEPT_LINES {
                let body = lines[idx];
                let body_trimmed = body.trim_start();
                // Stop at the next failure header or a fresh `test ` line.
                if is_failure_header(body_trimmed) || body_trimmed.starts_with("test ") {
                    break;
                }
                if body.is_empty() {
                    idx += 1;
                    continue;
                }
                kept.push(body);
                idx += 1;
            }
            continue;
        }

        // Everything else (Compiling/Checking spam, `... ok`, running headers,
        // blank lines) is dropped.
        idx += 1;
    }

    // If we matched nothing structural, fall through to the generic
    // classifier rather than returning an empty result. `failed` only
    // sharpens intent here; an empty match is uninformative either way.
    if kept.is_empty() {
        let _ = failed;
        return None;
    }

    Some(kept.join("\n"))
}

/// Cheap detection: does this text contain any cargo-shaped marker at all?
fn looks_like_cargo(raw: &str) -> bool {
    raw.contains("Compiling ")
        || raw.contains("Checking ")
        || raw.contains("error[")
        || raw.contains("error:")
        || raw.contains("warning:")
        || raw.contains("test result:")
}

/// A diagnostic trigger line (already left-trimmed).
fn is_diagnostic_trigger(trimmed: &str) -> bool {
    trimmed.starts_with("error[")
        || trimmed.starts_with("error:")
        || trimmed.starts_with("warning:")
}

/// Is `line` (RAW, not trimmed) a continuation of a diagnostic block?
///
/// Continuations are blank lines, indented lines, or the rustc frame markers
/// `-->`, `|`, `=`. We test indentation on the raw line because the location /
/// code-frame markers are themselves indented in real output.
fn is_continuation(line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    if line.starts_with(char::is_whitespace) {
        return true;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with("-->") || trimmed.starts_with('|') || trimmed.starts_with('=') {
        return true;
    }
    // rustc code-frame gutter lines: `10 | bar.foo();` or `LL | ...`. After the
    // leading line-number (digits) or `LL` marker and spaces comes a `|`.
    let after_gutter = trimmed.trim_start_matches(|c: char| c.is_ascii_digit() || c == 'L');
    after_gutter.trim_start().starts_with('|') && after_gutter.len() < trimmed.len()
}

/// A `---- some::test stdout ----` style failure header (already left-trimmed).
fn is_failure_header(trimmed: &str) -> bool {
    trimmed.starts_with("----") && trimmed.ends_with("----") && trimmed.len() > 4
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(body: &str) -> String {
        let noise = (0..100)
            .map(|i| format!("   Compiling crate{i} v0.1.0"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{noise}\n{body}")
    }

    #[test]
    fn keeps_error_with_its_code_frame_block() {
        let raw = big("error[E0599]: no method named `foo` found\n \
             --> src/x.rs:10:20\n   |\n10 |     bar.foo();\n   |         ^^^ method not found\n   = help: did you mean `food`?\n\
             error: could not compile `x` due to previous error");
        let out = compact(&raw, 1).expect("cargo output should compact");
        assert!(out.contains("error[E0599]"));
        assert!(
            out.contains("--> src/x.rs:10:20"),
            "must keep the location line (block-aware)"
        );
        assert!(
            out.contains("method not found"),
            "must keep the code-frame continuation"
        );
        assert!(out.contains("could not compile"));
        assert!(!out.contains("Compiling crate50"), "must drop compile spam");
        assert!(out.len() < raw.len());
    }

    #[test]
    fn keeps_test_summary_and_failures_only() {
        let raw = big("running 200 tests\n\
             test a::ok ... ok\ntest a::ok2 ... ok\n\
             test a::boom ... FAILED\n\
             failures:\n---- a::boom stdout ----\nthread 'a::boom' panicked at 'assertion failed: x == y'\n\
             test result: FAILED. 199 passed; 1 failed");
        let out = compact(&raw, 101).expect("compact");
        assert!(out.contains("test result: FAILED. 199 passed; 1 failed"));
        assert!(out.contains("a::boom"));
        assert!(out.contains("panicked at"));
        assert!(!out.contains("a::ok2 ... ok"), "drop passing tests");
        assert!(out.len() < raw.len());
    }

    #[test]
    fn returns_none_for_non_cargo_text() {
        assert!(compact("totally unrelated output\n".repeat(50).as_str(), 0).is_none());
    }
}
