//! The repository-control surface (`.git`, `.wayland-core`) is READ-ONLY to
//! the in-process file tools, and a workspace with no version control can be
//! made to keep the strict profile.
//!
//! Graded end-to-end through `AgentBootstrap`, not against the predicate: the
//! predicate had unit coverage from the first commit, and what actually decides
//! whether a `Write` lands is which VFS the bootstrap installs on the registry.
//! Every assertion here therefore goes through the same
//! `engine.current_tool_context().vfs` the production dispatcher hands to
//! `WriteTool::execute_with_ctx`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_tools::vfs::VirtualFs;
use wcore_types::message::FinishReason;

fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(5),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

/// A config whose workspace carries a current local trust grant — the only
/// input that selects the trusted-local profile.
fn trusted_config() -> Config {
    let mut config = minimal_config();
    config.workspace_trust = wcore_types::workspace_trust::resolve_workspace_trust(
        "test-fingerprint",
        [wcore_types::workspace_trust::WorkspaceTrustInput::grant(
            wcore_types::workspace_trust::AuthoritySource::LocalSession,
        )],
    );
    config
}

/// `OutputSink` that keeps every `emit_info` so a startup notice can be
/// asserted. `NullSink` discards them, which would make "it announces itself"
/// unfalsifiable.
#[derive(Default)]
struct RecordingSink {
    info: Mutex<Vec<String>>,
}

impl RecordingSink {
    fn infos(&self) -> Vec<String> {
        self.info.lock().expect("info lock").clone()
    }
}

impl OutputSink for RecordingSink {
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
    fn emit_tool_call(&self, _name: &str, _input: &str) {}
    fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
    fn emit_stream_start(&self, _msg_id: &str) {}
    fn emit_stream_end(
        &self,
        _msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_creation_tokens: u64,
        _cache_read_tokens: u64,
        _finish_reason: FinishReason,
    ) {
    }
    fn emit_error(&self, _msg: &str, _retryable: bool) {}
    fn emit_info(&self, msg: &str) {
        self.info.lock().expect("info lock").push(msg.to_string());
    }
}

/// A workspace that looks like a real checkout: a git directory with a hook
/// path that already exists (so a pre-change tree genuinely writes the file
/// rather than failing on a missing parent), plus a project skill.
fn populate(root: &Path) {
    std::fs::create_dir_all(root.join(".git/hooks")).expect("hooks dir");
    std::fs::write(root.join(".git/HEAD"), b"ref: refs/heads/main\n").expect("HEAD");
    std::fs::create_dir_all(root.join(".wayland-core/skills/demo")).expect("skills dir");
    std::fs::write(root.join(".wayland-core/skills/demo/SKILL.md"), b"# demo\n").expect("skill");
    std::fs::create_dir_all(root.join("src")).expect("src dir");
}

async fn tool_vfs_for(config: Config, root: &Path) -> Arc<dyn VirtualFs> {
    let sink: Arc<dyn OutputSink> = Arc::new(RecordingSink::default());
    let result = AgentBootstrap::new(config, root.to_str().expect("utf8 workdir"), sink)
        .build()
        .await
        .expect("bootstrap should succeed");
    Arc::clone(&result.engine.current_tool_context().vfs)
}

/// The trusted-local profile is the gap this closes. It installs no jail, so
/// before this change `Write` reached `RealFs` and a hook or a project skill
/// was writable — arbitrary code execution on the operator's next commit, and
/// arbitrary instruction injection into their next session.
#[tokio::test]
async fn trusted_session_cannot_write_the_repository_control_surface() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    populate(&root);
    let vfs = tool_vfs_for(trusted_config(), &root).await;

    assert!(
        vfs.write(&root.join(".git/hooks/pre-commit"), b"#!/bin/sh\nid\n")
            .await
            .is_err(),
        "a git hook must not be writable by the file tools"
    );
    assert!(
        vfs.write(&root.join(".git/config"), b"[core]\n")
            .await
            .is_err(),
        "git config reaches execution through core.fsmonitor / filter.* and must not be writable"
    );
    assert!(
        vfs.write(&root.join(".wayland-core/skills/demo/SKILL.md"), b"# owned\n")
            .await
            .is_err(),
        "a project skill is the executable surface the trust fingerprint binds; it must not be \
         writable by the tool that the fingerprint is supposed to certify"
    );
    assert!(
        vfs.remove_file(&root.join(".wayland-core/skills/demo/SKILL.md"))
            .await
            .is_err(),
        "deleting the control surface is a mutation too"
    );

    // Positive controls. Without these the test would pass just as well
    // against a VFS that refused everything.
    vfs.write(&root.join("src/main.rs"), b"fn main() {}\n")
        .await
        .expect("an ordinary project file must still be writable");
    assert_eq!(
        vfs.read(&root.join(".git/HEAD"))
            .await
            .expect("reading .git must still work"),
        b"ref: refs/heads/main\n",
        "the deny is on WRITES only; reading git state is ordinary session work"
    );
    vfs.read(&root.join(".wayland-core/skills/demo/SKILL.md"))
        .await
        .expect("reading a project skill must still work");
}

/// The strict profile already denied `.git/config` and `.git/hooks/` via the
/// secret-suffix list, so `.wayland-core` is what is new here — and the
/// assertions are identical to the trusted case on purpose: which profile a
/// session lands in must not decide whether its own skills are rewritable.
#[tokio::test]
async fn contained_session_cannot_write_the_repository_control_surface() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    populate(&root);
    // No trust grant => strict/contained profile.
    let vfs = tool_vfs_for(minimal_config(), &root).await;

    assert!(
        vfs.write(&root.join(".wayland-core/skills/demo/SKILL.md"), b"# owned\n")
            .await
            .is_err(),
        "a project skill must not be writable in the strict profile either"
    );
    assert!(
        vfs.write(&root.join(".git/hooks/pre-commit"), b"#!/bin/sh\nid\n")
            .await
            .is_err(),
        "a git hook must not be writable in the strict profile"
    );
    vfs.write(&root.join("src/main.rs"), b"fn main() {}\n")
        .await
        .expect("an ordinary project file must still be writable");
}

/// `[security] require_vcs_for_writes` OFF (the shipped default) must leave an
/// unversioned trusted workspace exactly as it was. This is the assertion that
/// makes the change safe to ship, so it is graded before the opt-in behaviour.
#[tokio::test]
async fn unversioned_workspace_keeps_the_trusted_profile_by_default() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    let sink = Arc::new(RecordingSink::default());

    let result = AgentBootstrap::new(
        trusted_config(),
        root.to_str().expect("utf8 workdir"),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
    )
    .build()
    .await
    .expect("bootstrap should succeed");

    assert_eq!(
        result
            .engine
            .tools()
            .workspace_policy()
            .expect("policy")
            .trust(),
        wcore_tools::workspace_policy::WorkspaceTrust::Trusted,
        "the default must not change the profile of an existing trusted workspace"
    );
    assert!(
        !sink
            .infos()
            .iter()
            .any(|m| m.contains("not under version control")),
        "no notice may fire while the switch is off"
    );
}

/// Opt-in behaviour: no `.git` at or above the root means no undo, so the
/// session drops to the strict profile AND says so where the operator can see
/// it. `tracing::warn!` alone would be invisible with `RUST_LOG` unset.
#[tokio::test]
async fn unversioned_workspace_downgrades_and_announces_when_required() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    std::fs::create_dir_all(root.join("src")).expect("src dir");
    let mut config = trusted_config();
    config.security.require_vcs_for_writes = true;
    let sink = Arc::new(RecordingSink::default());

    let result = AgentBootstrap::new(
        config,
        root.to_str().expect("utf8 workdir"),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
    )
    .build()
    .await
    .expect("bootstrap should succeed");

    assert_eq!(
        result
            .engine
            .tools()
            .workspace_policy()
            .expect("policy")
            .trust(),
        wcore_tools::workspace_policy::WorkspaceTrust::Contained,
        "an unversioned workspace has no undo and must not get the trusted profile"
    );
    let infos = sink.infos();
    assert!(
        infos.iter().any(|m| m.contains("not under version control")),
        "the downgrade must be announced on the user-visible channel; got {infos:?}"
    );
}

/// The same switch, on a workspace that IS a repository, must change nothing —
/// otherwise the check would be a blanket downgrade wearing a VCS costume.
#[tokio::test]
async fn versioned_workspace_keeps_the_trusted_profile_when_required() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    populate(&root);
    let mut config = trusted_config();
    config.security.require_vcs_for_writes = true;
    let sink = Arc::new(RecordingSink::default());

    let result = AgentBootstrap::new(
        config,
        root.to_str().expect("utf8 workdir"),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
    )
    .build()
    .await
    .expect("bootstrap should succeed");

    assert_eq!(
        result
            .engine
            .tools()
            .workspace_policy()
            .expect("policy")
            .trust(),
        wcore_tools::workspace_policy::WorkspaceTrust::Trusted,
        "a real checkout has an undo and keeps its granted profile"
    );
    assert!(
        !sink
            .infos()
            .iter()
            .any(|m| m.contains("not under version control")),
        "no notice may fire for a versioned workspace"
    );
}

/// A subdirectory of a repository is still version-controlled. Checked because
/// the naive spelling (`root.join(".git").exists()`) would downgrade every
/// crate directory in a workspace and make the switch unusable.
#[tokio::test]
async fn subdirectory_of_a_repository_counts_as_versioned() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let root = std::fs::canonicalize(workdir.path()).expect("canonical workdir");
    populate(&root);
    let nested = root.join("crates/component");
    std::fs::create_dir_all(&nested).expect("nested dir");
    let mut config = trusted_config();
    config.security.require_vcs_for_writes = true;
    let sink = Arc::new(RecordingSink::default());

    let result = AgentBootstrap::new(
        config,
        nested.to_str().expect("utf8 workdir"),
        Arc::clone(&sink) as Arc<dyn OutputSink>,
    )
    .build()
    .await
    .expect("bootstrap should succeed");

    assert_eq!(
        result
            .engine
            .tools()
            .workspace_policy()
            .expect("policy")
            .trust(),
        wcore_tools::workspace_policy::WorkspaceTrust::Trusted,
        "a subdirectory of a checkout is version-controlled through its ancestors"
    );
}
