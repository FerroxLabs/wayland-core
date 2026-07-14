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
    let root = Path::new(m.skill_root.as_deref().unwrap());
    assert_eq!(root.file_name().unwrap(), "skill-0");
    assert!(
        root.join("guide.md").is_file(),
        "reference file should exist under the catalog-owned root"
    );
    let process_root_name = root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .expect("process extraction root should have a UTF-8 name");
    let nonce = process_root_name
        .strip_prefix("wayland-core-bundled-skills-")
        .expect("process extraction root should use the standard prefix");
    uuid::Uuid::parse_str(nonce).expect("process extraction root should carry a UUID nonce");
}

// ---------------------------------------------------------------------------
// TC-10.12: extraction — directory and file created
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_12_extract_creates_dir_and_file() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("data.md", "content")],
            ..minimal_def("tc-12-skill")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let dir = PathBuf::from(
        skills[0]
            .skill_root
            .as_deref()
            .expect("catalog-owned extraction should succeed"),
    );
    let file = dir.join("data.md");
    assert!(file.exists(), "extracted file should exist");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "content",
        "file content should match"
    );
}

// ---------------------------------------------------------------------------
// TC-10.13: directory permission 0o700 (unix only)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_13_dir_permission_0700() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("perm.md", "x")],
            ..minimal_def("tc-13-skill")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let dir = PathBuf::from(
        skills[0]
            .skill_root
            .as_deref()
            .expect("catalog-owned extraction should succeed"),
    );
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
}

// ---------------------------------------------------------------------------
// TC-10.14: file permission 0o600 (unix only)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tc_10_14_file_permission_0600() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("file.md", "y")],
            ..minimal_def("tc-14-skill")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let dir = PathBuf::from(
        skills[0]
            .skill_root
            .as_deref()
            .expect("catalog-owned extraction should succeed"),
    );
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
}

// ---------------------------------------------------------------------------
// TC-10.15: path traversal rejected at integration layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_15_path_traversal_rejected_integration() {
    let mut catalog = BundledSkillCatalog::new();
    let escape_path = catalog
        .extraction_root
        .as_ref()
        .expect("test extraction root should be available")
        .join("escape.txt");
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("../escape.txt", "pwned")],
            ..minimal_def("tc-15-evil")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    assert!(
        skills[0].skill_root.is_none(),
        "a rejected traversal must not publish a skill root"
    );
    assert!(
        !escape_path.exists(),
        "traversal file must not be created outside the indexed skill root"
    );
}

// ---------------------------------------------------------------------------
// TC-10.16: extraction failure returns None, not panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_16_extraction_failure_returns_none() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(&mut catalog, minimal_def("tc-16-empty"));
    let skills = catalog.prepare_bundled_skills().await;
    assert!(
        skills[0].skill_root.is_none(),
        "empty files should not publish an extraction root"
    );
}

// ---------------------------------------------------------------------------
// TC-10.17: extraction path is catalog-owned, not caller-name-derived
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tc_10_17_extract_dir_is_catalog_owned() {
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("guide.md", "safe")],
            ..minimal_def("../caller-controlled-name")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let path = Path::new(
        skills[0]
            .skill_root
            .as_deref()
            .expect("catalog-owned extraction should succeed"),
    );
    let s = path.to_string_lossy();
    assert!(
        s.contains("wayland-core-bundled-skills"),
        "path should contain wayland-core-bundled-skills"
    );
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("skill-0")
    );
    assert!(
        !s.contains("caller-controlled-name"),
        "the extraction path must not contain the caller-provided skill name"
    );
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
        vec!["plugin-first".to_owned(), "plugin-second".to_owned()],
        "embedded() is fixture-free and appended plugin order is stable"
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
// Verifies that capability-relative extraction and ACL hardening work together.
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn open_windows_acl_subject(path: &Path, directory: bool) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        FILE_SHARE_WRITE, READ_CONTROL,
    };

    let flags = if directory {
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT
    } else {
        FILE_FLAG_OPEN_REPARSE_POINT
    };
    std::fs::OpenOptions::new()
        .access_mode(READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(flags)
        .open(path)
        .unwrap_or_else(|error| panic!("open ACL subject {}: {error}", path.display()))
}

#[cfg(windows)]
fn assert_windows_owner_only_dacl<T: std::os::windows::io::AsRawHandle>(handle: &T) {
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACCESS_ALLOWED_ACE_TYPE, ACL, DACL_SECURITY_INFORMATION, EqualSid,
        GetAce, GetSecurityDescriptorControl, OWNER_SECURITY_INFORMATION, PSID, SE_DACL_PROTECTED,
    };

    let mut owner: PSID = std::ptr::null_mut();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: the handle carries READ_CONTROL and all output pointers are live.
    let result = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    assert_eq!(result, 0, "GetSecurityInfo failed: {result:#x}");
    let _descriptor = WindowsLocalAlloc(descriptor);
    assert!(!dacl.is_null(), "secured object must have a DACL");

    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor is the live buffer returned by GetSecurityInfo.
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0,
        "GetSecurityDescriptorControl failed: {}",
        std::io::Error::last_os_error()
    );
    assert_ne!(
        control & SE_DACL_PROTECTED,
        0,
        "bundled extraction DACL must be protected"
    );

    // SAFETY: dacl belongs to the live descriptor and is valid for inspection.
    assert_eq!(unsafe { (*dacl).AceCount }, 1, "DACL must contain one ACE");
    let mut raw_ace = std::ptr::null_mut();
    // SAFETY: index zero exists because AceCount is exactly one.
    assert_ne!(
        unsafe { GetAce(dacl, 0, &mut raw_ace) },
        0,
        "GetAce failed: {}",
        std::io::Error::last_os_error()
    );
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: GetAce returned the sole ACE in the valid DACL.
    assert_eq!(
        unsafe { (*ace).Header.AceType },
        ACCESS_ALLOWED_ACE_TYPE,
        "sole DACL entry must be an allow ACE"
    );
    // SAFETY: SidStart is the first byte of the variable-length SID stored in
    // ACCESS_ALLOWED_ACE.
    let allowed_sid = unsafe { std::ptr::addr_of_mut!((*ace).SidStart).cast() };
    let token_user = current_windows_token_user().expect("query current TokenUser");
    assert!(!owner.is_null(), "secured object must have an owner");
    // SAFETY: owner belongs to the live descriptor and TokenUser remains live.
    assert_ne!(
        unsafe { EqualSid(owner, token_user.sid()) },
        0,
        "object owner must equal the current process TokenUser"
    );
    // SAFETY: both pointers name live, valid SIDs for the duration of the call.
    assert_ne!(
        unsafe { EqualSid(allowed_sid, token_user.sid()) },
        0,
        "allowed SID must equal the current process TokenUser"
    );
}

#[cfg(windows)]
#[tokio::test]
#[serial]
async fn tc_10_29_windows_bundled_skill_extract_succeeds() {
    // Extract two files and verify both land under the expected directory.
    // On Windows this exercises handle-relative, no-follow creation plus the
    // fail-closed handle-bound DACL paths for both directories and files.
    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("skill.md", "windows test"), ("meta.toml", "[skill]")],
            ..minimal_def("tc-29-win-skill")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;
    let dir = PathBuf::from(
        skills[0]
            .skill_root
            .as_deref()
            .expect("Windows extraction must succeed"),
    );

    assert!(dir.join("skill.md").exists(), "skill.md must be created");
    assert!(dir.join("meta.toml").exists(), "meta.toml must be created");

    // Verify the directory path is under the expected temp prefix.
    let path_str = dir.to_string_lossy();
    assert!(
        path_str.contains("wayland-core-bundled-skills"),
        "extract dir must use the standard bundled-skill temp prefix, got: {path_str}"
    );

    let catalog_dir_path = dir.parent().expect("catalog directory");
    let process_root_path = catalog_dir_path.parent().expect("process extraction root");
    let process_root = open_windows_acl_subject(process_root_path, true);
    let catalog_dir = open_windows_acl_subject(catalog_dir_path, true);
    let skill_dir = open_windows_acl_subject(&dir, true);
    let skill_file = open_windows_acl_subject(&dir.join("skill.md"), false);
    assert_windows_owner_only_dacl(&process_root);
    assert_windows_owner_only_dacl(&catalog_dir);
    assert_windows_owner_only_dacl(&skill_dir);
    assert_windows_owner_only_dacl(&skill_file);
}

#[cfg(windows)]
#[tokio::test]
async fn tc_10_30_windows_acl_uses_token_user_without_launching_icacls() {
    let hostile = tempfile::tempdir().expect("hostile executable directory");
    let marker = hostile.path().join("icacls-launched");
    std::fs::write(
        hostile.path().join("icacls.cmd"),
        b"@echo off\r\n> \"%ICACLS_MARKER%\" echo launched\r\nexit /b 0\r\n",
    )
    .expect("plant hostile icacls.cmd");

    let current_exe = std::env::current_exe().expect("current test executable");
    let current_exe = current_exe.to_string_lossy().into_owned();
    let hostile_path = hostile.path().to_string_lossy().into_owned();
    let mut child = wcore_config::shell::shell_command_argv(
        &current_exe,
        &[
            "--exact",
            "bundled::tests::tc_10_30_windows_acl_subprocess",
            "--ignored",
            "--nocapture",
        ],
    );
    child
        .current_dir(hostile.path())
        .env("PATH", &hostile_path)
        .env("ICACLS_MARKER", &marker)
        .env("USERNAME", "not-the-token-user");
    child.kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.output())
        .await
        .expect("isolated ACL subprocess timed out")
        .expect("run isolated ACL subprocess");
    assert!(
        output.status.success(),
        "isolated hostile-environment extraction failed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "hostile icacls sentinel must not be launched"
    );
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "subprocess helper for hostile Windows process state"]
async fn tc_10_30_windows_acl_subprocess() {
    assert_eq!(
        std::env::var("USERNAME").as_deref(),
        Ok("not-the-token-user")
    );
    assert!(std::env::current_dir()
        .expect("hostile subprocess cwd")
        .join("icacls.cmd")
        .is_file());

    let mut catalog = BundledSkillCatalog::new();
    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("guide.md", "handle ACL")],
            ..minimal_def("tc-30-handle-acl")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;

    let root = PathBuf::from(
        skills[0]
            .skill_root
            .as_deref()
            .expect("hostile PATH/cwd must not affect native ACL hardening"),
    );
    let file = open_windows_acl_subject(&root.join("guide.md"), false);
    assert_windows_owner_only_dacl(&file);
}

#[cfg(windows)]
#[tokio::test]
#[serial]
async fn tc_10_31_windows_non_directory_component_is_rejected() {
    let mut catalog = BundledSkillCatalog::new();
    let catalog_root = catalog
        .extraction_root
        .as_ref()
        .expect("catalog extraction root")
        .to_owned();
    std::fs::create_dir(&catalog_root).expect("plant catalog directory");
    std::fs::write(catalog_root.join("skill-0"), b"not a directory")
        .expect("plant non-directory component");

    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("guide.md", "must stay contained")],
            ..minimal_def("reparse-probe")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;

    assert!(
        skills[0].skill_root.is_none(),
        "every extraction component must be reopened as a directory"
    );
}

#[cfg(windows)]
#[tokio::test]
#[serial]
async fn tc_10_32_windows_junction_component_is_rejected() {
    let outside = tempfile::tempdir().expect("outside temp directory");
    let mut catalog = BundledSkillCatalog::new();
    let catalog_root = catalog
        .extraction_root
        .as_ref()
        .expect("catalog extraction root")
        .to_owned();
    std::fs::create_dir(&catalog_root).expect("plant catalog directory");
    let junction = catalog_root.join("skill-0");
    let junction_arg = junction.to_string_lossy().into_owned();
    let outside_arg = outside.path().to_string_lossy().into_owned();
    let output = wcore_config::shell::shell_command_argv(
        "cmd.exe",
        &["/D", "/C", "mklink", "/J", &junction_arg, &outside_arg],
    )
    .output()
    .await
    .expect("launch standard Windows junction command");
    assert!(
        output.status.success(),
        "standard-runner junction setup must succeed; stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    register_bundled_skill(
        &mut catalog,
        BundledSkillDefinition {
            files: &[("guide.md", "must stay contained")],
            ..minimal_def("reparse-probe")
        },
    );
    let skills = catalog.prepare_bundled_skills().await;

    assert!(
        skills[0].skill_root.is_none(),
        "a reparse-point directory must fail closed"
    );
    assert!(
        !outside.path().join("guide.md").exists(),
        "capability-relative extraction must not follow the planted reparse point"
    );
}

#[cfg(windows)]
#[test]
fn tc_10_33_windows_root_handle_pins_directory_until_drop() {
    let temp = tempfile::tempdir().expect("temporary parent");
    let root = temp.path().join("pinned-root");
    std::fs::create_dir(&root).expect("create root");
    let retained = open_windows_capability_root(&root).expect("open retained capability");

    assert!(
        std::fs::remove_dir(&root).is_err(),
        "a retained no-delete handle must prevent root replacement"
    );
    drop(retained);
    std::fs::remove_dir(&root).expect("root must become removable after capability drop");
}

#[cfg(windows)]
#[test]
fn tc_10_34_windows_relative_create_starts_owner_only() {
    let temp = tempfile::tempdir().expect("temporary parent");
    let root = temp.path().join("atomic-root");
    create_windows_owner_only_directory(&root).expect("create secured process root");
    let retained = open_windows_capability_root(&root).expect("open retained process root");

    let directory =
        create_windows_relative_object(&retained, std::ffi::OsStr::new("atomic-directory"), true)
            .expect("create secured relative directory");
    assert_windows_owner_only_dacl(&directory);

    let directory = windows_directory_from_file(directory).expect("retain created directory");
    let file =
        create_windows_relative_object(&directory, std::ffi::OsStr::new("atomic-file"), false)
            .expect("create secured relative file");
    assert_windows_owner_only_dacl(&file);
}
