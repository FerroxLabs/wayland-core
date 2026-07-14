// Phase 10 inline tests for src/skills/bundled/mod.rs
// Covers TC-10.01 ~ TC-10.28 (registration API, field mapping, file extraction,
// resolve_skill_file_path path validation, prepare_bundled_skills, isolation).

use super::*;
use serial_test::serial;
use std::path::Path;
use wcore_types::model_aliases::ANTHROPIC_OPUS;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn minimal_def(name: &'static str) -> BundledSkillDefinition {
    BundledSkillDefinition {
        name,
        description: "test skill",
        when_to_use: None,
        argument_hint: None,
        allowed_tools: &[],
        model: None,
        disable_model_invocation: false,
        user_invocable: false,
        context: None,
        agent: None,
        files: &[],
        content: "content",
    }
}

// ---------------------------------------------------------------------------
// TC-10.01: register single skill
// ---------------------------------------------------------------------------

#[test]
fn tc_10_01_register_single_skill() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("tc-01"));
    let skills = catalog.get_bundled_skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "tc-01");
}

// ---------------------------------------------------------------------------
// TC-10.02: multiple registrations accumulate
// ---------------------------------------------------------------------------

#[test]
fn tc_10_02_register_multiple_accumulate() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("a"));
    register_bundled_skill(&mut catalog, minimal_def("b"));
    register_bundled_skill(&mut catalog, minimal_def("c"));
    let skills = catalog.get_bundled_skills();
    assert_eq!(skills.len(), 3);
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"a") && names.contains(&"b") && names.contains(&"c"));
}

// ---------------------------------------------------------------------------
// TC-10.03: fresh catalogs do not inherit entries
// ---------------------------------------------------------------------------

#[test]
fn tc_10_03_fresh_catalog_is_empty() {
    let mut first = BundledSkillCatalog::new();
    register_bundled_skill(&mut first, minimal_def("first-only"));
    let second = BundledSkillCatalog::new();
    assert_eq!(first.get_bundled_skills().len(), 1);
    assert!(second.get_bundled_skills().is_empty());
}

// ---------------------------------------------------------------------------
// TC-10.04: init_bundled_skills registers hello skill
// ---------------------------------------------------------------------------

#[test]
fn tc_10_04_init_registers_hello() {
    let catalog = init_bundled_skills();
    let skills = catalog.get_bundled_skills();
    assert!(!skills.is_empty());
    assert!(skills.iter().any(|s| s.name == "hello"));
}

// ---------------------------------------------------------------------------
// TC-10.05: full field mapping
// ---------------------------------------------------------------------------

#[test]
fn tc_10_05_full_field_mapping() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            name: "full-skill",
            description: "desc",
            when_to_use: Some("when"),
            argument_hint: Some("arg"),
            allowed_tools: &["Bash", "Read"],
            model: Some(ANTHROPIC_OPUS),
            disable_model_invocation: false,
            user_invocable: true,
            context: Some("inline"),
            agent: Some("my-agent"),
            files: &[],
            content: "body",
        },
    );
    let skills = catalog.get_bundled_skills();
    let m = &skills[0];
    assert_eq!(m.name, "full-skill");
    assert_eq!(m.description, "desc");
    assert_eq!(m.when_to_use.as_deref(), Some("when"));
    assert_eq!(m.argument_hint.as_deref(), Some("arg"));
    assert_eq!(m.allowed_tools, vec!["Bash", "Read"]);
    assert_eq!(m.model.as_deref(), Some(ANTHROPIC_OPUS));
    assert!(!m.disable_model_invocation);
    assert!(m.user_invocable);
    assert_eq!(m.agent.as_deref(), Some("my-agent"));
    assert!(m.has_user_specified_description);
}

// ---------------------------------------------------------------------------
// TC-10.06: source and loaded_from are Bundled
// ---------------------------------------------------------------------------

#[test]
fn tc_10_06_source_and_loaded_from_bundled() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("src-test"));
    let skills = catalog.get_bundled_skills();
    let m = &skills[0];
    assert_eq!(m.source, SkillSource::Bundled);
    assert_eq!(m.loaded_from, LoadedFrom::Bundled);
}

// ---------------------------------------------------------------------------
// TC-10.07: context="inline" maps to ExecutionContext::Inline
// ---------------------------------------------------------------------------

#[test]
fn tc_10_07_context_inline_maps_correctly() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            context: Some("inline"),
            ..minimal_def("ctx-inline")
        },
    );
    let m = &catalog.get_bundled_skills()[0];
    assert_eq!(m.execution_context, ExecutionContext::Inline);
}

// ---------------------------------------------------------------------------
// TC-10.08: context="fork" maps to ExecutionContext::Fork
// ---------------------------------------------------------------------------

#[test]
fn tc_10_08_context_fork_maps_correctly() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            context: Some("fork"),
            ..minimal_def("ctx-fork")
        },
    );
    let m = &catalog.get_bundled_skills()[0];
    assert_eq!(m.execution_context, ExecutionContext::Fork);
}

// ---------------------------------------------------------------------------
// TC-10.09: context=None defaults to ExecutionContext::Inline
// ---------------------------------------------------------------------------

#[test]
fn tc_10_09_context_none_defaults_to_inline() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("ctx-none"));
    let m = &catalog.get_bundled_skills()[0];
    assert_eq!(
        m.execution_context,
        ExecutionContext::Inline,
        "context=None should default to Inline"
    );
}

// ---------------------------------------------------------------------------
// TC-10.10: no files → skill_root is None
// ---------------------------------------------------------------------------

#[test]
fn tc_10_10_no_files_skill_root_none() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("no-files"));
    let m = &catalog.get_bundled_skills()[0];
    assert!(m.skill_root.is_none());
}

// ---------------------------------------------------------------------------
// TC-10.11: with files → prepare_bundled_skills sets skill_root
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_11_files_skill_root_set_by_prepare() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("guide.md", "# Guide")],
            ..minimal_def("file-skill")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let m = skills.iter().find(|s| s.name == "file-skill").unwrap();
    assert!(
        m.skill_root.is_some(),
        "skill_root should be set by prepare_bundled_skills"
    );
    assert!(m.skill_root.as_ref().unwrap().contains("file-skill"));
}

// ---------------------------------------------------------------------------
// TC-10.12: extraction — directory and file created
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_12_extract_creates_dir_and_file() {
    let result = extract_bundled_skill_files("tc-12-skill", &[("data.md", "content")]).await;
    let dir = result.expect("extraction should succeed");
    let file = dir.join("data.md");
    assert!(file.exists(), "extracted file should exist");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "content",
        "file content should match"
    );
    // cleanup
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// TC-10.13: directory permission 0o700 (unix only)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_13_dir_permission_0700() {
    let result = extract_bundled_skill_files("tc-13-skill", &[("perm.md", "x")]).await;
    let dir = result.expect("extraction should succeed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&dir).unwrap();
        assert_eq!(
            meta.permissions().mode() & 0o777,
            0o700,
            "directory must be owner-only (0o700)"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// TC-10.14: file permission 0o600 (unix only)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_14_file_permission_0600() {
    let result = extract_bundled_skill_files("tc-14-skill", &[("file.md", "y")]).await;
    let dir = result.expect("extraction should succeed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fmeta = std::fs::metadata(dir.join("file.md")).unwrap();
        assert_eq!(
            fmeta.permissions().mode() & 0o777,
            0o600,
            "file must be owner-only (0o600)"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// TC-10.15: path traversal rejected at integration layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_15_path_traversal_rejected_integration() {
    let result = extract_bundled_skill_files("tc-15-evil", &[("../escape.txt", "pwned")]).await;
    // Either extraction fails entirely, or the traversal entry is skipped
    if let Some(dir) = result {
        assert!(
            !dir.parent().unwrap().join("escape.txt").exists(),
            "traversal file must not be created outside extract dir"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
    // If result is None, the test also passes (extraction was rejected)
}

// ---------------------------------------------------------------------------
// TC-10.16: extraction failure returns None, not panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_16_extraction_failure_returns_none() {
    // Pass an empty files slice — extract_bundled_skill_files returns None for empty
    let result = extract_bundled_skill_files("tc-16-empty", &[]).await;
    assert!(
        result.is_none(),
        "empty files should return None without panic"
    );
}

// ---------------------------------------------------------------------------
// TC-10.17: get_bundled_skill_extract_dir path format
// ---------------------------------------------------------------------------

#[test]
fn tc_10_17_extract_dir_path_format() {
    let path = get_bundled_skill_extract_dir("my-skill");
    let s = path.to_string_lossy();
    assert!(
        s.contains("wayland-core-bundled-skills"),
        "path should contain wayland-core-bundled-skills"
    );
    assert!(s.contains("my-skill"), "path should contain skill name");
}

// ---------------------------------------------------------------------------
// TC-10.19: get_bundled_skills is idempotent (does not consume catalog)
// ---------------------------------------------------------------------------

#[test]
fn tc_10_19_get_bundled_skills_idempotent() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("idem-a"));
    register_bundled_skill(&mut catalog, minimal_def("idem-b"));
    assert_eq!(catalog.get_bundled_skills().len(), 2);
    assert_eq!(catalog.get_bundled_skills().len(), 2);
}

// ---------------------------------------------------------------------------
// TC-10.23: content_length field is correct
// ---------------------------------------------------------------------------

#[test]
fn tc_10_23_content_length_correct() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            content: "hello world",
            ..minimal_def("cl-skill")
        },
    );
    let m = &catalog.get_bundled_skills()[0];
    assert_eq!(m.content_length, "hello world".len());
}

// ---------------------------------------------------------------------------
// TC-10.24: embedded definitions stay ahead of appended plugin entries
// ---------------------------------------------------------------------------

#[test]
fn tc_10_24_embedded_then_plugin_insertion_order_is_preserved() {
    let mut catalog = BundledSkillCatalog::embedded();
    register_bundled_skill(&mut catalog, minimal_def("plugin-first"));
    register_bundled_skill(&mut catalog, minimal_def("plugin-second"));
    let names: Vec<_> = catalog
        .get_bundled_skills()
        .into_iter()
        .map(|skill| skill.name)
        .collect();
    assert_eq!(
        names,
        vec![
            "hello".to_owned(),
            "plugin-first".to_owned(),
            "plugin-second".to_owned()
        ]
    );
}

// ---------------------------------------------------------------------------
// TC-10.25: unknown context string defaults to Inline
// ---------------------------------------------------------------------------

#[test]
fn tc_10_25_unknown_context_defaults_to_inline() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            context: Some("unknown-value"),
            ..minimal_def("ctx-unknown")
        },
    );
    let m = &catalog.get_bundled_skills()[0];
    assert_eq!(m.execution_context, ExecutionContext::Inline);
}

// ---------------------------------------------------------------------------
// TC-10.27: resolve_skill_file_path path validation (private fn, inline test)
// ---------------------------------------------------------------------------

#[test]
fn tc_10_27a_resolve_normal_path_ok() {
    let result = resolve_skill_file_path(Path::new("/base"), "sub/file.md");
    assert!(result.is_ok(), "normal relative path should be Ok");
    assert_eq!(
        result.unwrap(),
        std::path::PathBuf::from("/base/sub/file.md")
    );
}

#[test]
fn tc_10_27b_resolve_traversal_rejected() {
    let result = resolve_skill_file_path(Path::new("/base"), "../escape.txt");
    assert!(result.is_err(), "path traversal '../' must be rejected");
}

#[test]
fn tc_10_27c_resolve_absolute_path_rejected() {
    // Use a platform-appropriate absolute path so `Path::is_absolute()` returns true
    #[cfg(unix)]
    let abs_path = "/etc/passwd";
    #[cfg(windows)]
    let abs_path = "C:\\Windows\\System32\\drivers\\etc\\hosts";

    let result = resolve_skill_file_path(Path::new("/base"), abs_path);
    assert!(result.is_err(), "absolute path must be rejected");
}

#[test]
fn tc_10_27d_resolve_disguised_traversal_rejected() {
    let result = resolve_skill_file_path(Path::new("/base"), "sub/../escape");
    assert!(
        result.is_err(),
        "disguised traversal 'sub/../escape' must be rejected"
    );
}

// ---------------------------------------------------------------------------
// TC-10.28: init_bundled_skills is idempotent
// ---------------------------------------------------------------------------

#[test]
fn tc_10_28_init_idempotent() {
    let first = init_bundled_skills();
    let second = init_bundled_skills();
    let first_count = first
        .get_bundled_skills()
        .iter()
        .filter(|skill| skill.name == "hello")
        .count();
    let second_count = second
        .get_bundled_skills()
        .iter()
        .filter(|skill| skill.name == "hello")
        .count();
    assert_eq!(
        (first_count, second_count),
        (1, 1),
        "init_bundled_skills must be idempotent — hello should appear exactly once"
    );
}

// ---------------------------------------------------------------------------
// TC-10.29 (Windows only): bundled skill extraction must succeed on Windows.
// Audit W-3 regression guard (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
// Verifies that create_dir_secure + open_secure work on Windows — previously
// they compiled but the #[cfg(not(unix))] path had no ACL restriction at all.
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[tokio::test]
#[serial]
async fn tc_10_29_windows_bundled_skill_extract_succeeds() {
    // Extract two files and verify both land under the expected directory.
    // On Windows this exercises the icacls ACL-tightening path in
    // create_dir_secure() and open_secure().
    let dir = extract_bundled_skill_files(
        "tc-29-win-skill",
        &[("skill.md", "windows test"), ("meta.toml", "[skill]")],
    )
    .await
    .expect("Windows extraction must succeed");

    assert!(dir.join("skill.md").exists(), "skill.md must be created");
    assert!(dir.join("meta.toml").exists(), "meta.toml must be created");

    // Verify the directory path is under the expected temp prefix.
    let path_str = dir.to_string_lossy();
    assert!(
        path_str.contains("wayland-core-bundled-skills"),
        "extract dir must use the standard bundled-skill temp prefix, got: {path_str}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
