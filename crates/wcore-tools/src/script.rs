//! F13: typed RPC tool scripting DSL.
//!
//! `Script` is a Tool that runs N built-in tools in sequence. Each step
//! produces a JSON value cached under its `id`; later steps reference
//! prior outputs via `${stepId.json.pointer.path}`. One ToolResult is
//! returned to the host; per-step trace records expand in F9 (W1).
//!
//! Safety rails:
//! - Allow-list of tools (no Spawn, no Script-recursion, no MCP, no plugins).
//! - `${ref}` is json-pointer only: dots between matched braces, no
//!   arithmetic, no shell, no expression language.
//! - `approval_required: true` returns is_error in W4 — the formal Suspend
//!   wire-up lands in W7. The destructive step does NOT execute.
//! - `max_output_lines` truncates the aggregated result to a bounded shape.
//!
//! Gated by `Capabilities.rpc_tool_script` (W0 slot at events.rs:139);
//! the engine only registers ScriptTool when the flag is engine-on.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::dispatcher::ToolDispatcher;

/// Allow-listed tools that may appear as `step.tool`. Order is significant
/// for tests — keep in lockstep with bootstrap.rs's built-in registration.
/// `RepoMap` is included (W3→W4 hand-off, audit HIGH-1) — it is read-only
/// and shape-bounded, semantically equivalent to `Grep`. Recursive `Script`
/// and `SpawnTool` are explicitly excluded.
pub const ALLOW_LIST: &[&str] = &["Read", "Write", "Edit", "Grep", "Glob", "Bash", "RepoMap"];

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptInput {
    pub steps: Vec<ScriptStep>,
    #[serde(default)]
    pub max_output_lines: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptStep {
    pub id: String,
    pub tool: String,
    pub input: Value,
    #[serde(default)]
    pub approval_required: bool,
}

#[derive(Debug, Error)]
pub enum StepError {
    #[error("unknown ref: {0}")]
    UnknownRef(String),

    #[error("ref syntax: {0}")]
    RefSyntax(String),

    #[error("ref path miss: {ref_expr} (no value at that path)")]
    RefPathMiss { ref_expr: String },

    #[error("tool '{0}' is not in the Script allow-list")]
    ToolNotAllowed(String),

    #[error("step id collision: {0}")]
    DuplicateStepId(String),

    #[error("step {step_id} failed: {message}")]
    StepFailed { step_id: String, message: String },

    #[error("approval denied at step {0}")]
    ApprovalDenied(String),
}

// ---------------------------------------------------------------------------
// Ref resolution
// ---------------------------------------------------------------------------

/// Walk `input`, replacing every `"${...}"` string with the resolved value
/// from `prior_outputs`. Non-string values pass through unchanged.
pub fn resolve_input(
    input: &Value,
    prior_outputs: &HashMap<String, Value>,
) -> Result<Value, StepError> {
    match input {
        Value::String(s) => resolve_string(s, prior_outputs),
        Value::Array(arr) => {
            let resolved: Result<Vec<_>, _> = arr
                .iter()
                .map(|v| resolve_input(v, prior_outputs))
                .collect();
            Ok(Value::Array(resolved?))
        }
        Value::Object(obj) => {
            let mut resolved = serde_json::Map::with_capacity(obj.len());
            for (k, v) in obj {
                resolved.insert(k.clone(), resolve_input(v, prior_outputs)?);
            }
            Ok(Value::Object(resolved))
        }
        _ => Ok(input.clone()),
    }
}

fn resolve_string(s: &str, prior_outputs: &HashMap<String, Value>) -> Result<Value, StepError> {
    // Whole-string ${...} → return the typed value (string, number, object, ...).
    if let Some(inner) = whole_ref(s) {
        let (step_id, path) = split_ref(inner)?;
        validate_path_syntax(path)?;
        let step_output = prior_outputs
            .get(step_id)
            .ok_or_else(|| StepError::UnknownRef(inner.to_string()))?;
        return lookup(step_output, path)
            .cloned()
            .ok_or_else(|| StepError::RefPathMiss {
                ref_expr: inner.to_string(),
            });
    }

    // Mixed string with embedded refs → stringify each value, splice into result.
    if !s.contains("${") {
        return Ok(Value::String(s.to_string()));
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('}')
            .ok_or_else(|| StepError::RefSyntax(format!("unterminated ${{ in {s}")))?;
        let inner = &rest[start + 2..start + end];
        let (step_id, path) = split_ref(inner)?;
        validate_path_syntax(path)?;
        let step_output = prior_outputs
            .get(step_id)
            .ok_or_else(|| StepError::UnknownRef(inner.to_string()))?;
        let value = lookup(step_output, path).ok_or_else(|| StepError::RefPathMiss {
            ref_expr: inner.to_string(),
        })?;
        match value {
            Value::String(s) => out.push_str(s),
            other => out.push_str(&serde_json::to_string(other).unwrap_or_default()),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    Ok(Value::String(out))
}

fn whole_ref(s: &str) -> Option<&str> {
    let inner = s.strip_prefix("${").and_then(|x| x.strip_suffix('}'))?;
    // Reject if there's another ${ or } inside.
    if !inner.contains("${") && !inner.contains('}') {
        Some(inner)
    } else {
        None
    }
}

fn split_ref(inner: &str) -> Result<(&str, &str), StepError> {
    let dot = inner
        .find('.')
        .ok_or_else(|| StepError::RefSyntax(format!("ref missing '.': {inner}")))?;
    Ok((&inner[..dot], &inner[dot + 1..]))
}

/// Path must be `name(\.name)*` where name is `[A-Za-z0-9_]+`. Rejects
/// arithmetic, whitespace, brackets, anything that would suggest an
/// expression language.
fn validate_path_syntax(path: &str) -> Result<(), StepError> {
    if path.is_empty() {
        return Err(StepError::RefSyntax("empty path".into()));
    }
    for ch in path.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.') {
            return Err(StepError::RefSyntax(format!(
                "illegal char {ch:?} in path {path}"
            )));
        }
    }
    Ok(())
}

fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cursor = root;
    for part in path.split('.') {
        cursor = match cursor {
            Value::Object(obj) => obj.get(part)?,
            Value::Array(arr) => arr.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cursor)
}

// ---------------------------------------------------------------------------
// W7 S4: approval producer + script output sink — defined locally in
// wcore-tools so we don't need an upward dep edge to wcore-agent. The
// real implementations live in wcore-agent (ApprovalBridge implements
// ApprovalProducer; OutputSink is bridged to ScriptOutputSink via a
// thin adapter at the bootstrap site).
// ---------------------------------------------------------------------------

/// W7 S4: producer side of the approval bridge. Implemented by
/// `wcore_agent::approval::ApprovalBridge`.
#[async_trait]
pub trait ApprovalProducer: Send + Sync {
    /// Request approval. Returns the resume token + a oneshot receiver
    /// that resolves when the host's `ApprovalResume` command arrives.
    async fn request(
        &self,
        call_id: String,
        reason: String,
        context: String,
    ) -> (String, tokio::sync::oneshot::Receiver<ApprovalOutcomeLite>);
}

/// W7 S4: minimal outcome value-type used by `ApprovalProducer`.
/// Mirrors `wcore_agent::approval::ApprovalOutcome` but lives here so
/// `wcore-tools` stays independent of `wcore-agent`.
#[derive(Debug, Clone)]
pub struct ApprovalOutcomeLite {
    pub approved: bool,
    pub modifications: Option<serde_json::Value>,
}

/// W7 S4: emit-side trait for the three S4 protocol events.
/// `wcore-agent` provides a thin adapter that bridges this to its
/// full `OutputSink`.
pub trait ScriptOutputSink: Send + Sync {
    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    );
    fn emit_suspend(&self, reason: &str, resume_token: &str);
}

// ---------------------------------------------------------------------------
// ScriptTool
// ---------------------------------------------------------------------------

pub struct ScriptTool {
    dispatcher: Arc<dyn ToolDispatcher>,
    /// W7 S4: optional approval-bridge plumbing. None = today's W4
    /// short-circuit (returns error_result for approval_required steps).
    /// Some = full request/await/dispatch flow.
    approval_bridge: Option<Arc<dyn ApprovalProducer>>,
    /// W7 S4: optional sink for ApprovalRequired/Suspend emissions.
    /// Without it, the bridge round-trip still works but no host UI is
    /// notified — useful for unit tests that don't care about events.
    script_output: Option<Arc<dyn ScriptOutputSink>>,
}

impl ScriptTool {
    /// Unchanged W4 constructor — keeps all 10 existing call-sites
    /// compiling without any edit. Returns a ScriptTool that short-
    /// circuits on `approval_required: true` steps with the W4 error.
    pub fn new(dispatcher: Arc<dyn ToolDispatcher>) -> Self {
        Self {
            dispatcher,
            approval_bridge: None,
            script_output: None,
        }
    }

    /// W7 S4 builder: wire the ApprovalBridge + script output sink so
    /// `approval_required: true` steps actually request approval and
    /// emit the `ApprovalRequired` + `Suspend` events.
    pub fn with_approval(
        mut self,
        bridge: Arc<dyn ApprovalProducer>,
        sink: Arc<dyn ScriptOutputSink>,
    ) -> Self {
        self.approval_bridge = Some(bridge);
        self.script_output = Some(sink);
        self
    }
}

#[async_trait]
impl Tool for ScriptTool {
    fn name(&self) -> &str {
        "Script"
    }

    fn description(&self) -> &str {
        "Run a typed sequence of built-in tools in one call. Each step has an \
         id, a tool name (allow-list: Read, Write, Edit, Grep, Glob, Bash, \
         RepoMap), and an input object. Later steps reference earlier outputs \
         via '${stepId.path.to.field}'. The host sees one ToolResult; the full \
         per-step trace lands in observability."
    }

    fn input_schema(&self) -> JsonSchema {
        serde_json::json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id":    {"type": "string"},
                            "tool":  {"type": "string"},
                            "input": {"type": "object"},
                            "approval_required": {"type": "boolean"}
                        },
                        "required": ["id", "tool", "input"]
                    }
                },
                "max_output_lines": {"type": "integer", "minimum": 1}
            },
            "required": ["steps"]
        })
    }

    fn is_concurrency_safe(&self, _: &Value) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // Legacy entry point — no parent context. Delegate to the
        // ctx-aware path with a synthesised default so sub-steps still
        // see *a* context (vfs = RealFs, cancel = open, notifier = None).
        let ctx = crate::context::ToolContext::test_default();
        self.execute_with_ctx(input, &ctx).await
    }

    /// W8b.2.A — ctx-aware Script entry. Sub-step dispatch routes
    /// through `dispatcher.dispatch_with_ctx(...)` so each child tool
    /// inherits the parent's `vfs`, `cancel`, and `file_write_notifier`.
    async fn execute_with_ctx(
        &self,
        input: Value,
        ctx: &crate::context::ToolContext,
    ) -> ToolResult {
        let parsed: ScriptInput = match serde_json::from_value(input) {
            Ok(v) => v,
            Err(e) => return error_result(format!("invalid Script input: {e}")),
        };

        let max_lines = parsed.max_output_lines.unwrap_or(200);
        let mut outputs: HashMap<String, Value> = HashMap::new();
        let mut transcript: Vec<String> = Vec::new();

        // Duplicate-id pre-check.
        let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for step in &parsed.steps {
            if !seen_ids.insert(step.id.as_str()) {
                return error_result(format!("DuplicateStepId: {}", step.id));
            }
        }

        for step in &parsed.steps {
            // Allow-list enforcement (rejects Script, SpawnTool, MCP).
            if !ALLOW_LIST.contains(&step.tool.as_str()) {
                return error_result(format!(
                    "tool '{}' is not in the Script allow-list",
                    step.tool
                ));
            }

            // Resolve ${ref} substitutions against prior outputs.
            let resolved_input = match resolve_input(&step.input, &outputs) {
                Ok(v) => v,
                Err(e) => return error_result(format!("{}: {}", step.id, e)),
            };

            // W7 S4: approval_required wire-up. When the tool was built
            // via .with_approval(bridge, sink), the step requests approval
            // via the bridge, emits ApprovalRequired + Suspend through the
            // sink, awaits the outcome, and dispatches if approved.
            //
            // Without the builder, the W4 short-circuit fires (preserves
            // legacy test behaviour for `ScriptTool::new(disp)` callers).
            if step.approval_required {
                let Some(bridge) = self.approval_bridge.as_ref() else {
                    return error_result(format!(
                        "approval_required step '{}' but no approval bridge configured \
                         (ScriptTool was built via ::new without .with_approval)",
                        step.id
                    ));
                };
                let call_id = format!("script:{}", step.id);
                let reason = format!("Script step '{}' is approval-gated", step.id);
                let context = serde_json::to_string(&step).unwrap_or_default();
                let (token, rx) = bridge
                    .request(call_id.clone(), reason.clone(), context.clone())
                    .await;
                if let Some(out) = self.script_output.as_ref() {
                    out.emit_approval_required(&call_id, &token, &reason, &context);
                    out.emit_suspend("awaiting_approval", &token);
                }
                match rx.await {
                    Ok(outcome) if outcome.approved => {
                        // fall through to dispatch
                    }
                    Ok(_) => {
                        return error_result(format!("step '{}' rejected by user", step.id));
                    }
                    Err(_) => {
                        return error_result(format!("step '{}' approval channel closed", step.id));
                    }
                }
            }

            // W8b.2.A — propagate the parent ctx into the child tool.
            // ToolDispatcher's default `dispatch_with_ctx` falls back to
            // `dispatch` for any impl that hasn't opted in, so existing
            // hosts stay byte-identical until they migrate.
            let result = self
                .dispatcher
                .dispatch_with_ctx(&step.tool, resolved_input, ctx)
                .await;
            if result.is_error {
                return error_result(format!(
                    "step '{}' (tool {}) failed: {}",
                    step.id, step.tool, result.content
                ));
            }

            // Parse output as JSON if possible; else stash as string.
            let value: Value = serde_json::from_str(&result.content)
                .unwrap_or_else(|_| Value::String(result.content.clone()));
            transcript.push(format!("[{}] {}: {}", step.id, step.tool, result.content));
            outputs.insert(step.id.clone(), value);
        }

        let joined = transcript.join("\n");
        let truncated = truncate_lines(&joined, max_lines);
        ToolResult {
            content: truncated,
            is_error: false,
        }
    }
}

fn error_result(msg: String) -> ToolResult {
    ToolResult {
        content: msg,
        is_error: true,
    }
}

fn truncate_lines(s: &str, max: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max {
        return s.to_string();
    }
    let head = &lines[..max];
    format!(
        "{}\n... (truncated {} lines)",
        head.join("\n"),
        lines.len() - max
    )
}
