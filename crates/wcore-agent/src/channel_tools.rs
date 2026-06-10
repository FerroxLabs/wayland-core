//! Channel tool posture enforcement.
//!
//! A channel-originated agent turn runs a real [`AgentEngine`] on the host.
//! The sender is REMOTE, so that engine must NOT inherit the local CLI's
//! full host access — otherwise an (allowlisted-but-untrusted, or
//! compromised) chat user could `Read`/`Grep` host secrets and have the
//! reply ship them back. This module maps a per-channel
//! [`ChannelToolPosture`] onto a concrete, reduced toolset:
//!
//! - **Conversational** (default): drop every built-in host filesystem /
//!   shell tool. Keep only the fail-closed [`CONVERSATIONAL_SAFE`] allowlist
//!   (conversational + network tools) plus operator-wired MCP tools.
//! - **Workspace**: as Conversational, but add the vfs-jailable filesystem
//!   tools ([`WORKSPACE_FS_TOOLS`]) back and pin a [`SandboxedFs`] jail on
//!   the registry so they cannot escape the configured workspace root.
//!   Shell/exec tools stay dropped — they bypass the jail.
//! - **Full**: no filtering, no jail — identical to a local CLI session.
//!
//! Enforcement is at the [`ToolRegistry`] (not just the LLM schema): a
//! dropped tool is un-dispatchable, so even a hallucinated call cannot
//! reach it.
//!
//! [`AgentEngine`]: crate::engine::AgentEngine
//! [`SandboxedFs`]: wcore_tools::vfs::SandboxedFs

use std::path::PathBuf;
use std::sync::Arc;

use wcore_channels::ChannelToolPosture;
use wcore_protocol::events::ToolCategory;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::Tool;

/// Resolved tool posture for one channel: the posture plus the concrete
/// workspace root the `Workspace` jail confines filesystem tools to.
#[derive(Debug, Clone)]
pub struct ChannelToolScope {
    pub posture: ChannelToolPosture,
    pub workspace_root: PathBuf,
}

/// Built-in tools provably free of host filesystem / shell access — safe to
/// expose to a remote channel sender in `Conversational` posture.
///
/// **Fail-closed allowlist.** A tool NOT named here (and not an
/// operator-wired MCP tool) is DROPPED. A newly-added host-touching built-in
/// therefore can never silently leak to channels: it stays dropped until
/// someone deliberately adds it here. (Network tools `web`/`WebFetch` reach
/// the network, not the host fs; SSRF is gated separately by the egress
/// policy.)
const CONVERSATIONAL_SAFE: &[&str] = &[
    "send_message",
    "todo",
    "clarify",
    "AskUserQuestion",
    "markdown_table",
    "web",
    "WebFetch",
    "ToolSearch",
];

/// Filesystem tools added back in `Workspace` posture. Every one routes its
/// reads/writes through `ctx.vfs`, so a
/// [`SandboxedFs`](wcore_tools::vfs::SandboxedFs) jail genuinely confines it.
///
/// `Grep`/`Glob` are deliberately EXCLUDED: they only probe the top-level
/// path argument through `ctx.vfs` and then shell out (`rg`/`grep`) or walk
/// the glob crate against the real filesystem at the process cwd — the
/// recursive scan is NOT confined by the jail, so a `Grep`/`Glob` with the
/// default `path="."` (or a symlink inside the workspace) would escape it.
/// Until their subprocess cwd is pinned to the jail root and symlink
/// following is disabled, they stay unavailable in `Workspace` (and `Full`
/// remains the escape hatch for unconfined search). Likewise Bash, Git,
/// RepoMap, pdf_extract, kubectl, gcloud, aws_cli, sql_query, Script touch
/// the host fs/shell outside `ctx.vfs` and stay unavailable.
const WORKSPACE_FS_TOOLS: &[&str] = &["Read", "Write", "Edit"];

/// Operator-wired MCP tools are kept under restricted postures: they are
/// deliberate, named extensions the operator installed, not ambient host
/// access. (Caveat: an MCP server that itself exposes host filesystem
/// access should be threat-modeled as `Full`-equivalent for that channel.)
fn is_mcp(t: &dyn Tool) -> bool {
    matches!(t.category(), ToolCategory::Mcp)
}

/// Whether `tool` survives under `posture`.
fn keep_under(posture: ChannelToolPosture, tool: &dyn Tool) -> bool {
    match posture {
        ChannelToolPosture::Full => true,
        ChannelToolPosture::Conversational => {
            CONVERSATIONAL_SAFE.contains(&tool.name()) || is_mcp(tool)
        }
        ChannelToolPosture::Workspace => {
            CONVERSATIONAL_SAFE.contains(&tool.name())
                || WORKSPACE_FS_TOOLS.contains(&tool.name())
                || is_mcp(tool)
        }
    }
}

/// Apply a channel tool scope to a freshly-built registry: drop the tools
/// the posture forbids and, for `Workspace`, install the `SandboxedFs` jail
/// so the surviving filesystem tools cannot escape `scope.workspace_root`.
///
/// A no-op for [`ChannelToolPosture::Full`] (and never called for a local
/// CLI engine, which has no scope). Must run AFTER the full toolset —
/// including MCP tools — is registered, and BEFORE the registry is moved
/// into the engine.
pub fn apply_posture(registry: &mut ToolRegistry, scope: &ChannelToolScope) {
    if scope.posture != ChannelToolPosture::Full {
        let posture = scope.posture;
        registry.retain(|t| keep_under(posture, t));
    }
    if scope.posture == ChannelToolPosture::Workspace {
        let jail =
            wcore_tools::vfs::SandboxedFs::new(wcore_tools::vfs::RealFs, scope.workspace_root.clone());
        registry.set_tool_vfs(Arc::new(jail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use wcore_types::tool::ToolResult;

    struct FakeTool {
        name: String,
        category: ToolCategory,
    }

    #[async_trait]
    impl Tool for FakeTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }
        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".into(),
                is_error: false,
            }
        }
        fn category(&self) -> ToolCategory {
            self.category
        }
    }

    fn tool(name: &str, category: ToolCategory) -> FakeTool {
        FakeTool {
            name: name.into(),
            category,
        }
    }

    /// The real built-in roster (name, category) as registered in
    /// `bootstrap.rs`. Drives the fail-closed enforcement assertions below.
    /// `Info` here is the broad "read/info" bucket the real tools use; the
    /// allowlist — not the category — is what protects against the
    /// Info-category host-fs readers (Read/Grep/Glob/RepoMap/pdf_extract).
    fn builtin_roster() -> Vec<FakeTool> {
        use ToolCategory::*;
        [
            // Host filesystem / shell / exec — MUST be dropped in conversational.
            ("Read", Info),
            ("Write", Edit),
            ("Edit", Edit),
            ("Grep", Info),
            ("Glob", Info),
            ("Bash", Exec),
            ("Git", Info),
            ("RepoMap", Info),
            ("pdf_extract", Info),
            ("Archive", Info),
            ("image_inspect", Info),
            ("email_parse", Info),
            ("Jsonl", Info),
            ("Script", Exec),
            ("kubectl", Exec),
            ("gcloud", Exec),
            ("aws_cli", Exec),
            ("sql_query", Info),
            ("postgres_schema", Info),
            ("session_search", Info),
            ("cronjob", Info),
            ("Delegate", Info),
            // Conversational / network — safe to keep.
            ("send_message", Info),
            ("todo", Info),
            ("clarify", Info),
            ("AskUserQuestion", Info),
            ("markdown_table", Info),
            ("web", Info),
            ("WebFetch", Info),
            ("ToolSearch", Info),
            // Operator-wired MCP — kept under restricted postures.
            ("some_mcp_tool", Mcp),
        ]
        .into_iter()
        .map(|(n, c)| tool(n, c))
        .collect()
    }

    /// Tools that must NEVER survive `Conversational` (host fs/shell/exec).
    const HOST_TOOLS: &[&str] = &[
        "Read",
        "Write",
        "Edit",
        "Grep",
        "Glob",
        "Bash",
        "Git",
        "RepoMap",
        "pdf_extract",
        "Archive",
        "image_inspect",
        "email_parse",
        "Jsonl",
        "Script",
        "kubectl",
        "gcloud",
        "aws_cli",
        "sql_query",
        "postgres_schema",
        "session_search",
        "cronjob",
        "Delegate",
    ];

    #[test]
    fn conversational_drops_every_host_tool() {
        for t in builtin_roster() {
            if HOST_TOOLS.contains(&t.name()) {
                assert!(
                    !keep_under(ChannelToolPosture::Conversational, &t),
                    "host tool '{}' must be dropped in conversational posture",
                    t.name()
                );
            }
        }
    }

    #[test]
    fn conversational_keeps_safe_and_mcp_tools() {
        for t in builtin_roster() {
            if CONVERSATIONAL_SAFE.contains(&t.name()) || matches!(t.category(), ToolCategory::Mcp) {
                assert!(
                    keep_under(ChannelToolPosture::Conversational, &t),
                    "safe/mcp tool '{}' must survive conversational posture",
                    t.name()
                );
            }
        }
    }

    #[test]
    fn workspace_adds_back_only_vfs_jailable_fs_tools() {
        // The five vfs-jailable fs tools come back…
        for name in WORKSPACE_FS_TOOLS {
            assert!(
                keep_under(ChannelToolPosture::Workspace, &tool(name, ToolCategory::Info)),
                "workspace must expose vfs-jailable fs tool '{name}'"
            );
        }
        // …but shell/exec, non-vfs fs readers, AND the shell-out search
        // tools (Grep/Glob — not confined by the vfs jail) stay dropped.
        for name in [
            "Bash", "Git", "RepoMap", "pdf_extract", "kubectl", "Script", "Grep", "Glob",
        ] {
            assert!(
                !keep_under(ChannelToolPosture::Workspace, &tool(name, ToolCategory::Info)),
                "workspace must NOT expose host-escaping tool '{name}'"
            );
        }
    }

    #[test]
    fn full_keeps_everything() {
        for t in builtin_roster() {
            assert!(
                keep_under(ChannelToolPosture::Full, &t),
                "full posture keeps every tool, including '{}'",
                t.name()
            );
        }
    }

    #[test]
    fn apply_posture_workspace_installs_jail() {
        let mut reg = ToolRegistry::new();
        let scope = ChannelToolScope {
            posture: ChannelToolPosture::Workspace,
            workspace_root: PathBuf::from("/tmp"),
        };
        apply_posture(&mut reg, &scope);
        assert!(reg.tool_vfs().is_some(), "workspace posture pins a SandboxedFs jail");
    }

    #[test]
    fn apply_posture_conversational_no_jail() {
        let mut reg = ToolRegistry::new();
        let scope = ChannelToolScope {
            posture: ChannelToolPosture::Conversational,
            workspace_root: PathBuf::from("/tmp"),
        };
        apply_posture(&mut reg, &scope);
        assert!(
            reg.tool_vfs().is_none(),
            "conversational posture installs no vfs (fs tools are dropped, not jailed)"
        );
    }
}
