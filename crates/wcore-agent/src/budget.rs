//! M5.3 — re-export shim for the runtime-budget types now owned by the
//! dedicated `wcore-budget` crate.
//!
//! The original `ExecutionBudget` + `ExecutionBudgetView` + RAII guards lived
//! here (W8a A.2). They moved to `wcore-budget` so the M5.3 session/user-keyed
//! `BudgetTracker` + `BudgetEvent` telemetry surface could share defaults and
//! TOML schema without an upward dep into `wcore-agent`. All pre-existing
//! call sites (`use wcore_agent::budget::ExecutionBudget;`, …) keep
//! compiling unchanged via these re-exports.

pub use wcore_budget::{AgentDepthGuard, ExecutionBudget, ExecutionBudgetView, ToolRunGuard};
