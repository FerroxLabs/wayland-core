//! The format-adapter seam. An adapter parses a foreign plugin laid out on
//! disk and lowers it into a [`CanonicalDraft`]. Only adapters know foreign
//! formats; everything downstream is format-blind.

use std::path::Path;

use crate::Result;
use crate::model::{CanonicalDraft, SourceEntry};

pub trait PluginFormatAdapter: Send + Sync {
    /// Stable adapter id, e.g. `"claude-code"`, `"mcp-registry"`.
    fn id(&self) -> &'static str;
    /// Sniff whether this adapter recognizes the layout rooted at `root`.
    fn detect(&self, root: &Path) -> bool;
    /// Lower a plugin (already fetched into the quarantine/cache at `root`),
    /// listed under `marketplace` as `entry`, into a Wayland-native draft.
    fn lower(&self, marketplace: &str, entry: &SourceEntry, root: &Path) -> Result<CanonicalDraft>;
}

/// First format whose marker matches, by priority. Returns the adapter id.
///
/// An explicit vendor manifest wins over the loose `skills/` + `.mcp.json`
/// heuristic, and `.codex-plugin/plugin.json` is checked before
/// `.claude-plugin/plugin.json` — the same order Codex itself uses in
/// `DISCOVERABLE_PLUGIN_MANIFEST_PATHS`, so a plugin shipping both manifests
/// resolves to the one its own vendor would pick.
pub fn detect_format(root: &Path) -> Option<String> {
    if root.join(".codex-plugin/plugin.json").exists() {
        return Some("codex".to_string());
    }
    if root.join(".claude-plugin/plugin.json").exists()
        || (root.join("skills").is_dir() && root.join(".mcp.json").exists())
    {
        return Some("claude-code".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_claude_code_by_marker_dir() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".claude-plugin")).unwrap();
        fs::write(
            d.path().join(".claude-plugin/plugin.json"),
            r#"{"name":"x"}"#,
        )
        .unwrap();
        assert_eq!(detect_format(d.path()).as_deref(), Some("claude-code"));
    }

    #[test]
    fn detects_codex_by_marker_dir() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".codex-plugin")).unwrap();
        fs::write(
            d.path().join(".codex-plugin/plugin.json"),
            r#"{"name":"x"}"#,
        )
        .unwrap();
        assert_eq!(detect_format(d.path()).as_deref(), Some("codex"));
    }

    #[test]
    fn codex_marker_wins_when_both_manifests_are_present() {
        // Deterministic, and matches Codex's own discovery order. Without this
        // the winner would depend on the order of the `if` arms.
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".codex-plugin")).unwrap();
        fs::create_dir_all(d.path().join(".claude-plugin")).unwrap();
        fs::write(
            d.path().join(".codex-plugin/plugin.json"),
            r#"{"name":"x"}"#,
        )
        .unwrap();
        fs::write(
            d.path().join(".claude-plugin/plugin.json"),
            r#"{"name":"x"}"#,
        )
        .unwrap();
        assert_eq!(detect_format(d.path()).as_deref(), Some("codex"));
    }

    #[test]
    fn loose_marker_still_detects_claude_code() {
        // Polarity control for the arm reordering above: the pre-existing
        // heuristic path must survive.
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("skills")).unwrap();
        fs::write(d.path().join(".mcp.json"), "{}").unwrap();
        assert_eq!(detect_format(d.path()).as_deref(), Some("claude-code"));
    }

    #[test]
    fn unknown_when_no_markers() {
        let d = tempdir().unwrap();
        assert!(detect_format(d.path()).is_none());
    }
}
