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

/// The `degraded_from` note `ChainedWebBackend` stamps on a result its primary
/// did NOT serve, wherever the tool envelope put it.
///
/// gh#1068. The note was already being produced correctly and read by nobody:
/// the user saw an ordinary successful search and could not tell that the
/// backend they configured had never run, so they were diagnosed against a
/// backend that was never involved. This is the renderer that closes it - the
/// tool card is a sink the user demonstrably reads, unlike the `warn!` beside
/// it, which with `RUST_LOG` unset only ever reaches a log file.
fn degraded_note(payload: &Value) -> Option<&Value> {
    payload
        .get("degraded_from")
        .or_else(|| payload.get("data")?.get("degraded_from"))
        .filter(|v| v.is_object())
}

/// A one-time notice about how web search was configured (the keyless
/// Parallel.ai privacy disclosure, or an unrecognised `WAYLAND_WEB_BACKEND`),
/// attached to the first search by `AnnouncingWebBackend`.
fn selection_notice(payload: &Value) -> Option<&str> {
    payload
        .get("notice")
        .or_else(|| payload.get("data")?.get("notice"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn note_field<'a>(note: &'a Value, key: &str) -> Option<&'a str> {
    note.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
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
        let mut line = if duration.is_zero() {
            // The card model carries no timing yet, so `Duration::ZERO` is a
            // placeholder, not a measurement - do not render it as "0.0s".
            format!("Found {n} {unit}")
        } else {
            format!("Found {n} {unit} in {}", fmt_duration(duration))
        };
        // A result the configured backend did not serve is not an ordinary
        // success, and must not read like one.
        if let Some(note) = degraded_note(payload) {
            match (note_field(note, "backend"), note_field(note, "served_by")) {
                (Some(skipped), Some(served)) => {
                    line.push_str(&format!(" - served by {served}, {skipped} failed"));
                }
                (Some(skipped), None) => line.push_str(&format!(" - {skipped} failed")),
                _ => {}
            }
        }
        line
    }

    fn detail_lines(&self, payload: &Value, theme: &Theme) -> Vec<Line<'static>> {
        let mut out: Vec<Line<'static>> = Vec::new();
        let title_style = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
        let meta_style = Style::default().fg(theme.text_dim);
        let warn_style = Style::default().fg(theme.warning);

        // Both of these belong ABOVE the results: they change what the results
        // mean. They are rendered even when the rows are unreadable, because
        // "which backend answered, and why not the one you configured" is
        // exactly the question an empty-looking card leaves the user with.
        if let Some(text) = selection_notice(payload) {
            out.push(Line::from(Span::styled(text.to_string(), warn_style)));
        }
        if let Some(note) = degraded_note(payload) {
            let skipped = note_field(note, "backend").unwrap_or("the configured backend");
            let served = note_field(note, "served_by").unwrap_or("a fallback");
            let mut msg = format!("{skipped} did not answer; served by {served} instead");
            if let Some(reason) = note_field(note, "reason") {
                msg.push_str(&format!(" - {reason}"));
            }
            out.push(Line::from(Span::styled(msg, warn_style)));
        }

        let results = match result_rows(payload) {
            Some(r) => r,
            None => return out,
        };

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

#[cfg(test)]
mod degradation_visibility_tests {
    use super::*;
    use serde_json::json;

    fn degraded_payload() -> Value {
        json!({
            "success": true,
            "data": {
                "web": [{ "title": "A", "url": "https://a.com", "snippet": "s" }],
                "degraded_from": {
                    "backend": "exa",
                    "served_by": "duckduckgo",
                    "reason": "exa returned HTTP 401"
                }
            }
        })
    }

    /// RED ARM. gh#1068. `degraded_from` is produced correctly by
    /// `ChainedWebBackend` and consumed by nobody: the user sees an ordinary
    /// successful search and cannot learn that the backend they configured was
    /// never involved. A fallback-served result must SAY it was.
    #[test]
    fn a_fallback_served_result_says_so_on_the_card() {
        let f = WebFormatter;
        let s = f.summary_line(&degraded_payload(), Duration::ZERO);
        assert!(
            s.contains("exa") && s.contains("duckduckgo"),
            "the skipped backend and the one that actually served must both be \
             on the summary line, got: {s:?}"
        );
    }

    /// RED ARM. The reason is the half that makes it actionable — "exa was
    /// skipped" without "HTTP 401" sends the user to the wrong fix.
    #[test]
    fn the_detail_lines_carry_why_the_primary_was_skipped() {
        let f = WebFormatter;
        let theme = Theme::no_color();
        let rendered: String = f
            .detail_lines(&degraded_payload(), &theme)
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("HTTP 401"),
            "the primary's actual failure reason must reach the card: {rendered:?}"
        );
    }

    /// Control / guard against the obvious way to pass: an undegraded result
    /// must not grow a degradation line.
    #[test]
    fn an_undegraded_result_is_unchanged() {
        let f = WebFormatter;
        let p = json!({ "data": { "web": [{ "title": "A", "url": "https://a.com" }] } });
        assert_eq!(f.summary_line(&p, Duration::ZERO), "Found 1 result");
    }
}
