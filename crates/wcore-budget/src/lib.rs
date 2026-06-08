//! wcore-budget — budget caps, trackers, and telemetry events.
//!
//! M5.3 extracts the pre-existing budget surfaces from `wcore-agent` and
//! `wcore-config` into this dedicated crate, then adds session-keyed /
//! user-keyed enforcement (`BudgetCap` + `BudgetTracker`) and a
//! `BudgetEvent` telemetry channel that mirrors the M3.3 memory pattern.
//!
//! ## Two enforcement models live here
//!
//! - **Global session caps** (`ExecutionBudget` + `ExecutionBudgetView`):
//!   the W8a tree-shaped, Arc-shared, wall-time / tool-runtime / token /
//!   cost rollup. Behaviour preserved verbatim from the wcore-agent
//!   original so every pre-existing call site compiles unchanged.
//!
//! - **Per-session / per-user caps** (`BudgetCap` + `BudgetTracker`):
//!   the M5.3-new model. Keyed by session id and (optionally) user id,
//!   with an event sink that emits `BudgetEvent::{Charge, CapWarn,
//!   CapBlock}` for observability.
//!
//! The TOML schema (`BudgetConfig`) ships here too because both runtime
//! models need to share defaults. `wcore-config::budget` is a re-export.

pub mod config;
pub mod execution;
pub mod tracker;

pub use config::BudgetConfig;
pub use execution::{AgentDepthGuard, ExecutionBudget, ExecutionBudgetView, ToolRunGuard};
pub use tracker::{
    BudgetCap, BudgetCapBuilder, BudgetError, BudgetEvent, BudgetEventSink, BudgetTracker,
};
