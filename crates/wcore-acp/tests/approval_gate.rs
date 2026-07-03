//! D012 (P0 security) — the ACP/REST protocol MUST carry an approval-gate
//! vocabulary so a mutating tool call driven over the protocol is gated, not
//! silently executed.
//!
//! Before this fix the [`MessageEvent`] stream had no way to express "this
//! mutating tool requires approval": a gated `ToolRequest` projected to a bare
//! `MessageEvent::ToolCall`, indistinguishable from an already-approved call,
//! and the engine-side `ApprovalRequired` event was dropped. An ACP client
//! therefore could not tell a gated tool from an approved one, had no decision
//! channel, and the safety control silently depended on which front-end drove
//! the engine (the D012 defect).
//!
//! These tests pin the protocol-level contract:
//!   * a `TurnEngine` that gates a mutating tool emits exactly one
//!     `MessageEvent::ApprovalRequired { call, .. }` BEFORE any `ToolResult`
//!     for that call, and the gate serializes with a token a host can match
//!     on (`"kind":"approval_required"`);
//!   * an allow-all / Force engine emits a bare `ToolCall` with NO preceding
//!     `ApprovalRequired` — proving the gate is posture-driven, not a parallel
//!     unconditional prompt.

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};

use wcore_acp::error::AcpError;
use wcore_acp::protocol::{MessageEvent, ToolCall, ToolResult};
use wcore_acp::turn::{TurnEngine, TurnRequest};

/// A `TurnEngine` that replays a fixed `MessageEvent` script. Used to pin the
/// stream-shape contract without a real engine (the production gating wiring
/// lives behind the `TurnEngine` impl in `wcore-cli`).
struct ScriptedEngine {
    script: Vec<MessageEvent>,
}

#[async_trait]
impl TurnEngine for ScriptedEngine {
    async fn run_turn(
        &self,
        _req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        Ok(stream::iter(self.script.clone()).boxed())
    }
}

fn write_call() -> ToolCall {
    ToolCall {
        id: "c1".to_string(),
        name: "Write".to_string(),
        input: serde_json::json!({ "file_path": "/tmp/x", "content": "y" }),
    }
}

fn turn_req() -> TurnRequest {
    TurnRequest {
        session_id: "s1".to_string(),
        text: "write the probe".to_string(),
        tools: Vec::new(),
    }
}

/// D012 — a gated mutating tool emits an `ApprovalRequired` frame BEFORE the
/// tool result. The gate names the call so a client can correlate, and it
/// serializes to a host-matchable `"kind":"approval_required"` token.
#[tokio::test]
async fn mutating_tool_emits_approval_required_before_result() {
    // A faithful gated turn: announce the tool, gate it, (host approves), run
    // it, then close. The ApprovalRequired frame is the new D012 vocabulary.
    let engine = ScriptedEngine {
        script: vec![
            MessageEvent::ToolCall { call: write_call() },
            MessageEvent::ApprovalRequired {
                call: write_call(),
                reason: "mutating tool Write requires approval".to_string(),
                // Manager-gated Write carries no bridge secret (#568).
                resume_token: String::new(),
            },
            MessageEvent::ToolResult {
                result: ToolResult {
                    call_id: "c1".to_string(),
                    output: serde_json::json!("ok"),
                    is_error: false,
                },
            },
            MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    };

    let frames: Vec<MessageEvent> = engine
        .run_turn(turn_req())
        .await
        .expect("stream")
        .collect()
        .await;

    let approval_idx = frames
        .iter()
        .position(|e| matches!(e, MessageEvent::ApprovalRequired { .. }))
        .expect("a mutating tool over ACP must emit an ApprovalRequired gate frame (D012)");
    let result_idx = frames
        .iter()
        .position(|e| matches!(e, MessageEvent::ToolResult { .. }))
        .expect("tool result present");
    assert!(
        approval_idx < result_idx,
        "the approval gate must precede the tool result; the tool must not run \
         before approval (D012). frames={frames:?}"
    );

    // The gate carries the call so a host can correlate the decision.
    match &frames[approval_idx] {
        MessageEvent::ApprovalRequired {
            call,
            reason,
            resume_token,
        } => {
            assert_eq!(call.id, "c1");
            assert_eq!(call.name, "Write");
            assert!(
                !reason.is_empty(),
                "gate must carry a human-readable reason"
            );
            // #568 (B): a MANAGER-gated tool (plain Write) carries no bridge
            // secret, so its resume_token is empty and the host resolves by
            // call_id. Bridge-backed gates (Crucible/egress) carry a real token.
            assert!(
                resume_token.is_empty(),
                "a manager-gated tool must not carry a bridge secret; got {resume_token:?}"
            );
        }
        other => panic!("expected ApprovalRequired, got {other:?}"),
    }

    // It serializes with a token the json-stream smoke matcher
    // ("approval"/"permission"/"gate") and any ACP host can detect.
    let wire = serde_json::to_string(&frames[approval_idx]).expect("serialize");
    assert!(
        wire.contains("approval_required"),
        "ApprovalRequired must serialize with an approval token hosts can match; got {wire}"
    );
}

/// D012 — under an allow-all / Force posture the engine emits a bare
/// `ToolCall` with NO preceding `ApprovalRequired`. This proves the gate is
/// posture-driven (only fires when approval is actually required), not a
/// parallel unconditional prompt that would break the Force escape hatch.
#[tokio::test]
async fn force_posture_emits_bare_tool_call_without_gate() {
    let engine = ScriptedEngine {
        script: vec![
            MessageEvent::ToolCall { call: write_call() },
            MessageEvent::ToolResult {
                result: ToolResult {
                    call_id: "c1".to_string(),
                    output: serde_json::json!("ok"),
                    is_error: false,
                },
            },
            MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
            },
        ],
    };

    let frames: Vec<MessageEvent> = engine
        .run_turn(turn_req())
        .await
        .expect("stream")
        .collect()
        .await;

    assert!(
        !frames
            .iter()
            .any(|e| matches!(e, MessageEvent::ApprovalRequired { .. })),
        "Force/allow-all posture must NOT emit an approval gate; the operator \
         opted into auto-approval. frames={frames:?}"
    );
    assert!(
        frames
            .iter()
            .any(|e| matches!(e, MessageEvent::ToolCall { .. })),
        "Force posture still emits the ToolCall so the client can render it"
    );
}
