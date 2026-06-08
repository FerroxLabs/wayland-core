//! W8b D.1 — `SelfCorrectionHook` integration via the `HookEngine`.
//!
//! Asserts the three modes (Off / Enabled / Aggressive), the
//! classification policy, and the InjectMessage shape that flows back
//! through `run_post_tool_use`.

use serde_json::json;

use wcore_agent::hooks::{HookEngine, SelfCorrectMode, SelfCorrectionHook};
use wcore_config::hooks::HooksConfig;

fn fresh_engine_with(mode: SelfCorrectMode) -> HookEngine {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(SelfCorrectionHook::new(mode)));
    engine
}

#[tokio::test]
async fn classifies_tool_error_and_injects_correction() {
    let engine = fresh_engine_with(SelfCorrectMode::Enabled);
    let outcome = engine
        .run_post_tool_use(
            "Bash",
            "c-1",
            &json!({"command": "cargo test"}),
            "error[E0432]: could not compile foo",
            /*is_error=*/ true,
        )
        .await;

    assert_eq!(
        outcome.injected_messages.len(),
        1,
        "Enabled mode + is_error must inject exactly one correction"
    );
    let m = &outcome.injected_messages[0];
    let text: String = m
        .content
        .iter()
        .filter_map(|b| match b {
            wcore_types::message::ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        text.contains("compile") && text.contains("Bash"),
        "injected message must reference both the class and the tool name, got: {text}"
    );
}

#[tokio::test]
async fn enabled_mode_skips_when_call_succeeded() {
    let engine = fresh_engine_with(SelfCorrectMode::Enabled);
    let outcome = engine
        .run_post_tool_use(
            "Read",
            "c-2",
            &json!({"file_path": "/tmp/x"}),
            "10\thello world",
            /*is_error=*/ false,
        )
        .await;

    assert!(
        outcome.injected_messages.is_empty(),
        "Enabled mode + success must NOT inject a correction"
    );
}

#[tokio::test]
async fn off_mode_emits_nothing() {
    let engine = fresh_engine_with(SelfCorrectMode::Off);
    let outcome = engine
        .run_post_tool_use(
            "Bash",
            "c-3",
            &json!({"command": "cargo test"}),
            "anything",
            true,
        )
        .await;
    assert!(
        outcome.injected_messages.is_empty(),
        "Off mode must never inject — found: {:?}",
        outcome.injected_messages
    );
}

#[tokio::test]
async fn aggressive_mode_triggers_on_success_too() {
    let engine = fresh_engine_with(SelfCorrectMode::Aggressive);
    let outcome = engine
        .run_post_tool_use(
            "Read",
            "c-4",
            &json!({"file_path": "/tmp/ok"}),
            "10\thello world",
            /*is_error=*/ false,
        )
        .await;

    assert_eq!(
        outcome.injected_messages.len(),
        1,
        "Aggressive mode injects even on successful tool calls"
    );
}
