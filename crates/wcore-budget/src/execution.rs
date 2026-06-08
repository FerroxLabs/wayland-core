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
                processes_active: 0,
                agent_depth: 0,
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
            })),
            parent: None,
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
    parent: Option<Arc<RwLock<BudgetState>>>,
}

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
        if let Some(parent) = self.parent.as_ref() {
            return check_state(&parent.read());
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
        if let Some(parent) = self.parent.as_ref() {
            let mut p = parent.write();
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
        if let Some(parent) = self.parent.as_ref() {
            let mut p = parent.write();
            p.cost_usd += usd;
        }
    }

    /// Increment `processes_active` for the lifetime of the returned
    /// guard. Used by tools that fork sub-processes (Bash, Script) to
    /// surface concurrent-process pressure to the budget.
    pub fn enter_tool_run(&self) -> ToolRunGuard {
        {
            let mut s = self.inner.write();
            s.processes_active = s.processes_active.saturating_add(1);
        }
        if let Some(parent) = self.parent.as_ref() {
            let mut p = parent.write();
            p.processes_active = p.processes_active.saturating_add(1);
        }
        ToolRunGuard { view: self.clone() }
    }

    /// Increment `agent_depth` for the lifetime of the returned guard.
    /// Used by sub-agent spawn paths to surface delegation depth.
    pub fn enter_agent(&self) -> AgentDepthGuard {
        {
            let mut s = self.inner.write();
            s.agent_depth = s.agent_depth.saturating_add(1);
        }
        if let Some(parent) = self.parent.as_ref() {
            let mut p = parent.write();
            p.agent_depth = p.agent_depth.saturating_add(1);
        }
        AgentDepthGuard { view: self.clone() }
    }

    /// Build a child view. `override_` replaces the caps on the child
    /// only; parent caps still apply for the rollup. None → inherit.
    pub fn sub_budget(&self, override_: Option<ExecutionBudget>) -> ExecutionBudgetView {
        let parent_arc = self.inner.clone();
        let budget = override_.unwrap_or_else(|| self.inner.read().budget.clone());
        let started_at = self.inner.read().started_at;
        ExecutionBudgetView {
            inner: Arc::new(RwLock::new(BudgetState {
                budget,
                started_at,
                tool_runtime: Duration::ZERO,
                processes_active: 0,
                agent_depth: 0,
                tokens_in: 0,
                tokens_out: 0,
                cost_usd: 0.0,
            })),
            parent: Some(parent_arc),
        }
    }

    /// Wall-time elapsed since `start_root()` (for diagnostics + the
    /// BudgetExceeded event payload in A.7).
    pub fn elapsed(&self) -> Duration {
        self.inner.read().started_at.elapsed()
    }

    /// Snapshot of current state for `BudgetExceeded.observed` formatting.
    pub fn observed_for(&self, reason: &str) -> String {
        let s = self.inner.read();
        match reason {
            "max_wall_time" => format!("{:.1}s", s.started_at.elapsed().as_secs_f64()),
            "max_tool_runtime" => format!("{:.1}s", s.tool_runtime.as_secs_f64()),
            "max_processes" => s.processes_active.to_string(),
            "max_agent_depth" => s.agent_depth.to_string(),
            "max_tokens_in" => s.tokens_in.to_string(),
            "max_tokens_out" => s.tokens_out.to_string(),
            "max_cost_usd" => format!("${:.4}", s.cost_usd),
            _ => String::new(),
        }
    }

    /// Snapshot of the cap value matching `reason` for the
    /// `BudgetExceeded.limit` payload.
    pub fn limit_for(&self, reason: &str) -> String {
        let s = self.inner.read();
        match reason {
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
            "max_processes" => s
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
        }
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
        return Some("max_processes");
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

/// RAII guard returned by `ExecutionBudgetView::enter_tool_run`; decrements
/// `processes_active` on drop on this view and all ancestors.
pub struct ToolRunGuard {
    view: ExecutionBudgetView,
}

impl Drop for ToolRunGuard {
    fn drop(&mut self) {
        let mut s = self.view.inner.write();
        s.processes_active = s.processes_active.saturating_sub(1);
        drop(s);
        if let Some(parent) = self.view.parent.as_ref() {
            let mut p = parent.write();
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
        if let Some(parent) = self.view.parent.as_ref() {
            let mut p = parent.write();
            p.agent_depth = p.agent_depth.saturating_sub(1);
        }
    }
}
