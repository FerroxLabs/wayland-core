//! `WebFetch` tool - simple HTTP GET that returns a page as readable text.
//!
//! ## Why this exists
//!
//! The `Browser` tool (in `wcore-browser`) targets the interactive
//! browsing case: click, fill, screenshot, multi-step navigation through
//! JS-heavy SPAs. It dispatches into a Camoufox / Browserbase
//! backend - neither of which is present on a fresh `wayland-core` install
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
//!
//! `text` is additionally capped at [`WEB_FETCH_MAX_TEXT_CHARS`] before it
//! enters the conversation (wayland#1150). When that trim fires, `truncated`
//! is `true` and a `truncation_notice` field is added saying how much was
//! dropped and what to do about it - so the model can tell the user rather
//! than reasoning over a silently shortened page.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

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

/// Cap on the page text this tool puts into the MODEL'S CONTEXT.
///
/// Deliberately a different, much smaller number than
/// [`WEB_FETCH_MAX_RESPONSE_BYTES`], because the two bound different things.
/// That one bounds the network read and the readability parser's input - a
/// transport and CPU budget, paid once. This one bounds what enters the
/// conversation, and a tool result is not paid for once: it is replayed in the
/// `messages` array of EVERY later request for the rest of the session.
///
/// wayland#1150 measured the gap. One `WebFetch` of an HTML page put about
/// 50,000 characters of page text into a chat holding two visible sentences,
/// and the next turn cost 83,208 input tokens against a local model that then
/// spent minutes in prefill. 50,000 was not arbitrary: it is
/// [`crate::tool_output_limits::DEFAULT_MAX_BYTES`], the shared cap every tool
/// inherits from [`crate::Tool::max_result_size`], and it is sized for a
/// command's stdout rather than for an unbounded remote document.
///
/// 20,000 characters is roughly 5,000 tokens - the budget [`crate::grep`]
/// already takes for the same reason (unbounded external content, read once
/// and reasoned about), and more than the readable body of essentially any
/// article or documentation page. A model that needs what was cut is told so
/// by the notice below and can fetch a narrower URL; silently dropping the
/// tail is the half of this that would be its own bug.
pub const WEB_FETCH_MAX_TEXT_CHARS: usize = 20_000;

/// Trim `text` to [`WEB_FETCH_MAX_TEXT_CHARS`] characters, returning the kept
/// prefix and how many characters were dropped.
///
/// Character-wise, not byte-wise. The cap is about context cost, the notice
/// reports a character count, and slicing at a byte offset would either panic
/// or cut a multi-byte scalar in half - `char_indices` gives a real boundary
/// for free.
fn cap_page_text(text: &str) -> (String, usize) {
    let total = text.chars().count();
    if total <= WEB_FETCH_MAX_TEXT_CHARS {
        return (text.to_string(), 0);
    }
    let end = text
        .char_indices()
        .nth(WEB_FETCH_MAX_TEXT_CHARS)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    (text[..end].to_string(), total - WEB_FETCH_MAX_TEXT_CHARS)
}

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

    /// Run the full validated fetch pipeline and return the backend outcome.
    ///
    /// This is the **single shared validation path**: scheme check →
    /// [`is_safe_url`] (SSRF / private-network guard) → [`check_website_access`]
    /// (operator blocklist, fail-closed) → timeout clamp → backend fetch (whose
    /// client is itself SSRF-safe: validated-IP DNS resolver + per-redirect
    /// re-validation). Both the [`Tool::execute`] entry and any non-tool caller
    /// (e.g. the CLI `@url` resolver) go through this, so the security checks
    /// are defined exactly once and cannot drift into a second weaker path.
    ///
    /// `Err` carries a human-readable rejection reason (no `WebFetch:` prefix)
    /// for an invalid or blocked URL.
    pub async fn fetch_validated(
        &self,
        url: &str,
        readable: bool,
        timeout_ms: u32,
    ) -> Result<FetchOutcome, String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("`url` is empty".to_string());
        }
        if url.len() > WEB_FETCH_MAX_URL_BYTES {
            return Err(format!(
                "URL too long ({} bytes, limit {})",
                url.len(),
                WEB_FETCH_MAX_URL_BYTES
            ));
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("only http:// and https:// URLs are supported".to_string());
        }
        if !is_safe_url(url) {
            return Err("blocked - URL targets a private or internal network address".to_string());
        }
        match check_website_access(url, None) {
            Ok(Some(block)) => return Err(block.message),
            Ok(None) => {}
            // tools-io-18: fail CLOSED on policy-evaluation error — a blocklist
            // we cannot evaluate must block, not bypass.
            Err(e) => {
                tracing::warn!(target: "wcore_tools::web_fetch", "website_policy error: {e}");
                return Err("blocked - website access policy could not be evaluated".to_string());
            }
        }
        let timeout_ms = timeout_ms.clamp(1_000, WEB_FETCH_MAX_TIMEOUT_MS);
        let req = FetchRequest {
            url: url.to_string(),
            timeout_ms,
            readable,
        };
        Ok(self.backend.fetch(&req).await)
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
        // #403: serialize WebFetch calls. Running several fetches in parallel —
        // especially repeated fetches to the same slow host — multiplied the
        // load that pinned the readability parser and made the timeout/fallback
        // cascade worse. The engine runs non-concurrency-safe tools one at a
        // time, which is a simple, host-agnostic guard against that hammering.
        false
    }

    fn max_result_size(&self) -> usize {
        // Headroom over the text cap so `orchestration::truncate_result` never
        // fires on this tool's result. That cut compares BYTES against this
        // number and, above it, replaces the middle of the serialized JSON
        // with a marker - destroying the envelope and leaving the surviving
        // head declaring `"truncated": false` over a body that was truncated
        // twice. Bounding the payload here, in the field the trim applies to,
        // is `pdf_tool` / `doc_tool` / `email_parse_tool`'s pattern.
        //
        // Six bytes per character is the worst case for JSON-escaped text (a
        // control character renders as `\u00XX`); the rest covers the URL
        // (itself capped at `WEB_FETCH_MAX_URL_BYTES`), the content type and
        // the notice.
        WEB_FETCH_MAX_TEXT_CHARS * 6 + WEB_FETCH_MAX_URL_BYTES + 4_096
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Mcp
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Remote fetches can affect external rate limits and lack a durable reconciler.
        ToolEffectContract::default()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let url = match input.get("url").and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => return err_result("WebFetch: missing required `url` field"),
        };

        let readable = input
            .get("readable")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .map(|v| v.min(u64::from(WEB_FETCH_MAX_TIMEOUT_MS)) as u32)
            .unwrap_or(WEB_FETCH_DEFAULT_TIMEOUT_MS);

        // One validated path (scheme + SSRF + website-policy + clamp + fetch).
        let outcome = match self.fetch_validated(&url, readable, timeout_ms).await {
            Ok(o) => o,
            Err(msg) => return err_result(format!("WebFetch: {msg}")),
        };

        match outcome {
            FetchOutcome::Ok {
                status,
                content_type,
                text,
                truncated,
                final_url,
            } => {
                // wayland#1150. The backend's cap bounds the read; this bounds
                // the CONTEXT. Trimming here, on the `text` field, rather than
                // leaving it to the registry-level cut in
                // `orchestration::truncate_result`, is the whole point: that
                // cut slices the SERIALIZED JSON in half and splices a marker
                // into the middle, which leaves the model an unparseable
                // envelope whose surviving head still says `"truncated": false`.
                let total = text.chars().count();
                let (text, dropped) = cap_page_text(&text);
                let mut body = json!({
                    "url": final_url,
                    "status": status,
                    "content_type": content_type,
                    "text": text,
                    "truncated": truncated || dropped > 0,
                });
                if dropped > 0 {
                    // Say it in the payload, not in a log line: `warn!` never
                    // reaches a user whose RUST_LOG is unset, which is every
                    // default install. The model is the one that has to know
                    // the page is incomplete, and it only reads this.
                    body["truncation_notice"] = json!(format!(
                        "Only the first {kept} characters of this page are in context; \
                         {dropped} more were dropped (the page held {total}). Say so if you \
                         summarise it. If what you need is missing, fetch a more specific URL \
                         rather than this one again - a repeat fetch returns the same prefix.",
                        kept = WEB_FETCH_MAX_TEXT_CHARS,
                    ));
                }
                ToolResult {
                    content: body.to_string(),
                    is_error: false,
                }
            }
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
    // Fixtures use a literal public IP, never a hostname.
    //
    // The tool path runs `url_safety::is_safe_url`, which performs a REAL DNS
    // resolution and fails closed when it returns no addresses, so a hostname
    // fixture makes these tests depend on the runner's resolver. A literal IP
    // takes the `parse::<IpAddr>()` fast path and skips resolution entirely.
    // Hostname resolution is covered hermetically in `url_safety`'s own tests
    // via `is_safe_url_with(.., fake_resolver)`. Same convention as
    // `transcription_tools`.
    use super::*;
    use wcore_types::tool::ToolEffectKind;

    #[test]
    fn effect_contract_remains_opaque() {
        let contract = WebFetchTool::default().effect_contract(&json!({
            "url": "https://example.com"
        }));
        assert_eq!(contract.kind, ToolEffectKind::Opaque);
        assert!(contract.reconciler.is_none());
    }

    #[tokio::test]
    async fn null_backend_returns_fail_loud_error() {
        let tool = WebFetchTool::default();
        let r = tool
            .execute(json!({ "url": "https://93.184.216.34" }))
            .await;
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

    // -- wayland#1150: a fetched page must not blow the context window ----

    fn page_backend(text: String) -> Arc<CapturingFetchBackend> {
        Arc::new(CapturingFetchBackend::new(FetchOutcome::Ok {
            status: 200,
            content_type: "text/html".to_string(),
            text,
            truncated: false,
            final_url: "https://93.184.216.34/".to_string(),
        }))
    }

    async fn fetch_page(text: String) -> Value {
        let tool = WebFetchTool::new(page_backend(text));
        let r = tool
            .execute(json!({ "url": "https://93.184.216.34" }))
            .await;
        assert!(!r.is_error, "unexpected error: {}", r.content);
        serde_json::from_str(&r.content).unwrap_or_else(|e| {
            panic!(
                "a WebFetch result must be parseable JSON - {e}: {}",
                r.content
            )
        })
    }

    /// **wayland#1150.** A single fetch must not put an unbounded document into
    /// the conversation.
    ///
    /// The reporter measured a `WebFetch` result of about 50,000 characters of
    /// HTML sitting in a chat with two visible sentences in it, and the next
    /// turn costing 83,208 input tokens against a local model. A tool result is
    /// replayed to the provider on every subsequent turn, so this is a
    /// per-turn cost for the rest of the session, not a one-off.
    ///
    /// The size in this test is the reporter's own.
    #[tokio::test]
    async fn a_large_fetched_page_does_not_enter_the_context_whole() {
        let page = "A".repeat(50_000);
        let body = fetch_page(page.clone()).await;
        let text = body["text"].as_str().expect("the result must carry `text`");
        assert!(
            text.chars().count() <= WEB_FETCH_MAX_TEXT_CHARS,
            "a {}-character page put {} characters into the context; the cap is {}",
            page.chars().count(),
            text.chars().count(),
            WEB_FETCH_MAX_TEXT_CHARS
        );
    }

    /// Trimming silently would trade one bug for another: the model would
    /// summarise a page it believes it read in full. `truncated` must flip and
    /// the notice must name the missing amount.
    #[tokio::test]
    async fn a_trimmed_page_says_so_and_says_how_much() {
        let page = "A".repeat(50_000);
        let body = fetch_page(page).await;
        assert_eq!(
            body["truncated"].as_bool(),
            Some(true),
            "a trimmed page reported truncated=false, so the model has no way to know: {body}"
        );
        let notice = body["truncation_notice"]
            .as_str()
            .unwrap_or_else(|| panic!("a trimmed page carries no truncation_notice: {body}"));
        assert!(
            notice.contains("30000")
                || notice.contains(&(50_000 - WEB_FETCH_MAX_TEXT_CHARS).to_string()),
            "the notice must name how much was dropped, got: {notice}"
        );
    }

    /// NEGATIVE CONTROL. A page under the cap must be passed through untouched
    /// and must NOT claim to have been trimmed - otherwise the assertions above
    /// would pass against a tool that mangles every fetch.
    #[tokio::test]
    async fn a_page_under_the_cap_is_untouched_and_not_marked_truncated() {
        let page = "B".repeat(WEB_FETCH_MAX_TEXT_CHARS);
        let body = fetch_page(page.clone()).await;
        assert_eq!(body["text"].as_str(), Some(page.as_str()));
        assert_eq!(body["truncated"].as_bool(), Some(false));
        assert!(
            body.get("truncation_notice").is_none(),
            "an untrimmed page must not carry a truncation notice: {body}"
        );
    }

    /// The cut must land on a character boundary. A multi-byte page trimmed at
    /// a byte offset either panics or emits a lone continuation byte, and the
    /// second is worse: it survives into the context as replacement characters.
    #[tokio::test]
    async fn a_multibyte_page_is_cut_on_a_character_boundary() {
        // Every scalar is 3 bytes, so no byte-offset cut can land cleanly.
        let page = "\u{4f60}".repeat(50_000);
        let body = fetch_page(page).await;
        let text = body["text"].as_str().expect("the result must carry `text`");
        assert_eq!(
            text.chars().count(),
            WEB_FETCH_MAX_TEXT_CHARS,
            "a CJK page was cut to the wrong length, which means it was cut by bytes"
        );
        assert!(
            !text.contains('\u{fffd}'),
            "the trim produced a replacement character, so it split a scalar"
        );
    }

    /// The registry-level cut must never reach this tool's envelope.
    ///
    /// `orchestration::truncate_result` compares `content.len()` in BYTES
    /// against `max_result_size()` and, above it, replaces the middle of the
    /// string with `... [truncated N chars] ...`. On a JSON payload that is not
    /// a truncation, it is corruption: the model gets an unparseable object
    /// whose surviving head still reads `"truncated": false`. Bounding the
    /// payload inside the tool is only half the fix; this is the half that
    /// stops the outer cut firing at all.
    #[tokio::test]
    async fn the_worst_case_result_fits_inside_this_tools_declared_budget() {
        // Worst case for the trim: a page of multi-byte scalars, so the kept
        // prefix is `WEB_FETCH_MAX_TEXT_CHARS` characters of 3 bytes each, plus
        // the notice and the envelope.
        let page = "\u{4f60}".repeat(50_000);
        let tool = WebFetchTool::new(page_backend(page));
        let r = tool
            .execute(json!({ "url": "https://93.184.216.34" }))
            .await;
        assert!(!r.is_error, "unexpected error: {}", r.content);
        assert!(
            r.content.len() <= tool.max_result_size(),
            "a worst-case result is {} bytes against a declared budget of {} - the registry cut \
             fires and the model receives wreckage instead of JSON",
            r.content.len(),
            tool.max_result_size()
        );
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
            final_url: "https://93.184.216.34/trending".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let r = tool
            .execute(json!({ "url": "https://93.184.216.34/trending" }))
            .await;
        assert!(!r.is_error, "unexpected error: {}", r.content);
        assert!(r.content.contains("rust-lang/rust"));
        assert!(r.content.contains("\"status\":200"));
        assert!(r.content.contains("\"truncated\":false"));
        let captured = backend.snapshot();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].url, "https://93.184.216.34/trending");
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
            final_url: "https://93.184.216.34/".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let _ = tool
            .execute(json!({
                "url": "https://93.184.216.34/",
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
            final_url: "https://93.184.216.34/v1".into(),
        }));
        let tool = WebFetchTool::new(backend.clone());
        let r = tool
            .execute(json!({ "url": "https://93.184.216.34/v1", "readable": false }))
            .await;
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
            .execute(json!({ "url": "https://93.184.216.34/missing" }))
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
    fn is_not_concurrency_safe() {
        // #403: WebFetch is serialized to avoid parallel same-host hammering
        // that worsened the readability-timeout cascade.
        let tool = WebFetchTool::default();
        assert!(!tool.is_concurrency_safe(&json!({})));
    }
}
