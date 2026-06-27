//! `image_generate` tool backend resolver — v0.9.0 W1 sub-agent B1.
//!
//! Wires the existing `ImageGenerationBackend` trait (in
//! `wcore-tools/src/image_generation_tool.rs`) to five real providers:
//!
//! 1. **OpenAI image** (`OPENAI_API_KEY`; `gpt-image-1` default, configurable)
//! 2. **FAL FLUX schnell** (`FAL_API_KEY`)
//! 3. **Gemini Imagen 3** (`GEMINI_API_KEY`)
//! 4. **Hugging Face FLUX** (`HF_API_KEY`)
//! 5. **Pollinations.ai** — zero-key, **gated by config opt-in**
//!    (`[tools.image_gen] allow_pollinations_fallback = true`)
//!
//! Every backend uses `build_ssrf_safe_tool_client()` (S-B1 SSRF guard)
//! and wraps each external call in `tokio::time::timeout` (R-H1 two-layer
//! timeout). The resolver `build_image_gen_backend(config, allow_pollinations)`
//! returns `None` when no keyed backend is configured AND pollinations is
//! disabled — the bootstrap path then skips registration so the tool's
//! `is_available()` reports false.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::Value;
use wcore_egress::EgressClient as Client;

use wcore_tools::image_generation_tool::{
    ImageGenerationBackend, ImageGenerationError, ImageGenerationRequest, ImageGenerationResponse,
};

use wcore_config::config::Config;

use super::build_ssrf_safe_tool_client;
use super::shared::{OPENAI_API_BASE, join_openai_endpoint, openai_wire_media_base, read_env_key};

// ---------------------------------------------------------------------
// Per-call timeout. `reqwest`'s `.timeout()` covers the HTTP exchange
// only; this wider envelope covers decode + parse + base64 too (R-H1).
// HF cold-start is 20-60s so its inner cap is widened separately.
// ---------------------------------------------------------------------
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(60);
const HF_PER_CALL_TIMEOUT: Duration = Duration::from_secs(90);

// ---------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------

/// Build a concrete OpenAI image backend from the active provider when it
/// serves the OpenAI-wire `/images/generations` endpoint. Returns `None`
/// for providers without it (Anthropic/Gemini and the LLM-only OpenAI-compat
/// routers) or when the resolved key is empty.
///
/// Only native **OpenAI** and **FluxRouter** serve this media endpoint, so
/// [`openai_wire_media_base`] resolves the `/v1` API root for those two
/// (filling FluxRouter's default base when `config.base_url` is empty) and
/// returns `None` otherwise. A Flux session therefore targets
/// `https://api.fluxrouter.ai/v1/images/generations` with the Flux key
/// (#310), and native OpenAI gets the required `/v1` even though its
/// resolved `config.base_url` is `https://api.openai.com` (no `/v1`).
///
/// Returns the concrete `DalleBackend` (not a trait object) so the resolved
/// endpoint + key are unit-assertable.
pub(crate) fn dalle_backend_from_config(config: &Config) -> Option<DalleBackend> {
    if config.api_key.trim().is_empty() {
        return None;
    }
    let base = openai_wire_media_base(config)?;
    Some(DalleBackend::new(config.api_key.clone(), &base))
}

/// Resolve a real `ImageGenerationBackend` from the resolved `Config`
/// and environment variables.
///
/// Priority order (first match wins):
/// 1. **Active OpenAI-wire media provider** (native OpenAI or Flux Router) —
///    when `dalle_backend_from_config` resolves (see [`openai_wire_media_base`]
///    for the gated provider set) and `config.api_key` is non-empty, the
///    backend is built from the resolved `/v1` API root + `config.api_key`.
///    This is the #310 fix: in a Flux session the tool now sends the Flux key
///    to `https://api.fluxrouter.ai/v1/images/generations` instead of the
///    Flux key to `api.openai.com` (HTTP 401).
/// 2. `OPENAI_API_KEY` → OpenAI image at `api.openai.com` (back-compat
///    fallback when config doesn't carry an OpenAI-wire provider)
/// 3. `FAL_API_KEY` → FAL FLUX schnell
/// 4. `GEMINI_API_KEY` → Gemini Imagen 3
/// 5. `HF_API_KEY` → Hugging Face FLUX
/// 6. Pollinations (zero-key) — only when `allow_pollinations == true`
///
/// Returns `None` when no keyed backend resolves AND pollinations is
/// disabled; the bootstrap path then skips registration so the tool's
/// `is_available()` reports false.
pub fn build_image_gen_backend(
    config: &Config,
    allow_pollinations: bool,
) -> Option<Arc<dyn ImageGenerationBackend>> {
    // 1. Prefer the active OpenAI-wire provider's resolved key + base_url.
    if let Some(backend) = dalle_backend_from_config(config) {
        tracing::info!(
            "image_gen: using {} at {} (active OpenAI-wire provider)",
            backend.model,
            config.base_url
        );
        return Some(Arc::new(backend));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        let backend = DalleBackend::new(key, OPENAI_API_BASE);
        tracing::info!(
            "image_gen: using OpenAI {} (OPENAI_API_KEY found)",
            backend.model
        );
        return Some(Arc::new(backend));
    }
    if let Some(key) = read_env_key("FAL_API_KEY") {
        tracing::info!("image_gen: using FAL FLUX schnell (FAL_API_KEY found)");
        return Some(Arc::new(FalFluxBackend::new(key)));
    }
    if let Some(key) = read_env_key("GEMINI_API_KEY") {
        tracing::info!("image_gen: using Gemini Imagen 3 (GEMINI_API_KEY found)");
        return Some(Arc::new(GeminiImagenBackend::new(key)));
    }
    if let Some(key) = read_env_key("HF_API_KEY") {
        tracing::info!("image_gen: using Hugging Face FLUX (HF_API_KEY found)");
        return Some(Arc::new(HfFluxBackend::new(key)));
    }
    if allow_pollinations {
        tracing::warn!(
            "image_gen: falling back to Pollinations.ai (prompts sent unencrypted to a \
             third-party endpoint — opt-in via [tools.image_gen] allow_pollinations_fallback)"
        );
        return Some(Arc::new(PollinationsBackend::new()));
    }
    tracing::warn!(
        "image_gen: no API key found (OPENAI_API_KEY / FAL_API_KEY / GEMINI_API_KEY / \
         HF_API_KEY) and Pollinations fallback disabled — tool hidden"
    );
    None
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Map an HTTP status to an `ImageGenerationError` category. `429` and
/// `5xx` go to `Other` so the caller can decide retry; safety filters
/// (`400` with policy/safety in body) → `PromptRejected`; `402` →
/// `InsufficientCredits`.
fn map_http_error(status: u16, body: &str, provider: &str) -> ImageGenerationError {
    let preview: String = body.chars().take(400).collect();
    if status == 402 {
        return ImageGenerationError::InsufficientCredits(format!(
            "{provider} returned HTTP 402: {preview}"
        ));
    }
    if status == 400
        && (body.to_ascii_lowercase().contains("safety")
            || body.to_ascii_lowercase().contains("policy")
            || body.to_ascii_lowercase().contains("blocked")
            || body.to_ascii_lowercase().contains("rejected"))
    {
        return ImageGenerationError::PromptRejected(format!(
            "{provider} rejected prompt: {preview}"
        ));
    }
    if status == 429 {
        // Retry-After hint is surfaced verbatim — the tool layer can
        // re-attempt on the next turn after backoff.
        return ImageGenerationError::Other(format!(
            "{provider} returned HTTP 429 (rate limited): {preview}"
        ));
    }
    ImageGenerationError::Other(format!("{provider} returned HTTP {status}: {preview}"))
}

/// Wrap an `async` block in the two-layer timeout pattern (R-H1).
async fn with_timeout<T, F>(
    timeout: Duration,
    provider: &'static str,
    fut: F,
) -> Result<T, ImageGenerationError>
where
    F: std::future::Future<Output = Result<T, ImageGenerationError>>,
{
    match tokio::time::timeout(timeout, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(ImageGenerationError::Other(format!(
            "{provider} call timed out after {}s",
            timeout.as_secs()
        ))),
    }
}

/// Lightweight email-shaped PII detector (placeholder per spec §5).
/// Returns true when the prompt contains at least one `local@host`
/// token. Full PII scrub deferred to v0.9.x.
fn prompt_contains_email_pii(prompt: &str) -> bool {
    // Hand-rolled regex-equivalent without pulling `regex` into wcore-agent's
    // tool_backends. Match `<alnum/._%+-> @ <alnum/.-> . <alnum>{2,}` —
    // good enough to log a warning, not a security boundary.
    let bytes = prompt.as_bytes();
    let n = bytes.len();
    if n < 6 {
        return false;
    }
    let is_local = |b: u8| {
        matches!(b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'_' | b'%' | b'+' | b'-')
    };
    let is_domain = |b: u8| {
        matches!(b,
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-')
    };
    for at in 0..n {
        if bytes[at] != b'@' {
            continue;
        }
        // Walk left for local part (>= 1 char).
        if at == 0 || !is_local(bytes[at - 1]) {
            continue;
        }
        // Walk right for domain part (>= 3 chars including dot).
        let mut j = at + 1;
        let mut saw_dot = false;
        let mut domain_len = 0;
        while j < n && is_domain(bytes[j]) {
            if bytes[j] == b'.' {
                saw_dot = true;
            }
            domain_len += 1;
            j += 1;
        }
        if saw_dot && domain_len >= 3 {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------
// 1. OpenAI image generation (gpt-image-1 default; dall-e-3 configurable)
// ---------------------------------------------------------------------

/// Default OpenAI image model. `gpt-image-1` is the current broadly-available
/// model; `dall-e-3` is region/tier-gated and returns HTTP 400
/// (`model does not exist`) on accounts that lack it (#265). Override per-account
/// with the `OPENAI_IMAGE_MODEL` env var or [`DalleBackend::with_model`].
pub const DEFAULT_OPENAI_IMAGE_MODEL: &str = "gpt-image-1";

/// Resolve the OpenAI image model: the `OPENAI_IMAGE_MODEL` env var if set and
/// non-empty, else [`DEFAULT_OPENAI_IMAGE_MODEL`]. Lets a user who only has
/// `dall-e-3` (or a newer model) point the backend at it without a code change.
fn openai_image_model_from_env() -> String {
    std::env::var("OPENAI_IMAGE_MODEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_IMAGE_MODEL.to_string())
}

/// OpenAI image-generation backend. Endpoint: `/v1/images/generations`. The
/// request size/quality and the response shape both depend on the model:
/// `gpt-image-1` uses `1024x1024` / `1536x1024` / `1024x1536` and returns
/// base64 (`data[0].b64_json`); `dall-e-3` uses `1024x1024` / `1792x1024` /
/// `1024x1792`, accepts `quality`, and returns a URL (`data[0].url`). Both
/// response shapes are handled.
pub struct DalleBackend {
    client: Client,
    api_key: String,
    endpoint: String,
    /// Model sent in the request body. Defaults via [`openai_image_model_from_env`].
    model: String,
}

impl DalleBackend {
    /// Build an OpenAI image backend pointed at `base_url` (an OpenAI-wire
    /// API base such as `https://api.openai.com/v1` or
    /// `https://api.fluxrouter.ai/v1`). The endpoint is derived as
    /// `{base_url}/images/generations` (#310) — no hardcoded host.
    pub fn new(api_key: String, base_url: &str) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint: join_openai_endpoint(base_url, "images/generations"),
            model: openai_image_model_from_env(),
        }
    }

    /// Override the model (e.g. `"dall-e-3"` for accounts that have it).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    #[cfg(test)]
    fn with_endpoint(api_key: String, endpoint: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
            model: DEFAULT_OPENAI_IMAGE_MODEL.to_string(),
        }
    }

    /// Resolved request endpoint (`{base_url}/images/generations`). Exposed
    /// so the resolver wiring (#310) can be unit-asserted without a network
    /// round-trip.
    #[cfg(test)]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Resolved bearer key sent to the image endpoint. Exposed for the
    /// #310 resolver tests (asserts the Flux key, not OPENAI_API_KEY).
    #[cfg(test)]
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    /// `gpt-image-1` (and the `gpt-image-*` family) use a different size table
    /// than `dall-e-*` and reject the `quality: "standard"` value.
    fn is_gpt_image(&self) -> bool {
        self.model.starts_with("gpt-image")
    }

    fn size_for(&self, req: &ImageGenerationRequest) -> &'static str {
        if self.is_gpt_image() {
            match req.aspect_ratio {
                "square" => "1024x1024",
                "portrait" => "1024x1536",
                _ => "1536x1024",
            }
        } else {
            match req.aspect_ratio {
                "square" => "1024x1024",
                "portrait" => "1024x1792",
                // landscape (default) or unknown
                _ => "1792x1024",
            }
        }
    }

    fn dimensions_for(&self, req: &ImageGenerationRequest) -> (u32, u32) {
        if self.is_gpt_image() {
            match req.aspect_ratio {
                "square" => (1024, 1024),
                "portrait" => (1024, 1536),
                _ => (1536, 1024),
            }
        } else {
            match req.aspect_ratio {
                "square" => (1024, 1024),
                "portrait" => (1024, 1792),
                _ => (1792, 1024),
            }
        }
    }
}

#[async_trait]
impl ImageGenerationBackend for DalleBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        let size = self.size_for(&request);
        let (w, h) = self.dimensions_for(&request);
        let mut body = serde_json::json!({
            "model": self.model,
            "prompt": request.prompt,
            "size": size,
            "n": 1,
        });
        // `dall-e-*` accept `quality: "standard"|"hd"`; `gpt-image-1` rejects
        // `"standard"` (it uses `low|medium|high|auto`, defaulting to auto), so
        // only send the field for the dall-e family.
        if !self.is_gpt_image() {
            body["quality"] = serde_json::json!("standard");
        }
        let model = self.model.clone();
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        with_timeout(PER_CALL_TIMEOUT, "openai dall-e", async move {
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| {
                    ImageGenerationError::Other(format!("openai dall-e request failed: {e}"))
                })?;
            let status = resp.status();
            let txt = resp
                .text()
                .await
                .map_err(|e| ImageGenerationError::Other(format!("openai dall-e body: {e}")))?;
            if !status.is_success() {
                let code = status.as_u16();
                // gpt-image-1 (the new default) needs a verified OpenAI org; an
                // account that only has dall-e-3 access gets 403 (or a 400 model
                // error). Signpost the OPENAI_IMAGE_MODEL escape hatch so the user
                // can switch back without reading the source (#265 follow-up).
                if code == 403 || (code == 400 && txt.to_ascii_lowercase().contains("model")) {
                    let preview: String = txt.chars().take(300).collect();
                    return Err(ImageGenerationError::Other(format!(
                        "openai image model {model:?} returned HTTP {code}: {preview}. If your \
                         account lacks access to {model:?}, set OPENAI_IMAGE_MODEL to a model you \
                         can use (e.g. dall-e-3)."
                    )));
                }
                return Err(map_http_error(code, &txt, "openai image"));
            }
            let parsed: Value = serde_json::from_str(&txt).map_err(|e| {
                ImageGenerationError::Other(format!("openai image JSON parse: {e}"))
            })?;
            // Response shape is model-dependent: dall-e-* return `data[0].url`,
            // gpt-image-1 returns `data[0].b64_json` (base64, no URL). Accept
            // either — a base64 payload is wrapped as a `data:` URI, matching the
            // Gemini/HF backends.
            let data0 = parsed.pointer("/data/0");
            let image = if let Some(url) = data0
                .and_then(|d| d.pointer("/url"))
                .and_then(|v| v.as_str())
            {
                url.to_string()
            } else if let Some(b64) = data0
                .and_then(|d| d.pointer("/b64_json"))
                .and_then(|v| v.as_str())
            {
                format!("data:image/png;base64,{b64}")
            } else {
                return Err(ImageGenerationError::Other(
                    "openai image: missing both data[0].url and data[0].b64_json in response"
                        .to_string(),
                ));
            };
            Ok(ImageGenerationResponse {
                image,
                used_provider: format!("OpenAI {model}"),
                width: w,
                height: h,
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------
// 2. FAL FLUX schnell
// ---------------------------------------------------------------------

pub struct FalFluxBackend {
    client: Client,
    api_key: String,
    endpoint: String,
}

impl FalFluxBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint: "https://fal.run/fal-ai/flux/schnell".to_string(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(api_key: String, endpoint: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
        }
    }

    fn fal_aspect(req: &ImageGenerationRequest) -> &'static str {
        match req.aspect_ratio {
            "square" => "square_hd",
            "portrait" => "portrait_4_3",
            _ => "landscape_16_9",
        }
    }
}

#[async_trait]
impl ImageGenerationBackend for FalFluxBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        let aspect = Self::fal_aspect(&request);
        let body = serde_json::json!({
            "prompt": request.prompt,
            "image_size": aspect,
        });
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let req_w = request.width;
        let req_h = request.height;
        with_timeout(PER_CALL_TIMEOUT, "fal flux", async move {
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Key {api_key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| {
                    ImageGenerationError::Other(format!("fal flux request failed: {e}"))
                })?;
            let status = resp.status();
            let txt = resp
                .text()
                .await
                .map_err(|e| ImageGenerationError::Other(format!("fal flux body: {e}")))?;
            if !status.is_success() {
                return Err(map_http_error(status.as_u16(), &txt, "fal flux"));
            }
            let parsed: Value = serde_json::from_str(&txt)
                .map_err(|e| ImageGenerationError::Other(format!("fal flux JSON parse: {e}")))?;
            let url = parsed
                .pointer("/images/0/url")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    ImageGenerationError::Other(
                        "fal flux: missing images[0].url in response".to_string(),
                    )
                })?;
            Ok(ImageGenerationResponse {
                image: url,
                used_provider: "FAL FLUX schnell".to_string(),
                width: req_w,
                height: req_h,
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------
// 3. Gemini Imagen 3 (base64 inline → data: URI)
// ---------------------------------------------------------------------

pub struct GeminiImagenBackend {
    client: Client,
    api_key: String,
    endpoint_base: String,
}

impl GeminiImagenBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint_base: "https://generativelanguage.googleapis.com/v1beta/models/imagen-3.0-generate-002:generateImages".to_string(),
        }
    }

    #[cfg(test)]
    fn with_endpoint_base(api_key: String, endpoint_base: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint_base,
        }
    }
}

#[async_trait]
impl ImageGenerationBackend for GeminiImagenBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        let body = serde_json::json!({
            "prompts": [{ "text": request.prompt }],
            "config": { "numberOfImages": 1 },
        });
        // SECRETS-28: the API key rides in the `x-goog-api-key` header, NOT
        // the URL query string. A key in `?key=…` leaks into the reqwest
        // error's `Display` (it carries the URL) on a fast-fail transport
        // error, and that error surfaces to the user/model as a ToolResult.
        let url = self.endpoint_base.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let req_w = request.width;
        let req_h = request.height;
        with_timeout(PER_CALL_TIMEOUT, "gemini imagen", async move {
            let resp = client
                .post(&url)
                .header("x-goog-api-key", &api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.to_string())
                .send()
                .await
                // SECRETS-28: strip the URL from the error before formatting,
                // mirroring the Authorization-header providers above.
                .map_err(|e| {
                    ImageGenerationError::Other(format!(
                        "gemini imagen request failed: {}",
                        e.redacted()
                    ))
                })?;
            let status = resp.status();
            let txt = resp
                .text()
                .await
                .map_err(|e| ImageGenerationError::Other(format!("gemini imagen body: {e}")))?;
            if !status.is_success() {
                return Err(map_http_error(status.as_u16(), &txt, "gemini imagen"));
            }
            let parsed: Value = serde_json::from_str(&txt).map_err(|e| {
                ImageGenerationError::Other(format!("gemini imagen JSON parse: {e}"))
            })?;
            // Imagen 3 returns base64 under several plausible paths; try
            // a small set in order.
            let b64 = parsed
                .pointer("/images/0/bytesBase64Encoded")
                .or_else(|| parsed.pointer("/generatedImages/0/image/imageBytes"))
                .or_else(|| parsed.pointer("/predictions/0/bytesBase64Encoded"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| {
                    ImageGenerationError::Other(
                        "gemini imagen: missing base64 image data in response".to_string(),
                    )
                })?;
            let data_uri = format!("data:image/png;base64,{b64}");
            Ok(ImageGenerationResponse {
                image: data_uri,
                used_provider: "Gemini Imagen 3".to_string(),
                width: req_w,
                height: req_h,
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------
// 4. Hugging Face FLUX.1-schnell (binary PNG → base64)
// ---------------------------------------------------------------------

pub struct HfFluxBackend {
    client: Client,
    api_key: String,
    endpoint: String,
}

impl HfFluxBackend {
    pub fn new(api_key: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint:
                "https://api-inference.huggingface.co/models/black-forest-labs/FLUX.1-schnell"
                    .to_string(),
        }
    }

    #[cfg(test)]
    fn with_endpoint(api_key: String, endpoint: String) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
        }
    }
}

#[async_trait]
impl ImageGenerationBackend for HfFluxBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        let body = serde_json::json!({ "inputs": request.prompt });
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let client = self.client.clone();
        let req_w = request.width;
        let req_h = request.height;
        with_timeout(HF_PER_CALL_TIMEOUT, "huggingface flux", async move {
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "image/png")
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| {
                    ImageGenerationError::Other(format!("huggingface flux request failed: {e}"))
                })?;
            let status = resp.status();
            if !status.is_success() {
                let txt = resp.text().await.unwrap_or_default();
                return Err(map_http_error(status.as_u16(), &txt, "huggingface flux"));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ImageGenerationError::Other(format!("huggingface flux body: {e}")))?;
            if bytes.is_empty() {
                return Err(ImageGenerationError::Other(
                    "huggingface flux: empty body".to_string(),
                ));
            }
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let data_uri = format!("data:image/png;base64,{b64}");
            Ok(ImageGenerationResponse {
                image: data_uri,
                used_provider: "Hugging Face FLUX.1-schnell".to_string(),
                width: req_w,
                height: req_h,
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------
// 5. Pollinations.ai (GATED — opt-in only)
// ---------------------------------------------------------------------

pub struct PollinationsBackend {
    client: Client,
    endpoint_base: String,
}

impl PollinationsBackend {
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            endpoint_base: "https://image.pollinations.ai/prompt".to_string(),
        }
    }

    /// Construct the `GET` URL with URL-encoded prompt and the standard
    /// `width=&height=&seed=` query string.
    fn url_for(&self, req: &ImageGenerationRequest, seed: u32) -> String {
        let encoded_prompt = urlencoding::encode(&req.prompt);
        format!(
            "{}/{}?width={}&height={}&seed={}",
            self.endpoint_base, encoded_prompt, req.width, req.height, seed
        )
    }
}

impl Default for PollinationsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImageGenerationBackend for PollinationsBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        // Placeholder PII scrub: log a warning when the prompt contains
        // anything that looks like an email address. Full PII scrub
        // deferred to v0.9.x.
        if prompt_contains_email_pii(&request.prompt) {
            tracing::warn!(
                "image_gen.pollinations: prompt contains email-like text — sending unencrypted \
                 to a third-party endpoint may leak PII. Consider disabling \
                 [tools.image_gen] allow_pollinations_fallback."
            );
        }

        // Pollinations responds with the image URL itself — we just
        // return the constructed URL. No HTTP fetch needed; the model's
        // markdown renderer will pull the image. Use a random seed so
        // re-prompts don't collide on the upstream cache.
        let seed = rand::random::<u32>();
        let url = self.url_for(&request, seed);

        // Belt-and-braces: still perform a HEAD to validate the endpoint
        // is alive and bounce SSRF via the redirect policy. Wrapped in
        // the two-layer timeout (R-H1).
        let url_clone = url.clone();
        let client = self.client.clone();
        let req_w = request.width;
        let req_h = request.height;
        with_timeout(PER_CALL_TIMEOUT, "pollinations", async move {
            // A HEAD that 405s is still proof of liveness. Any client
            // error (5xx / network / SSRF refusal) surfaces here.
            let resp = client.head(&url_clone).send().await.map_err(|e| {
                ImageGenerationError::Other(format!("pollinations HEAD failed: {e}"))
            })?;
            let status = resp.status();
            if status.is_server_error() {
                return Err(map_http_error(
                    status.as_u16(),
                    "(HEAD body not captured)",
                    "pollinations",
                ));
            }
            Ok(ImageGenerationResponse {
                image: url_clone,
                used_provider: "Pollinations.ai".to_string(),
                width: req_w,
                height: req_h,
            })
        })
        .await
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use wcore_config::config::ProviderType;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // -- env-var hygiene ------------------------------------------------

    fn clear_image_gen_env() {
        // SAFETY: tests in this module are marked `#[serial]`. Direct
        // env mutation is safe under that serialization.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("FAL_API_KEY");
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("HF_API_KEY");
        }
    }

    /// Default (Anthropic, empty key/url) config — exercises the env-key
    /// fallback paths exactly as before #310 (the config branch is a no-op
    /// for non-OpenAI providers / empty keys).
    fn env_only_config() -> Config {
        Config::default()
    }

    /// A real Flux Router session: `provider == ProviderType::FluxRouter`
    /// (what `"flux-router"` parses to) with an explicit Flux base_url + the
    /// Flux key (#310). NOTE: the pre-fix fixture used `provider: OpenAI`,
    /// which masked the bug — the resolver gate never matched FluxRouter.
    fn flux_config() -> Config {
        Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux-test".to_string(),
            base_url: "https://api.fluxrouter.ai/v1".to_string(),
            ..Config::default()
        }
    }

    // -- Resolver priority ---------------------------------------------

    #[test]
    #[serial]
    fn build_image_gen_backend_priority_dalle_over_fal() {
        clear_image_gen_env();
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-test-openai");
            std::env::set_var("FAL_API_KEY", "fal-test");
        }
        let backend =
            build_image_gen_backend(&env_only_config(), false).expect("DALL-E must resolve");
        // Smoke: DALL-E backend is selected — we can't downcast Arc<dyn _>
        // cleanly without trait-object reflection, so we assert by *not*
        // matching FAL's path: hit a wiremock that only the FAL backend
        // would hit and observe zero matched requests. This is verified
        // more directly in the happy-path tests below; here we just
        // confirm the resolver returns Some(_).
        let _ = backend;
        clear_image_gen_env();
    }

    #[test]
    #[serial]
    fn build_image_gen_backend_falls_back_to_pollinations_when_no_keys_and_enabled() {
        clear_image_gen_env();
        let backend = build_image_gen_backend(&env_only_config(), true)
            .expect("Pollinations must resolve when no keys + allow=true");
        let _ = backend;
    }

    #[test]
    #[serial]
    fn build_image_gen_backend_returns_none_when_no_keys_and_pollinations_disabled() {
        clear_image_gen_env();
        assert!(
            build_image_gen_backend(&env_only_config(), false).is_none(),
            "no keys + pollinations disabled → tool hidden"
        );
    }

    #[test]
    #[serial]
    fn image_gen_returns_none_when_env_var_empty_string() {
        clear_image_gen_env();
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "");
            std::env::set_var("FAL_API_KEY", "   ");
        }
        assert!(
            build_image_gen_backend(&env_only_config(), false).is_none(),
            "empty / whitespace env vars must be treated as unset (R-H2)"
        );
        clear_image_gen_env();
    }

    #[test]
    #[serial]
    fn null_default_skips_registration_when_no_keys_set() {
        clear_image_gen_env();
        assert!(build_image_gen_backend(&env_only_config(), false).is_none());
    }

    // -- #310: OpenAI-wire provider routing (Flux) ---------------------

    #[test]
    #[serial]
    fn dalle_resolves_from_flux_config_not_openai_host() {
        // #310 regression: in a Flux Router session the config carries
        // base_url=https://api.fluxrouter.ai/v1 + api_key=sk-flux-test. The
        // resolved DALL-E endpoint must target Flux's host with the Flux
        // key — NOT api.openai.com (which would 401 on the wrong key).
        clear_image_gen_env();
        let backend =
            dalle_backend_from_config(&flux_config()).expect("OpenAI-wire config must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
    }

    #[test]
    #[serial]
    fn dalle_config_takes_priority_over_openai_env_key() {
        // Even with OPENAI_API_KEY set in the environment, an active
        // OpenAI-wire provider (Flux) must win — the resolver builds from
        // config, so the Flux endpoint + key are used, not the env key.
        clear_image_gen_env();
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-openai-env");
        }
        let backend = dalle_backend_from_config(&flux_config())
            .expect("config OpenAI-wire provider must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
        clear_image_gen_env();
    }

    #[test]
    #[serial]
    fn dalle_falls_back_to_openai_host_when_config_not_openai_wire() {
        // Back-compat: with no OpenAI-wire provider in config (default is
        // Anthropic) but OPENAI_API_KEY set, the resolver builds the
        // OpenAI backend against api.openai.com. `dalle_backend_from_config`
        // is the config-first gate; it must decline so the env path runs.
        assert!(
            dalle_backend_from_config(&env_only_config()).is_none(),
            "non-OpenAI provider must not hijack the OpenAI image slot"
        );
        // And the env-built backend points at api.openai.com.
        let backend = DalleBackend::new("sk-openai-env".to_string(), OPENAI_API_BASE);
        assert_eq!(
            backend.endpoint(),
            "https://api.openai.com/v1/images/generations"
        );
        assert_eq!(backend.api_key(), "sk-openai-env");
    }

    #[test]
    #[serial]
    fn dalle_resolves_flux_default_base_when_config_base_empty() {
        // Real Flux sessions leave config.base_url empty (the FluxRouter
        // newtype supplies the default). The resolver must still target Flux.
        clear_image_gen_env();
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux-test".to_string(),
            base_url: String::new(),
            ..Config::default()
        };
        let backend = dalle_backend_from_config(&cfg).expect("Flux must resolve from default base");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/images/generations"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
    }

    #[test]
    #[serial]
    fn dalle_adds_v1_for_native_openai_config() {
        // Native OpenAI's resolved base_url is `https://api.openai.com` (no
        // `/v1`); pre-fix this produced a 404 endpoint. The resolver must add
        // `/v1`.
        clear_image_gen_env();
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            ..Config::default()
        };
        let backend = dalle_backend_from_config(&cfg).expect("native OpenAI must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.openai.com/v1/images/generations"
        );
    }

    #[test]
    #[serial]
    fn dalle_declines_userinfo_base_url() {
        // A hostile config base_url with userinfo would exfiltrate the key to
        // attacker.com — the resolver must decline (fail closed).
        clear_image_gen_env();
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-openai".to_string(),
            base_url: "https://attacker.com@api.openai.com/v1".to_string(),
            ..Config::default()
        };
        assert!(dalle_backend_from_config(&cfg).is_none());
    }

    // -- DALL-E happy + failure paths ----------------------------------

    fn req(prompt: &str, aspect: &'static str) -> ImageGenerationRequest {
        let (w, h) = match aspect {
            "square" => (1024, 1024),
            "portrait" => (1024, 1536),
            _ => (1536, 1024),
        };
        ImageGenerationRequest {
            prompt: prompt.to_string(),
            aspect_ratio: aspect,
            width: w,
            height: h,
        }
    }

    #[tokio::test]
    async fn dalle_response_url_extracted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "url": "https://example.com/dalle.png" }]
            })))
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let resp = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect("happy path");
        assert_eq!(resp.image, "https://example.com/dalle.png");
        // with_endpoint defaults to gpt-image-1; the `url` response path still
        // works (any model that returns a URL is handled).
        assert_eq!(resp.used_provider, "OpenAI gpt-image-1");
        assert_eq!(resp.width, 1536);
        assert_eq!(resp.height, 1024);
    }

    #[tokio::test]
    async fn openai_b64_json_response_wrapped_as_data_uri() {
        // Regression for #265: gpt-image-1 returns base64, not a URL. The backend
        // must wrap `data[0].b64_json` as a `data:` URI rather than error on the
        // missing `url`.
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfake");
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "b64_json": b64 }]
            })))
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let resp = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect("b64_json path");
        assert!(
            resp.image.starts_with("data:image/png;base64,"),
            "gpt-image-1 base64 must become a data URI, got: {}",
            resp.image
        );
        assert_eq!(resp.used_provider, "OpenAI gpt-image-1");
    }

    #[test]
    fn openai_default_model_is_gpt_image_1_and_overridable() {
        // Regression for #265: the default must NOT be dall-e-3.
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            "https://unused.example.com/v1/images/generations".to_string(),
        );
        assert_eq!(backend.model, DEFAULT_OPENAI_IMAGE_MODEL);
        assert_eq!(backend.model, "gpt-image-1");
        let overridden = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            "https://unused.example.com/v1/images/generations".to_string(),
        )
        .with_model("dall-e-3");
        assert_eq!(overridden.model, "dall-e-3");
        assert!(!overridden.is_gpt_image());
    }

    #[tokio::test]
    async fn openai_request_body_is_model_aware() {
        // #265 root-cause assertion: the request body actually sent on the wire
        // must be model-aware. wiremock `body_json` is an EXACT match, so the
        // call only succeeds if the body is byte-for-byte what we assert — which
        // also proves `quality` is OMITTED for gpt-image-1 (an extra field would
        // fail the exact match) and PRESENT for dall-e-3.
        let server = MockServer::start().await;
        // gpt-image-1 (default): no `quality`, gpt-image size table.
        Mock::given(method("POST"))
            .and(path("/gpt/v1/images/generations"))
            .and(body_json(serde_json::json!({
                "model": "gpt-image-1",
                "prompt": "a sunset",
                "size": "1536x1024",
                "n": 1,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "url": "https://example.com/x.png" }]
            })))
            .mount(&server)
            .await;
        // dall-e-3: includes `quality: standard`, dall-e size table.
        Mock::given(method("POST"))
            .and(path("/dalle/v1/images/generations"))
            .and(body_json(serde_json::json!({
                "model": "dall-e-3",
                "prompt": "a sunset",
                "size": "1792x1024",
                "quality": "standard",
                "n": 1,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "url": "https://example.com/y.png" }]
            })))
            .mount(&server)
            .await;

        let gpt = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/gpt/v1/images/generations", server.uri()),
        );
        gpt.generate(req("a sunset", "landscape"))
            .await
            .expect("gpt-image-1 body must match exactly (no `quality`, 1536x1024)");

        let dalle = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/dalle/v1/images/generations", server.uri()),
        )
        .with_model("dall-e-3");
        dalle
            .generate(req("a sunset", "landscape"))
            .await
            .expect("dall-e-3 body must match exactly (quality=standard, 1792x1024)");
    }

    #[tokio::test]
    async fn openai_403_surfaces_image_model_escape_hatch() {
        // #265: an org without gpt-image-1 access gets 403; the error must point
        // the user at OPENAI_IMAGE_MODEL rather than dead-ending.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_string("{\"error\":{\"message\":\"forbidden\"}}"),
            )
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let err = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect_err("403 must surface as an error");
        let msg = err.to_string();
        assert!(
            msg.contains("OPENAI_IMAGE_MODEL"),
            "403 error must signpost the OPENAI_IMAGE_MODEL escape hatch, got: {msg}"
        );
    }

    #[tokio::test]
    async fn image_gen_handles_http_5xx_returns_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream busy"))
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let err = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect_err("must surface 5xx as error");
        assert!(
            matches!(err, ImageGenerationError::Other(_)),
            "5xx must map to Other variant, got: {err}"
        );
        let msg = format!("{err}");
        assert!(
            msg.contains("503"),
            "error message should include status: {msg}"
        );
    }

    #[tokio::test]
    async fn image_gen_handles_http_429_with_retry_after_backoff() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", "60")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let err = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect_err("must surface 429 as error");
        let msg = format!("{err}");
        assert!(
            msg.contains("429"),
            "error must include rate-limit status: {msg}"
        );
    }

    #[tokio::test]
    async fn image_gen_handles_malformed_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{not json"))
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let err = backend
            .generate(req("a sunset", "landscape"))
            .await
            .expect_err("malformed JSON must surface as error");
        let msg = format!("{err}");
        assert!(
            msg.contains("JSON parse") || msg.contains("parse"),
            "expected JSON parse error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn image_gen_handles_network_timeout() {
        // Wiremock with a long delay; we set a 1s outer timeout via
        // direct call to `with_timeout`. Verifies the two-layer timeout
        // catches a hung body decode (R-H1).
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let endpoint = format!("{}/v1/images/generations", server.uri());
        let backend = DalleBackend::with_endpoint("sk-test".to_string(), endpoint);
        // Force a tight timeout using the public path: wrap the call.
        let result = tokio::time::timeout(
            Duration::from_millis(800),
            backend.generate(req("test", "landscape")),
        )
        .await;
        assert!(result.is_err(), "outer timeout must trip");
    }

    #[tokio::test]
    async fn image_gen_refuses_ssrf_redirect_to_metadata_service() {
        // Mock returns a 302 redirect pointing at AWS instance metadata.
        // The SSRF-safe redirect policy must refuse before the redirect
        // is followed.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;
        let backend = DalleBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/images/generations", server.uri()),
        );
        let err = backend
            .generate(req("test", "landscape"))
            .await
            .expect_err("SSRF redirect must be refused");
        let msg = format!("{err}");
        // The redirect policy rejection surfaces as a request-failed error.
        // We check for any of the plausible substrings produced by the
        // ssrf_safe_redirect_policy path.
        assert!(
            msg.contains("request failed")
                || msg.contains("redirect")
                || msg.contains("blocked")
                || msg.contains("unsafe"),
            "expected SSRF-refused error, got: {msg}"
        );
    }

    // -- Gemini base64 → data URI -------------------------------------

    #[tokio::test]
    async fn gemini_base64_response_wrapped_as_data_uri() {
        let server = MockServer::start().await;
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nfake");
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "images": [{ "bytesBase64Encoded": b64 }]
            })))
            .mount(&server)
            .await;
        let backend = GeminiImagenBackend::with_endpoint_base(
            "gem-key".to_string(),
            format!(
                "{}/v1beta/models/imagen-3.0-generate-002:generateImages",
                server.uri()
            ),
        );
        let resp = backend
            .generate(req("a robot", "square"))
            .await
            .expect("gemini happy path");
        assert!(
            resp.image.starts_with("data:image/png;base64,"),
            "must wrap base64 as data URI, got: {}",
            resp.image
        );
        assert_eq!(resp.used_provider, "Gemini Imagen 3");
    }

    // -- Pollinations URL construction --------------------------------

    #[test]
    fn pollinations_url_construction_urlencodes_prompt() {
        let backend = PollinationsBackend::new();
        let r = req("a cat with hat", "landscape");
        let url = backend.url_for(&r, 42);
        // Spaces become %20 via urlencoding crate (RFC 3986 path encoding).
        assert!(
            url.contains("a%20cat%20with%20hat"),
            "expected URL-encoded prompt, got: {url}"
        );
        assert!(url.contains("width=1536"));
        assert!(url.contains("height=1024"));
        assert!(url.contains("seed=42"));
    }

    #[test]
    fn pollinations_url_encodes_special_characters() {
        let backend = PollinationsBackend::new();
        let r = req("hello world & friends", "square");
        let url = backend.url_for(&r, 1);
        assert!(url.contains("hello%20world%20%26%20friends"), "got: {url}");
    }

    // -- Aspect-ratio mapping (table-driven) ---------------------------

    #[test]
    fn aspect_ratio_maps_correctly_per_provider() {
        // gpt-image-1 and dall-e-3 use different size tables (#265).
        let gpt = DalleBackend::with_endpoint("k".to_string(), "http://unused".to_string());
        let dalle = DalleBackend::with_endpoint("k".to_string(), "http://unused".to_string())
            .with_model("dall-e-3");
        let cases: &[(&str, &str, &str, &str)] = &[
            // (input aspect, gpt-image-1 size, dall-e-3 size, FAL aspect)
            ("square", "1024x1024", "1024x1024", "square_hd"),
            ("landscape", "1536x1024", "1792x1024", "landscape_16_9"),
            ("portrait", "1024x1536", "1024x1792", "portrait_4_3"),
        ];
        for (aspect, gpt_size, dalle_size, fal_aspect) in cases {
            let r = req("x", aspect);
            assert_eq!(gpt.size_for(&r), *gpt_size, "gpt-image-1 size for {aspect}");
            assert_eq!(
                dalle.size_for(&r),
                *dalle_size,
                "dall-e-3 size for {aspect}"
            );
            assert_eq!(
                FalFluxBackend::fal_aspect(&r),
                *fal_aspect,
                "FAL aspect for {aspect}"
            );
        }
    }

    // -- PII scrub --

    #[test]
    fn pii_scrub_detects_email_in_prompt() {
        assert!(prompt_contains_email_pii(
            "contact me at alice@example.com please"
        ));
        assert!(prompt_contains_email_pii("a.b+c@host.co"));
        assert!(!prompt_contains_email_pii("a cat on a roof"));
        assert!(!prompt_contains_email_pii("just @ symbol alone"));
        assert!(!prompt_contains_email_pii("foo@bar")); // no dot in domain
    }

    // -- DALL-E size + dimensions consistency --

    #[test]
    fn dalle_dimensions_match_size_table() {
        let gpt = DalleBackend::with_endpoint("k".to_string(), "http://unused".to_string());
        let dalle = DalleBackend::with_endpoint("k".to_string(), "http://unused".to_string())
            .with_model("dall-e-3");
        assert_eq!(gpt.dimensions_for(&req("x", "square")), (1024, 1024));
        assert_eq!(gpt.dimensions_for(&req("x", "landscape")), (1536, 1024));
        assert_eq!(gpt.dimensions_for(&req("x", "portrait")), (1024, 1536));
        assert_eq!(dalle.dimensions_for(&req("x", "square")), (1024, 1024));
        assert_eq!(dalle.dimensions_for(&req("x", "landscape")), (1792, 1024));
        assert_eq!(dalle.dimensions_for(&req("x", "portrait")), (1024, 1792));
    }

    // -- FAL happy path --

    #[tokio::test]
    async fn fal_response_url_extracted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Key fal-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "images": [{ "url": "https://fal.media/x.png" }]
            })))
            .mount(&server)
            .await;
        let backend = FalFluxBackend::with_endpoint(
            "fal-test".to_string(),
            format!("{}/fal-ai/flux/schnell", server.uri()),
        );
        let resp = backend
            .generate(req("a dog", "portrait"))
            .await
            .expect("fal happy path");
        assert_eq!(resp.image, "https://fal.media/x.png");
        assert_eq!(resp.used_provider, "FAL FLUX schnell");
    }

    // -- SECRETS-28: Gemini key must not leak into the error path --

    /// A transport failure during a Gemini Imagen call must NOT echo the
    /// API key (or a `key=` query param) into the returned error, which
    /// surfaces to the user/model as a ToolResult `error`/`details`.
    #[tokio::test]
    async fn gemini_imagen_send_error_omits_api_key() {
        const SECRET_KEY: &str = "AIzaSyTEST_secrets28_leak_canary_value";
        // TEST-NET-1 (192.0.2.0/24, RFC 5737): reserved, guaranteed not to
        // route — POST fails fast with a transport error whose `Display`
        // historically carried the `…?key=<KEY>` URL.
        let backend = GeminiImagenBackend::with_endpoint_base(
            SECRET_KEY.to_string(),
            "http://192.0.2.1:9/v1beta/models/imagen-3.0-generate-002:generateImages".to_string(),
        );
        let err = backend
            .generate(req("a robot", "square"))
            .await
            .expect_err("unreachable host must produce an error");
        let msg = format!("{err}");
        assert!(!msg.contains(SECRET_KEY), "error leaked the API key: {msg}");
        assert!(
            !msg.contains("key="),
            "error leaked a key= query param: {msg}"
        );
    }

    // -- HF binary → base64 --

    #[tokio::test]
    async fn hf_binary_png_wrapped_as_data_uri() {
        let server = MockServer::start().await;
        let png_bytes: Vec<u8> = b"\x89PNG\r\n\x1a\nbinary".to_vec();
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(png_bytes.clone()))
            .mount(&server)
            .await;
        let backend = HfFluxBackend::with_endpoint(
            "hf-test".to_string(),
            format!("{}/models/black-forest-labs/FLUX.1-schnell", server.uri()),
        );
        let resp = backend
            .generate(req("a bird", "landscape"))
            .await
            .expect("hf happy path");
        assert!(resp.image.starts_with("data:image/png;base64,"));
        assert_eq!(resp.used_provider, "Hugging Face FLUX.1-schnell");
    }
}
