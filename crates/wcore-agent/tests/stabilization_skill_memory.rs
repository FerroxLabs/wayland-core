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
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

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
            .provider(provider.clone())
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
    *provider.scripts.lock().unwrap() = vec![
        vec![
            LlmEvent::ToolUse {
                id: "added-skill".into(),
                name: "Skill".into(),
                input: serde_json::json!({"skill":"w11-beta"}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                usage: TokenUsage::default(),
            },
        ],
        plain_answer(),
    ];
    built
        .engine
        .run("invoke the added skill", "w11-invoke")
        .await
        .unwrap();
    assert_eq!(catalog.invocation_failed("w11-beta"), Some(false));
    std::fs::write(root.path().join(".wayland-core/skills/w11-beta/SKILL.md"),
        "---\nname: w11-beta\ndescription: zorbulate telemetry\ndisable-model-invocation: true\n---\nREVOKED-BODY").unwrap();
    built
        .engine
        .run("zorbulate telemetry", "w11-revoked")
        .await
        .unwrap();
    assert!(catalog.resolve_for_model("w11-beta").await.is_err());
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 6);
    let current = captured[1]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            wcore_types::message::ContentBlock::Text { text }
                if text.starts_with("Current skill inventory") =>
            {
                Some(text)
            }
            _ => None,
        })
        .expect("next request carries the current inventory");
    assert!(current.contains("w11-beta"));
    assert!(
        serde_json::to_string(&captured[4].messages)
            .unwrap()
            .contains("ADDED-BODY"),
        "added skill actually executed through the engine"
    );
    assert_eq!(
        captured[0].system, captured[1].system,
        "stable system prefix is not rewritten on refresh"
    );
    assert_eq!(captured[1].system, captured[2].system);
}

#[tokio::test]
async fn normal_answer_and_actual_skill_loading_do_not_fabricate_task_success() {
    let root = tempdir().unwrap();
    write_skill(
        root.path(),
        "w11-credit",
        "Load these instructions; this is not task verification.",
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(RecordingProvider {
        scripts: Mutex::new(vec![plain_answer()]),
        requests,
    });
    let mut built =
        AgentBootstrap::new(config(), root.path().to_str().unwrap(), Arc::new(NullSink))
            .without_channels(true)
            .provider(provider.clone())
            .build()
            .await
            .unwrap();
    built
        .engine
        .set_skill_router(wcore_skills::SkillRouter::with_seed(17));
    built
        .engine
        .run("@@skill=w11-credit zorbulate telemetry", "no-use")
        .await
        .unwrap();
    let router = built.engine.skill_router().unwrap().clone();
    assert_eq!(
        router.lock().unwrap().stats("w11-credit").total(),
        0,
        "a suggestion without invocation has no outcome"
    );
    *provider.scripts.lock().unwrap() = vec![
        vec![
            LlmEvent::ToolUse {
                id: "real-skill-call".into(),
                name: "Skill".into(),
                input: serde_json::json!({"skill":"w11-credit"}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
                usage: TokenUsage::default(),
            },
        ],
        plain_answer(),
    ];
    built
        .engine
        .run("@@skill=w11-credit zorbulate telemetry", "used")
        .await
        .unwrap();
    assert_eq!(
        built
            .engine
            .skill_catalog()
            .unwrap()
            .invocation_failed("w11-credit"),
        Some(false),
        "real Skill dispatch must be observed"
    );
    assert_eq!(
        router.lock().unwrap().stats("w11-credit").success,
        0,
        "loading instructions is not verified task success, even when model ends normally"
    );
}

#[test]
fn clearly_unrelated_skill_families_are_not_adaptive_candidates() {
    use wcore_skills::{
        refs::SkillCatalog,
        types::{SkillMetadata, SkillSource},
    };
    fn meta(name: &str, description: &str) -> SkillMetadata {
        let raw = format!("---\nname: {name}\ndescription: {description}\n---\ninstructions");
        let parsed = wcore_skills::frontmatter::parse_frontmatter_with_source(&raw, None);
        wcore_skills::frontmatter::parse_skill_fields(
            &parsed.frontmatter,
            &parsed.content,
            name,
            SkillSource::Bundled,
            wcore_skills::types::LoadedFrom::Bundled,
            None,
        )
    }
    let catalog = SkillCatalog::from_metadata_vec(vec![
        meta("rust-review", "inspect Rust lifetimes and compiler errors"),
        meta("meal-prep", "cook pasta and vegetables"),
    ]);
    assert_eq!(
        wcore_skills::SkillRouter::relevant_candidates(
            "please inspect Rust compiler errors",
            &catalog.visible()
        ),
        vec!["rust-review"]
    );
    assert!(
        wcore_skills::SkillRouter::relevant_candidates(
            "calculate orbital velocity",
            &catalog.visible()
        )
        .is_empty()
    );
}

#[tokio::test]
async fn merged_recall_reaches_provider_with_bounded_matching_activation() {
    use wcore_memory::{
        MemoryApi,
        v2_types::{AccessToken, Fact, FactId, Tier},
    };
    let root = tempdir().unwrap();
    let memory: Arc<dyn MemoryApi> =
        Arc::new(wcore_memory::open_for_test(root.path()).await.unwrap());
    let query = "deployment region Bangkok";
    for i in 0..7 {
        let (tier, subject, predicate, object) = if i < 5 {
            (
                Tier::Project,
                "dessert",
                "recipe",
                format!("sugar butter flour {i}"),
            )
        } else if i == 5 {
            (Tier::Global, "deployment", "region", "Bangkok".into())
        } else {
            (Tier::Global, "deployment", "region", query.repeat(10_000))
        };
        memory
            .assert_fact(
                Fact {
                    id: FactId(uuid::Uuid::new_v4()),
                    tier,
                    ts: chrono::Utc::now().timestamp(),
                    subject: subject.into(),
                    predicate: predicate.into(),
                    object,
                    confidence: 1.0,
                    source_episode: None,
                    superseded_by: None,
                },
                AccessToken::System,
            )
            .await
            .unwrap();
    }
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
    built.engine.set_memory_api(memory.clone());
    built.engine.run(query, "recall").await.unwrap();
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let block = captured[0]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .find_map(|b| match b {
            wcore_types::message::ContentBlock::Text { text }
                if text.starts_with("<system-reminder>\nRecalled from your durable") =>
            {
                Some(text)
            }
            _ => None,
        })
        .expect("positive recall must reach actual provider request");
    assert!(block.len() <= 4096);
    assert!(block.contains("Bangkok"));
    let activation = memory.activation_log().unwrap().last().unwrap();
    assert!(!activation.injected.is_empty());
    assert!(activation.injected.iter().any(|i| i.tier == Tier::Global));
    for item in activation.injected {
        assert!(block.contains(&format!("- {}\n", item.preview)));
        assert!(item.preview.len() <= 768);
    }
}
