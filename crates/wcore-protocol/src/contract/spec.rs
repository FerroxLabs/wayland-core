use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::anvil::{
    ANVIL_DIGEST_ALGORITHM, ANVIL_RECEIPT_CONTRACT_VERSION, ANVIL_RECEIPT_ORIGIN,
    AnvilInvalidationReason, AnvilReceipt, AnvilReceiptInvalidation,
    anvil_invalidation_body_digest, anvil_receipt_body_digest,
};
use crate::child::{
    ChildDeliveryState, ChildDeliveryTarget, ChildDesiredState, ChildId, ChildOrigin, ChildParent,
    ChildPolicySnapshot, ChildRecoveryState, ChildRequestEvidence, ChildTimestamps, ChildWorkspace,
    ChildWorkspaceMode, DURABLE_CHILD_PROTOCOL_VERSION, DurableChildRecord, DurableChildResult,
    DurableChildStatus, DurableChildTransition,
};
use crate::diagnostics::{
    ConfigSourceDisposition, ConfigSourceRole, McpConnectionState, McpDeclarationOrigin,
    McpExecutableReadiness, McpExposureState, McpServerDiagnostic, McpTransportKind,
    McpWorkingDirectoryRole, RuntimeConfigSource, RuntimeDiagnosticsSnapshotV1,
    RuntimeDiagnosticsUnavailableReason, RuntimeEngineMode, RuntimeProcessBinding,
    RuntimeProfileBinding, RuntimeRemediationCode, RuntimeWorkspaceKind, UnsupportedConfigOverride,
};
use crate::events::{
    BudgetGrantRefusalReason, BudgetGrantResult, Capabilities, CapabilityActivation, CapabilityId,
    CapabilityReasonCode, ErrorInfo, MonitorDirective, MonitorReason, NonCritical,
    OperatorResolutionEvidence, OperatorResolutionEvidenceSource, OperatorToolEffectOutcome,
    OperatorToolEffectResolution, OutputType, ProtocolEvent, RecoveryBudgetSnapshot,
    RecoveryCursor, RecoveryLifecycle, RecoveryReconcileReason, RecoveryReplayItem,
    RecoveryReplayKind, RecoveryTurnSnapshot, RecoveryUnavailableReason, RenderMime,
    SessionPersistence, ToolCategory, ToolInfo, ToolStatus, TurnCost, Usage,
    WorkflowChildTerminalState, WorkflowNodeState, WorkflowTerminalState,
};
use crate::execution_policy::{ExecutionPolicyChangeReason, ExecutionPolicySequence};
use crate::goal::{
    GOAL_PROTOCOL_VERSION, GoalAuthorityWire, GoalLifecycleWire, GoalLoopOwnerWire, GoalProjection,
    GoalTaskWire, GoalTaskWireStatus, GoalTransitionKind,
};
use wcore_types::execution_policy::{
    ApprovalPolicy, BaselineExecutionPolicy, EffectiveExecutionPolicy, PolicySource,
};
use wcore_types::goal::{GoalStrategy, GoalTerminalState, LoopPolicy};
use wcore_types::workspace_trust::{
    AuthoritySource, DeveloperCapability, WorkspacePolicyReceipt, WorkspaceSandboxProfile,
    WorkspaceTrustInput, resolve_workspace_trust,
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
        "continue_with_budget",
        "commands/continue_with_budget.json",
        ["request_id"],
        Safety,
        "request_id",
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
        "remove_mcp_server",
        "commands/remove_mcp_server.json",
        ["lifecycle_version", "request_id", "name"],
        Safety,
        "request_id",
        "runtime_mcp_lifecycle_v1"
    ),
    // #314 - the three host-initiated authority grants. Each has been
    // dispatched in `crates/wcore-cli/src/main.rs` since 0.13.6 and named in
    // `PRODUCER_COMMAND_TYPES`; none was declared here, so the command union a
    // Desktop host derives its emitter or its conformance check from did not
    // contain them, and a corpus-driven host could not send one at all.
    //
    // `capability: "available"`, not `path_grants_v1`: that capability is the
    // feature-detect for the `always_path` APPROVAL SCOPE - a different promise
    // on a different frame, which is exactly why `path_boundary_prompt_v1` sits
    // beside it rather than inside it. These commands are always accepted and
    // always answered on this contract version; the launcher opt-in decides
    // whether the answer is a grant receipt or a legible refusal, which is the
    // same shape `set_mode` already publishes as "available" for its `--force`
    // gate.
    //
    // `Safety`, not `Observational`: each mutates the workspace authority that
    // the OS sandbox and the in-process file tools both read.
    wire!(
        "grant_workspace_capability",
        "commands/grant_workspace_capability.json",
        ["executable"],
        Safety,
        "session",
        "available"
    ),
    wire!(
        "grant_path",
        "commands/grant_path.json",
        ["grant_id", "root"],
        Safety,
        "grant_id",
        "available"
    ),
    wire!(
        "revoke_path",
        "commands/revoke_path.json",
        ["grant_id"],
        Safety,
        "grant_id",
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
    // F22-C1 — host CONTROL of a durable Goal. Every one of these is answered
    // in the CLI command loop; none is a shape with no dispatcher.
    // `Safety`, not `Observational`: each mutates a durable authority-bearing
    // chain, which is the same class `resume_turn` is graded at.
    wire!(
        "goal_open",
        "commands/goal_open.json",
        [
            "goal_version",
            "request_id",
            "session_id",
            "goal_id",
            "objective",
            "iterations",
            "strategy",
            "max_tokens"
        ],
        Safety,
        "request_id_and_goal_id",
        "durable_goals_v1"
    ),
    wire!(
        "goal_declare_task",
        "commands/goal_declare_task.json",
        [
            "goal_version",
            "request_id",
            "session_id",
            "goal_id",
            "task_id"
        ],
        Safety,
        "request_id_and_goal_id",
        "durable_goals_v1"
    ),
    wire!(
        "goal_advance",
        "commands/goal_advance.json",
        [
            "goal_version",
            "request_id",
            "session_id",
            "goal_id",
            "cursor"
        ],
        Safety,
        "request_id_goal_id_and_cursor",
        "durable_goals_v1"
    ),
    wire!(
        "goal_cancel",
        "commands/goal_cancel.json",
        [
            "goal_version",
            "request_id",
            "session_id",
            "goal_id",
            "cursor"
        ],
        Safety,
        "request_id_goal_id_and_cursor",
        "durable_goals_v1"
    ),
    wire!(
        "goal_resync",
        "commands/goal_resync.json",
        ["goal_version", "request_id", "session_id"],
        Safety,
        "request_id_and_goal_id",
        "durable_goals_v1"
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
    // `session_id` and `session_persistence` are BOTH required. `session_id` is
    // this variant's declared correlation key, and a correlation key that the
    // producer may drop is indistinguishable at the host from a malformed
    // frame — so it is always on the wire, `null` when there is no session, and
    // `session_persistence` (never null) says which cause produced the null.
    wire!(
        "ready",
        "events/ready.json",
        [
            "version",
            "session_id",
            "session_persistence",
            "capabilities",
            "contract",
            "execution_policy"
        ],
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
    // Emitted once per session by `wcore-cli/src/main.rs`, immediately after
    // `ready`. Output-only: it reports the trust and sandbox authority the
    // session actually resolved, and a host cannot submit this shape to widen
    // anything. Safety, because a host that renders "trusted workspace" from a
    // frame it never validated is mis-stating the security posture.
    wire!(
        "workspace_policy",
        "events/workspace_policy.json",
        ["policy"],
        Safety,
        "workspace_fingerprint",
        "available"
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
    // Flattened `CapabilityActivation`. Startup claims arrive after `ready`;
    // runtime claims arrive at the real success seam. `reason` is present only
    // on `unavailable` (see `CapabilityActivation::is_well_formed`), so it is
    // modelled but not required.
    wire!(
        "capability_activation",
        "events/capability_activation.json",
        ["capability", "stage"],
        Observational,
        "capability",
        "available"
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
    // Sibling of `tool_request` for a call that runs WITHOUT being asked
    // about. Same payload shape on purpose: a host registers the `call_id`
    // from either frame, and must be able to render the same card. Classified
    // `Safety` rather than `Observational` because it is the only wire record
    // that a tool executed with no operator prompt.
    wire!(
        "call_announced",
        "events/call_announced.json",
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
        "mcp_removal_result",
        "events/mcp_removal_result.json",
        [
            "lifecycle_version",
            "request_id",
            "name",
            "outcome",
            "removed_tools"
        ],
        Safety,
        "request_id",
        "runtime_mcp_lifecycle_v1"
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
    // One physical provider request attempt. `failure` is absent on a clean
    // attempt, so nothing beyond the discriminator is required.
    wire!(
        "provider_attempt",
        "events/provider_attempt.json",
        [],
        Observational,
        "session",
        "available"
    ),
    // A scheduled retry decision. Deliberately a separate variant from
    // `provider_attempt` so a host counting physical attempts never
    // double-counts a decision.
    wire!(
        "provider_retry",
        "events/provider_retry.json",
        [],
        Observational,
        "session",
        "available"
    ),
    // A typed failure discovered after the physical send completed. Carries no
    // retry authority; `failure` is the stable class.
    wire!(
        "provider_failure",
        "events/provider_failure.json",
        ["failure"],
        Observational,
        "session",
        "available"
    ),
    // Structured monitor control flow. Safety: this is how a host tells a
    // deliberate stop/replan apart from a generic engine error, and mis-reading
    // it means telling the user the run crashed when Core chose to stop.
    wire!(
        "mid_flight_monitor_decision",
        "events/mid_flight_monitor_decision.json",
        ["directive", "reason"],
        Safety,
        "session",
        "available"
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
        "budget_grant_result",
        "events/budget_grant_result.json",
        [
            "request_id",
            "additional_tokens",
            "additional_cost_usd",
            "outcome"
        ],
        Safety,
        "request_id",
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
    // #1098: "show this to the user" as CONTENT, never a path. Observational
    // — losing it costs a display, never authority — and the only producer
    // event that carries an explicit `critical` classification, which is what
    // lets a host pinned below minor 16 drop it instead of hard-erroring.
    wire!(
        "render_artifact",
        "events/render_artifact.json",
        [
            "msg_id",
            "call_id",
            "title",
            "mime",
            "content",
            "truncated",
            "critical"
        ],
        Observational,
        "call_id",
        "render_artifact_v1"
    ),
    // Non-destructive compaction notice, gated by the `ready` capability flag
    // of the same name. `active_window_percent` is the same opaque 0..=100
    // scale as `Usage.active_window_percent` and is omitted when unmeasurable.
    wire!(
        "compact_offload",
        "events/compact_offload.json",
        ["msg_id", "reason", "tokens_freed"],
        Observational,
        "msg_id",
        "non_destructive_compact"
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
    // F22-C1 — durable Goals become observable to a host for the first time.
    // Observational, not Safety: these carry no authority and a host that
    // cannot read them is not made less safe, only blind. The capability is
    // registered `ShapeOnly` because Core emits these but there is no host
    // command to pull one yet — see the seam request in 22-C1-SUMMARY.md.
    wire!(
        "goal_snapshot",
        "events/goal_snapshot.json",
        [
            "goal_version",
            "session_id",
            "goal_id",
            "cursor",
            "state_digest",
            "goal"
        ],
        Observational,
        "goal_id_and_cursor",
        "durable_goals_v1"
    ),
    wire!(
        "goal_transition",
        "events/goal_transition.json",
        [
            "goal_version",
            "session_id",
            "goal_id",
            "cursor",
            "transition",
            "lifecycle"
        ],
        Observational,
        "goal_id_and_cursor",
        "durable_goals_v1"
    ),
    // F22-C1 — the refusal channel for the five Goal control commands.
    // `Safety`, not `Observational`: a host that cannot read this cannot
    // distinguish a refused control command from an accepted no-op, and the
    // command loop's catch-all arm makes that silence the default.
    wire!(
        "goal_control_refused",
        "events/goal_control_refused.json",
        [
            "goal_version",
            "request_id",
            "session_id",
            "goal_id",
            "reason"
        ],
        Safety,
        "request_id_and_goal_id",
        "durable_goals_v1"
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
    "remove_mcp_server",
    "grant_workspace_capability",
    "grant_path",
    "revoke_path",
    "approval_resume",
    "host_send_message_result",
    // F22-C1 host Goal control.
    "goal_open",
    "goal_declare_task",
    "goal_advance",
    "goal_cancel",
    "goal_resync",
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
    "call_announced",
    "tool_running",
    "tool_result",
    "tool_cancelled",
    "stream_end",
    "error",
    "info",
    "config_changed",
    "mcp_ready",
    "mcp_failed",
    "mcp_removal_result",
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
    "budget_grant_result",
    "tool_panicked",
    "plugin_registration_failed",
    "plugin_event",
    "evolution_event",
    "browser_event",
    "browser_policy_denied",
    "cua_event",
    "cua_policy_denied",
    "host_send_message_request",
    "render_artifact",
    "compact_offload",
    "anvil_receipt",
    "anvil_receipt_invalidated",
    "goal_snapshot",
    "goal_transition",
    "goal_control_refused",
    "pong",
];

pub const SOURCE_INPUTS: &[&str] = &[
    "crates/wcore-protocol/src/child.rs",
    "crates/wcore-protocol/src/commands.rs",
    "crates/wcore-protocol/src/events.rs",
    "crates/wcore-protocol/src/diagnostics.rs",
    "crates/wcore-protocol/src/reader.rs",
    "crates/wcore-protocol/src/writer.rs",
    "crates/wcore-protocol/src/anvil.rs",
    "crates/wcore-protocol/src/execution_policy.rs",
    "crates/wcore-protocol/src/goal.rs",
    "crates/wcore-protocol/src/workflow.rs",
    "crates/wcore-protocol/src/contract/mod.rs",
    "crates/wcore-protocol/src/contract/canonical.rs",
    "crates/wcore-protocol/src/contract/spec.rs",
    "crates/wcore-protocol/src/contract/generate.rs",
    "crates/wcore-protocol/src/contract/observation.rs",
    "crates/wcore-protocol/src/contract/check.rs",
    "crates/wcore-protocol/src/bin/wcore-contract.rs",
    "crates/wcore-types/src/execution_policy.rs",
    "crates/wcore-types/src/spawner.rs",
    "crates/wcore-types/src/child_transaction.rs",
    "crates/wcore-types/src/workspace_trust.rs",
    "crates/wcore-agent/src/output/protocol_sink.rs",
    "crates/wcore-agent/src/bootstrap.rs",
    "crates/wcore-agent/src/engine.rs",
    "crates/wcore-agent/src/plugins/loader.rs",
    "crates/wcore-agent/src/plugins/mcp_delivery.rs",
    "crates/wcore-agent/src/orchestration/workflow/runner.rs",
    "crates/wcore-agent/src/orchestration/anvil/forge.rs",
    "crates/wcore-cli/src/main.rs",
    "crates/wcore-cli/src/budget_grants.rs",
    "crates/wcore-cli/src/packaged_runtime.rs",
    "crates/wcore-cli/src/runtime_diagnostics.rs",
    "crates/wcore-budget/src/tracker.rs",
    "crates/wcore-agent/src/budget_authority.rs",
    "crates/wcore-config/src/shell/executable_readiness.rs",
    "crates/wcore-config/src/shell/mcp_stdio_launch_context.rs",
    "crates/wcore-mcp/src/transport/stdio.rs",
    "crates/wcore-mcp/src/transport/stdio_readiness.rs",
    "crates/wcore-mcp/src/manager.rs",
    "crates/wcore-plugin-subprocess/src/mcp_bridge.rs",
    "crates/wcore-tools/src/registry.rs",
];

/// Canonical F18 durable-child values constructed through the real protocol
/// types. Commands and events remain an F22 concern.
pub fn durable_child_fixture_values() -> BTreeMap<String, Value> {
    fn digest(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn prepared(child_id: &str) -> DurableChildRecord {
        DurableChildRecord {
            schema_version: DURABLE_CHILD_PROTOCOL_VERSION,
            declaration_id: format!("declare-{child_id}"),
            child_id: ChildId::new(child_id).expect("fixture child id must be valid"),
            parent: ChildParent {
                session_id: "session-desktop-001".into(),
                turn_id: Some("turn-001".into()),
                parent_child_id: None,
                workflow_run_id: Some("workflow-run-001".into()),
                graph_node_id: Some("research".into()),
                parent_call_id: Some("call-spawn-001".into()),
            },
            origin: ChildOrigin::Workflow,
            request: ChildRequestEvidence::redacted(digest('a')),
            policy_snapshot: ChildPolicySnapshot {
                contract_version: "1.0".into(),
                exact_digest: digest('b'),
                posture: "smart".into(),
                approvals: "on_request".into(),
                sandbox: "workspace_write".into(),
                source: "parent_intersection".into(),
                managed_floor_active: true,
                dangerous_activation_id_digest: None,
            },
            provider: Some("anthropic".into()),
            model: Some("claude-sonnet-4-5".into()),
            workspace: ChildWorkspace {
                mode: ChildWorkspaceMode::Isolated,
                workspace_id: format!("workspace-{child_id}"),
            },
            status: DurableChildStatus::Prepared,
            desired_state: ChildDesiredState::Run,
            recovery: ChildRecoveryState::Clean,
            revision: 0,
            timestamps: ChildTimestamps {
                created_at_unix_ms: 1_721_000_000_000,
                updated_at_unix_ms: 1_721_000_000_000,
                queued_at_unix_ms: None,
                started_at_unix_ms: None,
                terminal_at_unix_ms: None,
            },
            result: None,
            delivery_target: Some(ChildDeliveryTarget::ParentTurn),
            delivery_state: ChildDeliveryState::Pending,
            attempt: 1,
            retry_of: None,
            applied_events: BTreeMap::new(),
        }
    }

    let queued = prepared("child-002");
    let mut terminal = prepared("child-001");
    terminal.status = DurableChildStatus::Succeeded;
    terminal.revision = 3;
    terminal.timestamps.updated_at_unix_ms = 1_721_000_003_000;
    terminal.timestamps.queued_at_unix_ms = Some(1_721_000_001_000);
    terminal.timestamps.started_at_unix_ms = Some(1_721_000_002_000);
    terminal.timestamps.terminal_at_unix_ms = Some(1_721_000_003_000);
    terminal.result = Some(DurableChildResult {
        exact_digest: digest('c'),
        turns: 2,
        input_tokens: 1_024,
        output_tokens: 256,
        artifact_digests: vec![digest('d')],
    });
    terminal
        .applied_events
        .insert("enqueue-001".into(), digest('e'));
    terminal
        .applied_events
        .insert("start-001".into(), digest('f'));
    terminal
        .applied_events
        .insert("succeed-001".into(), digest('1'));

    BTreeMap::from([
        (
            "types/durable_child_list.json".into(),
            serde_json::to_value(vec![terminal.clone(), queued])
                .expect("durable child list fixture must serialize"),
        ),
        (
            "types/durable_child_record.json".into(),
            serde_json::to_value(terminal).expect("durable child record fixture must serialize"),
        ),
        (
            "types/durable_child_transition.json".into(),
            serde_json::to_value(DurableChildTransition::RequestCancel)
                .expect("durable child transition fixture must serialize"),
        ),
    ])
}

/// Canonical command inputs. Every value is accepted by `ProtocolCommand`.
pub fn command_fixture_values() -> BTreeMap<String, Value> {
    BTreeMap::from([
        (
            "commands/add_mcp_server.json".into(),
            json!({"type":"add_mcp_server","name":"desktop-tools","transport":"stdio","command":"desktop-mcp","args":["--stdio"],"env":{"WAYLAND_PROFILE":"desktop"},"url":"https://mcp.invalid/v1","headers":{"X-Wayland-Contract":"v1"},"allow_local":false}),
        ),
        (
            "commands/remove_mcp_server.json".into(),
            json!({"type":"remove_mcp_server","lifecycle_version":1,"request_id":"mcp-remove-001","name":"desktop-tools"}),
        ),
        // #314. `access` and `expires_at_ms` are carried explicitly although
        // both are `#[serde(default)]`: the schema branch is inferred from the
        // fixture, so a fixture that omits an optional field publishes a branch
        // that says nothing about the frames a host really sends.
        (
            "commands/grant_workspace_capability.json".into(),
            json!({"type":"grant_workspace_capability","executable":"cargo"}),
        ),
        (
            "commands/grant_path.json".into(),
            json!({"type":"grant_path","grant_id":"grant-001","root":"/srv/reports","access":"read","expires_at_ms":1_767_225_600_000_u64}),
        ),
        (
            "commands/revoke_path.json".into(),
            json!({"type":"revoke_path","grant_id":"grant-001"}),
        ),
        // F22-C1 host Goal control. Every value below is accepted by
        // `ProtocolCommand` — pinned by the corpus's
        // `every_command_fixture_deserializes_through_protocol_command`.
        // Note there is no `parent_max_tokens` anywhere here, and that is not
        // an omission: the parent envelope is the session's authority and a
        // host cannot state one. See the module note in `commands.rs`.
        (
            "commands/goal_open.json".into(),
            json!({"type":"goal_open","goal_version":1,"request_id":"goal-open-001","session_id":"session-desktop-001","goal_id":"goal-001","objective":"ship the desktop contract","iterations":8,"strategy":"fleet","max_tokens":10000}),
        ),
        (
            "commands/goal_declare_task.json".into(),
            json!({"type":"goal_declare_task","goal_version":1,"request_id":"goal-task-001","session_id":"session-desktop-001","goal_id":"goal-001","task_id":"publish","depends_on":["build"],"idempotency_key":"idem-publish"}),
        ),
        (
            "commands/goal_advance.json".into(),
            json!({"type":"goal_advance","goal_version":1,"request_id":"goal-advance-001","session_id":"session-desktop-001","goal_id":"goal-001","cursor":{"journal_sequence":22,"journal_digest":"sha256:goalcursor"}}),
        ),
        (
            "commands/goal_cancel.json".into(),
            json!({"type":"goal_cancel","goal_version":1,"request_id":"goal-cancel-001","session_id":"session-desktop-001","goal_id":"goal-001","cursor":{"journal_sequence":22,"journal_digest":"sha256:goalcursor"}}),
        ),
        (
            "commands/goal_resync.json".into(),
            json!({"type":"goal_resync","goal_version":1,"request_id":"goal-resync-001","session_id":"session-desktop-001","goal_id":"goal-001"}),
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
            "commands/continue_with_budget.json".into(),
            json!({"type":"continue_with_budget","request_id":"budget-001","additional_tokens":250000,"additional_cost_usd":2.5}),
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
            "compat/commands/continue_with_budget.cost-only.json".into(),
            json!({"type":"continue_with_budget","request_id":"budget-cost-only","additional_cost_usd":2.5}),
        ),
        (
            "compat/commands/continue_with_budget.tokens-only.json".into(),
            json!({"type":"continue_with_budget","request_id":"budget-tokens-only","additional_tokens":250000}),
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

/// Canonical Goal projection fixture (F22-C1).
///
/// Deliberately NOT the empty/default shape. It carries a live loop owner, a
/// completed-but-undelivered task and a dependent blocked behind it, because
/// those are the three states a Desktop consumer is most likely to render
/// wrongly: an outbox entry that looks delivered, a dependency that looks
/// runnable, and a loop-owner lease that looks like a lock rather than
/// liveness evidence. A fixture built from defaults would exercise none of them.
fn goal_projection() -> GoalProjection {
    GoalProjection {
        goal_id: "goal-001".into(),
        objective: "ship the release candidate".into(),
        authority: GoalAuthorityWire {
            effective_limits: BTreeMap::from([("max_tokens".into(), 10_000)]),
            strategy: GoalStrategy::Fleet,
            loop_policy: LoopPolicy::Fixed { iterations: 8 },
            parent_envelope_digest: "wayland-core-goal-fleet/v1".into(),
            snapshot_digest: digest('8'),
        },
        lifecycle: GoalLifecycleWire::Running,
        iterations_started: 3,
        iteration_ceiling: Some(8),
        resume_count: 1,
        opened_at_unix_ms: 1_721_000_000_000,
        cursor: RecoveryCursor {
            journal_sequence: Some(22),
            journal_digest: journal_digest('9'),
        },
        tasks: vec![
            GoalTaskWire {
                task_id: "task-build".into(),
                depends_on: BTreeSet::new(),
                idempotency_key: "idem-task-build".into(),
                status: GoalTaskWireStatus::CompletedUndelivered,
                epoch: 2,
                attempts: 2,
                outcome: Some(GoalTerminalState::SelfChecked),
                dependency_releases: 0,
                last_transition_seq: 20,
            },
            GoalTaskWire {
                task_id: "task-publish".into(),
                depends_on: BTreeSet::from(["task-build".to_owned()]),
                idempotency_key: "idem-task-publish".into(),
                status: GoalTaskWireStatus::Blocked,
                epoch: 0,
                attempts: 0,
                outcome: None,
                dependency_releases: 0,
                last_transition_seq: 18,
            },
        ],
        loop_owner: Some(GoalLoopOwnerWire {
            strategy: GoalStrategy::Fleet,
            epoch: 1,
            lease_expires_unix_ms: 1_721_000_060_000,
        }),
        loop_owner_epochs: 1,
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
            "events/budget_grant_result.json".into(),
            ProtocolEvent::BudgetGrantResult {
                result: BudgetGrantResult::granted("budget-001".into(), 250_000, 2.5),
            },
        ),
        // `unavailable` is the stage that carries `reason`, so this fixture is
        // the one that publishes the reason vocabulary. The live stages are
        // reason-less by construction (`CapabilityActivation::is_well_formed`)
        // and validate against the same branch, since `reason` is optional.
        (
            "events/capability_activation.json".into(),
            ProtocolEvent::CapabilityActivation {
                activation: CapabilityActivation::unavailable(
                    CapabilityId::MidFlightMonitor,
                    CapabilityReasonCode::DisabledByConfig,
                ),
            },
        ),
        (
            "events/compact_offload.json".into(),
            ProtocolEvent::CompactOffload {
                msg_id: "msg-001".into(),
                reason: "window_pressure".into(),
                tokens_freed: 4096,
                active_window_percent: Some(48),
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
            "events/render_artifact.json".into(),
            ProtocolEvent::RenderArtifact {
                msg_id: "msg-001".into(),
                call_id: "call-render-001".into(),
                title: "Quarterly summary".into(),
                mime: RenderMime::Markdown,
                content: "# Quarterly summary\n\nRevenue held.\n".into(),
                truncated: false,
                critical: NonCritical,
            },
        ),
        (
            "events/goal_snapshot.json".into(),
            ProtocolEvent::GoalSnapshot {
                goal_version: GOAL_PROTOCOL_VERSION,
                session_id: "session-desktop-001".into(),
                goal_id: "goal-001".into(),
                cursor: RecoveryCursor {
                    journal_sequence: Some(22),
                    journal_digest: "sha256:goalcursor".into(),
                },
                state_digest: "sha256:goalstate".into(),
                goal: goal_projection(),
            },
        ),
        (
            "events/goal_transition.json".into(),
            ProtocolEvent::GoalTransition {
                goal_version: GOAL_PROTOCOL_VERSION,
                session_id: "session-desktop-001".into(),
                goal_id: "goal-001".into(),
                cursor: RecoveryCursor {
                    journal_sequence: Some(22),
                    journal_digest: "sha256:goalcursor".into(),
                },
                transition: GoalTransitionKind::LoopOwnerClaimed,
                lifecycle: GoalLifecycleWire::Running,
            },
        ),
        (
            "events/goal_control_refused.json".into(),
            ProtocolEvent::GoalControlRefused {
                goal_version: GOAL_PROTOCOL_VERSION,
                request_id: "goal-advance-001".into(),
                session_id: "session-desktop-001".into(),
                goal_id: "goal-001".into(),
                // The fixture carries `cursor_stale` deliberately: it is the
                // reason a correct host hits in normal operation (its view
                // moved under it), so the shape Desktop must handle well is
                // the one it will actually see, not a malformed-input case.
                reason: crate::events::GoalControlRefusalReason::CursorStale,
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
                already_connected: true,
            },
        ),
        (
            "events/mcp_removal_result.json".into(),
            ProtocolEvent::McpRemovalResult {
                lifecycle_version: 1,
                request_id: "mcp-remove-001".into(),
                name: "desktop-tools".into(),
                outcome: crate::events::McpRemovalOutcome::Removed,
                removed_tools: vec!["fetch".into(), "search".into()],
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
        (
            "events/mid_flight_monitor_decision.json".into(),
            ProtocolEvent::MidFlightMonitorDecision {
                directive: MonitorDirective::Stop,
                reason: MonitorReason::RepeatedToolRoute,
            },
        ),
        ("events/pong.json".into(), ProtocolEvent::Pong),
        // `failure` is optional on both attempt and retry: a clean attempt
        // carries none. The fixtures populate it so the published schema
        // describes the field a host has to read, and `required` stays at the
        // discriminator alone so a clean attempt still validates.
        (
            "events/provider_attempt.json".into(),
            ProtocolEvent::ProviderAttempt {
                failure: Some("http_503".into()),
            },
        ),
        (
            "events/provider_failure.json".into(),
            ProtocolEvent::ProviderFailure {
                failure: "stream_truncated".into(),
            },
        ),
        (
            "events/provider_retry.json".into(),
            ProtocolEvent::ProviderRetry {
                failure: Some("timeout".into()),
            },
        ),
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
                session_persistence: SessionPersistence::Durable,
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
                    escalation: None,
                },
            },
        ),
        (
            "events/call_announced.json".into(),
            ProtocolEvent::CallAnnounced {
                msg_id: "msg-001".into(),
                call_id: "call-tool-002".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command":"cargo test"}),
                    description: "Run the test suite".into(),
                    escalation: None,
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
        // Built through `resolve_workspace_trust`, not by hand: the receipt
        // records what the real precedence resolver decided, so the published
        // fixture cannot drift from the authority rule it claims to describe.
        (
            "events/workspace_policy.json".into(),
            ProtocolEvent::WorkspacePolicy {
                policy: WorkspacePolicyReceipt {
                    trust: resolve_workspace_trust(
                        "0".repeat(64),
                        [WorkspaceTrustInput::grant(AuthoritySource::User)],
                    ),
                    profile: WorkspaceSandboxProfile::TrustedLocalSmart,
                    backend: "bwrap".into(),
                    writable_roots: vec!["/workspace".into()],
                    readable_roots: vec!["/workspace".into(), "/usr/share".into()],
                    capabilities: vec![DeveloperCapability {
                        name: "cargo".into(),
                        executable: "/usr/bin/cargo".into(),
                        read_only_roots: vec!["/usr/lib/rustlib".into()],
                    }],
                },
            },
        ),
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
    let (initial_policy, _) = execution_policy_sequence();
    BTreeMap::from([
        (
            // The frame a Desktop host meets on a keyring-less server: current
            // producer, current capabilities, current policy — a REAL, named,
            // resumable session, and no crash replay behind it. Identical to
            // `events/ready.json` apart from the one field this posture
            // changes, including the contract descriptor
            // `insert_negotiation_fixtures` stamps onto both.
            //
            // REPLACES `compat/events/ready.degraded.json`, which carried
            // `session_id: null` + `disabled_by_host`. That fixture described
            // the shape a keyring-less host produced when it responded to a
            // missing key by journaling nothing. It no longer journals nothing,
            // so the corpus was publishing an example of a frame this producer
            // cannot emit — the same defect the `session_persistence` field
            // exists to prevent, one layer up. The legacy shape survives below,
            // under a name that says what it is.
            //
            // It is a REAL serialization of `ProtocolEvent::Ready`, not a JSON
            // edit of the durable fixture, so a producer that goes back to
            // dropping `session_id` under `None`, or back to calling this
            // posture `durable`, produces a fixture the corpus check reds on.
            "compat/events/ready.journaled-without-replay.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: Some("2f3a5c7e9b1d4f608a2c4e6081b3d5f7".into()),
                session_persistence: SessionPersistence::JournaledWithoutReplay,
                capabilities: capabilities(),
                contract: None,
                execution_policy: Some(initial_policy.clone()),
            },
        ),
        (
            // LEGACY, and retained for exactly one reason: `disabled_by_host`
            // was published on the wire, so a 0.12.x Core still sends it and a
            // host may hold it against a session it is still tracking. The
            // schema must therefore keep ACCEPTING it, and a corpus with no
            // example of a value its schema accepts leaves the host to take
            // that on trust.
            //
            // This producer can no longer emit it — enforced, not asserted, by
            // `session_persistence_for`'s own test in `protocol_sink.rs`, which
            // requires no input combination to yield it. The filename carries
            // the same statement to anyone reading the corpus rather than the
            // code, because a fixture is read as an example of what a producer
            // does unless it says otherwise.
            "compat/events/ready.disabled-by-host.legacy.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: None,
                session_persistence: SessionPersistence::DisabledByHost,
                capabilities: capabilities(),
                contract: None,
                execution_policy: Some(initial_policy),
            },
        ),
        (
            "compat/events/budget_grant_result.turn-in-progress.json".into(),
            ProtocolEvent::BudgetGrantResult {
                result: BudgetGrantResult::refused(
                    "budget-active-turn".into(),
                    1,
                    0.0,
                    BudgetGrantRefusalReason::TurnInProgress,
                ),
            },
        ),
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
            // The truncated shape a host must also be able to render: a
            // partial artifact carrying the in-band marker. Every field is
            // required on this event, so "minimal" here means the OTHER
            // branch of `truncated`, which is the one a host is most likely
            // to have never exercised.
            "compat/events/render_artifact.truncated.json".into(),
            ProtocolEvent::RenderArtifact {
                msg_id: "msg-minimal".into(),
                call_id: "call-render-minimal".into(),
                title: "Large log".into(),
                mime: RenderMime::Plain,
                content: "first line\n\n[wcore: CONTENT TRUNCATED. …]\n".into(),
                truncated: true,
                critical: NonCritical,
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
            // Still the LEGACY minimum — no `contract`, no `execution_policy`,
            // so it has never satisfied the current `ready` schema branch and
            // is not meant to. The keyring-less PRODUCTION shape is
            // `compat/events/ready.journaled-without-replay.json`, derived in
            // `insert_negotiation_fixtures` with the descriptor stamped.
            "compat/events/ready.minimal.json".into(),
            ProtocolEvent::Ready {
                version: "0.12.25".into(),
                session_id: None,
                session_persistence: SessionPersistence::DisabledByOperator,
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
