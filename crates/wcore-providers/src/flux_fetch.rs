//! FluxRouter web_fetch — a dedicated, **non-chat** client.
//!
//! web_fetch is a dedicated Flux endpoint (`POST {base}/fetch`, i.e.
//! `https://api.fluxrouter.ai/v1/fetch`), NOT the chat-completions surface and
//! NOT a chat-embedded tool (the chat `tools:[{type:"web_fetch"}]` form is not
//! wired — contract §4.1). It is a single synchronous request/response: fetch a
//! URL, convert to markdown, bill, return. See `docs/FLUX-CAPABILITIES-CONTRACT.md`
//! §4.
//!
//! Gating is paid-only (contract §2): a free / paid-but-uncleared key gets a
//! `402 upgrade_required` with no provider call and no bill. We reuse T1's
//! [`crate::openai::parse_flux_402`] so the typed entitlement errors
//! ([`ProviderError::UpgradeRequired`] etc.) are identical to the chat surface.
//!
//! Quirks handled here (contract §4.2 / §4.7):
//! - The body reads ONLY `{url, render}`; no other field is honored. `render`
//!   MUST serialize as the JSON literal `true`/`false` (a string `"true"` is
//!   treated as false server-side), so it is a Rust `bool`.
//! - A stable `x-request-id` header is sent per call so a retried fetch is
//!   idempotent on billing (ledger `ON CONFLICT`). We mint a fresh v4 UUID per
//!   call; callers wanting retry-idempotency reuse the same client+request and
//!   pass an explicit id via [`FetchRequest::with_request_id`].
//! - ~25s timeout, one URL per call (no batch). SSRF + social filters apply to
//!   both arms server-side; the client surfaces those `400` bodies verbatim.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ProviderError;

/// HTTP header carrying the idempotency key for billing dedupe (contract §4.7).
const REQUEST_ID_HEADER: &str = "x-request-id";

/// A successful web_fetch response (HTTP 200, contract §4.4).
///
/// Always exactly these three keys — no `_flux`, no headers. Jina passthrough
/// lines (`Title:` / `URL Source:`) live *inside* `markdown`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FetchResponse {
    /// The fetched page rendered as markdown.
    #[serde(default)]
    pub markdown: String,
    /// Always `"markdown"` today; kept as a field so a future format is not a
    /// breaking parse.
    #[serde(default)]
    pub format: String,
    /// The URL that was fetched (echoed by the server; may be normalized).
    #[serde(default)]
    pub url: String,
}

/// A request to the web_fetch endpoint (contract §4.2).
///
/// Only `url` and `render` are serialized — the server reads no other field.
/// `render` is a strict bool (default `false`); `true` selects the JS-rendered
/// premium arm (scrape.do, ~$0.02 vs ~$0.005).
#[derive(Debug, Clone, Serialize)]
pub struct FetchRequest {
    /// REQUIRED, non-empty, http(s) only (the server enforces SSRF/social
    /// filters — contract §4.6).
    pub url: String,
    /// Optional, default false. `true` → JS-rendered premium arm.
    pub render: bool,
    /// Per-call idempotency key sent as the `x-request-id` header. Not part of
    /// the JSON body. Defaults to a fresh v4 UUID.
    #[serde(skip)]
    pub request_id: String,
}

impl FetchRequest {
    /// New default-arm fetch for `url` with a fresh idempotency id.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            render: false,
            request_id: Uuid::new_v4().to_string(),
        }
    }

    /// Select the JS-rendered premium arm (scrape.do) when `render` is true.
    pub fn with_render(mut self, render: bool) -> Self {
        self.render = render;
        self
    }

    /// Override the idempotency id (`x-request-id`). Use the SAME id across a
    /// retry of the SAME fetch so billing dedupes (contract §4.7). An empty /
    /// whitespace value keeps the generated id.
    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        if !id.trim().is_empty() {
            self.request_id = id;
        }
        self
    }

    /// Serialize to the wire body — exactly `{url, render}` (the `request_id`
    /// field is `#[serde(skip)]` and travels as a header, not the body).
    pub fn to_body(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

/// Dedicated client for the FluxRouter web_fetch endpoint.
///
/// Holds an [`wcore_egress::EgressClient`] with the non-streaming tool timeout
/// policy (a hard wall-clock cap suiting the ~25s synchronous fetch), the
/// Bearer key, and the resolved endpoint URL.
pub struct FluxFetchClient {
    client: wcore_egress::EgressClient,
    api_key: String,
    /// Fully-resolved endpoint, e.g. `https://api.fluxrouter.ai/v1/fetch`.
    endpoint: String,
}

impl FluxFetchClient {
    /// Build a client. `base_url` is the Flux OpenAI-compatible base ending in
    /// `/v1` (e.g. [`crate::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL`]); the
    /// `/fetch` path is appended. A trailing slash on `base_url` is tolerated.
    pub fn new(api_key: &str, base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/');
        Self {
            // Tool client: connect + read timeouts PLUS a request-level
            // wall-clock cap. A fetch is a single finite response, not a token
            // stream, so the cap is correct here.
            client: crate::http_client::build_tool_client(),
            api_key: api_key.to_string(),
            endpoint: format!("{base}/fetch"),
        }
    }

    /// The resolved endpoint URL (for diagnostics / tests).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Fetch a URL → markdown. On a non-2xx, a recognised Flux 402 maps to the
    /// typed entitlement error (contract §4.6, `upgrade_required`); the SSRF /
    /// social `400` bodies and everything else surface as [`ProviderError::Api`]
    /// with the verbatim body so the CLI can show the server's reason.
    pub async fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::MissingApiKey);
        }
        if request.url.trim().is_empty() {
            return Err(ProviderError::Api {
                status: 400,
                message: "url required".to_string(),
            });
        }
        let body = request.to_body();

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .header(REQUEST_ID_HEADER, &request.request_id)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|e| format!("<body read failed: {e}>"));
            // Reuse T1's 402 mapper: web_fetch's not-entitled body is
            // `{"error":"upgrade_required","message":"..."}` → UpgradeRequired.
            if status.as_u16() == 402
                && let Some(err) = crate::openai::parse_flux_402(&body_text)
            {
                return Err(err);
            }
            return Err(ProviderError::Api {
                status: status.as_u16(),
                message: body_text,
            });
        }

        let raw = response
            .text()
            .await
            .map_err(|e| ProviderError::Parse(format!("fetch response read failed: {e}")))?;
        serde_json::from_str::<FetchResponse>(&raw)
            .map_err(|e| ProviderError::Parse(format!("fetch response parse failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- request body shape -------------------------------------------------

    #[test]
    fn default_request_omits_render_off_and_carries_url() {
        let req = FetchRequest::new("https://example.com");
        let body = req.to_body();
        assert_eq!(body["url"], "https://example.com");
        // render defaults to false and MUST serialize as a JSON bool, not a
        // string (contract §4.2: a string "true" is treated as false).
        assert_eq!(body["render"], serde_json::Value::Bool(false));
        // request_id is #[serde(skip)] — it travels as a header, not the body.
        assert!(body.get("request_id").is_none());
        // No other field is read by the server; we never emit extras.
        assert_eq!(
            body.as_object().map(|o| o.len()),
            Some(2),
            "body must be exactly {{url, render}}"
        );
    }

    #[test]
    fn render_true_serializes_as_json_bool() {
        let body = FetchRequest::new("https://x.test")
            .with_render(true)
            .to_body();
        assert_eq!(body["render"], serde_json::Value::Bool(true));
    }

    #[test]
    fn request_id_defaults_to_a_fresh_uuid() {
        let a = FetchRequest::new("https://x.test");
        let b = FetchRequest::new("https://x.test");
        assert!(!a.request_id.is_empty());
        // v4 UUIDs are 36 chars (8-4-4-4-12) and per-call unique.
        assert_eq!(a.request_id.len(), 36);
        assert_ne!(a.request_id, b.request_id);
    }

    #[test]
    fn with_request_id_overrides_and_ignores_blank() {
        let req = FetchRequest::new("https://x.test").with_request_id("stable-id-123");
        assert_eq!(req.request_id, "stable-id-123");
        // A blank override keeps the generated id (still a 36-char uuid).
        let made = FetchRequest::new("https://x.test");
        let original = made.request_id.clone();
        let kept = made.with_request_id("   ");
        assert_eq!(kept.request_id, original);
    }

    // --- serde round-trip of the {markdown, format, url} response -----------

    #[test]
    fn deserialize_captured_fetch_response() {
        // Contract §4.4 captured shape — exactly three keys.
        let raw = r#"{
            "markdown": "Title: Example Domain\n\nURL Source: https://example.com/\n\nMarkdown Content:\n# Example Domain\n",
            "format": "markdown",
            "url": "https://example.com"
        }"#;
        let resp: FetchResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(resp.format, "markdown");
        assert_eq!(resp.url, "https://example.com");
        assert!(resp.markdown.contains("# Example Domain"));
        // Jina passthrough lines live INSIDE markdown.
        assert!(resp.markdown.contains("Title: Example Domain"));
    }

    #[test]
    fn deserialize_tolerates_unexpected_extra_keys() {
        // Defensive: a future `_flux` or header echo must not break the parse.
        let raw = r#"{"markdown":"x","format":"markdown","url":"https://x","_flux":{"k":1}}"#;
        let resp: FetchResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(resp.markdown, "x");
    }

    // --- 402 upgrade_required mapping (reuses T1 parse_flux_402) -------------

    #[test]
    fn upgrade_required_402_maps_to_typed_error() {
        // Contract §4.6: web_fetch not-entitled → 402 upgrade_required, code is
        // the top-level `error` STRING with a sibling `message`.
        let body = r#"{"error":"upgrade_required","message":"web_fetch is a paid capability; upgrade or clear a charge"}"#;
        let err = crate::openai::parse_flux_402(body).expect("recognised 402");
        match err {
            ProviderError::UpgradeRequired { message } => {
                assert!(message.contains("paid capability"));
            }
            other => panic!("expected UpgradeRequired, got {other:?}"),
        }
    }

    #[test]
    fn ssrf_blocked_target_400_is_not_a_402_and_surfaces_verbatim() {
        // Contract §4.6: SSRF → `400 {"error":"blocked target:…"}`. This is NOT
        // a 402, so parse_flux_402 must not claim it (the client surfaces it as
        // ProviderError::Api{400, <body>}).
        let body = r#"{"error":"blocked target: 169.254.169.254"}"#;
        assert!(
            crate::openai::parse_flux_402(body).is_none(),
            "a 400 SSRF body must not be parsed as a 402 entitlement error"
        );
    }

    #[test]
    fn social_blocked_400_is_not_a_402() {
        // Contract §4.6: social hosts → `400 {"error":"social_blocked"}`.
        let body = r#"{"error":"social_blocked"}"#;
        assert!(
            crate::openai::parse_flux_402(body).is_none(),
            "a 400 social_blocked body must not be parsed as a 402"
        );
    }

    // --- client construction ------------------------------------------------

    #[test]
    fn endpoint_appends_fetch_path_and_tolerates_trailing_slash() {
        let c = FluxFetchClient::new("k", "https://api.fluxrouter.ai/v1");
        assert_eq!(c.endpoint(), "https://api.fluxrouter.ai/v1/fetch");
        let c = FluxFetchClient::new("k", "https://api.fluxrouter.ai/v1/");
        assert_eq!(c.endpoint(), "https://api.fluxrouter.ai/v1/fetch");
    }

    #[tokio::test]
    async fn fetch_with_empty_key_is_missing_api_key() {
        let c = FluxFetchClient::new("   ", "https://api.fluxrouter.ai/v1");
        let req = FetchRequest::new("https://example.com");
        assert!(matches!(
            c.fetch(&req).await,
            Err(ProviderError::MissingApiKey)
        ));
    }

    #[tokio::test]
    async fn fetch_with_empty_url_is_400() {
        let c = FluxFetchClient::new("k", "https://api.fluxrouter.ai/v1");
        let req = FetchRequest::new("   ");
        match c.fetch(&req).await {
            Err(ProviderError::Api { status, .. }) => assert_eq!(status, 400),
            other => panic!("expected Api{{400}}, got {other:?}"),
        }
    }
}
