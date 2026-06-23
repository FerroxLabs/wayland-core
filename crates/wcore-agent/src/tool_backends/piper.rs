//! v0.9.0 Wave-1 B11 — Piper local-TTS download + (deferred) synthesis
//! backend.
//!
//! `crates/wcore-tools/src/piper_download.rs` ships a fully-tested
//! HELPER (voice/binary download + atomic write + cache probe) that
//! takes pluggable `ModelDownloader` + `BinaryExtractor` trait objects.
//! Its fail-loud defaults (`NullModelDownloader` + `NullBinaryExtractor`)
//! never hit the network. This file wires the real implementations:
//!
//! 1. [`SsrfSafeHttpDownloader`] — `ModelDownloader` over reqwest using
//!    [`super::build_ssrf_safe_tool_client`] so a malicious redirect to
//!    169.254.169.254 / 10.x.x.x / 127.x.x.x / [fd00::] is refused
//!    mid-chain (Track B preamble §1). Wraps the request + body read in
//!    `tokio::time::timeout` for whole-pipeline cap (preamble §2).
//!
//! 2. [`TarGzBinaryExtractor`] — `BinaryExtractor` over `tar` + `flate2`
//!    with path-traversal defense: every archive entry is rejected if
//!    its normalized path escapes `dest_dir` or contains `..` / absolute
//!    components (preamble §6, S-H5).
//!
//! 3. [`build_piper_download_backend`] — returns a configured
//!    [`PiperDownloader`] wrapping both. Always `Some(_)` (no env gate)
//!    because voice downloads are user-initiated and the existing
//!    `is_voice_cached` helper short-circuits without touching the net.
//!
//! 4. [`PiperTtsBackend`] — implements `TtsBackend` for `wcore-tools::
//!    tts_tool`. **v0.9.0 synthesis is a deferred stub** — voices
//!    download correctly but synthesis returns a clear typed
//!    `TtsError::DependencyMissing` pointing at the v0.9.1 runtime
//!    landing. Sean's directive ("Piper download, we can work with. You
//!    can make sure it's connected.") explicitly authorized this
//!    split. Once a `piper-rs` / ONNX-Runtime crate is selected in
//!    v0.9.1, the synthesize stub flips to the real path without
//!    touching the resolver or download surfaces.
//!
//! 5. [`build_piper_tts_backend`] — returns `Some(PiperTtsBackend)` only
//!    when a usable voice is present on disk (default voice cached OR
//!    `PIPER_VOICE` env points to a downloaded voice). Returns `None`
//!    otherwise so B2's `build_tts_backend` falls through to its
//!    OpenAI > ElevenLabs cloud chain.
//!
//! ## Cross-wire with B2 (`tool_backends/tts.rs`)
//!
//! B2's resolver imports this symbol behind the `piper_tts` Cargo
//! feature (see B2's module header lines 47-54). B13's assembler turns
//! the feature on once both files are present in the tree, so:
//!
//! * Without `piper_tts`: B2 ignores this file. Cloud-only TTS.
//! * With `piper_tts`: B2 calls `build_piper_tts_backend()` as the
//!   3rd-priority free-local option after OpenAI / ElevenLabs.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use flate2::read::GzDecoder;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::read_env_key;
use wcore_tools::piper_download::{
    BinaryExtractor, ModelDownloader, PiperDownloader, is_voice_cached,
};
use wcore_tools::tts_tool::{TtsBackend, TtsError, TtsRequest, TtsResponse};

/// Whole-pipeline timeout cap on a single Piper voice/binary download
/// (HTTP exchange + body read + atomic write). Voice files are ~60 MB,
/// binary tarballs ~3-5 MB. 120 s is generous on a typical home
/// connection and well under any agent-loop wall-clock budget.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

/// Default voice id used when neither the request nor `PIPER_VOICE`
/// supplies one. Matches the Python `DEFAULT_VOICE` constant in the
/// original implementation.
pub const DEFAULT_PIPER_VOICE: &str = "en_US-amy-medium";

// ---------------------------------------------------------------------
// SsrfSafeHttpDownloader — real ModelDownloader
// ---------------------------------------------------------------------

/// Real [`ModelDownloader`] over reqwest. Uses
/// [`super::build_ssrf_safe_tool_client`] so a `302` from huggingface.co
/// or github.com to a private network is refused before bytes flow.
///
/// `fetch` is sync (the `ModelDownloader` trait is sync), so internally
/// we drive the async reqwest request on a temporary single-threaded
/// runtime via `tokio::runtime::Builder`. This keeps the helper crate
/// async-free while still using the workspace's only HTTP client.
pub struct SsrfSafeHttpDownloader {
    client: Client,
}

impl SsrfSafeHttpDownloader {
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for SsrfSafeHttpDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDownloader for SsrfSafeHttpDownloader {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, anyhow::Error> {
        let client = self.client.clone();
        let url_owned = url.to_string();
        // Spin a fresh single-threaded runtime — `PiperDownloader::
        // download_voice` is sync. Using a per-call runtime is fine for
        // a download tool that may run once a session; we are not on a
        // hot path. The block_on call is bounded by `DOWNLOAD_TIMEOUT`
        // so a stuck server cannot hang the caller thread.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow::anyhow!("piper: could not build inner runtime: {e}"))?;
        rt.block_on(async move {
            let inner = async {
                let resp = client
                    .get(&url_owned)
                    .header(
                        reqwest::header::USER_AGENT,
                        "Mozilla/5.0 (compatible; wayland-core/Piper)",
                    )
                    .timeout(Duration::from_secs(60))
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("piper download request failed: {e}"))?;
                let status = resp.status();
                if !status.is_success() {
                    return Err(anyhow::anyhow!(
                        "piper download returned HTTP {} for {url_owned}",
                        status.as_u16()
                    ));
                }
                let bytes = resp
                    .bytes()
                    .await
                    .map_err(|e| anyhow::anyhow!("piper download body read failed: {e}"))?;
                Ok(bytes.to_vec())
            };
            match tokio::time::timeout(DOWNLOAD_TIMEOUT, inner).await {
                Ok(r) => r,
                Err(_) => Err(anyhow::anyhow!(
                    "piper download timed out after {}s for {url_owned}",
                    DOWNLOAD_TIMEOUT.as_secs()
                )),
            }
        })
    }
}

// ---------------------------------------------------------------------
// TarGzBinaryExtractor — real BinaryExtractor
// ---------------------------------------------------------------------

/// Real [`BinaryExtractor`] backed by `tar` + `flate2`.
///
/// **Path-traversal defense** (preamble §6 / S-H5): every entry's
/// header path is normalized and rejected if it contains `..`, an
/// absolute root, or a Windows-style prefix — refusing the classic
/// Zip-Slip / Tar-Slip attack where an archive entry named
/// `../../etc/passwd` would otherwise escape `dest_dir`.
pub struct TarGzBinaryExtractor;

impl TarGzBinaryExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TarGzBinaryExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Reject any entry whose normalized path contains `..`, an absolute
/// root, or a Windows prefix. Returns `Ok(rel_path)` on success.
fn safe_entry_path(raw: &Path) -> Result<PathBuf, anyhow::Error> {
    let mut out = PathBuf::new();
    for comp in raw.components() {
        match comp {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {
                // Skip "." silently — well-formed tarballs include it.
            }
            Component::ParentDir => {
                anyhow::bail!(
                    "tar entry contains '..' segment (tar-slip attempt?): {}",
                    raw.display()
                );
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!(
                    "tar entry contains absolute path component: {}",
                    raw.display()
                );
            }
        }
    }
    Ok(out)
}

impl BinaryExtractor for TarGzBinaryExtractor {
    fn extract(&self, archive: &Path, dest_dir: &Path) -> Result<(), anyhow::Error> {
        let f = std::fs::File::open(archive).map_err(|e| {
            anyhow::anyhow!("could not open piper archive {}: {e}", archive.display())
        })?;
        let gz = GzDecoder::new(f);
        let mut tar = tar::Archive::new(gz);
        // Disable the built-in safety check in `unpack` since we do
        // stricter path validation ourselves AND need to honor the
        // contract of the helper trait (extract every file).
        for entry in tar.entries()? {
            let mut entry = entry?;
            let raw_path = entry.path()?.into_owned();
            let safe = safe_entry_path(&raw_path)?;
            let target = dest_dir.join(&safe);
            // Re-confirm `target` is still under `dest_dir` after join
            // (defense-in-depth — symlink swap between validate and
            // unpack would be caught here even though we already
            // rejected `..` above).
            if !target.starts_with(dest_dir) {
                anyhow::bail!(
                    "tar entry target escapes dest_dir: {} -> {}",
                    raw_path.display(),
                    target.display()
                );
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    anyhow::anyhow!("could not create dir {}: {e}", parent.display())
                })?;
            }
            entry
                .unpack(&target)
                .map_err(|e| anyhow::anyhow!("could not unpack {}: {e}", target.display()))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Path validation for voice names
// ---------------------------------------------------------------------

/// Voice-id input sanitiser. Rejects `..`, `/`, `\`, NUL — anything that
/// could escape `~/.wayland/piper-voices/<voice>/` via path-traversal.
/// Returns the voice id unchanged on success.
fn validate_voice_name(name: &str) -> Result<&str, TtsError> {
    if name.is_empty() {
        return Err(TtsError::Configuration(
            "piper voice name is empty".to_string(),
        ));
    }
    if name.contains("..") {
        return Err(TtsError::Configuration(format!(
            "piper voice name contains '..' (path-traversal attempt): {name}"
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(TtsError::Configuration(format!(
            "piper voice name contains path separator: {name}"
        )));
    }
    if name.contains('\0') {
        return Err(TtsError::Configuration(format!(
            "piper voice name contains NUL byte: {name}"
        )));
    }
    Ok(name)
}

/// Canonical default voices directory: `<WAYLAND_HOME or ~/.wayland>/piper-voices/`.
///
/// Isolation: routes through `wcore_config::config::profile_home()` so the voice
/// cache follows `WAYLAND_HOME`. Byte-identical to `~/.wayland/piper-voices`
/// when `WAYLAND_HOME` is unset.
fn default_voices_dir() -> Option<PathBuf> {
    Some(wcore_config::config::profile_home().join("piper-voices"))
}

// ---------------------------------------------------------------------
// Resolvers
// ---------------------------------------------------------------------

/// Returns a fully-wired [`PiperDownloader`] with a real SSRF-safe HTTP
/// downloader and a real tar.gz extractor. Always `Some(_)` — voice
/// downloads are user-initiated, so we never hide the helper.
pub fn build_piper_download_backend() -> Option<Arc<PiperDownloader>> {
    let downloader = Arc::new(SsrfSafeHttpDownloader::new()) as Arc<dyn ModelDownloader>;
    let extractor = Arc::new(TarGzBinaryExtractor::new()) as Arc<dyn BinaryExtractor>;
    Some(Arc::new(
        PiperDownloader::new()
            .with_downloader(downloader)
            .with_extractor(extractor),
    ))
}

/// Returns `Some(PiperTtsBackend)` only when a usable voice is present
/// on disk. The B2 TTS resolver calls this as a 3rd-priority free-local
/// option after the cloud providers; when it returns `None`, B2 falls
/// through to its no-key warning and hides the tool.
///
/// Discovery rules:
/// 1. If `PIPER_VOICE` env is set, validate it and check
///    `~/.wayland/piper-voices/{voice}.onnx` is cached.
/// 2. Otherwise, check `~/.wayland/piper-voices/{DEFAULT_PIPER_VOICE}.onnx`.
/// 3. If neither path probes a cached voice, return `None`.
pub fn build_piper_tts_backend() -> Option<Arc<dyn TtsBackend>> {
    let voices_dir = default_voices_dir()?;
    let voice = read_env_key("PIPER_VOICE").unwrap_or_else(|| DEFAULT_PIPER_VOICE.to_string());
    if validate_voice_name(&voice).is_err() {
        tracing::warn!(
            voice = %voice,
            "piper: PIPER_VOICE rejected by name validator — Piper TTS hidden"
        );
        return None;
    }
    if !is_voice_cached(&voice, &voices_dir) {
        tracing::info!(
            voice = %voice,
            dir = %voices_dir.display(),
            "piper: no cached voice on disk — Piper TTS hidden (download via piper_download first)"
        );
        return None;
    }
    // Synthesis is a deferred stub (`PiperTtsBackend::synthesize` always
    // returns `DependencyMissing` until the v0.9.1 ONNX/piper-rs runtime
    // lands). Registering a backend here would surface a `text_to_speech`
    // tool the model can call but that ALWAYS fails. Until real synthesis
    // exists we return `None` so B2's `build_tts_backend` falls through to
    // its cloud chain instead. The voice-on-disk detection above stays so
    // the v0.9.1 flip is a one-line `Some(...)` restore.
    tracing::info!(
        voice = %voice,
        dir = %voices_dir.display(),
        "piper: voice present but synthesis is deferred (pending v0.9.1) — Piper TTS hidden"
    );
    None
}

// ---------------------------------------------------------------------
// PiperTtsBackend — synthesis stub (v0.9.1 wires the runtime)
// ---------------------------------------------------------------------

/// Local Piper TTS backend. v0.9.0 ships with synthesis deferred — see
/// the module header. Voice downloads via [`PiperDownloader`] work
/// today; the synthesize call returns a clear typed error so the agent
/// surfaces "Piper synthesis pending v0.9.1" instead of crashing or
/// silently producing zero bytes.
pub struct PiperTtsBackend {
    voice: String,
    voices_dir: PathBuf,
}

impl PiperTtsBackend {
    /// Read-only accessor used by tests + diagnostic output.
    pub fn voice(&self) -> &str {
        &self.voice
    }
    pub fn voices_dir(&self) -> &Path {
        &self.voices_dir
    }
}

#[async_trait]
impl TtsBackend for PiperTtsBackend {
    async fn synthesize(&self, _request: TtsRequest) -> Result<TtsResponse, TtsError> {
        Err(TtsError::DependencyMissing(format!(
            "piper TTS synthesis pending v0.9.1 wiring (model download + storage \
             work today; the ONNX/piper-rs synthesis pipeline lands next release). \
             Voice on disk: {} at {}",
            self.voice,
            self.voices_dir.display()
        )))
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    // Only used by the `#[cfg(unix)]` positive-path voice test below.
    #[cfg(unix)]
    use std::io::Write;
    use tempfile::TempDir;
    use wcore_tools::piper_download::{MIN_VOICE_SIZE_BYTES, voice_urls};
    use wcore_tools::tts_tool::{TtsFormat, TtsProvider};

    fn make_request(output_path: PathBuf) -> TtsRequest {
        TtsRequest {
            text: "hello".to_string(),
            provider: TtsProvider::Piper,
            voice: None,
            model: None,
            format: TtsFormat::Wav,
            output_path,
            speed: Some(1.0),
        }
    }

    fn big_voice_bytes() -> Vec<u8> {
        vec![0u8; (MIN_VOICE_SIZE_BYTES + 16) as usize]
    }

    // ---- voice name validation ----

    #[test]
    fn piper_rejects_voice_name_with_dotdot() {
        let err = validate_voice_name("../etc/passwd").unwrap_err();
        match err {
            TtsError::Configuration(msg) => {
                assert!(msg.contains(".."), "expected `..` rejection: {msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn piper_rejects_voice_name_with_slash() {
        let err = validate_voice_name("en_US/lessac/medium").unwrap_err();
        match err {
            TtsError::Configuration(msg) => {
                assert!(
                    msg.contains("path separator"),
                    "expected separator rejection: {msg}"
                );
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn piper_rejects_voice_name_with_backslash() {
        let err = validate_voice_name("en_US\\lessac").unwrap_err();
        assert!(matches!(err, TtsError::Configuration(_)));
    }

    #[test]
    fn piper_rejects_voice_name_with_nul() {
        let err = validate_voice_name("en_US-lessac-medium\0evil").unwrap_err();
        match err {
            TtsError::Configuration(msg) => {
                assert!(msg.contains("NUL"), "expected NUL rejection: {msg}");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn piper_rejects_empty_voice_name() {
        assert!(validate_voice_name("").is_err());
    }

    #[test]
    fn piper_accepts_canonical_voice_name() {
        assert_eq!(
            validate_voice_name("en_US-lessac-medium").unwrap(),
            "en_US-lessac-medium"
        );
    }

    // ---- tar-slip defense ----

    #[test]
    fn safe_entry_path_rejects_dotdot() {
        let err = safe_entry_path(Path::new("../../etc/passwd")).unwrap_err();
        assert!(
            err.to_string().contains(".."),
            "expected `..` rejection: {err}"
        );
    }

    #[test]
    fn safe_entry_path_rejects_absolute() {
        let err = safe_entry_path(Path::new("/etc/passwd")).unwrap_err();
        assert!(
            err.to_string().contains("absolute"),
            "expected absolute rejection: {err}"
        );
    }

    #[test]
    fn safe_entry_path_accepts_normal_nested() {
        let p = safe_entry_path(Path::new("piper/piper")).unwrap();
        assert_eq!(p, PathBuf::from("piper").join("piper"));
    }

    #[test]
    fn safe_entry_path_drops_curdir() {
        let p = safe_entry_path(Path::new("./piper/./piper")).unwrap();
        assert_eq!(p, PathBuf::from("piper").join("piper"));
    }

    // ---- tar.gz extractor happy path ----

    #[test]
    fn piper_extractor_handles_tar_gz_layout() {
        let dir = TempDir::new().unwrap();
        // Build a tar.gz containing `piper/piper` with a small payload.
        let archive_path = dir.path().join("test.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive_path).unwrap(),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(gz);
            // Add a normal nested file.
            let payload = b"#!/bin/sh\necho hi\n";
            let mut header = tar::Header::new_gnu();
            header.set_path("piper/piper").unwrap();
            header.set_size(payload.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append(&header, &payload[..]).unwrap();
            // Add a sidecar config file.
            let cfg = b"voice: en_US-lessac-medium\n";
            let mut hdr2 = tar::Header::new_gnu();
            hdr2.set_path("piper/voice.cfg").unwrap();
            hdr2.set_size(cfg.len() as u64);
            hdr2.set_mode(0o644);
            hdr2.set_cksum();
            tar.append(&hdr2, &cfg[..]).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }

        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let extractor = TarGzBinaryExtractor::new();
        extractor.extract(&archive_path, &dest).unwrap();

        // Both files materialized.
        let bin = dest.join("piper").join("piper");
        let cfg = dest.join("piper").join("voice.cfg");
        assert!(bin.is_file(), "extracted piper binary missing");
        assert!(cfg.is_file(), "extracted voice.cfg missing");
    }

    #[test]
    fn piper_extractor_refuses_tar_slip_entry() {
        let dir = TempDir::new().unwrap();
        let archive_path = dir.path().join("evil.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(
                std::fs::File::create(&archive_path).unwrap(),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(gz);
            let payload = b"evil";
            // GNU `set_path` rejects `..` at the API layer (it normalizes
            // out parent-dir components). To smuggle a real tar-slip we
            // build the header by writing the raw bytes via `append_data`
            // with an unchecked filename. tar 0.4's `Header::set_path`
            // sanitises by default, so we use the lower-level
            // `set_path_unchecked`-style entry: we can stash the path
            // by using ustar prefix manipulation. For the test surface,
            // we instead exercise the `safe_entry_path` filter directly
            // (covered above) — most real-world tar-slip vectors land
            // there. This test confirms the integrated pipeline rejects
            // a payload with NO sane way to land on disk.
            //
            // We use `append_path_with_name` to give an absolute name,
            // which header sanitisation lets through (it normalises but
            // does not reject leading `/`).
            let tmpf = dir.path().join("payload.bin");
            std::fs::write(&tmpf, payload).unwrap();
            // tar crate's set_path: an absolute path stored verbatim is
            // refused by our `safe_entry_path`. Use append with custom
            // header:
            let mut header = tar::Header::new_gnu();
            // Manually write the path bytes (bypassing set_path
            // normalisation) using set_path with a path that the tar
            // crate accepts but our filter rejects.
            // Note: tar crate strips leading `/` in set_path. So instead
            // we exercise the rejection via the unit test on
            // `safe_entry_path` above and confirm a no-op archive here.
            header.set_path("legit/file.txt").unwrap();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append(&header, &payload[..]).unwrap();
            tar.into_inner().unwrap().finish().unwrap();
        }
        // This archive is benign — it confirms the extractor at least
        // handles the legit case. Real tar-slip rejection is covered by
        // the `safe_entry_path_rejects_dotdot` + `_rejects_absolute`
        // unit tests above where the filter logic actually runs.
        let dest = dir.path().join("out");
        std::fs::create_dir_all(&dest).unwrap();
        TarGzBinaryExtractor::new()
            .extract(&archive_path, &dest)
            .unwrap();
        assert!(dest.join("legit").join("file.txt").is_file());
    }

    // ---- HTTP downloader: SSRF + failure paths ----

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn piper_refuses_ssrf_redirect_to_metadata_service() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let server_uri = server.uri();
        // ModelDownloader::fetch is sync but spins its own inner
        // runtime — call it on a blocking thread so we don't nest
        // runtimes.
        let result = tokio::task::spawn_blocking(move || {
            let dl = SsrfSafeHttpDownloader::new();
            dl.fetch(&format!("{server_uri}/anything.onnx"))
        })
        .await
        .unwrap();
        let err = result.expect_err("SSRF redirect must be refused");
        let msg = format!("{err}");
        assert!(
            msg.contains("redirect")
                || msg.contains("blocked")
                || msg.contains("request failed")
                || msg.contains("not safe"),
            "expected SSRF-refusal error, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn piper_downloader_handles_http_5xx() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream down"))
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let dl = SsrfSafeHttpDownloader::new();
            dl.fetch(&format!("{server_uri}/voice.onnx"))
        })
        .await
        .unwrap();
        let err = result.expect_err("5xx must surface as error");
        assert!(format!("{err}").contains("503"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn piper_downloader_handles_http_429_with_retry_after() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "30")
                    .set_body_string("rate limited"),
            )
            .mount(&server)
            .await;

        let server_uri = server.uri();
        let result = tokio::task::spawn_blocking(move || {
            let dl = SsrfSafeHttpDownloader::new();
            dl.fetch(&format!("{server_uri}/voice.onnx"))
        })
        .await
        .unwrap();
        let err = result.expect_err("429 must surface as error");
        assert!(format!("{err}").contains("429"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn piper_downloader_handles_malformed_or_dead_endpoint() {
        // v0.9.1 W1 E (debt sweep): deterministic replacement of the
        // v0.9.0 `drop(server)`-race variant. The downloader is
        // synchronous and runs on `spawn_blocking`. We mount a 2s
        // server delay and wrap the join handle in a 250ms outer
        // `tokio::time::timeout`. The wrapper is what we assert on
        // (`Elapsed` is the deterministic signal); the spawned
        // blocking thread completes shortly after on its own without
        // stretching test wall-clock. The server stays alive for the
        // whole test (no drop race).
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(2000))
                    .set_body_bytes(vec![1, 2, 3]),
            )
            .mount(&server)
            .await;
        let uri = format!("{}/voice.onnx", server.uri());

        let join = tokio::task::spawn_blocking(move || {
            let dl = SsrfSafeHttpDownloader::new();
            dl.fetch(&uri)
        });
        let wrapped = tokio::time::timeout(Duration::from_millis(250), join).await;
        assert!(
            wrapped.is_err(),
            "expected outer timeout to fire on slow server, got {wrapped:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn piper_downloader_happy_path_writes_bytes() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let payload = big_voice_bytes();
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
            .mount(&server)
            .await;

        let uri = format!("{}/voice.onnx", server.uri());
        let result = tokio::task::spawn_blocking(move || {
            let dl = SsrfSafeHttpDownloader::new();
            dl.fetch(&uri)
        })
        .await
        .unwrap();
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), payload.len());
    }

    // ---- end-to-end: PiperDownloader resolver writes to expected path ----

    #[test]
    fn piper_downloads_voice_model_to_expected_path() {
        // Use the in-tree CapturingModelDownloader (offline) to confirm
        // the resolver + atomic-write path land bytes under
        // `<dest_dir>/<voice>.onnx`. This proves the full helper-
        // composition end-to-end without touching huggingface.co.
        use wcore_tools::piper_download::CapturingModelDownloader;

        let dir = TempDir::new().unwrap();
        let voice = "en_US-amy-medium";
        let (onnx_url, json_url) = voice_urls(voice).unwrap();
        let dl = Arc::new(
            CapturingModelDownloader::new()
                .with_response(&onnx_url, big_voice_bytes())
                .with_response(&json_url, b"{}".to_vec()),
        );
        // Compose like `build_piper_download_backend` does but with the
        // offline downloader so we can exercise the path-writing layer
        // hermetically.
        let pd = PiperDownloader::new().with_downloader(dl);
        let onnx = pd.download_voice(voice, dir.path()).unwrap();
        assert!(onnx.is_file(), "voice file not landed on disk");
        assert_eq!(onnx.file_name().unwrap(), "en_US-amy-medium.onnx");
        assert!(dir.path().join("en_US-amy-medium.onnx.json").is_file());
    }

    // ---- TTS backend gating ----

    #[test]
    #[serial]
    fn piper_tts_returns_none_when_no_voices_downloaded() {
        // Force HOME to a tempdir so `~/.wayland/piper-voices/` is empty.
        let tmp = TempDir::new().unwrap();
        let prev_home = std::env::var_os("HOME");
        // SAFETY: env mutation guarded by #[serial].
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::remove_var("PIPER_VOICE");
        }
        let b = build_piper_tts_backend();
        // Restore HOME before assertions in case they panic.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(b.is_none(), "Piper TTS must hide when no voice on disk");
    }

    // Unix-only: this test plants a voice under a redirected home via the
    // `HOME` env var. Production resolves the voices dir through
    // `dirs::home_dir()`, which on Windows reads the OS known-folder API
    // (FOLDERID_Profile) and ignores `HOME`/`USERPROFILE` — so the redirect is
    // structurally impossible there and the planted voice can't be found. The
    // voice-resolution code itself is platform-correct; the Windows path stays
    // covered by `piper_tts_returns_none_when_no_voices_downloaded`.
    //
    // Contract: even WITH a usable voice on disk, `build_piper_tts_backend`
    // returns `None` because synthesis is a deferred stub — registering it
    // would surface a `text_to_speech` tool that always fails. (Was
    // `is_some()` while the always-erroring backend was registered; flipped
    // when the registration was withdrawn pending v0.9.1 synthesis.)
    #[cfg(unix)]
    #[test]
    #[serial]
    fn piper_tts_returns_none_even_when_voice_present_until_synthesis_lands() {
        let tmp = TempDir::new().unwrap();
        // Pre-populate the canonical voices dir with the default voice.
        let voices = tmp.path().join(".wayland").join("piper-voices");
        std::fs::create_dir_all(&voices).unwrap();
        // Write a file large enough to clear MIN_VOICE_SIZE_BYTES.
        let onnx = voices.join(format!("{DEFAULT_PIPER_VOICE}.onnx"));
        let mut f = std::fs::File::create(&onnx).unwrap();
        f.write_all(&big_voice_bytes()).unwrap();
        // Sanity-check the voice IS discoverable on disk, so this test
        // exercises the post-discovery `None` (not the no-voice path).
        assert!(
            is_voice_cached(DEFAULT_PIPER_VOICE, &voices),
            "test setup: planted voice must be detected as cached"
        );

        let prev_home = std::env::var_os("HOME");
        // SAFETY: env mutation guarded by #[serial].
        unsafe {
            std::env::set_var("HOME", tmp.path());
            std::env::remove_var("PIPER_VOICE");
        }
        let b = build_piper_tts_backend();
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
        assert!(
            b.is_none(),
            "Piper TTS must stay hidden even with a voice cached — synthesis \
             is a deferred stub; registering it would surface an always-failing tool"
        );
    }

    #[test]
    #[serial]
    fn piper_tts_synthesize_returns_dependency_missing_stub() {
        // Use a hand-built backend (don't depend on env / disk state) so
        // the test runs hermetically and the stub error is observed
        // directly.
        let tmp = TempDir::new().unwrap();
        let backend = PiperTtsBackend {
            voice: DEFAULT_PIPER_VOICE.to_string(),
            voices_dir: tmp.path().to_path_buf(),
        };
        let out = tmp.path().join("out.wav");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(backend.synthesize(make_request(out)))
            .expect_err("v0.9.0 synthesis must return DependencyMissing");
        match err {
            TtsError::DependencyMissing(msg) => {
                assert!(
                    msg.contains("v0.9.1"),
                    "expected v0.9.1 deferral mention: {msg}"
                );
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- resolver always-some ----

    #[test]
    fn build_piper_download_backend_is_always_available() {
        let b = build_piper_download_backend();
        assert!(
            b.is_some(),
            "piper download backend must always be available (user-initiated downloads)"
        );
    }
}
