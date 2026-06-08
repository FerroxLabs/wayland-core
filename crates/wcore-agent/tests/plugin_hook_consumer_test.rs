//! Task 1.3 — `PluginHook` type + `HookEngine` consumer + `AgentEngine` setter.
//!
//! Tests:
//! 1. `has_hooks` reflects plugin hooks.
//! 2. A `PluginHook` at `PreToolUse` fires (log line appears) inside
//!    `run_pre_tool_use`.
//! 3. `AgentEngine::register_plugin_hooks` forwards hooks into the engine's
//!    `HookEngine`.
//! 4. Phases without a current entrypoint (`SessionStart`, `PrePrompt`,
//!    `PreCompact`) are stored and visible via `plugin_hooks()`.
//! 5. The smoke runner path populates `InitializeOutcome.hooks` as
//!    `Vec<PluginHook>` with the originating plugin name set correctly.

use wcore_agent::hooks::HookEngine;
use wcore_agent::plugins::runner::PluginHook;
use wcore_config::hooks::HooksConfig;
use wcore_plugin_api::registry::hooks::HookPhase;

// ── helpers ──────────────────────────────────────────────────────────────────

fn empty_engine() -> HookEngine {
    HookEngine::new(HooksConfig::default())
}

fn pre_tool_hook(plugin: &str, name: &str) -> PluginHook {
    PluginHook {
        plugin: plugin.to_string(),
        phase: HookPhase::PreToolUse,
        name: name.to_string(),
    }
}

// ── 1. has_hooks ─────────────────────────────────────────────────────────────

#[test]
fn has_hooks_false_when_no_hooks() {
    let engine = empty_engine();
    assert!(!engine.has_hooks());
}

#[test]
fn has_hooks_true_after_register_plugin_hook() {
    let mut engine = empty_engine();
    engine.register_plugin_hook(pre_tool_hook("test-plugin", "my-hook"));
    assert!(engine.has_hooks());
}

// ── 2. PreToolUse fires and appears in hook_trace ─────────────────────────────
// v0.9.1.2 F10: plugin-hook fire lines moved from `log_lines` (which feeds the
// TUI transcript) to `hook_trace` (telemetry only — routed to tracing::debug!).

#[tokio::test]
async fn pre_tool_use_plugin_hook_fires_log_line() {
    let mut engine = empty_engine();
    engine.register_plugin_hook(pre_tool_hook("my-plugin", "guard"));

    let outcome = engine
        .run_pre_tool_use("bash", &serde_json::json!({"cmd": "ls"}))
        .await
        .expect("run_pre_tool_use must not error");

    // The hook must produce a trace line containing the plugin and hook names.
    let found = outcome
        .hook_trace
        .iter()
        .any(|l| l.contains("my-plugin") && l.contains("guard") && l.contains("bash"));
    assert!(
        found,
        "expected a pre_tool_use hook_trace line mentioning the plugin, hook, and tool name; got: {:?}",
        outcome.hook_trace
    );
    // And it must NOT appear in log_lines — those go to the user-facing transcript.
    assert!(
        outcome.log_lines.is_empty(),
        "plugin-hook fire lines must not leak into log_lines: {:?}",
        outcome.log_lines
    );
}

#[tokio::test]
async fn pre_tool_use_fires_only_pre_tool_use_phase_hooks() {
    let mut engine = empty_engine();
    // Register one hook at each of the four non-firing phases.
    engine.register_plugin_hook(PluginHook {
        plugin: "p".into(),
        phase: HookPhase::TurnStart,
        name: "ts".into(),
    });
    engine.register_plugin_hook(PluginHook {
        plugin: "p".into(),
        phase: HookPhase::PrePrompt,
        name: "pp".into(),
    });
    // And one at PreToolUse.
    engine.register_plugin_hook(pre_tool_hook("p", "target"));

    let outcome = engine
        .run_pre_tool_use("echo", &serde_json::Value::Null)
        .await
        .expect("ok");

    // Only the PreToolUse hook should emit a line — into hook_trace (F10).
    assert_eq!(
        outcome.hook_trace.len(),
        1,
        "expected exactly 1 hook_trace line; got: {:?}",
        outcome.hook_trace
    );
    assert!(outcome.hook_trace[0].contains("target"));
    // log_lines stays empty — plugin-hook fire lines are telemetry, not transcript.
    assert!(outcome.log_lines.is_empty());
}

// ── 3. AgentEngine::register_plugin_hooks ─────────────────────────────────────

#[tokio::test]
async fn agent_engine_register_plugin_hooks_wires_into_hook_engine() {
    use std::sync::Arc;
    use wcore_agent::engine::AgentEngine;
    use wcore_agent::output::terminal::TerminalSink;
    use wcore_config::config::Config;
    use wcore_tools::registry::ToolRegistry;

    let cfg = Config::default();
    let output = Arc::new(TerminalSink::new(true));
    let mut engine = AgentEngine::new(cfg, ToolRegistry::new(), output);

    // Engine starts with no plugin hooks.
    assert!(
        !engine
            .hook_engine()
            .map(|h| !h.plugin_hooks().is_empty())
            .unwrap_or(false)
    );

    engine.register_plugin_hooks(vec![pre_tool_hook("wired-plugin", "wired-hook")]);

    // After registration the hook engine should have one plugin hook.
    let he = engine.hook_engine().expect("hook engine present");
    assert_eq!(he.plugin_hooks().len(), 1);
    assert_eq!(he.plugin_hooks()[0].name, "wired-hook");
    assert_eq!(he.plugin_hooks()[0].plugin, "wired-plugin");
    assert!(matches!(he.plugin_hooks()[0].phase, HookPhase::PreToolUse));

    // And has_hooks reflects it.
    assert!(he.has_hooks());
}

// ── 4. Stored-but-not-fired phases are accessible ────────────────────────────

#[test]
fn stored_but_not_fired_phases_are_stored() {
    let mut engine = empty_engine();
    for phase in [
        HookPhase::SessionStart,
        HookPhase::PrePrompt,
        HookPhase::PreCompact,
    ] {
        engine.register_plugin_hook(PluginHook {
            plugin: "p".into(),
            phase,
            name: "h".into(),
        });
    }
    // All three are stored.
    assert_eq!(engine.plugin_hooks().len(), 3);
    // has_hooks is true.
    assert!(engine.has_hooks());
}

// ── 3b. register_plugin_hooks on an engine with no existing plugin hooks ──────

#[tokio::test]
async fn register_plugin_hooks_on_empty_engine_does_not_panic() {
    use std::sync::Arc;
    use wcore_agent::engine::AgentEngine;
    use wcore_agent::output::terminal::TerminalSink;
    use wcore_config::config::Config;
    use wcore_tools::registry::ToolRegistry;

    let cfg = Config::default();
    let output = Arc::new(TerminalSink::new(true));
    let mut engine = AgentEngine::new(cfg, ToolRegistry::new(), output);
    // Calling with an empty vec on a fresh engine (hooks: Some, no plugin hooks yet) must not panic.
    engine.register_plugin_hooks(vec![]);
}

// ── 3c. All four currently-untested firing entrypoints get coverage ───────────

#[tokio::test]
async fn all_firing_phases_emit_log_lines() {
    let mut engine = empty_engine();

    // Register one hook at each of the four phases that have firing entrypoints.
    for (phase, name) in [
        (HookPhase::PostToolUse, "post-hook"),
        (HookPhase::TurnStart, "ts-hook"),
        (HookPhase::TurnEnd, "te-hook"),
        (HookPhase::SessionEnd, "se-hook"),
    ] {
        engine.register_plugin_hook(PluginHook {
            plugin: "multi-phase-plugin".into(),
            phase,
            name: name.into(),
        });
    }

    // v0.9.1.2 F10: plugin-hook fire lines now flow into `hook_trace` (telemetry)
    // rather than `log_lines` (transcript). Tests assert on hook_trace.

    // PostToolUse fires.
    let post = engine
        .run_post_tool_use("write", "call-1", &serde_json::Value::Null, "ok", false)
        .await;
    assert!(
        post.hook_trace
            .iter()
            .any(|l| l.contains("multi-phase-plugin") && l.contains("post-hook")),
        "expected PostToolUse hook_trace line; got: {:?}",
        post.hook_trace
    );

    // TurnStart fires.
    use wcore_agent::hooks::TurnContext;
    let ts = engine
        .on_turn_start(
            1,
            &TurnContext {
                turn: 1,
                model: "test".into(),
                message_count: 0,
            },
        )
        .await;
    assert!(
        ts.hook_trace
            .iter()
            .any(|l| l.contains("multi-phase-plugin") && l.contains("ts-hook")),
        "expected TurnStart hook_trace line; got: {:?}",
        ts.hook_trace
    );

    // TurnEnd fires.
    use wcore_agent::hooks::TurnResult;
    let te = engine
        .on_turn_end(
            1,
            &TurnResult {
                turn: 1,
                tool_call_count: 0,
                input_tokens: 0,
                output_tokens: 0,
            },
        )
        .await;
    assert!(
        te.hook_trace
            .iter()
            .any(|l| l.contains("multi-phase-plugin") && l.contains("te-hook")),
        "expected TurnEnd hook_trace line; got: {:?}",
        te.hook_trace
    );

    // SessionEnd fires.
    use wcore_agent::hooks::SessionEndSummary;
    let se = engine
        .on_session_end(&SessionEndSummary {
            turns: 3,
            total_input_tokens: 0,
            total_output_tokens: 0,
        })
        .await;
    assert!(
        se.hook_trace
            .iter()
            .any(|l| l.contains("multi-phase-plugin") && l.contains("se-hook")),
        "expected SessionEnd hook_trace line; got: {:?}",
        se.hook_trace
    );
}

// ── 5. InitializeOutcome.hooks is Vec<PluginHook> with plugin name set ────────

#[tokio::test]
async fn initialize_all_populates_plugin_hook_with_plugin_name() {
    use wcore_agent::plugins::{PluginLoader, PluginRunner};
    use wcore_config::plugins_config::PluginsConfig;
    use wcore_plugin_api::{
        Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult,
        registry::hooks::HookPhase,
    };

    struct HookPlugin;

    static HOOK_MANIFEST_TOML: &str = r#"
[plugin]
name = "wayland-hook-fixture"
version = "0.0.1"
description = "registers a hook for Task 1.3 test"
entry = "builtin:hook-fixture"
authors = ["t"]
license = "MIT"
[permissions]
register_hooks = true
"#;

    fn hook_manifest() -> &'static PluginManifest {
        static M: std::sync::OnceLock<PluginManifest> = std::sync::OnceLock::new();
        M.get_or_init(|| PluginManifest::from_toml_str(HOOK_MANIFEST_TOML).unwrap())
    }

    #[async_trait::async_trait]
    impl Plugin for HookPlugin {
        fn manifest(&self) -> &PluginManifest {
            hook_manifest()
        }
        async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
            ctx.hooks
                .as_mut()
                .unwrap()
                .register_hook(HookPhase::PreToolUse, "fixture-pre-tool".into())?;
            Ok(())
        }
    }

    struct HookFactory;
    impl PluginFactory for HookFactory {
        fn name(&self) -> &'static str {
            "wayland-hook-fixture"
        }
        fn build(&self) -> Box<dyn Plugin> {
            Box::new(HookPlugin)
        }
    }
    inventory::submit! { &HookFactory as &dyn wcore_plugin_api::PluginFactory }

    let cfg = PluginsConfig::default();
    let mut loader = PluginLoader::discover(&cfg);
    let captured = loader.validate_all().expect("validate");
    let mut runner = PluginRunner::new();
    let outcome = runner.initialize_all(&captured).await.expect("init ok");

    let hook = outcome
        .hooks
        .iter()
        .find(|h| h.name == "fixture-pre-tool")
        .expect("fixture hook must be in InitializeOutcome.hooks");

    assert_eq!(hook.plugin, "wayland-hook-fixture");
    assert!(matches!(hook.phase, HookPhase::PreToolUse));
}

// ── v0.9.1.2 F10 regression: plugin-hook fire lines stay out of log_lines ────

/// Synthesizes a `PostToolUse` plugin hook fire and asserts the resulting
/// HookOutcome contains the lifecycle line in `hook_trace` only — never in
/// `log_lines`. `log_lines` is the channel the engine + orchestration layers
/// surface as transcript content; `hook_trace` is the telemetry-only channel
/// that drain sites route to `tracing::debug!`.
///
/// Sean's screenshots #78 + #79 showed
/// `[plugin-hook:wayland-ijfw:ijfw_observation_capture] post_tool_use fired
/// for tool "web"` landing in the transcript on every tool call. The F2 fix
/// (v0.9.1.1) added a filter at engine drain points but missed the
/// orchestration-layer drain that ran INSIDE the tool execution loop. This
/// test asserts the architectural fix: lifecycle lines never enter the
/// transcript channel in the first place.
#[tokio::test]
async fn post_tool_use_hook_does_not_leak_to_transcript_v0912() {
    let mut engine = empty_engine();
    engine.register_plugin_hook(PluginHook {
        plugin: "wayland-ijfw".into(),
        phase: HookPhase::PostToolUse,
        name: "ijfw_observation_capture".into(),
    });

    let outcome = engine
        .run_post_tool_use("web", "call-1", &serde_json::Value::Null, "ok", false)
        .await;

    // The fire line MUST exist in hook_trace so /doctor and log files can
    // confirm the hook ran.
    assert!(
        outcome
            .hook_trace
            .iter()
            .any(|l| l.contains("ijfw_observation_capture") && l.contains("post_tool_use fired")),
        "hook fire must be recorded in hook_trace: {:?}",
        outcome.hook_trace
    );

    // It MUST NOT appear in log_lines — those flow into the transcript.
    // No `post_tool_use fired` line, no `[plugin-hook:` line, nothing.
    for line in &outcome.log_lines {
        assert!(
            !line.contains("post_tool_use fired"),
            "plugin-hook fire line leaked into log_lines (transcript channel): {line:?}"
        );
        assert!(
            !line.starts_with("[plugin-hook:"),
            "[plugin-hook:...] prefix leaked into log_lines: {line:?}"
        );
    }
}

/// Same shape as the post_tool_use case, but for `run_pre_tool_use` — the
/// orchestration-layer drain that fires INSIDE the per-tool-use loop. v0.9.1.1
/// F2 didn't cover this drain at all; v0.9.1.2 F10 routes the fire line
/// straight to `hook_trace`, removing the need for filter logic at the drain
/// site (though the belt-and-suspenders filter on `log_lines` stays too).
#[tokio::test]
async fn pre_tool_use_per_tool_drain_filters_hook_lines_v0912() {
    let mut engine = empty_engine();
    engine.register_plugin_hook(PluginHook {
        plugin: "wayland-ijfw".into(),
        phase: HookPhase::PreToolUse,
        name: "ijfw_observation_capture".into(),
    });

    let outcome = engine
        .run_pre_tool_use("github_api", &serde_json::Value::Null)
        .await
        .expect("no shell hook configured, so cannot error");

    assert!(
        outcome
            .hook_trace
            .iter()
            .any(|l| l.contains("ijfw_observation_capture") && l.contains("pre_tool_use fired")),
        "hook fire must be recorded in hook_trace: {:?}",
        outcome.hook_trace
    );

    // Critical for transcript hygiene: no fire line should reach log_lines.
    assert!(
        outcome.log_lines.is_empty(),
        "pre_tool_use plugin-hook fire line MUST NOT enter log_lines (transcript channel); got: {:?}",
        outcome.log_lines
    );
}
