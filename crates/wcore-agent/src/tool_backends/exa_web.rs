//! Exa neural search backend. Requires `EXA_API_KEY`.
//!
//! API docs: <https://docs.exa.ai/reference/search>. POSTs to
//! `https://api.exa.ai/search` with an `x-api-key` header and requests inline
//! text contents so each result carries a snippet.

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

const EXA_URL: &str = "https://api.exa.ai/search";
/// Exa `text` contents can be large; cap the snippet so results stay compact.
const SNIPPET_CAP: usize = 1000;

pub struct ExaWebBackend {
    client: Client,
    api_key: String,
}

impl ExaWebBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
        }
    }
}

#[async_trait]
impl WebBackend for ExaWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let body = json!({
            "query": query,
            "numResults": limit.clamp(1, 20),
            "contents": { "text": true },
        });
        let resp = match self
            .client
            .post(EXA_URL)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("x-api-key", &self.api_key)
            .timeout(std::time::Duration::from_secs(15))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("exa request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "exa returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(300).collect::<String>()
                ),
            };
        }
        let parsed: Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("exa response was not JSON: {e}"),
                };
            }
        };
        let raw = parsed
            .get("results")
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
            if title.is_empty() || !(url.starts_with("http://") || url.starts_with("https://")) {
                continue;
            }
            let snippet: String = r
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(SNIPPET_CAP)
                .collect();
            results.push(json!({ "title": title, "url": url, "snippet": snippet }));
        }
        if results.is_empty() {
            return WebOutcome::Err {
                message: "exa returned no valid results".to_string(),
            };
        }
        WebOutcome::Ok {
            payload: json!({ "web": results }),
        }
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Exa extract not wired; use the WebFetch tool on a specific URL.".to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "Exa crawl not supported.".to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "exa"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live smoke test against the real Exa `/search` endpoint. `#[ignore]`d;
    /// run with `EXA_API_KEY=… cargo test -p wcore-agent --lib
    /// exa_web::tests::live_ -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live network + paid key: needs EXA_API_KEY"]
    async fn live_exa_search_returns_results() {
        let Some(key) = std::env::var("EXA_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())
        else {
            eprintln!("SKIP live_exa: EXA_API_KEY unset");
            return;
        };
        match ExaWebBackend::new(key)
            .search("latest stable rust compiler version", 3)
            .await
        {
            WebOutcome::Ok { payload } => {
                let web = payload
                    .get("web")
                    .and_then(Value::as_array)
                    .expect("web[] present");
                assert!(!web.is_empty(), "expected >=1 exa result");
                let url = web[0].get("url").and_then(Value::as_str).unwrap_or("");
                assert!(url.starts_with("http"), "url must be http(s): {url}");
                eprintln!("LIVE EXA OK — {} results; first: {url}", web.len());
            }
            WebOutcome::Err { message } => panic!("live exa returned Err: {message}"),
        }
    }
}
