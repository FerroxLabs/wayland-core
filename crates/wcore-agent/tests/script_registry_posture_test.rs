//! GH#847 — the ScriptTool dispatcher mini-registry must obey the channel tool
//! posture, proven through the REAL production bootstrap.
//!
//! `AgentBootstrap::build` gives `ScriptTool` its own mini-registry
//! (`build_script_dispatcher_registry`), separate from the main session
//! registry. `apply_posture` runs later, on the MAIN registry only, and never
//! sees the mini-registry. Without an explicit second application, a
//! channel-remote `Full` engine that has Script enabled hands a remote sender
//! `Script([{tool: Grep}])` / `Script([{tool: Glob}])` — a live re-open of the
//! exfiltration vector #232 (unconfined recursive search reads project secrets
//! past `SecretDenyFs`) and MF1 closed on the main registry. `Grep`/`Glob` are
//! both on `ScriptTool::ALLOW_LIST`, so the Script layer does not stop them;
//! the registry drop is the only thing that does.
//!
//! Existing coverage did not pin this. `channel_tool_posture_test.rs` asserts
//! against `engine.tool_names()` — the MAIN registry — which stays correct even
//! with the mini-registry unguarded, and the `bootstrap.rs` unit test
//! `script_dispatcher_registry_obeys_full_channel_posture` calls
//! `apply_posture` itself, so it proves the helper works, never that the
//! production call site invokes it. These tests instead drive `build()` with a
//! channel scope set and then dispatch a real `Script` step through the
//! resulting engine, so they observe the SCRIPT DISPATCH registry specifically.
//!
//! HOW THIS FAILS IF THE DEFECT RETURNS. Delete the
//! `if let Some(scope) = self.channel_tool_posture.as_ref() { ... }` block that
//! calls `crate::channel_tools::apply_posture(&mut dispatch_reg, scope, ...)` in
//! `crates/wcore-agent/src/bootstrap.rs` (the block immediately after
//! `build_script_dispatcher_registry(...)` inside
//! `if self.config.builtin_tools.script.enabled`) and the mini-registry keeps
//! its `GrepTool`/`GlobTool` registrations. The `ClosureDispatcher` then finds
//! them, `Script` executes the search for real, and the returned `ToolResult`
//! no longer carries the dispatcher's `tool '<name>' not in registry` miss —
//! reddening `full_channel_posture_drops_grep_from_script_dispatch_registry`
//! and `full_channel_posture_drops_glob_from_script_dispatch_registry`. Only
//! that production block can produce the asserted string for `Grep`/`Glob`,
//! because `build_script_dispatcher_registry` registers both unconditionally.

use std::sync::Arc;

use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::channel_tools::ChannelToolScope;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_channels::ChannelToolPosture;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_tools::Tool;
use wcore_types::tool::ToolResult;

/// Script enabled (default off) on an otherwise inert config: `build()` never
/// connects, the tests only dispatch tools on the resulting engine.
fn script_enabled_config() -> Config {
    let mut config = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    config.builtin_tools.script.enabled = true;
    config
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

/// Dispatch a single-step `Script` call through the engine's registered
/// `ScriptTool` — i.e. through the mini-registry the production bootstrap built
/// — using the same `ToolContext` production dispatch would mint.
async fn run_one_script_step(
    engine: &AgentEngine,
    tool: &str,
    input: serde_json::Value,
) -> ToolResult {
    let registry = engine.tools();
    let script = registry
        .get("Script")
        .expect("Script is registered when builtin_tools.script.enabled and the posture keeps it");
    let ctx = engine.current_tool_context();
    script
        .execute_with_ctx(
            serde_json::json!({
                "steps": [{ "id": "s1", "tool": tool, "input": input }]
            }),
            &ctx,
        )
        .await
}

/// The registry-miss string `ClosureDispatcher` returns from the bootstrap's
/// script dispatcher closure when the named tool is absent. Reaching it proves
/// the tool was dropped from the SCRIPT DISPATCH registry, not merely hidden
/// from the LLM schema or blocked by the Script allow-list (which permits both
/// `Grep` and `Glob`).
fn assert_not_in_script_registry(result: &ToolResult, tool: &str) {
    assert!(
        result.is_error,
        "Script step calling '{tool}' must fail under Full channel posture, got: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains(&format!("tool '{tool}' not in registry")),
        "Script's dispatch of '{tool}' must miss the mini-registry (posture-dropped), got: {}",
        result.content
    );
}

#[tokio::test]
async fn full_channel_posture_drops_grep_from_script_dispatch_registry() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();

    let result = AgentBootstrap::new(script_enabled_config(), ws.clone(), null_output())
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Full,
            workspace_root: tmp.path().to_path_buf(),
        })
        .build()
        .await
        .expect("bootstrap");

    // Precondition: the vector only exists because Script survives Full.
    assert!(
        result.engine.tool_names().iter().any(|n| n == "Script"),
        "Full channel posture keeps Script, so the mini-registry is reachable"
    );

    let out = run_one_script_step(
        &result.engine,
        "Grep",
        serde_json::json!({ "pattern": "SECRET", "path": ws }),
    )
    .await;
    assert_not_in_script_registry(&out, "Grep");
}

#[tokio::test]
async fn full_channel_posture_drops_glob_from_script_dispatch_registry() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();

    let result = AgentBootstrap::new(script_enabled_config(), ws.clone(), null_output())
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Full,
            workspace_root: tmp.path().to_path_buf(),
        })
        .build()
        .await
        .expect("bootstrap");

    let out = run_one_script_step(
        &result.engine,
        "Glob",
        serde_json::json!({ "pattern": "**/*.pem", "path": ws }),
    )
    .await;
    assert_not_in_script_registry(&out, "Glob");
}

/// Counterpart guard: the posture retain must be surgical, not a wipe. A tool
/// Full keeps (`Read`) must still dispatch inside Script on the same
/// channel-remote engine — otherwise the two assertions above could pass
/// against an empty mini-registry and prove nothing about the posture.
#[tokio::test]
async fn full_channel_posture_keeps_read_dispatchable_inside_script() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();
    let file = tmp.path().join("kept.txt");
    std::fs::write(&file, b"dispatchable\n").unwrap();

    let result = AgentBootstrap::new(script_enabled_config(), ws, null_output())
        .without_channels(true)
        .channel_tool_posture(ChannelToolScope {
            posture: ChannelToolPosture::Full,
            workspace_root: tmp.path().to_path_buf(),
        })
        .build()
        .await
        .expect("bootstrap");

    let out = run_one_script_step(
        &result.engine,
        "Read",
        serde_json::json!({ "file_path": file.to_str().unwrap() }),
    )
    .await;
    assert!(
        !out.content.contains("not in registry"),
        "Full posture must keep Read in the Script mini-registry, got: {}",
        out.content
    );
    assert!(
        !out.is_error,
        "Read inside Script must succeed on a Full channel-remote engine, got: {}",
        out.content
    );
}

/// Counterpart guard: a LOCAL engine has no channel scope, so the production
/// block never runs and local Script keeps its full toolset. This pins that the
/// registry miss the tests above assert is caused BY the posture — not by
/// `build_script_dispatcher_registry` having stopped registering `Grep` at all,
/// which would make those assertions pass for the wrong reason.
///
/// Deliberately asserts only on the absence of the registry-miss string: whether
/// the real `Grep` finds a match (or errors because no `rg`/`grep` binary is
/// installed on the runner) is environment-dependent and not what is under test.
#[tokio::test]
async fn local_engine_without_posture_keeps_grep_in_script_dispatch_registry() {
    let tmp = tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap().to_string();
    std::fs::write(tmp.path().join("hay.txt"), b"needle\n").unwrap();

    let result = AgentBootstrap::new(script_enabled_config(), ws.clone(), null_output())
        .without_channels(true)
        .build()
        .await
        .expect("bootstrap");

    let out = run_one_script_step(
        &result.engine,
        "Grep",
        serde_json::json!({ "pattern": "needle", "path": ws }),
    )
    .await;
    assert!(
        !out.content.contains("not in registry"),
        "a local (unscoped) engine must keep Grep in the Script mini-registry, got: {}",
        out.content
    );
}
