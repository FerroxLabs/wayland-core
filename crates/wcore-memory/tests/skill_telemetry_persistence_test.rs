// M3.5 — tests for `MemoryApi::record_skill_use` + `top_procedures`.
//
// These exercise the public trait surface so any future impl (NullMemory,
// PartitionDispatcher, mocks) is covered by the same contract.

use wcore_memory::api::MemoryApi;
use wcore_memory::memory::Memory;
use wcore_memory::v2_types::{AccessToken, Tier};

#[tokio::test]
async fn record_skill_use_creates_procedure_on_first_use() {
    let mem = Memory::open_in_memory().await.unwrap();
    mem.record_skill_use("test-skill", true, 42).await.unwrap();

    let procs = mem
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .unwrap();
    let row = procs
        .iter()
        .find(|p| p.name == "skill:test-skill")
        .expect("record_skill_use must upsert a procedure named 'skill:<name>'");
    assert_eq!(row.use_count, 1);
    assert_eq!(row.success_count, 1);
    assert!(
        (row.thompson_alpha - 2.0).abs() < 1e-6,
        "alpha starts at 1 + 1 success"
    );
    assert!(
        (row.thompson_beta - 1.0).abs() < 1e-6,
        "beta stays at 1 (no failures)"
    );
}

#[tokio::test]
async fn record_skill_use_increments_existing_row() {
    let mem = Memory::open_in_memory().await.unwrap();
    mem.record_skill_use("multi-skill", true, 10).await.unwrap();
    mem.record_skill_use("multi-skill", false, 15)
        .await
        .unwrap();
    mem.record_skill_use("multi-skill", true, 20).await.unwrap();

    let procs = mem
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .unwrap();
    let row = procs
        .iter()
        .find(|p| p.name == "skill:multi-skill")
        .unwrap();
    assert_eq!(row.use_count, 3);
    assert_eq!(row.success_count, 2);
    // alpha = 1 + 2 successes, beta = 1 + 1 failure.
    assert!((row.thompson_alpha - 3.0).abs() < 1e-6);
    assert!((row.thompson_beta - 2.0).abs() < 1e-6);
}

#[tokio::test]
async fn top_procedures_filters_by_min_uses() {
    // v0.6.4 Task 6.6b — ranking is now Thompson-sampled (stochastic) per
    // the Forge-parity design. Deterministic ordering across `good` and
    // `mediocre` is asserted by the unit-level statistical test in
    // `partition::tests::strong_arm_wins_top_slot_at_least_95pct` against
    // a `with_seed(42)` sampler. Here we only verify the `min_uses` filter,
    // which is unchanged.
    let mem = Memory::open_in_memory().await.unwrap();

    for _ in 0..5 {
        mem.record_skill_use("good", true, 1).await.unwrap();
    }
    for _ in 0..3 {
        mem.record_skill_use("mediocre", true, 1).await.unwrap();
    }
    for _ in 0..2 {
        mem.record_skill_use("mediocre", false, 1).await.unwrap();
    }
    // brand-new: 1 use → filtered out by min_uses=3
    mem.record_skill_use("brand-new", true, 1).await.unwrap();

    let top = mem
        .top_procedures(Tier::Project, 10, 3, AccessToken::System)
        .await
        .unwrap();

    assert!(
        !top.iter().any(|p| p.name == "skill:brand-new"),
        "min_uses must filter out under-used rows"
    );
    assert!(top.iter().any(|p| p.name == "skill:good"));
    assert!(top.iter().any(|p| p.name == "skill:mediocre"));
}

#[tokio::test]
async fn top_procedures_truncates_to_k() {
    let mem = Memory::open_in_memory().await.unwrap();
    for name in ["a", "b", "c", "d"] {
        mem.record_skill_use(name, true, 1).await.unwrap();
    }
    let top = mem
        .top_procedures(Tier::Project, 2, 1, AccessToken::System)
        .await
        .unwrap();
    assert_eq!(top.len(), 2, "k=2 must truncate to two rows");
}
