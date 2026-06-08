//! v0.8.1 U9 — `wayland-core self-update` subcommand.
//!
//! Pulls the latest release from GitHub (`FerroxLabs/wayland-core`),
//! verifies the `.sig` artifact against the pinned marketplace pubkey
//! (ed25519), and atomically replaces the running binary via
//! `self_replace`.
//!
//! Threat model:
//! - The release API URL is a static const; we never interpolate user
//!   input into a host. The artifact + sig URLs come straight from the
//!   GitHub API response (`browser_download_url`).
//! - Verification is ed25519 over the *binary bytes* — same shape used
//!   by `wcore-agent::plugins::sig_verifier` for plugin signatures. A
//!   tampered binary OR a tampered sig is rejected with
//!   `SignatureVerificationFailed`.
//! - The download is size-checked against the `Content-Length` header
//!   when present; if absent, we still verify the signature so a
//!   short-write attack still cannot succeed.

use anyhow::{Context, Result, bail};
use ed25519_dalek::{Signature, VerifyingKey, ed25519::signature::Verifier};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// GitHub repo that hosts wayland-core releases. Pinned to the production
/// org so a misconfigured workspace cannot redirect updates elsewhere.
pub const RELEASES_REPO: &str = "FerroxLabs/wayland-core";

/// Pinned marketplace ed25519 public key (32 raw bytes, base64-encoded).
///
/// IMPORTANT: replace the placeholder below at v0.8.1 release-cut time
/// with the real production marketplace key. Until then, the key field
/// is honored via the `WAYLAND_SELF_UPDATE_PUBKEY_B64` environment
/// override (used in tests; documented in `RELEASING.md` for the
/// transition period).
///
/// The const intentionally encodes a deterministic all-zero key so that
/// in the absence of an env override the verification path is
/// guaranteed to FAIL closed (an all-zero ed25519 key cannot validly
/// sign anything a release would publish).
pub const MARKETPLACE_PUBKEY_B64: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// Env override for the pinned pubkey. The CLI prefers this when set so
/// the release engineer can roll the key without a code change. Tests
/// also use this to inject a freshly-generated keypair.
pub const ENV_PUBKEY_B64: &str = "WAYLAND_SELF_UPDATE_PUBKEY_B64";

/// Entry point. `check_only=true` prints the version diff and returns
/// without touching disk.
pub async fn run(check_only: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");
    // F-029: distinguish "no releases yet" (404, clean exit) from a
    // genuinely broken repo (other non-2xx, hard error).
    let release = match fetch_latest_release(RELEASES_REPO).await? {
        Some(r) => r,
        None => {
            println!("current: v{current_version}");
            println!("latest:  no releases published yet on FerroxLabs/wayland-core");
            return Ok(());
        }
    };
    let latest_version = release.version();
    println!("current: v{current_version}");
    println!("latest:  v{latest_version}");

    if latest_version == current_version {
        println!("already up to date.");
        return Ok(());
    }
    if check_only {
        println!("(check-only: not installing)");
        return Ok(());
    }

    let artifact_name = artifact_name_for_host();
    let sig_name = format!("{artifact_name}.sig");

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == artifact_name)
        .with_context(|| format!("no {artifact_name} in release v{latest_version}"))?;
    let sig_asset = release
        .assets
        .iter()
        .find(|a| a.name == sig_name)
        .with_context(|| format!("no {sig_name} in release v{latest_version}"))?;

    let tmp = tempfile::tempdir()?;
    let bin_path = tmp.path().join(&artifact_name);
    let sig_path = tmp.path().join(&sig_name);
    download_to(&asset.browser_download_url, &bin_path).await?;
    download_to(&sig_asset.browser_download_url, &sig_path).await?;

    let pubkey = load_pinned_pubkey()?;
    verify_signature(&pubkey, &bin_path, &sig_path)
        .context("signature verification failed — refusing to install untrusted binary")?;

    atomic_swap(&bin_path)?;
    println!("upgraded to v{latest_version}");
    Ok(())
}

// ---------------------------------------------------------------------
// Release fetch
// ---------------------------------------------------------------------

/// Raw GitHub release shape. Only the fields we read are modeled.
#[derive(Debug, serde::Deserialize)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Strip the leading `v` and the trailing `-wayland-base` from the
    /// release tag so consumers see a SemVer string that matches
    /// `CARGO_PKG_VERSION`.
    pub fn version(&self) -> &str {
        self.tag
            .trim_start_matches('v')
            .trim_end_matches("-wayland-base")
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// Lower-level fetch used by tests (mockito sets a custom base URL).
///
/// Distinguishes two failure modes so callers can render appropriate messages:
/// - HTTP 404: the repo exists but has no `latest` release published yet.
///   Returns `Ok(None)` instead of an error so the caller can say "no releases
///   yet" without treating it as a broken-repo error.
/// - Any other non-2xx status: returns `Err` (unexpected / broken repo).
pub async fn fetch_latest_release_from_url(url: &str) -> Result<Option<Release>> {
    let client = wcore_egress::EgressClient::builder()
        .user_agent(concat!("wayland-core/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build reqwest client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .context("GET releases/latest")?;
    let status = resp.status();
    // 404 = no releases published yet (repo exists, no tags). Return None
    // so the caller can print a clean "no releases yet" message.
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        bail!("GET {url} failed: HTTP {status}");
    }
    let release: Release = resp.json().await.context("parse release JSON")?;
    Ok(Some(release))
}

/// Pull the latest release JSON. `repo` is `<org>/<name>`.
/// Returns `Ok(None)` when the repo has no releases yet (HTTP 404).
pub async fn fetch_latest_release(repo: &str) -> Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    fetch_latest_release_from_url(&url).await
}

// ---------------------------------------------------------------------
// Host artifact mapping
// ---------------------------------------------------------------------

/// Map `(target_os, target_arch)` → release artifact filename. Matches
/// the names produced by `.github/workflows/release.yml`.
pub fn artifact_name_for_host() -> String {
    artifact_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Pure mapping for tests.
pub fn artifact_name_for(os: &str, arch: &str) -> String {
    match (os, arch) {
        ("macos", "aarch64") => "wayland-core-aarch64-apple-darwin".into(),
        ("macos", "x86_64") => "wayland-core-x86_64-apple-darwin".into(),
        ("linux", "x86_64") => "wayland-core-x86_64-unknown-linux-gnu".into(),
        ("linux", "aarch64") => "wayland-core-aarch64-unknown-linux-gnu".into(),
        ("windows", "x86_64") => "wayland-core-x86_64-pc-windows-msvc.exe".into(),
        (o, a) => format!("wayland-core-{a}-{o}"),
    }
}

// ---------------------------------------------------------------------
// Streaming download
// ---------------------------------------------------------------------

/// Streaming GET into `path`. Verifies bytes-written matches the
/// `Content-Length` header when present.
pub async fn download_to(url: &str, path: &Path) -> Result<()> {
    let client = wcore_egress::EgressClient::builder()
        .user_agent(concat!("wayland-core/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("build reqwest client for download")?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    if !resp.status().is_success() {
        bail!("GET {url} failed: HTTP {}", resp.status());
    }
    let expected = resp.content_length();

    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("create {}", path.display()))?;
    let mut stream = resp.bytes_stream();
    let mut written: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read response chunk")?;
        file.write_all(&chunk).await.context("write chunk")?;
        written += chunk.len() as u64;
    }
    file.flush().await.context("flush file")?;
    drop(file);

    if let Some(exp) = expected
        && exp != written
    {
        bail!("download size mismatch for {url}: expected {exp} bytes, got {written}");
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------

/// Resolve the pinned marketplace pubkey. Env override takes precedence
/// over the compiled-in const so release engineers can rotate the key
/// without a code change.
pub fn load_pinned_pubkey() -> Result<VerifyingKey> {
    let b64 = std::env::var(ENV_PUBKEY_B64).unwrap_or_else(|_| MARKETPLACE_PUBKEY_B64.to_string());
    parse_pubkey_b64(&b64).context("parse pinned marketplace pubkey")
}

/// Parse a base64-encoded 32-byte ed25519 verifying key.
pub fn parse_pubkey_b64(b64: &str) -> Result<VerifyingKey> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .context("pubkey base64 decode")?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("pubkey must be 32 raw ed25519 bytes after base64 decode"))?;
    VerifyingKey::from_bytes(&arr).context("pubkey not a valid ed25519 point")
}

/// Verify `sig_path` (raw 64-byte ed25519 signature) over the contents
/// of `bin_path` against `pubkey`.
pub fn verify_signature(pubkey: &VerifyingKey, bin_path: &Path, sig_path: &Path) -> Result<()> {
    let bin =
        std::fs::read(bin_path).with_context(|| format!("read binary {}", bin_path.display()))?;
    let sig_bytes =
        std::fs::read(sig_path).with_context(|| format!("read sig {}", sig_path.display()))?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("sig must be 64 raw ed25519 bytes"))?;
    let sig = Signature::from_bytes(&sig_arr);
    pubkey.verify(&bin, &sig).context("ed25519 verify failed")?;
    Ok(())
}

// ---------------------------------------------------------------------
// Atomic swap
// ---------------------------------------------------------------------

/// Replace the running binary with the new one at `new_bin_path`. Uses
/// `self_replace` for cross-platform atomicity (POSIX `rename`, Windows
/// `MoveFileExW` + the running-exe-lock dance).
pub fn atomic_swap(new_bin_path: &Path) -> Result<()> {
    // Permission bits: ensure the new file is executable on Unix before
    // we swap it in. On Windows file permissions don't carry, so this is
    // a Unix-only fixup.
    set_executable(new_bin_path)?;
    self_replace::self_replace(new_bin_path)
        .with_context(|| format!("self_replace from {}", new_bin_path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("chmod 755 {}", path.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Helper for tests: returns the path the running exe would be replaced
/// with. Wrapper around `std::env::current_exe` so tests can assert it
/// resolves without actually swapping.
pub fn current_exe_path() -> Result<PathBuf> {
    std::env::current_exe().context("std::env::current_exe")
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{SigningKey, ed25519::signature::Signer};
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    #[test]
    fn release_version_strips_v_prefix() {
        let r = Release {
            tag: "v0.8.1".into(),
            assets: vec![],
        };
        assert_eq!(r.version(), "0.8.1");
    }

    #[test]
    fn release_version_strips_wayland_base_suffix() {
        let r = Release {
            tag: "v0.7.0-wayland-base".into(),
            assets: vec![],
        };
        assert_eq!(r.version(), "0.7.0");
    }

    #[test]
    fn release_version_handles_bare_tag() {
        let r = Release {
            tag: "1.2.3".into(),
            assets: vec![],
        };
        assert_eq!(r.version(), "1.2.3");
    }

    #[test]
    fn artifact_name_macos_arm64() {
        assert_eq!(
            artifact_name_for("macos", "aarch64"),
            "wayland-core-aarch64-apple-darwin"
        );
    }

    #[test]
    fn artifact_name_linux_x64() {
        assert_eq!(
            artifact_name_for("linux", "x86_64"),
            "wayland-core-x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn artifact_name_windows_x64() {
        assert_eq!(
            artifact_name_for("windows", "x86_64"),
            "wayland-core-x86_64-pc-windows-msvc.exe"
        );
    }

    #[test]
    fn artifact_name_for_host_matches_known_shape() {
        // The host-derived name MUST start with "wayland-core-".
        let n = artifact_name_for_host();
        assert!(n.starts_with("wayland-core-"), "got {n}");
    }

    #[test]
    fn parse_pubkey_b64_round_trip() {
        use base64::Engine;
        let sk = SigningKey::generate(&mut OsRng);
        let b64 = base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().as_bytes());
        let vk = parse_pubkey_b64(&b64).unwrap();
        assert_eq!(vk.as_bytes(), sk.verifying_key().as_bytes());
    }

    #[test]
    fn parse_pubkey_b64_rejects_garbage() {
        assert!(parse_pubkey_b64("not-base64!!!").is_err());
        assert!(parse_pubkey_b64("AAAA").is_err()); // wrong length
    }

    fn write_pair(tmp: &TempDir, body: &[u8], sig: &[u8]) -> (PathBuf, PathBuf) {
        let bin = tmp.path().join("artifact");
        let s = tmp.path().join("artifact.sig");
        std::fs::write(&bin, body).unwrap();
        std::fs::write(&s, sig).unwrap();
        (bin, s)
    }

    #[test]
    fn verify_signature_accepts_valid_sig() {
        let sk = SigningKey::generate(&mut OsRng);
        let body = b"release-binary-bytes";
        let sig: Signature = sk.sign(body);
        let tmp = TempDir::new().unwrap();
        let (bin, sig_path) = write_pair(&tmp, body, &sig.to_bytes());
        assert!(verify_signature(&sk.verifying_key(), &bin, &sig_path).is_ok());
    }

    #[test]
    fn verify_signature_rejects_tampered_blob() {
        let sk = SigningKey::generate(&mut OsRng);
        let body = b"release-binary-bytes";
        let sig: Signature = sk.sign(body);
        let tmp = TempDir::new().unwrap();
        // Write the *tampered* body but the original sig.
        let (bin, sig_path) = write_pair(&tmp, b"tampered-bytes", &sig.to_bytes());
        assert!(verify_signature(&sk.verifying_key(), &bin, &sig_path).is_err());
    }

    #[test]
    fn verify_signature_rejects_wrong_key() {
        let signer = SigningKey::generate(&mut OsRng);
        let other = SigningKey::generate(&mut OsRng);
        let body = b"release-binary-bytes";
        let sig: Signature = signer.sign(body);
        let tmp = TempDir::new().unwrap();
        let (bin, sig_path) = write_pair(&tmp, body, &sig.to_bytes());
        assert!(verify_signature(&other.verifying_key(), &bin, &sig_path).is_err());
    }

    #[test]
    fn verify_signature_rejects_short_sig() {
        let sk = SigningKey::generate(&mut OsRng);
        let tmp = TempDir::new().unwrap();
        let (bin, sig_path) = write_pair(&tmp, b"body", b"too-short");
        assert!(verify_signature(&sk.verifying_key(), &bin, &sig_path).is_err());
    }

    /// Mockito round-trip: fetch_latest_release_from_url against a fake
    /// GitHub API endpoint. Exercises the JSON parse + Release shape.
    #[tokio::test]
    async fn fetch_latest_release_parses_mock_response() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "tag_name": "v0.9.0-wayland-base",
            "assets": [
                {"name": "wayland-core-x86_64-unknown-linux-gnu",
                 "browser_download_url": "https://example.test/bin"},
                {"name": "wayland-core-x86_64-unknown-linux-gnu.sig",
                 "browser_download_url": "https://example.test/bin.sig"}
            ]
        });
        let mock = server
            .mock("GET", "/repos/FerroxLabs/wayland-core/releases/latest")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;
        let url = format!(
            "{}/repos/FerroxLabs/wayland-core/releases/latest",
            server.url()
        );
        let release = fetch_latest_release_from_url(&url).await.unwrap().unwrap();
        mock.assert_async().await;
        assert_eq!(release.version(), "0.9.0");
        assert_eq!(release.assets.len(), 2);
        assert_eq!(
            release.assets[0].name,
            "wayland-core-x86_64-unknown-linux-gnu"
        );
    }

    /// F-029: 404 means "no releases yet" — returns Ok(None), not Err.
    #[tokio::test]
    async fn fetch_latest_release_returns_none_on_404() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/repos/FerroxLabs/wayland-core/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        let url = format!(
            "{}/repos/FerroxLabs/wayland-core/releases/latest",
            server.url()
        );
        let result = fetch_latest_release_from_url(&url).await.unwrap();
        mock.assert_async().await;
        assert!(result.is_none(), "404 should return Ok(None), not Err");
    }

    /// F-029: other non-2xx errors still return Err (broken repo / server).
    #[tokio::test]
    async fn fetch_latest_release_errors_on_500() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/repos/FerroxLabs/wayland-core/releases/latest")
            .with_status(500)
            .create_async()
            .await;
        let url = format!(
            "{}/repos/FerroxLabs/wayland-core/releases/latest",
            server.url()
        );
        let result = fetch_latest_release_from_url(&url).await;
        mock.assert_async().await;
        assert!(result.is_err(), "non-404 error should return Err");
    }

    /// Smoke: the const placeholder pubkey decodes but is all-zero. It
    /// MUST be rejected by ed25519 verification (zero point has no
    /// matching signatures for normal release payloads). This guards
    /// against a release where the placeholder ships unrotated — the
    /// CLI will fail-closed.
    #[test]
    fn placeholder_pubkey_decodes() {
        let vk = parse_pubkey_b64(MARKETPLACE_PUBKEY_B64).unwrap();
        // All-zero is a valid (if degenerate) ed25519 point; the load
        // succeeds, but any non-trivial signed payload will fail to
        // verify against it. We assert the decode succeeds and the key
        // bytes are zero.
        assert!(vk.as_bytes().iter().all(|b| *b == 0));
    }

    #[test]
    fn load_pinned_pubkey_uses_env_override() {
        use base64::Engine;
        let sk = SigningKey::generate(&mut OsRng);
        let b64 = base64::engine::general_purpose::STANDARD.encode(sk.verifying_key().as_bytes());
        // SAFETY: tests run serial within a process; set + read + unset
        // around a single call.
        // SAFETY (env): single-threaded for the duration of this test
        // body. We restore the original value if present.
        let original = std::env::var(ENV_PUBKEY_B64).ok();
        // SAFETY: setting env vars is unsafe in newer Rust editions.
        unsafe {
            std::env::set_var(ENV_PUBKEY_B64, &b64);
        }
        let loaded = load_pinned_pubkey().unwrap();
        assert_eq!(loaded.as_bytes(), sk.verifying_key().as_bytes());
        unsafe {
            match original {
                Some(v) => std::env::set_var(ENV_PUBKEY_B64, v),
                None => std::env::remove_var(ENV_PUBKEY_B64),
            }
        }
    }
}
