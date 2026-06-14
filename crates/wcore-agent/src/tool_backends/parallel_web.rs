//! Parallel.ai web search backend.
//!
//! Two paths, selected by whether a key is configured:
//!
//! * **Free (no key)** — the anonymous hosted Search MCP at
//!   `https://search.parallel.ai/mcp` (Streamable-HTTP JSON-RPC). One search =
//!   a 3-step handshake: `initialize` → `notifications/initialized` →
//!   `tools/call web_search`. The response envelope arrives as `application/json`
//!   OR `text/event-stream` (SSE); both are buffered and decoded by the same
//!   path. NO Authorization header — sending an empty bearer flips the server
//!   to 401. This is the zero-config default web backend.
//! * **Keyed (`PARALLEL_API_KEY`)** — the REST endpoint
//!   `https://api.parallel.ai/v1/search` with an `x-api-key` header, a single
//!   POST. Higher limits, no handshake.
//!
//! Both return per-result `{url, title, excerpts[]}`, mapped to the engine's
//! `{"web":[{title,url,snippet}]}` shape. Results are validated (http(s) URL +
//! non-empty title) — a zombie `200` ("please upgrade") or an empty/invalid set
//! becomes `WebOutcome::Err` so the `ChainedWebBackend` falls through to
//! DuckDuckGo.
//!
//! Session caching across calls is deliberately NOT done in this first cut: a
//! full handshake per search is what the reference clients do, and the tight
//! per-call budget + DDG fallback cover the latency. (Optimization for later.)

use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

const MCP_URL: &str = "https://search.parallel.ai/mcp";
const MCP_PROTOCOL: &str = "2025-06-18";
const REST_URL: &str = "https://api.parallel.ai/v1/search";
/// Per-HTTP-request ceiling (each handshake leg / the REST call).
const STEP_TIMEOUT: Duration = Duration::from_secs(8);
/// Whole-handshake budget for the free MCP path (covers all 3 round trips).
const MCP_OVERALL_TIMEOUT: Duration = Duration::from_secs(12);

/// Parallel.ai backend. `api_key = None` → free MCP; `Some` → keyed REST.
pub struct ParallelWebBackend {
    client: Client,
    api_key: Option<String>,
}

impl ParallelWebBackend {
    /// Free anonymous MCP backend — the zero-config default.
    pub fn free() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key: None,
        }
    }

    /// Keyed REST backend (`PARALLEL_API_KEY`).
    pub fn keyed(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key: Some(api_key),
        }
    }

    /// Keyed REST search: single POST with `x-api-key`.
    async fn rest_search(&self, key: &str, query: &str, limit: u32) -> WebOutcome {
        let body = json!({
            "search_queries": [query],
            "advanced_settings": { "max_results": limit.clamp(1, 20) },
        });
        let resp = match self
            .client
            .post(REST_URL)
            .header(CONTENT_TYPE, "application/json")
            .header("x-api-key", key)
            .timeout(STEP_TIMEOUT)
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return WebOutcome::Err {
                    message: format!("parallel rest request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return WebOutcome::Err {
                message: format!(
                    "parallel rest returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(300).collect::<String>()
                ),
            };
        }
        match serde_json::from_str::<Value>(&txt) {
            Ok(v) => map_parallel_results(&v, limit as usize),
            Err(e) => WebOutcome::Err {
                message: format!("parallel rest response was not JSON: {e}"),
            },
        }
    }

    /// One free-MCP search: 3-step handshake then parse the tool payload.
    async fn mcp_call(&self, query: &str, _limit: u32) -> Result<Value, String> {
        // 1. initialize — capture the optional session id from the header.
        let init_body = json!({
            "jsonrpc": "2.0", "id": "init-1", "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL,
                "capabilities": {},
                "clientInfo": { "name": "wayland-core", "version": env!("CARGO_PKG_VERSION") },
            },
        });
        let resp = self
            .client
            .post(MCP_URL)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .timeout(STEP_TIMEOUT)
            .body(init_body.to_string())
            .send()
            .await
            .map_err(|e| format!("initialize request failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!(
                "initialize returned HTTP {}",
                resp.status().as_u16()
            ));
        }
        let session = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let _ = resp.text().await; // drain init body (capabilities unused)

        // 2. notifications/initialized (a notification; server returns 202).
        let mut note = self
            .client
            .post(MCP_URL)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .timeout(STEP_TIMEOUT);
        if let Some(ref sid) = session {
            note = note.header("Mcp-Session-Id", sid.as_str());
        }
        note.body(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string())
            .send()
            .await
            .map_err(|e| format!("initialized notification failed: {e}"))?;

        // 3. tools/call web_search.
        let call_body = json!({
            "jsonrpc": "2.0", "id": "call-1", "method": "tools/call",
            "params": {
                "name": "web_search",
                "arguments": { "objective": query, "search_queries": [query] },
            },
        });
        let mut call = self
            .client
            .post(MCP_URL)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .timeout(STEP_TIMEOUT);
        if let Some(ref sid) = session {
            call = call.header("Mcp-Session-Id", sid.as_str());
        }
        let resp = call
            .body(call_body.to_string())
            .send()
            .await
            .map_err(|e| format!("tools/call request failed: {e}"))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| format!("tools/call body read failed: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "tools/call returned HTTP {}: {}",
                status.as_u16(),
                text.chars().take(300).collect::<String>()
            ));
        }
        parse_mcp_body(&text, "call-1")
    }
}

#[async_trait]
impl WebBackend for ParallelWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        if let Some(ref key) = self.api_key {
            return self.rest_search(key, query, limit).await;
        }
        match tokio::time::timeout(MCP_OVERALL_TIMEOUT, self.mcp_call(query, limit)).await {
            Ok(Ok(payload)) => map_parallel_results(&payload, limit as usize),
            Ok(Err(e)) => WebOutcome::Err {
                message: format!("parallel search failed: {e}"),
            },
            Err(_) => WebOutcome::Err {
                message: "parallel search timed out".to_string(),
            },
        }
    }

    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "extract not supported by the Parallel search backend; use the WebFetch tool \
                      on a specific URL, or set FIRECRAWL_API_KEY."
                .to_string(),
        }
    }

    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "crawl not supported by the Parallel search backend; set FIRECRAWL_API_KEY."
                .to_string(),
        }
    }

    fn backend_id(&self) -> &str {
        "parallel"
    }
}

/// Map a Parallel payload (`{results:[{url,title,excerpts}]}`) into the engine
/// `{"web":[{title,url,snippet}]}` shape, validating each result. Returns `Err`
/// on an error-shaped payload (`isError`) or when no result has a valid http(s)
/// URL + non-empty title — so the chain falls back to DuckDuckGo.
fn map_parallel_results(payload: &Value, limit: usize) -> WebOutcome {
    if payload.get("isError").and_then(Value::as_bool) == Some(true) {
        return WebOutcome::Err {
            message: "parallel returned an error payload".to_string(),
        };
    }
    let raw = payload
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
        let snippet = r
            .get("excerpts")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();
        results.push(json!({ "title": title, "url": url, "snippet": snippet }));
        if results.len() >= limit.max(1) {
            break;
        }
    }
    if results.is_empty() {
        return WebOutcome::Err {
            message: "parallel returned no valid results".to_string(),
        };
    }
    WebOutcome::Ok {
        payload: json!({ "web": results }),
    }
}

/// Decode an MCP response body (plain JSON or SSE) and extract the `tools/call`
/// tool payload for `req_id`.
fn parse_mcp_body(text: &str, req_id: &str) -> Result<Value, String> {
    let msgs = iter_mcp_messages(text);
    let envelope = select_envelope(&msgs, req_id)
        .ok_or_else(|| "parallel mcp: no JSON-RPC result/error in response".to_string())?;
    extract_tool_payload(&envelope)
}

/// Yield JSON-RPC objects from a plain-JSON body (one object/array) or an SSE
/// body (`data:` lines, events separated by blank lines).
fn iter_mcp_messages(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let body = text.trim();
    if body.is_empty() {
        return out;
    }
    if body.starts_with('{') || body.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Value>(body) {
            push_object(&mut out, v);
        }
        return out;
    }
    let mut data_lines: Vec<String> = Vec::new();
    for raw in body.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        } else if line.trim().is_empty() {
            flush_sse_event(&mut data_lines, &mut out);
        }
    }
    flush_sse_event(&mut data_lines, &mut out);
    out
}

fn flush_sse_event(data_lines: &mut Vec<String>, out: &mut Vec<Value>) {
    if data_lines.is_empty() {
        return;
    }
    if let Ok(v) = serde_json::from_str::<Value>(&data_lines.join("\n")) {
        push_object(out, v);
    }
    data_lines.clear();
}

fn push_object(out: &mut Vec<Value>, v: Value) {
    match v {
        Value::Array(items) => {
            for item in items {
                if item.is_object() {
                    out.push(item);
                }
            }
        }
        other if other.is_object() => out.push(other),
        _ => {}
    }
}

/// Pick the result/error message whose `id` matches `req_id` (scanning past
/// progress notifications); fall back to the last result/error-bearing message.
fn select_envelope(msgs: &[Value], req_id: &str) -> Option<Value> {
    let mut fallback: Option<Value> = None;
    for m in msgs {
        if m.get("result").is_none() && m.get("error").is_none() {
            continue;
        }
        if m.get("id").and_then(Value::as_str) == Some(req_id) {
            return Some(m.clone());
        }
        fallback = Some(m.clone());
    }
    fallback
}

/// Extract the tool result payload from a `tools/call` envelope — prefer
/// `result.structuredContent`, else the first JSON-parseable text block.
fn extract_tool_payload(envelope: &Value) -> Result<Value, String> {
    if let Some(err) = envelope.get("error") {
        return Err(format!(
            "parallel mcp error: {}",
            err.to_string().chars().take(300).collect::<String>()
        ));
    }
    let result = envelope.get("result").cloned().unwrap_or(Value::Null);
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err("parallel mcp tool reported isError".to_string());
    }
    if let Some(sc) = result.get("structuredContent")
        && sc.is_object()
    {
        return Ok(sc.clone());
    }
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(t) = block.get("text").and_then(Value::as_str)
                && let Ok(parsed) = serde_json::from_str::<Value>(t)
                && parsed.is_object()
            {
                return Ok(parsed);
            }
        }
    }
    Err("parallel mcp returned no parseable content".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_valid_results() {
        let p = json!({"results": [
            {"url": "https://a/", "title": "A", "excerpts": ["x", "y"]},
            {"url": "https://b/", "title": "B", "excerpts": ["z"]},
        ]});
        match map_parallel_results(&p, 5) {
            WebOutcome::Ok { payload } => {
                let web = payload.get("web").and_then(Value::as_array).unwrap();
                assert_eq!(web.len(), 2);
                assert_eq!(web[0]["title"], json!("A"));
                assert_eq!(web[0]["snippet"], json!("x\n\ny"));
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn rejects_zombie_and_empty_and_invalid_urls() {
        assert!(matches!(
            map_parallel_results(&json!({"isError": true}), 5),
            WebOutcome::Err { .. }
        ));
        assert!(matches!(
            map_parallel_results(&json!({"results": []}), 5),
            WebOutcome::Err { .. }
        ));
        // no http(s) url / empty title -> filtered -> Err
        assert!(matches!(
            map_parallel_results(&json!({"results": [{"url": "ftp://x/", "title": "T"}]}), 5),
            WebOutcome::Err { .. }
        ));
        assert!(matches!(
            map_parallel_results(&json!({"results": [{"url": "https://x/", "title": ""}]}), 5),
            WebOutcome::Err { .. }
        ));
    }

    #[test]
    fn respects_limit() {
        let p = json!({"results": [
            {"url": "https://a/", "title": "A"},
            {"url": "https://b/", "title": "B"},
            {"url": "https://c/", "title": "C"},
        ]});
        match map_parallel_results(&p, 2) {
            WebOutcome::Ok { payload } => {
                assert_eq!(payload["web"].as_array().unwrap().len(), 2);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn parses_plain_json_envelope() {
        let plain = r#"{"jsonrpc":"2.0","id":"call-1","result":{"structuredContent":{"results":[{"url":"https://a/","title":"A","excerpts":["x"]}]}}}"#;
        let payload = parse_mcp_body(plain, "call-1").expect("should parse");
        assert!(payload.get("results").is_some());
    }

    #[test]
    fn parses_sse_envelope_skipping_progress() {
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"call-1\",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"{\\\"results\\\":[{\\\"url\\\":\\\"https://a/\\\",\\\"title\\\":\\\"A\\\"}]}\"}]}}\n\n";
        let payload = parse_mcp_body(sse, "call-1").expect("should parse SSE");
        assert_eq!(payload["results"][0]["title"], json!("A"));
    }

    #[test]
    fn surfaces_jsonrpc_error() {
        let err = r#"{"jsonrpc":"2.0","id":"call-1","error":{"code":-32000,"message":"nope"}}"#;
        assert!(parse_mcp_body(err, "call-1").is_err());
    }

    /// Live end-to-end smoke test against the real Parallel free MCP endpoint
    /// (the zero-config default backend). Exercises the full production path —
    /// `EgressClient`, the 3-step handshake, envelope parse, and result mapping.
    /// `#[ignore]`d so CI never depends on a third-party network; run manually:
    /// `cargo test -p wcore-agent --lib parallel_web::tests::live_ -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "live network: hits the real https://search.parallel.ai/mcp endpoint"]
    async fn live_parallel_free_search_returns_results() {
        let backend = ParallelWebBackend::free();
        match backend
            .search("latest stable rust compiler version", 5)
            .await
        {
            WebOutcome::Ok { payload } => {
                let web = payload
                    .get("web")
                    .and_then(Value::as_array)
                    .expect("web[] present");
                assert!(!web.is_empty(), "expected >=1 result from live Parallel");
                let first = &web[0];
                let url = first.get("url").and_then(Value::as_str).unwrap_or("");
                let title = first.get("title").and_then(Value::as_str).unwrap_or("");
                assert!(url.starts_with("http"), "result url must be http(s): {url}");
                assert!(!title.is_empty(), "result title must be non-empty");
                eprintln!(
                    "LIVE PARALLEL OK — {} results; first: {url} | {title}",
                    web.len()
                );
            }
            WebOutcome::Err { message } => panic!("live parallel search returned Err: {message}"),
        }
    }
}
