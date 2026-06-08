//! Observation — one signal fed into the backend per turn.

use serde::{Deserialize, Serialize};

/// Outcome of one turn from the user's perspective. The backend
/// folds this into expertise / preference / style estimates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Outcome {
    Accepted,
    Rejected,
    Corrected,
    Ignored,
    Praised,
}

/// Tool / skill hint — which model+skill served the turn. Lets the
/// backend correlate outcomes with the model used so a
/// `PreferenceLearner` can score model+skill combinations.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ToolHint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Optional domain tag (`"rust"`, `"copywriting"`, …) the
    /// expertise estimator updates against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}

/// One observation per turn. `style_fingerprint` is optional because
/// not every signal source produces one (a tool-only turn might
/// skip).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub outcome: Option<Outcome>,
    #[serde(default)]
    pub hint: ToolHint,
    /// Per-turn style fingerprint produced by `StyleDetector::observe`.
    /// 4 axes 0-1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style_fingerprint: Option<[f32; 4]>,
    /// Unix epoch seconds.
    #[serde(default)]
    pub ts_secs: i64,
}

impl Observation {
    pub fn accepted() -> Self {
        Self {
            outcome: Some(Outcome::Accepted),
            ..Self::default()
        }
    }
    pub fn rejected() -> Self {
        Self {
            outcome: Some(Outcome::Rejected),
            ..Self::default()
        }
    }
}
