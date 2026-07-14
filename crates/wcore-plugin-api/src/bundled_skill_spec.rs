//! Owned plugin wire mirror of the host's bundled skill fields. The host moves
//! these strings into its session-local catalog without depending on this crate
//! from `wcore-skills`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct BundledSkillSpec {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub argument_hint: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub disable_model_invocation: bool,
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    pub context: Option<String>,
    pub agent: Option<String>,
    /// `(relative_path, content)` pairs — host adapter extracts to disk.
    #[serde(default)]
    pub files: Vec<(String, String)>,
    pub content: String,
}

fn default_true() -> bool {
    true
}
