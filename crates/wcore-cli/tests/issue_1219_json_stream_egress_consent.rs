//! FerroxLabs/wayland#1219 — an egress consent on the `--json-stream` path must
//! reach the host, and a consent nobody saw must never be reported as declined.
//!
//! ## What this grades, and why it is NOT the easier adjacent property
//!
//! The criterion is about **what the user is told**. The easy substitute is
//! "some deny path executes" or "the sink's emit method works when you switch
//! the flag on yourself". Neither would have caught this bug: the emit method
//! was always correct, and `with_hitl_suspend` had ZERO callers in the
//! workspace, so the `--json-stream` runtime built a sink whose approval
//! surface was permanently shut. So these tests
//!
//!   * build the sink through the **production** constructor
//!     (`wcore_cli::json_stream_sink::build_json_stream_sink`) — never a
//!     lookalike assembled here, which would switch the flag on for the
//!     product and prove nothing; and
//!   * drive a **real** `EgressVerdict::Ask` through the **real**
//!     `AgentEgressPolicy` + `BridgeConsentDoorbell`; and
//!   * assert the **surfaced text** — the frame the host receives, and the
//!     exact deny sentence the user reads — not merely that a code path ran.
//!
//! No network: `AgentEgressPolicy::check` classifies a `reqwest::Request` that
//! is never sent.

use std::sync::Arc;
use std::time::Duration;

use wcore_agent::approval::{ApprovalBridge, ApprovalOutcome};
use wcore_agent::egress::{
    AgentEgressPolicy, AllowList, BridgeConsentDoorbell, install_consent_doorbell,
};
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_cli::json_stream_sink::build_json_stream_sink;
use wcore_config::tools::AdvertisedCapabilitiesConfig;
use wcore_egress::{EgressDecision, EgressPolicy};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;

/// The host's stdout, captured. Everything asserted below is read from here —
/// this is the only thing a Desktop host can actually see.
#[derive(Default)]
struct HostWire {
    frames: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl HostWire {
    fn frames(&self) -> Vec<serde_json::Value> {
        self.frames.lock().expect("wire lock").clone()
    }

    fn approval_required(&self) -> Option<serde_json::Value> {
        self.frames()
            .into_iter()
            .find(|f| f["type"] == "approval_required")
    }
}

impl ProtocolEmitter for HostWire {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.frames
            .lock()
            .expect("wire lock")
            .push(serde_json::to_value(event).expect("serialize"));
        Ok(())
    }
}

/// A data-less GET to a domain nobody allow-listed — the `EgressVerdict::Ask`
/// shape (an exfil-shaped request would classify as `Exfil` and never reach the
/// doorbell at all).
fn ask_shaped_request() -> reqwest::Request {
    reqwest::Request::new(
        reqwest::Method::GET,
        "https://react.dev/learn".parse().expect("url"),
    )
}

/// The production `--json-stream` sink, over a captured wire.
fn json_stream_sink(wire: Arc<HostWire>) -> ProtocolSink {
    build_json_stream_sink(
        wire,
        false,
        Arc::new(AdvertisedCapabilitiesConfig::default()),
    )
}

const FALSE_DECLINE: &str = "was declined at the consent prompt";

// ---------------------------------------------------------------------------
// c1 + c4
// ---------------------------------------------------------------------------

/// **c1 / c4.** An `EgressVerdict::Ask` on the json-stream path reaches the
/// host as an approval request the host can answer.
///
/// "Can answer" is graded literally: the resolving task learns the token the
/// way a host does — off the emitted `approval_required` frame, never from
/// `bridge.pending_tokens()`, the in-process shortcut that made a sibling test
/// vacuous under wayland#1180 — and the request then resolves to `Allow`.
///
/// RED against today's gate: drop `.with_hitl_suspend(true)` from
/// `build_json_stream_sink` and no frame is ever written.
#[tokio::test]
async fn an_egress_ask_on_the_json_stream_path_reaches_the_host() {
    let wire = Arc::new(HostWire::default());
    let sink: Arc<dyn OutputSink> = Arc::new(json_stream_sink(wire.clone()));
    let bridge = Arc::new(ApprovalBridge::new());

    let policy = AgentEgressPolicy::enforcing(AllowList::default());
    assert!(
        install_consent_doorbell(&policy, bridge.clone(), sink.clone()),
        "the production json-stream sink was judged to have no approval surface"
    );

    // The host: watch the wire, echo the token back.
    let host = {
        let wire = wire.clone();
        let bridge = bridge.clone();
        tokio::spawn(async move {
            for _ in 0..4000 {
                if let Some(frame) = wire.approval_required() {
                    let token = frame["resume_token"]
                        .as_str()
                        .expect("approval_required carried no resume_token")
                        .to_string();
                    bridge
                        .resolve(
                            &token,
                            ApprovalOutcome {
                                approved: true,
                                modifications: None,
                                cancellation: None,
                            },
                        )
                        .await;
                    return Some(frame);
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            None
        })
    };

    let decision =
        tokio::time::timeout(Duration::from_secs(20), policy.check(&ask_shaped_request()))
            .await
            .expect("the turn stalled instead of prompting — wayland#1219's five minutes");

    let frame = host
        .await
        .expect("host task")
        .expect("no approval_required frame ever reached the host");

    // The frame the host receives is answerable and recognisable as egress.
    assert!(
        frame["call_id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("egress:"),
        "host cannot tell this is an egress consent: {frame}"
    );
    assert!(
        frame["resume_token"]
            .as_str()
            .unwrap_or_default()
            .starts_with("apr-"),
        "host was handed a token it cannot resolve with: {frame}"
    );
    let context: serde_json::Value =
        serde_json::from_str(frame["context"].as_str().unwrap_or("null")).unwrap_or_default();
    assert_eq!(context["kind"], "egress_consent", "frame: {frame}");
    assert_eq!(context["registrable"], "react.dev", "frame: {frame}");

    // And answering it actually decided the request.
    assert!(
        matches!(decision, EgressDecision::Allow),
        "the host approved, and the request was still refused: {decision:?}"
    );
}

/// **c1, the capability half.** The host is TOLD it may answer: `ready`
/// advertises `capabilities.hitl_suspend`. Without it a correct host has no
/// reason to render the modal, so the frame arriving is not enough on its own.
#[test]
fn the_json_stream_sink_advertises_the_approval_capability() {
    let wire = Arc::new(HostWire::default());
    let sink = json_stream_sink(wire);
    assert!(
        sink.approval_surface_available(),
        "the production json-stream sink still reports no approval surface"
    );
}

// ---------------------------------------------------------------------------
// c2
// ---------------------------------------------------------------------------

/// **c2.** No path installs `BridgeConsentDoorbell` where the sink cannot
/// emit. The guard both arms of the disjunction run through.
///
/// The mute arm is not hypothetical: it is a `ProtocolSink` built the way
/// `main.rs` built it before this fix.
#[test]
fn a_blocking_doorbell_is_never_installed_over_a_sink_that_cannot_prompt() {
    let wire = Arc::new(HostWire::default());
    let pre_fix_sink: Arc<dyn OutputSink> = Arc::new(
        // The 0.13.11 shape: every builder call main.rs made EXCEPT
        // with_hitl_suspend.
        ProtocolSink::with_emitter(wire)
            .with_structured_traces(false)
            .with_sub_agent_traces(true)
            .deferring_info_until_ready(),
    );
    assert!(!pre_fix_sink.approval_surface_available());

    let policy = AgentEgressPolicy::enforcing(AllowList::default());
    assert!(
        !install_consent_doorbell(&policy, Arc::new(ApprovalBridge::new()), pre_fix_sink),
        "installed a blocking consent doorbell over a sink that cannot prompt"
    );
    assert!(
        !policy.has_doorbell(),
        "the policy is wired to ring a doorbell nobody can hear"
    );
}

// ---------------------------------------------------------------------------
// c3
// ---------------------------------------------------------------------------

/// **c3.** A consent that was never shown is never reported to the user as
/// "declined at the consent prompt".
///
/// This installs the doorbell over a mute sink DIRECTLY, bypassing the c2
/// guard, because c3 is about the message and must hold even if some future
/// path re-introduces that wiring. Two assertions, and the first is the
/// criterion's own sentence:
///
///   1. the deny reason does not contain the false decline; and
///   2. nothing was ever written to the wire — i.e. the consent really was
///      never shown, so assertion 1 is not passing for some other reason.
///
/// It also bounds the wait: the defect's other half is a five-minute stall.
#[tokio::test]
async fn a_consent_never_shown_is_not_reported_as_declined() {
    let wire = Arc::new(HostWire::default());
    let mute: Arc<dyn OutputSink> = Arc::new(ProtocolSink::with_emitter(wire.clone()));
    let policy = AgentEgressPolicy::enforcing(AllowList::default());
    policy.set_doorbell(Arc::new(BridgeConsentDoorbell::new(
        Arc::new(ApprovalBridge::new()),
        mute,
    )));

    let decision =
        tokio::time::timeout(Duration::from_secs(10), policy.check(&ask_shaped_request()))
            .await
            .expect("the turn stalled on a prompt that can never be shown");

    let EgressDecision::Deny { reason } = decision else {
        panic!("expected a fail-closed deny, got {decision:?}");
    };

    assert!(
        wire.approval_required().is_none(),
        "premise broken: a prompt WAS shown, so this is not the never-shown case"
    );
    assert!(
        !reason.contains(FALSE_DECLINE),
        "the user was blamed for declining a prompt that was never shown: {reason}"
    );
    assert!(
        reason.contains("without asking you"),
        "the message does not say what actually happened: {reason}"
    );
}

/// **Beyond c3, same sentence's class.** The prompt WAS shown and the approval
/// TTL reaped it. The user did not decline; they did not answer. #1083 already
/// established that a bridge self-resolution must stay distinguishable from a
/// human decision — this pins the egress wording to that rule.
#[tokio::test]
async fn an_unanswered_consent_is_not_reported_as_declined_either() {
    let wire = Arc::new(HostWire::default());
    let sink: Arc<dyn OutputSink> = Arc::new(json_stream_sink(wire.clone()));
    let bridge = Arc::new(ApprovalBridge::new());
    let policy = AgentEgressPolicy::enforcing(AllowList::default());
    assert!(install_consent_doorbell(&policy, bridge.clone(), sink));

    // Stand in for the reaper: resolve with the outcome IT produces.
    let reaper = {
        let wire = wire.clone();
        let bridge = bridge.clone();
        tokio::spawn(async move {
            for _ in 0..4000 {
                if let Some(frame) = wire.approval_required() {
                    let token = frame["resume_token"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    bridge.resolve(&token, ApprovalOutcome::cancelled()).await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    let decision =
        tokio::time::timeout(Duration::from_secs(20), policy.check(&ask_shaped_request()))
            .await
            .expect("stalled");
    reaper.await.expect("reaper task");

    let EgressDecision::Deny { reason } = decision else {
        panic!("expected a fail-closed deny, got {decision:?}");
    };
    assert!(
        wire.approval_required().is_some(),
        "premise broken: this case requires the prompt to have been shown"
    );
    assert!(
        !reason.contains(FALSE_DECLINE),
        "silence was reported as a decline: {reason}"
    );
    assert!(
        reason.contains("no answer"),
        "the message does not say what actually happened: {reason}"
    );
}

/// The control for the two negatives above: an operator who really declines
/// still gets told they declined. Without this, deleting the phrase entirely
/// would pass every assertion here.
#[tokio::test]
async fn an_actual_decline_is_still_reported_as_a_decline() {
    let wire = Arc::new(HostWire::default());
    let sink: Arc<dyn OutputSink> = Arc::new(json_stream_sink(wire.clone()));
    let bridge = Arc::new(ApprovalBridge::new());
    let policy = AgentEgressPolicy::enforcing(AllowList::default());
    assert!(install_consent_doorbell(&policy, bridge.clone(), sink));

    let operator = {
        let wire = wire.clone();
        let bridge = bridge.clone();
        tokio::spawn(async move {
            for _ in 0..4000 {
                if let Some(frame) = wire.approval_required() {
                    let token = frame["resume_token"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    bridge
                        .resolve(
                            &token,
                            ApprovalOutcome {
                                approved: false,
                                modifications: None,
                                cancellation: None,
                            },
                        )
                        .await;
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    let decision =
        tokio::time::timeout(Duration::from_secs(20), policy.check(&ask_shaped_request()))
            .await
            .expect("stalled");
    operator.await.expect("operator task");

    let EgressDecision::Deny { reason } = decision else {
        panic!("expected a deny, got {decision:?}");
    };
    assert!(
        reason.contains(FALSE_DECLINE),
        "a real decline stopped saying so: {reason}"
    );
}
