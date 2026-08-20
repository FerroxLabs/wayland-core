//! T3-3.6 — `transcribe_audio` speech-to-text tool.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md).
//! The Python original supports four backends (faster-whisper local,
//! Groq Whisper, OpenAI Whisper, Mistral Voxtral) with provider
//! auto-detection from env vars. Wayland's engine deliberately ships
//! **no embedded STT model** — speech-to-text is a host concern that
//! either binds a local model crate or a remote API client. To honor
//! the NO-STUBS contract of T3, this port covers the **dispatch
//! surface and safety boundary** only:
//!
//! * Schema + input validation (path vs. URL, supported audio
//!   formats, hard size cap).
//! * SSRF defense for URL inputs — reuses `url_safety::is_safe_url`
//!   and `website_policy::check_website_access` (same composition as
//!   `vision_tools`).
//! * Hard size cap: 25 MB (matches the Python original and the
//!   OpenAI Whisper API limit — the most restrictive among the
//!   supported backends).
//! * Inline magic-byte MIME sniffing for the supported audio formats
//!   (mp3, mp4/m4a, wav, ogg, webm, flac, aac). No `infer`, `mime`,
//!   or `symphonia` crate is pulled in — keeps `wcore-tools`
//!   link-time identical to the pre-port baseline.
//! * Two pluggable seams (mirror of the `VisionBackend` / `ImageFetcher`
//!   pattern in `vision_tools.rs`):
//!     * `AudioFetcher` — fetches bytes from an HTTP/HTTPS URL.
//!     * `TranscriptionBackend` — sends `{mime, bytes, language?}` to
//!       a real STT model and returns the transcript text (plus
//!       optional per-segment timestamps).
//! * `NullAudioFetcher` / `NullTranscriptionBackend` fail loudly with
//!   structured errors rather than silently succeeding — this is the
//!   no-stub guarantee.
//! * `CapturingTranscriptionBackend` + `StaticAudioFetcher` for
//!   hermetic testing (no network).
//!
//! Divergences from the Python original (intentional):
//! * No env-var-driven provider auto-detection. The host wires the
//!   backend explicitly; provider preference is a host policy concern
//!   and lives in `wcore-config`, not in the tool crate.
//! * No ffmpeg shell-out for format conversion. The backend receives
//!   the raw bytes plus sniffed MIME and decides whether it needs
//!   conversion (provider-specific concern).
//! * No on-disk temp directory for converted audio. The fetched
//!   payload is held in memory only for the duration of the
//!   `execute()` call.
//! * No model auto-correction across providers. The backend owns
//!   model selection.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::media_intake::{AUDIO_KINDS, IntakeError, IntakePolicy, MediaKind, admit_bytes};
use crate::url_safety::is_safe_url;
use crate::website_policy::check_website_access;

/// Hard cap on raw audio bytes (25 MB). Mirrors the Python original's
/// `MAX_FILE_SIZE` and matches the OpenAI Whisper API limit.
pub const TRANSCRIPTION_MAX_BYTES: usize = 25 * 1024 * 1024;

/// Lower-bound sanity check — anything shorter than 16 bytes cannot
/// hold a real container header.
pub const TRANSCRIPTION_MIN_BYTES: usize = 16;

/// MIME types accepted by `transcribe_audio`.
const SUPPORTED_AUDIO_MIMES: &[&str] = &[
    "audio/mpeg",
    "audio/mp4",
    "audio/aac",
    "audio/wav",
    "audio/ogg",
    "audio/webm",
    "audio/flac",
];

/// The [`IntakePolicy`] this surface applies over the shared media intake.
/// The caps and the accepted set are this tool's; the mechanism is not.
pub fn audio_intake_policy() -> IntakePolicy {
    IntakePolicy::new(
        TRANSCRIPTION_MIN_BYTES as u64,
        TRANSCRIPTION_MAX_BYTES as u64,
    )
    .accepting(AUDIO_KINDS)
    .named("audio")
}

/// Result of sniffing an audio file's magic bytes. Returns `None`
/// when the header doesn't match any supported audio container.
///
/// Delegates to [`MediaKind::from_magic`] — the single magic-byte table for
/// every media class the engine ingests — and narrows it to audio. Keeping a
/// second table here is how a WAV came to be classified as a WebP image on the
/// other side of the tree.
///
/// Magic-byte references:
/// * MP3: `FF Fx` (MPEG sync) or `ID3` (ID3v2 header).
/// * MP4 / M4A: `?? ?? ?? ?? 66 74 79 70` (ftyp atom at offset 4).
/// * WAV: `RIFF` ... `WAVE`.
/// * OGG: `OggS`.
/// * WEBM / MKV: `1A 45 DF A3` (EBML).
/// * FLAC: `fLaC`.
/// * AAC ADTS: `FF F1` / `FF F9`.
pub fn detect_audio_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    MediaKind::from_magic(bytes)
        .filter(|k| k.is_audio())
        .map(MediaKind::as_str)
}

/// Render a shared-intake refusal in this surface's historical wording, so
/// unifying the mechanism does not churn a string a host may be matching on.
fn render_audio_intake_error(e: IntakeError) -> String {
    match e {
        IntakeError::TooLarge { actual, .. } => {
            format!("Audio too large: {actual} bytes (limit {TRANSCRIPTION_MAX_BYTES} bytes)")
        }
        IntakeError::TooSmall { actual } => {
            format!("Audio too small to be valid ({actual} bytes)")
        }
        IntakeError::NotRegularFile(p) => {
            format!("Audio path is not a regular file: {}", p.display())
        }
        IntakeError::UnrecognisedFormat
        | IntakeError::FormatMismatch { .. }
        | IntakeError::KindNotAccepted { .. } => UNSUPPORTED_AUDIO_FORMAT.to_string(),
        other => other.to_string(),
    }
}

const UNSUPPORTED_AUDIO_FORMAT: &str =
    "Unsupported audio format (only MP3, MP4/M4A, AAC, WAV, OGG, WEBM, FLAC are supported)";

/// Validate that a string looks like a transcription-acceptable URL.
pub fn validate_audio_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL is empty".to_string());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "Only http:// and https:// URLs are supported (got: {})",
            url.chars().take(40).collect::<String>()
        ));
    }
    if !is_safe_url(url) {
        return Err(format!(
            "URL blocked by SSRF safety policy: {}",
            url.chars().take(80).collect::<String>()
        ));
    }
    Ok(())
}

/// Source of audio bytes — URL, local path, or raw bytes.
#[derive(Debug, Clone)]
pub enum AudioSource {
    Url(String),
    Path(PathBuf),
    Bytes { mime: &'static str, bytes: Vec<u8> },
}

/// Pluggable audio-fetcher boundary.
#[async_trait]
pub trait AudioFetcher: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Default fetcher — every fetch fails loudly.
pub struct NullAudioFetcher;

#[async_trait]
impl AudioFetcher for NullAudioFetcher {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        Err(
            "No audio fetcher configured. Wire an AudioFetcher implementation when \
             constructing TranscribeAudioTool to enable URL-based transcription."
                .to_string(),
        )
    }
}

/// Hermetic test fetcher — returns a fixed payload for any URL.
pub struct StaticAudioFetcher {
    pub payload: Vec<u8>,
}

impl StaticAudioFetcher {
    pub fn new(payload: Vec<u8>) -> Self {
        Self { payload }
    }
}

#[async_trait]
impl AudioFetcher for StaticAudioFetcher {
    async fn fetch(&self, _url: &str) -> Result<Vec<u8>, String> {
        Ok(self.payload.clone())
    }
}

/// One transcript segment with start/end seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    pub start_seconds: f32,
    pub end_seconds: f32,
    pub text: String,
}

/// Outcome of a transcription backend call.
#[derive(Debug, Clone)]
pub enum TranscriptionOutcome {
    Ok {
        transcript: String,
        language: Option<String>,
        segments: Vec<TranscriptSegment>,
    },
    Err {
        message: String,
    },
}

/// Pluggable STT backend that performs the actual transcription.
#[async_trait]
pub trait TranscriptionBackend: Send + Sync {
    async fn transcribe(
        &self,
        mime: &'static str,
        bytes: &[u8],
        language: Option<&str>,
    ) -> TranscriptionOutcome;
}

/// Default backend — every transcribe fails loudly. NO-STUBS.
pub struct NullTranscriptionBackend;

#[async_trait]
impl TranscriptionBackend for NullTranscriptionBackend {
    async fn transcribe(
        &self,
        _mime: &'static str,
        _bytes: &[u8],
        _language: Option<&str>,
    ) -> TranscriptionOutcome {
        TranscriptionOutcome::Err {
            message: "No transcription backend configured. Wire a TranscriptionBackend \
                      implementation when constructing TranscribeAudioTool to enable \
                      speech-to-text."
                .to_string(),
        }
    }
}

/// In-memory backend that captures every transcribe call for tests.
pub struct CapturingTranscriptionBackend {
    transcript: String,
    language: Option<String>,
    segments: Vec<TranscriptSegment>,
    pub captured: parking_lot::Mutex<Vec<CapturedTranscribe>>,
}

/// Single captured transcribe invocation.
#[derive(Debug, Clone)]
pub struct CapturedTranscribe {
    pub mime: &'static str,
    pub bytes_len: usize,
    pub language: Option<String>,
}

impl CapturingTranscriptionBackend {
    pub fn new(canned_transcript: impl Into<String>) -> Self {
        Self {
            transcript: canned_transcript.into(),
            language: None,
            segments: Vec::new(),
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    pub fn with_segments(mut self, segments: Vec<TranscriptSegment>) -> Self {
        self.segments = segments;
        self
    }

    pub fn snapshot(&self) -> Vec<CapturedTranscribe> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl TranscriptionBackend for CapturingTranscriptionBackend {
    async fn transcribe(
        &self,
        mime: &'static str,
        bytes: &[u8],
        language: Option<&str>,
    ) -> TranscriptionOutcome {
        self.captured.lock().push(CapturedTranscribe {
            mime,
            bytes_len: bytes.len(),
            language: language.map(|s| s.to_string()),
        });
        TranscriptionOutcome::Ok {
            transcript: self.transcript.clone(),
            language: self.language.clone(),
            segments: self.segments.clone(),
        }
    }
}

/// `transcribe_audio` tool — Wayland engine port of
/// `transcription_tools.py`.
pub struct TranscribeAudioTool {
    backend: Arc<dyn TranscriptionBackend>,
    fetcher: Arc<dyn AudioFetcher>,
    backend_configured: bool,
}

impl Default for TranscribeAudioTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullTranscriptionBackend),
            fetcher: Arc::new(NullAudioFetcher),
            backend_configured: false,
        }
    }
}

impl TranscribeAudioTool {
    pub fn new(backend: Arc<dyn TranscriptionBackend>, fetcher: Arc<dyn AudioFetcher>) -> Self {
        Self {
            backend,
            fetcher,
            backend_configured: true,
        }
    }

    /// Resolve a source (URL, local path, or direct bytes) into raw
    /// bytes plus sniffed MIME.
    ///
    /// Every arm goes through the shared [`crate::media_intake`]. Before this
    /// was unified the local-path arm applied NO path validation — no
    /// absolute-path requirement, no traversal refusal, no UNC refusal (an
    /// outbound SMB connect and a NetNTLM leak on Windows), no system-path
    /// deny-list and no symlink discipline — and read with an unbounded
    /// `fs::read` issued against a SECOND by-name resolution, so the declared
    /// 25 MB cap bounded a `stat` the read never consulted.
    pub async fn resolve_source(
        &self,
        source: &AudioSource,
    ) -> Result<(&'static str, Vec<u8>), String> {
        let policy = audio_intake_policy();
        let bytes = match source {
            AudioSource::Bytes { mime, bytes } => {
                // Caller-supplied bytes still face the shared caps; the caller
                // has already decided the MIME, so only the bounds apply.
                let len = bytes.len();
                if len < TRANSCRIPTION_MIN_BYTES {
                    return Err(format!("Audio too small to be valid ({len} bytes)"));
                }
                if len > TRANSCRIPTION_MAX_BYTES {
                    return Err(format!(
                        "Audio too large: {} bytes (limit {} bytes)",
                        len, TRANSCRIPTION_MAX_BYTES,
                    ));
                }
                return Ok((*mime, bytes.clone()));
            }
            AudioSource::Url(url) => {
                validate_audio_url(url)?;
                match check_website_access(url, None) {
                    Ok(Some(block)) => return Err(block.message),
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(
                            target: "wcore_tools::transcription_tools",
                            "website_policy error: {e}",
                        );
                    }
                }
                self.fetcher.fetch(url).await?
            }
            AudioSource::Path(path) => {
                // ONE resolution. The shared intake validates the path, opens
                // it exactly once through a symlink-refusing walk, bounds it
                // from THAT descriptor before any payload read, and proves the
                // container from the bytes it actually read.
                let admitted = crate::media_intake::admit_path(path, &policy)
                    .map_err(render_audio_intake_error)?;
                let mime = admitted.kind.as_str();
                debug_assert!(SUPPORTED_AUDIO_MIMES.contains(&mime));
                return Ok((mime, admitted.bytes));
            }
        };

        // Fetched bytes face the SAME caps and the SAME format decision a
        // local path would have faced.
        let kind = admit_bytes(&bytes, &policy).map_err(render_audio_intake_error)?;
        let mime = kind.as_str();
        debug_assert!(SUPPORTED_AUDIO_MIMES.contains(&mime));
        Ok((mime, bytes))
    }
}

#[async_trait]
impl Tool for TranscribeAudioTool {
    fn name(&self) -> &str {
        "transcribe_audio"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Transcribe speech to text from an audio file. Provide EITHER `audio_path` (local \
         file) OR `audio_url` (http/https URL — private/internal addresses are rejected). \
         Optionally pass `language` as an ISO-639-1 code (e.g. \"en\") to hint the model. \
         Supported formats: MP3, MP4/M4A, AAC, WAV, OGG, WEBM, FLAC. Hard size cap: 25 MB."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "audio_path": {
                    "type": "string",
                    "description": "Absolute or relative path to a local audio file."
                },
                "audio_url": {
                    "type": "string",
                    "description": "HTTP/HTTPS URL of the audio file to transcribe."
                },
                "language": {
                    "type": "string",
                    "description": "Optional ISO-639-1 language hint (e.g. \"en\", \"fr\")."
                }
            }
            // The top-level `oneOf` mutual-exclusion that used to live here
            // tripped Anthropic's tool-schema validator (it rejects oneOf /
            // allOf / anyOf at the top of input_schema, returning HTTP 400
            // for the entire request). Runtime `execute()` below still
            // enforces "exactly one of audio_path / audio_url".
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let path = input
            .get("audio_path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let url = input
            .get("audio_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let source = match (path, url) {
            (Some(p), None) => AudioSource::Path(PathBuf::from(p)),
            (None, Some(u)) => AudioSource::Url(u.to_string()),
            (Some(_), Some(_)) => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": "Specify exactly one of 'audio_path' or 'audio_url', not both.",
                    })
                    .to_string(),
                    is_error: true,
                };
            }
            (None, None) => {
                return ToolResult {
                    content: json!({
                        "success": false,
                        "error": "Missing required parameter: provide either 'audio_path' or 'audio_url'.",
                    })
                    .to_string(),
                    is_error: true,
                };
            }
        };

        let language = input
            .get("language")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let (mime, bytes) = match self.resolve_source(&source).await {
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

        match self.backend.transcribe(mime, &bytes, language).await {
            TranscriptionOutcome::Ok {
                transcript,
                language: detected,
                segments,
            } => {
                let segments_json: Vec<Value> = segments
                    .into_iter()
                    .map(|s| {
                        json!({
                            "start_seconds": s.start_seconds,
                            "end_seconds": s.end_seconds,
                            "text": s.text,
                        })
                    })
                    .collect();
                ToolResult {
                    content: json!({
                        "success": true,
                        "transcript": transcript,
                        "mime": mime,
                        "bytes": bytes.len(),
                        "language": detected,
                        "segments": segments_json,
                    })
                    .to_string(),
                    is_error: false,
                }
            }
            TranscriptionOutcome::Err { message } => ToolResult {
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
    // Audio-URL fixtures use a literal public IP, never a hostname.
    //
    // `validate_audio_url` -> `url_safety::is_safe_url` performs a REAL DNS
    // resolution and fails closed when it returns no addresses. With a
    // hostname these tests therefore require working DNS on the runner, and
    // six of them went red together on a macOS CI runner whose resolver
    // hiccuped. A literal IP takes the `parse::<IpAddr>()` fast path and skips
    // resolution entirely, so the tests exercise the tool rather than the
    // network. Hostname resolution is covered hermetically in `url_safety`'s
    // own tests via `is_safe_url_with(.., fake_resolver)`.
    //
    // The one surviving `example.com` below is deliberate: it is the `ftp://`
    // case, rejected on scheme before any resolution is attempted.
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn ogg_bytes() -> Vec<u8> {
        let mut v = b"OggS".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    fn mp3_bytes() -> Vec<u8> {
        let mut v = b"ID3".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    fn wav_bytes() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0u8; 4]);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&[0u8; 32]);
        v
    }

    fn flac_bytes() -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        v.extend_from_slice(&[0u8; 64]);
        v
    }

    fn m4a_bytes() -> Vec<u8> {
        let mut v = vec![0u8, 0, 0, 0x20];
        v.extend_from_slice(b"ftyp");
        v.extend_from_slice(b"M4A ");
        v.extend_from_slice(&[0u8; 24]);
        v
    }

    #[test]
    fn detect_audio_mime_recognizes_each_format() {
        assert_eq!(detect_audio_mime(&ogg_bytes()), Some("audio/ogg"));
        assert_eq!(detect_audio_mime(&mp3_bytes()), Some("audio/mpeg"));
        assert_eq!(detect_audio_mime(&wav_bytes()), Some("audio/wav"));
        assert_eq!(detect_audio_mime(&flac_bytes()), Some("audio/flac"));
        assert_eq!(detect_audio_mime(&m4a_bytes()), Some("audio/mp4"));

        let mut webm = vec![0x1A, 0x45, 0xDF, 0xA3];
        webm.extend_from_slice(&[0u8; 32]);
        assert_eq!(detect_audio_mime(&webm), Some("audio/webm"));

        let mut mpeg_sync = vec![0xFFu8, 0xFB];
        mpeg_sync.extend_from_slice(&[0u8; 32]);
        assert_eq!(detect_audio_mime(&mpeg_sync), Some("audio/mpeg"));

        let mut aac = vec![0xFFu8, 0xF1];
        aac.extend_from_slice(&[0u8; 32]);
        assert_eq!(detect_audio_mime(&aac), Some("audio/aac"));

        assert_eq!(detect_audio_mime(b"plain text body"), None);
        assert_eq!(detect_audio_mime(b""), None);
        assert_eq!(detect_audio_mime(&[0u8; 3]), None);
    }

    #[test]
    fn validate_audio_url_accepts_http_and_https() {
        assert!(validate_audio_url("http://93.184.216.34/a.mp3").is_ok());
        assert!(validate_audio_url("https://93.184.216.34/a.ogg").is_ok());
    }

    #[test]
    fn validate_audio_url_rejects_unsupported_schemes_and_ssrf() {
        assert!(validate_audio_url("file:///etc/passwd").is_err());
        assert!(validate_audio_url("ftp://example.com/a.mp3").is_err());
        assert!(validate_audio_url("").is_err());
        assert!(validate_audio_url("http://127.0.0.1/a.mp3").is_err());
        assert!(validate_audio_url("http://169.254.169.254/x.mp3").is_err());
        assert!(validate_audio_url("http://10.0.0.1/a.mp3").is_err());
    }

    fn must_exec(t: &TranscribeAudioTool, input: Value) -> ToolResult {
        futures::executor::block_on(t.execute(input))
    }

    #[test]
    fn happy_path_url_calls_backend_with_sniffed_mime() {
        let segments = vec![TranscriptSegment {
            start_seconds: 0.0,
            end_seconds: 2.5,
            text: "hello world".to_string(),
        }];
        let backend = Arc::new(
            CapturingTranscriptionBackend::new("hello world")
                .with_language("en")
                .with_segments(segments.clone()),
        );
        let fetcher = Arc::new(StaticAudioFetcher::new(ogg_bytes()));
        let tool = TranscribeAudioTool::new(backend.clone(), fetcher);

        let result = must_exec(
            &tool,
            json!({
                "audio_url": "https://93.184.216.34/voice.ogg",
                "language": "en",
            }),
        );
        assert!(!result.is_error, "got error: {}", result.content);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["mime"], json!("audio/ogg"));
        assert_eq!(parsed["transcript"], json!("hello world"));
        assert_eq!(parsed["language"], json!("en"));
        let segs = parsed["segments"].as_array().unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0]["text"], json!("hello world"));

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].mime, "audio/ogg");
        assert_eq!(snap[0].bytes_len, ogg_bytes().len());
        assert_eq!(snap[0].language, Some("en".to_string()));
    }

    #[test]
    fn happy_path_local_path_sniffs_from_content() {
        let dir = tempdir().expect("tempdir");
        let fixture = dir.path().join("clip.bogus");
        fs::write(&fixture, wav_bytes()).expect("write fixture");

        let backend = Arc::new(CapturingTranscriptionBackend::new("file said hi"));
        let tool = TranscribeAudioTool::new(backend.clone(), Arc::new(NullAudioFetcher));
        let r = must_exec(&tool, json!({ "audio_path": fixture.to_string_lossy() }));
        assert!(!r.is_error, "got error: {}", r.content);
        let parsed: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(parsed["mime"], json!("audio/wav"));
        assert_eq!(parsed["transcript"], json!("file said hi"));

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].mime, "audio/wav");
        assert!(snap[0].language.is_none());
    }

    #[test]
    fn null_backend_fails_loudly() {
        let tool = TranscribeAudioTool::new(
            Arc::new(NullTranscriptionBackend),
            Arc::new(StaticAudioFetcher::new(ogg_bytes())),
        );
        let r = must_exec(&tool, json!({ "audio_url": "https://93.184.216.34/a.ogg" }));
        assert!(r.is_error);
        assert!(
            r.content.contains("No transcription backend configured"),
            "got: {}",
            r.content
        );
    }

    #[test]
    fn null_fetcher_fails_loudly_on_url() {
        let tool = TranscribeAudioTool::default();
        let r = must_exec(&tool, json!({ "audio_url": "https://93.184.216.34/a.ogg" }));
        assert!(r.is_error);
        assert!(
            r.content.contains("No audio fetcher configured"),
            "got: {}",
            r.content
        );
    }

    #[test]
    fn invalid_input_rejected() {
        let tool = TranscribeAudioTool::new(
            Arc::new(CapturingTranscriptionBackend::new("nope")),
            Arc::new(StaticAudioFetcher::new(ogg_bytes())),
        );

        let r = must_exec(&tool, json!({}));
        assert!(r.is_error);
        assert!(r.content.contains("audio_path") || r.content.contains("audio_url"));

        let r = must_exec(
            &tool,
            json!({
                "audio_path": "/tmp/x.mp3",
                "audio_url": "https://93.184.216.34/x.mp3",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("exactly one"));

        let r = must_exec(&tool, json!({ "audio_url": "" }));
        assert!(r.is_error);
    }

    #[test]
    fn unsupported_mime_rejected() {
        let backend = Arc::new(CapturingTranscriptionBackend::new("never called"));
        let fetcher = Arc::new(StaticAudioFetcher::new(
            b"<!DOCTYPE html><html><body>nope</body></html>".to_vec(),
        ));
        let tool = TranscribeAudioTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({ "audio_url": "https://93.184.216.34/page.html" }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Unsupported audio format"));
        assert_eq!(backend.snapshot().len(), 0);
    }

    #[test]
    fn ssrf_url_blocked_before_fetch() {
        let backend = Arc::new(CapturingTranscriptionBackend::new("never called"));
        let fetcher = Arc::new(StaticAudioFetcher::new(ogg_bytes()));
        let tool = TranscribeAudioTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({
                "audio_url": "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            }),
        );
        assert!(r.is_error);
        assert!(
            r.content.contains("SSRF") || r.content.contains("blocked"),
            "got: {}",
            r.content
        );
        assert_eq!(backend.snapshot().len(), 0);
    }

    #[test]
    fn oversized_payload_rejected() {
        let backend = Arc::new(CapturingTranscriptionBackend::new("never called"));
        let mut bytes = b"OggS".to_vec();
        bytes.resize(TRANSCRIPTION_MAX_BYTES + 1024, 0u8);
        let fetcher = Arc::new(StaticAudioFetcher::new(bytes));
        let tool = TranscribeAudioTool::new(backend.clone(), fetcher);
        let r = must_exec(
            &tool,
            json!({ "audio_url": "https://93.184.216.34/huge.ogg" }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("too large"), "got: {}", r.content);
        assert_eq!(backend.snapshot().len(), 0);
    }

    /// Wording changed deliberately when this path joined the shared intake,
    /// and the assertion GAINED clauses rather than losing one.
    ///
    /// Before: `Failed to stat audio file: No such file or directory` — a
    /// message describing an implementation step (`fs::metadata`) that the
    /// unified path no longer performs, because it opens once instead of
    /// stat-ing by name and then reading by name again.
    ///
    /// After: `File not found: <path>` — the SAME wording the PDF and document
    /// surfaces already produce for the same condition, so a host matching on
    /// "not found" now gets one answer from every media surface instead of
    /// three. The assertion now additionally requires that the refusal names
    /// the offending path, which the old message did not carry.
    #[test]
    fn missing_local_path_rejected() {
        let backend = Arc::new(CapturingTranscriptionBackend::new("never called"));
        let tool = TranscribeAudioTool::new(backend.clone(), Arc::new(NullAudioFetcher));
        // The path must be ABSENT but otherwise VALID, or the intake refuses it
        // on absoluteness before it ever reaches the not-found branch this test
        // is about. `/nonexistent/...` is absolute on unix and not on Windows,
        // where `Path::is_absolute` demands a drive or UNC prefix.
        let missing = if cfg!(windows) {
            r"C:\nonexistent\path\audio.mp3"
        } else {
            "/nonexistent/path/audio.mp3"
        };
        let r = must_exec(&tool, json!({ "audio_path": missing }));
        assert!(r.is_error);
        assert!(
            r.content.contains("not found") || r.content.contains("not a regular file"),
            "got: {}",
            r.content
        );
        // `content` is a JSON document here, so a Windows path arrives with its
        // separators escaped (`C:\\nonexistent\\...`). Compare against the
        // JSON-encoded form so this keeps meaning "the refusal names this path"
        // on every platform instead of "the refusal contains these raw bytes" —
        // which no correct Windows message could ever satisfy. On unix the
        // encoding is the identity.
        let named = serde_json::to_string(missing).expect("encode refused path");
        assert!(
            r.content.contains(named.trim_matches('"')),
            "the refusal must name the path it refused; got: {}",
            r.content
        );
        assert_eq!(backend.snapshot().len(), 0);
    }

    #[test]
    fn direct_bytes_source_passes_through() {
        let backend = Arc::new(CapturingTranscriptionBackend::new("from bytes"));
        let fetcher = Arc::new(NullAudioFetcher);
        let tool = TranscribeAudioTool::new(backend.clone(), fetcher);
        let source = AudioSource::Bytes {
            mime: "audio/wav",
            bytes: wav_bytes(),
        };
        let (mime, bytes) = futures::executor::block_on(tool.resolve_source(&source))
            .expect("bytes source resolves");
        assert_eq!(mime, "audio/wav");
        assert_eq!(bytes, wav_bytes());

        let too_small = AudioSource::Bytes {
            mime: "audio/wav",
            bytes: b"x".to_vec(),
        };
        let err = futures::executor::block_on(tool.resolve_source(&too_small)).unwrap_err();
        assert!(err.contains("too small"), "got: {err}");
    }
}
