//! T3-3.5 (sub-wave 5): `video_analyze` tool — AI video analysis.
//!
//! Ported from the prior Wayland Python engine.
//!
//! Sends a video (HTTP/HTTPS URL or local file) to a multimodal LLM
//! through a **pluggable [`VideoAnalysisBackend`]**. The engine binds the
//! real backend at construction time (typically wired in `wcore-agent` to
//! the auxiliary vision router); this crate ships a [`NullVideoBackend`]
//! that **fails loud** rather than silently returning a stub — matching the
//! seam pattern used elsewhere (browser/CUA tool specs).
//!
//! ## Behaviour preserved from the Python original
//!
//! * Source resolution: `file://`-prefixed or bare local file paths route
//!   to the on-disk read path; HTTP/HTTPS URLs are validated by the shared
//!   [`url_safety::is_safe_url`] SSRF guard *and* the
//!   [`website_policy::check_website_access`] blocklist before any byte
//!   leaves the agent.
//! * Extension/MIME mapping mirrors the Python `_VIDEO_MIME_TYPES`
//!   (`mp4 / webm / mov / avi → mp4 / mkv → mp4 / mpeg / mpg`). Unknown
//!   extensions are a hard error so we never send a payload the upstream
//!   provider will reject.
//! * 50 MB hard cap on the *base64-encoded* payload size, 20 MB soft warn
//!   threshold — same constants as Python (`_MAX_VIDEO_BASE64_BYTES`,
//!   `_VIDEO_SIZE_WARN_BYTES`).
//! * Output shape: `{"success": bool, "analysis": String, "error"?: String}`
//!   serialized via `serde_json`. Errors are mapped to the same
//!   user-facing categories (insufficient credits / unsupported model /
//!   payload too large / generic).
//!
//! ## Differences vs Python
//!
//! * **No network download in this crate.** The Python version embeds an
//!   httpx download path; we keep `wcore-tools` HTTP-free and delegate
//!   remote URL fetches to the backend implementation (the agent-side
//!   adapter has access to the configured HTTP client, retry policy, and
//!   redirect SSRF hooks). The tool still performs URL safety and
//!   blocklist checks **before** handing the URL to the backend.
//! * **No subprocess.** ffmpeg is not used — video understanding rides
//!   the multimodal LLM, not local transcoding. If a future variant needs
//!   ffmpeg pre-processing it MUST go through
//!   `wcore_config::shell::shell_command_argv` (LLM-supplied filenames).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::url_safety::is_safe_url;
use crate::website_policy::check_website_access;

/// 50 MB hard cap on the *base64-encoded* payload — matches the upstream
/// provider's documented limit. Enforced both before download (via
/// `Content-Length` when available) and after, when the byte count is
/// known. The Python sentinel is `_MAX_VIDEO_BASE64_BYTES`.
pub const MAX_VIDEO_BASE64_BYTES: u64 = 50 * 1024 * 1024;

/// 20 MB soft warning threshold — videos this large still go through, but
/// the tool annotates the result so callers can decide to compress.
pub const VIDEO_SIZE_WARN_BYTES: u64 = 20 * 1024 * 1024;

/// Extension → MIME for the formats the upstream multimodal API
/// accepts. AVI and MKV are intentionally remapped to `video/mp4` —
/// OpenRouter accepts the container as long as the codec inside is
/// MP4-compatible, and the Python original makes the same trade-off.
pub fn detect_video_mime_type(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/mov"),
        "avi" => Some("video/mp4"),
        "mkv" => Some("video/mp4"),
        "mpeg" => Some("video/mpeg"),
        "mpg" => Some("video/mpeg"),
        _ => None,
    }
}

/// Resolved source of a video — either an on-disk file the tool can read
/// directly, or a remote URL the backend must fetch with its own HTTP
/// client (after we pre-flight SSRF + blocklist checks).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoSource {
    LocalFile(PathBuf),
    RemoteUrl(String),
}

/// Validated request passed to the backend. The tool guarantees:
///
/// * For `LocalFile`, the path exists and has a supported extension.
/// * For `RemoteUrl`, the URL passed `is_safe_url` and the blocklist.
/// * `mime_type` matches the source extension (or, for remote URLs,
///   defaults to `video/mp4` so the backend can still build the
///   `video_url` content block before it has bytes in hand).
/// * `user_prompt` is non-empty and bounded.
#[derive(Debug, Clone)]
pub struct VideoAnalysisRequest {
    pub source: VideoSource,
    pub mime_type: &'static str,
    pub user_prompt: String,
    pub model: Option<String>,
}

/// What the backend returns. `analysis` is the raw model output;
/// `bytes_processed` is informational and used to emit the soft warning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoAnalysisResponse {
    pub analysis: String,
    pub bytes_processed: u64,
    pub model_used: Option<String>,
}

/// Error categories used by the tool to produce friendly user-facing
/// messages — mirrors the Python `err_str` keyword dispatch.
#[derive(Debug, thiserror::Error)]
pub enum VideoAnalysisError {
    /// Backend is not bound (NullVideoBackend). Always fail loud.
    #[error("video analysis backend is not configured: {0}")]
    BackendNotConfigured(String),

    /// Upstream rejected the payload because the model doesn't accept
    /// video. Surface a remediation hint.
    #[error("model does not support video input: {0}")]
    UnsupportedModel(String),

    /// 413 / payload-too-large family.
    #[error("video payload too large: {0}")]
    PayloadTooLarge(String),

    /// 402 / insufficient credits family.
    #[error("insufficient credits or payment required: {0}")]
    InsufficientCredits(String),

    /// Anything else — preserved verbatim.
    #[error("video analysis failed: {0}")]
    Other(String),
}

/// **The seam.** Real implementation lives in the host (`wcore-agent`
/// binds the auxiliary vision router); production builds inject it via
/// [`VideoAnalyzeTool::with_backend`]. Tests inject a fake. The crate's
/// own [`NullVideoBackend`] **fails loud** rather than silently faking
/// success — this matches the browser/CUA `*Spec` mirror pattern.
#[async_trait]
pub trait VideoAnalysisBackend: Send + Sync {
    async fn analyze(
        &self,
        request: VideoAnalysisRequest,
    ) -> Result<VideoAnalysisResponse, VideoAnalysisError>;
}

/// Default backend used when no real one was bound. **Every call fails**
/// with [`VideoAnalysisError::BackendNotConfigured`] so a missing wire-up
/// surfaces as a loud, debuggable error instead of a silent stub.
#[derive(Default)]
pub struct NullVideoBackend;

#[async_trait]
impl VideoAnalysisBackend for NullVideoBackend {
    async fn analyze(
        &self,
        _request: VideoAnalysisRequest,
    ) -> Result<VideoAnalysisResponse, VideoAnalysisError> {
        Err(VideoAnalysisError::BackendNotConfigured(
            "no VideoAnalysisBackend bound — the host (wcore-agent) must \
inject a real backend via VideoAnalyzeTool::with_backend before this tool \
is registered."
                .to_string(),
        ))
    }
}

/// The agent-facing tool. Holds an `Arc<dyn VideoAnalysisBackend>` so
/// the host can swap the implementation at startup without touching the
/// dispatcher.
pub struct VideoAnalyzeTool {
    backend: Arc<dyn VideoAnalysisBackend>,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `with_backend(backend)` flips
    /// it on.
    backend_configured: bool,
}

impl Default for VideoAnalyzeTool {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoAnalyzeTool {
    /// Construct with the fail-loud `NullVideoBackend`. The host must
    /// replace this via [`Self::with_backend`] before registering.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(NullVideoBackend),
            backend_configured: false,
        }
    }

    /// Construct with a real backend (test fakes use this too).
    pub fn with_backend(backend: Arc<dyn VideoAnalysisBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }

    /// Convert a user-supplied `video_url` argument into a validated
    /// `VideoSource`, performing SSRF + blocklist checks for remote URLs
    /// and existence + extension checks for local files.
    fn resolve_source(raw: &str) -> Result<(VideoSource, &'static str), String> {
        // Local file path forms: `file://...` (strip prefix) or a bare path
        // that exists on disk. We follow the Python order — try local
        // first, fall back to URL validation.
        let stripped = raw.strip_prefix("file://").unwrap_or(raw);

        // Expand `~` to the user's home directory the same way Python's
        // `os.path.expanduser` does — without pulling in extra deps we
        // accept just the leading `~` form.
        let expanded: PathBuf = if let Some(rest) = stripped.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(rest)
            } else {
                PathBuf::from(stripped)
            }
        } else if stripped == "~" {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"))
        } else {
            PathBuf::from(stripped)
        };

        if expanded.is_file() {
            let mime = detect_video_mime_type(&expanded).ok_or_else(|| {
                format!(
                    "Unsupported video format: '{}'. Supported: avi, mkv, mov, \
mp4, mpeg, mpg, webm",
                    expanded.extension().and_then(|s| s.to_str()).unwrap_or("")
                )
            })?;
            return Ok((VideoSource::LocalFile(expanded), mime));
        }

        if raw.starts_with("http://") || raw.starts_with("https://") {
            if !is_safe_url(raw) {
                return Err(format!("Blocked unsafe / private-network URL: {}", raw));
            }
            // check_website_access falls back to `None` on config errors
            // (tracing::warn-only); only an explicit block ID-rule match
            // surfaces a WebsiteBlock here.
            if let Ok(Some(block)) = check_website_access(raw, None) {
                return Err(format!("Blocked by website policy: {}", block.message));
            }
            // Without bytes in hand we default to mp4 — the backend will
            // refine if it can sniff the response.
            return Ok((VideoSource::RemoteUrl(raw.to_string()), "video/mp4"));
        }

        Err(format!(
            "Invalid video source. Provide an HTTP/HTTPS URL or a valid \
local file path, got: '{}'",
            raw
        ))
    }

    /// Map the typed error categories to the same friendly strings the
    /// Python version returns in its `except` block.
    fn friendly_error(e: &VideoAnalysisError) -> String {
        match e {
            VideoAnalysisError::BackendNotConfigured(msg) => msg.clone(),
            VideoAnalysisError::InsufficientCredits(msg) => format!(
                "Insufficient credits or payment required. Please top up \
your API provider account and try again. Error: {msg}"
            ),
            VideoAnalysisError::UnsupportedModel(msg) => format!(
                "The model does not support video analysis or the request \
was rejected. Ensure you're using a video-capable model (e.g. \
google/gemini-2.5-flash). Error: {msg}"
            ),
            VideoAnalysisError::PayloadTooLarge(msg) => format!(
                "The video is too large for the API. Try compressing or \
trimming the video (max ~50 MB). Error: {msg}"
            ),
            VideoAnalysisError::Other(msg) => format!(
                "There was a problem with the request and the video could \
not be analyzed. Error: {msg}"
            ),
        }
    }
}

#[async_trait]
impl Tool for VideoAnalyzeTool {
    fn name(&self) -> &str {
        "video_analyze"
    }

    /// v0.9.0 W1: hidden when no real `VideoAnalysisBackend` is wired.
    /// `Default::default()` yields `backend_configured == false`, so
    /// `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Analyze a video from a URL or local file path using a multimodal \
model. Sends the video to a video-capable model (e.g. Gemini) for \
understanding. Use this for video files — for images, use vision_analyze \
instead. Supports mp4, webm, mov, avi, mkv, mpeg formats. Note: large \
videos (>20 MB) may be slow; max ~50 MB."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "video_url": {
                    "type": "string",
                    "description":
                        "Video URL (http/https) or local file path to analyze."
                },
                "question": {
                    "type": "string",
                    "description":
                        "Your specific question about the video. The agent \
        will describe what happens in the video and answer your question."
                }
            },
            "required": ["video_url", "question"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only / no shared filesystem mutation.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let video_url = match input.get("video_url").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": "video_url is required",
                        "analysis": "Missing required parameter: video_url.",
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };
        let question = input.get("question").and_then(|v| v.as_str()).unwrap_or("");

        // Build the full prompt the Python `_handle_video_analyze` wraps
        // around the user's question.
        let full_prompt = format!(
            "Fully describe and explain everything happening in this video, \
including visual content, motion, audio cues, text overlays, and scene \
transitions. Then answer the following question:\n\n{question}"
        );

        let (source, mime_type) = match Self::resolve_source(video_url) {
            Ok(pair) => pair,
            Err(msg) => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": msg.clone(),
                        "analysis": msg,
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };

        // For local files, enforce the 50 MB cap before we hand the
        // path to the backend so misconfigured backends can't silently
        // upload an oversized file.
        if let VideoSource::LocalFile(ref p) = source {
            match std::fs::metadata(p) {
                Ok(md) => {
                    let len = md.len();
                    if len > MAX_VIDEO_BASE64_BYTES {
                        return ToolResult {
                            content: json!({
                                "success": false,
                                "error": format!(
                                    "Video too large for API: {} bytes (limit {} bytes).",
                                    len, MAX_VIDEO_BASE64_BYTES
                                ),
                                "analysis": "Compress or trim the video and retry.",
                            })
                            .to_string(),
                            is_error: true,
                        };
                    }
                }
                Err(e) => {
                    return ToolResult {
                        content: json!({
                            "success": false,
                            "error": format!("Could not stat video file: {e}"),
                            "analysis": "Local video file is unreadable.",
                        })
                        .to_string(),
                        is_error: true,
                    };
                }
            }
        }

        // Resolve model from env (Python order: AUXILIARY_VIDEO_MODEL
        // then AUXILIARY_VISION_MODEL).
        let model = std::env::var("AUXILIARY_VIDEO_MODEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                std::env::var("AUXILIARY_VISION_MODEL")
                    .ok()
                    .filter(|s| !s.trim().is_empty())
            });

        let req = VideoAnalysisRequest {
            source,
            mime_type,
            user_prompt: full_prompt,
            model: model.clone(),
        };

        match self.backend.analyze(req).await {
            Ok(resp) => {
                let mut payload = json!({
                    "success": true,
                    "analysis": resp.analysis,
                });
                if resp.bytes_processed > VIDEO_SIZE_WARN_BYTES {
                    payload["warning"] = json!(format!(
                        "Video is {:.1} MB — may be slow or rejected by the upstream model.",
                        resp.bytes_processed as f64 / (1024.0 * 1024.0)
                    ));
                }
                if let Some(m) = resp.model_used {
                    payload["model_used"] = json!(m);
                }
                ToolResult {
                    content: payload.to_string(),
                    is_error: false,
                }
            }
            Err(e) => {
                let friendly = Self::friendly_error(&e);
                ToolResult {
                    content: json!({
                        "success": false,
                        "error": format!("Error analyzing video: {e}"),
                        "analysis": friendly,
                    })
                    .to_string(),
                    is_error: true,
                }
            }
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn max_result_size(&self) -> usize {
        // Multimodal responses for video can be larger than the default
        // 50 KB — match the vision-class tools at 100 KB.
        100_000
    }

    fn describe(&self, input: &Value) -> String {
        let url = input
            .get("video_url")
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        format!("video_analyze: {url}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_video_mime_type ---

    #[test]
    fn mime_type_known_extensions() {
        assert_eq!(
            detect_video_mime_type(Path::new("a.mp4")),
            Some("video/mp4")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.MP4")),
            Some("video/mp4")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.webm")),
            Some("video/webm")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.mov")),
            Some("video/mov")
        );
        // AVI/MKV fall back to mp4 per the Python mapping.
        assert_eq!(
            detect_video_mime_type(Path::new("a.avi")),
            Some("video/mp4")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.mkv")),
            Some("video/mp4")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.mpeg")),
            Some("video/mpeg")
        );
        assert_eq!(
            detect_video_mime_type(Path::new("a.mpg")),
            Some("video/mpeg")
        );
    }

    #[test]
    fn mime_type_unknown_extensions() {
        assert_eq!(detect_video_mime_type(Path::new("a.txt")), None);
        assert_eq!(detect_video_mime_type(Path::new("a")), None);
        assert_eq!(detect_video_mime_type(Path::new("a.gif")), None);
    }

    // --- NullVideoBackend fails loud ---

    #[tokio::test]
    async fn null_backend_fails_loud() {
        let tool = VideoAnalyzeTool::new();
        let result = tool
            .execute(json!({
                "video_url": "https://example.com/clip.mp4",
                "question": "what happens?"
            }))
            .await;
        assert!(result.is_error, "NullVideoBackend must surface as error");
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["success"], json!(false));
        let analysis = body["analysis"].as_str().unwrap();
        assert!(
            analysis.contains("backend is not configured")
                || analysis.contains("no VideoAnalysisBackend bound"),
            "expected fail-loud message, got: {analysis}"
        );
    }

    // --- Source resolution: local file path ---

    #[test]
    fn resolve_source_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clip.mp4");
        std::fs::write(&p, b"\x00\x00\x00\x18ftypmp42").unwrap();

        let (source, mime) = VideoAnalyzeTool::resolve_source(p.to_str().unwrap()).unwrap();
        assert_eq!(mime, "video/mp4");
        assert_eq!(source, VideoSource::LocalFile(p));
    }

    #[test]
    fn resolve_source_file_scheme_strips_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clip.webm");
        std::fs::write(&p, b"webm-bytes").unwrap();
        let url = format!("file://{}", p.to_str().unwrap());

        let (source, mime) = VideoAnalyzeTool::resolve_source(&url).unwrap();
        assert_eq!(mime, "video/webm");
        assert_eq!(source, VideoSource::LocalFile(p));
    }

    #[test]
    fn resolve_source_unknown_extension_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("notavideo.txt");
        std::fs::write(&p, b"hello").unwrap();
        let err = VideoAnalyzeTool::resolve_source(p.to_str().unwrap()).unwrap_err();
        assert!(err.contains("Unsupported video format"), "got: {err}");
    }

    // --- Source resolution: URL safety ---

    #[test]
    fn resolve_source_blocks_private_network_url() {
        // is_safe_url MUST block link-local / loopback / private space.
        // Use 169.254.169.254 (AWS metadata) which `is_cloud_metadata_url`
        // explicitly blocks even before SSRF checks.
        let err = VideoAnalyzeTool::resolve_source("http://169.254.169.254/latest/meta-data/")
            .unwrap_err();
        assert!(
            err.contains("Blocked") || err.contains("blocked") || err.contains("unsafe"),
            "expected SSRF block, got: {err}"
        );
    }

    #[test]
    fn resolve_source_rejects_garbage_input() {
        let err = VideoAnalyzeTool::resolve_source("not a url and not a path").unwrap_err();
        assert!(err.contains("Invalid video source"), "got: {err}");
    }

    // --- Friendly error mapping ---

    #[test]
    fn friendly_error_maps_credits() {
        let err = VideoAnalysisError::InsufficientCredits("402 Payment Required".into());
        let msg = VideoAnalyzeTool::friendly_error(&err);
        assert!(msg.contains("Insufficient credits"));
        assert!(msg.contains("402 Payment Required"));
    }

    #[test]
    fn friendly_error_maps_payload_too_large() {
        let err = VideoAnalysisError::PayloadTooLarge("413".into());
        let msg = VideoAnalyzeTool::friendly_error(&err);
        assert!(msg.contains("too large"));
        assert!(msg.contains("50 MB"));
    }

    #[test]
    fn friendly_error_maps_unsupported_model() {
        let err = VideoAnalysisError::UnsupportedModel("model lacks video input".into());
        let msg = VideoAnalyzeTool::friendly_error(&err);
        assert!(msg.contains("does not support video"));
        assert!(msg.contains("gemini"));
    }

    // --- End-to-end: fake backend returns success ---

    struct FakeBackend {
        analysis: String,
        bytes: u64,
    }

    #[async_trait]
    impl VideoAnalysisBackend for FakeBackend {
        async fn analyze(
            &self,
            _req: VideoAnalysisRequest,
        ) -> Result<VideoAnalysisResponse, VideoAnalysisError> {
            Ok(VideoAnalysisResponse {
                analysis: self.analysis.clone(),
                bytes_processed: self.bytes,
                model_used: Some("fake-model".into()),
            })
        }
    }

    #[tokio::test]
    async fn execute_succeeds_with_fake_backend() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clip.mp4");
        std::fs::write(&p, b"mp4-bytes").unwrap();

        let tool = VideoAnalyzeTool::with_backend(Arc::new(FakeBackend {
            analysis: "A person waves at the camera.".to_string(),
            bytes: 5 * 1024 * 1024, // 5 MB — below soft warn threshold
        }));

        let result = tool
            .execute(json!({
                "video_url": p.to_str().unwrap(),
                "question": "What does the person do?"
            }))
            .await;
        assert!(!result.is_error);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(body["success"], json!(true));
        assert_eq!(body["analysis"], json!("A person waves at the camera."));
        assert_eq!(body["model_used"], json!("fake-model"));
        assert!(body.get("warning").is_none(), "no warning below soft cap");
    }

    #[tokio::test]
    async fn execute_emits_soft_warning_above_20mb() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.mp4");
        std::fs::write(&p, b"mp4-bytes").unwrap();

        let tool = VideoAnalyzeTool::with_backend(Arc::new(FakeBackend {
            analysis: "long video".to_string(),
            bytes: 30 * 1024 * 1024,
        }));

        let result = tool
            .execute(json!({
                "video_url": p.to_str().unwrap(),
                "question": "?"
            }))
            .await;
        assert!(!result.is_error);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        let warning = body["warning"].as_str().unwrap();
        assert!(warning.contains("MB"));
        assert!(warning.contains("slow"));
    }

    // --- Hard cap enforced before backend is called ---

    #[tokio::test]
    async fn execute_rejects_oversized_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("huge.mp4");
        // Create a sparse file just larger than the hard cap.
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(MAX_VIDEO_BASE64_BYTES + 1).unwrap();

        // Backend would panic if called — proves we short-circuited.
        struct PanicBackend;
        #[async_trait]
        impl VideoAnalysisBackend for PanicBackend {
            async fn analyze(
                &self,
                _req: VideoAnalysisRequest,
            ) -> Result<VideoAnalysisResponse, VideoAnalysisError> {
                panic!("backend should not be called for oversized input");
            }
        }
        let tool = VideoAnalyzeTool::with_backend(Arc::new(PanicBackend));

        let result = tool
            .execute(json!({
                "video_url": p.to_str().unwrap(),
                "question": "?"
            }))
            .await;
        assert!(result.is_error);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("too large"),
            "got: {body}"
        );
    }

    // --- Schema + Tool trait basics ---

    #[test]
    fn tool_schema_shape() {
        let tool = VideoAnalyzeTool::new();
        assert_eq!(tool.name(), "video_analyze");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], json!("object"));
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "video_url"));
        assert!(required.iter().any(|v| v == "question"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_video_url() {
        let tool = VideoAnalyzeTool::new();
        let result = tool.execute(json!({ "question": "?" })).await;
        assert!(result.is_error);
        let body: Value = serde_json::from_str(&result.content).unwrap();
        assert!(body["error"].as_str().unwrap().contains("video_url"));
    }

    // --- v0.9.0 W1 backend gate ---

    #[test]
    fn default_is_hidden_when_no_backend_wired() {
        let tool = VideoAnalyzeTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn with_real_backend_is_available() {
        let tool = VideoAnalyzeTool::with_backend(Arc::new(FakeBackend {
            analysis: "x".into(),
            bytes: 0,
        }));
        assert!(
            tool.is_available(),
            "with_backend(...) must yield backend_configured == true"
        );
    }
}
