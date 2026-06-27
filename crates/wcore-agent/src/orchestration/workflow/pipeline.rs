//! A5 — no-barrier `pipeline(over, stages)` scheduler.
//!
//! ## What "no barrier" means
//!
//! A `parallel` step is a **barrier**: every branch must finish before the
//! step completes. A `pipeline` step is the opposite — given a collection
//! `over: "$changed_files"` and an ordered list of stages, **each item flows
//! through all stages independently**. Item A can be in stage 3 while item B
//! is still in stage 1; the only synchronisation point is the very end, when
//! every item's chain has terminated (whether by completing all stages or by
//! dropping out on a stage error).
//!
//! This is the SPEC §3 item 2 mechanic and the one genuinely novel piece of
//! execution in the workflow layer. It is implemented **here, in the runner**
//! — the per-turn `ExecutionGraph::execute` walker (which is barrier-only,
//! `join_all` per tick) is left untouched.
//!
//! ## How items stream without a barrier
//!
//! Each input item gets exactly one item future. That future runs the item
//! through every stage **sequentially within the item** (stage N reads stage
//! N-1's output for that item), but the per-item futures run **concurrently
//! with each other**. There is no cross-item join between stages, so a fast
//! item races ahead through all stages while a slow item is still on stage 1.
//!
//! ## Bounded concurrency (no starvation, no OOM)
//!
//! The item futures are driven through a [`futures::stream::StreamExt::buffer_unordered`]
//! pump capped at [`DEFAULT_PIPELINE_CONCURRENCY`], so a huge `over:` collection
//! never allocates/polls every item future at once (the resource-bound guard —
//! the semaphore caps LLM calls but NOT future allocation). Within that, every
//! stage dispatch acquires a permit from a **shared** [`Semaphore`] before
//! spawning the sub-agent, and releases it the moment that stage's agent
//! returns. The cap bounds *total in-flight stage agents across all items*, so a
//! wide pipeline cannot starve the relay/heartbeat tasks the rest of the runtime
//! depends on (gemini's starvation flag in the PLAN).
//! The permit is held only for the duration of one stage's LLM call, then
//! released so another item's stage can proceed — this is what lets items
//! interleave rather than running in lockstep waves.
//!
//! ## Failure semantics
//!
//! A stage that errors (LLM-layer error, or — when the stage carries a
//! schema — a validation failure that survives the retry budget) **drops
//! that item to `null` and skips its remaining stages**. The pipeline as a
//! whole does **not** abort: other items keep flowing. The result vector
//! preserves input order, with a `null` hole wherever an item dropped out.

use std::sync::Arc;

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::Semaphore;

use super::dsl::AgentSpec;
use super::runner::{
    PipelineStageDispatch, StageResult, StageSchemaErr, build_prompt, resolve_stage_schema,
};
use crate::spawner::{AgentSpawner, SubAgentConfig};

/// Default cap on total in-flight pipeline stage agents across all items.
/// Mirrors the `Swarm` topology cap (SPEC §2) — wide enough to stream, small
/// enough not to starve the relay/heartbeat tasks. Callers needing a tighter
/// or looser bound pass an explicit [`Semaphore`] to [`run_pipeline`].
pub(crate) const DEFAULT_PIPELINE_CONCURRENCY: usize = 20;

/// Outcome of executing one no-barrier pipeline step.
pub(crate) struct PipelineOutcome {
    /// One entry per input item, **in input order**. `Value::Null` marks an
    /// item that dropped out on a stage error (or a schema-validation failure
    /// past the retry budget). A completed item carries its final stage's
    /// structured output (the validated `Value` for a schema stage, else the
    /// stage text as a JSON string).
    pub(crate) items: Vec<Value>,
    /// Every executed stage's record, across all items, in completion order
    /// (so callers can surface progress / AgentNav rows). Stage ids are
    /// namespaced `"{pipeline_id}[{item_index}]:{stage_id}"` so concurrent
    /// items never collide.
    pub(crate) stage_results: Vec<StageResult>,
    /// FIX 1 — `Some(attempted)` if any item's dispatch tripped the per-run
    /// dispatch budget (the count the charge would have reached). The runner
    /// aborts the whole run when this is set; `None` means the pipeline drained
    /// within budget.
    pub(crate) budget_breached: Option<usize>,
}

/// Run a no-barrier `pipeline` step.
///
/// - `pipeline_id` — the step id; used to namespace stage records.
/// - `items` — the resolved `over:` collection. Each element seeds one
///   independent item-chain. A non-array `over` resolution yields an empty
///   pipeline (no items) — the caller validates the ref shape upstream.
/// - `stages` — the ordered stage specs (prompt + optional schema + optional
///   per-stage `input` ref are honoured; `input` selects a field of the
///   *current item value*, else the whole item value feeds the stage).
/// - `dispatch` — the runner-side seam that turns a stage spec + input into a
///   dispatched [`SubAgentConfig`] and resolves its schema. Reused so the
///   pipeline honours A4 schema validation/retry without re-implementing it.
/// - `sem` — the shared concurrency permit source. Pass the runner's global
///   semaphore so pipeline stages share the cap with the rest of the run.
pub(crate) async fn run_pipeline(
    spawner: &AgentSpawner,
    dispatch: &PipelineStageDispatch<'_>,
    pipeline_id: &str,
    items: &[Value],
    stages: &[AgentSpec],
    sem: Arc<Semaphore>,
) -> PipelineOutcome {
    // One independent future per item. Each future walks all stages sequentially
    // *for that item*; the futures run concurrently with one another, so there
    // is no barrier between stages across items.
    //
    // FIX 3 — bounded polling. A huge `items` must NOT allocate and poll every
    // item future at once (the semaphore caps in-flight LLM calls but NOT future
    // allocation), so we drive the item futures through a `buffer_unordered`
    // stream that holds at most [`DEFAULT_PIPELINE_CONCURRENCY`] futures live at
    // a time. Results stream back in completion order; we scatter each into an
    // order-preserving vector by its item index, so input order (with `null`
    // holes) is preserved exactly as `join_all` did.
    // Own each seed value up front so no borrow of `items` lives across an
    // `.await` (which would over-constrain the item futures' lifetimes).
    let seeds: Vec<(usize, Value)> = items.iter().cloned().enumerate().collect();
    let item_futs = seeds.into_iter().map(|(idx, item)| {
        let pipeline_id = pipeline_id.to_string();
        let stages = stages.to_vec();
        let sem = Arc::clone(&sem);
        run_item(spawner, dispatch, pipeline_id, idx, item, stages, sem)
    });

    let mut items_out = vec![Value::Null; items.len()];
    let mut stage_results = Vec::new();
    let mut budget_breached: Option<usize> = None;

    let mut stream =
        futures::stream::iter(item_futs).buffer_unordered(DEFAULT_PIPELINE_CONCURRENCY);
    while let Some(outcome) = stream.next().await {
        // Collect by item index, never by completion order, so the result
        // vector preserves input order with `null` holes for dropped items.
        items_out[outcome.index] = outcome.value;
        stage_results.extend(outcome.stages);
        if let Some(attempted) = outcome.budget_breached {
            // Record the smallest breach count seen (any breach aborts the run;
            // the count is diagnostic). The stream keeps draining the already
            // in-flight futures, which is bounded by the buffer width.
            budget_breached = Some(match budget_breached {
                Some(prev) => prev.min(attempted),
                None => attempted,
            });
        }
    }

    PipelineOutcome {
        items: items_out,
        stage_results,
        budget_breached,
    }
}

/// Per-item result threaded back from [`run_item`].
struct ItemOutcome {
    index: usize,
    /// Final stage value, or `Value::Null` if the item dropped out.
    value: Value,
    stages: Vec<StageResult>,
    /// FIX 1 — `Some(attempted)` if a stage dispatch for this item tripped the
    /// per-run dispatch budget. The runner aborts the whole run when set.
    budget_breached: Option<usize>,
}

/// Run a single item through every stage in order. Returns the final stage's
/// value (or `Null` on a drop) plus this item's stage records.
async fn run_item(
    spawner: &AgentSpawner,
    dispatch: &PipelineStageDispatch<'_>,
    pipeline_id: String,
    index: usize,
    seed: Value,
    stages: Vec<AgentSpec>,
    sem: Arc<Semaphore>,
) -> ItemOutcome {
    let mut current = seed;
    let mut stage_records = Vec::with_capacity(stages.len());

    for stage in &stages {
        // Resolve the stage's input from the current item value: a per-stage
        // `input` selects a field of it, else the whole current value flows in.
        let input = match &stage.input {
            Some(path) => {
                super::super::graph::InputMapper::Select { path: path.clone() }.apply(&current)
            }
            None => current.clone(),
        };
        let prompt = build_prompt(&stage.prompt, &input);

        // FIX 1 — charge the per-item-per-stage dispatch against the run budget
        // BEFORE acquiring a permit/dispatching. A breach aborts the whole run
        // (propagated up via `budget_breached`), not just this item.
        if let Err(attempted) = dispatch.budget.try_charge() {
            return ItemOutcome {
                index,
                value: Value::Null,
                stages: stage_records,
                budget_breached: Some(attempted),
            };
        }

        // Acquire a permit so total in-flight stage agents stay under the cap.
        // Held only across this one stage's dispatch, then dropped so another
        // item's stage can take it — this is what lets items interleave.
        // `acquire_owned()` only errors if the semaphore is closed; the runner
        // never closes the pipeline semaphore, so treat closure as "drop this
        // item" rather than silently dispatching capless.
        let Ok(permit) = sem.clone().acquire_owned().await else {
            return ItemOutcome {
                index,
                value: Value::Null,
                stages: stage_records,
                budget_breached: None,
            };
        };
        let result = spawner
            .spawn_one(SubAgentConfig {
                name: format!("{pipeline_id}[{index}]:{}", stage.id),
                prompt,
                max_turns: dispatch.max_turns,
                max_tokens: dispatch.max_tokens,
                system_prompt: None,
                provider: None,
                model: None,
                temperature: None,
            })
            .await;
        drop(permit);

        if result.is_error {
            // Drop this item to null; skip its remaining stages. The run as a
            // whole continues — other items are unaffected.
            stage_records.push(StageResult {
                node_id: format!("{pipeline_id}[{index}]:{}", stage.id),
                text: result.text,
                is_error: true,
                turns: result.turns,
            });
            return ItemOutcome {
                index,
                value: Value::Null,
                stages: stage_records,
                budget_breached: None,
            };
        }

        // Honour A4 schema validation/retry when the stage carries a schema.
        // A validation failure past the retry budget drops the item, exactly
        // like an LLM-layer error.
        match resolve_stage_schema(
            spawner,
            dispatch,
            &pipeline_id,
            index,
            stage,
            result,
            &input,
            &sem,
        )
        .await
        {
            Ok(resolved) => {
                stage_records.push(StageResult {
                    node_id: format!("{pipeline_id}[{index}]:{}", stage.id),
                    text: resolved.text.clone(),
                    is_error: false,
                    turns: resolved.turns,
                });
                current = resolved.value;
            }
            Err(StageSchemaErr::Dropped { message, turns }) => {
                stage_records.push(StageResult {
                    node_id: format!("{pipeline_id}[{index}]:{}", stage.id),
                    text: message,
                    is_error: true,
                    turns,
                });
                return ItemOutcome {
                    index,
                    value: Value::Null,
                    stages: stage_records,
                    budget_breached: None,
                };
            }
            Err(StageSchemaErr::BudgetExceeded { attempted, turns }) => {
                // FIX 1 — a schema retry tripped the run budget. Record the
                // partial stage, then signal a hard run-abort up to the runner.
                stage_records.push(StageResult {
                    node_id: format!("{pipeline_id}[{index}]:{}", stage.id),
                    text: "dispatch budget exceeded".to_string(),
                    is_error: true,
                    turns,
                });
                return ItemOutcome {
                    index,
                    value: Value::Null,
                    stages: stage_records,
                    budget_breached: Some(attempted),
                };
            }
        }
    }

    ItemOutcome {
        index,
        value: current,
        stages: stage_records,
        budget_breached: None,
    }
}
