// W5 Group F acceptance: CDC journals every one of the 10 mutation paths.

use std::sync::Arc;

use serde_json::json;

use wcore_memory::api::MemoryApi;
use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::partition::working::WorkingEntry;
use wcore_memory::v2_types::{
    AccessToken, Episode, EpisodeId, EpisodeStatus, Fact, FactId, Procedure, ProcedureId,
    ProcedureStatus, Tier,
};

async fn fresh() -> (PartitionDispatcher, CdcWriter) {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc_writer = CdcWriter::new_stub();
    let cdc = Arc::new(cdc_writer.clone());
    let d = PartitionDispatcher::new(gate, db, embedder, cdc, Some("s".into()));
    (d, cdc_writer)
}

#[tokio::test]
async fn insert_paths_each_produce_one_entry() {
    let (d, cdc) = fresh().await;
    // 1) record_episode
    let ep = Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: 1,
        episode_type: "x".into(),
        summary: "s".into(),
        atomic_facts: vec![],
        source: "main-agent".into(),
        source_product: "wcore-agent".into(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    };
    d.record_episode(ep, AccessToken::MainAgent).await.unwrap();

    // 2) assert_fact
    let f = Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: 1,
        subject: "a".into(),
        predicate: "is".into(),
        object: "b".into(),
        confidence: 1.0,
        source_episode: None,
        superseded_by: None,
    };
    d.assert_fact(f, AccessToken::MainAgent).await.unwrap();

    // 3) upsert_procedure
    let p = Procedure {
        id: ProcedureId::new(),
        tier: Tier::Project,
        ts: 1,
        name: "n".into(),
        description: "".into(),
        artifact: "".into(),
        status: ProcedureStatus::Staged,
        created_by: "evolution".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    d.upsert_procedure(p, AccessToken::MainAgent).await.unwrap();

    let entries = cdc.entries();
    let parts: Vec<&str> = entries
        .iter()
        .filter(|e| e.op == "insert")
        .map(|e| e.partition.as_str())
        .collect();
    assert!(parts.contains(&"episodic"), "missing episodic: {parts:?}");
    assert!(parts.contains(&"semantic"), "missing semantic: {parts:?}");
    assert!(
        parts.contains(&"procedural"),
        "missing procedural: {parts:?}"
    );
}

#[tokio::test]
async fn supersede_emitted_when_object_changes() {
    let (d, cdc) = fresh().await;
    for obj in ["alpha", "beta"] {
        let f = Fact {
            id: FactId::new(),
            tier: Tier::Project,
            ts: 1,
            subject: "x".into(),
            predicate: "is".into(),
            object: obj.into(),
            confidence: 1.0,
            source_episode: None,
            superseded_by: None,
        };
        d.assert_fact(f, AccessToken::MainAgent).await.unwrap();
    }
    let n_supersedes = cdc.entries().iter().filter(|e| e.op == "supersede").count();
    assert!(
        n_supersedes >= 1,
        "missing supersede: entries={:?}",
        cdc.entries()
    );
}

#[tokio::test]
async fn procedure_status_use_user_model_delta_decay_emit_entries() {
    let (d, cdc) = fresh().await;
    let p = Procedure {
        id: ProcedureId::new(),
        tier: Tier::Project,
        ts: 1,
        name: "n".into(),
        description: "".into(),
        artifact: "".into(),
        status: ProcedureStatus::Staged,
        created_by: "evolution".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    let id = d.upsert_procedure(p, AccessToken::MainAgent).await.unwrap();
    d.procedural
        .transition(&id, Tier::Project, ProcedureStatus::Active)
        .await
        .unwrap();
    d.procedural
        .record_use(&id, Tier::Project, true)
        .await
        .unwrap();
    d.update_user_model("k", json!("v"), AccessToken::System)
        .await
        .unwrap();

    // Seed a 60-day-old episode + run decay to trigger an archive emit.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    d.record_episode(
        Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: now - 60 * 86400,
            episode_type: "old".into(),
            summary: "old".into(),
            atomic_facts: vec![],
            source: "main-agent".into(),
            source_product: "wcore-agent".into(),
            session_id: None,
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        },
        AccessToken::MainAgent,
    )
    .await
    .unwrap();
    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    engine.decay().await.unwrap();

    // P1 spillover.
    for i in 0..60 {
        d.working
            .push(WorkingEntry::Turn {
                ts: i,
                role: "u".into(),
                text: "x".into(),
            })
            .await
            .unwrap();
    }

    let entries = cdc.entries();
    let ops: Vec<&str> = entries.iter().map(|e| e.op.as_str()).collect();
    assert!(ops.contains(&"status_transition"), "{ops:?}");
    assert!(ops.contains(&"use"), "{ops:?}");
    assert!(ops.contains(&"delta"), "{ops:?}");
    assert!(ops.contains(&"decay_archive"), "{ops:?}");
    assert!(ops.contains(&"spillover"), "{ops:?}");
}

#[tokio::test]
async fn ordering_is_monotonic_per_tier() {
    let (d, cdc) = fresh().await;
    for i in 0..5 {
        d.record_episode(
            Episode {
                id: EpisodeId::new(),
                tier: Tier::Project,
                ts: i,
                episode_type: "x".into(),
                summary: format!("s{i}"),
                atomic_facts: vec![],
                source: "main-agent".into(),
                source_product: "wcore-agent".into(),
                session_id: None,
                project_root: None,
                decay_score: 1.0,
                status: EpisodeStatus::Active,
            },
            AccessToken::MainAgent,
        )
        .await
        .unwrap();
    }
    let project_seqs: Vec<u64> = cdc
        .entries()
        .iter()
        .filter(|e| e.tier == "project")
        .map(|e| e.seq)
        .collect();
    let mut sorted = project_seqs.clone();
    sorted.sort();
    assert_eq!(project_seqs, sorted, "seq must be monotonic per tier");
}

#[tokio::test]
async fn jsonl_sink_replay_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("project.changelog.jsonl");
    let cdc_writer = CdcWriter::new_with_sinks(None, Some(path.clone()), None).unwrap();
    cdc_writer
        .append_episode(
            Tier::Project,
            &Episode {
                id: EpisodeId::new(),
                tier: Tier::Project,
                ts: 1,
                episode_type: "x".into(),
                summary: "s".into(),
                atomic_facts: vec!["a".into()],
                source: "main-agent".into(),
                source_product: "wcore-agent".into(),
                session_id: None,
                project_root: None,
                decay_score: 1.0,
                status: EpisodeStatus::Active,
            },
        )
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    let line = raw.lines().next().expect("at least one line");
    let parsed: wcore_memory::cdc::CdcEntry = serde_json::from_str(line).unwrap();
    assert_eq!(parsed.op, "insert");
    assert_eq!(parsed.partition, "episodic");
    let _ = serde_json::from_value::<wcore_memory::cdc::EpisodePayload>(parsed.payload).unwrap();
}
