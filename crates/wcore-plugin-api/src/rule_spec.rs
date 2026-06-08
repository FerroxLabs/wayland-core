//! System-prompt-fragment registered by a plugin.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct RuleSpec {
    pub name: String,
    pub content: String,
    pub scope: RuleScope,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleScope {
    Universal,
    ProjectScoped,
}
