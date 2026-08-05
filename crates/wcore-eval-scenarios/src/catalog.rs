//! Canonical single-session scenario inventory and selection.

use std::collections::HashSet;

use thiserror::Error;

use crate::Scenario;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CatalogError {
    #[error("scenario catalog is empty")]
    Empty,
    #[error("invalid scenario ID '{0}'")]
    InvalidId(String),
    #[error("duplicate scenario ID '{0}'")]
    DuplicateId(String),
    #[error("unknown scenario '{0}'")]
    UnknownId(String),
    #[error("no scenarios matched filter '{0}'")]
    NoMatches(String),
    #[error("exact scenario IDs and a substring filter cannot be combined")]
    ConflictingSelectors,
}

/// Build the canonical single-session catalog in execution order.
///
/// The cheap canary remains first. The remaining module order matches the
/// existing live harness so centralization does not change paid-run behavior.
pub fn standard_scenarios() -> Result<Vec<Scenario>, CatalogError> {
    let mut scenarios = crate::personas::all();
    scenarios.extend(crate::qa::all());
    scenarios.extend(crate::coverage::all());
    scenarios.extend(crate::mcp_scenarios::all());
    scenarios.extend(crate::hook_scenarios::all());
    scenarios.extend(crate::protocol_scenarios::all());
    scenarios.extend(crate::cron_scenarios::all());
    validate(&scenarios)?;
    Ok(scenarios)
}

/// Select a deterministic catalog-order subset.
///
/// Exact IDs are deduplicated so a repeated CLI flag cannot accidentally run
/// a paid scenario twice. Repeated trials require an explicit future contract.
pub fn select_scenarios(
    mut scenarios: Vec<Scenario>,
    exact_ids: &[String],
    filter: Option<&str>,
) -> Result<Vec<Scenario>, CatalogError> {
    if !exact_ids.is_empty() && filter.is_some() {
        return Err(CatalogError::ConflictingSelectors);
    }

    if !exact_ids.is_empty() {
        let mut requested = HashSet::with_capacity(exact_ids.len());
        for id in exact_ids {
            if requested.insert(id.as_str())
                && !scenarios
                    .iter()
                    .any(|scenario| scenario.name == id.as_str())
            {
                return Err(CatalogError::UnknownId(id.clone()));
            }
        }
        scenarios.retain(|scenario| requested.contains(scenario.name));
    } else if let Some(filter) = filter {
        scenarios.retain(|scenario| scenario.name.contains(filter));
        if scenarios.is_empty() {
            return Err(CatalogError::NoMatches(filter.to_string()));
        }
    }

    Ok(scenarios)
}

fn validate(scenarios: &[Scenario]) -> Result<(), CatalogError> {
    if scenarios.is_empty() {
        return Err(CatalogError::Empty);
    }

    let mut ids = HashSet::with_capacity(scenarios.len());
    for scenario in scenarios {
        if !valid_id(scenario.name) {
            return Err(CatalogError::InvalidId(scenario.name.to_string()));
        }
        if !ids.insert(scenario.name) {
            return Err(CatalogError::DuplicateId(scenario.name.to_string()));
        }
    }
    Ok(())
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Category;

    #[test]
    fn standard_catalog_is_valid_and_canary_first() {
        let scenarios = standard_scenarios().expect("valid standard catalog");
        assert_eq!(scenarios.len(), 36);
        assert_eq!(scenarios[0].name, "canary");
    }

    #[test]
    fn repeated_exact_ids_select_once_in_catalog_order() {
        let scenarios = standard_scenarios().unwrap();
        let ids = vec![
            "qa_slash_clear".to_string(),
            "canary".to_string(),
            "qa_slash_clear".to_string(),
        ];
        let selected = select_scenarios(scenarios, &ids, None).unwrap();
        let names: Vec<&str> = selected.iter().map(|scenario| scenario.name).collect();
        assert_eq!(names, ["canary", "qa_slash_clear"]);
    }

    #[test]
    fn validation_rejects_invalid_and_duplicate_ids() {
        let invalid = vec![Scenario::new("Bad-ID", Category::Coverage)];
        assert_eq!(
            validate(&invalid),
            Err(CatalogError::InvalidId("Bad-ID".to_string()))
        );

        let duplicate = vec![
            Scenario::new("same", Category::Coverage),
            Scenario::new("same", Category::Coverage),
        ];
        assert_eq!(
            validate(&duplicate),
            Err(CatalogError::DuplicateId("same".to_string()))
        );
    }
}
