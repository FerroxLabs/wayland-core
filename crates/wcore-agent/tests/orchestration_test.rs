mod common;

use common::{MockTool, auto_approve_confirmer};
use serde_json::{Value, json};
use wcore_agent::hooks::{Hook, HookAction, HookEngine};
use wcore_agent::orchestration::execute_tool_calls;
use wcore_compact::CompactionLevel;
use wcore_config::hooks::{HookDef, HooksConfig};
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input: json!({}),
        extra: None,
    }
}

fn make_pre_hook(name: &str, tool_match: &str, command: &str) -> HookDef {
    HookDef {
        name: name.to_string(),
        tool_match: vec![tool_match.to_string()],
        file_match: vec![],
        command: command.to_string(),
        timeout_ms: 5_000,
    }
}

fn make_post_hook(name: &str, tool_match: &str, command: &str) -> HookDef {
    HookDef {
        name: name.to_string(),
        tool_match: vec![tool_match.to_string()],
        file_match: vec![],
        command: command.to_string(),
        timeout_ms: 5_000,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Single tool call executes and returns the expected result
#[tokio::test]
async fn test_execute_single_tool_call() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("echo", "hello", false)));

    let tool_calls = vec![make_tool_use("call-1", "echo")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            assert_eq!(tool_use_id, "call-1");
            assert_eq!(content, "hello");
            assert!(!is_error);
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// Two concurrent-safe tools execute in parallel and both return results
#[tokio::test]
async fn test_execute_concurrent_safe_tools() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("tool_a", "result_a", false)));
    registry.register(Box::new(MockTool::new("tool_b", "result_b", false)));

    let tool_calls = vec![
        make_tool_use("id-a", "tool_a"),
        make_tool_use("id-b", "tool_b"),
    ];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 2);

    // Collect content strings keyed by tool_use_id for order-independent assertion
    let content_map: std::collections::HashMap<_, _> = results
        .iter()
        .filter_map(|r| match r {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();

    assert_eq!(content_map.get("id-a"), Some(&"result_a"));
    assert_eq!(content_map.get("id-b"), Some(&"result_b"));
}

/// Two sequential (non-concurrent) tools execute one after the other and both succeed
#[tokio::test]
async fn test_execute_non_concurrent_tools_sequential() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::sequential("seq_a", "seq_result_a")));
    registry.register(Box::new(MockTool::sequential("seq_b", "seq_result_b")));

    let tool_calls = vec![
        make_tool_use("id-a", "seq_a"),
        make_tool_use("id-b", "seq_b"),
    ];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 2);

    let content_map: std::collections::HashMap<_, _> = results
        .iter()
        .filter_map(|r| match r {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();

    assert_eq!(content_map.get("id-a"), Some(&"seq_result_a"));
    assert_eq!(content_map.get("id-b"), Some(&"seq_result_b"));
}

/// Calling a tool that is not registered returns an error ToolResult with "Unknown tool"
#[tokio::test]
async fn test_unknown_tool_returns_error() {
    let registry = ToolRegistry::new(); // empty registry

    let tool_calls = vec![make_tool_use("id-x", "nonexistent_tool")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execute_tool_calls itself should not fail");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(*is_error, "unknown tool should produce is_error = true");
            assert!(
                content.contains("Unknown tool"),
                "error message should mention 'Unknown tool', got: {}",
                content
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// A tool that signals an error surfaces is_error = true in the result
#[tokio::test]
async fn test_tool_error_returns_error_result() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("fail_tool", "error message", true)));

    let tool_calls = vec![make_tool_use("id-fail", "fail_tool")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(*is_error, "tool error should propagate as is_error = true");
            assert_eq!(content, "error message");
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// A pre-tool-use hook that exits with a non-zero status blocks tool execution
#[tokio::test]
async fn test_pre_hook_blocks_tool() {
    let hook_config = HooksConfig {
        pre_tool_use: vec![make_pre_hook("blocker", "echo", "exit 1")],
        post_tool_use: vec![],
        stop: vec![],
        ..Default::default()
    };
    let mut hook_engine = HookEngine::new(hook_config);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("echo", "should not appear", false)));

    let tool_calls = vec![make_tool_use("id-blocked", "echo")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        Some(&mut hook_engine),
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execute_tool_calls itself should not fail");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(
                *is_error,
                "blocked execution should produce is_error = true"
            );
            assert!(
                content.contains("Blocked by hook"),
                "result should mention 'Blocked by hook', got: {}",
                content
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// A post-tool-use hook runs after the tool but does not alter the tool's result
#[tokio::test]
async fn test_post_hook_runs_after_tool() {
    let hook_config = HooksConfig {
        pre_tool_use: vec![],
        post_tool_use: vec![make_post_hook("post-logger", "echo", "echo done")],
        stop: vec![],
        ..Default::default()
    };
    let mut hook_engine = HookEngine::new(hook_config);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("echo", "result", false)));

    let tool_calls = vec![make_tool_use("id-post", "echo")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        Some(&mut hook_engine),
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            // Post-hooks must not mutate the tool result
            assert!(!is_error);
            assert_eq!(content, "result");
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

/// Results that exceed max_result_size are truncated with a "[truncated N chars]" marker
#[tokio::test]
async fn test_tool_result_truncation() {
    // Default max_result_size is 50_000; build a result that exceeds it
    let long_result: String = "x".repeat(60_000);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("big_tool", &long_result, false)));

    let tool_calls = vec![make_tool_use("id-big", "big_tool")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(!is_error);
            assert!(
                content.len() < long_result.len(),
                "truncated result should be shorter than the original"
            );
            assert!(
                content.contains("truncated"),
                "truncated result should contain the word 'truncated', got length {}",
                content.len()
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// W2 (F1) — Rust hook integration tests
// ---------------------------------------------------------------------------

/// A Rust pre_tool_use hook that blocks every call with a fixed reason.
struct BlockHook {
    reason: String,
}

#[async_trait::async_trait]
impl Hook for BlockHook {
    fn name(&self) -> &str {
        "test-blocker"
    }
    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        HookAction::Block {
            reason: self.reason.clone(),
        }
    }
}

/// A Rust pre_tool_use hook that overwrites the tool input.
struct ModifyHook {
    new_input: Value,
}

#[async_trait::async_trait]
impl Hook for ModifyHook {
    fn name(&self) -> &str {
        "test-modifier"
    }
    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        HookAction::ModifyInput(self.new_input.clone())
    }
}

/// Echoes the JSON input back as the tool result content. Used to assert
/// that ModifyInput actually replaced the input the tool saw.
struct EchoInputTool;

#[async_trait::async_trait]
impl wcore_tools::Tool for EchoInputTool {
    fn name(&self) -> &str {
        "echo_input"
    }
    fn description(&self) -> &str {
        "Echoes the JSON input verbatim"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn category(&self) -> wcore_protocol::events::ToolCategory {
        wcore_protocol::events::ToolCategory::Info
    }
    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }
    async fn execute(&self, input: Value) -> wcore_types::tool::ToolResult {
        wcore_types::tool::ToolResult {
            content: serde_json::to_string(&input).unwrap_or_default(),
            is_error: false,
        }
    }
}

#[tokio::test]
async fn rust_hook_block_synthesises_error_tool_result() {
    let mut hook_engine = HookEngine::new(HooksConfig::default());
    hook_engine.register_rust_hook(Box::new(BlockHook {
        reason: "policy".into(),
    }));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("echo", "should not run", false)));

    let tool_calls = vec![make_tool_use("call-blocked", "echo")];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        Some(&mut hook_engine),
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(*is_error, "Rust hook Block must produce is_error=true");
            assert!(
                content.contains("policy"),
                "synthetic error should include the Block reason, got: {content}"
            );
            assert!(
                !content.contains("should not run"),
                "tool body must not have executed, got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}

#[tokio::test]
async fn rust_hook_modify_input_changes_tool_invocation() {
    let mut hook_engine = HookEngine::new(HooksConfig::default());
    hook_engine.register_rust_hook(Box::new(ModifyHook {
        new_input: json!({"injected": true}),
    }));

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(EchoInputTool));

    let tool_calls = vec![ContentBlock::ToolUse {
        id: "call-modify".into(),
        name: "echo_input".into(),
        input: json!({"original": "yes"}),
        extra: None,
    }];
    let confirmer = auto_approve_confirmer();

    let results = execute_tool_calls(
        &registry,
        &tool_calls,
        &confirmer,
        Some(&mut hook_engine),
        CompactionLevel::Off,
        false,
    )
    .await
    .expect("execution should succeed");

    assert_eq!(results.len(), 1);
    match &results[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(!is_error);
            assert!(
                content.contains("injected"),
                "ModifyInput must replace the input the tool sees, got: {content}"
            );
            assert!(
                !content.contains("original"),
                "original input must be replaced, got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {:?}", other),
    }
}
