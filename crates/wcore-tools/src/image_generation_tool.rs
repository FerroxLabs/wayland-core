//! T3-3.6 (sub-wave 6): `image_generate` tool — text-to-image generation.
//!
//! Ported from the prior Wayland Python engine.
//!
//! Sends a text prompt to a multimodal image-generation backend through a
//! **pluggable [`ImageGenerationBackend`]**. The engine binds the real
//! backend at construction time (typically wired in `wcore-agent` to the
//! provider chain Google Imagen → OpenAI gpt-image-1/DALL-E 3 → FAL
//! FLUX 2 Pro); this crate ships a [`NullImageGenerationBackend`] that
//! **fails loud** rather than silently returning a stub — matching the
//! seam pattern used in `vision_tools` and `video_analyze_tool`.
//!
//! ## Behaviour preserved from the Python original
//!
//! * Required `prompt` (non-empty string). Empty / whitespace-only is an
//!   immediate user-facing error before the backend is consulted.
//! * Optional `aspect_ratio` enum: `"landscape"` (default), `"square"`,
//!   `"portrait"`. Any other string normalizes back to `"landscape"`
//!   (matches `_normalize_aspect` in Python).
//! * Output JSON shape:
//!
//!   ```json
//!   {
//!     "success": true,
//!     "image": "<url-or-data-url>",
//!     "freeFallbackUsed": false,
//!     "usedProvider": "Google Imagen" | "OpenAI gpt-image-1" | ...,
//!     "width": 1536,
//!     "height": 1024
//!   }
//!   ```
//!
//!   On failure: `{"success": false, "image": null, "error": "...",
//!   "freeFallbackUsed": false}`.
//!
//! ## Differences vs Python
//!
//! * **No embedded HTTP client.** The Python original embeds `httpx`
//!   calls to Google Imagen + OpenAI + FAL. The engine port keeps
//!   `wcore-tools` HTTP-free — the backend implementation (host-wired)
//!   owns provider selection and HTTP transport. The tool still
//!   validates inputs and (optionally) SSRF-checks any URL returned by
//!   the backend before forwarding to the model.
//! * **No environment variable inspection.** Provider selection
//!   (`_select_provider`) lives entirely in the backend; the engine
//!   never reads `GOOGLE_API_KEY` / `OPENAI_API_KEY` / `FAL_KEY`.
//! * **Hard cap on returned payload size.** If the backend returns a
//!   `data:` URL, its decoded byte length is bounded by
//!   [`MAX_IMAGE_PAYLOAD_BYTES`] (10 MB). Non-data URLs are bounded by
//!   the same length on the URL string itself, defending against
//!   pathological backends that return megabyte-long URLs.
//! * **No filesystem I/O.** The Python version's debug-helper JSON dump
//!   is intentionally omitted — call-site debug logging is the host's
//!   responsibility.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;

/// Hard cap on the returned image payload — applied to either the
/// decoded bytes of a `data:` URL or the length of a remote URL string.
/// 10 MB matches the Anthropic vision image upload ceiling for the
/// most common provider in the chain (gpt-image-1 returns base64).
pub const MAX_IMAGE_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

/// Default aspect ratio when callers omit it or supply something invalid.
/// Mirrors `DEFAULT_ASPECT_RATIO` in the Python original.
pub const DEFAULT_ASPECT_RATIO: &str = "landscape";

/// Aspect-ratio label accepted by the tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    Landscape,
    Square,
    Portrait,
}

impl AspectRatio {
    /// Normalize a user-supplied aspect ratio. Mirrors `_normalize_aspect`
    /// — unknown values silently fall back to `Landscape`.
    pub fn normalize(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s == "square" => Self::Square,
            Some(s) if s == "portrait" => Self::Portrait,
            // Empty, "landscape", or anything else.
            _ => Self::Landscape,
        }
    }

    /// `(width, height)` in pixels — mirrors `ASPECT_RATIO_DIMENSIONS`.
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Landscape => (1536, 1024),
            Self::Square => (1024, 1024),
            Self::Portrait => (1024, 1536),
        }
    }

    /// String label for the JSON schema enum + backend wire format.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Square => "square",
            Self::Portrait => "portrait",
        }
    }
}

/// Validated request handed to the backend.
#[derive(Debug, Clone, Serialize)]
pub struct ImageGenerationRequest {
    pub prompt: String,
    pub aspect_ratio: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Successful backend response. `image` is either an `https://` URL or a
/// `data:image/...;base64,...` URL — the tool surfaces both shapes
/// transparently to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationResponse {
    /// `https://...` or `data:image/png;base64,...` URL.
    pub image: String,
    /// Human-readable provider label (e.g. "Google Imagen",
    /// "OpenAI gpt-image-1", "FAL FLUX 2 Pro").
    pub used_provider: String,
    pub width: u32,
    pub height: u32,
}

/// Error categories used by the tool to produce friendly user-facing
/// messages. Backends should map their internal errors here so the tool
/// can surface stable categories.
#[derive(Debug, thiserror::Error)]
pub enum ImageGenerationError {
    /// Backend is not bound ([`NullImageGenerationBackend`]). Always
    /// fail loud.
    #[error("image generation backend is not configured: {0}")]
    BackendNotConfigured(String),

    /// No provider in the chain has a configured API key. The Python
    /// original surfaces this through the registry `check_fn` — engine
    /// backends raise it directly.
    #[error("no image generation provider configured: {0}")]
    NoProviderConfigured(String),

    /// Upstream API rejected the prompt (safety filter, policy, etc.).
    #[error("upstream rejected prompt: {0}")]
    PromptRejected(String),

    /// 402 / insufficient credits family.
    #[error("insufficient credits or payment required: {0}")]
    InsufficientCredits(String),

    /// Anything else — preserved verbatim.
    #[error("image generation failed: {0}")]
    Other(String),
}

/// **The seam.** Real implementation lives in the host (`wcore-agent`
/// binds the provider chain); production builds inject it via
/// [`ImageGenerationTool::with_backend`]. Tests inject a
/// [`CapturingImageGenerationBackend`]. The crate's own
/// [`NullImageGenerationBackend`] **fails loud** rather than silently
/// faking success — this matches the browser/CUA `*Spec` mirror pattern
/// and the vision/video tool seams.
#[async_trait]
pub trait ImageGenerationBackend: Send + Sync {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError>;
}

/// Default backend used when no real one was bound. **Every call fails**
/// with [`ImageGenerationError::BackendNotConfigured`] so a missing
/// wire-up surfaces as a loud, debuggable error instead of a silent stub.
#[derive(Default)]
pub struct NullImageGenerationBackend;

#[async_trait]
impl ImageGenerationBackend for NullImageGenerationBackend {
    async fn generate(
        &self,
        _request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        Err(ImageGenerationError::BackendNotConfigured(
            "no ImageGenerationBackend bound — the host (wcore-agent) must \
inject a real backend via ImageGenerationTool::with_backend before this tool \
is registered."
                .to_string(),
        ))
    }
}

/// Captured invocation — exposed so tests can assert on what the tool
/// sent to the backend.
#[derive(Debug, Clone)]
pub struct CapturedGeneration {
    pub prompt: String,
    pub aspect_ratio: &'static str,
    pub width: u32,
    pub height: u32,
}

/// In-memory test backend that records every call and returns a canned
/// response. Lives in the prod module so downstream crates can reuse it
/// (mirrors `CapturingVisionBackend` in `vision_tools.rs`).
pub struct CapturingImageGenerationBackend {
    response: ImageGenerationResponse,
    pub captured: Mutex<Vec<CapturedGeneration>>,
}

impl CapturingImageGenerationBackend {
    pub fn new(response: ImageGenerationResponse) -> Self {
        Self {
            response,
            captured: Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Vec<CapturedGeneration> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl ImageGenerationBackend for CapturingImageGenerationBackend {
    async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ImageGenerationError> {
        self.captured.lock().push(CapturedGeneration {
            prompt: request.prompt.clone(),
            aspect_ratio: request.aspect_ratio,
            width: request.width,
            height: request.height,
        });
        Ok(self.response.clone())
    }
}

/// The agent-facing tool. Holds an `Arc<dyn ImageGenerationBackend>` so
/// the host can swap the implementation at startup without touching the
/// dispatcher.
pub struct ImageGenerationTool {
    backend: Arc<dyn ImageGenerationBackend>,
    backend_configured: bool,
}

impl Default for ImageGenerationTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageGenerationTool {
    /// Construct with the fail-loud [`NullImageGenerationBackend`]. The
    /// host must replace this via [`Self::with_backend`] before
    /// registering.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(NullImageGenerationBackend),
            backend_configured: false,
        }
    }

    /// Construct with a real backend (test fakes use this too).
    pub fn with_backend(backend: Arc<dyn ImageGenerationBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }

    /// Bound the size of the backend-returned `image` field. For
    /// `data:` URLs we decode the base64 segment and check the byte
    /// count; for `https://` URLs we just bound the URL string length.
    /// Defends against pathological backends that return megabyte-long
    /// payloads which would blow the dispatcher's tool-output budget.
    fn check_payload_size(image: &str) -> Result<(), String> {
        if image.len() > MAX_IMAGE_PAYLOAD_BYTES {
            return Err(format!(
                "Image payload too large: {} bytes (limit {} bytes).",
                image.len(),
                MAX_IMAGE_PAYLOAD_BYTES
            ));
        }
        // For data URLs the .len() check is already an upper bound on
        // the decoded size (base64 encodes 3 bytes as 4 chars, so the
        // decoded size is strictly smaller than the encoded size). No
        // need to decode here — the check above is conservative.
        Ok(())
    }

    /// If the backend returned an `https://` URL, sanity-check that it
    /// isn't pointing at private/internal infrastructure. `data:` URLs
    /// and `http://` URLs pass through (data: is local; http:// is
    /// intentionally accepted because some backends may return
    /// pre-signed temporary URLs). This is defense-in-depth: the
    /// dispatcher's outbound HTTP client should also revalidate before
    /// fetching.
    fn check_image_url_ssrf(image: &str) -> Result<(), String> {
        if image.starts_with("https://") && !is_safe_url(image) {
            return Err(format!(
                "Backend returned a URL that points at a private/internal \
network address — refusing to surface it: {}",
                image.chars().take(80).collect::<String>()
            ));
        }
        Ok(())
    }

    /// Convert a typed error into the friendly user-facing string the
    /// Python original surfaces.
    fn friendly_error(e: &ImageGenerationError) -> String {
        match e {
            ImageGenerationError::BackendNotConfigured(msg) => msg.clone(),
            ImageGenerationError::NoProviderConfigured(msg) => format!(
                "No image generation provider configured. Set one of \
GOOGLE_API_KEY, OPENAI_API_KEY, or FAL_KEY. Error: {msg}"
            ),
            ImageGenerationError::PromptRejected(msg) => format!(
                "The image generation provider rejected this prompt \
(safety filter or policy violation). Error: {msg}"
            ),
            ImageGenerationError::InsufficientCredits(msg) => format!(
                "Insufficient credits or payment required. Please top up \
your image-generation provider account. Error: {msg}"
            ),
            ImageGenerationError::Other(msg) => {
                format!("There was a problem generating the image. Error: {msg}")
            }
        }
    }
}

#[async_trait]
impl Tool for ImageGenerationTool {
    fn name(&self) -> &str {
        "image_generate"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Generate high-quality images from text prompts. Uses the configured \
image-gen provider (Google Imagen, OpenAI DALL-E, or FAL FLUX 2 Pro). \
Returns a single image URL or data URL. Display it via markdown: ![desc](URL)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description":
                        "The text prompt describing the desired image. Be detailed and descriptive."
                },
                "aspect_ratio": {
                    "type": "string",
                    "enum": ["landscape", "square", "portrait"],
                    "description":
                        "Aspect ratio: 'landscape' (16:9), 'portrait' (9:16), 'square' (1:1).",
                    "default": "landscape"
                }
            },
            "required": ["prompt"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // No filesystem mutation, no shared state beyond the backend
        // (which is itself thread-safe via Send + Sync).
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn max_result_size(&self) -> usize {
        // Allow room for a data: URL up to MAX_IMAGE_PAYLOAD_BYTES.
        MAX_IMAGE_PAYLOAD_BYTES + 4096
    }

    fn describe(&self, input: &Value) -> String {
        let prompt = input
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        let aspect = input
            .get("aspect_ratio")
            .and_then(|v| v.as_str())
            .unwrap_or("landscape");
        // Truncate the prompt for the describe line.
        let preview: String = prompt.chars().take(60).collect();
        format!("image_generate({aspect}): {preview}")
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // 1. Validate prompt — required, non-empty after trimming.
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "image": null,
                        "error": "Prompt is required and must be a non-empty string",
                        "freeFallbackUsed": false,
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };

        // 2. Normalize aspect ratio.
        let aspect_str = input.get("aspect_ratio").and_then(|v| v.as_str());
        let aspect = AspectRatio::normalize(aspect_str);
        let (width, height) = aspect.dimensions();

        // 3. Hand off to the backend.
        let req = ImageGenerationRequest {
            prompt: prompt.clone(),
            aspect_ratio: aspect.as_str(),
            width,
            height,
        };

        match self.backend.generate(req).await {
            Ok(resp) => {
                // Bound the returned payload size (defense-in-depth
                // against pathological backends).
                if let Err(e) = Self::check_payload_size(&resp.image) {
                    return ToolResult {
                        content: json!({
                            "success": false,
                            "image": null,
                            "error": e,
                            "freeFallbackUsed": false,
                        })
                        .to_string(),
                        is_error: true,
                    };
                }
                // SSRF sanity check on returned URL (data: URLs and
                // http:// pre-signed URLs pass through).
                if let Err(e) = Self::check_image_url_ssrf(&resp.image) {
                    return ToolResult {
                        content: json!({
                            "success": false,
                            "image": null,
                            "error": e,
                            "freeFallbackUsed": false,
                        })
                        .to_string(),
                        is_error: true,
                    };
                }

                ToolResult {
                    content: json!({
                        "success": true,
                        "image": resp.image,
                        "freeFallbackUsed": false,
                        "usedProvider": resp.used_provider,
                        "width": resp.width,
                        "height": resp.height,
                    })
                    .to_string(),
                    is_error: false,
                }
            }
            Err(e) => {
                let friendly = Self::friendly_error(&e);
                ToolResult {
                    content: json!({
                        "success": false,
                        "image": null,
                        "error": format!("Error generating image: {e}"),
                        "details": friendly,
                        "freeFallbackUsed": false,
                    })
                    .to_string(),
                    is_error: true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_response(image: &str) -> ImageGenerationResponse {
        ImageGenerationResponse {
            image: image.to_string(),
            used_provider: "OpenAI gpt-image-1".to_string(),
            width: 1536,
            height: 1024,
        }
    }

    fn parse(content: &str) -> Value {
        serde_json::from_str(content).expect("tool output must be valid JSON")
    }

    // --- AspectRatio::normalize ---

    #[test]
    fn aspect_ratio_normalize_known_values() {
        assert_eq!(
            AspectRatio::normalize(Some("landscape")),
            AspectRatio::Landscape
        );
        assert_eq!(AspectRatio::normalize(Some("square")), AspectRatio::Square);
        assert_eq!(
            AspectRatio::normalize(Some("portrait")),
            AspectRatio::Portrait
        );
        // Case-insensitive + whitespace tolerated (matches Python `.lower().strip()`).
        assert_eq!(
            AspectRatio::normalize(Some(" PORTRAIT ")),
            AspectRatio::Portrait
        );
    }

    #[test]
    fn aspect_ratio_normalize_falls_back_to_landscape() {
        assert_eq!(AspectRatio::normalize(None), AspectRatio::Landscape);
        assert_eq!(AspectRatio::normalize(Some("")), AspectRatio::Landscape);
        assert_eq!(
            AspectRatio::normalize(Some("widescreen")),
            AspectRatio::Landscape
        );
    }

    #[test]
    fn aspect_ratio_dimensions_match_python() {
        assert_eq!(AspectRatio::Landscape.dimensions(), (1536, 1024));
        assert_eq!(AspectRatio::Square.dimensions(), (1024, 1024));
        assert_eq!(AspectRatio::Portrait.dimensions(), (1024, 1536));
    }

    // --- Null backend fails loud ---

    #[tokio::test]
    async fn null_backend_fails_loud() {
        let tool = ImageGenerationTool::new();
        let result = tool
            .execute(json!({"prompt": "a serene mountain landscape"}))
            .await;
        assert!(result.is_error, "Null backend must surface error");
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(false));
        assert!(v["image"].is_null());
        let err = v["error"].as_str().expect("error string present");
        assert!(
            err.contains("not configured") || err.contains("backend is not configured"),
            "expected fail-loud message, got: {err}"
        );
        assert_eq!(v["freeFallbackUsed"], json!(false));
    }

    // --- Happy path with Capturing backend ---

    #[tokio::test]
    async fn capturing_backend_happy_path() {
        let backend = Arc::new(CapturingImageGenerationBackend::new(ok_response(
            "https://example.com/img.png",
        )));
        let tool = ImageGenerationTool::with_backend(backend.clone());
        let result = tool
            .execute(json!({
                "prompt": "a serene mountain landscape",
                "aspect_ratio": "square"
            }))
            .await;
        assert!(!result.is_error, "happy path must not be an error");
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["image"], json!("https://example.com/img.png"));
        assert_eq!(v["usedProvider"], json!("OpenAI gpt-image-1"));
        assert_eq!(v["freeFallbackUsed"], json!(false));
        assert_eq!(v["width"], json!(1536));
        assert_eq!(v["height"], json!(1024));

        // Backend was called with the normalized aspect ratio + dimensions.
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].prompt, "a serene mountain landscape");
        assert_eq!(snap[0].aspect_ratio, "square");
        assert_eq!(snap[0].width, 1024);
        assert_eq!(snap[0].height, 1024);
    }

    // --- Missing / empty prompt rejection ---

    #[tokio::test]
    async fn missing_prompt_is_rejected_before_backend() {
        let backend = Arc::new(CapturingImageGenerationBackend::new(ok_response(
            "https://example.com/x.png",
        )));
        let tool = ImageGenerationTool::with_backend(backend.clone());

        // Case A: missing entirely.
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(false));
        assert!(v["error"].as_str().unwrap().contains("Prompt is required"));

        // Case B: empty / whitespace-only.
        let result = tool.execute(json!({"prompt": "   "})).await;
        assert!(result.is_error);
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(false));

        // Backend must not have been called for either case.
        assert_eq!(
            backend.snapshot().len(),
            0,
            "backend must not be called when prompt is missing"
        );
    }

    // --- Oversized output cap ---

    #[tokio::test]
    async fn oversized_backend_payload_is_capped() {
        // Build a payload larger than the hard cap.
        let huge = "data:image/png;base64,".to_string() + &"A".repeat(MAX_IMAGE_PAYLOAD_BYTES + 1);
        let backend = Arc::new(CapturingImageGenerationBackend::new(
            ImageGenerationResponse {
                image: huge,
                used_provider: "FAL FLUX 2 Pro".to_string(),
                width: 1024,
                height: 1024,
            },
        ));
        let tool = ImageGenerationTool::with_backend(backend);
        let result = tool.execute(json!({"prompt": "test"})).await;
        assert!(result.is_error, "oversized payload must surface error");
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(false));
        assert!(v["image"].is_null());
        assert!(
            v["error"].as_str().unwrap().contains("too large"),
            "expected size-cap error, got: {}",
            v["error"]
        );
    }

    // --- Schema shape ---

    #[test]
    fn input_schema_shape() {
        let tool = ImageGenerationTool::new();
        let schema = tool.input_schema();
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["required"], json!(["prompt"]));
        let props = &schema["properties"];
        assert_eq!(props["prompt"]["type"], json!("string"));
        assert_eq!(
            props["aspect_ratio"]["enum"],
            json!(["landscape", "square", "portrait"])
        );
        assert_eq!(props["aspect_ratio"]["default"], json!("landscape"));
        // Tool metadata.
        assert_eq!(tool.name(), "image_generate");
        assert!(!tool.description().is_empty());
        assert!(matches!(tool.category(), ToolCategory::Info));
        // Concurrency safe — no shared state mutation.
        assert!(tool.is_concurrency_safe(&json!({"prompt": "x"})));
    }

    // --- SSRF guard on returned URL ---

    #[tokio::test]
    async fn ssrf_guard_blocks_private_url() {
        // 127.0.0.1 is unconditionally private per is_safe_url.
        let backend = Arc::new(CapturingImageGenerationBackend::new(ok_response(
            "https://127.0.0.1/secret.png",
        )));
        let tool = ImageGenerationTool::with_backend(backend);
        let result = tool.execute(json!({"prompt": "test"})).await;
        assert!(result.is_error, "SSRF private URL must be rejected");
        let v = parse(&result.content);
        assert_eq!(v["success"], json!(false));
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("private/internal network"),
            "expected SSRF error, got: {}",
            v["error"]
        );
    }

    // --- describe() doesn't leak full prompt for very long inputs ---

    #[test]
    fn describe_truncates_long_prompt() {
        let tool = ImageGenerationTool::new();
        let long = "x".repeat(200);
        let d = tool.describe(&json!({"prompt": long, "aspect_ratio": "portrait"}));
        assert!(d.starts_with("image_generate(portrait):"));
        // The preview should be bounded — 60 chars of the prompt.
        assert!(d.len() < 200, "describe must truncate long prompts: {d}");
    }
}
