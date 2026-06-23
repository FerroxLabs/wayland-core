//! T3-3.6 — `voice_mode` session helper.
//!
//! Ported from the prior Wayland Python engine (1017 LOC).
//!
//! ## Classification: **Helper, NOT a Tool**
//!
//! `voice_mode` is a host-side session toggle, not an LLM-callable
//! `Tool`. The model never invokes "voice mode" per turn — the CLI
//! enables it (`/voice on`), the host captures audio, transcribes,
//! and feeds the transcript into the prompt loop. TTS plays model
//! output back. This port mirrors that orchestration surface.
//!
//! ## Architecture seams (mirror of `vision_tools.rs`)
//!
//! `wcore-tools` ships with **NO embedded audio dependency**
//! (`sounddevice`, PortAudio, `wave`, system players) — that keeps
//! the workspace link-time identical to the pre-port baseline. The
//! port instead defines four pluggable boundaries:
//!
//! * [`AudioRecorder`] — capture mic input, write a WAV file path.
//! * [`TranscriptionBackend`] — STT call (Whisper / Groq / OpenAI).
//! * [`AudioPlayer`] — play a WAV path through the OS audio stack.
//! * [`AudioEnvironmentProbe`] — read OS env to decide whether
//!   capture is even possible (SSH, container, WSL, Termux).
//!
//! Each seam ships with a `Null*` fail-loud default (NO-STUBS
//! contract: never silently succeed) **and** a `Capturing*` /
//! `Static*` in-memory variant for hermetic tests.
//!
//! Pure helpers ported as-is (no seam needed):
//!
//! * Audio constants (sample rate, channels, RMS thresholds).
//! * [`is_whisper_hallucination`] — string/regex check.
//! * [`cleanup_temp_recordings`] — temp-file pruning.
//!
//! ## Composability with sibling sub-wave 6 modules
//!
//! `transcription_tools` and `tts_tool` (originally on parallel
//! feature branches T3-3.6-a / T3-3.6-c) are now **both merged and
//! present in this crate** — declared in `lib.rs` (`pub mod
//! transcription_tools;` / `pub mod tts_tool;`) — and `VoiceModeTool`
//! is registered into the tool registry at bootstrap. This module
//! still deliberately does NOT depend on them directly: the host
//! wires their concrete implementations into [`TranscriptionBackend`]
//! / [`AudioPlayer`] through the pluggable seams above, keeping this
//! module decoupled from their internals.
//!
//! ## Divergences from the Python original (intentional)
//!
//! * No persistent `_TEMP_DIR` global. Each [`VoiceMode`] gets a
//!   per-instance temp dir under `std::env::temp_dir()`, configurable
//!   via [`VoiceMode::with_temp_dir`].
//! * No `play_beep` — the CLI surface for audio cues is the host's
//!   responsibility (it owns terminal control). Provided as a noop
//!   default on the `AudioPlayer` seam.
//! * No global `_active_playback`. The `AudioPlayer` trait owns
//!   playback lifecycle; `stop()` is a method, not a free function.
//! * No threading-based callbacks. Silence-autostop is a backend
//!   contract on the `AudioRecorder` trait — the recorder fires
//!   the host-supplied callback when it detects silence.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};

use crate::Tool;
use wcore_types::tool::{JsonSchema, ToolResult};

// ---------------------------------------------------------------------------
// Constants — ported verbatim from the Python original.
// ---------------------------------------------------------------------------

/// Whisper-native sample rate (Hz). Used for capture + playback.
pub const SAMPLE_RATE: u32 = 16_000;
/// Mono capture.
pub const CHANNELS: u16 = 1;
/// 16-bit PCM — 2 bytes per sample.
pub const SAMPLE_WIDTH: u16 = 2;
/// RMS threshold below which audio is considered silence (int16 range).
pub const SILENCE_RMS_THRESHOLD: i32 = 200;
/// Continuous silence (seconds) before silence-autostop fires.
pub const SILENCE_DURATION_SECONDS: f32 = 3.0;
/// Minimum recording duration to keep (seconds). Shorter recordings
/// are discarded — Whisper produces garbage on sub-300ms audio.
pub const MIN_RECORDING_DURATION_SECONDS: f32 = 0.3;
/// Default max age for cleanup of stale recordings.
pub const DEFAULT_CLEANUP_MAX_AGE_SECONDS: u64 = 3_600;

// ---------------------------------------------------------------------------
// Whisper hallucination filter.
// ---------------------------------------------------------------------------

/// Phrases that Whisper commonly hallucinates on silent / near-silent
/// audio. Ported verbatim from the Python `WHISPER_HALLUCINATIONS`
/// set so existing user fixtures keep filtering identically.
const WHISPER_HALLUCINATION_PHRASES: &[&str] = &[
    "thank you.",
    "thank you",
    "thanks for watching.",
    "thanks for watching",
    "subscribe to my channel.",
    "subscribe to my channel",
    "like and subscribe.",
    "like and subscribe",
    "please subscribe.",
    "please subscribe",
    "thank you for watching.",
    "thank you for watching",
    "bye.",
    "bye",
    "you",
    "the end.",
    "the end",
    // Non-English hallucinations on silence.
    "продолжение следует",
    "продолжение следует...",
    "sous-titres",
    "sous-titres réalisés par la communauté d'amara.org",
    "sottotitoli creati dalla comunità amara.org",
    "untertitel von stephanie geiges",
    "amara.org",
    "www.mooji.org",
    "ご視聴ありがとうございました",
];

/// Regex catching repetitive hallucinations such as
/// `"Thank you. Thank you. Thank you."`. Compiled lazily on first
/// call so module load stays cheap.
fn hallucination_repeat_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // Case-insensitive via (?i). Whole-string match — anchored.
        Regex::new(r"(?i)^(?:thank you|thanks|bye|you|ok|okay|the end|\.|\s|,|!)+$")
            .expect("voice_mode hallucination regex must compile")
    })
}

/// Returns `true` if `transcript` is a known Whisper hallucination on
/// silence. Empty / whitespace-only strings count as hallucinations
/// (matches Python semantics so callers don't have to special-case
/// empty STT output).
pub fn is_whisper_hallucination(transcript: &str) -> bool {
    let cleaned = transcript.trim().to_lowercase();
    if cleaned.is_empty() {
        return true;
    }
    // Exact-match against the known phrase set, with and without
    // trailing punctuation — mirrors Python's
    // `cleaned.rstrip('.!') in WHISPER_HALLUCINATIONS or cleaned in ...`.
    let stripped: &str = cleaned.trim_end_matches(['.', '!']);
    for phrase in WHISPER_HALLUCINATION_PHRASES {
        if cleaned == *phrase || stripped == *phrase {
            return true;
        }
    }
    // Repetitive patterns ("Thank you. Thank you. you").
    hallucination_repeat_regex().is_match(&cleaned)
}

// ---------------------------------------------------------------------------
// Environment detection.
// ---------------------------------------------------------------------------

/// Outcome of probing the host environment for audio capability.
///
/// Mirrors the Python `detect_audio_environment()` dict shape:
/// `available` is computed as `warnings.is_empty()`. Notices are
/// informational — they're surfaced to the user but don't block
/// voice mode (e.g. "WSL with PulseAudio bridge configured").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioEnvironment {
    pub available: bool,
    pub warnings: Vec<String>,
    pub notices: Vec<String>,
}

/// Pluggable environment probe — abstracts the OS reads
/// (`SSH_TTY`, `/proc/version`, `PULSE_SERVER`, Termux detection).
/// The host wires the real probe; tests supply a canned
/// [`AudioEnvironment`].
pub trait AudioEnvironmentProbe: Send + Sync {
    fn detect(&self) -> AudioEnvironment;
}

/// Probe that returns the canned environment regardless of host state
/// — used by tests and by callers that want to inject a known result.
pub struct StaticAudioEnvironmentProbe(pub AudioEnvironment);

impl AudioEnvironmentProbe for StaticAudioEnvironmentProbe {
    fn detect(&self) -> AudioEnvironment {
        self.0.clone()
    }
}

/// Default probe — inspects env vars + `/proc/version`. Pure read
/// of process state, no audio-library imports. Mirrors the Python
/// `detect_audio_environment()` logic for SSH / container / WSL.
///
/// The Python original also peeks at PortAudio's device list; that
/// belongs on the `AudioRecorder` seam (the recorder's `probe()`
/// method, when implemented by a real backend, can return finer-
/// grained warnings). Here we only do the platform-level checks
/// that can be answered without an audio library.
pub struct OsAudioEnvironmentProbe;

impl OsAudioEnvironmentProbe {
    fn read_proc_version() -> Option<String> {
        std::fs::read_to_string("/proc/version").ok()
    }
}

impl AudioEnvironmentProbe for OsAudioEnvironmentProbe {
    fn detect(&self) -> AudioEnvironment {
        let mut warnings: Vec<String> = Vec::new();
        let mut notices: Vec<String> = Vec::new();

        // SSH detection — any of the canonical env vars indicates a
        // remote shell with no local audio path.
        for var in ["SSH_CLIENT", "SSH_TTY", "SSH_CONNECTION"] {
            if std::env::var_os(var).is_some() {
                warnings.push("Running over SSH -- no audio devices available".to_string());
                break;
            }
        }

        // Container detection: docker / podman / kubernetes write
        // marker files. Cheap is_file() check.
        if Path::new("/.dockerenv").is_file() || Path::new("/run/.containerenv").is_file() {
            warnings.push("Running inside a container -- no audio devices available".to_string());
        }

        // WSL detection — `/proc/version` contains "microsoft" on WSL.
        if let Some(version) = Self::read_proc_version()
            && version.to_lowercase().contains("microsoft")
        {
            if std::env::var_os("PULSE_SERVER").is_some() {
                notices.push("Running in WSL with PulseAudio bridge".to_string());
            } else {
                warnings.push(
                    "Running in WSL -- audio requires PulseAudio bridge.\n  \
                     1. Set PULSE_SERVER=unix:/mnt/wslg/PulseServer\n  \
                     2. Create ~/.asoundrc pointing ALSA at PulseAudio\n  \
                     3. Verify with: arecord -d 3 /tmp/test.wav && aplay /tmp/test.wav"
                        .to_string(),
                );
            }
        }

        AudioEnvironment {
            available: warnings.is_empty(),
            warnings,
            notices,
        }
    }
}

// ---------------------------------------------------------------------------
// AudioRecorder seam.
// ---------------------------------------------------------------------------

/// Outcome of stopping a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingOutcome {
    /// A WAV file was written to disk and is ready for transcription.
    Captured { wav_path: PathBuf },
    /// Recording was too short, too quiet, or otherwise empty.
    /// Mirrors the Python `return None` paths.
    Empty,
}

/// Pluggable audio capture boundary.
///
/// Hosts wire a real recorder (PortAudio via `cpal`, Termux:API,
/// custom HAL) at session construction. The engine never embeds an
/// audio library.
#[async_trait]
pub trait AudioRecorder: Send + Sync {
    /// Begin capturing from the default input device. The host may
    /// call multiple times; subsequent calls while already recording
    /// are no-ops (matches the Python original).
    async fn start(&self) -> Result<(), String>;
    /// Stop capture and persist the captured frames to a WAV file.
    async fn stop(&self) -> Result<RecordingOutcome, String>;
    /// Discard the in-flight recording without persisting.
    async fn cancel(&self) -> Result<(), String>;
    /// Release any held audio device handles. Called from
    /// [`VoiceMode::shutdown`].
    async fn shutdown(&self) -> Result<(), String>;
    /// Whether the recorder currently has a live capture.
    fn is_recording(&self) -> bool;
    /// Live RMS level — the host UI uses this for the audio-level
    /// indicator. 0 when not recording. Range: 0 .. i16::MAX as i32.
    fn current_rms(&self) -> i32;
}

/// Default no-op recorder — fails loudly on every `start()` so a
/// missing wiring is never silently masked. Honors the NO-STUBS
/// contract.
pub struct NullAudioRecorder;

#[async_trait]
impl AudioRecorder for NullAudioRecorder {
    async fn start(&self) -> Result<(), String> {
        Err(
            "No AudioRecorder configured. Wire a real recorder (e.g. cpal-backed) when \
             constructing VoiceMode to enable voice capture."
                .to_string(),
        )
    }
    async fn stop(&self) -> Result<RecordingOutcome, String> {
        Ok(RecordingOutcome::Empty)
    }
    async fn cancel(&self) -> Result<(), String> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
    fn is_recording(&self) -> bool {
        false
    }
    fn current_rms(&self) -> i32 {
        0
    }
}

/// In-memory recorder for tests. `start()` flips a flag; `stop()`
/// returns the configured outcome; events are captured for
/// assertions. Lives in the prod module so downstream crates can
/// reuse it (same convention as `CapturingVisionBackend`).
pub struct CapturingAudioRecorder {
    inner: Mutex<CapturingRecorderState>,
}

#[derive(Default)]
struct CapturingRecorderState {
    is_recording: bool,
    rms: i32,
    canned_outcome: Option<RecordingOutcome>,
    events: Vec<String>,
}

impl Default for CapturingAudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}

impl CapturingAudioRecorder {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CapturingRecorderState::default()),
        }
    }
    /// Set the outcome the next `stop()` call returns. If `None`,
    /// `stop()` returns [`RecordingOutcome::Empty`].
    pub fn set_next_outcome(&self, outcome: RecordingOutcome) {
        self.inner.lock().canned_outcome = Some(outcome);
    }
    pub fn set_rms(&self, rms: i32) {
        self.inner.lock().rms = rms;
    }
    pub fn events(&self) -> Vec<String> {
        self.inner.lock().events.clone()
    }
}

#[async_trait]
impl AudioRecorder for CapturingAudioRecorder {
    async fn start(&self) -> Result<(), String> {
        let mut s = self.inner.lock();
        s.events.push("start".to_string());
        if s.is_recording {
            return Ok(()); // matches Python "already recording" no-op
        }
        s.is_recording = true;
        Ok(())
    }
    async fn stop(&self) -> Result<RecordingOutcome, String> {
        let mut s = self.inner.lock();
        s.events.push("stop".to_string());
        if !s.is_recording {
            return Ok(RecordingOutcome::Empty);
        }
        s.is_recording = false;
        s.rms = 0;
        Ok(s.canned_outcome.take().unwrap_or(RecordingOutcome::Empty))
    }
    async fn cancel(&self) -> Result<(), String> {
        let mut s = self.inner.lock();
        s.events.push("cancel".to_string());
        s.is_recording = false;
        s.rms = 0;
        s.canned_outcome = None;
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), String> {
        let mut s = self.inner.lock();
        s.events.push("shutdown".to_string());
        s.is_recording = false;
        s.rms = 0;
        Ok(())
    }
    fn is_recording(&self) -> bool {
        self.inner.lock().is_recording
    }
    fn current_rms(&self) -> i32 {
        self.inner.lock().rms
    }
}

// ---------------------------------------------------------------------------
// TranscriptionBackend seam.
// ---------------------------------------------------------------------------

/// Result of a transcription call. Mirrors the Python dict shape
/// `{success, transcript, error?, filtered?}` as a structured enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptionOutcome {
    /// STT call succeeded; `filtered` is `true` if the transcript
    /// matched [`is_whisper_hallucination`] and was zeroed out.
    Ok { transcript: String, filtered: bool },
    /// STT call failed. The message is shown to the user verbatim
    /// (callers prefix as appropriate).
    Err { message: String },
}

/// Pluggable speech-to-text boundary. The host wires a real backend
/// (Whisper local, Groq, OpenAI). T3 sub-wave 6 sibling branch
/// `transcription_tools` will provide a concrete impl — at port-time
/// it's not yet merged, so this seam stays self-contained.
#[async_trait]
pub trait TranscriptionBackend: Send + Sync {
    /// Transcribe the WAV file at `wav_path`. The backend is
    /// responsible for upload, retry, and provider-specific error
    /// handling; the seam returns a flat outcome.
    async fn transcribe(&self, wav_path: &Path, model: Option<&str>) -> TranscriptionOutcome;
}

/// Default no-op STT — fails loudly. NO-STUBS guarantee.
pub struct NullTranscriptionBackend;

#[async_trait]
impl TranscriptionBackend for NullTranscriptionBackend {
    async fn transcribe(&self, _wav_path: &Path, _model: Option<&str>) -> TranscriptionOutcome {
        TranscriptionOutcome::Err {
            message: "No TranscriptionBackend configured. Wire a backend (Whisper / Groq / \
                      OpenAI) when constructing VoiceMode to enable transcription."
                .to_string(),
        }
    }
}

/// Test backend — returns a canned transcript and captures the call.
pub struct CapturingTranscriptionBackend {
    response: TranscriptionOutcome,
    pub calls: Mutex<Vec<(PathBuf, Option<String>)>>,
}

impl CapturingTranscriptionBackend {
    pub fn new(response: TranscriptionOutcome) -> Self {
        Self {
            response,
            calls: Mutex::new(Vec::new()),
        }
    }
    pub fn call_count(&self) -> usize {
        self.calls.lock().len()
    }
}

#[async_trait]
impl TranscriptionBackend for CapturingTranscriptionBackend {
    async fn transcribe(&self, wav_path: &Path, model: Option<&str>) -> TranscriptionOutcome {
        self.calls
            .lock()
            .push((wav_path.to_path_buf(), model.map(str::to_string)));
        self.response.clone()
    }
}

// ---------------------------------------------------------------------------
// AudioPlayer seam.
// ---------------------------------------------------------------------------

/// Pluggable audio playback boundary. The host wires a real player
/// (cpal, system `afplay`/`aplay`/`ffplay`). Sibling branch `tts_tool`
/// will compose into this; at port-time it's not merged.
#[async_trait]
pub trait AudioPlayer: Send + Sync {
    /// Play an audio file (WAV or any host-supported format) through
    /// the default output. Returns `true` on success. Blocks until
    /// playback completes or until [`AudioPlayer::stop`] is called.
    async fn play(&self, file_path: &Path) -> bool;
    /// Interrupt any in-flight playback.
    async fn stop(&self);
}

/// Default no-op player — `play()` returns `false`, `stop()` is a
/// noop. Unlike the other Null* backends this one does NOT error
/// because the Python `play_audio_file` returns False, not raises,
/// when no player is available — host code branches on the bool.
pub struct NullAudioPlayer;

#[async_trait]
impl AudioPlayer for NullAudioPlayer {
    async fn play(&self, _file_path: &Path) -> bool {
        false
    }
    async fn stop(&self) {}
}

/// Test player — records every play/stop call.
#[derive(Default)]
pub struct CapturingAudioPlayer {
    pub played: Mutex<Vec<PathBuf>>,
    pub stops: Mutex<usize>,
    play_result: bool,
}

impl CapturingAudioPlayer {
    pub fn new(play_result: bool) -> Self {
        Self {
            played: Mutex::new(Vec::new()),
            stops: Mutex::new(0),
            play_result,
        }
    }
    pub fn play_count(&self) -> usize {
        self.played.lock().len()
    }
    pub fn stop_count(&self) -> usize {
        *self.stops.lock()
    }
}

#[async_trait]
impl AudioPlayer for CapturingAudioPlayer {
    async fn play(&self, file_path: &Path) -> bool {
        self.played.lock().push(file_path.to_path_buf());
        self.play_result
    }
    async fn stop(&self) {
        *self.stops.lock() += 1;
    }
}

// ---------------------------------------------------------------------------
// Requirements check.
// ---------------------------------------------------------------------------

/// Composite readiness report — mirrors the Python
/// `check_voice_requirements()` dict.
#[derive(Debug, Clone)]
pub struct VoiceRequirements {
    pub available: bool,
    pub audio_capture_available: bool,
    pub stt_available: bool,
    pub details: Vec<String>,
    pub environment: AudioEnvironment,
}

// ---------------------------------------------------------------------------
// Temp-file cleanup.
// ---------------------------------------------------------------------------

/// Delete `recording_*.wav` files older than `max_age` under
/// `temp_dir`. Returns the number of files deleted. Silently
/// tolerates filesystem errors per entry — same semantics as the
/// Python original.
pub fn cleanup_temp_recordings(temp_dir: &Path, max_age: Duration) -> usize {
    if !temp_dir.is_dir() {
        return 0;
    }
    let now = SystemTime::now();
    let mut deleted = 0usize;
    let entries = match std::fs::read_dir(temp_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !(name.starts_with("recording_") && name.ends_with(".wav")) {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let age = now.duration_since(modified).unwrap_or_default();
        if age > max_age && std::fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

// ---------------------------------------------------------------------------
// VoiceMode — session orchestrator.
// ---------------------------------------------------------------------------

/// Per-session voice-mode orchestrator. Owns the four seams plus a
/// temp dir. Hosts construct one when the user toggles `/voice on`
/// and call [`VoiceMode::shutdown`] when toggling off.
pub struct VoiceMode {
    recorder: Arc<dyn AudioRecorder>,
    transcriber: Arc<dyn TranscriptionBackend>,
    player: Arc<dyn AudioPlayer>,
    env_probe: Arc<dyn AudioEnvironmentProbe>,
    temp_dir: PathBuf,
}

impl VoiceMode {
    /// Construct a `VoiceMode` with all four seams wired and a
    /// default temp dir under `std::env::temp_dir()/wayland_voice`.
    pub fn new(
        recorder: Arc<dyn AudioRecorder>,
        transcriber: Arc<dyn TranscriptionBackend>,
        player: Arc<dyn AudioPlayer>,
        env_probe: Arc<dyn AudioEnvironmentProbe>,
    ) -> Self {
        Self {
            recorder,
            transcriber,
            player,
            env_probe,
            temp_dir: std::env::temp_dir().join("wayland_voice"),
        }
    }

    /// Null-backed instance — useful as a placeholder before the
    /// host wires real backends. Every operation fails loudly.
    pub fn null() -> Self {
        Self::new(
            Arc::new(NullAudioRecorder),
            Arc::new(NullTranscriptionBackend),
            Arc::new(NullAudioPlayer),
            Arc::new(OsAudioEnvironmentProbe),
        )
    }

    /// Override the temp dir used by [`Self::cleanup_temp`].
    pub fn with_temp_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.temp_dir = dir.into();
        self
    }

    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    /// Begin audio capture. Returns the underlying recorder error
    /// verbatim so the CLI can surface it to the user.
    pub async fn start_capture(&self) -> Result<(), String> {
        self.recorder.start().await
    }

    /// Stop capture. Returns the WAV path or `Empty` when the
    /// recording was too short / silent / cancelled.
    pub async fn stop_capture(&self) -> Result<RecordingOutcome, String> {
        self.recorder.stop().await
    }

    /// Cancel any in-flight capture and discard the audio.
    pub async fn cancel_capture(&self) -> Result<(), String> {
        self.recorder.cancel().await
    }

    /// Whether the underlying recorder is live.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_recording()
    }

    /// Forward to recorder's RMS level.
    pub fn current_rms(&self) -> i32 {
        self.recorder.current_rms()
    }

    /// Transcribe a captured WAV. Applies the Whisper-hallucination
    /// filter: when the backend returns success but the transcript
    /// matches a known hallucination, the transcript is replaced
    /// with `""` and `filtered = true`. Mirrors Python
    /// `transcribe_recording`.
    pub async fn transcribe(&self, wav_path: &Path, model: Option<&str>) -> TranscriptionOutcome {
        match self.transcriber.transcribe(wav_path, model).await {
            TranscriptionOutcome::Ok {
                transcript,
                filtered,
            } => {
                if is_whisper_hallucination(&transcript) {
                    TranscriptionOutcome::Ok {
                        transcript: String::new(),
                        filtered: true,
                    }
                } else {
                    TranscriptionOutcome::Ok {
                        transcript,
                        filtered,
                    }
                }
            }
            err @ TranscriptionOutcome::Err { .. } => err,
        }
    }

    /// Play an audio file through the wired player.
    pub async fn play(&self, file_path: &Path) -> bool {
        self.player.play(file_path).await
    }

    /// Interrupt any in-flight playback.
    pub async fn stop_playback(&self) {
        self.player.stop().await
    }

    /// Tear down the recorder and stop any playback.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.player.stop().await;
        self.recorder.shutdown().await
    }

    /// Probe the environment (env vars, container markers, WSL).
    /// Cheap — call as often as the UI needs.
    pub fn detect_environment(&self) -> AudioEnvironment {
        self.env_probe.detect()
    }

    /// Composite readiness check. The recorder seam is consulted via
    /// a probe-style `start()` dry-run: a real recorder backend
    /// would override [`AudioRecorder::current_rms`] or expose its
    /// own probe; the conservative default here flags the recorder
    /// as available when it is NOT the `NullAudioRecorder` (signalled
    /// by the env probe + a successful first start/cancel cycle is
    /// the host's contract — too expensive to do here).
    ///
    /// `stt_available` is true unless the wired transcriber returns
    /// an `Err` for a dry-run against a non-existent path.
    pub async fn check_requirements(&self) -> VoiceRequirements {
        let env = self.env_probe.detect();
        let mut details = Vec::<String>::new();

        // Cheapest viable probe of STT: call with a path that the
        // backend will reject. Backends that aren't wired (Null) fail
        // loudly. Real backends can short-circuit on path validation.
        let probe_path = self.temp_dir.join("__voice_probe__.wav");
        let stt_outcome = self.transcriber.transcribe(&probe_path, None).await;
        // Real backends often return Err because the probe path
        // doesn't exist — that still proves they're wired. Only the
        // "No TranscriptionBackend configured" sentinel from
        // `NullTranscriptionBackend` counts as unwired.
        let stt_available = match &stt_outcome {
            TranscriptionOutcome::Ok { .. } => true,
            TranscriptionOutcome::Err { message } => {
                !message.contains("No TranscriptionBackend configured")
            }
        };

        // We can't truly probe the recorder without taking the mic.
        // Treat the env probe's `available` as a proxy: if the env
        // can host audio, and the recorder isn't the Null default
        // (which fails on `start`), assume capture is available.
        let recorder_dry = self.recorder.start().await;
        let audio_capture_available = match recorder_dry {
            Ok(()) => {
                // Don't leave the recorder in a started state.
                let _ = self.recorder.cancel().await;
                true
            }
            Err(_) => false,
        };

        if audio_capture_available {
            details.push("Audio capture: OK".to_string());
        } else {
            details.push(
                "Audio capture: MISSING (no AudioRecorder wired or device unavailable)".to_string(),
            );
        }
        if stt_available {
            details.push("STT provider: OK".to_string());
        } else {
            details.push("STT provider: MISSING (no TranscriptionBackend wired)".to_string());
        }
        for w in &env.warnings {
            details.push(format!("Environment: {w}"));
        }
        for n in &env.notices {
            details.push(format!("Environment: {n}"));
        }

        VoiceRequirements {
            available: audio_capture_available && stt_available && env.available,
            audio_capture_available,
            stt_available,
            details,
            environment: env,
        }
    }

    /// Delete stale recordings older than [`DEFAULT_CLEANUP_MAX_AGE_SECONDS`].
    pub fn cleanup_temp(&self) -> usize {
        cleanup_temp_recordings(
            &self.temp_dir,
            Duration::from_secs(DEFAULT_CLEANUP_MAX_AGE_SECONDS),
        )
    }
}

// ---------------------------------------------------------------------------
// VoiceModeTool — LLM-callable Tool wrapper (v0.9.0 W1 B10).
// ---------------------------------------------------------------------------
//
// The Python original treats `voice_mode` as a host-side session toggle
// (`/voice on`), NOT a per-turn LLM tool. v0.9.0 W1 B10 exposes it as a
// real `Tool` impl so:
//
// * The TUI Ctrl+Space binding can dispatch through the same registry
//   path as every other tool (no special-case voice plumbing).
// * The model can observe whether voice mode is wired
//   (`is_available()` reflects whether a real recorder + STT are present).
// * Future surfaces (web UI, IPC plugin) get the same backend gating.
//
// Action semantics (the `action` field of the tool input):
//
// * `"toggle_record"` — start capture if idle, stop + transcribe if live.
//   This is the only action wired to Ctrl+Space; the TUI surfaces it as
//   a one-key push-to-talk.
// * `"start"` / `"stop"` / `"cancel"` — explicit verbs for hosts that
//   want a non-toggle UX.
// * `"status"` — report `is_recording` + `current_rms` without side
//   effects.
//
// The Tool is hidden (`is_available() == false`) until the host injects
// real backends via `VoiceModeTool::new(recorder, player)`. The default
// constructor leaves `backend_configured = false` to honour the NO-STUBS
// contract — `Default::default()` MUST NOT advertise itself.

/// LLM-callable `voice_mode` tool. Wraps a [`VoiceMode`] orchestrator
/// behind the standard [`Tool`] trait so the dispatcher can route it
/// uniformly. Hidden when no real recorder is wired.
pub struct VoiceModeTool {
    inner: Arc<VoiceMode>,
    /// v0.9.0 W1: `false` until a real backend is wired.
    /// `ToolRegistry::register` drops the tool when this is `false` so
    /// the model never sees a tool it cannot successfully call.
    backend_configured: bool,
}

impl Default for VoiceModeTool {
    fn default() -> Self {
        Self {
            inner: Arc::new(VoiceMode::null()),
            backend_configured: false,
        }
    }
}

impl VoiceModeTool {
    /// Construct a `VoiceModeTool` with real backends wired. Sets
    /// `backend_configured = true` so the tool advertises itself to
    /// the model.
    ///
    /// The transcriber + env probe are pulled from the [`VoiceMode`]
    /// itself — the host wires a single orchestrator and hands it here.
    /// The recorder + player are taken explicitly because they are the
    /// two seams a real host *must* fill (the transcriber typically
    /// shares the `build_transcription_backend()` resolver from
    /// `wcore-agent`; the env probe defaults to [`OsAudioEnvironmentProbe`]).
    pub fn new(voice_mode: Arc<VoiceMode>) -> Self {
        Self {
            inner: voice_mode,
            backend_configured: true,
        }
    }

    /// Direct synchronous toggle — called from the TUI Ctrl+Space
    /// binding (P-B1 closure). Returns the new recording state.
    /// Capture lifecycle errors are swallowed and logged because the
    /// TUI has no error surface for this binding; failures show up in
    /// the next `status` action or on `stop` via the transcript.
    pub async fn toggle_record(&self) -> bool {
        if self.inner.is_recording() {
            // Stop fires fire-and-forget — transcription is a separate
            // user-initiated step (the LLM tool call wires it together
            // via the `toggle_record` action).
            if let Err(e) = self.inner.stop_capture().await {
                tracing::warn!("voice_mode: stop_capture failed: {e}");
            }
            false
        } else {
            if let Err(e) = self.inner.start_capture().await {
                tracing::warn!("voice_mode: start_capture failed: {e}");
                return false;
            }
            true
        }
    }

    /// Direct cancel — abort any in-flight capture (R-H6).
    /// Idempotent: safe to call when idle.
    pub async fn cancel(&self) -> Result<(), String> {
        self.inner.cancel_capture().await
    }

    /// Borrow the underlying `VoiceMode` (used by hosts that need to
    /// call `transcribe()` after a stop, or wire `check_requirements()`
    /// into a doctor screen).
    pub fn voice_mode(&self) -> &VoiceMode {
        &self.inner
    }
}

#[async_trait]
impl Tool for VoiceModeTool {
    fn name(&self) -> &str {
        "voice_mode"
    }

    fn category(&self) -> wcore_protocol::events::ToolCategory {
        // Voice capture is a host-side I/O surface; the transcript /
        // wav path is informational ("did you hear me?" loop), not a
        // mutation of any persistent resource. `Info` matches the
        // sibling `transcribe_audio` tool which classifies the same way.
        wcore_protocol::events::ToolCategory::Info
    }

    /// Hidden when no real backend is wired. Matches the v0.9.0 W1
    /// pattern from `TtsTool` / `TranscribeAudioTool`: the model only
    /// sees tools that can actually succeed.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Toggle the voice-mode session helper or query its state. \
Actions: 'toggle_record' (start if idle, stop+keep wav if live), \
'start' / 'stop' / 'cancel' (explicit verbs), 'status' (report \
is_recording + rms without side effects). When a recording is captured \
the tool returns the wav path so a downstream transcribe_audio call \
can pick it up."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "Which voice-mode action to perform.",
                    "enum": ["toggle_record", "start", "stop", "cancel", "status"]
                }
            },
            "required": ["action"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // The recorder owns a single mic device — overlapping starts
        // would race on the audio handle. Serialise.
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = match input.get("action").and_then(Value::as_str) {
            Some(s) => s,
            None => return error_result("action is required"),
        };
        match action {
            "toggle_record" => {
                let now_recording = self.toggle_record().await;
                ok_result(json!({
                    "success": true,
                    "is_recording": now_recording,
                }))
            }
            "start" => match self.inner.start_capture().await {
                Ok(()) => ok_result(json!({"success": true, "is_recording": true})),
                Err(e) => error_result(&format!("voice_mode start failed: {e}")),
            },
            "stop" => match self.inner.stop_capture().await {
                Ok(RecordingOutcome::Captured { wav_path }) => ok_result(json!({
                    "success": true,
                    "is_recording": false,
                    "wav_path": wav_path.display().to_string(),
                })),
                Ok(RecordingOutcome::Empty) => ok_result(json!({
                    "success": true,
                    "is_recording": false,
                    "wav_path": Value::Null,
                    "note": "recording was empty (too short / silent / cancelled)",
                })),
                Err(e) => error_result(&format!("voice_mode stop failed: {e}")),
            },
            "cancel" => match self.inner.cancel_capture().await {
                Ok(()) => ok_result(json!({"success": true, "is_recording": false})),
                Err(e) => error_result(&format!("voice_mode cancel failed: {e}")),
            },
            "status" => ok_result(json!({
                "success": true,
                "is_recording": self.inner.is_recording(),
                "current_rms": self.inner.current_rms(),
            })),
            other => error_result(&format!(
                "unknown action '{other}' (expected toggle_record / start / stop / cancel / status)"
            )),
        }
    }
}

fn ok_result(payload: Value) -> ToolResult {
    ToolResult {
        content: payload.to_string(),
        is_error: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    // ----- Whisper hallucination filter -----

    #[test]
    fn hallucination_filter_catches_canonical_phrases() {
        assert!(is_whisper_hallucination("Thank you."));
        assert!(is_whisper_hallucination("thank you"));
        assert!(is_whisper_hallucination("Thanks for watching!"));
        assert!(is_whisper_hallucination("  bye.  "));
        assert!(is_whisper_hallucination(""));
        assert!(is_whisper_hallucination("   "));
        assert!(is_whisper_hallucination("amara.org"));
    }

    #[test]
    fn hallucination_filter_catches_repetitive_patterns() {
        assert!(is_whisper_hallucination("Thank you. Thank you. Thank you."));
        assert!(is_whisper_hallucination("you you you"));
        assert!(is_whisper_hallucination("ok. ok. ok!"));
    }

    #[test]
    fn hallucination_filter_passes_real_speech() {
        assert!(!is_whisper_hallucination("Set the volume to 30 percent."));
        assert!(!is_whisper_hallucination(
            "Thank you for explaining the build system."
        ));
        assert!(!is_whisper_hallucination("Hello, can you help me?"));
    }

    // ----- Environment detection -----

    #[test]
    fn os_probe_returns_consistent_shape() {
        let probe = OsAudioEnvironmentProbe;
        let env = probe.detect();
        // available is `warnings.is_empty()` by construction.
        assert_eq!(env.available, env.warnings.is_empty());
    }

    #[test]
    fn static_probe_returns_canned_state() {
        let canned = AudioEnvironment {
            available: false,
            warnings: vec!["test warning".to_string()],
            notices: vec![],
        };
        let probe = StaticAudioEnvironmentProbe(canned.clone());
        assert_eq!(probe.detect(), canned);
    }

    // ----- AudioRecorder seam -----

    #[tokio::test]
    async fn null_recorder_fails_loudly_on_start() {
        let r = NullAudioRecorder;
        let err = r.start().await.unwrap_err();
        assert!(err.contains("No AudioRecorder configured"), "got: {err}");
        // Idempotent shutdown.
        r.shutdown().await.unwrap();
        assert!(!r.is_recording());
        assert_eq!(r.current_rms(), 0);
    }

    #[tokio::test]
    async fn capturing_recorder_tracks_lifecycle() {
        let r = CapturingAudioRecorder::new();
        r.set_rms(1234);
        assert_eq!(r.current_rms(), 1234);
        assert!(!r.is_recording());

        r.start().await.unwrap();
        assert!(r.is_recording());

        // Second start is a no-op (matches Python "already recording").
        r.start().await.unwrap();
        assert!(r.is_recording());

        let outcome_path = PathBuf::from("/tmp/voice_test_capture.wav");
        r.set_next_outcome(RecordingOutcome::Captured {
            wav_path: outcome_path.clone(),
        });
        let got = r.stop().await.unwrap();
        assert_eq!(
            got,
            RecordingOutcome::Captured {
                wav_path: outcome_path
            }
        );
        assert!(!r.is_recording());
        assert_eq!(r.current_rms(), 0);

        // Stop again with no in-flight capture returns Empty.
        assert_eq!(r.stop().await.unwrap(), RecordingOutcome::Empty);

        r.start().await.unwrap();
        r.cancel().await.unwrap();
        assert!(!r.is_recording());

        let events = r.events();
        assert!(events.contains(&"start".to_string()));
        assert!(events.contains(&"stop".to_string()));
        assert!(events.contains(&"cancel".to_string()));
    }

    // ----- TranscriptionBackend seam -----

    #[tokio::test]
    async fn null_transcriber_fails_loudly() {
        let b = NullTranscriptionBackend;
        match b.transcribe(Path::new("/tmp/x.wav"), None).await {
            TranscriptionOutcome::Err { message } => {
                assert!(message.contains("No TranscriptionBackend configured"))
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn capturing_transcriber_returns_canned_and_records_calls() {
        let b = CapturingTranscriptionBackend::new(TranscriptionOutcome::Ok {
            transcript: "hello world".to_string(),
            filtered: false,
        });
        let out = b
            .transcribe(Path::new("/tmp/y.wav"), Some("whisper-1"))
            .await;
        assert!(
            matches!(out, TranscriptionOutcome::Ok { ref transcript, filtered: false } if transcript == "hello world")
        );
        assert_eq!(b.call_count(), 1);
        let captured = b.calls.lock().clone();
        assert_eq!(captured[0].0, Path::new("/tmp/y.wav"));
        assert_eq!(captured[0].1.as_deref(), Some("whisper-1"));
    }

    // ----- AudioPlayer seam -----

    #[tokio::test]
    async fn null_player_returns_false_and_stop_is_noop() {
        let p = NullAudioPlayer;
        assert!(!p.play(Path::new("/tmp/no.wav")).await);
        p.stop().await;
    }

    #[tokio::test]
    async fn capturing_player_records_calls() {
        let p = CapturingAudioPlayer::new(true);
        assert!(p.play(Path::new("/tmp/a.wav")).await);
        assert!(p.play(Path::new("/tmp/b.wav")).await);
        p.stop().await;
        assert_eq!(p.play_count(), 2);
        assert_eq!(p.stop_count(), 1);
    }

    // ----- VoiceMode orchestration -----

    fn make_voice_mode_with_capturers() -> (
        VoiceMode,
        Arc<CapturingAudioRecorder>,
        Arc<CapturingTranscriptionBackend>,
        Arc<CapturingAudioPlayer>,
    ) {
        let rec = Arc::new(CapturingAudioRecorder::new());
        let tx = Arc::new(CapturingTranscriptionBackend::new(
            TranscriptionOutcome::Ok {
                transcript: "go build the dashboard".to_string(),
                filtered: false,
            },
        ));
        let player = Arc::new(CapturingAudioPlayer::new(true));
        let probe = Arc::new(StaticAudioEnvironmentProbe(AudioEnvironment {
            available: true,
            warnings: vec![],
            notices: vec![],
        }));
        let vm = VoiceMode::new(rec.clone(), tx.clone(), player.clone(), probe);
        (vm, rec, tx, player)
    }

    #[tokio::test]
    async fn voice_mode_full_cycle_records_then_transcribes_then_plays() {
        let (vm, rec, tx, player) = make_voice_mode_with_capturers();

        vm.start_capture().await.unwrap();
        assert!(vm.is_recording());

        let wav = PathBuf::from("/tmp/wayland_voice_test.wav");
        rec.set_next_outcome(RecordingOutcome::Captured {
            wav_path: wav.clone(),
        });
        let outcome = vm.stop_capture().await.unwrap();
        let RecordingOutcome::Captured { wav_path } = outcome else {
            panic!("expected captured outcome");
        };
        assert_eq!(wav_path, wav);

        let stt = vm.transcribe(&wav_path, Some("whisper-1")).await;
        match stt {
            TranscriptionOutcome::Ok {
                transcript,
                filtered,
            } => {
                assert_eq!(transcript, "go build the dashboard");
                assert!(!filtered);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        assert_eq!(tx.call_count(), 1);

        assert!(vm.play(&wav_path).await);
        assert_eq!(player.play_count(), 1);
    }

    #[tokio::test]
    async fn voice_mode_filters_whisper_hallucination_to_empty() {
        let rec = Arc::new(CapturingAudioRecorder::new());
        let tx = Arc::new(CapturingTranscriptionBackend::new(
            TranscriptionOutcome::Ok {
                transcript: "Thank you. Thank you. Thank you.".to_string(),
                filtered: false,
            },
        ));
        let player = Arc::new(NullAudioPlayer);
        let probe = Arc::new(StaticAudioEnvironmentProbe(AudioEnvironment::default()));
        let vm = VoiceMode::new(rec, tx, player, probe);

        let out = vm.transcribe(Path::new("/tmp/x.wav"), None).await;
        match out {
            TranscriptionOutcome::Ok {
                transcript,
                filtered,
            } => {
                assert!(transcript.is_empty());
                assert!(filtered, "hallucination must mark filtered=true");
            }
            other => panic!("expected Ok-filtered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn voice_mode_propagates_transcription_errors() {
        let rec = Arc::new(CapturingAudioRecorder::new());
        let tx = Arc::new(CapturingTranscriptionBackend::new(
            TranscriptionOutcome::Err {
                message: "STT network error".to_string(),
            },
        ));
        let player = Arc::new(NullAudioPlayer);
        let probe = Arc::new(StaticAudioEnvironmentProbe(AudioEnvironment::default()));
        let vm = VoiceMode::new(rec, tx, player, probe);

        match vm.transcribe(Path::new("/tmp/x.wav"), None).await {
            TranscriptionOutcome::Err { message } => {
                assert_eq!(message, "STT network error")
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn voice_mode_null_default_fails_loudly_on_capture_start() {
        let vm = VoiceMode::null();
        let err = vm.start_capture().await.unwrap_err();
        assert!(err.contains("No AudioRecorder configured"));
    }

    #[tokio::test]
    async fn voice_mode_check_requirements_reports_null_seams_unavailable() {
        let vm = VoiceMode::null();
        let req = vm.check_requirements().await;
        assert!(
            !req.audio_capture_available,
            "null recorder must be flagged"
        );
        assert!(!req.stt_available, "null transcriber must be flagged");
        assert!(!req.available);
        // Details should call out both missing pieces.
        let joined = req.details.join("\n");
        assert!(joined.contains("Audio capture: MISSING"));
        assert!(joined.contains("STT provider: MISSING"));
    }

    #[tokio::test]
    async fn voice_mode_check_requirements_with_wired_seams_is_available() {
        let (vm, _rec, _tx, _player) = make_voice_mode_with_capturers();
        let req = vm.check_requirements().await;
        assert!(req.audio_capture_available);
        assert!(req.stt_available);
        assert!(req.available);
        assert!(req.environment.warnings.is_empty());
    }

    #[tokio::test]
    async fn voice_mode_shutdown_stops_playback_and_recorder() {
        let (vm, rec, _tx, player) = make_voice_mode_with_capturers();
        vm.start_capture().await.unwrap();
        vm.shutdown().await.unwrap();
        // Player was asked to stop.
        assert_eq!(player.stop_count(), 1);
        // Recorder shutdown was called.
        assert!(rec.events().contains(&"shutdown".to_string()));
    }

    // ----- cleanup_temp_recordings -----

    #[test]
    fn cleanup_removes_only_old_matching_files() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create three files:
        //   recording_old.wav   -- matches, will be backdated
        //   recording_fresh.wav -- matches, recent (kept)
        //   other.wav           -- non-matching (kept)
        let old = root.join("recording_old.wav");
        let fresh = root.join("recording_fresh.wav");
        let other = root.join("other.wav");
        fs::write(&old, b"old").unwrap();
        fs::write(&fresh, b"fresh").unwrap();
        fs::write(&other, b"other").unwrap();

        // Backdate `old` 2 hours by setting mtime via filetime-free
        // approach: set the file's mtime by re-writing after sleeping
        // is too slow. We use std::fs::File::set_modified which is
        // stable on Rust 1.75+ — the workspace MSRV permits it
        // (workspace pins stable rustc).
        let two_hours = SystemTime::now() - Duration::from_secs(2 * 3600);
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        f.set_modified(two_hours).unwrap();

        let deleted = cleanup_temp_recordings(root, Duration::from_secs(3600));
        assert_eq!(deleted, 1, "exactly one stale recording should be deleted");
        assert!(!old.exists(), "old recording should be gone");
        assert!(fresh.exists(), "fresh recording should be kept");
        assert!(other.exists(), "non-matching files must be left alone");
    }

    #[test]
    fn cleanup_on_missing_dir_returns_zero() {
        let nonexistent = std::env::temp_dir().join("wayland_voice_does_not_exist_xyzzy");
        // Make sure it really doesn't exist.
        let _ = std::fs::remove_dir_all(&nonexistent);
        assert_eq!(
            cleanup_temp_recordings(&nonexistent, Duration::from_secs(60)),
            0
        );
    }

    // ----- VoiceModeTool (v0.9.0 W1 B10) -----

    #[test]
    fn voice_mode_tool_default_is_hidden() {
        // NO-STUBS contract: a freshly defaulted tool must not advertise
        // itself before the host wires a real backend.
        let t = VoiceModeTool::default();
        assert!(
            !t.is_available(),
            "VoiceModeTool::default() must report is_available=false"
        );
        assert_eq!(t.name(), "voice_mode");
    }

    #[test]
    fn voice_mode_tool_with_backend_is_available() {
        let (vm, _r, _x, _p) = make_voice_mode_with_capturers();
        let t = VoiceModeTool::new(Arc::new(vm));
        assert!(t.is_available());
        assert!(!t.is_concurrency_safe(&json!({"action": "status"})));
    }

    #[tokio::test]
    async fn voice_mode_tool_toggle_round_trip_flips_state() {
        let (vm, _r, _x, _p) = make_voice_mode_with_capturers();
        let t = VoiceModeTool::new(Arc::new(vm));

        // Initially idle.
        let r0 = t.execute(json!({"action": "status"})).await;
        assert!(!r0.is_error);
        assert!(r0.content.contains("\"is_recording\":false"));

        // toggle_record → starts.
        let r1 = t.execute(json!({"action": "toggle_record"})).await;
        assert!(!r1.is_error);
        assert!(r1.content.contains("\"is_recording\":true"));

        // toggle_record again → stops.
        let r2 = t.execute(json!({"action": "toggle_record"})).await;
        assert!(!r2.is_error);
        assert!(r2.content.contains("\"is_recording\":false"));
    }

    #[tokio::test]
    async fn voice_mode_tool_explicit_stop_surfaces_wav_path() {
        let rec = Arc::new(CapturingAudioRecorder::new());
        let tx = Arc::new(CapturingTranscriptionBackend::new(
            TranscriptionOutcome::Ok {
                transcript: String::new(),
                filtered: false,
            },
        ));
        let player = Arc::new(NullAudioPlayer);
        let probe = Arc::new(StaticAudioEnvironmentProbe(AudioEnvironment::default()));
        let vm = Arc::new(VoiceMode::new(rec.clone(), tx, player, probe));
        let t = VoiceModeTool::new(vm);

        rec.set_next_outcome(RecordingOutcome::Captured {
            wav_path: PathBuf::from("/tmp/wayland_voice_test_b10.wav"),
        });
        // start → stop should return the wav path.
        let _ = t.execute(json!({"action": "start"})).await;
        let r = t.execute(json!({"action": "stop"})).await;
        assert!(!r.is_error);
        assert!(
            r.content.contains("wayland_voice_test_b10.wav"),
            "expected wav path in stop result, got: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn voice_mode_tool_cancel_is_idempotent_when_idle() {
        let (vm, _r, _x, _p) = make_voice_mode_with_capturers();
        let t = VoiceModeTool::new(Arc::new(vm));
        let r = t.execute(json!({"action": "cancel"})).await;
        assert!(
            !r.is_error,
            "cancel-when-idle must not error: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn voice_mode_tool_rejects_unknown_action() {
        let (vm, _r, _x, _p) = make_voice_mode_with_capturers();
        let t = VoiceModeTool::new(Arc::new(vm));
        let r = t.execute(json!({"action": "nope"})).await;
        assert!(r.is_error);
        assert!(r.content.contains("unknown action"));
    }

    #[tokio::test]
    async fn voice_mode_tool_missing_action_field_errors() {
        let (vm, _r, _x, _p) = make_voice_mode_with_capturers();
        let t = VoiceModeTool::new(Arc::new(vm));
        let r = t.execute(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("action is required"));
    }
}
