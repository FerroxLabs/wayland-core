use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use tokio_util::sync::CancellationToken;

/// AUDIT B-1 — per-category tool-dispatch wall-clock timeout.
///
/// Every tool dispatch is wrapped in `tokio::time::timeout` keyed on the
/// tool's [`ToolCategory`]. On elapse the dispatcher fires the call's
/// `ToolContext.cancel` (so a cooperative tool can wind down) and
/// synthesizes an error `ToolResult` — the `tool_use` still gets its
/// `tool_result` and the agent loop continues instead of hanging.
///
/// Limits follow the locked design decision (project owner, 2026-05-22):
///   * `Exec`  — 600s. Interactive shells / long builds legitimately
///     need minutes; `BashTool` also caps itself internally.
///   * `Mcp`   — 120s. Covers MCP/network tools whose subprocess or
///     endpoint can wedge.
///   * `Info` / `Edit` — 30s. A file read or edit should never take
///     longer; a stuck one is a bug, not slow legitimate work.
fn tool_dispatch_timeout(category: ToolCategory) -> Duration {
    match category {
        ToolCategory::Exec => Duration::from_secs(600),
        ToolCategory::Mcp => Duration::from_secs(120),
        ToolCategory::Info | ToolCategory::Edit => Duration::from_secs(30),
    }
}

// Anvil — native gated-forge engine (`drive_climb`), sibling of council.
pub mod anvil;
// Crucible (Mixture-of-Providers) council: cross-provider proposers + a
// provenance-aware aggregator. Hosts `CouncilProviderResolver`, which keys a
// provider id to an `Arc<dyn LlmProvider>` (resolution lives here, not in the
// leaf `wcore-types`, because it needs `wcore-providers` + `wcore-config`).
pub mod council;

// W8b.2.B C.1: directed-graph executor (additive — not wired into the
// per-turn loop yet; that lands in C.5).
pub mod graph;

// W8b.2.B C.2: graph template factories (Direct, Sequential, Parallel,
// Iterative, Hierarchical, Consensus, SelfCritique, Adaptive).
pub mod templates;

// W8b.2.B C.3: keyword-based intent classifier + loop selector that
// maps tasks to graph templates.
pub mod intent;

// W8b.2.B C.4: mid-flight monitor — budget consumer + repeated-error
// detector that emits MonitorAction decisions to the graph walker.
pub mod monitor;

// Wave OR (W8b.2.B.1): production NodeExecutor adapter that bridges
// `ExecutionGraph::execute` to the existing `execute_tool_calls_*`
// dispatch path. Wired into `engine::run` so per-turn dispatch flows
// through the graph machinery (Direct template = byte-identical to
// pre-OR behavior; non-Direct templates become invocable).
pub mod node_executor;

// v0.8.0 Task K: wire `wcore_dispatch::TemplateRouter` as the primary
// orchestration-template selector for the per-turn `LoopSelector`
// fallback. Maps each `Template` enum variant to its existing
// `GraphConfig` constructor (Direct/Consensus/SelfCritique/Adaptive/
// Hierarchical) so `multi_agent_consensus` and `hierarchical_delegation`
// — which had no caller before — become reachable. `IntentClassifier`
// remains the deterministic cold-start fallback.
pub mod template_routing;

// Dynamic Workflows (2026-05-30) — declarative RON front-end that lowers
// onto the existing `graph::GraphConfig` IR. Execution flows through a
// dedicated `WorkflowRunner` over the FleetDispatcher path; the per-turn
// `ExecutionGraph` walker is untouched.
pub mod workflow;

#[cfg(test)]
mod f13_durability_tests;

use crate::confirm::{ConfirmResult, ToolConfirmer};
use crate::engine::is_hook_lifecycle_line;
use crate::hooks::HookEngine;
use crate::journal_effects::{
    PreparedHookPhaseLease, PreparedToolLease, StartedHookPhaseLease, TurnEffectScope,
};
use crate::session_journal::{
    ApprovalDecision, ApprovalResolution, HookManifestSlot, HookPhaseConsumption,
    HookPhaseNotStartedReason, HookSlotReceipt, HookSlotSource, HookSlotTerminalStatus,
    ToolHookPhase, ToolNotStartedReason, ToolUnknownReason, state_payload_digest,
};
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_protocol::events::{OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_types::message::ContentBlock;
use wcore_types::skill_types::ContextModifier;
use wcore_types::tool::{ToolEffectKind, ToolResult};

use wcore_tools::registry::ToolRegistry;

use crate::tool_budget::ToolBudgetTracker;

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DispatcherCrashCut {
    BeforePrepared,
    AfterPrepared,
    BeforeRunning,
    AfterRunning,
    AfterPhysicalEffect,
    BeforeTerminalAppend,
    AfterTerminalAppend,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalCrashCut {
    AfterRequested,
    BeforeResolved,
    AfterResolved,
}

#[cfg(test)]
tokio::task_local! {
    static DISPATCHER_CRASH_CUT: std::cell::Cell<Option<DispatcherCrashCut>>;
}

#[cfg(test)]
tokio::task_local! {
    static APPROVAL_CRASH_CUT: std::cell::Cell<Option<ApprovalCrashCut>>;
}

#[cfg(test)]
fn inject_dispatcher_crash(cut: DispatcherCrashCut) {
    let _ = DISPATCHER_CRASH_CUT.try_with(|armed| {
        if armed.get() == Some(cut) {
            armed.set(None);
            panic!("injected dispatcher crash at {cut:?}");
        }
    });
}

#[cfg(test)]
fn inject_approval_crash(cut: ApprovalCrashCut) {
    let _ = APPROVAL_CRASH_CUT.try_with(|armed| {
        if armed.get() == Some(cut) {
            armed.set(None);
            panic!("injected approval crash at {cut:?}");
        }
    });
}

#[cfg(test)]
pub(crate) fn take_dispatcher_crash_cut() -> Option<DispatcherCrashCut> {
    DISPATCHER_CRASH_CUT
        .try_with(std::cell::Cell::take)
        .ok()
        .flatten()
}

#[cfg(test)]
async fn scope_dispatcher_crash_cut<F>(cut: DispatcherCrashCut, future: F) -> F::Output
where
    F: std::future::Future,
{
    DISPATCHER_CRASH_CUT
        .scope(std::cell::Cell::new(Some(cut)), future)
        .await
}

#[cfg(test)]
pub(crate) async fn with_dispatcher_crash_after_physical_effect<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    DISPATCHER_CRASH_CUT
        .scope(
            std::cell::Cell::new(Some(DispatcherCrashCut::AfterPhysicalEffect)),
            future,
        )
        .await
}

/// The combined output of a tool execution batch: protocol content blocks
/// paired with per-call context modifiers (None for non-skill tools).
pub struct ToolCallOutcome {
    pub results: Vec<ContentBlock>,
    pub modifiers: Vec<Option<ContextModifier>>,
    /// Aggregated outcomes from POST-tool-use hooks across all tool calls
    /// in this turn. The agent-level engine consumes these via
    /// `apply_turn_end_outcome` (W2 F1). `log_lines` is already drained
    /// at the orchestration layer (eprintln) so the entries here only
    /// carry `injected_messages` and `switch_model`.
    pub hook_outcomes: Vec<crate::hooks::HookOutcome>,
    /// `tool_use` ids whose result was synthesized because the dispatch
    /// timeout-cancel path won (see `execute_single_with_streaming`), not
    /// because the tool ran to completion. The engine reads these to set
    /// `ToolCallTrace.cancelled` on the matching trace. Empty on the normal
    /// path.
    pub cancelled_ids: Vec<String>,
}

type BatchToolOutcome = (
    ContentBlock,
    Option<ContextModifier>,
    Option<crate::hooks::HookOutcome>,
    bool,
);

type ObservedToolEffect = Option<(
    wcore_tools::effects::ToolEffectDisposition,
    serde_json::Value,
)>;
type ToolDispatchResult = std::thread::Result<(ToolResult, ObservedToolEffect)>;

impl std::ops::Deref for ToolCallOutcome {
    type Target = Vec<ContentBlock>;
    fn deref(&self) -> &Self::Target {
        &self.results
    }
}

impl std::ops::DerefMut for ToolCallOutcome {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.results
    }
}

/// Partition tool calls and execute them with optional confirmation and hooks
pub async fn execute_tool_calls(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_streaming(
        registry,
        tool_calls,
        confirmer,
        hooks,
        compaction_level,
        toon_enabled,
        None,
        &CancellationToken::new(),
        None,
    )
    .await
}

/// W7 F4: variant of `execute_tool_calls` accepting an optional
/// streaming context. When `streaming` is `Some` AND a dispatched tool
/// reports `supports_streaming() && output.streaming_tools_advertised()`,
/// per-line chunks flow through `OutputSink::emit_tool_chunk`. Otherwise
/// behaviour is byte-identical to the pre-W7 path.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_streaming(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_budget(
        registry,
        tool_calls,
        confirmer,
        hooks,
        compaction_level,
        toon_enabled,
        streaming,
        None,
        cancel,
        file_write_notifier,
    )
    .await
}

/// v0.6.1 hardening (CRIT-1) — wraps `execute_tool_calls_with_budget`
/// with a `PolicyGate` check. Tools the gate denies are filtered out
/// before dispatch and surface as `ToolResult { is_error: true }` in
/// the returned outcome, exactly like a tool that ran and failed.
///
/// Filtering before dispatch (rather than gating per-tool inside
/// `execute_single*`) means:
///   1. Denied tools never reach hook engines, sandbox spawns, or any
///      other side-effecting machinery — a deny is a hard short-circuit.
///   2. We don't need to thread the gate through every dispatch fn
///      signature, which keeps the diff surgical and reduces risk of
///      missed call sites becoming bypass vectors.
///
/// `policy_gate = None` produces byte-identical behaviour to
/// `execute_tool_calls_with_budget`, so existing call sites are
/// unaffected unless they explicitly opt in via this entry.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_policy_gate(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    budget: Option<&ToolBudgetTracker>,
    policy_gate: Option<&crate::policy_gate::PolicyGate>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let Some(gate) = policy_gate else {
        // Fast path: no policy configured. Delegate verbatim.
        return execute_tool_calls_with_budget(
            registry,
            tool_calls,
            confirmer,
            hooks,
            compaction_level,
            toon_enabled,
            streaming,
            budget,
            cancel,
            file_write_notifier,
        )
        .await;
    };

    let filtered = filter_tool_calls_by_policy(tool_calls, gate);
    let allowed_calls = filtered.allowed_calls();
    let inner_outcome = execute_tool_calls_with_budget(
        registry,
        &allowed_calls,
        confirmer,
        hooks,
        compaction_level,
        toon_enabled,
        streaming,
        budget,
        cancel,
        file_write_notifier,
    )
    .await?;

    Ok(merge_policy_outcome(filtered, inner_outcome))
}

struct PolicyFilteredCalls {
    allowed: Vec<(usize, ContentBlock)>,
    denied: Vec<(usize, ContentBlock)>,
    total: usize,
}

impl PolicyFilteredCalls {
    fn allowed_calls(&self) -> Vec<ContentBlock> {
        self.allowed.iter().map(|(_, call)| call.clone()).collect()
    }

    fn journal_denials(
        &mut self,
        registry: &ToolRegistry,
        effect_scope: Option<&TurnEffectScope>,
        original_calls: &[ContentBlock],
    ) {
        let tool_calls = original_calls
            .iter()
            .filter(|call| matches!(call, ContentBlock::ToolUse { .. }))
            .collect::<Vec<_>>();
        for (ordinal, denied) in &mut self.denied {
            let Some(call) = tool_calls.get(*ordinal).copied() else {
                continue;
            };
            let ContentBlock::ToolUse {
                id, name, input, ..
            } = call
            else {
                continue;
            };
            let policy = match denied {
                ContentBlock::ToolResult { content, .. } => content.clone(),
                _ => "policy gate denied tool invocation".to_string(),
            };
            let contract = registry
                .get(name)
                .map(|tool| tool.effect_contract(input))
                .unwrap_or_default();
            if let Err(error) = record_tool_not_started(
                effect_scope,
                id,
                *ordinal as u64,
                name,
                input,
                input,
                contract,
                ToolNotStartedReason::PolicyDenied { policy },
                None,
            ) {
                *denied = journal_authority_failure(id, error);
            }
        }
    }
}

/// Apply the optional actor/tool ACL gate before selecting a host or terminal
/// approval path. This is not the immutable Managed execution-policy floor.
/// The index tags let the caller restore the model's original call order.
fn filter_tool_calls_by_policy(
    tool_calls: &[ContentBlock],
    gate: &crate::policy_gate::PolicyGate,
) -> PolicyFilteredCalls {
    let mut allowed: Vec<(usize, ContentBlock)> = Vec::with_capacity(tool_calls.len());
    let mut denied: Vec<(usize, ContentBlock)> = Vec::new();
    let mut result_idx = 0;
    for call in tool_calls {
        match call {
            ContentBlock::ToolUse { id, name, .. } => {
                // Top-level dispatch uses the gate's default actor;
                // sub-agent attribution is a v0.7 follow-up that needs
                // source_agent threading through orchestration.
                match gate.check_tool(name, None) {
                    Ok(()) => allowed.push((result_idx, call.clone())),
                    Err(deny) => denied.push((
                        result_idx,
                        ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: format!("Denied by policy: {deny}"),
                            is_error: true,
                        },
                    )),
                }
                result_idx += 1;
            }
            // Match the underlying dispatchers: defensive non-ToolUse input
            // produces no tool result and is excluded from merge indexing.
            _ => continue,
        }
    }

    PolicyFilteredCalls {
        allowed,
        denied,
        total: result_idx,
    }
}

fn merge_policy_outcome(
    filtered: PolicyFilteredCalls,
    inner_outcome: ToolCallOutcome,
) -> ToolCallOutcome {
    // Re-merge into original order. `allowed[i]` corresponds to
    // `inner_outcome.results[i]`; `denied[j].0` is its original index.
    let mut results: Vec<Option<ContentBlock>> = (0..filtered.total).map(|_| None).collect();
    let mut modifiers: Vec<Option<Option<ContextModifier>>> =
        (0..filtered.total).map(|_| None).collect();
    for (allowed_pos, (orig_idx, _)) in filtered.allowed.iter().enumerate() {
        results[*orig_idx] = Some(inner_outcome.results[allowed_pos].clone());
        modifiers[*orig_idx] = Some(inner_outcome.modifiers[allowed_pos].clone());
    }
    for (orig_idx, denied_block) in filtered.denied {
        results[orig_idx] = Some(denied_block);
        modifiers[orig_idx] = Some(None);
    }
    // SAFETY: every index 0..total is set exactly once — either via the
    // `allowed` loop (which iterates all allowed positions and maps them
    // back to `orig_idx`) or via the `denied` loop (which covers the
    // remaining indices). The two loops partition 0..total, so every
    // `Option` slot is `Some` by the time we reach this point.
    ToolCallOutcome {
        results: results
            .into_iter()
            .map(|r| r.expect("merge covers all indices"))
            .collect(),
        modifiers: modifiers
            .into_iter()
            .map(|m| m.expect("merge covers all indices"))
            .collect(),
        hook_outcomes: inner_outcome.hook_outcomes,
        // Cancelled ids are the inner dispatcher's tool_use ids; the policy
        // gate only filters denied tools (which never dispatch), so forwarding
        // verbatim keeps the mapping correct.
        cancelled_ids: inner_outcome.cancelled_ids,
    }
}

/// W8b.2.A-5: variant accepting an optional `ToolBudgetTracker` so the
/// dispatcher records per-tool call counts + wall-time around every
/// dispatch site. The legacy `execute_tool_calls_with_streaming`
/// delegates here with `None`, preserving byte-identical behaviour for
/// every existing caller.
///
/// When `budget` is `Some`, each tool call is wrapped in
/// `tracker.start(name)`; the RAII guard records elapsed runtime on
/// drop (and on the cancel path).
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_budget(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_budget_and_effects(
        registry,
        tool_calls,
        confirmer,
        hooks,
        compaction_level,
        toon_enabled,
        streaming,
        budget,
        cancel,
        file_write_notifier,
        None,
        None,
    )
    .await
}

/// Production F13 dispatch entry point. Existing callers deliberately route
/// through [`execute_tool_calls_with_budget`] with no journal authority; a
/// persisted engine must supply the active turn scope here.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_calls_with_budget_and_effects(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    mut hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: Option<&TurnEffectScope>,
    effect_ordinals: Option<&[u64]>,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut hook_outcomes = Vec::new();
    // `tool_use` ids whose result came from the dispatch timeout-cancel path.
    let mut cancelled_ids = Vec::new();

    for batch in partition(registry, tool_calls) {
        if batch.is_concurrent {
            // For concurrent batch, confirm all first, then execute approved ones.
            // Concurrent tools are never SkillTool (is_concurrency_safe=false for Skill),
            // so no skill hooks merging is needed here.
            let mut batch_outcomes: Vec<Option<BatchToolOutcome>> =
                (0..batch.calls.len()).map(|_| None).collect();
            let mut approved = Vec::new();
            for (batch_idx, call) in batch.calls.iter().enumerate() {
                match confirm_call(registry, confirmer, call)? {
                    ConfirmedCall::Denied(denied) => {
                        let denied = record_terminal_denial(
                            registry,
                            effect_scope,
                            durable_tool_call_ordinal(effect_ordinals, tool_calls, call),
                            call,
                            denied,
                        );
                        batch_outcomes[batch_idx] = Some((denied, None, None, false));
                    }
                    ConfirmedCall::Execute { approval_bound } => {
                        approved.push((batch_idx, call, approval_bound));
                    }
                }
            }
            // Reborrow as shared for concurrent execution. Concurrent
            // batches never include Bash (Bash is_concurrency_safe=false),
            // so streaming is intentionally not threaded here.
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            // AUDIT B-8: each future carries its OWN per-category
            // timeout inside `execute_single_with_streaming`, so a hung
            // sibling becomes an error `ToolResult` on its own deadline
            // without dragging the whole batch. `join_all` is therefore
            // safe — every member terminates within its category limit.
            let futures: Vec<_> = approved
                .iter()
                .map(|(_, call, approval_bound)| {
                    execute_single_with_budget(
                        registry,
                        call,
                        hooks_shared,
                        compaction_level,
                        toon_enabled,
                        budget,
                        *approval_bound,
                        cancel,
                        file_write_notifier,
                        effect_scope,
                        durable_tool_call_ordinal(effect_ordinals, tool_calls, call),
                    )
                })
                .collect();
            let batch_results = futures::future::join_all(futures).await;
            for ((batch_idx, _, _), (block, modifier, post_outcome, was_cancelled)) in
                approved.into_iter().zip(batch_results)
            {
                batch_outcomes[batch_idx] =
                    Some((block, modifier, Some(post_outcome), was_cancelled));
            }
            for outcome in batch_outcomes {
                let (block, modifier, post_outcome, was_cancelled) =
                    outcome.expect("confirmation partitions every concurrent call");
                if was_cancelled && let ContentBlock::ToolResult { tool_use_id, .. } = &block {
                    cancelled_ids.push(tool_use_id.clone());
                }
                results.push(block);
                modifiers.push(modifier);
                if let Some(post_outcome) = post_outcome {
                    hook_outcomes.push(post_outcome);
                }
            }
        } else {
            for call in &batch.calls {
                match confirm_call(registry, confirmer, call)? {
                    ConfirmedCall::Denied(denied) => {
                        results.push(record_terminal_denial(
                            registry,
                            effect_scope,
                            durable_tool_call_ordinal(effect_ordinals, tool_calls, call),
                            call,
                            denied,
                        ));
                        modifiers.push(None);
                    }
                    ConfirmedCall::Execute { approval_bound } => {
                        // Reborrow as shared for execute_single, then reclaim mut for merge.
                        let block;
                        let modifier;
                        let post_outcome;
                        let was_cancelled;
                        {
                            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
                            (block, modifier, post_outcome, was_cancelled) =
                                execute_single_with_streaming(
                                    registry,
                                    call,
                                    hooks_shared,
                                    compaction_level,
                                    toon_enabled,
                                    streaming.clone(),
                                    budget,
                                    approval_bound,
                                    cancel,
                                    file_write_notifier,
                                    effect_scope,
                                    durable_tool_call_ordinal(effect_ordinals, tool_calls, call),
                                    None,
                                )
                                .await;
                        }
                        // Merge skill hooks after a successful sequential execution.
                        if !block_is_error(&block) {
                            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
                        }
                        if was_cancelled
                            && let ContentBlock::ToolResult { tool_use_id, .. } = &block
                        {
                            cancelled_ids.push(tool_use_id.clone());
                        }
                        results.push(block);
                        modifiers.push(modifier);
                        hook_outcomes.push(post_outcome);
                    }
                }
            }
        }
    }

    Ok(ToolCallOutcome {
        results,
        modifiers,
        hook_outcomes,
        cancelled_ids,
    })
}

/// Signal that the user wants to abort
#[derive(Debug)]
pub enum ExecutionControl {
    Quit,
}

/// Confirm a single tool call and record whether approval was granted for the
/// displayed arguments rather than inherited from an automatic allow rule.
enum ConfirmedCall {
    Execute { approval_bound: bool },
    Denied(ContentBlock),
}

fn confirm_call(
    registry: &ToolRegistry,
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    call: &ContentBlock,
) -> Result<ConfirmedCall, ExecutionControl> {
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = call
    else {
        return Ok(ConfirmedCall::Execute {
            approval_bound: false,
        });
    };

    let category = registry
        .get(name)
        .map(|tool| tool.category_for(input))
        .unwrap_or(ToolCategory::Exec);
    let input_display = serde_json::to_string(input).unwrap_or_default();
    // SAFETY: `Mutex<ToolConfirmer>` is held by short critical
    // sections (`check`, `is_auto_approve`, `add_to_allow_list`); the
    // only panic surface inside them is a `let _ = io::stderr().flush()`
    // call which now no longer panics (Wave RB). Poisoning is therefore
    // unreachable, and even in the hypothetical poisoned case the
    // dispatch can't proceed safely so a panic here is acceptable.
    // The type is public API; converting it to `parking_lot::Mutex`
    // would break every caller, so we keep std::sync and document
    // the invariant.
    let mut confirmer = confirmer.lock().unwrap();
    let approval_bound = confirmer.approval_is_input_bound(name);
    let result = confirmer.check_for(name, category, &truncate_display(&input_display, 200));

    match result {
        ConfirmResult::Approved => Ok(ConfirmedCall::Execute { approval_bound }),
        ConfirmResult::Denied => Ok(ConfirmedCall::Denied(ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: "Tool execution denied by user".to_string(),
            is_error: true,
        })),
        ConfirmResult::Quit => Err(ExecutionControl::Quit),
    }
}

/// W7 F4: per-tool-call streaming context. Threaded through
/// `execute_single_with_streaming` so the engine can bridge a tool's
/// `ToolOutputSink` to the parent `OutputSink::emit_tool_chunk`. Owned
/// fields so the context is `Clone` and can be passed into per-call
/// closures cheaply (the inner `Arc` is the shared sink).
#[derive(Clone)]
pub struct StreamingContext {
    pub output: std::sync::Arc<dyn crate::output::OutputSink>,
    pub msg_id: String,
}

/// W7 F4: thin `ToolOutputSink` adapter that forwards each chunk through
/// the parent `OutputSink::emit_tool_chunk` for `ProtocolEvent::ToolChunk`
/// emission. Constructed per-call so msg_id + call_id + tool_name are
/// captured cleanly.
struct ProtocolToolSink {
    output: std::sync::Arc<dyn crate::output::OutputSink>,
    msg_id: String,
    call_id: String,
    tool_name: String,
    redactor: Mutex<crate::output_redaction::StreamingRedactor>,
}

struct PreparedHookAuthority {
    lease: PreparedHookPhaseLease,
    slots: Vec<HookManifestSlot>,
}

struct HookAuthorityRequest<'a> {
    provider_call_id: &'a str,
    ordinal: u64,
    phase: ToolHookPhase,
    tool_execution_id: Option<String>,
    tool_name: &'a str,
    tool_input: &'a serde_json::Value,
}

fn prepare_hook_authority(
    effect_scope: Option<&TurnEffectScope>,
    hook_engine: &HookEngine,
    request: HookAuthorityRequest<'_>,
) -> Result<Option<PreparedHookAuthority>, String> {
    let Some(scope) = effect_scope else {
        return Ok(None);
    };
    let runtime_phase = match request.phase {
        ToolHookPhase::PreToolUse => HookPhase::PreToolUse,
        ToolHookPhase::PostToolUse => HookPhase::PostToolUse,
    };
    let descriptors =
        hook_engine.tool_hook_manifest(runtime_phase, request.tool_name, request.tool_input);
    if descriptors.is_empty() {
        return Ok(None);
    }
    let slots = descriptors
        .iter()
        .enumerate()
        .map(|(ordinal, descriptor)| {
            let descriptor_digest = state_payload_digest(descriptor)?;
            let source = match descriptor.get("kind").and_then(serde_json::Value::as_str) {
                Some("rust") => HookSlotSource::Rust,
                Some("plugin") => HookSlotSource::Plugin,
                Some("shell") => HookSlotSource::Shell,
                _ => {
                    return Err(crate::session_journal::JournalError::InvalidTransition(
                        "tool hook manifest contains an unknown source".to_string(),
                    ));
                }
            };
            Ok(HookManifestSlot {
                ordinal: ordinal as u64,
                slot_id: format!(
                    "{}-{ordinal}-{descriptor_digest}",
                    match source {
                        HookSlotSource::Rust => "rust",
                        HookSlotSource::Plugin => "plugin",
                        HookSlotSource::Shell => "shell",
                    }
                ),
                source,
                descriptor_digest,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("hook manifest could not be bound: {error}"))?;
    let input_digest = state_payload_digest(request.tool_input)
        .map_err(|error| format!("hook input could not be digested: {error}"))?;
    let hook_authority_digest = state_payload_digest(&hook_engine.tool_hook_authority())
        .map_err(|error| format!("hook authority could not be digested: {error}"))?;
    let hook_manifest_digest = state_payload_digest(
        &serde_json::to_value(&slots)
            .map_err(|error| format!("hook manifest could not be encoded: {error}"))?,
    )
    .map_err(|error| format!("hook manifest could not be digested: {error}"))?;
    let lease = scope
        .prepare_hook_phase(
            request.provider_call_id,
            request.ordinal,
            request.phase,
            request.tool_execution_id,
            input_digest,
            hook_authority_digest,
            hook_manifest_digest,
            slots.clone(),
        )
        .map_err(|error| format!("durable hook authority could not be prepared: {error}"))?;
    Ok(Some(PreparedHookAuthority { lease, slots }))
}

fn finish_hook_authority(
    started: StartedHookPhaseLease,
    slots: Vec<HookManifestSlot>,
    outcome: &crate::hooks::HookOutcome,
    effective_input: Option<&serde_json::Value>,
) -> Result<HookPhaseConsumption, String> {
    if outcome.completed_slots > slots.len()
        || (outcome.completed_slots < slots.len() && outcome.block.is_none())
    {
        return Err(
            "hook runner did not authoritatively terminate every manifest slot".to_string(),
        );
    }
    let receipts = slots
        .into_iter()
        .enumerate()
        .map(|(ordinal, slot)| HookSlotReceipt {
            ordinal: slot.ordinal,
            slot_id: slot.slot_id,
            descriptor_digest: slot.descriptor_digest,
            status: if ordinal < outcome.completed_slots {
                HookSlotTerminalStatus::Completed
            } else {
                HookSlotTerminalStatus::SkippedAfterBlock
            },
        })
        .collect::<Vec<_>>();
    let outcome_digest = state_payload_digest(&serde_json::json!({
        "block": outcome.block,
        "modified_input": outcome.modified_input,
        "injected_messages": outcome.injected_messages,
        "switch_model": outcome.switch_model,
    }))
    .map_err(|error| format!("hook outcome could not be digested: {error}"))?;
    let receipts_digest = state_payload_digest(
        &serde_json::to_value(&receipts)
            .map_err(|error| format!("hook receipts could not be encoded: {error}"))?,
    )
    .map_err(|error| format!("hook receipts could not be digested: {error}"))?;
    let effective_input_digest = effective_input
        .map(state_payload_digest)
        .transpose()
        .map_err(|error| format!("effective hook input could not be digested: {error}"))?;
    started
        .finish(
            effective_input_digest,
            outcome_digest,
            receipts_digest,
            receipts,
        )
        .map_err(|error| format!("durable hook outcome could not be finished: {error}"))
}

impl wcore_tools::ToolOutputSink for ProtocolToolSink {
    fn emit_chunk(&self, chunk: &str) {
        let mut redactor = self.redactor.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(redacted) = redactor.push(chunk) {
            self.output
                .emit_tool_chunk(&self.msg_id, &self.call_id, &self.tool_name, &redacted);
        }
    }
}

impl Drop for ProtocolToolSink {
    fn drop(&mut self) {
        let redactor = self.redactor.get_mut().unwrap_or_else(|p| p.into_inner());
        if let Some(redacted) = redactor.finish() {
            self.output
                .emit_tool_chunk(&self.msg_id, &self.call_id, &self.tool_name, &redacted);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_tool_effect(
    effect_scope: Option<&TurnEffectScope>,
    provider_call_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: &serde_json::Value,
    effective_input: &serde_json::Value,
    contract: wcore_types::tool::ToolEffectContract,
    effect_receipt: Option<serde_json::Value>,
    pre_hook_phase_id: Option<&str>,
) -> Result<Option<PreparedToolLease>, String> {
    effect_scope
        .map(|scope| {
            let prepared = match (effect_receipt, pre_hook_phase_id) {
                (Some(receipt), Some(phase_id)) => scope
                    .prepare_tool_with_effect_receipt_after_hook(
                        provider_call_id,
                        ordinal,
                        tool,
                        requested_input.clone(),
                        effective_input.clone(),
                        contract,
                        receipt,
                        phase_id,
                    ),
                (Some(receipt), None) => scope.prepare_tool_with_effect_receipt(
                    provider_call_id,
                    ordinal,
                    tool,
                    requested_input.clone(),
                    effective_input.clone(),
                    contract,
                    receipt,
                ),
                (None, Some(phase_id)) => scope.prepare_tool_after_hook(
                    provider_call_id,
                    ordinal,
                    tool,
                    requested_input.clone(),
                    effective_input.clone(),
                    contract,
                    phase_id,
                ),
                (None, None) => scope.prepare_tool_with_contract(
                    provider_call_id,
                    ordinal,
                    tool,
                    requested_input.clone(),
                    effective_input.clone(),
                    contract,
                ),
            };
            prepared.map_err(|error| format!("durable tool intent could not be recorded: {error}"))
        })
        .transpose()
}

async fn store_prepared_effect_checkpoint(
    effect_scope: Option<&TurnEffectScope>,
    prepared: Option<&wcore_tools::effects::PreparedToolEffect>,
) -> Result<(), String> {
    let (Some(scope), Some(prepared), Some(preimage)) = (
        effect_scope,
        prepared,
        prepared.and_then(|effect| effect.preimage_bytes()),
    ) else {
        return Ok(());
    };
    let identity = prepared
        .filesystem_receipt()
        .checkpoint_identity()
        .ok_or_else(|| "prepared filesystem preimage has no checkpoint identity".to_string())?;
    if identity.len != preimage.len() as u64 {
        return Err("prepared filesystem checkpoint length does not match its receipt".to_string());
    }
    let scope = (*scope).clone();
    let digest = identity.sha256.clone();
    let preimage = preimage.to_vec();
    tokio::task::spawn_blocking(move || scope.store_effect_checkpoint(&digest, &preimage))
        .await
        .map_err(|error| format!("prepared filesystem checkpoint task failed: {error}"))?
        .map_err(|error| format!("prepared filesystem checkpoint could not be stored: {error}"))
}

fn journal_authority_failure(call_id: &str, error: impl std::fmt::Display) -> ContentBlock {
    ContentBlock::ToolResult {
        tool_use_id: call_id.to_string(),
        content: crate::output_redaction::redact_tool_output(&format!(
            "Tool was not executed because durable session authority failed: {error}"
        )),
        is_error: true,
    }
}

#[allow(clippy::too_many_arguments)]
fn record_tool_not_started(
    effect_scope: Option<&TurnEffectScope>,
    provider_call_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: &serde_json::Value,
    effective_input: &serde_json::Value,
    contract: wcore_types::tool::ToolEffectContract,
    reason: ToolNotStartedReason,
    pre_hook_phase_id: Option<&str>,
) -> Result<(), String> {
    prepare_tool_effect(
        effect_scope,
        provider_call_id,
        ordinal,
        tool,
        requested_input,
        effective_input,
        contract,
        None,
        pre_hook_phase_id,
    )?
    .map(|lease| lease.not_started(reason))
    .transpose()
    .map_err(|error| format!("durable not-started outcome could not be recorded: {error}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_tool_attempt_not_started(
    effect_scope: Option<&TurnEffectScope>,
    provider_call_id: &str,
    ordinal: u64,
    tool: &str,
    requested_input: &serde_json::Value,
    effective_input: &serde_json::Value,
    contract: wcore_types::tool::ToolEffectContract,
    reason: ToolNotStartedReason,
    pre_hook_phase_id: Option<&str>,
    recovered_retry: Option<&RecoveredToolRetry<'_>>,
) -> Result<(), String> {
    if let Some(retry) = recovered_retry {
        let scope = effect_scope
            .ok_or_else(|| "recovered tool retry has no durable effect scope".to_string())?;
        return scope
            .retry_not_started_tool(retry.prior_tool_execution_id)
            .and_then(|lease| lease.not_started(reason))
            .map_err(|error| {
                format!("durable recovered tool retry could not be terminalized: {error}")
            });
    }
    record_tool_not_started(
        effect_scope,
        provider_call_id,
        ordinal,
        tool,
        requested_input,
        effective_input,
        contract,
        reason,
        pre_hook_phase_id,
    )
}

fn record_terminal_denial(
    registry: &ToolRegistry,
    effect_scope: Option<&TurnEffectScope>,
    ordinal: u64,
    call: &ContentBlock,
    denied: ContentBlock,
) -> ContentBlock {
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = call
    else {
        return denied;
    };
    let contract = registry
        .get(name)
        .map(|tool| tool.effect_contract(input))
        .unwrap_or_default();
    record_tool_not_started(
        effect_scope,
        id,
        ordinal,
        name,
        input,
        input,
        contract,
        ToolNotStartedReason::ApprovalDenied {
            approval_id: format!("terminal:{id}"),
        },
        None,
    )
    .map_or_else(|error| journal_authority_failure(id, error), |()| denied)
}

/// Materialize a recovered no-start outcome only after the exact F13 tool
/// intent and terminal not-started receipt are durable. Recovery uses this
/// for approval decisions that were journaled before the original process
/// could create a tool execution record.
pub(crate) fn record_recovered_tool_not_started(
    registry: &ToolRegistry,
    effect_scope: &TurnEffectScope,
    ordinal: u64,
    call: &ContentBlock,
    reason: ToolNotStartedReason,
    content: String,
) -> ContentBlock {
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = call
    else {
        return ContentBlock::ToolResult {
            tool_use_id: String::new(),
            content: "Recovered non-tool block was not executed".to_string(),
            is_error: true,
        };
    };
    let result = ContentBlock::ToolResult {
        tool_use_id: id.clone(),
        content,
        is_error: true,
    };
    let contract = registry
        .get(name)
        .map(|tool| tool.effect_contract(input))
        .unwrap_or_default();
    record_tool_not_started(
        Some(effect_scope),
        id,
        ordinal,
        name,
        input,
        input,
        contract,
        reason,
        None,
    )
    .map_or_else(|error| journal_authority_failure(id, error), |()| result)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
async fn execute_single(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    approval_bound: bool,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> (
    ContentBlock,
    Option<ContextModifier>,
    crate::hooks::HookOutcome,
    // `was_cancelled`: true only when the dispatch timeout-cancel path won.
    bool,
) {
    execute_single_with_budget(
        registry,
        call,
        hooks,
        compaction_level,
        toon_enabled,
        None,
        approval_bound,
        cancel,
        file_write_notifier,
        None,
        0,
    )
    .await
}

/// W8b.2.A-5: budget-aware variant of `execute_single`. Used by the
/// concurrent batch path in `execute_tool_calls_with_budget`.
#[allow(clippy::too_many_arguments)]
async fn execute_single_with_budget(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    approval_bound: bool,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: Option<&TurnEffectScope>,
    ordinal: u64,
) -> (
    ContentBlock,
    Option<ContextModifier>,
    crate::hooks::HookOutcome,
    // `was_cancelled`: true only when the dispatch timeout-cancel path won.
    bool,
) {
    execute_single_with_streaming(
        registry,
        call,
        hooks,
        compaction_level,
        toon_enabled,
        None,
        budget,
        approval_bound,
        cancel,
        file_write_notifier,
        effect_scope,
        ordinal,
        None,
    )
    .await
}

/// W7 F4: variant of `execute_single` that accepts an optional
/// streaming context. When `streaming` is `Some` AND the resolved tool
/// reports `supports_streaming() && output.streaming_tools_advertised()`,
/// the dispatcher routes through `execute_streaming` with a per-call
/// `ProtocolToolSink`; otherwise behaviour is byte-identical to the
/// pre-W7 path.
#[allow(clippy::too_many_arguments)]
async fn execute_single_with_streaming(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    budget: Option<&ToolBudgetTracker>,
    approval_bound: bool,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: Option<&TurnEffectScope>,
    ordinal: u64,
    recovered_retry: Option<&RecoveredToolRetry<'_>>,
) -> (
    ContentBlock,
    Option<ContextModifier>,
    crate::hooks::HookOutcome,
    // `was_cancelled`: true only when the dispatch timeout-cancel path won
    // (the tool's result is synthesized, not produced by a completed run).
    bool,
) {
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = call
    else {
        unreachable!("execute_single called with non-ToolUse block")
    };

    if let Some(retry) = recovered_retry
        && (retry.tool != name || retry.ordinal != ordinal)
    {
        return (
            journal_authority_failure(
                id,
                "recovered tool retry identity does not match the durable attempt",
            ),
            None,
            crate::hooks::HookOutcome::default(),
            false,
        );
    }

    // Run pre-tool-use hooks. A crash-proven retry reuses the pre-hook
    // authority from its original attempt: rerunning hooks could mutate the
    // input differently or repeat an external hook effect.
    let mut effective_input = input.clone();
    let mut pre_outcome = crate::hooks::HookOutcome::default();
    let mut pre_hook_phase_id = None;
    if let Some(retry) = recovered_retry {
        let requested_input_digest = match state_payload_digest(input) {
            Ok(digest) => digest,
            Err(error) => {
                return (
                    journal_authority_failure(id, error),
                    None,
                    pre_outcome,
                    false,
                );
            }
        };
        if requested_input_digest != retry.requested_input_digest {
            return (
                journal_authority_failure(
                    id,
                    "recovered tool retry input does not match the durable request",
                ),
                None,
                pre_outcome,
                false,
            );
        }
        if retry.requested_input_digest != retry.effective_input_digest {
            return (
                journal_authority_failure(
                    id,
                    "recovered tool retry cannot reconstruct hook-modified input from redacted durable state",
                ),
                None,
                pre_outcome,
                false,
            );
        }
        if retry.pre_hook_phase_id.is_none() && retry.pre_hook_consumption.is_some() {
            return (
                journal_authority_failure(
                    id,
                    "recovered tool retry consumption has no durable pre-hook authority",
                ),
                None,
                pre_outcome,
                false,
            );
        }
        pre_hook_phase_id = retry.pre_hook_phase_id.map(str::to_owned);
        if let Some(consumption) = retry.pre_hook_consumption.clone() {
            pre_outcome.durable_hook_phases.push(consumption);
        }
    } else if let Some(hook_engine) = hooks {
        let prepared_hook = match prepare_hook_authority(
            effect_scope,
            hook_engine,
            HookAuthorityRequest {
                provider_call_id: id,
                ordinal,
                phase: ToolHookPhase::PreToolUse,
                tool_execution_id: None,
                tool_name: name,
                tool_input: input,
            },
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                return (
                    journal_authority_failure(id, error),
                    None,
                    pre_outcome,
                    false,
                );
            }
        };
        let started_hook = match prepared_hook
            .map(|prepared| {
                prepared
                    .lease
                    .start(None)
                    .map(|started| (started, prepared.slots))
            })
            .transpose()
        {
            Ok(started) => started,
            Err(error) => {
                return (
                    journal_authority_failure(id, error),
                    None,
                    pre_outcome,
                    false,
                );
            }
        };
        match hook_engine.run_pre_tool_use_strict(name, input).await {
            Ok(mut outcome) => {
                let candidate_input = outcome.modified_input.as_ref().unwrap_or(input);
                if let Some((started, slots)) = started_hook {
                    let phase_id = started.id().to_string();
                    match finish_hook_authority(started, slots, &outcome, Some(candidate_input)) {
                        Ok(consumption) => {
                            pre_hook_phase_id = Some(phase_id);
                            outcome.durable_hook_phases.push(consumption);
                        }
                        Err(error) => {
                            return (
                                journal_authority_failure(id, error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                    }
                }
                if let Some(reason) = outcome.block.clone() {
                    let durable = record_tool_not_started(
                        effect_scope,
                        id,
                        ordinal,
                        name,
                        input,
                        &effective_input,
                        registry
                            .get(name)
                            .map(|tool| tool.effect_contract(&effective_input))
                            .unwrap_or_default(),
                        ToolNotStartedReason::HookDenied {
                            reason: reason.clone(),
                        },
                        pre_hook_phase_id.as_deref(),
                    );
                    let block = durable.map_or_else(
                        |error| journal_authority_failure(id, error),
                        |()| ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: crate::output_redaction::redact_tool_output(&format!(
                                "Blocked by hook: {reason}"
                            )),
                            is_error: true,
                        },
                    );
                    return (block, None, outcome, false);
                }
                if let Some(v) = outcome.modified_input.clone() {
                    if approval_bound && v != *input {
                        let reason = "modified tool input requires fresh approval".to_string();
                        let durable = record_tool_not_started(
                            effect_scope,
                            id,
                            ordinal,
                            name,
                            input,
                            &v,
                            registry
                                .get(name)
                                .map(|tool| tool.effect_contract(&v))
                                .unwrap_or_default(),
                            ToolNotStartedReason::HookDenied {
                                reason: reason.clone(),
                            },
                            pre_hook_phase_id.as_deref(),
                        );
                        let block = durable.map_or_else(
                            |error| journal_authority_failure(id, error),
                            |()| ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: format!("Blocked by hook: {reason}"),
                                is_error: true,
                            },
                        );
                        return (block, None, outcome, false);
                    }
                    effective_input = v;
                }
                // v0.9.1.2 F10: route hook lifecycle trace to `tracing::debug!`
                // (file sink in TUI mode, stderr in non-TUI) — never eprintln!,
                // which paints the alt-screen and clobbers the transcript /
                // composer area. `hook_trace` is plugin-hook fire lines +
                // rust-hook "action ignored at phase X" diagnostics. Any
                // legitimate user-facing line still in `log_lines` (shell
                // hook stdout etc.) is filtered through `is_hook_lifecycle_line`
                // as belt-and-suspenders.
                for line in outcome.hook_trace.drain(..) {
                    let line = crate::output_redaction::redact_tool_output(&line);
                    tracing::debug!(target: "wcore_agent::hooks", "{line}");
                }
                for line in outcome.log_lines.drain(..) {
                    let line = crate::output_redaction::redact_tool_output(&line);
                    if is_hook_lifecycle_line(&line) {
                        tracing::debug!(target: "wcore_agent::hooks", "{line}");
                    } else {
                        eprintln!("{line}");
                    }
                }
                pre_outcome = outcome;
            }
            Err(e) => {
                let reason = e.to_string();
                if let Some((started, _)) = started_hook {
                    let unknown = started.abandon_unknown().map_or_else(
                        |journal_error| journal_authority_failure(id, journal_error),
                        |()| {
                            journal_authority_failure(
                                id,
                                format!("pre-tool hook outcome is unknown: {reason}"),
                            )
                        },
                    );
                    return (unknown, None, pre_outcome, false);
                }
                let durable = record_tool_not_started(
                    effect_scope,
                    id,
                    ordinal,
                    name,
                    input,
                    &effective_input,
                    registry
                        .get(name)
                        .map(|tool| tool.effect_contract(&effective_input))
                        .unwrap_or_default(),
                    ToolNotStartedReason::HookDenied {
                        reason: reason.clone(),
                    },
                    None,
                );
                let block = durable.map_or_else(
                    |error| journal_authority_failure(id, error),
                    |()| ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: crate::output_redaction::redact_tool_output(&format!(
                            "Blocked by hook: {reason}"
                        )),
                        is_error: true,
                    },
                );
                return (block, None, pre_outcome, false);
            }
        }
    }

    // Set true only when the dispatch timeout-cancel path below wins, so the
    // engine can flag the synthesized result's trace as cancelled.
    let mut was_cancelled = false;
    let (result, modifier, prepared_post_hook, durable_tool_result_digest) = match registry
        .get(name)
    {
        Some(tool) => {
            let max_size = tool.max_result_size();
            let effect_contract = tool.effect_contract(&effective_input);
            if let Some(retry) = recovered_retry
                && retry.effect_contract != &effect_contract
            {
                return (
                    journal_authority_failure(
                        id,
                        "recovered tool retry effect contract does not match the durable attempt",
                    ),
                    None,
                    pre_outcome,
                    false,
                );
            }
            // AUDIT B-1 follow-up — pick the timeout category based on
            // THIS call's input, not just the tool's bare `category()`.
            // SkillTool is `Info` (30s) for inline skills (returns
            // SKILL.md text — should be fast) but `Exec` (600s) for
            // fork-mode skills that spawn a sub-agent and can legitimately
            // run many turns. `category_for` defaults to `category()` so
            // every other tool stays byte-identical.
            let category = tool.category_for(&effective_input);
            // AUDIT B-4: consult the per-tool circuit breaker BEFORE
            // dispatch. The breaker lives on `ToolRegistry`; the agent
            // loop previously bypassed it entirely by calling
            // `registry.get()` + `execute_with_ctx()` directly. A tool
            // that trips the breaker (3 failures in 30s) short-circuits
            // here with an error `ToolResult` instead of being hammered
            // every turn — pairs with the B-1 timeout so a flaky MCP
            // server is both bounded per-call AND backed off across
            // calls.
            if registry.breaker_is_open(name) {
                if let Err(error) = record_tool_attempt_not_started(
                    effect_scope,
                    id,
                    ordinal,
                    name,
                    input,
                    &effective_input,
                    effect_contract.clone(),
                    ToolNotStartedReason::CircuitOpen,
                    pre_hook_phase_id.as_deref(),
                    recovered_retry,
                ) {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!(
                            "Tool '{name}' circuit open: too many recent failures, \
                             try again later"
                        ),
                        is_error: true,
                    },
                    None,
                    pre_outcome,
                    false,
                );
            }
            if cancel.is_cancelled() {
                let reason = "session cancellation was requested before tool dispatch".to_string();
                if let Err(error) = record_tool_attempt_not_started(
                    effect_scope,
                    id,
                    ordinal,
                    name,
                    input,
                    &effective_input,
                    effect_contract.clone(),
                    ToolNotStartedReason::Cancelled {
                        reason: reason.clone(),
                    },
                    pre_hook_phase_id.as_deref(),
                    recovered_retry,
                ) {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Tool '{name}' was not started: {reason}"),
                        is_error: true,
                    },
                    None,
                    pre_outcome,
                    false,
                );
            }
            // W7 F4: route through execute_streaming when:
            //   1) the tool supports streaming (Bash today)
            //   2) the caller supplied a StreamingContext
            //   3) the parent sink has advertised streaming_tools (i.e.
            //      ProtocolSink::with_streaming_tools(true))
            // AUDIT B-1: route through ctx-aware entry points with a
            // LIVE child of the session-root cancellation token. Before
            // this fix the dispatcher minted `ToolContext::test_default()`
            // — a fresh, never-cancelled stub — so cooperative tools
            // (BashTool, McpToolProxy) never observed a host cancel.
            // The child token also fires on the dispatch timeout below,
            // giving a cooperative tool the chance to wind down before
            // the future is dropped.
            let call_cancel = cancel.child_token();
            // The filesystem tools see is the registry's configured vfs when
            // one is installed (e.g. a channel `Workspace` engine pins a
            // `SandboxedFs` jail here), else an unconfined `RealFs` — the
            // local-CLI default. Carried on the registry so no new parameter
            // has to thread through the whole dispatch stack.
            let tool_vfs = registry
                .tool_vfs()
                .unwrap_or_else(|| std::sync::Arc::new(wcore_tools::vfs::RealFs));
            let mut tool_ctx = wcore_tools::context::ToolContext::new(
                id.clone(),
                call_cancel.clone(),
                tool_vfs,
                None,
                std::sync::Arc::new(wcore_tools::NullToolOutputSink),
            );
            if let Some(n) = file_write_notifier {
                tool_ctx = tool_ctx.with_file_write_notifier(std::sync::Arc::clone(n));
            }
            if let Some(policy) = registry.workspace_policy() {
                tool_ctx = tool_ctx.with_workspace(policy);
            }
            tool_ctx = tool_ctx.with_sandbox(registry.sandbox_runtime());
            // F11: classify and admit the concrete call before dispatch. This
            // is not derived from ToolCategory: authority classification and
            // host-process consumption are different concerns. The RAII guard
            // also commits partial runtime on timeout/cancellation.
            let execution_class = tool.execution_class_for(&effective_input);
            let category_timeout = tool_dispatch_timeout(category);
            let _budget_guard = match budget {
                Some(tracker) => match tracker.try_start(name, execution_class, category_timeout) {
                    Ok(guard) => Some(guard),
                    Err(error) => {
                        if let Err(journal_error) = record_tool_attempt_not_started(
                            effect_scope,
                            id,
                            ordinal,
                            name,
                            input,
                            &effective_input,
                            effect_contract.clone(),
                            ToolNotStartedReason::BudgetDenied {
                                reason: error.reason.to_string(),
                            },
                            pre_hook_phase_id.as_deref(),
                            recovered_retry,
                        ) {
                            return (
                                journal_authority_failure(id, journal_error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                        return (
                            ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: format!(
                                    "Tool '{name}' was not started: budget cap '{}' would be \
                                     exceeded (limit {}, observed {}).",
                                    error.reason, error.limit, error.observed
                                ),
                                is_error: true,
                            },
                            None,
                            pre_outcome,
                            false,
                        );
                    }
                },
                None => None,
            };
            let prepared_runtime =
                match AssertUnwindSafe(tool.prepare_effect(&effective_input, &tool_ctx))
                    .catch_unwind()
                    .await
                {
                    Ok(Ok(prepared)) => prepared,
                    Ok(Err(result)) => {
                        let error = crate::output_redaction::redact_tool_output(&result.content);
                        if let Err(journal_error) = record_tool_attempt_not_started(
                            effect_scope,
                            id,
                            ordinal,
                            name,
                            input,
                            &effective_input,
                            effect_contract.clone(),
                            ToolNotStartedReason::InvalidInput {
                                error: error.clone(),
                            },
                            pre_hook_phase_id.as_deref(),
                            recovered_retry,
                        ) {
                            return (
                                journal_authority_failure(id, journal_error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                        return (
                            ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: error,
                                is_error: result.is_error,
                            },
                            None,
                            pre_outcome,
                            false,
                        );
                    }
                    Err(payload) => {
                        let error = crate::output_redaction::redact_tool_output(
                            &extract_panic_message(&payload),
                        );
                        if let Err(journal_error) = record_tool_attempt_not_started(
                            effect_scope,
                            id,
                            ordinal,
                            name,
                            input,
                            &effective_input,
                            effect_contract.clone(),
                            ToolNotStartedReason::DispatchFailed {
                                error: error.clone(),
                            },
                            pre_hook_phase_id.as_deref(),
                            recovered_retry,
                        ) {
                            return (
                                journal_authority_failure(id, journal_error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                        return (
                            ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: format!("Tool effect preparation panicked: {error}"),
                                is_error: true,
                            },
                            None,
                            pre_outcome,
                            false,
                        );
                    }
                };
            let durable_receipt = match prepared_runtime.as_ref() {
                Some(prepared) => match prepared.durable_receipt() {
                    Ok(receipt) => Some(receipt),
                    Err(error) => {
                        let reason =
                            format!("prepared effect receipt could not be encoded: {error}");
                        if let Err(journal_error) = record_tool_attempt_not_started(
                            effect_scope,
                            id,
                            ordinal,
                            name,
                            input,
                            &effective_input,
                            effect_contract.clone(),
                            ToolNotStartedReason::DispatchFailed {
                                error: reason.clone(),
                            },
                            pre_hook_phase_id.as_deref(),
                            recovered_retry,
                        ) {
                            return (
                                journal_authority_failure(id, journal_error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                        return (
                            ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content: reason,
                                is_error: true,
                            },
                            None,
                            pre_outcome,
                            false,
                        );
                    }
                },
                None => None,
            };
            if let Err(error) =
                store_prepared_effect_checkpoint(effect_scope, prepared_runtime.as_ref()).await
            {
                let reason = crate::output_redaction::redact_tool_output(&error);
                if let Err(journal_error) = record_tool_attempt_not_started(
                    effect_scope,
                    id,
                    ordinal,
                    name,
                    input,
                    &effective_input,
                    effect_contract.clone(),
                    ToolNotStartedReason::DispatchFailed {
                        error: reason.clone(),
                    },
                    pre_hook_phase_id.as_deref(),
                    recovered_retry,
                ) {
                    return (
                        journal_authority_failure(id, journal_error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: reason,
                        is_error: true,
                    },
                    None,
                    pre_outcome,
                    false,
                );
            }
            #[cfg(test)]
            inject_dispatcher_crash(DispatcherCrashCut::BeforePrepared);
            let prepared_effect = match recovered_retry {
                Some(retry) if durable_receipt.as_ref() != retry.effect_receipt => Err(
                    "recovered tool retry effect receipt does not match the durable attempt"
                        .to_string(),
                ),
                Some(retry) => effect_scope
                    .ok_or_else(|| "recovered tool retry has no durable effect scope".to_string())
                    .and_then(|scope| {
                        scope
                            .retry_not_started_tool(retry.prior_tool_execution_id)
                            .map(Some)
                            .map_err(|error| {
                                format!(
                                    "durable recovered tool retry could not be recorded: {error}"
                                )
                            })
                    }),
                None => prepare_tool_effect(
                    effect_scope,
                    id,
                    ordinal,
                    name,
                    input,
                    &effective_input,
                    effect_contract.clone(),
                    durable_receipt,
                    pre_hook_phase_id.as_deref(),
                ),
            };
            let prepared_effect = match prepared_effect {
                Ok(lease) => lease,
                Err(error) => {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
            };
            let prepared_post_hook = match hooks {
                Some(hook_engine) => prepare_hook_authority(
                    effect_scope,
                    hook_engine,
                    HookAuthorityRequest {
                        provider_call_id: id,
                        ordinal,
                        phase: ToolHookPhase::PostToolUse,
                        tool_execution_id: prepared_effect
                            .as_ref()
                            .map(|lease| lease.id().to_string()),
                        tool_name: name,
                        tool_input: &effective_input,
                    },
                ),
                None => Ok(None),
            };
            let mut prepared_post_hook = match prepared_post_hook {
                Ok(prepared) => prepared,
                Err(error) => {
                    if let Some(prepared) = prepared_effect {
                        let _ = prepared.not_started(ToolNotStartedReason::DispatchFailed {
                            error: error.clone(),
                        });
                    }
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
            };
            #[cfg(test)]
            inject_dispatcher_crash(DispatcherCrashCut::AfterPrepared);
            if cancel.is_cancelled() {
                let reason =
                    "session cancellation was requested before physical tool start".to_string();
                if let Some(prepared) = prepared_post_hook.take()
                    && let Err(error) = prepared.lease.not_applicable()
                {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                if let Some(prepared) = prepared_effect
                    && let Err(error) = prepared.not_started(ToolNotStartedReason::Cancelled {
                        reason: reason.clone(),
                    })
                {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Tool '{name}' was not started: {reason}"),
                        is_error: true,
                    },
                    None,
                    pre_outcome,
                    false,
                );
            }
            let effect_context =
                prepared_effect
                    .as_ref()
                    .map(|lease| wcore_tools::context::ToolEffectContext {
                        tool_execution_id: lease.id().to_string(),
                        idempotency_key: lease.idempotency_key().to_string(),
                    });
            #[cfg(test)]
            inject_dispatcher_crash(DispatcherCrashCut::BeforeRunning);
            let started_effect = match prepared_effect.map(PreparedToolLease::start).transpose() {
                Ok(lease) => lease,
                Err(error) => {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
            };
            #[cfg(test)]
            inject_dispatcher_crash(DispatcherCrashCut::AfterRunning);
            // Wave RB RELIABILITY MAJOR: wrap every tool dispatch in
            // `FutureExt::catch_unwind` so a panic inside the tool's
            // future (programming bug, divide-by-zero, slice OOB, etc.)
            // is caught at the dispatcher rather than propagating up
            // through `JoinError` and crashing the orchestration loop.
            // `AssertUnwindSafe` is safe here because the inner future
            // does not retain any state across the panic point — the
            // tool reference, `tool_ctx`, and `effective_input` are
            // either re-used (in the error-path code below) or dropped
            // after this match completes. On panic we synthesise a
            // `ToolResult { is_error: true }` so the LLM context
            // observes a normal tool failure; the session continues.
            // The streaming `StreamingContext`'s sink receives a
            // `ToolPanicked` event for the host's typed diagnostic
            // surface.
            //
            // AUDIT B-1 / B-8: the panic-safe dispatch future is itself
            // wrapped in `tokio::time::timeout` keyed on the tool's
            // category. A wedged tool (hung MCP subprocess, slow HTTP
            // endpoint, blocked syscall) elapses its deadline, the
            // call's cancel token is fired so a cooperative tool can
            // wind down, and an error `ToolResult` is synthesised — the
            // `tool_use` still gets a `tool_result` and the agent loop
            // continues instead of hanging forever.
            let dispatch_fut = async {
                if let Some(prepared) = prepared_runtime {
                    AssertUnwindSafe(tool.execute_prepared_effect(prepared, &tool_ctx))
                        .catch_unwind()
                        .await
                        .map(|execution| {
                            (
                                execution.result,
                                Some((execution.disposition, execution.observed_receipt)),
                            )
                        })
                } else if let Some(ctx) = streaming.as_ref() {
                    if tool.supports_streaming() && ctx.output.streaming_tools_advertised() {
                        let sink = ProtocolToolSink {
                            output: std::sync::Arc::clone(&ctx.output),
                            msg_id: ctx.msg_id.clone(),
                            call_id: id.clone(),
                            tool_name: name.clone(),
                            redactor: Mutex::new(crate::output_redaction::StreamingRedactor::new()),
                        };
                        AssertUnwindSafe(tool.execute_streaming_with_effect_ctx(
                            effective_input.clone(),
                            &tool_ctx,
                            effect_context.as_ref(),
                            &sink,
                        ))
                        .catch_unwind()
                        .await
                        .map(|result| (result, None))
                    } else {
                        AssertUnwindSafe(tool.execute_with_effect_ctx(
                            effective_input.clone(),
                            &tool_ctx,
                            effect_context.as_ref(),
                        ))
                        .catch_unwind()
                        .await
                        .map(|result| (result, None))
                    }
                } else {
                    AssertUnwindSafe(tool.execute_with_effect_ctx(
                        effective_input.clone(),
                        &tool_ctx,
                        effect_context.as_ref(),
                    ))
                    .catch_unwind()
                    .await
                    .map(|result| (result, None))
                }
            };
            let timeout = _budget_guard
                .as_ref()
                .and_then(crate::tool_budget::ToolRunHandle::dispatch_time_limit)
                .unwrap_or(category_timeout);
            let timed: Result<ToolDispatchResult, tokio::time::error::Elapsed> =
                tokio::time::timeout(timeout, dispatch_fut).await;
            #[cfg(test)]
            inject_dispatcher_crash(DispatcherCrashCut::AfterPhysicalEffect);
            let mut unknown_effect = None;
            let (r, observed_effect) = match timed {
                Err(_elapsed) => {
                    // Dispatch exceeded its category deadline. Fire the
                    // call's cancel token so a cooperative tool can
                    // abort its own work, then synthesise an error
                    // result so the LLM still sees a paired tool_result.
                    call_cancel.cancel();
                    // Flag the trace: this result was synthesized by the
                    // cancel path, not produced by a completed tool run.
                    was_cancelled = true;
                    let secs = timeout.as_secs_f64();
                    eprintln!(
                        "[tool-timeout] tool={} call_id={} category={:?} elapsed>{:.3}s",
                        name, id, category, secs
                    );
                    if let Some(ctx) = streaming.as_ref() {
                        ctx.output.emit_tool_panicked(
                            &ctx.msg_id,
                            id,
                            name,
                            &format!("timed out after {secs:.3}s"),
                        );
                    }
                    unknown_effect = Some((
                        ToolUnknownReason::TimedOut {
                            timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
                        },
                        serde_json::json!({
                            "tool": name,
                            "call_id": id,
                            "timeout_ms": timeout.as_millis(),
                        }),
                    ));
                    (
                        ToolResult {
                            content: format!(
                                "Tool '{name}' timed out after {secs:.3}s and was cancelled. \
                                 The operation may be hung; consider a narrower request."
                            ),
                            is_error: true,
                        },
                        None,
                    )
                }
                Ok(Ok(result)) => result,
                Ok(Err(payload)) => {
                    let panic_message = crate::output_redaction::redact_tool_output(
                        &extract_panic_message(&payload),
                    );
                    eprintln!(
                        "[tool-panic] tool={} call_id={} panic={}",
                        name, id, panic_message
                    );
                    if let Some(ctx) = streaming.as_ref() {
                        ctx.output
                            .emit_tool_panicked(&ctx.msg_id, id, name, &panic_message);
                    }
                    unknown_effect = Some((
                        ToolUnknownReason::Panicked {
                            message: panic_message.clone(),
                        },
                        serde_json::json!({
                            "tool": name,
                            "call_id": id,
                            "panic": panic_message.clone(),
                        }),
                    ));
                    (
                        ToolResult {
                            content: format!(
                                "Tool panicked; session continuing. Panic: {}",
                                panic_message
                            ),
                            is_error: true,
                        },
                        None,
                    )
                }
            };
            if matches!(
                observed_effect.as_ref().map(|(disposition, _)| disposition),
                Some(wcore_tools::effects::ToolEffectDisposition::Unknown)
            ) {
                unknown_effect = Some((
                    ToolUnknownReason::AmbiguousFailure {
                        error: crate::output_redaction::redact_tool_output(&r.content),
                    },
                    observed_effect
                        .as_ref()
                        .map(|(_, receipt)| receipt.clone())
                        .unwrap_or_else(|| serde_json::json!({"outcome":"unknown"})),
                ));
            }
            if unknown_effect.is_none() && call_cancel.is_cancelled() && r.is_error {
                unknown_effect = Some((
                    ToolUnknownReason::Cancelled {
                        reason: "tool cancellation observed after durable start".to_string(),
                    },
                    serde_json::json!({
                        "tool": name,
                        "call_id": id,
                        "cancelled": true,
                    }),
                ));
            }
            if unknown_effect.is_none()
                && r.is_error
                && (matches!(effect_contract.kind, ToolEffectKind::Opaque)
                    || matches!(effect_contract.kind, ToolEffectKind::ProviderIdempotent)
                    || (matches!(
                        effect_contract.kind,
                        ToolEffectKind::FilesystemTransactional
                    ) && observed_effect.is_none()))
            {
                unknown_effect = Some((
                    ToolUnknownReason::AmbiguousFailure {
                        error: crate::output_redaction::redact_tool_output(&r.content),
                    },
                    serde_json::json!({
                        "tool": name,
                        "call_id": id,
                        "reported_error": true,
                    }),
                ));
            }
            // AUDIT B-4: record the dispatch outcome against the
            // breaker. A timeout or panic counts as a failure (synthetic
            // `is_error: true` results above), so a tool that keeps
            // wedging eventually trips the breaker and is short-circuited
            // on the next turn.
            registry.record_breaker_outcome(name, r.is_error);
            // _budget_guard drops here, recording elapsed runtime.
            let modifier = if r.is_error {
                None
            } else {
                tool.context_modifier_for(&effective_input)
            };
            // Redact the original result before any truncation or compaction.
            // Otherwise a secret crossing the truncation boundary can be cut
            // into a non-matching prefix and escape every downstream scrub.
            let redacted_content = crate::output_redaction::redact_tool_output(&r.content);
            let error_content = if r.is_error && tool.is_deferred() {
                maybe_append_deferred_hint(&redacted_content, tool.input_schema(), &effective_input)
            } else {
                redacted_content
            };
            let content = truncate_result(&error_content, max_size);
            let content = wcore_compact::compact_output(&content, compaction_level);
            let content = if toon_enabled {
                wcore_compact::compact_output_toon(&content)
            } else {
                content
            };
            let content = crate::output_redaction::redact_tool_output(&content);
            let mut durable_tool_result_digest = None;
            if let Some(lease) = started_effect {
                let durable_result = serde_json::json!({
                    "content": content.clone(),
                    "is_error": r.is_error,
                    "effect_receipt": observed_effect
                        .as_ref()
                        .map(|(_, receipt)| receipt.clone()),
                });
                let result_digest = state_payload_digest(&durable_result)
                    .map_err(|error| format!("tool result could not be digested: {error}"));
                let result_digest = match result_digest {
                    Ok(digest) => digest,
                    Err(error) => {
                        return (
                            journal_authority_failure(id, error),
                            None,
                            pre_outcome,
                            false,
                        );
                    }
                };
                #[cfg(test)]
                inject_dispatcher_crash(DispatcherCrashCut::BeforeTerminalAppend);
                let journal_result = if let Some((reason, evidence)) = unknown_effect {
                    lease.unknown(reason, evidence).map(|_| ())
                } else if r.is_error {
                    durable_tool_result_digest = Some(result_digest);
                    lease.fail(content.clone(), durable_result)
                } else {
                    durable_tool_result_digest = Some(result_digest);
                    lease.succeed(durable_result)
                };
                if let Err(error) = journal_result {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                #[cfg(test)]
                inject_dispatcher_crash(DispatcherCrashCut::AfterTerminalAppend);
            }
            (
                ToolResult {
                    content,
                    is_error: r.is_error,
                },
                modifier,
                prepared_post_hook,
                durable_tool_result_digest,
            )
        }
        None => {
            let journal_result = prepare_tool_effect(
                effect_scope,
                id,
                ordinal,
                name,
                input,
                &effective_input,
                wcore_types::tool::ToolEffectContract::default(),
                None,
                pre_hook_phase_id.as_deref(),
            )
            .and_then(|lease| {
                lease
                    .map(|lease| lease.not_started(ToolNotStartedReason::UnknownTool))
                    .transpose()
                    .map_err(|error| error.to_string())
                    .map(|_| ())
            });
            if let Err(error) = journal_result {
                return (
                    journal_authority_failure(id, error),
                    None,
                    pre_outcome,
                    false,
                );
            }
            if effect_scope.is_some() {
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Unknown tool: {}", name),
                        is_error: true,
                    },
                    None,
                    pre_outcome,
                    false,
                );
            }
            (
                ToolResult {
                    content: format!("Unknown tool: {}", name),
                    is_error: true,
                },
                None,
                None,
                None,
            )
        }
    };

    // Run post-tool-use hooks
    let mut post_outcome = crate::hooks::HookOutcome::default();
    if let Some(hook_engine) = hooks {
        let mut outcome = if let Some(prepared) = prepared_post_hook {
            let Some(result_digest) = durable_tool_result_digest else {
                if let Err(error) = prepared
                    .lease
                    .not_started(HookPhaseNotStartedReason::ToolOutcomeUnknown)
                {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: result.content,
                        is_error: result.is_error,
                    },
                    modifier,
                    pre_outcome,
                    was_cancelled,
                );
            };
            let started = match prepared.lease.start(Some(result_digest)) {
                Ok(started) => started,
                Err(error) => {
                    return (
                        journal_authority_failure(id, error),
                        None,
                        pre_outcome,
                        false,
                    );
                }
            };
            match hook_engine
                .run_post_tool_use_strict(
                    name,
                    id,
                    &effective_input,
                    &result.content,
                    result.is_error,
                )
                .await
            {
                Ok(mut outcome) => {
                    match finish_hook_authority(started, prepared.slots, &outcome, None) {
                        Ok(consumption) => outcome.durable_hook_phases.push(consumption),
                        Err(error) => {
                            return (
                                journal_authority_failure(id, error),
                                None,
                                pre_outcome,
                                false,
                            );
                        }
                    }
                    outcome
                }
                Err(error) => {
                    let failure = error.to_string();
                    let unknown = started.abandon_unknown().map_or_else(
                        |journal_error| journal_authority_failure(id, journal_error),
                        |()| {
                            journal_authority_failure(
                                id,
                                format!("post-tool hook outcome is unknown: {failure}"),
                            )
                        },
                    );
                    return (unknown, None, pre_outcome, false);
                }
            }
        } else {
            hook_engine
                .run_post_tool_use(name, id, &effective_input, &result.content, result.is_error)
                .await
        };
        // v0.9.1.2 F10: hook lifecycle telemetry goes to `tracing::debug!`
        // ONLY — never eprintln! (which leaks into the TUI alt-screen and
        // overlaps the composer/transcript). `hook_trace` is the
        // architectural new home for plugin-hook fire lines + rust-hook
        // "action ignored" diagnostics. `log_lines` is the only place
        // shell-hook stdout lands; we filter it through
        // `is_hook_lifecycle_line` as belt-and-suspenders in case any
        // future code path pushes a lifecycle line there.
        for msg in outcome.hook_trace.drain(..) {
            let msg = crate::output_redaction::redact_tool_output(&msg);
            tracing::debug!(target: "wcore_agent::hooks", "{msg}");
        }
        for msg in outcome.log_lines.drain(..) {
            let msg = crate::output_redaction::redact_tool_output(&msg);
            if is_hook_lifecycle_line(&msg) {
                tracing::debug!(target: "wcore_agent::hooks", "{msg}");
            } else {
                eprintln!("{msg}");
            }
        }
        // injected_messages and switch_model bubble up via
        // ToolCallOutcome.hook_outcomes; the agent-level engine applies
        // them through apply_turn_end_outcome.
        post_outcome = outcome;
    }

    pre_outcome
        .injected_messages
        .append(&mut post_outcome.injected_messages);
    if post_outcome.switch_model.is_some() {
        pre_outcome.switch_model = post_outcome.switch_model.take();
    }
    pre_outcome
        .fired_actions
        .append(&mut post_outcome.fired_actions);
    pre_outcome
        .durable_hook_phases
        .append(&mut post_outcome.durable_hook_phases);

    (
        ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: result.content,
            is_error: result.is_error,
        },
        modifier,
        pre_outcome,
        was_cancelled,
    )
}

/// Execute tool calls with JSON stream protocol approval flow
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_approval(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_and_budget(
        registry,
        tool_calls,
        approval_manager,
        writer,
        msg_id,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        None,
        cancel,
        file_write_notifier,
    )
    .await
}

/// Approval-backed dispatch with the same per-tool budget accounting used by
/// the terminal path. The legacy entry point delegates here with no tracker.
#[allow(clippy::too_many_arguments)]
pub async fn execute_tool_calls_with_approval_and_budget(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_budget_and_effects(
        registry,
        tool_calls,
        approval_manager,
        writer,
        msg_id,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        budget,
        cancel,
        file_write_notifier,
        None,
        None,
    )
    .await
}

/// Host-approval variant of the F13 production dispatch entry point.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_tool_calls_with_approval_budget_and_effects(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: Option<&TurnEffectScope>,
    effect_ordinals: Option<&[u64]>,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_budget_effects_inner(
        registry,
        tool_calls,
        approval_manager,
        writer,
        msg_id,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        budget,
        cancel,
        file_write_notifier,
        effect_scope,
        effect_ordinals,
        None,
        None,
    )
    .await
}

/// Execute exactly one recovered tool call whose original approval has already
/// been resolved in the durable journal. The call id remains input-bound and
/// no broader live approval mode is inferred from the recovered decision.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_recovered_approved_tool_call_with_effects(
    registry: &ToolRegistry,
    tool_call: &ContentBlock,
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: &TurnEffectScope,
    effect_ordinal: u64,
    recovered_approval_call_id: &str,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_budget_effects_inner(
        registry,
        std::slice::from_ref(tool_call),
        approval_manager,
        writer,
        msg_id,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        budget,
        cancel,
        file_write_notifier,
        Some(effect_scope),
        Some(std::slice::from_ref(&effect_ordinal)),
        Some(recovered_approval_call_id),
        None,
    )
    .await
}

#[derive(Clone)]
struct RecoveredToolRetry<'a> {
    call_id: &'a str,
    prior_tool_execution_id: &'a str,
    tool: &'a str,
    ordinal: u64,
    effect_contract: &'a wcore_types::tool::ToolEffectContract,
    effect_receipt: Option<&'a serde_json::Value>,
    requested_input_digest: &'a str,
    effective_input_digest: &'a str,
    pre_hook_phase_id: Option<&'a str>,
    pre_hook_consumption: Option<HookPhaseConsumption>,
}

/// Execute a crash-proven no-start retry as a linked F13 attempt. The prior
/// receipt must match the freshly prepared runtime receipt before the retry
/// lease is recorded, preserving the original physical-effect contract.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_recovered_retry_tool_call_with_effects(
    registry: &ToolRegistry,
    tool_call: &ContentBlock,
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: &TurnEffectScope,
    effect_ordinal: u64,
    recovered_approval_call_id: Option<&str>,
    call_id: &str,
    prior_tool_execution_id: &str,
    tool: &str,
    prior_ordinal: u64,
    effect_contract: &wcore_types::tool::ToolEffectContract,
    effect_receipt: Option<&serde_json::Value>,
    requested_input_digest: &str,
    effective_input_digest: &str,
    pre_hook_phase_id: Option<&str>,
    pre_hook_consumption: Option<HookPhaseConsumption>,
) -> Result<ToolCallOutcome, ExecutionControl> {
    execute_tool_calls_with_approval_budget_effects_inner(
        registry,
        std::slice::from_ref(tool_call),
        approval_manager,
        writer,
        msg_id,
        allow_list,
        hooks,
        compaction_level,
        toon_enabled,
        budget,
        cancel,
        file_write_notifier,
        Some(effect_scope),
        Some(std::slice::from_ref(&effect_ordinal)),
        recovered_approval_call_id,
        Some(RecoveredToolRetry {
            call_id,
            prior_tool_execution_id,
            tool,
            ordinal: prior_ordinal,
            effect_contract,
            effect_receipt,
            requested_input_digest,
            effective_input_digest,
            pre_hook_phase_id,
            pre_hook_consumption,
        }),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_tool_calls_with_approval_budget_effects_inner(
    registry: &ToolRegistry,
    tool_calls: &[ContentBlock],
    approval_manager: &Arc<ToolApprovalManager>,
    writer: &Arc<dyn ProtocolEmitter>,
    msg_id: &str,
    allow_list: &[String],
    mut hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
    effect_scope: Option<&TurnEffectScope>,
    effect_ordinals: Option<&[u64]>,
    recovered_approval_call_id: Option<&str>,
    recovered_retry: Option<RecoveredToolRetry<'_>>,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut hook_outcomes = Vec::new();
    // `tool_use` ids whose result came from the dispatch timeout-cancel path.
    let mut cancelled_ids = Vec::new();

    for (ordinal, call) in tool_calls.iter().enumerate() {
        let ContentBlock::ToolUse {
            id, name, input, ..
        } = call
        else {
            continue;
        };

        let tool = registry.get(name);
        let category = tool
            .map(|t| t.category_for(input))
            .unwrap_or(ToolCategory::Exec);
        let description = tool.map(|t| t.describe(input)).unwrap_or_default();

        // Check if approval is needed. W0: thread the shell command string
        // (Bash `command` input) into the gate so a prefix-scoped allow rule
        // (`ApprovalScope::AlwaysPrefix`) can auto-approve only commands whose
        // head matches the stored prefix — not the whole exec category.
        let command = input.get("command").and_then(|v| v.as_str());
        // v0.9.3 W8 H2-integration: AskUserQuestion ALWAYS needs approval —
        // even in AutoEdit mode, where the `Info` category is auto-approved.
        // Without this carve-out, an AskUser tool call in AutoEdit mode skips
        // the approval gate, hits AskUserQuestionTool::execute()'s loud
        // is_error: true fallback, and the LLM sees an error result for a
        // question it asked. The mode-cycle to AutoEdit (Shift+Tab) is a
        // normal user action, so this is reachable.
        // W5.6 H-2: also check the tool-name-scoped always-allow set so
        // `ApprovalScope::Always` on "Bash" auto-approves only future Bash
        // calls, not every Exec-category tool (Write, Edit, etc.).
        let tool_name_approved = allow_list.contains(&name.to_string())
            || approval_manager.is_tool_name_auto_approved(name);
        // The shared manager is the sole live posture authority for host-backed
        // sessions. A boot-time confirmer snapshot would make Force -> Default
        // de-escalation cosmetic while tools continued to bypass approval.
        let globally_approved = approval_manager.current_mode() == "force";
        let scoped_auto_approval = !globally_approved
            && !tool_name_approved
            && approval_manager.is_auto_approved_tool_cmd(
                &category.to_string(),
                Some(name),
                command,
            );
        let recovered_approval = recovered_approval_call_id == Some(id.as_str());
        if recovered_approval_call_id.is_some() && !recovered_approval {
            tracing::error!(
                target: "wcore_agent::orchestration",
                expected_call_id = recovered_approval_call_id,
                actual_call_id = %id,
                "recovered approval authority did not match the dispatched tool call"
            );
            return Err(ExecutionControl::Quit);
        }
        let recovered_retry_for_call = match recovered_retry.as_ref() {
            Some(retry) if retry.call_id != id => {
                tracing::error!(
                    target: "wcore_agent::orchestration",
                    expected_call_id = retry.call_id,
                    actual_call_id = %id,
                    "recovered retry authority did not match the dispatched tool call"
                );
                return Err(ExecutionControl::Quit);
            }
            retry => retry,
        };
        let needs_approval = !recovered_approval
            && (name == "AskUserQuestion"
                || (!globally_approved && !tool_name_approved && !scoped_auto_approval));
        // Category-, mode- and prefix-scoped rules authorize the arguments
        // that matched them. A hook may still mutate input after a global or
        // tool-name-wide grant, but may not widen an input-scoped grant.
        let approval_bound = recovered_approval || needs_approval || scoped_auto_approval;

        if needs_approval {
            let approval_intent = serde_json::json!({
                "provider_call_id": id,
                "tool": name,
                "category": category.to_string(),
                "input": input,
            });
            let mut durable_approval = match effect_scope
                .map(|scope| scope.request_approval_with_id(id.clone(), &approval_intent))
                .transpose()
            {
                Ok(lease) => lease,
                Err(error) => {
                    results.push(journal_authority_failure(id, error));
                    modifiers.push(None);
                    hook_outcomes.push(crate::hooks::HookOutcome::default());
                    continue;
                }
            };
            #[cfg(test)]
            inject_approval_crash(ApprovalCrashCut::AfterRequested);

            // Register the pending request before emission. A local desktop
            // host can answer synchronously from `emit`; emitting first would
            // race that response against pending-map installation and lose it.
            let rx = approval_manager.request_approval(id, &category, name);
            if writer
                .emit(&ProtocolEvent::ToolRequest {
                    msg_id: msg_id.to_string(),
                    call_id: id.clone(),
                    tool: ToolInfo {
                        name: name.clone(),
                        category,
                        args: input.clone(),
                        description,
                    },
                })
                .is_err()
            {
                approval_manager.drop_pending(id);
                if let Some(lease) = durable_approval.take() {
                    let _ = lease.resolve(ApprovalResolution::Cancelled);
                }
                return Err(ExecutionControl::Quit);
            }

            // AUDIT B-7 / D-5: race the approval await against the
            // session-root cancel token. Before this fix a turn
            // cancelled (`Esc`) while parked here dropped the future
            // and leaked the `PendingApproval` entry forever. Now a
            // cancel resolves the await deterministically; we also call
            // `drop_pending` so the manager's map does not retain a
            // stale `Sender` (belt-and-suspenders with the B-2 reaper).
            let approval = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    approval_manager.drop_pending(id);
                    if let Some(lease) = durable_approval.take() {
                        let _ = lease.resolve(ApprovalResolution::Cancelled);
                    }
                    return Err(ExecutionControl::Quit);
                }
                res = rx => res,
            };
            let durable_resolution = match &approval {
                Ok(ToolApprovalResult::Approved { .. }) => ApprovalResolution::Decided {
                    decision: ApprovalDecision::AllowOnce,
                },
                Ok(ToolApprovalResult::Denied { reason })
                    if reason == "approval timed out (no host response)" =>
                {
                    ApprovalResolution::TimedOut
                }
                Ok(ToolApprovalResult::Denied { .. }) => ApprovalResolution::Decided {
                    decision: ApprovalDecision::Deny,
                },
                Err(_) => ApprovalResolution::Cancelled,
            };
            #[cfg(test)]
            inject_approval_crash(ApprovalCrashCut::BeforeResolved);
            if let Some(lease) = durable_approval.take()
                && let Err(error) = lease.resolve(durable_resolution)
            {
                results.push(journal_authority_failure(id, error));
                modifiers.push(None);
                hook_outcomes.push(crate::hooks::HookOutcome::default());
                continue;
            }
            #[cfg(test)]
            inject_approval_crash(ApprovalCrashCut::AfterResolved);
            match approval {
                Ok(ToolApprovalResult::Approved { answer: Some(s) })
                    if name == "AskUserQuestion" =>
                {
                    // v0.9.3 W0.3 — answer routed through approval channel
                    // synthesizes the tool result directly (bypassing
                    // dispatch). Scoped to AskUserQuestion only: the user's
                    // choice IS the tool's output, and dispatch's
                    // AskUserQuestionTool::execute() is a loud-defensive
                    // `is_error: true` fallback (W0.4) anyway.
                    //
                    // v0.9.3 W8 H1-reliability — tool-name guard added:
                    // before this guard, ANY tool's `Approved { answer }`
                    // would synthesize, letting a buggy/compromised host
                    // fabricate arbitrary "tool output" for Bash/Edit/Write.
                    // Non-AskUserQuestion `Approved { answer: Some(_) }`
                    // now falls through to dispatch (see arm below) so the
                    // tool actually runs.
                    //
                    // v0.9.4 W3a — belt-and-suspenders: in debug/test builds
                    // this fires immediately if a refactor ever routes another
                    // tool into this arm. In release it is a no-op.
                    debug_assert!(
                        name == "AskUserQuestion",
                        "synth arm reached for non-AskUser tool: {name}"
                    );
                    let _ = writer.emit(&ProtocolEvent::ToolRunning {
                        msg_id: msg_id.to_string(),
                        call_id: id.clone(),
                        tool_name: name.clone(),
                    });
                    results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: crate::output_redaction::redact_tool_output(&s),
                        is_error: false,
                    });
                    modifiers.push(None);
                    hook_outcomes.push(crate::hooks::HookOutcome::default());
                    continue;
                }
                Ok(ToolApprovalResult::Approved { answer: Some(_) }) => {
                    // v0.9.3 W8 H1-reliability — answer present but tool is
                    // NOT AskUserQuestion. The host included an answer that
                    // is only meaningful for AskUserQuestion; ignore it and
                    // dispatch the tool normally so its real execute() runs
                    // and produces the real output. Logged at WARN because
                    // a well-behaved host should not be sending answers for
                    // non-AskUser tools — if this fires in production, the
                    // host has a bug.
                    tracing::warn!(
                        target: "wcore_agent::orchestration",
                        tool = %name,
                        "ToolApprove.answer received for non-AskUserQuestion tool; ignoring synth path and falling through to dispatch"
                    );
                    // fall through to existing dispatch
                }
                Ok(ToolApprovalResult::Approved { answer: None }) => { /* fall through to existing dispatch */
                }
                Ok(ToolApprovalResult::Denied { reason }) => {
                    let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                        msg_id: msg_id.to_string(),
                        call_id: id.clone(),
                        reason: reason.clone(),
                    });
                    let denied = ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: crate::output_redaction::redact_tool_output(&format!(
                            "Tool denied: {reason}"
                        )),
                        is_error: true,
                    };
                    let contract = tool
                        .map(|tool| tool.effect_contract(input))
                        .unwrap_or_default();
                    let denied = record_tool_not_started(
                        effect_scope,
                        id,
                        ordinal as u64,
                        name,
                        input,
                        input,
                        contract,
                        ToolNotStartedReason::ApprovalDenied {
                            approval_id: id.clone(),
                        },
                        None,
                    )
                    .map_or_else(|error| journal_authority_failure(id, error), |()| denied);
                    results.push(denied);
                    modifiers.push(None);
                    hook_outcomes.push(crate::hooks::HookOutcome::default());
                    continue;
                }
                Err(_) => {
                    // Channel dropped — client disconnected, or the
                    // B-2 TTL reaper collected an abandoned approval.
                    return Err(ExecutionControl::Quit);
                }
            }
        }

        // Emit tool_running
        let _ = writer.emit(&ProtocolEvent::ToolRunning {
            msg_id: msg_id.to_string(),
            call_id: id.clone(),
            tool_name: name.clone(),
        });

        // Execute the tool (reborrow as shared for execute_single, then reclaim mut for merge).
        let result;
        let modifier;
        let post_outcome;
        let was_cancelled;
        {
            let hooks_shared: Option<&HookEngine> = hooks.as_deref();
            (result, modifier, post_outcome, was_cancelled) = execute_single_with_streaming(
                registry,
                call,
                hooks_shared,
                compaction_level,
                toon_enabled,
                None,
                budget,
                approval_bound,
                cancel,
                file_write_notifier,
                effect_scope,
                effect_ordinals
                    .and_then(|ordinals| ordinals.get(ordinal))
                    .copied()
                    .unwrap_or(ordinal as u64),
                recovered_retry_for_call,
            )
            .await;
        }
        if was_cancelled && let ContentBlock::ToolResult { tool_use_id, .. } = &result {
            cancelled_ids.push(tool_use_id.clone());
        }

        // Emit tool_result event
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &result
        {
            let status = if *is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Success
            };
            let _ = writer.emit(&ProtocolEvent::ToolResult {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool_name: name.clone(),
                status,
                output: content.clone(),
                output_type: OutputType::Text,
                metadata: None,
            });
        }

        // Merge skill hooks after a successful execution.
        if !block_is_error(&result) {
            maybe_merge_skill_hooks(registry, call, hooks.as_deref_mut());
        }

        results.push(result);
        modifiers.push(modifier);
        hook_outcomes.push(post_outcome);
    }

    Ok(ToolCallOutcome {
        results,
        modifiers,
        hook_outcomes,
        cancelled_ids,
    })
}

/// If `call` is a Skill tool call that returned successfully, parse and merge
/// its declared hooks into the active HookEngine.
/// If `call` is a Skill tool call that returned successfully, merge skill hooks into the engine.
fn merge_skill_hooks_into(engine: &mut HookEngine, registry: &ToolRegistry, call: &ContentBlock) {
    let ContentBlock::ToolUse { name, input, .. } = call else {
        return;
    };
    if name != "Skill" {
        return;
    }
    let Some(tool) = registry.get(name) else {
        return;
    };
    if let Some(skill_hooks) = tool.skill_hooks_for(input) {
        engine.merge_hooks(skill_hooks);
    }
}

fn maybe_merge_skill_hooks(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&mut HookEngine>,
) {
    if let Some(engine) = hooks {
        merge_skill_hooks_into(engine, registry, call);
    }
}

/// Returns true when a ContentBlock::ToolResult has is_error=true.
fn block_is_error(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolResult { is_error: true, .. })
}

/// When a deferred tool fails AND the input is missing required fields from
/// its full schema, append a hint telling the LLM to call ToolSearch first.
/// If required fields are all present (or the schema has none), the original
/// error is returned unchanged — the failure is a runtime issue, not a
/// missing-schema problem.
fn maybe_append_deferred_hint(
    original_error: &str,
    schema: serde_json::Value,
    input: &serde_json::Value,
) -> String {
    let missing: Vec<&str> = schema["required"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|key| input.get(key).is_none())
                .collect()
        })
        .unwrap_or_default();

    if missing.is_empty() {
        return original_error.to_string();
    }

    format!(
        "{}\n\nThis tool's full schema was not loaded — required field(s) missing: {}. \
         Check the tool's parameter list and retry with all required fields.",
        original_error,
        missing.join(", ")
    )
}

fn truncate_result(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let half = max_chars / 2;
    // Find char boundaries to avoid panicking on multi-byte characters
    let head_end = content
        .char_indices()
        .nth(half)
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let tail_start = content
        .char_indices()
        .rev()
        .nth(half - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let head = &content[..head_end];
    let tail = &content[tail_start..];
    format!(
        "{}\n\n... [truncated {} chars] ...\n\n{}",
        head,
        content.len() - max_chars,
        tail
    )
}

/// Wave RB RELIABILITY MAJOR. Extract a best-effort human-readable
/// message from a `Box<dyn Any + Send>` panic payload. Mirrors the
/// `std::panic` default panic hook: try `&str`, then `String`, otherwise
/// fall back to a generic placeholder. The result is suffixed onto the
/// synthesised `ToolResult.content` and surfaced via the
/// `ToolPanicked` protocol event.
fn extract_panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "<non-string panic payload>".to_string()
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find a char boundary to avoid panicking on multi-byte characters
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

struct Batch<'a> {
    is_concurrent: bool,
    calls: Vec<&'a ContentBlock>,
}

fn durable_tool_call_ordinal(
    effect_ordinals: Option<&[u64]>,
    calls: &[ContentBlock],
    call: &ContentBlock,
) -> u64 {
    let local = calls
        .iter()
        .position(|candidate| std::ptr::eq(candidate, call))
        .expect("partition retains references into the original call slice");
    effect_ordinals
        .and_then(|ordinals| ordinals.get(local))
        .copied()
        .unwrap_or(local as u64)
}

fn partition<'a>(registry: &ToolRegistry, calls: &'a [ContentBlock]) -> Vec<Batch<'a>> {
    let mut batches: Vec<Batch<'a>> = Vec::new();

    for call in calls {
        let ContentBlock::ToolUse { name, input, .. } = call else {
            continue;
        };
        let is_safe = registry
            .get(name)
            .map(|t| t.is_concurrency_safe(input))
            .unwrap_or(false);

        match batches.last_mut() {
            Some(last) if last.is_concurrent && is_safe => {
                last.calls.push(call);
            }
            _ => {
                batches.push(Batch {
                    is_concurrent: is_safe,
                    calls: vec![call],
                });
            }
        }
    }

    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -- truncate_display -----------------------------------------------------

    #[test]
    fn truncate_display_ascii_short_unchanged() {
        assert_eq!(truncate_display("hello", 10), "hello");
    }

    #[test]
    fn truncate_display_ascii_truncated() {
        let result = truncate_display("hello world", 5);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 20);
    }

    #[test]
    fn truncate_display_cjk_does_not_panic() {
        // 200 CJK chars: each is 3 bytes, so byte index 200 falls mid-character
        let cjk: String = "你好世界测试".chars().cycle().take(200).collect();
        let result = truncate_display(&cjk, 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_display_mixed_cjk_ascii_does_not_panic() {
        let mixed = "abc你好def世界ghi测试".repeat(20);
        let result = truncate_display(&mixed, 30);
        assert!(result.ends_with("..."));
    }

    // -- truncate_result ------------------------------------------------------

    #[test]
    fn truncate_result_short_unchanged() {
        let s = "short content";
        assert_eq!(truncate_result(s, 1000), s);
    }

    #[test]
    fn truncate_result_cjk_does_not_panic() {
        let cjk: String = "这是一段较长的中文内容用于测试截断功能".repeat(50);
        let result = truncate_result(&cjk, 100);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn truncate_result_mixed_cjk_ascii_does_not_panic() {
        let mixed = "Hello你好World世界Test测试".repeat(100);
        let result = truncate_result(&mixed, 200);
        assert!(result.contains("truncated"));
    }

    // -- maybe_append_deferred_hint -------------------------------------------

    #[test]
    fn deferred_hint_appended_when_required_field_missing() {
        let schema = json!({
            "type": "object",
            "properties": { "tasks": { "type": "array" } },
            "required": ["tasks"]
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("Missing or invalid 'tasks' array", schema, &input);
        assert!(result.contains("Missing or invalid 'tasks' array"));
        // F-022: hint no longer mentions "ToolSearch" harness language; it
        // lists the missing required field(s) instead.
        assert!(result.contains("tasks"));
        assert!(result.contains("required field"));
    }

    #[test]
    fn deferred_hint_not_appended_when_required_fields_present() {
        let schema = json!({
            "type": "object",
            "properties": { "tasks": { "type": "array" } },
            "required": ["tasks"]
        });
        let input = json!({"tasks": [{"name": "t1", "prompt": "do x"}]});
        let result = maybe_append_deferred_hint("Some runtime error", schema, &input);
        assert_eq!(result, "Some runtime error");
        assert!(!result.contains("ToolSearch"));
    }

    #[test]
    fn deferred_hint_not_appended_when_no_required_field() {
        let schema = json!({
            "type": "object",
            "properties": {}
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("some error", schema, &input);
        assert_eq!(result, "some error");
    }

    #[test]
    fn deferred_hint_not_appended_when_required_is_empty() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        let input = json!({});
        let result = maybe_append_deferred_hint("some error", schema, &input);
        assert_eq!(result, "some error");
    }

    #[test]
    fn deferred_hint_appended_for_partial_missing_fields() {
        let schema = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string" },
                "b": { "type": "string" }
            },
            "required": ["a", "b"]
        });
        let input = json!({"a": "present"});
        let result = maybe_append_deferred_hint("validation failed", schema, &input);
        // F-022: hint lists missing fields rather than "ToolSearch".
        assert!(result.contains("b"));
        assert!(result.contains("required field"));
    }

    // -- execute_single integration tests (deferred tool hint) ----------------

    use wcore_tools::Tool;
    use wcore_tools::registry::ToolRegistry;

    struct MockDeferredTool {
        schema: serde_json::Value,
    }

    #[async_trait::async_trait]
    impl Tool for MockDeferredTool {
        fn name(&self) -> &str {
            "MockDeferred"
        }
        fn description(&self) -> &str {
            "A mock deferred tool for testing"
        }
        fn input_schema(&self) -> serde_json::Value {
            self.schema.clone()
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn is_deferred(&self) -> bool {
            true
        }
        async fn execute(&self, input: serde_json::Value) -> wcore_types::tool::ToolResult {
            if input.get("tasks").is_none() {
                return wcore_types::tool::ToolResult {
                    content: "Missing or invalid 'tasks' array".to_string(),
                    is_error: true,
                };
            }
            wcore_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            wcore_protocol::events::ToolCategory::Exec
        }
    }

    struct MockNonDeferredTool;

    #[async_trait::async_trait]
    impl Tool for MockNonDeferredTool {
        fn name(&self) -> &str {
            "MockNonDeferred"
        }
        fn description(&self) -> &str {
            "A mock non-deferred tool"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({
                "type": "object",
                "properties": { "cmd": { "type": "string" } },
                "required": ["cmd"]
            })
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn execute(&self, input: serde_json::Value) -> wcore_types::tool::ToolResult {
            if input.get("cmd").is_none() {
                return wcore_types::tool::ToolResult {
                    content: "Missing cmd".to_string(),
                    is_error: true,
                };
            }
            if input.get("cmd").and_then(serde_json::Value::as_str) == Some("sleep") {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            wcore_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            wcore_protocol::events::ToolCategory::Exec
        }
        fn execution_class_for(
            &self,
            _input: &serde_json::Value,
        ) -> wcore_tools::ToolExecutionClass {
            wcore_tools::ToolExecutionClass::ProcessSpawning
        }
    }

    fn make_registry_with_deferred() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(MockDeferredTool {
            schema: json!({
                "type": "object",
                "properties": { "tasks": { "type": "array" } },
                "required": ["tasks"]
            }),
        }));
        registry.register(Box::new(MockNonDeferredTool));
        registry
    }

    fn effect_fixture() -> (
        tempfile::TempDir,
        crate::session_journal::SessionJournal,
        TurnEffectScope,
    ) {
        use crate::journal_effects::JournalEffectCoordinator;
        use crate::session_journal::{SessionEvent, SessionJournal};

        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::open(dir.path().join("session.journal"), "session").unwrap();
        journal
            .append(SessionEvent::TurnStarted {
                turn_id: "turn".into(),
                user_message: "test".into(),
            })
            .unwrap();
        let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("turn");
        (dir, journal, scope)
    }

    #[tokio::test]
    async fn durable_dispatch_terminalizes_success_before_return() {
        use crate::session_journal::{StoredToolInput, ToolEffectState};

        let registry = make_registry_with_deferred();
        let (_dir, journal, scope) = effect_fixture();
        let call = ContentBlock::ToolUse {
            id: "provider-call".into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "ok", "secret": "must-not-persist"}),
            extra: None,
        };

        let (result, _, _, _) = execute_single_with_budget(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            false,
            &CancellationToken::new(),
            None,
            Some(&scope),
            7,
        )
        .await;

        assert!(!block_is_error(&result));
        let state = journal.state().unwrap();
        let tool = state.tools.values().next().expect("one durable tool");
        assert_eq!(tool.provider_call_id, "provider-call");
        assert_eq!(tool.ordinal, 7);
        assert!(matches!(tool.effect, ToolEffectState::Succeeded));
        assert!(matches!(
            &tool.requested_input,
            StoredToolInput::Redacted { .. }
        ));
        assert!(
            !serde_json::to_string(&state)
                .unwrap()
                .contains("must-not-persist")
        );
    }

    #[tokio::test]
    async fn opaque_reported_error_is_unknown_not_false_terminal_failure() {
        use crate::session_journal::{ToolEffectState, ToolUnknownReason};

        let registry = make_registry_with_deferred();
        let (_dir, journal, scope) = effect_fixture();
        let call = ContentBlock::ToolUse {
            id: "provider-call".into(),
            name: "MockNonDeferred".into(),
            input: json!({}),
            extra: None,
        };

        let (result, _, _, _) = execute_single_with_budget(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            false,
            &CancellationToken::new(),
            None,
            Some(&scope),
            0,
        )
        .await;

        assert!(block_is_error(&result));
        let state = journal.state().unwrap();
        let tool = state.tools.values().next().expect("one durable tool");
        assert!(matches!(
            &tool.effect,
            ToolEffectState::Unknown {
                reason: ToolUnknownReason::AmbiguousFailure { .. },
                ..
            }
        ));
    }

    struct CrashCutOpaqueTool {
        physical_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl Tool for CrashCutOpaqueTool {
        fn name(&self) -> &str {
            "CrashCutOpaque"
        }

        fn description(&self) -> &str {
            "Opaque physical effect used to prove dispatcher crash boundaries"
        }

        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "additionalProperties": false })
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            false
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            self.physical_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ToolResult {
                content: "physical effect committed".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Exec
        }
    }

    fn crash_cut_registry(physical_calls: &Arc<std::sync::atomic::AtomicUsize>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CrashCutOpaqueTool {
            physical_calls: Arc::clone(physical_calls),
        }));
        registry
    }

    #[tokio::test]
    async fn live_dispatcher_crash_cuts_replay_without_repeating_opaque_effects() {
        use crate::journal_effects::JournalEffectCoordinator;
        use crate::session_journal::{
            SessionEvent, SessionJournal, ToolEffectState, ToolNotStartedReason, ToolUnknownReason,
        };
        use std::cell::Cell;
        use std::sync::atomic::Ordering;

        let cuts = [
            (DispatcherCrashCut::BeforePrepared, None, 0),
            (
                DispatcherCrashCut::AfterPrepared,
                Some(ToolEffectState::Prepared),
                0,
            ),
            (
                DispatcherCrashCut::BeforeRunning,
                Some(ToolEffectState::Prepared),
                0,
            ),
            (
                DispatcherCrashCut::AfterRunning,
                Some(ToolEffectState::Running),
                0,
            ),
            (
                DispatcherCrashCut::AfterPhysicalEffect,
                Some(ToolEffectState::Running),
                1,
            ),
            (
                DispatcherCrashCut::BeforeTerminalAppend,
                Some(ToolEffectState::Running),
                1,
            ),
            (
                DispatcherCrashCut::AfterTerminalAppend,
                Some(ToolEffectState::Succeeded),
                1,
            ),
        ];

        for (cut, expected_effect, expected_physical_calls) in cuts {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.journal");
            let journal = SessionJournal::open(&path, "session").unwrap();
            journal
                .append(SessionEvent::TurnStarted {
                    turn_id: "turn".into(),
                    user_message: "crash-cut proof".into(),
                })
                .unwrap();
            let scope = JournalEffectCoordinator::new(journal.clone()).for_turn("turn");
            let physical_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let registry = crash_cut_registry(&physical_calls);
            let call = ContentBlock::ToolUse {
                id: "provider-call".into(),
                name: "CrashCutOpaque".into(),
                input: json!({}),
                extra: None,
            };

            let crashed = DISPATCHER_CRASH_CUT
                .scope(Cell::new(Some(cut)), async {
                    AssertUnwindSafe(execute_single_with_budget(
                        &registry,
                        &call,
                        None,
                        wcore_compact::CompactionLevel::Off,
                        false,
                        None,
                        false,
                        &CancellationToken::new(),
                        None,
                        Some(&scope),
                        3,
                    ))
                    .catch_unwind()
                    .await
                })
                .await;
            assert!(crashed.is_err(), "{cut:?} did not cut the live dispatcher");
            assert_eq!(
                physical_calls.load(Ordering::SeqCst),
                expected_physical_calls,
                "{cut:?} crossed the physical boundary at the wrong time"
            );

            drop(scope);
            drop(journal);
            drop(registry);

            let replayed = SessionJournal::recovered_state(&path).unwrap();
            match expected_effect.as_ref() {
                None => assert!(replayed.tools.is_empty(), "{cut:?}"),
                Some(expected) => {
                    let tool = replayed.tools.values().next().expect("one durable tool");
                    assert_eq!(&tool.effect, expected, "{cut:?}");
                }
            }

            let reopened = SessionJournal::open(&path, "session").unwrap();
            let replayed_tool_id = reopened.state().unwrap().tools.keys().next().cloned();
            if let (Some(tool_execution_id), Some(effect)) =
                (replayed_tool_id.as_ref(), expected_effect.as_ref())
            {
                match effect {
                    ToolEffectState::Prepared => {
                        reopened
                            .append(SessionEvent::ToolExecutionNotStarted {
                                tool_execution_id: tool_execution_id.clone(),
                                reason: ToolNotStartedReason::Cancelled {
                                    reason: "injected crash before durable start".to_string(),
                                },
                            })
                            .unwrap();
                    }
                    ToolEffectState::Running => {
                        reopened
                            .append(SessionEvent::ToolExecutionUnknown {
                                tool_execution_id: tool_execution_id.clone(),
                                reason: ToolUnknownReason::Interrupted,
                                evidence: json!({
                                    "recovery": "dispatcher_crash_cut_test",
                                    "cut": format!("{cut:?}"),
                                }),
                            })
                            .unwrap();
                    }
                    ToolEffectState::Succeeded => {}
                    other => panic!("unexpected pre-recovery effect at {cut:?}: {other:?}"),
                }
            }

            let restarted_scope = JournalEffectCoordinator::new(reopened.clone()).for_turn("turn");
            let restarted_registry = crash_cut_registry(&physical_calls);
            let (restarted_result, _, _, _) = execute_single_with_budget(
                &restarted_registry,
                &call,
                None,
                wcore_compact::CompactionLevel::Off,
                false,
                None,
                false,
                &CancellationToken::new(),
                None,
                Some(&restarted_scope),
                3,
            )
            .await;

            if expected_effect.is_none() {
                assert!(
                    !block_is_error(&restarted_result),
                    "a crash before prepared must allow the first physical attempt"
                );
                assert_eq!(physical_calls.load(Ordering::SeqCst), 1, "{cut:?}");
                assert!(matches!(
                    reopened
                        .state()
                        .unwrap()
                        .tools
                        .values()
                        .next()
                        .expect("restarted tool")
                        .effect,
                    ToolEffectState::Succeeded
                ));
            } else {
                assert!(
                    block_is_error(&restarted_result),
                    "{cut:?} must refuse an automatic duplicate attempt"
                );
                assert_eq!(
                    physical_calls.load(Ordering::SeqCst),
                    expected_physical_calls,
                    "{cut:?} repeated an opaque physical effect after restart"
                );
            }
        }
    }

    #[test]
    fn filtered_dispatch_preserves_original_provider_ordinal() {
        let calls = vec![ContentBlock::ToolUse {
            id: "provider-call".into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "ok"}),
            extra: None,
        }];
        assert_eq!(durable_tool_call_ordinal(Some(&[9]), &calls, &calls[0]), 9);
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_error_missing_required_appends_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "MockDeferred".into(),
            input: json!({}),
            extra: None,
        };
        let (result, _, _, _) = execute_single(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            false,
            &CancellationToken::new(),
            None,
        )
        .await;
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &result
        {
            assert!(is_error);
            assert!(content.contains("Missing or invalid 'tasks' array"));
            // F-022: hint no longer leaks "ToolSearch" harness language.
            assert!(content.contains("required field"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_error_with_required_present_no_hint() {
        let registry = make_registry_with_deferred();
        // tasks is present but wrong type — tool still fails, but required field exists
        let call = ContentBlock::ToolUse {
            id: "call_2".into(),
            name: "MockDeferred".into(),
            input: json!({"tasks": "not_an_array"}),
            extra: None,
        };
        let (result, _, _, _) = execute_single(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            false,
            &CancellationToken::new(),
            None,
        )
        .await;
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &result
        {
            // Tool succeeds because input.get("tasks") is Some
            assert!(!is_error);
            assert!(!content.contains("ToolSearch"));
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_deferred_tool_success_no_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_3".into(),
            name: "MockDeferred".into(),
            input: json!({"tasks": [{"name": "t1", "prompt": "do x"}]}),
            extra: None,
        };
        let (result, _, _, _) = execute_single(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            false,
            &CancellationToken::new(),
            None,
        )
        .await;
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &result
        {
            assert!(!is_error);
            assert_eq!(content, "ok");
        } else {
            panic!("expected ToolResult");
        }
    }

    #[tokio::test]
    async fn execute_single_non_deferred_tool_error_no_hint() {
        let registry = make_registry_with_deferred();
        let call = ContentBlock::ToolUse {
            id: "call_4".into(),
            name: "MockNonDeferred".into(),
            input: json!({}),
            extra: None,
        };
        let (result, _, _, _) = execute_single(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            false,
            &CancellationToken::new(),
            None,
        )
        .await;
        if let ContentBlock::ToolResult {
            content, is_error, ..
        } = &result
        {
            assert!(is_error);
            assert!(content.contains("Missing cmd"));
            assert!(!content.contains("ToolSearch"));
        } else {
            panic!("expected ToolResult");
        }
    }

    // ---- W8b.2.A-5: ToolBudgetTracker dispatcher wiring ----------------

    #[tokio::test]
    async fn budget_tracker_records_each_tool_call_through_dispatcher() {
        use crate::tool_budget::ToolBudgetTracker;
        use std::sync::Arc;
        use std::sync::Mutex;

        let registry = make_registry_with_deferred();
        let calls = vec![
            ContentBlock::ToolUse {
                id: "c1".into(),
                name: "MockNonDeferred".into(),
                input: json!({"cmd": "a"}),
                extra: None,
            },
            ContentBlock::ToolUse {
                id: "c2".into(),
                name: "MockNonDeferred".into(),
                input: json!({"cmd": "b"}),
                extra: None,
            },
            ContentBlock::ToolUse {
                id: "c3".into(),
                name: "MockDeferred".into(),
                input: json!({"tasks": []}),
                extra: None,
            },
        ];
        let tracker = ToolBudgetTracker::new();
        let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));

        let outcome = execute_tool_calls_with_budget(
            &registry,
            &calls,
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            Some(&tracker),
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("dispatch should not return ExecutionControl");

        // All three calls produced a ToolResult (no quit / no deny).
        assert_eq!(outcome.results.len(), 3);

        let usage_nd = tracker.usage_for("MockNonDeferred");
        assert_eq!(
            usage_nd.calls, 2,
            "MockNonDeferred should be recorded twice"
        );
        let usage_d = tracker.usage_for("MockDeferred");
        assert_eq!(usage_d.calls, 1, "MockDeferred should be recorded once");

        // Each recorded call should have a non-negative runtime (zero
        // is acceptable for sub-microsecond stubs but the bucket must
        // exist with the correct call count).
        let all = tracker.all_usage();
        assert!(all.contains_key("MockNonDeferred"));
        assert!(all.contains_key("MockDeferred"));
    }

    #[tokio::test]
    async fn budget_tracker_unobserved_when_none_passed() {
        use crate::tool_budget::ToolBudgetTracker;
        use std::sync::Arc;
        use std::sync::Mutex;

        let registry = make_registry_with_deferred();
        let calls = vec![ContentBlock::ToolUse {
            id: "c1".into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "a"}),
            extra: None,
        }];
        let tracker = ToolBudgetTracker::new();
        let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));

        // Call the legacy path (no budget) — tracker must stay empty.
        let _ = execute_tool_calls_with_streaming(
            &registry,
            &calls,
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("dispatch should not return ExecutionControl");

        assert_eq!(
            tracker.usage_for("MockNonDeferred").calls,
            0,
            "legacy path must NOT record into a tracker the caller didn't pass"
        );
    }

    #[tokio::test]
    async fn dispatcher_refuses_process_tool_before_execution_when_cap_is_zero() {
        let registry = make_registry_with_deferred();
        let budget = crate::budget::ExecutionBudget {
            max_processes: Some(0),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget);
        let call = ContentBlock::ToolUse {
            id: "process-denied".into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "a"}),
            extra: None,
        };

        let (result, _, _, _) = execute_single_with_budget(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            Some(&tracker),
            false,
            &CancellationToken::new(),
            None,
            None,
            0,
        )
        .await;

        let ContentBlock::ToolResult {
            content, is_error, ..
        } = result
        else {
            panic!("expected paired tool result")
        };
        assert!(is_error);
        assert!(content.contains("max_concurrent_process_tools"));
        assert_eq!(tracker.usage_for("MockNonDeferred").calls, 0);
    }

    #[tokio::test]
    async fn dispatcher_preempts_at_remaining_tool_runtime_budget() {
        let registry = make_registry_with_deferred();
        let budget = crate::budget::ExecutionBudget {
            max_tool_runtime: Some(Duration::from_millis(20)),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget.clone());
        let call = ContentBlock::ToolUse {
            id: "runtime-denied".into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "sleep"}),
            extra: None,
        };

        let (result, _, _, was_cancelled) = execute_single_with_budget(
            &registry,
            &call,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            Some(&tracker),
            false,
            &CancellationToken::new(),
            None,
            None,
            0,
        )
        .await;

        let ContentBlock::ToolResult { is_error, .. } = result else {
            panic!("expected paired tool result")
        };
        assert!(is_error);
        assert!(was_cancelled);
        assert!(budget.is_exceeded());
    }

    #[tokio::test]
    async fn concurrent_dispatch_cannot_multiply_remaining_tool_runtime() {
        let registry = make_registry_with_deferred();
        let budget = crate::budget::ExecutionBudget {
            max_tool_runtime: Some(Duration::from_millis(30)),
            ..Default::default()
        }
        .start_root();
        let tracker = ToolBudgetTracker::with_execution_budget(budget);
        let calls = vec![
            ContentBlock::ToolUse {
                id: "runtime-first".into(),
                name: "MockNonDeferred".into(),
                input: json!({"cmd": "sleep"}),
                extra: None,
            },
            ContentBlock::ToolUse {
                id: "runtime-second".into(),
                name: "MockNonDeferred".into(),
                input: json!({"cmd": "sleep"}),
                extra: None,
            },
        ];
        let confirmer = Arc::new(std::sync::Mutex::new(ToolConfirmer::new(true, vec![])));

        let outcome = execute_tool_calls_with_budget(
            &registry,
            &calls,
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            Some(&tracker),
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("dispatch should return paired results");

        assert_eq!(outcome.results.len(), 2);
        let contents: Vec<_> = outcome
            .results
            .iter()
            .map(|result| match result {
                ContentBlock::ToolResult { content, .. } => content.as_str(),
                _ => panic!("expected paired tool result"),
            })
            .collect();
        assert_eq!(
            contents
                .iter()
                .filter(|content| content.contains("max_tool_runtime"))
                .count(),
            1,
            "one sibling must be rejected before execution"
        );
        assert_eq!(
            contents
                .iter()
                .filter(|content| content.contains("timed out"))
                .count(),
            1,
            "the admitted sibling retains the remaining per-call deadline"
        );
        assert_eq!(
            tracker.usage_for("MockNonDeferred").calls,
            1,
            "only the admitted call may enter the tool"
        );
    }

    // ---- W3a host-trust synth chokepoint tests --------------------------

    struct NullEmitter;
    impl wcore_protocol::writer::ProtocolEmitter for NullEmitter {
        fn emit(&self, _event: &wcore_protocol::events::ProtocolEvent) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// W3a.2 — non-AskUser tool with `Approved { answer: Some(_) }` must
    /// fall through to normal dispatch (tool actually executes) and the
    /// injected answer must NOT become the tool result.
    #[tokio::test]
    async fn non_askuser_synth_falls_through_to_dispatch_v094() {
        use wcore_protocol::{ToolApprovalManager, commands::ApprovalScope};

        let registry = make_registry_with_deferred();
        let mgr = Arc::new(ToolApprovalManager::new());
        let writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter> = Arc::new(NullEmitter);

        // MockNonDeferred requires {"cmd": ...} to succeed; we provide it.
        let call_id = "call-nonaskuser-1";
        let tool_call = ContentBlock::ToolUse {
            id: call_id.into(),
            name: "MockNonDeferred".into(),
            input: json!({"cmd": "hello"}),
            extra: None,
        };

        // Spawn a task that resolves the approval once the main function
        // parks on `rx.await`. The spawned task yields once (tokio::task::yield_now)
        // to let the function reach the await point, then calls approve() with an
        // injected answer — simulating a host bug sending an answer for a non-AskUser
        // tool. The function must fall through to dispatch, ignoring the answer.
        let mgr_clone = Arc::clone(&mgr);
        let call_id_clone = call_id.to_string();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            mgr_clone.approve(
                &call_id_clone,
                ApprovalScope::Once,
                Some("injected-answer-must-not-appear".into()),
            );
        });

        let outcome = execute_tool_calls_with_approval(
            &registry,
            &[tool_call],
            &mgr,
            &writer,
            "msg-1",
            &[],  // allow_list empty
            None, // no hook engine
            wcore_compact::CompactionLevel::Off,
            false,
            &tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("should not return ExecutionControl");

        assert_eq!(outcome.results.len(), 1, "one result expected");
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = &outcome.results[0]
        else {
            panic!("expected ToolResult");
        };
        // Tool executed normally: MockNonDeferred with cmd="hello" returns "ok".
        assert!(!is_error, "tool should succeed (not error)");
        assert_eq!(
            content, "ok",
            "result must be from tool execute(), not injected answer"
        );
        assert!(
            !content.contains("injected"),
            "injected answer must not appear in tool result; got: {content}"
        );
    }

    /// W3a.2 positive companion — AskUserQuestion with `answer: Some(_)`
    /// synthesizes the tool result directly (tool is never dispatched).
    #[tokio::test]
    async fn askuser_answer_synthesizes_result_v094() {
        use wcore_protocol::{ToolApprovalManager, commands::ApprovalScope};

        // AskUserQuestion does NOT need to be in the registry for the synth
        // path: the guard fires before dispatch, short-circuiting via continue.
        let registry = make_registry_with_deferred();
        let mgr = Arc::new(ToolApprovalManager::new());
        let writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter> = Arc::new(NullEmitter);

        let call_id = "call-askuser-1";
        let tool_call = ContentBlock::ToolUse {
            id: call_id.into(),
            name: "AskUserQuestion".into(),
            input: json!({"question": "Continue?", "options": ["yes", "no"]}),
            extra: None,
        };

        // Spawn the approval after the function parks on rx.await.
        let mgr_clone = Arc::clone(&mgr);
        let call_id_clone = call_id.to_string();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            mgr_clone.approve(&call_id_clone, ApprovalScope::Once, Some("yes".into()));
        });

        let outcome = execute_tool_calls_with_approval(
            &registry,
            &[tool_call],
            &mgr,
            &writer,
            "msg-2",
            &[],
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            &tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
        .expect("should not return ExecutionControl");

        assert_eq!(outcome.results.len(), 1, "one result expected");
        let ContentBlock::ToolResult {
            content, is_error, ..
        } = &outcome.results[0]
        else {
            panic!("expected ToolResult");
        };
        // Synth arm: the answer string IS the result content.
        assert!(!is_error, "synthesized result must not be an error");
        assert_eq!(
            content, "yes",
            "synthesized result must equal the approved answer"
        );
    }

    // ---- #133 call_id stability across protocol tool frames ---------------

    /// Capturing emitter — records every protocol event for post-hoc
    /// assertions on the tool_request/tool_running/tool_result call_ids.
    struct CapturingEmitter(Mutex<Vec<wcore_protocol::events::ProtocolEvent>>);
    impl wcore_protocol::writer::ProtocolEmitter for CapturingEmitter {
        fn emit(&self, event: &wcore_protocol::events::ProtocolEvent) -> std::io::Result<()> {
            if let Ok(mut events) = self.0.lock() {
                events.push(event.clone());
            }
            Ok(())
        }
    }

    /// #133 — for a PARALLEL tool batch driven through the approval gate,
    /// every call must emit tool_request, tool_running, and tool_result with
    /// the SAME call_id as the originating ToolUse block. Hosts (the Wayland
    /// desktop) merge tool cards strictly by call_id: any divergence leaves a
    /// card in "Executing" forever (desktop #486).
    #[tokio::test]
    async fn parallel_batch_tool_result_call_id_matches_tool_request() {
        use wcore_protocol::events::ProtocolEvent;
        use wcore_protocol::{ToolApprovalManager, commands::ApprovalScope};

        let registry = make_registry_with_deferred();
        let mgr = Arc::new(ToolApprovalManager::new());
        let emitter = Arc::new(CapturingEmitter(Mutex::new(Vec::new())));
        let writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter> =
            Arc::clone(&emitter) as Arc<dyn wcore_protocol::writer::ProtocolEmitter>;

        let call_ids = ["call_par_a", "call_par_b"];
        let calls: Vec<ContentBlock> = call_ids
            .iter()
            .map(|id| ContentBlock::ToolUse {
                id: (*id).into(),
                name: "MockNonDeferred".into(),
                input: json!({"cmd": *id}),
                extra: None,
            })
            .collect();

        // The gate parks on each call's approval sequentially; keep nudging
        // both ids until each pending entry appears and resolves (approve()
        // is a no-op for a not-yet-registered or already-resolved id).
        let mgr_clone = Arc::clone(&mgr);
        tokio::spawn(async move {
            for _ in 0..10_000 {
                tokio::task::yield_now().await;
                for id in call_ids {
                    mgr_clone.approve(id, ApprovalScope::Once, None);
                }
            }
        });

        // Timeout wrapper: if the nudger exhausts before both approvals land,
        // the gate parks on `rx` forever — fail loud instead of hanging.
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            execute_tool_calls_with_approval(
                &registry,
                &calls,
                &mgr,
                &writer,
                "msg-par",
                &[],
                None,
                wcore_compact::CompactionLevel::Off,
                false,
                &tokio_util::sync::CancellationToken::new(),
                None,
            ),
        )
        .await
        .expect("approval round-trip timed out — approve-nudger exhausted")
        .expect("should not return ExecutionControl");
        assert_eq!(outcome.results.len(), 2, "both tools must produce results");

        let events = emitter.0.lock().expect("emitter mutex").clone();
        for expected in call_ids {
            let requested = events.iter().any(
                |e| matches!(e, ProtocolEvent::ToolRequest { call_id, .. } if call_id == expected),
            );
            let running = events.iter().any(
                |e| matches!(e, ProtocolEvent::ToolRunning { call_id, .. } if call_id == expected),
            );
            let resulted = events.iter().any(
                |e| matches!(e, ProtocolEvent::ToolResult { call_id, .. } if call_id == expected),
            );
            assert!(requested, "tool_request missing for {expected}");
            assert!(running, "tool_running missing for {expected}");
            assert!(resulted, "tool_result missing for {expected}");
        }
        // No frame may carry a call_id outside the originating ToolUse ids —
        // an empty or fabricated id would strand a card host-side.
        for e in &events {
            if let ProtocolEvent::ToolRequest { call_id, .. }
            | ProtocolEvent::ToolRunning { call_id, .. }
            | ProtocolEvent::ToolResult { call_id, .. } = e
            {
                assert!(
                    call_ids.contains(&call_id.as_str()),
                    "unexpected call_id on protocol frame: {call_id:?}"
                );
            }
        }
        // The conversation-level ToolResult blocks echo the same ids.
        let result_ids: Vec<&str> = outcome
            .results
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(result_ids, call_ids, "ToolResult ids must match in order");
    }

    // ---- rank 58: dispatch timeout-cancel surfaces a cancelled id ----------

    /// A tool whose `execute` never resolves. Combined with `start_paused`
    /// tokio time, the dispatch wrapper's per-category timeout (`Info` = 30s)
    /// elapses in virtual time, firing the cancel path under test.
    struct HangingTool;

    #[async_trait::async_trait]
    impl Tool for HangingTool {
        fn name(&self) -> &str {
            "Hanging"
        }
        fn description(&self) -> &str {
            "A tool that never returns (for the timeout-cancel test)"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object", "properties": {} })
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            false
        }
        async fn execute(&self, _input: serde_json::Value) -> wcore_types::tool::ToolResult {
            // Park forever; the dispatcher's timeout is the only way out.
            std::future::pending::<()>().await;
            unreachable!("HangingTool::execute never resolves")
        }
        // `Info` (30s) is the shortest category — auto-advanced instantly
        // under start_paused.
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            wcore_protocol::events::ToolCategory::Info
        }
    }

    #[tokio::test(start_paused = true)]
    async fn dispatch_timeout_records_cancelled_id() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(HangingTool));
        let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));
        let call = ContentBlock::ToolUse {
            id: "call-hang-1".into(),
            name: "Hanging".into(),
            input: json!({}),
            extra: None,
        };

        let outcome = execute_tool_calls_with_budget(
            &registry,
            std::slice::from_ref(&call),
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
            None,
            None,
            &CancellationToken::new(),
            None,
        )
        .await
        .expect("dispatch should not abort the batch");

        // The timed-out tool's id is surfaced for the engine to flag on the
        // ToolCallTrace; its result is a synthesized error, not a real run.
        assert_eq!(
            outcome.cancelled_ids,
            vec!["call-hang-1".to_string()],
            "the timed-out tool_use id must be reported as cancelled"
        );
        let ContentBlock::ToolResult { is_error, .. } = &outcome.results[0] else {
            panic!("expected a ToolResult");
        };
        assert!(is_error, "a cancelled dispatch yields an error result");
    }
}
