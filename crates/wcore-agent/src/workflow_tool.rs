//! B1 — `WorkflowTool`: the LLM-facing surface for the dynamic-workflow
//! engine.
//!
//! The tool accepts an inline RON `workflow` string, parses it via
//! [`WorkflowPlan::parse`] (A1's lowering), executes it through
//! [`WorkflowRunner::run`] (A3's FleetDispatcher-backed executor) and returns
//! the final state plus a per-stage summary as the tool result.
//!
//! Like [`crate::spawn_tool::SpawnTool`], it holds an [`AgentSpawner`] so each
//! invocation drives the same proven sub-agent dispatch path. The runner
//! borrows the spawner, so `WorkflowTool` keeps an `Arc<AgentSpawner>` and
//! constructs a fresh [`WorkflowRunner`] per `execute`.
//!
//! Scope (B1): inline RON only. Saved-by-name resolution (a `name` parameter
//! resolving against `.wayland/workflows/*.ron`) is B2 and is intentionally
//! absent here.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::orchestration::workflow::runner::{
    WorkflowPlan, WorkflowRunError, WorkflowRunResult, WorkflowRunner,
};
use crate::spawner::AgentSpawner;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_types::tool::{JsonSchema, ToolResult};

/// LLM-facing tool that parses and executes an inline RON workflow through the
/// dynamic-workflow engine.
pub struct WorkflowTool {
    spawner: Arc<AgentSpawner>,
}

impl WorkflowTool {
    pub fn new(spawner: Arc<AgentSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        "Workflow"
    }

    fn description(&self) -> &str {
        "Run a multi-stage agent ForgeFlow described inline in RON. \
         The ForgeFlow lowers to an execution graph and runs each stage through \
         the same sub-agent dispatch path as Spawn, threading each stage's \
         output into the next.\n\n\
         - Provide the ForgeFlow as a RON string in the `workflow` parameter \
         (its root document is `Workflow(...)`).\n\
         - Optionally provide an `inputs` object as the workflow's initial \
         state; its keys become readable state refs, e.g. a \
         `Pipeline(over: \"changed_files\")` stage consumes \
         `inputs.changed_files`. Omit it to start from empty state.\n\
         - Stages run in dependency order; sibling stages (a fan-out) run \
         concurrently and aggregate.\n\
         - Returns the final workflow state plus a per-stage summary.\n\
         - Invalid RON returns a typed parse error (the workflow does not run)."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "workflow": {
                    "type": "string",
                    "description": "The workflow definition as an inline RON string \
                                    (Workflow(meta: (...), phases: [Phase(...)]))."
                },
                "inputs": {
                    "type": "object",
                    "description": "Optional initial workflow state. Each key is a \
                                    state ref the workflow can read — e.g. an \
                                    `over:`-pipeline streams over an array supplied \
                                    here. Absent → the workflow starts from empty \
                                    state."
                }
            },
            "required": ["workflow"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Manages its own multi-agent concurrency, like Spawn.
        false
    }

    fn is_deferred(&self) -> bool {
        false
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let src = match input.get("workflow").and_then(Value::as_str) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Missing or invalid 'workflow' string parameter".to_string(),
                    is_error: true,
                };
            }
        };

        // Parse + lower the RON. A malformed workflow returns the typed parse
        // error to the model rather than panicking.
        let plan = match WorkflowPlan::parse(src) {
            Ok(plan) => plan,
            Err(e) => {
                return ToolResult {
                    content: format!("Workflow parse error: {e}"),
                    is_error: true,
                };
            }
        };

        // Optional caller-supplied initial state. An `inputs` object becomes
        // the runner's starting state so an `over:`-pipeline can stream over a
        // caller-provided array; a missing (or non-object) `inputs` falls back
        // to the empty-state behaviour. The runner normalises a non-object
        // initial to an empty object, so passing the raw value through is safe.
        let initial = match input.get("inputs") {
            Some(v @ Value::Object(_)) => v.clone(),
            _ => Value::Object(serde_json::Map::new()),
        };

        // Execute through the runner. The runner borrows the spawner, so build
        // a fresh runner per call over the shared `Arc<AgentSpawner>`.
        let runner = WorkflowRunner::new(&self.spawner);
        match runner.run(&plan, initial).await {
            Ok(result) => ToolResult {
                content: render_run_result(&result),
                is_error: false,
            },
            Err(e) => ToolResult {
                content: render_run_error(&e),
                is_error: true,
            },
        }
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn describe(&self, _input: &Value) -> String {
        "Workflow: run an inline RON workflow".to_string()
    }
}

/// Render a successful run as a per-stage summary followed by the final state.
fn render_run_result(result: &WorkflowRunResult) -> String {
    let mut out = String::from("# Workflow complete\n\n## Stages\n");
    for stage in &result.stage_results {
        let status = if stage.is_error { "ERROR" } else { "OK" };
        out.push_str(&format!(
            "- {} [{}] (turns: {})\n",
            stage.node_id, status, stage.turns
        ));
    }
    out.push_str("\n## Final state\n");
    let state = serde_json::to_string_pretty(&result.final_state)
        .unwrap_or_else(|_| result.final_state.to_string());
    out.push_str(&state);
    out
}

/// Render a run error, surfacing any partial result the runner preserved so the
/// model still sees the stages that completed before the failure.
fn render_run_error(err: &WorkflowRunError) -> String {
    match err {
        WorkflowRunError::StageFailed { partial, .. }
        | WorkflowRunError::SchemaValidationFailed { partial, .. } => {
            format!(
                "Workflow failed: {err}\n\n## Partial result\n{}",
                render_run_result(partial)
            )
        }
        other => format!("Workflow failed: {other}"),
    }
}
