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

/// Validation failures while persisting or restoring durable budget authority.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BudgetSnapshotError {
    /// The serialized representation is malformed, unsafe, or inconsistent.
    #[error("invalid budget snapshot: {reason}")]
    Invalid { reason: String },
    /// A snapshot can only be applied once to a pristine restore target.
    #[error("budget snapshot restore target is not pristine")]
    RestoreTargetNotPristine,
    /// Snapshot schemas are versioned and must be explicitly supported.
    #[error("unsupported budget snapshot schema version {found}; expected {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
}

pub use config::{BudgetConfig, BudgetConfigError};
pub use execution::{
    AgentDepthGuard, ExecutionBudget, ExecutionBudgetSnapshot, ExecutionBudgetView,
    ProcessCleanupProof, ToolRunGuard,
};
pub use tracker::{
    BudgetCap, BudgetCapBuilder, BudgetError, BudgetEvent, BudgetEventSink, BudgetExtensionError,
    BudgetExtensionOutcome, BudgetReservation, BudgetTracker, BudgetTrackerSnapshot,
    RestoredReservationReconciliation,
};
