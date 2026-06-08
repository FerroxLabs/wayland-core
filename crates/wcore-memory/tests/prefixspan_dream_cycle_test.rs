// v0.6.4 Task 6.6a — PrefixSpan wired into the dream cycle.
//
// Verifies the design-doc `forge_canonical` golden: 3 sessions, each with
// the same `[read, edit, test]` ordered tool sequence, must produce 4
// staged procedures (one per pattern of length >= 2 at support=3,
// confidence=1.0): [read,edit,test], [read,edit], [read,test], [edit,test].
//
// Extraction strategy: parse `atomic_facts` entries that begin with the
// literal prefix `"tool:"` — the existing format already produced by
// `compact.rs:153` when offloading `WorkingEntry::ToolCall` rows. One
// `ToolSequence` is built per `session_id` with tools ordered by `ts`.

use std::sync::Arc;

use wcore_memory::audit::AuditLog;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::{Embedder, HashedEmbedder};
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::partition::PartitionDispatcher;
use wcore_memory::v2_types::{
    AccessToken, Episode, EpisodeId, EpisodeStatus, ProcedureStatus, Tier,
};

async fn fresh_dispatcher() -> PartitionDispatcher {
    let db = Arc::new(Db::open_memory().unwrap());
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = Arc::new(MemoryAccessGate::new(audit, AccessPolicy::empty()));
    let embedder: Arc<dyn Embedder> = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    PartitionDispatcher::new(gate, db, embedder, cdc, Some("sess".into()))
}

async fn seed_session(d: &PartitionDispatcher, session_id: &str, ts_base: i64) {
    // One episode per session whose `atomic_facts` carry the tool-call
    // sequence as `"tool:<name> ..."` strings in the order they occurred.
    // Matches the production format emitted by `compact.rs`.
    let facts = vec![
        "tool:read opened src/lib.rs".to_string(),
        "tool:edit added function foo".to_string(),
        "tool:test cargo test -p wcore-memory".to_string(),
    ];
    d.episodic
        .record(Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: ts_base,
            // Use a non-noise episode_type so the legacy crystallize path
            // ALSO fires; the asserts below tolerate the extra procedure.
            episode_type: "tool_run".into(),
            summary: format!("session {session_id} tool run"),
            atomic_facts: facts,
            source: "main-agent".into(),
            source_product: "wcore-agent".into(),
            session_id: Some(session_id.into()),
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn crystallize_mines_forge_canonical_patterns() {
    let d = fresh_dispatcher().await;
    seed_session(&d, "s1", 100).await;
    seed_session(&d, "s2", 200).await;
    seed_session(&d, "s3", 300).await;

    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    let emitted = engine.crystallize().await.unwrap();
    assert!(
        emitted >= 4,
        "expected crystallize to emit ≥4 procedures (4 PrefixSpan patterns), got {emitted}"
    );

    let procs = d.procedural.list(Tier::Project).await.unwrap();
    let names: Vec<String> = procs
        .iter()
        .filter(|p| p.status == ProcedureStatus::Staged && p.created_by == "evolution")
        .map(|p| p.name.clone())
        .collect();

    let expected = [
        "seq:read→edit→test",
        "seq:read→edit",
        "seq:read→test",
        "seq:edit→test",
    ];
    for want in expected {
        assert!(
            names.iter().any(|n| n == want),
            "missing staged procedure {want:?} in {names:?}"
        );
    }
}

#[tokio::test]
async fn crystallize_noop_when_no_tool_sequences() {
    // No episodes seeded → PrefixSpan should silently emit zero patterns,
    // and the legacy episode-type path emits zero too.
    let d = fresh_dispatcher().await;
    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    let emitted = engine.crystallize().await.unwrap();
    assert_eq!(emitted, 0, "empty corpus should crystallize nothing");
}

#[tokio::test]
async fn crystallize_ignores_non_tool_atomic_facts() {
    // atomic_facts without a `tool:` prefix must not contribute to
    // sequences. With only one session and no tool prefixes, PrefixSpan
    // sees zero ToolSequences and emits zero patterns.
    let d = fresh_dispatcher().await;
    d.episodic
        .record(Episode {
            id: EpisodeId::new(),
            tier: Tier::Project,
            ts: 1,
            episode_type: "free_form".into(),
            summary: "no tool calls here".into(),
            atomic_facts: vec![
                "user: how do I refactor this".into(),
                "assistant: extract a helper".into(),
            ],
            source: "main-agent".into(),
            source_product: "wcore-agent".into(),
            session_id: Some("only".into()),
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        })
        .await
        .unwrap();

    let engine = wcore_memory::consolidate::ConsolidationEngine::new(d.clone());
    let _ = engine.crystallize().await.unwrap();
    let procs = d.procedural.list(Tier::Project).await.unwrap();
    let seq_procs: Vec<_> = procs
        .iter()
        .filter(|p| p.name.starts_with("seq:"))
        .collect();
    assert!(
        seq_procs.is_empty(),
        "PrefixSpan should not emit any seq:* procedures when no atomic_facts carry a tool: prefix; got {seq_procs:?}"
    );
    // Silence unused-import warnings if AccessToken is referenced only by helpers.
    let _ = AccessToken::System;
}
