//! `BrowserBinaryManager` — pinned-version download + SHA-256 verification.
//!
//! Wave BR (v0.2.1): the previous E.10 scaffold shipped `verify_sha256` only;
//! the actual download path was deferred. This file now ships the REAL
//! download flow:
//!
//!   * `download_to(url, dest, expected_sha)` — async reqwest GET with
//!     `HTTPS_PROXY` / `https_proxy` env honored, streamed to a temp file,
//!     verified against the expected SHA-256, atomically moved into place.
//!   * `ensure_camoufox()` — high-level entry point. Returns the cached
//!     binary path if it already exists + checksums OK, otherwise downloads
//!     from [`CAMOUFOX_DOWNLOAD_URL`].
//!   * Offline mode (`offline = true`) fails fast with [`BinaryError::OfflineMissing`]
//!     when the cache miss would otherwise trigger a network call.
//!
//!   * `provision_camoufox(&CamoufoxDownloadConfig)` - the supervisor-facing
//!     entry point. Opt-in (disabled by default) and fail-closed: with the
//!     feature off nothing is fetched, and with it on but no operator-pinned
//!     SHA-256 for the resolved platform artifact it REFUSES rather than
//!     fetching an unverified executable. Handles per-platform artifact
//!     selection, archive extraction, and the Unix executable bit.
//!
//! Tests use a wiremock server as the download origin so no live network
//! hit is required — proves the wire-shape end-to-end including SHA
//! verification and rejection of tampered payloads.

use std::path::{Path, PathBuf};

use thiserror::Error;

#[allow(dead_code)] // surface kept for downstream config; the constant pins our supported sidecar version.
pub const CAMOUFOX_VERSION: &str = "127.0.2-beta.23";

/// Default download URL. The Camoufox project publishes per-platform
/// binaries on its GitHub releases page; the launcher (this manager) is
/// allowed to override the URL via [`BrowserBinaryManager::download_to`].
///
/// We do NOT hard-fail if the user points this elsewhere — the SHA-256
/// verification is the security boundary. The URL is documentation +
/// default; the digest is the lock.
#[allow(dead_code)]
pub const CAMOUFOX_DOWNLOAD_URL: &str = "https://github.com/daijro/camoufox/releases/download/v127.0.2-beta.23/camoufox-127.0.2-beta.23-macos-arm64.tar.gz";

/// Placeholder SHA-256 — operators MUST override via config when downloading
/// the canonical artifact. The empty-string sentinel exists so a config that
/// forgets to pin a digest is caught by [`verify_sha256`] returning
/// [`BinaryError::ChecksumMismatch`] (since every real SHA differs from
/// 32 zero-bytes).
///
/// The auto-download path REFUSES to use this constant directly: callers
/// must pass their own `expected_sha_hex` so the lock is explicit.
#[allow(dead_code)]
pub const CAMOUFOX_SHA256_PLACEHOLDER: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Error)]
pub enum BinaryError {
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("offline mode but binary missing at {0}")]
    OfflineMissing(PathBuf),
    #[error("network error: {0}")]
    Network(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("refused placeholder SHA-256 — pin a real digest via config")]
    PlaceholderSha,
    #[error("download server HTTP {status} at {url}")]
    HttpStatus { status: u16, url: String },
    #[error(
        "Camoufox auto-download is enabled but no artifact is configured for platform {0} - add [browser.camoufox_download.artifacts.\"{0}\"] with a url and sha256"
    )]
    UnconfiguredPlatform(String),
    #[error(
        "Camoufox auto-download is enabled but no sha256 is pinned for platform {0}; refusing to fetch an unverified executable - set browser.camoufox_download.artifacts.\"{0}\".sha256"
    )]
    UnpinnedDigest(String),
    #[error("archive extraction failed: {0}")]
    Extract(String),
    #[error("archive did not contain the configured executable at {0}")]
    MissingExecutable(String),
    #[error("unsafe path in configuration or archive: {0}")]
    UnsafePath(String),
    #[error(
        "refusing to fetch an executable over {scheme} - browser.camoufox_download artifact url {url} must use https (plain http is accepted only for a loopback host)"
    )]
    InsecureScheme { scheme: String, url: String },
}

/// Manager surface — `ensure_camoufox` downloads if missing, verifies SHA,
/// and returns the path. Other backends (chromium, browserbase) reuse the
/// `verify_sha256` + `download_to` helpers.
pub struct BrowserBinaryManager {
    /// Install root — by default `~/.wayland-core/browser/bin/`.
    pub install_root: PathBuf,
    /// When `true`, refuse to make any network call.
    pub offline: bool,
    /// Optional `HTTPS_PROXY` override (otherwise picked from env).
    pub https_proxy: Option<String>,
}

impl BrowserBinaryManager {
    pub fn new(install_root: PathBuf, offline: bool) -> Self {
        Self {
            install_root,
            offline,
            https_proxy: std::env::var("HTTPS_PROXY")
                .ok()
                .or_else(|| std::env::var("https_proxy").ok()),
        }
    }

    /// Build the reqwest client honoring `HTTPS_PROXY` / `https_proxy`.
    fn build_client(&self) -> Result<wcore_egress::EgressClient, BinaryError> {
        let mut b = wcore_egress::EgressClient::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(5))
            // Don't follow redirects silently — pin the URL we asked for.
            // Real Camoufox releases come from GitHub which DOES 302 to a
            // CDN; we allow up to 10 hops so the realistic path works,
            // then SHA-256 catches any swap.
            .redirect(reqwest::redirect::Policy::limited(10));
        if let Some(proxy) = self.https_proxy.as_ref() {
            let p =
                reqwest::Proxy::https(proxy).map_err(|e| BinaryError::Network(e.to_string()))?;
            b = b.proxy(p);
        }
        b.build().map_err(|e| BinaryError::Network(e.to_string()))
    }

    /// High-level: ensure the Camoufox binary is present + verified.
    /// Returns the path to the on-disk artifact.
    ///
    /// `expected_sha_hex` is the operator-pinned digest — passing the
    /// `CAMOUFOX_SHA256_PLACEHOLDER` sentinel is rejected.
    pub async fn ensure_camoufox(
        &self,
        download_url: &str,
        expected_sha_hex: &str,
    ) -> Result<PathBuf, BinaryError> {
        let dest = self.install_root.join(format!(
            "camoufox-{}",
            sanitize_version_for_filename(CAMOUFOX_VERSION)
        ));

        // Cache hit?
        if dest.exists()
            && let Ok(()) = Self::verify_sha256(&dest, expected_sha_hex)
        {
            return Ok(dest);
        }

        if self.offline {
            return Err(BinaryError::OfflineMissing(dest));
        }

        if expected_sha_hex.eq_ignore_ascii_case(CAMOUFOX_SHA256_PLACEHOLDER) {
            return Err(BinaryError::PlaceholderSha);
        }

        self.download_to(download_url, &dest, expected_sha_hex)
            .await?;
        Ok(dest)
    }

    /// Download a URL to a destination path, streaming the body and
    /// SHA-256-verifying before atomic-move into place. Sets the parent
    /// directory if missing. Refuses placeholder SHAs.
    pub async fn download_to(
        &self,
        url: &str,
        dest: &Path,
        expected_sha_hex: &str,
    ) -> Result<(), BinaryError> {
        if expected_sha_hex.eq_ignore_ascii_case(CAMOUFOX_SHA256_PLACEHOLDER) {
            return Err(BinaryError::PlaceholderSha);
        }
        if self.offline {
            return Err(BinaryError::OfflineMissing(dest.to_path_buf()));
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let client = self.build_client()?;
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| BinaryError::Network(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(BinaryError::HttpStatus {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }

        let body = resp
            .bytes()
            .await
            .map_err(|e| BinaryError::Network(e.to_string()))?;

        let actual = sha256_hex(&body);
        if !actual.eq_ignore_ascii_case(expected_sha_hex) {
            return Err(BinaryError::ChecksumMismatch {
                expected: expected_sha_hex.to_string(),
                actual,
            });
        }

        // Atomic move: write to .tmp first, then rename.
        // Using plain fs::write here — tmp is scratch; the rename below is the atomic commit.
        let tmp = dest.with_extension("tmp");
        std::fs::write(&tmp, &body)?;
        std::fs::rename(&tmp, dest)?;
        Ok(())
    }

    /// Verify the on-disk SHA-256 against a known-good digest. Public so
    /// E.10's TDD test can feed a tampered binary and assert refusal.
    pub fn verify_sha256(path: &Path, expected_hex: &str) -> Result<(), BinaryError> {
        let bytes = std::fs::read(path)?;
        let actual_hex = sha256_hex(&bytes);
        if actual_hex.eq_ignore_ascii_case(expected_hex) {
            Ok(())
        } else {
            Err(BinaryError::ChecksumMismatch {
                expected: expected_hex.to_string(),
                actual: actual_hex,
            })
        }
    }

    /// Provision a runnable Camoufox executable from operator configuration.
    ///
    /// This is the entry point the supervisor calls. Before it existed,
    /// [`Self::ensure_camoufox`] had zero production callers, so a host
    /// without `camofox-browser` on PATH simply failed at first use.
    ///
    /// **Opt-in and fail-closed.** Exactly three outcomes:
    ///
    ///   * `Ok(None)` - auto-download is disabled (the default). Nothing is
    ///     fetched, nothing is written, the caller keeps the program it had.
    ///   * `Ok(Some(path))` - an artifact whose bytes matched the
    ///     operator-pinned SHA-256 was installed, unpacked if it was an
    ///     archive, and made executable.
    ///   * `Err(_)` - an actionable refusal. An enabled download with no
    ///     artifact configured for this platform, or an artifact with no
    ///     pinned digest, is [`BinaryError::UnconfiguredPlatform`] /
    ///     [`BinaryError::UnpinnedDigest`]. There is deliberately no path
    ///     that fetches an unverified binary, and none that falls back to
    ///     running one.
    pub async fn provision_camoufox(
        &self,
        download: &wcore_config::browser::CamoufoxDownloadConfig,
    ) -> Result<Option<PathBuf>, BinaryError> {
        if !download.enabled {
            return Ok(None);
        }
        let key = wcore_config::browser::platform_key();
        let artifact = download
            .artifact_for_current_platform()
            .ok_or_else(|| BinaryError::UnconfiguredPlatform(key.clone()))?;
        let expected_sha = artifact.sha256.trim();
        if expected_sha.is_empty() {
            return Err(BinaryError::UnpinnedDigest(key));
        }
        require_secure_url(&artifact.url)?;
        let downloaded = self.ensure_camoufox(&artifact.url, expected_sha).await?;
        let exe = self.materialize_executable(&downloaded, &artifact.archive_exe_path)?;
        Ok(Some(exe))
    }

    /// Turn a SHA-verified on-disk artifact into a runnable executable path.
    ///
    /// An empty `archive_exe_path` means the artifact IS the executable.
    /// Otherwise the artifact is unpacked into a sibling directory under the
    /// install root and the named member is returned. The executable bit is
    /// applied through the single [`set_executable`] helper.
    fn materialize_executable(
        &self,
        artifact: &Path,
        archive_exe_path: &str,
    ) -> Result<PathBuf, BinaryError> {
        let rel = archive_exe_path.trim();
        if rel.is_empty() {
            set_executable(artifact)?;
            return Ok(artifact.to_path_buf());
        }
        let rel_path = safe_relative_path(rel)?;
        let unpack_root = self.install_root.join(format!(
            "camoufox-{}-unpacked",
            sanitize_version_for_filename(CAMOUFOX_VERSION)
        ));
        let exe = unpack_root.join(&rel_path);
        // Already unpacked by a previous run. The archive was SHA-verified
        // above, so re-extracting buys nothing.
        if !exe.is_file() {
            extract_archive(artifact, &unpack_root)?;
        }
        if !exe.is_file() {
            return Err(BinaryError::MissingExecutable(rel.to_string()));
        }
        set_executable(&exe)?;
        Ok(exe)
    }
}

/// Reject a configured artifact URL whose transport is not `https`.
///
/// Being precise about what this does and does not buy, because the module
/// doc above makes the opposite claim ("the URL is documentation + default;
/// the digest is the lock") and that claim is CORRECT: the operator-pinned
/// SHA-256 is compared before anything is moved into place or made
/// executable, so an active attacker on a plain-HTTP fetch can make
/// provisioning FAIL but cannot swap the executable. This check is therefore
/// defence in depth, not the security boundary. What it actually closes is
/// (a) the artifact URL and anything an operator embedded in it - a signed
/// query parameter, a token - travelling in clear text, and (b) a config typo
/// silently downgrading the transport with no diagnostic.
///
/// Plain `http` to a LOOPBACK host is allowed. An operator serving a local
/// mirror is not exposed to a network observer, so refusing it would cost a
/// legitimate deployment and buy nothing.
fn require_secure_url(url: &str) -> Result<(), BinaryError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|error| BinaryError::Network(format!("invalid artifact url {url}: {error}")))?;
    let scheme = parsed.scheme();
    if scheme.eq_ignore_ascii_case("https") {
        return Ok(());
    }
    let host = parsed.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if scheme.eq_ignore_ascii_case("http") && loopback {
        return Ok(());
    }
    Err(BinaryError::InsecureScheme {
        scheme: scheme.to_string(),
        url: url.to_string(),
    })
}

/// Reject configured member paths that are absolute or contain `..` -
/// joining either would escape the install root.
fn safe_relative_path(rel: &str) -> Result<PathBuf, BinaryError> {
    let p = Path::new(rel);
    if p.components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(BinaryError::UnsafePath(rel.to_string()));
    }
    Ok(p.to_path_buf())
}

/// Unpack `archive` into `dest_root`.
///
/// The format is chosen from the file's **magic bytes**, not from the URL
/// spelling and not from the host OS, so no `cfg!(windows)` branch decides
/// it. Both extractors are the crates' own traversal-safe entry points:
/// `tar::Archive::unpack` skips `..` components and `zip::ZipArchive::extract`
/// resolves members through `enclosed_name`.
fn extract_archive(archive: &Path, dest_root: &Path) -> Result<(), BinaryError> {
    use std::io::Read as _;

    let mut magic = [0u8; 4];
    {
        let mut f = std::fs::File::open(archive)?;
        // A short read is fine: a file too small to hold either signature
        // matches neither and falls through to the error below.
        let mut filled = 0usize;
        while filled < magic.len() {
            match f.read(&mut magic[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
    }
    std::fs::create_dir_all(dest_root)?;

    if magic[..2] == [0x1f, 0x8b] {
        let f = std::fs::File::open(archive)?;
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
        ar.unpack(dest_root)
            .map_err(|e| BinaryError::Extract(format!("tar.gz: {e}")))?;
        Ok(())
    } else if magic == [0x50, 0x4b, 0x03, 0x04] {
        let f = std::fs::File::open(archive)?;
        let mut zip =
            zip::ZipArchive::new(f).map_err(|e| BinaryError::Extract(format!("zip: {e}")))?;
        zip.extract(dest_root)
            .map_err(|e| BinaryError::Extract(format!("zip: {e}")))?;
        Ok(())
    } else {
        Err(BinaryError::Extract(format!(
            "unrecognised archive format (magic {magic:02x?}); expected gzip or zip"
        )))
    }
}

/// The one place the executable-bit platform difference is expressed.
/// Windows carries no mode bits - an extracted `.exe` is runnable as written.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), BinaryError> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), BinaryError> {
    Ok(())
}

/// Filesystem-safe filename chunk for a version string. Keeps `[A-Za-z0-9._-]`,
/// substitutes anything else with `-`.
fn sanitize_version_for_filename(v: &str) -> String {
    v.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Minimal SHA-256 (so we don't pull `sha2` just for this module). Lifted
/// from FIPS 180-4 reference; ~70 lines. Verified by the `known_vectors`
/// test against the empty-string + "abc" vectors.
pub fn sha256_hex(input: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (input.len() as u64) * 8;
    let mut buf = Vec::with_capacity(input.len() + 72);
    buf.extend_from_slice(input);
    buf.push(0x80);
    while buf.len() % 64 != 56 {
        buf.push(0);
    }
    buf.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in buf.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let mj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(mj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for v in h {
        out.push_str(&format!("{v:08x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn known_sha256_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn verify_rejects_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        std::fs::write(&path, b"hello world").unwrap();
        let r = BrowserBinaryManager::verify_sha256(&path, "00".repeat(32).as_str());
        assert!(matches!(r, Err(BinaryError::ChecksumMismatch { .. })));
    }

    #[test]
    fn verify_accepts_correct_digest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        std::fs::write(&path, b"abc").unwrap();
        BrowserBinaryManager::verify_sha256(
            &path,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
    }

    #[tokio::test]
    async fn download_to_streams_body_and_verifies_sha() {
        let server = MockServer::start().await;
        let payload = b"camoufox-binary-content-v1";
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.as_ref()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let dest = dir.path().join("camoufox.tar.gz");
        let url = format!("{}/camoufox.tar.gz", server.uri());
        let sha = sha256_hex(payload);
        mgr.download_to(&url, &dest, &sha).await.unwrap();
        assert!(dest.exists());
        let on_disk = std::fs::read(&dest).unwrap();
        assert_eq!(on_disk, payload);
    }

    #[tokio::test]
    async fn download_to_rejects_sha_mismatch() {
        let server = MockServer::start().await;
        let payload = b"tampered-payload";
        Mock::given(method("GET"))
            .and(path("/bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.as_ref()))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let dest = dir.path().join("bin");
        let url = format!("{}/bin", server.uri());
        // Use a non-placeholder wrong SHA (digest of a different known input)
        // so we exercise the SHA-verification branch (not the placeholder
        // refusal which has its own dedicated test below).
        let wrong_sha = sha256_hex(b"different-payload");
        let r = mgr.download_to(&url, &dest, &wrong_sha).await;
        assert!(
            matches!(r, Err(BinaryError::ChecksumMismatch { .. })),
            "expected ChecksumMismatch, got {r:?}"
        );
        // Tampered payload MUST NOT have landed on disk.
        assert!(!dest.exists(), "rejected payload leaked to disk");
    }

    #[tokio::test]
    async fn download_to_rejects_placeholder_sha() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let r = mgr
            .download_to(
                "http://does-not-matter.example/",
                &dir.path().join("x"),
                CAMOUFOX_SHA256_PLACEHOLDER,
            )
            .await;
        assert!(matches!(r, Err(BinaryError::PlaceholderSha)));
    }

    #[tokio::test]
    async fn download_to_surfaces_http_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bin"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let dest = dir.path().join("bin");
        let url = format!("{}/bin", server.uri());
        let r = mgr.download_to(&url, &dest, &sha256_hex(b"x")).await;
        match r {
            Err(BinaryError::HttpStatus { status, .. }) => assert_eq!(status, 404),
            other => panic!("expected HttpStatus, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn offline_mode_refuses_download() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), true);
        let r = mgr
            .download_to(
                "http://example.invalid/",
                &dir.path().join("x"),
                &sha256_hex(b"x"),
            )
            .await;
        assert!(matches!(r, Err(BinaryError::OfflineMissing(_))));
    }

    #[tokio::test]
    async fn ensure_camoufox_uses_cache_on_repeat() {
        let server = MockServer::start().await;
        let payload = b"camoufox-cached-payload-v1";
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.as_ref()))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let url = format!("{}/camoufox.tar.gz", server.uri());
        let sha = sha256_hex(payload);
        let p1 = mgr.ensure_camoufox(&url, &sha).await.unwrap();
        let p2 = mgr.ensure_camoufox(&url, &sha).await.unwrap();
        assert_eq!(p1, p2);
        // Mock `.expect(1)` enforces the second call was a cache hit.
    }

    // ── F7: opt-in, fail-closed provisioning ─────────────────────────────
    //
    // Every arm below mounts the origin with an explicit `.expect(n)`.
    // `MockServer` verifies expectations when it drops, so "nothing was
    // fetched" is asserted against the wire, not inferred from a return
    // value. No test contacts a real network.

    fn artifacts_for(
        url: &str,
        sha: &str,
        exe: &str,
    ) -> std::collections::BTreeMap<String, wcore_config::browser::BinaryArtifact> {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            wcore_config::browser::platform_key(),
            wcore_config::browser::BinaryArtifact {
                url: url.to_string(),
                sha256: sha.to_string(),
                archive_exe_path: exe.to_string(),
            },
        );
        m
    }

    /// A gzipped tar holding one member at `rel`, deliberately mode 0o644 so
    /// the executable bit can only come from our own chmod.
    fn tar_gz_with(rel: &str, body: &[u8]) -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut builder = tar::Builder::new(enc);
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        builder.append_data(&mut header, rel, body).unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn dir_entry_count(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    /// (a) Feature OFF ⇒ nothing is fetched and nothing is written, even
    /// though a perfectly good artifact with a correct digest is configured.
    #[tokio::test]
    async fn provision_disabled_fetches_nothing() {
        let server = MockServer::start().await;
        let payload = tar_gz_with("bin/camoufox", b"#!/bin/sh\nexit 0\n");
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
            .expect(0)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: false,
            artifacts: artifacts_for(
                &format!("{}/camoufox.tar.gz", server.uri()),
                &sha256_hex(&payload),
                "bin/camoufox",
            ),
        };
        let got = mgr.provision_camoufox(&cfg).await.unwrap();
        assert!(
            got.is_none(),
            "auto-download is off; provisioning must be a no-op, got {got:?}"
        );
        assert_eq!(
            dir_entry_count(dir.path()),
            0,
            "the disabled path wrote into the install root"
        );
    }

    /// (b) Feature ON but the artifact carries no operator-pinned digest ⇒
    /// REFUSE. Never fetch-and-trust, never run an unverified binary.
    #[tokio::test]
    async fn provision_enabled_without_digest_refuses_without_fetching() {
        let server = MockServer::start().await;
        let payload = tar_gz_with("bin/camoufox", b"#!/bin/sh\nexit 0\n");
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
            .expect(0)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: true,
            artifacts: artifacts_for(
                &format!("{}/camoufox.tar.gz", server.uri()),
                "   ",
                "bin/camoufox",
            ),
        };
        let r = mgr.provision_camoufox(&cfg).await;
        assert!(
            matches!(r, Err(BinaryError::UnpinnedDigest(_))),
            "expected UnpinnedDigest refusal, got {r:?}"
        );
        assert_eq!(
            dir_entry_count(dir.path()),
            0,
            "refusal still touched the install root"
        );
    }

    /// (b') Feature ON with no artifact at all for the running platform ⇒
    /// refuse. Never borrow another platform's URL.
    #[tokio::test]
    async fn provision_enabled_without_platform_artifact_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let mut artifacts = artifacts_for("http://example.invalid/x", "aa", "");
        artifacts.clear();
        artifacts.insert(
            "some-other-platform".to_string(),
            wcore_config::browser::BinaryArtifact {
                url: "http://example.invalid/other".into(),
                sha256: "bb".into(),
                archive_exe_path: String::new(),
            },
        );
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: true,
            artifacts,
        };
        let r = mgr.provision_camoufox(&cfg).await;
        match r {
            Err(BinaryError::UnconfiguredPlatform(key)) => {
                assert_eq!(key, wcore_config::browser::platform_key());
            }
            other => panic!("expected UnconfiguredPlatform, got {other:?}"),
        }
    }

    /// (c) A digest mismatch refuses AND leaves no partial file behind — not
    /// the destination, not the `.tmp` scratch file, not an unpack dir.
    #[tokio::test]
    async fn provision_digest_mismatch_refuses_and_leaves_no_partial_file() {
        let server = MockServer::start().await;
        let payload = tar_gz_with("bin/camoufox", b"#!/bin/sh\nexit 0\n");
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: true,
            artifacts: artifacts_for(
                &format!("{}/camoufox.tar.gz", server.uri()),
                &sha256_hex(b"a completely different artifact"),
                "bin/camoufox",
            ),
        };
        let r = mgr.provision_camoufox(&cfg).await;
        assert!(
            matches!(r, Err(BinaryError::ChecksumMismatch { .. })),
            "expected ChecksumMismatch refusal, got {r:?}"
        );
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(
            leftovers.is_empty(),
            "rejected payload left files behind: {leftovers:?}"
        );
    }

    /// The happy path: verified archive is unpacked and the member named by
    /// `archive_exe_path` comes back with the executable bit set. The tar
    /// member is written 0o644, so a passing mode check can only come from
    /// [`set_executable`].
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_extracts_archive_and_sets_exec_bit() {
        use std::os::unix::fs::PermissionsExt as _;

        let server = MockServer::start().await;
        let payload = tar_gz_with("camoufox/camoufox", b"#!/bin/sh\nexit 0\n");
        Mock::given(method("GET"))
            .and(path("/camoufox.tar.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: true,
            artifacts: artifacts_for(
                &format!("{}/camoufox.tar.gz", server.uri()),
                &sha256_hex(&payload),
                "camoufox/camoufox",
            ),
        };
        let exe = mgr
            .provision_camoufox(&cfg)
            .await
            .expect("verified artifact must provision")
            .expect("enabled download must yield a path");
        assert!(exe.is_file(), "no executable at {}", exe.display());
        assert!(
            exe.starts_with(dir.path()),
            "provisioned outside the install root: {}",
            exe.display()
        );
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_ne!(
            mode & 0o111,
            0,
            "extracted member is not executable (mode {mode:o}); the archive ships it 0o644"
        );

        // Second call is a cache hit — `.expect(1)` on the mock enforces it.
        let again = mgr.provision_camoufox(&cfg).await.unwrap().unwrap();
        assert_eq!(again, exe);
    }

    /// A configured member path that climbs out of the install root is
    /// refused before anything is joined to it.
    #[test]
    fn safe_relative_path_rejects_traversal_and_absolute() {
        assert!(matches!(
            safe_relative_path("../../etc/passwd"),
            Err(BinaryError::UnsafePath(_))
        ));
        assert!(matches!(
            safe_relative_path("/etc/passwd"),
            Err(BinaryError::UnsafePath(_))
        ));
        assert!(safe_relative_path("camoufox/camoufox").is_ok());
    }

    /// The transport check, both directions. The negative cases are the
    /// point, but the positive ones are here too: a rule that refuses every
    /// URL would pass the refusals and break every real deployment.
    #[test]
    fn require_secure_url_refuses_non_https_transports() {
        for url in [
            "http://mirror.example.com/camoufox.tar.gz",
            "http://203.0.113.7:8080/camoufox.tar.gz",
            "ftp://mirror.example.com/camoufox.tar.gz",
            "file:///tmp/camoufox.tar.gz",
        ] {
            assert!(
                matches!(
                    require_secure_url(url),
                    Err(BinaryError::InsecureScheme { .. })
                ),
                "{url} was accepted; an executable would be fetched over an \
                 unauthenticated transport"
            );
        }
    }

    /// Positive control for the arm above: https anywhere, and plain http
    /// only to a loopback host (the local-mirror carve-out the doc comment
    /// justifies, and the transport the provisioning wiring tests use).
    #[test]
    fn require_secure_url_accepts_https_and_loopback_http() {
        for url in [
            "https://mirror.example.com/camoufox.tar.gz",
            "HTTPS://mirror.example.com/camoufox.tar.gz",
            "http://127.0.0.1:8080/camoufox.tar.gz",
            "http://localhost:8080/camoufox.tar.gz",
            "http://[::1]:8080/camoufox.tar.gz",
        ] {
            assert!(
                require_secure_url(url).is_ok(),
                "{url} was refused; the rule is stricter than intended"
            );
        }
    }

    /// The refusal must happen at the CONFIG surface, before any client is
    /// built or any request is issued. Asserting on the returned error alone
    /// would not distinguish "refused" from "tried and failed".
    #[tokio::test]
    async fn provision_camoufox_refuses_a_plain_http_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = BrowserBinaryManager::new(dir.path().to_path_buf(), false);
        let mut artifacts = std::collections::BTreeMap::new();
        artifacts.insert(
            wcore_config::browser::platform_key(),
            wcore_config::browser::BinaryArtifact {
                url: "http://mirror.example.com/camoufox.tar.gz".to_string(),
                sha256: sha256_hex(b"payload"),
                archive_exe_path: "camoufox/camoufox".to_string(),
            },
        );
        let cfg = wcore_config::browser::CamoufoxDownloadConfig {
            enabled: true,
            artifacts,
        };

        let err = mgr
            .provision_camoufox(&cfg)
            .await
            .expect_err("a plain-http artifact url must be refused, not fetched");
        assert!(
            matches!(err, BinaryError::InsecureScheme { .. }),
            "expected an InsecureScheme refusal, got: {err}"
        );
        assert!(
            err.to_string().contains("https"),
            "the refusal must name the transport the operator has to switch to, got: {err}"
        );
        assert!(
            !dir.path().join("camoufox.tar.gz").exists(),
            "the refused path still wrote into the install root"
        );
    }
}
