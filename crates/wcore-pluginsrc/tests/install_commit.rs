use std::fs;
use std::path::Path;

use tempfile::tempdir;
use wcore_plugin_api::manifest::PluginManifest;
use wcore_pluginsrc::PluginFormatAdapter;
use wcore_pluginsrc::claude_code::ClaudeCodeAdapter;
use wcore_pluginsrc::commit::{CommitMeta, commit_plan};
use wcore_pluginsrc::model::{SourceEntry, SourceKind};

fn write(p: &Path, body: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

#[test]
fn commit_writes_self_contained_native_plugin() {
    let fetched = tempdir().unwrap();
    let root = fetched.path();
    write(
        &root.join(".claude-plugin/plugin.json"),
        r#"{"name":"db","version":"1.0.0"}"#,
    );
    write(
        &root.join("skills/query/SKILL.md"),
        "---\nname: query\ndescription: q\n---\nbody",
    );
    write(
        &root.join("agents/reviewer.md"),
        "---\nname: reviewer\ndescription: r\nmodel: sonnet\n---\nReview.",
    );
    write(
        &root.join(".mcp.json"),
        r#"{"mcpServers":{"database":{"command":"${CLAUDE_PLUGIN_ROOT}/srv","args":[]}}}"#,
    );

    let entry = SourceEntry {
        name: "db".into(),
        kind: SourceKind::RelativePath("./db".into()),
        strict: true,
        declared_version: None,
        description: None,
    };
    let draft = ClaudeCodeAdapter.lower("acme", &entry, root).unwrap();

    let store = tempdir().unwrap();
    let meta = CommitMeta {
        marketplace: "acme",
        format: "claude-code",
        resolved_sha: Some("abc123".into()),
    };
    let dir = commit_plan(&draft, &meta, root, store.path()).unwrap();

    // Self-contained, flat, single-level directory the loader will discover.
    assert_eq!(dir.file_name().unwrap().to_str().unwrap(), "db@acme");

    // The generated plugin.toml must parse via the REAL manifest parser, and
    // classify as a declarative plugin carrying its MCP server.
    let toml_src = fs::read_to_string(dir.join("plugin.toml")).unwrap();
    let manifest =
        PluginManifest::from_toml_str(&toml_src).expect("generated plugin.toml must parse");
    assert_eq!(manifest.runtime.as_ref().unwrap().kind, "declarative");
    assert!(manifest.mcp_server.is_some());
    assert!(manifest.permissions.register_skills);
    assert!(manifest.permissions.register_agents);
    assert!(manifest.permissions.register_mcp_server);

    // Content copied verbatim / converted.
    assert!(dir.join("skills/query/SKILL.md").is_file());
    assert!(dir.join("agents/reviewer.yaml").is_file());

    // Provenance sidecar recorded the origin + resolved sha.
    let prov = fs::read_to_string(dir.join("provenance.json")).unwrap();
    assert!(prov.contains("\"marketplace\": \"acme\""));
    assert!(prov.contains("abc123"));

    // Spawn-consent sidecar (Lane E): the plugin ships an MCP server, so a
    // consent grant is recorded — exactly one key, a 64-char hex SHA-256.
    let consent = wcore_plugin_api::McpSpawnConsent::load(&dir).expect("consent sidecar readable");
    let consent = consent.expect("a plugin with an MCP server must record a consent grant");
    assert_eq!(consent.mcp_spawn_keys.len(), 1);
    let key = &consent.mcp_spawn_keys[0];
    assert_eq!(key.len(), 64);
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));

    // Transactional: no staging directory left behind.
    assert!(!store.path().join(".staging-db@acme").exists());
}

#[test]
fn reinstall_replaces_existing_directory() {
    let fetched = tempdir().unwrap();
    let root = fetched.path();
    write(&root.join(".claude-plugin/plugin.json"), r#"{"name":"p"}"#);
    write(
        &root.join("skills/one/SKILL.md"),
        "---\nname: one\ndescription: d\n---\nx",
    );

    let entry = SourceEntry {
        name: "p".into(),
        kind: SourceKind::RelativePath("./p".into()),
        strict: true,
        declared_version: None,
        description: None,
    };
    let store = tempdir().unwrap();
    let meta = CommitMeta {
        marketplace: "m",
        format: "claude-code",
        resolved_sha: None,
    };

    let draft1 = ClaudeCodeAdapter.lower("m", &entry, root).unwrap();
    let dir = commit_plan(&draft1, &meta, root, store.path()).unwrap();
    assert!(dir.join("skills/one/SKILL.md").is_file());

    // Second install with a different skill set must fully replace the first.
    fs::remove_dir_all(root.join("skills/one")).unwrap();
    write(
        &root.join("skills/two/SKILL.md"),
        "---\nname: two\ndescription: d\n---\nx",
    );
    let draft2 = ClaudeCodeAdapter.lower("m", &entry, root).unwrap();
    let dir2 = commit_plan(&draft2, &meta, root, store.path()).unwrap();
    assert_eq!(dir, dir2);
    assert!(dir2.join("skills/two/SKILL.md").is_file());
    assert!(
        !dir2.join("skills/one").exists(),
        "stale skill must be gone"
    );
}
