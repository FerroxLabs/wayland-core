//! Canned `ProtocolEvent` sequences for testing the TUI.
//!
//! `#[cfg(test)]`-only. The protocol bridge ([`super::protocol_bridge`])
//! and the Wave-1 surface agents drive `App` purely from
//! `ProtocolEvent`s; these fixtures are deterministic engine-side event
//! streams so a surface test can reach a realistic `App` state without a
//! live engine. Wave-1 tests `use crate::tui::fixtures::*`.
//!
//! Every fixture returns only events the *engine emits* — there is no
//! `ProtocolEvent` for a user message (the surface router adds the user
//! `TurnView` when the user submits input), so a "full conversation"
//! fixture is the assistant-side stream that answers an already-sent
//! user turn.

use serde_json::json;
use wcore_protocol::events::{
    Capabilities, ErrorInfo, FinishReason, OutputType, ProtocolEvent, ToolCategory, ToolInfo,
    ToolStatus, Usage,
};

/// A full assistant reply: stream start, thinking, two text deltas,
/// stream end. Feeding this through the bridge flushes one assistant
/// `TurnView` and leaves no stream in flight.
pub fn full_conversation() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::StreamStart {
            msg_id: "m1".into(),
        },
        ProtocolEvent::Thinking {
            text: "The user wants a greeting.".into(),
            msg_id: "m1".into(),
        },
        ProtocolEvent::TextDelta {
            text: "Hello! ".into(),
            msg_id: "m1".into(),
        },
        ProtocolEvent::TextDelta {
            text: "How can I help?".into(),
            msg_id: "m1".into(),
        },
        ProtocolEvent::StreamEnd {
            msg_id: "m1".into(),
            finish_reason: FinishReason::Stop,
            usage: Some(Usage {
                input_tokens: 42,
                output_tokens: 17,
                cache_read_tokens: None,
                cache_write_tokens: None,
                active_window_percent: None,
            }),
            agent_run_id: None,
        },
    ]
}

/// A single tool call that requires approval and then succeeds:
/// `ToolRequest` → `ApprovalRequired` → `ToolRunning` → `ToolResult`.
/// Drives one tool card through `Running` → `AwaitingApproval` →
/// `Running` → `Ok`.
pub fn tool_call_with_approval() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::ToolRequest {
            msg_id: "m1".into(),
            call_id: "call-1".into(),
            tool: ToolInfo {
                name: "Bash".into(),
                category: ToolCategory::Exec,
                args: json!({"command": "cargo test"}),
                description: "Execute: cargo test".into(),
            },
        },
        ProtocolEvent::ApprovalRequired {
            call_id: "call-1".into(),
            resume_token: "tok-1".into(),
            correlation_id: "tok-1".into(),
            reason: "exec".into(),
            context: "run `cargo test`".into(),
        },
        ProtocolEvent::ToolRunning {
            msg_id: "m1".into(),
            call_id: "call-1".into(),
            tool_name: "Bash".into(),
        },
        ProtocolEvent::ToolResult {
            msg_id: "m1".into(),
            call_id: "call-1".into(),
            tool_name: "Bash".into(),
            status: ToolStatus::Success,
            output: "test result: ok. 12 passed".into(),
            output_type: OutputType::Text,
            metadata: None,
        },
    ]
}

/// An `Edit` tool call carrying old/new content in its request args.
/// Feeding this through the bridge populates the tool card's
/// `edit_preview` with a renderable `DiffModel` — no extra protocol
/// event needed.
pub fn edit_tool_call() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::ToolRequest {
            msg_id: "m1".into(),
            call_id: "call-edit".into(),
            tool: ToolInfo {
                name: "Edit".into(),
                category: ToolCategory::Edit,
                args: json!({
                    "file_path": "crates/wcore-cli/src/main.rs",
                    "old_string": "fn main() {}",
                    "new_string": "fn main() {\n    run();\n}",
                }),
                description: "Edit crates/wcore-cli/src/main.rs".into(),
            },
        },
        ProtocolEvent::ToolResult {
            msg_id: "m1".into(),
            call_id: "call-edit".into(),
            tool_name: "Edit".into(),
            status: ToolStatus::Success,
            output: "Edited crates/wcore-cli/src/main.rs".into(),
            output_type: OutputType::Diff,
            metadata: None,
        },
    ]
}

/// A sub-agent spawn: two `SubAgentEvent`s for the same parent — a
/// streaming line and a terminal `info` — so the bridge registers one
/// `SubAgentView` that ends `Done` with a non-empty feed.
pub fn sub_agent_spawn() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "spawn:reviewer".into(),
            agent_name: "reviewer".into(),
            inner: json!({
                "type": "text_delta",
                "text": "Reviewing the diff...",
                "msg_id": "sub-1",
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "spawn:reviewer".into(),
            agent_name: "reviewer".into(),
            inner: json!({
                "type": "stream_end",
                "msg_id": "sub-1",
                "finish_reason": "stop",
                "usage": {"input_tokens": 200, "output_tokens": 120},
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "spawn:reviewer".into(),
            agent_name: "reviewer".into(),
            inner: json!({
                "type": "info",
                "msg_id": "sub-1",
                "message": "Sub-agent finished: no issues found.",
            }),
        },
    ]
}

/// ForgeFlows-Live Phase 2 — a workflow run: `SubAgentEvent`s carrying the
/// `"workflow:<node_id>"` `parent_call_id` prefix for two nodes
/// (`stage-1`, `stage-2`), each a streaming line, a `stream_end` with
/// usage, and a terminal `info`. Feeding this through the bridge populates
/// `session.sub_agents` (unchanged SubAgents tab) AND `app.workflows` with
/// one workflow group holding two `Done` nodes with non-empty feeds.
pub fn workflow_run() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-1".into(),
            agent_name: "planner".into(),
            inner: json!({
                "type": "text_delta",
                "text": "Planning the change...",
                "msg_id": "wf-1",
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-1".into(),
            agent_name: "planner".into(),
            inner: json!({
                "type": "stream_end",
                "msg_id": "wf-1",
                "finish_reason": "stop",
                "usage": {"input_tokens": 300, "output_tokens": 180},
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-1".into(),
            agent_name: "planner".into(),
            inner: json!({
                "type": "info",
                "msg_id": "wf-1",
                "message": "Stage 1 finished: plan ready.",
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-2".into(),
            agent_name: "builder".into(),
            inner: json!({
                "type": "text_delta",
                "text": "Building from the plan...",
                "msg_id": "wf-2",
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-2".into(),
            agent_name: "builder".into(),
            inner: json!({
                "type": "stream_end",
                "msg_id": "wf-2",
                "finish_reason": "stop",
                "usage": {"input_tokens": 250, "output_tokens": 140},
            }),
        },
        ProtocolEvent::SubAgentEvent {
            parent_call_id: "workflow:stage-2".into(),
            agent_name: "builder".into(),
            inner: json!({
                "type": "info",
                "msg_id": "wf-2",
                "message": "Stage 2 finished: build complete.",
            }),
        },
    ]
}

/// A diagnostics burst: an `Error` and an `Info`. Each becomes a
/// `System` turn — handy for testing the transcript's system-notice
/// rendering.
pub fn diagnostics() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::Error {
            msg_id: None,
            error: ErrorInfo {
                code: "rate_limit".into(),
                message: "Too many requests; retrying.".into(),
                retryable: true,
            },
        },
        ProtocolEvent::Info {
            msg_id: "m1".into(),
            message: "Context compacted.".into(),
        },
    ]
}

/// A `ConfigChanged` event advertising MCP + non-destructive compaction.
/// Useful for config-surface tests.
pub fn config_changed() -> Vec<ProtocolEvent> {
    vec![ProtocolEvent::ConfigChanged {
        capabilities: Capabilities {
            tool_approval: true,
            thinking: true,
            mcp: true,
            non_destructive_compact: true,
            ..Default::default()
        },
    }]
}
