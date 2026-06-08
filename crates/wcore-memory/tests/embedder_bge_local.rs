//! M4.7b — Smoke test for the local bge-small-en-v1.5 STUB backend.
//!
//! Gated behind `--features local-embedder` AND `not(feature = "bge-local")`
//! so it only fires for the deterministic fallback path. The real
//! candle-backed semantic test lives in `tests/bge_local_real.rs`
//! (M5b3 step 3, #[ignore]'d).
//!
//! Asserts the public-surface invariants the trait demands for the stub:
//!   * `dim()` == 384
//!   * `name()` == "bge-small-en-v1.5/384-stub" (the `-stub` suffix
//!     differentiates the stub from the real backend in telemetry +
//!     the sqlite-vec migration check)
//!   * L2-normalized output (norm ≈ 1.0) on representative inputs
//!   * deterministic across two `.embed()` calls with identical input

#![cfg(all(feature = "local-embedder", not(feature = "bge-local")))]

use wcore_memory::embed::{Embedder, LocalBgeSmallEmbedder};

#[tokio::test]
async fn bge_local_smoke() -> anyhow::Result<()> {
    let embedder = LocalBgeSmallEmbedder::new().await?;

    assert_eq!(embedder.dim(), 384, "bge-small must report 384 dims");
    assert_eq!(
        embedder.name(),
        "bge-small-en-v1.5/384-stub",
        "name must mark the deterministic fallback until M5.7"
    );

    for probe in [
        "wayland-core bge-small local embedding probe",
        "the rust async runtime is fast",
    ] {
        let v1 = embedder.embed(probe).await?;
        let v2 = embedder.embed(probe).await?;

        assert_eq!(v1.len(), 384, "vector must be 384-dim");
        assert_eq!(v2.len(), 384);

        let norm: f32 = v1.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit L2 norm for `{probe}`, got {norm}"
        );

        assert_eq!(
            v1, v2,
            "bge-local embedding must be deterministic for identical input"
        );
    }

    Ok(())
}
