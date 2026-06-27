//! Crucible (Mixture-of-Providers) council module.
//!
//! Slice-1 hosts the read-only cross-provider council: N sub-agents each
//! pinned to a different LLM provider answer a task in parallel, and a
//! provenance-aware aggregator fuses them into one result.
//!
//! This module root re-exports the council's building blocks. The provider
//! resolution seam ([`CouncilProviderResolver`]) lives here in `wcore-agent`
//! (not `wcore-types`) because turning a provider id string into a keyed
//! `Arc<dyn LlmProvider>` requires `wcore-providers` + `wcore-config`, which
//! sit above the leaf types crate.

pub mod advisor;
pub mod aggregator;
pub mod assembler;
pub mod assembler_log;
pub mod driver;
pub mod gate;
pub mod plan_card;
pub mod proposal;
pub mod resolver;
pub mod roster;
pub mod run;
pub mod spend;

pub use advisor::{ADVISOR_HEADER, build_advisor_turn};
pub use aggregator::{Aggregator, LlmSynthesisAggregator};
pub use assembler::{AssemblyPlan, AssemblyPolicy, DEFAULT_FLUX_POOL, assemble, bootstrap_pool};
pub use assembler_log::{assembly_log_line, log_assembly};
pub use driver::{
    CouncilApprover, CouncilOverrides, CouncilRunResult, apply_judge_override, build_gate,
    build_policy, drive_council, roster_from_plan,
};
pub use gate::{CouncilDecision, GateConfig, Stakes, classify_task, member_count};
pub use plan_card::plan_to_card;
pub use proposal::{AggregateResult, Proposal};
pub use resolver::{CouncilProviderResolver, ProviderResolver, ResolveError, family};
pub use roster::{CrucibleConfigError, ProposerSpec, Roster, validate_and_build};
pub use run::{
    COUNCIL_PROPOSER_SYSTEM_PROMPT, CouncilError, CouncilOutcome, DEFAULT_PROPOSER_MAX_TOKENS,
    SkippedProposer, run_council,
};
pub use spend::{CouncilSpend, PreflightEstimate, ProviderSpend, is_priceable};
