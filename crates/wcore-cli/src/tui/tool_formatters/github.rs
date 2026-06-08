//! `github` tool formatter.
//!
//! Expected payload shape:
//! ```json
//! { "action": "Created", "repo": "user/repo", "id": 42,
//!   "html_url": "https://github.com/user/repo/issues/42" }
//! ```
//! `action` is a verb like `Created`/`Updated`/`Merged`/`Commented on`.
//! `id` is the issue or PR number.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{i64_or, str_or};
use crate::tui::theme::Theme;

pub struct GithubFormatter;

impl ToolResultFormatter for GithubFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let action = str_or(payload, "action", "Did");
        let repo = str_or(payload, "repo", "?");
        let id = i64_or(payload, "id", 0);
        format!("{} {} #{}", action, repo, id)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let style = Style::default().fg(theme.text_dim);

        // Title (if present) makes the card readable at a glance.
        if let Some(title) = payload.get("title").and_then(Value::as_str) {
            lines.push(Line::from(Span::styled(title.to_string(), style)));
        }
        if let Some(url) = payload.get("html_url").and_then(Value::as_str) {
            lines.push(Line::from(Span::styled(url.to_string(), style)));
        }
        lines
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        match payload.get("html_url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => vec![u.to_string()],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn github_summary_format() {
        let f = GithubFormatter;
        let payload = json!({
            "action": "Created",
            "repo": "FerroxLabs/wayland-core",
            "id": 42,
            "html_url": "https://github.com/FerroxLabs/wayland-core/issues/42",
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Created FerroxLabs/wayland-core #42");
    }

    #[test]
    fn github_extracts_html_url() {
        let f = GithubFormatter;
        let payload = json!({
            "action": "Merged",
            "repo": "x/y",
            "id": 1,
            "html_url": "https://github.com/x/y/pull/1",
        });
        let urls = f.extract_urls(&payload);
        assert_eq!(urls, vec!["https://github.com/x/y/pull/1".to_string()]);
    }
}
