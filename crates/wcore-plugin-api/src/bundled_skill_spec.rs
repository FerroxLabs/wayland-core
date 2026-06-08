//! Mirror of `wcore-skills::BundledSkillDefinition` (verified at
//! `crates/wcore-skills/src/bundled/mod.rs:16`). The host adapter translates
//! by leaking owned `String`s to satisfy the `&'static str` shape of the
//! underlying registry — plugin lifetime == process lifetime, so the leak is
//! acceptable per the bundled-skill registry's own static design.

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
