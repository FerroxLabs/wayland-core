//! Assertion + trace-assertion types — plan §2.1.
//!
//! T1/T2 wave declares the type shapes so scenarios can be written and
//! the runner can collect them. T3 (Wave 0) implements the text-level
//! matching logic for the original variants; Wave 1.1 adds result-level
//! variants (`StderrContains`, `StderrContainsAny`, `CostWithinTolerance`)
//! that operate on [`crate::runner::ScenarioResult`] via
//! [`Assertion::check_result`] rather than raw final text — closing the
//! R-009 silent-pass archetype that motivated this gate.
//!
//! # Silent-pass CI gate
//!
//! `clippy::todo` is denied at the crate root (see `lib.rs`) — any new
//! `todo!()` here or anywhere in this crate will fail
//! `cargo clippy -p wcore-eval-scenarios -- -D warnings`, preventing
//! the silent-pass regression from creeping back in.

use std::path::Path;

use crate::runner::ScenarioResult;
use crate::trace::ToolTrace;

/// Output-text assertion — runs against the final assistant text after
/// the last turn completes.
#[derive(Debug, Clone)]
pub enum Assertion {
    Contains(&'static str),
    ContainsAny(Vec<&'static str>),
    NotContains(&'static str),
    Regex(&'static str),
    JsonPath {
        path: &'static str,
        expected: serde_json::Value,
    },
    MinLength(usize),
    /// M-4: "must produce ≥ N distinct matches of this regex" — guards
    /// against hallucinated single-template answers passing a loose
    /// `Contains` check (S11 trending repos was the motivating bug).
    MinDistinctMatches {
        regex: &'static str,
        n: usize,
    },

    // --- Wave 1.1: result-level assertions (close the silent-pass class) ---
    /// Assert that `ScenarioResult::stderr_tail` contains the given substring.
    /// Missing = FAIL (not WARN). Eliminates the R-009/R-011/R-012 archetype.
    StderrContains(&'static str),

    /// Assert that the `session_cost` event was received (cost_usd > 0) AND
    /// that `cost_usd` is within `tolerance_fraction` of `expected_usd`.
    ///
    /// Example: `CostWithinTolerance { expected_usd: 0.002, tolerance_fraction: 0.10 }`
    /// passes when `|cost_usd - 0.002| / 0.002 <= 0.10`.
    ///
    /// If `cost_usd == 0.0` the event was not received — always FAIL.
    CostWithinTolerance {
        expected_usd: f64,
        tolerance_fraction: f64,
    },

    /// Assert that a JSON-stream event with `"type": event_type` was
    /// captured AND that it carries `field_name` with value equal to
    /// `expected_value` (string comparison). Uses `ScenarioResult::stderr_tail`
    /// as a proxy for "engine emitted the log that confirms the event" when
    /// the runner does not surface raw protocol events directly.
    ///
    /// Primary use: confirm a ready-event field via a log line that echoes it
    /// (e.g. `user-model: using local backend`). For event presence without
    /// field checking, use `StderrContains`.
    StderrContainsAny(Vec<&'static str>),

    /// D1/D2: assert that some `info` protocol event emitted across the run
    /// carries the given substring. Used to verify slash-command acks
    /// ("style updated", "conversation cleared", "mode updated") and engine
    /// notices. Checked against `ScenarioResult::info_events` via `check_result`.
    InfoContains(&'static str),

    // --- Artifact assertions: did the persona actually produce the file? ---
    // Paths are RELATIVE to the scenario workdir (the agent's cwd / tempenv
    // root). Checked via `check_artifacts(workdir)` — the whole point of a
    // "use it like a human" run is verifying the book.pdf / index.html /
    // report.md exists on disk, not just that the model claimed it.
    /// The file at `workdir/<path>` exists and is non-empty.
    FileExists(&'static str),

    /// The file at `workdir/<path>` does NOT exist. Used to prove a *denied*
    /// tool-approval actually blocked the write (D3 `DenyAll`), or that a
    /// forbidden side effect didn't happen.
    FileAbsent(&'static str),

    /// The file at `workdir/<path>` exists and contains `needle` (UTF-8).
    FileContains {
        path: &'static str,
        needle: &'static str,
    },

    /// The file at `workdir/<path>` exists and parses as `format`.
    /// Supported formats: `"pdf"` (magic `%PDF-`), `"json"` (serde parse),
    /// `"html"` (`<html`/`<!doctype html`), `"md"` (non-empty UTF-8 text).
    FileParsesAs {
        path: &'static str,
        format: &'static str,
    },
}

impl Assertion {
    /// Evaluate this assertion against an assistant-final-text sample.
    /// Returns `Ok(())` on pass, `Err(observed_summary)` on fail.
    ///
    /// Wave 1.1 result-level variants (`StderrContains`, `StderrContainsAny`,
    /// `CostWithinTolerance`) must be checked via [`Assertion::check_result`]
    /// instead — calling `check` on them returns Err pointing at the right method.
    pub fn check(&self, final_text: &str) -> Result<(), String> {
        match self {
            Assertion::Contains(needle) => {
                if final_text.contains(needle) {
                    Ok(())
                } else {
                    Err(format!(
                        "Contains({needle:?}) FAIL: substring not found in output ({} chars)",
                        final_text.len()
                    ))
                }
            }

            Assertion::ContainsAny(needles) => {
                if needles.iter().any(|n| final_text.contains(n)) {
                    Ok(())
                } else {
                    Err(format!(
                        "ContainsAny({needles:?}) FAIL: none of the substrings found in output ({} chars)",
                        final_text.len()
                    ))
                }
            }

            Assertion::NotContains(needle) => {
                if !final_text.contains(needle) {
                    Ok(())
                } else {
                    Err(format!(
                        "NotContains({needle:?}) FAIL: forbidden substring found in output"
                    ))
                }
            }

            Assertion::Regex(pattern) => {
                // Use a simple hand-rolled check without pulling in the
                // `regex` crate. For the subset of patterns used in the
                // regression suite (literal substrings + basic wildcards)
                // this is sufficient. If a full regex engine is needed,
                // add `regex` to Cargo.toml and replace this block.
                //
                // Supported pattern syntax:
                //   - Plain literal string match (most cases)
                //   - Leading `^` anchors to start-of-text
                //   - Trailing `$` anchors to end-of-text
                //   - `.*` treated as "any characters" (wildcard segment)
                regex_match_simple(pattern, final_text).map_err(|()| {
                    format!(
                        "Regex({pattern:?}) FAIL: pattern not matched in output ({} chars)",
                        final_text.len()
                    )
                })
            }

            Assertion::JsonPath { path, expected } => {
                // Parse final_text as JSON and evaluate a simple dotted
                // path. Supports dot-separated keys only (e.g. "foo.bar").
                let v: serde_json::Value = serde_json::from_str(final_text).map_err(|e| {
                    format!("JsonPath({path:?}) FAIL: final_text is not valid JSON: {e}")
                })?;
                let actual = json_path_get(&v, path).ok_or_else(|| {
                    format!("JsonPath({path:?}) FAIL: path not found in JSON output")
                })?;
                if actual == expected {
                    Ok(())
                } else {
                    Err(format!(
                        "JsonPath({path:?}) FAIL: expected {expected}, got {actual}"
                    ))
                }
            }

            Assertion::MinLength(min) => {
                if final_text.len() >= *min {
                    Ok(())
                } else {
                    Err(format!(
                        "MinLength({min}) FAIL: output is {} chars (need ≥ {min})",
                        final_text.len()
                    ))
                }
            }

            Assertion::MinDistinctMatches { regex, n } => {
                // Count distinct non-overlapping matches of the pattern.
                // Uses the same simple regex engine as Assertion::Regex.
                let count = count_distinct_matches(regex, final_text);
                if count >= *n {
                    Ok(())
                } else {
                    Err(format!(
                        "MinDistinctMatches({regex:?}, n={n}) FAIL: found {count} distinct matches (need ≥ {n})"
                    ))
                }
            }

            // Wave 1.1 result-level variants — wrong method called; redirect.
            Assertion::StderrContains(_)
            | Assertion::CostWithinTolerance { .. }
            | Assertion::StderrContainsAny(_)
            | Assertion::InfoContains(_) => {
                Err("call check_result() for result-level assertions (StderrContains / StderrContainsAny / CostWithinTolerance / InfoContains)".to_string())
            }

            // Artifact variants — filesystem checks; wrong method called.
            Assertion::FileExists(_)
            | Assertion::FileAbsent(_)
            | Assertion::FileContains { .. }
            | Assertion::FileParsesAs { .. } => Err(
                "call check_artifacts(workdir) for artifact assertions (FileExists / FileContains / FileParsesAs)"
                    .to_string(),
            ),
        }
    }

    /// True for filesystem (artifact) variants — the runner dispatches these
    /// to [`Assertion::check_artifacts`] instead of [`Assertion::check`].
    pub fn is_artifact(&self) -> bool {
        matches!(
            self,
            Assertion::FileExists(_)
                | Assertion::FileAbsent(_)
                | Assertion::FileContains { .. }
                | Assertion::FileParsesAs { .. }
        )
    }

    /// True for Wave-1.1 result-level variants — the runner dispatches these
    /// to [`Assertion::check_result`] (after the [`ScenarioResult`] is built)
    /// instead of [`Assertion::check`]. Closes the gap where these were never
    /// evaluated by the runner (cross-audit finding #4; masterplan Part A
    /// "fix Wave 1.1 assertion wiring").
    pub fn is_result_level(&self) -> bool {
        matches!(
            self,
            Assertion::StderrContains(_)
                | Assertion::StderrContainsAny(_)
                | Assertion::CostWithinTolerance { .. }
                | Assertion::InfoContains(_)
        )
    }

    /// Evaluate an artifact assertion against the scenario `workdir` (the
    /// agent's cwd). Relative paths resolve under `workdir`. Returns `Ok(())`
    /// on pass, `Err(message)` on fail.
    ///
    /// Panics on non-artifact variants — those use [`Assertion::check`] /
    /// [`Assertion::check_result`].
    pub fn check_artifacts(&self, workdir: &Path) -> Result<(), String> {
        match self {
            Assertion::FileExists(rel) => {
                let p = workdir.join(rel);
                match std::fs::metadata(&p) {
                    Ok(m) if m.len() > 0 => Ok(()),
                    Ok(_) => Err(format!(
                        "FileExists({rel:?}) FAIL: file exists but is empty"
                    )),
                    Err(_) => Err(format!(
                        "FileExists({rel:?}) FAIL: no file at {}",
                        p.display()
                    )),
                }
            }

            Assertion::FileAbsent(rel) => {
                let p = workdir.join(rel);
                match std::fs::metadata(&p) {
                    // Treat an empty file as "absent enough" — a denied write
                    // sometimes leaves a 0-byte touch. A non-empty file means
                    // the write actually landed → FAIL.
                    Ok(m) if m.len() > 0 => Err(format!(
                        "FileAbsent({rel:?}) FAIL: file exists with {} bytes at {} \
                         (the write was NOT blocked)",
                        m.len(),
                        p.display()
                    )),
                    _ => Ok(()),
                }
            }

            Assertion::FileContains { path, needle } => {
                let p = workdir.join(path);
                let body = std::fs::read_to_string(&p).map_err(|e| {
                    format!(
                        "FileContains({path:?}) FAIL: cannot read {}: {e}",
                        p.display()
                    )
                })?;
                if body.contains(needle) {
                    Ok(())
                } else {
                    Err(format!(
                        "FileContains({path:?}, {needle:?}) FAIL: needle not found in {} ({} bytes)",
                        p.display(),
                        body.len()
                    ))
                }
            }

            Assertion::FileParsesAs { path, format } => {
                let p = workdir.join(path);
                let bytes = std::fs::read(&p).map_err(|e| {
                    format!(
                        "FileParsesAs({path:?}) FAIL: cannot read {}: {e}",
                        p.display()
                    )
                })?;
                let ok = match *format {
                    "pdf" => bytes.starts_with(b"%PDF-"),
                    "json" => serde_json::from_slice::<serde_json::Value>(&bytes).is_ok(),
                    "html" => {
                        let lower = String::from_utf8_lossy(&bytes).to_lowercase();
                        lower.contains("<html") || lower.contains("<!doctype html")
                    }
                    // Markdown has no strict grammar — a non-empty UTF-8 text
                    // file is the honest bar (FileContains adds content checks).
                    "md" => std::str::from_utf8(&bytes).is_ok() && !bytes.is_empty(),
                    other => {
                        return Err(format!(
                            "FileParsesAs({path:?}) FAIL: unknown format {other:?} \
                             (supported: pdf | json | html | md)"
                        ));
                    }
                };
                if ok {
                    Ok(())
                } else {
                    Err(format!(
                        "FileParsesAs({path:?}, {format:?}) FAIL: {} ({} bytes) does not parse as {format}",
                        p.display(),
                        bytes.len()
                    ))
                }
            }

            _ => panic!(
                "check_artifacts() called on a non-artifact assertion {self:?} — \
                 use check(final_text) / check_result(result)"
            ),
        }
    }

    /// Evaluate a Wave-1.1 result-level assertion against a completed
    /// [`ScenarioResult`]. Returns `Ok(())` on pass, `Err(message)` on fail.
    ///
    /// Panics on text-level variants — those must use [`Assertion::check`].
    pub fn check_result(&self, result: &ScenarioResult) -> Result<(), String> {
        match self {
            Assertion::StderrContains(needle) => {
                if result.stderr_tail.contains(needle) {
                    Ok(())
                } else {
                    Err(format!(
                        "expected stderr to contain {needle:?} but it was absent.\n\
                         stderr tail:\n{}",
                        result.stderr_tail
                    ))
                }
            }

            Assertion::StderrContainsAny(needles) => {
                let found = needles.iter().any(|n| result.stderr_tail.contains(*n));
                if found {
                    Ok(())
                } else {
                    Err(format!(
                        "expected stderr to contain at least one of {:?} but none matched.\n\
                         stderr tail:\n{}",
                        needles, result.stderr_tail
                    ))
                }
            }

            Assertion::CostWithinTolerance {
                expected_usd,
                tolerance_fraction,
            } => {
                if result.cost_usd == 0.0 {
                    return Err(format!(
                        "session_cost event not received (cost_usd == 0.0) — \
                         the engine may not emit cost attribution for this provider/build. \
                         Expected cost ≈ ${expected_usd:.6}"
                    ));
                }
                let deviation = (result.cost_usd - expected_usd).abs() / expected_usd;
                if deviation <= *tolerance_fraction {
                    Ok(())
                } else {
                    Err(format!(
                        "cost ${:.7} deviates {:.1}% from expected ${:.6} \
                         (tolerance {:.0}%)",
                        result.cost_usd,
                        deviation * 100.0,
                        expected_usd,
                        tolerance_fraction * 100.0,
                    ))
                }
            }

            Assertion::InfoContains(needle) => {
                if result.info_events.iter().any(|m| m.contains(needle)) {
                    Ok(())
                } else {
                    Err(format!(
                        "expected an `info` event containing {needle:?} but none matched.\n\
                         info events: {:?}",
                        result.info_events
                    ))
                }
            }

            _ => panic!(
                "check_result() called on a text-level assertion variant {:?} — \
                 use check(final_text) for these",
                self
            ),
        }
    }
}

/// Trace-level assertion — runs against the ordered [`ToolTrace`] the
/// runner accumulates from json-stream `ToolResult` events.
#[derive(Debug, Clone)]
pub enum TraceAssertion {
    CountAtLeast {
        tool: &'static str,
        n: usize,
    },
    CountAtMost {
        tool: &'static str,
        n: usize,
    },
    OrderedBefore {
        earlier: &'static str,
        later: &'static str,
    },
    /// No `ToolResult { is_error: true }` anywhere in the trace.
    NoErrors,
    /// M-4: NoErrors scoped to a single tool (S11 must allow other
    /// tool errors but the load-bearing WebFetch must not 4xx).
    NoErrorsOnTool(&'static str),
}

impl TraceAssertion {
    /// Evaluate against an accumulated [`ToolTrace`].
    pub fn check(&self, trace: &ToolTrace) -> Result<(), String> {
        match self {
            TraceAssertion::CountAtLeast { tool, n } => {
                let count = trace.count(tool);
                if count >= *n {
                    Ok(())
                } else {
                    Err(format!(
                        "CountAtLeast({tool:?}, n={n}) FAIL: tool called {count} times (need ≥ {n})"
                    ))
                }
            }

            TraceAssertion::CountAtMost { tool, n } => {
                let count = trace.count(tool);
                if count <= *n {
                    Ok(())
                } else {
                    Err(format!(
                        "CountAtMost({tool:?}, n={n}) FAIL: tool called {count} times (need ≤ {n})"
                    ))
                }
            }

            TraceAssertion::OrderedBefore { earlier, later } => {
                let earlier_pos = trace.entries.iter().position(|e| e.tool_name == *earlier);
                let later_pos = trace.entries.iter().position(|e| e.tool_name == *later);

                match (earlier_pos, later_pos) {
                    (Some(e_pos), Some(l_pos)) if e_pos < l_pos => Ok(()),
                    (Some(_), Some(_)) => Err(format!(
                        "OrderedBefore({earlier:?}, {later:?}) FAIL: \
                         {later} appeared before {earlier} in the trace"
                    )),
                    (None, _) => Err(format!(
                        "OrderedBefore({earlier:?}, {later:?}) FAIL: \
                         {earlier} never appeared in the trace"
                    )),
                    (_, None) => Err(format!(
                        "OrderedBefore({earlier:?}, {later:?}) FAIL: \
                         {later} never appeared in the trace"
                    )),
                }
            }

            TraceAssertion::NoErrors => {
                let errors: Vec<_> = trace
                    .entries
                    .iter()
                    .filter(|e| e.is_error)
                    .map(|e| e.tool_name.as_str())
                    .collect();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "NoErrors FAIL: {} tool call(s) returned errors: {:?}",
                        errors.len(),
                        errors
                    ))
                }
            }

            TraceAssertion::NoErrorsOnTool(tool) => {
                let errors: Vec<_> = trace
                    .entries
                    .iter()
                    .filter(|e| e.tool_name == *tool && e.is_error)
                    .collect();
                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "NoErrorsOnTool({tool:?}) FAIL: {tool} returned {} error(s)",
                        errors.len()
                    ))
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Simple pattern matching without pulling in the `regex` crate.
/// Supports: literal match, `^` start-anchor, `$` end-anchor, `.*` wildcard.
fn regex_match_simple(pattern: &str, text: &str) -> Result<(), ()> {
    // Strip anchors and collect segments split by `.*`.
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$') && !pattern.ends_with("\\$");

    let core = pattern.trim_start_matches('^').trim_end_matches('$');

    if !core.contains(".*") {
        // Plain literal (possibly anchored).
        if anchored_start && anchored_end {
            return if text == core { Ok(()) } else { Err(()) };
        }
        if anchored_start {
            return if text.starts_with(core) {
                Ok(())
            } else {
                Err(())
            };
        }
        if anchored_end {
            return if text.ends_with(core) {
                Ok(())
            } else {
                Err(())
            };
        }
        return if text.contains(core) { Ok(()) } else { Err(()) };
    }

    // Split on `.*` and match segments left-to-right.
    let segments: Vec<&str> = core.split(".*").collect();
    let mut remaining = text;

    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            continue;
        }
        let is_first = i == 0;
        let is_last = i == segments.len() - 1;

        if is_first && anchored_start {
            if !remaining.starts_with(seg) {
                return Err(());
            }
            remaining = &remaining[seg.len()..];
        } else if is_last && anchored_end {
            if !remaining.ends_with(seg) {
                return Err(());
            }
        } else if let Some(pos) = remaining.find(seg) {
            remaining = &remaining[pos + seg.len()..];
        } else {
            return Err(());
        }
    }

    Ok(())
}

/// Count non-overlapping occurrences of a simple pattern in text.
fn count_distinct_matches(pattern: &str, text: &str) -> usize {
    if pattern.is_empty() {
        return 0;
    }
    // For wildcard patterns use a greedy left-to-right scan; for plain
    // literals use standard string search.
    if !pattern.contains(".*") && !pattern.starts_with('^') && !pattern.ends_with('$') {
        // Fast path: literal substring count.
        let mut count = 0usize;
        let mut search_from = 0usize;
        while let Some(pos) = text[search_from..].find(pattern) {
            count += 1;
            search_from += pos + pattern.len().max(1);
            if search_from >= text.len() {
                break;
            }
        }
        return count;
    }
    // For anchored / wildcard patterns: match is binary (0 or 1).
    if regex_match_simple(pattern, text).is_ok() {
        1
    } else {
        0
    }
}

/// Evaluate a simple dotted JSON path against a Value.
/// Supports dot-separated object keys and `[N]` array indexing.
fn json_path_get<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = v;
    for segment in path.split('.') {
        // Check for array index notation: segment ends with `[N]`
        if let Some(bracket_pos) = segment.find('[') {
            let key = &segment[..bracket_pos];
            let idx_str = segment[bracket_pos + 1..].trim_end_matches(']');
            let idx: usize = idx_str.parse().ok()?;
            if !key.is_empty() {
                current = current.get(key)?;
            }
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trace::{ToolTrace, TraceEntry};

    fn make_trace(tools: &[(&str, bool)]) -> ToolTrace {
        ToolTrace {
            entries: tools
                .iter()
                .enumerate()
                .map(|(i, (name, is_error))| TraceEntry {
                    call_id: format!("call-{i}"),
                    tool_name: name.to_string(),
                    input: String::new(),
                    output: String::new(),
                    is_error: *is_error,
                    duration: None,
                    turn: 0,
                })
                .collect(),
        }
    }

    // --- Assertion::Contains ---

    #[test]
    fn contains_pass() {
        assert!(
            Assertion::Contains("hello")
                .check("say hello world")
                .is_ok()
        );
    }

    #[test]
    fn contains_fail() {
        assert!(
            Assertion::Contains("nope")
                .check("say hello world")
                .is_err()
        );
    }

    #[test]
    fn contains_any_pass() {
        assert!(
            Assertion::ContainsAny(vec!["nope", "hello"])
                .check("say hello world")
                .is_ok()
        );
    }

    #[test]
    fn contains_any_fail() {
        assert!(
            Assertion::ContainsAny(vec!["foo", "bar"])
                .check("say hello world")
                .is_err()
        );
    }

    #[test]
    fn not_contains_pass() {
        assert!(Assertion::NotContains("nope").check("say hello").is_ok());
    }

    #[test]
    fn not_contains_fail() {
        assert!(Assertion::NotContains("hello").check("say hello").is_err());
    }

    #[test]
    fn min_length_pass() {
        assert!(Assertion::MinLength(5).check("hello world").is_ok());
    }

    #[test]
    fn min_length_fail() {
        assert!(Assertion::MinLength(100).check("hi").is_err());
    }

    // --- Assertion::Regex ---

    #[test]
    fn regex_literal_pass() {
        assert!(Assertion::Regex("hello").check("say hello world").is_ok());
    }

    #[test]
    fn regex_literal_fail() {
        assert!(Assertion::Regex("nope").check("say hello world").is_err());
    }

    #[test]
    fn regex_start_anchor_pass() {
        assert!(Assertion::Regex("^say").check("say hello world").is_ok());
    }

    #[test]
    fn regex_start_anchor_fail() {
        assert!(Assertion::Regex("^hello").check("say hello world").is_err());
    }

    #[test]
    fn regex_end_anchor_pass() {
        assert!(Assertion::Regex("world$").check("say hello world").is_ok());
    }

    #[test]
    fn regex_end_anchor_fail() {
        assert!(Assertion::Regex("hello$").check("say hello world").is_err());
    }

    #[test]
    fn regex_wildcard_pass() {
        assert!(
            Assertion::Regex("hello.*world")
                .check("hello beautiful world")
                .is_ok()
        );
    }

    #[test]
    fn regex_wildcard_fail() {
        assert!(
            Assertion::Regex("hello.*world")
                .check("world beautiful hello")
                .is_err()
        );
    }

    // --- Assertion::JsonPath ---

    #[test]
    fn jsonpath_pass() {
        let text = r#"{"foo": {"bar": 42}}"#;
        assert!(
            Assertion::JsonPath {
                path: "foo.bar",
                expected: serde_json::json!(42),
            }
            .check(text)
            .is_ok()
        );
    }

    #[test]
    fn jsonpath_fail_wrong_value() {
        let text = r#"{"foo": {"bar": 99}}"#;
        assert!(
            Assertion::JsonPath {
                path: "foo.bar",
                expected: serde_json::json!(42),
            }
            .check(text)
            .is_err()
        );
    }

    #[test]
    fn jsonpath_fail_not_json() {
        assert!(
            Assertion::JsonPath {
                path: "foo",
                expected: serde_json::json!(1),
            }
            .check("not json")
            .is_err()
        );
    }

    // --- Assertion::MinDistinctMatches ---

    #[test]
    fn min_distinct_matches_literal_pass() {
        // "hello" appears 3 times.
        assert!(
            Assertion::MinDistinctMatches {
                regex: "hello",
                n: 3
            }
            .check("hello hello hello world")
            .is_ok()
        );
    }

    #[test]
    fn min_distinct_matches_literal_fail() {
        assert!(
            Assertion::MinDistinctMatches {
                regex: "hello",
                n: 4
            }
            .check("hello hello hello world")
            .is_err()
        );
    }

    // --- TraceAssertion::CountAtLeast ---

    #[test]
    fn count_at_least_pass() {
        let trace = make_trace(&[
            ("read_file", false),
            ("read_file", false),
            ("write_file", false),
        ]);
        assert!(
            TraceAssertion::CountAtLeast {
                tool: "read_file",
                n: 2
            }
            .check(&trace)
            .is_ok()
        );
    }

    #[test]
    fn count_at_least_fail() {
        let trace = make_trace(&[("read_file", false)]);
        assert!(
            TraceAssertion::CountAtLeast {
                tool: "read_file",
                n: 2
            }
            .check(&trace)
            .is_err()
        );
    }

    // --- TraceAssertion::CountAtMost ---

    #[test]
    fn count_at_most_pass() {
        let trace = make_trace(&[("read_file", false)]);
        assert!(
            TraceAssertion::CountAtMost {
                tool: "read_file",
                n: 3
            }
            .check(&trace)
            .is_ok()
        );
    }

    #[test]
    fn count_at_most_fail() {
        let trace = make_trace(&[
            ("read_file", false),
            ("read_file", false),
            ("read_file", false),
        ]);
        assert!(
            TraceAssertion::CountAtMost {
                tool: "read_file",
                n: 2
            }
            .check(&trace)
            .is_err()
        );
    }

    // --- TraceAssertion::OrderedBefore ---

    #[test]
    fn ordered_before_pass() {
        let trace = make_trace(&[("search", false), ("read_file", false)]);
        assert!(
            TraceAssertion::OrderedBefore {
                earlier: "search",
                later: "read_file"
            }
            .check(&trace)
            .is_ok()
        );
    }

    #[test]
    fn ordered_before_fail_reversed() {
        let trace = make_trace(&[("read_file", false), ("search", false)]);
        assert!(
            TraceAssertion::OrderedBefore {
                earlier: "search",
                later: "read_file"
            }
            .check(&trace)
            .is_err()
        );
    }

    #[test]
    fn ordered_before_fail_missing() {
        let trace = make_trace(&[("read_file", false)]);
        assert!(
            TraceAssertion::OrderedBefore {
                earlier: "search",
                later: "read_file"
            }
            .check(&trace)
            .is_err()
        );
    }

    // --- TraceAssertion::NoErrors ---

    #[test]
    fn no_errors_pass() {
        let trace = make_trace(&[("read_file", false), ("write_file", false)]);
        assert!(TraceAssertion::NoErrors.check(&trace).is_ok());
    }

    #[test]
    fn no_errors_fail() {
        let trace = make_trace(&[("read_file", false), ("write_file", true)]);
        assert!(TraceAssertion::NoErrors.check(&trace).is_err());
    }

    // --- TraceAssertion::NoErrorsOnTool ---

    #[test]
    fn no_errors_on_tool_pass_different_tool_errors() {
        // write_file errors are fine; only read_file must be clean.
        let trace = make_trace(&[("read_file", false), ("write_file", true)]);
        assert!(
            TraceAssertion::NoErrorsOnTool("read_file")
                .check(&trace)
                .is_ok()
        );
    }

    #[test]
    fn no_errors_on_tool_fail() {
        let trace = make_trace(&[("read_file", true)]);
        assert!(
            TraceAssertion::NoErrorsOnTool("read_file")
                .check(&trace)
                .is_err()
        );
    }

    // --- ToolTrace::count ---

    #[test]
    fn tool_trace_count() {
        let trace = make_trace(&[
            ("read_file", false),
            ("read_file", false),
            ("write_file", false),
        ]);
        assert_eq!(trace.count("read_file"), 2);
        assert_eq!(trace.count("write_file"), 1);
        assert_eq!(trace.count("nonexistent"), 0);
    }

    // --- json_path_get ---

    #[test]
    fn json_path_nested() {
        let v = serde_json::json!({"a": {"b": {"c": "found"}}});
        let result = json_path_get(&v, "a.b.c");
        assert_eq!(result, Some(&serde_json::json!("found")));
    }

    #[test]
    fn json_path_missing() {
        let v = serde_json::json!({"a": 1});
        assert!(json_path_get(&v, "a.b.c").is_none());
    }

    // --- Artifact assertions (FileExists / FileContains / FileParsesAs) ---

    #[test]
    fn file_exists_pass_and_empty_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("book.md"), b"# Chapter 1\n").unwrap();
        std::fs::write(dir.path().join("empty.md"), b"").unwrap();

        assert!(
            Assertion::FileExists("book.md")
                .check_artifacts(dir.path())
                .is_ok()
        );
        // Empty file = FAIL (a persona that "wrote" a 0-byte artifact failed).
        assert!(
            Assertion::FileExists("empty.md")
                .check_artifacts(dir.path())
                .is_err()
        );
        // Missing file = FAIL.
        assert!(
            Assertion::FileExists("nope.md")
                .check_artifacts(dir.path())
                .is_err()
        );
    }

    #[test]
    fn file_contains_pass_and_fail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("post.md"), b"Launch week! Join us.").unwrap();
        assert!(
            Assertion::FileContains {
                path: "post.md",
                needle: "Launch week"
            }
            .check_artifacts(dir.path())
            .is_ok()
        );
        assert!(
            Assertion::FileContains {
                path: "post.md",
                needle: "discount code"
            }
            .check_artifacts(dir.path())
            .is_err()
        );
    }

    #[test]
    fn file_parses_as_pdf_json_html_md() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("r.pdf"), b"%PDF-1.7\n...binary...").unwrap();
        std::fs::write(dir.path().join("d.json"), br#"{"ok": true}"#).unwrap();
        std::fs::write(dir.path().join("p.html"), b"<!DOCTYPE html><html></html>").unwrap();
        std::fs::write(dir.path().join("n.md"), b"# Title\n\nbody").unwrap();
        std::fs::write(dir.path().join("notpdf.pdf"), b"i am not a pdf").unwrap();
        std::fs::write(dir.path().join("bad.json"), b"{ not json").unwrap();

        let pass = |p: &'static str, f: &'static str| {
            Assertion::FileParsesAs { path: p, format: f }.check_artifacts(dir.path())
        };
        assert!(pass("r.pdf", "pdf").is_ok());
        assert!(pass("d.json", "json").is_ok());
        assert!(pass("p.html", "html").is_ok());
        assert!(pass("n.md", "md").is_ok());
        // Negative cases.
        assert!(pass("notpdf.pdf", "pdf").is_err());
        assert!(pass("bad.json", "json").is_err());
        // Unknown format is an explicit error, not a silent pass.
        assert!(pass("n.md", "xml").is_err());
        // Missing file.
        assert!(pass("ghost.pdf", "pdf").is_err());
    }

    #[test]
    fn artifact_variants_redirect_from_check() {
        // Calling the text-level check() on an artifact variant must error
        // (not silently pass) so a mis-wired runner is caught.
        assert!(Assertion::FileExists("x").check("any text").is_err());
        assert!(Assertion::FileExists("x").is_artifact());
        assert!(!Assertion::Contains("x").is_artifact());
    }
}

// ---------------------------------------------------------------------------
// Unit tests for Wave-1.1 assertion variants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod wave_1_1_tests {
    use super::*;
    use crate::providers::ProviderId;
    use crate::runner::ScenarioResult;
    use crate::trace::ToolTrace;
    use std::time::Duration;

    fn make_result(stderr_tail: &str, cost_usd: f64) -> ScenarioResult {
        ScenarioResult {
            name: "test".to_string(),
            provider: ProviderId::Anthropic,
            passed: true,
            failures: Vec::new(),
            wall_time: Duration::from_millis(100),
            cost_usd,
            trace: ToolTrace::default(),
            final_text: String::new(),
            stderr_tail: stderr_tail.to_string(),
            turn_results: Vec::new(),
            workdir: std::path::PathBuf::new(),
            boot_time: Duration::ZERO,
            info_events: Vec::new(),
        }
    }

    // --- StderrContains ---

    #[test]
    fn stderr_contains_pass() {
        let result = make_result(
            "channel_manager.start_all() complete — inbound polling active",
            0.0,
        );
        let a = Assertion::StderrContains("start_all() complete");
        assert!(a.check_result(&result).is_ok());
    }

    #[test]
    fn stderr_contains_fail_when_absent() {
        let result = make_result("some other log line", 0.0);
        let a = Assertion::StderrContains("start_all() complete");
        let err = a.check_result(&result).unwrap_err();
        assert!(
            err.contains("start_all() complete"),
            "error should name needle: {err}"
        );
        assert!(
            err.contains("some other log line"),
            "error should show actual stderr: {err}"
        );
    }

    #[test]
    fn stderr_contains_fail_on_empty_stderr() {
        let result = make_result("", 0.0);
        let a = Assertion::StderrContains("anything");
        assert!(a.check_result(&result).is_err());
    }

    // --- StderrContainsAny ---

    #[test]
    fn stderr_contains_any_pass_first() {
        let result = make_result("user-model: using local backend", 0.0);
        let a = Assertion::StderrContainsAny(vec!["local backend", "HONCHO_API_KEY"]);
        assert!(a.check_result(&result).is_ok());
    }

    #[test]
    fn stderr_contains_any_pass_second() {
        let result = make_result("HONCHO_API_KEY not set", 0.0);
        let a = Assertion::StderrContainsAny(vec!["local backend", "HONCHO_API_KEY"]);
        assert!(a.check_result(&result).is_ok());
    }

    #[test]
    fn stderr_contains_any_fail_when_none_match() {
        let result = make_result("completely unrelated output", 0.0);
        let a = Assertion::StderrContainsAny(vec!["local backend", "HONCHO_API_KEY"]);
        let err = a.check_result(&result).unwrap_err();
        assert!(
            err.contains("local backend"),
            "error should list needles: {err}"
        );
    }

    // --- CostWithinTolerance ---

    #[test]
    fn cost_within_tolerance_pass_exact() {
        let result = make_result("", 0.002);
        let a = Assertion::CostWithinTolerance {
            expected_usd: 0.002,
            tolerance_fraction: 0.10,
        };
        assert!(a.check_result(&result).is_ok());
    }

    #[test]
    fn cost_within_tolerance_pass_edge() {
        // 9.9% deviation — within 10% tolerance
        let result = make_result("", 0.002 * 1.099);
        let a = Assertion::CostWithinTolerance {
            expected_usd: 0.002,
            tolerance_fraction: 0.10,
        };
        assert!(a.check_result(&result).is_ok());
    }

    #[test]
    fn cost_within_tolerance_fail_over() {
        // 50% deviation — outside 10% tolerance
        let result = make_result("", 0.003);
        let a = Assertion::CostWithinTolerance {
            expected_usd: 0.002,
            tolerance_fraction: 0.10,
        };
        let err = a.check_result(&result).unwrap_err();
        assert!(
            err.contains("deviates"),
            "error should mention deviation: {err}"
        );
    }

    #[test]
    fn cost_within_tolerance_fail_no_event() {
        // cost_usd == 0.0 means session_cost was never received
        let result = make_result("some logs here", 0.0);
        let a = Assertion::CostWithinTolerance {
            expected_usd: 0.002,
            tolerance_fraction: 0.10,
        };
        let err = a.check_result(&result).unwrap_err();
        assert!(
            err.contains("not received"),
            "error should say event not received: {err}"
        );
    }

    // --- check() returns Err for Wave-1.1 variants (not todo!) ---

    #[test]
    fn check_returns_err_for_wave11_stderr_contains() {
        let a = Assertion::StderrContains("foo");
        assert!(
            a.check("any text").is_err(),
            "StderrContains should return Err from check()"
        );
    }

    #[test]
    fn check_returns_err_for_wave11_cost() {
        let a = Assertion::CostWithinTolerance {
            expected_usd: 0.001,
            tolerance_fraction: 0.1,
        };
        assert!(
            a.check("any text").is_err(),
            "CostWithinTolerance should return Err from check()"
        );
    }
}
