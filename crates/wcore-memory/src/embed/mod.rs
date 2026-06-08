// M4.5 — Embedder is now a trait.
//
// The hashed-token bag impl that shipped in W5 (M8) lives in `hashed.rs`
// as `HashedEmbedder`. Real backends (OpenAI/Voyage/local-bge) land in
// M4.6 / M4.7 / M4.7b behind their own Cargo features.
//
// Cross-backend invariants:
// * Every `embed(text)` returns an L2-normalized vector of length `dim()`.
// * `dim()` is constant for the lifetime of a given embedder instance —
//   sqlite-vec table creation (M4.8) reads it once.
// * `name()` returns a stable diagnostic string for telemetry and for
//   migration checks (the procedural partition can detect a backend
//   swap and rebuild its index).
// * Backends MUST report errors via `MemoryError::Embedding(String)` —
//   no new error variants. See PLAN-AMENDMENTS.md §C2.

use crate::error::Result;

pub mod hashed;
pub use hashed::HashedEmbedder;

pub mod openai;
pub use openai::OpenAiEmbedder;

pub mod voyage;
pub use voyage::VoyageEmbedder;

pub mod bge_local;
pub use bge_local::LocalBgeSmallEmbedder;

// T3-7 — SHA-256 model integrity pin (ported from ijfw vectors.js X3/S8).
// Independent helper; the `Embedder` trait above is unchanged. Backends
// that load local model files can call `verify_model_sha256` post-download
// to refuse a model whose digest doesn't match a user-supplied pin.
pub mod sha_pin;
pub use sha_pin::{PinError, verify_model_sha256};

/// Embedding dimensionality for the default (hashed) backend. Kept public
/// so legacy call sites that hard-coded 384 still resolve, but new code
/// should prefer `Embedder::dim()` so it stays correct across backends.
pub const EMBEDDING_DIM: usize = 384;

/// Pluggable embedder backend.
///
/// Cheap to share — every caller holds `Arc<dyn Embedder>`. Implementors
/// must be `Send + Sync + 'static` so the dispatcher (and background
/// scheduler) can move them across tokio tasks.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync + 'static {
    /// Embed one text into an L2-normalized vector of length `self.dim()`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Vector dimensionality. Constant for the lifetime of `self`.
    fn dim(&self) -> usize;

    /// Stable backend identifier (e.g. "hashed/384", "openai/text-embedding-3-small/1536").
    /// Used by telemetry + sqlite-vec schema migration checks.
    fn name(&self) -> &'static str;
}

/// Cosine similarity between two same-length vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Encode a Vec<f32> into a BLOB for SQLite storage.
pub fn encode_blob(v: &[f32]) -> Vec<u8> {
    bytemuck::cast_slice::<f32, u8>(v).to_vec()
}

/// Decode a BLOB back to Vec<f32>. Returns Err if the blob length isn't a
/// multiple of 4.
pub fn decode_blob(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(crate::error::MemoryError::Embedding(format!(
            "blob length {} not a multiple of 4",
            bytes.len()
        )));
    }
    Ok(bytemuck::cast_slice::<u8, f32>(bytes).to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_roundtrip() {
        let v: Vec<f32> = (0..EMBEDDING_DIM).map(|i| i as f32 * 0.01).collect();
        let b = encode_blob(&v);
        let back = decode_blob(&b).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn blob_decode_rejects_misaligned() {
        let bad = vec![0u8; 7];
        assert!(decode_blob(&bad).is_err());
    }

    #[test]
    fn cosine_handles_mismatched_lengths() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
    }
}
