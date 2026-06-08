//! Agent YAML schema mirror (F2-compatible). Owned here so
//! `ScopedAgentRegistry` does not depend on `wcore-agent`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct AgentManifest {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub system_prompt: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
}
