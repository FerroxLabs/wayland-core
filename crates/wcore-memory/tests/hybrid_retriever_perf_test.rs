// W5 Group D acceptance gate: HybridRetriever p95 < 100ms on a synthetic
// 10K-episode corpus. Ignored by default; run via
// `cargo nextest run --run-ignored=only -p wcore-memory -- hybrid_retriever_perf`.

use std::sync::Arc;
use std::time::Instant;

use wcore_memory::api::MemoryApi;
use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::v2_types::{AccessToken, Episode, EpisodeId, EpisodeStatus, Query, Tier};

async fn fresh_dispatcher() -> PartitionDispatcher {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    PartitionDispatcher::new(gate, db, embedder, cdc, Some("perf".into()))
}

#[tokio::test]
#[ignore]
async fn hybrid_retriever_perf_p95_under_100ms() {
    let d = fresh_dispatcher().await;
    // Seed 10K episodes. Generation runs over the embedder; the dispatcher
    // routes them via the record path (FTS5 + vec blob).
    let topics = [
        "rust async",
        "javascript bundle",
        "memory consolidation",
        "candle embeddings",
        "sqlite vec",
        "tokio runtime",
        "wasm browser",
        "kernel module",
        "vim macro",
        "kubernetes pod",
    ];
    for i in 0..10_000usize {
        let topic = topics[i % topics.len()];
        let ep = Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: i as i64,
            episode_type: "synthetic".into(),
            summary: format!("{topic} doc {i}"),
            atomic_facts: vec![format!("note {i}")],
            source: "main-agent".into(),
            source_product: "wcore-agent".into(),
            session_id: Some(format!("s{}", i % 50)),
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        };
        d.record_episode(ep, AccessToken::MainAgent).await.unwrap();
    }

    // Time 100 queries; p95 must be < 100ms.
    let queries = [
        "rust async runtime",
        "javascript bundle size",
        "memory consolidation pipeline",
        "candle CPU embeddings",
        "sqlite-vec virtual table",
        "tokio scheduler",
        "wasm bindings",
        "kernel symbol",
        "vim plugin",
        "kubernetes deploy",
    ];
    let mut samples: Vec<u128> = Vec::with_capacity(100);
    for q in queries.iter().cycle().take(100) {
        let started = Instant::now();
        let _ = d
            .search(
                Query {
                    text: (*q).into(),
                    tier: Tier::Project,
                    limit_per_modality: 20,
                    ..Query::default()
                },
                AccessToken::MainAgent,
            )
            .await
            .unwrap();
        samples.push(started.elapsed().as_millis());
    }
    samples.sort_unstable();
    let p95 = samples[(samples.len() as f64 * 0.95).floor() as usize];
    println!("retriever p95 (10K corpus, 100 queries): {p95}ms");
    assert!(
        p95 < 100,
        "p95 latency {p95}ms exceeds the 100ms gate (samples = {samples:?})"
    );
}

#[tokio::test]
#[ignore]
async fn binary_size_baseline() {
    // Records the release binary size delta. The wave-level baseline is
    // captured by the H.3 step; this test is the gate marker so the
    // --run-ignored=only sweep records a known assertion.
    let wcore = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|p| p.join("target").exists())
        .map(|p| p.join("target").join("release").join("wayland-core"));
    if let Some(path) = wcore.as_ref().filter(|p| p.exists()) {
        let size = std::fs::metadata(path).unwrap().len();
        println!("wayland-core release size: {} MB", size / 1024 / 1024);
    } else {
        println!("(release binary not built; H.3 records the measurement)");
    }
}

/// Zero-execution guard — and it has to RUN to be one.
///
/// Every test in this binary is `#[ignore]`d, so `cargo test --test hybrid_retriever_perf_test`
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
         cargo test -p wcore-memory --test hybrid_retriever_perf_test -- --ignored --test-threads=1"
    );
}
