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
/// round-trip.
///
/// Order matters, and it is deliberately **results first**. DuckDuckGo
/// echoes the query straight back into the page (`<input name="q"
/// value="…">`), so a body marker can also show up on a perfectly good
/// results page whenever the user happened to search for that text.
/// Parsing before sniffing means a page that served results is always
/// reported as results; the challenge markers only get a say once there is
/// nothing to report.
fn interpret_search_response(status: reqwest::StatusCode, html: &str, limit: usize) -> WebOutcome {
    // An explicit 429 is authoritative on its own — no body needed.
    if status.as_u16() == 429 {
        return WebOutcome::Err {
            message: rate_limit_message(status),
        };
    }
    if status.is_success() {
        let results = parse_duckduckgo_html(html, limit);
        if !results.is_empty() {
            return WebOutcome::Ok {
                payload: serde_json::json!({ "web": results }),
            };
        }
        if is_challenge_page(html) {
            return WebOutcome::Err {
                message: rate_limit_message(status),
            };
        }
        // Genuinely nothing to parse and no challenge: name the status so
        // the reader can tell a plain 200 apart from a silent 202 instead
        // of taking the format guess on faith.
        return WebOutcome::Err {
            message: format!(
                "duckduckgo returned HTTP {} with no parseable results and no bot challenge. This \
                 backend cannot tell a genuinely empty result set apart from a change to their \
                 HTML, so it will not guess — either way nothing usable came back and the answer above \
                 it must not be built on this. {}",
                status.as_u16(),
                crate::tool_backends::shared::WEB_SEARCH_KEY_REMEDY
            ),
        };
    }
    if is_challenge_page(html) {
        return WebOutcome::Err {
            message: rate_limit_message(status),
        };
    }
    WebOutcome::Err {
        message: format!(
            "duckduckgo returned HTTP {} (body sniff: {})",
            status.as_u16(),
            html.chars().take(200).collect::<String>()
        ),
    }
}

/// Attribute-anchored markers for DuckDuckGo's anti-automation
/// interstitial ("Unfortunately, bots use DuckDuckGo too." plus a
/// select-the-ducks puzzle): the CSS block name on the modal, and the
/// challenge endpoint both of its forms submit to.
///
/// The anchoring is load-bearing. A bare `anomaly-modal` / `anomaly.js`
/// substring search also matches the query DuckDuckGo echoes into
/// `value="…"` and `<title>`, so searching for `anomaly.js` would make the
/// backend flag its own results page as a challenge. The echo is
/// HTML-escaped and so can never contain the `"` these patterns require.
const CHALLENGE_MARKER_PATTERNS: [&str; 2] = [
    r#"class="[^"]*anomaly-modal"#,
    r#"action="[^"]*anomaly\.js"#,
];

/// True when the body is DuckDuckGo's bot-challenge interstitial rather
/// than a search-results page.
fn is_challenge_page(html: &str) -> bool {
    CHALLENGE_MARKER_PATTERNS
        .iter()
        .any(|p| regex::Regex::new(p).is_ok_and(|re| re.is_match(html)))
}

/// Describe a rate-limit / bot challenge truthfully.
///
/// DuckDuckGo serves the challenge with **HTTP 202** — a success status —
/// and no `result__a` markup at all. `StatusCode::is_success` accepts 202,
/// so the empty parse used to fall through to "their HTML format may have
/// changed", which sent users hunting a parser bug that does not exist
/// while the real cause was the free HTML endpoint throttling their IP
/// (#930).
fn rate_limit_message(status: reqwest::StatusCode) -> String {
    format!(
        "duckduckgo refused this query as automated traffic (HTTP {}) and returned a bot \
         challenge page instead of search results — nothing was searched, and this is NOT a \
         parsing failure. The free HTML endpoint rate-limits by IP: measured against the live \
         endpoint it serves about two queries and then refuses for minutes (still refusing four minutes later), so retrying does \
         not clear it — and on a shared egress IP (CI, an office NAT, a VPN exit) the budget \
         may have been spent by someone else before your first query. {}",
        status.as_u16(),
        crate::tool_backends::shared::WEB_SEARCH_KEY_REMEDY
    )
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

    /// Trimmed excerpt of a real capture of `https://html.duckduckgo.com/html/`
    /// (POST `q=rust+programming+language`, 2026-08-17, from a datacentre IP):
    /// HTTP 202, 14 346 bytes, zero `result__a`, 56 `anomaly-modal`
    /// occurrences. Both markers are kept in their real attribute positions —
    /// `class=` on the modal divs and `action=` on the challenge form.
    const CHALLENGE_FIXTURE: &str = r#"<!DOCTYPE html>
<html lang="en"><head><title>DuckDuckGo</title></head>
<body>
  <form id="challenge-form" action="//duckduckgo.com/anomaly.js?sv=html&cc=sre&st=1786976247" method="POST">
    <div class="anomaly-modal__mask">
      <div class="anomaly-modal__modal  is-ie" data-testid="anomaly-modal">
        <div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
        <div class="anomaly-modal__description">Please complete the following challenge to confirm this search was made by a human.</div>
        <div class="anomaly-modal__instructions">Select all squares containing a duck:</div>
      </div>
    </div>
  </form>
</body></html>"#;

    /// Results page in the shape really served in 2026: organic hits carry a
    /// direct `href`, not the older `//duckduckgo.com/l/?uddg=` wrapper.
    const RESULTS_FIXTURE: &str = r#"<html><body>
<div class="result"><a rel="nofollow" class="result__a" href="https://rust-lang.org/">Rust <b>Programming</b> Language</a>
<a class="result__snippet" href="https://rust-lang.org/">A language empowering everyone.</a></div>
</body></html>"#;

    /// A **successful** results page for the query `anomaly.js source map`,
    /// in the real shape: DuckDuckGo echoes the query into `<title>` and into
    /// two `value="…"` inputs, so the literal text `anomaly.js` appears three
    /// times on a page that served ten real results.
    const RESULTS_ECHOING_MARKER_FIXTURE: &str = r#"<html><head>
  <title>anomaly.js source map at DuckDuckGo</title></head><body>
  <input name="q" class="search__input" type="text" value="anomaly.js source map" />
  <input type="hidden" name="q" value="anomaly.js source map" />
<div class="result"><a rel="nofollow" class="result__a" href="https://github.com/denandz/sourcemapper">GitHub - denandz/sourcemapper</a>
<a class="result__snippet" href="https://github.com/denandz/sourcemapper">Extract JavaScript source trees.</a></div>
</body></html>"#;

    fn err_message(outcome: WebOutcome) -> String {
        match outcome {
            WebOutcome::Err { message } => message,
            WebOutcome::Ok { payload } => panic!("expected Err, got Ok({payload})"),
        }
    }

    fn web_array(outcome: WebOutcome) -> Vec<serde_json::Value> {
        match outcome {
            WebOutcome::Ok { payload } => payload["web"].as_array().expect("web array").clone(),
            WebOutcome::Err { message } => panic!("expected Ok, got Err({message})"),
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

    /// The mirror-image lie: DuckDuckGo echoes the query into the page, so a
    /// bare marker substring search flags a *successful* search for
    /// "anomaly.js" as a bot challenge and throws its results away.
    #[test]
    fn results_survive_a_query_that_echoes_a_challenge_marker() {
        let web = web_array(interpret_search_response(
            StatusCode::OK,
            RESULTS_ECHOING_MARKER_FIXTURE,
            5,
        ));
        assert_eq!(
            web.len(),
            1,
            "a served results page must be reported as results even when the query text \
             collides with a challenge marker"
        );
        assert_eq!(web[0]["url"], "https://github.com/denandz/sourcemapper");
    }

    /// Same collision at the detector level: the markers must be anchored to
    /// the attributes DuckDuckGo puts them in, not matched anywhere in the
    /// body.
    #[test]
    fn challenge_detection_is_attribute_anchored() {
        assert!(
            is_challenge_page(CHALLENGE_FIXTURE),
            "real challenge page must be detected"
        );
        assert!(
            !is_challenge_page(RESULTS_ECHOING_MARKER_FIXTURE),
            "an echoed query must not be mistaken for challenge markup"
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
    /// detector that fired on everything would still pass the tests above.
    #[test]
    fn ordinary_empty_page_still_reports_a_parse_failure() {
        let html = "<html><body><div class=\"no-results\">No results.</div></body></html>";
        let msg = err_message(interpret_search_response(StatusCode::OK, html, 5));
        assert!(
            msg.contains("no parseable results"),
            "an unchallenged empty page must keep the parser diagnosis: {msg}"
        );
        assert!(
            msg.contains("200"),
            "the fallback diagnosis must still name the status it saw: {msg}"
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
        let web = web_array(interpret_search_response(
            StatusCode::OK,
            RESULTS_FIXTURE,
            5,
        ));
        assert_eq!(web.len(), 1);
        assert_eq!(web[0]["title"], "Rust Programming Language");
        assert_eq!(web[0]["url"], "https://rust-lang.org/");
        assert_eq!(web[0]["snippet"], "A language empowering everyone.");
    }

    /// Ordering invariant: if a body ever carried both real results and
    /// challenge markup, the results must win. Synthetic — DuckDuckGo has not
    /// been observed serving both at once — but it is what pins the
    /// parse-before-sniff order that keeps an echoed query harmless.
    #[test]
    fn results_win_over_challenge_markup_on_the_same_page() {
        let mixed = format!("{RESULTS_FIXTURE}{CHALLENGE_FIXTURE}");
        let web = web_array(interpret_search_response(StatusCode::OK, &mixed, 5));
        assert_eq!(
            web.len(),
            1,
            "served results must outrank challenge markup on the same page"
        );
    }

    /// The older `uddg=` wrapper is still decoded, so restoring direct-href
    /// results above did not drop wrapper support.
    #[test]
    fn uddg_wrapper_urls_are_still_decoded() {
        assert_eq!(
            decode_ddg_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&rut=x"),
            "https://www.rust-lang.org/"
        );
    }
}

#[cfg(test)]
mod dead_end_remedy_tests {
    use super::*;

    /// RED ARM. Measured 2026-08-26 against the live endpoint: after two
    /// queries from one IP the free HTML endpoint serves the challenge, and it
    /// was STILL serving it four minutes later. "Wait a minute and retry" is
    /// therefore a false remedy that costs the user a support cycle.
    #[test]
    fn rate_limit_message_does_not_advise_a_wait_that_does_not_work() {
        let m = rate_limit_message(reqwest::StatusCode::from_u16(202).unwrap());
        assert!(
            !m.to_ascii_lowercase().contains("wait a minute"),
            "the advertised wait is measurably false: {m}"
        );
    }

    /// RED ARM. The dead end has to carry a remedy the user can actually
    /// complete. Brave stopped issuing keyless free tiers in Feb 2026 (card
    /// required), so naming it alone sends the user into a payment form.
    #[test]
    fn every_dead_end_names_a_verified_no_card_remedy() {
        let msgs = [
            rate_limit_message(reqwest::StatusCode::from_u16(202).unwrap()),
            match interpret_search_response(reqwest::StatusCode::OK, "<html></html>", 5) {
                WebOutcome::Err { message } => message,
                other => panic!("an unparseable body must be an Err, got {other:?}"),
            },
        ];
        for m in msgs {
            assert!(
                m.contains("https://app.tavily.com") && m.contains("TAVILY_API_KEY"),
                "a dead end must name the concrete free option and its env var: {m}"
            );
        }
    }
}
