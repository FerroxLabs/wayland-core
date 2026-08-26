//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

/// Tavily search backend. Requires `TAVILY_API_KEY` — paid (no free
/// tier on a card-less account at v0.6 launch).
///
/// API docs: <https://docs.tavily.com/api-reference>
pub struct TavilyWebBackend {
    client: Client,
    api_key: String,
}

impl TavilyWebBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
        }
    }
}

#[async_trait]
impl WebBackend for TavilyWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let limit = limit.clamp(1, 20);
        let body = serde_json::json!({
            "api_key": self.api_key,
            "query": query,
            "max_results": limit,
            "search_depth": "basic",
        });
        let resp = match self
            .client
            .post("https://api.tavily.com/search")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("tavily request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "tavily returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(300).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("tavily response was not JSON: {e}"),
                };
            }
        };
        map_tavily_results(&parsed)
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Tavily extract not yet wired — use WebFetch on individual URLs".to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Tavily crawl not supported".to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "tavily"
    }
}
/// Map a Tavily `POST /search` 200 body into a `WebOutcome`.
///
/// Split out of `search` so the payload contract is testable without network
/// I/O, mirroring `parallel_web::map_parallel_results`.
fn map_tavily_results(parsed: &serde_json::Value) -> WebOutcome {
    let raw_results = parsed
        .get("results")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let results = super::shared::map_validated_rows(&raw_results, "content");
    if results.is_empty() {
        return WebOutcome::Err {
            message: "tavily returned no valid results".to_string(),
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

    /// gh#452 — an empty `results` array must be `Err`, not a successful empty
    /// search. `ChainedWebBackend` treats every `Ok` as final, so `Ok{web:[]}`
    /// silently disables the DuckDuckGo floor and the user sees a paid key
    /// return nothing with no error to explain it.
    #[test]
    fn empty_results_array_is_an_error_not_an_empty_success() {
        let parsed = serde_json::json!({ "results": [] });
        let msg = err_message(map_tavily_results(&parsed));
        assert!(
            msg.contains("tavily"),
            "message must name the backend: {msg}"
        );
    }

    /// The same requirement when the key is absent entirely (schema drift):
    /// `unwrap_or_default()` must not launder a missing array into a success.
    #[test]
    fn missing_results_key_is_an_error() {
        let parsed = serde_json::json!({ "query": "rust" });
        let msg = err_message(map_tavily_results(&parsed));
        assert!(
            msg.contains("tavily"),
            "message must name the backend: {msg}"
        );
    }

    /// Rows with no title or a non-http url carry no usable information; if
    /// they are all that came back, that is the empty case too.
    #[test]
    fn rows_without_title_or_http_url_do_not_count_as_results() {
        let parsed = serde_json::json!({
            "results": [
                { "title": "", "url": "https://example.com", "content": "x" },
                { "title": "ok", "url": "ftp://example.com", "content": "x" },
            ]
        });
        let msg = err_message(map_tavily_results(&parsed));
        assert!(
            msg.contains("tavily"),
            "message must name the backend: {msg}"
        );
    }

    /// Positive control: a well-formed body still maps to Ok, so the guards
    /// above cannot pass by rejecting everything.
    #[test]
    fn valid_results_are_returned() {
        let parsed = serde_json::json!({
            "results": [
                { "title": "Rust", "url": "https://rust-lang.org", "content": "snippet" },
                { "title": "Docs", "url": "http://doc.rust-lang.org", "content": "more" },
            ]
        });
        assert_eq!(web_len(map_tavily_results(&parsed)), 2);
    }
}
