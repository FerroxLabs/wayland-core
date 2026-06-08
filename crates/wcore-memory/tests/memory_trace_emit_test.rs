//! M3.3.2 — `PartitionDispatcher`/`Memory` emit one trace per MemoryApi call
//! when a `MemoryTraceSink` is attached.
//!
//! Uses an `AtomicU64`-backed `CountingSink` so we don't bring in any
//! observability JSON dependency; the dispatcher only requires the
//! `MemoryTraceSink` trait (re-exported from `wcore-observability::sink`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use wcore_memory::api::MemoryApi;
use wcore_memory::memory::Memory;
use wcore_memory::v2_types::{
    AccessToken, Episode, EpisodeId, EpisodeStatus, Fact, FactId, Procedure, ProcedureId,
    ProcedureStatus, Query, Tier,
};
use wcore_observability::sink::MemoryTraceSink;

#[derive(Default)]
struct CountingSink {
    record_episode_emits: AtomicU64,
    assert_fact_emits: AtomicU64,
    upsert_procedure_emits: AtomicU64,
    list_procedures_emits: AtomicU64,
    search_emits: AtomicU64,
    user_model_emits: AtomicU64,
    update_user_model_emits: AtomicU64,
    dream_now_emits: AtomicU64,
    compact_emits: AtomicU64,
    get_episode_emits: AtomicU64,
    total: AtomicU64,
    last_success: AtomicU64, // 1 = true, 0 = false (only meaningful for the last emit)
}

impl MemoryTraceSink for CountingSink {
    fn emit(&self, op: &str, _partition: &str, _tier: &str, _latency_ms: u64, success: bool) {
        self.total.fetch_add(1, Ordering::SeqCst);
        self.last_success
            .store(u64::from(success), Ordering::SeqCst);
        match op {
            "record_episode" => {
                self.record_episode_emits.fetch_add(1, Ordering::SeqCst);
            }
            "assert_fact" => {
                self.assert_fact_emits.fetch_add(1, Ordering::SeqCst);
            }
            "upsert_procedure" => {
                self.upsert_procedure_emits.fetch_add(1, Ordering::SeqCst);
            }
            "list_procedures" => {
                self.list_procedures_emits.fetch_add(1, Ordering::SeqCst);
            }
            "search" => {
                self.search_emits.fetch_add(1, Ordering::SeqCst);
            }
            "user_model" => {
                self.user_model_emits.fetch_add(1, Ordering::SeqCst);
            }
            "update_user_model" => {
                self.update_user_model_emits.fetch_add(1, Ordering::SeqCst);
            }
            "dream_now" => {
                self.dream_now_emits.fetch_add(1, Ordering::SeqCst);
            }
            "compact" => {
                self.compact_emits.fetch_add(1, Ordering::SeqCst);
            }
            "get_episode" => {
                self.get_episode_emits.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn ep() -> Episode {
    Episode {
        id: EpisodeId::new(),
        tier: Tier::Project,
        ts: now_secs(),
        episode_type: "test".into(),
        summary: "hello".into(),
        atomic_facts: vec![],
        source: "test".into(),
        source_product: "wayland-core".into(),
        session_id: None,
        project_root: None,
        decay_score: 1.0,
        status: EpisodeStatus::Active,
    }
}

#[tokio::test]
async fn record_episode_emits_one_trace() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    mem.record_episode(ep(), AccessToken::System).await.unwrap();

    assert_eq!(sink.record_episode_emits.load(Ordering::SeqCst), 1);
    assert_eq!(sink.total.load(Ordering::SeqCst), 1);
    assert_eq!(
        sink.last_success.load(Ordering::SeqCst),
        1,
        "successful op should emit success=true"
    );
}

#[tokio::test]
async fn search_emits_one_trace() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    let _ = mem
        .search(Query::default(), AccessToken::MainAgent)
        .await
        .unwrap();

    assert_eq!(sink.search_emits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn user_model_read_emits_one_trace() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    mem.user_model(AccessToken::System).await.unwrap();

    assert_eq!(sink.user_model_emits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn assert_fact_emits_one_trace() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    let f = Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: now_secs(),
        subject: "x".into(),
        predicate: "is".into(),
        object: "y".into(),
        confidence: 1.0,
        source_episode: None,
        superseded_by: None,
    };
    mem.assert_fact(f, AccessToken::System).await.unwrap();

    assert_eq!(sink.assert_fact_emits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn upsert_and_list_procedures_emit_traces() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    let p = Procedure {
        id: ProcedureId::new(),
        tier: Tier::Project,
        ts: now_secs(),
        name: "test-skill".into(),
        description: "test desc".into(),
        artifact: "body".into(),
        status: ProcedureStatus::Active,
        created_by: "test".into(),
        thompson_alpha: 1.0,
        thompson_beta: 1.0,
        use_count: 0,
        success_count: 0,
    };
    mem.upsert_procedure(p, AccessToken::System).await.unwrap();
    mem.list_procedures(Tier::Project, AccessToken::System)
        .await
        .unwrap();

    assert_eq!(sink.upsert_procedure_emits.load(Ordering::SeqCst), 1);
    assert_eq!(sink.list_procedures_emits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dream_now_and_compact_emit_traces() {
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    let _ = mem.dream_now().await;
    let _ = mem.compact(1_000_000).await;

    assert_eq!(sink.dream_now_emits.load(Ordering::SeqCst), 1);
    assert_eq!(sink.compact_emits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn no_sink_attached_means_no_panic() {
    // Memory with no trace sink must still work end-to-end.
    let mem = Memory::open_in_memory().await.unwrap();
    mem.record_episode(ep(), AccessToken::System).await.unwrap();
    let _ = mem.search(Query::default(), AccessToken::MainAgent).await;
}

#[tokio::test]
async fn failed_op_emits_success_false() {
    use serde_json::Value;
    let mem = Memory::open_in_memory().await.unwrap();
    let sink: Arc<CountingSink> = Arc::new(CountingSink::default());
    let mem = mem.with_trace_sink(sink.clone());

    // SubAgent token cannot update_user_model (P5 is gated), so this errors.
    let res = mem
        .update_user_model(
            "k",
            Value::String("v".into()),
            AccessToken::SubAgent {
                agent_name: "r".into(),
            },
        )
        .await;
    assert!(res.is_err(), "sub-agent must be denied core writes");
    assert_eq!(sink.update_user_model_emits.load(Ordering::SeqCst), 1);
    assert_eq!(
        sink.last_success.load(Ordering::SeqCst),
        0,
        "denied op must emit success=false"
    );
}
