//! W09: real engine dispatches and physical POSTs share the session envelope.
mod common;

use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Notify};
use wcore_agent::budget_authority::{BudgetAuthoritySeed, SharedBudgetAuthorityCoordinator};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::null_sink::NullSink;
use wcore_agent::session_journal::BudgetWallClockAuthority;
use wcore_budget::{BudgetCap, BudgetTracker};
use wcore_config::compat::ProviderCompat;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::builder_send_with_retry;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const SESSION: &str = "auxiliary-fixture";

enum Reply {
    Complete {
        text: String,
        usage: TokenUsage,
        tool: bool,
    },
    PromptTooLong,
    Fallback,
    Hold {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    },
    MissingDone,
}

struct PhysicalProvider {
    url: String,
    replies: Mutex<VecDeque<Reply>>,
    models: Mutex<Vec<String>>,
}

#[async_trait]
impl LlmProvider for PhysicalProvider {
    async fn stream(
        &self,
        request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.models.lock().unwrap().push(request.model.clone());
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        builder_send_with_retry(client.post(&self.url)).await?;
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected physical dispatch");
        let (tx, rx) = mpsc::channel(4);
        match reply {
            Reply::Fallback => {
                wcore_providers::retry::admit_configured_fallback(
                    "openai",
                    "fixture-fallback",
                    "openai",
                    "gpt-4o",
                    true,
                )?;
                builder_send_with_retry(client.post(&self.url)).await?;
                tx.send(LlmEvent::TextDelta(
                    "<summary>fallback summary</summary>".into(),
                ))
                .await
                .unwrap();
                tx.send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: usage(100, 20),
                })
                .await
                .unwrap();
            }
            Reply::PromptTooLong => return Err(ProviderError::PromptTooLong("fixture".into())),
            Reply::MissingDone => {
                tx.send(LlmEvent::TextDelta("partial".into()))
                    .await
                    .unwrap();
            }
            Reply::Hold { entered, release } => {
                entered.notify_one();
                release.notified().await;
                tx.send(LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::Stop,
                    usage: usage(1, 1),
                })
                .await
                .unwrap();
            }
            Reply::Complete { text, usage, tool } => {
                if tool {
                    tx.send(LlmEvent::ToolUse {
                        id: "tool-1".into(),
                        name: "mock_tool".into(),
                        input: serde_json::json!({}),
                        extra: None,
                    })
                    .await
                    .unwrap();
                } else {
                    tx.send(LlmEvent::TextDelta(text)).await.unwrap();
                }
                let stop_reason = if tool {
                    StopReason::ToolUse
                } else {
                    StopReason::EndTurn
                };
                tx.send(LlmEvent::Done {
                    stop_reason,
                    finish_reason: FinishReason::from_stop_reason(stop_reason),
                    usage,
                })
                .await
                .unwrap();
            }
        }
        Ok(rx)
    }
}

fn usage(input_tokens: u64, output_tokens: u64) -> TokenUsage {
    TokenUsage {
        input_tokens,
        output_tokens,
        ..Default::default()
    }
}
fn reply(text: &str, usage: TokenUsage) -> Reply {
    Reply::Complete {
        text: text.into(),
        usage,
        tool: false,
    }
}
fn first() -> Reply {
    Reply::Complete {
        text: String::new(),
        usage: usage(170_000, 10),
        tool: true,
    }
}
fn config() -> wcore_config::config::Config {
    let mut config = common::test_config();
    config.model = "gpt-4o".into();
    config.compat = ProviderCompat::openai_defaults();
    config.max_tokens = 1024;
    config.max_tokens_explicit = true;
    config.compact.context_window = Some(200_000);
    config.compact.compaction_model = Some("gpt-4o-mini".into());
    config
}
async fn fixture(
    replies: Vec<Reply>,
    cap: BudgetCap,
) -> (
    MockServer,
    Arc<PhysicalProvider>,
    AgentEngine,
    Arc<parking_lot::Mutex<BudgetTracker>>,
) {
    fixture_config(replies, cap, config()).await
}
async fn fixture_config(
    replies: Vec<Reply>,
    cap: BudgetCap,
    config: wcore_config::config::Config,
) -> (
    MockServer,
    Arc<PhysicalProvider>,
    AgentEngine,
    Arc<parking_lot::Mutex<BudgetTracker>>,
) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let provider = Arc::new(PhysicalProvider {
        url: server.uri(),
        replies: Mutex::new(replies.into()),
        models: Mutex::new(Vec::new()),
    });
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(common::MockTool::new(
        "mock_tool",
        "tool result",
        false,
    )));
    let mut engine =
        AgentEngine::new_with_provider(provider.clone(), config, tools, Arc::new(NullSink));
    let tracker = Arc::new(parking_lot::Mutex::new(BudgetTracker::new(cap)));
    engine.set_budget_tracker(Arc::clone(&tracker));
    engine.set_budget_session_id(SESSION);
    (server, provider, engine, tracker)
}
fn ample() -> BudgetCap {
    BudgetCap::builder()
        .per_session_tokens(1_000_000)
        .per_session_usd(100.0)
        .build()
}
async fn posts(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

#[tokio::test]
async fn priced_conversation_compaction_conversation_reconciles_once() {
    let mut compact_usage = usage(200, 30);
    compact_usage.cache_read_tokens = 40;
    compact_usage.cache_creation_tokens = 50;
    compact_usage.reported_cost_usd = Some(0.07);
    let (server, provider, mut engine, tracker) = fixture(
        vec![
            first(),
            reply("<summary>saved</summary>", compact_usage),
            reply("done", usage(20, 2)),
        ],
        ample(),
    )
    .await;
    let result = engine.run("do work", "one").await.unwrap();
    assert_eq!(posts(&server).await, 3);
    assert_eq!(
        *provider.models.lock().unwrap(),
        ["gpt-4o", "gpt-4o-mini", "gpt-4o"]
    );
    assert_eq!(result.usage.input_tokens, 170_220);
    assert_eq!(result.usage.output_tokens, 42);
    assert_eq!(result.usage.cache_read_tokens, 40);
    assert_eq!(result.usage.cache_creation_tokens, 50);
    assert_eq!(engine.usage_snapshot().1.total_input_tokens(), 170_310);
    let tracker = tracker.lock();
    assert_eq!(tracker.session_totals(SESSION).0, 170_352);
    // gpt-4o conversation list rates plus the compactor's explicit bill.
    assert!(
        (tracker.session_totals(SESSION).1 - (170_020.0 * 2.5e-6 + 12.0 * 10e-6 + 0.07)).abs()
            < 1e-8
    );
    assert_eq!(tracker.reserved_totals(SESSION), (0, 0.0));
}

/// W12 mutation target: removing compaction admission causes a second POST.
#[tokio::test]
async fn exhausted_compaction_cap_prevents_physical_post() {
    let (server, _, mut engine, _) =
        fixture(vec![first(), reply("must not send", usage(1, 1))], ample()).await;
    let authority = shared_authority(2_000);
    engine
        .inherit_test_budget_authority(authority.clone())
        .unwrap();
    assert!(engine.run("do work", "one").await.is_err());
    assert_eq!(
        posts(&server).await,
        1,
        "compaction cannot bypass the reserved output ceiling"
    );
    assert_eq!(
        authority
            .lock()
            .inspect(|tracker, _| tracker.session_totals(SESSION).0)
            .unwrap(),
        170_010
    );
    assert_eq!(
        authority
            .lock()
            .inspect(|tracker, _| tracker.reserved_totals(SESSION))
            .unwrap(),
        (0, 0.0)
    );
}

#[tokio::test]
async fn empty_summary_still_charges_authoritative_usage() {
    let (server, _, mut engine, tracker) = fixture(
        vec![
            first(),
            reply("", usage(200, 30)),
            reply("done", usage(20, 2)),
        ],
        ample(),
    )
    .await;
    engine.run("do work", "one").await.unwrap();
    assert_eq!(posts(&server).await, 3);
    assert_eq!(tracker.lock().session_totals(SESSION).0, 170_262);
    assert_eq!(engine.usage_snapshot().0.output_tokens, 42);
}

#[tokio::test]
async fn missing_usage_and_partial_stream_consume_the_admitted_bound() {
    for compact in [
        reply("<summary>no usage</summary>", TokenUsage::default()),
        Reply::MissingDone,
    ] {
        let (server, _, mut engine, tracker) =
            fixture(vec![first(), compact, reply("done", usage(20, 2))], ample()).await;
        engine.run("do work", "one").await.unwrap();
        assert_eq!(posts(&server).await, 3);
        assert!(tracker.lock().session_totals(SESSION).0 > 170_032 + 1_000);
        assert!(engine.usage_snapshot().0.output_tokens > 1_000);
        assert_eq!(tracker.lock().reserved_totals(SESSION), (0, 0.0));
    }
}

#[tokio::test]
async fn prompt_too_long_retry_has_a_separate_charge() {
    let (server, _, mut engine, tracker) = fixture(
        vec![
            reply("seed", usage(10, 1)),
            first(),
            Reply::PromptTooLong,
            reply("<summary>retry</summary>", usage(100, 20)),
            reply("done", usage(20, 2)),
        ],
        ample(),
    )
    .await;
    engine.run("seed history", "seed").await.unwrap();
    engine.run("do work", "one").await.unwrap();
    assert_eq!(posts(&server).await, 5);
    assert!(tracker.lock().session_totals(SESSION).0 > 170_152 + 1_000);
    assert_eq!(tracker.lock().reserved_totals(SESSION), (0, 0.0));
}

#[tokio::test]
async fn compaction_usage_overshoot_is_recorded_and_stops_the_next_post() {
    let (server, _, mut engine, tracker) = fixture(
        vec![
            first(),
            reply("<summary>large</summary>", usage(100, 100_000)),
            reply("must not send", usage(1, 1)),
        ],
        BudgetCap::builder()
            .per_session_output_tokens(20_000)
            .build(),
    )
    .await;
    assert!(engine.run("do work", "one").await.is_err());
    assert_eq!(posts(&server).await, 2);
    assert_eq!(tracker.lock().session_totals(SESSION).0, 270_110);
    assert_eq!(engine.usage_snapshot().0.output_tokens, 100_010);
}

#[tokio::test]
async fn cancellation_after_compaction_post_consumes_bound() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (server, _, mut engine, tracker) = fixture(
        vec![
            first(),
            Reply::Hold {
                entered: entered.clone(),
                release,
            },
        ],
        ample(),
    )
    .await;
    let cancel = engine.cancel_token();
    let run = tokio::spawn(async move {
        let _ = engine.run("do work", "one").await;
        engine
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    cancel.cancel();
    let engine = tokio::time::timeout(std::time::Duration::from_secs(10), run)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(posts(&server).await, 2);
    assert!(tracker.lock().session_totals(SESSION).0 > 170_010 + 1_000);
    assert!(engine.usage_snapshot().0.output_tokens > 1_000);
    assert_eq!(tracker.lock().reserved_totals(SESSION), (0, 0.0));
}

fn shared_authority(output_limit: u64) -> SharedBudgetAuthorityCoordinator {
    BudgetAuthoritySeed {
        provider_caps: BudgetCap::builder()
            .per_session_output_tokens(output_limit)
            .build(),
        preserve_committed_session_extensions: true,
        execution_policy: Default::default(),
        wall_clock: BudgetWallClockAuthority::ActiveRuntime,
        process_cleanup_proof: None,
        daily_authority: None,
    }
    .detached(SESSION)
    .unwrap()
}

#[tokio::test]
async fn two_real_child_engines_cannot_double_reserve_shared_capacity() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (server, provider, mut first_child, _) = fixture(
        vec![
            Reply::Hold {
                entered: entered.clone(),
                release: release.clone(),
            },
            reply("must not send", usage(1, 1)),
        ],
        ample(),
    )
    .await;
    let authority = shared_authority(1_500);
    first_child
        .inherit_test_budget_authority(authority.clone())
        .unwrap();
    let mut second_child =
        AgentEngine::new_with_provider(provider, config(), ToolRegistry::new(), Arc::new(NullSink));
    second_child
        .inherit_test_budget_authority(authority.clone())
        .unwrap();
    let first_run = tokio::spawn(async move { first_child.run("first child", "one").await });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let second = second_child.run("second child", "two").await;
    assert!(second.is_err() || second.unwrap().text.is_empty());
    assert_eq!(posts(&server).await, 1);
    release.notify_one();
    first_run.await.unwrap().unwrap();
    assert_eq!(
        authority
            .lock()
            .inspect(|tracker, _| tracker.reserved_totals(SESSION))
            .unwrap(),
        (0, 0.0)
    );
}

#[tokio::test]
async fn unknown_compaction_price_is_refused_under_explicit_usd_cap() {
    let mut config = config();
    config.compact.compaction_model = Some("unknown-compaction-model".into());
    config.budget.max_cost_usd = Some(100.0);
    let (server, _, mut engine, tracker) = fixture_config(
        vec![first(), reply("must not send", usage(1, 1))],
        ample(),
        config,
    )
    .await;
    assert!(engine.run("do work", "one").await.is_err());
    assert_eq!(posts(&server).await, 1);
    assert_eq!(tracker.lock().reserved_totals(SESSION), (0, 0.0));
}

#[tokio::test]
async fn persisted_compaction_usage_is_not_added_again_on_resume() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config();
    common::configure_persisted_test_session(&mut config, root.path());
    let manager = wcore_agent::session::SessionManager::new(
        std::path::PathBuf::from(&config.session.directory),
        10,
    );
    let (server, provider, mut engine, tracker) = fixture_config(
        vec![
            first(),
            reply("<summary>saved</summary>", usage(200, 30)),
            reply("done", usage(20, 2)),
            reply("resumed", usage(3, 1)),
        ],
        ample(),
        config.clone(),
    )
    .await;
    engine
        .init_session("test", &root.path().to_string_lossy(), None)
        .unwrap();
    engine.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    engine.run("do work", "one").await.unwrap();
    let expected = engine.usage_snapshot().0;
    let session_id = engine.current_session_id().unwrap();
    drop(engine);
    let active = manager.load_for_run(&session_id).unwrap();
    let mut resumed = AgentEngine::resume_active_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        Arc::new(NullSink),
        active,
    );
    resumed.use_recovery_test_key(&common::RECOVERY_TEST_KEY);
    resumed.set_budget_tracker(tracker);
    resumed.set_budget_session_id(SESSION);
    assert_eq!(
        resumed.usage_snapshot().0.input_tokens,
        expected.input_tokens
    );
    resumed.run("next", "two").await.unwrap();
    assert_eq!(posts(&server).await, 4);
    assert_eq!(
        resumed.usage_snapshot().0.input_tokens,
        expected.input_tokens + 3
    );
    assert_eq!(resumed.usage_snapshot().1.input_tokens, 3);
}

#[tokio::test]
async fn configured_fallback_replaces_compaction_grant_and_settles_its_model() {
    let (server, _, mut engine, tracker) = fixture(
        vec![first(), Reply::Fallback, reply("done", usage(20, 2))],
        ample(),
    )
    .await;
    engine.run("do work", "one").await.unwrap();
    assert_eq!(posts(&server).await, 4);
    assert!(tracker.lock().session_totals(SESSION).0 > 170_152 + 1_000);
    assert_eq!(tracker.lock().reserved_totals(SESSION), (0, 0.0));
    assert_eq!(
        engine.usage_snapshot().0.total_input_tokens() + engine.usage_snapshot().0.output_tokens,
        tracker.lock().session_totals(SESSION).0
    );
}
