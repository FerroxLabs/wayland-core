//! `bash` (shell exec) tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "cmd": "ls -la", "exit_code": 0, "stdout": "...", "stderr": "..." }
//! ```
//! The summary preview clips `cmd` to 30 chars so it fits on a single
//! tool-card line.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{i64_or, str_or};
use crate::tui::theme::Theme;

/// Max chars of the command shown in the summary.
const CMD_PREVIEW: usize = 30;

/// Max lines of stdout shown in the detail view.
const MAX_STDOUT_LINES: usize = 20;

pub struct BashFormatter;

impl ToolResultFormatter for BashFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let cmd = str_or(payload, "cmd", "?");
        let preview: String = cmd.chars().take(CMD_PREVIEW).collect();
        let exit = i64_or(payload, "exit_code", 0);
        let stdout_bytes = payload
            .get("stdout")
            .and_then(Value::as_str)
            .map(|s| s.len())
            .unwrap_or(0);
        format!("Ran `{}` · exit {} · {} bytes", preview, exit, stdout_bytes)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let stdout = str_or(payload, "stdout", "");
        let style = Style::default().fg(theme.text_dim);
        stdout
            .lines()
            .take(MAX_STDOUT_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bash_summary_format() {
        let f = BashFormatter;
        let payload = json!({
            "cmd": "ls -la /tmp",
            "exit_code": 0,
            "stdout": "total 4\nfile1\nfile2",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        // 19 bytes in stdout ("total 4\nfile1\nfile2").
        assert_eq!(s, "Ran `ls -la /tmp` · exit 0 · 19 bytes");
    }

    #[test]
    fn bash_summary_truncates_long_cmd() {
        let f = BashFormatter;
        let payload = json!({
            "cmd": "echo this-is-a-very-long-command-that-will-be-clipped",
            "exit_code": 1,
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        // 30 chars + the wrapping pieces.
        assert!(s.starts_with("Ran `"));
        assert!(s.contains("· exit 1 · 0 bytes"));
        // The portion between the backticks is exactly CMD_PREVIEW chars.
        let inner = s.trim_start_matches("Ran `").split('`').next().unwrap();
        assert_eq!(inner.chars().count(), CMD_PREVIEW);
    }

    #[test]
    fn bash_detail_lines_clipped_to_max() {
        let f = BashFormatter;
        let stdout_lines: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let payload = json!({
            "cmd": "yes",
            "exit_code": 0,
            "stdout": stdout_lines.join("\n"),
        });
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);
        assert_eq!(lines.len(), MAX_STDOUT_LINES);
    }
}
