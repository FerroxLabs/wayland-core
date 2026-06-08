// M3.5.3 — `ProceduralSkillTelemetrySink` end-to-end through `MemoryApi`.
//
// Verifies that a sink wired with an `Arc<dyn MemoryApi>` persists events
// into the procedural partition via `record_skill_use` (the row appears in
// `list_procedures` after a sub-millisecond settle).

use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::memory::Memory;
use wcore_memory::v2_types::{AccessToken, Tier};
use wcore_skills::telemetry::{
    ProceduralSkillTelemetrySink, SkillOutcome, SkillTelemetryEvent, SkillTelemetrySink,
};

#[tokio::test]
async fn procedural_sink_writes_via_record_skill_use() {
    let mem = Memory::open_in_memory().await.unwrap();
    let api: Arc<dyn MemoryApi> = Arc::new(mem.clone());
    let sink = ProceduralSkillTelemetrySink::new(api);

    sink.record(SkillTelemetryEvent {
        skill_name: "via-sink".into(),
        session_id: None,
        outcome: SkillOutcome::Success,
        latency_ms: 5,
        ts_secs: 1_700_000_000,
    });

    // `record()` is sync; it spawns a detached tokio task. Poll a few
    // times to give the task room to land (CI scheduling can be lumpy).
    let mut found = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let procs = mem
            .list_procedures(Tier::Project, AccessToken::System)
            .await
            .unwrap();
        if procs.iter().any(|p| p.name == "skill:via-sink") {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "ProceduralSkillTelemetrySink must persist via record_skill_use within 500ms"
    );
}

#[tokio::test]
async fn procedural_sink_records_failure_as_beta_increment() {
    let mem = Memory::open_in_memory().await.unwrap();
    let api: Arc<dyn MemoryApi> = Arc::new(mem.clone());
    let sink = ProceduralSkillTelemetrySink::new(api);

    sink.record(SkillTelemetryEvent {
        skill_name: "fail-skill".into(),
        session_id: Some("s1".into()),
        outcome: SkillOutcome::Failure,
        latency_ms: 7,
        ts_secs: 1_700_000_000,
    });

    // Poll for the row.
    let mut row = None;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let procs = mem
            .list_procedures(Tier::Project, AccessToken::System)
            .await
            .unwrap();
        if let Some(r) = procs.iter().find(|p| p.name == "skill:fail-skill").cloned() {
            row = Some(r);
            break;
        }
    }
    let row = row.expect("failure event must still upsert a procedure row");
    assert_eq!(row.use_count, 1);
    assert_eq!(row.success_count, 0);
    // alpha unchanged (1), beta bumped to 2 by the failure.
    assert!((row.thompson_alpha - 1.0).abs() < 1e-6);
    assert!((row.thompson_beta - 2.0).abs() < 1e-6);
}

#[tokio::test]
async fn null_sink_does_not_touch_memory() {
    use wcore_skills::telemetry::NullTelemetrySink;
    let mem = Memory::open_in_memory().await.unwrap();
    let sink = NullTelemetrySink;
    sink.record(SkillTelemetryEvent {
        skill_name: "ignored".into(),
        session_id: None,
        outcome: SkillOutcome::Success,
        latency_ms: 1,
        ts_secs: 0,
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let procs = mem
        .list_procedures(Tier::Project, AccessToken::System)
        .await
        .unwrap();
    assert!(
        procs.is_empty(),
        "NullTelemetrySink must never write to memory"
    );
}
