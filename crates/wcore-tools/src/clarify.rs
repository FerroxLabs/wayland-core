//! T3-3.1.1 — clarify_tool port from the prior Wayland Python engine.
//!
//! The `clarify` tool lets the agent surface a structured question (with up
//! to 4 multiple-choice options, plus an implicit "Other" the UI appends)
//! or a free-form open-ended prompt, to the user. The actual user-facing
//! interaction lives in the host/platform layer (CLI / Wayland desktop app / gateway).
//! This tool's job is to validate the agent's intent and emit a structured
//! payload describing the question. The host intercepts tool calls named
//! `clarify` to perform the real prompt and inject the response back into
//! the conversation.
//!
//! Divergence vs the Python original: Python takes a `callback` kwarg
//! injected by the runner. In Rust we don't yet thread a callback through
//! `ToolContext` (host integration lands in a later sub-wave), so
//! `execute()` returns the validated question shape rather than blocking
//! on a callback. The shape matches the Python success payload's keys
//! (`question`, `choices_offered`) plus a `status: "pending"` marker so
//! the host can distinguish "agent asked something" from "agent finished
//! asking and has a user response".

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Maximum predefined choices the agent may offer. The UI is expected to
/// append a 5th "Other (type your answer)" option on top of this.
pub const MAX_CHOICES: usize = 4;

/// `clarify` — ask the user a multi-choice or open-ended question.
///
/// Zero-state tool: holds no fields. Construction is trivial via the
/// `Default` impl (`ClarifyTool` or `ClarifyTool::default()`).
#[derive(Default)]
pub struct ClarifyTool;

impl ClarifyTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ClarifyTool {
    fn name(&self) -> &str {
        "clarify"
    }

    fn description(&self) -> &str {
        "Ask the user a question when you need clarification, feedback, or a \
decision before proceeding. Supports two modes:\n\n\
1. Multiple choice — provide up to 4 choices. The user picks one or types their own answer via a 5th 'Other' option.\n\
2. Open-ended — omit choices entirely. The user types a free-form response.\n\n\
Use this tool when:\n\
- The task is ambiguous and you need the user to choose an approach\n\
- You want post-task feedback ('How did that work out?')\n\
- You want to offer to save a skill or update memory\n\
- A decision has meaningful trade-offs the user should weigh in on\n\n\
Do NOT use this tool for simple yes/no confirmation of dangerous commands. \
Prefer making a reasonable default choice yourself when the decision is low-stakes."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to present to the user."
                },
                "choices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_CHOICES,
                    "description": "Up to 4 answer choices. Omit this parameter \
        entirely to ask an open-ended question. When provided, the UI automatically \
        appends an 'Other (type your answer)' option."
                }
            },
            "required": ["question"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // The tool is purely about emitting a structured request to the
        // host — no shared state, no filesystem, no network. Safe to run
        // concurrently with anything else even if the host serializes the
        // actual user interaction.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // Validate `question`: must be a non-empty string after trim.
        let question_raw = match input.get("question").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Question text is required.".to_string(),
                    is_error: true,
                };
            }
        };
        let question = question_raw.trim();
        if question.is_empty() {
            return ToolResult {
                content: "Question text is required.".to_string(),
                is_error: true,
            };
        }

        // Validate `choices`: must be a JSON array of strings if present.
        // Per the Python source, non-array `choices` is an explicit error.
        // Empty / all-whitespace items are dropped; if the resulting list
        // is empty we fall back to open-ended (None). Excess items beyond
        // MAX_CHOICES are clamped (NOT an error — matches Python).
        let choices_offered: Option<Vec<String>> = match input.get("choices") {
            None | Some(Value::Null) => None,
            Some(Value::Array(arr)) => {
                let cleaned: Vec<String> = arr
                    .iter()
                    .filter_map(|v| match v {
                        Value::String(s) => {
                            let trimmed = s.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        }
                        // Non-string items: stringify via Display of the
                        // JSON value (preserves Python's `str(c).strip()`
                        // intent without panicking on non-string input).
                        _ => {
                            let s = v.to_string();
                            let trimmed = s.trim();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        }
                    })
                    .take(MAX_CHOICES)
                    .collect();
                if cleaned.is_empty() {
                    None
                } else {
                    Some(cleaned)
                }
            }
            Some(_) => {
                return ToolResult {
                    content: "choices must be a list of strings.".to_string(),
                    is_error: true,
                };
            }
        };

        // Emit a structured "pending user input" payload. The host layer
        // intercepts tool calls named `clarify` and replaces this with the
        // real user response before the next agent turn.
        let payload = json!({
            "status": "pending",
            "question": question,
            "choices_offered": choices_offered,
        });

        ToolResult {
            content: payload.to_string(),
            is_error: false,
        }
    }

    fn category(&self) -> ToolCategory {
        // Clarify is informational from the engine's perspective — it
        // produces a structured prompt, mutates nothing, executes nothing.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let q = input
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing question)");
        // Keep the describe-line short — full questions can be long.
        let head: String = q.chars().take(80).collect();
        if q.chars().count() > 80 {
            format!("clarify: {head}…")
        } else {
            format!("clarify: {head}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Happy path: tool emits a pending-input payload echoing the question
    /// and the clamped/cleaned choice list. Also covers the registry-style
    /// look-up by name (test #1 from the dispatch contract).
    #[tokio::test]
    async fn registers_and_executes_happy_path() {
        let tool = ClarifyTool::new();

        // (1) Registration shape: name + schema are stable + required.
        assert_eq!(tool.name(), "clarify");
        let schema = tool.input_schema();
        assert_eq!(schema["type"], json!("object"));
        assert_eq!(schema["required"], json!(["question"]));
        assert_eq!(schema["properties"]["choices"]["maxItems"], json!(4));

        // (2) Execution returns non-error with expected shape.
        let input = json!({
            "question": "  Pick one of these  ",
            "choices": ["alpha", "beta", "gamma"]
        });
        let result = tool.execute(input).await;
        assert!(
            !result.is_error,
            "happy path must succeed: {}",
            result.content
        );

        let parsed: Value =
            serde_json::from_str(&result.content).expect("content must be valid JSON");
        assert_eq!(parsed["status"], json!("pending"));
        // Trimmed.
        assert_eq!(parsed["question"], json!("Pick one of these"));
        assert_eq!(parsed["choices_offered"], json!(["alpha", "beta", "gamma"]));
    }

    /// Choices beyond MAX_CHOICES are clamped (not rejected) and empty
    /// items are dropped. Matches the Python source's behavior.
    #[tokio::test]
    async fn clamps_excess_choices_and_drops_blanks() {
        let tool = ClarifyTool::new();
        let input = json!({
            "question": "Pick something",
            "choices": ["a", "", "  ", "b", "c", "d", "e", "f"]
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error);

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        // After filtering blanks: [a, b, c, d, e, f], clamped to 4 → [a,b,c,d].
        assert_eq!(parsed["choices_offered"], json!(["a", "b", "c", "d"]));
    }

    /// Open-ended: omitting `choices` entirely yields `choices_offered: null`.
    #[tokio::test]
    async fn open_ended_when_choices_omitted() {
        let tool = ClarifyTool::new();
        let input = json!({ "question": "What now?" });
        let result = tool.execute(input).await;
        assert!(!result.is_error);

        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(parsed["question"], json!("What now?"));
        assert!(parsed["choices_offered"].is_null());
    }

    /// Invalid input — missing required `question` field — must error cleanly.
    /// This is test #3 from the dispatch contract.
    #[tokio::test]
    async fn errors_on_missing_question() {
        let tool = ClarifyTool::new();
        let input = json!({ "choices": ["a", "b"] });
        let result = tool.execute(input).await;
        assert!(result.is_error);
        assert!(
            result.content.contains("required"),
            "error msg should mention required: {}",
            result.content
        );
    }

    /// Whitespace-only question is treated as empty (matches Python).
    #[tokio::test]
    async fn errors_on_whitespace_question() {
        let tool = ClarifyTool::new();
        let input = json!({ "question": "   \n\t  " });
        let result = tool.execute(input).await;
        assert!(result.is_error);
        assert!(result.content.contains("required"));
    }

    /// Non-array `choices` value (e.g. a string) must error cleanly per
    /// the Python source's explicit type guard.
    #[tokio::test]
    async fn errors_on_non_array_choices() {
        let tool = ClarifyTool::new();
        let input = json!({
            "question": "Pick",
            "choices": "not an array"
        });
        let result = tool.execute(input).await;
        assert!(result.is_error);
        assert!(result.content.contains("list"));
    }

    /// All-blank choices array collapses to open-ended (None) rather than
    /// returning an error. Matches Python.
    #[tokio::test]
    async fn all_blank_choices_collapse_to_open_ended() {
        let tool = ClarifyTool::new();
        let input = json!({
            "question": "Pick",
            "choices": ["", "   ", "\t"]
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error);
        let parsed: Value = serde_json::from_str(&result.content).unwrap();
        assert!(parsed["choices_offered"].is_null());
    }

    /// Concurrency safety is a static property — verify it's claimed.
    #[test]
    fn is_concurrency_safe_default_true() {
        let tool = ClarifyTool::new();
        assert!(tool.is_concurrency_safe(&json!({})));
    }
}
