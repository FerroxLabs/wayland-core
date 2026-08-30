//! F-005 engine-side contract test: ApprovalRequired → ApprovalResume round-trip,
//! driven through the **real emitter**.
//!
//! The contract the Wayland app (Cluster L) must implement to unblock HITL-gated tools:
//!
//!   1. Engine emits `ProtocolEvent::ApprovalRequired { call_id, resume_token, .. }`
//!   2. Host sends `ProtocolCommand::ApprovalResume { resume_token, approved, .. }`
//!   3. Engine accepts the command and routes it to the approval bridge.
//!
//! # Why this file moved out of `wcore-protocol` (wayland#934, 2026-08-28)
//!
//! It used to live in `crates/wcore-protocol/tests/` and every token in it was
//! hand-written: `"tok-xyz"`, `"tok-deny"`, `"unique-bridge-token-42"`. Serde was
//! the only thing under test, and serde was not where the risk was. **Measured:**
//! with `ApprovalBridge::request_with_ttl` mutated to mint `String::new()` instead
//! of `apr-{uuid}` — an engine that emits an EMPTY `resume_token`, which no host
//! can echo and no bridge can resolve, hanging every gated tool forever — all four
//! tests passed. They could not see it, because none of them ever called the thing
//! that mints a token.
//!
//! The bridge lives in `wcore-agent`, which is ABOVE `wcore-protocol` in the crate
//! graph, so the old location could not reach the emitter even in principle. The
//! file is here now, and step 1 starts at `ApprovalBridge::request` rather than at
//! a string literal. Every assertion the old file made is still made; what is added
//! is that the value flowing through them is the one production mints.

use std::time::Duration;

use serde_json::{Value, json};
use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome, ApprovalRequest};
use wcore_agent::egress::{BridgeConsentDoorbell, ConsentDecision, ConsentDoorbell};
use wcore_agent::output::OutputSink;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::events::ProtocolEvent;

/// Mint a token the way production does, and hand back the pending receiver.
async fn real_token(
    bridge: &ApprovalBridge,
) -> (String, tokio::sync::oneshot::Receiver<ApprovalOutcome>) {
    bridge
        .request(ApprovalRequest {
            call_id: "call-abc".into(),
            reason: "Bash wants to delete files".into(),
            context: "rm -rf /tmp/test".into(),
        })
        .await
}

/// The token the emitter mints is fit to be emitted at all.
///
/// This is the assertion the old file structurally could not make, and the one the
/// `String::new()` mutation walks straight through. An empty `resume_token` on the
/// wire is not a cosmetic defect: the host echoes `""` back, `resolve("")` finds no
/// pending entry, and the gated tool blocks until the TTL reaper cancels it.
#[tokio::test]
async fn the_emitter_mints_a_token_that_can_be_echoed_and_resolved() {
    let bridge = ApprovalBridge::new();
    let (token, _rx) = real_token(&bridge).await;

    assert!(
        !token.is_empty(),
        "an empty resume_token cannot be echoed by any host and resolves nothing"
    );
    assert!(
        !token.chars().all(char::is_whitespace),
        "a whitespace-only token survives JSON but not a `trim()` anywhere downstream: {token:?}"
    );
    assert!(
        token.starts_with("apr-"),
        "the token must stay namespaced so a log reader can tell it from a call_id: {token:?}"
    );

    // GHSA-8r7g: it is a SECRET, so it must not be derivable from anything the
    // model can see. Two requests with byte-identical inputs must differ.
    let (second, _rx2) = real_token(&bridge).await;
    assert_ne!(
        token, second,
        "two approvals with identical inputs minted the same token, so the token is a function \
         of model-visible data and a model could self-approve"
    );
    assert!(
        token.len() >= "apr-".len() + 32,
        "token is too short to be unguessable: {token:?}"
    );
}

/// Engine emits `ApprovalRequired` — the JSON wire shape, carrying a REAL token.
#[tokio::test]
async fn approval_required_event_serializes_correctly() {
    let bridge = ApprovalBridge::new();
    let (token, _rx) = real_token(&bridge).await;

    let event = ProtocolEvent::ApprovalRequired {
        call_id: "call-abc".to_string(),
        resume_token: token.clone(),
        correlation_id: token.clone(),
        reason: "Bash wants to delete files".to_string(),
        context: "rm -rf /tmp/test".to_string(),
        plan: None,
    };

    let json_str = serde_json::to_string(&event).expect("event must serialize");
    let parsed: Value = serde_json::from_str(&json_str).expect("must be valid JSON");

    assert_eq!(parsed["type"], "approval_required");
    assert_eq!(parsed["call_id"], "call-abc");
    assert_eq!(parsed["reason"], "Bash wants to delete files");
    assert_eq!(parsed["context"], "rm -rf /tmp/test");
    // The field the host reads. Asserting it against the minted token rather than
    // against a literal is the whole point of this file's move.
    assert_eq!(parsed["resume_token"], token);
    assert_eq!(parsed["correlation_id"], token);
    assert!(
        parsed["resume_token"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "the emitted event carried an unusable resume_token: {parsed}"
    );
}

/// The whole loop: emitter → wire → host → command → bridge resolves the pending future.
///
/// The old `approval_resume_command_accepted_with_token_from_event` stopped at
/// "the command deserializes". That leaves the only question anyone cares about —
/// does the echoed token actually unblock the tool — untested. Here the receiver
/// returned by `request()` is awaited, so a token that round-trips syntactically
/// but resolves nothing fails.
#[tokio::test]
async fn the_echoed_token_resolves_the_pending_approval() {
    let bridge = ApprovalBridge::new();
    let (token, rx) = real_token(&bridge).await;

    // Engine side: serialise the event the host will read.
    let event = ProtocolEvent::ApprovalRequired {
        call_id: "call-abc".to_string(),
        resume_token: token.clone(),
        correlation_id: token.clone(),
        reason: "needs approval".to_string(),
        context: "tool context".to_string(),
        plan: None,
    };
    let event_json: Value = serde_json::to_value(&event).unwrap();

    // Host side: read `resume_token` out of the event JSON — no access to the
    // Rust value — and build the resume command from it.
    let token_from_event = event_json["resume_token"]
        .as_str()
        .expect("resume_token must be a string in event JSON");
    let host_cmd_json = json!({
        "type": "approval_resume",
        "resume_token": token_from_event,
        "approved": true,
    })
    .to_string();

    let cmd: ProtocolCommand =
        serde_json::from_str(&host_cmd_json).expect("ApprovalResume must deserialize");
    let ProtocolCommand::ApprovalResume {
        resume_token,
        approved,
        modifications,
    } = cmd
    else {
        panic!("expected ApprovalResume");
    };
    assert_eq!(
        resume_token, token,
        "token must survive event → wire → command intact"
    );
    assert!(approved, "approved flag must round-trip");
    assert!(
        modifications.is_none(),
        "modifications absent when not sent"
    );

    // Engine side again: route it to the bridge, exactly as the command handler does.
    assert!(
        bridge
            .resolve(
                &resume_token,
                ApprovalOutcome {
                    approved,
                    modifications,
                    cancellation: None
                },
            )
            .await,
        "the token the engine emitted did not resolve the approval the engine registered"
    );
    let outcome = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("the gated tool must be released, not left waiting for the TTL reaper")
        .expect("the sender must fire rather than be dropped");
    assert!(
        outcome.approved,
        "the host approved; the tool must see approved"
    );
}

/// Denial plus modifications, also driven end to end.
#[tokio::test]
async fn a_denial_with_modifications_reaches_the_waiting_tool() {
    let bridge = ApprovalBridge::new();
    let (token, rx) = real_token(&bridge).await;

    let host_json = json!({
        "type": "approval_resume",
        "resume_token": token,
        "approved": false,
        "modifications": {"substitute_command": "echo safe"}
    })
    .to_string();

    let cmd: ProtocolCommand = serde_json::from_str(&host_json)
        .expect("ApprovalResume with modifications must deserialize");
    let ProtocolCommand::ApprovalResume {
        resume_token,
        approved,
        modifications,
    } = cmd
    else {
        panic!("expected ApprovalResume");
    };
    assert!(!approved);
    let mods = modifications.expect("modifications must be present");
    assert_eq!(mods["substitute_command"], "echo safe");

    assert!(
        bridge
            .resolve(
                &resume_token,
                ApprovalOutcome {
                    approved,
                    modifications: Some(mods),
                    cancellation: None
                },
            )
            .await
    );
    let outcome = tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .expect("the tool must be released")
        .expect("the sender must fire");
    assert!(!outcome.approved);
    assert_eq!(
        outcome
            .modifications
            .expect("modifications must reach the tool")["substitute_command"],
        "echo safe",
        "the substitution the human typed must arrive at the tool, not just at the parser"
    );
}

/// The negative half, which the old file had no way to express: a token the engine
/// did NOT mint must resolve nothing.
///
/// Without this, every assertion above is equally well explained by `resolve`
/// returning `true` for anything — and an approval bridge that accepts an
/// arbitrary string is a self-approval hole (GHSA-8r7g), not a convenience.
#[tokio::test]
async fn a_token_the_engine_never_minted_resolves_nothing() {
    let bridge = ApprovalBridge::new();
    let (real, _rx) = real_token(&bridge).await;

    for forged in [
        "",
        "   ",
        "tok-xyz",
        "call-abc",
        "apr-00000000-0000-0000-0000-000000000000",
    ] {
        assert!(
            !bridge
                .resolve(
                    forged,
                    ApprovalOutcome {
                        approved: true,
                        modifications: None,
                        cancellation: None
                    },
                )
                .await,
            "a forged resume_token {forged:?} resolved an approval the engine did not issue it for"
        );
    }
    assert_eq!(
        bridge.pending_count().await,
        1,
        "the genuine approval must still be pending after every forgery attempt"
    );
    assert!(
        bridge.active_tokens().await.contains(&real),
        "known-positive: the real token is registered, so the refusals above are refusals and \
         not an empty bridge answering false to everything"
    );
}

/// A sink that records the `ApprovalRequired` emissions a host would see, and
/// NOTHING else. This is deliberately the only channel the "host" in
/// [`the_token_the_doorbell_emits_is_the_token_that_resolves_it`] is allowed to
/// read: a real host has no handle on the bridge.
#[derive(Default)]
struct RecordingSink {
    emitted: std::sync::Mutex<Vec<(String, String)>>,
}

impl RecordingSink {
    /// The (call_id, resume_token) of the first emission, if any.
    fn first(&self) -> Option<(String, String)> {
        self.emitted.lock().unwrap().first().cloned()
    }
}

impl OutputSink for RecordingSink {
    // The trait's required surface, none of which this test observes.
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
    fn emit_tool_call(&self, _name: &str, _input: &str) {}
    fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
    fn emit_stream_start(&self, _msg_id: &str) {}
    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end(
        &self,
        _msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_creation_tokens: u64,
        _cache_read_tokens: u64,
        _finish_reason: wcore_types::message::FinishReason,
    ) {
    }
    fn emit_error(&self, _msg: &str, _retryable: bool) {}
    fn emit_info(&self, _msg: &str) {}

    /// wayland#1219: this sink records the approval, so it has a surface.
    fn approval_surface_available(&self) -> bool {
        true
    }

    fn emit_approval_required(
        &self,
        call_id: &str,
        resume_token: &str,
        _reason: &str,
        _context: &str,
    ) {
        self.emitted
            .lock()
            .unwrap()
            .push((call_id.to_string(), resume_token.to_string()));
    }
}

/// THE EMITTER SEAM — the production code that puts a token in front of a host.
///
/// # The gap this closes
///
/// Everything above starts from `ApprovalBridge::request`, which is the
/// MINTING seam, and then hand-builds the `ApprovalRequired` event. But the
/// production egress path does not do that: `BridgeConsentDoorbell::ask`
/// registers on the bridge, emits `ApprovalRequired` through the `OutputSink`,
/// and parks on the receiver. The token the HOST gets is whatever reaches the
/// sink, and no test observed that argument. Nor do the doorbell's own unit
/// tests: they resolve via `bridge.pending_tokens()`, an in-process shortcut a
/// host does not have, so they are satisfied by a doorbell that emits anything
/// at all.
///
/// **Measured:** with the sink emission mutated to
/// `emit_approval_required(&call_id, "", ...)` — an egress consent no host can
/// ever answer, parking the request until the TTL reaper denies it, i.e. every
/// `Ask` network access silently failing closed after a long stall — all five
/// tests in this file and all three doorbell unit tests passed.
///
/// Here the resolver reads ONLY the sink. It echoes back exactly the bytes the
/// doorbell put on the wire, which is the whole of a host's authority.
#[tokio::test]
async fn the_token_the_doorbell_emits_is_the_token_that_resolves_it() {
    let bridge = std::sync::Arc::new(ApprovalBridge::new());
    let sink = std::sync::Arc::new(RecordingSink::default());
    let doorbell = BridgeConsentDoorbell::new(bridge.clone(), sink.clone());

    // The "host": polls the SINK for the emission and echoes the token back.
    // It never touches the bridge's pending set, so it cannot succeed by
    // knowing something a host would not know.
    let host = {
        let bridge = bridge.clone();
        let sink = sink.clone();
        tokio::spawn(async move {
            loop {
                if let Some((call_id, token)) = sink.first() {
                    let resolved = bridge
                        .resolve(
                            &token,
                            ApprovalOutcome {
                                approved: true,
                                modifications: None,
                                cancellation: None,
                            },
                        )
                        .await;
                    return (call_id, token, resolved);
                }
                tokio::task::yield_now().await;
            }
        })
    };

    // Bounded: an unanswerable token parks `ask` until the TTL reaper, so a
    // bare await here would HANG instead of failing.
    let decision = tokio::time::timeout(
        Duration::from_secs(10),
        doorbell.ask("react.dev", "react.dev", "data-less GET"),
    )
    .await;
    let (call_id, token, resolved) = host.await.expect("the host task must not panic");

    assert!(
        !token.is_empty(),
        "the doorbell emitted an EMPTY resume_token: no host can echo it, so the consent \
         request parks until the TTL reaper denies it"
    );
    assert!(
        token.starts_with("apr-"),
        "the emitted token must be the bridge-minted secret, not some other string: {token:?}"
    );
    assert_ne!(
        token, call_id,
        "GHSA-8r7g: the emitted secret must not be the public correlation handle, or anything \
         that can see the call_id could self-approve"
    );
    assert!(
        call_id.starts_with("egress:"),
        "known-positive: the recorded emission is the egress doorbell's, so the assertions \
         above are about the right call: {call_id:?}"
    );
    assert!(
        resolved,
        "the token the doorbell put in front of the host did not resolve the approval the \
         doorbell had just registered - the wire handle and the bridge key disagree"
    );

    let decision = decision.expect(
        "the doorbell never observed the host's answer: it was still parked after 10s, which \
         in production is an Ask verdict silently failing closed on the TTL",
    );
    assert_eq!(
        decision,
        ConsentDecision::Once,
        "an approve with no scope must reach the waiting egress request as Once"
    );
}
