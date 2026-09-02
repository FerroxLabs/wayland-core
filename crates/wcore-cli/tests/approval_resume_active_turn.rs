//! FerroxLabs/wayland#1180 — grade the bridge-backed approval resume seam.
//!
//! The seam is `approval_bridge.resolve(...)` in the `ApprovalResume` command
//! handler. It completes the `ApprovalRequired -> ApprovalResume` loop for an
//! approval that parks DURING a turn (an egress consent, a council card). It
//! previously lived inline in `main.rs` and had no test that would notice if it
//! were deleted; it now lives in `wcore_cli::approval_resume`, called from both
//! `main.rs` arms, and this drives it.
//!
//! **The trap this deliberately avoids.** The doorbell's own unit tests learn
//! the `resume_token` from `bridge.pending_tokens()` — a shortcut no host has —
//! and so passed unchanged under a mutation that emitted an EMPTY token. Here
//! the token comes from the `ApprovalRequired` event the doorbell EMITTED, i.e.
//! the host's only source. An emission carrying the wrong token fails this test
//! at the resolve, not silently.
//!
//! The fixture is the real production doorbell (`BridgeConsentDoorbell`) on a
//! real `ApprovalBridge`. No network, no spawned binary.

use std::sync::Arc;
use std::time::Duration;

use wcore_agent::approval::ApprovalBridge;
use wcore_agent::egress::{BridgeConsentDoorbell, ConsentDecision, ConsentDoorbell};
use wcore_agent::test_utils::{TestSink, TestSinkHandle};
use wcore_cli::approval_resume::handle_approval_resume;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;

/// Captures what the handler writes back to the host.
#[derive(Default)]
struct RecordingEmitter {
    events: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl RecordingEmitter {
    fn seen(&self) -> Vec<serde_json::Value> {
        self.events.lock().expect("emitter lock").clone()
    }

    fn kinds(&self) -> Vec<String> {
        self.seen()
            .iter()
            .map(|e| e["type"].as_str().unwrap_or("?").to_string())
            .collect()
    }
}

impl ProtocolEmitter for RecordingEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        let value = serde_json::to_value(event).expect("serialize event");
        self.events.lock().expect("emitter lock").push(value);
        Ok(())
    }
}

/// The host's view: read the `resume_token` off the emitted `ApprovalRequired`.
///
/// Never `bridge.pending_tokens()` — that is the shortcut that made the sibling
/// tests vacuous.
async fn resume_token_from_the_wire(handle: &TestSinkHandle) -> String {
    for _ in 0..2000 {
        if let Some(event) = handle
            .snapshot()
            .into_iter()
            .find(|e| e["type"] == "approval_required")
        {
            return event["resume_token"]
                .as_str()
                .unwrap_or_default()
                .to_string();
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the doorbell never emitted approval_required");
}

fn parked_egress_consent() -> (
    Arc<ApprovalBridge>,
    TestSinkHandle,
    tokio::task::JoinHandle<ConsentDecision>,
) {
    let bridge = Arc::new(ApprovalBridge::new());
    let sink = TestSink::new();
    let handle = sink.handle();
    let doorbell = BridgeConsentDoorbell::new(bridge.clone(), Arc::new(sink));
    let ask = tokio::spawn(async move {
        doorbell
            .ask("example.com", "example.com", "POST with a request body")
            .await
    });
    (bridge, handle, ask)
}

/// THE graded arm. Remove `approval_bridge.resolve(...)` from the handler and
/// the parked consent never returns: this fails on the timeout.
#[tokio::test]
async fn a_bridge_backed_approval_parked_mid_turn_is_resumed_by_the_handler() {
    let (bridge, handle, ask) = parked_egress_consent();
    let resume_token = resume_token_from_the_wire(&handle).await;
    assert!(
        resume_token.starts_with("apr-"),
        "the host was handed a token it cannot resolve with: {resume_token:?}"
    );

    let writer = RecordingEmitter::default();
    let resolved = handle_approval_resume(
        &bridge,
        &writer,
        resume_token.clone(),
        true,
        Some(serde_json::json!({ "egress_scope": "always" })),
    )
    .await;
    assert!(
        resolved,
        "the handler reported nothing was waiting for {resume_token}"
    );

    let decision = tokio::time::timeout(Duration::from_secs(5), ask)
        .await
        .expect("the parked consent must resume once the host answers")
        .expect("consent task panicked");
    assert_eq!(
        decision,
        ConsentDecision::Always,
        "the host's decision — including its modifications — must reach the \
         parked turn, not merely unblock it"
    );

    let seen = writer.seen();
    let echo = seen
        .iter()
        .find(|e| e["type"] == "approval_resume")
        .expect("the host must get its approval_resume echo");
    assert_eq!(echo["resume_token"], serde_json::json!(resume_token));
    assert_eq!(echo["approved"], serde_json::json!(true));
    assert!(
        !seen.iter().any(|e| e["type"] == "info"),
        "a resume that DID resolve must not report a stale token: {seen:?}"
    );
}

/// NEGATIVE CONTROL — must pass in both mutation arms.
///
/// A handler that resolved indiscriminately, or that emitted nothing at all,
/// would be a different bug wearing the same green. So: a token nobody is
/// waiting on must leave the parked consent parked, and the echo must still go
/// out.
#[tokio::test]
async fn a_token_nobody_is_waiting_on_resolves_nothing_and_still_echoes() {
    let (bridge, handle, mut ask) = parked_egress_consent();
    let real_token = resume_token_from_the_wire(&handle).await;

    let writer = RecordingEmitter::default();
    handle_approval_resume(
        &bridge,
        &writer,
        "apr-00000000-0000-0000-0000-000000000000".to_string(),
        true,
        None,
    )
    .await;

    assert!(
        writer.kinds().contains(&"approval_resume".to_string()),
        "the echo is unconditional so a host can clear its modal: {:?}",
        writer.kinds()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(250), &mut ask)
            .await
            .is_err(),
        "a wrong token released the parked consent — any token would then \
         approve any pending request"
    );

    // Leave nothing running: the real token still works.
    handle_approval_resume(&bridge, &writer, real_token, false, None).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), ask).await;
}

/// The stale-resume diagnostic. Also graded by the ticket's mutation (a handler
/// that stopped resolving would report every resume as stale), which is correct
/// — it is the same seam.
#[tokio::test]
async fn a_stale_resume_is_named_on_the_wire() {
    let bridge = ApprovalBridge::new();
    let writer = RecordingEmitter::default();

    let resolved =
        handle_approval_resume(&bridge, &writer, "apr-long-gone".to_string(), true, None).await;

    assert!(!resolved);
    let info = writer
        .seen()
        .into_iter()
        .find(|e| e["type"] == "info")
        .expect("an unknown token must be surfaced, not silently dropped");
    assert!(
        info["message"]
            .as_str()
            .unwrap_or_default()
            .contains("apr-long-gone"),
        "the diagnostic must name the token: {info}"
    );
}
