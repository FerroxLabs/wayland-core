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
        interpret_response(status.as_u16(), &html, limit)
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

/// Classify one DuckDuckGo response body into a `WebOutcome`.
///
/// Split out of `search` so the whole post-transport decision — challenge
/// detection, HTTP status handling, parsing, and the empty-result verdict —
/// is one testable unit rather than three untestable branches behind a
/// network call.
fn interpret_response(status: u16, html: &str, limit: usize) -> WebOutcome {
    if is_challenge_response(status, html) {
        return WebOutcome::Err {
            message: format!(
                "duckduckgo blocked this search with its anti-bot challenge page (HTTP {status}) \
                 — a rate limit on this IP, not an HTML-format change. The block clears on \
                 DuckDuckGo's schedule, not ours; for a search path that does not depend on this \
                 IP's reputation, configure a keyed provider (BRAVE_SEARCH_API_KEY / \
                 TAVILY_API_KEY / EXA_API_KEY / SEARXNG_URL / FIRECRAWL_API_KEY)."
            ),
        };
    }
    if !(200..300).contains(&status) {
        return WebOutcome::Err {
            message: format!(
                "duckduckgo returned HTTP {} (body sniff: {})",
                status,
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

/// Is this response DuckDuckGo's anti-bot CAPTCHA ("anomaly") page rather
/// than a result page?
///
/// DuckDuckGo serves the challenge with HTTP **202** — a 2xx status, so
/// `StatusCode::is_success()` accepts it, the parser finds no `result__a`
/// nodes, and the backend then blames its own selectors. That is the
/// misdiagnosis reported as issue #930: the markup had NOT changed, the IP
/// had been rate-limited (measured: two searches succeed, the third onward
/// return the 202 challenge until the block ages out).
///
/// This is a DENYLIST — markers observed on the live challenge page (the
/// `anomaly.js` form action, the `anomaly-modal` classes and the modal
/// title), plus the 202 status itself, since a served result page is always
/// `200`. Anything unmatched falls through to normal parsing, so a genuine
/// future format change still reports as a format change.
fn is_challenge_response(status: u16, html: &str) -> bool {
    if status == 202 {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    ["anomaly.js", "anomaly-modal", "bots use duckduckgo"]
        .iter()
        .any(|marker| lower.contains(marker))
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

    /// Verbatim excerpt of the page DuckDuckGo actually served on
    /// 2026-08-17 for the third of three rapid searches from one IP
    /// (HTTP **202**, 14 KB, zero `result__a` nodes). This is the body
    /// behind issue #930.
    const CHALLENGE_PAGE: &str = r#"<form id="img-form" action="//duckduckgo.com/anomaly.js?sv=html&cc=botnet&ti=1786971272&gk=d4cd0dabcf4caa22ad92fab40844c786&p=4f30e5fa6351d8d5d39c0a2fe1cfb8f2-afb559c3e57558791e2ccde213e98dbb-028954ea058882eb2225f5c42b46cfbe-c978ee9be4079b0071d383c2d75318e7-db8eb7a2f56c7477510da2e99ed68fce-9631df0edfabaffa569d4deb18a17da2-377ca22cef8125255602cdf7140cb5b8-3a6630c654b55d766a6ce3ab6149e994-afa56c1500702c055b003c48690ed93a&q=test query number 3 weather&o=NxQW9rwzA5t%2Fwiia6FaycBfPMkFyLX%2BhTVpuwR5CorQ0XxxGnFENbtAk3325FT1R%0A&r=ase" target="ifr" method="POST"></form>
        <form id="challenge-form" action="//duckduckgo.com/anomaly.js?sv=html&cc=botnet&st=1786971272&gk=d4cd0dabcf4caa22ad92fab40844c786&p=4f30e5fa6351d8d5d39c0a2fe1cfb8f2-afb559c3e57558791e2ccde213e98dbb-028954ea058882eb2225f5c42b46cfbe-c978ee9be4079b0071d383c2d75318e7-db8eb7a2f56c7477510da2e99ed68fce-9631df0edfabaffa569d4deb18a17da2-377ca22cef8125255602cdf7140cb5b8-3a6630c654b55d766a6ce3ab6149e994-afa56c1500702c055b003c48690ed93a&q=test query number 3 weather&o=NxQW9rwzA5t%2Fwiia6FaycBfPMkFyLX%2BhTVpuwR5CorQ0XxxGnFENbtAk3325FT1R%0A&r=ase" method="POST">
            <div class="anomaly-modal__mask">
                <div class="anomaly-modal__modal  is-ie" data-testid="anomaly-modal">
                    <div class="anomaly-modal__title">Unfortunately, bots use DuckDuckGo too.</div>
                    <div class="anomaly-modal__description">Please complete the following challenge to confirm this search was made by a human.</div>
                    <div class="anomaly-modal__instructions">Select all squares containing a duck:</div>
                    <div class="anomaly-modal__puzzle-margins">
                        <div class="anomaly-modal__puzzle">
   "#;

    /// Verbatim excerpt of a real HTTP 200 result page captured the same
    /// day — proof that the `result__a` / `result__snippet` markup the
    /// parser targets had NOT changed.
    const RESULT_PAGE: &str = r#"<div class="result results_links results_links_deep web-result ">
                  <div class="links_main links_deep result__body"> <!-- This is the visible part -->
                    
                      <h2 class="result__title">
                        <a rel="nofollow" class="result__a" href="https://sports.yahoo.com/articles/india-vs-sri-lanka-1st-041755980.html">India vs Sri Lanka 1st Test Day 3 Live Updates: Score, Weather, Win ...</a>
                      </h2>

                    

                    
                      <div class="result__extras">
                        <div class="result__extras__url">
                          <span class="result__icon">
                            <a rel="nofollow" href="https://sports.yahoo.com/articles/india-vs-sri-lanka-1st-041755980.html">
                              <img class="result__icon__img" width="16" height="16" alt="" src="//external-content.duckduckgo.com/ip3/sports.yahoo.com.ico" name="i15" />
                            </a>
                          </span>
                          <a class="result__url" href="https://sports.yahoo.com/articles/india-vs-sri-lanka-1st-041755980.html">
                            sports.yahoo.com/articles/india-vs-sri-lanka-1st-041755980.html
                          </a>
                          
                            <span>&nbsp; &nbsp; 2026-08-17T04:17:55.0000000</span>
                          
                        </div>
                      </div>
                    

                    
                      
                        <a class="result__snippet" href="https://sports.yahoo.com/articles/india-vs-sri-lanka-1st-041755980.html">Will rain disrupt the 1st <b>Test</b> in Galle? Check the complete 5-day <b>weather</b> forecast, hourly rain chances, and ground conditions for India vs Sri Lanka. Track Sri Lanka vs India 1st <b>Test</b> live win ...</a>
                      
                    

                    <div class="clear"></div>
                  </div>
                </div>
              
            
              
                <div class="result results_links results_links_deep web-result ">
                  <div class="links_main links_deep result__body"> <!-- This is the visible part -->
                    
                      <h2 class="result__title">
                        <a rel="nofollow" class="result__a" href="https://www.piwheels.org/project/weather-query-test/">piwheels - weather-query-test</a>
                      </h2>

                    

                    
                      <div class="result__extras">
                        <div class="result__extras__url">
                          <span class="result__icon">
                            <a rel="nofollow" href="https://www.piwheels.org/project/weather-query-test/">
                              <img class="result__icon__img" width="16" height="16" alt="" src="//external-content.duckduckgo.com/ip3/www.piwheels.org.ico" name="i15" />
                            </a>
                          </span>
                          <a class="result__url" href="https://www.piwheels.org/project/weather-query-test/">
                            www.piwheels.org/project/weather-query-test/
                          </a>
                          
                            <span>&nbsp; &nbsp; 2024-11-04T00:00:00.0000000</span>
                          
                        </div>
                      </div>
                    

                    
                      
                        <a class="result__snippet" href="https://www.piwheels.org/project/weather-query-test/">The piwheels project page for <b>weather</b>-<b>query</b>-<b>test</b>: forecast <b>weather</b></a>
                      
                    

                    <div class="clear"></div>
                  </div>
                </div>
              
            
              
                "#;

    fn err_message(outcome: WebOutcome) -> String {
        match outcome {
            WebOutcome::Err { message } => message,
            WebOutcome::Ok { payload } => panic!("expected Err, got Ok({payload})"),
        }
    }

    /// #930 symptom: a rate-limited search was reported to the user as
    /// "duckduckgo returned no parseable results (their HTML format may
    /// have changed)", which sent the reporter looking for a scraping
    /// rewrite when the real cause was an IP-level block.
    #[test]
    fn challenge_page_is_not_reported_as_a_format_change() {
        let msg = err_message(interpret_response(202, CHALLENGE_PAGE, 5));
        assert!(
            !msg.contains("format may have changed"),
            "a bot-challenge page must not be blamed on our selectors: {msg}"
        );
        assert!(
            msg.contains("challenge") && msg.contains("rate limit"),
            "the block must be named as a rate limit: {msg}"
        );
    }

    /// The body markers must stand on their own: if DuckDuckGo ever serves
    /// the same challenge with a plain 200, it is still a block.
    #[test]
    fn challenge_body_is_detected_even_when_served_with_http_200() {
        assert!(is_challenge_response(200, CHALLENGE_PAGE));
        let msg = err_message(interpret_response(200, CHALLENGE_PAGE, 5));
        assert!(
            msg.contains("challenge"),
            "challenge markup must be classified regardless of status: {msg}"
        );
    }

    /// The 202 leg must stand on its own too: a served result page is
    /// always 200, so any other 2xx is a block even with no known marker.
    #[test]
    fn http_202_without_known_markers_is_still_a_block() {
        assert!(is_challenge_response(
            202,
            "<html><body>nothing here</body></html>"
        ));
        let msg = err_message(interpret_response(
            202,
            "<html><body>nothing here</body></html>",
            5,
        ));
        assert!(
            msg.contains("challenge") && msg.contains("202"),
            "202 must be reported as a block, with its status: {msg}"
        );
    }

    /// Positive control for the denylist: real, current result markup must
    /// NOT trip challenge detection, and must still parse. Without this the
    /// three assertions above could be satisfied by blocking everything.
    #[test]
    fn live_result_page_is_not_a_challenge_and_still_parses() {
        assert!(
            !is_challenge_response(200, RESULT_PAGE),
            "a real result page must not be classified as a challenge"
        );
        let payload = match interpret_response(200, RESULT_PAGE, 5) {
            WebOutcome::Ok { payload } => payload,
            WebOutcome::Err { message } => panic!("expected Ok, got Err({message})"),
        };
        let web = payload["web"].as_array().expect("web array").clone();
        assert_eq!(web.len(), 2, "both results must survive parsing");
        assert_eq!(
            web[1]["url"].as_str().unwrap(),
            "https://www.piwheels.org/project/weather-query-test/"
        );
        assert_eq!(
            web[1]["title"].as_str().unwrap(),
            "piwheels - weather-query-test"
        );
        assert!(
            web[0]["snippet"]
                .as_str()
                .unwrap()
                .contains("Will rain disrupt"),
            "snippets must stay paired with their titles"
        );
    }

    /// The remediation must not advise a setting that changes nothing for
    /// the user most likely to see this message. `build_web_search_backend`
    /// constructs `chain(ParallelWebBackend::free())` for BOTH the keyless
    /// default and `WAYLAND_WEB_BACKEND=parallel`, so telling a default-config
    /// user to "set WAYLAND_WEB_BACKEND=parallel" re-selects the identical
    /// chain that just fell through to DuckDuckGo. Nor may it promise a
    /// recovery time nobody has measured.
    #[test]
    fn the_block_message_advises_no_no_op_remedy() {
        let msg = err_message(interpret_response(202, CHALLENGE_PAGE, 5));
        assert!(
            !msg.contains("WAYLAND_WEB_BACKEND=parallel"),
            "advising =parallel is a no-op in the default keyless config: {msg}"
        );
        assert!(
            !msg.contains("few minutes"),
            "the recovery time is DuckDuckGo's and has never been measured: {msg}"
        );
        assert!(
            msg.contains("BRAVE_SEARCH_API_KEY"),
            "a remedy that does work must still be offered: {msg}"
        );
    }

    /// The format-change verdict must survive for the case it was written
    /// for — a 200 result page whose markup we genuinely cannot read.
    #[test]
    fn unknown_markup_on_a_clean_200_still_reports_a_format_change() {
        let msg = err_message(interpret_response(
            200,
            "<html><body><p>hi</p></body></html>",
            5,
        ));
        assert!(
            msg.contains("format may have changed"),
            "an unrecognised 200 page is still a format-change report: {msg}"
        );
    }
}
