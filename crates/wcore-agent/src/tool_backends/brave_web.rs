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
        let raw_results = parsed
            .pointer("/web/results")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let results: Vec<serde_json::Value> = raw_results
            .into_iter()
            .map(|r| {
                serde_json::json!({
                    "title": r.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    "url": r.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    "snippet": r.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                })
            })
            .collect();
        WebOutcome::Ok {
            payload: serde_json::json!({ "web": results }),
        }
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
