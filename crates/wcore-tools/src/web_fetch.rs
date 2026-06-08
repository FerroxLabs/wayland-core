//! `WebFetch` tool - simple HTTP GET that returns a page as readable text.
//!
//! ## Why this exists
//!
//! The `Browser` tool (in `wcore-browser`) targets the interactive
//! browsing case: click, fill, screenshot, multi-step navigation through
//! JS-heavy SPAs. It dispatches into a Camoufox / Chromium / Browserbase
//! backend - none of which are present on a fresh `wayland-core` install
//! out of the box. A user asking "fetch github.com/trending and summarize
//! it" does NOT need a full browser; they need an HTTP GET against a
//! static HTML page, and a model can do everything else from the response.
//!
//! Before this tool existed, the model would call `Browser.navigate` for
//! exactly this case, the request would block on the missing sidecar, and
//! the user would watch a 60s spinner before getting a typed error. The
//! 10-minute hangs reported pre-Wave-RC were the dispatcher-layer outer
//! backstop on top of that.
//!
//! `WebFetch` closes the gap: zero external dependency at runtime, a
//! straightforward `reqwest` GET (host-supplied via `FetchBackend`), and
//! the model is told via tool description to prefer it for any read-only
//! page-fetch.
//!
//! ## Backend seam (mirrors `web_tools`, `github_tool`, etc.)
//!
//! `wcore-tools` ships no HTTP client (`reqwest` is not in its dep graph).
//! The tool surfaces a `FetchBackend` trait; the host (`wcore-agent`)
//! wires a real `reqwest::Client` in via `HttpFetchBackend`. The
//! `NullFetchBackend` returns a typed error so the tool can't silently
//! succeed without a backend (NO-STUBS contract).
//!
//! ## Security
//!
//! Every URL goes through:
//!   1. `http://` or `https://` scheme check
//!   2. `url_safety::is_safe_url` - SSRF / private-network guard
//!   3. `website_policy::check_website_access` - operator blocklist
//!
//! The backend never sees a URL that failed any of these.
//!
//! ## Output shape
//!
//! ```json
//! { "url": "...", "status": 200, "content_type": "text/html",
//!   "text": "...rendered text...", "truncated": false }
//! ```
//!
//! `text` is the readable text content of the page. HTML responses are
//! converted to text by the host backend (which has access to the
//! `wcore-browser::readability::extract` helper); non-HTML responses are
//! returned verbatim (`application/json`, `text/plain`, etc.) up to a
//! size cap.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;
use crate::website_policy::check_website_access;

/// Hard cap on the URL length we will hand to the backend.
pub const WEB_FETCH_MAX_URL_BYTES: usize = 4096;

/// Default per-call wall-clock cap surfaced to the backend.
pub const WEB_FETCH_DEFAULT_TIMEOUT_MS: u32 = 30_000;

/// Upper bound on the per-request timeout the model can request. 90s is
/// well below the `ToolCategory::Mcp` outer budget (120s) so the request
/// fails fast inside `WebFetch` rather than at the dispatcher tier.
pub const WEB_FETCH_MAX_TIMEOUT_MS: u32 = 90_000;

/// Cap on the body the backend returns to us.
pub const WEB_FETCH_MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// A single fetch request handed to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    pub timeout_ms: u32,
    /// When `true`, the backend should run HTML responses through a
    /// readability extractor and return text. When `false`, the backend
    /// returns the raw body verbatim.
    pub readable: bool,
}

/// What the backend returns. The Tool wraps this in a `ToolResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOutcome {
    Ok {
        status: u16,
        content_type: String,
        text: String,
        truncated: bool,
        final_url: String,
    },
    HttpError {
        status: u16,
        message: String,
    },
    Err {
        message: String,
    },
}

/// Pluggable HTTP-GET backend. `wcore-tools` ships no concrete impl that
/// touches the network; the host wires one via [`WebFetchTool::new`].
#[async_trait]
pub trait FetchBackend: Send + Sync {
    /// Perform the request. The URL has already passed scheme + SSRF +
    /// website-policy checks before this is called.
    async fn fetch(&self, req: &FetchRequest) -> FetchOutcome;
}

/// Default backend - every call fails loud so a host that forgot to wire
/// the real backend gets an honest error, never a silent stub.
pub struct NullFetchBackend;

#[async_trait]
impl FetchBackend for NullFetchBackend {
    async fn fetch(&self, _req: &FetchRequest) -> FetchOutcome {
        FetchOutcome::Err {
            message: "No fetch backend configured. The host must wire a FetchBackend \
                      implementation (typically wcore-agent's HttpFetchBackend) when \
                      constructing WebFetchTool."
                .to_string(),
        }
    }
}

/// Test backend that captures every request and replays a canned outcome.
pub struct CapturingFetchBackend {
    outcome: FetchOutcome,
    pub captured: parking_lot::Mutex<Vec<FetchRequest>>,
}

impl CapturingFetchBackend {
    pub fn new(outcome: FetchOutcome) -> Self {
        Self {
            outcome,
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Vec<FetchRequest> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl FetchBackend for CapturingFetchBackend {
    async fn fetch(&self, req: &FetchRequest) -> FetchOutcome {
        self.captured.lock().push(req.clone());
        self.outcome.clone()
    }
}

/// `WebFetch` tool - HTTP GET against a single URL, returns readable
/// text. Default-on for every wayland-core install.
pub struct WebFetchTool {
    backend: Arc<dyn FetchBackend>,
    backend_configured: bool,
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullFetchBackend),
            backend_configured: false,
        }
    }
}

impl WebFetchTool {
    pub fn new(backend: Arc<dyn FetchBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "error": message.into() }).to_string(),
        is_error: true,
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Fetch a single URL over HTTP/HTTPS and return its readable text. \
         Prefer this for ANY read-only page fetch - news articles, docs, \
         README files, GitHub pages, search-result pages, JSON APIs. Set \
         `readable: false` when the response is JSON / plain text and you \
         want the raw body. Use the `Browser` tool only when you need \
         interactive behavior the page requires (clicking, filling forms, \
         multi-step navigation through a JS-heavy SPA)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Absolute http:// or https:// URL to fetch."
                },
                "readable": {
                    "type": "boolean",
                    "description": "When true (default), HTML is run through a readability \
                                    extractor and returned as text. When false, the raw body \
                                    is returned verbatim (use for JSON / plain text).",
                    "default": true
                },
                "timeout_ms": {
                    "type": "integer",
                    "minimum": 1000,
                    "maximum": WEB_FETCH_MAX_TIMEOUT_MS,
                    "description": "Per-request wall-clock timeout in milliseconds. Defaults to \
                                    30000. Hard ceiling is 90000."
                }
            },
            "required": ["url"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let url = match input.get("url").and_then(Value::as_str) {
            Some(s) => s.trim().to_string(),
            None => return err_result("WebFetch: missing required `url` field"),
        };
        if url.is_empty() {
            return err_result("WebFetch: `url` is empty");
        }
        if url.len() > WEB_FETCH_MAX_URL_BYTES {
            return err_result(format!(
                "WebFetch: URL too long ({} bytes, limit {})",
                url.len(),
                WEB_FETCH_MAX_URL_BYTES
            ));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return err_result("WebFetch: only http:// and https:// URLs are supported");
        }

        if !is_safe_url(&url) {
            return err_result(
                "WebFetch: blocked - URL targets a private or internal network address",
            );
        }
        match check_website_access(&url, None) {
            Ok(Some(block)) => return err_result(format!("WebFetch: {}", block.message)),
            Ok(None) => {}
            // tools-io-18: fail CLOSED on policy-evaluation error. Previously
            // this warned and continued, so a malformed/unreadable operator
            // blocklist silently disabled the website-policy gate and let the
            // fetch through. A blocklist we cannot evaluate must block, not
            // bypass.
            Err(e) => {
                tracing::warn!(target: "wcore_tools::web_fetch", "website_policy error: {e}");
                return err_result(
                    "WebFetch: blocked - website access policy could not be evaluated",
                );
            }
        }

        let readable = input
            .get("readable")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map(|v| v.min(u64::from(WEB_FETCH_MAX_TIMEOUT_MS)) as u32)
            .unwrap_or(WEB_FETCH_DEFAULT_TIMEOUT_MS)
            .max(1_000);

        let req = FetchRequest {
            url: url.clone(),
            timeout_ms,
            readable,
        };

        match self.backend.fetch(&req).await {
            FetchOutcome::Ok {
                status,
                content_type,
                text,
                truncated,
                final_url,
            } => ToolResult {
                content: json!({
                    "url": final_url,
                    "status": status,
                    "content_type": content_type,
                    "text": text,
                    "truncated": truncated,
                })
                .to_string(),
                is_error: false,
            },
            FetchOutcome::HttpError { status, message } => {
                err_result(format!("WebFetch HTTP {status}: {message}"))
            }
            FetchOutcome::Err { message } => err_result(format!("WebFetch error: {message}")),
        }
    }
}

/// Register `WebFetch` into the tool registry bound to `backend`.
pub fn register_web_fetch_tool(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn FetchBackend>,
) {
    registry.register(Box::new(WebFetchTool::new(backend)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn null_backend_returns_fail_loud_error() {
        let tool = WebFetchTool::default();
        let r = tool.execute(json!({ "url": "https://example.com" })).await;
        assert!(r.is_error);
        assert!(
            r.content.contains("No fetch backend configured"),
            "unexpected message: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn missing_url_field_errors() {
        let tool = WebFetchTool::default();
        let r = tool.execute(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("missing required `url`"));
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let tool = WebFetchTool::default();
        let r = tool.execute(json!({ "url": "file:///etc/passwd" })).await;
        assert!(r.is_error);
        assert!(r.content.contains("http"));
    }

    #[tokio::test]
    async fn rejects_loopback_url() {
        let tool = WebFetchTool::default();
        let r = tool
            .execute(json!({ "url": "http://127.0.0.1/admin" }))
            .await;
        assert!(r.is_error);
        let msg = r.content.to_lowercase();
        assert!(
            msg.contains("private") || msg.contains("internal"),
            "unexpected message: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn rejects_aws_metadata() {
        let tool = WebFetchTool::default();
        let r = tool
            .execute(json!({ "url": "http://169.254.169.254/latest/meta-data/" }))
            .await;
        assert!(r.is_error);
    }

    #[tokio::test]
    async fn capturing_backend_records_request_and_returns_canned_outcome() {
        let backend = Arc::new(CapturingFetchBackend::new(FetchOutcome::Ok {
            status: 200,
            content_type: "text/html".into(),
            text: "trending: rust-lang/rust\ntrending: torvalds/linux".into(),
            truncated: false,
            final_url: "https://github.com/trending".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let r = tool
            .execute(json!({ "url": "https://github.com/trending" }))
            .await;
        assert!(!r.is_error, "unexpected error: {}", r.content);
        assert!(r.content.contains("rust-lang/rust"));
        assert!(r.content.contains("\"status\":200"));
        assert!(r.content.contains("\"truncated\":false"));
        let captured = backend.snapshot();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].url, "https://github.com/trending");
        assert!(captured[0].readable);
        assert_eq!(captured[0].timeout_ms, WEB_FETCH_DEFAULT_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn timeout_arg_is_clamped_to_max() {
        let backend = Arc::new(CapturingFetchBackend::new(FetchOutcome::Ok {
            status: 200,
            content_type: "text/plain".into(),
            text: String::new(),
            truncated: false,
            final_url: "https://example.com/".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let _ = tool
            .execute(json!({
                "url": "https://example.com/",
                "timeout_ms": 999_999_999u64
            }))
            .await;
        assert_eq!(backend.snapshot()[0].timeout_ms, WEB_FETCH_MAX_TIMEOUT_MS);
    }

    #[tokio::test]
    async fn readable_false_threads_through_to_backend() {
        let backend = Arc::new(CapturingFetchBackend::new(FetchOutcome::Ok {
            status: 200,
            content_type: "application/json".into(),
            text: "{\"ok\":true}".into(),
            truncated: false,
            final_url: "https://example.com/v1".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let r = tool
            .execute(json!({ "url": "https://example.com/v1", "readable": false }))
            .await;
        // is_safe_url does a real DNS lookup; in sandboxed / offline test
        // environments that resolution may fail. Skip the assertion if so
        // (the policy gate is exercised by the dedicated loopback /
        // metadata tests above; this one is asserting the `readable` flag
        // threads through, not the SSRF gate).
        if r.is_error && r.content.contains("private or internal network") {
            return;
        }
        assert!(!r.is_error, "unexpected error: {}", r.content);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1, "backend was not called");
        assert!(!snap[0].readable);
    }

    #[tokio::test]
    async fn http_error_outcome_surfaces_as_tool_error() {
        let backend = Arc::new(CapturingFetchBackend::new(FetchOutcome::HttpError {
            status: 404,
            message: "Not Found".into(),
        }));
        let tool = WebFetchTool::new(backend);
        let r = tool
            .execute(json!({ "url": "https://example.com/missing" }))
            .await;
        assert!(r.is_error);
        assert!(r.content.contains("404"));
    }

    #[test]
    fn schema_advertises_readable_and_timeout_fields() {
        let tool = WebFetchTool::default();
        let s = tool.input_schema();
        let props = s.get("properties").unwrap();
        assert!(props.get("url").is_some());
        assert!(props.get("readable").is_some());
        assert!(props.get("timeout_ms").is_some());
        let required = s.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0].as_str().unwrap(), "url");
    }

    #[test]
    fn category_is_mcp_not_exec() {
        let tool = WebFetchTool::default();
        assert!(matches!(tool.category(), ToolCategory::Mcp));
    }

    #[test]
    fn is_concurrency_safe_returns_true() {
        let tool = WebFetchTool::default();
        assert!(tool.is_concurrency_safe(&json!({})));
    }
}
