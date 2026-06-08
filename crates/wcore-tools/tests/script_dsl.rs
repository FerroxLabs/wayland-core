//! Script DSL: type parsing + ${ref} resolution + executor behaviour.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::RwLock;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::dispatcher::{ClosureDispatcher, ToolDispatcher};
use wcore_tools::registry::ToolRegistry;
use wcore_tools::script::{ScriptInput, ScriptStep, ScriptTool, StepError, resolve_input};
use wcore_types::tool::ToolResult;

fn outputs(pairs: &[(&str, Value)]) -> std::collections::HashMap<String, Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

// ---------------------------------------------------------------------------
// DSL parsing + ref resolution
// ---------------------------------------------------------------------------

#[test]
fn parses_minimal_script_input() {
    let raw = json!({
        "steps": [{ "id": "s1", "tool": "Grep", "input": {"pattern": "fn"} }],
        "max_output_lines": 100
    });
    let parsed: ScriptInput = serde_json::from_value(raw).expect("parse");
    assert_eq!(parsed.steps.len(), 1);
    assert_eq!(parsed.steps[0].id, "s1");
    assert!(!parsed.steps[0].approval_required);
    assert_eq!(parsed.max_output_lines, Some(100));
}

#[test]
fn resolve_input_substitutes_plain_string_ref() {
    let outs = outputs(&[("s1", json!({"matches": [{"file": "src/lib.rs"}]}))]);
    let resolved = resolve_input(&json!({"path": "${s1.matches.0.file}"}), &outs).unwrap();
    assert_eq!(resolved["path"], "src/lib.rs");
}

#[test]
fn resolve_input_substitutes_nested_object_value() {
    let outs = outputs(&[("s1", json!({"content": "hello world"}))]);
    let resolved = resolve_input(
        &json!({"edits": {"old_string": "${s1.content}", "new_string": "goodbye world"}}),
        &outs,
    )
    .unwrap();
    assert_eq!(resolved["edits"]["old_string"], "hello world");
    assert_eq!(resolved["edits"]["new_string"], "goodbye world");
}

#[test]
fn resolve_input_unknown_ref_returns_typed_error() {
    let outs = outputs(&[]);
    match resolve_input(&json!({"x": "${s99.nope}"}), &outs) {
        Err(StepError::UnknownRef(r)) => assert_eq!(r, "s99.nope"),
        other => panic!("expected UnknownRef, got {other:?}"),
    }
}

#[test]
fn resolve_input_invalid_path_returns_typed_error() {
    let outs = outputs(&[("s1", json!({"matches": []}))]);
    match resolve_input(&json!({"x": "${s1.matches.0.file}"}), &outs) {
        Err(StepError::RefPathMiss { ref_expr, .. }) => {
            assert!(ref_expr.contains("s1.matches.0.file"));
        }
        other => panic!("expected RefPathMiss, got {other:?}"),
    }
}

#[test]
fn ref_resolver_rejects_expression_syntax() {
    // Safety rail: the resolver is json-path only — no arithmetic, no shell.
    let outs = outputs(&[("s1", json!({"n": 5}))]);
    match resolve_input(&json!({"x": "${s1.n + 1}"}), &outs) {
        Err(StepError::RefSyntax(_)) => {}
        other => panic!("expected RefSyntax error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

struct CannedTool {
    name: String,
    output: String,
    cat: ToolCategory,
}

#[async_trait]
impl Tool for CannedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "canned"
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool {
        true
    }
    async fn execute(&self, _: Value) -> ToolResult {
        ToolResult {
            content: self.output.clone(),
            is_error: false,
        }
    }
    fn category(&self) -> ToolCategory {
        self.cat
    }
}

fn dispatcher_with(tools: Vec<(&str, &str, ToolCategory)>) -> Arc<dyn ToolDispatcher> {
    let mut reg = ToolRegistry::new();
    for (n, out, cat) in tools {
        reg.register(Box::new(CannedTool {
            name: n.to_string(),
            output: out.to_string(),
            cat,
        }));
    }
    let shared = Arc::new(RwLock::new(reg));
    Arc::new(ClosureDispatcher::new(Box::new(move |tool, input| {
        let reg = Arc::clone(&shared);
        Box::pin(async move {
            let guard = reg.read().await;
            match guard.get(&tool) {
                Some(t) => t.execute(input).await,
                None => ToolResult {
                    content: format!("not in registry: {tool}"),
                    is_error: true,
                },
            }
        })
    })))
}

#[tokio::test]
async fn script_runs_steps_in_order_and_aggregates_outputs() {
    let disp = dispatcher_with(vec![
        (
            "Grep",
            r#"{"matches": [{"file": "lib.rs"}]}"#,
            ToolCategory::Info,
        ),
        ("Read", "fn main() {}\n", ToolCategory::Info),
    ]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Grep", "input": {"pattern": "fn"}},
            {"id": "s2", "tool": "Read", "input": {"path": "${s1.matches.0.file}"}}
        ],
        "max_output_lines": 10
    });
    let result = tool.execute(input).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("fn main"));
    assert!(result.content.contains("s1"));
    assert!(result.content.contains("s2"));
}

#[tokio::test]
async fn script_rejects_tool_outside_allow_list() {
    let disp = dispatcher_with(vec![("Grep", "{}", ToolCategory::Info)]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    // SpawnTool name is the canonical disallowed tool.
    let input = json!({
        "steps": [{"id": "s1", "tool": "SpawnTool", "input": {}}],
        "max_output_lines": 10
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(result.content.contains("not in the Script allow-list"));
}

#[tokio::test]
async fn script_rejects_recursive_script_step() {
    let disp = dispatcher_with(vec![("Script", "{}", ToolCategory::Info)]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [{"id": "s1", "tool": "Script", "input": {}}],
        "max_output_lines": 10
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("not in the Script allow-list"),
        "expected allow-list error, got: {}",
        result.content
    );
}

#[tokio::test]
async fn script_truncates_output_to_max_lines() {
    let big = (0..500)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let disp = dispatcher_with(vec![("Grep", &big, ToolCategory::Info)]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [{"id": "s1", "tool": "Grep", "input": {}}],
        "max_output_lines": 50
    });
    let result = tool.execute(input).await;
    assert!(!result.is_error);
    let line_count = result.content.matches('\n').count();
    assert!(line_count <= 60, "{line_count} lines after truncation");
    assert!(result.content.contains("truncated"));
}

#[tokio::test]
async fn script_step_failure_short_circuits() {
    struct FailTool;
    #[async_trait]
    impl Tool for FailTool {
        fn name(&self) -> &str {
            "Bash"
        }
        fn description(&self) -> &str {
            "fails"
        }
        fn input_schema(&self) -> Value {
            json!({})
        }
        fn is_concurrency_safe(&self, _: &Value) -> bool {
            true
        }
        async fn execute(&self, _: Value) -> ToolResult {
            ToolResult {
                content: "boom".into(),
                is_error: true,
            }
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::Exec
        }
    }
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(FailTool));
    reg.register(Box::new(CannedTool {
        name: "Read".into(),
        output: "ran".into(),
        cat: ToolCategory::Info,
    }));
    let shared = Arc::new(RwLock::new(reg));
    let disp: Arc<dyn ToolDispatcher> =
        Arc::new(ClosureDispatcher::new(Box::new(move |tool, input| {
            let reg = Arc::clone(&shared);
            Box::pin(async move {
                let guard = reg.read().await;
                match guard.get(&tool) {
                    Some(t) => t.execute(input).await,
                    None => ToolResult {
                        content: format!("not in registry: {tool}"),
                        is_error: true,
                    },
                }
            })
        })));
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Bash", "input": {}},
            {"id": "s2", "tool": "Read", "input": {}}
        ]
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(result.content.contains("s1"));
    assert!(!result.content.contains("ran"), "s2 must not have run");
}

#[tokio::test]
async fn script_rejects_duplicate_step_id() {
    let disp = dispatcher_with(vec![("Read", "{}", ToolCategory::Info)]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Read", "input": {}},
            {"id": "s1", "tool": "Read", "input": {}}
        ]
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("DuplicateStepId") || result.content.contains("duplicate"),
        "got: {}",
        result.content
    );
}

#[tokio::test]
async fn script_approval_required_short_circuits_without_bridge() {
    // Pre-W7 behaviour preserved: ScriptTool::new(disp) without
    // .with_approval(...) still short-circuits on approval_required
    // steps. The W7-aware error message now mentions the bridge so
    // callers know to opt in via the builder. Full round-trip lives
    // in `wcore-agent/tests/approval_round_trip.rs`.
    let disp = dispatcher_with(vec![("Bash", "{}", ToolCategory::Exec)]);
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [{"id": "s1", "tool": "Bash", "input": {"command": "rm -rf /"}, "approval_required": true}]
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("approval"),
        "expected approval message, got: {}",
        result.content
    );
}

// Construct a minimal helper to silence unused-warning if any:
fn _construct(_step: ScriptStep) {}
