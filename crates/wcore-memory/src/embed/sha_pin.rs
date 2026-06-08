// T3-7 — Embedder model SHA-256 pin verification.
//
// Ported from ijfw/mcp-server/src/vectors.js (X3/S8 model integrity pin).
// When a backend boots a local model (e.g. bge-small ONNX, candle
// safetensors), callers can pass an expected SHA-256 hex digest; this
// helper streams the file through `sha2::Sha256` and refuses the embedder
// on mismatch — closed-fail, never silently downgrade.
//
// Scope: SHA-256 pin only. The `Embedder` trait in `mod.rs` is not
// touched; callers decide when to invoke `verify_model_sha256` (typically
// after download + before first `embed()` call).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// 8 KiB streaming-hash chunk. Bounded heap so very large model files
/// (multi-hundred-MB weights) don't spike memory.
const HASH_CHUNK: usize = 8 * 1024;

/// SHA-256 model-pin verification failures.
#[derive(Debug, thiserror::Error)]
pub enum PinError {
    /// File hashed cleanly but the digest didn't match the configured pin.
    /// Both digests are lower-case hex (64 chars).
    #[error("model SHA-256 mismatch at {path}: expected {expected}, got {actual}")]
    Mismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    /// Couldn't read the model file (missing, permission denied, mid-stream
    /// I/O error, etc.).
    #[error("model SHA-256 verification I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    /// The user-supplied `expected_hex` isn't a valid 64-char lower-case
    /// SHA-256 hex digest. We refuse to even hash the file in that case —
    /// a typo'd pin must be a closed-fail, not a silent skip.
    #[error("invalid expected SHA-256 hex `{0}`: {1}")]
    InvalidHex(String, String),
}

/// Verify that the file at `model_path` hashes to `expected_hex` (SHA-256).
///
/// `expected_hex` must be a 64-character ASCII hex string (case-insensitive).
/// Comparison is performed in lower-case. The file is streamed through an
/// 8 KiB buffer — heap usage is constant regardless of model size.
///
/// Returns `Ok(())` on match. Errors:
/// * [`PinError::InvalidHex`] — `expected_hex` is malformed.
/// * [`PinError::Io`] — file missing / unreadable.
/// * [`PinError::Mismatch`] — file hashed cleanly but digest differs.
pub fn verify_model_sha256(model_path: &Path, expected_hex: &str) -> Result<(), PinError> {
    let expected = canonicalize_expected(expected_hex)?;

    let mut file = File::open(model_path).map_err(|e| PinError::Io {
        path: model_path.to_path_buf(),
        source: e,
    })?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; HASH_CHUNK];
    loop {
        let n = file.read(&mut buf).map_err(|e| PinError::Io {
            path: model_path.to_path_buf(),
            source: e,
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual = hex_encode(&hasher.finalize());

    if actual == expected {
        Ok(())
    } else {
        Err(PinError::Mismatch {
            path: model_path.to_path_buf(),
            expected,
            actual,
        })
    }
}

/// Normalize the user-supplied pin to lower-case hex and validate length /
/// alphabet. Returns the canonical form ready for comparison.
fn canonicalize_expected(expected_hex: &str) -> Result<String, PinError> {
    let trimmed = expected_hex.trim();
    if trimmed.len() != 64 {
        return Err(PinError::InvalidHex(
            expected_hex.to_string(),
            format!("expected 64 hex chars, got {}", trimmed.len()),
        ));
    }
    if !trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PinError::InvalidHex(
            expected_hex.to_string(),
            "non-hex character in expected digest".to_string(),
        ));
    }
    Ok(trimmed.to_ascii_lowercase())
}

/// Minimal lower-case hex encoder. Keeps us off the `hex` crate so we don't
/// pull a new dep just for one 32-byte digest format.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// SHA-256("hello world\n") = a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447
    /// Precomputed independently (e.g. `printf 'hello world\n' | shasum -a 256`).
    const HELLO_HASH: &str = "a948904f2f0f479b8f8197694b30184b0d2ed1c1cd2a1ec0fb85d299a192a447";

    /// SHA-256 of the empty byte sequence.
    const EMPTY_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn write_fixture(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).expect("create fixture");
        f.write_all(bytes).expect("write fixture");
        f.sync_all().expect("sync fixture");
        p
    }

    #[test]
    fn verify_succeeds_on_match() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "model.bin", b"hello world\n");
        verify_model_sha256(&path, HELLO_HASH).expect("hash should match precomputed digest");
    }

    #[test]
    fn verify_rejects_mismatch() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "model.bin", b"hello world\n");
        // Wrong (but well-formed) expected digest.
        let wrong = "0".repeat(64);
        let err = verify_model_sha256(&path, &wrong).expect_err("must fail");
        match err {
            PinError::Mismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, wrong);
                assert_eq!(actual, HELLO_HASH);
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn verify_io_error_on_missing_file() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist.bin");
        let err = verify_model_sha256(&missing, HELLO_HASH).expect_err("must fail");
        match err {
            PinError::Io { path, source } => {
                assert_eq!(path, missing);
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn verify_rejects_invalid_hex() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "model.bin", b"hello world\n");

        // Wrong length.
        let err = verify_model_sha256(&path, "deadbeef").expect_err("must fail");
        assert!(matches!(err, PinError::InvalidHex(_, _)));

        // Right length, non-hex alphabet ('z' is not hex).
        let bad_alpha = "z".repeat(64);
        let err = verify_model_sha256(&path, &bad_alpha).expect_err("must fail");
        assert!(matches!(err, PinError::InvalidHex(_, _)));
    }

    #[test]
    fn verify_handles_empty_file() {
        let dir = tempdir().unwrap();
        let path = write_fixture(dir.path(), "model.bin", b"");
        verify_model_sha256(&path, EMPTY_HASH).expect("empty-file hash should verify");

        // And mixed-case input is normalized.
        verify_model_sha256(&path, &EMPTY_HASH.to_ascii_uppercase())
            .expect("uppercase hex should normalize");
    }

    #[test]
    fn verify_streams_large_file_across_chunks() {
        // Generate a deterministic ~64 KiB blob — well over the 8 KiB chunk —
        // and hash it independently so the test asserts the streaming loop
        // matches a one-shot hash of the same bytes.
        let dir = tempdir().unwrap();
        let mut blob = Vec::with_capacity(64 * 1024);
        for i in 0..(64 * 1024) {
            blob.push((i % 251) as u8);
        }
        let path = write_fixture(dir.path(), "big.bin", &blob);

        let expected = {
            let mut h = Sha256::new();
            h.update(&blob);
            hex_encode(&h.finalize())
        };

        verify_model_sha256(&path, &expected).expect("streaming hash must match one-shot");

        // And a single-byte flip must trigger Mismatch.
        let mut tampered = blob.clone();
        tampered[12345] ^= 0x01;
        let tampered_path = write_fixture(dir.path(), "tampered.bin", &tampered);
        let err = verify_model_sha256(&tampered_path, &expected).expect_err("must fail");
        assert!(matches!(err, PinError::Mismatch { .. }));
    }
}
