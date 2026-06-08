//! W8a A.5 — `BudgetConfig` TOML schema for `~/.wayland-core/config.toml`.
//!
//! Every cap is optional. The runtime `ExecutionBudget` is constructed from
//! this struct via `From` (defined in `execution.rs`). All fields default
//! to `None`, i.e. "no cap" — opt-in only.
//!
//! Moved verbatim from `wcore-config/src/budget.rs` in M5.3 (`wcore-config`
//! now re-exports this type so all pre-existing call sites compile
//! unchanged).
//!
//! Example TOML:
//!
//! ```toml
//! [budget]
//! max_wall_time_secs    = 600
//! max_tool_runtime_secs = 120
//! max_processes         = 8
//! max_agent_depth       = 4
//! max_tokens_in         = 200000
//! max_tokens_out        = 16384
//! max_cost_usd          = 1.50
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetConfig {
    pub max_wall_time_secs: Option<u64>,
    pub max_tool_runtime_secs: Option<u64>,
    pub max_processes: Option<usize>,
    pub max_agent_depth: Option<usize>,
    pub max_tokens_in: Option<u64>,
    pub max_tokens_out: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_toml_gives_default() {
        let bc: BudgetConfig = toml::from_str("").unwrap();
        assert_eq!(bc, BudgetConfig::default());
        assert!(bc.max_wall_time_secs.is_none());
        assert!(bc.max_cost_usd.is_none());
    }

    #[test]
    fn explicit_fields_parsed() {
        let bc: BudgetConfig = toml::from_str(
            r#"
                max_wall_time_secs = 600
                max_tokens_out = 16384
                max_cost_usd = 1.5
            "#,
        )
        .unwrap();
        assert_eq!(bc.max_wall_time_secs, Some(600));
        assert_eq!(bc.max_tokens_out, Some(16384));
        assert_eq!(bc.max_cost_usd, Some(1.5));
        assert!(bc.max_processes.is_none());
    }

    #[test]
    fn roundtrip_toml() {
        let original = BudgetConfig {
            max_wall_time_secs: Some(300),
            max_processes: Some(4),
            max_cost_usd: Some(0.25),
            ..Default::default()
        };
        let s = toml::to_string(&original).unwrap();
        let parsed: BudgetConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }
}
