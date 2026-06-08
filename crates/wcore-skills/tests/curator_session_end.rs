//! F11.A + F11.B — Curator skeleton + scoring + dedupe + archive.

use std::sync::Arc;

use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Procedure, ProcedureId, ProcedureStatus, Tier};
use wcore_skills::curate::Curator;

#[tokio::test]
async fn curator_skeleton_runs_and_returns_empty_report_when_no_p4() {
    let tmp = tempfile::tempdir().unwrap();
    let mem = wcore_memory::open_for_test(tmp.path()).await.unwrap();
    let cur = Curator::new(Arc::new(mem));
    let report = cur.run().await.expect("curator must run");
    assert_eq!(report.archived.len(), 0);
    assert_eq!(report.dedupes.len(), 0);
    assert_eq!(report.kept_active.len(), 0);
}

async fn seed_procedure(mem: &Arc<dyn MemoryApi>, name: &str, desc: &str, status: ProcedureStatus) {
    let id = ProcedureId(uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        name.as_bytes(),
    ));
    let p = Procedure {
        id,
        tier: Tier::Project,
        ts: 0,
        name: name.into(),
        description: desc.into(),
        artifact: "---\n---\n".into(),
        status,
        created_by: "test".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    mem.upsert_procedure(p, AccessToken::System).await.unwrap();
}

#[tokio::test]
async fn curator_archives_overlapping_staged_drafts_keeping_at_most_two_active() {
    let tmp = tempfile::tempdir().unwrap();
    let mem: Arc<dyn MemoryApi> = Arc::new(wcore_memory::open_for_test(tmp.path()).await.unwrap());
    // Five drafts with near-identical descriptions, seeded as Staged.
    for i in 0..5 {
        seed_procedure(
            &mem,
            &format!("auto-x-{i}"),
            "Auto-drafted from grep read edit bash",
            ProcedureStatus::Staged,
        )
        .await;
    }
    let report = Curator::new(mem.clone()).run().await.unwrap();
    assert!(
        report.kept_active.len() <= 2,
        "kept set = {:?}",
        report.kept_active
    );
    assert!(
        report.archived.len() >= 3,
        "archived = {:?}",
        report.archived
    );
}

#[tokio::test]
async fn curator_does_not_touch_pinned_procedures() {
    let tmp = tempfile::tempdir().unwrap();
    let mem: Arc<dyn MemoryApi> = Arc::new(wcore_memory::open_for_test(tmp.path()).await.unwrap());
    seed_procedure(
        &mem,
        "important",
        "User-pinned long ago",
        ProcedureStatus::Pinned,
    )
    .await;
    seed_procedure(
        &mem,
        "stale",
        "Never used recently",
        ProcedureStatus::Active,
    )
    .await;
    let report = Curator::new(mem.clone()).run().await.unwrap();
    assert!(
        !report.archived.contains(&"important".to_string()),
        "Pinned never archived"
    );
}
