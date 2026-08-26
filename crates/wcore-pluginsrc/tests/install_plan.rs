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

#[test]
fn servers_the_commit_step_will_not_install_are_named_on_the_plan() {
    // v1 writes one `[mcp_server]` and grants one spawn-consent key. A plugin
    // declaring more must say which ones do not survive, or the plan reads as
    // parity it does not have.
    let mut d = draft();
    d.mcp_servers.push(McpServerDraft {
        name: "second".into(),
        transport: McpTransport::Http {
            url: "https://x/mcp".into(),
        },
        env: BTreeMap::new(),
    });
    d.mcp_servers.push(McpServerDraft {
        name: "third".into(),
        transport: McpTransport::Sse {
            url: "https://y/mcp".into(),
        },
        env: BTreeMap::new(),
    });

    let plan = InstallPlan::from_draft(&d, "acme", "/store/x");
    let extra = plan
        .ignored
        .iter()
        .find(|i| i.kind == "mcp-extra-servers")
        .expect("extra servers must be reported");
    assert!(extra.detail.contains("second"), "{}", extra.detail);
    assert!(extra.detail.contains("third"), "{}", extra.detail);
    assert!(extra.detail.contains("database"), "{}", extra.detail);
    assert!(plan.render().contains("mcp-extra-servers"));

    // Off-by-one guard: the server that IS installed must be named as the
    // survivor, never inside the not-installed list. An `[0..]` slice still
    // satisfies every assertion above while telling the user their working
    // server was dropped.
    let (_, not_installed) = extra
        .detail
        .split_once("not installed: ")
        .expect("the dropped servers must be a named list");
    assert!(
        !not_installed.contains("database"),
        "the installed server must not be listed as dropped: {}",
        extra.detail
    );

    // The other half of the same fact: the plan previews only the server the
    // commit step actually grants a spawn-consent key for. Listing all three
    // under "will be allowed to spawn" is the same parity lie in reverse, and
    // it is what the TUI consent surface renders.
    assert_eq!(plan.spawns.len(), 1, "{:?}", plan.spawns);
    assert_eq!(plan.spawns[0].name, "database");
    let rendered = plan.render();
    let spawn_block = rendered
        .split("will be allowed to spawn:")
        .nth(1)
        .and_then(|t| t.split("  ignores").next())
        .expect("the spawn block must be rendered");
    assert!(!spawn_block.contains("second"), "{spawn_block}");
    assert!(!spawn_block.contains("third"), "{spawn_block}");
}

#[test]
fn a_single_server_plan_reports_no_extra_servers() {
    // Polarity control: the report fires on real loss, not on every install.
    let plan = InstallPlan::from_draft(&draft(), "acme", "/store/x");
    assert!(!plan.ignored.iter().any(|i| i.kind == "mcp-extra-servers"));
}
