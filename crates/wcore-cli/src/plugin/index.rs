//! Marketplace allowlist index — signed manifest of vetted plugins.
//!
//! v0.7.0 1.E.1: defines the on-the-wire schema, a signed-index
//! verifier, and a TTL-backed disk cache. The plugin-install wiring
//! that consumes this (`wayland-core plugin install <name>` searches
//! the index → resolves → clones+verifies+installs) lands as a Phase
//! 2 follow-up.
//!
//! Trust model: the index body is signed with ed25519. The verifying
//! pubkey is bundled at compile time (constant `INDEX_PUBKEY_HEX`) so
//! a compromised CDN cannot inject malicious entries — only an entity
//! controlling the bundled secret can ship a new index version.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Bundled marketplace pubkey, hex-encoded (32 bytes / 64 hex chars).
/// Placeholder until Sean ships the real `FerroxLabs/wayland-plugin-
/// index` signing key. Tests override this via [`IndexVerifier::with_pubkey`].
///
/// F-021 (security): [`IndexVerifier::bundled`] refuses to construct a verifier
/// from the all-zeros placeholder at runtime. The all-zeros key is the ed25519
/// identity point — signatures against it can be forged without a secret, so
/// using it as the marketplace trust root would neutralize the entire signing
/// chain. Replace this constant with the real FerroxLabs signing pubkey
/// before any user-facing marketplace verification path is shipped.
pub const INDEX_PUBKEY_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// One vetted plugin in the index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexEntry {
    /// Canonical plugin name (matches `PluginManifest.name`).
    pub name: String,
    /// Source URL (e.g. `github://owner/repo`).
    pub repo: String,
    /// Pinned tag (e.g. `v1.0.0`).
    pub tag: String,
    /// SHA-256 of the source archive at `tag`. Hex-encoded.
    pub sha256: String,
    /// Plugin author's ed25519 pubkey. Hex-encoded (used by the
    /// install path's signature verification, separate from the
    /// index-signing pubkey).
    pub pubkey: String,
    /// Human-readable one-liner.
    pub description: String,
    /// ISO-8601 date when this entry was reviewed.
    pub review_date: String,
    /// Free-form notes about the review pass.
    pub review_notes: String,
}

/// Schema-versioned index body. The signature covers the canonical
/// JSON-serialized body (everything inside `IndexEnvelope.body`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexBody {
    pub schema_version: String,
    pub plugins: Vec<IndexEntry>,
}

/// On-the-wire envelope wrapping a signed body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexEnvelope {
    /// Hex-encoded ed25519 signature over `serde_json::to_vec(&body)`.
    pub signature: String,
    pub body: IndexBody,
}

/// Errors from the index verifier / fetcher.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("bad signature: {0}")]
    BadSignature(String),
    #[error("bad pubkey: {0}")]
    BadPubKey(String),
    #[error("hex decode: {0}")]
    Hex(String),
    #[error("schema: {0}")]
    Schema(String),
}

/// Verify a signed index envelope. Builder so tests can override the
/// pubkey; production callers use [`IndexVerifier::bundled`].
pub struct IndexVerifier {
    pubkey: VerifyingKey,
}

impl IndexVerifier {
    /// Construct a verifier from the bundled marketplace pubkey.
    ///
    /// F-021: refuses to construct when `INDEX_PUBKEY_HEX` is still the
    /// all-zeros placeholder. The identity-point key would let anyone forge
    /// signatures, so any caller that hits this path before the real key ships
    /// gets a clear error instead of a permissive trust root.
    pub fn bundled() -> Result<Self, IndexError> {
        if INDEX_PUBKEY_HEX.bytes().all(|b| b == b'0') {
            return Err(IndexError::BadPubKey(
                "INDEX_PUBKEY_HEX is the all-zeros placeholder. Replace it with the real \
                 FerroxLabs signing pubkey before using the marketplace verifier. (F-021)"
                    .into(),
            ));
        }
        Self::with_pubkey_hex(INDEX_PUBKEY_HEX)
    }

    /// Construct a verifier from an arbitrary hex-encoded pubkey
    /// (32 bytes / 64 hex chars).
    pub fn with_pubkey_hex(hex: &str) -> Result<Self, IndexError> {
        let bytes = decode_hex_32(hex)?;
        let pubkey =
            VerifyingKey::from_bytes(&bytes).map_err(|e| IndexError::BadPubKey(e.to_string()))?;
        Ok(Self { pubkey })
    }

    /// Construct a verifier from a raw `VerifyingKey` (used by tests
    /// that generate a keypair in-process).
    pub fn with_pubkey(pubkey: VerifyingKey) -> Self {
        Self { pubkey }
    }

    /// Verify and return the body, or surface a structured error.
    pub fn verify(&self, env: &IndexEnvelope) -> Result<IndexBody, IndexError> {
        let sig_bytes = decode_hex_n(&env.signature, 64)?;
        let sig = Signature::from_slice(&sig_bytes)
            .map_err(|e| IndexError::BadSignature(e.to_string()))?;
        let body_bytes = serde_json::to_vec(&env.body)?;
        self.pubkey
            .verify(&body_bytes, &sig)
            .map_err(|e| IndexError::BadSignature(e.to_string()))?;
        // Schema gate: refuse forwards-incompatible versions. Bump the
        // accept-list as the schema evolves.
        if env.body.schema_version != "1.0" {
            return Err(IndexError::Schema(format!(
                "unsupported schema_version: {}",
                env.body.schema_version
            )));
        }
        Ok(env.body.clone())
    }
}

fn decode_hex_n(s: &str, expected_len: usize) -> Result<Vec<u8>, IndexError> {
    if s.len() != expected_len * 2 {
        return Err(IndexError::Hex(format!(
            "expected {} hex chars, got {}",
            expected_len * 2,
            s.len()
        )));
    }
    let mut out = Vec::with_capacity(expected_len);
    let bytes = s.as_bytes();
    for i in 0..expected_len {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn decode_hex_32(s: &str) -> Result<[u8; 32], IndexError> {
    let v = decode_hex_n(s, 32)?;
    let arr: [u8; 32] = v.try_into().map_err(|_| IndexError::Hex("length".into()))?;
    Ok(arr)
}

fn hex_nibble(b: u8) -> Result<u8, IndexError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + b - b'a'),
        b'A'..=b'F' => Ok(10 + b - b'A'),
        other => Err(IndexError::Hex(format!("bad nibble: {:#x}", other))),
    }
}

/// Disk-backed cache for the verified index body. Keyed by file path;
/// no concurrency control — the CLI runs single-threaded against the
/// cache.
#[derive(Debug, Clone)]
pub struct IndexCache {
    path: PathBuf,
    ttl_secs: u64,
}

impl IndexCache {
    /// Construct a cache at the given path with the given TTL.
    pub fn new(path: impl Into<PathBuf>, ttl_secs: u64) -> Self {
        Self {
            path: path.into(),
            ttl_secs,
        }
    }

    /// Default cache path: `$HOME/.wayland/index.json`. Falls back to
    /// the platform temp dir if `$HOME` is unset.
    pub fn default_path() -> PathBuf {
        match std::env::var_os("HOME") {
            Some(h) => Path::new(&h).join(".wayland").join("index.json"),
            None => std::env::temp_dir().join("wayland-index.json"),
        }
    }

    /// Read a fresh cached body. Returns `Ok(None)` if the file is
    /// missing or older than `ttl_secs`. Any other read / parse error
    /// surfaces.
    pub fn read(&self) -> Result<Option<IndexBody>, IndexError> {
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(IndexError::Io(e)),
        };
        let modified = meta.modified()?;
        let age = SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default()
            .as_secs();
        if age > self.ttl_secs {
            return Ok(None);
        }
        let bytes = std::fs::read(&self.path)?;
        let body: IndexBody = serde_json::from_slice(&bytes)?;
        Ok(Some(body))
    }

    /// Atomically write a verified body to disk. Creates the parent
    /// directory if missing.
    pub fn write(&self, body: &IndexBody) -> Result<(), IndexError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(body)?;
        // Write+rename for atomicity. Best-effort — fall back to a
        // direct write if the rename target is on a different fs.
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.path).or_else(|_| std::fs::write(&self.path, &bytes))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use tempfile::TempDir;

    fn fresh_keypair() -> (SigningKey, VerifyingKey) {
        let mut csprng = OsRng;
        let signing = SigningKey::generate(&mut csprng);
        let verifying = signing.verifying_key();
        (signing, verifying)
    }

    fn fresh_envelope(signing: &SigningKey) -> IndexEnvelope {
        let body = IndexBody {
            schema_version: "1.0".to_string(),
            plugins: vec![IndexEntry {
                name: "wayland-channel-matrix".to_string(),
                repo: "github://wayland-plugins/matrix".to_string(),
                tag: "v1.0.0".to_string(),
                sha256: "deadbeef".to_string(),
                pubkey: "feedface".to_string(),
                description: "Matrix channel adapter".to_string(),
                review_date: "2026-05-21".to_string(),
                review_notes: "v1.0.0 reviewed — clean".to_string(),
            }],
        };
        let bytes = serde_json::to_vec(&body).unwrap();
        let sig = signing.sign(&bytes);
        let sig_hex: String = sig
            .to_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        IndexEnvelope {
            signature: sig_hex,
            body,
        }
    }

    #[test]
    fn signed_index_roundtrip_verifies() {
        let (signing, verifying) = fresh_keypair();
        let env = fresh_envelope(&signing);
        let v = IndexVerifier::with_pubkey(verifying);
        let body = v.verify(&env).unwrap();
        assert_eq!(body.plugins.len(), 1);
        assert_eq!(body.plugins[0].name, "wayland-channel-matrix");
    }

    #[test]
    fn tampered_body_fails_verification() {
        let (signing, verifying) = fresh_keypair();
        let mut env = fresh_envelope(&signing);
        env.body.plugins[0].name = "wayland-channel-evil".to_string();
        let v = IndexVerifier::with_pubkey(verifying);
        let err = v.verify(&env).expect_err("expected bad-sig");
        assert!(matches!(err, IndexError::BadSignature(_)), "got {err:?}");
    }

    #[test]
    fn wrong_key_fails_verification() {
        let (signing, _) = fresh_keypair();
        let (_, other_pub) = fresh_keypair();
        let env = fresh_envelope(&signing);
        let v = IndexVerifier::with_pubkey(other_pub);
        let err = v.verify(&env).expect_err("expected bad-sig");
        assert!(matches!(err, IndexError::BadSignature(_)));
    }

    #[test]
    fn unsupported_schema_version_rejected() {
        let (signing, verifying) = fresh_keypair();
        let mut env = fresh_envelope(&signing);
        env.body.schema_version = "9.9".to_string();
        // Re-sign so the signature passes; the schema gate should reject.
        let bytes = serde_json::to_vec(&env.body).unwrap();
        let sig = signing.sign(&bytes);
        env.signature = sig
            .to_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        let v = IndexVerifier::with_pubkey(verifying);
        let err = v.verify(&env).expect_err("expected schema-reject");
        assert!(matches!(err, IndexError::Schema(_)));
    }

    #[test]
    fn cache_write_then_read_returns_body() {
        let tmp = TempDir::new().unwrap();
        let cache = IndexCache::new(tmp.path().join("index.json"), 60);
        let body = IndexBody {
            schema_version: "1.0".to_string(),
            plugins: Vec::new(),
        };
        cache.write(&body).unwrap();
        let got = cache.read().unwrap().expect("cached body");
        assert_eq!(got.schema_version, "1.0");
    }

    #[test]
    fn cache_expires_past_ttl() {
        let tmp = TempDir::new().unwrap();
        let cache = IndexCache::new(tmp.path().join("index.json"), 0);
        let body = IndexBody {
            schema_version: "1.0".to_string(),
            plugins: Vec::new(),
        };
        cache.write(&body).unwrap();
        // Sleep just past 0-second TTL.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let got = cache.read().unwrap();
        assert!(got.is_none(), "expected expired -> None");
    }

    #[test]
    fn cache_missing_file_returns_none() {
        let tmp = TempDir::new().unwrap();
        let cache = IndexCache::new(tmp.path().join("nope.json"), 60);
        let got = cache.read().unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn bundled_refuses_placeholder_pubkey() {
        // F-021: `IndexVerifier::bundled()` must refuse the all-zeros
        // placeholder at runtime, since the ed25519 identity-point key would
        // let anyone forge signatures against it.
        let err = IndexVerifier::bundled()
            .err()
            .expect("placeholder pubkey must be rejected");
        match err {
            IndexError::BadPubKey(msg) => {
                assert!(
                    msg.contains("F-021"),
                    "expected F-021 marker in placeholder rejection message, got: {msg}"
                );
                assert!(
                    msg.contains("placeholder"),
                    "expected `placeholder` in rejection message, got: {msg}"
                );
            }
            other => panic!("expected BadPubKey(placeholder), got variant: {other:?}"),
        }
    }
}
