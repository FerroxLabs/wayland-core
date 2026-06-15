use std::collections::BTreeMap;

use wcore_pluginsrc::model::{CanonicalDraft, IgnoredFeature, McpServerDraft, SkillAsset};
use wcore_pluginsrc::{CompatibilityGrade, InstallPlan, McpTransport};

fn draft() -> CanonicalDraft {
    let mut d = CanonicalDraft::empty("acme", "db");
    d.skills.push(SkillAsset {
        name: "query".into(),
        rel_dir: "skills/query".into(),
    });
    d.mcp_servers.push(McpServerDraft {
        name: "database".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["@x/srv".into()],
        },
        env: BTreeMap::from([("API_KEY".into(), "${API_KEY}".into())]),
    });
    d.ignored.push(IgnoredFeature {
        kind: "hooks".into(),
        detail: "PostToolUse x1".into(),
    });
    d
}

#[test]
fn plan_lists_spawns_and_grades_hooks_ignored() {
    let plan = InstallPlan::from_draft(&draft(), "acme", "/store/acme/db/1");

    // A plugin that drops hooks can never grade above HooksIgnored.
    assert_eq!(plan.grade, CompatibilityGrade::HooksIgnored);

    // The MCP server is surfaced for consent, env KEYS only (no values).
    assert_eq!(plan.spawns.len(), 1);
    assert_eq!(plan.spawns[0].command, "npx");
    assert_eq!(plan.spawns[0].transport_kind, "stdio");
    assert!(plan.spawns[0].env_keys.contains(&"API_KEY".to_string()));

    // Skill is namespaced under <marketplace>/<plugin>.
    assert!(
        plan.adds
            .iter()
            .any(|a| a.kind == "skill" && a.name == "acme/db:query")
    );

    let text = plan.render();
    assert!(text.contains("will be allowed to spawn"));
    assert!(text.contains("ignores"));
    // Consent text must not leak the env VALUE.
    assert!(!text.contains("${API_KEY}"));
}

#[test]
fn dry_run_plan_is_pure_no_store_written() {
    // store_path points at a path that does not exist; from_draft must not
    // create it (the plan is pure — commit happens elsewhere).
    let plan = InstallPlan::from_draft(&draft(), "acme", "/nonexistent/store/x");
    assert!(!std::path::Path::new("/nonexistent/store/x").exists());
    assert_eq!(plan.plugin, "db");
}
