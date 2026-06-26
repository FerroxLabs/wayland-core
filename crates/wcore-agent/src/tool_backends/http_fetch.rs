//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_fetch::{
    FetchBackend, FetchOutcome, FetchRequest, WEB_FETCH_MAX_RESPONSE_BYTES,
};

// ---------------------------------------------------------------------
// WebFetch — simple HTTP GET → readable text.
// ---------------------------------------------------------------------

/// Real `FetchBackend` over `reqwest`. Powers the `WebFetch` tool.
///
/// Built once per session via [`build_fetch_backend`] and registered in
/// `bootstrap.rs`. The reqwest client uses the non-streaming tool HTTP
/// policy (AUDIT B-5) and follows up to 10 redirects (matches what
/// `curl -L` and most browser-class HTTP libraries do for a normal GET).
///
/// The per-request `timeout_ms` from [`FetchRequest`] is applied per
/// call via the request builder's `.timeout(...)`, so a hung server
/// fails at the request layer rather than the dispatcher tier.
///
/// HTML responses are run through `wcore_browser::readability::extract`
/// when the caller passed `readable: true` (the default). Non-HTML
/// content types are returned verbatim (so a model fetching a JSON API
/// gets the JSON, not a mangled extraction).
pub struct HttpFetchBackend {
    client: Client,
}

impl HttpFetchBackend {
    pub fn new() -> Self {
        Self {
            // F-019 / #279: SSRF-resistant redirect policy instead of the
            // default 10-hop follow-all policy. Each redirect target is
            // re-validated via `is_safe_url` before following. WebFetch
            // and the github_api / linear / notion / gitlab backends all
            // share the same `build_ssrf_safe_tool_client` constructor so
            // the policy is one edit, not five.
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpFetchBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl FetchBackend for HttpFetchBackend {
    async fn fetch(&self, req: &FetchRequest) -> FetchOutcome {
        // Wall-clock cap on the WHOLE fetch operation. The previous code
        // only set `.timeout()` on the HTTP request, leaving body decode
        // + readability extraction unbounded. A 2 MB JS-heavy page
        // (e.g. news.google.com/search) returns the body in <2s but the
        // readability parser can pin a CPU for minutes — visible to the
        // user as a "running" spinner with no progress. The outer
        // `tokio::time::timeout` forces the whole future to bail with a
        // clear error if anything in the pipeline takes too long.
        let per_call_timeout = std::time::Duration::from_millis(u64::from(req.timeout_ms));
        let inner = self.fetch_inner(req, per_call_timeout);
        match tokio::time::timeout(per_call_timeout, inner).await {
            Ok(outcome) => outcome,
            Err(_) => FetchOutcome::Err {
                message: format!(
                    "fetch timed out: exceeded wall-clock deadline of {}ms (HTTP, body \
                     decode, and readability extraction combined)",
                    req.timeout_ms
                ),
            },
        }
    }
}

impl HttpFetchBackend {
    /// Inner fetch — runs HTTP, body decode, readability, truncate.
    /// Wrapped in a `tokio::time::timeout` by the trait impl above so the
    /// total operation always returns within the caller's deadline.
    async fn fetch_inner(
        &self,
        req: &FetchRequest,
        per_call_timeout: std::time::Duration,
    ) -> FetchOutcome {
        let response = match self
            .client
            .get(&req.url)
            .timeout(per_call_timeout)
            // Identify ourselves so origin servers don't 403 us as a
            // suspicious empty-UA bot. Plain enough to be honest, not
            // pretending to be a browser.
            .header(
                reqwest::header::USER_AGENT,
                "wayland-core/WebFetch (https://github.com/FerroxLabs/wayland-core)",
            )
            .header(
                reqwest::header::ACCEPT,
                "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.5",
            )
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // Map reqwest's typed errors to user-actionable strings.
                let msg = if e.is_timeout() {
                    format!("request timed out after {}ms", req.timeout_ms)
                } else if e.is_connect() {
                    format!("could not connect to host: {e}")
                } else if e.is_redirect() {
                    format!("too many redirects: {e}")
                } else {
                    format!("transport error: {e}")
                };
                return FetchOutcome::Err { message: msg };
            }
        };

        let status = response.status();
        let final_url = response.url().to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        // Bounded body read. We use `.text()` which honors the content
        // encoding header; if the body is larger than our cap we still
        // get the full thing (reqwest doesn't expose a streaming-with-
        // limit helper without dragging the futures crate into the
        // backend). Truncation happens below.
        let raw_text = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                return FetchOutcome::Err {
                    message: format!("could not read response body: {e}"),
                };
            }
        };

        // For HTML, optionally run readability extraction.
        //
        // Cap the input to the readability parser at the output cap so a
        // 2+ MB JS-heavy SPA (news.google.com is one) can't pin a CPU in
        // the parser. The parser walks the full DOM; on huge minified
        // HTML this is genuinely slow even when not pathological.
        // Truncating beforehand bounds parse time at the cost of dropping
        // late content, which the article-readability heuristic rarely
        // needs anyway (the meaningful text is near the top of the doc).
        let looks_like_html = content_type.to_ascii_lowercase().starts_with("text/html");
        let body = if req.readable && looks_like_html {
            let capped = if raw_text.len() > WEB_FETCH_MAX_RESPONSE_BYTES {
                let mut end = WEB_FETCH_MAX_RESPONSE_BYTES;
                while end > 0 && !raw_text.is_char_boundary(end) {
                    end -= 1;
                }
                &raw_text[..end]
            } else {
                raw_text.as_str()
            };
            wcore_browser::readability::extract(capped, wcore_browser::op::ReadMode::Article)
        } else {
            raw_text
        };

        let (text, truncated) = if body.len() > WEB_FETCH_MAX_RESPONSE_BYTES {
            // Snap to char boundary so we don't slice a multi-byte rune.
            let mut end = WEB_FETCH_MAX_RESPONSE_BYTES;
            while end > 0 && !body.is_char_boundary(end) {
                end -= 1;
            }
            (body[..end].to_string(), true)
        } else {
            (body, false)
        };

        if status.is_success() {
            FetchOutcome::Ok {
                status: status.as_u16(),
                content_type,
                text,
                truncated,
                final_url,
            }
        } else {
            FetchOutcome::HttpError {
                status: status.as_u16(),
                message: format!(
                    "HTTP {} — {}",
                    status.as_u16(),
                    text.chars().take(500).collect::<String>()
                ),
            }
        }
    }
}
