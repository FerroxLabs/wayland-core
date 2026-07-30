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
use super::{join_facts, opt_str, opt_u64};
use crate::tui::theme::Theme;

/// Max lines of fetched content shown in the expanded view.
const MAX_CONTENT_LINES: usize = 25;

pub struct WebFetchFormatter;

impl ToolResultFormatter for WebFetchFormatter {
    // UAT-T3. Measured against `wcore-tools/src/web_fetch.rs`, the tool emits
    // `{url, status, content_type, text, truncated}` — there is no `bytes`
    // field and no `readability_score`, so the summary reported `0 bytes` for
    // every fetch, and `content` was never found so the body was never shown.
    // `bytes` is derived from the text the tool did return; the readability
    // score is simply not available and is therefore not printed.
    fn summary_line(&self, payload: &Value, _duration: Duration) -> String {
        let mut facts: Vec<String> = Vec::new();
        if let Some(url) = opt_str(payload, "url") {
            facts.push(format!("Fetched {}", derive_domain(url)));
        } else {
            facts.push("Fetched".to_string());
        }
        if let Some(status) = opt_u64(payload, "status") {
            facts.push(format!("HTTP {status}"));
        }
        // Prefer a byte count the tool stated; otherwise measure the body it
        // actually returned. Never print a count when there is no body.
        let bytes =
            opt_u64(payload, "bytes").or_else(|| body_text(payload).map(|t| t.len() as u64));
        if let Some(b) = bytes {
            facts.push(format!("{b} bytes"));
        }
        if let Some(s) = payload.get("readability_score").and_then(Value::as_f64) {
            facts.push(format!("readability {s:.2}"));
        }
        if payload
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            facts.push("truncated".to_string());
        }
        join_facts(&facts)
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let style = Style::default().fg(theme.text_dim);
        let Some(content) = body_text(payload) else {
            return Vec::new();
        };
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

/// The fetched body, under whichever key the payload carries.
///
/// UAT-T3: `WebFetchTool` returns the page under `text`; this formatter read
/// `content`, found nothing, and rendered an empty detail view for every
/// successful fetch. Both keys are accepted so a payload from either shape
/// renders.
fn body_text(payload: &Value) -> Option<&str> {
    payload
        .get("text")
        .or_else(|| payload.get("content"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Strip scheme + path to get a `host`-shaped string. Same approach as
/// `web::derive_domain` — kept local rather than shared so each tool
/// formatter stays self-contained.
///
/// The caller only reaches this with a non-empty `url`, so the fallback is
/// the URL itself rather than a `?` that would read as a real host.
fn derive_domain(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(url)
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
