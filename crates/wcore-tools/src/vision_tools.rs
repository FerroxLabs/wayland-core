//! T3-3.5 — `vision_analyze` AI vision tool.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md).
//! The Python original routes image URLs (or local paths) through a
//! centralized auxiliary LLM router that supports OpenRouter, Codex,
//! native Anthropic, or any OpenAI-compatible endpoint. Wayland's
//! engine deliberately has **no embedded multimodal provider** — each
//! provider's vision support lives behind the `LlmProvider` trait and
//! is wired by the host. To honor the NO-STUBS contract of T3, this
//! port covers the **dispatch surface and safety boundary** only:
//!
//! * Schema + input validation (URL vs. local file, MIME sniffing).
//! * SSRF defense — reuses `url_safety::is_safe_url` and the website
//!   policy blocklist via `website_policy::check_website_access`.
//! * Hard size cap (20 MB base64) — matches Gemini's inline-data limit,
//!   the most restrictive major provider.
//! * Two pluggable seams (mirror of the `MessageTransport` pattern in
//!   `send_message.rs`):
//!     * `ImageFetcher` — fetches bytes from an HTTP/HTTPS URL.
//!     * `VisionBackend` — sends `{mime, bytes, prompt}` to a real
//!       multimodal model and returns the analysis text.
//! * `NullImageFetcher` / `NullVisionBackend` fail loudly with
//!   structured errors rather than silently succeeding — this is the
//!   no-stub guarantee.
//! * `CapturingVisionBackend` + `StaticImageFetcher` for hermetic
//!   testing (no network).
//!
//! Divergences from the Python original (intentional):
//! * No Pillow-based auto-resize. The provider crates (Anthropic /
//!   OpenAI / Bedrock / Vertex) are the right place for vendor-specific
//!   downscale-on-413 retries, since the size limits differ per
//!   provider. The engine port enforces the cross-provider 20 MB hard
//!   cap up front; backends may further reject smaller payloads.
//! * No `temp_vision_images/` working directory — the engine never
//!   persists fetched bytes to disk. The fetched payload is held in
//!   memory only for the duration of the `execute()` call.
//! * No retry-once-on-empty-content — that's a provider-router concern;
//!   backends return their final text.
//!
//! Dependency choice (documented per task instructions):
//! No heavy image dependencies (Pillow, tesseract, image-rs) are
//! introduced. MIME sniffing uses inline magic-bytes (~50 LOC). This
//! keeps `wcore-tools` link-time identical to the pre-port baseline
//! and defers vendor-specific image handling to the backend wired by
//! the host.

use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::path_validation::validate_user_path;
use crate::url_safety::is_safe_url;
use crate::website_policy::check_website_access;

/// Hard cap on raw image bytes (20 MB). Mirrors the Python original's
/// `_MAX_BASE64_BYTES`. Set against the raw-bytes size rather than the
/// base64-expanded size — providers vary in whether they accept raw or
/// base64, and 20 MB raw is a strict upper bound either way.
pub const VISION_MAX_BYTES: usize = 20 * 1024 * 1024;

/// Lower-bound sanity check — a JPEG/PNG of >16 bytes is the smallest
/// thing that could possibly be a real image (header alone). Anything
/// shorter is almost certainly an error page or empty body.
pub const VISION_MIN_BYTES: usize = 16;

/// MIME types accepted by `vision_analyze`. Mirrors the Python
/// `_detect_image_mime_type` allowlist.
const SUPPORTED_MIME_PREFIXES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/bmp",
    "image/webp",
];

/// Result of sniffing an image's magic bytes. Returns `None` when the
/// header doesn't match any supported image type.
///
/// Mirrors `_detect_image_mime_type` from the Python original. Pure
/// magic-byte inspection — no file system access, no external deps.
pub fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some("image/png");
    }
    // JPEG: FF D8 FF
    if bytes.len() >= 3 && &bytes[..3] == b"\xff\xd8\xff" {
        return Some("image/jpeg");
    }
    // GIF87a / GIF89a
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some("image/gif");
    }
    // BMP: "BM"
    if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        return Some("image/bmp");
    }
    // WEBP: "RIFF" .... "WEBP"
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Validate that a string looks like a vision-acceptable URL. Allows
/// only http/https schemes, requires a host, and runs SSRF defense
/// via [`is_safe_url`].
pub fn validate_image_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL is empty".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "Only http:// and https:// URLs are supported (got: {})",
            url.chars().take(40).collect::<String>()
        ));
    }
    // SSRF defense — reuses the workspace url_safety helper.
    if !is_safe_url(url) {
        return Err(format!(
            "URL blocked by SSRF safety policy: {}",
            url.chars().take(80).collect::<String>()
        ));
    }
    Ok(())
}

/// Classify an `image_url` argument: a LOCAL file to read, or a remote URL to
/// fetch. Returns the filesystem path for a `file://` URI or any non-`http(s)`
/// string (a desktop drag-drop sends an absolute temp path, not a URL);
/// `None` for `http(s)://` URLs, which stay on the fetcher path. Path SAFETY is
/// enforced later by [`validate_user_path`], not here.
fn local_image_path(image_url: &str) -> Result<Option<PathBuf>, String> {
    if image_url.starts_with("http://") || image_url.starts_with("https://") {
        return Ok(None);
    }
    if image_url.starts_with("file:") {
        let url = url::Url::parse(image_url).map_err(|e| format!("Invalid file URL: {e}"))?;
        if url.host_str().is_some_and(|host| host != "localhost") {
            return Err("Network file URLs are not allowed".to_string());
        }
        return url
            .to_file_path()
            .map(Some)
            .map_err(|()| "Invalid local file URL".to_string());
    }
    Ok(Some(PathBuf::from(image_url)))
}

/// True for a Windows UNC / network path (`\\server\share`, `\\?\UNC\...`).
/// Opening one triggers an outbound SMB connection (a NetNTLM-hash leak
/// vector) before any content check, so the vision local-file path refuses it
/// up front. Always `false` on Unix — those targets never produce a UNC path
/// prefix, so this is a no-op there.
fn is_network_path(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    matches!(
        path.components().next(),
        Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
    )
}

/// Open a local image without following a symlink/reparse point.
///
/// `O_NONBLOCK` prevents a hostile FIFO from hanging before the regular-file
/// check. Unix walks from `/` with directory handles and `openat(O_NOFOLLOW)`
/// so a raced parent rename cannot redirect the final open. Windows rejects
/// reparse-point parents before opening the leaf reparse point itself.
fn open_local_image(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd as _, FromRawFd as _};
        use std::os::unix::ffi::OsStrExt as _;
        use std::path::Component;

        let mut parts = path.components();
        if !matches!(parts.next(), Some(Component::RootDir)) {
            return Err(format!("Image path is not absolute: {}", path.display()));
        }
        let names = parts
            .map(|part| match part {
                Component::Normal(name) => CString::new(name.as_bytes())
                    .map_err(|_| format!("Image path contains NUL: {}", path.display())),
                _ => Err(format!(
                    "Image path contains an unsupported component: {}",
                    path.display()
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if names.is_empty() {
            return Err(format!("Image path has no file name: {}", path.display()));
        }

        let mut parent = File::open("/")
            .map_err(|e| format!("Cannot open image path root for {}: {e}", path.display()))?;
        for (index, name) in names.iter().enumerate() {
            let is_leaf = index + 1 == names.len();
            let flags = if is_leaf {
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC
            } else {
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
            };
            // SAFETY: `parent` is a live directory descriptor for every
            // non-leaf iteration, `name` is NUL-terminated, and no pointer is
            // retained after `openat` returns.
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(format!(
                    "Cannot open image path component in {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: `openat` returned a new owned descriptor on success.
            let opened = unsafe { File::from_raw_fd(fd) };
            if is_leaf {
                return Ok(opened);
            }
            parent = opened;
        }
        unreachable!("non-empty component walk returns at the leaf")
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

        for parent in path.ancestors().skip(1) {
            let metadata = std::fs::symlink_metadata(parent).map_err(|e| {
                format!(
                    "Cannot inspect image path component {}: {e}",
                    parent.display()
                )
            })?;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(format!(
                    "Image symlinks/reparse points are not allowed: {}",
                    parent.display()
                ));
            }
        }

        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|e| format!("Cannot open image file {}: {e}", path.display()))?;
        let metadata = file
            .metadata()
            .map_err(|e| format!("Cannot stat image file {}: {e}", path.display()))?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(format!(
                "Image symlinks/reparse points are not allowed: {}",
                path.display()
            ));
        }
        return Ok(file);
    }

    #[cfg(not(any(unix, windows)))]
    {
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| format!("Cannot open image file {}: {e}", path.display()))
    }
}

/// Resolve one local image path into sniffed MIME and bounded bytes.
///
/// This is the shared local-file boundary for the legacy `vision_analyze`
/// tool and host composer attachments. It never fetches a URL, opens the target
/// exactly once, rejects symlink/reparse components and non-regular files, and
/// bounds a file that grows after its metadata check.
pub fn load_local_image(image_path: &str) -> Result<(&'static str, Vec<u8>), String> {
    let path = local_image_path(image_path)?
        .ok_or_else(|| "Composer attachments must be local files".to_string())?;
    if is_network_path(&path) {
        return Err("Network/UNC image paths are not allowed".to_string());
    }
    let validated = validate_user_path(&path).map_err(|e| format!("Invalid image path: {e}"))?;
    let file = open_local_image(&validated)?;
    let meta = file
        .metadata()
        .map_err(|e| format!("Cannot stat image file {}: {e}", validated.display()))?;
    if !meta.is_file() {
        return Err(format!("Not a regular image file: {}", validated.display()));
    }
    if meta.len() > VISION_MAX_BYTES as u64 {
        return Err(format!(
            "Image too large for vision API: {} bytes (limit {} bytes)",
            meta.len(),
            VISION_MAX_BYTES,
        ));
    }

    use std::io::Read as _;
    let mut buf = Vec::with_capacity(meta.len().min(VISION_MAX_BYTES as u64) as usize);
    file.take(VISION_MAX_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("Failed to read image file {}: {e}", validated.display()))?;
    validate_image_bytes(buf)
}

fn validate_image_bytes(bytes: Vec<u8>) -> Result<(&'static str, Vec<u8>), String> {
    if bytes.len() < VISION_MIN_BYTES {
        return Err(format!(
            "Image too small to be valid ({} bytes)",
            bytes.len()
        ));
    }
    if bytes.len() > VISION_MAX_BYTES {
        return Err(format!(
            "Image too large for vision API: {} bytes (limit {} bytes)",
            bytes.len(),
            VISION_MAX_BYTES,
        ));
    }
    let mime = detect_image_mime(&bytes).ok_or_else(|| {
        "Unsupported image format (only PNG, JPEG, GIF, BMP, WEBP are supported)".to_string()
    })?;
    debug_assert!(SUPPORTED_MIME_PREFIXES.contains(&mime));
    Ok((mime, bytes))
}

/// Source of an image — either a remote URL the fetcher must resolve,
/// or raw bytes already loaded by the caller (used for hermetic tests).
#[derive(Debug, Clone)]
pub enum ImageSource<'a> {
    Url(&'a str),
    Bytes { mime: &'static str, bytes: Vec<u8> },
}

/// Pluggable image-fetcher boundary. Hosts that want URL support wire
/// an implementation that downloads bytes (with SSRF redirect guards,
/// retry, etc.); the engine deliberately ships **no concrete HTTP
/// client** to keep `wcore-tools` link-time stable.
#[async_trait]
pub trait ImageFetcher: Send + Sync {
    /// Fetch the bytes of an image identified by URL. Returns the raw
    /// body — the caller will MIME-sniff and size-check.
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Default fetcher returned when the host wires nothing — every
/// `fetch()` fails loudly. Tests that don't need URL support can
/// rely on this; integration tests provide a `StaticImageFetcher`.
pub struct NullImageFetcher;

#[async_trait]
impl ImageFetcher for NullImageFetcher {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        Err(
            "No image fetcher configured. Wire an ImageFetcher implementation when constructing \
             VisionAnalyzeTool to enable URL-based vision analysis."
                .to_string(),
        )
    }
}

/// Hermetic test fetcher — returns a fixed payload for any URL.
/// Lives in the prod module so downstream crates can reuse it
/// without depending on `#[cfg(test)]` symbols (mirrors
/// `CapturingMessageTransport` in `send_message.rs`).
pub struct StaticImageFetcher {
    pub payload: Vec<u8>,
}

impl StaticImageFetcher {
    pub fn new(payload: Vec<u8>) -> Self {
        Self { payload }
    }
}

#[async_trait]
impl ImageFetcher for StaticImageFetcher {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        Ok(self.payload.clone())
    }
}

/// Outcome of a vision-backend call. Mirrors the JSON shape the
/// Python tool serializes back to the model (`success` / `error`
/// dicts).
#[derive(Debug, Clone)]
pub enum VisionOutcome {
    Ok { analysis: String },
    Err { message: String },
}

/// Pluggable LLM backend that performs the actual multimodal call.
/// The host wires this at registration time; the engine never embeds
/// a multimodal provider directly. Mirrors the `MessageTransport`
/// pattern in `send_message.rs`.
#[async_trait]
pub trait VisionBackend: Send + Sync {
    /// Analyze `bytes` (already MIME-sniffed) with `prompt`. The
    /// backend is free to base64-encode, downscale, retry on
    /// vendor-specific errors, etc.
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome;
}

/// Default backend returned when the host wires nothing — every
/// `analyze()` fails loudly with a structured error so the tool
/// never appears to succeed silently. Honors the NO-STUBS contract.
pub struct NullVisionBackend;

#[async_trait]
impl VisionBackend for NullVisionBackend {
    async fn analyze(&self, _mime: &'static str, _bytes: &[u8], _prompt: &str) -> VisionOutcome {
        VisionOutcome::Err {
            message: "No vision backend configured. Wire a VisionBackend implementation when \
                      constructing VisionAnalyzeTool to enable image analysis."
                .to_string(),
        }
    }
}

/// In-memory backend that captures every analyze call for assertions
/// in tests, returning a canned response. Lives in the prod module so
/// downstream crates can reuse it (mirrors `CapturingMessageTransport`).
pub struct CapturingVisionBackend {
    response: String,
    pub captured: parking_lot::Mutex<Vec<CapturedAnalyze>>,
}

/// Single captured analyze invocation — useful for assertions.
#[derive(Debug, Clone)]
pub struct CapturedAnalyze {
    pub mime: &'static str,
    pub bytes_len: usize,
    pub prompt: String,
}

impl CapturingVisionBackend {
    pub fn new(canned_response: impl Into<String>) -> Self {
        Self {
            response: canned_response.into(),
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }
    pub fn snapshot(&self) -> Vec<CapturedAnalyze> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl VisionBackend for CapturingVisionBackend {
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome {
        self.captured.lock().push(CapturedAnalyze {
            mime,
            bytes_len: bytes.len(),
            prompt: prompt.to_string(),
        });
        VisionOutcome::Ok {
            analysis: self.response.clone(),
        }
    }
}

/// `vision_analyze` tool — Wayland engine port of `vision_tools.py`.
///
/// Construct via [`VisionAnalyzeTool::new`] passing a `VisionBackend`
/// (required — host-wired multimodal LLM) and an `ImageFetcher`
/// (required — host-wired HTTP client). Use `default()` for the
/// null-backed fail-loud variant in tests or when vision support is
/// not yet wired.
pub struct VisionAnalyzeTool {
    backend: Arc<dyn VisionBackend>,
    fetcher: Arc<dyn ImageFetcher>,
    /// `true` when [`new`] was called with a real backend, `false` for
    /// the null-backed [`default`]. Drives [`Tool::is_available`] so the
    /// agent's tool registry can skip this tool when no host backend is
    /// wired (advertising a tool that always errors is worse than not
    /// advertising it).
    backend_configured: bool,
}

impl Default for VisionAnalyzeTool {
    fn default() -> Self {
        // Bypass `new` so `backend_configured` reflects the null default.
        Self {
            backend: Arc::new(NullVisionBackend),
            fetcher: Arc::new(NullImageFetcher),
            backend_configured: false,
        }
    }
}

impl VisionAnalyzeTool {
    pub fn new(backend: Arc<dyn VisionBackend>, fetcher: Arc<dyn ImageFetcher>) -> Self {
        Self {
            backend,
            fetcher,
            backend_configured: true,
        }
    }

    /// Resolve an `image_url` argument into raw bytes + sniffed MIME.
    /// Pure async function — no Tool trait state needed. Exposed pub
    /// for backend authors who want to share validation logic.
    pub async fn resolve_source(&self, image_url: &str) -> Result<(&'static str, Vec<u8>), String> {
        // 1-3. Load raw bytes from a LOCAL file or a remote URL. A dropped
        //       local image (absolute path or `file://` URI) is read from disk;
        //       an `http(s)` URL is fetched via the host-wired fetcher.
        let bytes = match local_image_path(image_url)? {
            Some(_) => return load_local_image(image_url),
            None => {
                // Validate URL shape and SSRF safety.
                validate_image_url(image_url)?;
                // Optional website-policy blocklist check (fails open on
                // config errors — same semantics as the Python original).
                match check_website_access(image_url, None) {
                    Ok(Some(block)) => return Err(block.message),
                    Ok(None) => {}
                    Err(e) => {
                        // Match Python's behaviour: missing/broken policy
                        // config is non-fatal at the call site.
                        tracing::warn!(
                            target: "wcore_tools::vision_tools",
                            "website_policy error: {e}"
                        );
                    }
                }
                // Fetch raw bytes via the host-wired fetcher.
                self.fetcher.fetch(image_url).await?
            }
        };
        // 4. Size + MIME validation for the fetched source.
        validate_image_bytes(bytes)
    }
}

#[async_trait]
impl Tool for VisionAnalyzeTool {
    fn name(&self) -> &str {
        "vision_analyze"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Analyze images using AI vision. Provides a comprehensive description of the image and \
         answers a specific question about its content. Accepts an http(s):// URL OR a local \
         image file (an absolute path or a file:// URI) — use the local path for a file the \
         user dropped in or one you produced. For URLs, private/internal addresses and \
         policy-blocked hosts are rejected; local paths are subject to the same path-safety \
         rules as the Read tool. Accepted formats: PNG, JPEG, GIF, BMP, WEBP. Hard size cap: \
         20 MB."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "image_url": {
                    "type": "string",
                    "description": "The image to analyze: an http(s):// URL, an absolute local \
                                    file path, or a file:// URI. Prefer the local path for a \
                                    file the user provided on disk."
                },
                "question": {
                    "type": "string",
                    "description": "Specific question or request about the image. The model will \
                                    also produce a full description of the image."
                }
            },
            "required": ["image_url", "question"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Vision is read-only over an external resource — safe to run
        // multiple in parallel (matches Read/Grep/Glob).
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let image_url = match input.get("image_url").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim(),
            _ => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": "Missing required parameter: 'image_url'",
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };
        let question = match input.get("question").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => s,
            _ => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": "Missing required parameter: 'question'",
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };

        let (mime, bytes) = match self.resolve_source(image_url).await {
            Ok(pair) => pair,
            Err(e) => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": e,
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };

        // Mirror the Python prompt format so existing prompt examples
        // keep working.
        let full_prompt = format!(
            "Fully describe and explain everything about this image, then answer the following \
             question:\n\n{question}"
        );

        match self.backend.analyze(mime, &bytes, &full_prompt).await {
            VisionOutcome::Ok { analysis } => ToolResult {
                content: json!({
                    "success": true,
                    "analysis": analysis,
                    "mime": mime,
                    "bytes": bytes.len(),
                })
                .to_string(),
                is_error: false,
            },
            VisionOutcome::Err { message } => ToolResult {
                content: json!({
                    "success": false,
                    "error": message,
                })
                .to_string(),
                is_error: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Minimal valid PNG header bytes (89 50 4E 47 ...) — enough to
    /// pass `detect_image_mime` and the min-size check.
    fn png_bytes() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        // 8 bytes of dummy IHDR-ish payload (we only sniff the header).
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    /// Minimal JPEG header (FF D8 FF) + filler.
    fn jpeg_bytes() -> Vec<u8> {
        let mut v = vec![0xff, 0xd8, 0xff, 0xe0];
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    #[test]
    fn detect_image_mime_recognizes_each_format() {
        assert_eq!(detect_image_mime(&png_bytes()), Some("image/png"));
        assert_eq!(detect_image_mime(&jpeg_bytes()), Some("image/jpeg"));

        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0u8; 16]);
        assert_eq!(detect_image_mime(&gif), Some("image/gif"));

        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0u8; 32]);
        assert_eq!(detect_image_mime(&bmp), Some("image/bmp"));

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0u8; 4]); // file-size field
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&[0u8; 16]);
        assert_eq!(detect_image_mime(&webp), Some("image/webp"));

        // Non-image bytes return None.
        assert_eq!(detect_image_mime(b"plain text body"), None);
        assert_eq!(detect_image_mime(b""), None);
        assert_eq!(detect_image_mime(&[0u8; 3]), None);
    }

    #[test]
    fn validate_image_url_accepts_http_and_https() {
        assert!(validate_image_url("http://example.com/img.png").is_ok());
        assert!(validate_image_url("https://example.com/img.jpg").is_ok());
    }

    #[test]
    fn validate_image_url_rejects_unsupported_schemes_and_ssrf() {
        // Non-http schemes.
        assert!(validate_image_url("file:///etc/passwd").is_err());
        assert!(validate_image_url("ftp://example.com/x.jpg").is_err());
        assert!(validate_image_url("").is_err());
        // Loopback / private (SSRF).
        assert!(validate_image_url("http://127.0.0.1/x.png").is_err());
        assert!(validate_image_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_image_url("http://10.0.0.1/img.jpg").is_err());
    }

    fn must_exec(t: &VisionAnalyzeTool, input: Value) -> ToolResult {
        futures::executor::block_on(t.execute(input))
    }

    /// Happy path with `CapturingVisionBackend` + `StaticImageFetcher`
    /// — no network, no filesystem (other than tempdir below).
    #[test]
    fn happy_path_calls_backend_with_sniffed_mime() {
        let backend = Arc::new(CapturingVisionBackend::new("a kitten on a sofa"));
        let fetcher = Arc::new(StaticImageFetcher::new(png_bytes()));
        let tool = VisionAnalyzeTool::new(backend.clone(), fetcher);
        let result = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/cat.png",
                "question": "What animal is this?",
            }),
        );
        assert!(!result.is_error, "got error result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["mime"], json!("image/png"));
        assert_eq!(parsed["analysis"], json!("a kitten on a sofa"));
        // Backend saw a single call with the PNG bytes and a prompt
        // composed from the user's question.
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].mime, "image/png");
        assert_eq!(snap[0].bytes_len, png_bytes().len());
        assert!(snap[0].prompt.contains("What animal is this?"));
        assert!(snap[0].prompt.contains("Fully describe"));
    }

    /// Hermetic tempdir fixture handling — we read a PNG fixture from
    /// a tempdir, pass it to `StaticImageFetcher`, and verify the tool
    /// processes the file content (not the URL).
    #[test]
    fn hermetic_tempdir_fixture() {
        let dir = tempdir().expect("tempdir");
        let fixture_path = dir.path().join("fixture.jpg");
        fs::write(&fixture_path, jpeg_bytes()).expect("write fixture");
        let bytes = fs::read(&fixture_path).expect("read fixture");
        assert_eq!(detect_image_mime(&bytes), Some("image/jpeg"));

        let backend = Arc::new(CapturingVisionBackend::new("a JPEG image"));
        let fetcher = Arc::new(StaticImageFetcher::new(bytes));
        let tool = VisionAnalyzeTool::new(backend.clone(), fetcher);
        let result = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/photo.jpg",
                "question": "describe",
            }),
        );
        assert!(!result.is_error, "got error result: {}", result.content);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].mime, "image/jpeg");
    }

    // ── #637: local image file support ─────────────────────────────────

    #[test]
    fn local_image_path_classifies_url_vs_file() {
        // http(s) URLs stay on the fetcher path.
        assert_eq!(local_image_path("http://example.com/x.png"), Ok(None));
        assert_eq!(local_image_path("https://example.com/x.png"), Ok(None));
        // A `file://` URI strips to its absolute path on every platform.
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("x.png");
        let uri = url::Url::from_file_path(&path).expect("absolute file URL");
        assert_eq!(local_image_path(uri.as_str()), Ok(Some(path.clone())));
        // A bare path (what a desktop drop sends) is classified as local;
        // `load_local_image` performs the absolute-path safety check.
        assert_eq!(local_image_path(path.to_str().unwrap()), Ok(Some(path)));
    }

    #[test]
    fn local_image_loader_rejects_corrupt_and_oversized_files() {
        let dir = tempdir().expect("tempdir");
        let corrupt = dir.path().join("corrupt.png");
        fs::write(&corrupt, b"not an image payload").expect("write corrupt image");
        assert!(load_local_image(corrupt.to_str().unwrap()).is_err());

        let oversized = dir.path().join("oversized.png");
        let file = fs::File::create(&oversized).expect("create oversized image");
        file.set_len(VISION_MAX_BYTES as u64 + 1)
            .expect("extend sparse fixture");
        let error = load_local_image(oversized.to_str().unwrap()).unwrap_err();
        assert!(error.contains("too large"), "unexpected error: {error}");
    }

    #[test]
    fn post_open_growth_limit_is_fail_closed() {
        let bytes = vec![0; VISION_MAX_BYTES + 1];
        let error = validate_image_bytes(bytes).unwrap_err();
        assert!(error.contains("too large"), "unexpected error: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn local_image_loader_rejects_symlink_components_and_fifo() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("tempdir");
        let target = dir.path().join("target.png");
        let link = dir.path().join("link.png");
        fs::write(&target, png_bytes()).expect("write target");
        symlink(&target, &link).expect("create symlink");
        assert!(load_local_image(link.to_str().unwrap()).is_err());

        let actual_parent = dir.path().join("actual-parent");
        let linked_parent = dir.path().join("linked-parent");
        fs::create_dir(&actual_parent).expect("create actual parent");
        let nested_target = actual_parent.join("nested.png");
        fs::write(&nested_target, png_bytes()).expect("write nested target");
        symlink(&actual_parent, &linked_parent).expect("create parent symlink");
        let error =
            load_local_image(linked_parent.join("nested.png").to_str().unwrap()).unwrap_err();
        assert!(
            error.contains("open image path component"),
            "unexpected error: {error}"
        );

        let fifo = dir.path().join("pipe.png");
        let fifo_c = CString::new(fifo.as_os_str().as_bytes()).expect("fifo path");
        // SAFETY: `fifo_c` is a valid NUL-terminated pathname and the mode is
        // a conventional owner-only fixture permission.
        assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
        let error = load_local_image(fifo.to_str().unwrap()).unwrap_err();
        assert!(
            error.contains("regular file")
                || error.contains("regular image file")
                || error.contains("open image file"),
            "unexpected error: {error}"
        );
    }

    /// A dropped local image resolves from disk with the correct sniffed MIME,
    /// WITHOUT touching the fetcher (this tool uses the null fetcher).
    #[test]
    fn resolve_source_reads_local_png_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("dropped.png");
        fs::write(&path, png_bytes()).expect("write png");
        let tool = VisionAnalyzeTool::default(); // NullImageFetcher
        let (mime, bytes) =
            futures::executor::block_on(tool.resolve_source(path.to_str().unwrap()))
                .expect("local file should resolve");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, png_bytes());
    }

    /// A `file://` URI form of a local path also resolves.
    #[test]
    fn resolve_source_reads_local_file_via_file_uri() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("dropped.jpg");
        fs::write(&path, jpeg_bytes()).expect("write jpeg");
        let uri = format!("file://{}", path.to_str().unwrap());
        let tool = VisionAnalyzeTool::default();
        let (mime, _) = futures::executor::block_on(tool.resolve_source(&uri))
            .expect("file:// path should resolve");
        assert_eq!(mime, "image/jpeg");
    }

    /// Path safety is enforced exactly as the Read tool: non-absolute and
    /// `..`-traversal args are rejected, and a missing file errors cleanly
    /// rather than panicking.
    #[test]
    fn resolve_source_enforces_path_safety_on_local_files() {
        let tool = VisionAnalyzeTool::default();
        // Relative path — validate_user_path requires absolute.
        assert!(
            futures::executor::block_on(tool.resolve_source("relative/x.png")).is_err(),
            "relative path must be rejected"
        );
        // `..` traversal — rejected before any read.
        assert!(
            futures::executor::block_on(tool.resolve_source("/tmp/../etc/passwd")).is_err(),
            "traversal must be rejected"
        );
        // Absolute-but-missing — a clean error, not a panic.
        assert!(
            futures::executor::block_on(
                tool.resolve_source("/nonexistent/wayland/vision/missing.png")
            )
            .is_err(),
            "missing file must error"
        );
    }

    /// A non-regular file (here a directory) is refused — the same guard that
    /// closes the /dev/zero / FIFO read-hang DoS on special files.
    #[test]
    fn resolve_source_rejects_non_regular_file() {
        let dir = tempdir().expect("tempdir");
        let tool = VisionAnalyzeTool::default();
        let res = futures::executor::block_on(tool.resolve_source(dir.path().to_str().unwrap()));
        assert!(res.is_err(), "a directory must be rejected, got: {res:?}");
    }

    #[test]
    fn is_network_path_flags_unc_only() {
        // Ordinary paths are never network paths (the common case).
        assert!(!is_network_path(std::path::Path::new("/Users/me/x.png")));
        assert!(!is_network_path(std::path::Path::new("relative/x.png")));
        // A UNC path is flagged on Windows; on Unix the same string carries no
        // UNC prefix, so the platform-correct value there is `false`.
        #[cfg(windows)]
        assert!(is_network_path(std::path::Path::new(
            r"\\server\share\x.png"
        )));
    }

    /// End-to-end: a local image drives the backend, with the null fetcher
    /// proving the local path never crosses the network seam.
    #[test]
    fn execute_analyzes_local_file_end_to_end() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("photo.png");
        fs::write(&path, png_bytes()).expect("write png");
        let backend = Arc::new(CapturingVisionBackend::new("a local png"));
        let tool = VisionAnalyzeTool::new(backend.clone(), Arc::new(NullImageFetcher));
        let result = must_exec(
            &tool,
            json!({ "image_url": path.to_str().unwrap(), "question": "what is this?" }),
        );
        assert!(!result.is_error, "got error result: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["mime"], json!("image/png"));
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].mime, "image/png");
    }

    /// Null backend fails loudly with the structured error message —
    /// the no-stub contract.
    #[test]
    fn null_backend_fails_loudly() {
        let tool = VisionAnalyzeTool::new(
            Arc::new(NullVisionBackend),
            Arc::new(StaticImageFetcher::new(png_bytes())),
        );
        let r = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/x.png",
                "question": "?",
            }),
        );
        assert!(r.is_error);
        assert!(
            r.content.contains("No vision backend configured"),
            "got: {}",
            r.content
        );
    }

    /// Null fetcher (`default()`) fails loudly before the backend is
    /// ever called.
    #[test]
    fn null_fetcher_fails_loudly() {
        let tool = VisionAnalyzeTool::default();
        let r = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/x.png",
                "question": "?",
            }),
        );
        assert!(r.is_error);
        assert!(
            r.content.contains("No image fetcher configured"),
            "got: {}",
            r.content
        );
    }

    /// Missing / empty inputs return structured errors.
    #[test]
    fn invalid_input_rejected() {
        let tool = VisionAnalyzeTool::new(
            Arc::new(CapturingVisionBackend::new("x")),
            Arc::new(StaticImageFetcher::new(png_bytes())),
        );

        // Missing image_url.
        let r = must_exec(&tool, json!({ "question": "what?" }));
        assert!(r.is_error);
        assert!(r.content.contains("image_url"));

        // Missing question.
        let r = must_exec(&tool, json!({ "image_url": "https://example.com/a.png" }));
        assert!(r.is_error);
        assert!(r.content.contains("question"));

        // Empty strings count as missing.
        let r = must_exec(&tool, json!({ "image_url": "", "question": "what?" }));
        assert!(r.is_error);
    }

    /// Unsupported / non-image payloads are rejected after MIME sniff.
    #[test]
    fn unsupported_mime_rejected() {
        let backend = Arc::new(CapturingVisionBackend::new("never called"));
        // Payload is plain text — MIME sniff returns None.
        let bytes = b"<!DOCTYPE html><html><body>nope</body></html>".to_vec();
        let fetcher = Arc::new(StaticImageFetcher::new(bytes));
        let tool = VisionAnalyzeTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/page.html",
                "question": "?",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Unsupported image format"));
        // Backend must NOT have been called.
        assert_eq!(backend.snapshot().len(), 0);
    }

    /// SSRF-blocked URL never reaches the fetcher.
    #[test]
    fn ssrf_url_blocked_before_fetch() {
        let backend = Arc::new(CapturingVisionBackend::new("never called"));
        let fetcher = Arc::new(StaticImageFetcher::new(png_bytes()));
        let tool = VisionAnalyzeTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({
                "image_url": "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
                "question": "?",
            }),
        );
        assert!(r.is_error);
        assert!(
            r.content.contains("SSRF") || r.content.contains("blocked"),
            "got: {}",
            r.content
        );
        // Backend must NOT have been called.
        assert_eq!(backend.snapshot().len(), 0);
    }

    /// Oversized payload (> VISION_MAX_BYTES) is rejected before
    /// reaching the backend.
    #[test]
    fn oversized_payload_rejected() {
        let backend = Arc::new(CapturingVisionBackend::new("never called"));
        // Build a real PNG header + filler exceeding the cap.
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(VISION_MAX_BYTES + 1024, 0u8);
        let fetcher = Arc::new(StaticImageFetcher::new(bytes));
        let tool = VisionAnalyzeTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({
                "image_url": "https://example.com/huge.png",
                "question": "?",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("too large"), "got: {}", r.content);
        assert_eq!(backend.snapshot().len(), 0);
    }

    /// Tool registers in the dispatcher with the expected schema when a
    /// real backend is wired. The null-backed `default()` is now
    /// silently skipped by `ToolRegistry::register` (it overrides
    /// `Tool::is_available` to return `false`), so the test wires a
    /// `CapturingVisionBackend` to exercise the "actually registered"
    /// path.
    #[test]
    fn tool_registers_in_registry_when_backend_is_wired() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        let tool = VisionAnalyzeTool::new(
            Arc::new(CapturingVisionBackend::new("test-response")),
            Arc::new(NullImageFetcher),
        );
        reg.register(Box::new(tool));
        let defs = reg.to_tool_defs();
        let found = defs.iter().find(|d| d.name == "vision_analyze");
        assert!(
            found.is_some(),
            "vision_analyze must be present in registry when backend is wired"
        );
        let def = found.unwrap();
        let schema = &def.input_schema;
        let required = schema["required"].as_array().expect("required array");
        let required_strs: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(required_strs.contains(&"image_url"));
        assert!(required_strs.contains(&"question"));
    }

    /// The null-backed `default()` is NOT registered — advertising a tool
    /// that always errors burns turns in the agent loop.
    #[test]
    fn null_backed_default_is_skipped_by_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(VisionAnalyzeTool::default()));
        let defs = reg.to_tool_defs();
        let found = defs.iter().find(|d| d.name == "vision_analyze");
        assert!(
            found.is_none(),
            "null-backed default must NOT appear in the registry"
        );
    }
}
