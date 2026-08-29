//! FerroxLabs/wayland#1180 — an egress consent prompt must reach a human, or
//! no doorbell may be installed at all.
//!
//! `AgentEgressPolicy` rings a [`ConsentDoorbell`] on an `Ask` verdict and
//! awaits the answer. The doorbell's own contract says it "is only installed
//! where a real surface exists", and `egress::consent`'s module docs say that
//! where no interactive surface exists no doorbell is set and a data-less read
//! falls back to allow (the exfil boundary stays hard-denied regardless).
//!
//! Both statements were false. `bootstrap` installed `BridgeConsentDoorbell`
//! on every session that has an egress policy, and `OutputSink`'s
//! `emit_approval_required` is a no-op by default — on the `--json-stream`
//! path `ProtocolSink` additionally gated it behind `with_hitl_suspend(true)`,
//! which nothing in the workspace ever calls. So the prompt went nowhere, the
//! turn blocked on `rx.await` until the TTL reaper cancelled it 300 seconds
//! later, and the user was then told egress "was declined at the consent
//! prompt" — a prompt they were never shown.

use std::sync::{Arc, Mutex};

use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome};
use wcore_agent::egress::classify::AllowList;
use wcore_agent::egress::{
    AgentEgressPolicy, BridgeConsentDoorbell, ConsentDecision, ConsentDoorbell,
    install_consent_doorbell,
};
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;

#[derive(Default)]
struct WireRecorder {
    events: Mutex<Vec<ProtocolEvent>>,
}

impl ProtocolEmitter for WireRecorder {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}

impl WireRecorder {
    /// Every `ApprovalRequired` frame, as `(call_id, reason)`.
    fn approvals(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                ProtocolEvent::ApprovalRequired {
                    call_id, reason, ..
                } => Some((call_id.clone(), reason.clone())),
                _ => None,
            })
            .collect()
    }
}

/// The sink the `--json-stream` path actually builds (`main.rs`): the default
/// builder plus the two options it sets. Notably NOT `with_hitl_suspend`.
fn json_stream_sink(recorder: Arc<WireRecorder>) -> Arc<dyn OutputSink> {
    Arc::new(
        ProtocolSink::with_emitter(recorder)
            .with_sub_agent_traces(true)
            .deferring_info_until_ready(),
    )
}

/// Resolve the first approval the bridge registers, so a test never waits on
/// the 300s TTL. Returns the decision the doorbell produced.
async fn ask_and_approve(
    bridge: Arc<ApprovalBridge>,
    doorbell: BridgeConsentDoorbell,
) -> ConsentDecision {
    let resolver = tokio::spawn({
        let bridge = bridge.clone();
        async move {
            for _ in 0..10_000 {
                let pending = bridge.pending_tokens().await;
                if let Some(token) = pending.first() {
                    bridge
                        .resolve(
                            token,
                            ApprovalOutcome {
                                approved: true,
                                modifications: None,
                                cancellation: None,
                            },
                        )
                        .await;
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("no approval was ever registered on the bridge");
        }
    });
    let decision = doorbell
        .ask("react.dev", "react.dev", "data-less GET")
        .await;
    resolver.await.unwrap();
    decision
}

/// #1180. The prompt has to be ON THE WIRE. Without it the host cannot render
/// a modal and — because the secret `resume_token` only travels in this frame
/// — can never echo one back, so `handle_approval_resume` has nothing to
/// resolve and the turn can only end at the TTL.
#[tokio::test]
async fn a_json_stream_egress_consent_prompt_is_put_on_the_wire() {
    let recorder = Arc::new(WireRecorder::default());
    let sink = json_stream_sink(recorder.clone());
    let bridge = Arc::new(ApprovalBridge::new());
    let doorbell = BridgeConsentDoorbell::new(bridge.clone(), sink);

    let decision = ask_and_approve(bridge, doorbell).await;
    assert_eq!(decision, ConsentDecision::Once);

    let approvals = recorder.approvals();
    assert_eq!(
        approvals.len(),
        1,
        "the host was never shown the egress consent prompt; frames: {:?}",
        approvals
    );
    let (call_id, reason) = &approvals[0];
    assert!(
        call_id.starts_with("egress:"),
        "the frame must be recognisable as egress consent, got call_id {call_id}"
    );
    assert!(
        reason.contains("react.dev"),
        "the prompt must name the destination, got {reason}"
    );
}

/// #1180, the class. A sink that cannot put an approval in front of a human
/// must not be given a doorbell at all: the documented fallback for a
/// surfaceless session is *no doorbell*, which the policy resolves as allow
/// for a data-less read (the `Exfil` verdict stays hard-denied either way).
/// Installing one there is what produced the five-minute stall and the false
/// "declined at the consent prompt".
#[tokio::test]
async fn a_sink_that_cannot_prompt_is_never_given_a_doorbell() {
    let policy = AgentEgressPolicy::enforcing(AllowList::default());

    let terminal: Arc<dyn OutputSink> = Arc::new(TerminalSink::new(true));
    assert!(
        !terminal.can_prompt_for_approval(),
        "TerminalSink has no approval surface: its emit_approval_required is \
         the trait's no-op default"
    );
    assert!(
        !install_consent_doorbell(&policy, &Arc::new(ApprovalBridge::new()), &terminal),
        "a doorbell was installed on a sink that cannot show the prompt"
    );

    let recorder = Arc::new(WireRecorder::default());
    let json_stream = json_stream_sink(recorder);
    assert!(
        json_stream.can_prompt_for_approval(),
        "the json-stream host DOES render approvals; refusing to prompt it \
         would silently widen egress instead"
    );
    assert!(
        install_consent_doorbell(&policy, &Arc::new(ApprovalBridge::new()), &json_stream),
        "the json-stream sink must get the doorbell"
    );
}
