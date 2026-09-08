//! W03: exercise cached channel authority through the real provider and tools.
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use wcore_agent::{
    channel_dispatch::ChannelTurnDispatcher, channel_inbound::TurnDispatcher,
    channel_policy::ChannelPolicyRegistry,
};
use wcore_channels::{ChannelToolPosture, InboundPolicy, IncomingMessage, config::ChannelConfig};
use wcore_config::config::Config;

#[path = "../../wcore-cli/tests/support/mock_llm.rs"]
mod mock_llm;

fn policy(root: &Path, posture: ChannelToolPosture) -> ChannelConfig {
    ChannelConfig {
        name: "fixture".into(),
        platform: "slack".into(),
        enabled: true,
        options: toml::Table::new(),
        inbound: InboundPolicy {
            dm_allowlist: vec!["alice".into()],
            tools: posture,
            tool_workspace_root: Some(root.to_string_lossy().into_owned()),
            ..Default::default()
        },
    }
}

fn dispatcher(root: &Path, url: &str, channel: ChannelConfig) -> Arc<ChannelTurnDispatcher> {
    let mut config = Config {
        model: "claude-mock".into(),
        api_key: "fixture-not-real".into(),
        base_url: url.into(),
        ..Default::default()
    };
    config.compat = toml::from_str("cost_is_known_free = true").unwrap();
    config.memory.enabled = false;
    config.session.enabled = true;
    config.session.require_durability = true;
    config.session.directory = root.join("sessions").to_string_lossy().into_owned();
    let provider = Arc::new(
        wcore_providers::anthropic::AnthropicProvider::new(
            "fixture-not-real",
            url,
            wcore_config::compat::ProviderCompat::anthropic_defaults(),
            wcore_config::debug::DebugConfig::default(),
        )
        .with_cache(false),
    );
    let policies = Arc::new(ChannelPolicyRegistry::from_configs(vec![channel], root).unwrap());
    Arc::new(ChannelTurnDispatcher::new(
        config,
        root.to_string_lossy().into_owned(),
        provider,
        policies,
        None,
    ))
}

async fn turn(
    dispatcher: &ChannelTurnDispatcher,
    key: &str,
    id: &str,
) -> anyhow::Result<Option<String>> {
    let msg = IncomingMessage::new(id, "conversation", "alice", id, 0);
    tokio::time::timeout(
        Duration::from_secs(20),
        dispatcher.dispatch(key, "fixture", &msg),
    )
    .await?
}

fn last_tool_result(body: &Value) -> &Value {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter_map(|m| m["content"].as_array())
        .flat_map(|blocks| blocks.iter().rev())
        .find(|b| b["type"] == "tool_result")
        .expect("real tool execution receipt in the next provider request")
}

#[tokio::test]
async fn full_to_conversational_revokes_cached_read_and_preserves_history() {
    let root = tempfile::tempdir().unwrap();
    let secret = "w03-owned-file-canary-348f983c";
    let path = root.path().join("canary.txt");
    std::fs::write(&path, secret).unwrap();
    let mock = mock_llm::MockLlm::new()
        .tool_use("Read", json!({"file_path":path}))
        .text("control-complete")
        .tool_use("Read", json!({"file_path":path}))
        .text("denial-observed")
        .start()
        .await;
    let engine = dispatcher(
        root.path(),
        &mock.uri(),
        policy(root.path(), ChannelToolPosture::Full),
    );
    turn(&engine, "conversation-a", "original-history-marker")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    let receipt = last_tool_result(&body);
    assert_ne!(receipt["is_error"], true, "{receipt}");
    assert!(receipt["content"].to_string().contains(secret), "{receipt}");
    engine
        .reload_from_configs(vec![policy(
            root.path(),
            ChannelToolPosture::Conversational,
        )])
        .await
        .unwrap();
    turn(&engine, "conversation-a", "read-after-revocation")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    let receipt = last_tool_result(&body);
    assert_eq!(
        receipt["is_error"], true,
        "old Full authority survived reload: {receipt}"
    );
    assert!(
        body["messages"]
            .to_string()
            .contains("original-history-marker")
    );
    engine.reload_from_configs(vec![]).await.unwrap();
}

#[tokio::test]
async fn workspace_reload_denies_old_root_and_allows_new_root() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let old = a.join("old.txt");
    let new = b.join("new.txt");
    std::fs::write(&old, "w03-old-workspace-canary").unwrap();
    std::fs::write(&new, "w03-new-workspace-canary").unwrap();
    let mock = mock_llm::MockLlm::new()
        .tool_use("Read", json!({"file_path":old}))
        .text("old-control")
        .tool_use("Read", json!({"file_path":old}))
        .text("old-denied")
        .tool_use("Read", json!({"file_path":new}))
        .text("new-control")
        .start()
        .await;
    let engine = dispatcher(
        root.path(),
        &mock.uri(),
        policy(&a, ChannelToolPosture::Workspace),
    );
    turn(&engine, "workspace-conversation", "before")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    assert!(
        last_tool_result(&body)["content"]
            .to_string()
            .contains("w03-old-workspace-canary")
    );
    engine
        .reload_from_configs(vec![policy(&b, ChannelToolPosture::Workspace)])
        .await
        .unwrap();
    turn(&engine, "workspace-conversation", "old-after")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    assert_eq!(last_tool_result(&body)["is_error"], true);
    turn(&engine, "workspace-conversation", "new-after")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    assert_ne!(last_tool_result(&body)["is_error"], true);
    assert!(
        last_tool_result(&body)["content"]
            .to_string()
            .contains("w03-new-workspace-canary")
    );
    engine.reload_from_configs(vec![]).await.unwrap();
}

#[tokio::test]
async fn revocation_cancels_active_provider_and_refuses_old_queued_admission() {
    let root = tempfile::tempdir().unwrap();
    let original = policy(root.path(), ChannelToolPosture::Full);
    let old_policy = original.inbound.clone();
    let mock = mock_llm::MockLlm::new()
        .slow_text("must-not-deliver", 60_000)
        .start()
        .await;
    let engine = dispatcher(root.path(), &mock.uri(), original);
    let runner = engine.clone();
    let task = tokio::spawn(async move { turn(&runner, "held-conversation", "held").await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while mock.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let mut revoked = policy(root.path(), ChannelToolPosture::Full);
    revoked.inbound.dm_allowlist = vec!["bob".into()];
    engine.reload_from_configs(vec![revoked]).await.unwrap();
    assert!(
        task.is_finished(),
        "reload acknowledged while old dispatch still ran"
    );
    assert!(task.await.unwrap().is_err());
    let msg = IncomingMessage::new("queued", "conversation", "alice", "old admitted message", 0);
    assert!(
        engine
            .dispatch_admitted("held-conversation", "fixture", &msg, &old_policy)
            .await
            .is_err()
    );
    assert_eq!(mock.received_requests().await.unwrap().len(), 1);
    engine.reload_from_configs(vec![]).await.unwrap();
}

#[tokio::test]
async fn concurrent_first_turns_share_writer_while_other_sessions_progress() {
    let root = tempfile::tempdir().unwrap();
    let mock = mock_llm::MockLlm::new()
        .slow_text("first", 10_000)
        .text("other")
        .text("second")
        .start()
        .await;
    let channel = policy(root.path(), ChannelToolPosture::Conversational);
    let engine = dispatcher(root.path(), &mock.uri(), channel.clone());
    let first = engine.clone();
    let first = tokio::spawn(async move { turn(&first, "same", "first").await });
    tokio::time::timeout(Duration::from_secs(20), async {
        while mock.received_requests().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let second = engine.clone();
    let second = tokio::spawn(async move { turn(&second, "same", "second").await });
    let other = turn(&engine, "other", "other").await.unwrap();
    assert_eq!(other.as_deref(), Some("other"));
    assert!(
        !first.is_finished(),
        "unrelated session waited behind held first turn"
    );
    assert_eq!(first.await.unwrap().unwrap().as_deref(), Some("first"));
    assert_eq!(second.await.unwrap().unwrap().as_deref(), Some("second"));
    engine.reload_from_configs(vec![channel]).await.unwrap();
    turn(&engine, "same", "unchanged-scope-followup")
        .await
        .unwrap();
    let requests = mock.received_requests().await.unwrap();
    let body: Value = requests.last().unwrap().body_json().unwrap();
    assert!(body["messages"].to_string().contains("first"));
    assert!(body["messages"].to_string().contains("second"));
    engine.reload_from_configs(vec![]).await.unwrap();
}
