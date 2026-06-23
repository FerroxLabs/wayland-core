//! T3-3.8 (sub-wave 8): Piper voice + binary downloader (HELPER).
//!
//! Ported from the prior Wayland Python engine.
//!
//! The Python original performs three jobs:
//!
//! 1. **Voice download** — fetch `{voice_id}.onnx` (and best-effort
//!    `.onnx.json` metadata) from the Hugging Face
//!    `rhasspy/piper-voices` repo, atomically (write `.tmp`, rename on
//!    success) with a minimum-size sanity check that catches 404 HTML
//!    pages returned with a 200 status.
//! 2. **Binary download** — fetch the platform-specific Piper release
//!    tarball from the `rhasspy/piper` GitHub release, extract it, and
//!    chmod 0o755 the `piper` executable.
//! 3. **Cache probe** — `is_voice_cached` returns true when the
//!    `.onnx` is present and meets the minimum-size bound.
//!
//! ## Why this is a HELPER, not a tool
//!
//! `tts_tool` (already merged) explicitly declares Piper voice / binary
//! download to be **the backend's responsibility** — see the module
//! header of `tts_tool.rs`:
//!
//! > Every concrete provider (edge_tts, `elevenlabs`, `openai`,
//! > `mistralai`, raw httpx for MiniMax / xAI, subprocess-spawned
//! > `piper`) is the backend's responsibility.
//!
//! So this module is a self-contained helper a backend can pick up
//! when it wants to host the download path inside the engine process,
//! rather than a tool surfaced through the dispatcher.
//!
//! ## Differences vs Python
//!
//! * **Pluggable downloader.** The Python source calls
//!   `urllib.request.urlretrieve` directly. The Rust port injects a
//!   [`ModelDownloader`] trait so the helper is testable offline. The
//!   crate ships:
//!   * [`NullModelDownloader`] — fail-loud default, returns an error.
//!   * [`CapturingModelDownloader`] — test double that returns
//!     pre-recorded bytes per URL and records the URLs it was asked
//!     for.
//!
//!     A production caller injects an HTTP-backed implementation
//!     from the host (the engine itself does not add a new HTTP-
//!     client dep for this helper).
//!
//! * **Pluggable archive extractor.** Same rationale — `tarfile` /
//!   `gzip` would require pulling `tar` + `flate2` into
//!   `wcore-tools`. The helper takes a [`BinaryExtractor`] so the
//!   host wires its preferred extractor (or shells out to the system
//!   `tar` via `wcore-config::shell`). The crate ships a
//!   [`NullBinaryExtractor`] (fail-loud) and a
//!   [`CapturingBinaryExtractor`] (test double that materializes a
//!   pre-recorded set of files).
//!
//! * **Atomic write.** Matches Python's `.tmp` + rename semantics
//!   exactly, and on failure removes the leftover `.tmp` file (best
//!   effort, matches Python).
//!
//! * **No silent success.** Python suppresses metadata-JSON download
//!   failure with a warning. Rust port matches this exactly — the
//!   `.onnx.json` is treated as optional metadata and a download
//!   failure for it is logged via [`tracing::warn!`] but does not
//!   propagate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;

/// Pinned Piper release version. Matches the Python constant
/// `PIPER_VERSION` so URL composition is byte-identical.
pub const PIPER_VERSION: &str = "2023.11.14-2";

/// Base URL for Piper binary GitHub release archives. Composed with
/// [`PIPER_VERSION`] to match Python's `PIPER_BINARY_BASE`.
pub fn piper_binary_base() -> String {
    format!(
        "https://github.com/rhasspy/piper/releases/download/{}",
        PIPER_VERSION
    )
}

/// Base URL for Piper voice files on the Hugging Face
/// `rhasspy/piper-voices` repo. Matches Python `PIPER_VOICE_BASE`.
pub const PIPER_VOICE_BASE: &str = "https://huggingface.co/rhasspy/piper-voices/resolve/main";

/// Minimum acceptable `.onnx` file size in bytes. Real Piper voices
/// are ~60 MB; 1 MB is a generous lower bound that catches the
/// common failure mode of a 404 HTML page being downloaded with a
/// 200 status code. Matches Python `MIN_VOICE_SIZE_BYTES`.
pub const MIN_VOICE_SIZE_BYTES: u64 = 1_000_000;

/// Errors surfaced by the Piper download helper.
#[derive(Debug, Error)]
pub enum PiperError {
    #[error(
        "Invalid voice_id '{voice_id}'. Expected format: <country_code>-<name>-<quality> \
         (e.g. en_US-lessac-medium)"
    )]
    InvalidVoiceId { voice_id: String },

    #[error("Downloader is not bound. {0}")]
    DownloaderUnbound(&'static str),

    #[error("Extractor is not bound. {0}")]
    ExtractorUnbound(&'static str),

    #[error("Downloader returned an error for {url}: {source}")]
    Download {
        url: String,
        #[source]
        source: anyhow::Error,
    },

    #[error(
        "Downloaded voice file for '{voice_id}' is only {actual} bytes \
         (expected >= {min}). URL may be invalid: {url}"
    )]
    VoiceTooSmall {
        voice_id: String,
        actual: u64,
        min: u64,
        url: String,
    },

    #[error("Piper binary archive too small ({actual} bytes). URL may be invalid: {url}")]
    BinaryArchiveTooSmall { actual: u64, url: String },

    #[error("Could not find piper binary inside archive from {url}")]
    BinaryMissingInArchive { url: String },

    #[error("Unsupported platform: {platform}/{machine}")]
    UnsupportedPlatform { platform: String, machine: String },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Archive extraction failed for {archive}: {source}")]
    Extraction {
        archive: PathBuf,
        #[source]
        source: anyhow::Error,
    },
}

impl PiperError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Parsed components of a voice_id.
///
/// `voice_id` format is `{country_code}-{name}-{quality}` —
/// e.g. `en_US-lessac-medium` → (lang="en", country="en_US",
/// name="lessac", quality="medium"). The leading `lang` is taken as
/// the first `_`-delimited prefix of `country_code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceIdParts {
    pub lang: String,
    pub country: String,
    pub name: String,
    pub quality: String,
}

/// Parse a voice_id into its URL-path components. Matches the
/// Python `_voice_id_to_url_parts` exactly.
pub fn voice_id_to_url_parts(voice_id: &str) -> Result<VoiceIdParts, PiperError> {
    // Match Python's `.split("-", 2)`: split into at-most 3 parts.
    let mut parts = voice_id.splitn(3, '-');
    let country = parts.next();
    let name = parts.next();
    let quality = parts.next();
    let (country, name, quality) = match (country, name, quality) {
        (Some(c), Some(n), Some(q)) if !c.is_empty() && !n.is_empty() && !q.is_empty() => (c, n, q),
        _ => {
            return Err(PiperError::InvalidVoiceId {
                voice_id: voice_id.to_string(),
            });
        }
    };
    // Per Python: lang = country_code.split("_")[0]
    let lang = country.split('_').next().unwrap_or(country).to_string();
    Ok(VoiceIdParts {
        lang,
        country: country.to_string(),
        name: name.to_string(),
        quality: quality.to_string(),
    })
}

/// Compose the (onnx_url, onnx_json_url) pair for a voice_id.
/// Matches Python `_voice_urls`.
pub fn voice_urls(voice_id: &str) -> Result<(String, String), PiperError> {
    let p = voice_id_to_url_parts(voice_id)?;
    let base = format!(
        "{}/{}/{}/{}/{}/{}",
        PIPER_VOICE_BASE, p.lang, p.country, p.name, p.quality, voice_id
    );
    Ok((format!("{base}.onnx"), format!("{base}.onnx.json")))
}

/// Trait abstracting the network fetch. Implementations return the
/// raw bytes for `url` or an error.
pub trait ModelDownloader: Send + Sync {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, anyhow::Error>;
}

/// Fail-loud default. Calling `fetch` always returns an error —
/// production callers MUST inject a real downloader.
pub struct NullModelDownloader;

impl ModelDownloader for NullModelDownloader {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, anyhow::Error> {
        anyhow::bail!(
            "NullModelDownloader is fail-loud — host did not inject a Piper \
             model downloader. Attempted URL: {url}"
        );
    }
}

/// In-memory test double. Returns pre-recorded bytes per URL and
/// records every URL it was asked for (oldest first).
pub struct CapturingModelDownloader {
    responses: std::collections::HashMap<String, Vec<u8>>,
    requested: Mutex<Vec<String>>,
}

impl CapturingModelDownloader {
    pub fn new() -> Self {
        Self {
            responses: std::collections::HashMap::new(),
            requested: Mutex::new(Vec::new()),
        }
    }

    /// Register the response for `url`. Returns `self` for chaining.
    pub fn with_response(mut self, url: impl Into<String>, body: Vec<u8>) -> Self {
        self.responses.insert(url.into(), body);
        self
    }

    /// Snapshot of all URLs the helper has asked for, in order.
    pub fn requested(&self) -> Vec<String> {
        self.requested.lock().clone()
    }
}

impl Default for CapturingModelDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelDownloader for CapturingModelDownloader {
    fn fetch(&self, url: &str) -> Result<Vec<u8>, anyhow::Error> {
        self.requested.lock().push(url.to_string());
        match self.responses.get(url) {
            Some(bytes) => Ok(bytes.clone()),
            None => anyhow::bail!("CapturingModelDownloader has no canned response for {url}"),
        }
    }
}

/// Trait abstracting binary archive extraction so the helper does
/// not need to pull `tar` / `flate2` into `wcore-tools`.
///
/// Contract: implementations extract the gzipped tarball at
/// `archive` into `dest_dir`, materializing every file found
/// inside. The helper then probes for `piper/piper` or `piper`
/// inside `dest_dir` (matching Python `candidate_paths`).
pub trait BinaryExtractor: Send + Sync {
    fn extract(&self, archive: &Path, dest_dir: &Path) -> Result<(), anyhow::Error>;
}

/// Fail-loud default extractor.
pub struct NullBinaryExtractor;

impl BinaryExtractor for NullBinaryExtractor {
    fn extract(&self, archive: &Path, _: &Path) -> Result<(), anyhow::Error> {
        anyhow::bail!(
            "NullBinaryExtractor is fail-loud — host did not inject a Piper \
             archive extractor. Archive: {}",
            archive.display()
        );
    }
}

/// Test double for [`BinaryExtractor`] — when invoked, materializes
/// every (relative-path, bytes) pair from `files` under
/// `dest_dir`.
pub struct CapturingBinaryExtractor {
    files: Vec<(PathBuf, Vec<u8>)>,
    invocations: Mutex<Vec<(PathBuf, PathBuf)>>,
}

impl CapturingBinaryExtractor {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            invocations: Mutex::new(Vec::new()),
        }
    }

    /// Register a file to materialize on extract.
    pub fn with_file(mut self, rel_path: impl Into<PathBuf>, bytes: Vec<u8>) -> Self {
        self.files.push((rel_path.into(), bytes));
        self
    }

    pub fn invocations(&self) -> Vec<(PathBuf, PathBuf)> {
        self.invocations.lock().clone()
    }
}

impl Default for CapturingBinaryExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl BinaryExtractor for CapturingBinaryExtractor {
    fn extract(&self, archive: &Path, dest_dir: &Path) -> Result<(), anyhow::Error> {
        self.invocations
            .lock()
            .push((archive.to_path_buf(), dest_dir.to_path_buf()));
        for (rel, bytes) in &self.files {
            let abs = dest_dir.join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&abs, bytes)?;
        }
        Ok(())
    }
}

/// Per-process default platform string matching the filename
/// convention of Piper GitHub releases (`piper_<platform>.tar.gz`).
/// Mirrors Python `_detect_platform`.
pub fn detect_platform() -> Result<String, PiperError> {
    let os = std::env::consts::OS; // "macos", "linux", "windows", ...
    let arch = std::env::consts::ARCH; // "aarch64", "x86_64", ...
    match (os, arch) {
        ("macos", "aarch64") => Ok("macos_aarch64".to_string()),
        ("macos", _) => Ok("macos_x64".to_string()),
        ("linux", "aarch64") => Ok("linux_aarch64".to_string()),
        ("linux", _) => Ok("linux_x64".to_string()),
        (p, m) => Err(PiperError::UnsupportedPlatform {
            platform: p.to_string(),
            machine: m.to_string(),
        }),
    }
}

/// True if `dest_dir / {voice_id}.onnx` exists AND its size is at
/// least [`MIN_VOICE_SIZE_BYTES`]. Matches Python `is_voice_cached`.
pub fn is_voice_cached(voice_id: &str, dest_dir: &Path) -> bool {
    let path = dest_dir.join(format!("{voice_id}.onnx"));
    match std::fs::metadata(&path) {
        Ok(m) => m.is_file() && m.len() >= MIN_VOICE_SIZE_BYTES,
        Err(_) => false,
    }
}

/// Atomic write: writes `bytes` to `dest.with_extension(...".tmp")`
/// then renames to `dest`. On failure, removes the `.tmp` file
/// (best-effort). Matches the contract of Python `_atomic_download`.
fn atomic_write(dest: &Path, bytes: &[u8]) -> Result<(), PiperError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PiperError::io(parent, e))?;
    }
    // Build the `.tmp` sibling — match Python `dest.with_suffix(dest.suffix + ".tmp")`.
    let tmp_name = match dest.file_name() {
        Some(n) => {
            let mut s = n.to_os_string();
            s.push(".tmp");
            s
        }
        None => {
            return Err(PiperError::io(
                dest,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "destination has no file name",
                ),
            ));
        }
    };
    let tmp = dest.with_file_name(tmp_name);
    if let Err(e) = std::fs::write(&tmp, bytes) {
        let _ = std::fs::remove_file(&tmp);
        return Err(PiperError::io(tmp, e));
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(PiperError::io(dest, e));
    }
    Ok(())
}

/// Download voice `.onnx` (and best-effort `.onnx.json` metadata) to
/// `dest_dir`. Idempotent — returns immediately when the cache is
/// already populated.
///
/// Returns the absolute path of the `.onnx` file.
///
/// Errors:
/// * [`PiperError::InvalidVoiceId`] if the id can't be parsed.
/// * [`PiperError::Download`] if the network fetch failed.
/// * [`PiperError::VoiceTooSmall`] if the downloaded onnx is below
///   [`MIN_VOICE_SIZE_BYTES`] (likely a 404 HTML page). The bogus
///   file is removed before returning.
/// * [`PiperError::Io`] for fs errors.
pub fn download_voice(
    voice_id: &str,
    dest_dir: &Path,
    downloader: &dyn ModelDownloader,
) -> Result<PathBuf, PiperError> {
    let onnx_path = dest_dir.join(format!("{voice_id}.onnx"));
    let json_path = dest_dir.join(format!("{voice_id}.onnx.json"));

    // Fast-path: both files already present and onnx large enough.
    let onnx_meta = std::fs::metadata(&onnx_path).ok();
    let json_exists = json_path.exists();
    if let Some(m) = &onnx_meta
        && m.len() >= MIN_VOICE_SIZE_BYTES
        && json_exists
    {
        tracing::debug!(
            voice_id,
            path = %onnx_path.display(),
            "Piper voice already cached"
        );
        return Ok(onnx_path);
    }

    let (onnx_url, json_url) = voice_urls(voice_id)?;

    let onnx_below_min = match &onnx_meta {
        Some(m) => m.len() < MIN_VOICE_SIZE_BYTES,
        None => true,
    };
    if onnx_below_min {
        tracing::info!(
            voice_id,
            url = %onnx_url,
            dest = %onnx_path.display(),
            "Downloading Piper voice .onnx"
        );
        let bytes = downloader
            .fetch(&onnx_url)
            .map_err(|source| PiperError::Download {
                url: onnx_url.clone(),
                source,
            })?;
        let size = bytes.len() as u64;
        if size < MIN_VOICE_SIZE_BYTES {
            // Python writes the file first, then removes it on
            // size-check failure. We match the *post-condition* (the
            // bogus file is not left on disk) by skipping the write
            // entirely when it would be too small.
            return Err(PiperError::VoiceTooSmall {
                voice_id: voice_id.to_string(),
                actual: size,
                min: MIN_VOICE_SIZE_BYTES,
                url: onnx_url,
            });
        }
        atomic_write(&onnx_path, &bytes)?;
    }

    if !json_path.exists() {
        match downloader.fetch(&json_url) {
            Ok(bytes) => {
                // Metadata may be tiny — no size gate.
                if let Err(e) = atomic_write(&json_path, &bytes) {
                    tracing::warn!(
                        voice_id,
                        error = %e,
                        "Could not write voice JSON (treated as optional metadata)"
                    );
                }
            }
            Err(e) => {
                // Optional metadata — log but do NOT propagate, matches Python.
                tracing::warn!(
                    voice_id,
                    error = %e,
                    "Could not download voice JSON (treated as optional metadata)"
                );
            }
        }
    }

    Ok(onnx_path)
}

/// Download the Piper binary tarball for `platform` (or detected
/// platform if `None`), extract via `extractor`, install the
/// executable at `dest_dir / piper`, and return its path. Matches
/// Python `download_binary`.
///
/// Idempotent: returns immediately when `dest_dir / piper` already
/// exists (executable bit not re-checked on Windows where the
/// concept doesn't apply).
pub fn download_binary(
    platform: Option<&str>,
    dest_dir: &Path,
    downloader: &dyn ModelDownloader,
    extractor: &dyn BinaryExtractor,
) -> Result<PathBuf, PiperError> {
    let platform_owned;
    let platform = match platform {
        Some(p) => p,
        None => {
            platform_owned = detect_platform()?;
            platform_owned.as_str()
        }
    };

    let binary_path = dest_dir.join("piper");
    if binary_path.is_file() {
        tracing::debug!(
            path = %binary_path.display(),
            "Piper binary already installed"
        );
        return Ok(binary_path);
    }

    let archive_name = format!("piper_{platform}.tar.gz");
    let url = format!("{}/{}", piper_binary_base(), archive_name);
    let archive_path = dest_dir.join(&archive_name);

    std::fs::create_dir_all(dest_dir).map_err(|e| PiperError::io(dest_dir, e))?;

    let bytes = downloader
        .fetch(&url)
        .map_err(|source| PiperError::Download {
            url: url.clone(),
            source,
        })?;
    let size = bytes.len() as u64;
    if size < MIN_VOICE_SIZE_BYTES {
        return Err(PiperError::BinaryArchiveTooSmall { actual: size, url });
    }
    atomic_write(&archive_path, &bytes)?;

    let extract_dir = dest_dir.join("piper_extract_tmp");
    std::fs::create_dir_all(&extract_dir).map_err(|e| PiperError::io(&extract_dir, e))?;

    let extract_result = extractor.extract(&archive_path, &extract_dir);
    // Always remove the archive whether extraction succeeded or not
    // — matches Python `finally: archive_path.unlink(missing_ok=True)`.
    let _ = std::fs::remove_file(&archive_path);
    extract_result.map_err(|source| PiperError::Extraction {
        archive: archive_path,
        source,
    })?;

    // Match Python `candidate_paths`: first `piper/piper`, then bare `piper`.
    let candidate_paths = [
        extract_dir.join("piper").join("piper"),
        extract_dir.join("piper"),
    ];
    let mut extracted_bin: Option<PathBuf> = None;
    for c in &candidate_paths {
        if c.is_file() {
            extracted_bin = Some(c.clone());
            break;
        }
    }
    let extracted_bin = match extracted_bin {
        Some(b) => b,
        None => {
            let _ = std::fs::remove_dir_all(&extract_dir);
            return Err(PiperError::BinaryMissingInArchive { url });
        }
    };

    // Best-effort chmod 0o755 on unix; no-op elsewhere.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mut perms) = std::fs::metadata(&extracted_bin).map(|m| m.permissions()) {
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&extracted_bin, perms);
        }
    }

    std::fs::rename(&extracted_bin, &binary_path).map_err(|e| {
        let _ = std::fs::remove_dir_all(&extract_dir);
        PiperError::io(&binary_path, e)
    })?;

    let _ = std::fs::remove_dir_all(&extract_dir);

    tracing::info!(
        path = %binary_path.display(),
        "Piper binary installed"
    );
    Ok(binary_path)
}

/// Thin wrapper that owns an `Arc<dyn ModelDownloader>` plus
/// `Arc<dyn BinaryExtractor>` and exposes the three public ops as
/// instance methods. Mirrors the host-facing surface used by tools
/// that hold a backend behind a trait object.
pub struct PiperDownloader {
    downloader: Arc<dyn ModelDownloader>,
    extractor: Arc<dyn BinaryExtractor>,
}

impl PiperDownloader {
    /// Construct with the fail-loud defaults. Callers MUST replace
    /// at least the downloader before calling `download_voice` or
    /// `download_binary`.
    pub fn new() -> Self {
        Self {
            downloader: Arc::new(NullModelDownloader),
            extractor: Arc::new(NullBinaryExtractor),
        }
    }

    pub fn with_downloader(mut self, d: Arc<dyn ModelDownloader>) -> Self {
        self.downloader = d;
        self
    }

    pub fn with_extractor(mut self, e: Arc<dyn BinaryExtractor>) -> Self {
        self.extractor = e;
        self
    }

    pub fn is_voice_cached(&self, voice_id: &str, dest_dir: &Path) -> bool {
        is_voice_cached(voice_id, dest_dir)
    }

    pub fn download_voice(&self, voice_id: &str, dest_dir: &Path) -> Result<PathBuf, PiperError> {
        download_voice(voice_id, dest_dir, self.downloader.as_ref())
    }

    pub fn download_binary(
        &self,
        platform: Option<&str>,
        dest_dir: &Path,
    ) -> Result<PathBuf, PiperError> {
        download_binary(
            platform,
            dest_dir,
            self.downloader.as_ref(),
            self.extractor.as_ref(),
        )
    }
}

impl Default for PiperDownloader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn big_bytes() -> Vec<u8> {
        // Just over MIN_VOICE_SIZE_BYTES so the size gate passes.
        vec![0u8; (MIN_VOICE_SIZE_BYTES + 16) as usize]
    }

    // --- voice_id parsing ---

    #[test]
    fn voice_id_parses_canonical() {
        let p = voice_id_to_url_parts("en_US-lessac-medium").unwrap();
        assert_eq!(p.lang, "en");
        assert_eq!(p.country, "en_US");
        assert_eq!(p.name, "lessac");
        assert_eq!(p.quality, "medium");
    }

    #[test]
    fn voice_id_parses_name_with_dash_via_split_2() {
        // Python uses split("-", 2): the third chunk keeps any extra dashes.
        let p = voice_id_to_url_parts("en_GB-jenny_dioco-medium").unwrap();
        assert_eq!(p.lang, "en");
        assert_eq!(p.country, "en_GB");
        assert_eq!(p.name, "jenny_dioco");
        assert_eq!(p.quality, "medium");
    }

    #[test]
    fn voice_id_rejects_missing_segment() {
        let err = voice_id_to_url_parts("en_US-lessac").unwrap_err();
        match err {
            PiperError::InvalidVoiceId { voice_id } => {
                assert_eq!(voice_id, "en_US-lessac");
            }
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn voice_urls_match_python_layout() {
        let (onnx, json) = voice_urls("en_US-lessac-medium").unwrap();
        assert_eq!(
            onnx,
            "https://huggingface.co/rhasspy/piper-voices/resolve/main/\
             en/en_US/lessac/medium/en_US-lessac-medium.onnx"
        );
        assert_eq!(
            json,
            "https://huggingface.co/rhasspy/piper-voices/resolve/main/\
             en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"
        );
    }

    // --- Null downloader fails loud ---

    #[test]
    fn null_downloader_fails_loud() {
        let dl = NullModelDownloader;
        let err = dl.fetch("https://example.com/x").unwrap_err();
        assert!(
            err.to_string().contains("fail-loud"),
            "expected fail-loud, got: {err}"
        );
    }

    #[test]
    fn null_extractor_fails_loud() {
        let ex = NullBinaryExtractor;
        let err = ex
            .extract(Path::new("/tmp/a.tar.gz"), Path::new("/tmp/out"))
            .unwrap_err();
        assert!(
            err.to_string().contains("fail-loud"),
            "expected fail-loud, got: {err}"
        );
    }

    // --- is_voice_cached ---

    #[test]
    fn is_voice_cached_false_when_missing() {
        let dir = tempdir().unwrap();
        assert!(!is_voice_cached("en_US-lessac-medium", dir.path()));
    }

    #[test]
    fn is_voice_cached_false_when_too_small() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("en_US-lessac-medium.onnx");
        std::fs::write(&p, b"tiny").unwrap();
        assert!(!is_voice_cached("en_US-lessac-medium", dir.path()));
    }

    #[test]
    fn is_voice_cached_true_when_large_enough() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("en_US-lessac-medium.onnx");
        std::fs::write(&p, big_bytes()).unwrap();
        assert!(is_voice_cached("en_US-lessac-medium", dir.path()));
    }

    // --- download_voice happy path ---

    #[test]
    fn download_voice_writes_files_atomically() {
        let dir = tempdir().unwrap();
        let (onnx_url, json_url) = voice_urls("en_US-lessac-medium").unwrap();
        let dl = CapturingModelDownloader::new()
            .with_response(&onnx_url, big_bytes())
            .with_response(&json_url, b"{\"meta\":true}".to_vec());
        let path = download_voice("en_US-lessac-medium", dir.path(), &dl).unwrap();
        assert!(path.is_file());
        assert_eq!(path.file_name().unwrap(), "en_US-lessac-medium.onnx");
        assert!(dir.path().join("en_US-lessac-medium.onnx.json").is_file());
        // No .tmp residue:
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let name = entry.unwrap().file_name();
            let s = name.to_string_lossy();
            assert!(!s.ends_with(".tmp"), "stray .tmp file: {s}");
        }
        // Both URLs were requested in the canonical order.
        let req = dl.requested();
        assert_eq!(req, vec![onnx_url, json_url]);
    }

    #[test]
    fn download_voice_is_idempotent_when_cached() {
        let dir = tempdir().unwrap();
        // Pre-populate cache with both files.
        let onnx = dir.path().join("en_US-lessac-medium.onnx");
        let json = dir.path().join("en_US-lessac-medium.onnx.json");
        std::fs::write(&onnx, big_bytes()).unwrap();
        std::fs::write(&json, b"{}").unwrap();

        // Null downloader would fail loud — proves no network was hit.
        let path = download_voice("en_US-lessac-medium", dir.path(), &NullModelDownloader).unwrap();
        assert_eq!(path, onnx);
    }

    #[test]
    fn download_voice_rejects_too_small_response() {
        let dir = tempdir().unwrap();
        let (onnx_url, _json_url) = voice_urls("en_US-lessac-medium").unwrap();
        let dl = CapturingModelDownloader::new().with_response(&onnx_url, b"404 page".to_vec());
        let err = download_voice("en_US-lessac-medium", dir.path(), &dl).unwrap_err();
        match err {
            PiperError::VoiceTooSmall { actual, min, .. } => {
                assert!(actual < min);
            }
            _ => panic!("wrong variant: {err:?}"),
        }
        // Bogus file must NOT have been written.
        assert!(!dir.path().join("en_US-lessac-medium.onnx").exists());
    }

    #[test]
    fn download_voice_succeeds_even_when_json_fetch_fails() {
        // Matches Python: .onnx.json is optional metadata.
        let dir = tempdir().unwrap();
        let (onnx_url, _json_url) = voice_urls("en_US-lessac-medium").unwrap();
        // Capturing downloader has NO json response — fetch will error.
        let dl = CapturingModelDownloader::new().with_response(&onnx_url, big_bytes());
        let path = download_voice("en_US-lessac-medium", dir.path(), &dl).unwrap();
        assert!(path.is_file());
        assert!(!dir.path().join("en_US-lessac-medium.onnx.json").exists());
    }

    // --- download_binary happy path ---

    #[test]
    fn download_binary_happy_path() {
        let dir = tempdir().unwrap();
        let archive_name = "piper_macos_aarch64.tar.gz";
        let url = format!("{}/{}", piper_binary_base(), archive_name);
        let dl = CapturingModelDownloader::new().with_response(&url, big_bytes());
        // Extractor materializes `piper/piper` inside the extract dir.
        let ex = CapturingBinaryExtractor::new().with_file(
            PathBuf::from("piper").join("piper"),
            b"#!/bin/sh\n".to_vec(),
        );

        let path = download_binary(Some("macos_aarch64"), dir.path(), &dl, &ex).unwrap();
        assert_eq!(path, dir.path().join("piper"));
        assert!(path.is_file());
        // Archive and extract tmp dir cleaned up.
        assert!(!dir.path().join(archive_name).exists());
        assert!(!dir.path().join("piper_extract_tmp").exists());
        // Downloader saw exactly the expected URL.
        assert_eq!(dl.requested(), vec![url]);
        // Extractor was called once with the archive path + extract dir.
        let invs = ex.invocations();
        assert_eq!(invs.len(), 1);
    }

    #[test]
    fn download_binary_idempotent_when_already_installed() {
        let dir = tempdir().unwrap();
        // Pre-install the binary.
        std::fs::write(dir.path().join("piper"), b"#!/bin/sh\n").unwrap();
        let path = download_binary(
            Some("macos_aarch64"),
            dir.path(),
            &NullModelDownloader,
            &NullBinaryExtractor,
        )
        .unwrap();
        assert_eq!(path, dir.path().join("piper"));
    }

    #[test]
    fn download_binary_accepts_bare_piper_path_layout() {
        // Some archives unpack as a bare `piper` (no nested dir).
        let dir = tempdir().unwrap();
        let archive_name = "piper_linux_x64.tar.gz";
        let url = format!("{}/{}", piper_binary_base(), archive_name);
        let dl = CapturingModelDownloader::new().with_response(&url, big_bytes());
        let ex = CapturingBinaryExtractor::new().with_file("piper", b"#!/bin/sh\n".to_vec());
        let path = download_binary(Some("linux_x64"), dir.path(), &dl, &ex).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn download_binary_archive_too_small() {
        let dir = tempdir().unwrap();
        let archive_name = "piper_macos_aarch64.tar.gz";
        let url = format!("{}/{}", piper_binary_base(), archive_name);
        let dl = CapturingModelDownloader::new().with_response(&url, b"tiny".to_vec());
        let err = download_binary(Some("macos_aarch64"), dir.path(), &dl, &NullBinaryExtractor)
            .unwrap_err();
        match err {
            PiperError::BinaryArchiveTooSmall { actual, .. } => {
                assert!(actual < MIN_VOICE_SIZE_BYTES);
            }
            _ => panic!("wrong variant: {err:?}"),
        }
        // Archive bytes were never persisted (size gate runs before write).
        assert!(!dir.path().join(archive_name).exists());
    }

    #[test]
    fn download_binary_missing_inside_archive() {
        let dir = tempdir().unwrap();
        let archive_name = "piper_macos_aarch64.tar.gz";
        let url = format!("{}/{}", piper_binary_base(), archive_name);
        let dl = CapturingModelDownloader::new().with_response(&url, big_bytes());
        // Extractor materializes the WRONG file — no piper anywhere.
        let ex = CapturingBinaryExtractor::new().with_file("README.md", b"hello".to_vec());
        let err = download_binary(Some("macos_aarch64"), dir.path(), &dl, &ex).unwrap_err();
        match err {
            PiperError::BinaryMissingInArchive { .. } => {}
            _ => panic!("wrong variant: {err:?}"),
        }
        // Cleanup ran.
        assert!(!dir.path().join(archive_name).exists());
        assert!(!dir.path().join("piper_extract_tmp").exists());
    }

    // --- PiperDownloader wrapper ---

    #[test]
    fn wrapper_default_is_fail_loud() {
        let dir = tempdir().unwrap();
        let p = PiperDownloader::new();
        let err = p
            .download_voice("en_US-lessac-medium", dir.path())
            .unwrap_err();
        match err {
            PiperError::Download { .. } => {}
            _ => panic!("wrong variant: {err:?}"),
        }
    }

    #[test]
    fn wrapper_routes_through_injected_downloader() {
        let dir = tempdir().unwrap();
        let (onnx_url, json_url) = voice_urls("en_US-lessac-medium").unwrap();
        let dl = Arc::new(
            CapturingModelDownloader::new()
                .with_response(&onnx_url, big_bytes())
                .with_response(&json_url, b"{}".to_vec()),
        );
        let p = PiperDownloader::new().with_downloader(dl);
        let path = p.download_voice("en_US-lessac-medium", dir.path()).unwrap();
        assert!(path.is_file());
    }

    // --- detect_platform sanity ---

    #[test]
    fn detect_platform_returns_known_string() {
        // Only assert the format is one of the known release variants
        // for any supported host. Unsupported hosts error out.
        match detect_platform() {
            Ok(s) => {
                let known = ["macos_aarch64", "macos_x64", "linux_aarch64", "linux_x64"];
                assert!(known.contains(&s.as_str()), "unexpected: {s}");
            }
            Err(PiperError::UnsupportedPlatform { .. }) => {
                // Windows / unknown — error is expected.
            }
            Err(e) => panic!("wrong variant: {e:?}"),
        }
    }
}
