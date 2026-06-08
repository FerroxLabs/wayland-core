//! v0.9.3 W0.4 — `AskUserQuestion` engine tool.
//!
//! The agent invokes this with a question + a list of options. The user's
//! choice is routed back through `ProtocolCommand::ToolApprove.answer`
//! (W0.1) and synthesized as the tool result content by orchestration
//! (`wcore-agent::orchestration::mod.rs:911`, W0.3) — `execute()` is never
//! reached on the happy path.
//!
//! Approval gating: the tool reports `ToolCategory::Info` and is NOT added
//! to any default allow_list, so the existing orchestration gate at
//! `orchestration/mod.rs:877-879` always requires host approval. The host
//! then drives the question UI and, when the user picks an option, sends
//! `ToolApprove { answer: Some(label), ... }` — orchestration short-circuits
//! and feeds `label` to the LLM as the tool's output.
//!
//! Why `execute()` is loud-defensive: if the host or orchestration is
//! mis-wired (no `answer` payload), the fallback content is a clear error
//! string. Silent fall-through to "no result" would be much harder to debug.

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// `AskUserQuestion` — structured multi-choice question with an
/// approval-channel answer return path.
///
/// Zero-state tool; construction is trivial via `Default`.
#[derive(Default)]
pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn description(&self) -> &str {
        "Ask the user a structured question with multiple-choice options. \
The selected option's label becomes the tool result. Use when you need \
a discrete decision before proceeding (mode choice, file selection, \
yes/no with context). Prefer this over freeform clarification when the \
answer space is enumerable."
    }

    fn input_schema(&self) -> JsonSchema {
        // JsonSchema = serde_json::Value (alias at wcore-types/src/tool.rs:4),
        // built via the json!({}) macro per the clarify.rs precedent.
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to present to the user."
                },
                "header": {
                    "type": "string",
                    "description": "Short header / context shown above the question."
                },
                "multiSelect": {
                    "type": "boolean",
                    "default": false,
                    "description": "Allow multiple option selection. Default false."
                },
                "options": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string" },
                            "description": { "type": "string" },
                            "preview": { "type": "string" }
                        },
                        "required": ["label", "description"]
                    },
                    "description": "Selectable options (mirrors Claude Code's schema; no hard cap enforced engine-side)."
                }
            },
            "required": ["question", "options"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // The tool is purely a structured request to the host — no shared
        // state, no filesystem, no network on its own. Safe to run alongside
        // other tools even if the host serializes the user-facing UI.
        true
    }

    async fn execute(&self, _input: Value) -> ToolResult {
        // Defensive fallback. The happy path routes the user's choice back
        // via `ProtocolCommand::ToolApprove { answer: Some(_) }`, and
        // orchestration (`wcore-agent::orchestration::mod.rs:911`)
        // synthesizes the ToolResult content directly. If we got here, the
        // approval channel did not carry an `answer` — either the host did
        // not send one (older host pre-v0.9.3) or the synthesis arm in
        // orchestration is unwired. Fail loud so the bug is obvious.
        ToolResult {
            content: "AskUserQuestion: approval-answer channel not wired — \
orchestration short-circuit missing or host did not include `answer` in \
ToolApprove. Update host to v0.9.3+ or check orchestration::mod.rs:911."
                .to_string(),
            is_error: true,
        }
    }

    fn category(&self) -> ToolCategory {
        // Info: same category as Clarify (clarify.rs:178-181). The real
        // approval gate is in orchestration: AskUserQuestion is NOT in any
        // default allow_list and Info is NOT auto-approved by Default mode,
        // so every invocation pauses on the approval channel — which is
        // exactly where the user's answer gets attached.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let q = input
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing question)");
        let head: String = q.chars().take(80).collect();
        if q.chars().count() > 80 {
            format!("ask_user: {head}…")
        } else {
            format!("ask_user: {head}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Registration shape: stable name + schema with the required fields the
    /// host UI depends on. The TUI's AskUserQuestion modal binds to
    /// `question` + `options[].label` + `options[].description`.
    #[tokio::test]
    async fn registers_with_stable_schema() {
        let tool = AskUserQuestionTool::new();
        assert_eq!(tool.name(), "AskUserQuestion");

        let schema = tool.input_schema();
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["required"], json!(["question", "options"]));
        assert_eq!(
            schema["properties"]["options"]["items"]["required"],
            json!(["label", "description"])
        );
    }

    /// Category is Info — same as Clarify. Gating happens because the tool
    /// is absent from every default allow_list, not via a per-tool flag.
    #[test]
    fn category_is_info() {
        let tool = AskUserQuestionTool::new();
        assert!(matches!(tool.category(), ToolCategory::Info));
    }

    /// `execute()` is the loud-defensive fallback for the case where the
    /// approval-channel answer never arrived. The orchestration synthesis
    /// path (W0.3) is what runs on the happy path.
    #[tokio::test]
    async fn execute_is_loud_defensive_fallback() {
        let tool = AskUserQuestionTool::new();
        let input = json!({
            "question": "Pick one",
            "options": [
                {"label": "A", "description": "alpha"},
                {"label": "B", "description": "beta"}
            ]
        });
        let result = tool.execute(input).await;
        assert!(
            result.is_error,
            "execute() must return is_error so a mis-routed call is loud, got: {}",
            result.content
        );
        assert!(
            result.content.contains("approval-answer channel"),
            "fallback content must point at the channel: {}",
            result.content
        );
    }

    /// `describe()` truncates long questions but keeps the prefix the user
    /// will see in the tool-card title.
    #[test]
    fn describe_truncates_long_questions() {
        let tool = AskUserQuestionTool::new();
        let short = json!({ "question": "Pick a backend" });
        assert_eq!(tool.describe(&short), "ask_user: Pick a backend");

        let long_q = "x".repeat(120);
        let long = json!({ "question": long_q });
        let desc = tool.describe(&long);
        assert!(desc.starts_with("ask_user: "));
        assert!(desc.ends_with('…'));
    }

    /// Concurrency safety claim is part of the tool's contract — the tool
    /// is a request shape, not a side-effect, so it parallelizes with
    /// anything else the LLM dispatches in the same turn.
    #[test]
    fn is_concurrency_safe_default_true() {
        let tool = AskUserQuestionTool::new();
        assert!(tool.is_concurrency_safe(&json!({})));
    }
}
