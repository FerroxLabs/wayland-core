# wcore-budget — extraction notes (M5.3)

## Pre-existing types being moved (API parity preserved — names + signatures unchanged)

From `wcore-agent/src/budget.rs` → `wcore-budget/src/execution.rs`:
- `ExecutionBudget` (config struct, 7 optional caps)
- `ExecutionBudgetView` (Arc-shared, tree-rollup runtime view)
- `ToolRunGuard`, `AgentDepthGuard` (RAII counters)
- `ExecutionBudget::start_root()`, `View::is_exceeded()`, `first_exceeded_reason()`, `record_tokens()`, `record_cost()`, `enter_tool_run()`, `enter_agent()`, `sub_budget()`, `elapsed()`, `observed_for()`, `limit_for()`

From `wcore-config/src/budget.rs` → `wcore-budget/src/config.rs`:
- `BudgetConfig` (TOML schema, 7 optional caps in seconds/units)

Conversion `impl From<&BudgetConfig> for ExecutionBudget` (was in wcore-agent because of the dep direction) now lives in `wcore-budget` since both sides are co-located.

## Re-exports (so call sites compile unchanged)
- `wcore-agent/src/budget.rs` becomes a re-export shim: `pub use wcore_budget::{ExecutionBudget, ExecutionBudgetView, ToolRunGuard, AgentDepthGuard};`
- `wcore-config/src/budget.rs` becomes: `pub use wcore_budget::BudgetConfig;`
- All 43 call sites (`wcore_agent::budget::*`, `wcore_config::budget::BudgetConfig`) continue to compile.

## Call sites verified (none modified)
- `wcore-agent/src/{bootstrap,cancel}.rs`, `wcore-agent/src/orchestration/{mod,monitor}.rs`
- `wcore-agent/tests/{budget_test, bootstrap_budget_test, midflight_monitor_test, budget_guard_lifecycle_test, common/mod, acceptance/helpers, e2e/{compaction,openai,anthropic}}.rs`
- `wcore-protocol/src/events.rs` (documentation comment only — no type usage)

## NEW types added in this extract (M5.3 extension)
- `BudgetCap` (builder): `per_session_tokens`, `per_session_usd`, `per_user_daily_usd`. Distinct from `ExecutionBudget` (which is global session caps); `BudgetCap` is for the new session-keyed / user-keyed enforcement model.
- `BudgetTracker { caps: BudgetCap, session_state: HashMap<String, ChargeState>, user_daily: HashMap<String, DailyUsd> }` — `charge(session_id, tokens, usd)` plus `charge_for_user(session_id, user_id, tokens, usd)`.
- `BudgetError { CapExceeded { kind, limit, observed }, ... }` (thiserror).
- `BudgetEvent::{Charge, CapWarn, CapBlock}` (serde::Serialize).
- `BudgetEventSink` trait + `set_event_sink(&self, Arc<dyn BudgetEventSink>)` on `BudgetTracker`.

## Observability wire-in (telemetry expansion)
- `wcore-observability::sink::ObservabilityBudgetEventBridge` mirrors the M3.3 `ObservabilityMemoryTraceBridge` pattern — implements `BudgetEventSink`, forwards serialized `BudgetEvent` JSON to an inner `Arc<dyn SpanSink>`.
- `BudgetTracker::charge` emits `Charge` on every call, `CapWarn` at ≥80% of cap, `CapBlock` on cap exceeded.

## Engine hook wire-in (caps expansion)
- `wcore-agent/src/bootstrap.rs`: alongside the existing `ExecutionBudgetView`, build a `BudgetTracker` from a new optional `Config.session_cap` field (default = no caps) and pass it into `AgentEngine`. Bridge wired to the same `SpanSink` as memory.
- `wcore-agent/src/engine.rs`: at the LLM response handling site, call `tracker.charge(&session_id, tokens, usd)` — emits Charge on every response.

## Acceptance gates
- 28 pre-existing tests (25 wcore-agent budget + 3 wcore-config budget) pass UNCHANGED from the same call sites.
- New tests live in `crates/wcore-budget/tests/` (cap_enforcement, per_user_daily_cap, tracker_smoke, event_bridge) and `crates/wcore-budget/src/` inline.
