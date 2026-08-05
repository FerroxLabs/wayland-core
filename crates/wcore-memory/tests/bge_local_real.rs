//! M5b3 step 3 — semantic-similarity smoke test for the real
//! bge-small-en-v1.5 backend.
//!
//! Marked `#[ignore]` because it downloads ~133MB of model weights from
//! HuggingFace on first run. CI fires this via a dedicated job with
//! `--run-ignored=all`; default `cargo test --features bge-local` stays
//! offline.

#![cfg(feature = "bge-local")]

use wcore_memory::embed::{Embedder, LocalBgeSmallEmbedder};

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb)
}

#[tokio::test]
#[ignore = "downloads ~133MB model from HuggingFace — run via --run-ignored=all in dedicated CI job"]
async fn bge_real_semantic_similarity() {
    let e = LocalBgeSmallEmbedder::new().await.expect("init");
    assert_eq!(e.dim(), 384);
    assert_eq!(e.name(), "bge-small-en-v1.5/384");

    let v_hello = e.embed("hello world").await.expect("embed hello");
    let v_greeting = e.embed("greeting earth").await.expect("embed greeting");
    let v_rust = e.embed("rust programming").await.expect("embed rust");
    let v_muffin = e.embed("blueberry muffin").await.expect("embed muffin");

    let sim_related = cosine(&v_hello, &v_greeting);
    let sim_unrelated = cosine(&v_rust, &v_muffin);

    println!("hello vs greeting cosine: {sim_related:.3}");
    println!("rust   vs muffin   cosine: {sim_unrelated:.3}");
    assert!(
        sim_related > 0.5,
        "related cosine {sim_related} should be > 0.5"
    );
    assert!(sim_unrelated < sim_related, "unrelated < related");
}

#[tokio::test]
#[ignore = "downloads ~133MB model"]
async fn bge_real_deterministic() {
    let e = LocalBgeSmallEmbedder::new().await.expect("init");
    let v1 = e.embed("deterministic check").await.expect("embed");
    let v2 = e.embed("deterministic check").await.expect("embed");
    assert_eq!(v1, v2);
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test bge_local_real`
/// executes 0 of 2 and still exits 0 printing `test result: ok`. This guard is
/// deliberately NOT `#[ignore]`d: three suites in this repo carried a guard that
/// was itself ignored, which made each inert against precisely the scenario it
/// existed for — it could only fire under `--ignored`, by which point the real
/// cases were running anyway.
///
/// It always runs, so this binary can never report success on zero executed
/// tests, and it FAILS when a caller sets `WAYLAND_REQUIRE_IGNORED=1` to declare a run of the
/// ignored cases while passing an invocation that cannot execute any of them.
/// Skipped under nextest, whose `no-tests = "fail"` policy covers the same
/// ground at the invocation site.
#[test]
fn zero_execution_guard() {
    if std::env::var_os("NEXTEST").is_some() {
        return;
    }
    if std::env::var("WAYLAND_REQUIRE_IGNORED").as_deref() != Ok("1") {
        return;
    }
    let asked_for_ignored = std::env::args().any(|a| a == "--ignored" || a == "--include-ignored");
    assert!(
        asked_for_ignored,
        "declared intent to run this suite's 2 #[ignore]d cases, but neither \
         --ignored nor --include-ignored was passed, so zero of them can execute. \
         Exiting 0 here would certify nothing. Re-run with: \
         cargo test -p wcore-memory --test bge_local_real -- --ignored --test-threads=1"
    );
}
