//! FluxRouter image generation — a dedicated, **non-chat** client.
//!
//! Image generation is an OpenAI-Images-compatible endpoint
//! (`POST {base}/images/generations`), NOT the chat-completions surface, so it
//! does not go through [`crate::openai::OpenAIProvider`]. It is a single
//! synchronous request/response (generate → bill → return); there is no async
//! submit/poll. See `docs/FLUX-CAPABILITIES-CONTRACT.md` §3.
//!
//! Gating is paid-only (contract §2): a free / paid-but-uncleared key gets a
//! `402 premium_locked` with no provider call and no bill. We reuse T1's
//! [`crate::openai::parse_flux_402`] so the typed entitlement errors
//! ([`ProviderError::PremiumLocked`] etc.) are identical to the chat surface.
//!
//! Quirks handled here (contract §3.3 / §3.7):
//! - `gpt-image-*` arms **reject** `response_format` — the field is omitted for
//!   those arms.
//! - `size` and `response_format:"url"` are honored only by `together-flux`;
//!   other arms use a fixed size and return base64 natively. We pass the fields
//!   through as the caller set them and let the server apply that policy.
//! - ~60s synchronous timeout per provider call (no async poll). The client
//!   carries the non-streaming tool wall-clock cap (300s), generous enough for
//!   the slowest premium arm.

use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::ProviderError;

/// Default (cheapest) image arm — Together FLUX.1-schnell, ~$0.01/image
/// (contract §3.3 / §3.5). Used when the caller does not name a `model`.
pub const DEFAULT_IMAGE_MODEL: &str = "flux-image-together-flux";

/// One generated image. `data[i]` keys vary by arm (contract §3.4):
/// Gemini returns `b64_json` only; together-flux adds `timings`/`index`;
/// together-flux with `response_format:"url"` returns `url`. Unknown keys are
/// tolerated (`#[serde(default)]` on the fields we read; serde ignores extras).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImageDatum {
    /// Base64-encoded image bytes. Present for every arm except a
    /// together-flux request that explicitly asked for `response_format:"url"`.
    #[serde(default)]
    pub b64_json: Option<String>,
    /// Image URL — only when together-flux was asked for `response_format:"url"`.
    #[serde(default)]
    pub url: Option<String>,
}

/// Flux-specific response metadata. Currently carries the Gemini SynthID
/// notice (contract §3.4); absent on non-Gemini arms.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct FluxImageMeta {
    /// Human-readable SynthID watermark notice, surfaced to the user when the
    /// arm embeds an invisible watermark (Gemini/nano-banana arms).
    #[serde(default)]
    pub synthid_notice: Option<String>,
}

/// A successful image-generation response (HTTP 200, contract §3.4).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ImageResponse {
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub data: Vec<ImageDatum>,
    /// Flux passthrough metadata. Serialized as `_flux` on the wire.
    #[serde(rename = "_flux", default)]
    pub flux: Option<FluxImageMeta>,
}

impl ImageResponse {
    /// Decode `data[index].b64_json` into raw image bytes.
    ///
    /// Returns [`ProviderError::Parse`] when the index is out of range, the
    /// datum carries no `b64_json` (e.g. a together-flux `url` response — fetch
    /// `.url` instead), or the base64 is malformed.
    pub fn image_bytes(&self, index: usize) -> Result<Vec<u8>, ProviderError> {
        let datum = self.data.get(index).ok_or_else(|| {
            ProviderError::Parse(format!(
                "image response has {} image(s); index {index} out of range",
                self.data.len()
            ))
        })?;
        let b64 = datum.b64_json.as_deref().ok_or_else(|| {
            ProviderError::Parse(
                "image datum has no b64_json (a `url` response? fetch `.url` instead)".to_string(),
            )
        })?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| ProviderError::Parse(format!("invalid base64 image data: {e}")))
    }

    /// The SynthID watermark notice when the arm embedded one (Gemini),
    /// else `None`.
    pub fn synthid_notice(&self) -> Option<&str> {
        self.flux.as_ref().and_then(|m| m.synthid_notice.as_deref())
    }
}

/// A request to the image-generation endpoint (contract §3.2).
///
/// Built with [`ImageRequest::new`] and the chainable setters; the body is
/// serialized with `skip_serializing_if` so omitted options never reach the
/// wire (important: `response_format` MUST be absent for `gpt-image-*` arms).
#[derive(Debug, Clone, Serialize)]
pub struct ImageRequest {
    pub model: String,
    pub prompt: String,
    pub n: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

impl ImageRequest {
    /// New request for `prompt`, defaulting to the cheapest arm and `n=1`.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            model: DEFAULT_IMAGE_MODEL.to_string(),
            prompt: prompt.into(),
            n: 1,
            size: None,
            response_format: None,
            max_price: None,
            category: None,
        }
    }

    /// Set the arm (`model`). An empty/whitespace value keeps the default.
    pub fn with_model(mut self, model: Option<&str>) -> Self {
        if let Some(m) = model {
            let trimmed = m.trim();
            if !trimmed.is_empty() {
                self.model = trimmed.to_string();
            }
        }
        self
    }

    pub fn with_n(mut self, n: u32) -> Self {
        self.n = n.max(1);
        self
    }

    pub fn with_size(mut self, size: Option<String>) -> Self {
        self.size = size;
        self
    }

    pub fn with_response_format(mut self, fmt: Option<String>) -> Self {
        self.response_format = fmt;
        self
    }

    pub fn with_max_price(mut self, max_price: Option<f64>) -> Self {
        self.max_price = max_price;
        self
    }

    pub fn with_category(mut self, category: Option<String>) -> Self {
        self.category = category;
        self
    }

    /// True when `model` is a `gpt-image-*` arm. These arms reject the
    /// `response_format` field (contract §3.3 / §3.7), so the body builder
    /// drops it for them. Case-insensitive — aliases are case-insensitive.
    fn is_gpt_image_arm(&self) -> bool {
        self.model.to_ascii_lowercase().starts_with("gpt-image")
    }

    /// Serialize to the wire body, applying the per-arm field policy:
    /// `gpt-image-*` arms omit `response_format` entirely.
    pub fn to_body(&self) -> serde_json::Value {
        let mut req = self.clone();
        if req.is_gpt_image_arm() {
            req.response_format = None;
        }
        serde_json::to_value(&req).unwrap_or_else(|_| serde_json::json!({}))
    }
}

/// Dedicated client for the FluxRouter image-generation endpoint.
///
/// Holds an [`wcore_egress::EgressClient`] with the non-streaming tool timeout
/// policy (a hard wall-clock cap that suits a ~60s synchronous generation),
/// the Bearer key, and the resolved endpoint URL.
pub struct FluxImageClient {
    client: wcore_egress::EgressClient,
    api_key: String,
    /// Fully-resolved endpoint, e.g. `https://api.fluxrouter.ai/v1/images/generations`.
    endpoint: String,
}

impl FluxImageClient {
    /// Build a client. `base_url` is the Flux OpenAI-compatible base ending in
    /// `/v1` (e.g. [`crate::flux_router::FLUX_ROUTER_DEFAULT_BASE_URL`]); the
    /// `/images/generations` path is appended. A trailing slash on `base_url`
    /// is tolerated.
    pub fn new(api_key: &str, base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/');
        Self {
            // Tool client: connect + read timeouts PLUS a request-level
            // wall-clock cap. Image generation is a single finite response,
            // not a token stream, so the cap is correct here (the streaming
            // client's lack of a request cap would be wrong).
            client: crate::http_client::build_tool_client(),
            api_key: api_key.to_string(),
            endpoint: format!("{base}/images/generations"),
        }
    }

    /// The resolved endpoint URL (for diagnostics / tests).
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Generate image(s). On a non-2xx, a recognised Flux 402 maps to the
    /// typed entitlement error (contract §3.6); everything else surfaces as
    /// [`ProviderError::Api`].
    pub async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ProviderError> {
        if self.api_key.trim().is_empty() {
            return Err(ProviderError::MissingApiKey);
        }
        let body = request.to_body();

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|e| format!("<body read failed: {e}>"));
            // Reuse T1's 402 mapper: the image route's `premium_locked` /
            // `price_exceeds_max_price` and the shared error envelope are
            // recognised there. `premium_locked` is mapped to
            // `PremiumLocked { capability: "image generation", .. }`.
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
            .map_err(|e| ProviderError::Parse(format!("image response read failed: {e}")))?;
        serde_json::from_str::<ImageResponse>(&raw)
            .map_err(|e| ProviderError::Parse(format!("image response parse failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- request body shape -------------------------------------------------

    #[test]
    fn default_request_uses_cheapest_arm_and_n1() {
        let req = ImageRequest::new("a red apple");
        assert_eq!(req.model, DEFAULT_IMAGE_MODEL);
        assert_eq!(req.n, 1);
        let body = req.to_body();
        assert_eq!(body["model"], "flux-image-together-flux");
        assert_eq!(body["prompt"], "a red apple");
        assert_eq!(body["n"], 1);
        // Omitted options must NOT appear on the wire.
        assert!(body.get("size").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.get("max_price").is_none());
        assert!(body.get("category").is_none());
    }

    #[test]
    fn together_flux_keeps_response_format() {
        let body = ImageRequest::new("x")
            .with_model(Some("flux-image-together-flux"))
            .with_response_format(Some("url".into()))
            .with_size(Some("1024x1024".into()))
            .to_body();
        assert_eq!(body["response_format"], "url");
        assert_eq!(body["size"], "1024x1024");
    }

    #[test]
    fn gpt_image_arm_omits_response_format() {
        // Contract §3.3 / §3.7: gpt-image-* REJECTS response_format — the body
        // builder must drop it even when the caller set it.
        let body = ImageRequest::new("x")
            .with_model(Some("gpt-image-high"))
            .with_response_format(Some("b64_json".into()))
            .to_body();
        assert_eq!(body["model"], "gpt-image-high");
        assert!(
            body.get("response_format").is_none(),
            "gpt-image arms must omit response_format"
        );
    }

    #[test]
    fn gpt_image_arm_detection_is_case_insensitive() {
        let body = ImageRequest::new("x")
            .with_model(Some("GPT-Image-Med"))
            .with_response_format(Some("b64_json".into()))
            .to_body();
        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn with_model_none_keeps_default() {
        let req = ImageRequest::new("x").with_model(None);
        assert_eq!(req.model, DEFAULT_IMAGE_MODEL);
        let req = ImageRequest::new("x").with_model(Some("   "));
        assert_eq!(req.model, DEFAULT_IMAGE_MODEL);
    }

    #[test]
    fn max_price_and_category_serialize_when_set() {
        let body = ImageRequest::new("x")
            .with_max_price(Some(0.20))
            .with_category(Some("Pro".into()))
            .to_body();
        assert_eq!(body["max_price"], 0.20);
        assert_eq!(body["category"], "Pro");
    }

    // --- serde round-trips for BOTH captured §3.4 shapes ---------------------

    #[test]
    fn deserialize_together_flux_with_timings_and_index_extras() {
        // Contract §3.4 together-flux shape: provider passthrough extras
        // (`timings`, `index`), no `_flux`. We read b64_json and ignore extras.
        let raw = r#"{
            "created": 1781577103,
            "data": [ { "timings": {"inference": 1.2}, "index": 0, "b64_json": "aGVsbG8=" } ]
        }"#;
        let resp: ImageResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(resp.created, 1781577103);
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].b64_json.as_deref(), Some("aGVsbG8="));
        assert!(resp.flux.is_none());
        // b64 "aGVsbG8=" decodes to "hello".
        assert_eq!(resp.image_bytes(0).expect("decodes"), b"hello");
        assert!(resp.synthid_notice().is_none());
    }

    #[test]
    fn deserialize_nano_banana_with_synthid_notice() {
        // Contract §3.4 nano-banana (Gemini) shape: clean data[0] + the SynthID
        // notice under `_flux`.
        let raw = r#"{
            "created": 1781577200,
            "data": [ { "b64_json": "d29ybGQ=" } ],
            "_flux": { "synthid_notice": "This image contains an invisible SynthID watermark (Google)." }
        }"#;
        let resp: ImageResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(resp.data[0].b64_json.as_deref(), Some("d29ybGQ="));
        assert_eq!(resp.image_bytes(0).expect("decodes"), b"world");
        assert_eq!(
            resp.synthid_notice(),
            Some("This image contains an invisible SynthID watermark (Google).")
        );
    }

    #[test]
    fn deserialize_together_flux_url_response_has_no_b64() {
        let raw =
            r#"{ "created": 1, "data": [ { "url": "https://cdn.example/x.png", "index": 0 } ] }"#;
        let resp: ImageResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(
            resp.data[0].url.as_deref(),
            Some("https://cdn.example/x.png")
        );
        assert!(resp.data[0].b64_json.is_none());
        // No b64 to decode → a Parse error, not a panic.
        assert!(matches!(resp.image_bytes(0), Err(ProviderError::Parse(_))));
    }

    #[test]
    fn image_bytes_out_of_range_is_parse_error() {
        let resp = ImageResponse::default();
        assert!(matches!(resp.image_bytes(0), Err(ProviderError::Parse(_))));
    }

    #[test]
    fn image_bytes_rejects_malformed_base64() {
        let raw = r#"{ "data": [ { "b64_json": "not!base64!!" } ] }"#;
        let resp: ImageResponse = serde_json::from_str(raw).expect("parses");
        assert!(matches!(resp.image_bytes(0), Err(ProviderError::Parse(_))));
    }

    // --- 402 premium_locked mapping (reuses T1 parse_flux_402) ---------------

    #[test]
    fn premium_locked_402_maps_to_typed_error() {
        // Contract §3.6: image not-entitled → 402 premium_locked. T1's
        // parse_flux_402 maps this to PremiumLocked{capability:"image generation"}.
        let body = r#"{"error":{"message":"image generation requires a paid plan","code":"premium_locked"}}"#;
        let err = crate::openai::parse_flux_402(body).expect("recognised 402");
        match err {
            ProviderError::PremiumLocked {
                capability,
                message,
            } => {
                assert_eq!(capability, "image generation");
                assert!(message.contains("paid plan"));
            }
            other => panic!("expected PremiumLocked, got {other:?}"),
        }
    }

    #[test]
    fn endpoint_appends_path_and_tolerates_trailing_slash() {
        let c = FluxImageClient::new("k", "https://api.fluxrouter.ai/v1");
        assert_eq!(
            c.endpoint(),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
        let c = FluxImageClient::new("k", "https://api.fluxrouter.ai/v1/");
        assert_eq!(
            c.endpoint(),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
    }

    #[tokio::test]
    async fn generate_with_empty_key_is_missing_api_key() {
        let c = FluxImageClient::new("   ", "https://api.fluxrouter.ai/v1");
        let req = ImageRequest::new("x");
        assert!(matches!(
            c.generate(&req).await,
            Err(ProviderError::MissingApiKey)
        ));
    }
}
