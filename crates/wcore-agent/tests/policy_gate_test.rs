//! v0.6.1 hardening (CRIT-1) — end-to-end test that
//! [`execute_tool_calls_with_policy_gate`] actually consults
//! `wcore_permissions::PolicyEngine` before tool dispatch.
//!
//! Before this wave, `wcore-permissions` shipped as orphan code: the
//! crate compiled and its unit tests passed in isolation, but no
//! consumer in the engine called `PolicyEngine::check`. These tests
//! pin the wiring against regression — if the gate stops calling the
//! engine, all four scenarios go red.

mod common;

use std::sync::Arc;

use common::{MockTool, auto_approve_confirmer};
use serde_json::json;
use wcore_agent::orchestration::execute_tool_calls_with_policy_gate;
use wcore_agent::policy_gate::PolicyGate;
use wcore_compact::CompactionLevel;
use wcore_permissions::{Action, Actor, Permission, PolicyEngine, Resource};
use wcore_tools::registry::ToolRegistry;
use wcore_types::message::ContentBlock;

fn tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input: json!({}),
        extra: None,
    }
}

fn registry_with_echo() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register(Box::new(MockTool::new("echo", "hello", false)));
    r.register(Box::new(MockTool::new("write_file", "ok", false)));
    r
}

/// No gate = old behaviour. Every tool call runs.
#[tokio::test]
async fn unconfigured_gate_allows_all_tools() {
    let registry = registry_with_echo();
    let calls = vec![tool_use("c1", "echo"), tool_use("c2", "write_file")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls_with_policy_gate(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
        None,
        None,
        None,
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch succeeds");

    assert_eq!(outcome.results.len(), 2);
    for block in &outcome.results {
        match block {
            ContentBlock::ToolResult { is_error, .. } => assert!(!is_error),
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}

/// Empty engine (zero grants) denies every tool.
#[tokio::test]
async fn empty_engine_denies_all_tools() {
    let registry = registry_with_echo();
    let gate = PolicyGate::new(Arc::new(PolicyEngine::new()), Actor::User("default".into()));
    let calls = vec![tool_use("c1", "echo"), tool_use("c2", "write_file")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls_with_policy_gate(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
        None,
        None,
        Some(&gate),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch succeeds even when gate denies");

    assert_eq!(outcome.results.len(), 2);
    for block in &outcome.results {
        match block {
            ContentBlock::ToolResult {
                is_error, content, ..
            } => {
                assert!(*is_error, "denied tool must surface as error");
                assert!(
                    content.starts_with("Denied by policy"),
                    "expected policy-deny message, got: {content}"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}

/// Selective grant: only `echo` is permitted. `write_file` denied.
/// Order must be preserved across the partition.
#[tokio::test]
async fn selective_grant_allows_one_denies_other_preserving_order() {
    let registry = registry_with_echo();

    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("default".into()),
        resource: Resource::Tool("echo".into()),
        action: Action::Invoke,
    });
    let gate = PolicyGate::new(Arc::new(engine), Actor::User("default".into()));

    // Interleaved order: denied, allowed, denied, allowed. The wrapper
    // must restore original positions, not group allowed/denied.
    let calls = vec![
        tool_use("c1", "write_file"),
        tool_use("c2", "echo"),
        tool_use("c3", "write_file"),
        tool_use("c4", "echo"),
    ];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls_with_policy_gate(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
        None,
        None,
        Some(&gate),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch succeeds");

    assert_eq!(outcome.results.len(), 4);

    // c1: denied
    match &outcome.results[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "c1");
            assert!(*is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    // c2: allowed
    match &outcome.results[1] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "c2");
            assert!(!is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    // c3: denied
    match &outcome.results[2] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "c3");
            assert!(*is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
    // c4: allowed
    match &outcome.results[3] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "c4");
            assert!(!is_error);
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

/// Denied tool must NOT reach the underlying `MockTool::execute`.
///
/// This is implicit in `empty_engine_denies_all_tools` and
/// `selective_grant_allows_one_denies_other_preserving_order` above:
/// a denied `ToolResult.content` starts with "Denied by policy: ...",
/// which only the gate produces. A `MockTool` that actually ran would
/// return its configured result string instead. So if those two tests
/// pass, denial is a true short-circuit — the dispatcher never saw
/// the call.
#[tokio::test]
async fn denied_tool_result_content_proves_short_circuit() {
    // Mock returns "tool-ran-anyway" so we can distinguish gate output
    // from real dispatch output.
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(MockTool::new("explode", "tool-ran-anyway", false)));

    let gate = PolicyGate::new(Arc::new(PolicyEngine::new()), Actor::User("default".into()));
    let calls = vec![tool_use("c1", "explode")];
    let confirmer = auto_approve_confirmer();

    let outcome = execute_tool_calls_with_policy_gate(
        &registry,
        &calls,
        &confirmer,
        None,
        CompactionLevel::Off,
        false,
        None,
        None,
        Some(&gate),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch succeeds");

    match &outcome.results[0] {
        ContentBlock::ToolResult { content, .. } => {
            assert!(
                content.starts_with("Denied by policy"),
                "gate must short-circuit; got content: {content}"
            );
            assert!(
                !content.contains("tool-ran-anyway"),
                "underlying MockTool must NOT have executed; got: {content}"
            );
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}
