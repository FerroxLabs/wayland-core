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

use crate::confirm::{ConfirmResult, ToolConfirmer};
use crate::engine::is_hook_lifecycle_line;
use crate::hooks::HookEngine;
use wcore_protocol::events::{OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_types::message::ContentBlock;
use wcore_types::skill_types::ContextModifier;
use wcore_types::tool::ToolResult;

use wcore_tools::registry::ToolRegistry;

use crate::tool_budget::ToolBudgetTracker;

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

    // Partition into allowed + denied. Preserve original order via
    // index-tagged accumulators so the LLM sees results in call order.
    let mut allowed: Vec<(usize, ContentBlock)> = Vec::with_capacity(tool_calls.len());
    let mut denied: Vec<(usize, ContentBlock)> = Vec::new();
    for (idx, call) in tool_calls.iter().enumerate() {
        match call {
            ContentBlock::ToolUse { id, name, .. } => {
                // Top-level dispatch uses the gate's default actor;
                // sub-agent attribution is a v0.7 follow-up that needs
                // source_agent threading through orchestration.
                match gate.check_tool(name, None) {
                    Ok(()) => allowed.push((idx, call.clone())),
                    Err(deny) => denied.push((
                        idx,
                        ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: format!("Denied by policy: {deny}"),
                            is_error: true,
                        },
                    )),
                }
            }
            // Non-ToolUse blocks (defensive — orchestration shouldn't
            // get these in tool_calls, but if it does, pass through
            // untouched).
            _ => allowed.push((idx, call.clone())),
        }
    }

    let allowed_calls: Vec<ContentBlock> = allowed.iter().map(|(_, c)| c.clone()).collect();
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

    // Re-merge into original order. `allowed[i]` corresponds to
    // `inner_outcome.results[i]`; `denied[j].0` is its original index.
    let total = tool_calls.len();
    let mut results: Vec<Option<ContentBlock>> = (0..total).map(|_| None).collect();
    let mut modifiers: Vec<Option<Option<ContextModifier>>> = (0..total).map(|_| None).collect();
    for (allowed_pos, (orig_idx, _)) in allowed.iter().enumerate() {
        results[*orig_idx] = Some(inner_outcome.results[allowed_pos].clone());
        modifiers[*orig_idx] = Some(inner_outcome.modifiers[allowed_pos].clone());
    }
    for (orig_idx, denied_block) in denied {
        results[orig_idx] = Some(denied_block);
        modifiers[orig_idx] = Some(None);
    }
    // SAFETY: every index 0..total is set exactly once — either via the
    // `allowed` loop (which iterates all allowed positions and maps them
    // back to `orig_idx`) or via the `denied` loop (which covers the
    // remaining indices). The two loops partition 0..total, so every
    // `Option` slot is `Some` by the time we reach this point.
    Ok(ToolCallOutcome {
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
    })
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
    mut hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    streaming: Option<StreamingContext>,
    budget: Option<&ToolBudgetTracker>,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
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
            let mut approved = Vec::new();
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => {
                        results.push(denied);
                        modifiers.push(None);
                    }
                    None => approved.push(call),
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
                .map(|call| {
                    execute_single_with_budget(
                        registry,
                        call,
                        hooks_shared,
                        compaction_level,
                        toon_enabled,
                        budget,
                        cancel,
                        file_write_notifier,
                    )
                })
                .collect();
            let batch_results = futures::future::join_all(futures).await;
            for (block, modifier, post_outcome, was_cancelled) in batch_results {
                if was_cancelled && let ContentBlock::ToolResult { tool_use_id, .. } = &block {
                    cancelled_ids.push(tool_use_id.clone());
                }
                results.push(block);
                modifiers.push(modifier);
                hook_outcomes.push(post_outcome);
            }
        } else {
            for call in &batch.calls {
                match confirm_call(confirmer, call)? {
                    Some(denied) => {
                        results.push(denied);
                        modifiers.push(None);
                    }
                    None => {
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
                                    cancel,
                                    file_write_notifier,
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

/// Confirm a single tool call. Returns Ok(Some(result)) if denied, Ok(None) if approved, Err if quit.
fn confirm_call(
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    call: &ContentBlock,
) -> Result<Option<ContentBlock>, ExecutionControl> {
    let ContentBlock::ToolUse {
        id, name, input, ..
    } = call
    else {
        return Ok(None);
    };

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
    let result = confirmer
        .lock()
        .unwrap()
        .check(name, &truncate_display(&input_display, 200));

    match result {
        ConfirmResult::Approved => Ok(None),
        ConfirmResult::Denied => Ok(Some(ContentBlock::ToolResult {
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
}

impl wcore_tools::ToolOutputSink for ProtocolToolSink {
    fn emit_chunk(&self, chunk: &str) {
        self.output
            .emit_tool_chunk(&self.msg_id, &self.call_id, &self.tool_name, chunk);
    }
}

async fn execute_single(
    registry: &ToolRegistry,
    call: &ContentBlock,
    hooks: Option<&HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
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
        cancel,
        file_write_notifier,
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
    execute_single_with_streaming(
        registry,
        call,
        hooks,
        compaction_level,
        toon_enabled,
        None,
        budget,
        cancel,
        file_write_notifier,
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
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
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

    // Run pre-tool-use hooks
    let mut effective_input = input.clone();
    if let Some(hook_engine) = hooks {
        match hook_engine.run_pre_tool_use(name, input).await {
            Ok(mut outcome) => {
                if let Some(reason) = outcome.block.take() {
                    return (
                        ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            content: format!("Blocked by hook: {reason}"),
                            is_error: true,
                        },
                        None,
                        crate::hooks::HookOutcome::default(),
                        false,
                    );
                }
                if let Some(v) = outcome.modified_input {
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
                    tracing::debug!(target: "wcore_agent::hooks", "{line}");
                }
                for line in outcome.log_lines.drain(..) {
                    if is_hook_lifecycle_line(&line) {
                        tracing::debug!(target: "wcore_agent::hooks", "{line}");
                    } else {
                        eprintln!("{line}");
                    }
                }
            }
            Err(e) => {
                return (
                    ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Blocked by hook: {e}"),
                        is_error: true,
                    },
                    None,
                    crate::hooks::HookOutcome::default(),
                    false,
                );
            }
        }
    }

    // Set true only when the dispatch timeout-cancel path below wins, so the
    // engine can flag the synthesized result's trace as cancelled.
    let mut was_cancelled = false;
    let (result, modifier) = match registry.get(name) {
        Some(tool) => {
            let max_size = tool.max_result_size();
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
                    crate::hooks::HookOutcome::default(),
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
            // W8b.2.A-5: per-tool budget tracking. When the caller
            // supplied a tracker, start a RAII handle BEFORE dispatch.
            // The handle commits elapsed runtime on drop (cancel-safe
            // — partial runtime is recorded even on abort). Skipped
            // when budget is None so the legacy callers stay
            // byte-identical.
            let _budget_guard = budget.map(|t| t.start(name));
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
                if let Some(ctx) = streaming.as_ref() {
                    if tool.supports_streaming() && ctx.output.streaming_tools_advertised() {
                        let sink = ProtocolToolSink {
                            output: std::sync::Arc::clone(&ctx.output),
                            msg_id: ctx.msg_id.clone(),
                            call_id: id.clone(),
                            tool_name: name.clone(),
                        };
                        AssertUnwindSafe(tool.execute_streaming_with_ctx(
                            effective_input.clone(),
                            &tool_ctx,
                            &sink,
                        ))
                        .catch_unwind()
                        .await
                    } else {
                        AssertUnwindSafe(tool.execute_with_ctx(effective_input.clone(), &tool_ctx))
                            .catch_unwind()
                            .await
                    }
                } else {
                    AssertUnwindSafe(tool.execute_with_ctx(effective_input.clone(), &tool_ctx))
                        .catch_unwind()
                        .await
                }
            };
            let timeout = tool_dispatch_timeout(category);
            let timed: Result<std::thread::Result<ToolResult>, tokio::time::error::Elapsed> =
                tokio::time::timeout(timeout, dispatch_fut).await;
            let r = match timed {
                Err(_elapsed) => {
                    // Dispatch exceeded its category deadline. Fire the
                    // call's cancel token so a cooperative tool can
                    // abort its own work, then synthesise an error
                    // result so the LLM still sees a paired tool_result.
                    call_cancel.cancel();
                    // Flag the trace: this result was synthesized by the
                    // cancel path, not produced by a completed tool run.
                    was_cancelled = true;
                    let secs = timeout.as_secs();
                    eprintln!(
                        "[tool-timeout] tool={} call_id={} category={:?} elapsed>{}s",
                        name, id, category, secs
                    );
                    if let Some(ctx) = streaming.as_ref() {
                        ctx.output.emit_tool_panicked(
                            &ctx.msg_id,
                            id,
                            name,
                            &format!("timed out after {secs}s"),
                        );
                    }
                    ToolResult {
                        content: format!(
                            "Tool '{name}' timed out after {secs}s and was cancelled. \
                             The operation may be hung; consider a narrower request."
                        ),
                        is_error: true,
                    }
                }
                Ok(Ok(result)) => result,
                Ok(Err(payload)) => {
                    let panic_message = extract_panic_message(&payload);
                    eprintln!(
                        "[tool-panic] tool={} call_id={} panic={}",
                        name, id, panic_message
                    );
                    if let Some(ctx) = streaming.as_ref() {
                        ctx.output
                            .emit_tool_panicked(&ctx.msg_id, id, name, &panic_message);
                    }
                    ToolResult {
                        content: format!(
                            "Tool panicked; session continuing. Panic: {}",
                            panic_message
                        ),
                        is_error: true,
                    }
                }
            };
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
            let error_content = if r.is_error && tool.is_deferred() {
                maybe_append_deferred_hint(&r.content, tool.input_schema(), &effective_input)
            } else {
                r.content.clone()
            };
            let content = truncate_result(&error_content, max_size);
            let content = wcore_compact::compact_output(&content, compaction_level);
            let content = if toon_enabled {
                wcore_compact::compact_output_toon(&content)
            } else {
                content
            };
            (
                ToolResult {
                    content,
                    is_error: r.is_error,
                },
                modifier,
            )
        }
        None => (
            ToolResult {
                content: format!("Unknown tool: {}", name),
                is_error: true,
            },
            None,
        ),
    };

    // Run post-tool-use hooks
    let mut post_outcome = crate::hooks::HookOutcome::default();
    if let Some(hook_engine) = hooks {
        let mut outcome = hook_engine
            .run_post_tool_use(name, id, &effective_input, &result.content, result.is_error)
            .await;
        // v0.9.1.2 F10: hook lifecycle telemetry goes to `tracing::debug!`
        // ONLY — never eprintln! (which leaks into the TUI alt-screen and
        // overlaps the composer/transcript). `hook_trace` is the
        // architectural new home for plugin-hook fire lines + rust-hook
        // "action ignored" diagnostics. `log_lines` is the only place
        // shell-hook stdout lands; we filter it through
        // `is_hook_lifecycle_line` as belt-and-suspenders in case any
        // future code path pushes a lifecycle line there.
        for msg in outcome.hook_trace.drain(..) {
            tracing::debug!(target: "wcore_agent::hooks", "{msg}");
        }
        for msg in outcome.log_lines.drain(..) {
            if is_hook_lifecycle_line(&msg) {
                tracing::debug!(target: "wcore_agent::hooks", "{msg}");
            } else {
                eprintln!("{}", msg);
            }
        }
        // injected_messages and switch_model bubble up via
        // ToolCallOutcome.hook_outcomes; the agent-level engine applies
        // them through apply_turn_end_outcome.
        post_outcome = outcome;
    }

    (
        ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: result.content,
            is_error: result.is_error,
        },
        modifier,
        post_outcome,
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
    auto_approve: bool,
    allow_list: &[String],
    mut hooks: Option<&mut HookEngine>,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    cancel: &CancellationToken,
    file_write_notifier: Option<
        &std::sync::Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    >,
) -> Result<ToolCallOutcome, ExecutionControl> {
    let mut results = Vec::new();
    let mut modifiers = Vec::new();
    let mut hook_outcomes = Vec::new();
    // `tool_use` ids whose result came from the dispatch timeout-cancel path.
    let mut cancelled_ids = Vec::new();

    for call in tool_calls {
        let ContentBlock::ToolUse {
            id, name, input, ..
        } = call
        else {
            continue;
        };

        let tool = registry.get(name);
        let category = tool.map(|t| t.category()).unwrap_or(ToolCategory::Exec);
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
        let needs_approval = name == "AskUserQuestion"
            || (!auto_approve
                && !allow_list.contains(&name.to_string())
                && !approval_manager.is_auto_approved_cmd(&category.to_string(), command)
                && !approval_manager.is_tool_name_auto_approved(name));

        if needs_approval {
            // Emit tool_request and wait for approval
            let _ = writer.emit(&ProtocolEvent::ToolRequest {
                msg_id: msg_id.to_string(),
                call_id: id.clone(),
                tool: ToolInfo {
                    name: name.clone(),
                    category,
                    args: input.clone(),
                    description,
                },
            });

            let rx = approval_manager.request_approval(id, &category, name);
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
                    return Err(ExecutionControl::Quit);
                }
                res = rx => res,
            };
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
                        content: s,
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
                    results.push(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: format!("Tool denied: {reason}"),
                        is_error: true,
                    });
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
            (result, modifier, post_outcome, was_cancelled) = execute_single(
                registry,
                call,
                hooks_shared,
                compaction_level,
                toon_enabled,
                cancel,
                file_write_notifier,
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
            wcore_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            wcore_protocol::events::ToolCategory::Exec
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
            false, // auto_approve: false → gate fires, approval is needed
            &[],   // allow_list empty
            None,  // no hook engine
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
            false,
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
