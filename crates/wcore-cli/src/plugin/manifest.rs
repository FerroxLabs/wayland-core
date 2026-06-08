// M5.4: plugin manifest schema. Same shape for local TOML manifests and
// the JSON-on-disk install records — keeps the install/list round-trip
// trivial (one type, two serializers). Extending this struct requires
// `#[serde(default)]` on the new field so old manifests still parse.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,

    /// Marker for plugins that need the sandboxed-tools surface. Default
    /// is `false`; manifests that omit it parse fine.
    #[serde(default)]
    pub requires_sandbox: bool,

    #[serde(default)]
    pub description: String,

    /// Other plugin names this depends on. Empty by default. Resolved at
    /// install time in a future wave; currently informational only.
    #[serde(default)]
    pub dependencies: Vec<String>,
}
