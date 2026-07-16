use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::anvil::{
    ANVIL_DIGEST_ALGORITHM, ANVIL_RECEIPT_CONTRACT_VERSION, ANVIL_RECEIPT_ORIGIN,
    AnvilInvalidationReason, AnvilReceipt, AnvilReceiptInvalidation,
    anvil_invalidation_body_digest, anvil_receipt_body_digest,
};
use crate::diagnostics::{
    ConfigSourceDisposition, ConfigSourceRole, McpConnectionState, McpDeclarationOrigin,
    McpExecutableReadiness, McpExposureState, McpServerDiagnostic, McpTransportKind,
    McpWorkingDirectoryRole, RuntimeConfigSource, RuntimeDiagnosticsSnapshotV1,
    RuntimeDiagnosticsUnavailableReason, RuntimeEngineMode, RuntimeProcessBinding,
    RuntimeProfileBinding, RuntimeRemediationCode, RuntimeWorkspaceKind, UnsupportedConfigOverride,
};
use crate::events::{
    Capabilities, ErrorInfo, OperatorResolutionEvidence, OperatorResolutionEvidenceSource,
    OperatorToolEffectOutcome, OperatorToolEffectResolution, OutputType, ProtocolEvent,
    RecoveryBudgetSnapshot, RecoveryCursor, RecoveryLifecycle, RecoveryReconcileReason,
    RecoveryReplayItem, RecoveryReplayKind, RecoveryTurnSnapshot, RecoveryUnavailableReason,
    ToolCategory, ToolInfo, ToolStatus, TurnCost, Usage, WorkflowChildTerminalState,
    WorkflowNodeState, WorkflowTerminalState,
};
use crate::execution_policy::{ExecutionPolicyChangeReason, ExecutionPolicySequence};
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, EffectiveExecutionPolicy, PolicySource,
};

use super::fixtures_support::capabilities;

/// Normative authority classification for a producer wire variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractCriticality {
    Required,
    Safety,
    Observational,
}

/// One current Desktop-consumed wire variant.
#[derive(Debug, Clone, Copy)]
pub struct WireSpec {
    pub wire_type: &'static str,
    pub path: &'static str,
    pub required: &'static [&'static str],
    pub criticality: ContractCriticality,
    pub correlation: &'static str,
    pub capability: &'static str,
}

macro_rules! wire {
    ($wire:literal, $path:literal, [$($required:literal),*], $criticality:ident, $correlation:literal, $capability:literal) => {
        WireSpec {
            wire_type: $wire,
            path: $path,
            required: &["type", $($required),*],
            criticality: ContractCriticality::$criticality,
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
        Safety,
        "msg_id",
        "available"
    ),
    wire!(
        "stop",
        "commands/stop.json",
        [],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "tool_approve",
        "commands/tool_approve.json",
        ["call_id"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "tool_deny",
        "commands/tool_deny.json",
        ["call_id"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "approval_resume",
        "commands/approval_resume.json",
        ["resume_token", "approved"],
        Safety,
        "resume_token",
        "available"
    ),
    wire!(
        "init_history",
        "commands/init_history.json",
        ["text"],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "set_mode",
        "commands/set_mode.json",
        ["mode"],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "set_config",
        "commands/set_config.json",
        [],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "session_resync",
        "commands/session_resync.json",
        ["recovery_version", "request_id", "session_id"],
        Safety,
        "request_id_and_session_id",
        "turn_recovery_v1"
    ),
    wire!(
        "resume_turn",
        "commands/resume_turn.json",
        [
            "recovery_version",
            "request_id",
            "session_id",
            "turn_id",
            "cursor",
            "action"
        ],
        Safety,
        "request_id_and_cursor",
        "turn_recovery_v1"
    ),
    wire!(
        "resolve_interrupted_approval",
        "commands/resolve_interrupted_approval.json",
        [
            "recovery_version",
            "request_id",
            "session_id",
            "turn_id",
            "cursor",
            "approval_id",
            "decision"
        ],
        Safety,
        "request_id_cursor_and_approval_id",
        "turn_recovery_v1"
    ),
    wire!(
        "resolve_unknown_tool_effect",
        "commands/resolve_unknown_tool_effect.json",
        [
            "recovery_version",
            "session_id",
            "turn_id",
            "cursor",
            "tool_execution_id",
            "outcome",
            "operator_id",
            "evidence"
        ],
        Safety,
        "session_turn_tool_and_cursor",
        "operator_tool_effect_resolution_v1"
    ),
    wire!(
        "add_mcp_server",
        "commands/add_mcp_server.json",
        ["name", "transport"],
        Safety,
        "name",
        "available"
    ),
    wire!(
        "get_runtime_diagnostics",
        "commands/get_runtime_diagnostics.json",
        ["diagnostics_version", "request_id"],
        Safety,
        "request_id",
        "runtime_diagnostics_v1"
    ),
    wire!(
        "host_send_message_result",
        "commands/host_send_message_result.json",
        ["call_id", "ok"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "ping",
        "commands/ping.json",
        [],
        Observational,
        "connection",
        "available"
    ),
];

pub const EVENT_SPECS: &[WireSpec] = &[
    wire!(
        "ready",
        "events/ready.json",
        ["version", "capabilities", "contract", "execution_policy"],
        Required,
        "session_id",
        "available"
    ),
    wire!(
        "execution_policy",
        "events/execution_policy.json",
        [
            "critical",
            "contract_version",
            "revision",
            "reason",
            "effective_at_unix_ms",
            "policy"
        ],
        Safety,
        "revision",
        "effective_execution_policy_revisions"
    ),
    wire!(
        "session_recovery_snapshot",
        "events/session_recovery_snapshot.json",
        [
            "recovery_version",
            "request_id",
            "session_id",
            "cursor",
            "state_digest",
            "lifecycle",
            "budget"
        ],
        Safety,
        "request_id_and_cursor",
        "turn_recovery_v1"
    ),
    wire!(
        "session_recovery_replay",
        "events/session_recovery_replay.json",
        [
            "recovery_version",
            "request_id",
            "session_id",
            "through",
            "items"
        ],
        Safety,
        "request_id_and_cursor",
        "turn_recovery_v1"
    ),
    wire!(
        "session_recovery_unavailable",
        "events/session_recovery_unavailable.json",
        ["recovery_version", "request_id", "session_id", "reason"],
        Safety,
        "request_id_and_session_id",
        "turn_recovery_v1"
    ),
    wire!(
        "turn_recovery_lifecycle",
        "events/turn_recovery_lifecycle.json",
        [
            "recovery_version",
            "session_id",
            "turn_id",
            "cursor",
            "lifecycle"
        ],
        Safety,
        "turn_id_and_cursor",
        "turn_recovery_v1"
    ),
    wire!(
        "unknown_tool_effect_resolved",
        "events/unknown_tool_effect_resolved.json",
        [
            "recovery_version",
            "session_id",
            "turn_id",
            "cursor",
            "tool_execution_id",
            "outcome",
            "operator_id",
            "evidence"
        ],
        Safety,
        "session_turn_tool_and_cursor",
        "operator_tool_effect_resolution_v1"
    ),
    wire!(
        "stream_start",
        "events/stream_start.json",
        ["msg_id"],
        Observational,
        "msg_id",
        "available"
    ),
    wire!(
        "text_delta",
        "events/text_delta.json",
        ["text", "msg_id"],
        Observational,
        "msg_id",
        "available"
    ),
    wire!(
        "thinking",
        "events/thinking.json",
        ["text", "msg_id"],
        Observational,
        "msg_id",
        "available"
    ),
    wire!(
        "tool_request",
        "events/tool_request.json",
        ["msg_id", "call_id", "tool"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "tool_running",
        "events/tool_running.json",
        ["msg_id", "call_id", "tool_name"],
        Observational,
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
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "tool_cancelled",
        "events/tool_cancelled.json",
        ["msg_id", "call_id", "reason"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "stream_end",
        "events/stream_end.json",
        ["msg_id", "finish_reason"],
        Safety,
        "msg_id",
        "available"
    ),
    wire!(
        "error",
        "events/error.json",
        ["error"],
        Safety,
        "msg_id_or_session",
        "available"
    ),
    wire!(
        "info",
        "events/info.json",
        ["msg_id", "message"],
        Observational,
        "msg_id",
        "available"
    ),
    wire!(
        "config_changed",
        "events/config_changed.json",
        ["capabilities"],
        Observational,
        "session",
        "available"
    ),
    wire!(
        "mcp_ready",
        "events/mcp_ready.json",
        ["name", "tools"],
        Observational,
        "name",
        "available"
    ),
    wire!(
        "mcp_failed",
        "events/mcp_failed.json",
        ["name", "reason"],
        Safety,
        "name",
        "available"
    ),
    wire!(
        "runtime_diagnostics_snapshot",
        "events/runtime_diagnostics_snapshot.json",
        ["diagnostics_version", "request_id", "snapshot"],
        Safety,
        "request_id",
        "runtime_diagnostics_v1"
    ),
    wire!(
        "runtime_diagnostics_unavailable",
        "events/runtime_diagnostics_unavailable.json",
        [
            "diagnostics_version",
            "supported_version",
            "request_id",
            "reason"
        ],
        Safety,
        "request_id",
        "runtime_diagnostics_v1"
    ),
    wire!(
        "pong",
        "events/pong.json",
        [],
        Observational,
        "connection",
        "available"
    ),
    wire!(
        "trace_event",
        "events/trace_event.json",
        ["msg_id", "trace"],
        Observational,
        "msg_id",
        "structured_traces"
    ),
    wire!(
        "session_cost",
        "events/session_cost.json",
        ["session_id", "total_cost_usd", "per_turn"],
        Observational,
        "session_id",
        "cost_attribution"
    ),
    wire!(
        "sub_agent_event",
        "events/sub_agent_event.json",
        [
            "parent_call_id",
            "agent_name",
            "inner",
            "run_id",
            "child_run_id",
            "child_sequence",
            "event_id"
        ],
        Safety,
        "child_run_id_and_child_sequence",
        "workflow_lifecycle_v1"
    ),
    wire!(
        "workflow_started",
        "events/workflow_started.json",
        [
            "workflow_id",
            "name",
            "node_count",
            "run_id",
            "event_id",
            "sequence"
        ],
        Safety,
        "run_id_and_sequence",
        "workflow_lifecycle_v1"
    ),
    wire!(
        "workflow_node_event",
        "events/workflow_node_event.json",
        ["run_id", "node_id", "event_id", "sequence", "state"],
        Safety,
        "run_id_and_sequence",
        "workflow_lifecycle_v1"
    ),
    wire!(
        "workflow_finished",
        "events/workflow_finished.json",
        [
            "workflow_id",
            "succeeded",
            "run_id",
            "event_id",
            "sequence",
            "terminal_state"
        ],
        Safety,
        "run_id_and_sequence",
        "workflow_lifecycle_v1"
    ),
    wire!(
        "tool_chunk",
        "events/tool_chunk.json",
        ["msg_id", "call_id", "tool_name", "chunk"],
        Observational,
        "call_id",
        "streaming_tools"
    ),
    wire!(
        "provider_circuit_event",
        "events/provider_circuit_event.json",
        ["primary", "state"],
        Safety,
        "primary",
        "available"
    ),
    wire!(
        "provider_failover_receipt",
        "events/provider_failover_receipt.json",
        ["receipt"],
        Safety,
        "failed_provider_and_selected_provider",
        "semantic_failover_receipts"
    ),
    wire!(
        "approval_required",
        "events/approval_required.json",
        ["call_id", "resume_token", "reason", "context"],
        Safety,
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "suspend",
        "events/suspend.json",
        ["reason", "resume_token"],
        Safety,
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "approval_resume",
        "events/approval_resume.json",
        ["resume_token", "approved"],
        Safety,
        "resume_token",
        "hitl_suspend"
    ),
    wire!(
        "budget_exceeded",
        "events/budget_exceeded.json",
        ["reason", "observed", "limit"],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "tool_panicked",
        "events/tool_panicked.json",
        ["msg_id", "call_id", "tool_name", "panic_message"],
        Safety,
        "call_id",
        "available"
    ),
    wire!(
        "plugin_registration_failed",
        "events/plugin_registration_failed.json",
        ["plugin_name", "surface", "error_kind", "message"],
        Safety,
        "plugin_name_and_surface",
        "available"
    ),
    wire!(
        "plugin_event",
        "events/plugin_event.json",
        ["plugin_name", "event_type", "payload"],
        Observational,
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
        Observational,
        "run_id",
        "gepa_enabled"
    ),
    wire!(
        "browser_event",
        "events/browser_event.json",
        ["msg_id", "call_id", "op", "summary"],
        Observational,
        "call_id",
        "shape_only"
    ),
    wire!(
        "browser_policy_denied",
        "events/browser_policy_denied.json",
        ["msg_id", "url", "reason"],
        Safety,
        "msg_id",
        "shape_only"
    ),
    wire!(
        "cua_event",
        "events/cua_event.json",
        ["msg_id", "call_id", "op", "summary"],
        Observational,
        "call_id",
        "shape_only"
    ),
    wire!(
        "cua_policy_denied",
        "events/cua_policy_denied.json",
        ["msg_id", "op", "reason"],
        Safety,
        "msg_id",
        "shape_only"
    ),
    wire!(
        "host_send_message_request",
        "events/host_send_message_request.json",
        ["call_id", "platform", "body"],
        Safety,
        "call_id",
        "host_delegated_delivery"
    ),
    wire!(
        "anvil_receipt",
        "events/anvil_receipt.json",
        [
            "receipt_id",
            "event_id",
            "origin",
            "contract_version",
            "session_id",
            "run_id",
            "task_id",
            "sequence",
            "artifact_digest",
            "gate_closure_digest",
            "receipt_body_digest"
        ],
        Safety,
        "session_id_and_sequence",
        "anvil_receipts"
    ),
    wire!(
        "anvil_receipt_invalidated",
        "events/anvil_receipt_invalidated.json",
        [
            "receipt_id",
            "event_id",
            "origin",
            "contract_version",
            "session_id",
            "run_id",
            "task_id",
            "sequence",
            "reason",
            "prior_artifact_digest",
            "invalidation_body_digest"
        ],
        Safety,
        "session_id_and_sequence",
        "anvil_receipts"
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
    "session_resync",
    "resume_turn",
    "resolve_interrupted_approval",
    "resolve_unknown_tool_effect",
    "get_runtime_diagnostics",
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
    "session_recovery_snapshot",
    "session_recovery_replay",
    "session_recovery_unavailable",
    "turn_recovery_lifecycle",
    "unknown_tool_effect_resolved",
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
    "runtime_diagnostics_snapshot",
    "runtime_diagnostics_unavailable",
    "trace_event",
    "session_cost",
    "sub_agent_event",
    "workflow_started",
    "workflow_node_event",
    "workflow_finished",
    "tool_chunk",
    "provider_circuit_event",
    "provider_failover_receipt",
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
    "anvil_receipt_invalidated",
    "pong",
];

pub const SOURCE_INPUTS: &[&str] = &[
    "crates/wcore-protocol/src/commands.rs",
    "crates/wcore-protocol/src/events.rs",
    "crates/wcore-protocol/src/diagnostics.rs",
    "crates/wcore-protocol/src/reader.rs",
    "crates/wcore-protocol/src/writer.rs",
    "crates/wcore-protocol/src/anvil.rs",
    "crates/wcore-protocol/src/execution_policy.rs",
    "crates/wcore-protocol/src/workflow.rs",
    "crates/wcore-protocol/src/contract/mod.rs",
    "crates/wcore-protocol/src/contract/canonical.rs",
    "crates/wcore-protocol/src/contract/spec.rs",
    "crates/wcore-protocol/src/contract/generate.rs",
    "crates/wcore-protocol/src/contract/observation.rs",
    "crates/wcore-protocol/src/contract/check.rs",
    "crates/wcore-protocol/src/bin/wcore-contract.rs",
    "crates/wcore-types/src/execution_policy.rs",
    "crates/wcore-types/src/workspace_trust.rs",
    "crates/wcore-agent/src/output/protocol_sink.rs",
    "crates/wcore-agent/src/bootstrap.rs",
    "crates/wcore-agent/src/plugins/loader.rs",
    "crates/wcore-agent/src/plugins/mcp_delivery.rs",
    "crates/wcore-agent/src/orchestration/workflow/runner.rs",
    "crates/wcore-agent/src/orchestration/anvil/forge.rs",
    "crates/wcore-cli/src/main.rs",
    "crates/wcore-cli/src/packaged_runtime.rs",
    "crates/wcore-cli/src/runtime_diagnostics.rs",
    "crates/wcore-config/src/shell/executable_readiness.rs",
    "crates/wcore-config/src/shell/mcp_stdio_launch_context.rs",
    "crates/wcore-mcp/src/transport/stdio.rs",
    "crates/wcore-mcp/src/transport/stdio_readiness.rs",
    "crates/wcore-mcp/src/manager.rs",
    "crates/wcore-plugin-subprocess/src/mcp_bridge.rs",
    "crates/wcore-tools/src/registry.rs",
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
            "commands/get_runtime_diagnostics.json".into(),
            json!({"type":"get_runtime_diagnostics","diagnostics_version":1,"request_id":"runtime-diagnostics-001"}),
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
        (
            "commands/session_resync.json".into(),
            json!({"type":"session_resync","recovery_version":1,"request_id":"recovery-request-001","session_id":"session-desktop-001","after":{"journal_sequence":40,"journal_digest":journal_digest('4')}}),
        ),
        (
            "commands/resume_turn.json".into(),
            json!({"type":"resume_turn","recovery_version":1,"request_id":"recovery-request-002","session_id":"session-desktop-001","turn_id":"turn-002","cursor":{"journal_sequence":42,"journal_digest":journal_digest('6')},"action":"reconcile"}),
        ),
        (
            "commands/resolve_interrupted_approval.json".into(),
            json!({"type":"resolve_interrupted_approval","recovery_version":1,"request_id":"recovery-request-003","session_id":"session-desktop-001","turn_id":"turn-002","cursor":{"journal_sequence":42,"journal_digest":journal_digest('6')},"approval_id":"approval-002","decision":"approve","answer":"Proceed"}),
        ),
        (
            "commands/resolve_unknown_tool_effect.json".into(),
            serde_json::to_value(operator_tool_effect_resolution())
                .expect("operator-resolution command fixture must serialize")
                .as_object()
                .map(|fields| {
                    let mut command = fields.clone();
                    command.insert(
                        "type".into(),
                        Value::String("resolve_unknown_tool_effect".into()),
                    );
                    Value::Object(command)
                })
                .expect("operator-resolution command fixture must be an object"),
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
            "compat/commands/session_resync.genesis.json".into(),
            json!({"type":"session_resync","recovery_version":1,"request_id":"recovery-request-genesis","session_id":"session-desktop-001"}),
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

fn execution_policy_sequence() -> (
    crate::execution_policy::ExecutionPolicySnapshot,
    crate::execution_policy::ExecutionPolicySnapshot,
) {
    let launch = EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
        ApprovalPolicy::Prompt,
        PolicySource::DesktopLocalLaunch,
    ));
    let mut sequence = ExecutionPolicySequence::launch(launch, 1_721_000_000_000);
    let initial = sequence.current().clone();
    let auto_edit = EffectiveExecutionPolicy::baseline(&BaselineExecutionPolicy::smart(
        ApprovalPolicy::AutoEdit,
        PolicySource::Protocol,
    ));
    let changed = sequence
        .advance_if_changed(
            auto_edit,
            ExecutionPolicyChangeReason::ModeChange,
            1_721_000_000_100,
        )
        .expect("fixture revision cannot overflow")
        .expect("fixture policy must change")
        .clone();
    (initial, changed)
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn journal_digest(byte: char) -> String {
    byte.to_string().repeat(64)
}

fn recovery_cursor(sequence: Option<u64>, byte: char) -> RecoveryCursor {
    RecoveryCursor {
        journal_sequence: sequence,
        journal_digest: journal_digest(byte),
    }
}

fn operator_tool_effect_resolution() -> OperatorToolEffectResolution {
    OperatorToolEffectResolution {
        recovery_version: 1,
        session_id: "session-desktop-001".into(),
        turn_id: "turn-002".into(),
        cursor: RecoveryCursor {
            journal_sequence: Some(42),
            journal_digest: journal_digest('6'),
        },
        tool_execution_id: "tool-execution-002".into(),
        outcome: OperatorToolEffectOutcome::Succeeded,
        operator_id: "operator-desktop-001".into(),
        evidence: OperatorResolutionEvidence {
            source: OperatorResolutionEvidenceSource::ExternalSystemRecord,
            reference_id: "external-record-002".into(),
            observed_at_unix_ms: 1_721_000_003_000,
            digest: digest('7'),
        },
    }
}

pub(super) fn anvil_receipt() -> AnvilReceipt {
    let mut receipt = AnvilReceipt {
        receipt_id: "receipt-desktop-001".into(),
        event_id: "anvil-event-000".into(),
        origin: ANVIL_RECEIPT_ORIGIN.into(),
        contract_version: ANVIL_RECEIPT_CONTRACT_VERSION.into(),
        required_extensions: Vec::new(),
        session_id: "session-desktop-001".into(),
        run_id: "anvil-run-001".into(),
        task_id: "task-desktop-001".into(),
        sequence: 0,
        issued_at_unix_ms: 1_721_000_001_000,
        digest_algorithm: ANVIL_DIGEST_ALGORITHM.into(),
        artifact_scope: "git:tracked+untracked-excluding-ignored@.".into(),
        artifact_digest: digest('a'),
        gate_closure_digest: digest('b'),
        receipt_body_digest: String::new(),
        supersedes_receipt_id: None,
        terminal_state: "verified".into(),
        stamp: "verified".into(),
        checks_passed: 14,
        checks_total: 14,
        coverage: Some("line:87.5%".into()),
        iterations: 3,
        valve_fires: 1,
        cost_microcents: 7_000,
        priced: true,
        engine_version: "0.12.25".into(),
    };
    receipt.receipt_body_digest =
        anvil_receipt_body_digest(&receipt).expect("canonical receipt fixture must serialize");
    receipt
}

pub(super) fn anvil_invalidation() -> AnvilReceiptInvalidation {
    let mut invalidation = AnvilReceiptInvalidation {
        event_id: "anvil-event-001".into(),
        origin: ANVIL_RECEIPT_ORIGIN.into(),
        contract_version: ANVIL_RECEIPT_CONTRACT_VERSION.into(),
        required_extensions: Vec::new(),
        receipt_id: "receipt-desktop-001".into(),
        session_id: "session-desktop-001".into(),
        run_id: "anvil-run-001".into(),
        task_id: "task-desktop-001".into(),
        sequence: 1,
        issued_at_unix_ms: 1_721_000_002_000,
        reason: AnvilInvalidationReason::ArtifactMutated,
        prior_artifact_digest: digest('a'),
        observed_artifact_digest: Some(digest('c')),
        invalidation_body_digest: String::new(),
    };
    invalidation.invalidation_body_digest = anvil_invalidation_body_digest(&invalidation)
        .expect("canonical invalidation fixture must serialize");
    invalidation
}

pub(super) fn workflow_lifecycle_events() -> Vec<ProtocolEvent> {
    vec![
        ProtocolEvent::CorrelatedWorkflowStarted {
            workflow_id: "desktop-audit".into(),
            name: "Desktop audit".into(),
            node_count: 1,
            run_id: "workflow-run-001".into(),
            event_id: "workflow-event-000".into(),
            sequence: 0,
            parent_run_id: None,
        },
        ProtocolEvent::WorkflowNodeEvent {
            run_id: "workflow-run-001".into(),
            node_id: "scan".into(),
            child_run_id: Some("child-run-001".into()),
            event_id: "workflow-event-001".into(),
            sequence: 1,
            state: WorkflowNodeState::Queued,
            failure: None,
        },
        ProtocolEvent::WorkflowNodeEvent {
            run_id: "workflow-run-001".into(),
            node_id: "scan".into(),
            child_run_id: Some("child-run-001".into()),
            event_id: "workflow-event-002".into(),
            sequence: 2,
            state: WorkflowNodeState::Running,
            failure: None,
        },
        ProtocolEvent::CorrelatedSubAgentEvent {
            parent_call_id: "workflow:scan".into(),
            agent_name: "scan".into(),
            inner: json!({"type":"text_delta","text":"scan complete","msg_id":"child-msg-001"}),
            run_id: "workflow-run-001".into(),
            child_run_id: "child-run-001".into(),
            parent_child_run_id: None,
            child_sequence: 0,
            event_id: "child-event-000".into(),
            terminal_state: None,
        },
        ProtocolEvent::CorrelatedSubAgentEvent {
            parent_call_id: "workflow:scan".into(),
            agent_name: "scan".into(),
            inner: json!({
                "type": "info",
                "msg_id": "child-msg-terminal-001",
                "message": "Sub-agent 'scan' completed successfully"
            }),
            run_id: "workflow-run-001".into(),
            child_run_id: "child-run-001".into(),
            parent_child_run_id: None,
            child_sequence: 1,
            event_id: "child-event-001".into(),
            terminal_state: Some(WorkflowChildTerminalState::Succeeded),
        },
        ProtocolEvent::WorkflowNodeEvent {
            run_id: "workflow-run-001".into(),
            node_id: "scan".into(),
            child_run_id: Some("child-run-001".into()),
            event_id: "workflow-event-003".into(),
            sequence: 3,
            state: WorkflowNodeState::Succeeded,
            failure: None,
        },
        ProtocolEvent::CorrelatedWorkflowFinished {
            workflow_id: "desktop-audit".into(),
            succeeded: true,
            run_id: "workflow-run-001".into(),
            event_id: "workflow-event-004".into(),
            sequence: 4,
            terminal_state: WorkflowTerminalState::Succeeded,
            failure: None,
        },
    ]
}

/// Canonical events constructed through the real `ProtocolEvent` enum.
pub fn event_fixture_values() -> BTreeMap<String, ProtocolEvent> {
    use wcore_types::message::FinishReason;

    let (initial_policy, changed_policy) = execution_policy_sequence();
    let workflow = workflow_lifecycle_events();

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
            "events/execution_policy.json".into(),
            ProtocolEvent::ExecutionPolicy {
                snapshot: changed_policy,
            },
        ),
        (
            "events/session_recovery_snapshot.json".into(),
            ProtocolEvent::SessionRecoverySnapshot {
                recovery_version: 1,
                request_id: "recovery-request-001".into(),
                session_id: "session-desktop-001".into(),
                cursor: recovery_cursor(Some(40), '4'),
                state_digest: journal_digest('a'),
                lifecycle: RecoveryLifecycle::ReconciliationRequired,
                pending_turn: Some(RecoveryTurnSnapshot {
                    turn_id: "turn-002".into(),
                    msg_id: Some("msg-002".into()),
                    lifecycle: RecoveryLifecycle::ReconciliationRequired,
                    pending_call_id: Some("call-tool-002".into()),
                    reconcile_reason: Some(RecoveryReconcileReason::ToolOutcomeUnknown),
                }),
                budget: RecoveryBudgetSnapshot {
                    tokens_used: 12_000,
                    token_limit: Some(20_000),
                    cost_used_usd: 1.25,
                    cost_limit_usd: Some(5.0),
                },
            },
        ),
        (
            "events/session_recovery_replay.json".into(),
            ProtocolEvent::SessionRecoveryReplay {
                recovery_version: 1,
                request_id: "recovery-request-001".into(),
                session_id: "session-desktop-001".into(),
                from: Some(recovery_cursor(Some(40), '4')),
                through: recovery_cursor(Some(42), '6'),
                items: vec![
                    RecoveryReplayItem {
                        cursor: recovery_cursor(Some(41), '5'),
                        turn_id: Some("turn-002".into()),
                        kind: RecoveryReplayKind::ToolStarted,
                    },
                    RecoveryReplayItem {
                        cursor: recovery_cursor(Some(42), '6'),
                        turn_id: Some("turn-002".into()),
                        kind: RecoveryReplayKind::EffectUncertain,
                    },
                ],
            },
        ),
        (
            "events/session_recovery_unavailable.json".into(),
            ProtocolEvent::SessionRecoveryUnavailable {
                recovery_version: 1,
                request_id: "recovery-request-003".into(),
                session_id: "session-desktop-001".into(),
                reason: RecoveryUnavailableReason::CursorDigestMismatch,
            },
        ),
        (
            "events/turn_recovery_lifecycle.json".into(),
            ProtocolEvent::TurnRecoveryLifecycle {
                recovery_version: 1,
                session_id: "session-desktop-001".into(),
                turn_id: "turn-002".into(),
                cursor: recovery_cursor(Some(42), '6'),
                lifecycle: RecoveryLifecycle::ReconciliationRequired,
                reconcile_reason: Some(RecoveryReconcileReason::ToolOutcomeUnknown),
            },
        ),
        (
            "events/unknown_tool_effect_resolved.json".into(),
            ProtocolEvent::UnknownToolEffectResolved {
                resolution: operator_tool_effect_resolution(),
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
            "events/runtime_diagnostics_snapshot.json".into(),
            ProtocolEvent::RuntimeDiagnosticsSnapshot {
                diagnostics_version: 1,
                request_id: "runtime-diagnostics-001".into(),
                snapshot: RuntimeDiagnosticsSnapshotV1 {
                    process: RuntimeProcessBinding {
                        profile_binding: RuntimeProfileBinding::BoundProfile,
                        profile_name: Some("desktop".into()),
                        engine_mode: RuntimeEngineMode::Standard,
                        workspace_kind: RuntimeWorkspaceKind::Temporary,
                    },
                    config_sources: vec![RuntimeConfigSource {
                        role: ConfigSourceRole::Global,
                        disposition: ConfigSourceDisposition::Loaded,
                        precedence: 10,
                        display_path: Some("$CONFIG/wayland-core/config.toml".into()),
                        content_digest: Some(digest('d')),
                    }],
                    unsupported_overrides: vec![UnsupportedConfigOverride {
                        name: "WAYLAND_CONFIG_PATH".into(),
                        disposition: ConfigSourceDisposition::Ignored,
                    }],
                    mcp_servers: vec![McpServerDiagnostic {
                        name: "desktop-tools".into(),
                        origin: McpDeclarationOrigin::GlobalConfig,
                        transport: McpTransportKind::Stdio,
                        connection: McpConnectionState::Ready,
                        exposure: McpExposureState::Exposed,
                        deferred: false,
                        tool_count: 2,
                        resources_declared: false,
                        resources_exposed: false,
                        assistant_scoped: true,
                        executable_basename: Some("desktop-mcp".into()),
                        executable_readiness: McpExecutableReadiness::Resolved,
                        working_directory: McpWorkingDirectoryRole::ProjectRoot,
                        failure: None,
                        remediation: vec![RuntimeRemediationCode::OpenActiveConfig],
                    }],
                },
            },
        ),
        (
            "events/runtime_diagnostics_unavailable.json".into(),
            ProtocolEvent::RuntimeDiagnosticsUnavailable {
                diagnostics_version: 2,
                supported_version: 1,
                request_id: "runtime-diagnostics-unsupported".into(),
                reason: RuntimeDiagnosticsUnavailableReason::UnsupportedVersion,
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
            "events/provider_failover_receipt.json".into(),
            ProtocolEvent::ProviderFailoverReceipt {
                receipt: json!({
                    "reason": "rate_limit",
                    "failed_provider": "anthropic",
                    "failed_model": "claude-sonnet-4-6",
                    "candidates": [{
                        "provider": "openai",
                        "model": "gpt-5",
                        "region": "us-east",
                        "disposition": {"Ok": null},
                        "failure_reason": null,
                        "cooldown_reason": null,
                        "retry_after_ms": null,
                        "pricing": {
                            "source": "bundled",
                            "age_seconds": null,
                            "stale": false,
                            "priced": true,
                            "estimated_microcents": 77
                        }
                    }],
                    "selected_provider": "openai",
                    "selected_model": "gpt-5"
                }),
            },
        ),
        (
            "events/ready.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: Some("session-desktop-001".into()),
                capabilities: capabilities(),
                contract: None,
                execution_policy: Some(initial_policy),
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
        ("events/sub_agent_event.json".into(), workflow[3].clone()),
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
        ("events/workflow_finished.json".into(), workflow[6].clone()),
        (
            "events/workflow_node_event.json".into(),
            workflow[2].clone(),
        ),
        ("events/workflow_started.json".into(), workflow[0].clone()),
        (
            "events/anvil_receipt.json".into(),
            ProtocolEvent::AnvilReceipt {
                receipt: anvil_receipt(),
            },
        ),
        (
            "events/anvil_receipt_invalidated.json".into(),
            ProtocolEvent::AnvilReceiptInvalidated {
                invalidation: anvil_invalidation(),
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
                contract: None,
                execution_policy: None,
            },
        ),
        (
            "compat/events/sub_agent_event.legacy.json".into(),
            ProtocolEvent::SubAgentEvent {
                parent_call_id: "call-spawn-legacy".into(),
                agent_name: "legacy-child".into(),
                inner: json!({"type":"text_delta","text":"legacy child output","msg_id":"child-msg-legacy"}),
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
