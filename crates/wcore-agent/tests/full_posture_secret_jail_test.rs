//! GH#667 — a `Full`-posture channel engine (i.e. a REMOTE sender on
//! Slack/Discord/…) must not be able to read the project's own committed
//! secrets through the file tools, proven through the REAL production
//! bootstrap.
//!
//! `Full` posture is deliberately unconfined for ordinary project files, so
//! `crates/wcore-agent/src/channel_tools.rs::apply_posture` installs NO vfs for
//! it (the `if scope.posture == Workspace` arm at channel_tools.rs:175 is the
//! only place that does) and NO workspace policy. The secret jail for `Full`
//! therefore comes from ONE place: the `registry.workspace_policy().is_none()`
//! block in `crates/wcore-agent/src/bootstrap.rs` (~2961-2985), where
//! `is_channel_remote` forces `strict_workspace`, which selects
//! `WorkspacePolicy::contained()` and installs
//! `SandboxedFs::new(SecretDenyFs::new(RealFs, policy), workspace)` as the tool
//! vfs at bootstrap.rs:2974-2982.
//!
//! HOW THIS FAILS IF THE DEFECT RETURNS. Delete the `SecretDenyFs` wrapper at
//! `crates/wcore-agent/src/bootstrap.rs:2975-2981` — i.e. leave
//! `SandboxedFs::new(RealFs, workspace)` — or delete the whole
//! `if strict_workspace { … set_tool_vfs(jail) }` block at bootstrap.rs:2974-2983.
//! Either edit compiles clean and leaves every posture/toolset assertion in the
//! suite green, but the `Read` dispatched below then returns the secret's
//! contents instead of the `SecretDenyFs` refusal, reddening
//! `full_channel_posture_read_refuses_dotenv` and
//! `full_channel_posture_read_refuses_pem_private_key`.
//!
//! Coverage that looks adjacent but does NOT pin this line:
//!   * `channel_tool_posture_test.rs::workspace_posture_jails_filesystem_reads`
//!     drives the real bootstrap but at **Workspace** posture, where
//!     `apply_posture` (channel_tools.rs:185-186) has already set BOTH the tool
//!     vfs and the workspace policy — so `registry.workspace_policy().is_none()`
//!     at bootstrap.rs:2962 is false and the block under test never executes.
//!   * `workspace_policy_enforcement.rs::contained_jail_denies_secret_and_escape_allows_source`
//!     hand-builds `SandboxedFs∘SecretDenyFs` (line 12) and never calls
//!     `AgentBootstrap`. It proves the mechanism, not the wiring.
//!   * `script_registry_posture_test.rs::full_channel_posture_keeps_read_dispatchable_inside_script`
//!     reads a NON-secret file inside the root, which succeeds with or without
//!     the `SecretDenyFs` layer.
//!
//! `Grep`/`Glob` are not usable as the probe here: `FULL_CHANNEL_DENY`
//! (channel_tools.rs:113) drops both from `Full` channel posture, so `Read` is
//! the tool actually present on the wire.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::{AgentBootstrap, BootstrapResult};
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_types::tool::ToolResult;

/// Dead URL: `build()` never connects, these tests only dispatch tools on the
/// resulting engine.
fn minimal_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

/// Build a real engine the way `ChannelTurnDispatcher` builds a per-session one
/// for a remote sender at `Full` posture, rooted at `root`. Returns the whole
/// `BootstrapResult` so the session's other handles stay alive for the test.
async fn full_channel_session(root: &std::path::Path) -> BootstrapResult {
    AgentBootstrap::new(
        minimal_config(),
        root.to_str().expect("utf-8 tempdir").to_string(),
        null_output(),
    )
    .without_channels(true)
    .channel_tool_posture(ChannelToolScope {
        posture: ChannelToolPosture::Full,
        workspace_root: root.to_path_buf(),
    })
    .build()
    .await
    .expect("bootstrap")
}

/// Dispatch the engine's registered `Read` through the same `ToolContext`
/// production dispatch mints — so the read goes through `ctx.vfs`, i.e. through
/// whatever the bootstrap actually installed.
async fn read_through_tools(engine: &AgentEngine, path: &std::path::Path) -> ToolResult {
    let registry = engine.tools();
    let read = registry
        .get("Read")
        .expect("Full channel posture keeps Read (FULL_CHANNEL_DENY drops only Grep/Glob/Git)");
    let ctx = engine.current_tool_context();
    read.execute_with_ctx(
        serde_json::json!({ "file_path": path.to_str().unwrap() }),
        &ctx,
    )
    .await
}

/// The exact refusal `SecretDenyFs` produces. `VfsError::SecretDenied` is
/// declared at `crates/wcore-tools/src/vfs.rs:56` as
/// `#[error("refused: {path:?} is a protected secret path")]` and is the ONLY
/// producer of this string; `ReadTool::execute_with_ctx` wraps it as
/// `format!("Failed to read file {file_path}: {e}")`
/// (`crates/wcore-tools/src/read.rs:405-413`). Asserting the whole shape means a
/// plain IO error, a NotFound, or a jail `OutsideSandbox` cannot masquerade as a
/// secret refusal.
fn assert_secret_refusal(result: &ToolResult, path: &std::path::Path, plaintext: &str) {
    assert!(
        result.is_error,
        "Full channel posture must refuse to Read {}, got: {}",
        path.display(),
        result.content
    );
    assert!(
        result.content.starts_with("Failed to read file "),
        "refusal must come from ReadTool's vfs error arm, got: {}",
        result.content
    );
    assert!(
        result.content.contains("refused: ")
            && result.content.contains("is a protected secret path"),
        "refusal must be VfsError::SecretDenied, not an unrelated IO error, got: {}",
        result.content
    );
    assert!(
        !result.content.contains(plaintext),
        "the secret's contents must never reach the channel reply, got: {}",
        result.content
    );
}

/// #667 core case: `.env` in the workspace root.
#[tokio::test]
async fn full_channel_posture_read_refuses_dotenv() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let secret = root.join(".env");
    std::fs::write(&secret, b"API_KEY=SUPER_SECRET_TOKEN\n").unwrap();

    let session = full_channel_session(&root).await;
    let out = read_through_tools(&session.engine, &secret).await;
    assert_secret_refusal(&out, &secret, "SUPER_SECRET_TOKEN");
}

/// Second secret SHAPE, matched by a different `is_secret_path_static` rule
/// (`SECRET_EXTENSIONS` at `crates/wcore-tools/src/workspace_policy.rs:42`
/// rather than the `/.env` suffix rule at :26). Pins that the deny is the real
/// policy predicate and not a one-off `.env` special case.
#[tokio::test]
async fn full_channel_posture_read_refuses_pem_private_key() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let secret = root.join("deploy.pem");
    // Deliberately NOT shaped like a real PEM: the deny predicate keys on the
    // PATH (`is_secret_path_static`), never on the bytes, and the commit ratchet
    // correctly refuses key-shaped fixtures. The marker below is what the
    // refusal assertion proves never leaked.
    std::fs::write(&secret, b"deploy-pem-fixture SUPER_SECRET_TOKEN\n").unwrap();

    let session = full_channel_session(&root).await;
    let out = read_through_tools(&session.engine, &secret).await;
    assert_secret_refusal(&out, &secret, "SUPER_SECRET_TOKEN");
}

/// NEGATIVE CONTROL. `Full` posture is unconfined for ordinary project files —
/// the jail is secret-scoped, not a blanket read deny. Without this, both
/// assertions above would still pass on an engine whose `Read` refuses
/// EVERYTHING (a broken vfs, a wrong jail root, an empty registry), and the pair
/// would prove nothing about secrets specifically.
///
/// An over-broad deny cannot satisfy this test: it asserts the tool returned the
/// file's actual CONTENT (`is_error == false` plus the body text), which only a
/// successful read through `ctx.vfs` can produce.
#[tokio::test]
async fn full_channel_posture_still_reads_non_secret_workspace_file() {
    let tmp = tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let ordinary = root.join("README.md");
    std::fs::write(&ordinary, b"ordinary-project-content\n").unwrap();

    let session = full_channel_session(&root).await;
    let out = read_through_tools(&session.engine, &ordinary).await;
    assert!(
        !out.is_error,
        "a non-secret file inside the workspace must stay readable at Full posture, got: {}",
        out.content
    );
    assert!(
        out.content.contains("ordinary-project-content"),
        "the read must return the file's real content, got: {}",
        out.content
    );
}
