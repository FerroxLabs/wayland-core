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
//! max_concurrent_process_tools = 8
//! max_agent_depth       = 4
//! max_tokens_in         = 200000
//! max_tokens_out        = 16384
//! max_cost_usd          = 1.50
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Error, PartialEq)]
pub enum BudgetConfigError {
    #[error("max_cost_usd must be finite and non-negative, got {0}")]
    InvalidMaxCostUsd(f64),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetConfig {
    pub max_wall_time_secs: Option<u64>,
    pub max_tool_runtime_secs: Option<u64>,
    /// Maximum concurrent tool calls that may spawn native processes. This is
    /// not a descendant-PID limit inside one admitted shell command.
    #[serde(rename = "max_concurrent_process_tools", alias = "max_processes")]
    pub max_processes: Option<usize>,
    pub max_agent_depth: Option<usize>,
    pub max_tokens_in: Option<u64>,
    pub max_tokens_out: Option<u64>,
    pub max_cost_usd: Option<f64>,
}

impl BudgetConfig {
    pub fn validate(&self) -> Result<(), BudgetConfigError> {
        if let Some(usd) = self.max_cost_usd
            && (!usd.is_finite() || usd < 0.0)
        {
            return Err(BudgetConfigError::InvalidMaxCostUsd(usd));
        }
        Ok(())
    }

    /// Finite defaults for an ordinary interactive Smart session. These are
    /// deliberately generous enough for long builds while still bounding a
    /// lost unattended loop. Explicit configuration replaces each field.
    pub fn smart_default() -> Self {
        Self {
            max_wall_time_secs: Some(8 * 60 * 60),
            max_tool_runtime_secs: Some(4 * 60 * 60),
            max_processes: Some(32),
            max_agent_depth: Some(8),
            max_tokens_in: Some(10_000_000),
            max_tokens_out: Some(1_000_000),
            max_cost_usd: Some(25.0),
        }
    }

    /// Fill omitted fields from Smart Default without overriding an explicit
    /// operator value. This is applied at runtime bootstrap so the serialized
    /// configuration remains backwards-compatible and auditable.
    pub fn with_smart_defaults(&self) -> Self {
        let defaults = Self::smart_default();
        Self {
            max_wall_time_secs: self.max_wall_time_secs.or(defaults.max_wall_time_secs),
            max_tool_runtime_secs: self
                .max_tool_runtime_secs
                .or(defaults.max_tool_runtime_secs),
            max_processes: self.max_processes.or(defaults.max_processes),
            max_agent_depth: self.max_agent_depth.or(defaults.max_agent_depth),
            max_tokens_in: self.max_tokens_in.or(defaults.max_tokens_in),
            max_tokens_out: self.max_tokens_out.or(defaults.max_tokens_out),
            max_cost_usd: self.max_cost_usd.or(defaults.max_cost_usd),
        }
    }

    /// Resolve one session envelope from `[budget]` plus the legacy optional
    /// `[session_cap]` block. Explicit values replace Smart
    /// defaults; when both blocks explicitly constrain the same axis, the
    /// stricter value wins so adding a second policy source cannot widen the
    /// first one by accident.
    pub fn effective_session_envelope(budget: &Self, session_cap: Option<&Self>) -> Self {
        let mut effective = budget.with_smart_defaults();
        let Some(session_cap) = session_cap else {
            return effective;
        };
        effective.max_wall_time_secs =
            strictest_u64(budget.max_wall_time_secs, session_cap.max_wall_time_secs)
                .or(effective.max_wall_time_secs);
        effective.max_tool_runtime_secs = strictest_u64(
            budget.max_tool_runtime_secs,
            session_cap.max_tool_runtime_secs,
        )
        .or(effective.max_tool_runtime_secs);
        effective.max_processes = strictest_usize(budget.max_processes, session_cap.max_processes)
            .or(effective.max_processes);
        effective.max_agent_depth =
            strictest_usize(budget.max_agent_depth, session_cap.max_agent_depth)
                .or(effective.max_agent_depth);
        effective.max_tokens_in = strictest_u64(budget.max_tokens_in, session_cap.max_tokens_in)
            .or(effective.max_tokens_in);
        effective.max_tokens_out = strictest_u64(budget.max_tokens_out, session_cap.max_tokens_out)
            .or(effective.max_tokens_out);
        effective.max_cost_usd =
            strictest_f64(budget.max_cost_usd, session_cap.max_cost_usd).or(effective.max_cost_usd);
        effective
    }
}

fn strictest_usize(left: Option<usize>, right: Option<usize>) -> Option<usize> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn strictest_u64(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn strictest_f64(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
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
        assert!(s.contains("max_concurrent_process_tools = 4"));
        assert!(!s.contains("max_processes"));
        let parsed: BudgetConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn legacy_max_processes_alias_remains_readable() {
        let parsed: BudgetConfig = toml::from_str("max_processes = 7").unwrap();
        assert_eq!(parsed.max_processes, Some(7));
    }

    #[test]
    fn smart_defaults_are_finite_and_explicit_values_win() {
        let effective = BudgetConfig {
            max_cost_usd: Some(5.0),
            ..Default::default()
        }
        .with_smart_defaults();

        assert_eq!(effective.max_cost_usd, Some(5.0));
        assert!(effective.max_wall_time_secs.is_some());
        assert!(effective.max_tool_runtime_secs.is_some());
        assert!(effective.max_processes.is_some());
        assert!(effective.max_agent_depth.is_some());
        assert!(effective.max_tokens_in.is_some());
        assert!(effective.max_tokens_out.is_some());
    }

    #[test]
    fn rejects_non_finite_and_negative_cost_caps() {
        for usd in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.01] {
            let config = BudgetConfig {
                max_cost_usd: Some(usd),
                ..Default::default()
            };
            assert!(config.validate().is_err(), "accepted {usd}");
        }
        assert!(
            BudgetConfig {
                max_cost_usd: Some(0.0),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn session_envelope_preserves_disjoint_explicit_caps() {
        let budget = BudgetConfig {
            max_cost_usd: Some(1.0),
            max_wall_time_secs: Some(600),
            ..Default::default()
        };
        let session_cap = BudgetConfig {
            max_tokens_in: Some(1_000),
            max_processes: Some(2),
            ..Default::default()
        };

        let effective = BudgetConfig::effective_session_envelope(&budget, Some(&session_cap));

        assert_eq!(effective.max_cost_usd, Some(1.0));
        assert_eq!(effective.max_tokens_in, Some(1_000));
        assert_eq!(effective.max_tokens_out, Some(1_000_000));
        assert_eq!(effective.max_wall_time_secs, Some(600));
        assert_eq!(effective.max_processes, Some(2));
    }

    #[test]
    fn session_envelope_uses_stricter_duplicate_explicit_cap() {
        let budget = BudgetConfig {
            max_tokens_out: Some(500),
            max_cost_usd: Some(2.0),
            max_tool_runtime_secs: Some(300),
            max_agent_depth: Some(3),
            ..Default::default()
        };
        let session_cap = BudgetConfig {
            max_tokens_out: Some(200),
            max_cost_usd: Some(3.0),
            max_tool_runtime_secs: Some(120),
            max_agent_depth: Some(5),
            ..Default::default()
        };

        let effective = BudgetConfig::effective_session_envelope(&budget, Some(&session_cap));

        assert_eq!(effective.max_tokens_out, Some(200));
        assert_eq!(effective.max_cost_usd, Some(2.0));
        assert_eq!(effective.max_tool_runtime_secs, Some(120));
        assert_eq!(effective.max_agent_depth, Some(3));
    }
}
