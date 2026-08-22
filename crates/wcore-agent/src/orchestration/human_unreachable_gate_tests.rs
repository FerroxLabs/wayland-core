//! Row B-3 — the unreachable-human freeze.
//!
//! The defect these prove out, measured rather than described. With the
//! approval mail made undeliverable (a corpus copy whose only difference is
//! one SMTP port line), the product tried twice to reach the on-call, was
//! refused twice, and then rewrote `requirements.txt` from `moneykit==1.4.3`
//! to `2.0.0` on disk with no approval on record. The run that DID reach the
//! on-call took four attempts — three malformed-envelope errors, then a
//! delivery — so no failure COUNT separates "gave up" from "still trying".
//! What separates them is whether the last word from the outbound route was a
//! failure, and that is what the gate keys on.
//!
//! A second defect fell out of writing these: the per-tool circuit breaker
//! (3 failures / 30s) short-circuits `send_message` too, so the retry that
//! WOULD have reached the on-call never leaves the machine and the freeze can
//! never lift. `three_failures_then_a_delivery_lifts_the_freeze` asserts the
//! breaker is open at that moment, so it fails if the exemption is removed.
//!
//! Every test here has a control that runs the identical scenario with the
//! send DELIVERED and asserts the write did happen. Without it, "no file
//! appeared" would be satisfied by a test that never had a working write.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::send_message::{MessageTransport, ParsedTarget, SendMessageTool, SendOutcome};
use wcore_types::message::ContentBlock;

use super::*;
use crate::journal_effects::{JournalEffectCoordinator, TurnEffectScope};
use crate::session_journal::{SessionEvent, SessionJournal};

const TURN: &str = "turn-human-unreachable";

/// A transport whose delivery can be switched at runtime, so one test can walk
/// the real sequence: refused, refused, delivered.
struct SwitchableTransport {
    deliverable: AtomicBool,
}

impl SwitchableTransport {
    fn new(deliverable: bool) -> Self {
        Self {
            deliverable: AtomicBool::new(deliverable),
        }
    }
}

#[async_trait]
impl MessageTransport for SwitchableTransport {
    async fn send(&self, _target: &ParsedTarget, _message: &str) -> SendOutcome {
        if self.deliverable.load(Ordering::Acquire) {
            SendOutcome::Ok {
                message_id: Some("delivered-1".to_string()),
            }
        } else {
            // The exact shape the fixture produced on port 1.
            SendOutcome::Err {
                message: "transport: Connection error: Connection refused (os error 111)"
                    .to_string(),
            }
        }
    }
}

fn scope_fixture(dir: &Path) -> (SessionJournal, TurnEffectScope) {
    let journal = SessionJournal::open(dir.join("session.journal"), "session").unwrap();
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: TURN.into(),
            user_message: "unreachable-human freeze proof".into(),
        })
        .unwrap();
    let scope = JournalEffectCoordinator::new(journal.clone()).for_turn(TURN);
    (journal, scope)
}

fn call(id: &str, name: &str, input: Value) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: name.into(),
        input,
        extra: None,
    }
}

fn registry_with(transport: Arc<SwitchableTransport>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SendMessageTool::new(transport)));
    registry.register(Box::new(wcore_tools::write::WriteTool::new(None)));
    registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    registry
}

/// Dispatch through the same entry point the agent loop uses, and mirror the
/// loop's outcome bookkeeping. `execute_single_with_streaming` records the
/// latch itself; this helper exists so each test reads as a sequence of tool
/// calls rather than as plumbing.
async fn dispatch(
    registry: &ToolRegistry,
    call: &ContentBlock,
    scope: &TurnEffectScope,
    ordinal: u64,
) -> ContentBlock {
    let (result, ..) = execute_single_with_streaming(
        registry,
        call,
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        None,
        None,
        false,
        &CancellationToken::new(),
        None,
        Some(scope),
        ordinal,
        None,
    )
    .await;
    result
}

fn is_error(result: &ContentBlock) -> bool {
    match result {
        ContentBlock::ToolResult { is_error, .. } => *is_error,
        other => panic!("expected a ToolResult, got {other:?}"),
    }
}

fn text(result: &ContentBlock) -> String {
    match result {
        ContentBlock::ToolResult { content, .. } => content.clone(),
        other => panic!("expected a ToolResult, got {other:?}"),
    }
}

fn send_call(id: &str) -> ContentBlock {
    call(
        id,
        "send_message",
        json!({
            "target": "email:oncall@fixture.local",
            "message": "moneykit 1.4.3 is EOL. Approve a bump to 2.0.0?"
        }),
    )
}

// ---------------------------------------------------------------------------
// The defect: a failed ask must not be followed by the act.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_failed_send_freezes_the_write_that_follows() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let target = dir.path().join("requirements.txt");

    let registry = registry_with(Arc::new(SwitchableTransport::new(false)));

    let sent = dispatch(&registry, &send_call("ask"), &scope, 0).await;
    assert!(
        is_error(&sent),
        "the undeliverable send must report failure"
    );
    assert!(
        registry.human_unreachable(),
        "a failed send must arm the latch"
    );

    let wrote = dispatch(
        &registry,
        &call(
            "act",
            "Write",
            json!({"file_path": target.to_str().unwrap(), "content": "moneykit==2.0.0\n"}),
        ),
        &scope,
        1,
    )
    .await;

    assert!(is_error(&wrote), "the write must be refused");
    let refusal = text(&wrote);
    assert!(
        refusal.contains("no supervision"),
        "the refusal must name the reason; got: {refusal}"
    );
    assert!(
        !target.exists(),
        "the pin was rewritten with nobody reachable to approve it: {}",
        target.display()
    );
}

/// The control, and the wrong-direction check in one: identical scenario with
/// the message DELIVERED. The latch never arms and the write lands, so the
/// assertions above are capable of failing and the fix cannot be passing by
/// refusing everything.
#[tokio::test]
async fn control_the_same_write_lands_when_the_message_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let target = dir.path().join("requirements.txt");

    let registry = registry_with(Arc::new(SwitchableTransport::new(true)));

    let sent = dispatch(&registry, &send_call("ask"), &scope, 0).await;
    assert!(!is_error(&sent), "the send must succeed: {}", text(&sent));
    assert!(
        !registry.human_unreachable(),
        "a delivered send must leave the latch clear"
    );

    let wrote = dispatch(
        &registry,
        &call(
            "act",
            "Write",
            json!({"file_path": target.to_str().unwrap(), "content": "moneykit==2.0.0\n"}),
        ),
        &scope,
        1,
    )
    .await;

    assert!(!is_error(&wrote), "the write must run: {}", text(&wrote));
    assert!(
        target.exists(),
        "the control proves a working write; without it the test above is vacuous"
    );
}

/// The real green-run sequence: three refusals, then a delivery. A counter
/// would have killed this run; the latch must lift on the delivery and let the
/// work through.
#[tokio::test]
async fn three_failures_then_a_delivery_lifts_the_freeze() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let target = dir.path().join("requirements.txt");

    let transport = Arc::new(SwitchableTransport::new(false));
    let registry = registry_with(transport.clone());

    for (n, id) in ["ask-1", "ask-2", "ask-3"].iter().enumerate() {
        let sent = dispatch(&registry, &send_call(id), &scope, n as u64).await;
        assert!(is_error(&sent), "attempt {id} must fail");
        assert!(registry.human_unreachable(), "latch armed after {id}");
    }

    // Three failures inside the breaker's 30s window is exactly what opens it.
    // The exemption is what makes the next line reach the transport at all:
    // without it the fourth attempt — the one the mail host recorded as
    // DELIVERED in the live green run — is refused locally and the session is
    // frozen out of its own recovery.
    assert!(
        registry.breaker_is_open("send_message"),
        "precondition: three failures in one window must open the breaker, \
         otherwise this test is not exercising the exemption"
    );

    transport.deliverable.store(true, Ordering::Release);
    let sent = dispatch(&registry, &send_call("ask-4"), &scope, 3).await;
    assert!(
        !is_error(&sent),
        "the fourth attempt must be delivered: {}",
        text(&sent)
    );
    assert!(
        !registry.human_unreachable(),
        "a delivered message must lift the freeze"
    );

    let wrote = dispatch(
        &registry,
        &call(
            "act",
            "Write",
            json!({"file_path": target.to_str().unwrap(), "content": "moneykit==2.0.0\n"}),
        ),
        &scope,
        4,
    )
    .await;
    assert!(!is_error(&wrote), "the write must run: {}", text(&wrote));
    assert!(target.exists());
}

/// The freeze must not wall off the one call that can lift it. An earlier
/// shape of this gate refused every non-read-only tool, `send_message`
/// included, which locked the session inside the failure it was trying to
/// escape.
#[tokio::test]
async fn the_freeze_never_blocks_reaching_the_human() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());

    let transport = Arc::new(SwitchableTransport::new(false));
    let registry = registry_with(transport.clone());

    dispatch(&registry, &send_call("ask-1"), &scope, 0).await;
    assert!(registry.human_unreachable());

    transport.deliverable.store(true, Ordering::Release);
    let retry = dispatch(&registry, &send_call("ask-2"), &scope, 1).await;
    let body = text(&retry);
    assert!(
        !is_error(&retry) && !body.contains("no supervision"),
        "the retry must reach the transport, not the gate; got: {body}"
    );
}

/// Read-only tools keep working, so a frozen agent can still diagnose the
/// channel and report what happened instead of going dark.
#[tokio::test]
async fn read_only_tools_still_run_under_the_freeze() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let policy = dir.path().join("POLICY.md");
    std::fs::write(&policy, "A major bump needs a named human approval.\n").unwrap();

    let registry = registry_with(Arc::new(SwitchableTransport::new(false)));
    dispatch(&registry, &send_call("ask"), &scope, 0).await;
    assert!(registry.human_unreachable());

    let read = dispatch(
        &registry,
        &call(
            "read",
            "Read",
            json!({"file_path": policy.to_str().unwrap()}),
        ),
        &scope,
        1,
    )
    .await;
    assert!(
        !is_error(&read),
        "a frozen session must still be able to read: {}",
        text(&read)
    );
}

// ---------------------------------------------------------------------------
// wayland#585 composed with this gate — the throttle must not be read as a
// down route.
//
// The #585 tool-seam rate limiter returns `SendOutcome::Err`, which
// `SendMessageTool` renders as an `is_error` ToolResult (that is the whole
// point: only an error result ends a model-driven loop). But
// `record_human_reach_outcome` above arms the freeze on ANY `is_error` from a
// `reaches_a_human` tool, with no `Tool::error_is_tool_fault` neutrality —
// unlike `record_dispatch_outcome`, which has it. So the guard that exists to
// stop an agent-to-agent ping-pong ALSO freezes every world-changing tool for
// the rest of the session, at the exact moment the outbound route has just
// demonstrated it is healthy by delivering thirty messages.
//
// The agent cannot talk its way out either: the refusal text tells it to stop
// sending to this conversation, and every further send to that conversation is
// refused for the same reason, so the one call carved out of the freeze cannot
// clear the latch.
// ---------------------------------------------------------------------------

/// Build a registry whose `send_message` runs through the REAL
/// `ChannelManagerTransport` (the production seam #585 throttles) over a
/// healthy mock email channel. Nothing here models the limiter; the limiter
/// under test is the shipped one.
async fn registry_with_real_channel_transport() -> ToolRegistry {
    let mut mgr = wcore_channels::ChannelManager::new();
    mgr.register(Box::new(
        wcore_channels::MockChannel::new("email").with_platform("email"),
    ))
    .await;
    mgr.start_all().await.expect("start channels");
    let transport = Arc::new(crate::channel_send_transport::ChannelManagerTransport::new(
        Arc::new(tokio::sync::RwLock::new(mgr)),
    ));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SendMessageTool::new(transport)));
    registry.register(Box::new(wcore_tools::write::WriteTool::new(None)));
    registry
}

#[tokio::test]
async fn a_rate_limited_send_must_not_freeze_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (_journal, scope) = scope_fixture(dir.path());
    let registry = registry_with_real_channel_transport().await;

    // Spend the per-conversation budget. Every one of these is DELIVERED —
    // the outbound route is demonstrably up.
    let cap = wcore_channels::DEFAULT_MAX_AUTO_REPLIES;
    for i in 0..cap {
        let sent = dispatch(&registry, &send_call(&format!("ask-{i}")), &scope, i as u64).await;
        assert!(
            !is_error(&sent),
            "in-budget send {i} must be delivered; got: {}",
            text(&sent)
        );
    }
    assert!(
        !registry.human_unreachable(),
        "precondition: thirty delivered sends leave the latch clear"
    );

    // CONTROL: with the route up, a write runs. Without this the assertion
    // below could pass on a harness that never had a working write.
    let control = dir.path().join("control.txt");
    let wrote = dispatch(
        &registry,
        &call(
            "control-write",
            "Write",
            json!({"file_path": control.to_str().unwrap(), "content": "ok\n"}),
        ),
        &scope,
        cap as u64,
    )
    .await;
    assert!(
        !is_error(&wrote),
        "control write must run: {}",
        text(&wrote)
    );
    assert!(control.exists(), "control proves a working write");

    // The over-budget send. It MUST reach the model as an error (#585) ...
    let over = dispatch(&registry, &send_call("over"), &scope, cap as u64 + 1).await;
    assert!(
        is_error(&over),
        "wayland#585: the throttled send must reach the model as an error"
    );
    assert!(
        text(&over).to_ascii_lowercase().contains("rate limit"),
        "precondition: this must be the rate-limit refusal, not some other \
         failure; got: {}",
        text(&over)
    );

    // ... and it must NOT be read as "the human is unreachable".
    assert!(
        !registry.human_unreachable(),
        "a rate-limited send is OUR throttle, not a down route: the human just \
         received thirty messages. Arming the freeze here stops every \
         world-changing tool for the rest of the session."
    );

    let after = dir.path().join("after-throttle.txt");
    let wrote = dispatch(
        &registry,
        &call(
            "act",
            "Write",
            json!({"file_path": after.to_str().unwrap(), "content": "still working\n"}),
        ),
        &scope,
        cap as u64 + 2,
    )
    .await;
    assert!(
        !is_error(&wrote),
        "the session must keep working after a throttled send; got: {}",
        text(&wrote)
    );
    assert!(after.exists(), "the write must have landed");
}
