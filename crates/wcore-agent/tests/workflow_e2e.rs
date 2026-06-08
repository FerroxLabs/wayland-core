//! C2 — end-to-end integration proof for the Dynamic Workflows engine.
//!
//! This is the feature's headline test: a realistic `review-changes`-style
//! workflow authored in RON that drives the FULL stack in a single run —
//!
//! ```text
//!   Pipeline(over: changed_files)   no-barrier per-file streaming (A5)
//!         │  scan each changed file through two stages
//!         ▼
//!   Parallel(join: Collect)         fan-out verify (A3 fan-out + aggregator)
//!     ├── lint    (schema-validated `findings`, with a retry path — A4)
//!     └── audit   (plain text branch)
//!         ▼
//!   Agent(summarize, input: verdict)  reads the collected array downstream
//! ```
//!
//! It is parsed via [`WorkflowPlan::parse`] (A1 lowering → `GraphConfig`) and
//! executed via [`WorkflowRunner::run`] (A3 spawner-path executor) against a
//! single capturing mock provider whose response depends on which stage is
//! asking. The assertions prove, against ONE run:
//!
//! 1. **Every stage executes** — the no-barrier pipeline runs per file, both
//!    parallel branches run, the aggregator runs, and the downstream summary
//!    runs.
//! 2. **Data threads across stages** — each file flows through both pipeline
//!    stages; the collected `verdict` array reaches the `summarize` prompt.
//! 3. **Schema validation + retry** — the `lint` branch returns malformed JSON
//!    on its first dispatch and corrected JSON on the retry; the runner stores
//!    the parsed (structured) object, and the retry prompt carries the
//!    validation correction.
//! 4. **The parallel join aggregates** — `Collect` folds both branch outputs
//!    into an array on the aggregator's state key.
//! 5. **Final state is correct** — every node id has its expected entry.
//!
//! Modeled on `tests/workflow_runner_test.rs` + `tests/pipeline_test.rs`.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::test_config;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wcore_agent::orchestration::workflow::runner::{WorkflowPlan, WorkflowRunner};
use wcore_agent::spawner::AgentSpawner;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

/// The headline workflow: a no-barrier pipeline over `changed_files` feeding a
/// schema-validated parallel verify stage, whose collected verdict a final
/// `summarize` agent reads.
///
/// - `scan` streams each changed file through `extract` → `classify` (two
///   stages, no barrier — A5).
/// - `verdict` fans out two verify branches that each read the scan output:
///   `lint` is schema-bound (`findings`) and `audit` is free text. `Collect`
///   folds them into an array.
/// - `summarize` reads the collected `verdict` array.
const REVIEW_CHANGES: &str = r#"
Workflow(
    meta: (name: "review-changes", description: "review a diff end to end", est_agents: 7),
    schemas: {
        "findings": "{ \"type\": \"object\", \"required\": [\"findings\"], \"properties\": { \"findings\": { \"type\": \"array\", \"items\": { \"type\": \"string\" } } } }",
    },
    phases: [
        Phase(
            title: "scan",
            steps: [
                Pipeline(id: "scan", over: Some("changed_files"), stages: [
                    (id: "extract", prompt: "extract symbols from the file"),
                    (id: "classify", prompt: "classify the extracted symbols"),
                ]),
            ],
        ),
        Phase(
            title: "verify",
            steps: [
                Parallel(id: "verdict", branches: [
                    (id: "lint", prompt: "lint the scanned files and return findings JSON", schema: Some("findings"), input: Some("scan")),
                    (id: "audit", prompt: "audit the scanned files for risk", input: Some("scan")),
                ], join: Collect),
                Agent((id: "summarize", prompt: "summarize the verdict", input: Some("verdict"))),
            ],
        ),
    ],
)
"#;

/// A capturing provider that answers based on which stage is asking (detected
/// from marker substrings in the serialized request prompt). It records every
/// request so the test can assert data threaded across stages, and counts
/// `lint` dispatches so the schema retry is observable.
struct ReviewProvider {
    seen: Arc<Mutex<Vec<String>>>,
    /// How many times the schema-bound `lint` branch has been dispatched.
    lint_calls: Arc<Mutex<usize>>,
}

impl ReviewProvider {
    fn new(seen: Arc<Mutex<Vec<String>>>, lint_calls: Arc<Mutex<usize>>) -> Self {
        Self { seen, lint_calls }
    }

    /// Choose the response text for a request based on its prompt markers.
    fn response_for(&self, dump: &str) -> String {
        if dump.contains("extract symbols") {
            // Pipeline stage 1: re-embed the file tag so stage 2 (and the
            // downstream verify stage) can see which file flowed through.
            let file = file_tag(dump).unwrap_or_else(|| "unknown".to_string());
            format!("extracted::{file}")
        } else if dump.contains("classify the extracted symbols") {
            // Pipeline stage 2: must have received stage 1's `extracted::` output.
            let file = dump
                .split("extracted::")
                .nth(1)
                .map(|rest| {
                    let end = rest
                        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
                        .unwrap_or(rest.len());
                    rest[..end].to_string()
                })
                .unwrap_or_else(|| "unknown".to_string());
            format!("classified::{file}")
        } else if dump.contains("lint the scanned files") {
            // Schema-bound verify branch. First dispatch → malformed (a bare
            // array, but the schema wants an object); the retry → valid object.
            let mut calls = self.lint_calls.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                r#"["this is not the required object shape"]"#.to_string()
            } else {
                r#"{ "findings": ["unchecked unwrap", "missing error path"] }"#.to_string()
            }
        } else if dump.contains("audit the scanned files") {
            // Free-text verify branch.
            "audit: 2 medium-risk items".to_string()
        } else if dump.contains("summarize the verdict") {
            "summary: 1 lint finding set + 1 audit note".to_string()
        } else {
            "default".to_string()
        }
    }
}

/// Extract a `TAG=<word>` marker from a serialized request (the per-file seed).
fn file_tag(dump: &str) -> Option<String> {
    let idx = dump.find("TAG=")?;
    let rest = &dump[idx + 4..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
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
                input_tokens: 20,
                output_tokens: 10,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

#[async_trait]
impl LlmProvider for ReviewProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let dump = format!("{request:?}");
        let text = self.response_for(&dump);
        self.seen.lock().unwrap().push(dump);
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in ok_events(text) {
                let _ = tx.send(ev).await;
            }
        });
        Ok(rx)
    }
}

/// The full-stack proof: one `review-changes` run exercises the no-barrier
/// pipeline, the schema-validated + retried parallel verify fan-out, the
/// Collect aggregation, and the downstream read — all threading data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_changes_workflow_runs_full_stack_end_to_end() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let lint_calls = Arc::new(Mutex::new(0usize));
    let provider = Arc::new(ReviewProvider::new(
        Arc::clone(&seen),
        Arc::clone(&lint_calls),
    ));
    let spawner = AgentSpawner::new(provider, test_config());

    let plan = WorkflowPlan::parse(REVIEW_CHANGES).expect("review-changes workflow should parse");
    let runner = WorkflowRunner::new(&spawner);

    // The host injects the changed-file list as the initial state (two files,
    // each tagged so the per-file path is observable across stages).
    let initial = json!({ "changed_files": ["TAG=alpha.rs", "TAG=beta.rs"] });
    let result = runner
        .run(&plan, initial)
        .await
        .expect("review-changes workflow should run to completion");

    let state = &result.final_state;

    // --- 1. The no-barrier pipeline ran per file and threaded each file
    //        through BOTH stages (extract -> classify). ----------------------
    let scan = state
        .get("scan")
        .and_then(Value::as_array)
        .expect("`scan` pipeline output is an array");
    assert_eq!(scan.len(), 2, "one pipeline result per changed file");
    let scanned: Vec<&str> = scan.iter().filter_map(Value::as_str).collect();
    assert!(
        scanned.iter().all(|s| s.starts_with("classified::")),
        "every file flowed through stage 1 (extract) into stage 2 (classify); got {scanned:?}"
    );
    // The original file tag survived both stages (extract -> classify chaining).
    assert!(
        scanned.iter().any(|s| s.contains("alpha.rs"))
            && scanned.iter().any(|s| s.contains("beta.rs")),
        "both files' tags survived the two-stage pipeline; got {scanned:?}"
    );

    // --- 2. Schema validation + retry on the `lint` branch. -----------------
    // The lint branch was dispatched twice: malformed first, corrected on retry.
    assert_eq!(
        *lint_calls.lock().unwrap(),
        2,
        "lint should dispatch once + one schema-correction retry"
    );
    // The runner stored the PARSED structured object (not the raw JSON string).
    let lint = state.get("lint").expect("lint output stored in state");
    assert!(
        lint.is_object(),
        "validated lint output must be stored as structured JSON, got {lint:?}"
    );
    let findings: Vec<&str> = lint
        .get("findings")
        .and_then(Value::as_array)
        .expect("findings array present after schema validation")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(findings, vec!["unchecked unwrap", "missing error path"]);
    // The retry prompt fed the validation error back to the agent.
    let lint_retry_carried_correction = seen.lock().unwrap().iter().any(|r| {
        r.contains("lint the scanned files") && r.contains("did not match the required schema")
    });
    assert!(
        lint_retry_carried_correction,
        "the lint retry prompt must carry the schema correction"
    );

    // --- 3. Both parallel branches ran and the join aggregated. -------------
    assert!(
        state.get("audit").and_then(Value::as_str).is_some(),
        "the free-text `audit` branch produced output"
    );
    let verdict = state
        .get("verdict")
        .and_then(Value::as_array)
        .expect("the Collect aggregator `verdict` holds an array");
    assert_eq!(
        verdict.len(),
        2,
        "Collect join folds both verify branches into the verdict array"
    );

    // --- 4. The downstream `summarize` agent read the collected verdict. ----
    let summarize = state
        .get("summarize")
        .and_then(Value::as_str)
        .expect("`summarize` produced output");
    assert!(
        summarize.contains("summary"),
        "summarize ran and produced its summary; got {summarize:?}"
    );
    // The summarize prompt carried the collected verdict array downstream
    // (proving the aggregator output is reachable via the aggregator node id).
    let summarize_req = seen
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.contains("summarize the verdict"))
        .cloned()
        .expect("summarize stage dispatched an LLM call");
    assert!(
        summarize_req.contains("audit: 2 medium-risk items"),
        "summarize prompt must carry the collected verify branch outputs; got {summarize_req}"
    );

    // --- 5. Every workflow node executed (final-state completeness). --------
    for id in ["scan", "lint", "audit", "verdict", "summarize"] {
        assert!(
            state.get(id).is_some(),
            "final state missing `{id}` — a stage did not execute"
        );
    }
    // No stage was recorded as errored once the lint retry recovered.
    assert!(
        result.stage_results.iter().all(|s| !s.is_error),
        "no stage should remain errored after the lint retry succeeded"
    );
}
