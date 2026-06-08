// M4.6 — Live OpenAI embeddings integration test.
//
// Gated behind the `live-openai` Cargo feature so the default test suite
// stays offline. Run with:
//
//     OPENAI_API_KEY=sk-... cargo nextest run -p wcore-memory \
//         --features live-openai --test embedder_openai_live
//
// The contract is "you asked for live tests, you must supply the key" —
// a missing `OPENAI_API_KEY` panics with a clear message rather than
// silently skipping. This matches the existing `live-*` integration
// tests in wcore-providers.

#![cfg(feature = "live-openai")]

use wcore_memory::embed::{Embedder, OpenAiEmbedder};

fn api_key() -> String {
    std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "live-openai feature is enabled but OPENAI_API_KEY is unset — \
             set OPENAI_API_KEY to a valid key or drop the feature flag"
        )
    })
}

#[tokio::test]
async fn embed_returns_1536_dim_l2_normalized_vector() {
    let key = api_key();
    let e = OpenAiEmbedder::new(key, None).expect("constructor");

    let v = e.embed("the rust async runtime").await.expect("embed");

    assert_eq!(
        v.len(),
        1536,
        "text-embedding-3-small must return 1536-dim vectors"
    );
    assert_eq!(v.len(), e.dim(), "vector length must match Embedder::dim()");

    // L2-normalized: ‖v‖₂ ≈ 1.0. Tolerance is generous because the OpenAI
    // payload arrives unnormalized and we renormalize in f32 — drift up
    // to ~1e-5 is normal for 1536-dim sums.
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "vector must be L2-normalized, got ‖v‖={norm}"
    );

    assert!(v.iter().all(|x| x.is_finite()), "all components finite");
}

#[tokio::test]
async fn embed_is_deterministic_for_identical_input() {
    let key = api_key();
    let e = OpenAiEmbedder::new(key, None).expect("constructor");

    let v1 = e.embed("deterministic check").await.expect("embed #1");
    let v2 = e.embed("deterministic check").await.expect("embed #2");

    assert_eq!(v1.len(), v2.len());

    // OpenAI claims determinism for embeddings models, but floating-point
    // serialization through JSON can introduce 1-ULP wobble. Compare
    // cosine similarity (which is what the rest of the memory system
    // uses) rather than bit-exact equality.
    let cos: f32 = v1.iter().zip(v2.iter()).map(|(a, b)| a * b).sum();
    assert!(
        cos > 0.9999,
        "identical input must produce ~identical embedding, got cos={cos}"
    );
}

#[tokio::test]
async fn name_advertises_model_and_dim() {
    let key = api_key();
    let e = OpenAiEmbedder::new(key, None).expect("constructor");
    assert_eq!(e.name(), "openai/text-embedding-3-small/1536");
}
