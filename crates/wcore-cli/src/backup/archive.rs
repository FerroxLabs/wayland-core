//! Archive creation and verification (F26-03).
//!
//! # Format
//!
//! A gzip-compressed tar carrying, in this order:
//!
//! 1. `manifest.json` — every payload with its size and SHA-256, the whole-tree
//!    digest, and the credential capture record.
//! 2. `payload/<root-relative path>` — one entry per carried file.
//!
//! The manifest is EMBEDDED rather than written alongside, so verification has
//! something to check against even when the archive has been moved between
//! machines, and so a manifest cannot be separated from the bytes it describes.
//!
//! # Verification can actually fail, in three distinct ways
//!
//! An archive is an untrusted input by the time it is restored. Verification
//! rejects, separately and with distinguishable errors:
//!
//! * a payload whose bytes do not match its declared digest (tampering);
//! * an entry path that escapes the extraction root (`..`, absolute, or a drive
//!   prefix) — the zip-slip class;
//! * a manifest entry with no corresponding payload in the archive.
//!
//! One combined "verification fails on a bad archive" check would hide which of
//! the three is unimplemented, so each is asserted on its own.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use super::{BackupError, Payload, collect_payloads, sha256_hex};

/// The manifest entry name inside the archive.
pub const MANIFEST_ENTRY: &str = "manifest.json";
/// The prefix every carried file sits under.
pub const PAYLOAD_PREFIX: &str = "payload/";

/// The digest algorithm and normalization identifier, printed by BOTH platforms'
/// interruption proofs so a cross-platform digest comparison can be shown to be
/// measuring content rather than encoding.
///
/// `content=raw-bytes` is deliberate and is the honest statement: file content is
/// hashed exactly as stored, with NO line-ending normalization. A backup that
/// normalized line endings would not be exact, and exactness is the property this
/// phase exists to prove.
pub const DIGEST_ALGO: &str =
    "sha256/wcore-portability-tree-v1 path-norm=slash-relative content=raw-bytes";

/// One carried file, as the manifest records it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PayloadEntry {
    /// Root-relative, `/`-separated. The single identity shared by the manifest,
    /// the tar entry name and the restored path.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    /// Unix mode where the source had one. Applied on restore only on unix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

/// What the archive could and could not capture about the source's credentials.
///
/// This is the record that stops an archive from presenting itself as a complete
/// capture when the source's secrets lived somewhere the archive cannot reach.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialCapture {
    /// `auto`, `plaintext`, `keyring` or `encrypted_file`, as read from the
    /// source home's `config.toml`. `unknown` when the home declares none.
    pub backend: String,
    /// Whether the in-tree secret entries were carried.
    pub carried: bool,
    /// Absolute paths the source config names for out-of-tree secret material
    /// (the `encrypted_file` backend's `cipher_path` / `key_params_path`), keyed
    /// by config field. These are machine-specific and must never survive a
    /// restore verbatim.
    #[serde(default)]
    pub external_paths: BTreeMap<String, String>,
    /// True when the backend keeps secrets somewhere the filesystem archive
    /// cannot reach at all (the OS keyring).
    pub secrets_outside_tree: bool,
}

/// The embedded manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub created_utc: String,
    pub digest_algo: String,
    /// Digest over the CARRIED payload set, computed from the manifest's own
    /// entries so it can be re-derived from the archive without the source.
    pub tree_digest: String,
    pub payloads: Vec<PayloadEntry>,
    pub credentials: CredentialCapture,
    /// Top-level secret entry NAMES the archive deliberately did not carry.
    /// This is precisely what a redacted archive cannot round-trip.
    #[serde(default)]
    pub absent_secrets: Vec<String>,
}

pub const FORMAT_ID: &str = "wayland-core-backup";
pub const FORMAT_VERSION: u32 = 1;

impl Manifest {
    /// Digest over the payload set, derived from the manifest alone.
    ///
    /// Same construction as `wcore_config::portability::tree_digest`: a total
    /// order from the data, each entry length-prefixed so no combination of path
    /// and hash can be re-parsed as a different pair.
    pub(crate) fn compute_tree_digest(payloads: &[PayloadEntry]) -> String {
        use sha2::{Digest, Sha256};
        let ordered: BTreeMap<&str, &str> = payloads
            .iter()
            .map(|p| (p.path.as_str(), p.sha256.as_str()))
            .collect();
        let mut h = Sha256::new();
        for (rel, content_hash) in ordered {
            h.update(rel.len().to_le_bytes());
            h.update(rel.as_bytes());
            h.update(content_hash.as_bytes());
        }
        super::hex(&h.finalize())
    }
}

/// Create an archive of `home` at `out`.
///
/// Refuses, before writing anything:
/// * a missing source;
/// * an existing file at `out` (publication is never a silent replacement);
/// * an `out` path that lies inside `home` (an archive containing itself grows
///   until the disk does).
pub fn create_archive(
    home: &Path,
    out: &Path,
    include_secrets: bool,
) -> Result<Manifest, BackupError> {
    if !home.is_dir() {
        return Err(BackupError::SourceMissing(home.to_path_buf()));
    }
    if out.exists() {
        return Err(BackupError::OutputExists(out.to_path_buf()));
    }
    if output_is_inside_source(home, out) {
        return Err(BackupError::OutputInsideSource(out.to_path_buf()));
    }

    let (payloads, omitted) = collect_payloads(home, !include_secrets)?;
    let credentials = super::remap::capture_credentials(home, include_secrets)?;

    let mut entries = Vec::with_capacity(payloads.len());
    let mut blobs: Vec<(String, Vec<u8>)> = Vec::with_capacity(payloads.len());
    for Payload { rel, abs } in &payloads {
        let bytes = std::fs::read(abs).map_err(BackupError::io("read payload"))?;
        entries.push(PayloadEntry {
            path: rel.clone(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            mode: file_mode(abs),
        });
        blobs.push((rel.clone(), bytes));
    }

    let manifest = Manifest {
        format: FORMAT_ID.to_string(),
        version: FORMAT_VERSION,
        created_utc: chrono::Utc::now().to_rfc3339(),
        digest_algo: DIGEST_ALGO.to_string(),
        tree_digest: Manifest::compute_tree_digest(&entries),
        payloads: entries,
        credentials,
        absent_secrets: omitted,
    };

    let bytes = pack(&manifest, &blobs)?;

    // Publish through the existing atomic primitive: a crash during publication
    // leaves either no archive or a complete one, never a truncated file that
    // verification would have to distinguish from tampering.
    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(BackupError::io("create archive parent"))?;
    }
    wcore_config::atomic_io::atomic_write(out, &bytes)
        .map_err(BackupError::io("publish archive"))?;

    Ok(manifest)
}

pub(crate) fn pack(
    manifest: &Manifest,
    blobs: &[(String, Vec<u8>)],
) -> Result<Vec<u8>, BackupError> {
    use flate2::Compression;
    use flate2::write::GzEncoder;

    let manifest_bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|e| BackupError::Journal(format!("serialize manifest: {e}")))?;

    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, MANIFEST_ENTRY, manifest_bytes.as_slice())
        .map_err(BackupError::io("append manifest"))?;

    for (rel, bytes) in blobs {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        let name = format!("{PAYLOAD_PREFIX}{rel}");
        builder
            .append_data(&mut h, &name, bytes.as_slice())
            .map_err(BackupError::io("append payload"))?;
    }

    let encoder = builder
        .into_inner()
        .map_err(BackupError::io("finish tar"))?;
    encoder.finish().map_err(BackupError::io("finish gzip"))
}

/// Read every entry out of an archive: the manifest plus a map of payload path
/// to bytes. Rejects a traversal path before any content is trusted.
pub(crate) fn unpack(path: &Path) -> Result<(Manifest, BTreeMap<String, Vec<u8>>), BackupError> {
    let file = std::fs::File::open(path).map_err(BackupError::io("open archive"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(decoder);

    let mut manifest: Option<Manifest> = None;
    let mut payloads: BTreeMap<String, Vec<u8>> = BTreeMap::new();

    let entries = ar
        .entries()
        .map_err(|e| BackupError::NotAnArchive(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| BackupError::NotAnArchive(e.to_string()))?;
        // Read the raw recorded name. Never `entry.path()`, which normalizes and
        // can hide the very component being checked for.
        let raw = String::from_utf8_lossy(&entry.path_bytes()).into_owned();

        if raw == MANIFEST_ENTRY {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(BackupError::io("read manifest entry"))?;
            let m: Manifest = serde_json::from_slice(&buf)
                .map_err(|e| BackupError::NotAnArchive(format!("manifest is not valid: {e}")))?;
            manifest = Some(m);
            continue;
        }

        let Some(rel) = raw.strip_prefix(PAYLOAD_PREFIX) else {
            return Err(BackupError::VerificationFailed(format!(
                "archive entry '{raw}' is outside the '{PAYLOAD_PREFIX}' namespace"
            )));
        };
        reject_traversal(rel)?;

        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(BackupError::io("read payload entry"))?;
        payloads.insert(rel.to_string(), buf);
    }

    let manifest = manifest.ok_or_else(|| {
        BackupError::NotAnArchive(format!("no {MANIFEST_ENTRY} entry in archive"))
    })?;
    if manifest.format != FORMAT_ID {
        return Err(BackupError::NotAnArchive(format!(
            "format is '{}', expected '{FORMAT_ID}'",
            manifest.format
        )));
    }
    Ok((manifest, payloads))
}

/// Reject any archive-relative path that could escape the extraction root.
///
/// Checked on the RAW recorded name, so an absolute path, a `..` component, a
/// Windows drive prefix or a UNC prefix is refused before the bytes are used.
pub(crate) fn reject_traversal(rel: &str) -> Result<(), BackupError> {
    if rel.is_empty() {
        return Err(BackupError::VerificationFailed(
            "archive carries an entry with an empty path".to_string(),
        ));
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(BackupError::VerificationFailed(format!(
            "archive entry path is absolute and would escape the extraction root: '{rel}'"
        )));
    }
    // A Windows drive or UNC prefix, spelled with either separator.
    let bytes = rel.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return Err(BackupError::VerificationFailed(format!(
            "archive entry path carries a drive prefix and would escape the extraction root: '{rel}'"
        )));
    }
    for component in rel.split(['/', '\\']) {
        if component == ".." {
            return Err(BackupError::VerificationFailed(format!(
                "archive entry path escapes the extraction root: '{rel}'"
            )));
        }
    }
    // Belt and braces: reparse through the platform's own component model, so a
    // shape this string check did not anticipate is still refused.
    if Path::new(rel)
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(BackupError::VerificationFailed(format!(
            "archive entry path is not a plain relative path: '{rel}'"
        )));
    }
    Ok(())
}

/// Verify an archive without writing anything.
///
/// Returns the manifest on success. Every failure names which of the three
/// rejections fired.
pub fn verify_archive(path: &Path) -> Result<Manifest, BackupError> {
    let (manifest, payloads) = unpack(path)?;

    for entry in &manifest.payloads {
        // Rejection 3: the manifest declares a payload the archive does not
        // contain. Checked before the digest so a missing payload is never
        // reported as a digest mismatch.
        let Some(bytes) = payloads.get(&entry.path) else {
            return Err(BackupError::VerificationFailed(format!(
                "manifest declares payload '{}' which the archive does not contain",
                entry.path
            )));
        };
        // Rejection 1: the bytes do not match the declared digest.
        let actual = sha256_hex(bytes);
        if actual != entry.sha256 {
            return Err(BackupError::VerificationFailed(format!(
                "payload '{}' does not match its declared digest (declared {}, actual {})",
                entry.path, entry.sha256, actual
            )));
        }
        if bytes.len() as u64 != entry.bytes {
            return Err(BackupError::VerificationFailed(format!(
                "payload '{}' declares {} bytes but carries {}",
                entry.path,
                entry.bytes,
                bytes.len()
            )));
        }
    }

    // An archive carrying payloads the manifest never declared is also a
    // mismatch: it would restore files nothing vouched for.
    for name in payloads.keys() {
        if !manifest.payloads.iter().any(|p| &p.path == name) {
            return Err(BackupError::VerificationFailed(format!(
                "archive carries payload '{name}' which the manifest does not declare"
            )));
        }
    }

    let recomputed = Manifest::compute_tree_digest(&manifest.payloads);
    if recomputed != manifest.tree_digest {
        return Err(BackupError::VerificationFailed(format!(
            "manifest tree digest does not match its own payload set (declared {}, actual {recomputed})",
            manifest.tree_digest
        )));
    }

    Ok(manifest)
}

/// True when `out` would be written inside `home`.
///
/// Compared on canonicalized paths where possible so a symlinked or
/// `..`-containing spelling of the same location is still caught. `out` does not
/// exist yet, so its PARENT is the thing canonicalized.
fn output_is_inside_source(home: &Path, out: &Path) -> bool {
    let home_c = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let parent = out.parent().unwrap_or_else(|| Path::new("."));
    let parent_c = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    parent_c.starts_with(&home_c)
}

fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).ok().map(|m| m.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_home(home: &Path) {
        std::fs::create_dir_all(home.join("skills/demo")).unwrap();
        std::fs::write(home.join("config.toml"), "[storage]\n").unwrap();
        std::fs::write(home.join("skills/demo/SKILL.md"), "hello").unwrap();
    }

    #[test]
    fn create_embeds_a_manifest_naming_every_payload_and_verify_passes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);
        let out = dir.path().join("b.tar.gz");

        let m = create_archive(&home, &out, false).unwrap();
        assert_eq!(m.format, FORMAT_ID);
        let names: Vec<&str> = m.payloads.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(names, vec!["config.toml", "skills/demo/SKILL.md"]);
        assert!(m.payloads.iter().all(|p| p.sha256.len() == 64));

        let verified = verify_archive(&out).unwrap();
        assert_eq!(verified.tree_digest, m.tree_digest);
    }

    #[test]
    fn create_refuses_to_overwrite_an_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);
        let out = dir.path().join("b.tar.gz");
        std::fs::write(&out, b"PRIOR").unwrap();

        let err = create_archive(&home, &out, false).unwrap_err();
        assert!(matches!(err, BackupError::OutputExists(_)), "{err:?}");
        // The refusal must leave the prior file untouched, not half-replaced.
        assert_eq!(std::fs::read(&out).unwrap(), b"PRIOR");
    }

    #[test]
    fn create_refuses_an_output_path_inside_the_tree_being_archived() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);

        let err = create_archive(&home, &home.join("self.tar.gz"), false).unwrap_err();
        assert!(matches!(err, BackupError::OutputInsideSource(_)), "{err:?}");

        // Nested, and spelled via `..` so the check is not a prefix-string test.
        let sneaky = home.join("skills/../inner.tar.gz");
        let err = create_archive(&home, &sneaky, false).unwrap_err();
        assert!(matches!(err, BackupError::OutputInsideSource(_)), "{err:?}");
    }

    #[test]
    fn verify_rejects_a_tampered_payload() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);
        let out = dir.path().join("b.tar.gz");
        let m = create_archive(&home, &out, false).unwrap();

        // Repack the SAME manifest over altered bytes: the archive is
        // well-formed and only the content lies, which is the tampering case.
        let (_, mut payloads) = unpack(&out).unwrap();
        payloads.insert("config.toml".to_string(), b"TAMPERED".to_vec());
        let blobs: Vec<(String, Vec<u8>)> = payloads.into_iter().collect();
        let bytes = pack(&m, &blobs).unwrap();
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, bytes).unwrap();

        let err = verify_archive(&bad).unwrap_err();
        match err {
            BackupError::VerificationFailed(msg) => {
                assert!(msg.contains("does not match its declared digest"), "{msg}")
            }
            other => panic!("expected a digest rejection, got {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_a_manifest_entry_with_no_payload() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);
        let out = dir.path().join("b.tar.gz");
        let mut m = create_archive(&home, &out, false).unwrap();

        // Declare a payload that is not in the archive.
        m.payloads.push(PayloadEntry {
            path: "ghost.txt".to_string(),
            bytes: 3,
            sha256: sha256_hex(b"abc"),
            mode: None,
        });
        m.tree_digest = Manifest::compute_tree_digest(&m.payloads);
        let (_, payloads) = unpack(&out).unwrap();
        let blobs: Vec<(String, Vec<u8>)> = payloads.into_iter().collect();
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, pack(&m, &blobs).unwrap()).unwrap();

        let err = verify_archive(&bad).unwrap_err();
        match err {
            BackupError::VerificationFailed(msg) => {
                assert!(msg.contains("which the archive does not contain"), "{msg}")
            }
            other => panic!("expected a missing-payload rejection, got {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_a_traversal_path_inside_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_home(&home);
        let out = dir.path().join("b.tar.gz");
        let m = create_archive(&home, &out, false).unwrap();

        // A payload whose recorded name climbs out of the extraction root.
        let blobs = vec![("../../escape.txt".to_string(), b"pwned".to_vec())];
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, pack(&m, &blobs).unwrap()).unwrap();

        let err = verify_archive(&bad).unwrap_err();
        match err {
            BackupError::VerificationFailed(msg) => {
                assert!(msg.contains("escapes the extraction root"), "{msg}")
            }
            other => panic!("expected a traversal rejection, got {other:?}"),
        }
    }

    #[test]
    fn reject_traversal_covers_the_escape_shapes_and_admits_plain_paths() {
        for bad in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "\\windows\\system32",
            "C:/windows",
            "a\\..\\..\\x",
            "",
        ] {
            assert!(
                reject_traversal(bad).is_err(),
                "traversal shape was admitted: {bad:?}"
            );
        }
        // Positive control: without these, a checker that rejected everything
        // would look correct.
        for good in ["a.txt", "a/b/c.txt", "skills/demo/SKILL.md", "./a.txt"] {
            assert!(
                reject_traversal(good).is_ok(),
                "plain path was refused: {good:?}"
            );
        }
    }

    #[test]
    fn tree_digest_moves_when_any_payload_changes() {
        let a = vec![PayloadEntry {
            path: "x".into(),
            bytes: 1,
            sha256: sha256_hex(b"1"),
            mode: None,
        }];
        let b = vec![PayloadEntry {
            path: "x".into(),
            bytes: 1,
            sha256: sha256_hex(b"2"),
            mode: None,
        }];
        assert_ne!(
            Manifest::compute_tree_digest(&a),
            Manifest::compute_tree_digest(&b)
        );
        // And a pure rename moves it too, so a moved file is not invisible.
        let c = vec![PayloadEntry {
            path: "y".into(),
            bytes: 1,
            sha256: sha256_hex(b"1"),
            mode: None,
        }];
        assert_ne!(
            Manifest::compute_tree_digest(&a),
            Manifest::compute_tree_digest(&c)
        );
    }
}
