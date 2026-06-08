//! Tool-trace representation + json-stream/session-JSON parsers.
//!
//! T1/T2 declare the shape; T3 (Wave 0) implements the actual parsing,
//! cross-validation, and DeepSeek `reasoning_content` normalization
//! (per cross-audit L-5).
//!
//! # Silent-pass CI gate
//!
//! `clippy::todo` is denied at the crate root (see `lib.rs`) — any new
//! `todo!()` here or anywhere in this crate will fail
//! `cargo clippy -p wcore-eval-scenarios -- -D warnings`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Ordered list of tool invocations observed across a scenario run.
/// Built incrementally by the runner from `ProtocolEvent::ToolRequest`
/// / `ToolResult` events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolTrace {
    pub entries: Vec<TraceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub call_id: String,
    pub tool_name: String,
    /// JSON-encoded input arguments as supplied by the model.
    pub input: String,
    /// Final output text (truncated by the engine if long).
    pub output: String,
    pub is_error: bool,
    /// Wall-time observed between `ToolRequest` and `ToolResult` for
    /// this `call_id`. `None` until both have been seen.
    pub duration: Option<Duration>,
    /// Which turn (0-indexed) this call occurred during.
    pub turn: usize,
}

impl ToolTrace {
    /// Count how many entries name `tool`.
    pub fn count(&self, tool: &str) -> usize {
        self.entries.iter().filter(|e| e.tool_name == tool).count()
    }

    /// Count how many entries name `tool` AND occurred during turn `turn`.
    /// Used to scope per-turn `expected_tools`/`forbidden_tools` checks so a
    /// tool fired in turn 1 doesn't vacuously satisfy a turn-2 expectation
    /// (multi-turn cross-contamination — cross-audit finding #6).
    pub fn count_in_turn(&self, tool: &str, turn: usize) -> usize {
        self.entries
            .iter()
            .filter(|e| e.tool_name == tool && e.turn == turn)
            .count()
    }

    /// Parse a session JSON file (the engine writes one per session to
    /// `[session].directory`) into a `ToolTrace`. Used as a cross-check
    /// against the live json-stream events; a unit test asserts the two
    /// sources agree (plan §3.2).
    ///
    /// The session JSON format written by wcore-agent stores messages with
    /// embedded tool_use / tool_result blocks. We extract tool calls and
    /// their results from the message history.
    pub fn parse_session(path: &std::path::Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let v: serde_json::Value = serde_json::from_str(&raw)?;
        let mut entries = Vec::new();

        // Session JSON stores an array of messages under "messages".
        // Each assistant message may have tool_use blocks; corresponding
        // tool messages hold the tool_result.
        if let Some(messages) = v.get("messages").and_then(|m| m.as_array()) {
            for (turn_idx, msg) in messages.iter().enumerate() {
                let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                if role == "tool" {
                    // tool message: has call_id, tool_name, content (the output)
                    let call_id = msg
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tool_name = msg
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let output = msg
                        .get("content")
                        .map(|c| {
                            if let Some(s) = c.as_str() {
                                s.to_string()
                            } else {
                                c.to_string()
                            }
                        })
                        .unwrap_or_default();
                    let is_error = msg
                        .get("is_error")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    entries.push(TraceEntry {
                        call_id,
                        tool_name,
                        input: String::new(),
                        output,
                        is_error,
                        duration: None,
                        turn: turn_idx,
                    });
                }
            }
        }

        Ok(Self { entries })
    }
}
