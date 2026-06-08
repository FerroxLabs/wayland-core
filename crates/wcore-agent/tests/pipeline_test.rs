//! A5 — integration tests for the no-barrier `pipeline(over, stages)` mechanic.
//!
//! These prove the four properties the PLAN/SPEC demand of the scheduler:
//!
//! 1. **No barrier (timing proof):** a fast item completes ALL stages before a
//!    slow item finishes stage 1. The barrier baseline (a `Parallel` step,
//!    which DOES join) is shown to behave differently.
//! 2. **Single-item failure isolation:** one item's stage error drops exactly
//!    that item to `null`; the others complete.
//! 3. **Order + holes:** the result length equals the input length, with the
//!    `null` hole in the dropped item's position.
//! 4. **Concurrency cap:** no more than K stage agents are ever in flight.
//!
//! Each item is tagged `TAG=<name>` in its seed value; the mock provider keys
//! its per-item delay and its completion log off that tag, and re-embeds the
//! tag in its output so the tag survives across stages (each stage's prompt
//! carries the prior stage's output).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::test_config;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wcore_agent::orchestration::workflow::runner::{WorkflowPlan, WorkflowRunner};
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Extract the `TAG=<word>` marker from a request's serialized prompt.
fn tag_of(request: &LlmRequest) -> Option<String> {
    let dump = format!("{request:?}");
    let idx = dump.find("TAG=")?;
    let rest = &dump[idx + 4..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn ok_events(text: String) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

/// A provider that delays each stage by a per-tag amount and records the
/// completion order as `(tag, global_seq)`. It re-embeds `TAG=<tag>` in its
/// output so the tag survives stage→stage threading. It also tracks the
/// maximum number of concurrently in-flight `stream` calls (the concurrency
/// cap proof).
struct TimedProvider {
    /// tag → delay applied to EVERY stage of that item.
    delays: std::collections::HashMap<String, Duration>,
    /// Completion log: tag in the order stages finished.
    completions: Arc<Mutex<Vec<String>>>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

impl TimedProvider {
    fn new(
        delays: &[(&str, Duration)],
        completions: Arc<Mutex<Vec<String>>>,
        in_flight: Arc<AtomicUsize>,
        max_in_flight: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            delays: delays.iter().map(|(t, d)| (t.to_string(), *d)).collect(),
            completions,
            in_flight,
            max_in_flight,
        }
    }
}

#[async_trait]
impl LlmProvider for TimedProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let tag = tag_of(request).unwrap_or_else(|| "untagged".to_string());
        let delay = self.delays.get(&tag).copied().unwrap_or(Duration::ZERO);

        // Track in-flight concurrency.
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.completions.lock().unwrap().push(tag.clone());

        let (tx, rx) = mpsc::channel(8);
        // Re-embed the tag so the next stage's prompt still carries it.
        tokio::spawn(async move {
            for ev in ok_events(format!("done TAG={tag}")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A provider that errors on every stage of one specific tagged item, and
/// succeeds (re-embedding the tag) for all others.
struct FailTagProvider {
    fail_tag: String,
}

#[async_trait]
impl LlmProvider for FailTagProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let tag = tag_of(request).unwrap_or_else(|| "untagged".to_string());
        if tag == self.fail_tag {
            return Err(ProviderError::Connection("boom".into()));
        }
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            for ev in ok_events(format!("done TAG={tag}")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A two-item, three-stage no-barrier pipeline over `changed_files`.
fn three_stage_pipeline_src() -> &'static str {
    r#"
Workflow(
    meta: (name: "nobar", est_agents: 6),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("changed_files"), stages: [
            (id: "s1", prompt: "stage one"),
            (id: "s2", prompt: "stage two"),
            (id: "s3", prompt: "stage three"),
        ]),
    ])],
)
"#
}

/// 1. NO-BARRIER TIMING PROOF.
///
/// Two items, `fast` and `slow`, three stages each. The `slow` item's stage 1
/// sleeps long; `fast` stages are instant. With true item-level streaming the
/// `fast` item completes ALL three of its stages before `slow` even finishes
/// stage 1. A barrier-per-stage scheduler could not produce this ordering: it
/// would have to finish stage 1 for BOTH items (incl. the long sleep) before
/// any item's stage 2 ran.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_barrier_fast_item_finishes_all_stages_before_slow_finishes_stage_one() {
    let completions = Arc::new(Mutex::new(Vec::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(TimedProvider::new(
        &[("slow", Duration::from_millis(300))],
        Arc::clone(&completions),
        Arc::clone(&in_flight),
        Arc::clone(&max_in_flight),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(three_stage_pipeline_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let initial = json!({ "changed_files": ["TAG=fast", "TAG=slow"] });

    let result = runner.run(&plan, initial).await.expect("pipeline runs");

    let log = completions.lock().unwrap().clone();
    // The fast item must have logged all three of its stage completions before
    // the slow item's first stage completion appears.
    let first_slow = log.iter().position(|t| t == "slow").expect("slow ran");
    let fast_before_first_slow = log[..first_slow].iter().filter(|t| *t == "fast").count();
    assert_eq!(
        fast_before_first_slow, 3,
        "fast item should finish all 3 stages before slow finishes stage 1; log = {log:?}"
    );

    // Both items completed: the result array has two non-null entries.
    let items = result.final_state["pl"].as_array().expect("pl is an array");
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|v| !v.is_null()), "both items complete");
}

/// 2 + 3. SINGLE-ITEM FAILURE drops exactly one item to `null`; the others
/// complete; result length == input length with the hole in the right slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_stage_failure_drops_exactly_one_item_to_null_preserving_order() {
    let provider = Arc::new(FailTagProvider {
        fail_tag: "mid".to_string(),
    });
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(three_stage_pipeline_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    // Three items; the middle one ("mid") errors on stage 1.
    let initial = json!({ "changed_files": ["TAG=a", "TAG=mid", "TAG=c"] });

    let result = runner
        .run(&plan, initial)
        .await
        .expect("run does not abort");

    let items = result.final_state["pl"].as_array().expect("pl is an array");
    // Length preserved.
    assert_eq!(items.len(), 3, "result length == input length");
    // The hole is in position 1 (the `mid` item).
    assert!(!items[0].is_null(), "item a completes");
    assert!(items[1].is_null(), "item mid dropped to null");
    assert!(!items[2].is_null(), "item c completes");
}

/// 4. CONCURRENCY CAP honored: with a cap of K=2 and 6 items each delayed,
/// the provider never sees more than 2 stage agents in flight simultaneously.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrency_cap_bounds_in_flight_stage_agents() {
    let completions = Arc::new(Mutex::new(Vec::new()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    // Every item delayed equally so many would otherwise overlap.
    let delays: Vec<(&str, Duration)> = vec![
        ("i0", Duration::from_millis(60)),
        ("i1", Duration::from_millis(60)),
        ("i2", Duration::from_millis(60)),
        ("i3", Duration::from_millis(60)),
        ("i4", Duration::from_millis(60)),
        ("i5", Duration::from_millis(60)),
    ];
    let provider = Arc::new(TimedProvider::new(
        &delays,
        Arc::clone(&completions),
        Arc::clone(&in_flight),
        Arc::clone(&max_in_flight),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    // Single-stage pipeline keeps the test about the cap, not stage chaining.
    let src = r#"
Workflow(
    meta: (name: "capped", est_agents: 6),
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("changed_files"), stages: [
            (id: "s1", prompt: "only stage"),
        ]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::with_pipeline_concurrency(&spawner, 2);
    let initial = json!({
        "changed_files": ["TAG=i0", "TAG=i1", "TAG=i2", "TAG=i3", "TAG=i4", "TAG=i5"]
    });

    let result = runner.run(&plan, initial).await.expect("pipeline runs");

    assert!(
        max_in_flight.load(Ordering::SeqCst) <= 2,
        "no more than K=2 stage agents in flight, saw {}",
        max_in_flight.load(Ordering::SeqCst)
    );
    // All six items still completed.
    let items = result.final_state["pl"].as_array().expect("pl is an array");
    assert_eq!(items.len(), 6);
    assert!(items.iter().all(|v| !v.is_null()), "all items complete");
}

/// A provider that always returns schema-INVALID output (a bare string when an
/// object is required), forcing the schema-retry budget to be exhausted on
/// every item. It delays each call and tracks the max concurrent in-flight
/// `stream` calls — so a retry path that escapes the semaphore would show up as
/// `max_in_flight` exceeding the cap.
struct InvalidSchemaDelayProvider {
    delay: Duration,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmProvider for InvalidSchemaDelayProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);

        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            // Always invalid: the schema requires an object, this is a string.
            for ev in ok_events("\"not an object\"".to_string()) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// FIX 1 regression: schema-validation RETRIES inside a no-barrier pipeline
/// must hold a semaphore permit too. With a cap of K=1 and multiple items whose
/// single schema stage always fails validation (forcing 1 + MAX_SCHEMA_RETRIES
/// dispatches each), the provider must never see more than 1 stage agent in
/// flight — even though every dispatch beyond the first is a retry. Before the
/// fix the retry re-dispatches bypassed the semaphore, so several retries
/// overlapped and `max_in_flight` exceeded the cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn schema_retries_respect_pipeline_concurrency_cap() {
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(InvalidSchemaDelayProvider {
        delay: Duration::from_millis(40),
        in_flight: Arc::clone(&in_flight),
        max_in_flight: Arc::clone(&max_in_flight),
    });
    let spawner = AgentSpawner::new(provider, test_config());

    // Single schema-bearing stage; every item fails validation and retries.
    let src = r#"
Workflow(
    meta: (name: "retry-cap", est_agents: 3),
    schemas: { "obj": "{ \"type\": \"object\" }" },
    phases: [Phase(title: "p", steps: [
        Pipeline(id: "pl", over: Some("changed_files"), stages: [
            (id: "s1", prompt: "only stage", schema: Some("obj")),
        ]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::with_pipeline_concurrency(&spawner, 1);
    let initial = json!({ "changed_files": ["TAG=a", "TAG=b", "TAG=c"] });

    let result = runner.run(&plan, initial).await.expect("pipeline runs");

    assert_eq!(
        max_in_flight.load(Ordering::SeqCst),
        1,
        "schema retries must hold a permit: cap is 1 but saw {} concurrent dispatches",
        max_in_flight.load(Ordering::SeqCst)
    );
    // Every item exhausted its retry budget and dropped to null.
    let items = result.final_state["pl"].as_array().expect("pl is an array");
    assert_eq!(items.len(), 3);
    assert!(
        items.iter().all(Value::is_null),
        "all items drop to null after exhausting schema retries"
    );
}

/// A `Value` whose `over` ref does not resolve to an array runs zero items and
/// writes an empty array — never panics.
#[tokio::test]
async fn non_array_over_runs_zero_items() {
    let provider = Arc::new(FailTagProvider {
        fail_tag: "never".to_string(),
    });
    let spawner = AgentSpawner::new(provider, test_config());
    let plan = WorkflowPlan::parse(three_stage_pipeline_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    // `changed_files` is absent → Select yields Null → zero items.
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("empty pipeline runs");
    let items = result.final_state["pl"].as_array().expect("pl is an array");
    assert!(items.is_empty(), "no items, empty result array");
    // GAP-2: a missing `over` ref must surface a VISIBLE stage explaining why
    // zero agents ran — not a silent empty "completed". A missing key is a
    // malformed-plan signal, so the stage is flagged as an error.
    let pl_stage = result
        .stage_results
        .iter()
        .find(|s| s.node_id == "pl")
        .expect("the zero-item pipeline must record a visible stage");
    assert!(
        pl_stage.is_error,
        "a missing over-ref is a malformed-plan signal: {}",
        pl_stage.text
    );
    assert!(
        pl_stage.text.contains("not found") && pl_stage.text.contains("0 agents"),
        "stage must name the missing-key reason: {}",
        pl_stage.text
    );
}

/// GAP-1: an `over` ref that resolves to an empty array (e.g. a clean git tree
/// → empty `changed_files`) dispatches zero agents — a legitimate no-work
/// outcome, but it must still be VISIBLE in the run summary rather than a silent
/// empty success. Unlike a missing ref, an empty array is NOT flagged as an
/// error.
#[tokio::test]
async fn empty_over_array_records_a_visible_non_error_stage_gap1() {
    let provider = Arc::new(FailTagProvider {
        fail_tag: "never".to_string(),
    });
    let spawner = AgentSpawner::new(provider, test_config());
    let plan = WorkflowPlan::parse(three_stage_pipeline_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    // changed_files present but empty (clean tree) → zero items.
    let result = runner
        .run(&plan, json!({ "changed_files": [] }))
        .await
        .expect("empty pipeline runs");
    let pl_stage = result
        .stage_results
        .iter()
        .find(|s| s.node_id == "pl")
        .expect("the zero-item pipeline must record a visible stage");
    assert!(
        !pl_stage.is_error,
        "an empty array is legitimate no-work, not an error: {}",
        pl_stage.text
    );
    assert!(
        pl_stage.text.contains("was empty") && pl_stage.text.contains("0 agents"),
        "stage must name the empty-input reason: {}",
        pl_stage.text
    );
}
