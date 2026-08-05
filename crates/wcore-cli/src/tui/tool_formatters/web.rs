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
use super::{fmt_duration, join_facts};
use crate::tui::theme::Theme;

/// Max URLs returned by `extract_urls` — feeds the Sources block which
/// runs out of vertical space past about ten entries.
const MAX_URLS: usize = 10;

/// Max snippet preview length (chars) shown in `detail_lines`.
const SNIPPET_PREVIEW: usize = 80;

pub struct WebFormatter;

/// The array of result rows, wherever `WebTool` actually put it.
///
/// UAT-T3: measured in `wcore-tools/src/web_tools.rs`, the three operations do
/// NOT agree on a key, and the SEARCH arm — by far the most used — is the one
/// this formatter never matched:
///
/// * `search`  → `{"success": true, "data": {"web": [...]}}`   (`dispatch_search`)
/// * `extract` → `{"success": true, "results": [...]}`         (`dispatch_extract`)
/// * `crawl`   → `{"success": true, "results": [...]}`
///
/// Reading only top-level `results` meant every successful web search
/// rendered `Found 0 results`. Check every shape the tool can emit.
fn result_rows(payload: &Value) -> Option<&Vec<Value>> {
    if let Some(a) = payload.get("results").and_then(Value::as_array) {
        return Some(a);
    }
    let data = payload.get("data")?;
    data.get("web")
        .or_else(|| data.get("results"))
        .and_then(Value::as_array)
}

impl ToolResultFormatter for WebFormatter {
    fn summary_line(&self, payload: &Value, duration: Duration) -> String {
        // No rows found is NOT the same as zero rows returned. The former
        // means this formatter could not read the payload, and saying
        // "Found 0 results" about it is a fabrication.
        let Some(rows) = result_rows(payload) else {
            return String::new();
        };
        let n = rows.len();
        let unit = if n == 1 { "result" } else { "results" };
        if duration.is_zero() {
            // The card model carries no timing yet, so `Duration::ZERO` is a
            // placeholder, not a measurement — do not render it as "0.0s".
            format!("Found {n} {unit}")
        } else {
            format!("Found {n} {unit} in {}", fmt_duration(duration))
        }
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let results = match result_rows(payload) {
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
            // `content` is the key the extract/crawl rows actually use
            // (`web_tools.rs::rejected_to_rows` and the backend payloads);
            // `snippet` is the search-row key. Try both before giving up.
            let snippet: String = r
                .get("snippet")
                .or_else(|| r.get("content"))
                .or_else(|| r.get("error"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(SNIPPET_PREVIEW)
                .collect();
            out.push(Line::from(Span::styled(title, title_style)));
            // Only render the meta line for parts that exist — an unknown
            // domain must not print as `?`.
            let meta = join_facts(&[domain.unwrap_or_default(), snippet]);
            if !meta.is_empty() {
                out.push(Line::from(Span::styled(format!("  {meta}"), meta_style)));
            }
        }
        out
    }

    fn extract_urls(&self, payload: &Value) -> Vec<String> {
        // Same key-shape bug as `summary_line`: the Sources block was empty
        // for every web SEARCH because the rows live under `data.web`.
        result_rows(payload)
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
///
/// Returns `None` when neither is present. It used to return the literal
/// `"?"`, which the meta line then rendered as though `?` were the site the
/// result came from (UAT-T3: unknown must read as unknown, and the cheapest
/// way to guarantee that is to have no string to print).
fn derive_domain(result: &Value) -> Option<String> {
    if let Some(d) = result
        .get("domain")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(d.to_string());
    }
    let u = result
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    // Cheap split — avoids a `url` crate dep for what is a UI hint.
    // `https://example.com/foo?x=1` → `example.com`.
    let after_scheme = u.split_once("://").map(|(_, rest)| rest).unwrap_or(u);
    after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
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

    /// UAT-T3. This test previously asserted that an UNREADABLE payload
    /// renders as `Found 0 results in 0.5s` — i.e. it pinned the fabrication
    /// in place as the specified behaviour. It is not weakened here, it is
    /// inverted: "I could not read the payload" and "the search returned
    /// nothing" are different facts and must not render identically.
    #[test]
    fn web_summary_is_empty_when_the_payload_cannot_be_read() {
        let f = WebFormatter;
        let s = f.summary_line(&json!({}), Duration::from_millis(500));
        assert_eq!(s, "", "must not claim 0 results for an unreadable payload");
    }

    /// A genuinely empty result set still reports zero — that IS a fact.
    #[test]
    fn web_summary_reports_a_real_empty_result_set() {
        let f = WebFormatter;
        let s = f.summary_line(&json!({ "results": [] }), Duration::from_millis(500));
        assert_eq!(s, "Found 0 results in 0.5s");
    }

    /// The SEARCH arm's real payload shape, which this formatter never
    /// matched: rows live at `data.web`, not at top-level `results`.
    #[test]
    fn web_summary_reads_the_real_search_payload_shape() {
        let f = WebFormatter;
        let payload = json!({
            "success": true,
            "data": { "web": [
                { "title": "A", "url": "https://a.com" },
                { "title": "B", "url": "https://b.com" },
            ]}
        });
        let s = f.summary_line(&payload, Duration::from_secs_f64(1.0));
        assert_eq!(s, "Found 2 results in 1.0s");
        assert_eq!(f.extract_urls(&payload).len(), 2, "Sources block was empty");
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
