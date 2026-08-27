mod common;

use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;
use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_agent::session::SessionManager;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{StopReason, TokenUsage};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{
    MockLlmProvider, MockTool, RECOVERY_TEST_KEY, configure_persisted_test_session, test_config,
};

// ---------------------------------------------------------------------------
// Helper: build a no-color OutputFormatter for silent test output
// ---------------------------------------------------------------------------
fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

// ---------------------------------------------------------------------------
// test_engine_text_response_ends_turn
//
// Verifies that when the LLM returns a pure text response the engine:
//   - captures the full text
//   - reports StopReason::EndTurn
//   - completes in a single turn
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_text_response_ends_turn() {
    let provider = Arc::new(MockLlmProvider::with_text_response("Hello, world!"));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine.run("Hi", "").await.expect("engine should succeed");

    assert_eq!(result.text, "Hello, world!");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.turns, 1);
}

// ---------------------------------------------------------------------------
// test_engine_tool_use_executes_and_continues
//
// Verifies the agentic loop when the LLM first requests a tool then, after
// receiving the tool result, produces a final text answer.
//   - Turn 1: LLM emits ToolUse for "mock_tool"
//   - Turn 2: LLM emits TextDelta("Done") + EndTurn
//   - result.turns == 2 and result.text == "Done"
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_tool_use_executes_and_continues() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "tool-1".to_string(),
            name: "mock_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                ..Default::default()
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Done".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                ..Default::default()
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "tool output", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Use the tool", "")
        .await
        .expect("engine should succeed");

    assert_eq!(result.turns, 2);
    assert_eq!(result.text, "Done");
}

// ---------------------------------------------------------------------------
// test_engine_max_tokens_handling
//
// Verifies that a MaxTokens stop reason is surfaced correctly when the LLM
// hits its token limit mid-response.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_tokens_handling() {
    let events = vec![
        LlmEvent::TextDelta("partial".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::MaxTokens,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::MaxTokens,
            ),
            usage: TokenUsage {
                input_tokens: 200,
                output_tokens: 100,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                ..Default::default()
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_events(events));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Give me a long answer", "")
        .await
        .expect("engine should succeed");

    assert_eq!(result.stop_reason, StopReason::MaxTokens);
    assert_eq!(result.text, "partial");
}

// ---------------------------------------------------------------------------
// test_engine_message_accumulation
//
// Verifies that consecutive calls to `run` accumulate messages across turns.
// Session persistence is used to observe the messages externally since
// engine.messages is private.
//
// After two independent `run` calls the persisted session must contain
// exactly 4 messages: [user, assistant, user, assistant].
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_message_accumulation() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let dir = tempdir().expect("tempdir should be created");

    // Provider needs two responses (one per run() call)
    let provider = Arc::new(
        MockLlmProvider::with_turns(vec![
            vec![
                LlmEvent::TextDelta("Response 1".to_string()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                        StopReason::EndTurn,
                    ),
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                        ..Default::default()
                    },
                },
            ],
            vec![
                LlmEvent::TextDelta("Response 2".to_string()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                        StopReason::EndTurn,
                    ),
                    usage: TokenUsage {
                        input_tokens: 10,
                        output_tokens: 5,
                        cache_creation_tokens: 0,
                        cache_read_tokens: 0,
                        ..Default::default()
                    },
                },
            ],
        ])
        .with_physical_url(server.uri()),
    );

    let mut config = test_config();
    configure_persisted_test_session(&mut config, dir.path());
    let session_dir = std::path::PathBuf::from(&config.session.directory);

    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config.clone(), registry, output);

    // Initialize session so save_session() has a session to persist
    engine
        .init_session("test-provider", &dir.path().to_string_lossy(), None)
        .expect("init_session should succeed");
    engine.use_recovery_test_key(&RECOVERY_TEST_KEY);

    engine
        .run("First message", "")
        .await
        .expect("first run should succeed");
    engine
        .run("Second message", "")
        .await
        .expect("second run should succeed");

    // Load the persisted session and count accumulated messages
    let session_manager = SessionManager::new(session_dir, 10);
    let session = session_manager
        .load("latest")
        .expect("session should be loadable");

    // Expected layout: user, assistant, user, assistant
    assert_eq!(
        session.messages.len(),
        4,
        "expected 4 messages (user+assistant for each run), got {}",
        session.messages.len()
    );
}

// ---------------------------------------------------------------------------
// test_engine_token_usage_tracking
//
// Verifies that token usage is accumulated correctly across multiple turns.
//   - Turn 1: ToolUse with usage(80 in, 30 out)
//   - Turn 2: EndTurn  with usage(100 in, 50 out)
//   - Expected total: input=180, output=80
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_token_usage_tracking() {
    let turn1 = vec![
        LlmEvent::ToolUse {
            id: "tool-1".to_string(),
            name: "mock_tool".to_string(),
            input: json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::ToolUse,
            ),
            usage: TokenUsage {
                input_tokens: 80,
                output_tokens: 30,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                ..Default::default()
            },
        },
    ];
    let turn2 = vec![
        LlmEvent::TextDelta("Final answer".to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                StopReason::EndTurn,
            ),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                ..Default::default()
            },
        },
    ];

    let provider = Arc::new(MockLlmProvider::with_turns(vec![turn1, turn2]));
    let config = test_config();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Do work", "")
        .await
        .expect("engine should succeed");

    assert_eq!(
        result.usage.input_tokens, 180,
        "input tokens should accumulate across turns"
    );
    assert_eq!(
        result.usage.output_tokens, 80,
        "output tokens should accumulate across turns"
    );
}

// ---------------------------------------------------------------------------
// test_engine_max_turns_returns_ok
//
// Verifies that the engine returns Ok with StopReason::MaxTurns when the
// LLM keeps requesting tools beyond the configured max_turns limit.
//
// With max_turns=1 the engine executes one turn.  If that turn has tool
// calls it processes them, then loops back and hits the limit.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_max_turns_returns_ok() {
    let tool_use_turn = || {
        vec![
            LlmEvent::ToolUse {
                id: "tool-1".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::ToolUse,
                ),
                usage: TokenUsage {
                    input_tokens: 50,
                    output_tokens: 20,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    ..Default::default()
                },
            },
        ]
    };

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        tool_use_turn(),
        tool_use_turn(),
    ]));

    let mut config = test_config();
    config.max_turns = Some(1);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let result = engine
        .run("Keep calling tools", "")
        .await
        .expect("should return Ok, not Err");

    assert_eq!(result.stop_reason, StopReason::MaxTurns);
    assert_eq!(result.turns, 1);
    // #457: the max_turns exit must surface finish_reason=max_turns (NOT length)
    // so the host offers "Continue" instead of the "use a bigger model" UX. This
    // exercises the real production path (finish_run_terminated), guarding against
    // the emit site regressing back to a hardcoded FinishReason.
    assert_eq!(
        result.finish_reason,
        wcore_types::message::FinishReason::MaxTurns,
        "max_turns run must emit finish_reason=max_turns"
    );
}

// ---------------------------------------------------------------------------
// a_turn_capped_run_admits_it_on_the_answer_stream
//
// A-10 (job-corpus survey 432c9a0f, video sub-case): the run died on the turn
// cap one step before it had the answer. The cap notice goes to `emit_info`,
// which is STDERR, and `AgentResult.text` is empty on this path — so the
// stdout a `-p` consumer reads ended mid-work and was scored as a WRONG
// ANSWER rather than as no answer.
//
// The admission has to travel on the same stream the answer travelled on.
// This asserts the text-delta stream carries it, and names the reason.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_turn_capped_run_admits_it_on_the_answer_stream() {
    let tool_use_turn = || {
        vec![
            LlmEvent::ToolUse {
                id: "tool-1".to_string(),
                name: "mock_tool".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::ToolUse,
                ),
                usage: TokenUsage::default(),
            },
        ]
    };
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        tool_use_turn(),
        tool_use_turn(),
    ]));
    let mut config = test_config();
    config.max_turns = Some(1);
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("mock_tool", "result", false)));

    let sink = Arc::new(wcore_agent::test_utils::TestSink::new());
    let handle = sink.handle();
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, sink);
    let result = engine
        .run("Keep calling tools", "")
        .await
        .expect("a turn-cap exit is a clean termination");
    assert_eq!(result.stop_reason, StopReason::MaxTurns);

    let answer_stream: String = handle
        .snapshot()
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("text_delta"))
        .filter_map(|e| e.get("text").and_then(|t| t.as_str()).map(str::to_owned))
        .collect();
    assert!(
        answer_stream.contains("[stopped early]"),
        "the answer stream must carry the admission, got: {answer_stream:?}"
    );
    assert!(
        answer_stream.contains("turn limit of 1"),
        "the admission must name the turn cap as the reason, got: {answer_stream:?}"
    );
    assert!(
        answer_stream.contains("not an answer"),
        "the admission must say the partial work is not an answer, got: {answer_stream:?}"
    );
}

// ---------------------------------------------------------------------------
// a_guard_stop_is_not_reported_as_the_turn_cap
//
// `StopReason::MaxTurns` is shared by the turn cap, the runaway-loop breaker,
// the consecutive-failure breaker and the pre-send budget denial. Measured on
// A-10 (green3): the failure-loop breaker stopped a run at turn 6 of a
// 20-turn budget and the first cut of the admission announced "hit its turn
// limit after 6 turns" - a manufactured explanation, in the one sentence
// whose whole job is to stop the product manufacturing things.
//
// With no turn cap configured at all, the admission must still arrive and
// must NOT claim a turn limit.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn a_guard_stop_is_not_reported_as_the_turn_cap() {
    let failing_turn = || {
        vec![
            LlmEvent::ToolUse {
                id: "tool-1".to_string(),
                name: "always_fails".to_string(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::ToolUse,
                ),
                usage: TokenUsage::default(),
            },
        ]
    };
    let provider = Arc::new(MockLlmProvider::with_turns(
        (0..40).map(|_| failing_turn()).collect::<Vec<_>>(),
    ));
    let mut config = test_config();
    config.max_turns = None;
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("always_fails", "boom", true)));

    let sink = Arc::new(wcore_agent::test_utils::TestSink::new());
    let handle = sink.handle();
    let mut engine = AgentEngine::new_with_provider(provider, config, registry, sink);
    let result = engine
        .run("Keep calling the broken tool", "")
        .await
        .expect("a guard stop is a clean termination");
    assert_eq!(
        result.stop_reason,
        StopReason::MaxTurns,
        "the guards share the MaxTurns verdict - that sharing is the hazard under test"
    );

    let answer_stream: String = handle
        .snapshot()
        .iter()
        .filter(|e| e.get("type").and_then(|t| t.as_str()) == Some("text_delta"))
        .filter_map(|e| e.get("text").and_then(|t| t.as_str()).map(str::to_owned))
        .collect();
    assert!(
        answer_stream.contains("[stopped early]"),
        "a guard stop must still admit itself, got: {answer_stream:?}"
    );
    assert!(
        !answer_stream.contains("turn limit"),
        "a guard stop must NOT be reported as the turn cap, got: {answer_stream:?}"
    );
    assert!(
        answer_stream.contains("a run guard stopped it"),
        "the admission must say a guard stopped it, got: {answer_stream:?}"
    );
}

// ---------------------------------------------------------------------------
// test_engine_api_error_handling
//
// AUDIT E-C2 — a mid-stream `LlmEvent::Error` is now a RETRYABLE failure,
// not an immediate fatal abort. The turn fails as `AgentError::ApiError`
// only after the bounded retry budget (1 initial + 2 retries) is
// exhausted, so every attempt must yield the error. This verifies the
// error message is still surfaced once retries are spent.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_engine_api_error_handling() {
    // Budget PINNED, not inherited. This test drives a provider that fails
    // every attempt; the shipped default is 10 retries on the shared backoff
    // curve (127.5 s of scheduled sleep), and what is under test here is the
    // failure OUTCOME, not the size of the budget.
    let _retry_budget = wcore_agent::test_utils::PinnedRetryBudget::pin(2);
    // 3 turns, each a mid-stream error — exhausts the stream-retry
    // budget so the run fails hard.
    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        vec![LlmEvent::Error("test error".to_string())],
        vec![LlmEvent::Error("test error".to_string())],
        vec![LlmEvent::Error("test error".to_string())],
    ]));
    let config = test_config();
    let registry = ToolRegistry::new();
    let output = silent_output();

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, output);
    let err = engine
        .run("Hello", "")
        .await
        .map(|_| panic!("expected error, got Ok"))
        .unwrap_err();

    match err {
        // The payload is the SAME user-facing prose the engine emitted, not
        // the bare provider reason. That is the convention the sibling refusal
        // in this very function already used (the output-stall gate emits
        // `gate_msg` and returns `ApiError(gate_msg)`); the retry-exhausted
        // branch was the outlier. No consumer matches on the payload -- the
        // only reads are `e.to_string()` for display and `ApiError(_)` variant
        // checks -- so the richer string costs nothing and tells the user what
        // actually happened and what to do next.
        //
        // Both halves are asserted. A payload that dropped the provider's own
        // words, or one that dropped the retry-exhaustion framing, would each
        // still satisfy a single `contains`.
        AgentError::ApiError(msg) => {
            assert!(
                msg.contains("test error"),
                "the provider's own words must survive into the payload, got: {msg}"
            );
            assert!(
                msg.contains("Provider stream failed after retries"),
                "the payload must say the retry budget was spent, got: {msg}"
            );
        }
        other => panic!("expected ApiError, got: {:?}", other),
    }
}
