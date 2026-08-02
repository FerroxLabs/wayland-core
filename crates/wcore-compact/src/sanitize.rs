use std::sync::LazyLock;

use regex::Regex;

// SAFETY: static literal regex; verified by the strip_ansi unit
// tests. A failure here would mean the literal was edited to an
// invalid syntax, which CI catches before release.
static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").unwrap());

pub fn strip_ansi(text: &str) -> String {
    ANSI_RE.replace_all(text, "").into_owned()
}

/// Collapse carriage-return overwrites (progress bars) to the final state of
/// each line.
///
/// A `\r` that is IMMEDIATELY followed by the `\n` this function splits on is
/// not an overwrite — it is the second half of a CRLF line terminator, which is
/// what every Windows child process emits. Keeping only the text after the last
/// `\r` therefore erased the whole line on Windows: `"ok\r\n"` collapsed to
/// `""`. Because this runs on every tool result the engine returns
/// (`wcore_compact::compact_output`, default level `Safe`), that silently
/// deleted the entire visible output of every tool on the platform — a Bash
/// command reported `Exit code: 0` with an empty `STDOUT:` while its side
/// effects had really happened, and the model then reasoned from "the command
/// produced no output". Strip the terminator first, then collapse.
pub fn collapse_cr_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        // Keyed on the split index, not on `result.is_empty()`: an empty first
        // line left `result` empty, so the separator for the SECOND line was
        // skipped too and a leading blank line vanished from the output.
        if index > 0 {
            result.push('\n');
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(last) = line.rsplit('\r').next() {
            result.push_str(last);
        }
    }
    result
}

pub fn merge_blank_lines(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_blank = false;
    // `.lines()` strips trailing `\r` so CRLF inputs collapse correctly on
    // Windows. `.trim()` / `.trim_end()` below would also strip `\r`, but
    // using `.lines()` keeps the helper consistent with the rest of the
    // module (trim_trailing_whitespace already uses it).
    for line in text.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank {
            if !prev_blank {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push('\n');
            }
            prev_blank = true;
        } else {
            if !result.is_empty() && !prev_blank {
                result.push('\n');
            } else if prev_blank && result.ends_with('\n') {
                // blank section already has trailing newline
            } else if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line.trim_end());
            prev_blank = false;
        }
    }
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub fn trim_trailing_whitespace(text: &str) -> String {
    let mut result = text
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    if text.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub fn sanitize(text: &str) -> String {
    let text = strip_ansi(text);
    let text = collapse_cr_lines(&text);
    let text = trim_trailing_whitespace(&text);
    merge_blank_lines(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_color_codes() {
        let input = "\x1b[31mError\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "Error: something failed");
    }

    #[test]
    fn strip_ansi_bold_and_nested() {
        let input = "\x1b[1m\x1b[32mCompiling\x1b[0m wcore-compact v0.1.0";
        assert_eq!(strip_ansi(input), "Compiling wcore-compact v0.1.0");
    }

    #[test]
    fn strip_ansi_no_codes_unchanged() {
        let input = "plain text without any codes";
        assert_eq!(strip_ansi(input), input);
    }

    #[test]
    fn strip_ansi_cursor_movement() {
        let input = "\x1b[2K\x1b[1G> prompt";
        assert_eq!(strip_ansi(input), "> prompt");
    }

    #[test]
    fn strip_ansi_empty_input() {
        assert_eq!(strip_ansi(""), "");
    }

    // --- collapse_cr_lines ---

    #[test]
    fn collapse_cr_overwrites() {
        let input = "Downloading... 10%\rDownloading... 50%\rDownloading... 100%\nDone.";
        assert_eq!(collapse_cr_lines(input), "Downloading... 100%\nDone.");
    }

    #[test]
    fn collapse_cr_no_cr_unchanged() {
        let input = "line1\nline2\nline3";
        assert_eq!(collapse_cr_lines(input), input);
    }

    /// The regression this function shipped: every Windows child writes CRLF,
    /// and treating the terminator's `\r` as an overwrite deleted the whole
    /// line. `"ok\r\n"` came back as `""`.
    #[test]
    fn collapse_cr_keeps_crlf_line_content() {
        let input = "first\r\nsecond\r\n";
        assert_eq!(collapse_cr_lines(input), "first\nsecond\n");
    }

    /// The overwrite semantics this function exists for must survive the fix,
    /// including when the progress line is itself CRLF-terminated.
    #[test]
    fn collapse_cr_overwrites_before_a_crlf_terminator() {
        let input = "10%\r50%\r100%\r\ndone\r\n";
        assert_eq!(collapse_cr_lines(input), "100%\ndone\n");
    }

    /// A leading blank line was dropped: the empty first line left `result`
    /// empty, so the separator for the second line was skipped as well.
    #[test]
    fn collapse_cr_keeps_a_leading_blank_line() {
        assert_eq!(collapse_cr_lines("\nfoo"), "\nfoo");
        assert_eq!(collapse_cr_lines("\r\nfoo\r\n"), "\nfoo\n");
    }

    // --- merge_blank_lines ---

    #[test]
    fn merge_consecutive_blank_lines() {
        let input = "a\n\n\n\n\nb";
        assert_eq!(merge_blank_lines(input), "a\n\nb");
    }

    #[test]
    fn merge_blank_lines_preserves_single() {
        let input = "a\n\nb\n\nc";
        assert_eq!(merge_blank_lines(input), input);
    }

    #[test]
    fn merge_blank_lines_whitespace_only_lines() {
        let input = "a\n   \n  \n\nb";
        assert_eq!(merge_blank_lines(input), "a\n\nb");
    }

    // --- trim_trailing_whitespace ---

    #[test]
    fn trim_trailing_spaces() {
        let input = "hello   \nworld\t\t\nfoo";
        assert_eq!(trim_trailing_whitespace(input), "hello\nworld\nfoo");
    }

    #[test]
    fn trim_trailing_no_trailing() {
        let input = "clean\nlines";
        assert_eq!(trim_trailing_whitespace(input), input);
    }

    // --- sanitize (combined safe layer) ---

    /// The product-shaped case: a `BashTool` result captured from a Windows
    /// child. Sanitizing it must be indistinguishable from sanitizing the same
    /// output captured on Unix — anything else means the engine hands the model
    /// a different reality depending on the host OS.
    #[test]
    fn sanitize_keeps_a_crlf_tool_result_intact() {
        let crlf = "Exit code: 0\nSTDOUT:\nproof-string\r\n\nSTDERR:\n";
        let lf = "Exit code: 0\nSTDOUT:\nproof-string\n\nSTDERR:\n";
        let sanitized = sanitize(crlf);
        assert!(
            sanitized.contains("proof-string"),
            "CRLF output was erased: {sanitized:?}"
        );
        assert_eq!(sanitized, sanitize(lf));
    }

    #[test]
    fn sanitize_applies_all() {
        let input = "\x1b[32mCompiling\x1b[0m foo   \n\n\n\nbar\rbar done\n";
        let result = sanitize(input);
        assert!(!result.contains("\x1b["));
        assert!(!result.contains("\n\n\n"));
        assert!(!result.contains("   \n"));
        assert!(result.contains("bar done"));
    }
}
