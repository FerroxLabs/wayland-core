//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

use super::shared::urlencode;

/// Brave Search API backend. Requires `BRAVE_SEARCH_API_KEY` —
/// Brave's free tier gives 2 000 queries / month with no card on file.
///
/// API docs: <https://api.search.brave.com/app/documentation/web-search>
pub struct BraveWebBackend {
    client: Client,
    api_key: String,
}

impl BraveWebBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
        }
    }
}

#[async_trait]
impl WebBackend for BraveWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let limit = limit.clamp(1, 20);
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencode(query),
            limit
        );
        let resp = match self
            .client
            .get(&url)
            .header("X-Subscription-Token", &self.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("brave request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "brave returned HTTP {}: {}",
                    status.as_u16(),
                    body.chars().take(300).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("brave response was not JSON: {e}"),
                };
            }
        };
        map_brave_results(&parsed)
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "extract not supported by Brave Search; set FIRECRAWL_API_KEY or use WebFetch"
                .to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "crawl not supported by Brave Search; set FIRECRAWL_API_KEY".to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "brave"
    }
}
/// Map a Brave `GET /res/v1/web/search` 200 body into a `WebOutcome`.
///
/// Split out of `search` so the payload contract is testable without network
/// I/O, mirroring `parallel_web::map_parallel_results`.
fn map_brave_results(parsed: &serde_json::Value) -> WebOutcome {
    let raw_results = parsed
        .pointer("/web/results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let results = super::shared::map_validated_rows(&raw_results, "description");
    if results.is_empty() {
        return WebOutcome::Err {
            message: "brave returned no valid results".to_string(),
        };
    }
    WebOutcome::Ok {
        payload: serde_json::json!({ "web": results }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_message(outcome: WebOutcome) -> String {
        match outcome {
            WebOutcome::Err { message } => message,
            WebOutcome::Ok { payload } => {
                panic!("expected Err, got Ok({payload})")
            }
        }
    }

    fn web_len(outcome: WebOutcome) -> usize {
        match outcome {
            WebOutcome::Ok { payload } => payload
                .get("web")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or_else(|| panic!("no web array in {payload}")),
            WebOutcome::Err { message } => panic!("expected Ok, got Err({message})"),
        }
    }

    /// gh#452 — see the matching test in `tavily_web`. An empty result set must
    /// be `Err` so `ChainedWebBackend` falls through to DuckDuckGo instead of
    /// serving a successful empty search.
    #[test]
    fn empty_results_array_is_an_error_not_an_empty_success() {
        let parsed = serde_json::json!({ "web": { "results": [] } });
        let msg = err_message(map_brave_results(&parsed));
        assert!(
            msg.contains("brave"),
            "message must name the backend: {msg}"
        );
    }

    /// Brave omits `web` entirely when a query matches nothing, so the missing
    /// pointer is the common case, not an exotic one.
    #[test]
    fn missing_web_pointer_is_an_error() {
        let parsed = serde_json::json!({ "query": { "original": "rust" } });
        let msg = err_message(map_brave_results(&parsed));
        assert!(
            msg.contains("brave"),
            "message must name the backend: {msg}"
        );
    }

    #[test]
    fn rows_without_title_or_http_url_do_not_count_as_results() {
        let parsed = serde_json::json!({
            "web": { "results": [
                { "title": "", "url": "https://example.com", "description": "x" },
                { "title": "ok", "url": "ftp://example.com", "description": "x" },
            ] }
        });
        let msg = err_message(map_brave_results(&parsed));
        assert!(
            msg.contains("brave"),
            "message must name the backend: {msg}"
        );
    }

    /// Positive control.
    #[test]
    fn valid_results_are_returned() {
        let parsed = serde_json::json!({
            "web": { "results": [
                { "title": "Rust", "url": "https://rust-lang.org", "description": "snippet" },
                { "title": "Docs", "url": "http://doc.rust-lang.org", "description": "more" },
            ] }
        });
        assert_eq!(web_len(map_brave_results(&parsed)), 2);
    }
}
