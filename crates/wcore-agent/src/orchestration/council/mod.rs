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

pub mod resolver;
pub mod roster;

pub use resolver::{CouncilProviderResolver, ProviderResolver, ResolveError};
pub use roster::{CrucibleConfigError, ProposerSpec, Roster, validate_and_build};
