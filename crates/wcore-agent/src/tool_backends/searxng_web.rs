//! SearXNG meta-search backend. Gated by `SEARXNG_URL` (your own or a public
//! instance) — wayland-core ships the connector, not the instance.
//!
//! GETs `<SEARXNG_URL>/search?q=<query>&format=json` and maps the JSON
//! `results[]` (sorted by `score` desc) into the engine shape.
//!
//! ⚠️ The instance must be **publicly resolvable**: requests go through the
//! SSRF-safe client, so a `SEARXNG_URL` pointing at `localhost`/a private IP is
//! rejected by the DNS resolver. Self-hosters must expose SearXNG behind a
//! public hostname for now (a scoped `WAYLAND_SEARXNG_ALLOW_PRIVATE` opt-in is
//! a planned follow-up).

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::urlencode;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

pub struct SearxngWebBackend {
    client: Client,
    base_url: String,
}

impl SearxngWebBackend {
    pub fn new(base_url: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }
}

#[async_trait]
impl WebBackend for SearxngWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let url = format!(
            "{}/search?q={}&format=json",
            self.base_url,
            urlencode(query)
        );
        let resp = match self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("searxng request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "searxng returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(300).collect::<String>()
                ),
            };
        }
        let parsed: Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("searxng response was not JSON: {e}"),
                };
            }
        };
        map_searxng_results(&parsed, limit as usize)
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "SearXNG does not support extract; use the WebFetch tool on a URL."
                .to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "SearXNG does not support crawl.".to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "searxng"
    }
}

/// Map a SearXNG JSON response into the engine shape: sort by `score` desc,
/// validate http(s) URL + title, cap to `limit`.
fn map_searxng_results(parsed: &Value, limit: usize) -> WebOutcome {
    let mut raw = parsed
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Sort by score descending (missing score sorts last).
    raw.sort_by(|a, b| {
        let sb = b.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        let sa = a.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
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
        let snippet = r
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        results.push(json!({ "title": title, "url": url, "snippet": snippet }));
        if results.len() >= limit.max(1) {
            break;
        }
    }
    if results.is_empty() {
        return WebOutcome::Err {
            message: "searxng returned no valid results".to_string(),
        };
    }
    WebOutcome::Ok {
        payload: json!({ "web": results }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_by_score_desc_and_caps() {
        let parsed = json!({"results": [
            {"title": "low", "url": "https://low/", "content": "c", "score": 0.1},
            {"title": "high", "url": "https://high/", "content": "c", "score": 0.9},
            {"title": "mid", "url": "https://mid/", "content": "c", "score": 0.5},
        ]});
        match map_searxng_results(&parsed, 2) {
            WebOutcome::Ok { payload } => {
                let web = payload["web"].as_array().unwrap();
                assert_eq!(web.len(), 2, "capped to limit");
                assert_eq!(web[0]["title"], json!("high"));
                assert_eq!(web[1]["title"], json!("mid"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_and_invalid() {
        assert!(matches!(
            map_searxng_results(&json!({"results": []}), 5),
            WebOutcome::Err { .. }
        ));
        assert!(matches!(
            map_searxng_results(&json!({"results": [{"title": "x", "url": "ftp://x/"}]}), 5),
            WebOutcome::Err { .. }
        ));
    }
}
