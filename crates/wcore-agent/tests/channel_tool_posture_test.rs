//! End-to-end proof that a channel-originated engine's tool posture is
//! enforced by the production `AgentBootstrap::build` path.
//!
//! The production gate this closes: a channel turn runs a real engine on
//! the host, so without scoping, a remote chat sender could `Read`/`Grep`
//! host secrets. These tests drive the real bootstrap (same path
//! `ChannelTurnDispatcher` uses for its per-session engines) and assert:
//!
//!   1. Default (no posture) — the local CLI engine keeps the FULL toolset
//!      (`Read` AND `Bash` present). Proves the change is opt-in and does
//!      not regress local sessions.
//!   2. `Conversational` — every host filesystem/shell tool is GONE from
//!      the registry (un-dispatchable, not merely hidden); a conversational
//!      tool survives.
//!   3. `Workspace` — `Read` is present but the engine's tool `vfs` is a
//!      `SandboxedFs` jail: a read inside the workspace root succeeds, a
//!      read outside it is refused. `Bash` (non-jailable) stays gone.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::output::null_sink::NullSink;
use wcore_agent::output::OutputSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

fn bootstrap_config() -> Config {
    // Dead URL: `build()` never connects, the tests only inspect the engine.
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

const HOST_TOOLS: &[&str] = &["Read", "Grep", "Glob", "Write", "Edit", "Bash"];

#[tokio::test]
async fn default_posture_keeps_full_toolset() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);

    let result = AgentBootstrap::new(bootstrap_config(), ws, sink)
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap");

    let names = result.engine.tool_names();
    assert!(names.iter().any(|n| n == "Read"), "local engine keeps Read");
    assert!(names.iter().any(|n| n == "Bash"), "local engine keeps Bash");
}

#[tokio::test]
async fn conversational_posture_drops_all_host_tools() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);

    let result = AgentBootstrap::new(bootstrap_config(), ws.clone(), sink)
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Conversational,
            workspace_root: ws.into(),
        })
        .build()
        .await
        .expect("bootstrap");

    let names = result.engine.tool_names();
    for host in HOST_TOOLS {
        assert!(
            !names.iter().any(|n| n == host),
            "conversational posture must drop host tool '{host}', got: {names:?}"
        );
    }
    // A conversational tool still works (registry isn't simply emptied).
    assert!(
        names.iter().any(|n| n == "todo"),
        "conversational tools survive, got: {names:?}"
    );
}

#[tokio::test]
async fn workspace_posture_jails_filesystem_reads() {
    let inside = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let inside_file = inside.path().join("inside.txt");
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&inside_file, b"in").unwrap();
    std::fs::write(&outside_file, b"out").unwrap();

    let ws = inside.path().to_str().unwrap().to_string();
    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);

    let result = AgentBootstrap::new(bootstrap_config(), ws.clone(), sink)
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Workspace,
            workspace_root: inside.path().to_path_buf(),
        })
        .build()
        .await
        .expect("bootstrap");

    let names = result.engine.tool_names();
    // Vfs-jailable fs tools come back…
    assert!(names.iter().any(|n| n == "Read"), "workspace exposes Read");
    assert!(names.iter().any(|n| n == "Edit"), "workspace exposes Edit");
    // …but non-jailable shell/exec tools stay gone — including Grep/Glob,
    // which shell out and would escape the jail with a default `path="."`.
    for gone in ["Bash", "Grep", "Glob"] {
        assert!(
            !names.iter().any(|n| n == gone),
            "workspace must NOT expose non-jailable tool '{gone}', got: {names:?}"
        );
    }

    // The engine's tool vfs is a SandboxedFs jailed to the workspace root.
    let ctx = result.engine.current_tool_context();
    assert!(
        ctx.vfs.read(&inside_file).await.is_ok(),
        "read inside the workspace root must succeed"
    );
    assert!(
        ctx.vfs.read(&outside_file).await.is_err(),
        "read OUTSIDE the workspace root must be refused by the jail"
    );
}
