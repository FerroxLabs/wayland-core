//! Learned preferences — per-domain expertise + free-form tags.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[derive(Default)]
pub enum ExpertiseLevel {
    Novice,
    #[default]
    Intermediate,
    Expert,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Per-domain expertise (e.g. `"rust"`, `"react"`, `"copywriting"`).
    #[serde(default)]
    pub expertise: BTreeMap<String, ExpertiseLevel>,
    /// Free-form tags the model can read at render time
    /// (e.g. `"prefers_inline_code"`, `"no_emoji"`).
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
}
