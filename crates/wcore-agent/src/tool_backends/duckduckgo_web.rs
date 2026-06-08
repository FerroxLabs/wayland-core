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
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "duckduckgo returned HTTP {} (body sniff: {})",
                    status.as_u16(),
                    html.chars().take(200).collect::<String>()
                ),
            };
        }
        let results = parse_duckduckgo_html(&html, limit);
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
