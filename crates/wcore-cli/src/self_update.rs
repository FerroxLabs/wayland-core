//! `wayland-core self-update` subcommand.
//!
//! Pulls the latest release from GitHub (`FerroxLabs/wayland-core`),
//! verifies its **keyless Sigstore build-provenance attestation** via the
//! GitHub CLI (`gh attestation verify`), extracts the binary from the
//! release archive, and atomically replaces the running binary via
//! `self_replace`.
//!
//! Why keyless (not a pinned ed25519 release key): the prior scheme shipped
//! an all-zero placeholder key and the release pipeline never produced `.sig`
//! files, so self-update could not work and carried long-lived-key custody
//! debt. Releases are now signed keylessly via GitHub OIDC + Sigstore
//! (`actions/attest-build-provenance` in `release.yml`); verification needs no
//! pinned key — `gh` checks the attestation against the source repo and the
//! public Sigstore transparency log. See finding R16.
//!
//! Threat model:
//! - The release API URL is a static const; we never interpolate user input
//!   into a host. The archive URL comes straight from the GitHub API response
//!   (`browser_download_url`).
//! - Provenance is verified against the pinned source repo
//!   (`FerroxLabs/wayland-core`): a binary not built by that repo's release
//!   workflow fails verification, so a swapped/tampered archive is rejected
//!   before extraction.
//! - The download is size-checked against the `Content-Length` header when
//!   present; verification then runs over the downloaded archive regardless.
//! - `gh` must be present. If it is not, we refuse to install rather than
//!   skip verification — fail closed, with guidance to install `gh` or
//!   reinstall via npm (itself provenance-backed).

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// The release-lifecycle half of this subcommand: the ordered update decision,
/// the bundled release trust root, persisted freeze protection and revocation
/// enforcement.
///
/// `#[path]` places it at `src/update_trust.rs` rather than
/// `src/self_update/update_trust.rs`, and declaring it HERE rather than in
/// `lib.rs` keeps this change off the two files every concurrent lane shares.
/// `self_update.rs` is 631 lines at base and AGENTS.md caps a module at 1000,
/// so the new logic is a sibling module rather than growth in place.
#[path = "update_trust.rs"]
pub mod update_trust;

/// GitHub repo that hosts wayland-core releases. Pinned to the production
/// org so a misconfigured workspace cannot redirect updates elsewhere.
pub const RELEASES_REPO: &str = "FerroxLabs/wayland-core";

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

    let tmp = tempfile::tempdir()?;
    let now_unix = unix_now();

    // Release trust root and signed manifest, resolved BEFORE the version
    // decision so a placeholder trust root is reported to the user here rather
    // than discovered at swap time. This is additive: it neither replaces nor
    // weakens the keyless attestation check further down, which still runs over
    // the downloaded archive and still fails closed.
    let manifest_path = tmp.path().join("release-manifest.json");
    let (manifest, manifest_status) =
        load_release_manifest(&release, &manifest_path, now_unix).await;
    println!("manifest: {manifest_status}");

    // A revocation the user never learns about protects nobody, so a revoked
    // RUNNING version is reported whatever the decision turns out to be.
    for line in update_trust::check_only_report(manifest.as_ref(), current_version) {
        println!("{line}");
    }

    // The install-or-not choice: a pure function of the running version, the
    // offered release, the verified manifest and the persisted state. It reads
    // no network, no environment and no clock beyond the injected instant.
    let freeze_state = update_trust::FreezeState::load();
    let decision = update_trust::decide_update(&update_trust::UpdateOffer {
        running_version: current_version,
        offered_version: latest_version,
        manifest: manifest.as_ref(),
        state: &freeze_state,
        now_unix,
        max_manifest_age_secs: update_trust::DEFAULT_MAX_MANIFEST_AGE_SECS,
    });

    let accepted_sequence = match &decision {
        update_trust::UpdateDecision::AlreadyUpToDate { .. } => {
            println!("already up to date.");
            return Ok(());
        }
        update_trust::UpdateDecision::Proceed { sequence, .. } => *sequence,
        refusal => {
            // A check-only run REPORTS a refusal and exits cleanly; an install
            // run fails, because refusing to install is the outcome.
            if check_only {
                println!("{}", refusal.message());
                return Ok(());
            }
            bail!("{}", refusal.message());
        }
    };

    if check_only {
        println!("(check-only: not installing)");
        return Ok(());
    }

    // The release packages one archive per target (see release.yml):
    // `wayland-core-vX.Y.Z-<triple>.{tar.gz,zip}`. Match that exactly.
    let archive_name = archive_name_for_host(latest_version);

    let asset = release
        .assets
        .iter()
        .find(|a| a.name == archive_name)
        .with_context(|| format!("no {archive_name} in release v{latest_version}"))?;

    let archive_path = tmp.path().join(&archive_name);
    download_to(&asset.browser_download_url, &archive_path).await?;

    // Bind the bytes we actually downloaded to the artifact the signed
    // manifest names. Without this the manifest's artifact digests would be
    // decorative: a correctly signed manifest would sit beside whatever
    // archive the source chose to hand over. ANDed with the attestation check
    // below, never a substitute for it.
    if let Some(manifest) = manifest.as_ref() {
        let (digest, bytes) = digest_file(&archive_path)?;
        manifest
            .check_archive(&archive_name, &digest, bytes)
            .map_err(|error| anyhow::anyhow!("{error}"))
            .context("refusing to install an archive the signed manifest does not vouch for")?;
    }

    // Keyless provenance check BEFORE we extract or swap anything.
    verify_provenance(&archive_path, RELEASES_REPO)
        .await
        .context("provenance verification failed — refusing to install untrusted binary")?;

    let unpack_dir = tmp.path().join("unpack");
    std::fs::create_dir(&unpack_dir).context("create unpack dir")?;
    let bin_path = extract_binary(&archive_path, &unpack_dir)
        .context("extract binary from verified release archive")?;

    atomic_swap(&bin_path)?;

    // The high-water mark advances ONLY here, after a successful install —
    // never on a decision, and never on a refusal. A failure to persist it is
    // reported but does not un-install a binary that is already swapped in.
    if let Err(error) = update_trust::FreezeState::record_install(accepted_sequence, now_unix) {
        eprintln!("warning: could not persist the release freeze high-water mark: {error}");
    }

    println!("upgraded to v{latest_version}");
    Ok(())
}

/// SHA-256 and byte length of a file on disk, streamed so a large archive is
/// never held whole in memory.
fn digest_file(path: &Path) -> Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher)
        .with_context(|| format!("digest {}", path.display()))?;
    Ok((format!("{:x}", hasher.finalize()), bytes))
}

/// Seconds since the Unix epoch. A clock that predates the epoch yields 0,
/// which the age rule then treats as maximally stale — fail closed.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// Fetch and verify the release's signed manifest, returning it alongside a
/// user-facing status line.
///
/// Never returns an error: every failure mode degrades to `None` plus a status
/// the caller prints, and it is [`update_trust::decide_update`] that decides
/// what a missing manifest means. That keeps the policy in one pure place
/// instead of scattering fail-open judgements through the fetch path.
async fn load_release_manifest(
    release: &Release,
    dest: &Path,
    now_unix: u64,
) -> (Option<update_trust::VerifiedManifest>, String) {
    let verifier = match update_trust::ReleaseVerifier::bundled() {
        Ok(verifier) => verifier,
        Err(error) => return (None, format!("UNAVAILABLE — {error}")),
    };

    let Some(asset) = release.assets.iter().find(|asset| {
        asset
            .name
            .ends_with(update_trust::RELEASE_MANIFEST_ASSET_SUFFIX)
    }) else {
        return (
            None,
            format!(
                "UNAVAILABLE — release {} publishes no *{} asset",
                release.tag,
                update_trust::RELEASE_MANIFEST_ASSET_SUFFIX
            ),
        );
    };

    if let Err(error) = download_to(&asset.browser_download_url, dest).await {
        return (
            None,
            format!("UNAVAILABLE — could not download {}: {error:#}", asset.name),
        );
    }
    let bytes = match std::fs::read(dest) {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                None,
                format!("UNAVAILABLE — could not read {}: {error}", dest.display()),
            );
        }
    };

    match verifier.verify_manifest_json(&bytes, now_unix) {
        Ok(manifest) => {
            let status = format!(
                "VERIFIED {} sequence={} key_id={}",
                asset.name,
                manifest.sequence(),
                manifest.signing_key_id()
            );
            (Some(manifest), status)
        }
        Err(error) => (None, format!("REFUSED — {error}")),
    }
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
    ///
    /// Delegates to [`update_trust::normalize_version_tag`] so this rule has
    /// exactly one implementation: the ordering comparison and the printed
    /// version can never disagree about what a release tag means.
    pub fn version(&self) -> &str {
        update_trust::normalize_version_tag(&self.tag)
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

/// Map `(target_os, target_arch)` → the Rust target triple used in release
/// artifact names. Matches the build matrix in `.github/workflows/release.yml`.
pub fn target_triple_for(os: &str, arch: &str) -> String {
    match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin".into(),
        ("macos", "x86_64") => "x86_64-apple-darwin".into(),
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu".into(),
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu".into(),
        ("windows", "x86_64") => "x86_64-pc-windows-msvc".into(),
        ("windows", "aarch64") => "aarch64-pc-windows-msvc".into(),
        (o, a) => format!("{a}-unknown-{o}"),
    }
}

/// Release archive filename for the host, e.g.
/// `wayland-core-v0.11.0-aarch64-apple-darwin.tar.gz`. Windows targets ship a
/// `.zip`; every other target ships `.tar.gz` (see release.yml packaging).
pub fn archive_name_for_host(version: &str) -> String {
    archive_name_for(version, std::env::consts::OS, std::env::consts::ARCH)
}

/// Pure mapping for tests.
pub fn archive_name_for(version: &str, os: &str, arch: &str) -> String {
    let triple = target_triple_for(os, arch);
    let ext = if os == "windows" { "zip" } else { "tar.gz" };
    format!("wayland-core-v{version}-{triple}.{ext}")
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
// Keyless provenance verification
// ---------------------------------------------------------------------

/// Verify the release archive's Sigstore build-provenance attestation via the
/// GitHub CLI, pinning the source repo. Fails closed: a missing `gh`, a failed
/// check, or any non-zero exit is an error — never a silent skip.
pub async fn verify_provenance(archive_path: &Path, repo: &str) -> Result<()> {
    verify_provenance_with("gh", archive_path, repo).await
}

/// Inner form with an injectable program name so tests can exercise the
/// `gh`-missing path without a real `gh` on the box.
async fn verify_provenance_with(program: &str, archive_path: &Path, repo: &str) -> Result<()> {
    let archive = archive_path
        .to_str()
        .context("archive path is not valid UTF-8")?;
    // argv mode: no shell is involved, so metacharacters in arguments are never
    // interpreted. `archive` is a path we just created under our own tempdir
    // (never attacker-controlled) and `repo` is a compile-time const.
    let output = wcore_config::shell::shell_command_argv(
        program,
        &["attestation", "verify", archive, "--repo", repo],
    )
    .output()
    .await
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "GitHub CLI (`gh`) not found — it is required to verify release \
                 provenance. Install it from https://cli.github.com, or update via \
                 npm instead: npm install -g @ferroxlabs/wayland-core@latest"
            )
        } else {
            anyhow::Error::new(e).context("spawn `gh attestation verify`")
        }
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`gh attestation verify` rejected {}: {}",
            archive_path.display(),
            stderr.trim()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------

/// Extract the wayland-core binary from a verified release archive into
/// `dest_dir`, returning the path to the extracted executable. Supports
/// `.tar.gz` (macOS/Linux) and `.zip` (Windows). Call only AFTER provenance
/// verification has succeeded.
pub fn extract_binary(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("archive has no filename")?;
    if name.ends_with(".zip") {
        extract_zip(archive_path, dest_dir)
    } else if name.ends_with(".tar.gz") {
        extract_tar_gz(archive_path, dest_dir)
    } else {
        bail!("unrecognized archive extension for {name}");
    }
}

fn extract_tar_gz(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let f = std::fs::File::open(archive_path)
        .with_context(|| format!("open {}", archive_path.display()))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut ar = tar::Archive::new(gz);
    ar.unpack(dest_dir)
        .with_context(|| format!("unpack tar.gz into {}", dest_dir.display()))?;
    find_extracted_binary(dest_dir)
}

fn extract_zip(archive_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let f = std::fs::File::open(archive_path)
        .with_context(|| format!("open {}", archive_path.display()))?;
    let mut zip = zip::ZipArchive::new(f).context("read zip archive")?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("read zip entry")?;
        if entry.is_dir() {
            continue;
        }
        // Zip-slip guard: reject any entry whose path escapes via `..` or an
        // absolute root, and write only the flat filename into dest_dir.
        let enclosed = entry
            .enclosed_name()
            .context("zip entry has an unsafe path")?;
        let Some(file_name) = enclosed.file_name() else {
            continue;
        };
        let out = dest_dir.join(file_name);
        let mut w =
            std::fs::File::create(&out).with_context(|| format!("create {}", out.display()))?;
        std::io::copy(&mut entry, &mut w).context("write zip entry")?;
    }
    find_extracted_binary(dest_dir)
}

/// Locate the extracted wayland-core executable in `dir`. The release archive
/// holds exactly the binary (`wayland-core` or `wayland-core.exe`).
fn find_extracted_binary(dir: &Path) -> Result<PathBuf> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        if let Some(stem) = path.file_name().and_then(|n| n.to_str())
            && (stem == "wayland-core" || stem == "wayland-core.exe")
        {
            return Ok(path);
        }
    }
    bail!(
        "no wayland-core binary found in extracted archive at {}",
        dir.display()
    );
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
    use std::io::Write;
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
    fn target_triple_known_hosts() {
        assert_eq!(
            target_triple_for("macos", "aarch64"),
            "aarch64-apple-darwin"
        );
        assert_eq!(target_triple_for("macos", "x86_64"), "x86_64-apple-darwin");
        assert_eq!(
            target_triple_for("linux", "x86_64"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(
            target_triple_for("windows", "x86_64"),
            "x86_64-pc-windows-msvc"
        );
    }

    #[test]
    fn archive_name_unix_is_tar_gz() {
        assert_eq!(
            archive_name_for("0.11.0", "linux", "x86_64"),
            "wayland-core-v0.11.0-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            archive_name_for("0.11.0", "macos", "aarch64"),
            "wayland-core-v0.11.0-aarch64-apple-darwin.tar.gz"
        );
    }

    #[test]
    fn archive_name_windows_is_zip() {
        assert_eq!(
            archive_name_for("0.11.0", "windows", "x86_64"),
            "wayland-core-v0.11.0-x86_64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn archive_name_for_host_matches_known_shape() {
        let n = archive_name_for_host("9.9.9");
        assert!(n.starts_with("wayland-core-v9.9.9-"), "got {n}");
        assert!(n.ends_with(".tar.gz") || n.ends_with(".zip"), "got {n}");
    }

    /// A `.tar.gz` holding the binary round-trips through extract_binary.
    #[test]
    fn extract_tar_gz_recovers_binary() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp
            .path()
            .join("wayland-core-v9.9.9-x86_64-unknown-linux-gnu.tar.gz");
        let body = b"#!/bin/sh\necho wayland\n";
        {
            let f = std::fs::File::create(&archive).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut builder = tar::Builder::new(enc);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "wayland-core", &body[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let dest = tmp.path().join("unpack");
        std::fs::create_dir(&dest).unwrap();
        let bin = extract_binary(&archive, &dest).unwrap();
        assert_eq!(bin.file_name().unwrap(), "wayland-core");
        assert_eq!(std::fs::read(&bin).unwrap(), body);
    }

    /// A `.zip` holding the Windows binary round-trips through extract_binary.
    #[test]
    fn extract_zip_recovers_binary() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp
            .path()
            .join("wayland-core-v9.9.9-x86_64-pc-windows-msvc.zip");
        let body = b"MZ\x90\x00fake-windows-binary";
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            zw.start_file("wayland-core.exe", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(body).unwrap();
            zw.finish().unwrap();
        }
        let dest = tmp.path().join("unpack");
        std::fs::create_dir(&dest).unwrap();
        let bin = extract_binary(&archive, &dest).unwrap();
        assert_eq!(bin.file_name().unwrap(), "wayland-core.exe");
        assert_eq!(std::fs::read(&bin).unwrap(), body);
    }

    #[test]
    fn extract_binary_rejects_unknown_extension() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("wayland-core-v9.9.9-weird.rar");
        std::fs::write(&archive, b"junk").unwrap();
        assert!(extract_binary(&archive, tmp.path()).is_err());
    }

    /// Provenance verification fails closed when `gh` is absent: the error must
    /// carry actionable install guidance, never a silent skip.
    #[tokio::test]
    async fn verify_provenance_fails_closed_without_gh() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("artifact.tar.gz");
        std::fs::write(&archive, b"bytes").unwrap();
        let err = verify_provenance_with(
            "wayland-core-no-such-gh-binary-xyz",
            &archive,
            "FerroxLabs/wayland-core",
        )
        .await
        .expect_err("missing gh must error, not pass");
        let msg = format!("{err:#}");
        assert!(msg.contains("GitHub CLI"), "got: {msg}");
    }

    /// Mockito round-trip: fetch_latest_release_from_url against a fake
    /// GitHub API endpoint. Exercises the JSON parse + Release shape.
    #[tokio::test]
    async fn fetch_latest_release_parses_mock_response() {
        let mut server = mockito::Server::new_async().await;
        let body = serde_json::json!({
            "tag_name": "v0.9.0-wayland-base",
            "assets": [
                {"name": "wayland-core-v0.9.0-x86_64-unknown-linux-gnu.tar.gz",
                 "browser_download_url": "https://example.test/archive"}
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
        assert_eq!(release.assets.len(), 1);
        assert_eq!(
            release.assets[0].name,
            "wayland-core-v0.9.0-x86_64-unknown-linux-gnu.tar.gz"
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
}
