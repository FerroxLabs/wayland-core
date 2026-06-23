//! T3-3.7 (sub-wave 7): `meet_*` tools — Google Meet integration via a
//! pluggable [`GoogleMeetBackend`].
//!
//! Ported from the prior Wayland Python engine (754 LOC).
//!
//! The Python original spawns a detached Playwright + headless-Chromium
//! subprocess that joins a Google Meet URL, scrapes live captions into a
//! transcript file on disk, and optionally streams TTS audio into the
//! call via a virtual audio device (PulseAudio null-sink on Linux,
//! BlackHole on macOS). Wayland's engine deliberately does **not** embed
//! a browser-automation stack, a per-meeting state directory under
//! `$WAYLAND_HOME/workspace/meetings/`, or a virtual-mic bridge — these
//! belong to the host (`wcore-browser` / `wayland-browser` / a wcore-cua
//! audio bridge wired by the agent crate or a plugin).
//!
//! This port collapses **all five operations behind a single trait**:
//! [`GoogleMeetBackend`]. The host binds the real backend at startup;
//! the crate's own [`NullGoogleMeetBackend`] **fails loud** with
//! structured errors rather than silently faking success — matching the
//! seam discipline established in `vision_tools.rs` and `tts_tool.rs`.
//!
//! ## Tools registered
//!
//! | Tool             | Backend op            | Side effects                          |
//! |------------------|-----------------------|---------------------------------------|
//! | `meet_join`      | [`GoogleMeetBackend::join`]       | spawns / dispatches bot     |
//! | `meet_status`    | [`GoogleMeetBackend::status`]     | read-only                   |
//! | `meet_transcript`| [`GoogleMeetBackend::transcript`] | read-only                   |
//! | `meet_leave`     | [`GoogleMeetBackend::leave`]      | terminates bot              |
//! | `meet_say`       | [`GoogleMeetBackend::say`]        | enqueues TTS                |
//!
//! ## Behaviour preserved from the Python original
//!
//! * **URL allowlist.** Only `https://meet.google.com/<3-4-3-letter-code>`
//!   URLs are accepted. The shape comes verbatim from
//!   `_MEET_URL_RE`. No subpath traversal, no other Google domains, no
//!   `http://` scheme.
//! * **Mode validation.** `meet_join` accepts `mode in {transcribe,
//!   realtime}`; anything else is rejected up-front (mirrors the
//!   Python `_handle_meet_join` check).
//! * **Empty-text rejection** for `meet_say` — matches Python.
//! * **JSON result envelope** — `{"success": bool, ...}` matches the
//!   Python `_json` / `_err` helpers.
//! * **Explicit by design.** No calendar scanning, no auto-dial. The
//!   agent is responsible for announcing itself in the meeting (no
//!   automatic consent announcement).
//!
//! ## Differences vs Python
//!
//! * **No embedded Playwright / Chromium spawn path.** The Python
//!   `_start_bot` shells out to `python -m tools.google_meet_bot` with
//!   `start_new_session=True`, writes a `.active.json` pointer file
//!   under `$WAYLAND_HOME/workspace/meetings/`, and polls `kill(pid, 0)`
//!   for liveness. None of that lives in the engine — the host backend
//!   owns process lifecycle, state directory layout, and PID
//!   bookkeeping. The engine port is a pure dispatch + validation surface.
//! * **No remote-node registry.** The Python `_resolve_node_client`
//!   path (looking up an approved remote node by name) belongs to a
//!   host's MCP / RPC layer. If the backend wants to dispatch to a
//!   remote node it does so internally — the engine just forwards the
//!   `node` arg as an opaque string in the request.
//! * **No on-disk transcript file.** The backend returns transcript
//!   lines in-memory via [`MeetTranscriptResponse`]; if the backend
//!   wants to persist a file it does so on its own.

use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

// ---------------------------------------------------------------------
// URL safety + meeting-id extraction
// ---------------------------------------------------------------------

/// Validate a Google Meet URL the same way the Python original's
/// `_MEET_URL_RE` does: `https://meet.google.com/<3-4-letter>-<3-4-letter>-<3-4-letter>`
/// with optional query string. Returns the extracted meeting id (e.g.
/// `abc-defg-hij`) on match.
///
/// We compile a fresh `Regex` per call to keep this helper allocation-
/// free at module-init time; pattern compilation is microseconds and
/// dwarfed by the network call that follows. If profiling ever shows
/// this as hot, swap to `once_cell::sync::Lazy`.
pub fn extract_meeting_id(url: &str) -> Option<String> {
    // Pattern matches Python's `^https://meet\.google\.com/([a-z]{3,4}-[a-z]{3,4}-[a-z]{3,4})(?:\?[^#\s]*)?$`.
    let re =
        Regex::new(r"^https://meet\.google\.com/([a-z]{3,4}-[a-z]{3,4}-[a-z]{3,4})(?:\?[^#\s]*)?$")
            .expect("static regex compiles");
    re.captures(url.trim())
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// Cheap shape check — equivalent to Python's `_is_safe_meet_url`.
pub fn is_safe_meet_url(url: &str) -> bool {
    extract_meeting_id(url).is_some()
}

// ---------------------------------------------------------------------
// Mode + default constants
// ---------------------------------------------------------------------

/// Bot operating mode passed to `meet_join`. `Transcribe` is listen-only;
/// `Realtime` additionally enables agent speech via `meet_say` (requires
/// a backend that wires a virtual-mic / TTS bridge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MeetMode {
    Transcribe,
    Realtime,
}

impl MeetMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MeetMode::Transcribe => "transcribe",
            MeetMode::Realtime => "realtime",
        }
    }

    /// Strict parse — unknown values yield `Err`. Matches Python's
    /// `mode not in ("transcribe", "realtime")` rejection in
    /// `_handle_meet_join`.
    pub fn parse_strict(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "transcribe" => Ok(MeetMode::Transcribe),
            "realtime" => Ok(MeetMode::Realtime),
            other => Err(format!(
                "mode must be 'transcribe' or 'realtime' (got {other:?})"
            )),
        }
    }
}

/// Default display name when the caller omits `guest_name`. Matches
/// Python `'Wayland Agent'`.
pub const DEFAULT_GUEST_NAME: &str = "Wayland Agent";

// ---------------------------------------------------------------------
// Request / response types (the backend seam payloads)
// ---------------------------------------------------------------------

/// Parameters for [`GoogleMeetBackend::join`]. The tool guarantees the
/// `url` field has been validated against [`is_safe_meet_url`] and
/// `mode` has been parsed strictly before the backend sees the request.
#[derive(Debug, Clone)]
pub struct MeetJoinRequest {
    /// Full validated `https://meet.google.com/<code>` URL.
    pub url: String,
    /// Extracted meeting id (e.g. `abc-defg-hij`), provided so backends
    /// don't have to re-parse the URL.
    pub meeting_id: String,
    pub mode: MeetMode,
    /// Display name when joining as guest.
    pub guest_name: String,
    /// Optional max-duration string (e.g. `"30m"`, `"2h"`, `"90s"`).
    /// The engine does not interpret this — backends parse and enforce
    /// it however they see fit.
    pub duration: Option<String>,
    /// If true, run the bot's browser headed (debug only).
    pub headed: bool,
    /// Opaque remote-node identifier. The engine passes it through;
    /// the backend decides whether to dispatch locally or remotely.
    pub node: Option<String>,
}

/// Single caption line scraped from a Meet call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetTranscriptLine {
    /// Speaker label (e.g. `"Sean"`) — may be empty if the bot couldn't
    /// attribute the line.
    #[serde(default)]
    pub speaker: String,
    /// The caption text.
    pub text: String,
    /// Timestamp the bot saw this caption, in seconds since the meeting
    /// joined. Optional because some backends only surface deltas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_seconds: Option<f64>,
}

/// Response from [`GoogleMeetBackend::status`]. Mirrors the Python
/// `_bot_status` dict, slimmed to the fields the model actually uses.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetStatusResponse {
    /// Whether the bot process is alive.
    pub alive: bool,
    /// Whether the bot has been admitted to the call (vs sitting in the
    /// lobby).
    pub in_meeting: bool,
    /// Number of transcript lines captured so far.
    pub transcript_lines: u64,
    /// Optional last-caption timestamp (epoch seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_caption_at: Option<f64>,
    /// Optional freeform diagnostic message (e.g. `"in lobby, awaiting host"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response from [`GoogleMeetBackend::transcript`]. Empty `lines` is
/// valid (a meeting that hasn't generated captions yet).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetTranscriptResponse {
    pub lines: Vec<MeetTranscriptLine>,
    /// Total caption lines in the underlying store — informs the
    /// caller when `last` was applied (the response may be shorter).
    #[serde(default)]
    pub total_lines: u64,
}

/// Response from [`GoogleMeetBackend::join`]. The optional fields
/// mirror the Python `_start_bot` return shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetJoinResponse {
    pub meeting_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_pid: Option<u32>,
    /// Optional path to the transcript file (some backends persist; the
    /// engine never does).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// Response from [`GoogleMeetBackend::leave`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetLeaveResponse {
    pub meeting_id: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

/// Response from [`GoogleMeetBackend::say`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetSayResponse {
    /// Number of items queued (typically 1, but a backend may chunk).
    #[serde(default)]
    pub queued: u32,
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// Typed error categories surfaced by the backend.
#[derive(Debug, thiserror::Error)]
pub enum MeetError {
    /// Backend not bound — fail loud, never silent.
    #[error("google_meet backend is not configured: {0}")]
    BackendNotConfigured(String),

    /// No active meeting (calling `status` / `transcript` / `leave` /
    /// `say` before `join`).
    #[error("no active google_meet session: {0}")]
    NoActiveSession(String),

    /// `meet_say` invoked but the active meeting was joined in
    /// `transcribe` mode.
    #[error("realtime mode required for meet_say: {0}")]
    NotRealtime(String),

    /// Prerequisites missing on the host (Playwright, Chromium, etc.).
    #[error("google_meet dependency missing: {0}")]
    DependencyMissing(String),

    /// Catch-all for upstream failures.
    #[error("google_meet error: {0}")]
    Other(String),
}

// ---------------------------------------------------------------------
// The seam
// ---------------------------------------------------------------------

/// **The seam.** One trait spans all five `meet_*` tools; the host
/// binds a single implementation that internally dispatches on the
/// operation. Backends are free to spawn a Playwright bot, dispatch to
/// a remote node, or maintain in-memory state.
#[async_trait]
pub trait GoogleMeetBackend: Send + Sync {
    async fn join(&self, request: MeetJoinRequest) -> Result<MeetJoinResponse, MeetError>;
    async fn status(&self, node: Option<&str>) -> Result<MeetStatusResponse, MeetError>;
    async fn transcript(
        &self,
        node: Option<&str>,
        last: Option<u32>,
    ) -> Result<MeetTranscriptResponse, MeetError>;
    async fn leave(&self, node: Option<&str>) -> Result<MeetLeaveResponse, MeetError>;
    async fn say(&self, node: Option<&str>, text: &str) -> Result<MeetSayResponse, MeetError>;
}

// ---------------------------------------------------------------------
// NullGoogleMeetBackend — fail loud
// ---------------------------------------------------------------------

/// Default backend used when no real one was bound. **Every call
/// fails** with [`MeetError::BackendNotConfigured`] so missing wire-up
/// surfaces as a loud error instead of silent stubbing.
#[derive(Default)]
pub struct NullGoogleMeetBackend;

#[async_trait]
impl GoogleMeetBackend for NullGoogleMeetBackend {
    async fn join(&self, _request: MeetJoinRequest) -> Result<MeetJoinResponse, MeetError> {
        Err(null_err("join"))
    }
    async fn status(&self, _node: Option<&str>) -> Result<MeetStatusResponse, MeetError> {
        Err(null_err("status"))
    }
    async fn transcript(
        &self,
        _node: Option<&str>,
        _last: Option<u32>,
    ) -> Result<MeetTranscriptResponse, MeetError> {
        Err(null_err("transcript"))
    }
    async fn leave(&self, _node: Option<&str>) -> Result<MeetLeaveResponse, MeetError> {
        Err(null_err("leave"))
    }
    async fn say(&self, _node: Option<&str>, _text: &str) -> Result<MeetSayResponse, MeetError> {
        Err(null_err("say"))
    }
}

fn null_err(op: &str) -> MeetError {
    MeetError::BackendNotConfigured(format!(
        "no GoogleMeetBackend bound — the host must inject a real backend before \
         meet_{op} is registered."
    ))
}

// ---------------------------------------------------------------------
// CapturingGoogleMeetBackend — hermetic test fake
// ---------------------------------------------------------------------

/// Each captured invocation, tagged by operation. Useful for asserting
/// that the tool layer forwarded args correctly.
#[derive(Debug, Clone)]
pub enum CapturedMeetCall {
    Join(MeetJoinRequest),
    Status {
        node: Option<String>,
    },
    Transcript {
        node: Option<String>,
        last: Option<u32>,
    },
    Leave {
        node: Option<String>,
    },
    Say {
        node: Option<String>,
        text: String,
    },
}

/// Test-only backend that records every call and returns a canned
/// success response. Mirrors `CapturingTtsBackend` in `tts_tool.rs`.
pub struct CapturingGoogleMeetBackend {
    pub calls: parking_lot::Mutex<Vec<CapturedMeetCall>>,
    pub status_response: MeetStatusResponse,
    pub transcript_response: MeetTranscriptResponse,
}

impl Default for CapturingGoogleMeetBackend {
    fn default() -> Self {
        Self {
            calls: parking_lot::Mutex::new(Vec::new()),
            status_response: MeetStatusResponse {
                alive: true,
                in_meeting: true,
                transcript_lines: 0,
                last_caption_at: None,
                message: None,
            },
            transcript_response: MeetTranscriptResponse::default(),
        }
    }
}

impl CapturingGoogleMeetBackend {
    pub fn snapshot(&self) -> Vec<CapturedMeetCall> {
        self.calls.lock().clone()
    }
}

#[async_trait]
impl GoogleMeetBackend for CapturingGoogleMeetBackend {
    async fn join(&self, request: MeetJoinRequest) -> Result<MeetJoinResponse, MeetError> {
        let meeting_id = request.meeting_id.clone();
        self.calls.lock().push(CapturedMeetCall::Join(request));
        Ok(MeetJoinResponse {
            meeting_id,
            bot_pid: Some(4242),
            transcript_path: None,
        })
    }
    async fn status(&self, node: Option<&str>) -> Result<MeetStatusResponse, MeetError> {
        self.calls.lock().push(CapturedMeetCall::Status {
            node: node.map(str::to_string),
        });
        Ok(self.status_response.clone())
    }
    async fn transcript(
        &self,
        node: Option<&str>,
        last: Option<u32>,
    ) -> Result<MeetTranscriptResponse, MeetError> {
        self.calls.lock().push(CapturedMeetCall::Transcript {
            node: node.map(str::to_string),
            last,
        });
        Ok(self.transcript_response.clone())
    }
    async fn leave(&self, node: Option<&str>) -> Result<MeetLeaveResponse, MeetError> {
        self.calls.lock().push(CapturedMeetCall::Leave {
            node: node.map(str::to_string),
        });
        Ok(MeetLeaveResponse {
            meeting_id: "abc-defg-hij".to_string(),
            reason: "agent called meet_leave".to_string(),
            transcript_path: None,
        })
    }
    async fn say(&self, node: Option<&str>, text: &str) -> Result<MeetSayResponse, MeetError> {
        self.calls.lock().push(CapturedMeetCall::Say {
            node: node.map(str::to_string),
            text: text.to_string(),
        });
        Ok(MeetSayResponse { queued: 1 })
    }
}

// ---------------------------------------------------------------------
// Shared envelope helpers
// ---------------------------------------------------------------------

fn ok_envelope(extra: Value) -> ToolResult {
    let merged = match extra {
        Value::Object(mut m) => {
            m.insert("success".to_string(), json!(true));
            Value::Object(m)
        }
        other => json!({ "success": true, "data": other }),
    };
    ToolResult {
        content: merged.to_string(),
        is_error: false,
    }
}

fn err_envelope(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({
            "success": false,
            "error": msg.into(),
        })
        .to_string(),
        is_error: true,
    }
}

fn err_envelope_with_node(msg: impl Into<String>, node: Option<&str>) -> ToolResult {
    let mut o = serde_json::Map::new();
    o.insert("success".to_string(), json!(false));
    o.insert("error".to_string(), json!(msg.into()));
    if let Some(n) = node {
        o.insert("node".to_string(), json!(n));
    }
    ToolResult {
        content: Value::Object(o).to_string(),
        is_error: true,
    }
}

fn map_meet_error(e: MeetError) -> String {
    e.to_string()
}

// ---------------------------------------------------------------------
// meet_join tool
// ---------------------------------------------------------------------

/// `meet_join` tool — join a Google Meet URL and start scraping live
/// captions. Spawns a bot via the wired [`GoogleMeetBackend`].
pub struct MeetJoinTool {
    backend: Arc<dyn GoogleMeetBackend>,
    /// v0.9.0 W1 B9 (2026-05-27): defaults to `false` so the registry's
    /// `is_available()` filter hides the tool when no real backend has
    /// been wired. `new(backend)` flips this on; the real backend lives
    /// in `wcore_agent::tool_backends::google_meet`.
    backend_configured: bool,
}

impl Default for MeetJoinTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullGoogleMeetBackend),
            backend_configured: false,
        }
    }
}

impl MeetJoinTool {
    pub fn new(backend: Arc<dyn GoogleMeetBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for MeetJoinTool {
    fn name(&self) -> &str {
        "meet_join"
    }

    /// v0.9.0 W1 B9: hidden when no `GoogleMeetBackend` is wired —
    /// `Default::default()` yields `backend_configured == false`, so the
    /// registry's `is_available()` filter drops the tool before the
    /// model ever sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Join a Google Meet call and start scraping live captions into a transcript. \
         Only meet.google.com URLs are accepted; no calendar scanning, no auto-dial. \
         Spawns a headless browser bot that runs in parallel — returns immediately. \
         Poll with meet_status and read captions with meet_transcript. The agent \
         must announce itself in the meeting (no automatic consent announcement)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Full https://meet.google.com/... URL. Required."
                },
                "mode": {
                    "type": "string",
                    "enum": ["transcribe", "realtime"],
                    "description":
                        "transcribe (default): listen-only, scrape captions. \
                         realtime: also enable agent speech via meet_say."
                },
                "guest_name": {
                    "type": "string",
                    "description": "Display name to use when joining as guest. Defaults to 'Wayland Agent'."
                },
                "duration": {
                    "type": "string",
                    "description": "Optional max duration before auto-leave (e.g. '30m', '2h', '90s')."
                },
                "headed": {
                    "type": "boolean",
                    "description": "Run the bot's browser headed instead of headless (debug only). Default false."
                },
                "node": {
                    "type": "string",
                    "description":
                        "Name of a registered remote node to run the bot on. Pass 'auto' \
                         to use the single registered node. Default: run locally."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // The backend holds at most one active meeting at a time;
        // concurrent joins would race on `.active.json`. Serialize.
        false
    }

    fn category(&self) -> ToolCategory {
        // Side-effecting: spawns / dispatches a bot.
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let url = match input.get("url").and_then(Value::as_str).map(str::trim) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return err_envelope("url is required"),
        };
        let meeting_id = match extract_meeting_id(&url) {
            Some(id) => id,
            None => {
                return err_envelope(format!(
                    "url must be a https://meet.google.com/<code> URL (got {})",
                    url.chars().take(80).collect::<String>()
                ));
            }
        };
        let mode = match input.get("mode").and_then(Value::as_str) {
            Some(raw) => match MeetMode::parse_strict(raw) {
                Ok(m) => m,
                Err(e) => return err_envelope(e),
            },
            None => MeetMode::Transcribe,
        };
        let guest_name = input
            .get("guest_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_GUEST_NAME)
            .to_string();
        let duration = input
            .get("duration")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let headed = input
            .get("headed")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let node = input
            .get("node")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let request = MeetJoinRequest {
            url,
            meeting_id: meeting_id.clone(),
            mode,
            guest_name,
            duration,
            headed,
            node: node.clone(),
        };

        match self.backend.join(request).await {
            Ok(resp) => {
                let mut o = serde_json::Map::new();
                o.insert("meetingId".to_string(), json!(resp.meeting_id));
                if let Some(pid) = resp.bot_pid {
                    o.insert("botPid".to_string(), json!(pid));
                }
                if let Some(path) = resp.transcript_path {
                    o.insert("transcriptPath".to_string(), json!(path));
                }
                if let Some(n) = node {
                    o.insert("node".to_string(), json!(n));
                }
                ok_envelope(Value::Object(o))
            }
            Err(e) => err_envelope_with_node(map_meet_error(e), node.as_deref()),
        }
    }

    fn describe(&self, input: &Value) -> String {
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("<missing>");
        let mode = input
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or("transcribe");
        format!("meet_join: url={url} mode={mode}")
    }
}

// ---------------------------------------------------------------------
// meet_status tool
// ---------------------------------------------------------------------

pub struct MeetStatusTool {
    backend: Arc<dyn GoogleMeetBackend>,
    /// v0.9.0 W1 B9: see `MeetJoinTool::backend_configured`.
    backend_configured: bool,
}

impl Default for MeetStatusTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullGoogleMeetBackend),
            backend_configured: false,
        }
    }
}

impl MeetStatusTool {
    pub fn new(backend: Arc<dyn GoogleMeetBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for MeetStatusTool {
    fn name(&self) -> &str {
        "meet_status"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Report the current Meet session state — whether the bot is alive, has joined, \
         is sitting in the lobby, number of transcript lines captured, and last-caption \
         timestamp."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "node": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true // read-only
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let node = input
            .get("node")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match self.backend.status(node.as_deref()).await {
            Ok(s) => {
                let mut o = serde_json::Map::new();
                o.insert("alive".to_string(), json!(s.alive));
                o.insert("inMeeting".to_string(), json!(s.in_meeting));
                o.insert("transcriptLines".to_string(), json!(s.transcript_lines));
                if let Some(t) = s.last_caption_at {
                    o.insert("lastCaptionAt".to_string(), json!(t));
                }
                if let Some(m) = s.message {
                    o.insert("message".to_string(), json!(m));
                }
                if let Some(n) = node {
                    o.insert("node".to_string(), json!(n));
                }
                ok_envelope(Value::Object(o))
            }
            Err(e) => err_envelope_with_node(map_meet_error(e), node.as_deref()),
        }
    }
}

// ---------------------------------------------------------------------
// meet_transcript tool
// ---------------------------------------------------------------------

pub struct MeetTranscriptTool {
    backend: Arc<dyn GoogleMeetBackend>,
    /// v0.9.0 W1 B9: see `MeetJoinTool::backend_configured`.
    backend_configured: bool,
}

impl Default for MeetTranscriptTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullGoogleMeetBackend),
            backend_configured: false,
        }
    }
}

impl MeetTranscriptTool {
    pub fn new(backend: Arc<dyn GoogleMeetBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for MeetTranscriptTool {
    fn name(&self) -> &str {
        "meet_transcript"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Read the scraped transcript for the active Meet session. Returns the full \
         transcript unless 'last' is set, in which case returns the last N caption \
         lines only."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "last": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional: return only the last N caption lines."
                },
                "node": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // Python coerces `last` via int() and treats <1 as None. Match
        // that: accept ints, ignore values < 1, ignore non-numeric.
        let last = input
            .get("last")
            .and_then(|v| v.as_i64())
            .filter(|&n| n >= 1)
            .map(|n| n as u32);
        let node = input
            .get("node")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match self.backend.transcript(node.as_deref(), last).await {
            Ok(t) => {
                let mut o = serde_json::Map::new();
                o.insert(
                    "lines".to_string(),
                    serde_json::to_value(&t.lines).unwrap_or_else(|_| json!([])),
                );
                o.insert("totalLines".to_string(), json!(t.total_lines));
                if let Some(n) = node {
                    o.insert("node".to_string(), json!(n));
                }
                ok_envelope(Value::Object(o))
            }
            Err(e) => err_envelope_with_node(map_meet_error(e), node.as_deref()),
        }
    }
}

// ---------------------------------------------------------------------
// meet_leave tool
// ---------------------------------------------------------------------

pub struct MeetLeaveTool {
    backend: Arc<dyn GoogleMeetBackend>,
    /// v0.9.0 W1 B9: see `MeetJoinTool::backend_configured`.
    backend_configured: bool,
}

impl Default for MeetLeaveTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullGoogleMeetBackend),
            backend_configured: false,
        }
    }
}

impl MeetLeaveTool {
    pub fn new(backend: Arc<dyn GoogleMeetBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for MeetLeaveTool {
    fn name(&self) -> &str {
        "meet_leave"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Leave the active Meet call cleanly, stop caption scraping, and finalize the \
         transcript file. Safe to call when no meeting is active — returns success=false \
         with a reason."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "node": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let node = input
            .get("node")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match self.backend.leave(node.as_deref()).await {
            Ok(r) => {
                let mut o = serde_json::Map::new();
                o.insert("meetingId".to_string(), json!(r.meeting_id));
                o.insert("reason".to_string(), json!(r.reason));
                if let Some(path) = r.transcript_path {
                    o.insert("transcriptPath".to_string(), json!(path));
                }
                if let Some(n) = node {
                    o.insert("node".to_string(), json!(n));
                }
                ok_envelope(Value::Object(o))
            }
            Err(e) => err_envelope_with_node(map_meet_error(e), node.as_deref()),
        }
    }
}

// ---------------------------------------------------------------------
// meet_say tool
// ---------------------------------------------------------------------

pub struct MeetSayTool {
    backend: Arc<dyn GoogleMeetBackend>,
    /// v0.9.0 W1 B9: see `MeetJoinTool::backend_configured`. Note that
    /// even when wired, `MeetSayTool` surfaces a `MeetApiCapabilityError`
    /// from the HTTP backend because Meet REST v2 does not expose
    /// in-call TTS — the Playwright bot path handles that.
    backend_configured: bool,
}

impl Default for MeetSayTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullGoogleMeetBackend),
            backend_configured: false,
        }
    }
}

impl MeetSayTool {
    pub fn new(backend: Arc<dyn GoogleMeetBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for MeetSayTool {
    fn name(&self) -> &str {
        "meet_say"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Speak text into the active Meet call. Requires the active meeting to have been \
         joined with mode='realtime'. The text is queued to the bot's TTS / audio bridge; \
         the generated audio is streamed into the browser's fake microphone. Returns \
         immediately — actual speech lags by a couple of seconds."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text to speak." },
                "node": { "type": "string" }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Order of utterances matters — serialize.
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let text = match input.get("text").and_then(Value::as_str).map(str::trim) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => return err_envelope("text is required"),
        };
        let node = input
            .get("node")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        match self.backend.say(node.as_deref(), &text).await {
            Ok(r) => {
                let mut o = serde_json::Map::new();
                o.insert("queued".to_string(), json!(r.queued));
                if let Some(n) = node {
                    o.insert("node".to_string(), json!(n));
                }
                ok_envelope(Value::Object(o))
            }
            Err(e) => err_envelope_with_node(map_meet_error(e), node.as_deref()),
        }
    }
}

// ---------------------------------------------------------------------
// Convenience: build all five tools off a single backend instance.
// ---------------------------------------------------------------------

/// Construct one instance of each `meet_*` tool sharing the given
/// backend. Returns a tuple in canonical order (join, status,
/// transcript, leave, say) so registry-bootstrap code can `let` them
/// out positionally without misordering.
pub fn build_meet_toolset(
    backend: Arc<dyn GoogleMeetBackend>,
) -> (
    MeetJoinTool,
    MeetStatusTool,
    MeetTranscriptTool,
    MeetLeaveTool,
    MeetSayTool,
) {
    (
        MeetJoinTool::new(backend.clone()),
        MeetStatusTool::new(backend.clone()),
        MeetTranscriptTool::new(backend.clone()),
        MeetLeaveTool::new(backend.clone()),
        MeetSayTool::new(backend),
    )
}

// =====================================================================
// Tests
// =====================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn parse_json(s: &str) -> Value {
        serde_json::from_str(s).expect("tool returned non-JSON")
    }

    // --- URL allowlist ---

    #[test]
    fn url_allowlist_accepts_canonical_form() {
        let ok = [
            "https://meet.google.com/abc-defg-hij",
            "https://meet.google.com/abcd-efgh-ijkl",
            "https://meet.google.com/abc-defg-hij?authuser=0",
        ];
        for u in ok {
            assert!(is_safe_meet_url(u), "expected pass: {u}");
            assert!(extract_meeting_id(u).is_some(), "id extract failed: {u}");
        }
    }

    #[test]
    fn url_allowlist_rejects_unsafe_forms() {
        let bad = [
            "",
            "meet.google.com/abc-defg-hij",
            "http://meet.google.com/abc-defg-hij",  // http
            "https://Meet.google.com/abc-defg-hij", // capital M
            "https://meet.google.com/abc-defg-hi",  // last group too short
            "https://meet.google.com/abc-defg-hij/extra",
            "https://meet.google.com/abc-defg-hij#frag",
            "https://meet.google.com/", // no code
            "https://example.com/abc-defg-hij",
            "javascript:alert(1)",
        ];
        for u in bad {
            assert!(!is_safe_meet_url(u), "expected reject: {u}");
            assert!(
                extract_meeting_id(u).is_none(),
                "id extract should fail: {u}"
            );
        }
    }

    #[test]
    fn meeting_id_extracted_correctly() {
        assert_eq!(
            extract_meeting_id("https://meet.google.com/abc-defg-hij").as_deref(),
            Some("abc-defg-hij")
        );
        assert_eq!(
            extract_meeting_id("  https://meet.google.com/abcd-efgh-ijkl?x=1  ").as_deref(),
            Some("abcd-efgh-ijkl")
        );
    }

    // --- mode parsing ---

    #[test]
    fn mode_strict_parse() {
        assert_eq!(
            MeetMode::parse_strict("transcribe").unwrap(),
            MeetMode::Transcribe
        );
        assert_eq!(
            MeetMode::parse_strict("REALTIME").unwrap(),
            MeetMode::Realtime
        );
        assert_eq!(
            MeetMode::parse_strict("  realtime  ").unwrap(),
            MeetMode::Realtime
        );
        let err = MeetMode::parse_strict("listen").unwrap_err();
        assert!(
            err.contains("transcribe") && err.contains("realtime"),
            "unexpected error: {err}"
        );
    }

    // --- schema shape ---

    #[test]
    fn tool_names_and_schemas() {
        let join = MeetJoinTool::default();
        assert_eq!(join.name(), "meet_join");
        let s = join.input_schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"], json!(["url"]));
        assert_eq!(s["additionalProperties"], json!(false));

        let status = MeetStatusTool::default();
        assert_eq!(status.name(), "meet_status");
        assert_eq!(status.category(), ToolCategory::Info);

        let transcript = MeetTranscriptTool::default();
        assert_eq!(transcript.name(), "meet_transcript");
        assert_eq!(
            transcript.input_schema()["properties"]["last"]["minimum"],
            json!(1)
        );

        let leave = MeetLeaveTool::default();
        assert_eq!(leave.name(), "meet_leave");
        assert_eq!(leave.category(), ToolCategory::Edit);

        let say = MeetSayTool::default();
        assert_eq!(say.name(), "meet_say");
        assert_eq!(say.input_schema()["required"], json!(["text"]));
    }

    // --- null backend fails loud for every operation ---

    #[tokio::test]
    async fn null_backend_fails_loud_on_every_op() {
        let join = MeetJoinTool::default();
        let r = join
            .execute(json!({"url": "https://meet.google.com/abc-defg-hij"}))
            .await;
        assert!(r.is_error);
        let v = parse_json(&r.content);
        assert_eq!(v["success"], false);
        assert!(v["error"].as_str().unwrap().contains("not configured"));

        let status = MeetStatusTool::default();
        let r = status.execute(json!({})).await;
        assert!(r.is_error);
        assert_eq!(parse_json(&r.content)["success"], false);

        let transcript = MeetTranscriptTool::default();
        let r = transcript.execute(json!({})).await;
        assert!(r.is_error);
        assert_eq!(parse_json(&r.content)["success"], false);

        let leave = MeetLeaveTool::default();
        let r = leave.execute(json!({})).await;
        assert!(r.is_error);
        assert_eq!(parse_json(&r.content)["success"], false);

        let say = MeetSayTool::default();
        let r = say.execute(json!({"text": "hello"})).await;
        assert!(r.is_error);
        assert_eq!(parse_json(&r.content)["success"], false);
    }

    // --- input validation (no backend call) ---

    #[tokio::test]
    async fn meet_join_rejects_unsafe_url_without_calling_backend() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetJoinTool::new(backend.clone());

        for bad in [
            json!({}),
            json!({"url": ""}),
            json!({"url": "http://meet.google.com/abc-defg-hij"}),
            json!({"url": "https://evil.example.com/abc-defg-hij"}),
        ] {
            let res = tool.execute(bad).await;
            assert!(res.is_error, "expected error for invalid url");
            assert_eq!(parse_json(&res.content)["success"], false);
        }
        assert!(
            backend.snapshot().is_empty(),
            "backend must not be invoked for invalid input"
        );
    }

    #[tokio::test]
    async fn meet_join_rejects_invalid_mode_without_calling_backend() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetJoinTool::new(backend.clone());
        let res = tool
            .execute(json!({
                "url": "https://meet.google.com/abc-defg-hij",
                "mode": "listen-only"
            }))
            .await;
        assert!(res.is_error);
        let v = parse_json(&res.content);
        assert!(
            v["error"].as_str().unwrap().contains("transcribe"),
            "unexpected error: {}",
            v["error"]
        );
        assert!(backend.snapshot().is_empty());
    }

    #[tokio::test]
    async fn meet_say_rejects_empty_text_without_calling_backend() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetSayTool::new(backend.clone());
        for body in [json!({}), json!({"text": ""}), json!({"text": "   \t\n"})] {
            let r = tool.execute(body).await;
            assert!(r.is_error);
            assert!(
                parse_json(&r.content)["error"]
                    .as_str()
                    .unwrap()
                    .contains("text")
            );
        }
        assert!(backend.snapshot().is_empty());
    }

    // --- capturing backend round-trip ---

    #[tokio::test]
    async fn meet_join_forwards_all_args_to_backend() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetJoinTool::new(backend.clone());
        let res = tool
            .execute(json!({
                "url": "https://meet.google.com/abc-defg-hij?authuser=1",
                "mode": "realtime",
                "guest_name": "Sentinel",
                "duration": "30m",
                "headed": true,
                "node": "macbook"
            }))
            .await;
        assert!(!res.is_error, "{}", res.content);
        let v = parse_json(&res.content);
        assert_eq!(v["success"], true);
        assert_eq!(v["meetingId"], "abc-defg-hij");
        assert_eq!(v["node"], "macbook");
        assert_eq!(v["botPid"], 4242);

        let calls = backend.snapshot();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            CapturedMeetCall::Join(req) => {
                assert_eq!(req.url, "https://meet.google.com/abc-defg-hij?authuser=1");
                assert_eq!(req.meeting_id, "abc-defg-hij");
                assert_eq!(req.mode, MeetMode::Realtime);
                assert_eq!(req.guest_name, "Sentinel");
                assert_eq!(req.duration.as_deref(), Some("30m"));
                assert!(req.headed);
                assert_eq!(req.node.as_deref(), Some("macbook"));
            }
            other => panic!("expected Join, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meet_join_defaults_apply() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetJoinTool::new(backend.clone());
        let res = tool
            .execute(json!({"url": "https://meet.google.com/abc-defg-hij"}))
            .await;
        assert!(!res.is_error);
        let calls = backend.snapshot();
        match &calls[0] {
            CapturedMeetCall::Join(req) => {
                assert_eq!(req.mode, MeetMode::Transcribe);
                assert_eq!(req.guest_name, DEFAULT_GUEST_NAME);
                assert!(req.duration.is_none());
                assert!(!req.headed);
                assert!(req.node.is_none());
            }
            _ => panic!("expected Join"),
        }
    }

    #[tokio::test]
    async fn meet_status_forwards_node_and_returns_envelope() {
        let backend_inner = CapturingGoogleMeetBackend {
            status_response: MeetStatusResponse {
                alive: true,
                in_meeting: false,
                transcript_lines: 7,
                last_caption_at: Some(123.5),
                message: Some("in lobby".to_string()),
            },
            ..Default::default()
        };
        let backend = Arc::new(backend_inner);
        let tool = MeetStatusTool::new(backend.clone());
        let r = tool.execute(json!({"node": "macbook"})).await;
        assert!(!r.is_error, "{}", r.content);
        let v = parse_json(&r.content);
        assert_eq!(v["success"], true);
        assert_eq!(v["alive"], true);
        assert_eq!(v["inMeeting"], false);
        assert_eq!(v["transcriptLines"], 7);
        assert_eq!(v["lastCaptionAt"], 123.5);
        assert_eq!(v["message"], "in lobby");
        assert_eq!(v["node"], "macbook");

        let calls = backend.snapshot();
        assert!(matches!(
            &calls[0],
            CapturedMeetCall::Status { node } if node.as_deref() == Some("macbook")
        ));
    }

    #[tokio::test]
    async fn meet_transcript_passes_last_when_positive_otherwise_drops() {
        let backend_inner = CapturingGoogleMeetBackend {
            transcript_response: MeetTranscriptResponse {
                lines: vec![
                    MeetTranscriptLine {
                        speaker: "Sean".into(),
                        text: "hi".into(),
                        t_seconds: Some(0.5),
                    },
                    MeetTranscriptLine {
                        speaker: "".into(),
                        text: "ok".into(),
                        t_seconds: None,
                    },
                ],
                total_lines: 99,
            },
            ..Default::default()
        };
        let backend = Arc::new(backend_inner);
        let tool = MeetTranscriptTool::new(backend.clone());

        // Positive last passes through.
        let r = tool.execute(json!({"last": 5})).await;
        assert!(!r.is_error);
        let v = parse_json(&r.content);
        assert_eq!(v["totalLines"], 99);
        assert_eq!(v["lines"].as_array().unwrap().len(), 2);
        assert_eq!(v["lines"][0]["speaker"], "Sean");
        assert_eq!(v["lines"][0]["text"], "hi");

        // Zero / negative get dropped (Python coerces to None).
        let _ = tool.execute(json!({"last": 0})).await;
        let _ = tool.execute(json!({"last": -3})).await;
        // Missing entirely.
        let _ = tool.execute(json!({})).await;

        let calls = backend.snapshot();
        assert_eq!(calls.len(), 4);
        match &calls[0] {
            CapturedMeetCall::Transcript { last, .. } => assert_eq!(*last, Some(5)),
            _ => panic!("expected Transcript"),
        }
        for call in &calls[1..] {
            match call {
                CapturedMeetCall::Transcript { last, .. } => {
                    assert!(last.is_none(), "expected last=None, got {last:?}");
                }
                other => panic!("expected Transcript, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn meet_leave_round_trip() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetLeaveTool::new(backend.clone());
        let r = tool.execute(json!({})).await;
        assert!(!r.is_error);
        let v = parse_json(&r.content);
        assert_eq!(v["success"], true);
        assert_eq!(v["meetingId"], "abc-defg-hij");
        assert!(v["reason"].as_str().unwrap().contains("meet_leave"));
        assert!(
            matches!(&backend.snapshot()[0], CapturedMeetCall::Leave { node } if node.is_none())
        );
    }

    #[tokio::test]
    async fn meet_say_forwards_text_and_node() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let tool = MeetSayTool::new(backend.clone());
        let r = tool
            .execute(json!({"text": "Hello team", "node": "macbook"}))
            .await;
        assert!(!r.is_error, "{}", r.content);
        let v = parse_json(&r.content);
        assert_eq!(v["success"], true);
        assert_eq!(v["queued"], 1);
        assert_eq!(v["node"], "macbook");

        match &backend.snapshot()[0] {
            CapturedMeetCall::Say { text, node } => {
                assert_eq!(text, "Hello team");
                assert_eq!(node.as_deref(), Some("macbook"));
            }
            other => panic!("expected Say, got {other:?}"),
        }
    }

    // --- backend errors propagate cleanly ---

    #[tokio::test]
    async fn backend_error_propagates_as_error_envelope() {
        struct AlwaysFails;
        #[async_trait]
        impl GoogleMeetBackend for AlwaysFails {
            async fn join(&self, _r: MeetJoinRequest) -> Result<MeetJoinResponse, MeetError> {
                Err(MeetError::DependencyMissing("playwright".to_string()))
            }
            async fn status(&self, _n: Option<&str>) -> Result<MeetStatusResponse, MeetError> {
                Err(MeetError::NoActiveSession("never started".to_string()))
            }
            async fn transcript(
                &self,
                _n: Option<&str>,
                _l: Option<u32>,
            ) -> Result<MeetTranscriptResponse, MeetError> {
                Err(MeetError::Other("boom".to_string()))
            }
            async fn leave(&self, _n: Option<&str>) -> Result<MeetLeaveResponse, MeetError> {
                Err(MeetError::NoActiveSession("nope".to_string()))
            }
            async fn say(&self, _n: Option<&str>, _t: &str) -> Result<MeetSayResponse, MeetError> {
                Err(MeetError::NotRealtime("transcribe-only".to_string()))
            }
        }
        let backend: Arc<dyn GoogleMeetBackend> = Arc::new(AlwaysFails);
        let join = MeetJoinTool::new(backend.clone());
        let r = join
            .execute(json!({"url": "https://meet.google.com/abc-defg-hij"}))
            .await;
        assert!(r.is_error);
        assert!(
            parse_json(&r.content)["error"]
                .as_str()
                .unwrap()
                .contains("playwright")
        );

        let say = MeetSayTool::new(backend.clone());
        let r = say.execute(json!({"text": "hi"})).await;
        assert!(r.is_error);
        let v = parse_json(&r.content);
        assert!(v["error"].as_str().unwrap().contains("realtime"));
    }

    // --- build_meet_toolset wires every tool to the same backend ---

    #[tokio::test]
    async fn build_meet_toolset_shares_one_backend() {
        let backend = Arc::new(CapturingGoogleMeetBackend::default());
        let (join, status, transcript, leave, say) = build_meet_toolset(backend.clone());
        let _ = join
            .execute(json!({"url": "https://meet.google.com/abc-defg-hij"}))
            .await;
        let _ = status.execute(json!({})).await;
        let _ = transcript.execute(json!({})).await;
        let _ = say.execute(json!({"text": "hi"})).await;
        let _ = leave.execute(json!({})).await;
        let calls = backend.snapshot();
        assert_eq!(calls.len(), 5, "every tool should hit the same backend");
    }

    // --- describe() summary ---

    #[test]
    fn describe_join_summary() {
        let t = MeetJoinTool::default();
        let s = t.describe(&json!({
            "url": "https://meet.google.com/abc-defg-hij",
            "mode": "realtime"
        }));
        assert!(s.contains("realtime"), "summary missing mode: {s}");
        assert!(s.contains("abc-defg-hij"), "summary missing url: {s}");
    }
}
