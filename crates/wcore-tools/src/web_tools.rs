//! T3-3.8 — `web_search` / `web_extract` / `web_crawl` tools.
//!
//! Ported from the prior Wayland Python engine. The
//! Python original is a multi-backend dispatcher across Exa / Firecrawl /
//! Parallel / Tavily / DuckDuckGo / trafilatura, each with its own HTTP
//! client, response normalizer, optional LLM-summarization pass, and
//! debug-session logger. The Rust port keeps the **dispatch surface and
//! safety boundary** here in `wcore-tools` and lifts every concrete HTTP
//! / vendor SDK out behind a pluggable [`WebBackend`] trait, mirroring
//! the [`vision_tools`](crate::vision_tools) and
//! [`transcription_tools`](crate::transcription_tools) seam pattern.
//!
//! ## What is in the port
//!
//! * A single [`WebTool`] with an `operation` discriminator that exposes
//!   the three entry points (`search`, `extract`, `crawl`) through
//!   one Tool. Tool name: `"web"`. The same backend instance handles all
//!   three operations.
//! * [`WebBackend`] — async trait the host implements once per real
//!   provider (Tavily, Firecrawl, Exa, Parallel, …). Each call receives
//!   already-validated inputs; the backend is the only place that owns
//!   an HTTP client.
//! * [`NullWebBackend`] — default fail-loud backend (NO-STUBS).
//! * [`CapturingWebBackend`] — in-prod-module hermetic test backend that
//!   captures every call and returns canned responses, mirroring
//!   `CapturingVisionBackend` / `CapturingTranscriptionBackend`.
//! * SSRF + website-policy gating happens **before** the backend is
//!   called, for *every* URL in the inputs. This matches the post-fix
//!   layout in the prior Wayland Python engine where the website
//!   blocklist was moved out of the firecrawl branch so trafilatura and
//!   the free fallback couldn't bypass it.
//! * The two parameter validators ([`validate_search_query`],
//!   [`validate_url_list`]) and the [`WebOperation`] enum are public so
//!   downstream crates can reuse the rules.
//!
//! ## What is intentionally NOT in the port
//!
//! * No HTTP client. `wcore-tools` ships no `reqwest`/`hyper` dep
//!   (mirrors `vision_tools`, `transcription_tools`, `tts_tool`).
//! * No vendor SDK adapters (Firecrawl, Tavily, Exa, Parallel SDKs). The
//!   host wires those into a `WebBackend` implementation.
//! * No LLM summarization / chunking pass. The Python tool's optional
//!   `use_llm_processing` step calls the auxiliary-client router, which
//!   is a `wcore-providers` concern. Backends are free to summarize
//!   their own output if the host wires that in; the engine port
//!   forwards content verbatim.
//! * No `WEB_TOOLS_DEBUG=…` JSON dump file. The standard
//!   [`crate::debug_helpers`] session is the right surface; backends or
//!   the dispatcher can log there if needed.
//! * No `services.web-search.active` / `web.backend` config read. That
//!   is host wiring — the host picks which `WebBackend` it constructs.
//!
//! ## Security contract
//!
//! 1. URL list is rejected when **any** URL fails [`is_safe_url`] — the
//!    backend is never called with a private-network / loopback URL.
//! 2. URLs are screened by [`check_website_access`] (fail-open on
//!    malformed config — same semantics as vision_tools).
//! 3. URLs are rejected up front if they contain what looks like an API
//!    key prefix (`sk-`, `pk-`, percent-encoded variants). This mirrors
//!    the prior engine's secrets-in-URL guard and prevents
//!    accidental exfiltration via the URL bar.
//! 4. Search queries are length-bounded (4 KB) so an attacker can't
//!    force a megabyte-of-junk request through the backend.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;
use crate::website_policy::check_website_access;

/// Hard cap on a single search query. Mirrors the practical limit
/// observed across Tavily / Firecrawl / Exa (~2 KB) with headroom.
pub const WEB_MAX_QUERY_BYTES: usize = 4096;

/// Hard cap on the number of URLs passed in one extract call. Matches
/// the practical Firecrawl batch ceiling.
pub const WEB_MAX_EXTRACT_URLS: usize = 32;

/// Hard cap on a single URL length — anything longer is almost
/// certainly a smuggled blob, and most backends 4xx far below this.
pub const WEB_MAX_URL_BYTES: usize = 4096;

/// Default `limit` for `search` — mirrors the Python default (5).
pub const WEB_DEFAULT_SEARCH_LIMIT: u32 = 5;

/// Upper bound on `limit` accepted from the model. Anything larger is
/// clamped (matches the prior engine's Tavily request which clamps at 20).
pub const WEB_MAX_SEARCH_LIMIT: u32 = 50;

/// Inline key-prefix sniff. The full redactor lives in the prior
/// Wayland Python engine; here we keep a conservative subset that catches
/// the most common exfiltration vectors. Backends + the redact crate
/// remain responsible for content-side scrubbing.
fn url_contains_apparent_secret(url: &str) -> bool {
    // Match the Python `_PREFIX_RE` shape: vendor prefix followed by a
    // long token. We check both raw and once-percent-decoded forms.
    fn check(s: &str) -> bool {
        const PREFIXES: &[&str] = &["sk-", "pk-", "sk_live_", "rk_", "xoxb-", "xoxp-", "ghp_"];
        for needle in PREFIXES {
            if let Some(idx) = s.find(needle) {
                let rest = &s[idx + needle.len()..];
                // Require at least 16 trailing token-shaped chars to
                // avoid false positives on slugs like `sk-blog`.
                let token_chars = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                    .count();
                if token_chars >= 16 {
                    return true;
                }
            }
        }
        false
    }
    if check(url) {
        return true;
    }
    // Single-pass percent decode: replace `%XX` → byte. Lossless for
    // ASCII; non-ASCII falls back to the original character.
    let mut decoded = String::with_capacity(url.len());
    let bytes = url.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                decoded.push((h * 16 + l) as u8 as char);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    check(&decoded)
}

/// Validate a search query string. Returns `Ok(trimmed)` on success.
pub fn validate_search_query(query: &str) -> Result<&str, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("Search query is empty".to_string());
    }
    if trimmed.len() > WEB_MAX_QUERY_BYTES {
        return Err(format!(
            "Search query too long: {} bytes (limit {})",
            trimmed.len(),
            WEB_MAX_QUERY_BYTES,
        ));
    }
    Ok(trimmed)
}

/// Run every URL through SSRF + secret + length + website-policy gates
/// and return the filtered safe list plus a parallel list of structured
/// rejections (so the caller can return them as failed-row entries in
/// the response — matching the prior engine's partial-success shape).
///
/// `config_path = None` reads the cached website-policy bundle.
pub fn validate_url_list(urls: &[String]) -> (Vec<String>, Vec<UrlRejection>) {
    let mut safe = Vec::with_capacity(urls.len());
    let mut rejected = Vec::new();
    for url in urls {
        let u = url.trim();
        if u.is_empty() {
            rejected.push(UrlRejection {
                url: url.clone(),
                reason: "URL is empty".to_string(),
            });
            continue;
        }
        if u.len() > WEB_MAX_URL_BYTES {
            rejected.push(UrlRejection {
                url: u.chars().take(80).collect(),
                reason: format!(
                    "URL too long: {} bytes (limit {})",
                    u.len(),
                    WEB_MAX_URL_BYTES
                ),
            });
            continue;
        }
        if !(u.starts_with("http://") || u.starts_with("https://")) {
            rejected.push(UrlRejection {
                url: u.to_string(),
                reason: "Only http:// and https:// URLs are supported".to_string(),
            });
            continue;
        }
        if url_contains_apparent_secret(u) {
            rejected.push(UrlRejection {
                url: u.to_string(),
                reason:
                    "Blocked: URL contains what appears to be an API key or token. Secrets must \
                     not be sent in URLs."
                        .to_string(),
            });
            continue;
        }
        if !is_safe_url(u) {
            rejected.push(UrlRejection {
                url: u.to_string(),
                reason: "Blocked: URL targets a private or internal network address".to_string(),
            });
            continue;
        }
        match check_website_access(u, None) {
            Ok(Some(block)) => {
                rejected.push(UrlRejection {
                    url: u.to_string(),
                    reason: block.message,
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(target: "wcore_tools::web_tools", "website_policy error: {e}");
            }
        }
        safe.push(u.to_string());
    }
    (safe, rejected)
}

/// A single URL that failed pre-flight validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlRejection {
    pub url: String,
    pub reason: String,
}

/// Which operation a [`WebTool::execute`] call should
/// dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebOperation {
    /// `web_search_tool` — query the configured search backend.
    Search,
    /// `web_extract_tool` — extract content from one or more URLs.
    Extract,
    /// `web_crawl_tool` — crawl a site starting at a base URL.
    Crawl,
}

impl WebOperation {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "search" => Some(Self::Search),
            "extract" => Some(Self::Extract),
            "crawl" => Some(Self::Crawl),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Extract => "extract",
            Self::Crawl => "crawl",
        }
    }
}

/// Outcome of a backend call. Mirrors the JSON shape the prior engine's
/// tool emits — the backend returns a structured value and the Tool wraps
/// it in `{success, data: …}` / `{success, results: …}`.
#[derive(Debug, Clone)]
pub enum WebOutcome {
    /// Successful response. `payload` is splice-merged into the final
    /// result object — backends should return `{"web":[…]}` for search
    /// and `{"results":[…]}` for extract/crawl to match the prior engine's
    /// shapes.
    Ok { payload: Value },
    /// Structured error. Becomes `{success:false, error:<message>}`.
    Err { message: String },
}

/// A single extract/crawl request — backends see already-validated
/// inputs and can focus purely on network I/O.
#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub urls: Vec<String>,
    pub format: Option<String>,
    pub use_llm_processing: bool,
}

/// A single crawl request.
#[derive(Debug, Clone)]
pub struct CrawlRequest {
    pub url: String,
    pub instructions: Option<String>,
    pub depth: String,
    pub use_llm_processing: bool,
}

/// Pluggable backend boundary. Hosts that want web support wire a
/// concrete implementation backed by Firecrawl / Tavily / Exa /
/// Parallel / DuckDuckGo / trafilatura. The engine deliberately ships
/// no concrete HTTP client (mirrors `VisionBackend` / `TtsBackend`).
#[async_trait]
pub trait WebBackend: Send + Sync {
    /// Run a search. `query` is already trimmed + length-checked;
    /// `limit` is already clamped to `[1, WEB_MAX_SEARCH_LIMIT]`.
    async fn search(&self, query: &str, limit: u32) -> WebOutcome;

    /// Extract content from the listed URLs. All URLs in `req.urls`
    /// have passed SSRF + website-policy + secrets checks already.
    async fn extract(&self, req: ExtractRequest) -> WebOutcome;

    /// Crawl `req.url`. It has passed SSRF + website-policy + secrets
    /// checks already.
    async fn crawl(&self, req: CrawlRequest) -> WebOutcome;

    /// Optional friendly identifier ("firecrawl", "tavily", …) for
    /// error messages and debug logs. Default is `"unknown"`.
    fn backend_id(&self) -> &str {
        "unknown"
    }
}

/// Default backend returned when the host wires nothing — every call
/// fails loudly with a structured error so the tool never silently
/// succeeds. Honors the NO-STUBS contract.
pub struct NullWebBackend;

#[async_trait]
impl WebBackend for NullWebBackend {
    async fn search(&self, _query: &str, _limit: u32) -> WebOutcome {
        WebOutcome::Err {
            message: "No web backend configured. Wire a WebBackend implementation when \
                      constructing WebTool to enable web_search/extract/crawl."
                .to_string(),
        }
    }
    async fn extract(&self, _req: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "No web backend configured. Wire a WebBackend implementation when \
                      constructing WebTool to enable web_search/extract/crawl."
                .to_string(),
        }
    }
    async fn crawl(&self, _req: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "No web backend configured. Wire a WebBackend implementation when \
                      constructing WebTool to enable web_search/extract/crawl."
                .to_string(),
        }
    }
    fn backend_id(&self) -> &str {
        "null"
    }
}

/// A single captured web call — recorded by [`CapturingWebBackend`].
#[derive(Debug, Clone)]
pub enum CapturedWebCall {
    Search { query: String, limit: u32 },
    Extract(ExtractRequest),
    Crawl(CrawlRequest),
}

/// In-memory backend that records every call and returns canned
/// responses. Lives in the prod module so downstream crates can reuse
/// it without depending on `#[cfg(test)]` symbols (mirrors
/// `CapturingVisionBackend`).
pub struct CapturingWebBackend {
    pub search_payload: Value,
    pub extract_payload: Value,
    pub crawl_payload: Value,
    pub captured: parking_lot::Mutex<Vec<CapturedWebCall>>,
}

impl CapturingWebBackend {
    /// Construct with default empty payloads — `web: []` / `results: []`.
    pub fn new() -> Self {
        Self {
            search_payload: json!({ "web": [] }),
            extract_payload: json!({ "results": [] }),
            crawl_payload: json!({ "results": [] }),
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn with_search_payload(mut self, payload: Value) -> Self {
        self.search_payload = payload;
        self
    }
    pub fn with_extract_payload(mut self, payload: Value) -> Self {
        self.extract_payload = payload;
        self
    }
    pub fn with_crawl_payload(mut self, payload: Value) -> Self {
        self.crawl_payload = payload;
        self
    }

    pub fn snapshot(&self) -> Vec<CapturedWebCall> {
        self.captured.lock().clone()
    }
}

impl Default for CapturingWebBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WebBackend for CapturingWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        self.captured.lock().push(CapturedWebCall::Search {
            query: query.to_string(),
            limit,
        });
        WebOutcome::Ok {
            payload: self.search_payload.clone(),
        }
    }
    async fn extract(&self, req: ExtractRequest) -> WebOutcome {
        self.captured.lock().push(CapturedWebCall::Extract(req));
        WebOutcome::Ok {
            payload: self.extract_payload.clone(),
        }
    }
    async fn crawl(&self, req: CrawlRequest) -> WebOutcome {
        self.captured.lock().push(CapturedWebCall::Crawl(req));
        WebOutcome::Ok {
            payload: self.crawl_payload.clone(),
        }
    }
    fn backend_id(&self) -> &str {
        "capturing"
    }
}

/// `web` tool — Wayland engine port of the prior engine's three web entry
/// points.
///
/// Single Tool with an `operation` discriminator (search / extract /
/// crawl) that dispatches through a host-supplied [`WebBackend`]. Use
/// [`WebTool::new`] passing a backend; use [`WebTool::default`] for the
/// null-backed fail-loud variant.
pub struct WebTool {
    backend: Arc<dyn WebBackend>,
}

impl Default for WebTool {
    fn default() -> Self {
        Self::new(Arc::new(NullWebBackend))
    }
}

impl WebTool {
    pub fn new(backend: Arc<dyn WebBackend>) -> Self {
        Self { backend }
    }

    /// Borrow the configured backend (useful for tests that inspect
    /// `CapturingWebBackend.snapshot()` after `execute()` returns).
    pub fn backend(&self) -> &Arc<dyn WebBackend> {
        &self.backend
    }

    /// True when a real (non-null) `WebBackend` is wired. The host should
    /// gate registration on this so the model is not advertised a tool
    /// that always errors — the agent burns turns retrying a tool that
    /// cannot succeed (and on the TUI the AwaitingApproval modal makes
    /// the failure look like a hang). Returns `false` for the default
    /// [`NullWebBackend`] and `true` for any other backend.
    pub fn is_backed(&self) -> bool {
        self.backend.backend_id() != "null"
    }

    async fn dispatch_search(&self, input: &Value) -> ToolResult {
        let raw_query = match input.get("query").and_then(Value::as_str) {
            Some(s) => s,
            None => return err_result("Missing required parameter: 'query'"),
        };
        let query = match validate_search_query(raw_query) {
            Ok(q) => q,
            Err(e) => return err_result(&e),
        };
        let mut limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            .unwrap_or(WEB_DEFAULT_SEARCH_LIMIT);
        if limit == 0 {
            limit = WEB_DEFAULT_SEARCH_LIMIT;
        }
        if limit > WEB_MAX_SEARCH_LIMIT {
            limit = WEB_MAX_SEARCH_LIMIT;
        }
        match self.backend.search(query, limit).await {
            WebOutcome::Ok { payload } => ok_result(json!({
                "success": true,
                "data": payload,
            })),
            WebOutcome::Err { message } => err_result(&message),
        }
    }

    async fn dispatch_extract(&self, input: &Value) -> ToolResult {
        let urls = match input.get("urls").and_then(Value::as_array) {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>(),
            None => return err_result("Missing required parameter: 'urls' (array of strings)"),
        };
        if urls.is_empty() {
            return err_result("Parameter 'urls' must contain at least one URL");
        }
        if urls.len() > WEB_MAX_EXTRACT_URLS {
            return err_result(&format!(
                "Too many URLs: {} (limit {})",
                urls.len(),
                WEB_MAX_EXTRACT_URLS,
            ));
        }
        let format = input
            .get("format")
            .and_then(Value::as_str)
            .map(str::to_string);
        let use_llm_processing = input
            .get("use_llm_processing")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let (safe, rejected) = validate_url_list(&urls);
        // If every URL was rejected, short-circuit without calling the
        // backend — mirrors the prior engine returning a results-only response.
        if safe.is_empty() {
            return ok_result(json!({
                "success": true,
                "results": rejected_to_rows(&rejected),
            }));
        }

        let req = ExtractRequest {
            urls: safe,
            format,
            use_llm_processing,
        };
        match self.backend.extract(req).await {
            WebOutcome::Ok { mut payload } => {
                // Merge rejected URLs as failure rows into the backend
                // results, mirroring the prior engine's partial-success format.
                if !rejected.is_empty()
                    && let Some(results) = payload
                        .as_object_mut()
                        .and_then(|m| m.get_mut("results"))
                        .and_then(|v| v.as_array_mut())
                {
                    for row in rejected_to_rows(&rejected) {
                        results.push(row);
                    }
                }
                ok_result(json!({
                    "success": true,
                    "results": payload.get("results").cloned().unwrap_or_else(|| json!([])),
                }))
            }
            WebOutcome::Err { message } => err_result(&message),
        }
    }

    async fn dispatch_crawl(&self, input: &Value) -> ToolResult {
        let raw_url = match input.get("url").and_then(Value::as_str) {
            Some(s) => s,
            None => return err_result("Missing required parameter: 'url'"),
        };
        // Normalize scheme — the prior engine prepends https:// for bare hosts.
        let url_owned;
        let url = if raw_url.starts_with("http://") || raw_url.starts_with("https://") {
            raw_url
        } else {
            url_owned = format!("https://{}", raw_url);
            &url_owned
        };

        let (safe, rejected) = validate_url_list(&[url.to_string()]);
        if safe.is_empty() {
            let reason = rejected
                .first()
                .map(|r| r.reason.clone())
                .unwrap_or_else(|| "URL rejected".to_string());
            return err_result(&reason);
        }

        let instructions = input
            .get("instructions")
            .and_then(Value::as_str)
            .map(str::to_string);
        let depth = input
            .get("depth")
            .and_then(Value::as_str)
            .unwrap_or("basic")
            .to_string();
        // Bound depth to the two values the prior engine recognizes.
        if depth != "basic" && depth != "advanced" {
            return err_result("Parameter 'depth' must be 'basic' or 'advanced'");
        }
        let use_llm_processing = input
            .get("use_llm_processing")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let req = CrawlRequest {
            url: safe.into_iter().next().unwrap(),
            instructions,
            depth,
            use_llm_processing,
        };
        match self.backend.crawl(req).await {
            WebOutcome::Ok { payload } => ok_result(json!({
                "success": true,
                "results": payload.get("results").cloned().unwrap_or_else(|| json!([])),
            })),
            WebOutcome::Err { message } => err_result(&message),
        }
    }
}

fn rejected_to_rows(rejected: &[UrlRejection]) -> Vec<Value> {
    rejected
        .iter()
        .map(|r| {
            json!({
                "url": r.url,
                "title": "",
                "content": "",
                "error": r.reason,
            })
        })
        .collect()
}

fn ok_result(content: Value) -> ToolResult {
    ToolResult {
        content: content.to_string(),
        is_error: false,
    }
}

fn err_result(message: &str) -> ToolResult {
    ToolResult {
        content: json!({
            "success": false,
            "error": message,
        })
        .to_string(),
        is_error: true,
    }
}

#[async_trait]
impl Tool for WebTool {
    fn name(&self) -> &str {
        "web"
    }

    fn is_available(&self) -> bool {
        self.is_backed()
    }

    fn description(&self) -> &str {
        "Search the web, extract content from specific URLs, or crawl a site. Set `operation` \
         to one of `search`, `extract`, or `crawl`. SSRF defense and website-policy blocklist are \
         enforced on every URL; private addresses, blocked hosts, and URLs that contain apparent \
         API keys are rejected before any backend call."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["search", "extract", "crawl"],
                    "description": "Which web operation to run."
                },
                "query": {
                    "type": "string",
                    "description": "Search query. Required when operation=search."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": WEB_MAX_SEARCH_LIMIT,
                    "description": "Max number of search results. Default 5."
                },
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "URLs to extract content from. Required when operation=extract."
                },
                "url": {
                    "type": "string",
                    "description": "Base URL to crawl. Required when operation=crawl."
                },
                "instructions": {
                    "type": "string",
                    "description": "Optional natural-language instructions for a crawl."
                },
                "depth": {
                    "type": "string",
                    "enum": ["basic", "advanced"],
                    "description": "Crawl depth — 'basic' or 'advanced'. Default 'basic'."
                },
                "format": {
                    "type": "string",
                    "description": "Optional output format hint for extract (e.g. 'markdown', 'html')."
                },
                "use_llm_processing": {
                    "type": "boolean",
                    "description": "Whether the backend should run an LLM summarization pass. Default true."
                }
            },
            "required": ["operation"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // All three ops are read-only over external resources — safe
        // to run multiple in parallel (matches vision_analyze).
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let op_str = match input.get("operation").and_then(Value::as_str) {
            Some(s) => s,
            None => {
                return err_result(
                    "Missing required parameter: 'operation' (one of: search, extract, crawl)",
                );
            }
        };
        let op = match WebOperation::parse_str(op_str) {
            Some(op) => op,
            None => {
                return err_result(&format!(
                    "Unknown operation '{}'. Expected one of: search, extract, crawl",
                    op_str,
                ));
            }
        };
        match op {
            WebOperation::Search => self.dispatch_search(&input).await,
            WebOperation::Extract => self.dispatch_extract(&input).await,
            WebOperation::Crawl => self.dispatch_crawl(&input).await,
        }
    }
}

// ────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("ToolResult content must be JSON")
    }

    /// Quick assertion helper: the backend should NOT have been called.
    fn assert_no_backend_calls(backend: &CapturingWebBackend) {
        assert!(
            backend.snapshot().is_empty(),
            "Backend was unexpectedly called: {:?}",
            backend.snapshot()
        );
    }

    #[test]
    fn validate_search_query_rejects_empty_and_oversize() {
        assert!(validate_search_query("   ").is_err());
        assert!(validate_search_query("hello").is_ok());
        let big = "x".repeat(WEB_MAX_QUERY_BYTES + 1);
        assert!(validate_search_query(&big).is_err());
    }

    #[test]
    fn url_contains_apparent_secret_flags_sk_token_and_percent_decoded() {
        assert!(url_contains_apparent_secret(
            "https://evil.com/?k=sk-AAAAAAAAAAAAAAAA"
        ));
        // Same secret but percent-encoded.
        assert!(url_contains_apparent_secret(
            "https://evil.com/?k=%73k-AAAAAAAAAAAAAAAA"
        ));
        // Slug-shaped, NOT a token.
        assert!(!url_contains_apparent_secret("https://example.com/sk-blog"));
        // No prefix at all.
        assert!(!url_contains_apparent_secret("https://example.com/foo/bar"));
    }

    #[test]
    fn web_operation_round_trips() {
        for op in [
            WebOperation::Search,
            WebOperation::Extract,
            WebOperation::Crawl,
        ] {
            assert_eq!(WebOperation::parse_str(op.as_str()), Some(op));
        }
        assert_eq!(WebOperation::parse_str("bogus"), None);
    }

    #[tokio::test]
    async fn search_happy_path_invokes_backend_with_clamped_limit() {
        let backend = Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [
                {"title": "Hit", "url": "https://example.com/", "description": "ok", "position": 1}
            ]
        })));
        let tool = WebTool::new(backend.clone());
        let result = tool
            .execute(json!({
                "operation": "search",
                "query": "rust async",
                // Over the cap — must be clamped to WEB_MAX_SEARCH_LIMIT.
                "limit": 9999
            }))
            .await;
        assert!(!result.is_error);
        let v = parse(&result);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["data"]["web"][0]["title"], json!("Hit"));

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            CapturedWebCall::Search { query, limit } => {
                assert_eq!(query, "rust async");
                assert_eq!(*limit, WEB_MAX_SEARCH_LIMIT);
            }
            other => panic!("expected Search, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn search_rejects_missing_and_empty_query_without_backend() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());

        // Missing
        let r = tool.execute(json!({ "operation": "search" })).await;
        assert!(r.is_error);
        assert_no_backend_calls(&backend);

        // Empty after trim
        let r2 = tool
            .execute(json!({ "operation": "search", "query": "   " }))
            .await;
        assert!(r2.is_error);
        assert_no_backend_calls(&backend);
    }

    #[tokio::test]
    async fn extract_ssrf_blocked_before_backend() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());
        let r = tool
            .execute(json!({
                "operation": "extract",
                "urls": [
                    "http://127.0.0.1/admin",
                    "http://localhost:8080/secret",
                    "http://169.254.169.254/latest/meta-data/"
                ]
            }))
            .await;
        // All blocked → backend never called, response is success with
        // every row marked as error (matches the prior engine's partial-results).
        assert!(!r.is_error);
        assert_no_backend_calls(&backend);
        let v = parse(&r);
        let rows = v["results"].as_array().expect("results array");
        assert_eq!(rows.len(), 3);
        for row in rows {
            let err = row["error"].as_str().unwrap_or("");
            assert!(
                err.contains("private or internal"),
                "expected SSRF block message, got: {err}"
            );
        }
    }

    #[tokio::test]
    async fn extract_secret_in_url_blocked_before_backend() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());
        let r = tool
            .execute(json!({
                "operation": "extract",
                "urls": ["https://evil.com/path?token=sk-AAAAAAAAAAAAAAAA"]
            }))
            .await;
        assert!(!r.is_error);
        assert_no_backend_calls(&backend);
        let v = parse(&r);
        let rows = v["results"].as_array().expect("results array");
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["error"].as_str().unwrap().contains("API key"));
    }

    #[tokio::test]
    async fn extract_mixed_safe_and_unsafe_calls_backend_with_only_safe_urls() {
        let backend = Arc::new(CapturingWebBackend::new().with_extract_payload(json!({
            "results": [
                {"url": "https://example.com/", "title": "Example", "content": "..."}
            ]
        })));
        let tool = WebTool::new(backend.clone());
        let r = tool
            .execute(json!({
                "operation": "extract",
                "urls": ["https://example.com/", "http://127.0.0.1/"]
            }))
            .await;
        assert!(!r.is_error);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1, "backend should be called exactly once");
        match &snap[0] {
            CapturedWebCall::Extract(req) => {
                assert_eq!(req.urls, vec!["https://example.com/".to_string()]);
            }
            other => panic!("expected Extract, got {other:?}"),
        }
        let v = parse(&r);
        let rows = v["results"].as_array().expect("results array");
        // 1 backend row + 1 rejected row.
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn extract_rejects_oversized_url_list() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());
        let urls: Vec<String> = (0..(WEB_MAX_EXTRACT_URLS + 1))
            .map(|i| format!("https://example.com/{i}"))
            .collect();
        let r = tool
            .execute(json!({
                "operation": "extract",
                "urls": urls,
            }))
            .await;
        assert!(r.is_error);
        assert_no_backend_calls(&backend);
        let v = parse(&r);
        assert!(v["error"].as_str().unwrap().contains("Too many URLs"));
    }

    #[tokio::test]
    async fn crawl_normalizes_scheme_and_blocks_private_after_normalize() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());

        // Bare host gets https:// prepended, then blocked by SSRF.
        let r = tool
            .execute(json!({
                "operation": "crawl",
                "url": "127.0.0.1",
            }))
            .await;
        assert!(r.is_error, "expected SSRF rejection after scheme normalize");
        assert_no_backend_calls(&backend);
    }

    #[tokio::test]
    async fn crawl_happy_path_invokes_backend_with_defaults() {
        let backend = Arc::new(CapturingWebBackend::new().with_crawl_payload(json!({
            "results": [{"url": "https://example.com/", "title": "Ex", "content": "ok"}]
        })));
        let tool = WebTool::new(backend.clone());
        let r = tool
            .execute(json!({
                "operation": "crawl",
                "url": "example.com",
                "instructions": "find contact info"
            }))
            .await;
        assert!(!r.is_error);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            CapturedWebCall::Crawl(req) => {
                assert_eq!(req.url, "https://example.com");
                assert_eq!(req.depth, "basic");
                assert!(req.use_llm_processing);
                assert_eq!(req.instructions.as_deref(), Some("find contact info"));
            }
            other => panic!("expected Crawl, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn crawl_rejects_invalid_depth_before_backend() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());
        let r = tool
            .execute(json!({
                "operation": "crawl",
                "url": "https://example.com/",
                "depth": "ludicrous"
            }))
            .await;
        assert!(r.is_error);
        assert_no_backend_calls(&backend);
    }

    #[tokio::test]
    async fn null_backend_fails_loud() {
        let tool = WebTool::default();
        let r = tool
            .execute(json!({"operation": "search", "query": "anything"}))
            .await;
        assert!(r.is_error);
        let v = parse(&r);
        assert!(v["error"].as_str().unwrap().contains("No web backend"));
    }

    #[tokio::test]
    async fn unknown_operation_is_rejected() {
        let backend = Arc::new(CapturingWebBackend::new());
        let tool = WebTool::new(backend.clone());
        let r = tool.execute(json!({"operation": "delete_internet"})).await;
        assert!(r.is_error);
        assert_no_backend_calls(&backend);
    }

    #[test]
    fn tool_metadata_is_well_formed() {
        let tool = WebTool::default();
        assert_eq!(tool.name(), "web");
        // Concurrency safe — multiple in parallel is OK.
        assert!(tool.is_concurrency_safe(&json!({})));
        // Category is Info (read-only external lookup).
        assert!(matches!(tool.category(), ToolCategory::Info));
        // Schema parses and lists all three operations.
        let schema = tool.input_schema();
        let ops = schema["properties"]["operation"]["enum"]
            .as_array()
            .expect("operation enum");
        assert_eq!(ops.len(), 3);
    }
}
