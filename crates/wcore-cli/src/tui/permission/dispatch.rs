//! The permission dispatcher (v0.9.2 W2 keystone). A pure function that
//! routes a tool name to its bespoke `PermissionComponent`, or to the
//! `FallbackComponent` for anything without one (MCP tools, plugin tools,
//! future tools).
//!
//! FROZEN SIGNATURE: `permission_component_for(tool_name: &str) ->
//! Box<dyn PermissionComponent>`. W3/W4 register their components by
//! UNCOMMENTING the relevant arm below (and adding the matching
//! `pub mod <name>;` to `components/mod.rs` + `use` line here). The arm
//! list mirrors SPEC §1C verbatim so the structure is locked from W2.

use super::PermissionComponent;
use super::components::fallback::FallbackComponent;

// W3 components.
use super::components::bash::BashComponent;
use super::components::fileedit::FileEditComponent;
use super::components::filesystem::FilesystemComponent;
use super::components::filewrite::FileWriteComponent;
use super::components::powershell::PowerShellComponent;
// W4 components.
use super::components::ask_user::AskUserQuestionComponent;
use super::components::crucible::CrucibleComponent;
use super::components::enter_plan::EnterPlanModeComponent;
use super::components::exit_plan::ExitPlanModeComponent;
use super::components::notebook::NotebookEditComponent;
use super::components::skill::SkillComponent;
use super::components::webfetch::WebFetchComponent;
// W4 feature-gated components.
#[cfg(feature = "monitor")]
use super::components::monitor::MonitorComponent;
#[cfg(feature = "review_artifact")]
use super::components::review_artifact::ReviewArtifactComponent;
#[cfg(feature = "workflow")]
use super::components::workflow::WorkflowComponent;

/// Route a tool name to its bespoke component, or `FallbackComponent` for
/// anything without one (MCP tools, plugin tools, future tools).
/// Feature-gated arms (Workflow/Monitor/ReviewArtifact) fall to Fallback
/// when their feature is off.
pub fn permission_component_for(tool_name: &str) -> Box<dyn PermissionComponent> {
    match tool_name {
        // The arm list is the SPEC §1C dispatcher block. Each arm routes a
        // tool name to its bespoke component; anything without an arm
        // degrades to Fallback (still a clean card). The dispatcher
        // SIGNATURE is frozen here. Components currently SCAFFOLD stubs —
        // the W3/W4 component agents flesh out each `impl`.
        "Bash" => Box::new(BashComponent),                  // W3
        "PowerShell" => Box::new(PowerShellComponent),      // W3
        "Edit" | "FileEdit" => Box::new(FileEditComponent), // W3
        "Write" | "FileWrite" => Box::new(FileWriteComponent), // W3
        "Read" | "Glob" | "Grep" => Box::new(FilesystemComponent), // W3
        "WebFetch" => Box::new(WebFetchComponent),          // W4
        "NotebookEdit" => Box::new(NotebookEditComponent),  // W4
        "EnterPlanMode" => Box::new(EnterPlanModeComponent), // W4
        "ExitPlanMode" => Box::new(ExitPlanModeComponent),  // W4
        "Skill" => Box::new(SkillComponent),                // W4
        "AskUserQuestion" => Box::new(AskUserQuestionComponent), // W4
        "Crucible" => Box::new(CrucibleComponent),          // Stage 4a
        #[cfg(feature = "workflow")]
        "Workflow" => Box::new(WorkflowComponent), // W4
        #[cfg(feature = "monitor")]
        "Monitor" => Box::new(MonitorComponent), // W4
        #[cfg(feature = "review_artifact")]
        "ReviewArtifact" => Box::new(ReviewArtifactComponent), // W4
        _ => Box::new(FallbackComponent),                   // THE keystone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_mcp_tool_routes_to_fallback() {
        let c = permission_component_for("mcp__foo__bar");
        // Fallback's icon is the generic blocked glyph.
        assert_eq!(c.icon(), "⊘");
    }

    #[test]
    fn arbitrary_unknown_tool_routes_to_fallback() {
        // Names that will NEVER have a bespoke arm degrade to a clean card
        // — the property that makes the 15-component matrix tractable.
        // (Deliberately excludes "Bash"/"Edit"/etc. so W3/W4 wiring this
        // test does not need editing when those arms go live.)
        for tool in ["SomeBrandNewTool", "mcp__github__create_issue", ""] {
            assert_eq!(permission_component_for(tool).icon(), "⊘");
        }
    }

    #[test]
    fn known_tools_route_to_bespoke_components_not_fallback() {
        // Every registered arm must NOT land on Fallback. The stub
        // components carry the `•` placeholder icon (W3/W4 replace the
        // glyph when they flesh out each `impl`); the load-bearing
        // assertion is that the route is bespoke (not the `⊘` keystone).
        let bespoke = [
            "Bash",
            "PowerShell",
            "Edit",
            "FileEdit",
            "Write",
            "FileWrite",
            "Read",
            "Glob",
            "Grep",
            "WebFetch",
            "NotebookEdit",
            "EnterPlanMode",
            "ExitPlanMode",
            "Skill",
            "AskUserQuestion",
        ];
        for tool in bespoke {
            assert_ne!(
                permission_component_for(tool).icon(),
                "⊘",
                "{tool} should route to a bespoke component, not Fallback"
            );
        }
    }

    #[cfg(feature = "workflow")]
    #[test]
    fn workflow_tool_routes_to_bespoke_when_feature_on() {
        assert_ne!(permission_component_for("Workflow").icon(), "⊘");
    }

    #[cfg(feature = "monitor")]
    #[test]
    fn monitor_tool_routes_to_bespoke_when_feature_on() {
        assert_ne!(permission_component_for("Monitor").icon(), "⊘");
    }

    #[cfg(feature = "review_artifact")]
    #[test]
    fn review_artifact_tool_routes_to_bespoke_when_feature_on() {
        assert_ne!(permission_component_for("ReviewArtifact").icon(), "⊘");
    }

    // Rank-90 fix: `monitor` and `review_artifact` are now in the crate's
    // DEFAULT feature set (see Cargo.toml), so a normal build must route the
    // `Monitor` / `ReviewArtifact` tools to their tool-specific component
    // rather than degrading to the generic FallbackComponent. These tests
    // are NOT feature-gated — they would fail to compile (and so flag the
    // regression) if the features were dropped from `default`. The assertion
    // pins the exact bespoke icon (`◉` / `▤`), which is strictly stronger
    // than "not the `⊘` fallback glyph": the MonitorComponent's filled circle
    // and ReviewArtifactComponent's document glyph are unique to those
    // components, so matching them proves the request reached the correct
    // tool-specific component, not merely some non-fallback one.
    #[test]
    fn monitor_routes_to_its_component_with_default_features() {
        let c = permission_component_for("Monitor");
        assert_ne!(c.icon(), "⊘", "Monitor must not degrade to Fallback");
        assert_eq!(
            c.icon(),
            "◉",
            "Monitor must route to MonitorComponent (filled-circle icon)"
        );
    }

    #[test]
    fn review_artifact_routes_to_its_component_with_default_features() {
        let c = permission_component_for("ReviewArtifact");
        assert_ne!(c.icon(), "⊘", "ReviewArtifact must not degrade to Fallback");
        assert_eq!(
            c.icon(),
            "▤",
            "ReviewArtifact must route to ReviewArtifactComponent (document icon)"
        );
    }
}
