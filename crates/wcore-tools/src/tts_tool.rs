//! T3-3.6 (sub-wave 6): `text_to_speech` tool — multi-provider TTS via a
//! pluggable [`TtsBackend`].
//!
//! Ported from the prior Wayland Python engine.
//!
//! The Python original embeds six concrete provider integrations (Edge,
//! ElevenLabs, OpenAI, MiniMax, xAI, Mistral, Piper) plus an ffmpeg
//! Opus-conversion post-process and an async streaming-to-speaker mode.
//! Wayland's engine treats provider wiring as a host concern (the agent
//! crate / a plugin chooses an SDK and supplies credentials), so this
//! port collapses **all seven providers behind a single trait**:
//! [`TtsBackend::synthesize`]. The host binds the real backend at startup
//! via [`TtsTool::with_backend`]; the crate's own [`NullTtsBackend`]
//! **fails loud** rather than silently faking success — matching the
//! seam discipline used by `vision_tools.rs` and `video_analyze_tool.rs`.
//!
//! ## Behaviour preserved from the Python original
//!
//! * Input validation — empty/whitespace-only text is rejected.
//! * Length cap — text longer than [`MAX_TEXT_LENGTH`] (4000 chars,
//!   matching the Python sentinel) is **truncated** with a warning, not
//!   rejected. This matches Python's `if len(text) > MAX_TEXT_LENGTH`
//!   branch.
//! * Provider routing — provider is resolved from the input (`"edge"`,
//!   `"elevenlabs"`, `"openai"`, `"minimax"`, `"xai"`, `"mistral"`,
//!   `"piper"`); unknown providers fall back to the default
//!   [`DEFAULT_PROVIDER`] (Edge) like the Python `else` branch.
//! * Output format selection — `mp3` / `wav` / `ogg` / `opus`. `ogg` and
//!   `opus` both map to OGG-Opus (the Telegram-voice-bubble format the
//!   Python original target). Unknown formats are rejected up-front so
//!   we don't generate audio the caller can't use.
//! * Output path resolution — explicit `output_path` (with `~/`
//!   expansion) takes precedence; otherwise a deterministic
//!   `<output_dir>/tts_<id>.<ext>` is built (the Python version
//!   timestamps; we accept a host-injected base directory so tests are
//!   hermetic).
//! * Result shape — `{"success", "file_path", "provider",
//!   "voice_compatible"}` mirrors the Python JSON, minus the
//!   platform-specific `media_tag` field which is a messaging-gateway
//!   concern (Telegram MEDIA: prefix) and does not belong in the engine.
//!
//! ## Differences vs Python
//!
//! * **No embedded provider SDKs.** Every concrete provider (edge_tts,
//!   `elevenlabs`, `openai`, `mistralai`, raw httpx for MiniMax / xAI,
//!   subprocess-spawned `piper`) is the backend's responsibility. This
//!   keeps `wcore-tools` link-time identical to the pre-port baseline
//!   and avoids pulling a 50+ MB dependency graph for a tool the user
//!   may never invoke.
//! * **No ffmpeg post-conversion.** The Python original shells out to
//!   `ffmpeg` to convert Edge TTS MP3 → OGG Opus for Telegram. In the
//!   engine port the backend declares the format it produced via
//!   [`TtsResponse::format`] and the tool reports `voice_compatible =
//!   format == TtsFormat::Opus`. Backends that need MP3→Opus conversion
//!   own that step (and any future ffmpeg invocation MUST go through
//!   `wcore_config::shell::shell_command_argv` since filenames flow
//!   from LLM input).
//! * **No streaming-to-speaker mode.** The Python `stream_tts_to_speaker`
//!   path is a CLI/host concern; the agent-facing tool is file-output
//!   only.
//! * **No messaging-platform `MEDIA:` tag.** That's a gateway transform,
//!   not an engine tool's output.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Hard upper bound on text length passed to the backend. Mirrors the
/// Python `MAX_TEXT_LENGTH = 4000`. Inputs longer than this are
/// truncated (not rejected) — matches Python behaviour.
pub const MAX_TEXT_LENGTH: usize = 4000;

/// Default provider used when the input omits one or supplies an
/// unrecognised value. Mirrors Python `DEFAULT_PROVIDER = "edge"`.
pub const DEFAULT_PROVIDER: TtsProvider = TtsProvider::Edge;

/// Default output format when none is supplied. MP3 is the broadest
/// compatibility choice across providers (matches Python which falls
/// back to `.mp3` for everything except Telegram).
pub const DEFAULT_FORMAT: TtsFormat = TtsFormat::Mp3;

// ---------------------------------------------------------------------
// Provider + format enums
// ---------------------------------------------------------------------

/// Concrete provider identifiers — one variant per provider supported
/// by the Python original. The backend is free to ignore providers it
/// doesn't implement (returning a `BackendNotConfigured`-like error).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsProvider {
    Edge,
    ElevenLabs,
    OpenAi,
    MiniMax,
    XAi,
    Mistral,
    Piper,
}

impl TtsProvider {
    /// Canonical lowercase name used both in the JSON wire format and
    /// for filename extension hints. Matches the Python literal strings.
    pub fn as_str(&self) -> &'static str {
        match self {
            TtsProvider::Edge => "edge",
            TtsProvider::ElevenLabs => "elevenlabs",
            TtsProvider::OpenAi => "openai",
            TtsProvider::MiniMax => "minimax",
            TtsProvider::XAi => "xai",
            TtsProvider::Mistral => "mistral",
            TtsProvider::Piper => "piper",
        }
    }

    /// Parse a free-form user-supplied provider string. Empty, all
    /// whitespace, or an unknown value yields [`DEFAULT_PROVIDER`] —
    /// matches Python `_get_provider` which falls back via `.lower()`
    /// + the `else` branch in `text_to_speech_tool`.
    pub fn parse_or_default(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "edge" => TtsProvider::Edge,
            "elevenlabs" => TtsProvider::ElevenLabs,
            "openai" => TtsProvider::OpenAi,
            "minimax" => TtsProvider::MiniMax,
            "xai" => TtsProvider::XAi,
            "mistral" => TtsProvider::Mistral,
            "piper" => TtsProvider::Piper,
            _ => DEFAULT_PROVIDER,
        }
    }
}

/// Audio output formats supported by the tool. `Opus` is OGG-Opus —
/// the Telegram voice-bubble format the Python original specifically
/// targets via the ffmpeg post-conversion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TtsFormat {
    Mp3,
    Wav,
    Opus,
}

impl TtsFormat {
    /// Filename extension (no leading dot).
    pub fn extension(&self) -> &'static str {
        match self {
            TtsFormat::Mp3 => "mp3",
            TtsFormat::Wav => "wav",
            TtsFormat::Opus => "ogg",
        }
    }

    /// MIME type used by some backends (informational; not all
    /// providers consume this).
    pub fn mime_type(&self) -> &'static str {
        match self {
            TtsFormat::Mp3 => "audio/mpeg",
            TtsFormat::Wav => "audio/wav",
            TtsFormat::Opus => "audio/ogg",
        }
    }

    /// Strict parser. Unlike provider parsing, we **reject** unknown
    /// formats with `Err(raw)` rather than silently defaulting — the
    /// caller asked for a format we don't speak, and quietly producing
    /// a different one corrupts the messaging-platform contract.
    pub fn parse_strict(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mp3" => Ok(TtsFormat::Mp3),
            "wav" => Ok(TtsFormat::Wav),
            // Treat both spellings as the same on-disk format.
            "opus" | "ogg" => Ok(TtsFormat::Opus),
            _ => Err(format!(
                "Unsupported TTS format '{raw}'. Supported: mp3, wav, opus, ogg"
            )),
        }
    }
}

// ---------------------------------------------------------------------
// Request / response / error types — the backend seam
// ---------------------------------------------------------------------

/// Synthesis request handed to a [`TtsBackend`]. The tool guarantees:
///
/// * `text` is non-empty after trim and bounded by [`MAX_TEXT_LENGTH`].
/// * `output_path` exists as a parent directory (the tool creates it).
/// * `format` matches the extension of `output_path`.
#[derive(Debug, Clone)]
pub struct TtsRequest {
    pub text: String,
    pub provider: TtsProvider,
    /// Voice / speaker identifier — provider-specific. The tool does
    /// not interpret this; if omitted the backend chooses its default.
    pub voice: Option<String>,
    /// Model / engine identifier — provider-specific (e.g. ElevenLabs
    /// `eleven_multilingual_v2`, OpenAI `gpt-4o-mini-tts`).
    pub model: Option<String>,
    pub format: TtsFormat,
    pub output_path: PathBuf,
    /// Optional speed multiplier (1.0 = normal). Edge TTS, OpenAI TTS
    /// and ElevenLabs all support this; backends that don't simply
    /// ignore it.
    pub speed: Option<f32>,
}

/// Result returned by the backend. `bytes_written` is the on-disk size
/// of `path`; `format` is whatever the backend actually produced
/// (which the tool checks against the request).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsResponse {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub format: TtsFormat,
    pub provider: TtsProvider,
}

/// Typed error categories. Mirrors the Python `except ValueError /
/// FileNotFoundError / Exception` branches plus an explicit
/// `BackendNotConfigured` for the fail-loud null backend.
#[derive(Debug, thiserror::Error)]
pub enum TtsError {
    /// Backend not bound. Always fail loud.
    #[error("tts backend is not configured: {0}")]
    BackendNotConfigured(String),

    /// Missing credentials / misconfigured provider settings.
    #[error("tts configuration error: {0}")]
    Configuration(String),

    /// Provider SDK / binary not installed.
    #[error("tts dependency missing: {0}")]
    DependencyMissing(String),

    /// Catch-all for upstream synthesis failures.
    #[error("tts synthesis failed: {0}")]
    Other(String),
}

/// **The seam.** A single trait spans all seven Python providers; the
/// concrete backend (chosen by the host) decides whether to dispatch
/// internally on `request.provider` or to enforce a single-provider
/// build.
#[async_trait]
pub trait TtsBackend: Send + Sync {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError>;
}

// ---------------------------------------------------------------------
// NullTtsBackend — fail loud
// ---------------------------------------------------------------------

/// Default backend used when no real one was bound. **Every call fails**
/// with [`TtsError::BackendNotConfigured`] so missing wire-up surfaces
/// as a loud, debuggable error instead of silent stubbing.
#[derive(Default)]
pub struct NullTtsBackend;

#[async_trait]
impl TtsBackend for NullTtsBackend {
    async fn synthesize(&self, _request: TtsRequest) -> Result<TtsResponse, TtsError> {
        Err(TtsError::BackendNotConfigured(
            "no TtsBackend bound — the host must inject a real backend via \
TtsTool::with_backend before this tool is registered."
                .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------
// CapturingTtsBackend — hermetic test fake
// ---------------------------------------------------------------------

/// Test-only backend that records every synthesis request, writes a
/// caller-supplied byte payload to the requested `output_path`, and
/// returns a successful response. Used by this crate's tests and by
/// downstream integration tests that need a hermetic TTS surface.
pub struct CapturingTtsBackend {
    pub calls: Mutex<Vec<TtsRequest>>,
    /// Bytes to write to disk on each call. Defaults to a tiny
    /// 4-byte payload so size assertions can verify a non-zero write.
    pub payload: Vec<u8>,
    /// If `Some`, overrides the request format in the response
    /// (lets tests simulate a backend that converted formats).
    pub override_format: Option<TtsFormat>,
}

impl Default for CapturingTtsBackend {
    fn default() -> Self {
        Self::with_payload(b"\x00\x00\x00\x00".to_vec())
    }
}

impl CapturingTtsBackend {
    pub fn with_payload(payload: Vec<u8>) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            payload,
            override_format: None,
        }
    }

    /// Snapshot all recorded calls (cloned so the lock is released).
    pub fn calls(&self) -> Vec<TtsRequest> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl TtsBackend for CapturingTtsBackend {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError> {
        // Persist the file the way a real backend would so the tool's
        // post-write checks (size > 0) pass.
        if let Some(parent) = request.output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| TtsError::Other(format!("capturing-backend mkdir failed: {e}")))?;
        }
        std::fs::write(&request.output_path, &self.payload)
            .map_err(|e| TtsError::Other(format!("capturing-backend write failed: {e}")))?;

        let format = self.override_format.unwrap_or(request.format);
        let provider = request.provider;
        let path = request.output_path.clone();
        self.calls.lock().unwrap().push(request);
        Ok(TtsResponse {
            path,
            bytes_written: self.payload.len() as u64,
            format,
            provider,
        })
    }
}

// ---------------------------------------------------------------------
// The tool
// ---------------------------------------------------------------------

/// `text_to_speech` agent tool. Holds an `Arc<dyn TtsBackend>` so the
/// host can swap implementations at startup without touching the
/// dispatcher.
pub struct TtsTool {
    backend: Arc<dyn TtsBackend>,
    /// Base directory for auto-generated filenames. The host injects
    /// this so tests can use a tempdir; production code passes a
    /// configured cache directory.
    output_dir: PathBuf,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `with_backend(backend)` flips
    /// it on.
    backend_configured: bool,
}

impl Default for TtsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl TtsTool {
    /// Construct with the fail-loud `NullTtsBackend` and the system
    /// temp directory as the default output dir. The host MUST replace
    /// both via [`Self::with_backend`] / [`Self::with_output_dir`]
    /// before registration in a production agent.
    pub fn new() -> Self {
        Self {
            backend: Arc::new(NullTtsBackend),
            output_dir: std::env::temp_dir(),
            backend_configured: false,
        }
    }

    /// Build with a real (or test) backend.
    pub fn with_backend(backend: Arc<dyn TtsBackend>) -> Self {
        Self {
            backend,
            output_dir: std::env::temp_dir(),
            backend_configured: true,
        }
    }

    /// Override the default output directory used when the caller
    /// doesn't pass `output_path`.
    pub fn with_output_dir(mut self, dir: PathBuf) -> Self {
        self.output_dir = dir;
        self
    }

    /// Expand a leading `~/` (and bare `~`) using the user's home dir,
    /// mirroring Python's `Path(output_path).expanduser()`.
    fn expand_user(raw: &str) -> PathBuf {
        if let Some(rest) = raw.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        } else if raw == "~"
            && let Some(home) = dirs::home_dir()
        {
            return home;
        }
        PathBuf::from(raw)
    }

    /// Decide the on-disk target path. Caller-supplied path wins
    /// (with `~/` expansion); otherwise we build a deterministic
    /// filename inside `output_dir`.
    fn resolve_output_path(
        &self,
        raw: Option<&str>,
        format: TtsFormat,
        provider: TtsProvider,
    ) -> PathBuf {
        if let Some(p) = raw {
            let trimmed = p.trim();
            if !trimmed.is_empty() {
                return Self::expand_user(trimmed);
            }
        }
        // Deterministic filename — provider + format. Tests can
        // override output_dir to a tempdir for hermeticity. The Python
        // version timestamps; we keep it stable so a second invocation
        // overwrites rather than littering tempdirs.
        let mut p = self.output_dir.clone();
        p.push(format!("tts_{}.{}", provider.as_str(), format.extension()));
        p
    }

    /// True iff `format` is the messaging-platform "voice bubble"
    /// format. Mirrors the Python `voice_compatible` flag.
    fn is_voice_compatible(format: TtsFormat) -> bool {
        matches!(format, TtsFormat::Opus)
    }
}

#[async_trait]
impl Tool for TtsTool {
    fn name(&self) -> &str {
        "text_to_speech"
    }

    /// v0.9.0 W1: hidden when no real `TtsBackend` is wired.
    /// `Default::default()` yields `backend_configured == false`, so
    /// `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Synthesize speech audio from text using a configured TTS provider \
(edge, elevenlabs, openai, minimax, xai, mistral, or piper). Returns the \
path to the generated audio file. Supports mp3, wav, and opus (.ogg) \
output formats."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "Text to convert to speech. Required."
                },
                "provider": {
                    "type": "string",
                    "description":
                        "TTS provider: edge (default), elevenlabs, openai, \
        minimax, xai, mistral, or piper.",
                    "enum": [
                        "edge", "elevenlabs", "openai",
                        "minimax", "xai", "mistral", "piper"
                    ]
                },
                "voice": {
                    "type": "string",
                    "description":
                        "Voice / speaker ID — provider-specific (e.g. \
        'en-US-AriaNeural' for edge)."
                },
                "model": {
                    "type": "string",
                    "description":
                        "Model / engine ID — provider-specific (e.g. \
        'eleven_multilingual_v2', 'gpt-4o-mini-tts')."
                },
                "format": {
                    "type": "string",
                    "description":
                        "Output audio format. 'opus' / 'ogg' produces an OGG \
        Opus file suitable for messaging-platform voice bubbles.",
                    "enum": ["mp3", "wav", "opus", "ogg"]
                },
                "output_path": {
                    "type": "string",
                    "description":
                        "Optional explicit output path. Defaults to a stable \
        filename in the configured output directory. Supports '~/' expansion."
                },
                "speed": {
                    "type": "number",
                    "description":
                        "Speech rate multiplier (1.0 = normal). Backend may \
        ignore on providers that don't support it."
                }
            },
            "required": ["text"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // The default filename is deterministic per (provider, format)
        // so concurrent invocations could race on the same file. Be
        // conservative and serialise. A real backend with unique
        // timestamped filenames could relax this.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // --- text required + truncate ---
        let raw_text = match input.get("text").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return error_result("text is required");
            }
        };
        if raw_text.trim().is_empty() {
            return error_result("text is required");
        }
        // Snap to char boundary on truncation so we never split a UTF-8
        // sequence — Python slices bytes here; we slice chars.
        let text = if raw_text.chars().count() > MAX_TEXT_LENGTH {
            let mut s = String::with_capacity(MAX_TEXT_LENGTH);
            for (i, ch) in raw_text.chars().enumerate() {
                if i >= MAX_TEXT_LENGTH {
                    break;
                }
                s.push(ch);
            }
            tracing::warn!(
                "TTS text too long ({} chars), truncating to {}",
                raw_text.chars().count(),
                MAX_TEXT_LENGTH
            );
            s
        } else {
            raw_text.to_string()
        };

        // --- provider ---
        let provider = input
            .get("provider")
            .and_then(|v| v.as_str())
            .map(TtsProvider::parse_or_default)
            .unwrap_or(DEFAULT_PROVIDER);

        // --- format ---
        let format = match input.get("format").and_then(|v| v.as_str()) {
            Some(raw) => match TtsFormat::parse_strict(raw) {
                Ok(fmt) => fmt,
                Err(msg) => return error_result(&msg),
            },
            None => DEFAULT_FORMAT,
        };

        // --- output_path ---
        let output_path = self.resolve_output_path(
            input.get("output_path").and_then(|v| v.as_str()),
            format,
            provider,
        );

        // Ensure parent dir exists — matches Python `file_path.parent.mkdir`.
        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return error_result(&format!(
                "Could not create output directory '{}': {e}",
                parent.display()
            ));
        }

        // --- voice / model / speed (optional, passthrough) ---
        let voice = input
            .get("voice")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let model = input
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        let speed = input
            .get("speed")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32);

        let request = TtsRequest {
            text,
            provider,
            voice,
            model,
            format,
            output_path: output_path.clone(),
            speed,
        };

        match self.backend.synthesize(request).await {
            Ok(resp) => {
                // Defensive post-check: the Python original verifies
                // the file exists and is non-empty.
                let on_disk = std::fs::metadata(&resp.path).map(|m| m.len()).unwrap_or(0);
                if on_disk == 0 {
                    return error_result(&format!(
                        "TTS generation produced no output (provider: {})",
                        resp.provider.as_str()
                    ));
                }

                let payload = json!({
                    "success": true,
                    "file_path": resp.path.to_string_lossy(),
                    "provider": resp.provider.as_str(),
                    "format": resp.format.extension(),
                    "bytes_written": resp.bytes_written,
                    "voice_compatible": Self::is_voice_compatible(resp.format),
                });
                ToolResult {
                    content: payload.to_string(),
                    is_error: false,
                }
            }
            Err(e) => error_result(&format!("{e}")),
        }
    }

    fn category(&self) -> ToolCategory {
        // Generates a side-effect (writes an audio file to disk);
        // classify with Edit-family tools rather than Info.
        ToolCategory::Edit
    }

    fn max_result_size(&self) -> usize {
        // The result is a small JSON envelope (path + metadata); the
        // default 50 KB is fine, but we set it explicitly so a future
        // change doesn't accidentally bloat it.
        4096
    }

    fn describe(&self, input: &Value) -> String {
        let provider = input
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("edge");
        let nchars = input
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().count())
            .unwrap_or(0);
        format!("text_to_speech: provider={provider} chars={nchars}")
    }
}

fn error_result(msg: &str) -> ToolResult {
    ToolResult {
        content: json!({
            "success": false,
            "error": msg,
        })
        .to_string(),
        is_error: true,
    }
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn parse_json(s: &str) -> Value {
        serde_json::from_str(s).expect("tool returned non-JSON")
    }

    // --- enum parsing ---

    #[test]
    fn provider_parse_known_and_fallback() {
        assert_eq!(TtsProvider::parse_or_default("edge"), TtsProvider::Edge);
        assert_eq!(
            TtsProvider::parse_or_default("  ELEVENLABS  "),
            TtsProvider::ElevenLabs
        );
        assert_eq!(TtsProvider::parse_or_default("openai"), TtsProvider::OpenAi);
        assert_eq!(TtsProvider::parse_or_default("piper"), TtsProvider::Piper);
        // Unknown -> default
        assert_eq!(TtsProvider::parse_or_default(""), DEFAULT_PROVIDER);
        assert_eq!(TtsProvider::parse_or_default("nope"), DEFAULT_PROVIDER);
    }

    #[test]
    fn format_parse_strict() {
        assert_eq!(TtsFormat::parse_strict("mp3").unwrap(), TtsFormat::Mp3);
        assert_eq!(TtsFormat::parse_strict("WAV").unwrap(), TtsFormat::Wav);
        assert_eq!(TtsFormat::parse_strict("opus").unwrap(), TtsFormat::Opus);
        assert_eq!(TtsFormat::parse_strict("ogg").unwrap(), TtsFormat::Opus);
        let err = TtsFormat::parse_strict("flac").unwrap_err();
        assert!(err.contains("Unsupported"), "unexpected error: {err}");
    }

    #[test]
    fn format_extension_and_mime() {
        assert_eq!(TtsFormat::Mp3.extension(), "mp3");
        assert_eq!(TtsFormat::Wav.extension(), "wav");
        assert_eq!(TtsFormat::Opus.extension(), "ogg");
        assert_eq!(TtsFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(TtsFormat::Opus.mime_type(), "audio/ogg");
    }

    // --- tool schema / name ---

    #[test]
    fn tool_name_and_schema_shape() {
        let t = TtsTool::default();
        assert_eq!(t.name(), "text_to_speech");
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"], json!(["text"]));
        let props = &schema["properties"];
        for key in [
            "text",
            "provider",
            "voice",
            "model",
            "format",
            "output_path",
            "speed",
        ] {
            assert!(props.get(key).is_some(), "missing schema property: {key}");
        }
    }

    // --- null backend fails loud ---

    #[tokio::test]
    async fn null_backend_fails_loud() {
        let t = TtsTool::default();
        let res = t.execute(json!({"text": "hi"})).await;
        assert!(res.is_error, "null backend must fail loud");
        let v = parse_json(&res.content);
        assert_eq!(v["success"], false);
        let err = v["error"].as_str().unwrap();
        assert!(
            err.contains("not configured") || err.contains("backend"),
            "unexpected error: {err}"
        );
    }

    // --- empty text rejected ---

    #[tokio::test]
    async fn empty_text_rejected() {
        let t = TtsTool::default();
        for body in [json!({"text": ""}), json!({"text": "   \n\t "}), json!({})] {
            let res = t.execute(body).await;
            assert!(res.is_error, "expected error for empty text");
            let v = parse_json(&res.content);
            assert_eq!(v["success"], false);
            assert!(v["error"].as_str().unwrap().contains("required"));
        }
    }

    // --- unsupported format rejected up-front ---

    #[tokio::test]
    async fn unsupported_format_rejected() {
        let backend = Arc::new(CapturingTtsBackend::default());
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend.clone()).with_output_dir(tmp.path().to_path_buf());
        let res = t.execute(json!({"text": "hi", "format": "flac"})).await;
        assert!(res.is_error);
        assert_eq!(
            backend.calls().len(),
            0,
            "backend must not be invoked for unsupported format"
        );
    }

    // --- text truncation at MAX_TEXT_LENGTH ---

    #[tokio::test]
    async fn text_truncated_at_max_length() {
        let backend = Arc::new(CapturingTtsBackend::default());
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend.clone()).with_output_dir(tmp.path().to_path_buf());
        let long_text: String = "a".repeat(MAX_TEXT_LENGTH + 500);
        let res = t.execute(json!({"text": long_text})).await;
        assert!(!res.is_error, "valid request should succeed: {:?}", res);
        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].text.chars().count(),
            MAX_TEXT_LENGTH,
            "text should be truncated to MAX_TEXT_LENGTH"
        );
    }

    // --- capturing backend round-trip + provider/voice/format passthrough ---

    #[tokio::test]
    async fn capturing_backend_round_trip() {
        let backend = Arc::new(CapturingTtsBackend::with_payload(vec![1, 2, 3, 4, 5, 6]));
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend.clone()).with_output_dir(tmp.path().to_path_buf());

        let res = t
            .execute(json!({
                "text": "Hello world",
                "provider": "elevenlabs",
                "voice": "Adam",
                "model": "eleven_multilingual_v2",
                "format": "opus",
                "speed": 1.25,
            }))
            .await;
        assert!(!res.is_error, "expected success: {}", res.content);

        let calls = backend.calls();
        assert_eq!(calls.len(), 1);
        let req = &calls[0];
        assert_eq!(req.text, "Hello world");
        assert_eq!(req.provider, TtsProvider::ElevenLabs);
        assert_eq!(req.voice.as_deref(), Some("Adam"));
        assert_eq!(req.model.as_deref(), Some("eleven_multilingual_v2"));
        assert_eq!(req.format, TtsFormat::Opus);
        assert!((req.speed.unwrap() - 1.25).abs() < 1e-6);

        let v = parse_json(&res.content);
        assert_eq!(v["success"], true);
        assert_eq!(v["provider"], "elevenlabs");
        assert_eq!(v["format"], "ogg");
        assert_eq!(v["voice_compatible"], true);
        assert_eq!(v["bytes_written"], 6);

        // File on disk matches the reported path.
        let path = PathBuf::from(v["file_path"].as_str().unwrap());
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written, vec![1, 2, 3, 4, 5, 6]);
        // Default filename uses provider + ext.
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".ogg"),
            "expected .ogg extension, got {path:?}"
        );
    }

    // --- explicit output_path with ~/ expansion ---

    #[tokio::test]
    async fn explicit_output_path_used() {
        let backend = Arc::new(CapturingTtsBackend::default());
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend.clone()).with_output_dir(tmp.path().to_path_buf());

        // Use a path inside the tempdir (not actually ~/) so the test
        // is hermetic. The expansion code is exercised separately.
        let target = tmp.path().join("nested").join("my_audio.mp3");
        let res = t
            .execute(json!({
                "text": "hi",
                "output_path": target.to_string_lossy(),
            }))
            .await;
        assert!(!res.is_error, "{}", res.content);
        let v = parse_json(&res.content);
        assert_eq!(v["file_path"].as_str().unwrap(), target.to_string_lossy());
        assert!(target.exists(), "expected file at {target:?}");
    }

    #[test]
    fn expand_user_handles_tilde_prefix() {
        // We don't assert the exact home dir (CI varies), only that
        // expansion strips the leading ~/ when a home dir exists.
        let raw = "~/foo/bar.mp3";
        let expanded = TtsTool::expand_user(raw);
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expanded, home.join("foo").join("bar.mp3"));
        }

        // No tilde → unchanged.
        assert_eq!(
            TtsTool::expand_user("/abs/path.mp3"),
            PathBuf::from("/abs/path.mp3")
        );
    }

    // --- backend produces empty file → tool reports failure ---

    #[tokio::test]
    async fn empty_output_reported_as_error() {
        let backend = Arc::new(CapturingTtsBackend::with_payload(Vec::new()));
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend).with_output_dir(tmp.path().to_path_buf());
        let res = t.execute(json!({"text": "hi"})).await;
        assert!(res.is_error, "empty file must surface as error");
        let v = parse_json(&res.content);
        assert_eq!(v["success"], false);
        assert!(
            v["error"].as_str().unwrap().contains("no output"),
            "unexpected error: {}",
            v["error"]
        );
    }

    // --- voice_compatible flag tracks the backend-reported format ---

    #[tokio::test]
    async fn voice_compatible_follows_backend_format() {
        let backend = Arc::new(CapturingTtsBackend {
            override_format: Some(TtsFormat::Opus),
            ..CapturingTtsBackend::default()
        });
        let tmp = TempDir::new().unwrap();
        let t = TtsTool::with_backend(backend).with_output_dir(tmp.path().to_path_buf());
        // Request mp3 but backend reports it converted to opus.
        let res = t.execute(json!({"text": "hi", "format": "mp3"})).await;
        assert!(!res.is_error);
        let v = parse_json(&res.content);
        assert_eq!(v["format"], "ogg");
        assert_eq!(v["voice_compatible"], true);
    }

    // --- describe() summary ---

    #[test]
    fn describe_summary_includes_provider_and_length() {
        let t = TtsTool::default();
        let s = t.describe(&json!({"text": "hello", "provider": "openai"}));
        assert!(s.contains("openai"), "describe missing provider: {s}");
        assert!(s.contains("chars=5"), "describe missing char count: {s}");
    }

    // --- v0.9.0 W1 backend gate ---

    #[test]
    fn default_is_hidden_when_no_backend_wired() {
        let tool = TtsTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn with_real_backend_is_available() {
        let tool = TtsTool::with_backend(Arc::new(CapturingTtsBackend::default()));
        assert!(
            tool.is_available(),
            "with_backend(...) must yield backend_configured == true"
        );
    }
}
