// M5b3 — Real bge-small-en-v1.5 backend via direct candle integration.
//
// Replaces the M5.7-era reserved `local-embedder-real` compile_error guard
// and the prior fastembed/ort-sys attempt that broke on 4 of 11 CI jobs
// (ort-sys pre-built binaries missing for x86_64-darwin + aarch64-linux,
// Ubuntu OOM during ort linkage). Direct candle is pure-Rust, cross-
// compile clean, and stays well under the 5-min cold-CI budget.
//
// Two backends compile from this module:
//
//   * `bge-local` feature (opt-in) — real BERT inference via
//     candle-transformers, weights fetched from HuggingFace on first
//     `LocalBgeSmallEmbedder::new()` call (~133MB cached under
//     `~/.cache/huggingface`). Emits the canonical name
//     `"bge-small-en-v1.5/384"` so telemetry and sqlite-vec migration
//     checks can distinguish real from stub. Enable with
//     `cargo install wcore-cli --features bge-local`.
//
//   * `local-embedder` (default, without `bge-local`) — deterministic
//     384-dim L2-normalized hashed-token bag fallback. Default for
//     `cargo install wcore-cli`; users who want real semantic memory
//     either opt in to bge-local or configure a cloud embedder
//     (OpenAI / Voyage via `wcore-config`). Emits
//     `"bge-small-en-v1.5/384-stub"` so the migration check sees a
//     different backend identifier.
//
// Cross-backend invariants (enforced by mod.rs::Embedder docstring):
//   * 384-dim, L2-normalized output.
//   * Errors flow through `MemoryError::Embedding(String)` — no new
//     error variants. See PLAN-AMENDMENTS.md §C2.
//   * `Send + Sync + 'static` so the dispatcher can move the embedder
//     across tokio tasks.

/// Dimensionality of bge-small-en-v1.5. Hard-coded because the published
/// dim is invariant — the sqlite-vec schema (M4.8) reads this once at
/// table-create time and refuses backends with a different dim.
pub const BGE_SMALL_DIM: usize = 384;

// ─────────────────────────────────────────────────────────────────────────────
// Real backend — direct candle integration.
// Compiled when the `bge-local` feature is opt-in via
// `cargo install wcore-cli --features bge-local`.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "bge-local")]
mod real {
    use std::sync::Arc;

    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config};
    use hf_hub::api::tokio::Api;
    use parking_lot::Mutex;
    use tokenizers::Tokenizer;

    use super::super::Embedder;
    use super::BGE_SMALL_DIM;
    use crate::error::{MemoryError, Result};

    /// Real bge-small-en-v1.5 embedder. Holds the loaded BERT model +
    /// tokenizer behind an `Arc<Mutex<_>>` so the type is `Send + Sync`
    /// and cheap to clone. Inference itself is offloaded to a blocking
    /// thread via `spawn_blocking` so the tokio scheduler is not stalled.
    pub struct LocalBgeSmallEmbedder {
        inner: Arc<Mutex<BgeState>>,
    }

    struct BgeState {
        model: BertModel,
        tokenizer: Tokenizer,
        device: Device,
    }

    impl LocalBgeSmallEmbedder {
        /// Construct the embedder. First call fetches the model weights
        /// (~133MB) into `~/.cache/huggingface/`; subsequent calls reuse
        /// the cache and complete in <100ms.
        pub async fn new() -> Result<Self> {
            let device = Device::Cpu;
            let api =
                Api::new().map_err(|e| MemoryError::Embedding(format!("hf-hub Api::new: {e}")))?;
            let repo = api.model("BAAI/bge-small-en-v1.5".to_string());

            // First-call downloads (~133MB) to ~/.cache/huggingface/, then cached.
            // Emit a one-line breadcrumb so operators understand a cold cache
            // is a >0s startup pause and not a hang.
            eprintln!(
                "Loading semantic memory model (bge-small-en-v1.5, ~133MB first-run download)..."
            );
            let config_path = repo
                .get("config.json")
                .await
                .map_err(|e| MemoryError::Embedding(format!("download config.json: {e}")))?;
            let tokenizer_path = repo
                .get("tokenizer.json")
                .await
                .map_err(|e| MemoryError::Embedding(format!("download tokenizer.json: {e}")))?;
            let weights_path = repo
                .get("model.safetensors")
                .await
                .map_err(|e| MemoryError::Embedding(format!("download model.safetensors: {e}")))?;

            let config_str = std::fs::read_to_string(&config_path)
                .map_err(|e| MemoryError::Embedding(format!("read config: {e}")))?;
            let config: Config = serde_json::from_str(&config_str)
                .map_err(|e| MemoryError::Embedding(format!("parse config: {e}")))?;

            let tokenizer = Tokenizer::from_file(&tokenizer_path)
                .map_err(|e| MemoryError::Embedding(format!("tokenizer load: {e}")))?;

            // SAFETY: mmap is unsafe because another process mutating the file
            // races with reads. The HF cache directory is owned by this user
            // and the weights file is never rewritten in place (downloads
            // land in a tmpfile, then atomic-rename).
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[&weights_path], DType::F32, &device)
            }
            .map_err(|e| MemoryError::Embedding(format!("safetensors load: {e}")))?;

            let model = BertModel::load(vb, &config)
                .map_err(|e| MemoryError::Embedding(format!("bert model load: {e}")))?;

            Ok(Self {
                inner: Arc::new(Mutex::new(BgeState {
                    model,
                    tokenizer,
                    device,
                })),
            })
        }
    }

    #[async_trait::async_trait]
    impl Embedder for LocalBgeSmallEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let text = text.to_string();
            let inner = self.inner.clone();
            let vec = tokio::task::spawn_blocking(move || -> Result<Vec<f32>> {
                let state = inner.lock();
                let encoding = state
                    .tokenizer
                    .encode(text.as_str(), true)
                    .map_err(|e| MemoryError::Embedding(format!("encode: {e}")))?;
                let tokens: Vec<u32> = encoding.get_ids().to_vec();
                let token_ids = Tensor::new(tokens.as_slice(), &state.device)
                    .map_err(|e| MemoryError::Embedding(format!("tensor: {e}")))?
                    .unsqueeze(0)
                    .map_err(|e| MemoryError::Embedding(format!("unsqueeze: {e}")))?;
                let token_type_ids = token_ids
                    .zeros_like()
                    .map_err(|e| MemoryError::Embedding(format!("zeros: {e}")))?;
                let output = state
                    .model
                    .forward(&token_ids, &token_type_ids, None)
                    .map_err(|e| MemoryError::Embedding(format!("forward: {e}")))?;
                // Mean pool over the sequence dim (dim 1 = sequence length).
                let pooled = output
                    .mean(1)
                    .map_err(|e| MemoryError::Embedding(format!("mean: {e}")))?;
                // L2 normalize.
                let norm = pooled
                    .sqr()
                    .and_then(|t| t.sum_keepdim(1))
                    .and_then(|t| t.sqrt())
                    .map_err(|e| MemoryError::Embedding(format!("norm: {e}")))?;
                let normalized = pooled
                    .broadcast_div(&norm)
                    .map_err(|e| MemoryError::Embedding(format!("broadcast_div: {e}")))?;
                let vec: Vec<f32> = normalized
                    .squeeze(0)
                    .and_then(|t| t.to_vec1())
                    .map_err(|e| MemoryError::Embedding(format!("to_vec1: {e}")))?;
                Ok(vec)
            })
            .await
            .map_err(|e| MemoryError::Embedding(format!("spawn_blocking join: {e}")))??;
            Ok(vec)
        }

        fn dim(&self) -> usize {
            BGE_SMALL_DIM
        }

        fn name(&self) -> &'static str {
            // No `-stub` suffix — this is the real backend. The sqlite-vec
            // migration check (M4.8) distinguishes this from the stub by
            // matching on the exact string.
            "bge-small-en-v1.5/384"
        }
    }
}

#[cfg(feature = "bge-local")]
pub use real::LocalBgeSmallEmbedder;

// ─────────────────────────────────────────────────────────────────────────────
// Stub backend — deterministic L2-normalized hashed-token bag.
// Compiled when the `bge-local` feature is OFF. Used for CI determinism
// and offline builds. Identical public surface so call sites do not care
// which backend is wired.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "bge-local"))]
mod stub {
    use std::hash::{Hash, Hasher};

    use super::super::Embedder;
    use super::BGE_SMALL_DIM;
    use crate::error::{MemoryError, Result};

    /// Deterministic 384-dim L2-normalized fallback. Cheap to clone; no
    /// per-instance state.
    #[derive(Clone, Debug, Default)]
    pub struct LocalBgeSmallEmbedder;

    impl LocalBgeSmallEmbedder {
        /// Async to mirror the real backend's signature.
        pub async fn new() -> Result<Self> {
            Ok(Self)
        }
    }

    #[async_trait::async_trait]
    impl Embedder for LocalBgeSmallEmbedder {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            if text.is_empty() {
                // Sentinel unit vector so cosine is defined (self-cosine == 1.0).
                let mut v = vec![0.0f32; BGE_SMALL_DIM];
                v[0] = 1.0;
                return Ok(v);
            }

            let mut accum = vec![0.0f32; BGE_SMALL_DIM];
            for tok in tokenize(text) {
                let h = stable_hash(&tok);
                let b1 = (h % BGE_SMALL_DIM as u64) as usize;
                let b2 = ((h >> 16) % BGE_SMALL_DIM as u64) as usize;
                accum[b1] += 1.0;
                accum[b2] += 1.0;
            }

            l2_normalize(&mut accum);
            if !accum.iter().all(|v| v.is_finite()) {
                return Err(MemoryError::Embedding(
                    "LocalBgeSmallEmbedder: non-finite vector after normalize".into(),
                ));
            }
            Ok(accum)
        }

        fn dim(&self) -> usize {
            BGE_SMALL_DIM
        }

        fn name(&self) -> &'static str {
            // `-stub` suffix is load-bearing — telemetry and sqlite-vec
            // migration checks rely on the exact string to differentiate
            // the deterministic fallback from the real candle backend.
            "bge-small-en-v1.5/384-stub"
        }
    }

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
        // Distinct pre-seed from HashedEmbedder so the two backends produce
        // divergent vectors on identical input.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        "wcore-memory-bge-local-stub".hash(&mut hasher);
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
            v[0] = 1.0;
        }
    }
}

#[cfg(not(feature = "bge-local"))]
pub use stub::LocalBgeSmallEmbedder;

// ─────────────────────────────────────────────────────────────────────────────
// Tests — only the stub-path tests run by default to keep `cargo test`
// offline. The real backend's semantic test lives in
// `tests/bge_local_real.rs` and is marked `#[ignore]` so it only fires
// under `--run-ignored=all` in a dedicated CI job.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(not(feature = "bge-local"))]
mod stub_tests {
    use super::super::Embedder;
    use super::super::{cosine, hashed::HashedEmbedder};
    use super::*;

    #[tokio::test]
    async fn dim_is_384() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        assert_eq!(e.dim(), BGE_SMALL_DIM);
        let v = e.embed("hello rust world").await.unwrap();
        assert_eq!(v.len(), BGE_SMALL_DIM);
    }

    #[tokio::test]
    async fn name_marks_stub() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        assert_eq!(e.name(), "bge-small-en-v1.5/384-stub");
    }

    #[tokio::test]
    async fn output_is_l2_normalized() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        let v = e.embed("rust async runtime").await.unwrap();
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm = {norm}");
    }

    #[tokio::test]
    async fn self_cosine_is_one() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        let v = e.embed("wayland-core local embeddings").await.unwrap();
        let c = cosine(&v, &v);
        assert!((c - 1.0).abs() < 1e-5, "self cosine {c}");
    }

    #[tokio::test]
    async fn embedding_is_deterministic() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        let v1 = e.embed("deterministic check").await.unwrap();
        let v2 = e.embed("deterministic check").await.unwrap();
        assert_eq!(v1, v2);
    }

    #[tokio::test]
    async fn empty_text_yields_unit_vector() {
        let e = LocalBgeSmallEmbedder::new().await.unwrap();
        let v = e.embed("").await.unwrap();
        assert_eq!(v.len(), BGE_SMALL_DIM);
        let c = cosine(&v, &v);
        assert!((c - 1.0).abs() < 1e-5);
    }

    /// Guard against silent aliasing: HashedEmbedder and the stub backend
    /// MUST produce divergent vectors on identical input.
    #[tokio::test]
    async fn diverges_from_hashed_backend() {
        let bge = LocalBgeSmallEmbedder::new().await.unwrap();
        let hashed = HashedEmbedder::new().await.unwrap();
        let probe = "wayland-core divergence probe";
        let v_bge = bge.embed(probe).await.unwrap();
        let v_hashed = hashed.embed(probe).await.unwrap();
        assert_eq!(v_bge.len(), v_hashed.len(), "both backends are 384-dim");
        assert_ne!(
            v_bge, v_hashed,
            "bge-local stub must not alias the hashed backend"
        );
    }
}

#[cfg(test)]
#[cfg(feature = "bge-local")]
mod real_smoke_tests {
    //! Sanity checks for the real backend that do NOT require a network
    //! round-trip. The full semantic test lives in
    //! `tests/bge_local_real.rs` under `#[ignore]`.

    use super::*;

    #[test]
    fn bge_dim_constant_is_384() {
        assert_eq!(BGE_SMALL_DIM, 384);
    }
}
