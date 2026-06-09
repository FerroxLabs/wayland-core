//! Integration tests for the agent-level `HookEngine` wrapper
//! (`wcore_agent::hooks`). Exercises every `HookAction` variant
//! across the five lifecycle phases plus the composition with the
//! shell-side `ShellHooks` engine.

use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_agent::hooks::{
    Hook, HookAction, HookEngine, HookOutcome, SessionEndSummary, TurnContext, TurnResult,
};
use wcore_config::hooks::{HookDef, HooksConfig};
use wcore_types::message::{ContentBlock, Message, Role};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

struct ContinueHook {
    name: String,
}

#[async_trait]
impl Hook for ContinueHook {
    fn name(&self) -> &str {
        &self.name
    }
}

struct BlockPreHook {
    name: String,
    reason: String,
}

#[async_trait]
impl Hook for BlockPreHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        HookAction::Block {
            reason: self.reason.clone(),
        }
    }
}

struct ModifyPreHook {
    name: String,
    value: Value,
}

#[async_trait]
impl Hook for ModifyPreHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn pre_tool_use(&self, _tool: &str, _input: &Value) -> HookAction {
        HookAction::ModifyInput(self.value.clone())
    }
}

struct InjectPostHook {
    name: String,
    msg: Message,
}

#[async_trait]
impl Hook for InjectPostHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn post_tool_use(
        &self,
        _tool: &str,
        _call_id: &str,
        _input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> HookAction {
        HookAction::InjectMessage(self.msg.clone())
    }
}

struct SwitchModelPostHook {
    name: String,
    target: String,
}

#[async_trait]
impl Hook for SwitchModelPostHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn post_tool_use(
        &self,
        _tool: &str,
        _call_id: &str,
        _input: &Value,
        _output: &str,
        _is_error: bool,
    ) -> HookAction {
        HookAction::SwitchModel(self.target.clone())
    }
}

/// Records the order in which `on_turn_start` is called on each instance.
struct RecorderTurnStartHook {
    name: String,
    log: std::sync::Arc<Mutex<Vec<(String, usize)>>>,
}

#[async_trait]
impl Hook for RecorderTurnStartHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn on_turn_start(&self, turn: usize, _ctx: &TurnContext) -> HookAction {
        self.log.lock().unwrap().push((self.name.clone(), turn));
        HookAction::Continue
    }
}

/// Records that on_session_end fired; used to confirm shell-side run_stop
/// is NOT invoked by on_session_end.
struct RecorderSessionEndHook {
    name: String,
    fired: std::sync::Arc<Mutex<bool>>,
}

#[async_trait]
impl Hook for RecorderSessionEndHook {
    fn name(&self) -> &str {
        &self.name
    }
    async fn on_session_end(&self, _summary: &SessionEndSummary) -> HookAction {
        *self.fired.lock().unwrap() = true;
        HookAction::Continue
    }
}

fn make_pre_hook_def(name: &str, command: &str) -> HookDef {
    HookDef {
        name: name.to_string(),
        tool_match: vec!["Read".to_string()],
        file_match: vec![],
        command: command.to_string(),
        timeout_ms: 5_000,
    }
}

fn make_post_hook_def(name: &str, command: &str) -> HookDef {
    HookDef {
        name: name.to_string(),
        tool_match: vec!["Read".to_string()],
        file_match: vec![],
        command: command.to_string(),
        timeout_ms: 5_000,
    }
}

fn empty_outcome_assertions(outcome: &HookOutcome) {
    assert!(outcome.block.is_none());
    assert!(outcome.modified_input.is_none());
    assert!(outcome.injected_messages.is_empty());
    assert!(outcome.switch_model.is_none());
}

// ---------------------------------------------------------------------------
// pre_tool_use tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pre_tool_use_continue_returns_empty_outcome() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(ContinueHook {
        name: "noop".into(),
    }));

    let outcome = engine
        .run_pre_tool_use("Read", &json!({}))
        .await
        .expect("no block");

    empty_outcome_assertions(&outcome);
    assert!(outcome.log_lines.is_empty());
}

#[tokio::test]
async fn pre_tool_use_block_sets_block_field() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(BlockPreHook {
        name: "blocker".into(),
        reason: "policy violation".into(),
    }));
    // Second hook would set modified_input, but we expect Block to short-circuit.
    engine.register_rust_hook(Box::new(ModifyPreHook {
        name: "later".into(),
        value: json!({"after": "block"}),
    }));

    let outcome = engine
        .run_pre_tool_use("Read", &json!({}))
        .await
        .expect("Ok(outcome) even on Block — shell hooks just don't run");

    assert_eq!(outcome.block.as_deref(), Some("policy violation"));
    assert!(
        outcome.modified_input.is_none(),
        "Block must short-circuit: later hooks must not run"
    );
}

#[tokio::test]
async fn pre_tool_use_modify_input_last_wins() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(ModifyPreHook {
        name: "first".into(),
        value: json!({"v": 1}),
    }));
    engine.register_rust_hook(Box::new(ModifyPreHook {
        name: "second".into(),
        value: json!({"v": 2}),
    }));

    let outcome = engine
        .run_pre_tool_use("Read", &json!({}))
        .await
        .expect("no block");

    assert_eq!(outcome.modified_input, Some(json!({"v": 2})));
}

#[tokio::test]
async fn pre_tool_use_shell_block_propagates() {
    // Rust hook says Continue; shell hook with `exit 1` blocks.
    #[cfg(unix)]
    let cmd = "exit 1";
    #[cfg(windows)]
    let cmd = "exit 1";

    let config = HooksConfig {
        pre_tool_use: vec![make_pre_hook_def("shell-blocker", cmd)],
        post_tool_use: vec![],
        stop: vec![],
        ..Default::default()
    };
    let mut engine = HookEngine::new(config);
    engine.register_rust_hook(Box::new(ContinueHook {
        name: "rust-noop".into(),
    }));

    let result = engine.run_pre_tool_use("Read", &json!({})).await;
    assert!(result.is_err(), "shell hook with exit 1 should block");
}

#[tokio::test]
async fn pre_tool_use_rust_block_short_circuits_shell() {
    // If the shell hook ran it would error (touch into a non-existent dir
    // would still be cheap; instead use exit 1 which we know blocks).
    // The assertion is that the *shell* error never reaches us — Rust Block
    // returns Ok(outcome) immediately.
    let config = HooksConfig {
        pre_tool_use: vec![make_pre_hook_def("would-block", "exit 1")],
        post_tool_use: vec![],
        stop: vec![],
        ..Default::default()
    };
    let mut engine = HookEngine::new(config);
    engine.register_rust_hook(Box::new(BlockPreHook {
        name: "rust-blocker".into(),
        reason: "first".into(),
    }));

    let outcome = engine
        .run_pre_tool_use("Read", &json!({}))
        .await
        .expect("Rust Block returns Ok, never invokes shell");

    assert_eq!(outcome.block.as_deref(), Some("first"));
}

// ---------------------------------------------------------------------------
// post_tool_use tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_tool_use_rust_inject_appends_to_outcome() {
    let mut engine = HookEngine::new(HooksConfig::default());
    let msg = Message::new(
        Role::User,
        vec![ContentBlock::Text {
            text: "hello".into(),
        }],
    );
    engine.register_rust_hook(Box::new(InjectPostHook {
        name: "injector".into(),
        msg: msg.clone(),
    }));

    let outcome = engine
        .run_post_tool_use("Read", "call-1", &json!({}), "result", false)
        .await;

    assert_eq!(outcome.injected_messages.len(), 1);
}

#[tokio::test]
async fn post_tool_use_rust_switch_model_last_wins() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(SwitchModelPostHook {
        name: "first".into(),
        target: "a".into(),
    }));
    engine.register_rust_hook(Box::new(SwitchModelPostHook {
        name: "second".into(),
        target: "b".into(),
    }));

    let outcome = engine
        .run_post_tool_use("Read", "call-1", &json!({}), "result", false)
        .await;

    assert_eq!(outcome.switch_model.as_deref(), Some("b"));
}

#[tokio::test]
async fn post_tool_use_shell_log_lines_appended() {
    let config = HooksConfig {
        pre_tool_use: vec![],
        post_tool_use: vec![make_post_hook_def("post-logger", "echo done")],
        stop: vec![],
        ..Default::default()
    };
    let engine = HookEngine::new(config);

    let outcome = engine
        .run_post_tool_use("Read", "call-1", &json!({}), "result", false)
        .await;

    assert!(!outcome.log_lines.is_empty());
    assert!(
        outcome.log_lines.iter().any(|l| l.contains("done")),
        "expected 'done' in {:?}",
        outcome.log_lines
    );
}

// ---------------------------------------------------------------------------
// on_turn_start / on_turn_end / on_session_end tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn on_turn_start_runs_every_rust_hook_in_order() {
    let log = std::sync::Arc::new(Mutex::new(Vec::<(String, usize)>::new()));
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(RecorderTurnStartHook {
        name: "first".into(),
        log: log.clone(),
    }));
    engine.register_rust_hook(Box::new(RecorderTurnStartHook {
        name: "second".into(),
        log: log.clone(),
    }));

    let ctx = TurnContext {
        turn: 7,
        model: "test".into(),
        message_count: 0,
    };
    let _outcome = engine.on_turn_start(7, &ctx).await;

    let recorded = log.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0], ("first".to_string(), 7));
    assert_eq!(recorded[1], ("second".to_string(), 7));
}

#[tokio::test]
async fn on_turn_end_continue_yields_empty_outcome() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(ContinueHook {
        name: "noop".into(),
    }));

    let result = TurnResult {
        turn: 1,
        tool_call_count: 0,
        input_tokens: 0,
        output_tokens: 0,
    };
    let outcome = engine.on_turn_end(1, &result).await;
    empty_outcome_assertions(&outcome);
}

#[tokio::test]
async fn on_session_end_runs_only_rust_hooks() {
    // Configure a shell stop hook; it must NOT fire on on_session_end.
    let config = HooksConfig {
        pre_tool_use: vec![],
        post_tool_use: vec![],
        stop: vec![HookDef {
            name: "should-not-fire".into(),
            tool_match: vec![],
            file_match: vec![],
            command: "echo SHELL-STOP-FIRED".into(),
            timeout_ms: 5_000,
        }],
        ..Default::default()
    };
    let mut engine = HookEngine::new(config);
    let fired = std::sync::Arc::new(Mutex::new(false));
    engine.register_rust_hook(Box::new(RecorderSessionEndHook {
        name: "rust-end".into(),
        fired: fired.clone(),
    }));

    let summary = SessionEndSummary {
        turns: 3,
        total_input_tokens: 100,
        total_output_tokens: 50,
    };
    let outcome = engine.on_session_end(&summary).await;

    assert!(*fired.lock().unwrap(), "Rust on_session_end must fire");
    assert!(
        !outcome.log_lines.iter().any(|l| l.contains("SHELL-STOP")),
        "shell stop hook must NOT fire on on_session_end: {:?}",
        outcome.log_lines
    );
}

// ---------------------------------------------------------------------------
// has_hooks / register_rust_hook / merge_hooks tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_rust_hook_after_construction() {
    let mut engine = HookEngine::new(HooksConfig::default());
    assert!(!engine.has_hooks());
    engine.register_rust_hook(Box::new(ContinueHook { name: "h".into() }));
    assert!(engine.has_hooks());
}

#[test]
fn has_hooks_false_with_neither() {
    let engine = HookEngine::new(HooksConfig::default());
    assert!(!engine.has_hooks());
}

#[test]
fn has_hooks_true_with_shell_only() {
    let config = HooksConfig {
        pre_tool_use: vec![make_pre_hook_def("pre", "echo ok")],
        post_tool_use: vec![],
        stop: vec![],
        ..Default::default()
    };
    let engine = HookEngine::new(config);
    assert!(engine.has_hooks());
}

#[test]
fn has_hooks_true_with_rust_only() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(ContinueHook { name: "h".into() }));
    assert!(engine.has_hooks());
}

#[tokio::test]
async fn merge_hooks_only_touches_shell_side() {
    let mut engine = HookEngine::new(HooksConfig::default());
    engine.register_rust_hook(Box::new(ContinueHook {
        name: "preserved".into(),
    }));

    let additional = HooksConfig {
        pre_tool_use: vec![make_pre_hook_def("merged", "exit 1")],
        post_tool_use: vec![],
        stop: vec![],
        ..Default::default()
    };
    engine.merge_hooks(additional);

    // The Rust hook is still there (it returns Continue, so we proceed
    // to the shell side which has the merged "exit 1" hook and blocks).
    let result = engine.run_pre_tool_use("Read", &json!({})).await;
    assert!(result.is_err(), "merged shell hook should block via exit 1");
}
