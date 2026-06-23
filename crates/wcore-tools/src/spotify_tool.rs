//! T3-3.7 (sub-wave 7): Spotify toolset — seven agent-facing tools that
//! share a single pluggable [`SpotifyBackend`].
//!
//! Ported from the prior Wayland Python engine (the 597-LOC Spotify
//! toolset and the URI/ID normalization helpers from its Spotify client).
//!
//! The Python original is built on top of a concrete `SpotifyClient`
//! (httpx-based, OAuth token from `wayland auth spotify`). Wayland's
//! engine deliberately ships **no HTTP client and no embedded Spotify
//! integration** — credentials, refresh, and the actual REST call are
//! the host's responsibility. This module mirrors the seam discipline
//! used in `vision_tools.rs` / `tts_tool.rs`:
//!
//! * [`SpotifyBackend`] — the host wires this at registration time.
//! * [`NullSpotifyBackend`] — default fail-loud backend, returns a
//!   structured "no backend configured" error on every call so the tool
//!   never silently appears to succeed (the NO-STUBS guarantee).
//! * [`CapturingSpotifyBackend`] — hermetic test backend that records
//!   every operation and returns canned JSON. Lives in the prod module
//!   so downstream crates can reuse it without `#[cfg(test)]` shenanigans.
//!
//! All seven Python handlers collapse into a single typed enum
//! [`SpotifyOp`] so the backend trait has exactly one entry point. Each
//! Tool struct ([`SpotifyPlaybackTool`], etc.) decodes its JSON args,
//! validates them, builds the matching `SpotifyOp` variant, and
//! dispatches.
//!
//! ## Differences vs Python
//!
//! * No HTTP, no OAuth, no auth-status probe — authentication is a pure
//!   backend concern. `NullSpotifyBackend` fails with a clear message
//!   ("No Spotify backend configured").
//! * No silent error swallowing. The Python `_describe_empty_playback`
//!   helper rewrites HTTP 204 responses into structured "not playing"
//!   payloads — preserved for `get_state` / `get_currently_playing` by
//!   detecting an `{"empty": true, ...}` payload coming back from the
//!   backend and reshaping it.
//! * URI/ID normalization is a pure function — no client round-trip
//!   needed. Ported verbatim from `spotify_client.normalize_spotify_id`
//!   / `normalize_spotify_uri` / `normalize_spotify_uris` so behaviour
//!   stays bit-identical (including the "expected-type mismatch" error
//!   message and the dedupe-by-order semantics).

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

// ---------------------------------------------------------------------
// URI / ID normalization — pure helpers, no I/O.
// ---------------------------------------------------------------------

/// Normalize a Spotify reference (`id`, `spotify:type:id` URI, or
/// `https://open.spotify.com/type/id` URL) to its bare ID. Mirrors
/// `normalize_spotify_id` in the Python original.
pub fn normalize_spotify_id(value: &str, expected_type: Option<&str>) -> Result<String, String> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err("Spotify id/uri/url is required.".to_string());
    }
    if let Some(rest) = cleaned.strip_prefix("spotify:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() >= 2 {
            let item_type = parts[0];
            if let Some(expected) = expected_type
                && item_type != expected
            {
                return Err(format!("Expected a Spotify {expected}, got {item_type}."));
            }
            return Ok(parts[1].to_string());
        }
    }
    if cleaned.contains("open.spotify.com") {
        let after_host = cleaned.split("open.spotify.com").nth(1).unwrap_or("");
        let path = after_host
            .trim_start_matches('/')
            .split(['?', '#'])
            .next()
            .unwrap_or("");
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        if parts.len() >= 2 {
            let item_type = parts[0];
            let item_id = parts[1];
            if let Some(expected) = expected_type
                && item_type != expected
            {
                return Err(format!("Expected a Spotify {expected}, got {item_type}."));
            }
            return Ok(item_id.to_string());
        }
    }
    Ok(cleaned.to_string())
}

/// Normalize a Spotify reference to a `spotify:type:id` URI. Mirrors
/// `normalize_spotify_uri`.
pub fn normalize_spotify_uri(value: &str, expected_type: Option<&str>) -> Result<String, String> {
    let cleaned = value.trim();
    if cleaned.is_empty() {
        return Err("Spotify URI/url/id is required.".to_string());
    }
    if let Some(rest) = cleaned.strip_prefix("spotify:") {
        if let Some(expected) = expected_type {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() >= 2 && parts[0] != expected {
                let got = parts[0];
                return Err(format!("Expected a Spotify {expected}, got {got}."));
            }
        }
        return Ok(cleaned.to_string());
    }
    let item_id = normalize_spotify_id(cleaned, expected_type)?;
    if let Some(expected) = expected_type {
        Ok(format!("spotify:{expected}:{item_id}"))
    } else {
        Ok(cleaned.to_string())
    }
}

/// Normalize a list of references to a deduped list of URIs.
/// Order-preserving dedupe (matches Python's `if uri not in uris`).
/// Empty input is an error.
pub fn normalize_spotify_uris(
    values: &[String],
    expected_type: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut uris: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let uri = normalize_spotify_uri(value, expected_type)?;
        if !uris.contains(&uri) {
            uris.push(uri);
        }
    }
    if uris.is_empty() {
        return Err("At least one Spotify item is required.".to_string());
    }
    Ok(uris)
}

// ---------------------------------------------------------------------
// Argument coercion helpers.
// ---------------------------------------------------------------------

fn coerce_limit(raw: Option<&Value>, default: i64, minimum: i64, maximum: i64) -> i64 {
    let value = raw
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|f| f as i64))
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .unwrap_or(default);
    value.clamp(minimum, maximum)
}

fn coerce_bool(raw: Option<&Value>, default: bool) -> bool {
    match raw {
        None => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Some(Value::Number(n)) => n.as_i64().map(|v| v != 0).unwrap_or(default),
        _ => default,
    }
}

fn as_list_of_strings(raw: Option<&Value>) -> Vec<String> {
    match raw {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                Vec::new()
            } else {
                vec![t.to_string()]
            }
        }
        Some(other) => vec![other.to_string()],
    }
}

// ---------------------------------------------------------------------
// SpotifyBackend trait + typed op enum.
// ---------------------------------------------------------------------

/// A typed Spotify operation handed to the backend. Each variant
/// represents one Python `SpotifyClient.*` method, post-normalization.
#[derive(Debug, Clone, PartialEq)]
pub enum SpotifyOp {
    // playback
    GetPlaybackState {
        market: Option<String>,
    },
    GetCurrentlyPlaying {
        market: Option<String>,
    },
    StartPlayback {
        device_id: Option<String>,
        context_uri: Option<String>,
        uris: Option<Vec<String>>,
        offset: Option<Value>,
        position_ms: Option<i64>,
    },
    PausePlayback {
        device_id: Option<String>,
    },
    SkipNext {
        device_id: Option<String>,
    },
    SkipPrevious {
        device_id: Option<String>,
    },
    Seek {
        position_ms: i64,
        device_id: Option<String>,
    },
    SetRepeat {
        state: String,
        device_id: Option<String>,
    },
    SetShuffle {
        state: bool,
        device_id: Option<String>,
    },
    SetVolume {
        volume_percent: i64,
        device_id: Option<String>,
    },
    RecentlyPlayed {
        limit: i64,
        after: Option<i64>,
        before: Option<i64>,
    },
    // devices
    GetDevices,
    TransferPlayback {
        device_id: String,
        play: bool,
    },
    // queue
    GetQueue,
    AddToQueue {
        uri: String,
        device_id: Option<String>,
    },
    // search
    Search {
        query: String,
        search_types: Vec<String>,
        limit: i64,
        offset: i64,
        market: Option<String>,
        include_external: Option<String>,
    },
    // playlists
    GetMyPlaylists {
        limit: i64,
        offset: i64,
    },
    GetPlaylist {
        playlist_id: String,
        market: Option<String>,
    },
    CreatePlaylist {
        name: String,
        public: bool,
        collaborative: bool,
        description: Option<String>,
    },
    AddPlaylistItems {
        playlist_id: String,
        uris: Vec<String>,
        position: Option<i64>,
    },
    RemovePlaylistItems {
        playlist_id: String,
        uris: Vec<String>,
        snapshot_id: Option<String>,
    },
    UpdatePlaylistDetails {
        playlist_id: String,
        name: Option<String>,
        public: Option<bool>,
        collaborative: Option<bool>,
        description: Option<String>,
    },
    // albums
    GetAlbum {
        album_id: String,
        market: Option<String>,
    },
    GetAlbumTracks {
        album_id: String,
        limit: i64,
        offset: i64,
        market: Option<String>,
    },
    // library
    GetSavedTracks {
        limit: i64,
        offset: i64,
        market: Option<String>,
    },
    GetSavedAlbums {
        limit: i64,
        offset: i64,
        market: Option<String>,
    },
    SaveLibraryItems {
        uris: Vec<String>,
    },
    RemoveSavedTracks {
        track_ids: Vec<String>,
    },
    RemoveSavedAlbums {
        album_ids: Vec<String>,
    },
}

/// Outcome of a backend dispatch.
#[derive(Debug, Clone)]
pub enum SpotifyOutcome {
    Ok(Value),
    Err {
        message: String,
        status_code: Option<u16>,
    },
}

/// Pluggable Spotify backend. The host implements this against its
/// chosen HTTP client + OAuth token store.
///
/// **Empty-playback sentinel.** Backends should surface HTTP 204 from
/// `GET /me/player` / currently-playing as `Ok({"empty": true,
/// "status_code": 204, "message": "..."})` so the tool reshapes into
/// the user-visible "is_playing: false" / "has_active_device: false"
/// envelopes (mirrors `_describe_empty_playback`).
#[async_trait]
pub trait SpotifyBackend: Send + Sync {
    async fn dispatch(&self, op: SpotifyOp) -> SpotifyOutcome;
}

/// Default fail-loud backend.
pub struct NullSpotifyBackend;

#[async_trait]
impl SpotifyBackend for NullSpotifyBackend {
    async fn dispatch(&self, _op: SpotifyOp) -> SpotifyOutcome {
        SpotifyOutcome::Err {
            message: "No Spotify backend configured. Wire a SpotifyBackend implementation \
                      (typically via the host's auth + httpx layer) when constructing the \
                      Spotify tools to enable Spotify control."
                .to_string(),
            status_code: None,
        }
    }
}

/// In-memory backend that records every dispatch for tests.
pub struct CapturingSpotifyBackend {
    response: Value,
    pub captured: Mutex<Vec<SpotifyOp>>,
}

impl CapturingSpotifyBackend {
    pub fn new(canned_response: Value) -> Self {
        Self {
            response: canned_response,
            captured: Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Vec<SpotifyOp> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl SpotifyBackend for CapturingSpotifyBackend {
    async fn dispatch(&self, op: SpotifyOp) -> SpotifyOutcome {
        self.captured.lock().push(op);
        SpotifyOutcome::Ok(self.response.clone())
    }
}

// ---------------------------------------------------------------------
// Shared dispatch helpers.
// ---------------------------------------------------------------------

fn describe_empty_playback(payload: &Value, action: &str) -> Option<Value> {
    let obj = payload.as_object()?;
    if !obj.get("empty").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let status_code = obj
        .get("status_code")
        .and_then(Value::as_i64)
        .unwrap_or(204);
    let message = obj.get("message").and_then(Value::as_str).map(String::from);
    match action {
        "get_currently_playing" => Some(json!({
            "success": true,
            "action": action,
            "is_playing": false,
            "status_code": status_code,
            "message": message.unwrap_or_else(|| "Spotify is not currently playing anything.".into()),
        })),
        "get_state" => Some(json!({
            "success": true,
            "action": action,
            "has_active_device": false,
            "status_code": status_code,
            "message": message.unwrap_or_else(|| "No active Spotify playback session was found.".into()),
        })),
        _ => None,
    }
}

fn tool_ok(value: Value) -> ToolResult {
    ToolResult {
        content: value.to_string(),
        is_error: false,
    }
}

fn tool_err(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "success": false, "error": message.into() }).to_string(),
        is_error: true,
    }
}

fn tool_err_with_status(message: impl Into<String>, status_code: Option<u16>) -> ToolResult {
    let mut payload = Map::new();
    payload.insert("success".into(), json!(false));
    payload.insert("error".into(), json!(message.into()));
    if let Some(code) = status_code {
        payload.insert("status_code".into(), json!(code));
    }
    ToolResult {
        content: Value::Object(payload).to_string(),
        is_error: true,
    }
}

async fn dispatch_to_tool_result(backend: &Arc<dyn SpotifyBackend>, op: SpotifyOp) -> ToolResult {
    match backend.dispatch(op).await {
        SpotifyOutcome::Ok(v) => tool_ok(v),
        SpotifyOutcome::Err {
            message,
            status_code,
        } => tool_err_with_status(message, status_code),
    }
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn get_str_owned(args: &Value, key: &str) -> Option<String> {
    get_str(args, key).map(|s| s.to_string())
}

fn get_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

// ---------------------------------------------------------------------
// Tool: spotify_playback
// ---------------------------------------------------------------------

pub struct SpotifyPlaybackTool {
    backend: Arc<dyn SpotifyBackend>,
    /// v0.9.0 Wave-1 B0 (2026-05-27): Spotify (7 tools) deferred to
    /// v0.9.1. `Self::default()` sets this to `false` so `is_available()`
    /// returns `false` and `ToolRegistry::register` hides the tool.
    /// Wave-1 v0.9.1 will flip this on via the OAuth wiring (B0
    /// scaffolding) when `SPOTIFY_REFRESH_TOKEN` is configured.
    backend_configured: bool,
}

impl Default for SpotifyPlaybackTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyPlaybackTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyPlaybackTool {
    fn name(&self) -> &str {
        "spotify_playback"
    }

    /// v0.9.0 W1 B0 (2026-05-27): Spotify deferred to v0.9.1. Hidden by
    /// default; `ToolRegistry::register` skips unavailable tools so the
    /// model never sees a tool it cannot call. Wave-1 v0.9.1 wires the
    /// real backend and flips this flag.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Control Spotify playback, inspect the active playback state, or fetch recently played \
         tracks."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "get_state", "get_currently_playing", "play", "pause", "next",
                        "previous", "seek", "set_repeat", "set_shuffle", "set_volume",
                        "recently_played"
                    ]
                },
                "device_id": {"type": "string"},
                "market": {"type": "string"},
                "context_uri": {"type": "string"},
                "uris": {"type": "array", "items": {"type": "string"}},
                "offset": {"type": "object"},
                "position_ms": {"type": "integer"},
                "state": {
                    "description": "For set_repeat use track/context/off. For set_shuffle use boolean-like true/false.",
                    "oneOf": [{"type": "string"}, {"type": "boolean"}]
                },
                "volume_percent": {"type": "integer"},
                "limit": {"type": "integer"},
                "after": {"type": "integer"},
                "before": {"type": "integer"}
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "get_state".to_string());

        let op = match action.as_str() {
            "get_state" => SpotifyOp::GetPlaybackState {
                market: get_str_owned(&input, "market"),
            },
            "get_currently_playing" => SpotifyOp::GetCurrentlyPlaying {
                market: get_str_owned(&input, "market"),
            },
            "play" => {
                let context_uri = match get_str(&input, "context_uri") {
                    Some(raw) if !raw.is_empty() => {
                        let item_type = if raw.starts_with("spotify:album:")
                            || raw.contains("/album/")
                        {
                            Some("album")
                        } else if raw.starts_with("spotify:playlist:") || raw.contains("/playlist/")
                        {
                            Some("playlist")
                        } else if raw.starts_with("spotify:artist:") || raw.contains("/artist/") {
                            Some("artist")
                        } else {
                            None
                        };
                        match normalize_spotify_uri(raw, item_type) {
                            Ok(u) => Some(u),
                            Err(e) => return tool_err(e),
                        }
                    }
                    _ => None,
                };
                let raw_uris = as_list_of_strings(input.get("uris"));
                let uris = if raw_uris.is_empty() {
                    None
                } else {
                    match normalize_spotify_uris(&raw_uris, Some("track")) {
                        Ok(u) => Some(u),
                        Err(e) => return tool_err(e),
                    }
                };
                let offset = input.get("offset").and_then(|v| v.as_object()).map(|obj| {
                    let filtered: Map<String, Value> = obj
                        .iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Value::Object(filtered)
                });
                SpotifyOp::StartPlayback {
                    device_id: get_str_owned(&input, "device_id"),
                    context_uri,
                    uris,
                    offset,
                    position_ms: get_i64(&input, "position_ms"),
                }
            }
            "pause" => SpotifyOp::PausePlayback {
                device_id: get_str_owned(&input, "device_id"),
            },
            "next" => SpotifyOp::SkipNext {
                device_id: get_str_owned(&input, "device_id"),
            },
            "previous" => SpotifyOp::SkipPrevious {
                device_id: get_str_owned(&input, "device_id"),
            },
            "seek" => {
                let position_ms = match get_i64(&input, "position_ms") {
                    Some(v) => v,
                    None => return tool_err("position_ms is required for action='seek'"),
                };
                SpotifyOp::Seek {
                    position_ms,
                    device_id: get_str_owned(&input, "device_id"),
                }
            }
            "set_repeat" => {
                let state = get_str(&input, "state")
                    .map(|s| s.trim().to_ascii_lowercase())
                    .unwrap_or_default();
                if !matches!(state.as_str(), "track" | "context" | "off") {
                    return tool_err("state must be one of: track, context, off");
                }
                SpotifyOp::SetRepeat {
                    state,
                    device_id: get_str_owned(&input, "device_id"),
                }
            }
            "set_shuffle" => SpotifyOp::SetShuffle {
                state: coerce_bool(input.get("state"), false),
                device_id: get_str_owned(&input, "device_id"),
            },
            "set_volume" => {
                let volume_raw = match get_i64(&input, "volume_percent") {
                    Some(v) => v,
                    None => return tool_err("volume_percent is required for action='set_volume'"),
                };
                let clamped = volume_raw.clamp(0, 100);
                SpotifyOp::SetVolume {
                    volume_percent: clamped,
                    device_id: get_str_owned(&input, "device_id"),
                }
            }
            "recently_played" => {
                let after = get_i64(&input, "after");
                let before = get_i64(&input, "before");
                if after.is_some() && before.is_some() {
                    return tool_err("Provide only one of 'after' or 'before'");
                }
                SpotifyOp::RecentlyPlayed {
                    limit: coerce_limit(input.get("limit"), 20, 1, 50),
                    after,
                    before,
                }
            }
            other => return tool_err(format!("Unknown spotify_playback action: {other}")),
        };

        match self.backend.dispatch(op).await {
            SpotifyOutcome::Ok(payload) => {
                let action_key = action.as_str();
                let reshaped = if matches!(action_key, "get_state" | "get_currently_playing") {
                    describe_empty_playback(&payload, action_key)
                } else {
                    None
                };
                let final_payload = reshaped.unwrap_or_else(|| match action_key {
                    "get_state" | "get_currently_playing" | "recently_played" => payload,
                    _ => json!({
                        "success": true,
                        "action": action_key,
                        "result": payload,
                    }),
                });
                tool_ok(final_payload)
            }
            SpotifyOutcome::Err {
                message,
                status_code,
            } => tool_err_with_status(message, status_code),
        }
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_devices
// ---------------------------------------------------------------------

pub struct SpotifyDevicesTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifyDevicesTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyDevicesTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyDevicesTool {
    fn name(&self) -> &str {
        "spotify_devices"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "List Spotify Connect devices or transfer playback to a different device."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "transfer"]},
                "device_id": {"type": "string"},
                "play": {"type": "boolean"}
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        get_str(input, "action")
            .map(|s| s.trim().eq_ignore_ascii_case("list"))
            .unwrap_or(true)
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "list".to_string());

        match action.as_str() {
            "list" => dispatch_to_tool_result(&self.backend, SpotifyOp::GetDevices).await,
            "transfer" => {
                let device_id = get_str_owned(&input, "device_id").unwrap_or_default();
                if device_id.trim().is_empty() {
                    return tool_err("device_id is required for action='transfer'");
                }
                match self
                    .backend
                    .dispatch(SpotifyOp::TransferPlayback {
                        device_id,
                        play: coerce_bool(input.get("play"), false),
                    })
                    .await
                {
                    SpotifyOutcome::Ok(result) => tool_ok(json!({
                        "success": true,
                        "action": "transfer",
                        "result": result,
                    })),
                    SpotifyOutcome::Err {
                        message,
                        status_code,
                    } => tool_err_with_status(message, status_code),
                }
            }
            other => tool_err(format!("Unknown spotify_devices action: {other}")),
        }
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_queue
// ---------------------------------------------------------------------

pub struct SpotifyQueueTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifyQueueTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyQueueTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyQueueTool {
    fn name(&self) -> &str {
        "spotify_queue"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Inspect the user's Spotify queue or add an item to it."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["get", "add"]},
                "uri": {"type": "string"},
                "device_id": {"type": "string"}
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        get_str(input, "action")
            .map(|s| s.trim().eq_ignore_ascii_case("get"))
            .unwrap_or(true)
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "get".to_string());

        match action.as_str() {
            "get" => dispatch_to_tool_result(&self.backend, SpotifyOp::GetQueue).await,
            "add" => {
                let raw_uri = get_str(&input, "uri").unwrap_or("");
                let uri = match normalize_spotify_uri(raw_uri, None) {
                    Ok(u) => u,
                    Err(e) => return tool_err(e),
                };
                match self
                    .backend
                    .dispatch(SpotifyOp::AddToQueue {
                        uri: uri.clone(),
                        device_id: get_str_owned(&input, "device_id"),
                    })
                    .await
                {
                    SpotifyOutcome::Ok(result) => tool_ok(json!({
                        "success": true,
                        "action": "add",
                        "uri": uri,
                        "result": result,
                    })),
                    SpotifyOutcome::Err {
                        message,
                        status_code,
                    } => tool_err_with_status(message, status_code),
                }
            }
            other => tool_err(format!("Unknown spotify_queue action: {other}")),
        }
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_search
// ---------------------------------------------------------------------

const SEARCH_TYPE_ALLOWLIST: &[&str] = &[
    "album",
    "artist",
    "playlist",
    "track",
    "show",
    "episode",
    "audiobook",
];

pub struct SpotifySearchTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifySearchTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifySearchTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifySearchTool {
    fn name(&self) -> &str {
        "spotify_search"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Search the Spotify catalog for tracks, albums, artists, playlists, shows, or episodes."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "types": {"type": "array", "items": {"type": "string"}},
                "type": {"type": "string"},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
                "market": {"type": "string"},
                "include_external": {"type": "string"}
            },
            "required": ["query"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = get_str(&input, "query").map(str::trim).unwrap_or("");
        if query.is_empty() {
            return tool_err("query is required");
        }

        let raw_types = if input.get("types").is_some() {
            as_list_of_strings(input.get("types"))
        } else if input.get("type").is_some() {
            as_list_of_strings(input.get("type"))
        } else {
            vec!["track".to_string()]
        };
        let search_types: Vec<String> = raw_types
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .filter(|s| SEARCH_TYPE_ALLOWLIST.contains(&s.as_str()))
            .collect();
        if search_types.is_empty() {
            return tool_err(
                "types must contain one or more of: album, artist, playlist, track, show, \
                 episode, audiobook",
            );
        }

        let limit = coerce_limit(input.get("limit"), 10, 1, 50);
        let offset = get_i64(&input, "offset").unwrap_or(0).max(0);

        dispatch_to_tool_result(
            &self.backend,
            SpotifyOp::Search {
                query: query.to_string(),
                search_types,
                limit,
                offset,
                market: get_str_owned(&input, "market"),
                include_external: get_str_owned(&input, "include_external"),
            },
        )
        .await
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_playlists
// ---------------------------------------------------------------------

pub struct SpotifyPlaylistsTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifyPlaylistsTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyPlaylistsTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyPlaylistsTool {
    fn name(&self) -> &str {
        "spotify_playlists"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "List, inspect, create, update, and modify Spotify playlists."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get", "create", "add_items", "remove_items", "update_details"]
                },
                "playlist_id": {"type": "string"},
                "market": {"type": "string"},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "public": {"type": "boolean"},
                "collaborative": {"type": "boolean"},
                "uris": {"type": "array", "items": {"type": "string"}},
                "position": {"type": "integer"},
                "snapshot_id": {"type": "string"}
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        match get_str(input, "action").map(|s| s.trim().to_ascii_lowercase()) {
            Some(a) => a == "list" || a == "get",
            None => false,
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "list".to_string());

        let op = match action.as_str() {
            "list" => SpotifyOp::GetMyPlaylists {
                limit: coerce_limit(input.get("limit"), 20, 1, 50),
                offset: get_i64(&input, "offset").unwrap_or(0).max(0),
            },
            "get" => {
                let playlist_id = match normalize_spotify_id(
                    get_str(&input, "playlist_id").unwrap_or(""),
                    Some("playlist"),
                ) {
                    Ok(id) => id,
                    Err(e) => return tool_err(e),
                };
                SpotifyOp::GetPlaylist {
                    playlist_id,
                    market: get_str_owned(&input, "market"),
                }
            }
            "create" => {
                let name = get_str(&input, "name").map(str::trim).unwrap_or("");
                if name.is_empty() {
                    return tool_err("name is required for action='create'");
                }
                SpotifyOp::CreatePlaylist {
                    name: name.to_string(),
                    public: coerce_bool(input.get("public"), false),
                    collaborative: coerce_bool(input.get("collaborative"), false),
                    description: get_str_owned(&input, "description"),
                }
            }
            "add_items" => {
                let playlist_id = match normalize_spotify_id(
                    get_str(&input, "playlist_id").unwrap_or(""),
                    Some("playlist"),
                ) {
                    Ok(id) => id,
                    Err(e) => return tool_err(e),
                };
                let raw_uris = as_list_of_strings(input.get("uris"));
                let uris = match normalize_spotify_uris(&raw_uris, None) {
                    Ok(u) => u,
                    Err(e) => return tool_err(e),
                };
                SpotifyOp::AddPlaylistItems {
                    playlist_id,
                    uris,
                    position: get_i64(&input, "position"),
                }
            }
            "remove_items" => {
                let playlist_id = match normalize_spotify_id(
                    get_str(&input, "playlist_id").unwrap_or(""),
                    Some("playlist"),
                ) {
                    Ok(id) => id,
                    Err(e) => return tool_err(e),
                };
                let raw_uris = as_list_of_strings(input.get("uris"));
                let uris = match normalize_spotify_uris(&raw_uris, None) {
                    Ok(u) => u,
                    Err(e) => return tool_err(e),
                };
                SpotifyOp::RemovePlaylistItems {
                    playlist_id,
                    uris,
                    snapshot_id: get_str_owned(&input, "snapshot_id"),
                }
            }
            "update_details" => {
                let playlist_id = match normalize_spotify_id(
                    get_str(&input, "playlist_id").unwrap_or(""),
                    Some("playlist"),
                ) {
                    Ok(id) => id,
                    Err(e) => return tool_err(e),
                };
                SpotifyOp::UpdatePlaylistDetails {
                    playlist_id,
                    name: get_str_owned(&input, "name"),
                    public: input.get("public").and_then(Value::as_bool),
                    collaborative: input.get("collaborative").and_then(Value::as_bool),
                    description: get_str_owned(&input, "description"),
                }
            }
            other => return tool_err(format!("Unknown spotify_playlists action: {other}")),
        };

        dispatch_to_tool_result(&self.backend, op).await
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_albums
// ---------------------------------------------------------------------

pub struct SpotifyAlbumsTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifyAlbumsTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyAlbumsTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyAlbumsTool {
    fn name(&self) -> &str {
        "spotify_albums"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Fetch Spotify album metadata or album tracks."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["get", "tracks"]},
                "album_id": {"type": "string"},
                "id": {"type": "string"},
                "market": {"type": "string"},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"}
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "get".to_string());

        let raw_id = get_str(&input, "album_id")
            .or_else(|| get_str(&input, "id"))
            .unwrap_or("");
        let album_id = match normalize_spotify_id(raw_id, Some("album")) {
            Ok(id) => id,
            Err(e) => return tool_err(e),
        };

        let op = match action.as_str() {
            "get" => SpotifyOp::GetAlbum {
                album_id,
                market: get_str_owned(&input, "market"),
            },
            "tracks" => SpotifyOp::GetAlbumTracks {
                album_id,
                limit: coerce_limit(input.get("limit"), 20, 1, 50),
                offset: get_i64(&input, "offset").unwrap_or(0).max(0),
                market: get_str_owned(&input, "market"),
            },
            other => return tool_err(format!("Unknown spotify_albums action: {other}")),
        };

        dispatch_to_tool_result(&self.backend, op).await
    }
}

// ---------------------------------------------------------------------
// Tool: spotify_library
// ---------------------------------------------------------------------

pub struct SpotifyLibraryTool {
    backend: Arc<dyn SpotifyBackend>,
    backend_configured: bool,
}

impl Default for SpotifyLibraryTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullSpotifyBackend),
            backend_configured: false,
        }
    }
}

impl SpotifyLibraryTool {
    pub fn new(backend: Arc<dyn SpotifyBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for SpotifyLibraryTool {
    fn name(&self) -> &str {
        "spotify_library"
    }

    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "List, save, or remove the user's saved Spotify tracks or albums. Use `kind` to select \
         which."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["tracks", "albums"],
                    "description": "Which library to operate on"
                },
                "action": {"type": "string", "enum": ["list", "save", "remove"]},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
                "market": {"type": "string"},
                "uris": {"type": "array", "items": {"type": "string"}},
                "ids": {"type": "array", "items": {"type": "string"}},
                "items": {"type": "array", "items": {"type": "string"}}
            },
            "required": ["kind", "action"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        get_str(input, "action")
            .map(|s| s.trim().eq_ignore_ascii_case("list"))
            .unwrap_or(false)
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let kind = get_str(&input, "kind")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        if !matches!(kind.as_str(), "tracks" | "albums") {
            return tool_err("kind must be one of: tracks, albums");
        }
        let action = get_str(&input, "action")
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_else(|| "list".to_string());
        let item_type = if kind == "tracks" { "track" } else { "album" };

        let op = match action.as_str() {
            "list" => {
                let limit = coerce_limit(input.get("limit"), 20, 1, 50);
                let offset = get_i64(&input, "offset").unwrap_or(0).max(0);
                let market = get_str_owned(&input, "market");
                if kind == "tracks" {
                    SpotifyOp::GetSavedTracks {
                        limit,
                        offset,
                        market,
                    }
                } else {
                    SpotifyOp::GetSavedAlbums {
                        limit,
                        offset,
                        market,
                    }
                }
            }
            "save" => {
                let raw = if input.get("uris").is_some() {
                    as_list_of_strings(input.get("uris"))
                } else {
                    as_list_of_strings(input.get("items"))
                };
                let uris = match normalize_spotify_uris(&raw, Some(item_type)) {
                    Ok(u) => u,
                    Err(e) => return tool_err(e),
                };
                SpotifyOp::SaveLibraryItems { uris }
            }
            "remove" => {
                let raw = if input.get("ids").is_some() {
                    as_list_of_strings(input.get("ids"))
                } else {
                    as_list_of_strings(input.get("items"))
                };
                let mut ids: Vec<String> = Vec::with_capacity(raw.len());
                for item in &raw {
                    match normalize_spotify_id(item, Some(item_type)) {
                        Ok(id) => ids.push(id),
                        Err(e) => return tool_err(e),
                    }
                }
                if ids.is_empty() {
                    return tool_err("ids/items is required for action='remove'");
                }
                if kind == "tracks" {
                    SpotifyOp::RemoveSavedTracks { track_ids: ids }
                } else {
                    SpotifyOp::RemoveSavedAlbums { album_ids: ids }
                }
            }
            other => return tool_err(format!("Unknown spotify_library action: {other}")),
        };

        dispatch_to_tool_result(&self.backend, op).await
    }
}

// ---------------------------------------------------------------------
// Convenience: register all seven tools sharing one backend.
// ---------------------------------------------------------------------

/// Register all seven Spotify tools into the supplied registry, each
/// sharing the same backend `Arc`. Hosts typically call this once at
/// startup after the OAuth token is loaded.
pub fn register_spotify_tools(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn SpotifyBackend>,
) {
    registry.register(Box::new(SpotifyPlaybackTool::new(backend.clone())));
    registry.register(Box::new(SpotifyDevicesTool::new(backend.clone())));
    registry.register(Box::new(SpotifyQueueTool::new(backend.clone())));
    registry.register(Box::new(SpotifySearchTool::new(backend.clone())));
    registry.register(Box::new(SpotifyPlaylistsTool::new(backend.clone())));
    registry.register(Box::new(SpotifyAlbumsTool::new(backend.clone())));
    registry.register(Box::new(SpotifyLibraryTool::new(backend)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_exec<T: Tool>(t: &T, input: Value) -> ToolResult {
        futures::executor::block_on(t.execute(input))
    }

    fn parse(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("valid JSON")
    }

    // -----------------------------------------------------------------
    // URI / ID normalization
    // -----------------------------------------------------------------

    #[test]
    fn normalize_id_handles_bare_id_uri_and_url() {
        assert_eq!(
            normalize_spotify_id("4iV5W9uYEdYUVa79Axb7Rh", None).unwrap(),
            "4iV5W9uYEdYUVa79Axb7Rh"
        );
        assert_eq!(
            normalize_spotify_id("spotify:track:4iV5W9uYEdYUVa79Axb7Rh", Some("track")).unwrap(),
            "4iV5W9uYEdYUVa79Axb7Rh"
        );
        assert_eq!(
            normalize_spotify_id(
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc",
                Some("playlist")
            )
            .unwrap(),
            "37i9dQZF1DXcBWIGoYBM5M"
        );
        let err = normalize_spotify_id("spotify:album:ABCDEF", Some("track")).unwrap_err();
        assert!(err.contains("Expected a Spotify track, got album"));
        assert!(normalize_spotify_id("   ", None).is_err());
    }

    #[test]
    fn normalize_uri_and_uris_dedupe_and_wrap() {
        assert_eq!(
            normalize_spotify_uri("ABCDEFG", Some("track")).unwrap(),
            "spotify:track:ABCDEFG"
        );
        assert_eq!(
            normalize_spotify_uri("spotify:track:XYZ", Some("track")).unwrap(),
            "spotify:track:XYZ"
        );
        assert!(normalize_spotify_uri("spotify:album:XYZ", Some("track")).is_err());
        let input = vec![
            "spotify:track:A".to_string(),
            "B".to_string(),
            "spotify:track:A".to_string(),
        ];
        let out = normalize_spotify_uris(&input, Some("track")).unwrap();
        assert_eq!(
            out,
            vec!["spotify:track:A".to_string(), "spotify:track:B".to_string()]
        );
        assert!(normalize_spotify_uris(&[], Some("track")).is_err());
    }

    // -----------------------------------------------------------------
    // Backend dispatch — happy paths.
    // -----------------------------------------------------------------

    #[test]
    fn playback_play_normalizes_uris_and_calls_backend() {
        let backend = Arc::new(CapturingSpotifyBackend::new(json!({"ok": true})));
        let tool = SpotifyPlaybackTool::new(backend.clone());
        let r = must_exec(
            &tool,
            json!({
                "action": "play",
                "uris": ["4iV5W9uYEdYUVa79Axb7Rh", "spotify:track:OtherTrackId"],
                "context_uri": "https://open.spotify.com/album/ABC123",
                "device_id": "device-1",
            }),
        );
        assert!(!r.is_error, "expected success: {}", r.content);
        let v = parse(&r);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["action"], json!("play"));
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            SpotifyOp::StartPlayback {
                uris,
                context_uri,
                device_id,
                ..
            } => {
                let u = uris.as_ref().expect("uris populated");
                assert_eq!(u.len(), 2);
                assert!(u[0].starts_with("spotify:track:"));
                assert_eq!(context_uri.as_deref(), Some("spotify:album:ABC123"));
                assert_eq!(device_id.as_deref(), Some("device-1"));
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn playback_get_state_reshapes_empty_sentinel() {
        let backend = Arc::new(CapturingSpotifyBackend::new(json!({
            "empty": true,
            "status_code": 204,
            "message": "No active device.",
        })));
        let tool = SpotifyPlaybackTool::new(backend);
        let r = must_exec(&tool, json!({"action": "get_state"}));
        assert!(!r.is_error);
        let v = parse(&r);
        assert_eq!(v["has_active_device"], json!(false));
        assert_eq!(v["status_code"], json!(204));
        assert_eq!(v["message"], json!("No active device."));
        assert_eq!(v["action"], json!("get_state"));
    }

    #[test]
    fn search_validates_query_and_filters_types() {
        let backend = Arc::new(CapturingSpotifyBackend::new(
            json!({"tracks": {"items": []}}),
        ));
        let tool = SpotifySearchTool::new(backend.clone());

        let r = must_exec(&tool, json!({}));
        assert!(r.is_error);
        assert!(r.content.contains("query is required"));

        let r = must_exec(&tool, json!({"query": "x", "types": ["pony"]}));
        assert!(r.is_error);
        assert!(r.content.contains("album, artist, playlist"));

        let r = must_exec(
            &tool,
            json!({"query": "kanye", "types": ["TRACK", "junk", "Artist"]}),
        );
        assert!(!r.is_error, "{}", r.content);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            SpotifyOp::Search {
                query,
                search_types,
                limit,
                ..
            } => {
                assert_eq!(query, "kanye");
                assert_eq!(
                    search_types,
                    &vec!["track".to_string(), "artist".to_string()]
                );
                assert_eq!(*limit, 10);
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn library_remove_requires_ids_and_normalizes() {
        let backend = Arc::new(CapturingSpotifyBackend::new(json!({"ok": true})));
        let tool = SpotifyLibraryTool::new(backend.clone());

        let r = must_exec(&tool, json!({"kind": "tracks", "action": "remove"}));
        assert!(r.is_error);
        assert!(
            r.content.contains("required") || r.content.contains("At least one"),
            "got: {}",
            r.content
        );

        let r = must_exec(
            &tool,
            json!({
                "kind": "albums",
                "action": "remove",
                "ids": ["spotify:album:ABC", "DEF"],
            }),
        );
        assert!(!r.is_error, "{}", r.content);
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            SpotifyOp::RemoveSavedAlbums { album_ids } => {
                assert_eq!(album_ids, &vec!["ABC".to_string(), "DEF".to_string()]);
            }
            other => panic!("unexpected op: {other:?}"),
        }

        let r = must_exec(
            &tool,
            json!({
                "kind": "tracks",
                "action": "remove",
                "ids": ["spotify:album:ABC"],
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Expected a Spotify track"));
    }

    // -----------------------------------------------------------------
    // Fail-loud + invalid input.
    // -----------------------------------------------------------------

    #[test]
    fn null_backend_fails_loudly_on_every_tool() {
        let r = must_exec(&SpotifyPlaybackTool::default(), json!({"action": "pause"}));
        assert!(r.is_error);
        assert!(r.content.contains("No Spotify backend configured"));

        let r = must_exec(&SpotifyDevicesTool::default(), json!({"action": "list"}));
        assert!(r.is_error);
        assert!(r.content.contains("No Spotify backend configured"));

        let r = must_exec(
            &SpotifySearchTool::default(),
            json!({"query": "x", "types": ["track"]}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("No Spotify backend configured"));

        let r = must_exec(&SpotifyAlbumsTool::default(), json!({"action": "get"}));
        assert!(r.is_error);
        assert!(r.content.contains("required") || r.content.contains("No Spotify backend"));
    }

    #[test]
    fn invalid_input_rejected() {
        let backend = Arc::new(CapturingSpotifyBackend::new(json!({})));

        let tool = SpotifyPlaybackTool::new(backend.clone());
        let r = must_exec(&tool, json!({"action": "seek"}));
        assert!(r.is_error);
        assert!(r.content.contains("position_ms"));

        let r = must_exec(&tool, json!({"action": "set_repeat", "state": "bogus"}));
        assert!(r.is_error);
        assert!(r.content.contains("track, context, off"));

        let r = must_exec(&tool, json!({"action": "set_volume"}));
        assert!(r.is_error);
        assert!(r.content.contains("volume_percent"));

        let r = must_exec(
            &tool,
            json!({"action": "recently_played", "after": 1000, "before": 2000}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("only one"));

        let r = must_exec(&tool, json!({"action": "wat"}));
        assert!(r.is_error);
        assert!(r.content.contains("Unknown spotify_playback action"));

        let dtool = SpotifyDevicesTool::new(backend.clone());
        let r = must_exec(&dtool, json!({"action": "transfer"}));
        assert!(r.is_error);
        assert!(r.content.contains("device_id"));

        let ltool = SpotifyLibraryTool::new(backend);
        let r = must_exec(&ltool, json!({"kind": "movies", "action": "list"}));
        assert!(r.is_error);
        assert!(r.content.contains("tracks, albums"));
    }

    // -----------------------------------------------------------------
    // Registry plumbing.
    // -----------------------------------------------------------------

    #[test]
    fn register_spotify_tools_populates_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        let backend: Arc<dyn SpotifyBackend> = Arc::new(NullSpotifyBackend);
        register_spotify_tools(&mut reg, backend);
        let defs = reg.to_tool_defs();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        for expected in [
            "spotify_playback",
            "spotify_devices",
            "spotify_queue",
            "spotify_search",
            "spotify_playlists",
            "spotify_albums",
            "spotify_library",
        ] {
            assert!(
                names.contains(&expected),
                "missing {expected} (found: {names:?})"
            );
        }
        let pb = defs.iter().find(|d| d.name == "spotify_playback").unwrap();
        let required: Vec<&str> = pb.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.contains(&"action"));
    }

    // -----------------------------------------------------------------
    // v0.9.0 Wave-1 B0 — Spotify hide-by-default (deferred to v0.9.1).
    // -----------------------------------------------------------------

    #[test]
    fn spotify_playback_default_is_unavailable() {
        let t = SpotifyPlaybackTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_devices_default_is_unavailable() {
        let t = SpotifyDevicesTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_queue_default_is_unavailable() {
        let t = SpotifyQueueTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_search_default_is_unavailable() {
        let t = SpotifySearchTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_playlists_default_is_unavailable() {
        let t = SpotifyPlaylistsTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_albums_default_is_unavailable() {
        let t = SpotifyAlbumsTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_library_default_is_unavailable() {
        let t = SpotifyLibraryTool::default();
        assert!(!t.is_available());
    }

    #[test]
    fn spotify_playback_with_real_backend_is_available() {
        // Proves the gate works both ways: when v0.9.1 wires a real
        // backend through `::new(...)`, the tool flips to available.
        let backend: Arc<dyn SpotifyBackend> = Arc::new(NullSpotifyBackend);
        let t = SpotifyPlaybackTool::new(backend);
        assert!(
            t.is_available(),
            "constructing via ::new with a backend should mark the tool available"
        );
    }
}
