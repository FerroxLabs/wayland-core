//! W11: skill refresh through the real next-turn boundary.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::tempdir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, StopReason, TokenUsage};

// ---------------------------------------------------------------------------
// Harness (shape shared with issue_1150_ordinary_turn_payload_test.rs)
// ---------------------------------------------------------------------------

struct RecordingProvider {
    scripts: Mutex<Vec<Vec<LlmEvent>>>,
    requests: Arc<Mutex<Vec<LlmRequest>>>,
}

#[async_trait]
impl LlmProvider for RecordingProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
        self.requests.lock().unwrap().push(request.clone());
        let mut scripts = self.scripts.lock().unwrap();
        let events = if scripts.len() > 1 {
            scripts.remove(0)
        } else {
            scripts[0].clone()
        };
        drop(scripts);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

fn plain_answer() -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta("done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        },
    ]
}

/// The #1150 reporter's route, which is what makes 1,310 chars the budget: an
/// unlisted local model over an OpenAI-compatible endpoint with no
/// `[compact] context_window`, so the session assumes `UNVERIFIED_CONTEXT_WINDOW`
/// (32,768) and the listing gets 1% of it in characters.
fn config() -> Config {
    let mut cfg = Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "issue-1280-local-32k-unlisted".into(),
        max_tokens: 1024,
        max_turns: Some(8),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    };
    cfg.tools.auto_approve = true;
    cfg.session.enabled = false;
    cfg
}

fn write_skill(root: &std::path::Path, name: &str, body: &str) {
    let dir = root.join(".wayland-core/skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: zorbulate telemetry\n---\n{body}\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn cached_skill_edit_add_remove_applies_on_next_turn() {
    let root = tempdir().unwrap();
    write_skill(root.path(), "w11-alpha", "ORIGINAL-BODY");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(vec![plain_answer()]),
        requests: requests.clone(),
    });
    let mut built =
        AgentBootstrap::new(config(), root.path().to_str().unwrap(), Arc::new(NullSink))
            .without_channels(true)
            .provider(provider)
            .build()
            .await
            .unwrap();
    let catalog = built.engine.skill_catalog().unwrap().clone();
    assert!(
        catalog
            .resolve_for_model("w11-alpha")
            .await
            .unwrap()
            .content
            .contains("ORIGINAL-BODY")
    );
    built
        .engine
        .run("zorbulate telemetry", "w11-first")
        .await
        .unwrap();
    write_skill(root.path(), "w11-alpha", "EDITED-BODY");
    write_skill(root.path(), "w11-beta", "ADDED-BODY");
    built
        .engine
        .run("zorbulate telemetry", "w11-edited")
        .await
        .unwrap();
    assert!(
        catalog
            .resolve_for_model("w11-alpha")
            .await
            .unwrap()
            .content
            .contains("EDITED-BODY"),
        "resolved body cache must refresh at the next user turn"
    );
    assert!(
        catalog
            .resolve_for_model("w11-beta")
            .await
            .unwrap()
            .content
            .contains("ADDED-BODY")
    );
    std::fs::remove_dir_all(root.path().join(".wayland-core/skills/w11-alpha")).unwrap();
    built
        .engine
        .run("zorbulate telemetry", "w11-removed")
        .await
        .unwrap();
    assert!(
        catalog.resolve_for_model("w11-alpha").await.is_err(),
        "removed skill cannot remain cached"
    );
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 3);
    assert_eq!(
        captured[0].system, captured[1].system,
        "stable system prefix is not rewritten on refresh"
    );
    assert_eq!(captured[1].system, captured[2].system);
}
