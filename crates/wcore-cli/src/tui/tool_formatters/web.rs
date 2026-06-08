//! `web` (search) tool formatter.
//!
//! Expected payload shape (from `wcore-tool-web` `WebSearchTool`):
//! ```json
//! { "results": [
//!     { "title": "...", "url": "...", "domain": "...", "snippet": "..." },
//!     ...
//! ] }
//! ```
//! `domain` may be absent (older payloads emit only `url`); when it
//! is, we derive a coarse domain from the URL host segment so the
//! detail lines still read cleanly.

use std::time::Duration;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::ToolResultFormatter;
use super::fmt_duration;
use crate::tui::theme::Theme;

/// Max URLs returned by `extract_urls` — feeds the Sources block which
/// runs out of vertical space past about ten entries.
const MAX_URLS: usize = 10;

/// Max snippet preview length (chars) shown in `detail_lines`.
const SNIPPET_PREVIEW: usize = 80;

pub struct WebFormatter;

impl ToolResultFormatter for WebFormatter {
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        let n = payload
            .get("results")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        format!("Found {} results in {}", n, fmt_duration(duration))
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let results = match payload.get("results").and_then(Value::as_array) {
            Some(r) => r,
            None => return out,
        };
        let title_style = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
        let meta_style = Style::default().fg(theme.text_dim);

        for r in results {
            let title = r
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("(untitled)")
                .to_string();
            let domain = derive_domain(r);
            let snippet: String = r
                .get("snippet")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(SNIPPET_PREVIEW)
                .collect();
            out.push(Line::from(Span::styled(title, title_style)));
            out.push(Line::from(Span::styled(
                format!("  {} · {}", domain, snippet),
                meta_style,
            )));
        }
        out
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        payload
            .get("results")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| r.get("url").and_then(Value::as_str).map(str::to_string))
                    .take(MAX_URLS)
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Derive a host string from either an explicit `domain` field (the
/// happy path) or from the `url`'s host segment (older payloads).
/// Falls back to `"?"` if neither is present.
fn derive_domain(result: &Value) -> String {
    if let Some(d) = result.get("domain").and_then(Value::as_str) {
        return d.to_string();
    }
    if let Some(u) = result.get("url").and_then(Value::as_str) {
        // Cheap split — avoids a `url` crate dep for what is a UI hint.
        // `https://example.com/foo?x=1` → `example.com`.
        let after_scheme = u.split_once("://").map(|(_, rest)| rest).unwrap_or(u);
        return after_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("?")
            .to_string();
    }
    "?".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn web_summary_counts_results() {
        let f = WebFormatter;
        let payload = json!({
            "results": [
                { "title": "A", "url": "https://a.com" },
                { "title": "B", "url": "https://b.com" },
                { "title": "C", "url": "https://c.com" },
            ]
        });
        let s = f.summary_line(&payload, Duration::from_secs_f64(2.3));
        assert_eq!(s, "Found 3 results in 2.3s");
    }

    #[test]
    fn web_summary_zero_results_on_missing_field() {
        let f = WebFormatter;
        let payload = json!({});
        let s = f.summary_line(&payload, Duration::from_millis(500));
        assert_eq!(s, "Found 0 results in 0.5s");
    }

    #[test]
    fn web_extract_urls_caps_and_filters() {
        let f = WebFormatter;
        let mut results = Vec::new();
        for i in 0..20 {
            results.push(json!({ "title": format!("R{i}"), "url": format!("https://r{i}.com") }));
        }
        let payload = json!({ "results": results });
        let urls = f.extract_urls(&payload);
        assert_eq!(urls.len(), MAX_URLS);
        assert_eq!(urls[0], "https://r0.com");
    }

    #[test]
    fn web_detail_lines_have_title_and_meta_per_result() {
        let f = WebFormatter;
        let payload = json!({
            "results": [
                { "title": "Example", "url": "https://example.com/page", "snippet": "Hello world" },
            ]
        });
        let theme = Theme::hearth();
        let lines = f.detail_lines(&payload, &theme);
        // Title + meta = 2 lines per result.
        assert_eq!(lines.len(), 2);
        let title_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(title_text, "Example");
        let meta_text: String = lines[1].spans.iter().map(|s| s.content.as_ref()).collect();
        // Derived domain from URL, then snippet preview.
        assert!(meta_text.contains("example.com"));
        assert!(meta_text.contains("Hello world"));
    }
}
