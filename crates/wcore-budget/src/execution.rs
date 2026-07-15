//! W8a A.2 — `ExecutionBudget` + `ExecutionBudgetView`.
//!
//! `ExecutionBudget` is the config struct (each cap optional). The runtime
//! companion is `ExecutionBudgetView`: cheap-to-clone (`Arc<RwLock<...>>`),
//! tree-shaped (parent + children), with counters for wall-time / tool
//! runtime / processes / agent depth / tokens / cost.
//!
//! Designed to be threaded through `ToolContext.budget` in W8a A.3 so every
//! tool can record usage and check `is_exceeded()` before launching long
//! work. Sub-budgets propagate counters upward by default so the root view
//! sees the full session rollup. Overriding stricter caps on a child does
//! NOT relax the parent.
//!
//! Moved verbatim from `wcore-agent/src/budget.rs` in M5.3 (`wcore-agent`
//! re-exports these types so all pre-existing call sites compile
//! unchanged).

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::config::BudgetConfig;

/// Config struct: every cap optional. Default = no caps.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExecutionBudget {
    pub max_wall_time: Option<Duration>,
    pub max_tool_runtime: Option<Duration>,
    pub max_processes: Option<usize>,
    pub max_agent_depth: Option<usize>,
    pub max_tokens_in: Option<u64>,
    pub max_tokens_out: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

impl ExecutionBudget {
    /// Start a fresh root view, capturing `Instant::now()` as the start.
    pub fn start_root(self) -> ExecutionBudgetView {
        ExecutionBudgetView {
            inner: Arc::new(RwLock::new(BudgetState {
                budget: self,
                started_at: Instant::now(),
                tool_runtime: Duration::ZERO,
                tool_runtime_reserved: Duration::ZERO,
                processes_active: 0,
                agent_depth: 0,
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
            })),
            ancestors: Arc::new(Vec::new()),
        }
    }
}

/// W8a A.5: build `ExecutionBudget` (Durations) from the TOML-shaped
/// `BudgetConfig` (seconds). Now lives in `wcore-budget` since M5.3
/// co-locates both types; pre-M5.3 this impl was in `wcore-agent`.
impl From<&BudgetConfig> for ExecutionBudget {
    fn from(c: &BudgetConfig) -> Self {
        Self {
            max_wall_time: c.max_wall_time_secs.map(Duration::from_secs),
            max_tool_runtime: c.max_tool_runtime_secs.map(Duration::from_secs),
            max_processes: c.max_processes,
            max_agent_depth: c.max_agent_depth,
            max_tokens_in: c.max_tokens_in,
            max_tokens_out: c.max_tokens_out,
            max_cost_usd: c.max_cost_usd,
        }
    }
}

impl From<BudgetConfig> for ExecutionBudget {
    fn from(c: BudgetConfig) -> Self {
        Self::from(&c)
    }
}

#[derive(Debug)]
struct BudgetState {
    budget: ExecutionBudget,
    started_at: Instant,
    tool_runtime: Duration,
    tool_runtime_reserved: Duration,
    processes_active: usize,
    agent_depth: usize,
    tokens_in: u64,
    tokens_out: u64,
    cost_usd: f64,
}

/// Runtime view onto a budget. Cheap to clone; tree-shaped — counters
/// recorded on a child also roll up to all ancestors.
#[derive(Clone)]
pub struct ExecutionBudgetView {
    inner: Arc<RwLock<BudgetState>>,
    /// Root-to-parent chain. Keeping the full chain makes arbitrary nesting
    /// observable and ensures every descendant charge reaches every ancestor.
    ancestors: Arc<Vec<Arc<RwLock<BudgetState>>>>,
}

/// A process-spawning tool could not reserve a slot without exceeding a
/// `max_concurrent_process_tools` cap on this view or one of its ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessAdmissionError {
    pub reason: &'static str,
    pub observed: usize,
    pub limit: usize,
}

impl std::fmt::Display for ProcessAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "process not started: budget cap '{}' would be exceeded (limit {}, observed {})",
            self.reason, self.limit, self.observed
        )
    }
}

impl std::error::Error for ProcessAdmissionError {}

/// A tool call could not reserve runtime without exhausting the aggregate
/// `max_tool_runtime` cap on this view or one of its ancestors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRuntimeAdmissionError {
    pub reason: &'static str,
    pub observed: Duration,
    pub limit: Duration,
}

impl std::fmt::Display for ToolRuntimeAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "tool not started: budget cap '{}' would be exceeded (limit {:.3}s, observed {:.3}s)",
            self.reason,
            self.limit.as_secs_f64(),
            self.observed.as_secs_f64()
        )
    }
}

impl std::error::Error for ToolRuntimeAdmissionError {}

impl ExecutionBudgetView {
    /// `true` once any cap is exceeded.
    pub fn is_exceeded(&self) -> bool {
        self.first_exceeded_reason().is_some()
    }

    /// First cap that has been exceeded (deterministic order: wall_time
    /// → tool_runtime → processes → agent_depth → tokens_in → tokens_out
    /// → cost_usd). Returns `None` if the view is still within all caps.
    ///
    /// Walks self first, then parent, then grandparent — caps closest to
    /// the leaf override caps further up.
    pub fn first_exceeded_reason(&self) -> Option<&'static str> {
        if let Some(r) = check_state(&self.inner.read()) {
            return Some(r);
        }
        for ancestor in self.ancestors.iter().rev() {
            if let Some(reason) = check_state(&ancestor.read()) {
                return Some(reason);
            }
        }
        None
    }

    /// Record token usage on this view; rolls up to all ancestors.
    pub fn record_tokens(&self, input: u64, output: u64) {
        {
            let mut s = self.inner.write();
            s.tokens_in = s.tokens_in.saturating_add(input);
            s.tokens_out = s.tokens_out.saturating_add(output);
        }
        for ancestor in self.ancestors.iter() {
            let mut p = ancestor.write();
            p.tokens_in = p.tokens_in.saturating_add(input);
            p.tokens_out = p.tokens_out.saturating_add(output);
        }
    }

    /// Record incremental USD cost on this view; rolls up to all ancestors.
    pub fn record_cost(&self, usd: f64) {
        {
            let mut s = self.inner.write();
            s.cost_usd += usd;
        }
        for ancestor in self.ancestors.iter() {
            let mut p = ancestor.write();
            p.cost_usd += usd;
        }
    }

    /// Record completed tool runtime on this view and every ancestor.
    ///
    /// The root-to-leaf lock order matches runtime admission so a sibling
    /// cannot race between a descendant charge and its parent rollup.
    pub fn record_tool_runtime(&self, runtime: Duration) {
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());
        for state in &mut states {
            state.tool_runtime = state.tool_runtime.saturating_add(runtime);
        }
    }

    /// Atomically reserve aggregate runtime for one tool call on this view and
    /// every ancestor. The admitted slice is the smaller of `requested` and
    /// the strictest remaining cap, so the final call may use the last partial
    /// slice without allowing concurrent calls to multiply it.
    ///
    /// The returned guard settles the reservation to actual elapsed runtime.
    /// Dropping it without settlement conservatively charges the reservation.
    pub fn try_reserve_tool_runtime(
        &self,
        requested: Duration,
    ) -> Result<ToolRuntimeGuard, ToolRuntimeAdmissionError> {
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());

        let mut admitted = requested;
        for state in states.iter().rev() {
            let Some(limit) = state.budget.max_tool_runtime else {
                continue;
            };
            let committed = state
                .tool_runtime
                .saturating_add(state.tool_runtime_reserved);
            let remaining = limit.saturating_sub(committed);
            if remaining.is_zero() {
                return Err(ToolRuntimeAdmissionError {
                    reason: "max_tool_runtime",
                    observed: committed.saturating_add(requested),
                    limit,
                });
            }
            admitted = admitted.min(remaining);
        }

        for state in &mut states {
            state.tool_runtime_reserved = state.tool_runtime_reserved.saturating_add(admitted);
        }
        drop(states);
        Ok(ToolRuntimeGuard {
            view: self.clone(),
            reserved: admitted,
            settled: false,
        })
    }

    /// Atomically reserve one process slot on this view and every ancestor.
    ///
    /// Locks are always acquired root-to-leaf, so sibling views cannot
    /// deadlock or each admit work against the same remaining parent slot.
    /// The returned guard releases the reservation on drop.
    pub fn try_enter_process(&self) -> Result<ToolRunGuard, ProcessAdmissionError> {
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());

        // Report the closest cap to the leaf while retaining root-to-leaf
        // lock acquisition for deadlock freedom.
        for state in states.iter().rev() {
            if let Some(limit) = state.budget.max_processes
                && state.processes_active >= limit
            {
                return Err(ProcessAdmissionError {
                    reason: "max_concurrent_process_tools",
                    observed: state.processes_active.saturating_add(1),
                    limit,
                });
            }
        }

        for state in &mut states {
            state.processes_active = state.processes_active.saturating_add(1);
        }
        drop(states);
        Ok(ToolRunGuard { view: self.clone() })
    }

    /// Remaining aggregate tool-runtime allowance across this view and every
    /// ancestor. `None` means no tool-runtime cap is configured.
    pub fn remaining_tool_runtime(&self) -> Option<Duration> {
        self.minimum_remaining(|state| {
            state.budget.max_tool_runtime.map(|cap| {
                cap.saturating_sub(
                    state
                        .tool_runtime
                        .saturating_add(state.tool_runtime_reserved),
                )
            })
        })
    }

    /// Remaining wall-time allowance across this view and every ancestor.
    /// `None` means no wall-time cap is configured.
    pub fn remaining_wall_time(&self) -> Option<Duration> {
        self.minimum_remaining(|state| {
            state
                .budget
                .max_wall_time
                .map(|cap| cap.saturating_sub(state.started_at.elapsed()))
        })
    }

    /// Remaining time a newly dispatched tool may run before either the
    /// aggregate tool-runtime or wall-time envelope is exhausted.
    pub fn remaining_tool_dispatch_time(&self) -> Option<Duration> {
        match (self.remaining_tool_runtime(), self.remaining_wall_time()) {
            (Some(tool), Some(wall)) => Some(tool.min(wall)),
            (Some(tool), None) => Some(tool),
            (None, Some(wall)) => Some(wall),
            (None, None) => None,
        }
    }

    /// Monotonic deadline for a tool future. Tokio callers convert it with
    /// `tokio::time::Instant::from_std`. An unrepresentable deadline fails
    /// closed at `Instant::now()`.
    pub fn tool_dispatch_deadline(&self) -> Option<Instant> {
        self.remaining_tool_dispatch_time().map(|remaining| {
            let now = Instant::now();
            now.checked_add(remaining).unwrap_or(now)
        })
    }

    /// Increment `agent_depth` for the lifetime of the returned guard.
    /// Used by sub-agent spawn paths to surface delegation depth.
    pub fn enter_agent(&self) -> AgentDepthGuard {
        {
            let mut s = self.inner.write();
            s.agent_depth = s.agent_depth.saturating_add(1);
        }
        for ancestor in self.ancestors.iter() {
            let mut p = ancestor.write();
            p.agent_depth = p.agent_depth.saturating_add(1);
        }
        AgentDepthGuard { view: self.clone() }
    }

    /// Build a child view. `override_` replaces the caps on the child
    /// only; parent caps still apply for the rollup. None → inherit.
    pub fn sub_budget(&self, override_: Option<ExecutionBudget>) -> ExecutionBudgetView {
        let mut ancestors = self.ancestors.as_ref().clone();
        ancestors.push(self.inner.clone());
        let budget = override_.unwrap_or_else(|| self.inner.read().budget.clone());
        ExecutionBudgetView {
            inner: Arc::new(RwLock::new(BudgetState {
                budget,
                started_at: Instant::now(),
                tool_runtime: Duration::ZERO,
                tool_runtime_reserved: Duration::ZERO,
                processes_active: 0,
                agent_depth: 0,
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
            })),
            ancestors: Arc::new(ancestors),
        }
    }

    /// Wall-time elapsed since `start_root()` (for diagnostics + the
    /// BudgetExceeded event payload in A.7).
    pub fn elapsed(&self) -> Duration {
        self.inner.read().started_at.elapsed()
    }

    /// Snapshot of current state for `BudgetExceeded.observed` formatting.
    pub fn observed_for(&self, reason: &str) -> String {
        self.with_reason_state(reason, |s| match reason {
            "max_wall_time" => format!("{:.1}s", s.started_at.elapsed().as_secs_f64()),
            "max_tool_runtime" => format!(
                "{:.1}s",
                s.tool_runtime
                    .saturating_add(s.tool_runtime_reserved)
                    .as_secs_f64()
            ),
            "max_concurrent_process_tools" => s.processes_active.to_string(),
            "max_agent_depth" => s.agent_depth.to_string(),
            "max_tokens_in" => s.tokens_in.to_string(),
            "max_tokens_out" => s.tokens_out.to_string(),
            "max_cost_usd" => format!("${:.4}", s.cost_usd),
            _ => String::new(),
        })
    }

    /// Snapshot of the cap value matching `reason` for the
    /// `BudgetExceeded.limit` payload.
    pub fn limit_for(&self, reason: &str) -> String {
        self.with_reason_state(reason, |s| match reason {
            "max_wall_time" => s
                .budget
                .max_wall_time
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_default(),
            "max_tool_runtime" => s
                .budget
                .max_tool_runtime
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_default(),
            "max_concurrent_process_tools" => s
                .budget
                .max_processes
                .map(|n| n.to_string())
                .unwrap_or_default(),
            "max_agent_depth" => s
                .budget
                .max_agent_depth
                .map(|n| n.to_string())
                .unwrap_or_default(),
            "max_tokens_in" => s
                .budget
                .max_tokens_in
                .map(|n| n.to_string())
                .unwrap_or_default(),
            "max_tokens_out" => s
                .budget
                .max_tokens_out
                .map(|n| n.to_string())
                .unwrap_or_default(),
            "max_cost_usd" => s
                .budget
                .max_cost_usd
                .map(|c| format!("${c:.4}"))
                .unwrap_or_default(),
            _ => String::new(),
        })
    }

    fn with_reason_state<T>(&self, reason: &str, render: impl Fn(&BudgetState) -> T) -> T {
        {
            let state = self.inner.read();
            if check_state(&state) == Some(reason) {
                return render(&state);
            }
        }
        for ancestor in self.ancestors.iter().rev() {
            let state = ancestor.read();
            if check_state(&state) == Some(reason) {
                return render(&state);
            }
        }
        render(&self.inner.read())
    }

    fn minimum_remaining(
        &self,
        remaining: impl Fn(&BudgetState) -> Option<Duration>,
    ) -> Option<Duration> {
        let mut minimum = remaining(&self.inner.read());
        for ancestor in self.ancestors.iter().rev() {
            if let Some(candidate) = remaining(&ancestor.read()) {
                minimum = Some(minimum.map_or(candidate, |current| current.min(candidate)));
            }
        }
        minimum
    }
}

fn check_state(s: &BudgetState) -> Option<&'static str> {
    if let Some(cap) = s.budget.max_wall_time
        && s.started_at.elapsed() > cap
    {
        return Some("max_wall_time");
    }
    if let Some(cap) = s.budget.max_tool_runtime
        && s.tool_runtime > cap
    {
        return Some("max_tool_runtime");
    }
    if let Some(cap) = s.budget.max_processes
        && s.processes_active > cap
    {
        return Some("max_concurrent_process_tools");
    }
    if let Some(cap) = s.budget.max_agent_depth
        && s.agent_depth > cap
    {
        return Some("max_agent_depth");
    }
    if let Some(cap) = s.budget.max_tokens_in
        && s.tokens_in > cap
    {
        return Some("max_tokens_in");
    }
    if let Some(cap) = s.budget.max_tokens_out
        && s.tokens_out > cap
    {
        return Some("max_tokens_out");
    }
    if let Some(cap) = s.budget.max_cost_usd
        && s.cost_usd > cap
    {
        return Some("max_cost_usd");
    }
    None
}

/// RAII guard returned by `ExecutionBudgetView::try_enter_process`; decrements
/// `processes_active` on drop on this view and all ancestors.
pub struct ToolRunGuard {
    view: ExecutionBudgetView,
}

/// RAII reservation returned by
/// [`ExecutionBudgetView::try_reserve_tool_runtime`].
pub struct ToolRuntimeGuard {
    view: ExecutionBudgetView,
    reserved: Duration,
    settled: bool,
}

impl ToolRuntimeGuard {
    /// Maximum runtime admitted for this call.
    pub fn admitted_runtime(&self) -> Duration {
        self.reserved
    }

    /// Replace the in-flight reservation with actual elapsed runtime.
    /// Repeated settlement is a no-op.
    pub fn settle(&mut self, actual: Duration) {
        if self.settled {
            return;
        }
        let mut states = Vec::with_capacity(self.view.ancestors.len() + 1);
        for ancestor in self.view.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.view.inner.write());
        for state in &mut states {
            state.tool_runtime_reserved = state.tool_runtime_reserved.saturating_sub(self.reserved);
            state.tool_runtime = state.tool_runtime.saturating_add(actual);
        }
        self.settled = true;
    }
}

impl Drop for ToolRuntimeGuard {
    fn drop(&mut self) {
        if !self.settled {
            self.settle(self.reserved);
        }
    }
}

impl Drop for ToolRunGuard {
    fn drop(&mut self) {
        let mut s = self.view.inner.write();
        s.processes_active = s.processes_active.saturating_sub(1);
        drop(s);
        for ancestor in self.view.ancestors.iter() {
            let mut p = ancestor.write();
            p.processes_active = p.processes_active.saturating_sub(1);
        }
    }
}

/// RAII guard returned by `ExecutionBudgetView::enter_agent`; decrements
/// `agent_depth` on drop on this view and all ancestors.
pub struct AgentDepthGuard {
    view: ExecutionBudgetView,
}

impl Drop for AgentDepthGuard {
    fn drop(&mut self) {
        let mut s = self.view.inner.write();
        s.agent_depth = s.agent_depth.saturating_sub(1);
        drop(s);
        for ancestor in self.view.ancestors.iter() {
            let mut p = ancestor.write();
            p.agent_depth = p.agent_depth.saturating_sub(1);
        }
    }
}
