//! M5.7 — SwarmMemoryBridge smoke tests.
//!
//! Verbatim acceptance from wave-E §E3 (substitutions applied):
//! - parent writes episodic → worker can read
//! - worker writes procedural → parent receives on merge
//! - cycle / descendant-read denial

use std::time::Duration;

use wcore_memory::Memory;
use wcore_memory::v2_types::{
    EpisodeId, EpisodeStatus, Procedure, ProcedureId, ProcedureStatus, Tier,
};
use wcore_swarm::{SwarmMemoryBridge, SwarmResult, WorkerStatus};

#[tokio::test]
async fn parent_writes_episodic_worker_reads_recent() {
    let mem = Memory::open_in_memory().await.unwrap();
    let parent = "parent-session";
    // Parent writes one episode under its own session id.
    let parent_ep = wcore_memory::v2_types::Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: 1_700_000_000,
        episode_type: "parent_fact".into(),
        summary: "parent-fact-1: foundational context".into(),
        atomic_facts: vec![],
        source: "main-agent".into(),
        source_product: "wcore-agent".into(),
        session_id: Some(parent.to_string()),
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    mem.dispatcher.episodic.record(parent_ep).await.unwrap();

    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), parent.to_string());
    let snapshot = bridge
        .read_for_child("worker-1", Tier::Project, 10)
        .await
        .unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot[0].summary.contains("parent-fact-1"));
    // The lineage edge for the worker should also have been recorded.
    assert_eq!(bridge.lineage_len().await, 1);
}

#[tokio::test]
async fn worker_outcome_records_episode_under_child_session() {
    let mem = Memory::open_in_memory().await.unwrap();
    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "orchestrator-x".to_string());

    let result = SwarmResult {
        worker_id: "w-42".to_string(),
        branch: "swarm/foo/w-42".to_string(),
        status: WorkerStatus::Succeeded,
        stdout: "ok\n".into(),
        stderr: String::new(),
        duration: Duration::from_millis(750),
    };
    let id = bridge.record_child_outcome("w-42", &result).await.unwrap();

    let ep = mem
        .dispatcher
        .episodic
        .get(&id, Tier::Project)
        .await
        .unwrap();
    assert_eq!(ep.session_id.as_deref(), Some("w-42"));
    assert!(ep.summary.contains("worker=w-42"));
    assert!(ep.summary.contains("status=succeeded"));
    assert_eq!(ep.source_product, "wcore-swarm");
}

#[tokio::test]
async fn worker_writes_procedural_parent_receives_on_merge() {
    let mem = Memory::open_in_memory().await.unwrap();
    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "parent-session".to_string());

    // Simulate the worker writing a procedural skill tagged with its
    // sub-agent source. In production the worker subprocess writes
    // this through its own MemoryApi handle.
    let child_proc = Procedure {
        id: ProcedureId::new(),
        tier: Tier::Project,
        ts: 2_000,
        name: "worker-discovered-skill".into(),
        description: "found by w-1 during dispatch".into(),
        artifact: "---\nname: worker-discovered\n---\nbody".into(),
        status: ProcedureStatus::Active,
        created_by: "sub-agent:worker-1".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    mem.dispatcher.procedural.upsert(child_proc).await.unwrap();

    let merged = bridge.merge_child_into_parent("worker-1").await.unwrap();
    assert!(merged >= 1, "expected at least one row merged");

    let parent_items = mem.dispatcher.procedural.list(Tier::Project).await.unwrap();
    assert!(
        parent_items.iter().any(
            |p| p.name == "worker-discovered-skill" && p.created_by.starts_with("swarm-merge:")
        ),
        "parent did not receive child's procedural learning: {parent_items:?}"
    );
}

#[tokio::test]
async fn cycle_detection_rejects_descendant_read() {
    let mem = Memory::open_in_memory().await.unwrap();
    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "root".to_string());

    bridge
        .record_child_lineage("child-a", "root")
        .await
        .unwrap();
    bridge
        .record_child_lineage("grandchild-b", "child-a")
        .await
        .unwrap();

    // grandchild reading from its ancestor "child-a": OK.
    bridge
        .read_ancestor_chain("grandchild-b", "child-a")
        .await
        .unwrap();

    // child reading from its descendant "grandchild-b": MUST fail.
    let err = bridge
        .read_ancestor_chain("child-a", "grandchild-b")
        .await
        .unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("descendant") || s.contains("downstream"),
        "got: {s}"
    );
}

#[tokio::test]
async fn cycle_rejected_on_record_child_lineage() {
    let mem = Memory::open_in_memory().await.unwrap();
    let bridge = SwarmMemoryBridge::new(mem.dispatcher.clone(), "root".to_string());

    bridge.record_child_lineage("a", "root").await.unwrap();
    bridge.record_child_lineage("b", "a").await.unwrap();
    // Adding (a, b) would create a -> b -> a -> b cycle.
    let err = bridge.record_child_lineage("a", "b").await.unwrap_err();
    assert!(err.to_string().to_lowercase().contains("cycle"));
}
