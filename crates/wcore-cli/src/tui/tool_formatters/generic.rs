//! Generic fallback formatter — pretty-prints the JSON payload.
//!
//! Used for any tool name without a dedicated formatter so the
//! dispatcher (`super::formatter_for`) is **Total**. The output is
//! intentionally plain: one-line summary derived from the first
//! string-valued top-level key, multi-line detail truncated to 30
//! lines with a `... (N more lines)` indicator if longer.

use std::time::Duration;

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::fmt_duration;
use crate::tui::theme::Theme;

/// Max number of lines the generic formatter shows in `detail_lines`
/// before truncating. Cards beyond this length get an explicit
/// `... (N more lines)` footer.
const MAX_DETAIL_LINES: usize = 30;

pub struct GenericFormatter;

impl ToolResultFormatter for GenericFormatter {
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        // Try to surface the first top-level string field (a status,
        // a result message, etc.) — most tools include exactly one.
        // Falls back to a duration-only line for non-object payloads.
        if let Some(obj) = payload.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    let trimmed = s.trim();
                    if !trimmed.is_empty() {
                        // Truncate long strings so the summary stays on one line.
                        let preview: String = trimmed.chars().take(60).collect();
                        let suffix = if trimmed.chars().count() > 60 {
                            "…"
                        } else {
                            ""
                        };
                        return format!("{k}: {preview}{suffix}");
                    }
                }
            }
        }
        format!("completed in {}", fmt_duration(duration))
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let pretty = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
        let all: Vec<&str> = pretty.lines().collect();
        let total = all.len();
        let style = Style::default().fg(theme.text_dim);

        if total <= MAX_DETAIL_LINES {
            return all
                .into_iter()
                .map(|s| Line::from(Span::styled(s.to_string(), style)))
                .collect();
        }

        let mut out: Vec<Line<'static>> = all
            .iter()
            .take(MAX_DETAIL_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect();
        // Truncation footer — uses warning (not error) since the
        // payload is not malformed, just trimmed for the compact view.
        let _ = Color::Reset; // discourage unused-import drift
        out.push(Line::from(Span::styled(
            format!("... ({} more lines)", total - MAX_DETAIL_LINES),
            Style::default().fg(theme.warning),
        )));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn generic_summary_picks_first_string_field() {
        let f = GenericFormatter;
        // Only one string-valued field so the test is independent of
        // serde_json's Map iteration order (which is alphabetical with
        // the default feature set, not insertion order). The non-string
        // `code` must be skipped.
        let payload = json!({
            "code": 0,
            "message": "done",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "message: done");
    }

    #[test]
    fn generic_summary_falls_back_to_duration_for_non_object() {
        let f = GenericFormatter;
        let payload = json!([1, 2, 3]);
        let s = f.summary_line(&payload, Duration::from_millis(1234));
        assert_eq!(s, "completed in 1.2s");
    }

    #[test]
    fn generic_summary_skips_empty_strings() {
        let f = GenericFormatter;
        let payload = json!({
            "blank": "   ",
            "real": "value",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "real: value");
    }

    #[test]
    fn generic_summary_truncates_long_string_values() {
        let f = GenericFormatter;
        let long = "x".repeat(120);
        let payload = json!({ "msg": long });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        // 60-char preview + "…" suffix.
        assert!(s.starts_with("msg: "));
        assert!(s.ends_with('…'));
        // "msg: " is 5 chars + 60 preview + "…".
        assert_eq!(s.chars().count(), 5 + 60 + 1);
    }

    #[test]
    fn generic_formatter_truncates_long_json() {
        let f = GenericFormatter;
        // Build a 100-field object — pretty-printed JSON is one line per
        // field plus opening/closing braces, so well over 30 lines.
        let mut obj = serde_json::Map::new();
        for i in 0..100 {
            obj.insert(format!("k{i}"), Value::from(i));
        }
        let payload = Value::Object(obj);
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);

        assert!(
            lines.len() <= MAX_DETAIL_LINES + 1,
            "got {} lines, expected ≤ {}",
            lines.len(),
            MAX_DETAIL_LINES + 1
        );
        // Last line is the truncation footer.
        let last = &lines[lines.len() - 1];
        let last_text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            last_text.starts_with("... (") && last_text.contains("more lines"),
            "footer was: {last_text}"
        );
    }

    #[test]
    fn generic_detail_lines_no_truncation_for_short_payload() {
        let f = GenericFormatter;
        let payload = json!({ "a": 1, "b": 2 });
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);
        // Pretty-printed `{ "a": 1, "b": 2 }` is 4 lines max — well
        // under the threshold, so no footer.
        assert!(lines.len() <= MAX_DETAIL_LINES);
        let last = &lines[lines.len() - 1];
        let last_text: String = last.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!last_text.contains("more lines"));
    }
}
