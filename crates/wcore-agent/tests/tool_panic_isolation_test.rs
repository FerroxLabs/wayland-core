//! Wave RB RELIABILITY MAJOR: tool panic isolation.
//!
//! Asserts that a tool's `execute_with_ctx` panicking does NOT crash
//! the orchestration loop. The dispatcher's `catch_unwind` boundary
//! converts the panic into a structured `ToolResult { is_error: true }`
//! so the LLM context observes a normal tool failure and the session
//! can continue with subsequent calls.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::orchestration::execute_tool_calls;
use wcore_compact::CompactionLevel;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;
use wcore_types::tool::ToolResult;

/// A tool whose `execute` body panics. Wraps a panic-message string so
/// the test asserts that the panic payload makes it back through the
/// `catch_unwind` extractor.
struct PanicTool {
    panic_msg: &'static str,
}

#[async_trait]
impl Tool for PanicTool {
    fn name(&self) -> &str {
        "panic_tool"
    }
    fn description(&self) -> &str {
        "Panics on dispatch — used by RB error-isolation tests."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }
    async fn execute(&self, _input: Value) -> ToolResult {
        panic!("{}", self.panic_msg);
    }
}

/// A normal tool that returns a deterministic OK result. Used after a
/// PanicTool dispatch to prove the orchestration loop survived.
struct OkTool;

#[async_trait]
impl Tool for OkTool {
    fn name(&self) -> &str {
        "ok_tool"
    }
    fn description(&self) -> &str {
        "Returns success."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }
    async fn execute(&self, _input: Value) -> ToolResult {
        ToolResult {
            content: "ok-result".into(),
            is_error: false,
        }
    }
}

fn auto_approve() -> Arc<StdMutex<ToolConfirmer>> {
    Arc::new(StdMutex::new(ToolConfirmer::new(true, vec![])))
}

fn tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({}),
        extra: None,
    }
}

/// A panicking tool returns a synthetic is_error=true ToolResult with
/// the panic message embedded, instead of crashing the dispatcher.
#[tokio::test]
async fn panicking_tool_returns_structured_error_not_crash() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(PanicTool {
        panic_msg: "intentional test panic — RB isolation",
    }));

    let calls = vec![tool_use("call-panic", "panic_tool")];
    let confirmer = auto_approve();

    let outcome = execute_tool_calls(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("dispatch must not return ExecutionControl on panic");

    assert_eq!(outcome.results.len(), 1, "exactly one result expected");
    match &outcome.results[0] {
        ContentBlock::ToolResult {
            content,
            is_error,
            tool_use_id,
        } => {
            assert!(is_error, "panic must surface as is_error=true");
            assert_eq!(tool_use_id, "call-panic");
            assert!(
                content.contains("Tool panicked"),
                "result content should mention panic; got {content:?}"
            );
            assert!(
                content.contains("intentional test panic — RB isolation"),
                "panic message should be preserved in content; got {content:?}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// After a tool panic, the next dispatched tool call still runs and
/// returns its real result. This is the "session continues" invariant.
#[tokio::test]
async fn session_survives_panic_subsequent_tool_runs_normally() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(PanicTool {
        panic_msg: "first call panics",
    }));
    registry.register(Box::new(OkTool));

    // Two sequential dispatches: panic then ok.
    let calls = vec![
        tool_use("call-panic", "panic_tool"),
        tool_use("call-ok", "ok_tool"),
    ];
    let confirmer = auto_approve();

    let outcome = execute_tool_calls(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("dispatch must not return ExecutionControl on panic");

    assert_eq!(outcome.results.len(), 2);
    // First call: panic -> is_error=true
    match &outcome.results[0] {
        ContentBlock::ToolResult { is_error, .. } => {
            assert!(is_error, "first result must be the panicked tool's error");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    // Second call: ok -> is_error=false with the real content.
    match &outcome.results[1] {
        ContentBlock::ToolResult {
            is_error, content, ..
        } => {
            assert!(!is_error, "ok_tool should succeed after panic");
            assert_eq!(content, "ok-result");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// Non-string panic payloads still flow through the catch_unwind
/// boundary without crashing. The extracted message falls back to
/// the documented placeholder.
#[tokio::test]
async fn non_string_panic_payload_falls_back_to_placeholder() {
    struct NonStringPanicTool;
    #[async_trait]
    impl Tool for NonStringPanicTool {
        fn name(&self) -> &str {
            "non_string_panic"
        }
        fn description(&self) -> &str {
            ""
        }
        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
        fn is_concurrency_safe(&self, _: &Value) -> bool {
            false
        }
        async fn execute(&self, _: Value) -> ToolResult {
            // panic with a non-string payload (a struct without Display).
            std::panic::panic_any(42_u32);
        }
    }

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(NonStringPanicTool));

    let calls = vec![tool_use("c-x", "non_string_panic")];
    let confirmer = auto_approve();

    let outcome = execute_tool_calls(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("dispatch must not return ExecutionControl on panic");

    assert_eq!(outcome.results.len(), 1);
    match &outcome.results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(is_error);
            assert!(
                content.contains("non-string panic payload"),
                "non-string panic should yield placeholder text; got {content:?}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
