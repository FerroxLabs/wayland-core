//! v0.6.5 Wave 6A.2 — `UserModelInferencer` consumes plugin-reified
//! user-model backends.
//!
//! Pins the carrier-to-consumer wiring closed by this wave: when bootstrap
//! installs a plugin-reified backend via
//! `AgentEngine::set_plugin_user_models`, the session-end PUM path must
//! mirror every inferred delta to that backend (via
//! `HonchoClient::learn_preference`) in addition to the local
//! `MemoryApi::update_user_model` write.
//!
//! The carrier was populated in v0.6.5 Task 1.5 but had no production
//! reader; this test guards against regression of that gap.

use serde_json::json;
use wayland_honcho::HonchoClient;
use wcore_agent::plugins::apply::{ReifiedUserModel, ReifiedUserModelBackend};
use wcore_agent::test_utils::e2e_fixture::E2eFixture;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

fn one_tool_call_script() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-0".into(),
            name: "Grep".into(),
            input: json!({ "pattern": "no-such-string-6A2", "path": "." }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

#[tokio::test]
async fn session_end_pum_mirrors_to_plugin_reified_honcho_backend() {
    let mut fx = E2eFixture::new()
        .with_provider_script(one_tool_call_script())
        .with_skills_lifecycle(true)
        .with_max_turns(1)
        .with_in_memory_backend()
        .build()
        .await;

    // Install a mock Honcho backend through the same setter bootstrap uses
    // when `applied.plugin_reified_user_models` is non-empty. The mock
    // exposes the recorded preferences via `recall_user`, so the test can
    // assert that the session-end PUM path wrote through it.
    let mock = HonchoClient::mock();
    fx.engine_mut()
        .set_plugin_user_models(vec![ReifiedUserModel {
            plugin: "wayland-honcho".to_string(),
            name: "honcho-mock".to_string(),
            backend: ReifiedUserModelBackend::Honcho(mock),
        }]);

    let _ = fx
        .send("trigger session end")
        .await
        .expect("synthetic run should not error");

    // Recall through a fresh client pointed at the same mock would require
    // mock-state sharing; instead, install a second handle by reaching back
    // into the engine. `plugin_user_models()` exposes the slice; the mock's
    // internal state is process-local but per-instance, so we recall via
    // the same instance held by the engine.
    // Capture the session id (if any) BEFORE borrowing the engine for the
    // reified-backend slice — `plugin_user_models()` takes `&self`.
    let session_id = fx.engine_mut().current_session_id();

    let installed = fx.engine_mut().plugin_user_models();
    assert_eq!(
        installed.len(),
        1,
        "engine should hold exactly one reified plugin user-model"
    );
    let ReifiedUserModelBackend::Honcho(client) = &installed[0].backend;

    // Engine routes under the session id when present, otherwise `"default"`.
    let user_id = session_id.as_deref().unwrap_or("default");
    let profile = client
        .recall_user(user_id)
        .await
        .expect("mock recall must succeed");

    assert!(
        !profile.preferences.is_empty(),
        "session-end PUM must mirror at least one inferred delta to the \
         plugin-reified Honcho backend; got empty preferences for user_id \
         `{user_id}`"
    );

    // Spot-check the W9 stable key — `preferences.tool_order` is the
    // first delta `UserModelInferencer::infer` emits for any non-empty
    // trace slice.
    assert!(
        profile.preferences.contains_key("preferences.tool_order"),
        "expected `preferences.tool_order` mirrored to plugin backend; \
         got keys {:?}",
        profile.preferences.keys().collect::<Vec<_>>()
    );
}
