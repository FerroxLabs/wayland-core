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
        let raw_results = parsed
            .get("results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let results: Vec<serde_json::Value> = raw_results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "title": r.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    "url": r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    "snippet": r.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
            })
            .collect();
        WebOutcome::Ok {
            payload: serde_json::json!({ "web": results }),
        }
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
