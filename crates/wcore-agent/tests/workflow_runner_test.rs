//! A3 — integration tests for `WorkflowRunner`.
//!
//! Promotes `tests/workflow_runner_spike.rs` from a hand-rolled kernel into
//! coverage of the real `WorkflowRunner` over a lowered `GraphConfig`,
//! modeled on the spike + `fleet_dispatcher_wired_test.rs`. A capturing
//! provider records each request so we can assert data threads between
//! stages, exactly as the spike does.
//!
//! Coverage:
//! 1. A 3-stage linear workflow (parsed from RON via A1) executes ALL stages
//!    and threads each stage's output into the next stage's prompt.
//! 2. A parallel fan-out runs N sibling branches and an aggregator collects
//!    their outputs into an array on the aggregator's state key.
//! 3. A stage failure surfaces as a typed `StageFailed` error carrying the
//!    partial result (prior completed stages are not discarded).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::test_config;
use serde_json::Value;
use tokio::sync::mpsc;
use wcore_agent::agents::bus::{AgentBus, AgentMessage};
use wcore_agent::orchestration::workflow::runner::{
    WorkflowPlan, WorkflowRunError, WorkflowRunner,
};
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// Records every request (Debug-formatted) and returns a distinct,
/// turn-indexed response so each stage is individually observable — the same
/// instrument the spike uses.
struct CapturingProvider {
    seen: Arc<Mutex<Vec<String>>>,
    turn: Mutex<usize>,
}

impl CapturingProvider {
    fn new(seen: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            seen,
            turn: Mutex::new(0),
        }
    }
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

#[async_trait]
impl LlmProvider for CapturingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        self.seen.lock().unwrap().push(format!("{request:?}"));
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(format!("STAGE{n}-OUT")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A provider that succeeds for the first `fail_at` calls (0-indexed) and then
/// returns an LLM-layer error on EVERY call from `fail_at` onward. Used to prove
/// a mid-run stage failure surfaces partial results.
///
/// The failure must be persistent, not a single call: the engine's stream-retry
/// loop (`MAX_STREAM_RETRIES`) correctly retries a transient
/// `ProviderError::Connection`, so a one-shot failure at `fail_at` would be
/// recovered on the next attempt and the stage would (rightly) succeed. Failing
/// from `fail_at` onward exhausts the retries and produces a genuine stage
/// failure — the `StageFailed` path this test exercises.
struct FailAtProvider {
    fail_at: usize,
    turn: Mutex<usize>,
}

#[async_trait]
impl LlmProvider for FailAtProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        if n >= self.fail_at {
            return Err(ProviderError::Connection("boom".into()));
        }
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(format!("STAGE{n}-OUT")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// Returns a configured text per call, in order, recording each request so
/// retry prompts can be inspected. Each workflow `spawn_one` is exactly one LLM
/// call, so for a single-node workflow call index == retry attempt index.
struct SequencedProvider {
    texts: Vec<String>,
    seen: Arc<Mutex<Vec<String>>>,
    turn: Mutex<usize>,
}

impl SequencedProvider {
    fn new(texts: Vec<&str>, seen: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            texts: texts.into_iter().map(String::from).collect(),
            seen,
            turn: Mutex::new(0),
        }
    }
}

#[async_trait]
impl LlmProvider for SequencedProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        self.seen.lock().unwrap().push(format!("{request:?}"));
        // Past the configured list, keep emitting the last text (stable tail).
        let text = self
            .texts
            .get(n)
            .or_else(|| self.texts.last())
            .cloned()
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(text) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// A single-node workflow whose agent declares the `findings` schema.
fn schema_workflow_src() -> &'static str {
    r#"
Workflow(
    meta: (name: "schema-flow", est_agents: 1),
    schemas: {
        "findings": "{ \"type\": \"object\", \"required\": [\"findings\"], \"properties\": { \"findings\": { \"type\": \"array\", \"items\": { \"type\": \"string\" } } } }",
    },
    phases: [Phase(title: "scan", steps: [
        Agent((id: "scan", prompt: "scan the diff", schema: Some("findings"))),
    ])],
)
"#
}

/// 4. A schema-conforming agent return passes on the first try and the runner
///    stores the *structured* (parsed) JSON into state — not the raw text.
#[tokio::test]
async fn schema_conforming_output_passes_first_try_and_stores_structured_data() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(SequencedProvider::new(
        vec![r#"{ "findings": ["a", "b"] }"#],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(schema_workflow_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("conforming output should run to completion");

    // Exactly one LLM call: no retry happened.
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "schema node should not retry"
    );

    // The state holds the PARSED object, so downstream refs see structured
    // data (an object with a `findings` array), not a JSON string.
    let scan = result
        .final_state
        .get("scan")
        .expect("scan output stored in state");
    assert!(
        scan.is_object(),
        "validated output must be stored as JSON, got {scan:?}"
    );
    let findings = scan
        .get("findings")
        .and_then(Value::as_array)
        .expect("findings array present");
    let items: Vec<&str> = findings.iter().filter_map(Value::as_str).collect();
    assert_eq!(items, vec!["a", "b"]);
}

/// 5. A malformed first output retries exactly once, then the corrected return
///    validates and the run succeeds. The retry prompt carries the validation
///    error so the agent knows what to fix.
#[tokio::test]
async fn schema_mismatch_retries_once_then_succeeds() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // Turn 0: malformed (a bare array, schema wants an object). Turn 1: valid.
    let provider = Arc::new(SequencedProvider::new(
        vec![r#"["not", "an", "object"]"#, r#"{ "findings": ["ok"] }"#],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(schema_workflow_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("a single retry should recover");

    // Exactly two LLM calls: the original + one retry.
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "expected exactly one retry (2 total calls)");
    // The retry prompt fed the validation error back to the agent.
    assert!(
        seen[1].contains("did not match the required schema"),
        "retry prompt must carry the schema correction; got: {}",
        seen[1]
    );

    // The corrected, validated object is stored.
    let scan = result.final_state.get("scan").expect("scan stored");
    assert_eq!(
        scan.get("findings").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
}

/// 6. Persistent mismatch: every attempt returns malformed output, so after the
///    retry budget the runner surfaces a typed `SchemaValidationFailed` error
///    carrying the partial result.
#[tokio::test]
async fn schema_persistent_mismatch_surfaces_typed_error_after_retries() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    // Always malformed (a string, never the required object).
    let provider = Arc::new(SequencedProvider::new(
        vec![r#""still not an object""#],
        Arc::clone(&seen),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(schema_workflow_src()).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let err = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect_err("persistent mismatch must fail");

    match err {
        WorkflowRunError::SchemaValidationFailed {
            stage,
            attempts,
            partial,
            ..
        } => {
            assert_eq!(stage, "scan");
            // 1 original + MAX_SCHEMA_RETRIES (2) = 3 attempts.
            assert_eq!(attempts, 3, "should exhaust the original + 2 retries");
            // The failed stage is recorded in the partial as errored.
            let scan = partial
                .stage_results
                .iter()
                .find(|s| s.node_id == "scan")
                .expect("scan stage recorded in partial");
            assert!(scan.is_error, "scan must be marked errored");
        }
        other => panic!("expected SchemaValidationFailed, got {other:?}"),
    }
    // 3 total LLM calls: original + 2 retries.
    assert_eq!(seen.lock().unwrap().len(), 3);
}

/// 1. A 3-stage linear workflow executes ALL stages, threading each stage's
///    output into the next stage's prompt — the runner's escape from the
///    first-dispatch-wins guard, now over a real lowered `GraphConfig`.
#[tokio::test]
async fn linear_workflow_executes_all_stages_and_threads_data() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config());

    // A pipeline of 3 stages; each later stage reads its predecessor's output
    // via a flat-key Select input ref (lowered by A1).
    let src = r#"
Workflow(
    meta: (name: "linear", description: "three stage chain", est_agents: 3),
    phases: [Phase(title: "run", steps: [
        Pipeline(id: "pipe", stages: [
            (id: "ingest", prompt: "ingest the data"),
            (id: "review", prompt: "review prior", input: Some("ingest")),
            (id: "verify", prompt: "verify prior", input: Some("review")),
        ]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("linear workflow should run to completion");

    // All three stages produced an output in the final state.
    let state = &result.final_state;
    assert_eq!(
        state.get("ingest").and_then(Value::as_str),
        Some("STAGE0-OUT")
    );
    assert_eq!(
        state.get("review").and_then(Value::as_str),
        Some("STAGE1-OUT")
    );
    assert_eq!(
        state.get("verify").and_then(Value::as_str),
        Some("STAGE2-OUT")
    );

    // Per-stage results recorded in execution order (3 AgentCalls).
    let agent_stages: Vec<&str> = result
        .stage_results
        .iter()
        .filter(|s| !s.is_error && s.turns >= 1)
        .map(|s| s.node_id.as_str())
        .collect();
    assert_eq!(agent_stages, vec!["ingest", "review", "verify"]);

    // Data threaded between stages: stage 2's request carries stage 1's
    // output, stage 3's carries stage 2's — identical to the spike's proof.
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "expected exactly one LLM call per stage");
    assert!(
        seen[1].contains("STAGE0-OUT"),
        "stage 2 prompt missing stage 1 output; got: {}",
        seen[1]
    );
    assert!(
        seen[2].contains("STAGE1-OUT"),
        "stage 3 prompt missing stage 2 output; got: {}",
        seen[2]
    );
}

/// 2. A parallel fan-out runs N sibling branches concurrently and an
///    aggregator collects their outputs into an array.
#[tokio::test]
async fn parallel_fanout_collects_sibling_outputs() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config());

    let src = r#"
Workflow(
    meta: (name: "fanout", est_agents: 3),
    phases: [Phase(title: "vote", steps: [
        Parallel(id: "tally", branches: [
            (id: "judge_a", prompt: "judge a"),
            (id: "judge_b", prompt: "judge b"),
            (id: "judge_c", prompt: "judge c"),
        ], join: Collect),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("fan-out workflow should run to completion");

    // Each branch produced an output.
    for branch in ["judge_a", "judge_b", "judge_c"] {
        assert!(
            result
                .final_state
                .get(branch)
                .and_then(Value::as_str)
                .is_some(),
            "branch {branch} produced no output"
        );
    }

    // The aggregator collected the three branch outputs into an array on its
    // own state key.
    let tally = result
        .final_state
        .get("tally")
        .and_then(Value::as_array)
        .expect("aggregator `tally` should hold a collected array");
    assert_eq!(tally.len(), 3, "aggregator should collect all 3 branches");
    let collected: Vec<&str> = tally.iter().filter_map(Value::as_str).collect();
    for branch_out in ["STAGE0-OUT", "STAGE1-OUT", "STAGE2-OUT"] {
        assert!(
            collected.contains(&branch_out),
            "collected array missing {branch_out}; got {collected:?}"
        );
    }

    // All three branches dispatched (one LLM call each).
    assert_eq!(seen.lock().unwrap().len(), 3);

    // Three AgentCall stage results plus the synthetic fan root + aggregator.
    let agent_stage_count = result.stage_results.iter().filter(|s| s.turns >= 1).count();
    assert_eq!(agent_stage_count, 3);
}

/// FIX 2 regression: a `Parallel(... join: Collect)` aggregator's collected
/// array must be reachable downstream via the aggregator NODE id. Before the
/// fix the Collect reducer was registered under the literal `"output"` key,
/// which `apply_aggregator` (keyed by the aggregator id) never looked up, so
/// a downstream stage reading the aggregator id saw the wrong value.
#[tokio::test]
async fn collect_aggregator_output_is_reachable_downstream_via_aggregator_id() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config());

    // Two branches collected into aggregator `tally`, then a `summarize` stage
    // reads `tally` as its input — proving the collected array threads through.
    let src = r#"
Workflow(
    meta: (name: "collect-downstream", est_agents: 3),
    phases: [Phase(title: "vote", steps: [
        Parallel(id: "tally", branches: [
            (id: "judge_a", prompt: "judge a"),
            (id: "judge_b", prompt: "judge b"),
        ], join: Collect),
        Agent((id: "summarize", prompt: "summarize", input: Some("tally"))),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("collect + downstream read should run to completion");

    // The aggregator holds the collected array (driven by the Collect reducer
    // keyed by `tally`, not the strategy default).
    let tally = result
        .final_state
        .get("tally")
        .and_then(Value::as_array)
        .expect("aggregator `tally` should hold a collected array");
    assert_eq!(tally.len(), 2, "both branch outputs collected");

    // The downstream `summarize` stage saw the collected array in its prompt —
    // i.e. the reducer output was reachable via the aggregator id, not Null.
    let seen = seen.lock().unwrap();
    let summarize_req = seen
        .last()
        .expect("summarize stage should have dispatched an LLM call");
    assert!(
        summarize_req.contains("STAGE0-OUT") && summarize_req.contains("STAGE1-OUT"),
        "summarize prompt must carry the collected branch outputs (reachable \
         via the aggregator id); got: {summarize_req}"
    );
}

/// 3. A stage failure surfaces as a typed `StageFailed` error carrying the
///    partial result — prior completed stages are preserved, not discarded.
#[tokio::test]
async fn stage_failure_surfaces_typed_error_with_partial_results() {
    // 3-stage linear chain; the 2nd LLM call (turn index 1, the `review`
    // stage) fails, so `ingest` should already be recorded as partial.
    let provider = Arc::new(FailAtProvider {
        fail_at: 1,
        turn: Mutex::new(0),
    });
    let spawner = AgentSpawner::new(provider, test_config());

    let src = r#"
Workflow(
    meta: (name: "failing", est_agents: 3),
    phases: [Phase(title: "run", steps: [
        Pipeline(id: "pipe", stages: [
            (id: "ingest", prompt: "ingest"),
            (id: "review", prompt: "review", input: Some("ingest")),
            (id: "verify", prompt: "verify", input: Some("review")),
        ]),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let err = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect_err("a failing stage must surface a typed error");

    match err {
        WorkflowRunError::StageFailed { stage, partial, .. } => {
            assert_eq!(stage, "review", "the failing stage should be `review`");
            // The prior `ingest` stage's result is preserved in the partial.
            assert_eq!(
                partial.final_state.get("ingest").and_then(Value::as_str),
                Some("STAGE0-OUT"),
                "partial result must retain the completed `ingest` stage"
            );
            // `ingest` recorded as a successful stage; `review` recorded as
            // the errored one. `verify` never ran.
            let ids: Vec<&str> = partial
                .stage_results
                .iter()
                .map(|s| s.node_id.as_str())
                .collect();
            assert!(ids.contains(&"ingest"), "partial should include `ingest`");
            assert!(
                ids.contains(&"review"),
                "partial should include the failed `review`"
            );
            assert!(!ids.contains(&"verify"), "`verify` must not have run");
            let review = partial
                .stage_results
                .iter()
                .find(|s| s.node_id == "review")
                .expect("review stage recorded");
            assert!(review.is_error, "`review` stage must be marked errored");
        }
        other => panic!("expected StageFailed, got {other:?}"),
    }
}

/// Build a `Parallel` workflow RON with `n` sibling branches, each its own
/// `AgentCall`, joined by `Collect`. Used to exercise the fan-out threshold:
/// `n > FLEET_FANOUT_THRESHOLD` (10) must shard through `FleetDispatcher`.
fn parallel_workflow_with_branches(n: usize) -> String {
    let branches: String = (0..n)
        .map(|i| format!("            (id: \"branch_{i}\", prompt: \"branch {i}\"),\n"))
        .collect();
    format!(
        "Workflow(\n\
         \x20   meta: (name: \"wide\", est_agents: {n}),\n\
         \x20   phases: [Phase(title: \"vote\", steps: [\n\
         \x20       Parallel(id: \"tally\", branches: [\n{branches}\
         \x20       ], join: Collect),\n\
         \x20   ])],\n\
         )\n"
    )
}

/// Drain up to `expected` bus events (or until `timeout` elapses) so a
/// fleet-vs-relay assertion can inspect the `parent_call_id` tags.
async fn drain_bus(
    rx: &mut tokio::sync::broadcast::Receiver<AgentMessage>,
    expected: usize,
    timeout: Duration,
) -> Vec<AgentMessage> {
    let mut out = Vec::new();
    while out.len() < expected {
        match tokio::time::timeout(timeout, rx.recv()).await {
            Ok(Ok(msg)) => out.push(msg),
            _ => break,
        }
    }
    out
}

/// Count `Spawned` events whose `parent_call_id` begins with `"fleet:"` — the
/// wire-presence signal that the `FleetDispatcher` sharding path ran (the same
/// signal `fleet_dispatcher_wired_test.rs` asserts on).
fn fleet_tagged_count(events: &[AgentMessage]) -> usize {
    events
        .iter()
        .filter(|ev| {
            matches!(
                ev,
                AgentMessage::Spawned { parent_call_id: Some(pid), .. } if pid.starts_with("fleet:")
            )
        })
        .count()
}

/// FIX A: a fan-out WIDER than one shard (`> FLEET_FANOUT_THRESHOLD` = 10)
/// routes through `spawn_via_fleet` — proven via the `fleet:` `parent_call_id`
/// prefix on the bus — and every branch's result still maps back to its own
/// node id (fleet returns results in shard order, not input order, so this also
/// locks in the by-name correlation).
#[tokio::test]
async fn wide_fanout_routes_through_fleet_and_maps_results_to_nodes() {
    let bus = Arc::new(AgentBus::new(512));
    let mut rx = bus.subscribe();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    // 11 siblings — the smallest fan-out that exceeds the threshold and crosses
    // the shard boundary (shard size 10).
    let src = parallel_workflow_with_branches(11);
    let plan = WorkflowPlan::parse(&src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("wide fan-out should run to completion");

    // Every branch node has a non-error output in the final state — i.e. each
    // shard-ordered result was correlated back to the right node id.
    for i in 0..11 {
        let id = format!("branch_{i}");
        let v = result
            .final_state
            .get(&id)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("branch `{id}` produced no output"));
        assert!(!v.is_empty(), "branch `{id}` output empty");
    }

    // The aggregator collected all 11 branch outputs.
    let tally = result
        .final_state
        .get("tally")
        .and_then(Value::as_array)
        .expect("aggregator `tally` should hold a collected array");
    assert_eq!(tally.len(), 11, "all 11 branches collected");

    // Wire-presence: at least the 11 sub-agents were spawned through the fleet
    // path (each Spawned tagged `fleet:...`).
    let events = drain_bus(&mut rx, 11 * 3, Duration::from_secs(5)).await;
    assert!(
        fleet_tagged_count(&events) >= 11,
        "wide fan-out must route through FleetDispatcher (>=11 `fleet:`-tagged \
         Spawned events); got {}. Events: {events:#?}",
        fleet_tagged_count(&events)
    );
}

/// FIX A negative control: a fan-out AT OR BELOW the threshold keeps the
/// per-task relay path — NO `fleet:` tags on the bus — while still producing all
/// branch outputs correctly.
#[tokio::test]
async fn narrow_fanout_stays_on_relay_path_no_fleet() {
    let bus = Arc::new(AgentBus::new(256));
    let mut rx = bus.subscribe();

    let seen = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
    let spawner = AgentSpawner::new(provider, test_config()).with_bus(Arc::clone(&bus));

    // Exactly 10 siblings — the threshold itself, which must NOT shard.
    let src = parallel_workflow_with_branches(10);
    let plan = WorkflowPlan::parse(&src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect("narrow fan-out should run to completion");

    let tally = result
        .final_state
        .get("tally")
        .and_then(Value::as_array)
        .expect("aggregator `tally` should hold a collected array");
    assert_eq!(tally.len(), 10, "all 10 branches collected");

    let events = drain_bus(&mut rx, 10 * 3, Duration::from_secs(5)).await;
    assert_eq!(
        fleet_tagged_count(&events),
        0,
        "a fan-out at the threshold must stay on the relay path (no `fleet:` \
         tags); got {} fleet-tagged events",
        fleet_tagged_count(&events)
    );
}

/// A provider that fails any request whose Debug-rendered body contains the
/// configured `needle` (a unique branch prompt substring), and succeeds for
/// every other request with a per-call distinct output. Used by FIX A to fail
/// exactly ONE named sibling in a concurrent wave, deterministically — unlike
/// `FailAtProvider`, which keys on a non-deterministic call-arrival index.
struct FailNamedProvider {
    needle: String,
    turn: Mutex<usize>,
}

#[async_trait]
impl LlmProvider for FailNamedProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let n = {
            let mut t = self.turn.lock().unwrap();
            let v = *t;
            *t += 1;
            v
        };
        if format!("{request:?}").contains(&self.needle) {
            return Err(ProviderError::Connection("boom".into()));
        }
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(format!("OK{n}")) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// FIX A: in a single parallel wave with one failing + two succeeding siblings,
/// the run returns the typed `StageFailed` error BUT the partial result retains
/// BOTH successful siblings' outputs in `final_state` and as committed
/// `stage_results`. Before the fix, the loop returned on the FIRST errored
/// sibling, discarding any sibling processed after it in the same wave.
#[tokio::test]
async fn parallel_wave_failing_sibling_preserves_successful_siblings() {
    // 3-branch fan-out (<= FLEET_FANOUT_THRESHOLD = 10) → relay path. The
    // `loser` branch fails; `winner_a` / `winner_b` succeed and must survive in
    // the partial.
    let provider = Arc::new(FailNamedProvider {
        needle: "LOSE_THIS_BRANCH".to_string(),
        turn: Mutex::new(0),
    });
    let spawner = AgentSpawner::new(provider, test_config());

    let src = r#"
Workflow(
    meta: (name: "partial-fanout", est_agents: 3),
    phases: [Phase(title: "vote", steps: [
        Parallel(id: "tally", branches: [
            (id: "winner_a", prompt: "win a"),
            (id: "loser", prompt: "LOSE_THIS_BRANCH"),
            (id: "winner_b", prompt: "win b"),
        ], join: Collect),
    ])],
)
"#;
    let plan = WorkflowPlan::parse(src).expect("workflow should parse");
    let runner = WorkflowRunner::new(&spawner);
    let err = runner
        .run(&plan, Value::Object(Default::default()))
        .await
        .expect_err("a failing sibling must surface a typed error");

    match err {
        WorkflowRunError::StageFailed { stage, partial, .. } => {
            assert_eq!(stage, "loser", "the failing sibling should be `loser`");

            // BOTH successful siblings committed to state despite the failure.
            for w in ["winner_a", "winner_b"] {
                let v = partial.final_state.get(w).and_then(Value::as_str);
                assert!(
                    v.is_some_and(|s| s.starts_with("OK")),
                    "successful sibling `{w}` must be retained in the partial state; \
                     got {:?}",
                    partial.final_state.get(w)
                );
            }

            // The failed sibling left NO state entry (it errored).
            assert!(
                partial.final_state.get("loser").is_none(),
                "failed sibling must not write a state value"
            );

            // Both winners recorded as successful stage results; loser as errored.
            let ok_ids: Vec<&str> = partial
                .stage_results
                .iter()
                .filter(|s| !s.is_error)
                .map(|s| s.node_id.as_str())
                .collect();
            assert!(
                ok_ids.contains(&"winner_a") && ok_ids.contains(&"winner_b"),
                "both successful siblings must appear as committed stage_results; \
                 got {ok_ids:?}"
            );
            let loser = partial
                .stage_results
                .iter()
                .find(|s| s.node_id == "loser")
                .expect("loser stage recorded");
            assert!(loser.is_error, "`loser` must be marked errored");
        }
        other => panic!("expected StageFailed, got {other:?}"),
    }
}

/// FIX C: `JoinStrategy::Merge` and `JoinStrategy::Concat` are documented v1
/// aliases of the array fold — the runner's `apply_aggregator` renders both as
/// a JSON array (identical to `Collect`), not a deep-merge / string-concat. This
/// locks in the documented, non-surprising behaviour the honest comments claim.
#[tokio::test]
async fn merge_and_concat_joins_fold_to_array_in_v1() {
    for join in ["Merge", "Concat"] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(CapturingProvider::new(Arc::clone(&seen)));
        let spawner = AgentSpawner::new(provider, test_config());

        let src = format!(
            r#"
Workflow(
    meta: (name: "join-{join}", est_agents: 2),
    phases: [Phase(title: "vote", steps: [
        Parallel(id: "agg", branches: [
            (id: "a", prompt: "branch a"),
            (id: "b", prompt: "branch b"),
        ], join: {join}),
    ])],
)
"#
        );
        let plan = WorkflowPlan::parse(&src).expect("workflow should parse");
        let runner = WorkflowRunner::new(&spawner);
        let result = runner
            .run(&plan, Value::Object(Default::default()))
            .await
            .expect("join workflow should run to completion");

        let agg = result
            .final_state
            .get("agg")
            .unwrap_or_else(|| panic!("aggregator `agg` missing for join {join}"));
        let arr = agg
            .as_array()
            .unwrap_or_else(|| panic!("join {join} must fold to an array in v1, got {agg:?}"));
        assert_eq!(
            arr.len(),
            2,
            "join {join} array must hold both branch outputs"
        );
    }
}
