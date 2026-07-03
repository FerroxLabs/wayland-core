//! Engine bridge seam for the ACP/REST transports.
//!
//! `wcore-acp` is a mid-layer crate and MUST NOT depend on `wcore-agent`
//! (the top-layer engine). The engine is reached through this trait,
//! implemented in `wcore-cli` exactly like [`crate::a2a::A2aHandler`]. Keeps
//! the transport crate engine-free while the CLI layer owns the engine.

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::Stream;

use crate::error::AcpError;
use crate::protocol::{MessageEvent, ToolDefinition};

/// Transport-neutral approval scope for a host-driven resolution.
///
/// Mirrors the `wcore-protocol` `ApprovalScope` concept WITHOUT a dependency
/// on that crate: `wcore-acp` is a mid-layer transport crate and deliberately
/// does NOT depend on `wcore-protocol` (the engine boundary). The CLI layer,
/// which depends on both, maps this onto the real `ApprovalScope` when it
/// reaches the session's `ToolApprovalManager`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ApprovalScopeWire {
    /// Approve only this one call.
    #[default]
    Once,
    /// Approve and persist auto-approval for this tool name.
    Always,
    /// Approve and persist a prefix-scoped auto-approval rule.
    AlwaysPrefix { prefix: String },
}

/// A host's decision answering an `ApprovalRequired` gate (Blocker #2).
///
/// Carried from the REST `/resolve` endpoint through [`TurnEngine`] to the
/// session's approval manager. `answer` threads an AskUserQuestion-class
/// choice back through the approval channel (ignored for ordinary
/// approve/deny of a mutating tool).
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    /// `true` = approve the gated call, `false` = deny it.
    pub approved: bool,
    /// Auto-approval persistence scope. Only meaningful when `approved`.
    pub scope: ApprovalScopeWire,
    /// Optional answer payload (AskUserQuestion choice). `None` for a plain
    /// approve/deny.
    pub answer: Option<String>,
    /// GHSA-8r7g M2 (wayland#568) — the SECRET `resume_token` for a
    /// bridge-backed gate. `None`/empty resolves via the approval manager by
    /// `call_id` (the legacy manager-gated path).
    pub resume_token: Option<String>,
}

/// Per-turn parameters handed to the engine bridge. A struct (not a long
/// arg list) so adding fields later is non-breaking.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    /// The ACP session id the turn belongs to. Conversation history is
    /// keyed on this so multi-turn sessions stay coherent.
    pub session_id: String,
    /// The user prompt text for this turn.
    pub text: String,
    /// Per-call tool overrides from the request body. Empty = use the
    /// session's configured allowlist (or the engine's full registry when
    /// the session record carries none).
    pub tools: Vec<ToolDefinition>,
}

/// Turns one user prompt into a stream of [`MessageEvent`]s.
///
/// Contract the implementation MUST honour (externally testable):
///   * zero-or-more `Thinking` / `TextDelta` frames as the model streams;
///   * for each tool: one `ToolCall { call }` BEFORE execution, then one
///     matching `ToolResult { result }` (`result.call_id == call.id`,
///     `is_error` set on failure) before any terminal frame;
///   * D012 (P0 security): when the session's approval posture GATES a tool
///     (the default for a network-exposed engine, where no explicit
///     allow-all / Force posture is set), the implementation MUST emit one
///     `ApprovalRequired { call, .. }` (`call.id == ToolCall.id`) BEFORE that
///     tool's `ToolResult`. The tool MUST NOT execute until the gate is
///     resolved; an unanswered gate times out rather than running ungated.
///     Under an explicit allow-all / Force posture the gate frame is omitted
///     and the bare `ToolCall` rides straight to `ToolResult`. This makes the
///     safety control posture-driven, never silently dependent on which
///     front-end drives the engine.
///   * EXACTLY ONE terminal frame, last: `Done { stop_reason }` carrying an
///     ACP StopReason string (`end_turn` | `max_tokens` | `max_turn_requests`
///     | `refusal` | `cancelled`), OR `Error { error }`. Nothing after it.
#[async_trait]
pub trait TurnEngine: Send + Sync {
    /// Run one prompt turn. The returned stream MUST end with exactly one
    /// terminal [`MessageEvent`] (`Done` or `Error`) and emit nothing after
    /// it. An `Err` here is reserved for failures that happen BEFORE the
    /// stream is established (e.g. building the engine session failed); once
    /// a stream exists, in-turn failures ride it as a terminal `Error` frame.
    async fn run_turn(
        &self,
        req: TurnRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError>;

    /// Resolve a pending tool-approval gate for `session_id` / `call_id`
    /// (Blocker #2). `call_id` is the `ToolCall.id` carried by the
    /// `ApprovalRequired` frame the host saw on the prompt stream.
    ///
    /// Contract:
    ///   * unknown session → `Err(AcpError::Session("... not found ..."))`
    ///     (maps to 404);
    ///   * unknown / already-resolved / expired `call_id` →
    ///     `Err(AcpError::Session("approval not found ..."))` (404, idempotent —
    ///     a second resolve of the same id is a clean not-found, never a panic);
    ///   * resolved → `Ok(())`.
    ///
    /// Default impl returns `AcpError::Protocol` so engines that do not gate
    /// (the scripted/mock test engines) compile unchanged; the production
    /// `EngineTurnEngine` overrides it to reach the session's approval manager.
    async fn resolve_approval(
        &self,
        _session_id: &str,
        _call_id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), AcpError> {
        Err(AcpError::Protocol(
            "approval resolution not supported by this engine".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JsonRpcError, ToolCall, ToolResult};
    use futures::stream::{self, StreamExt};
    use std::sync::Arc;

    /// A hand-written `TurnEngine` that replays a fixed event script. Used
    /// to pin the stream contract without an engine.
    struct MockTurnEngine {
        script: Vec<MessageEvent>,
    }

    #[async_trait]
    impl TurnEngine for MockTurnEngine {
        async fn run_turn(
            &self,
            _req: TurnRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
            Ok(stream::iter(self.script.clone()).boxed())
        }
    }

    /// T-A1: a turn yields `[TextDelta, ToolCall, ToolResult, Done]` in order,
    /// with exactly one terminal frame last and nothing after it.
    #[tokio::test]
    async fn mock_turn_engine_emits_ordered_stream_with_one_terminal() {
        let engine = MockTurnEngine {
            script: vec![
                MessageEvent::TextDelta {
                    text: "hi".to_string(),
                },
                MessageEvent::ToolCall {
                    call: ToolCall {
                        id: "c1".to_string(),
                        name: "Read".to_string(),
                        input: serde_json::json!({"path": "x"}),
                    },
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
        let req = TurnRequest {
            session_id: "s1".to_string(),
            text: "go".to_string(),
            tools: Vec::new(),
        };
        let frames: Vec<MessageEvent> = Arc::new(engine)
            .run_turn(req)
            .await
            .expect("stream")
            .collect()
            .await;

        assert_eq!(frames.len(), 4);
        assert!(matches!(frames[0], MessageEvent::TextDelta { .. }));
        assert!(matches!(frames[1], MessageEvent::ToolCall { .. }));
        assert!(matches!(frames[2], MessageEvent::ToolResult { .. }));
        // Exactly one terminal frame, and it is last.
        let terminals = frames
            .iter()
            .filter(|e| matches!(e, MessageEvent::Done { .. } | MessageEvent::Error { .. }))
            .count();
        assert_eq!(terminals, 1, "exactly one terminal frame");
        match frames.last().expect("last frame") {
            MessageEvent::Done { stop_reason } => assert_eq!(stop_reason, "end_turn"),
            other => panic!("expected Done last, got {other:?}"),
        }
    }

    /// A terminal `Error` frame is a valid stream ending too.
    #[tokio::test]
    async fn mock_turn_engine_error_terminal_is_accepted() {
        let engine = MockTurnEngine {
            script: vec![MessageEvent::Error {
                error: JsonRpcError {
                    code: -32003,
                    message: "boom".to_string(),
                    data: None,
                },
            }],
        };
        let req = TurnRequest {
            session_id: "s1".to_string(),
            text: "go".to_string(),
            tools: Vec::new(),
        };
        let frames: Vec<MessageEvent> = Arc::new(engine)
            .run_turn(req)
            .await
            .expect("stream")
            .collect()
            .await;
        assert_eq!(frames.len(), 1);
        assert!(matches!(frames[0], MessageEvent::Error { .. }));
    }
}
