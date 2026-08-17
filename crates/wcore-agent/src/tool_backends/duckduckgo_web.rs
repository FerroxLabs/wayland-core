//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_tools::{
    CrawlRequest, ExtractRequest, WEB_MAX_SEARCH_LIMIT, WebBackend, WebOutcome,
};

use super::shared::urlencode;

/// Free-of-charge default `WebBackend` over DuckDuckGo's HTML-lite
/// endpoint. No API key required.
///
/// Uses the public `https://html.duckduckgo.com/html/` form-POST
/// endpoint and parses the well-known `result__a` / `result__snippet`
/// markup. Quality is roughly equivalent to a DuckDuckGo search in a
/// browser — fine for "find me three news stories about X" and the
/// like, weaker than Tavily on RAG-specific queries.
pub struct DuckDuckGoWebBackend {
    client: Client,
}

impl DuckDuckGoWebBackend {
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for DuckDuckGoWebBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WebBackend for DuckDuckGoWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let limit = limit.clamp(1, WEB_MAX_SEARCH_LIMIT) as usize;
        let body = format!("q={}", urlencode(query));
        let resp = match self
            .client
            .post("https://html.duckduckgo.com/html/")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            // DuckDuckGo blocks the literal default reqwest UA. Use a
            // plain browser-ish UA so the endpoint returns the lite
            // HTML page; staying honest by including the project
            // identifier suffix.
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (compatible; wayland-core/WebSearch; https://github.com/FerroxLabs/wayland-core)",
            )
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml",
            )
            .timeout(std::time::Duration::from_secs(15))
            .body(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("duckduckgo request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let html = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("duckduckgo body read failed: {e}"),
                };
            }
        };
        interpret_search_response(status, &html, limit)
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "web extract is not supported by the free DuckDuckGo backend. \
                      Set FIRECRAWL_API_KEY or TAVILY_API_KEY in your env (or use the \
                      `WebFetch` tool to fetch a single URL)."
                .to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "web crawl is not supported by the free DuckDuckGo backend. \
                      Set FIRECRAWL_API_KEY in your env to enable crawling."
                .to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "duckduckgo"
    }
}

/// Classify a raw DuckDuckGo response into a [`WebOutcome`].
///
/// Split out of [`DuckDuckGoWebBackend::search`] so the decision — *which*
/// of the failure modes actually happened — is testable without a network
/// round-trip. Order matters: the anti-automation challenge is checked
/// first because it arrives with a 2xx status and an empty result list, so
/// every later branch would describe it wrongly.
fn interpret_search_response(status: reqwest::StatusCode, html: &str, limit: usize) -> WebOutcome {
    if let Some(message) = describe_challenge(status, html) {
        return WebOutcome::Err { message };
    }
    if !status.is_success() {
        return WebOutcome::Err {
            message: format!(
                "duckduckgo returned HTTP {} (body sniff: {})",
                status.as_u16(),
                html.chars().take(200).collect::<String>()
            ),
        };
    }
    let results = parse_duckduckgo_html(html, limit);
    if results.is_empty() {
        return WebOutcome::Err {
            message: "duckduckgo returned no parseable results (their HTML format may have \
                      changed; try setting BRAVE_SEARCH_API_KEY for a structured API)"
                .to_string(),
        };
    }
    WebOutcome::Ok {
        payload: serde_json::json!({ "web": results }),
    }
}

/// Markup fragments that appear only in DuckDuckGo's anti-automation
/// interstitial ("Unfortunately, bots use DuckDuckGo too." plus an
/// image puzzle) — the CSS block name and the challenge script.
const CHALLENGE_MARKERS: [&str; 2] = ["anomaly-modal", "anomaly.js"];

/// Detect a rate-limit / bot challenge and describe it truthfully.
///
/// DuckDuckGo serves the challenge with **HTTP 202** — a success status —
/// and no `result__a` markup at all. Before this check existed the empty
/// parse was reported as "their HTML format may have changed", which sent
/// users hunting a parser bug that does not exist while the real cause was
/// that the free HTML endpoint had throttled their IP (#930).
fn describe_challenge(status: reqwest::StatusCode, html: &str) -> Option<String> {
    let challenged = CHALLENGE_MARKERS.iter().any(|m| html.contains(m));
    if !challenged && status.as_u16() != 429 {
        return None;
    }
    Some(format!(
        "duckduckgo refused this query as automated traffic (HTTP {}) and returned a bot \
         challenge page instead of search results — nothing was searched, and this is NOT a \
         parsing failure. The free HTML endpoint rate-limits by IP after a couple of rapid \
         queries. Wait a minute and retry, or set BRAVE_SEARCH_API_KEY / TAVILY_API_KEY (or \
         WAYLAND_WEB_BACKEND) to use a structured search API that does not throttle scrapers.",
        status.as_u16()
    ))
}

/// Parse DuckDuckGo's HTML-lite result list into `[{title,url,snippet}]`.
///
/// The lite endpoint emits a stable structure:
/// ```text
/// <a class="result__a" href="//duckduckgo.com/l/?uddg=<percent-encoded-url>&…">Title</a>
/// <a class="result__snippet" href="…">Snippet</a>
/// ```
/// The real URL is the `uddg` query parameter on the wrapper redirect.
/// Falls back to using the wrapper URL verbatim if `uddg` is missing
/// (the model can still resolve it via a follow-up `WebFetch`).
fn parse_duckduckgo_html(html: &str, limit: usize) -> Vec<serde_json::Value> {
    use regex::Regex;
    // Two relaxed-multiline regexes: one for title+url, one for snippet.
    let title_re = Regex::new(
        r#"(?s)<a[^>]*class="[^"]*\bresult__a\b[^"]*"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#,
    )
    .ok();
    let snippet_re =
        Regex::new(r#"(?s)<a[^>]*class="[^"]*\bresult__snippet\b[^"]*"[^>]*>(.*?)</a>"#).ok();
    let (Some(title_re), Some(snippet_re)) = (title_re, snippet_re) else {
        return Vec::new();
    };
    let titles: Vec<(String, String)> = title_re
        .captures_iter(html)
        .filter_map(|c| {
            let href = c.get(1)?.as_str();
            let title = c.get(2)?.as_str();
            Some((href.to_string(), strip_html_tags(title)))
        })
        .collect();
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|c| c.get(1).map(|m| strip_html_tags(m.as_str())))
        .collect();

    let n = titles.len().min(limit);
    let mut out = Vec::with_capacity(n);
    for (i, pair) in titles.into_iter().take(n).enumerate() {
        let href: String = pair.0;
        let title: String = pair.1;
        let snippet = snippets.get(i).cloned().unwrap_or_default();
        out.push(serde_json::json!({
            "title": title,
            "url": decode_ddg_url(&href),
            "snippet": snippet,
        }));
    }
    out
}

/// Decode a DuckDuckGo result wrapper URL to the real target.
///
/// DDG wraps every result link as `//duckduckgo.com/l/?uddg=<percent-encoded>&…`.
/// Returns the decoded target on success; falls back to the wrapper URL
/// with `//` prefixed to `https:` so it's at least clickable.
fn decode_ddg_url(href: &str) -> String {
    let normalized = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    if let Some(qs_start) = normalized.find('?') {
        let qs = &normalized[qs_start + 1..];
        for pair in qs.split('&') {
            if let Some(val) = pair.strip_prefix("uddg=") {
                return percent_decode(val);
            }
        }
    }
    normalized
}

/// Strip HTML tags and decode the common entities (DuckDuckGo emits
/// `<b>highlighted</b>` keyword markers and HTML-encoded ampersands).
fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

/// Percent-decode a `%XX`-encoded string (also handles `+` → space).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    /// Excerpt of a real capture from `https://html.duckduckgo.com/html/`
    /// (POST `q=rust+programming+language`, 2026-08-17, from a datacentre
    /// IP): HTTP 202, 14 340 bytes, zero `result__a` occurrences.
    const CHALLENGE_FIXTURE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><title>DuckDuckGo</title>
<script src="../anomaly.js?sv=html&cc=sre"></script></head>
<body>
  <div class="anomaly-modal__mask">
    <div class="anomaly-modal__modal  is-ie" data-testid="anomaly-modal">
      <div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
      <div class="anomaly-modal__description">Please complete the following challenge to confirm this search was made by a human.</div>
    </div>
  </div>
</body></html>"#;

    /// A results page in the shape the parser expects.
    const RESULTS_FIXTURE: &str = r#"<html><body>
<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=x">Rust <b>Programming</b> Language</a>
<a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F">A language empowering everyone.</a></div>
</body></html>"#;

    fn err_message(outcome: WebOutcome) -> String {
        match outcome {
            WebOutcome::Err { message } => message,
            WebOutcome::Ok { payload } => panic!("expected Err, got Ok({payload})"),
        }
    }

    /// The bug in #930: a throttled client is told its parser is stale.
    #[test]
    fn challenge_page_is_reported_as_rate_limit_not_format_change() {
        let msg = err_message(interpret_search_response(
            StatusCode::ACCEPTED,
            CHALLENGE_FIXTURE,
            5,
        ));
        assert!(
            !msg.contains("HTML format may have changed"),
            "rate-limit response misreported as a DuckDuckGo HTML format change: {msg}"
        );
        assert!(
            msg.contains("202"),
            "message must name the status the user can verify: {msg}"
        );
        assert!(
            msg.contains("rate-limits") && msg.contains("bot challenge"),
            "message must name the real cause: {msg}"
        );
    }

    /// An explicit 429 is the same class of failure and must read the same,
    /// even though it carries no challenge markup.
    #[test]
    fn http_429_is_reported_as_rate_limit() {
        let msg = err_message(interpret_search_response(
            StatusCode::TOO_MANY_REQUESTS,
            "",
            5,
        ));
        assert!(msg.contains("429") && msg.contains("rate-limits"), "{msg}");
    }

    /// Polarity control: the detector must NOT claim rate limiting for an
    /// ordinary page that simply had nothing to parse. Without this, a
    /// detector that fired on everything would still pass the test above.
    #[test]
    fn ordinary_empty_page_still_reports_a_parse_failure() {
        let html = "<html><body><div class=\"no-results\">No results.</div></body></html>";
        let msg = err_message(interpret_search_response(StatusCode::OK, html, 5));
        assert!(
            msg.contains("no parseable results"),
            "an unchallenged empty page must keep the parser diagnosis: {msg}"
        );
    }

    /// Non-2xx keeps its own diagnosis rather than being swept into either
    /// of the other two.
    #[test]
    fn non_success_status_reports_the_http_error() {
        let msg = err_message(interpret_search_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "<html>upstream down</html>",
            5,
        ));
        assert!(msg.contains("returned HTTP 503"), "{msg}");
    }

    /// Positive control: a page that DOES carry results still parses, so a
    /// zero above means "no results were served", not "the query failed".
    #[test]
    fn results_page_parses_into_web_payload() {
        match interpret_search_response(StatusCode::OK, RESULTS_FIXTURE, 5) {
            WebOutcome::Ok { payload } => {
                let web = payload["web"].as_array().expect("web array");
                assert_eq!(web.len(), 1);
                assert_eq!(web[0]["title"], "Rust Programming Language");
                assert_eq!(web[0]["url"], "https://www.rust-lang.org/");
                assert_eq!(web[0]["snippet"], "A language empowering everyone.");
            }
            WebOutcome::Err { message } => panic!("expected Ok, got Err({message})"),
        }
    }
}
