//! Firecrawl web search backend. Requires `FIRECRAWL_API_KEY`; honors an
//! optional `FIRECRAWL_API_URL` for self-hosted instances.
//!
//! API docs: <https://docs.firecrawl.dev/api-reference/endpoint/search>.
//! This first cut wires **search** only; extract/crawl return the standard
//! "use WebFetch" message (Firecrawl's `/scrape` + `/crawl` are a follow-up).

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::read_env_key;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

const DEFAULT_BASE: &str = "https://api.firecrawl.dev";

pub struct FirecrawlWebBackend {
    client: Client,
    api_key: String,
    base_url: String,
}

impl FirecrawlWebBackend {
    pub fn new(api_key: String) -> Self {
        let base_url = read_env_key("FIRECRAWL_API_URL")
            .map(|u| u.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_BASE.to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            base_url,
        }
    }
}

#[async_trait]
impl WebBackend for FirecrawlWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let url = format!("{}/v1/search", self.base_url);
        let body = json!({ "query": query, "limit": limit.clamp(1, 20) });
        let resp = match self
            .client
            .post(&url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .timeout(std::time::Duration::from_secs(20))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("firecrawl request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "firecrawl returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(300).collect::<String>()
                ),
            };
        }
        let parsed: Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("firecrawl response was not JSON: {e}"),
                };
            }
        };
        let raw = parsed
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut results: Vec<Value> = Vec::new();
        for r in raw {
            let url = r
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let title = r
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let snippet = r
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            results.push(json!({ "title": title, "url": url, "snippet": snippet }));
        }
        if results.is_empty() {
            return WebOutcome::Err {
                message: "firecrawl returned no valid results".to_string(),
            };
        }
        WebOutcome::Ok {
            payload: json!({ "web": results }),
        }
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Firecrawl extract not yet wired in wayland-core; use the WebFetch tool."
                .to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Firecrawl crawl not yet wired in wayland-core.".to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "firecrawl"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live smoke test against the real Firecrawl `/v1/search` endpoint.
    /// `#[ignore]`d; run with `FIRECRAWL_API_KEY=… cargo test -p wcore-agent
    /// --lib firecrawl_web::tests::live_ -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live network + paid key: needs FIRECRAWL_API_KEY"]
    async fn live_firecrawl_search_returns_results() {
        let Some(key) = std::env::var("FIRECRAWL_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
        else {
            eprintln!("SKIP live_firecrawl: FIRECRAWL_API_KEY unset");
            return;
        };
        match FirecrawlWebBackend::new(key)
            .search("latest stable rust compiler version", 3)
            .await
        {
            WebOutcome::Ok { payload } => {
                let web = payload
                    .get("web")
                    .and_then(Value::as_array)
                    .expect("web[] present");
                assert!(!web.is_empty(), "expected >=1 firecrawl result");
                let url = web[0].get("url").and_then(Value::as_str).unwrap_or("");
                assert!(url.starts_with("http"), "url must be http(s): {url}");
                eprintln!("LIVE FIRECRAWL OK — {} results; first: {url}", web.len());
            }
            WebOutcome::Err { message } => panic!("live firecrawl returned Err: {message}"),
        }
    }
}
