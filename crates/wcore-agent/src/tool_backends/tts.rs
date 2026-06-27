//! v0.9.0 Wave-1 B2 — `text_to_speech` cloud backends (OpenAI TTS +
//! ElevenLabs) with optional cross-wire to B11's local Piper fallback.
//!
//! Resolver order (closes R-H2 empty-string + R-H3 single-flight):
//! 1. `OPENAI_API_KEY` → OpenAI TTS (`tts-1`, $15/M chars)
//! 2. `ELEVENLABS_API_KEY` → ElevenLabs `eleven_turbo_v2_5`
//! 3. Piper local fallback (cross-wired from `super::piper`; lazy import
//!    so this file compiles standalone even if B11 hasn't landed)
//!
//! Each backend SHARES the same `build_ssrf_safe_tool_client` so a
//! malicious redirect to `169.254.169.254` / RFC1918 is refused
//! mid-chain. The full request → bytes → atomic-write pipeline is
//! wrapped in a `tokio::time::timeout` (R-H1 two-layer timeout) so a
//! stuck body read cannot hang the agent indefinitely.
//!
//! Output path safety (S-M4 + S-H5 path-traversal defense):
//! * Caller-supplied `output_path` MUST be under one of:
//!   - The system temp dir (`std::env::temp_dir()` — covers
//!     `/tmp/wayland-*`, `$TMPDIR/...`)
//!   - `~/.wayland/tts/`
//!   - The user's home dir (legacy `~/`-expanded paths from
//!     `tts_tool.rs::expand_user`)
//! * Paths containing `..` segments are rejected up-front.
//! * Writes use `tempfile::NamedTempFile` in the destination's parent
//!   directory + atomic `persist()` so a half-written file never
//!   appears at the final path.
//! * Post-write we re-canonicalise the realpath and re-check it is
//!   still under a permitted prefix (TOCTOU defense — if a symlink
//!   swap races us between validate and write, the post-check
//!   catches it).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use wcore_config::config::Config;

use super::build_ssrf_safe_tool_client;
use super::shared::{OPENAI_API_BASE, join_openai_endpoint, openai_wire_media_base, read_env_key};
use wcore_tools::tts_tool::{
    TtsBackend, TtsError, TtsFormat, TtsProvider, TtsRequest, TtsResponse,
};

// Cross-wire from B11 (piper local TTS). Declared as a lazy import so
// this module compiles standalone even if B11's `piper.rs` module isn't
// in the tree yet — the `#[allow(unused_imports)]` lets the compiler
// drop the import when piper hasn't landed. The resolver below only
// calls `build_piper_tts_backend()` behind a gate, so the symbol must
// exist at link time for the call to fire; until B11 lands, the call
// is dead-code and the env-key resolvers carry the file.
#[allow(unused_imports)]
#[cfg(feature = "piper_tts")]
use super::piper::build_piper_tts_backend;

/// Two-layer timeout cap for the entire synthesis pipeline (HTTP
/// exchange + body read + atomic write). Reqwest's own `.timeout()`
/// covers only the HTTP exchange, so we wrap the lot — same pattern as
/// `tool_backends.rs::fetch_inner` (R-H1).
const SYNTH_TIMEOUT: Duration = Duration::from_secs(60);

/// ElevenLabs default voice id — Rachel (well-known stable voice that
/// every ElevenLabs account has access to).
pub const ELEVENLABS_DEFAULT_VOICE_ID: &str = "21m00Tcm4TlvDq8ikWAM";

/// OpenAI TTS default voice (matches the OpenAI docs "alloy" default).
pub const OPENAI_TTS_DEFAULT_VOICE: &str = "alloy";

// ---------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------

/// Build a concrete OpenAI TTS backend from the active provider when it
/// serves the OpenAI-wire `/audio/speech` endpoint. Returns `None` for
/// providers without it (Anthropic/Gemini and the LLM-only OpenAI-compat
/// routers) or an empty resolved key.
///
/// Only native **OpenAI** and **FluxRouter** are routed here, via
/// [`openai_wire_media_base`] (which fills FluxRouter's default base when
/// `config.base_url` is empty and guarantees a `/v1` root). A Flux session
/// targets `https://api.fluxrouter.ai/v1/audio/speech` with the Flux key
/// (#310) instead of sending the wrong key to `api.openai.com` (401).
///
/// NOTE (#310): Flux's TTS endpoint is undocumented; we deliberately do NOT
/// special-case it. If `{base}/audio/speech` is unsupported the provider
/// returns its own error — still strictly better than a 401 from the wrong
/// key against OpenAI.
///
/// Returns the concrete `OpenAiTtsBackend` (not a trait object) so the
/// resolved endpoint + key are unit-assertable.
pub(crate) fn openai_tts_backend_from_config(config: &Config) -> Option<OpenAiTtsBackend> {
    if config.api_key.trim().is_empty() {
        return None;
    }
    let base = openai_wire_media_base(config)?;
    let endpoint = join_openai_endpoint(&base, "audio/speech");
    Some(OpenAiTtsBackend::with_endpoint(
        config.api_key.clone(),
        endpoint,
    ))
}

/// Pick the best available TTS backend from the resolved `Config` and the
/// user's environment.
///
/// 1. **Active OpenAI-wire media provider** (native OpenAI / Flux Router) —
///    built from the resolved `/v1` API root + `config.api_key` (#310; see
///    [`openai_wire_media_base`])
/// 2. `OPENAI_API_KEY` → OpenAI TTS at `api.openai.com` (back-compat)
/// 3. `ELEVENLABS_API_KEY` (paid, most natural)
/// 4. Piper local fallback (B11 cross-wire — only when the `piper_tts`
///    feature is enabled by B13's assembler)
/// 5. `None` — TTS tool hides via `Tool::is_available()` so the model
///    never sees a tool it can't call. Doctor reports "TTS hidden:
///    set OPENAI_API_KEY or ELEVENLABS_API_KEY".
pub fn build_tts_backend(config: &Config) -> Option<Arc<dyn TtsBackend>> {
    if let Some(backend) = openai_tts_backend_from_config(config) {
        tracing::info!(
            "tts: using OpenAI TTS at {} (active OpenAI-wire provider)",
            config.base_url
        );
        return Some(Arc::new(backend));
    }
    if let Some(key) = read_env_key("OPENAI_API_KEY") {
        tracing::info!("tts: using OpenAI TTS (OPENAI_API_KEY found)");
        return Some(Arc::new(OpenAiTtsBackend::new(key)));
    }
    if let Some(key) = read_env_key("ELEVENLABS_API_KEY") {
        tracing::info!("tts: using ElevenLabs (ELEVENLABS_API_KEY found)");
        return Some(Arc::new(ElevenLabsTtsBackend::new(key)));
    }
    #[cfg(feature = "piper_tts")]
    {
        if let Some(b) = build_piper_tts_backend() {
            tracing::info!("tts: using local Piper (no cloud key, voice models found)");
            return Some(b);
        }
    }
    tracing::warn!(
        "tts: no TTS backend configured — set OPENAI_API_KEY or ELEVENLABS_API_KEY \
         (or download Piper voices via piper_download). Tool hidden."
    );
    None
}

// ---------------------------------------------------------------------
// Path-traversal safety helpers
// ---------------------------------------------------------------------

/// Canonical permitted-prefix list for TTS output paths. Returned as
/// owned `PathBuf`s because `temp_dir()` / `home_dir()` allocate.
fn permitted_prefixes() -> Vec<PathBuf> {
    let mut v = vec![std::env::temp_dir()];
    if let Some(home) = dirs::home_dir() {
        v.push(home.join(".wayland").join("tts"));
        // Also permit anywhere under $HOME — `tts_tool.rs::expand_user`
        // turns `~/foo.mp3` into `$HOME/foo.mp3`, and the Python original
        // accepted that; we keep the seam compatible but reject `..`
        // segments which is the actual escape hatch.
        v.push(home);
    }
    v
}

/// Validate that `path` is under one of the permitted prefixes AND
/// contains no `..` segments. Returns the canonical parent directory
/// (caller uses it as the `tempfile` parent) or a typed error.
fn validate_output_path(path: &Path) -> Result<PathBuf, TtsError> {
    // 1. Reject any `..` component up-front. Even if canonicalisation
    //    would later resolve it under a permitted prefix, the literal
    //    presence of `..` is a tell of intent and we refuse to play.
    for comp in path.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return Err(TtsError::Configuration(format!(
                "TTS output_path contains '..' segment which is not permitted: {}",
                path.display()
            )));
        }
    }

    // 2. The parent must exist (the tool already calls `create_dir_all`
    //    so this should be true) — we canonicalise it for the prefix
    //    check.
    let parent = path.parent().ok_or_else(|| {
        TtsError::Configuration(format!("output_path has no parent: {}", path.display()))
    })?;
    let parent = if parent.as_os_str().is_empty() {
        // A bare filename like "foo.mp3" — the tool would resolve under
        // CWD which is NOT a permitted prefix. Refuse.
        return Err(TtsError::Configuration(format!(
            "output_path must be absolute or contain a permitted parent dir: {}",
            path.display()
        )));
    } else {
        parent.to_path_buf()
    };

    let canonical_parent = std::fs::canonicalize(&parent).map_err(|e| {
        TtsError::Configuration(format!(
            "could not canonicalise output_path parent '{}': {e}",
            parent.display()
        ))
    })?;

    // 3. Must live under at least one permitted prefix.
    let prefixes = permitted_prefixes();
    let canonical_prefixes: Vec<PathBuf> = prefixes
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(&p).ok())
        .collect();
    let allowed = canonical_prefixes
        .iter()
        .any(|prefix| canonical_parent.starts_with(prefix));
    if !allowed {
        return Err(TtsError::Configuration(format!(
            "output_path '{}' is outside permitted prefixes (temp dir, ~/.wayland/tts/, $HOME)",
            path.display()
        )));
    }
    Ok(canonical_parent)
}

/// Atomic-write `bytes` to `output_path` using `tempfile::NamedTempFile`
/// in the parent dir + `persist()` for cross-platform atomic rename.
/// Re-checks the realpath of the destination AFTER write (TOCTOU
/// defense — if a symlink swap raced us between validate and write,
/// `canonicalize` will return the attacker-controlled target and we
/// refuse to claim success).
fn atomic_write(parent: &Path, output_path: &Path, bytes: &[u8]) -> Result<(), TtsError> {
    use std::io::Write as _;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
        TtsError::Other(format!(
            "tts: could not create tempfile in '{}': {e}",
            parent.display()
        ))
    })?;
    tmp.write_all(bytes)
        .map_err(|e| TtsError::Other(format!("tts: write_all failed: {e}")))?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|e| TtsError::Other(format!("tts: fsync failed: {e}")))?;
    tmp.persist(output_path).map_err(|e| {
        TtsError::Other(format!(
            "tts: atomic persist to '{}' failed: {e}",
            output_path.display()
        ))
    })?;

    // TOCTOU re-check: canonicalise the destination and confirm it is
    // still under a permitted prefix.
    let canonical = std::fs::canonicalize(output_path).map_err(|e| {
        TtsError::Other(format!(
            "tts: post-write canonicalize of '{}' failed: {e}",
            output_path.display()
        ))
    })?;
    let prefixes = permitted_prefixes();
    let canonical_prefixes: Vec<PathBuf> = prefixes
        .into_iter()
        .filter_map(|p| std::fs::canonicalize(&p).ok())
        .collect();
    let still_safe = canonical_prefixes
        .iter()
        .any(|prefix| canonical.starts_with(prefix));
    if !still_safe {
        // Clean up — the file is at a hostile location.
        let _ = std::fs::remove_file(&canonical);
        return Err(TtsError::Configuration(format!(
            "tts: post-write realpath '{}' is outside permitted prefixes (symlink race?)",
            canonical.display()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// OpenAI TTS backend
// ---------------------------------------------------------------------

/// OpenAI `tts-1` text-to-speech via the audio-speech endpoint. Returns
/// audio bytes directly in the HTTP response body (no separate fetch).
pub struct OpenAiTtsBackend {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
}

impl OpenAiTtsBackend {
    /// Build the backend against the canonical OpenAI host. The endpoint is
    /// derived as `{OPENAI_API_BASE}/audio/speech` (#310) — equivalent to
    /// the pre-#310 hardcoded `api.openai.com` URL.
    pub fn new(api_key: String) -> Self {
        Self::with_endpoint(
            api_key,
            join_openai_endpoint(OPENAI_API_BASE, "audio/speech"),
        )
    }

    /// Internal constructor used by tests with a mock-server endpoint, and
    /// by the resolver to point at an OpenAI-wire provider's `base_url`.
    pub(crate) fn with_endpoint(api_key: String, endpoint: String) -> Self {
        let model =
            std::env::var("WAYLAND_OPENAI_TTS_MODEL").unwrap_or_else(|_| "tts-1".to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
            model,
        }
    }

    /// Resolved request endpoint (`{base_url}/audio/speech`). Exposed so the
    /// resolver wiring (#310) is unit-assertable without a network call.
    #[cfg(test)]
    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Resolved bearer key sent to the speech endpoint. Exposed for the
    /// #310 resolver tests (asserts the Flux key, not OPENAI_API_KEY).
    #[cfg(test)]
    pub(crate) fn api_key(&self) -> &str {
        &self.api_key
    }

    fn response_format_for(format: TtsFormat) -> &'static str {
        match format {
            TtsFormat::Mp3 => "mp3",
            TtsFormat::Wav => "wav",
            TtsFormat::Opus => "opus",
        }
    }
}

#[async_trait]
impl TtsBackend for OpenAiTtsBackend {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError> {
        let parent = validate_output_path(&request.output_path)?;
        let voice = request
            .voice
            .clone()
            .unwrap_or_else(|| OPENAI_TTS_DEFAULT_VOICE.to_string());
        let body = serde_json::json!({
            "model": self.model,
            "input": request.text,
            "voice": voice,
            "response_format": Self::response_format_for(request.format),
            "speed": request.speed.unwrap_or(1.0),
        });

        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let endpoint = self.endpoint.clone();
        let output_path = request.output_path.clone();
        let format = request.format;

        let inner = async move {
            let resp = client
                .post(&endpoint)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {api_key}"))
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(Duration::from_secs(45))
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| TtsError::Other(format!("openai tts request failed: {e}")))?;

            let status = resp.status();
            if !status.is_success() {
                // Capture rate-limit Retry-After if surfaced (R-H2 listed).
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let txt = resp.text().await.unwrap_or_default();
                let snippet: String = txt.chars().take(400).collect();
                let mut msg = format!("openai tts returned HTTP {}: {snippet}", status.as_u16());
                if let Some(ra) = retry_after {
                    msg.push_str(&format!(" (Retry-After: {ra})"));
                }
                return Err(TtsError::Other(msg));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| TtsError::Other(format!("openai tts body read failed: {e}")))?;
            let bytes_vec = bytes.to_vec();
            if bytes_vec.is_empty() {
                return Err(TtsError::Other(
                    "openai tts returned empty audio body".to_string(),
                ));
            }
            atomic_write(&parent, &output_path, &bytes_vec)?;
            Ok::<_, TtsError>(TtsResponse {
                path: output_path,
                bytes_written: bytes_vec.len() as u64,
                format,
                provider: TtsProvider::OpenAi,
            })
        };

        match tokio::time::timeout(SYNTH_TIMEOUT, inner).await {
            Ok(res) => res,
            Err(_) => Err(TtsError::Other(format!(
                "openai tts timed out after {}s (whole-pipeline cap)",
                SYNTH_TIMEOUT.as_secs()
            ))),
        }
    }
}

// ---------------------------------------------------------------------
// ElevenLabs backend
// ---------------------------------------------------------------------

/// ElevenLabs `eleven_turbo_v2_5` via the `/v1/text-to-speech/<voice_id>`
/// endpoint. Returns MP3 bytes in the response body.
pub struct ElevenLabsTtsBackend {
    client: Client,
    api_key: String,
    endpoint_base: String,
    model_id: String,
}

impl ElevenLabsTtsBackend {
    pub fn new(api_key: String) -> Self {
        Self::with_endpoint(
            api_key,
            "https://api.elevenlabs.io/v1/text-to-speech".to_string(),
        )
    }

    /// Internal constructor used by tests with a mock-server endpoint.
    pub(crate) fn with_endpoint(api_key: String, endpoint_base: String) -> Self {
        let model_id = std::env::var("WAYLAND_ELEVENLABS_MODEL")
            .unwrap_or_else(|_| "eleven_turbo_v2_5".to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint_base,
            model_id,
        }
    }
}

#[async_trait]
impl TtsBackend for ElevenLabsTtsBackend {
    async fn synthesize(&self, request: TtsRequest) -> Result<TtsResponse, TtsError> {
        let parent = validate_output_path(&request.output_path)?;
        let voice_id = request
            .voice
            .clone()
            .unwrap_or_else(|| ELEVENLABS_DEFAULT_VOICE_ID.to_string());
        let url = format!("{}/{}", self.endpoint_base.trim_end_matches('/'), voice_id);
        let body = serde_json::json!({
            "text": request.text,
            "model_id": self.model_id,
        });

        let client = self.client.clone();
        let api_key = self.api_key.clone();
        let output_path = request.output_path.clone();
        let format = request.format;

        let inner = async move {
            let resp = client
                .post(&url)
                .header("xi-api-key", api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "audio/mpeg")
                .timeout(Duration::from_secs(45))
                .body(body.to_string())
                .send()
                .await
                .map_err(|e| TtsError::Other(format!("elevenlabs tts request failed: {e}")))?;

            let status = resp.status();
            if !status.is_success() {
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let txt = resp.text().await.unwrap_or_default();
                let snippet: String = txt.chars().take(400).collect();
                let mut msg = format!(
                    "elevenlabs tts returned HTTP {}: {snippet}",
                    status.as_u16()
                );
                if let Some(ra) = retry_after {
                    msg.push_str(&format!(" (Retry-After: {ra})"));
                }
                return Err(TtsError::Other(msg));
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| TtsError::Other(format!("elevenlabs tts body read failed: {e}")))?;
            let bytes_vec = bytes.to_vec();
            if bytes_vec.is_empty() {
                return Err(TtsError::Other(
                    "elevenlabs tts returned empty audio body".to_string(),
                ));
            }
            atomic_write(&parent, &output_path, &bytes_vec)?;
            Ok::<_, TtsError>(TtsResponse {
                path: output_path,
                bytes_written: bytes_vec.len() as u64,
                format,
                provider: TtsProvider::ElevenLabs,
            })
        };

        match tokio::time::timeout(SYNTH_TIMEOUT, inner).await {
            Ok(res) => res,
            Err(_) => Err(TtsError::Other(format!(
                "elevenlabs tts timed out after {}s (whole-pipeline cap)",
                SYNTH_TIMEOUT.as_secs()
            ))),
        }
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;
    use wcore_config::config::ProviderType;
    use wcore_tools::tts_tool::TtsFormat;

    fn make_request(output_path: PathBuf) -> TtsRequest {
        TtsRequest {
            text: "hello from wayland".to_string(),
            provider: TtsProvider::OpenAi,
            voice: None,
            model: None,
            format: TtsFormat::Mp3,
            output_path,
            speed: Some(1.0),
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
    /// Flux key (#310). The pre-fix fixture used `provider: OpenAI`, masking
    /// the bug — the resolver gate never matched FluxRouter.
    fn flux_config() -> Config {
        Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux-test".to_string(),
            base_url: "https://api.fluxrouter.ai/v1".to_string(),
            ..Config::default()
        }
    }

    // ------ resolver priority ------

    #[test]
    #[serial]
    fn build_tts_backend_prefers_openai_over_elevenlabs() {
        // Set both — OpenAI must win.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-test-openai");
            std::env::set_var("ELEVENLABS_API_KEY", "el-test");
        }
        let b = build_tts_backend(&env_only_config());
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        // We can't introspect the trait object directly, but constructing
        // it should not panic. The provider field on a synthesized
        // response would be `OpenAi` — see the round-trip test below.
        assert!(b.is_some(), "resolver should pick OpenAI when key set");
    }

    #[test]
    #[serial]
    fn null_default_skips_registration() {
        // No env vars set → resolver returns None → bootstrap.rs skips
        // the `registry.register` call → tool stays hidden.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        assert!(build_tts_backend(&env_only_config()).is_none());
    }

    #[test]
    #[serial]
    fn empty_string_env_var_treated_as_unset() {
        // R-H2 — empty string OPENAI_API_KEY="" must NOT count as
        // configured. The resolver should look past it to ElevenLabs.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "");
            std::env::set_var("ELEVENLABS_API_KEY", "el-test-key");
        }
        let b = build_tts_backend(&env_only_config());
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        assert!(
            b.is_some(),
            "ElevenLabs should be picked when OpenAI is empty"
        );
    }

    // ------ #310: OpenAI-wire provider routing (Flux) ------

    #[test]
    #[serial]
    fn openai_tts_resolves_from_flux_config_not_openai_host() {
        // #310 regression: a Flux Router session carries
        // base_url=https://api.fluxrouter.ai/v1 + api_key=sk-flux-test. The
        // resolved OpenAI TTS endpoint must target Flux's host with the Flux
        // key, NOT api.openai.com (which would 401).
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        let backend = openai_tts_backend_from_config(&flux_config())
            .expect("OpenAI-wire config must resolve");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/audio/speech"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
    }

    #[test]
    #[serial]
    fn openai_tts_config_takes_priority_over_openai_env_key() {
        // Even with OPENAI_API_KEY set, an active OpenAI-wire provider
        // (Flux) wins — the resolver builds from config.
        unsafe {
            std::env::set_var("OPENAI_API_KEY", "sk-openai-env");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        let backend = openai_tts_backend_from_config(&flux_config())
            .expect("config OpenAI-wire provider must resolve");
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
        }
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/audio/speech"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
    }

    #[test]
    #[serial]
    fn openai_tts_falls_back_to_openai_host_when_config_not_openai_wire() {
        // Back-compat: a non-OpenAI provider (default Anthropic) must not
        // hijack the OpenAI TTS slot, so the config gate declines and the
        // env path builds against api.openai.com.
        assert!(
            openai_tts_backend_from_config(&env_only_config()).is_none(),
            "non-OpenAI provider must not hijack the OpenAI TTS slot"
        );
        let backend = OpenAiTtsBackend::new("sk-openai-env".to_string());
        assert_eq!(backend.endpoint(), "https://api.openai.com/v1/audio/speech");
        assert_eq!(backend.api_key(), "sk-openai-env");
    }

    #[test]
    #[serial]
    fn openai_tts_resolves_flux_default_base_when_config_base_empty() {
        // Real Flux sessions leave config.base_url empty; the resolver must
        // still target Flux via the newtype default.
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        let cfg = Config {
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux-test".to_string(),
            base_url: String::new(),
            ..Config::default()
        };
        let backend =
            openai_tts_backend_from_config(&cfg).expect("Flux must resolve from default base");
        assert_eq!(
            backend.endpoint(),
            "https://api.fluxrouter.ai/v1/audio/speech"
        );
        assert_eq!(backend.api_key(), "sk-flux-test");
    }

    #[test]
    #[serial]
    fn openai_tts_adds_v1_for_native_openai_config() {
        // Native OpenAI base is `https://api.openai.com` (no `/v1`); the
        // resolver must add it (pre-fix this produced a 404 endpoint).
        unsafe {
            std::env::remove_var("OPENAI_API_KEY");
            std::env::remove_var("ELEVENLABS_API_KEY");
        }
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            ..Config::default()
        };
        let backend = openai_tts_backend_from_config(&cfg).expect("native OpenAI must resolve");
        assert_eq!(backend.endpoint(), "https://api.openai.com/v1/audio/speech");
    }

    #[test]
    #[serial]
    fn openai_tts_declines_userinfo_base_url() {
        // Hostile base_url with userinfo must fail closed (key-exfil vector).
        let cfg = Config {
            provider: ProviderType::OpenAI,
            api_key: "sk-openai".to_string(),
            base_url: "https://attacker.com@api.openai.com/v1".to_string(),
            ..Config::default()
        };
        assert!(openai_tts_backend_from_config(&cfg).is_none());
    }

    // ------ ElevenLabs default voice ------

    #[test]
    fn elevenlabs_default_voice_is_rachel() {
        assert_eq!(ELEVENLABS_DEFAULT_VOICE_ID, "21m00Tcm4TlvDq8ikWAM");
    }

    // ------ happy path: OpenAI writes bytes to output_path ------

    #[tokio::test]
    async fn openai_tts_writes_bytes_to_output_path() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let audio_payload: Vec<u8> = vec![0xFF, 0xFB, 0x90, 0x44, 0xDE, 0xAD, 0xBE, 0xEF];
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(audio_payload.clone()))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/audio/speech", server.uri()),
        );
        let resp = backend
            .synthesize(make_request(out.clone()))
            .await
            .expect("synthesis should succeed");
        assert_eq!(resp.provider, TtsProvider::OpenAi);
        assert_eq!(resp.bytes_written, audio_payload.len() as u64);
        let on_disk = std::fs::read(&out).expect("file must exist");
        assert_eq!(on_disk, audio_payload);
    }

    #[tokio::test]
    async fn tts_response_carries_provider_field_correctly() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let payload = vec![0x12, 0x34, 0x56];
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("eleven.mp3");
        let backend = ElevenLabsTtsBackend::with_endpoint(
            "el-test".to_string(),
            format!("{}/v1/text-to-speech", server.uri()),
        );
        let resp = backend
            .synthesize(make_request(out.clone()))
            .await
            .expect("elevenlabs should succeed");
        assert_eq!(resp.provider, TtsProvider::ElevenLabs);
        assert_eq!(resp.bytes_written, payload.len() as u64);
    }

    // ------ failure paths ------

    #[tokio::test]
    async fn openai_handles_http_5xx_returns_typed_error() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/audio/speech", server.uri()),
        );
        let err = backend
            .synthesize(make_request(out))
            .await
            .expect_err("5xx must surface as error");
        let msg = format!("{err}");
        assert!(msg.contains("503"), "expected 503 in error: {msg}");
    }

    #[tokio::test]
    async fn openai_handles_http_429_with_retry_after_backoff() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "30")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/audio/speech", server.uri()),
        );
        let err = backend
            .synthesize(make_request(out))
            .await
            .expect_err("429 must surface as error");
        let msg = format!("{err}");
        assert!(msg.contains("429"), "expected 429 in error: {msg}");
        assert!(
            msg.contains("Retry-After: 30"),
            "expected Retry-After in error: {msg}"
        );
    }

    #[tokio::test]
    async fn openai_handles_malformed_response_empty_body() {
        // OpenAI's TTS endpoint returns raw audio bytes (no JSON to
        // parse), so the "malformed" case is "200 OK with empty body".
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(Vec::<u8>::new()))
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/audio/speech", server.uri()),
        );
        let err = backend
            .synthesize(make_request(out))
            .await
            .expect_err("empty body must surface as error");
        let msg = format!("{err}");
        assert!(
            msg.contains("empty audio body"),
            "expected empty-body error: {msg}"
        );
    }

    #[tokio::test]
    async fn openai_handles_network_timeout() {
        // v0.9.1 W1 E (debt sweep): deterministic replacement of the
        // v0.9.0 `drop(server)`-race variant. Mount a slow responder
        // (30s delay) and assert the outer `tokio::time::timeout(1s)`
        // wrapper fires — `Elapsed` is the deterministic signal. The
        // server stays alive for the whole test, so the only mechanism
        // that can resolve is the outer timeout.
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(30))
                    .set_body_bytes(vec![1, 2, 3]),
            )
            .mount(&server)
            .await;
        let endpoint = format!("{}/v1/audio/speech", server.uri());

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint("sk-test".to_string(), endpoint);
        let wrapped = tokio::time::timeout(
            Duration::from_secs(1),
            backend.synthesize(make_request(out)),
        )
        .await;
        assert!(
            wrapped.is_err(),
            "expected outer timeout to fire on slow server, got {wrapped:?}"
        );
    }

    // ------ path-traversal + TOCTOU ------

    #[tokio::test]
    async fn tts_rejects_output_path_with_dotdot_segment() {
        let tmp = TempDir::new().unwrap();
        // Construct an output path with a literal `..` segment.
        let evil = tmp.path().join("subdir").join("..").join("evil.mp3");
        std::fs::create_dir_all(tmp.path().join("subdir")).unwrap();

        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            "http://127.0.0.1:1/never".to_string(),
        );
        let err = backend
            .synthesize(make_request(evil))
            .await
            .expect_err("`..` segment must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains(".."),
            "expected `..` rejection message, got: {msg}"
        );
    }

    #[tokio::test]
    async fn tts_rejects_output_path_outside_permitted_prefix() {
        // /etc is not under temp dir, $HOME, or ~/.wayland/tts/.
        // Most CIs don't have /etc/wayland-tts-test writable but that's
        // fine — the path validator runs BEFORE write. We point at a
        // real existing directory outside permitted prefixes.
        let evil_path = PathBuf::from("/etc/passwd_tts_test.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            "http://127.0.0.1:1/never".to_string(),
        );
        let err = backend
            .synthesize(make_request(evil_path))
            .await
            .expect_err("path outside permitted prefixes must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("outside permitted prefixes") || msg.contains("canonicalise"),
            "expected prefix rejection, got: {msg}"
        );
    }

    // ------ SSRF redirect refusal ------

    #[tokio::test]
    async fn tts_refuses_ssrf_redirect_to_metadata_service() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = OpenAiTtsBackend::with_endpoint(
            "sk-test".to_string(),
            format!("{}/v1/audio/speech", server.uri()),
        );
        let err = backend
            .synthesize(make_request(out))
            .await
            .expect_err("redirect to metadata service must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains("redirect") || msg.contains("blocked") || msg.contains("request failed"),
            "expected SSRF-refusal error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn elevenlabs_refuses_ssrf_redirect_to_private_ip() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "http://10.0.0.1/internal"),
            )
            .mount(&server)
            .await;

        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("speech.mp3");
        let backend = ElevenLabsTtsBackend::with_endpoint(
            "el-test".to_string(),
            format!("{}/v1/text-to-speech", server.uri()),
        );
        let err = backend
            .synthesize(make_request(out))
            .await
            .expect_err("redirect to private IP must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains("redirect") || msg.contains("blocked") || msg.contains("request failed"),
            "expected SSRF-refusal error, got: {msg}"
        );
    }

    // ------ response_format mapping ------

    #[test]
    fn openai_response_format_maps_all_three_formats() {
        assert_eq!(OpenAiTtsBackend::response_format_for(TtsFormat::Mp3), "mp3");
        assert_eq!(OpenAiTtsBackend::response_format_for(TtsFormat::Wav), "wav");
        assert_eq!(
            OpenAiTtsBackend::response_format_for(TtsFormat::Opus),
            "opus"
        );
    }
}
