//! `wcore-eval-scenarios` — scenario-level eval harness for `wayland-core`.
//!
//! Drives the real shipped binary against a real LLM API through a real
//! tool chain and asserts the OUTCOME — not just that the tools ran.
//! Plan: `.blackboard/EVAL-HARNESS-PLAN-2026-05-23.md` (v2, post-audit).
//!
//! ## What lands in T1 + T2 (this commit)
//!
//! - **T1**: crate scaffold, public API types, workspace wiring, the
//!   `[profile.eval]` nextest profile, and stubbed module surface so
//!   later waves (T3 assertions, T4 providers, T5 report+CLI) drop in
//!   without re-shaping.
//! - **T2**: the json-stream runner core (`runner::run`) end-to-end:
//!   spawn the binary in `--json-stream` mode (per cross-audit C-2 —
//!   the only mode that emits `SessionCost`), drive per-turn via
//!   `ProtocolCommand::Message` / `ProtocolEvent::StreamEnd`, enforce
//!   wall-time with `kill_on_drop(true)` + explicit `start_kill` on
//!   `Elapsed` (per M-1), drain stderr to a ring buffer (per M-9),
//!   and parse `ProtocolEvent::SessionCost` for cost reporting.
//!   Assertion firing + per-turn trace assembly land in T3.
//!
//! ## What's stubbed (later waves)
//!
//! Methods owned by T3/T4/T5 are declared at the TYPE level so callers
//! compile against the final API, but bodies return honest sentinel
//! values (empty vec / explicit "not implemented" payloads) — never
//! `todo!()`. The runner still drives a full scenario end-to-end;
//! assertions just won't fire yet.
//!
//! ## Silent-pass CI gate (crate-wide)
//!
//! `clippy::todo` is denied at the crate root — any `todo!()` added in
//! any module of this crate will fail
//! `cargo clippy -p wcore-eval-scenarios -- -D warnings`. This closes
//! the R-009 silent-pass archetype that motivated the T3 assertion gate
//! and prevents future `todo!()` rot in T4/T5/T6+ surfaces.
#![deny(clippy::todo)]

pub mod artifact;
pub mod assertions;
mod capability_honesty;
pub mod catalog;
mod child_env;
/// Phase 30 claims register, checker and renderer (F30-04): a claim carrying no resolving
/// evidence pointer, no bounds, or a scope its evidence does not contain cannot be rendered,
/// and the published documents are produced only by this module's renderer.
pub mod claims;
pub mod cost;
pub mod coverage;
pub mod cron_scenarios;
pub mod cross_session;
/// Phase 28 E5 black-box probe definitions (F28-01), one per dimension plus one per
/// mandatory cell. Executed by `scripts/f28-native-matrix.mjs`.
pub mod e5_cases;
/// Phase 28 E5 certification matrix generator (F28-01).
pub mod e5_matrix;
/// Phase 28 E5 soak definitions, VOID rules and verdicts (F28-02). Executed by
/// `scripts/f28-native-soak.mjs`.
pub mod e5_soak;
mod egress_evidence;
mod filesystem_evidence;
pub mod fixtures;
/// Phase 30 frontier comparative trial harness (F30-03): bounded measurements with no
/// unbounded representation, comparative results that cannot be built without every peer,
/// and a verdict rule that refuses a direction on an interval containing zero.
pub mod frontier_trials;
pub mod hook_scenarios;
pub mod journey;
pub mod judge;
pub mod mcp_scenarios;
pub mod personas;
mod process_tree;
pub mod protocol_scenarios;
pub mod providers;
#[cfg(unix)]
pub mod pty_capture;
pub mod qa;
pub mod receipt;
pub mod receipt_policy;
// Made public for Phase 24 Success Criterion 5: the journey driver promises
// exact-secret redaction before any capture reaches a planning document, and a
// mitigation reachable by nothing is not a mitigation. `wayland-journey redact`
// is the entry point; see `journey.rs`.
pub mod redaction;
/// Phase 29 signed release manifest + role-scoped trust root (F29-01/F29-04).
pub mod release_integrity;
/// Phase 29 closed four-state release ledger (F29-04).
pub mod release_states;
pub mod report;
pub mod runner;
/// Phase 29 deterministic CycloneDX SBOM transform (F29-01, closes F29-CEN-05).
pub mod sbom;
pub mod scenario;
/// Phase 30 scorecard types (F30-01, F30-02): the closed maturity and criterion
/// verdict enums, the seven-truth surface row, and the asymmetric verifier.
pub mod scorecard;
pub mod stderr_capture;
pub mod tempenv;
pub mod trace;
pub mod usability;
mod workspace_evidence;

// Public API re-exports — the surface external callers (scenario tests,
// the wayland-eval binary, future T6-T8 dispatch agents) import.
pub use assertions::{Assertion, TraceAssertion};
pub use cost::{CostReport, TurnCost};
pub use cross_session::{CrossSessionEnv, run_cross_session};
pub use judge::{Judge, Verdict};
pub use providers::{ProviderChoice, ProviderConfig, ProviderId};
pub use report::Report;
pub use runner::run;
pub use scenario::{
    ApprovalPolicy, Category, Platform, PlatformDisposition, Scenario, Turn, TurnCommand,
    UnsupportedPlatform,
};
pub use trace::{ToolTrace, TraceEntry};

// The runner produces a `ScenarioResult` — promoted to the crate root
// so callers don't need to know which sub-module owns the shape.
pub use runner::{ExecutionEvidence, Failure, ScenarioResult, TurnResult};
