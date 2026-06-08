//! M5.3 — re-export shim for the TOML budget schema now owned by the
//! dedicated `wcore-budget` crate.
//!
//! The original `BudgetConfig` lived here (W8a A.5). It moved to
//! `wcore-budget` so the runtime tracker + cap types could share the same
//! TOML schema without an upward dep. Pre-existing call sites that import
//! `wcore_config::budget::BudgetConfig` keep compiling unchanged.

pub use wcore_budget::BudgetConfig;
