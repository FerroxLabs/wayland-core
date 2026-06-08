//! W8b.2.B Task 7: verify `AgentBootstrap::build` mounts a `FileWatcher`
//! on the engine and threads a matching `FileWriteNotifier` into the
//! orchestration ToolContext path. Closes the W8b.2.A "MINOR" wiring item.
//!
//! Strategy: drive the production bootstrap path (not `build_for_test`)
//! against a tempdir workspace, then assert:
//!   1. `engine.file_watcher().is_some()` — watcher was constructed.
//!   2. `engine.current_tool_context().file_write_notifier.is_some()` —
//!      the dispatcher will mint per-call ctxs that carry the notifier.
//!
//! `notify-rs` may have platform-dependent registration latency; the test
//! does NOT exercise the runtime debounce path here — those invariants
//! are covered by `file_watcher_test.rs` (D.2) and
//! `file_watcher_notifier.rs` inline tests (W8b.2.A). Task 7 only pins
//! the bootstrap wiring contract.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

fn bootstrap_config() -> Config {
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

#[tokio::test]
async fn bootstrap_mounts_file_watcher_and_notifier_on_realfs_workspace() {
    let tmp = tempdir().expect("tempdir");
    let workspace = tmp.path().to_str().expect("tempdir path utf-8").to_string();

    let sink: Arc<dyn OutputSink> = Arc::new(NullSink);
    let bootstrap = AgentBootstrap::new(bootstrap_config(), workspace, sink);

    let result = bootstrap.build().await.expect("bootstrap should succeed");

    // The watcher arms on a detached "eventual install" thread (FileWatcher::new
    // does a recursive watch-add that can block on a busy host), so boot does
    // NOT wait for it — there is no grace window and nothing is built-then-
    // dropped. Poll for the install rather than asserting synchronously: a
    // healthy host installs in well under a second; the generous deadline only
    // ever elapses on a genuinely wedged FS-events backend, not under heavy
    // parallel-test load.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if result.engine.file_watcher().is_some() && result.engine.tool_write_notifier().is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }

    // 1. Engine has a watcher.
    assert!(
        result.engine.file_watcher().is_some(),
        "AgentBootstrap::build should eventually mount a FileWatcher on a real-fs workspace"
    );

    // 2. Engine carries a notifier the orchestration dispatcher will
    //    propagate into each per-call ToolContext.
    assert!(
        result.engine.tool_write_notifier().is_some(),
        "AgentBootstrap::build should eventually attach a FileWriteNotifier to the engine"
    );

    // 3. The synthesised ToolContext snapshot mirrors the dispatcher's
    //    construction and carries the notifier through. This is what
    //    Write/Edit tools observe at execute_with_ctx time.
    let ctx = result.engine.current_tool_context();
    assert!(
        ctx.file_write_notifier.is_some(),
        "current_tool_context() should expose the FileWriteNotifier so \
         Write/Edit tools observe it at dispatch time"
    );
}
