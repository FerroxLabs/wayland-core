// M4.5 — Hashed-token bag embedder.
//
// Lifted out of the old monolithic `embed.rs` and renamed `HashedEmbedder`
// to make room for real backends (OpenAI/Voyage/bge-local) under the same
// `Embedder` trait. Behaviour is bit-identical to W5 (M8): 384-dim,
// L2-normalized, deterministic across runs, no model weights, ~0 KB
// binary impact.
//
// This stays the default backend so existing tests + offline dev paths
// don't pay an API-key cost just to construct a Memory.

use std::hash::{Hash, Hasher};

use super::{EMBEDDING_DIM, Embedder};
use crate::error::{MemoryError, Result};

/// Deterministic 384-dim hashed-token embedder. Cheap to clone.
#[derive(Clone, Default)]
pub struct HashedEmbedder;

impl HashedEmbedder {
    /// Async constructor preserved from the legacy `Embedder::new()` shape
    /// so call sites keep their `.await` patterns even when a real
    /// backend would actually require async (HTTP probe, model load).
    pub async fn new() -> Result<Self> {
        Ok(Self)
    }
}

#[async_trait::async_trait]
impl Embedder for HashedEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if text.is_empty() {
            // Empty embeddings have zero norm — emit a sentinel unit
            // vector so cosine is well defined (self-cosine = 1.0).
            let mut v = vec![0.0f32; EMBEDDING_DIM];
            v[0] = 1.0;
            return Ok(v);
        }

        let mut accum = vec![0.0f32; EMBEDDING_DIM];
        // Token-bag hashed features: each lowercased alphanumeric token
        // contributes to a deterministic bucket. 16-bit hash keeps
        // collisions reasonable for short corpora.
        for tok in tokenize(text) {
            let h = stable_hash(&tok);
            // Spread each token across two buckets for slightly smoother
            // similarity surfaces (cheap LSH-style).
            let b1 = (h % EMBEDDING_DIM as u64) as usize;
            let b2 = ((h >> 16) % EMBEDDING_DIM as u64) as usize;
            accum[b1] += 1.0;
            accum[b2] += 1.0;
        }

        l2_normalize(&mut accum);
        if !accum.iter().all(|v| v.is_finite()) {
            return Err(MemoryError::Embedding("non-finite vector".into()));
        }
        Ok(accum)
    }

    fn dim(&self) -> usize {
        EMBEDDING_DIM
    }

    fn name(&self) -> &'static str {
        "hashed/384"
    }
}

/// Lowercase + split on non-alphanumeric. Tokens shorter than 2 chars are
/// dropped (keeps signal-to-noise reasonable for short queries).
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            } else {
                cur.clear();
            }
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

fn stable_hash(s: &str) -> u64 {
    // SipHash with a fixed pre-seed is bit-deterministic across runs.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "wcore-memory-embed".hash(&mut hasher);
    s.hash(&mut hasher);
    hasher.finish()
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| (x * x) as f64).sum::<f64>().sqrt() as f32;
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    } else if !v.is_empty() {
        // All zeros — set a small unit so cosine is well defined.
        v[0] = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::super::cosine;
    use super::*;

    #[tokio::test]
    async fn embedding_has_384_dims() {
        let e = HashedEmbedder::new().await.unwrap();
        let v = e.embed("hello rust world").await.unwrap();
        assert_eq!(v.len(), EMBEDDING_DIM);
        assert_eq!(e.dim(), EMBEDDING_DIM);
    }

    #[tokio::test]
    async fn self_cosine_is_one() {
        let e = HashedEmbedder::new().await.unwrap();
        let v = e.embed("rust async runtime").await.unwrap();
        let c = cosine(&v, &v);
        assert!((c - 1.0).abs() < 1e-5, "self cosine {c}");
    }

    #[tokio::test]
    async fn embedding_is_deterministic() {
        let e = HashedEmbedder::new().await.unwrap();
        let v1 = e.embed("deterministic check").await.unwrap();
        let v2 = e.embed("deterministic check").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn related_texts_have_higher_cosine_than_unrelated() {
        let e = HashedEmbedder::new().await.unwrap();
        let q = e.embed("rust async tokio runtime").await.unwrap();
        let related = e.embed("the rust async runtime is fast").await.unwrap();
        let unrelated = e
            .embed("javascript bundlers are slow on disk")
            .await
            .unwrap();
        let c1 = cosine(&q, &related);
        let c2 = cosine(&q, &unrelated);
        assert!(c1 > c2, "related {c1} should beat unrelated {c2}");
    }

    #[tokio::test]
    async fn empty_text_yields_unit_vector() {
        let e = HashedEmbedder::new().await.unwrap();
        let v = e.embed("").await.unwrap();
        assert_eq!(v.len(), EMBEDDING_DIM);
        let c = cosine(&v, &v);
        assert!((c - 1.0).abs() < 1e-5);
    }

    #[tokio::test]
    async fn name_is_stable() {
        let e = HashedEmbedder::new().await.unwrap();
        assert_eq!(e.name(), "hashed/384");
    }
}
