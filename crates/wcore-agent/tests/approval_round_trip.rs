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
use wcore_agent::approval::{ApprovalBridge, ApprovalCancelCause, ApprovalOutcome};
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
                        cancellation: None,
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
                        cancellation: None,
                    },
                )
                .await
        })
    };

    let result = tool.execute(input).await;
    let _ = rejector.await;
    assert!(result.is_error);
    assert!(
        result.content.contains("rejected by user"),
        "expected rejection text; got: {}",
        result.content
    );
    // #1083 CONTROL: an operator's own "no" must keep saying a USER rejected
    // it. If this drifted to the cancellation wording, the two tests below
    // would pass while telling every refusal the same new story — the same
    // defect, inverted.
    assert!(
        !result.content.contains("was not approved:"),
        "an operator decision is not a bridge cancellation; got: {}",
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

// ---------------------------------------------------------------------------
// FerroxLabs/wayland#1083 criterion 3, at the surface a user and a model
// actually read.
//
// Observed on released v0.13.5 (addb4f48): with an approval-gated script step
// parked and the host's command stream closing, the step resolved (the #1083
// EOF drain works) but reported
//
//     step 's_eof' rejected by user
//
// to the model and the transcript. Nobody rejected it. The TTL reaper produced
// the same sentence. `ApprovalOutcome` carried the cause by then, but the
// `ApprovalOutcomeLite` mirror dropped it on the floor, so the one consumer
// that renders text for a human could not tell the three cases apart.
// ---------------------------------------------------------------------------

/// Host-EOF arm. The step must say the HOST WENT AWAY.
#[tokio::test]
async fn script_step_says_the_host_disconnected_rather_than_blaming_a_user() {
    let bridge: Arc<ApprovalBridge> = Arc::new(ApprovalBridge::new());
    let bridge_producer: Arc<dyn ApprovalProducer> = bridge.clone() as Arc<dyn ApprovalProducer>;
    let sink: Arc<CapScriptSink> = Arc::new(CapScriptSink::default());
    let sink_for_tool: Arc<dyn ScriptOutputSink> = sink.clone() as Arc<dyn ScriptOutputSink>;

    let disp = dispatcher_returns("never-reached");
    let tool = ScriptTool::new(Arc::clone(&disp)).with_approval(bridge_producer, sink_for_tool);

    let input = json!({
        "steps": [
            {"id": "s_eof", "tool": "Bash", "input": {"command": "x"}, "approval_required": true}
        ]
    });

    // This is exactly what `deny_pending_approvals_on_host_eof` does to the
    // bridge when the CLI sees `commands_open = false`.
    let host_eof = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let _ = await_pending_token(&bridge).await;
            bridge
                .cancel_all_pending(ApprovalCancelCause::HostStreamClosed)
                .await
        })
    };

    let result = tool.execute(input).await;
    assert_eq!(
        host_eof.await.unwrap(),
        1,
        "the EOF drain must be what resolved this step -- if it were 0, the \
         step resolved some other way and the assertions below prove nothing"
    );

    assert!(result.is_error, "an unapproved step still fails closed");
    assert!(
        !result.content.contains("never-reached"),
        "the dispatcher must not run: {}",
        result.content
    );
    assert!(
        !result.content.contains("rejected by user"),
        "the host disconnected; no user rejected anything. Got: {}",
        result.content
    );
    assert!(
        result
            .content
            .contains(ApprovalCancelCause::HostStreamClosed.reason()),
        "the step must carry the host-disconnect reason so a reader can tell \
         it from a TTL expiry. Got: {}",
        result.content
    );
}

/// TTL arm. The same step, resolved by the reaper instead, must NOT claim the
/// host went away -- otherwise the EOF wording above discriminates nothing.
#[tokio::test]
async fn script_step_says_the_approval_expired_when_the_reaper_collects_it() {
    let bridge: Arc<ApprovalBridge> = Arc::new(ApprovalBridge::with_ttl(
        std::time::Duration::from_millis(20),
    ));
    let bridge_producer: Arc<dyn ApprovalProducer> = bridge.clone() as Arc<dyn ApprovalProducer>;
    let sink: Arc<CapScriptSink> = Arc::new(CapScriptSink::default());
    let sink_for_tool: Arc<dyn ScriptOutputSink> = sink.clone() as Arc<dyn ScriptOutputSink>;

    let disp = dispatcher_returns("never-reached");
    let tool = ScriptTool::new(Arc::clone(&disp)).with_approval(bridge_producer, sink_for_tool);

    let input = json!({
        "steps": [
            {"id": "s_ttl", "tool": "Bash", "input": {"command": "x"}, "approval_required": true}
        ]
    });

    let reaper = {
        let bridge = bridge.clone();
        tokio::spawn(async move {
            let _ = await_pending_token(&bridge).await;
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            bridge.reap_now().await
        })
    };

    let result = tool.execute(input).await;
    assert_eq!(
        reaper.await.unwrap(),
        1,
        "a real reaper collection must be what resolved this step"
    );

    assert!(result.is_error, "an expired step still fails closed");
    assert!(
        result
            .content
            .contains(ApprovalCancelCause::Expired.reason()),
        "an expiry must say so. Got: {}",
        result.content
    );
    assert!(
        !result
            .content
            .contains(ApprovalCancelCause::HostStreamClosed.reason()),
        "an expiry must NOT be reported as a host disconnect, or the two \
         cases collapse back into one message. Got: {}",
        result.content
    );
    assert!(
        !result.content.contains("rejected by user"),
        "nobody rejected an expired approval. Got: {}",
        result.content
    );
}
