//! Black-box integration tests for engine compaction integration (TC-2.6-*).
//!
//! These tests exercise the full `AgentEngine::run()` loop and verify
//! that the compaction pipeline (microcompact → autocompact → emergency)
//! is correctly wired into the agentic loop.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;

use tempfile::tempdir;
use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session::SessionManager;
use wcore_config::compact::CompactConfig;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};

use common::test_config;

// ── Helpers ────────────────────────────────────────────────────────────────

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// A mock provider that returns configurable per-turn events.
/// Tracks the number of stream() calls for order verification.
struct CompactMockProvider {
    turns: Mutex<VecDeque<Vec<LlmEvent>>>,
    call_count: Mutex<usize>,
}

impl CompactMockProvider {
    fn new(turns: Vec<Vec<LlmEvent>>) -> Self {
        Self {
            turns: Mutex::new(VecDeque::from(turns)),
            call_count: Mutex::new(0),
        }
    }

    fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }
}

#[async_trait]
impl LlmProvider for CompactMockProvider {
    async fn stream(
        &self,
        _request: &LlmRequest,
    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        let events = self.turns.lock().unwrap().pop_front().unwrap_or_else(|| {
            vec![LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::EndTurn,
                ),
                usage: TokenUsage::default(),
            }]
        });

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                let _ = tx.send(event).await;
            }
        });
        Ok(rx)
    }
}

/// Build events for a simple text response with configurable input_tokens.
fn text_turn(text: &str, input_tokens: u64) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ]
}

/// Build events for a summary LLM call (used by autocompact internally).
fn summary_turn(summary_text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(summary_text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens: 5_000,
                output_tokens: 2_000,
                ..Default::default()
            },
        },
    ]
}

// ── TC-2.6-01: First turn does not trigger compaction ──────────────────────

#[tokio::test]
async fn tc_2_6_01_first_turn_no_compaction() {
    // On the first turn last_input_tokens is 0, so neither autocompact
    // nor emergency should fire.
    let provider = Arc::new(CompactMockProvider::new(vec![text_turn("Hello", 50_000)]));

    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);
    let result = engine.run("Hi", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Hello");
    assert_eq!(result.turns, 1);
    // Only one call to stream() — no compaction call
    assert_eq!(provider.call_count(), 1);
}

// ── TC-2.6-03: Emergency truncation returns error ──────────────────────────

#[tokio::test]
async fn tc_2_6_03_emergency_returns_error() {
    // Emergency is the last safety net — it fires when autocompact is
    // disabled or circuit-broken.  We disable compact so only emergency
    // is active, then push input_tokens above the emergency limit.
    //
    // Turn 1: tool use, returns input_tokens above emergency threshold
    // Turn 2: emergency fires before the API call → ContextTooLong
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 198_000, // above emergency limit (197k)
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    // Turn 2 events are queued but should never be consumed
    let turn2 = text_turn("Should not reach", 50_000);

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, turn2]));
    let mut config = test_config();
    config.compact.enabled = false; // disable auto/micro so emergency is the only gate
    config.compact.context_window = 200_000;
    config.compact.emergency_buffer = 3_000;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);
    let err = engine.run("Do something", "msg-1").await.unwrap_err();

    match err {
        AgentError::ContextTooLong {
            input_tokens,
            limit,
        } => {
            assert_eq!(input_tokens, 198_000);
            assert_eq!(limit, 197_000);
        }
        other => panic!("expected ContextTooLong, got: {:?}", other),
    }

    // Only one call to stream() — second call blocked by emergency
    assert_eq!(provider.call_count(), 1);
}

// ── TC-2.6-04: Autocompact then continue ───────────────────────────────────

#[tokio::test]
async fn tc_2_6_04_autocompact_then_continue() {
    // Turn 1: tool use, returns input_tokens=170k (above autocompact threshold 167k)
    // Before turn 2: autocompact fires → LLM summary call → messages replaced
    // Turn 2 (after compact): text response with low input_tokens
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 170_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let compact_summary = summary_turn("<summary>Conversation summary</summary>");
    let turn2_after_compact = text_turn("Continuing after compact", 10_000);

    let provider = Arc::new(CompactMockProvider::new(vec![
        turn1,
        compact_summary,
        turn2_after_compact,
    ]));

    let mut config = test_config();
    config.compact = CompactConfig::default();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);
    let result = engine
        .run("Start work", "msg-1")
        .await
        .expect("should succeed after compact");

    assert_eq!(result.text, "Continuing after compact");
    assert_eq!(result.turns, 2);
    // 3 calls: turn1 + compact summary + turn2
    assert_eq!(provider.call_count(), 3);
}

// ── TC-2.6-05: Session save includes compacted messages ────────────────────

#[tokio::test]
async fn tc_2_6_05_session_save_after_compact() {
    let dir = tempdir().expect("tempdir");

    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 170_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let compact_summary = summary_turn("<summary>Session summary</summary>");
    let turn2 = text_turn("After compact", 10_000);

    let provider = Arc::new(CompactMockProvider::new(vec![
        turn1,
        compact_summary,
        turn2,
    ]));

    let mut config = test_config();
    config.compact = CompactConfig::default();
    config.session.enabled = true;
    config.session.directory = dir.path().to_string_lossy().into_owned();

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    engine
        .init_session("test", "/tmp", None)
        .expect("init session");

    engine.run("Start", "msg-1").await.expect("should succeed");

    // Load the saved session
    let mgr = SessionManager::new(dir.path().to_path_buf(), 10);
    let session = mgr.load("latest").expect("load session");

    // After compaction + turn2, messages should include the compact boundary,
    // summary, and the post-compact assistant/user messages.
    // The exact count depends on implementation, but should be small (not
    // the full pre-compact count).
    assert!(
        session.messages.len() < 10,
        "session should have compacted messages, got {}",
        session.messages.len()
    );

    // Verify at least one message contains compact boundary marker
    let has_boundary = session.messages.iter().any(|m| {
        m.content.iter().any(|b| {
            matches!(b, wcore_types::message::ContentBlock::Text { text } if text.contains("[Conversation compacted]"))
        })
    });
    assert!(
        has_boundary,
        "session should contain compact boundary marker"
    );
}

// ── TC-2.6-06: Disabled skips all except emergency ─────────────────────────

#[tokio::test]
async fn tc_2_6_06_disabled_skips_micro_auto() {
    // With compact disabled, a text response that reports high usage
    // should not trigger autocompact (only emergency if at limit).
    let provider = Arc::new(CompactMockProvider::new(vec![
        // Returns high but not emergency-level tokens
        text_turn("Normal response", 170_000),
    ]));

    let mut config = test_config();
    config.compact.enabled = false;

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);
    let result = engine.run("Hi", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Normal response");
    // Only 1 call — no compact summary call
    assert_eq!(provider.call_count(), 1);
}

#[tokio::test]
async fn tc_2_6_06b_disabled_still_fires_emergency() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 198_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];

    let provider = Arc::new(CompactMockProvider::new(vec![
        turn1,
        text_turn("unreachable", 0),
    ]));

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = 200_000;
    config.compact.emergency_buffer = 3_000;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let err = engine.run("Go", "msg-1").await.unwrap_err();

    assert!(
        matches!(err, AgentError::ContextTooLong { .. }),
        "emergency should fire even when disabled"
    );
}

// ── TC-2.6-07: input_tokens correctly tracked ──────────────────────────────

#[tokio::test]
async fn tc_2_6_07_input_tokens_tracked() {
    // Two turns: first returns 50k tokens, second returns 60k tokens.
    // We verify that the engine updates compact state after each turn.
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "t1".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 50_000,
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let turn2 = text_turn("Done", 60_000);

    let provider = Arc::new(CompactMockProvider::new(vec![turn1, turn2]));

    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.turns, 2);
    // Total usage should accumulate: 50k + 60k = 110k input tokens
    assert_eq!(result.usage.input_tokens, 110_000);
}

// ── TC-2.6-02: Execution order — micro before auto ────────────────────────

#[tokio::test]
async fn tc_2_6_02_micro_before_auto_execution_order() {
    // Build a scenario where both microcompact and autocompact trigger
    // in the same compaction cycle.  A custom provider captures the
    // messages sent to the autocompact LLM call so we can verify that
    // microcompact already cleared old tool results before autocompact
    // was invoked.

    let captured: Arc<Mutex<Option<Vec<wcore_types::message::Message>>>> =
        Arc::new(Mutex::new(None));
    let capture_ref = captured.clone();

    struct OrderProvider {
        regular_count: Mutex<usize>,
        captured: Arc<Mutex<Option<Vec<wcore_types::message::Message>>>>,
    }

    #[async_trait]
    impl LlmProvider for OrderProvider {
        async fn stream(
            &self,
            request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let is_compact = request.tools.is_empty();

            if is_compact {
                // Capture messages that autocompact sends to the LLM
                *self.captured.lock().unwrap() = Some(request.messages.clone());

                let events = vec![
                    LlmEvent::TextDelta("<summary>Order test summary</summary>".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::EndTurn,
                        ),
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 2_000,
                            ..Default::default()
                        },
                    },
                ];
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    for e in events {
                        let _ = tx.send(e).await;
                    }
                });
                return Ok(rx);
            }

            let count = {
                let mut c = self.regular_count.lock().unwrap();
                let v = *c;
                *c += 1;
                v
            };

            // Turns 0-6: tool use.  Turn 6 reports high input_tokens
            // so that micro and auto both trigger in the SAME cycle
            // (turn 7's run_compaction).
            // Turn 7 (after compact): text to end the run.
            //
            // micro_keep_recent = 3 → count threshold = 6.
            // After 7 tool-use turns: 7 > 6 → micro fires.
            // After turn 6: last_input_tokens = 170k > 167k → auto fires.
            let events = if count < 7 {
                let input_tokens = if count == 6 { 170_000 } else { 10_000 };
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{count}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::ToolUse,
                        ),
                        usage: TokenUsage {
                            input_tokens,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta("Done after compact".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::EndTurn,
                        ),
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for e in events {
                    let _ = tx.send(e).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(OrderProvider {
        regular_count: Mutex::new(0),
        captured: capture_ref,
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        micro_keep_recent: 3,
        compactable_tools: vec!["mock_tool".into()],
        context_window: 200_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(20);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "tool output data",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Start", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Done after compact");

    // Verify: the messages that autocompact received should contain
    // tool results cleared by microcompact (proving micro ran first
    // within the SAME compaction cycle).
    let msgs = captured.lock().unwrap();
    let msgs = msgs.as_ref().expect("autocompact should have been called");

    let cleared_count = msgs
        .iter()
        .flat_map(|m| m.content.iter())
        .filter(|b| {
            matches!(
                b,
                wcore_types::message::ContentBlock::ToolResult { content, .. }
                    if content == wcore_agent::compact::micro::CLEARED_TOOL_RESULT
            )
        })
        .count();

    // 7 tool results total, keep_recent=3 → 4 cleared by micro
    // before auto received the messages.
    assert_eq!(
        cleared_count, 4,
        "microcompact should have cleared 4 tool results before autocompact ran"
    );
}

// ── TC-2.6-E2E-02: Microcompact + autocompact cooperative scenario ────────

#[tokio::test]
async fn tc_2_6_e2e_02_micro_and_auto_cooperative() {
    // Verify that microcompact and autocompact cooperate in the same
    // compaction cycle.  Microcompact frees some tokens from old tool
    // results, and autocompact still fires because the input token
    // watermark (which is not reduced by micro) remains above threshold.

    let compact_call_count: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let counter_ref = compact_call_count.clone();

    struct CoopProvider {
        regular_count: Mutex<usize>,
        compact_calls: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl LlmProvider for CoopProvider {
        async fn stream(
            &self,
            request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let is_compact = request.tools.is_empty();

            if is_compact {
                *self.compact_calls.lock().unwrap() += 1;

                let events = vec![
                    LlmEvent::TextDelta("<summary>Cooperative summary</summary>".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::EndTurn,
                        ),
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 2_000,
                            ..Default::default()
                        },
                    },
                ];
                let (tx, rx) = mpsc::channel(64);
                tokio::spawn(async move {
                    for e in events {
                        let _ = tx.send(e).await;
                    }
                });
                return Ok(rx);
            }

            let count = {
                let mut c = self.regular_count.lock().unwrap();
                let v = *c;
                *c += 1;
                v
            };

            // 7 tool-use turns (count 0-6).  Turn 6 returns high tokens.
            // micro_keep_recent = 3 → count threshold = 6.
            // After 7 tool results: 7 > 6 → micro fires.
            // After turn 6: last_input_tokens = 170k > 167k → auto fires.
            let events = if count < 7 {
                let input_tokens = if count == 6 { 170_000 } else { 10_000 };
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{count}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::ToolUse,
                        ),
                        usage: TokenUsage {
                            input_tokens,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta("After cooperative compact".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::EndTurn,
                        ),
                        usage: TokenUsage {
                            input_tokens: 5_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for e in events {
                    let _ = tx.send(e).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(CoopProvider {
        regular_count: Mutex::new(0),
        compact_calls: counter_ref,
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        micro_keep_recent: 3,
        compactable_tools: vec!["mock_tool".into()],
        context_window: 200_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(20);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "tool output data",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "After cooperative compact");

    // Autocompact was called exactly once (micro freed tokens but
    // did not reduce last_input_tokens, so auto still fired).
    let calls = *compact_call_count.lock().unwrap();
    assert_eq!(
        calls, 1,
        "autocompact should fire exactly once despite microcompact running first"
    );

    // Total turns: 7 tool-use + 1 post-compact text = 8 engine turns,
    // plus 1 internal compact LLM call = 9 provider calls.
    assert_eq!(result.turns, 8);
}

// ── TC-2.6-E2E-03: Circuit breaker after repeated failures ─────────────────

#[tokio::test]
async fn tc_2_6_e2e_03_circuit_breaker_stops_retries() {
    // Simulate: 3 turns where autocompact would trigger but fails each time.
    // After 3 failures the circuit breaker trips and autocompact stops.
    //
    // We use a provider that always fails the compact summary call with
    // a generic API error, but succeeds for regular conversation turns.

    struct CircuitBreakerProvider {
        call_index: Mutex<usize>,
    }

    #[async_trait]
    impl LlmProvider for CircuitBreakerProvider {
        async fn stream(
            &self,
            request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            let idx = {
                let mut i = self.call_index.lock().unwrap();
                let v = *i;
                *i += 1;
                v
            };

            // Compact summary calls have no tools defined and include the
            // compact prompt in messages. We detect them by checking tools.is_empty().
            let is_compact_call = request.tools.is_empty();

            if is_compact_call {
                return Err(ProviderError::Api {
                    status: 500,
                    message: "Internal error".to_string(),
                });
            }

            // Regular conversation turns: tool use on odd calls, text on even
            let events = if idx % 2 == 0 {
                // Tool use turn → keeps the loop going
                vec![
                    LlmEvent::ToolUse {
                        id: format!("t{idx}"),
                        name: "mock_tool".to_string(),
                        input: serde_json::json!({}),
                        extra: None,
                    },
                    LlmEvent::Done {
                        stop_reason: StopReason::ToolUse,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::ToolUse,
                        ),
                        usage: TokenUsage {
                            input_tokens: 170_000, // above autocompact threshold
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            } else {
                // Text turn → ends the loop
                vec![
                    LlmEvent::TextDelta("Final".to_string()),
                    LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                            StopReason::EndTurn,
                        ),
                        usage: TokenUsage {
                            input_tokens: 170_000,
                            output_tokens: 100,
                            ..Default::default()
                        },
                    },
                ]
            };

            let (tx, rx) = mpsc::channel(64);
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event).await;
                }
            });
            Ok(rx)
        }
    }

    let provider = Arc::new(CircuitBreakerProvider {
        call_index: Mutex::new(0),
    });

    let mut config = test_config();
    config.compact = CompactConfig {
        max_failures: 3,
        // Set emergency very high so it doesn't interfere
        context_window: 500_000,
        emergency_buffer: 3_000,
        ..Default::default()
    };
    config.max_turns = Some(10);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(common::MockTool::new(
        "mock_tool",
        "result",
        false,
    )));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Work", "msg-1").await.expect("should succeed");

    assert_eq!(result.text, "Final");
}

// ── #636: graceful context-overflow degradation ────────────────────────────

/// End-to-end proof of #636 through the REAL pre-flight guard: a turn whose
/// assembled request exceeds the model's context ceiling used to abort with
/// `FinishReason::Length`. Now the guard sheds the oversized tool-result output
/// to disk and the run CONTINUES to a real second turn.
///
/// The model id `test-model` is unknown to `wcore_config::limits`, so the
/// guard's window falls back to `compact.context_window`; auto/micro/emergency
/// compaction are disabled so the pre-flight ceiling guard is the ONLY thing
/// that can stop the run. Turn 1's tool returns ~120k tokens of output (far over
/// the 40k ceiling) as a single `ToolResult`, so shedding that one block fits.
#[tokio::test]
async fn tc_2_6_context_overflow_sheds_tool_output_and_continues() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "big".to_string(),
            name: "mock_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 5_000, // low reported tokens → emergency can't fire
                output_tokens: 100,
                ..Default::default()
            },
        },
    ];
    let turn2 = text_turn("Continued after shedding", 5_000);
    let provider = Arc::new(CompactMockProvider::new(vec![turn1, turn2]));

    let mut config = test_config();
    config.compact.enabled = false; // no auto/micro/emergency — isolate the guard
    config.compact.context_window = 60_000; // unknown model → fallback window
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000; // ceiling = 60k - 20k = 40k tokens

    let mut registry = ToolRegistry::new();
    // ~120k tokens (480k chars @ ~4 chars/token) — far over the 40k ceiling, but
    // ONE oversized ToolResult, so the mechanical shed brings it back under.
    let huge = "x".repeat(480_000);
    registry.register(Box::new(common::MockTool::new("mock_tool", &huge, false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);
    let result = engine
        .run("summarize the file", "msg-1")
        .await
        .expect("run should succeed, not error");

    // The guard sheds + CONTINUES: it reaches the 2nd provider call rather than
    // aborting before it. If shedding had failed, the guard would have
    // terminated the run at turn 2 (call_count == 1).
    assert_eq!(
        provider.call_count(),
        2,
        "guard must shed the oversized tool output and continue to turn 2, not abort"
    );
    assert_eq!(result.text, "Continued after shedding");
    // A clean end-turn — NOT the `finish_run_terminated` MaxTurns/Length verdict.
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

/// #636 resume-heals regression: a session that was terminated with
/// `FinishReason::Length` — its persisted history still exceeds the model's
/// context ceiling — must SHED the oversized tool output and CONTINUE when it is
/// reopened, NOT re-abort on the first resumed turn. This guards the fix's
/// shedding of `self.messages` (the persisted/heal path), which the live-turn
/// test above does not exercise: without it, a saved over-ceiling session would
/// re-hit the ceiling guard on turn 1 of every reopen and be permanently stuck
/// (the unrecoverable-session death #636 set out to close).
#[tokio::test]
async fn tc_2_6_context_overflow_resumed_session_heals_and_continues() {
    // A single normal assistant reply is all the provider must return: if the
    // guard heals the resumed history it reaches this one call; if it does NOT,
    // the run aborts BEFORE any dispatch and call_count stays 0.
    let turn1 = text_turn("Resumed and continued", 5_000);
    let provider = Arc::new(CompactMockProvider::new(vec![turn1]));

    let mut config = test_config();
    config.compact.enabled = false; // isolate the pre-flight ceiling guard
    config.compact.context_window = 60_000; // unknown model → fallback window
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000; // ceiling = 60k - 20k = 40k tokens

    let registry = ToolRegistry::new();
    let output = silent_output();
    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);

    // Restore the history of a session that blew the ceiling: an assistant tool
    // call paired with a ~120k-token tool result (480k chars @ ~4 chars/token,
    // far over the 40k ceiling) — the exact shape `--resume` reloads from disk.
    let huge = "x".repeat(480_000);
    engine.load_conversation(vec![
        Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "earlier question".to_string(),
            }],
        ),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "big".to_string(),
                name: "mock_tool".to_string(),
                input: serde_json::json!({}),
                extra: None,
            }],
        ),
        Message::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: "big".to_string(),
                content: huge,
                is_error: false,
            }],
        ),
    ]);

    let result = engine
        .run("continue please", "msg-2")
        .await
        .expect("resumed run should succeed, not error");

    // The guard healed the persisted over-ceiling history and continued to the
    // provider. A re-abort would terminate before dispatch (call_count == 0).
    assert_eq!(
        provider.call_count(),
        1,
        "resumed over-ceiling session must shed persisted tool output and continue, not re-abort on turn 1"
    );
    assert_eq!(result.text, "Resumed and continued");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

/// #646 rung 2 — a turn whose overflow is a big pasted TEXT block (no tool
/// call) must degrade (truncate the block) and CONTINUE, not hard-abort. Rung 1
/// finds zero sheddable tool results here; rung 2 truncates the non-tool block.
#[tokio::test]
async fn tc_2_6_context_overflow_text_paste_truncates_and_continues() {
    let turn1 = text_turn("Handled the big paste", 5_000);
    let provider = Arc::new(CompactMockProvider::new(vec![turn1]));

    let mut config = test_config();
    config.compact.enabled = false; // isolate the pre-flight ceiling guard
    config.compact.context_window = 60_000; // unknown model → fallback window
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000; // ceiling = 40k tokens

    let registry = ToolRegistry::new();
    let output = silent_output();
    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);

    // A huge pasted user message (~120k tokens of plain text, no tool call).
    let huge_paste = "x".repeat(480_000);
    let result = engine
        .run(&huge_paste, "msg-1")
        .await
        .expect("run should succeed, not error");

    // rung 2 truncates the oversized text block and continues to the provider;
    // an abort would leave call_count == 0.
    assert_eq!(
        provider.call_count(),
        1,
        "rung 2 must truncate the paste and continue, not abort"
    );
    assert_eq!(result.text, "Handled the big paste");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

/// #646 rung 2 resume-heals — a session terminated on the conversation-heavy
/// path (a huge non-tool Text block in persisted history) must heal on resume:
/// rung 2 truncates the block so the first resumed turn continues instead of
/// re-aborting. This is the non-tool counterpart to the rung-1 resume test.
#[tokio::test]
async fn tc_2_6_context_overflow_resumed_text_session_heals() {
    let turn1 = text_turn("Resumed past the paste", 5_000);
    let provider = Arc::new(CompactMockProvider::new(vec![turn1]));

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = 60_000;
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000; // ceiling = 40k tokens

    let registry = ToolRegistry::new();
    let output = silent_output();
    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);

    // Restore a session whose history holds a huge non-tool Text block (the
    // shape a big paste leaves), far over the 40k ceiling.
    let huge = "x".repeat(480_000);
    engine.load_conversation(vec![
        Message::new(Role::User, vec![ContentBlock::Text { text: huge }]),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "prior reply".to_string(),
            }],
        ),
    ]);

    let result = engine
        .run("continue please", "msg-2")
        .await
        .expect("resumed run should succeed, not error");

    assert_eq!(
        provider.call_count(),
        1,
        "resumed conversation-heavy session must truncate persisted text and continue, not re-abort"
    );
    assert_eq!(result.text, "Resumed past the paste");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}

/// #646 rung 2 drop-oldest — engine level. When the overflow is spread across
/// MANY non-tool messages that are each individually UNDER the per-block budget
/// (so pass-1 truncation cannot help), the drop-oldest sliding window (pass 2)
/// must remove the oldest non-essential turns until the request fits, and the
/// run must continue to the provider instead of aborting. The two existing
/// rung-2 integration tests both use one huge block, so pass 1 rescues them and
/// this end-to-end drop-oldest path would otherwise be untested.
#[tokio::test]
async fn tc_2_6_context_overflow_many_small_turns_drop_oldest_and_continue() {
    let turn1 = text_turn("Continued after dropping old turns", 5_000);
    let provider = Arc::new(CompactMockProvider::new(vec![turn1]));

    let mut config = test_config();
    config.compact.enabled = false; // isolate the pre-flight ceiling guard
    config.compact.context_window = 60_000; // unknown model → fallback window
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000; // ceiling = 40k tokens

    let registry = ToolRegistry::new();
    let output = silent_output();
    let mut engine = AgentEngine::new_with_provider(provider.clone(), config, registry, output);

    // 24 plain-text turns of 20k chars each (~5k tokens apiece → ~120k tokens
    // total, far over the 40k ceiling). Each block is well under the per-block
    // budget (ceiling = 40k chars), so pass-1 truncation is a no-op and only
    // drop-oldest can bring the request under the ceiling.
    let history: Vec<Message> = (0..24)
        .map(|i| {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            Message::new(
                role,
                vec![ContentBlock::Text {
                    text: "y".repeat(20_000),
                }],
            )
        })
        .collect();
    engine.load_conversation(history);

    let result = engine
        .run("what's next", "msg-3")
        .await
        .expect("run should succeed, not error");

    assert_eq!(
        provider.call_count(),
        1,
        "rung 2 drop-oldest must shrink the many-turn history and continue, not abort"
    );
    assert_eq!(result.text, "Continued after dropping old turns");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
}
