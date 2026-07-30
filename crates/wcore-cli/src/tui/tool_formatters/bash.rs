//! `Bash` (shell exec) tool formatter.
//!
//! # Payload shape — read from the tool, not invented (UAT-T3)
//!
//! `BashTool` does **not** return JSON. All three of its result-construction
//! sites (`wcore-tools/src/bash.rs`, in `output_to_result`, `execute` and
//! `execute_with_ctx`) build the same plain-text string:
//!
//! ```text
//! Exit code: 0
//! STDOUT:
//! hello
//!
//! STDERR:
//!
//! ```
//!
//! and that same string is what the LLM receives as the tool result, so it is
//! deliberately not changed. The formatter parses it.
//!
//! This file previously documented a payload of
//! `{ "cmd": …, "exit_code": …, "stdout": … }`, which no version of the tool
//! has ever produced. Because `parse_payload` degrades a non-JSON result to
//! `Value::String`, every `payload.get(…)` returned `None` and the summary
//! rendered the hardcoded defaults — ``Ran `?` · exit 0 · 0 bytes`` — for
//! every call, *including calls that failed with a non-zero exit*. The tests
//! were green because they built the invented shape by hand.
//!
//! Two rules now hold here:
//!
//! * **Never render a value that was not read.** A payload this formatter
//!   cannot parse yields the tool's real first line, not a fabricated triple.
//! * **`cmd` is not a result field and never was.** The command lives in the
//!   tool *input*, which the card header already renders as
//!   `Bash({ "command": … })`. Printing it a second time from the result was
//!   the reason a `?` had to be invented at all, so the summary no longer
//!   carries it.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{first_line_preview, join_facts, raw_text};
use crate::tui::theme::Theme;

/// Max lines of stdout shown in the detail view.
const MAX_STDOUT_LINES: usize = 20;

/// Max chars of an unparseable payload echoed on the summary line.
const RAW_PREVIEW: usize = 60;

/// The three markers `BashTool` writes, in order.
const EXIT_PREFIX: &str = "Exit code: ";
const STDOUT_MARKER: &str = "\nSTDOUT:\n";
const STDERR_MARKER: &str = "\nSTDERR:\n";

/// What a `Bash` result string decomposed into.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BashOutput<'a> {
    pub exit_code: i64,
    pub stdout: &'a str,
    pub stderr: &'a str,
}

/// Parse the exact text `BashTool` emits on a completed run.
///
/// Returns `None` for every other shape the tool can produce — the
/// missing-parameter, timeout, cancellation and `Failed to execute command`
/// paths are all bare prose with no exit code in them. Those must NOT be
/// coerced into a fake exit code; the caller shows the prose instead.
pub(crate) fn parse_bash_output(s: &str) -> Option<BashOutput<'_>> {
    let rest = s.strip_prefix(EXIT_PREFIX)?;
    let nl = rest.find('\n')?;
    let exit_code: i64 = rest[..nl].trim().parse().ok()?;

    // `STDOUT:` must directly follow the exit line for this to be the shape we
    // think it is; anything else and we decline rather than guess.
    if !s[EXIT_PREFIX.len() + nl..].starts_with(STDOUT_MARKER) {
        return None;
    }
    let after_stdout_marker = EXIT_PREFIX.len() + nl + STDOUT_MARKER.len();
    let tail = &s[after_stdout_marker..];

    // Split stdout from stderr on the LAST `\nSTDERR:\n`. The producer writes
    // exactly one, but command output is arbitrary bytes and could contain the
    // literal; taking the last occurrence keeps the *stderr* section intact,
    // which is the half a reader is more likely to need verbatim. Genuinely
    // ambiguous either way — documented rather than silently chosen.
    let (stdout, stderr) = match tail.rfind(STDERR_MARKER) {
        Some(i) => (&tail[..i], &tail[i + STDERR_MARKER.len()..]),
        None => (tail, ""),
    };
    Some(BashOutput {
        exit_code,
        stdout,
        stderr,
    })
}

pub struct BashFormatter;

impl ToolResultFormatter for BashFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let Some(text) = raw_text(payload) else {
            // No output yet (a running card) or a JSON payload this tool does
            // not produce. Say nothing rather than invent a completed run.
            return String::new();
        };

        let Some(out) = parse_bash_output(text) else {
            // A real bash failure mode with no exit code in it — a timeout, a
            // cancellation, a missing parameter. The tool's own words are the
            // honest summary.
            return first_line_preview(text, RAW_PREVIEW);
        };

        let mut facts = vec![format!("exit {}", out.exit_code)];
        facts.push(format!("{} bytes stdout", out.stdout.len()));
        if !out.stderr.is_empty() {
            facts.push(format!("{} bytes stderr", out.stderr.len()));
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let Some(text) = raw_text(payload) else {
            return Vec::new();
        };
        let dim = Style::default().fg(theme.text_dim);
        let err = Style::default().fg(theme.error);

        let Some(out) = parse_bash_output(text) else {
            // Unparseable: show the raw result. Previously this branch showed
            // NOTHING, which is why "the command's actual output is never
            // displayed at all" was part of the original finding.
            return text
                .lines()
                .take(MAX_STDOUT_LINES)
                .map(|s| Line::from(Span::styled(s.to_string(), dim)))
                .collect();
        };

        let mut lines: Vec<Line<'static>> = out
            .stdout
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(MAX_STDOUT_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), dim)))
            .collect();

        // Surface stderr too — on a failing command it is usually the only
        // thing that explains the exit code, and it was previously dropped.
        let budget = MAX_STDOUT_LINES.saturating_sub(lines.len());
        lines.extend(
            out.stderr
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(budget)
                .map(|s| Line::from(Span::styled(s.to_string(), err))),
        );
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: these tests deliberately do NOT construct a `json!({...})`
    // payload. The suite that did was green throughout the entire life of
    // this defect. The sibling module `super::super::real_payload_tests`
    // executes the real `BashTool` and asserts on what it returns; the cases
    // here cover the parser's edges around that.

    fn payload(s: &str) -> Value {
        // Exactly what `parse_payload` produces for a non-JSON tool result.
        serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
    }

    #[test]
    fn parses_the_real_bash_result_shape() {
        let got = parse_bash_output("Exit code: 0\nSTDOUT:\nhello\n\nSTDERR:\n").unwrap();
        assert_eq!(got.exit_code, 0);
        assert_eq!(got.stdout, "hello\n");
        assert_eq!(got.stderr, "");
    }

    #[test]
    fn summary_reports_the_real_nonzero_exit_code() {
        // The heart of UAT-T3: a failed call must never read `exit 0`.
        let f = BashFormatter;
        let p = payload("Exit code: 1\nSTDOUT:\n\nSTDERR:\nls: cannot access\n");
        let s = f.summary_line(&p, Duration::ZERO);
        assert!(s.contains("exit 1"), "must report the real exit code: {s}");
        assert!(
            !s.contains("exit 0"),
            "must not claim success on a failure: {s}"
        );
    }

    #[test]
    fn summary_reports_the_real_stdout_byte_count() {
        let f = BashFormatter;
        // "HELLO_FROM_UAT\n" is 15 bytes — the exact case the UAT reported as
        // `0 bytes`.
        let p = payload("Exit code: 0\nSTDOUT:\nHELLO_FROM_UAT\n\nSTDERR:\n");
        let s = f.summary_line(&p, Duration::ZERO);
        assert!(s.contains("15 bytes stdout"), "wrong byte count: {s}");
        assert!(!s.contains("0 bytes stdout"), "still fabricating zero: {s}");
    }

    #[test]
    fn never_fabricates_a_command() {
        let f = BashFormatter;
        let p = payload("Exit code: 0\nSTDOUT:\nx\n\nSTDERR:\n");
        let s = f.summary_line(&p, Duration::ZERO);
        assert!(!s.contains('?'), "fabricated a command placeholder: {s}");
        assert!(
            !s.contains("Ran "),
            "still claiming to know the command: {s}"
        );
    }

    #[test]
    fn detail_lines_show_real_stdout() {
        let f = BashFormatter;
        let p = payload("Exit code: 0\nSTDOUT:\nHELLO_FROM_UAT\n\nSTDERR:\n");
        let lines = f.detail_lines(&p, &Theme::hearth());
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            rendered.iter().any(|l| l.contains("HELLO_FROM_UAT")),
            "stdout not surfaced: {rendered:?}"
        );
    }

    #[test]
    fn detail_lines_show_stderr_on_failure() {
        let f = BashFormatter;
        let p = payload("Exit code: 2\nSTDOUT:\n\nSTDERR:\nboom: it broke\n");
        let lines = f.detail_lines(&p, &Theme::hearth());
        let rendered: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            rendered.iter().any(|l| l.contains("boom: it broke")),
            "stderr not surfaced: {rendered:?}"
        );
    }

    #[test]
    fn non_exit_code_failure_modes_show_the_tools_own_words() {
        // `Bash` has four result shapes with no exit code in them. None may be
        // coerced into a fake one.
        let f = BashFormatter;
        for raw in [
            "Missing required parameter: command",
            "Command timed out after 30000ms",
            "Bash command cancelled by cancellation token",
            "Failed to execute command: sandbox produced no exit status",
        ] {
            assert!(parse_bash_output(raw).is_none(), "must not parse: {raw}");
            let s = f.summary_line(&payload(raw), Duration::ZERO);
            assert!(
                !s.contains("exit 0"),
                "invented an exit code for {raw}: {s}"
            );
            assert!(s.starts_with(&raw[..10]), "lost the real message: {s}");
        }
    }

    #[test]
    fn empty_payload_yields_no_summary_rather_than_a_fake_completion() {
        let f = BashFormatter;
        assert_eq!(f.summary_line(&Value::Null, Duration::ZERO), "");
        assert!(f.detail_lines(&Value::Null, &Theme::hearth()).is_empty());
    }

    #[test]
    fn detail_lines_clipped_to_max() {
        let f = BashFormatter;
        let body: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let p = payload(&format!(
            "Exit code: 0\nSTDOUT:\n{}\nSTDERR:\n",
            body.join("\n")
        ));
        assert_eq!(f.detail_lines(&p, &Theme::hearth()).len(), MAX_STDOUT_LINES);
    }
}
