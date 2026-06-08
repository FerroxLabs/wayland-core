//! `web_fetch` tool formatter.
//!
//! Expected payload shape (`wcore-tool-web-fetch`):
//! ```json
//! { "url": "https://...", "bytes": 1234, "readability_score": 0.87, "content": "..." }
//! ```
//! `readability_score` may be missing on a non-text fetch — in that
//! case we omit the score from the summary.

use std::time::Duration;

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::{str_or, u64_or};
use crate::tui::theme::Theme;

/// Max lines of fetched content shown in the expanded view.
const MAX_CONTENT_LINES: usize = 25;

pub struct WebFetchFormatter;

impl ToolResultFormatter for WebFetchFormatter {
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let url = str_or(payload, "url", "?");
        let domain = derive_domain(url);
        let bytes = u64_or(payload, "bytes", 0);
        let score = payload.get("readability_score").and_then(Value::as_f64);
        match score {
            Some(s) => format!(
                "Fetched {} · {} bytes · readability {:.2}",
                domain, bytes, s
            ),
            None => format!("Fetched {} · {} bytes", domain, bytes),
        }
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let style = Style::default().fg(theme.text_dim);
        let content = payload.get("content").and_then(Value::as_str).unwrap_or("");
        if content.is_empty() {
            return Vec::new();
        }
        content
            .lines()
            .take(MAX_CONTENT_LINES)
            .map(|s| Line::from(Span::styled(s.to_string(), style)))
            .collect()
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        match payload.get("url").and_then(Value::as_str) {
            Some(u) if !u.is_empty() => vec![u.to_string()],
            _ => Vec::new(),
        }
    }
}

/// Strip scheme + path to get a `host`-shaped string. Same approach as
/// `web::derive_domain` — kept local rather than shared so each tool
/// formatter stays self-contained.
fn derive_domain(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("?")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn web_fetch_summary_with_readability_score() {
        let f = WebFetchFormatter;
        let payload = json!({
            "url": "https://news.example.com/article",
            "bytes": 42137,
            "readability_score": 0.91,
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(
            s,
            "Fetched news.example.com · 42137 bytes · readability 0.91"
        );
    }

    #[test]
    fn web_fetch_summary_without_readability_score() {
        let f = WebFetchFormatter;
        let payload = json!({
            "url": "https://files.example.com/data.bin",
            "bytes": 1024,
        });
        let s = f.summary_line(&payload, Duration::from_secs(1));
        assert_eq!(s, "Fetched files.example.com · 1024 bytes");
    }

    #[test]
    fn web_fetch_extracts_single_url() {
        let f = WebFetchFormatter;
        let payload = json!({ "url": "https://example.com/page", "bytes": 100 });
        let urls = f.extract_urls(&payload);
        assert_eq!(urls, vec!["https://example.com/page".to_string()]);
    }
}
