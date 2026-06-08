//! M4.7 — live-HTTP round trip against Voyage AI's /v1/embeddings.
//!
//! Disabled in the default build. Enable with
//! `--features live-voyage` and set `VOYAGE_API_KEY` to exercise the
//! real wire. When the feature is on but the env var is absent we
//! skip cleanly so CI matrices that flip the feature don't false-fail.

#![cfg(feature = "live-voyage")]

use wcore_memory::embed::{Embedder, VoyageEmbedder};

/// Default Voyage model is `voyage-2` → 1024 dims, L2-normalized.
/// Asserts:
///   * vector length == 1024
///   * L2 norm ~= 1.0 (Voyage normalizes; defensively re-normalized too)
///   * two embed() calls with the same input yield equal vectors
#[tokio::test]
async fn live_voyage_round_trip() -> anyhow::Result<()> {
    let Ok(api_key) = std::env::var("VOYAGE_API_KEY") else {
        eprintln!("VOYAGE_API_KEY unset — skipping live Voyage test");
        return Ok(());
    };

    let embedder = VoyageEmbedder::new(api_key, None).await?;
    assert_eq!(embedder.dim(), 1024);
    assert_eq!(embedder.name(), "voyage/voyage-2/1024");

    let probe = "wayland-core voyage embedding live wire test";
    let v1 = embedder.embed(probe).await?;
    let v2 = embedder.embed(probe).await?;

    assert_eq!(v1.len(), 1024, "voyage-2 must return 1024 dims");
    assert_eq!(v2.len(), 1024);

    // Voyage embeddings are L2-normalized by default. The embedder
    // re-normalizes defensively, so norm should be ~1.0 either way.
    let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");

    // Determinism: identical input -> identical output across two
    // independent HTTP calls. Voyage doesn't sample, so this should
    // hold bit-for-bit.
    assert_eq!(
        v1, v2,
        "voyage embedding must be deterministic for identical input"
    );

    Ok(())
}
