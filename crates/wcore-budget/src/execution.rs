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
    pub fn start_root(mut self) -> ExecutionBudgetView {
        if self
            .max_cost_usd
            .is_some_and(|usd| !usd.is_finite() || usd < 0.0)
        {
            self.max_cost_usd = Some(0.0);
        }
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
                restore_applied: false,
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
    restore_applied: bool,
}

const EXECUTION_BUDGET_SNAPSHOT_VERSION: u32 = 1;

/// Serializable, immutable copy of a view and its root-to-parent authority.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionBudgetSnapshot {
    schema_version: u32,
    states: Vec<ExecutionBudgetStateSnapshot>,
}

/// Evidence supplied by the restart coordinator after it has proven that no
/// process admitted by the captured budget remains alive.
///
/// The budget layer validates the receipt shape but deliberately does not
/// interpret platform-specific cleanup authorities. That proof belongs to the
/// caller (for example, a cgroup or Job Object owner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCleanupProof {
    authority: String,
    receipt_digest: [u8; 32],
}

impl ProcessCleanupProof {
    /// Construct a cleanup receipt marker from the platform authority name and
    /// its non-zero 256-bit receipt digest.
    pub fn new(
        authority: impl Into<String>,
        receipt_digest: [u8; 32],
    ) -> Result<Self, crate::BudgetSnapshotError> {
        let authority = authority.into();
        if authority.trim().is_empty() {
            return Err(invalid_execution_snapshot(
                "process cleanup proof authority must not be empty",
            ));
        }
        if receipt_digest == [0; 32] {
            return Err(invalid_execution_snapshot(
                "process cleanup proof digest must not be all zeroes",
            ));
        }
        Ok(Self {
            authority,
            receipt_digest,
        })
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }

    pub fn receipt_digest(&self) -> &[u8; 32] {
        &self.receipt_digest
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionBudgetStateSnapshot {
    budget: ExecutionBudget,
    elapsed: Duration,
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
    /// Capture this view plus every ancestor required to retain stricter caps
    /// and roll-up counters after restart.
    pub fn snapshot(&self) -> Result<ExecutionBudgetSnapshot, crate::BudgetSnapshotError> {
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        let mut guards = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            guards.push(ancestor.read());
        }
        guards.push(self.inner.read());
        for state in &guards {
            states.push(ExecutionBudgetStateSnapshot {
                budget: state.budget.clone(),
                elapsed: state.started_at.elapsed(),
                tool_runtime: state.tool_runtime,
                tool_runtime_reserved: state.tool_runtime_reserved,
                processes_active: state.processes_active,
                agent_depth: state.agent_depth,
                tokens_in: state.tokens_in,
                tokens_out: state.tokens_out,
                cost_usd: state.cost_usd,
            });
        }
        let snapshot = ExecutionBudgetSnapshot {
            schema_version: EXECUTION_BUDGET_SNAPSHOT_VERSION,
            states,
        };
        validate_execution_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Build a runtime view from serialized enforcement authority.
    pub fn from_snapshot(
        snapshot: ExecutionBudgetSnapshot,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        build_execution_view_from_snapshot(snapshot)
    }

    /// Restore enforcement authority while charging process downtime against
    /// every wall-clock envelope in the captured root-to-leaf chain.
    ///
    /// The caller derives `downtime` from a durable wall-clock capture stored
    /// alongside the monotonic snapshot. Saturation fails closed: an
    /// unrepresentable elapsed duration restores as maximally elapsed rather
    /// than resetting the deadline.
    pub fn from_snapshot_with_downtime(
        mut snapshot: ExecutionBudgetSnapshot,
        downtime: Duration,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        for state in &mut snapshot.states {
            state.elapsed = state.elapsed.saturating_add(downtime);
        }
        build_execution_view_from_snapshot(snapshot)
    }

    /// Restore a captured root-to-leaf budget for a new process under current
    /// root policy.
    ///
    /// The captured root caps are intersected pointwise with `current_policy`.
    /// `elapsed_adjustment` is chosen by the caller so ActiveRuntime and
    /// AbsoluteDeadline authorities retain their distinct semantics. In-flight
    /// tool runtime is charged at its admitted maximum and agent depth is reset
    /// because in-process guards cannot survive restart. Process counters are
    /// retained unless a platform cleanup proof is supplied.
    pub fn from_snapshot_for_restart(
        mut snapshot: ExecutionBudgetSnapshot,
        current_policy: ExecutionBudget,
        elapsed_adjustment: Duration,
        process_cleanup: Option<&ProcessCleanupProof>,
    ) -> Result<Self, crate::BudgetSnapshotError> {
        validate_execution_snapshot(&snapshot)?;
        let current_policy = normalize_execution_budget(current_policy);
        let root = snapshot
            .states
            .first_mut()
            .ok_or_else(|| invalid_execution_snapshot("execution snapshot has no root state"))?;
        root.budget = intersect_execution_budget(&root.budget, &current_policy);

        for state in &mut snapshot.states {
            state.elapsed = state.elapsed.saturating_add(elapsed_adjustment);
            state.tool_runtime = state
                .tool_runtime
                .saturating_add(state.tool_runtime_reserved);
            state.tool_runtime_reserved = Duration::ZERO;
            state.agent_depth = 0;
            if process_cleanup.is_some() {
                state.processes_active = 0;
            }
        }
        build_execution_view_from_snapshot(snapshot)
    }

    /// Atomically apply serialized authority to an unshared, pristine root.
    /// Reapplication is rejected even when the restored counters were empty.
    pub fn restore_snapshot(
        &mut self,
        snapshot: ExecutionBudgetSnapshot,
    ) -> Result<(), crate::BudgetSnapshotError> {
        if !self.ancestors.is_empty()
            || Arc::strong_count(&self.inner) != 1
            || Arc::strong_count(&self.ancestors) != 1
            || !self.durable_state_is_empty()
        {
            return Err(crate::BudgetSnapshotError::RestoreTargetNotPristine);
        }
        *self = build_execution_view_from_snapshot(snapshot)?;
        Ok(())
    }

    fn durable_state_is_empty(&self) -> bool {
        let state = self.inner.read();
        !state.restore_applied
            && state.tool_runtime.is_zero()
            && state.tool_runtime_reserved.is_zero()
            && state.processes_active == 0
            && state.agent_depth == 0
            && state.tokens_in == 0
            && state.tokens_out == 0
            && state.cost_usd == 0.0
    }

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
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());
        for state in &mut states {
            state.tokens_in = state.tokens_in.saturating_add(input);
            state.tokens_out = state.tokens_out.saturating_add(output);
        }
    }

    /// Record incremental USD cost on this view; rolls up to all ancestors.
    pub fn record_cost(&self, usd: f64) {
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());
        for state in &mut states {
            state.cost_usd = conservative_cost_add(state.cost_usd, usd);
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
        let mut states = Vec::with_capacity(self.ancestors.len() + 1);
        for ancestor in self.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.inner.write());
        for state in &mut states {
            state.agent_depth = state.agent_depth.saturating_add(1);
        }
        drop(states);
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
                restore_applied: false,
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

fn validate_execution_snapshot(
    snapshot: &ExecutionBudgetSnapshot,
) -> Result<(), crate::BudgetSnapshotError> {
    if snapshot.schema_version != EXECUTION_BUDGET_SNAPSHOT_VERSION {
        return Err(crate::BudgetSnapshotError::UnsupportedVersion {
            found: snapshot.schema_version,
            expected: EXECUTION_BUDGET_SNAPSHOT_VERSION,
        });
    }
    if snapshot.states.is_empty() {
        return Err(invalid_execution_snapshot(
            "execution snapshot must contain at least one state",
        ));
    }
    let now = Instant::now();
    for (index, state) in snapshot.states.iter().enumerate() {
        validate_execution_usd(
            &format!("states[{index}].budget.max_cost_usd"),
            state.budget.max_cost_usd,
        )?;
        validate_execution_usd(&format!("states[{index}].cost_usd"), Some(state.cost_usd))?;
        if now.checked_sub(state.elapsed).is_none() {
            return Err(invalid_execution_snapshot(format!(
                "states[{index}].elapsed cannot be represented by the monotonic clock"
            )));
        }
    }
    Ok(())
}

fn build_execution_view_from_snapshot(
    snapshot: ExecutionBudgetSnapshot,
) -> Result<ExecutionBudgetView, crate::BudgetSnapshotError> {
    validate_execution_snapshot(&snapshot)?;
    let now = Instant::now();
    let mut states = Vec::with_capacity(snapshot.states.len());
    for state in snapshot.states {
        let started_at = now
            .checked_sub(state.elapsed)
            .ok_or_else(|| invalid_execution_snapshot("elapsed authority cannot be represented"))?;
        states.push(Arc::new(RwLock::new(BudgetState {
            budget: state.budget,
            started_at,
            tool_runtime: state.tool_runtime,
            tool_runtime_reserved: state.tool_runtime_reserved,
            processes_active: state.processes_active,
            agent_depth: state.agent_depth,
            tokens_in: state.tokens_in,
            tokens_out: state.tokens_out,
            cost_usd: state.cost_usd,
            restore_applied: true,
        })));
    }
    let inner = states
        .pop()
        .ok_or_else(|| invalid_execution_snapshot("execution snapshot has no leaf state"))?;
    Ok(ExecutionBudgetView {
        inner,
        ancestors: Arc::new(states),
    })
}

fn normalize_execution_budget(mut budget: ExecutionBudget) -> ExecutionBudget {
    if budget
        .max_cost_usd
        .is_some_and(|usd| !usd.is_finite() || usd < 0.0)
    {
        budget.max_cost_usd = Some(0.0);
    }
    budget
}

fn intersect_execution_budget(
    captured: &ExecutionBudget,
    current: &ExecutionBudget,
) -> ExecutionBudget {
    ExecutionBudget {
        max_wall_time: intersect_optional(captured.max_wall_time, current.max_wall_time),
        max_tool_runtime: intersect_optional(captured.max_tool_runtime, current.max_tool_runtime),
        max_processes: intersect_optional(captured.max_processes, current.max_processes),
        max_agent_depth: intersect_optional(captured.max_agent_depth, current.max_agent_depth),
        max_tokens_in: intersect_optional(captured.max_tokens_in, current.max_tokens_in),
        max_tokens_out: intersect_optional(captured.max_tokens_out, current.max_tokens_out),
        max_cost_usd: intersect_optional_f64(captured.max_cost_usd, current.max_cost_usd),
    }
}

fn intersect_optional<T: Ord + Copy>(captured: Option<T>, current: Option<T>) -> Option<T> {
    match (captured, current) {
        (Some(captured), Some(current)) => Some(captured.min(current)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn intersect_optional_f64(captured: Option<f64>, current: Option<f64>) -> Option<f64> {
    match (captured, current) {
        (Some(captured), Some(current)) => Some(captured.min(current)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn validate_execution_usd(
    field: &str,
    value: Option<f64>,
) -> Result<(), crate::BudgetSnapshotError> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        return Err(invalid_execution_snapshot(format!(
            "{field} must be finite and non-negative"
        )));
    }
    Ok(())
}

fn invalid_execution_snapshot(reason: impl Into<String>) -> crate::BudgetSnapshotError {
    crate::BudgetSnapshotError::Invalid {
        reason: reason.into(),
    }
}

fn conservative_cost_add(current: f64, increment: f64) -> f64 {
    if !current.is_finite() || current < 0.0 || !increment.is_finite() || increment < 0.0 {
        return f64::MAX;
    }
    let sum = current + increment;
    if sum.is_finite() { sum } else { f64::MAX }
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
        let mut states = Vec::with_capacity(self.view.ancestors.len() + 1);
        for ancestor in self.view.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.view.inner.write());
        for state in &mut states {
            state.processes_active = state.processes_active.saturating_sub(1);
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
        let mut states = Vec::with_capacity(self.view.ancestors.len() + 1);
        for ancestor in self.view.ancestors.iter() {
            states.push(ancestor.write());
        }
        states.push(self.view.inner.write());
        for state in &mut states {
            state.agent_depth = state.agent_depth.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_snapshot_json_roundtrip_preserves_tree_and_counters() {
        let root = ExecutionBudget {
            max_wall_time: Some(Duration::from_secs(60)),
            max_tool_runtime: Some(Duration::from_secs(10)),
            max_processes: Some(1),
            max_agent_depth: Some(2),
            max_tokens_in: Some(70),
            max_tokens_out: Some(20),
            max_cost_usd: Some(1.0),
        }
        .start_root();
        let child = root.sub_budget(Some(ExecutionBudget {
            max_tokens_in: Some(100),
            max_cost_usd: Some(2.0),
            ..Default::default()
        }));
        child.record_tokens(60, 10);
        child.record_cost(0.60);
        child.record_tool_runtime(Duration::from_secs(2));
        let _runtime = child
            .try_reserve_tool_runtime(Duration::from_secs(3))
            .unwrap();
        let _process = child.try_enter_process().unwrap();
        let _depth = child.enter_agent();

        let snapshot = child.snapshot().unwrap();
        let json = serde_json::to_vec(&snapshot).unwrap();
        let decoded: ExecutionBudgetSnapshot = serde_json::from_slice(&json).unwrap();
        let restored = ExecutionBudgetView::from_snapshot(decoded).unwrap();

        assert_eq!(restored.observed_for("max_tokens_in"), "60");
        assert_eq!(
            restored.remaining_tool_runtime(),
            Some(Duration::from_secs(5))
        );
        assert!(restored.try_enter_process().is_err());
        restored.record_tokens(11, 0);
        assert_eq!(restored.first_exceeded_reason(), Some("max_tokens_in"));
        assert_eq!(restored.limit_for("max_tokens_in"), "70");
    }

    #[test]
    fn execution_restore_refuses_duplicate_application() {
        let snapshot = ExecutionBudget::default().start_root().snapshot().unwrap();
        let mut target = ExecutionBudget::default().start_root();

        target.restore_snapshot(snapshot.clone()).unwrap();
        assert_eq!(
            target.restore_snapshot(snapshot).unwrap_err(),
            crate::BudgetSnapshotError::RestoreTargetNotPristine
        );
    }

    #[test]
    fn execution_snapshot_rejects_invalid_monetary_authority() {
        let mut cost_snapshot = ExecutionBudget::default().start_root().snapshot().unwrap();
        cost_snapshot.states[0].cost_usd = f64::NAN;
        assert!(matches!(
            ExecutionBudgetView::from_snapshot(cost_snapshot),
            Err(crate::BudgetSnapshotError::Invalid { .. })
        ));

        let mut cap_snapshot = ExecutionBudget::default().start_root().snapshot().unwrap();
        cap_snapshot.states[0].budget.max_cost_usd = Some(f64::NEG_INFINITY);
        assert!(matches!(
            ExecutionBudgetView::from_snapshot(cap_snapshot),
            Err(crate::BudgetSnapshotError::Invalid { .. })
        ));

        let budget = ExecutionBudget {
            max_cost_usd: Some(1.0),
            ..Default::default()
        }
        .start_root();
        budget.record_cost(f64::NAN);
        assert_eq!(budget.first_exceeded_reason(), Some("max_cost_usd"));
        assert!(budget.snapshot().is_ok());
    }

    #[test]
    fn execution_restore_retains_elapsed_wall_time_authority() {
        let view = ExecutionBudget {
            max_wall_time: Some(Duration::from_secs(4)),
            ..Default::default()
        }
        .start_root();
        let mut snapshot = view.snapshot().unwrap();
        snapshot.states[0].elapsed = Duration::from_secs(5);

        let restored = ExecutionBudgetView::from_snapshot(snapshot).unwrap();

        assert_eq!(restored.first_exceeded_reason(), Some("max_wall_time"));
        assert_eq!(restored.remaining_wall_time(), Some(Duration::ZERO));
    }

    #[test]
    fn execution_restore_charges_process_downtime_to_wall_time() {
        let view = ExecutionBudget {
            max_wall_time: Some(Duration::from_secs(10)),
            ..Default::default()
        }
        .start_root();
        let mut snapshot = view.snapshot().unwrap();
        snapshot.states[0].elapsed = Duration::from_secs(4);

        let restored =
            ExecutionBudgetView::from_snapshot_with_downtime(snapshot, Duration::from_secs(7))
                .unwrap();

        assert_eq!(restored.first_exceeded_reason(), Some("max_wall_time"));
        assert_eq!(restored.remaining_wall_time(), Some(Duration::ZERO));
    }

    #[test]
    fn execution_restore_keeps_inflight_runtime_reservations_conservative() {
        let view = ExecutionBudget {
            max_tool_runtime: Some(Duration::from_secs(10)),
            ..Default::default()
        }
        .start_root();
        view.record_tool_runtime(Duration::from_secs(2));
        let _inflight = view
            .try_reserve_tool_runtime(Duration::from_secs(3))
            .unwrap();
        let snapshot = view.snapshot().unwrap();

        let restored = ExecutionBudgetView::from_snapshot(snapshot).unwrap();
        let admitted = restored
            .try_reserve_tool_runtime(Duration::from_secs(10))
            .unwrap();

        assert_eq!(admitted.admitted_runtime(), Duration::from_secs(5));
    }

    #[test]
    fn restart_restore_intersects_current_root_policy_and_settles_transients() {
        let root = ExecutionBudget {
            max_wall_time: Some(Duration::from_secs(100)),
            max_tool_runtime: Some(Duration::from_secs(20)),
            max_processes: Some(3),
            max_agent_depth: Some(4),
            max_tokens_in: Some(1_000),
            max_tokens_out: Some(500),
            max_cost_usd: Some(10.0),
        }
        .start_root();
        root.record_tokens(200, 100);
        root.record_cost(2.0);
        root.record_tool_runtime(Duration::from_secs(3));
        let _runtime = root
            .try_reserve_tool_runtime(Duration::from_secs(4))
            .unwrap();
        let _process = root.try_enter_process().unwrap();
        let _depth = root.enter_agent();
        let captured = root.snapshot().unwrap();

        let restored = ExecutionBudgetView::from_snapshot_for_restart(
            captured,
            ExecutionBudget {
                max_wall_time: Some(Duration::from_secs(80)),
                max_tool_runtime: Some(Duration::from_secs(30)),
                max_processes: Some(2),
                max_agent_depth: Some(3),
                max_tokens_in: Some(800),
                max_tokens_out: Some(600),
                max_cost_usd: Some(8.0),
            },
            Duration::from_secs(5),
            None,
        )
        .unwrap();
        let snapshot = restored.snapshot().unwrap();
        let state = &snapshot.states[0];

        assert_eq!(state.budget.max_wall_time, Some(Duration::from_secs(80)));
        assert_eq!(state.budget.max_tool_runtime, Some(Duration::from_secs(20)));
        assert_eq!(state.budget.max_processes, Some(2));
        assert_eq!(state.budget.max_agent_depth, Some(3));
        assert_eq!(state.budget.max_tokens_in, Some(800));
        assert_eq!(state.budget.max_tokens_out, Some(500));
        assert_eq!(state.budget.max_cost_usd, Some(8.0));
        assert!(state.elapsed >= Duration::from_secs(5));
        assert_eq!(state.tool_runtime, Duration::from_secs(7));
        assert_eq!(state.tool_runtime_reserved, Duration::ZERO);
        assert_eq!(state.processes_active, 1);
        assert_eq!(state.agent_depth, 0);
        assert_eq!(state.tokens_in, 200);
        assert_eq!(state.tokens_out, 100);
        assert_eq!(state.cost_usd, 2.0);
    }

    #[test]
    fn restart_restore_clears_process_counts_only_with_cleanup_proof() {
        let view = ExecutionBudget {
            max_processes: Some(1),
            ..Default::default()
        }
        .start_root();
        let _process = view.try_enter_process().unwrap();
        let captured = view.snapshot().unwrap();

        let without_proof = ExecutionBudgetView::from_snapshot_for_restart(
            captured.clone(),
            ExecutionBudget::default(),
            Duration::ZERO,
            None,
        )
        .unwrap();
        assert!(without_proof.try_enter_process().is_err());

        let proof = ProcessCleanupProof::new("linux-cgroup-v2", [7; 32]).unwrap();
        let with_proof = ExecutionBudgetView::from_snapshot_for_restart(
            captured,
            ExecutionBudget::default(),
            Duration::ZERO,
            Some(&proof),
        )
        .unwrap();
        assert!(with_proof.try_enter_process().is_ok());
        assert_eq!(proof.authority(), "linux-cgroup-v2");
        assert_eq!(proof.receipt_digest(), &[7; 32]);
    }

    #[test]
    fn process_cleanup_proof_rejects_ambiguous_receipts() {
        assert!(ProcessCleanupProof::new(" ", [1; 32]).is_err());
        assert!(ProcessCleanupProof::new("linux-cgroup-v2", [0; 32]).is_err());
    }
}
