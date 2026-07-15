use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::events::{
    Capabilities, ErrorInfo, OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
    TurnCost, Usage,
};

use super::fixtures_support::capabilities;

/// One current Desktop-consumed wire variant.
#[derive(Debug, Clone, Copy)]
pub struct WireSpec {
    pub wire_type: &'static str,
    pub path: &'static str,
    pub required: &'static [&'static str],
    pub criticality: &'static str,
    pub correlation: &'static str,
    pub capability: &'static str,
}

macro_rules! wire {
    ($wire:literal, $path:literal, [$($required:literal),*], $criticality:literal, $correlation:literal, $capability:literal) => {
        WireSpec {
            wire_type: $wire,
            path: $path,
            required: &["type", $($required),*],
            criticality: $criticality,
            correlation: $correlation,
            capability: $capability,
        }
    };
}

pub const COMMAND_SPECS: &[WireSpec] = &[
    wire!(
        "message",
        "commands/message.json",
        ["msg_id", "content"],
        "safe",
        "msg_id",
        "available"
    ),
    wire!(
        "stop",
        "commands/stop.json",
        [],
        "safe",
        "session",
        "available"
    ),
    wire!(
        "tool_approve",
        "commands/tool_approve.json",
        ["call_id"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "tool_deny",
        "commands/tool_deny.json",
        ["call_id"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "approval_resume",
        "commands/approval_resume.json",
        ["resume_token", "approved"],
        "safe",
        "resume_token",
        "available"
    ),
    wire!(
        "init_history",
        "commands/init_history.json",
        ["text"],
        "safe",
        "session",
        "available"
    ),
    wire!(
        "set_mode",
        "commands/set_mode.json",
        ["mode"],
        "safe",
        "session",
        "available"
    ),
    wire!(
        "set_config",
        "commands/set_config.json",
        [],
        "safe",
        "session",
        "available"
    ),
    wire!(
        "add_mcp_server",
        "commands/add_mcp_server.json",
        ["name", "transport"],
        "safe",
        "name",
        "available"
    ),
    wire!(
        "host_send_message_result",
        "commands/host_send_message_result.json",
        ["call_id", "ok"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "ping",
        "commands/ping.json",
        [],
        "observation",
        "connection",
        "available"
    ),
];

pub const EVENT_SPECS: &[WireSpec] = &[
    wire!(
        "ready",
        "events/ready.json",
        ["version", "capabilities"],
        "required",
        "session_id",
        "available"
    ),
    wire!(
        "stream_start",
        "events/stream_start.json",
        ["msg_id"],
        "observation",
        "msg_id",
        "available"
    ),
    wire!(
        "text_delta",
        "events/text_delta.json",
        ["text", "msg_id"],
        "observation",
        "msg_id",
        "available"
    ),
    wire!(
        "thinking",
        "events/thinking.json",
        ["text", "msg_id"],
        "observation",
        "msg_id",
        "available"
    ),
    wire!(
        "tool_request",
        "events/tool_request.json",
        ["msg_id", "call_id", "tool"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "tool_running",
        "events/tool_running.json",
        ["msg_id", "call_id", "tool_name"],
        "observation",
        "call_id",
        "available"
    ),
    wire!(
        "tool_result",
        "events/tool_result.json",
        [
            "msg_id",
            "call_id",
            "tool_name",
            "status",
            "output",
            "output_type"
        ],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "tool_cancelled",
        "events/tool_cancelled.json",
        ["msg_id", "call_id", "reason"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "stream_end",
        "events/stream_end.json",
        ["msg_id", "finish_reason"],
        "safe",
        "msg_id",
        "available"
    ),
    wire!(
        "error",
        "events/error.json",
        ["error"],
        "safe",
        "msg_id_or_session",
        "available"
    ),
    wire!(
        "info",
        "events/info.json",
        ["msg_id", "message"],
        "observation",
        "msg_id",
        "available"
    ),
    wire!(
        "config_changed",
        "events/config_changed.json",
        ["capabilities"],
        "observation",
        "session",
        "available"
    ),
    wire!(
        "mcp_ready",
        "events/mcp_ready.json",
        ["name", "tools"],
        "observation",
        "name",
        "available"
    ),
    wire!(
        "mcp_failed",
        "events/mcp_failed.json",
        ["name", "reason"],
        "safe",
        "name",
        "available"
    ),
    wire!(
        "pong",
        "events/pong.json",
        [],
        "observation",
        "connection",
        "available"
    ),
    wire!(
        "trace_event",
        "events/trace_event.json",
        ["msg_id", "trace"],
        "observation",
        "msg_id",
        "structured_traces"
    ),
    wire!(
        "session_cost",
        "events/session_cost.json",
        ["session_id", "total_cost_usd", "per_turn"],
        "observation",
        "session_id",
        "cost_attribution"
    ),
    wire!(
        "sub_agent_event",
        "events/sub_agent_event.json",
        ["parent_call_id", "agent_name", "inner"],
        "observation",
        "parent_call_id",
        "legacy_identity_only"
    ),
    wire!(
        "tool_chunk",
        "events/tool_chunk.json",
        ["msg_id", "call_id", "tool_name", "chunk"],
        "observation",
        "call_id",
        "streaming_tools"
    ),
    wire!(
        "provider_circuit_event",
        "events/provider_circuit_event.json",
        ["primary", "state"],
        "safe",
        "primary",
        "available"
    ),
    wire!(
        "approval_required",
        "events/approval_required.json",
        ["call_id", "resume_token", "reason", "context"],
        "safe",
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "suspend",
        "events/suspend.json",
        ["reason", "resume_token"],
        "safe",
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "approval_resume",
        "events/approval_resume.json",
        ["resume_token", "approved"],
        "safe",
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "budget_exceeded",
        "events/budget_exceeded.json",
        ["reason", "observed", "limit"],
        "safe",
        "session",
        "available"
    ),
    wire!(
        "tool_panicked",
        "events/tool_panicked.json",
        ["msg_id", "call_id", "tool_name", "panic_message"],
        "safe",
        "call_id",
        "available"
    ),
    wire!(
        "plugin_registration_failed",
        "events/plugin_registration_failed.json",
        ["plugin_name", "surface", "error_kind", "message"],
        "safe",
        "plugin_name_and_surface",
        "available"
    ),
    wire!(
        "plugin_event",
        "events/plugin_event.json",
        ["plugin_name", "event_type", "payload"],
        "observation",
        "plugin_name",
        "shape_only"
    ),
    wire!(
        "evolution_event",
        "events/evolution_event.json",
        [
            "run_id",
            "generation",
            "parent_id",
            "child_id",
            "mutation_kind",
            "score",
            "retained"
        ],
        "observation",
        "run_id",
        "gepa_enabled"
    ),
    wire!(
        "browser_event",
        "events/browser_event.json",
        ["msg_id", "call_id", "op", "summary"],
        "observation",
        "call_id",
        "shape_only"
    ),
    wire!(
        "browser_policy_denied",
        "events/browser_policy_denied.json",
        ["msg_id", "url", "reason"],
        "safe",
        "msg_id",
        "shape_only"
    ),
    wire!(
        "cua_event",
        "events/cua_event.json",
        ["msg_id", "call_id", "op", "summary"],
        "observation",
        "call_id",
        "shape_only"
    ),
    wire!(
        "cua_policy_denied",
        "events/cua_policy_denied.json",
        ["msg_id", "op", "reason"],
        "safe",
        "msg_id",
        "shape_only"
    ),
    wire!(
        "host_send_message_request",
        "events/host_send_message_request.json",
        ["call_id", "platform", "body"],
        "safe",
        "call_id",
        "host_delegated_delivery"
    ),
];

pub const PRODUCER_COMMAND_TYPES: &[&str] = &[
    "message",
    "stop",
    "tool_approve",
    "tool_deny",
    "init_history",
    "set_mode",
    "set_config",
    "continue_with_budget",
    "add_mcp_server",
    "grant_workspace_capability",
    "approval_resume",
    "host_send_message_result",
    "ping",
];

pub const PRODUCER_EVENT_TYPES: &[&str] = &[
    "ready",
    "execution_policy",
    "workspace_policy",
    "capability_activation",
    "stream_start",
    "text_delta",
    "thinking",
    "tool_request",
    "tool_running",
    "tool_result",
    "tool_cancelled",
    "stream_end",
    "error",
    "info",
    "config_changed",
    "mcp_ready",
    "mcp_failed",
    "trace_event",
    "session_cost",
    "sub_agent_event",
    "workflow_started",
    "workflow_finished",
    "tool_chunk",
    "provider_circuit_event",
    "provider_attempt",
    "provider_retry",
    "provider_failure",
    "mid_flight_monitor_decision",
    "approval_required",
    "suspend",
    "approval_resume",
    "budget_exceeded",
    "tool_panicked",
    "plugin_registration_failed",
    "plugin_event",
    "evolution_event",
    "browser_event",
    "browser_policy_denied",
    "cua_event",
    "cua_policy_denied",
    "host_send_message_request",
    "compact_offload",
    "anvil_receipt",
    "pong",
];

pub const SOURCE_INPUTS: &[&str] = &[
    "crates/wcore-protocol/src/commands.rs",
    "crates/wcore-protocol/src/events.rs",
    "crates/wcore-protocol/src/reader.rs",
    "crates/wcore-protocol/src/writer.rs",
    "crates/wcore-protocol/src/contract/mod.rs",
    "crates/wcore-protocol/src/contract/canonical.rs",
    "crates/wcore-protocol/src/contract/spec.rs",
    "crates/wcore-protocol/src/contract/generate.rs",
    "crates/wcore-protocol/src/contract/check.rs",
    "crates/wcore-protocol/src/bin/wcore-contract.rs",
    "crates/wcore-types/src/execution_policy.rs",
    "crates/wcore-types/src/workspace_trust.rs",
    "crates/wcore-agent/src/output/protocol_sink.rs",
    "crates/wcore-agent/src/orchestration/workflow/runner.rs",
    "crates/wcore-agent/src/orchestration/anvil/forge.rs",
    "crates/wcore-cli/src/main.rs",
];

/// Canonical command inputs. Every value is accepted by `ProtocolCommand`.
pub fn command_fixture_values() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "commands/add_mcp_server.json".into(),
            json!({"type":"add_mcp_server","name":"desktop-tools","transport":"stdio","command":"desktop-mcp","args":["--stdio"],"env":{"WAYLAND_PROFILE":"desktop"},"url":"https://mcp.invalid/v1","headers":{"X-Wayland-Contract":"v1"}}),
        ),
        (
            "commands/approval_resume.json".into(),
            json!({"type":"approval_resume","resume_token":"resume-001","approved":true,"modifications":{"answer":"approved"}}),
        ),
        (
            "commands/host_send_message_result.json".into(),
            json!({"type":"host_send_message_result","call_id":"call-send-001","ok":true,"message_id":"desktop-message-001","error":""}),
        ),
        (
            "commands/init_history.json".into(),
            json!({"type":"init_history","text":"Pinned Desktop session context."}),
        ),
        (
            "commands/message.json".into(),
            json!({"type":"message","msg_id":"msg-001","content":"Inspect the current workspace.","files":["README.md"]}),
        ),
        ("commands/ping.json".into(), json!({"type":"ping"})),
        (
            "commands/set_config.json".into(),
            json!({"type":"set_config","model":"claude-sonnet-4-5","thinking":"enabled","thinking_budget":4096,"effort":"high","compaction":"safe"}),
        ),
        (
            "commands/set_mode.json".into(),
            json!({"type":"set_mode","mode":"force"}),
        ),
        ("commands/stop.json".into(), json!({"type":"stop"})),
        (
            "commands/tool_approve.json".into(),
            json!({"type":"tool_approve","call_id":"call-tool-001","scope":"once","answer":"Proceed"}),
        ),
        (
            "commands/tool_deny.json".into(),
            json!({"type":"tool_deny","call_id":"call-tool-002","reason":"Operator denied execution"}),
        ),
        (
            "compat/commands/add_mcp_server.minimal.json".into(),
            json!({"type":"add_mcp_server","name":"minimal","transport":"stdio"}),
        ),
        (
            "compat/commands/approval_resume.minimal.json".into(),
            json!({"type":"approval_resume","resume_token":"resume-minimal","approved":false}),
        ),
        (
            "compat/commands/host_send_message_result.minimal.json".into(),
            json!({"type":"host_send_message_result","call_id":"call-send-minimal","ok":false}),
        ),
        (
            "compat/commands/message.minimal.json".into(),
            json!({"type":"message","msg_id":"msg-minimal","content":"hello"}),
        ),
        (
            "compat/commands/set_config.minimal.json".into(),
            json!({"type":"set_config"}),
        ),
        (
            "compat/commands/set_mode.yolo.json".into(),
            json!({"type":"set_mode","mode":"yolo"}),
        ),
        (
            "compat/commands/tool_approve.always.json".into(),
            json!({"type":"tool_approve","call_id":"call-always","scope":"always"}),
        ),
        (
            "compat/commands/tool_approve.always-prefix.json".into(),
            json!({"type":"tool_approve","call_id":"call-prefix","scope":{"always_prefix":{"prefix":"cargo "}}}),
        ),
        (
            "compat/commands/tool_approve.minimal.json".into(),
            json!({"type":"tool_approve","call_id":"call-minimal"}),
        ),
        (
            "compat/commands/tool_deny.minimal.json".into(),
            json!({"type":"tool_deny","call_id":"call-deny-minimal"}),
        ),
    ])
}

/// Canonical events constructed through the real `ProtocolEvent` enum.
pub fn event_fixture_values() -> BTreeMap<String, ProtocolEvent> {
    use wcore_types::message::FinishReason;

    let usage = || Usage {
        input_tokens: 120,
        output_tokens: 40,
        cache_read_tokens: Some(16),
        cache_write_tokens: Some(8),
        active_window_percent: Some(37),
    };
    BTreeMap::from([
        (
            "events/approval_required.json".into(),
            ProtocolEvent::ApprovalRequired {
                call_id: "call-tool-001".into(),
                resume_token: "resume-001".into(),
                correlation_id: "resume-001".into(),
                reason: "Execution requires approval".into(),
                context: "Bash: cargo test".into(),
                plan: None,
            },
        ),
        (
            "events/approval_resume.json".into(),
            ProtocolEvent::ApprovalResume {
                resume_token: "resume-001".into(),
                approved: true,
            },
        ),
        (
            "events/browser_event.json".into(),
            ProtocolEvent::BrowserEvent {
                msg_id: "msg-001".into(),
                call_id: "call-browser-001".into(),
                op: "navigate".into(),
                url: Some("https://example.invalid/".into()),
                summary: "loaded".into(),
            },
        ),
        (
            "events/browser_policy_denied.json".into(),
            ProtocolEvent::BrowserPolicyDenied {
                msg_id: "msg-001".into(),
                url: "https://blocked.invalid/".into(),
                reason: "domain not allowed".into(),
            },
        ),
        (
            "events/budget_exceeded.json".into(),
            ProtocolEvent::BudgetExceeded {
                reason: "max_tokens_out".into(),
                observed: "8192".into(),
                limit: "4096".into(),
            },
        ),
        (
            "events/config_changed.json".into(),
            ProtocolEvent::ConfigChanged {
                capabilities: capabilities(),
            },
        ),
        (
            "events/cua_event.json".into(),
            ProtocolEvent::CuaEvent {
                msg_id: "msg-001".into(),
                call_id: "call-cua-001".into(),
                op: "left_click".into(),
                coords: Some([100, 200]),
                summary: "clicked at (100, 200)".into(),
            },
        ),
        (
            "events/cua_policy_denied.json".into(),
            ProtocolEvent::CuaPolicyDenied {
                msg_id: "msg-001".into(),
                op: "type".into(),
                app: "com.example.Editor".into(),
                reason: "application not allowed".into(),
            },
        ),
        (
            "events/error.json".into(),
            ProtocolEvent::Error {
                msg_id: Some("msg-001".into()),
                error: ErrorInfo {
                    code: "provider_error".into(),
                    message: "provider stream failed".into(),
                    retryable: true,
                },
            },
        ),
        (
            "events/evolution_event.json".into(),
            ProtocolEvent::EvolutionEvent {
                run_id: "evolve-run-001".into(),
                generation: 2,
                parent_id: "candidate-001".into(),
                child_id: "candidate-002".into(),
                mutation_kind: "paraphrase".into(),
                score: 0.875,
                retained: true,
            },
        ),
        (
            "events/host_send_message_request.json".into(),
            ProtocolEvent::HostSendMessageRequest {
                call_id: "call-send-001".into(),
                platform: "email".into(),
                chat_id: Some("operator@example.invalid".into()),
                thread_id: Some("thread-001".into()),
                body: "The run completed.".into(),
                subject: Some("Wayland update".into()),
                conversation_id: Some("session-desktop-001".into()),
            },
        ),
        (
            "events/info.json".into(),
            ProtocolEvent::Info {
                msg_id: "msg-001".into(),
                message: "Compaction completed".into(),
            },
        ),
        (
            "events/mcp_failed.json".into(),
            ProtocolEvent::McpFailed {
                name: "desktop-tools".into(),
                reason: "connection refused".into(),
            },
        ),
        (
            "events/mcp_ready.json".into(),
            ProtocolEvent::McpReady {
                name: "desktop-tools".into(),
                tools: vec!["search".into(), "fetch".into()],
            },
        ),
        (
            "events/plugin_event.json".into(),
            ProtocolEvent::PluginEvent {
                plugin_name: "wayland-example".into(),
                event_type: "index_ready".into(),
                payload: json!({"documents":3}),
            },
        ),
        (
            "events/plugin_registration_failed.json".into(),
            ProtocolEvent::PluginRegistrationFailed {
                plugin_name: "wayland-example".into(),
                surface: "tools".into(),
                error_kind: "access denied".into(),
                message: "tools permission was not granted".into(),
            },
        ),
        ("events/pong.json".into(), ProtocolEvent::Pong),
        (
            "events/provider_circuit_event.json".into(),
            ProtocolEvent::ProviderCircuitEvent {
                primary: "anthropic".into(),
                fallback: Some("openai".into()),
                state: "open".into(),
                error: Some("timeout".into()),
            },
        ),
        (
            "events/ready.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: Some("session-desktop-001".into()),
                capabilities: capabilities(),
            },
        ),
        (
            "events/session_cost.json".into(),
            ProtocolEvent::SessionCost {
                session_id: "session-desktop-001".into(),
                total_cost_usd: 0.0125,
                per_turn: vec![TurnCost {
                    turn: 1,
                    model: "claude-sonnet-4-5".into(),
                    provider: "anthropic".into(),
                    cost_usd: 0.0125,
                    priced: true,
                }],
            },
        ),
        (
            "events/stream_end.json".into(),
            ProtocolEvent::StreamEnd {
                msg_id: "msg-001".into(),
                finish_reason: FinishReason::Stop,
                usage: Some(usage()),
                usage_delta: Some(usage()),
                agent_run_id: Some("agent-run-001".into()),
            },
        ),
        (
            "events/stream_start.json".into(),
            ProtocolEvent::StreamStart {
                msg_id: "msg-001".into(),
            },
        ),
        (
            "events/sub_agent_event.json".into(),
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "call-spawn-001".into(),
                agent_name: "researcher".into(),
                inner: json!({"type":"text_delta","text":"child output","msg_id":"child-msg-001"}),
            },
        ),
        (
            "events/suspend.json".into(),
            ProtocolEvent::Suspend {
                reason: "Awaiting operator approval".into(),
                resume_token: "resume-001".into(),
            },
        ),
        (
            "events/text_delta.json".into(),
            ProtocolEvent::TextDelta {
                text: "Inspection complete.".into(),
                msg_id: "msg-001".into(),
            },
        ),
        (
            "events/thinking.json".into(),
            ProtocolEvent::Thinking {
                text: "Reviewing protocol state".into(),
                msg_id: "msg-001".into(),
                subject: Some("Protocol review".into()),
            },
        ),
        (
            "events/tool_cancelled.json".into(),
            ProtocolEvent::ToolCancelled {
                msg_id: "msg-001".into(),
                call_id: "call-tool-002".into(),
                reason: "Operator denied execution".into(),
            },
        ),
        (
            "events/tool_chunk.json".into(),
            ProtocolEvent::ToolChunk {
                msg_id: "msg-001".into(),
                call_id: "call-tool-001".into(),
                tool_name: "Bash".into(),
                chunk: "running tests\n".into(),
            },
        ),
        (
            "events/tool_panicked.json".into(),
            ProtocolEvent::ToolPanicked {
                msg_id: "msg-001".into(),
                call_id: "call-tool-003".into(),
                tool_name: "Example".into(),
                panic_message: "fixture panic".into(),
            },
        ),
        (
            "events/tool_request.json".into(),
            ProtocolEvent::ToolRequest {
                msg_id: "msg-001".into(),
                call_id: "call-tool-001".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command":"cargo test"}),
                    description: "Run the test suite".into(),
                },
            },
        ),
        (
            "events/tool_result.json".into(),
            ProtocolEvent::ToolResult {
                msg_id: "msg-001".into(),
                call_id: "call-tool-001".into(),
                tool_name: "Bash".into(),
                status: ToolStatus::Success,
                output: "tests passed".into(),
                output_type: OutputType::Text,
                metadata: Some(json!({"exit_code":0})),
            },
        ),
        (
            "events/tool_running.json".into(),
            ProtocolEvent::ToolRunning {
                msg_id: "msg-001".into(),
                call_id: "call-tool-001".into(),
                tool_name: "Bash".into(),
            },
        ),
        (
            "events/trace_event.json".into(),
            ProtocolEvent::TraceEvent {
                msg_id: "msg-001".into(),
                trace: json!({"span":"provider","duration_ms":42}),
            },
        ),
    ])
}

pub fn compatibility_event_values() -> BTreeMap<String, ProtocolEvent> {
    use wcore_types::message::FinishReason;
    BTreeMap::from([
        (
            "compat/events/approval_required.minimal.json".into(),
            ProtocolEvent::ApprovalRequired {
                call_id: "call-minimal".into(),
                resume_token: "resume-minimal".into(),
                correlation_id: String::new(),
                reason: "approval required".into(),
                context: "fixture".into(),
                plan: None,
            },
        ),
        (
            "compat/events/browser_event.minimal.json".into(),
            ProtocolEvent::BrowserEvent {
                msg_id: "msg-minimal".into(),
                call_id: "call-browser-minimal".into(),
                op: "snapshot".into(),
                url: None,
                summary: "captured".into(),
            },
        ),
        (
            "compat/events/cua_event.minimal.json".into(),
            ProtocolEvent::CuaEvent {
                msg_id: "msg-minimal".into(),
                call_id: "call-cua-minimal".into(),
                op: "screenshot".into(),
                coords: None,
                summary: "captured".into(),
            },
        ),
        (
            "compat/events/cua_policy_denied.minimal.json".into(),
            ProtocolEvent::CuaPolicyDenied {
                msg_id: "msg-minimal".into(),
                op: "type".into(),
                app: String::new(),
                reason: "blocked".into(),
            },
        ),
        (
            "compat/events/error.session.json".into(),
            ProtocolEvent::Error {
                msg_id: None,
                error: ErrorInfo {
                    code: "session_error".into(),
                    message: "session failed".into(),
                    retryable: false,
                },
            },
        ),
        (
            "compat/events/host_send_message_request.minimal.json".into(),
            ProtocolEvent::HostSendMessageRequest {
                call_id: "call-send-minimal".into(),
                platform: "slack".into(),
                chat_id: None,
                thread_id: None,
                body: "hello".into(),
                subject: None,
                conversation_id: None,
            },
        ),
        (
            "compat/events/provider_circuit_event.minimal.json".into(),
            ProtocolEvent::ProviderCircuitEvent {
                primary: "anthropic".into(),
                fallback: None,
                state: "closed".into(),
                error: None,
            },
        ),
        (
            "compat/events/ready.minimal.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: None,
                capabilities: Capabilities::default(),
            },
        ),
        (
            "compat/events/stream_end.minimal.json".into(),
            ProtocolEvent::StreamEnd {
                msg_id: "msg-minimal".into(),
                finish_reason: FinishReason::Stop,
                usage: None,
                usage_delta: None,
                agent_run_id: None,
            },
        ),
        (
            "compat/events/thinking.minimal.json".into(),
            ProtocolEvent::Thinking {
                text: "thinking".into(),
                msg_id: "msg-minimal".into(),
                subject: None,
            },
        ),
        (
            "compat/events/tool_result.minimal.json".into(),
            ProtocolEvent::ToolResult {
                msg_id: "msg-minimal".into(),
                call_id: "call-minimal".into(),
                tool_name: "Read".into(),
                status: ToolStatus::Success,
                output: "ok".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        ),
    ])
}
