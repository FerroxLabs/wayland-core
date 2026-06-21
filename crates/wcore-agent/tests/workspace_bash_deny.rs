//! Task 9 — Workspace-posture Bash gating and deny-manifest assertions.
//!
//! Tests:
//!
//! 1. `apply_posture(Workspace, read_deny_enforced=true)` → Bash is retained
//!    in the registry.
//! 2. `apply_posture(Workspace, read_deny_enforced=false)` → Bash is dropped
//!    from the registry.
//! 3. A `WorkspacePolicy::contained` over a workspace containing `.env` →
//!    `secret_deny_paths()` is non-empty and includes the `.env` path.
//!    (Proves the deny list that `build_sandbox_pieces` copies into the
//!    manifest is populated by the real policy path.)

use async_trait::async_trait;
use wcore_agent::channel_tools::{ChannelToolScope, apply_posture};
use wcore_channels::ChannelToolPosture;
use wcore_protocol::events::ToolCategory;
use wcore_tools::{Tool, registry::ToolRegistry, workspace_policy::WorkspacePolicy};
use wcore_types::tool::{JsonSchema, ToolResult};

// ===========================================================================
// Minimal fake Bash tool for registry tests.
// ===========================================================================

struct FakeBash;

#[async_trait]
impl Tool for FakeBash {
    fn name(&self) -> &str {
        "Bash"
    }
    fn description(&self) -> &str {
        "fake bash for posture gating tests"
    }
    fn input_schema(&self) -> JsonSchema {
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
        ToolCategory::Exec
    }
}

// ===========================================================================
// 1. Workspace + read_deny_enforced=true → Bash is retained.
// ===========================================================================

#[test]
fn workspace_bash_retained_when_deny_enforced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeBash));
    let scope = ChannelToolScope {
        posture: ChannelToolPosture::Workspace,
        workspace_root: tmp.path().to_path_buf(),
    };
    apply_posture(&mut reg, &scope, /* read_deny_enforced= */ true);

    let names = reg.tool_names();
    assert!(
        names.iter().any(|n| n == "Bash"),
        "Bash must survive Workspace when read_deny_enforced=true; tools: {names:?}"
    );
}

// ===========================================================================
// 2. Workspace + read_deny_enforced=false → Bash is dropped.
// ===========================================================================

#[test]
fn workspace_bash_dropped_when_deny_not_enforced() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FakeBash));
    let scope = ChannelToolScope {
        posture: ChannelToolPosture::Workspace,
        workspace_root: tmp.path().to_path_buf(),
    };
    apply_posture(&mut reg, &scope, /* read_deny_enforced= */ false);

    let names = reg.tool_names();
    assert!(
        !names.iter().any(|n| n == "Bash"),
        "Bash must be dropped from Workspace when read_deny_enforced=false; tools: {names:?}"
    );
}

// ===========================================================================
// 3. WorkspacePolicy::contained over a workspace with .env →
//    secret_deny_paths() non-empty and contains .env.
// ===========================================================================

#[test]
fn contained_policy_secret_deny_paths_include_env() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).expect("canonicalize root");

    // Write a .env so compute_secret_deny finds a workspace secret.
    let env_file = root.join(".env");
    std::fs::write(&env_file, b"SECRET=hunter2").expect("write .env");

    let policy = WorkspacePolicy::contained(&root);
    let deny_paths = policy.secret_deny_paths();

    assert!(
        !deny_paths.is_empty(),
        "Contained workspace with .env must have non-empty secret_deny_paths()"
    );

    let canon_env = std::fs::canonicalize(&env_file).expect("canonicalize .env");
    assert!(
        deny_paths.contains(&canon_env),
        ".env must appear in secret_deny_paths(); paths: {deny_paths:?}",
    );
}
