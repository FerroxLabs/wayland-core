//! Dynamic Workflows — declarative RON front-end that lowers onto the
//! existing [`super::graph::GraphConfig`] IR.
//!
//! The workflow layer is a *front-door* (SPEC §4): authors write a
//! declarative RON workflow which [`dsl::parse_workflow`] parses and
//! lowers onto `GraphConfig::empty(...)` + the `add_*` builders. The
//! per-turn `ExecutionGraph` walker is left untouched; execution flows
//! through a dedicated `WorkflowRunner` (task A3) over the existing
//! FleetDispatcher path.
//!
//! Submodules are declared upfront (some as stubs implemented in later
//! tasks) so downstream tasks each own exactly one new file and never
//! re-edit this `mod.rs`.

pub mod dsl;
pub mod error;
pub mod estimate;
pub mod limits;
pub mod meta;
pub mod pipeline;
pub mod runner;
pub mod schema;
