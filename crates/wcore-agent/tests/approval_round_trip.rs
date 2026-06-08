//! W7 S4-3 integration test: Script step with approval_required +
//! ApprovalBridge round-trip.
//!
//! Builds an ApprovalBridge as `Arc<ApprovalBridge>` (kept for direct
//! resolution from the test scope) AND passes it as
//! `Arc<dyn ApprovalProducer>` to the tool (the trait surface
//! consumed by `ScriptTool::with_approval`). The same Arc cloned twice
//! into both shapes — no downcasting needed.

use std::sync::{Arc, Mutex};

use serde_json::json;
use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome};
use wcore_tools::Tool;
use wcore_tools::dispatcher::{ClosureDispatcher, ToolDispatcher};
use wcore_tools::script::{ApprovalProducer, ScriptOutputSink, ScriptTool};
use wcore_types::tool::ToolResult;

#[derive(Default)]
struct CapScriptSink {
    required: Mutex<Vec<(String, String, String, String)>>, // call_id, token, reason, ctx
    suspend: Mutex<Vec<(String, String)>>,                  // reason, token
}

impl ScriptOutputSink for CapScriptSink {
    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        reason: &str,
        context: &str,
    ) {
        self.required.lock().unwrap().push((
            call_id.into(),
            resume_token.into(),
            reason.into(),
            context.into(),
        ));
    }
    fn emit_suspend(&self, reason: &str, resume_token: &str) {
        self.suspend
            .lock()
            .unwrap()
            .push((reason.into(), resume_token.into()));
    }
}

fn dispatcher_returns(content: &'static str) -> Arc<dyn ToolDispatcher> {
    Arc::new(ClosureDispatcher::new(Box::new(move |_tool, _input| {
        Box::pin(async move {
            ToolResult {
                content: content.to_string(),
                is_error: false,
            }
        })
    })))
}

async fn await_pending_token(bridge: &Arc<ApprovalBridge>) -> String {
    loop {
        let pending = bridge.pending_tokens().await;
        if let Some(token) = pending.into_iter().next() {
            return token;
        }
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn script_approval_gate_dispatches_when_approved() {
    let bridge: Arc<ApprovalBridge> = Arc::new(ApprovalBridge::new());
    let bridge_producer: Arc<dyn ApprovalProducer> = bridge.clone() as Arc<dyn ApprovalProducer>;
    let sink: Arc<CapScriptSink> = Arc::new(CapScriptSink::default());
    let sink_for_tool: Arc<dyn ScriptOutputSink> = sink.clone() as Arc<dyn ScriptOutputSink>;

    let disp = dispatcher_returns("step-output-ok");
    let tool = ScriptTool::new(Arc::clone(&disp)).with_approval(bridge_producer, sink_for_tool);

    let input = json!({
        "steps": [
            {"id": "s1", "tool": "Bash", "input": {"command": "echo hi"}, "approval_required": true}
        ]
    });

    let approver = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let token = await_pending_token(&bridge).await;
            bridge
                .resolve(
                    &token,
                    ApprovalOutcome {
                        approved: true,
                        modifications: None,
                    },
                )
                .await
        })
    };

    let result = tool.execute(input).await;
    let _ = approver.await;
    assert!(
        !result.is_error,
        "expected script to succeed; got: {}",
        result.content
    );
    assert!(
        result.content.contains("step-output-ok"),
        "dispatch should have run; got: {}",
        result.content
    );

    let required = sink.required.lock().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0].0, "script:s1");

    let suspends = sink.suspend.lock().unwrap();
    assert_eq!(suspends.len(), 1);
    assert_eq!(suspends[0].0, "awaiting_approval");
}

#[tokio::test]
async fn script_approval_gate_rejects_when_denied() {
    let bridge: Arc<ApprovalBridge> = Arc::new(ApprovalBridge::new());
    let bridge_producer: Arc<dyn ApprovalProducer> = bridge.clone() as Arc<dyn ApprovalProducer>;
    let sink: Arc<CapScriptSink> = Arc::new(CapScriptSink::default());
    let sink_for_tool: Arc<dyn ScriptOutputSink> = sink.clone() as Arc<dyn ScriptOutputSink>;

    let disp = dispatcher_returns("never-reached");
    let tool = ScriptTool::new(Arc::clone(&disp)).with_approval(bridge_producer, sink_for_tool);

    let input = json!({
        "steps": [
            {"id": "s_deny", "tool": "Bash", "input": {"command": "danger"}, "approval_required": true}
        ]
    });

    let rejector = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let token = await_pending_token(&bridge).await;
            bridge
                .resolve(
                    &token,
                    ApprovalOutcome {
                        approved: false,
                        modifications: None,
                    },
                )
                .await
        })
    };

    let result = tool.execute(input).await;
    let _ = rejector.await;
    assert!(result.is_error);
    assert!(
        result.content.contains("rejected"),
        "expected rejection text; got: {}",
        result.content
    );
    assert!(
        !result.content.contains("never-reached"),
        "dispatcher must not run after rejection; got: {}",
        result.content
    );
}

#[tokio::test]
async fn script_approval_gate_without_bridge_still_short_circuits() {
    // Backwards-compat: ScriptTool::new(disp) (no .with_approval) keeps
    // the W4 error path for approval_required steps.
    let disp = dispatcher_returns("ignored");
    let tool = ScriptTool::new(Arc::clone(&disp));
    let input = json!({
        "steps": [
            {"id": "s_bare", "tool": "Bash", "input": {"command": "x"}, "approval_required": true}
        ]
    });
    let result = tool.execute(input).await;
    assert!(result.is_error);
    assert!(
        result.content.contains("no approval bridge"),
        "expected W4 short-circuit; got: {}",
        result.content
    );
}
