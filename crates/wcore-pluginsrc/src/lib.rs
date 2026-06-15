//! Install-time acquisition and lowering of foreign plugin formats into the
//! Wayland-native plugin model. Foreign-format knowledge lives ONLY here; the
//! runtime loader (wcore-agent) stays format-blind.
//!
//! Pipeline: `ForeignSource → (adapter) parse+lower → CanonicalDraft →
//! InstallPlan → (consent) → commit into the Wayland plugin store`.

pub mod adapter;
pub mod claude_code;
pub mod commit;
pub mod error;
pub mod mcp_registry;
pub mod model;
pub mod plan;

pub use adapter::{PluginFormatAdapter, detect_format};
pub use error::{PluginSrcError, Result};
// Re-exported so consumers can name/match MCP transports without depending on
// wcore-plugin-api directly.
pub use commit::{CommitMeta, Provenance, commit_plan};
pub use model::{
    AgentAsset, CanonicalDraft, CommandAsset, CompatibilityGrade, IgnoredFeature, McpServerDraft,
    ResolvedVersion, SkillAsset, SourceEntry, SourceKind,
};
pub use plan::{AddedComponent, Collision, InstallPlan, McpSpawnPreview};
pub use wcore_plugin_api::mcp_server_spec::{McpServerSpec, McpTransport};
