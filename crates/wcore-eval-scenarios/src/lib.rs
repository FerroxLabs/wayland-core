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
pub mod catalog;
mod child_env;
pub mod cost;
pub mod coverage;
pub mod cron_scenarios;
pub mod cross_session;
pub mod hook_scenarios;
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
mod redaction;
pub mod report;
pub mod runner;
pub mod scenario;
pub mod stderr_capture;
pub mod tempenv;
pub mod trace;
pub mod usability;

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
pub use runner::{Failure, ScenarioResult, TurnResult};
