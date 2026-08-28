//! Codex marketplace ingestion, end to end: catalog dialect selection, source
//! normalization, traversal rejection, the lossy-policy report reaching the
//! consent surface, and MCP spawn consent surviving a Codex-declared server.

use std::path::Path;

use wcore_cli::plugin::codex_marketplace::parse_codex_marketplace;
use wcore_cli::plugin::error::PluginCliError;
use wcore_cli::plugin::known::{MarketplaceRef, add_marketplace};
use wcore_cli::plugin::marketplace::{commit_install, resolve_and_plan};
use wcore_pluginsrc::CompatibilityGrade;
use wcore_pluginsrc::model::SourceKind;

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

// ---------------------------------------------------------------------------
// Catalog parsing
// ---------------------------------------------------------------------------

const CATALOG: &str = r#"{
  "name": "openai-curated",
  "interface": { "displayName": "ChatGPT Official" },
  "plugins": [
    { "name": "linear",
      "source": { "source": "local", "path": "./plugins/linear" },
      "policy": { "installation": "INSTALLED_BY_DEFAULT", "authentication": "ON_USE",
                  "products": ["codex"] },
      "category": "Productivity",
      "version": "1.2.0",
      "description": "linear plugin" },
    { "name": "bare", "source": "./plugins/bare" },
    { "name": "remote", "source": { "source": "url", "url": "https://h/r.git", "ref": "main" } },
    { "name": "remote-sub",
      "source": { "source": "url", "url": "https://h/r.git", "path": "pkgs/a", "sha": "deadbeef" } },
    { "name": "sub",
      "source": { "source": "git-subdir", "url": "https://h/r.git", "path": "pkgs/b" } },
    { "name": "pkg",
      "source": { "source": "npm", "package": "@scope/p", "version": "1.0.0",
                  "registry": "https://registry.example" } }
  ]
}"#;

#[test]
fn parses_every_codex_source_shape() {
    let (meta, entries) = parse_codex_marketplace(CATALOG).unwrap();
    assert_eq!(meta.name, "openai-curated");
    // Codex catalogs carry no owner block and no pluginRoot.
    assert!(meta.owner_name.is_none());
    assert!(meta.plugin_root.is_none());
    assert_eq!(entries.len(), 6);

    let by = |n: &str| entries.iter().find(|e| e.name == n).unwrap();

    assert_eq!(
        by("linear").kind,
        SourceKind::RelativePath("./plugins/linear".into())
    );
    assert_eq!(by("linear").declared_version.as_deref(), Some("1.2.0"));
    assert_eq!(by("linear").description.as_deref(), Some("linear plugin"));

    assert_eq!(
        by("bare").kind,
        SourceKind::RelativePath("./plugins/bare".into())
    );

    assert_eq!(
        by("remote").kind,
        SourceKind::Url {
            url: "https://h/r.git".into(),
            git_ref: Some("main".into()),
            sha: None
        }
    );

    // Codex folds the subdir case into `url` via an optional `path`.
    assert_eq!(
        by("remote-sub").kind,
        SourceKind::GitSubdir {
            url: "https://h/r.git".into(),
            path: "pkgs/a".into(),
            git_ref: None,
            sha: Some("deadbeef".into())
        }
    );

    assert_eq!(
        by("sub").kind,
        SourceKind::GitSubdir {
            url: "https://h/r.git".into(),
            path: "pkgs/b".into(),
            git_ref: None,
            sha: None
        }
    );

    assert_eq!(
        by("pkg").kind,
        SourceKind::Npm {
            package: "@scope/p".into(),
            version: Some("1.0.0".into()),
            registry: Some("https://registry.example".into())
        }
    );
}

#[test]
fn policy_and_category_are_reported_as_unsupported_never_honored() {
    let (_, entries) = parse_codex_marketplace(CATALOG).unwrap();
    let linear = entries.iter().find(|e| e.name == "linear").unwrap();
    let detail = linear
        .unsupported
        .iter()
        .map(|i| i.detail.clone())
        .collect::<Vec<_>>()
        .join(" | ");
    for needle in [
        "policy.installation",
        "INSTALLED_BY_DEFAULT",
        "policy.authentication",
        "policy.products",
        "category",
    ] {
        assert!(detail.contains(needle), "{needle} missing from {detail}");
    }

    // Polarity control: an entry with no policy block reports nothing.
    let bare = entries.iter().find(|e| e.name == "bare").unwrap();
    assert!(bare.unsupported.is_empty(), "got {:?}", bare.unsupported);
}

#[test]
fn rejects_traversal_in_every_codex_path_shape() {
    let cases = [
        (
            "local path",
            r#"{"name":"m","plugins":[{"name":"e","source":{"source":"local","path":"../../etc"}}]}"#,
        ),
        (
            "bare string path",
            r#"{"name":"m","plugins":[{"name":"e","source":"../../etc"}]}"#,
        ),
        (
            "url subdir path",
            r#"{"name":"m","plugins":[{"name":"e","source":{"source":"url","url":"https://h/r.git","path":"../x"}}]}"#,
        ),
        (
            "git-subdir path",
            r#"{"name":"m","plugins":[{"name":"e","source":{"source":"git-subdir","url":"https://h/r.git","path":"/etc"}}]}"#,
        ),
    ];
    for (label, json) in cases {
        let err = parse_codex_marketplace(json).unwrap_err();
        assert!(
            matches!(err, PluginCliError::PathTraversal(_)),
            "{label}: expected PathTraversal, got {err:?}"
        );
    }
}

#[test]
fn clean_paths_are_accepted() {
    // Polarity control for the rejection sweep above.
    let json = r#"{"name":"m","plugins":[
        {"name":"a","source":{"source":"local","path":"./plugins/a"}},
        {"name":"b","source":{"source":"git-subdir","url":"https://h/r.git","path":"pkgs/b"}}]}"#;
    assert_eq!(parse_codex_marketplace(json).unwrap().1.len(), 2);
}

#[test]
fn structural_errors_are_typed_not_panics() {
    for json in [
        r#"[]"#,
        r#"{"plugins":[]}"#,
        r#"{"name":"m"}"#,
        r#"{"name":"m","plugins":[{"source":"./a"}]}"#,
        r#"{"name":"m","plugins":[{"name":"a"}]}"#,
        r#"{"name":"m","plugins":[{"name":"a","source":{"source":"weird"}}]}"#,
    ] {
        assert!(parse_codex_marketplace(json).is_err(), "accepted {json}");
    }
}

// ---------------------------------------------------------------------------
// End to end through the shared pipeline
// ---------------------------------------------------------------------------

/// A local Codex marketplace holding one relative-path Codex plugin that ships
/// a skill, an MCP server, hooks and an app connector.
fn build_codex_fixture(dir: &Path) {
    write(
        &dir.join(".agents/plugins/marketplace.json"),
        r#"{
          "name": "codex-local",
          "interface": { "displayName": "Local" },
          "plugins": [ { "name": "demo",
                         "source": { "source": "local", "path": "./plugins/demo" },
                         "policy": { "installation": "AVAILABLE", "authentication": "ON_INSTALL" },
                         "category": "Productivity" } ]
        }"#,
    );
    write(
        &dir.join("plugins/demo/.codex-plugin/plugin.json"),
        r#"{"name":"demo","version":"0.3.0","description":"demo codex plugin",
            "interface":{"displayName":"Demo"}}"#,
    );
    write(
        &dir.join("plugins/demo/skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: greets\n---\nSay hello.",
    );
    write(
        &dir.join("plugins/demo/.mcp.json"),
        r#"{"mcpServers":{"fetch":{"command":"uvx","args":["mcp-server-fetch"],
            "env":{"FETCH_TOKEN":"t"}}}}"#,
    );
    write(
        &dir.join("plugins/demo/hooks/hooks.json"),
        r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo done"}]}]}}"#,
    );
    write(
        &dir.join("plugins/demo/.app.json"),
        r#"{"apps":{"linear":{"id":"connector_linear"}}}"#,
    );
}

fn register(store: &Path, fixture: &Path) {
    add_marketplace(
        store,
        MarketplaceRef {
            name: "codex-local".into(),
            source: fixture.to_string_lossy().into_owned(),
            official: false,
        },
    )
    .unwrap();
}

#[test]
fn codex_catalog_installs_through_the_shared_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let quarantine = tmp.path().join("quarantine");
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&store).unwrap();
    build_codex_fixture(&fixture);
    register(&store, &fixture);

    let planned = resolve_and_plan(&store, &quarantine, "codex-local", "demo").unwrap();

    // Detected and lowered by the Codex adapter, not the Claude Code one.
    assert_eq!(planned.format, "codex");
    assert_eq!(planned.plan.plugin, "demo");
    assert!(
        planned
            .plan
            .adds
            .iter()
            .any(|a| a.kind == "skill" && a.name == "codex-local/demo:hello"),
        "namespaced skill missing: {:?}",
        planned.plan.adds
    );

    // Planning is pure.
    assert!(!store.join("demo@codex-local").exists());

    // Commit writes the self-contained native plugin dir.
    let dir = commit_install(&store, &planned, "2026-08-27T00:00:00Z".into()).unwrap();
    assert!(dir.join("plugin.toml").is_file());
    assert!(dir.join("skills/hello/SKILL.md").is_file());
    let prov: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("provenance.json")).unwrap())
            .unwrap();
    assert_eq!(prov["format"], "codex", "provenance records the adapter");
}

#[test]
fn codex_mcp_server_cannot_reach_the_store_without_spawn_consent() {
    // The consent path is the sharp edge: a Codex manifest declaring a server
    // must produce a spawn preview on the plan AND a consent sidecar keyed to
    // exactly what it executes. If either is missing, a foreign manifest has
    // bought itself a process spawn for free.
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let quarantine = tmp.path().join("quarantine");
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&store).unwrap();
    build_codex_fixture(&fixture);
    register(&store, &fixture);

    let planned = resolve_and_plan(&store, &quarantine, "codex-local", "demo").unwrap();

    let spawn = planned
        .plan
        .spawns
        .iter()
        .find(|s| s.name == "fetch")
        .expect("declared MCP server must appear on the consent surface");
    assert_eq!(spawn.command, "uvx");
    assert_eq!(spawn.args, vec!["mcp-server-fetch".to_string()]);
    assert_eq!(spawn.transport_kind, "stdio");
    assert_eq!(spawn.env_keys, vec!["FETCH_TOKEN".to_string()]);
    // Env NAMES only — the value must not be on the consent surface.
    assert!(
        !format!("{:?}", planned.plan.spawns).contains("\"t\""),
        "env values must not be previewed"
    );
    // The rendered consent text a user actually approves must name the spawn.
    let rendered = planned.plan.render();
    assert!(rendered.contains("will be allowed to spawn"), "{rendered}");
    assert!(rendered.contains("uvx mcp-server-fetch"), "{rendered}");

    let dir = commit_install(&store, &planned, "2026-08-27T00:00:00Z".into()).unwrap();
    let sidecar = dir.join(wcore_plugin_api::CONSENT_SIDECAR);
    assert!(
        sidecar.is_file(),
        "commit must write the MCP spawn-consent sidecar"
    );
    let consent: wcore_plugin_api::McpSpawnConsent =
        serde_json::from_str(&std::fs::read_to_string(&sidecar).unwrap()).unwrap();
    let expected = wcore_plugin_api::consent_key_from_parts(
        &wcore_plugin_api::mcp_server_spec::McpTransport::Stdio {
            command: "uvx".into(),
            args: vec!["mcp-server-fetch".into()],
        },
        ["FETCH_TOKEN"].into_iter(),
    );
    assert_eq!(
        consent.mcp_spawn_keys,
        vec![expected],
        "the granted key must match what the Codex manifest declared"
    );
}

#[test]
fn codex_lossy_surfaces_reach_the_install_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let quarantine = tmp.path().join("quarantine");
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&store).unwrap();
    build_codex_fixture(&fixture);
    register(&store, &fixture);

    let planned = resolve_and_plan(&store, &quarantine, "codex-local", "demo").unwrap();
    let kinds: Vec<&str> = planned
        .plan
        .ignored
        .iter()
        .map(|i| i.kind.as_str())
        .collect();
    for kind in [
        "hooks",
        "apps",
        "interface",
        "manifest-metadata",
        "marketplace-policy",
        "marketplace-display",
    ] {
        assert!(
            kinds.contains(&kind),
            "{kind} must reach the plan, got {kinds:?}"
        );
    }

    // Hooks were dropped, so the grade must not claim content parity.
    assert_eq!(planned.plan.grade, CompatibilityGrade::HooksIgnored);

    // And all of it has to be VISIBLE in the text the user approves.
    let rendered = planned.plan.render();
    assert!(
        rendered.contains("ignores (unsupported in v1)"),
        "{rendered}"
    );
    assert!(rendered.contains("connector_linear"), "{rendered}");
    assert!(rendered.contains("PostToolUse"), "{rendered}");
    assert!(rendered.contains("policy.installation"), "{rendered}");
}

#[test]
fn a_claude_code_catalog_still_resolves_through_the_same_entry_point() {
    // Preservation control: adding the Codex dialect must not move the Claude
    // Code path off `resolve_and_plan`.
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let quarantine = tmp.path().join("quarantine");
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&store).unwrap();
    write(
        &fixture.join(".claude-plugin/marketplace.json"),
        r#"{"name":"cc","owner":{"name":"t"},"plugins":[{"name":"demo","source":"./demo"}]}"#,
    );
    write(
        &fixture.join("demo/.claude-plugin/plugin.json"),
        r#"{"name":"demo","version":"0.1.0"}"#,
    );
    write(
        &fixture.join("demo/skills/hello/SKILL.md"),
        "---\nname: hello\n---\nhi",
    );
    add_marketplace(
        &store,
        MarketplaceRef {
            name: "cc".into(),
            source: fixture.to_string_lossy().into_owned(),
            official: false,
        },
    )
    .unwrap();

    let planned = resolve_and_plan(&store, &quarantine, "cc", "demo").unwrap();
    assert_eq!(planned.format, "claude-code");
    assert_eq!(planned.plan.grade, CompatibilityGrade::ContentCompatible);
}

#[test]
fn a_source_with_no_recognized_catalog_is_refused_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let quarantine = tmp.path().join("quarantine");
    let fixture = tmp.path().join("fixture");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&fixture).unwrap();
    add_marketplace(
        &store,
        MarketplaceRef {
            name: "empty".into(),
            source: fixture.to_string_lossy().into_owned(),
            official: false,
        },
    )
    .unwrap();

    let msg = match resolve_and_plan(&store, &quarantine, "empty", "demo") {
        Ok(_) => panic!("a source with no catalog must not resolve"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains(".claude-plugin/marketplace.json"), "{msg}");
    assert!(msg.contains(".agents/plugins/marketplace.json"), "{msg}");
}
