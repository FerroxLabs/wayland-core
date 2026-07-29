//! The channel framework contract matrix — one case per contract element.
//!
//! Phase 24 Success Criterion 3. Each case is written so that removing the
//! implementation it covers turns it red; the mutation measurements are
//! recorded in `24-03-SURFACE-CONTRACT.md`.
//!
//! The recurring design rule across this file: **nothing is allowed to be the
//! sole witness to its own correctness.** Health is asserted from the manager's
//! recorded observations, never by asking an adapter how it is. Probe redaction
//! is asserted with a positive control proving the canary really was in the
//! adapter's hands first. Reload is asserted on the identity of the instance
//! that survives, not on the report the reload itself wrote.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use wcore_channels::Channel;
use wcore_channels::binding::{BindingSource, BindingTable, ConversationRef, RouteTarget};
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::health::HealthState;
use wcore_channels::manager::{ChannelManager, StartPolicy};
use wcore_channels::media::{MediaBounds, RawAttachment, normalize};
use wcore_channels::mock::MockChannel;
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::probe::{ProbeOutcome, ProbeReport};

/// The canary a probe must never emit. Long enough to be unambiguous in a
/// serialized blob.
const PROBE_CANARY: &str = "xoxb-F24D-MATRIX-CANARY-7c41e9b02d5a";

/// A reference adapter that implements the full contract, so the matrix can
/// distinguish "the contract works" from "the default fired".
///
/// It is deliberately configurable into each probe outcome, because a probe
/// that can only report success is not a probe.
struct ContractChannel {
    name: String,
    /// The credential this adapter holds. Present here so the redaction case
    /// has a POSITIVE CONTROL: the canary provably reached the adapter.
    token: Option<String>,
    /// Whether the platform would accept `token`.
    token_valid: bool,
    fingerprint: Option<String>,
    /// Distinguishes one INSTANCE from another with identical configuration —
    /// the reload cases assert on which instance survived.
    instance_tag: String,
    started: bool,
    inbound: VecDeque<ChannelEvent>,
    edits: Arc<AtomicU32>,
    deletes: Arc<AtomicU32>,
}

impl ContractChannel {
    fn new(name: &str, instance_tag: &str) -> Self {
        Self {
            name: name.to_string(),
            token: Some(PROBE_CANARY.to_string()),
            token_valid: true,
            fingerprint: Some("fp-v1".to_string()),
            instance_tag: instance_tag.to_string(),
            started: false,
            inbound: VecDeque::new(),
            edits: Arc::new(AtomicU32::new(0)),
            deletes: Arc::new(AtomicU32::new(0)),
        }
    }

    fn with_fingerprint(mut self, fp: Option<&str>) -> Self {
        self.fingerprint = fp.map(str::to_string);
        self
    }
}

#[async_trait]
impl Channel for ContractChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn platform(&self) -> &str {
        "contract"
    }
    async fn start(&mut self) -> Result<(), ChannelError> {
        self.started = true;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), ChannelError> {
        self.started = false;
        Ok(())
    }
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        if !self.started {
            return Err(ChannelError::NotStarted);
        }
        Ok(self.inbound.drain(..).collect())
    }
    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        if !self.started {
            return Err(ChannelError::NotStarted);
        }
        // The receipt id carries the instance tag: this is how the reload cases
        // observe WHICH instance answered, rather than trusting the report the
        // reload wrote about itself.
        Ok(MessageReceipt {
            id: format!("{}-out", self.instance_tag),
            conversation_id: msg.conversation_id,
            ts_secs: 0,
        })
    }
    async fn probe(&self) -> Result<ProbeReport, ChannelError> {
        match (&self.token, self.token_valid) {
            (None, _) => Ok(ProbeReport::incomplete(
                &self.name,
                self.platform(),
                vec!["bot_token".to_string()],
            )),
            (Some(_), false) => Ok(ProbeReport::unauthenticated(
                &self.name,
                self.platform(),
                "invalid_auth",
            )),
            (Some(_), true) => Ok(ProbeReport::ok(
                &self.name,
                self.platform(),
                "U-contract-bot",
            )),
        }
    }
    async fn edit_message(
        &self,
        conversation_id: &str,
        _message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.edits.fetch_add(1, Ordering::SeqCst);
        Ok(MessageReceipt {
            id: format!("{}-edit:{new_text}", self.instance_tag),
            conversation_id: conversation_id.to_string(),
            ts_secs: 0,
        })
    }
    async fn delete_message(
        &self,
        _conversation_id: &str,
        _message_id: &str,
    ) -> Result<(), ChannelError> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn config_fingerprint(&self) -> Option<String> {
        self.fingerprint.clone()
    }
    fn media_bounds(&self) -> MediaBounds {
        MediaBounds {
            max_bytes: 2048,
            max_attachments: 3,
        }
    }
    fn config_schema(&self) -> &str {
        r#"{"bot_token": "string"}"#
    }
}

/// An adapter whose poll fails until `heal` is set — drives the supervised
/// reconnect path so health can be observed transitioning and RECOVERING.
struct FlappingChannel {
    name: String,
    started: bool,
    heal: Arc<AtomicBool>,
}

#[async_trait]
impl Channel for FlappingChannel {
    fn name(&self) -> &str {
        &self.name
    }
    fn platform(&self) -> &str {
        "flapping"
    }
    async fn start(&mut self) -> Result<(), ChannelError> {
        self.started = true;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), ChannelError> {
        self.started = false;
        Ok(())
    }
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        if self.heal.load(Ordering::SeqCst) {
            Ok(Vec::new())
        } else {
            Err(ChannelError::Transport("socket reset by peer".into()))
        }
    }
    async fn send_message(&mut self, _m: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        Err(ChannelError::NotStarted)
    }
    fn config_schema(&self) -> &str {
        "{}"
    }
}

/// An adapter that publishes a platform-reported auth failure.
struct AuthExpiringChannel {
    started: bool,
    emitted: bool,
}

#[async_trait]
impl Channel for AuthExpiringChannel {
    fn name(&self) -> &str {
        "expiring"
    }
    fn platform(&self) -> &str {
        "contract"
    }
    async fn start(&mut self) -> Result<(), ChannelError> {
        self.started = true;
        Ok(())
    }
    async fn stop(&mut self) -> Result<(), ChannelError> {
        self.started = false;
        Ok(())
    }
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        if !self.emitted {
            self.emitted = true;
            return Ok(vec![ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::AuthError,
            }]);
        }
        Ok(Vec::new())
    }
    async fn send_message(&mut self, _m: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        Err(ChannelError::NotStarted)
    }
    fn config_schema(&self) -> &str {
        "{}"
    }
}

// ── Element 1: setup and authentication probe ────────────────────────────

#[tokio::test]
async fn probe_default_is_a_named_unsupported_and_never_a_green() {
    // The single most dangerous default in this contract. An adapter that never
    // implemented a probe must not read as ready, or every one of the ten
    // registered adapters silently attests to a configuration nobody checked.
    let ch = MockChannel::new("acme");
    let report = ch.probe().await.expect("default probe returns a report");
    assert_eq!(report.outcome, ProbeOutcome::Unsupported);
    assert!(!report.outcome.is_ready());
    assert!(!report.config_complete);
    assert!(!report.authenticated);
    assert!(
        !report.findings.is_empty(),
        "the report must SAY that nothing was checked, not just fail to claim it did"
    );
}

#[tokio::test]
async fn probe_reports_each_setup_state_as_a_distinct_operator_action() {
    let mut ch = ContractChannel::new("acme", "A");
    assert_eq!(ch.probe().await.unwrap().outcome, ProbeOutcome::Ok);
    assert_eq!(
        ch.probe().await.unwrap().identity.as_deref(),
        Some("U-contract-bot"),
        "the identity is the point: a channel started against the wrong \
         workspace looks identical to a right one without it"
    );

    ch.token_valid = false;
    let r = ch.probe().await.unwrap();
    assert_eq!(r.outcome, ProbeOutcome::Unauthenticated);
    assert!(
        r.config_complete,
        "config was complete; the credential was not"
    );

    ch.token = None;
    let r = ch.probe().await.unwrap();
    assert_eq!(r.outcome, ProbeOutcome::Incomplete);
    assert_eq!(r.findings, vec!["bot_token".to_string()]);
}

#[tokio::test]
async fn probe_output_never_carries_the_credential_it_tested() {
    // T-24-03-06, WITH A POSITIVE CONTROL. A canary that never reached the
    // adapter is trivially absent from its output, so first prove the adapter
    // is genuinely holding it.
    let ch = ContractChannel::new("acme", "A");
    assert_eq!(
        ch.token.as_deref(),
        Some(PROBE_CANARY),
        "POSITIVE CONTROL: the adapter really is holding the canary"
    );

    for valid in [true, false] {
        let mut ch = ContractChannel::new("acme", "A");
        ch.token_valid = valid;
        let report = ch.probe().await.unwrap();
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            !json.contains(PROBE_CANARY),
            "probe leaked the credential (token_valid={valid}): {json}"
        );
    }
}

#[tokio::test]
async fn probe_all_reports_every_registered_channel_including_the_unprobeable() {
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ContractChannel::new("real", "A")))
        .await;
    mgr.register(Box::new(MockChannel::new("silent"))).await;

    let reports = mgr.probe_all().await;
    assert_eq!(
        reports.len(),
        2,
        "a channel omitted from a probe listing is \
         indistinguishable from one that was never configured"
    );
    let by_name: std::collections::HashMap<_, _> =
        reports.iter().map(|r| (r.channel.as_str(), r)).collect();
    assert_eq!(by_name["real"].outcome, ProbeOutcome::Ok);
    assert_eq!(by_name["silent"].outcome, ProbeOutcome::Unsupported);
}

// ── Element 2: binding and routing ───────────────────────────────────────

#[test]
fn an_unbound_conversation_follows_the_declared_default() {
    // T-24-03-03. There is no constructor that omits the default, so there is
    // no path where an unbound conversation inherits the last-used identity.
    let table = BindingTable::new(RouteTarget::profile("declared-default"));
    let b = table.resolve(&ConversationRef {
        platform: "contract".into(),
        account_id: None,
        space_id: None,
        conversation_id: "brand-new".into(),
        thread_id: None,
    });
    assert_eq!(b.source, BindingSource::Default);
    assert_eq!(b.target.profile, "declared-default");
}

#[test]
fn binding_routes_a_thread_to_its_named_profile_and_agent() {
    let mut table = BindingTable::new(RouteTarget::profile("fallback"));
    let conv = ConversationRef {
        platform: "contract".into(),
        account_id: Some("A1".into()),
        space_id: None,
        conversation_id: "C1".into(),
        thread_id: Some("t7".into()),
    };
    table.bind(&conv, RouteTarget::with_agent("support", "triage"));
    let b = table.resolve(&conv);
    assert_eq!(b.source, BindingSource::Thread);
    assert_eq!(b.target.profile, "support");
    assert_eq!(b.target.agent.as_deref(), Some("triage"));
}

// ── Element 3: media normalisation ───────────────────────────────────────

#[test]
fn media_normalises_against_the_adapters_declared_bound_and_degrades_explicitly() {
    let ch = ContractChannel::new("acme", "A");
    let bounds = ch.media_bounds();
    assert_eq!(
        bounds.max_bytes, 2048,
        "the bound is the ADAPTER'S, not a constant here"
    );

    let (att, ok) = normalize(
        &RawAttachment {
            url: "https://x/a.png".into(),
            content_type: Some("image/png".into()),
            size_bytes: Some(100),
            filename: None,
        },
        bounds,
    );
    assert!(ok.is_fetchable());
    assert_eq!(att.kind, wcore_channels::event::MediaKind::Image);

    let (att, degraded) = normalize(
        &RawAttachment {
            url: "https://x/b.png".into(),
            content_type: Some("image/png".into()),
            size_bytes: Some(999_999),
            filename: None,
        },
        bounds,
    );
    assert!(!degraded.is_fetchable());
    assert!(degraded.reason().is_some(), "degradation must be explained");
    assert_eq!(
        att.url, "https://x/b.png",
        "the attachment survives — a silent drop is indistinguishable from \
         media the platform never sent"
    );
}

// ── Element 4: edit, delete and reaction ─────────────────────────────────

#[tokio::test]
async fn edit_delete_and_react_default_to_a_named_unsupported_never_a_silent_ok() {
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(MockChannel::new("plain"))).await;

    let e = mgr.edit_on("plain", "c1", "m1", "new").await.unwrap_err();
    assert!(
        matches!(&e, ChannelError::Unsupported { op, .. } if op == "edit"),
        "expected a named Unsupported, got {e:?}"
    );
    let e = mgr.delete_on("plain", "c1", "m1").await.unwrap_err();
    assert!(matches!(&e, ChannelError::Unsupported { op, .. } if op == "delete"));
    let e = mgr.react_on("plain", "c1", "m1", "👀").await.unwrap_err();
    assert!(matches!(&e, ChannelError::Unsupported { op, .. } if op == "react"));

    // And the message names the platform, so an operator reading a log knows
    // WHICH surface will never grow the API.
    assert!(e.to_string().contains("mock"), "got {e}");
}

#[tokio::test]
async fn an_implementing_adapter_actually_performs_edit_and_delete() {
    // The mirror case. Without it, an implementation that always returned
    // Unsupported would pass the case above and nothing else.
    let ch = ContractChannel::new("real", "A");
    let edits = Arc::clone(&ch.edits);
    let deletes = Arc::clone(&ch.deletes);
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ch)).await;

    let r = mgr.edit_on("real", "c1", "m1", "corrected").await.unwrap();
    assert!(r.id.contains("corrected"));
    mgr.delete_on("real", "c1", "m1").await.unwrap();
    assert_eq!(edits.load(Ordering::SeqCst), 1);
    assert_eq!(deletes.load(Ordering::SeqCst), 1);
}

// ── Element 5: health ────────────────────────────────────────────────────

#[tokio::test]
async fn a_registered_but_unstarted_channel_reads_unknown_not_healthy() {
    let mut mgr = ChannelManager::new();
    mgr.register(Box::new(ContractChannel::new("acme", "A")))
        .await;
    let h = mgr.health_of("acme").expect("registered");
    assert_eq!(
        h.state,
        HealthState::Unknown,
        "nothing has been polled; reporting healthy would be a claim with no \
         measurement behind it"
    );
    assert!(h.reason.is_some());
}

#[tokio::test]
async fn every_non_healthy_health_record_carries_a_reason() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
    mgr.register(Box::new(FlappingChannel {
        name: "flap".into(),
        started: false,
        heal: Arc::new(AtomicBool::new(false)),
    }))
    .await;
    mgr.start_all().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    for h in mgr.health() {
        assert!(
            h.reason_invariant_holds(),
            "health record without a reason is one an operator cannot act on: {h:?}"
        );
    }
    let h = mgr.health_of("flap").unwrap();
    assert_ne!(
        h.state,
        HealthState::Healthy,
        "a channel whose every poll fails is not healthy"
    );
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn a_platform_reported_auth_failure_is_distinct_from_a_transport_failure() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(10));
    mgr.register(Box::new(AuthExpiringChannel {
        started: false,
        emitted: false,
    }))
    .await;
    mgr.start_all().await.unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;

    let h = mgr.health_of("expiring").unwrap();
    assert_eq!(
        h.state,
        HealthState::Unauthenticated,
        "rotate-a-token and wait-for-the-network are different operator \
         actions and must be different states"
    );
    assert!(h.reason.is_some_and(|r| !r.is_empty()));
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn a_reconnect_is_observable_in_health_after_the_channel_recovers() {
    let heal = Arc::new(AtomicBool::new(false));
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(5));
    mgr.register(Box::new(FlappingChannel {
        name: "flap".into(),
        started: false,
        heal: Arc::clone(&heal),
    }))
    .await;
    mgr.start_all().await.unwrap();

    // Let it accumulate errors and enter supervised reconnect.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let degraded = mgr.health_of("flap").unwrap();
    assert_eq!(degraded.state, HealthState::Degraded);
    assert!(
        degraded.consecutive_errors > 0,
        "the error run must be visible"
    );

    // Heal the platform; the supervised reconnect's start() succeeds.
    heal.store(true, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let healed = mgr.health_of("flap").unwrap();
    assert_eq!(healed.state, HealthState::Healthy);
    assert!(
        healed.reconnects >= 1,
        "a channel that is up right now but has reconnected is FLAPPING; a \
         bare state field cannot say that. got {healed:?}"
    );
    mgr.stop_all().await.unwrap();
}

// ── Element 6: reload ────────────────────────────────────────────────────

#[tokio::test]
async fn reload_keeps_the_running_instance_of_an_unchanged_adapter() {
    // The defect this reaches: a reload that clears and re-registers everything
    // looks correct from its own report while silently discarding whatever the
    // surviving adapters were holding. The witness is the receipt's instance
    // tag — a fact the reload does not author.
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
    mgr.register(Box::new(ContractChannel::new("acme", "ORIGINAL")))
        .await;
    mgr.start_all().await.unwrap();

    let before = mgr
        .send_to("acme", OutgoingMessage::text("c1", "hi"))
        .await
        .unwrap();
    assert_eq!(before.id, "ORIGINAL-out");

    let report = mgr
        .reload(
            vec![Box::new(ContractChannel::new("acme", "REPLACEMENT"))],
            StartPolicy::StartNewlyRegistered,
        )
        .await;
    assert_eq!(report.unchanged, vec!["acme".to_string()]);
    assert!(report.replaced.is_empty());

    let after = mgr
        .send_to("acme", OutgoingMessage::text("c1", "hi"))
        .await
        .unwrap();
    assert_eq!(
        after.id, "ORIGINAL-out",
        "the unchanged adapter's RUNNING INSTANCE must survive a reload; \
         got the replacement, so the reload dropped whatever it was holding"
    );
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn reload_replaces_an_adapter_whose_fingerprint_changed() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
    mgr.register(Box::new(ContractChannel::new("acme", "ORIGINAL")))
        .await;
    mgr.start_all().await.unwrap();

    let report = mgr
        .reload(
            vec![Box::new(
                ContractChannel::new("acme", "ROTATED").with_fingerprint(Some("fp-v2")),
            )],
            StartPolicy::StartNewlyRegistered,
        )
        .await;
    assert_eq!(report.replaced, vec!["acme".to_string()]);
    let after = mgr
        .send_to("acme", OutgoingMessage::text("c1", "hi"))
        .await
        .unwrap();
    assert_eq!(
        after.id, "ROTATED-out",
        "an operator who rotated a credential and reloaded must not keep \
         sending through the adapter holding the old one"
    );
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn reload_treats_an_unfingerprintable_adapter_as_changed() {
    // "Cannot tell" must resolve toward replacing, not toward keeping. The
    // opposite direction is the silent-stale-credential bug.
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
    mgr.register(Box::new(
        ContractChannel::new("acme", "ORIGINAL").with_fingerprint(None),
    ))
    .await;
    mgr.start_all().await.unwrap();

    let report = mgr
        .reload(
            vec![Box::new(
                ContractChannel::new("acme", "FRESH").with_fingerprint(None),
            )],
            StartPolicy::StartNewlyRegistered,
        )
        .await;
    assert_eq!(report.replaced, vec!["acme".to_string()]);
    assert!(report.unchanged.is_empty());
    mgr.stop_all().await.unwrap();
}

#[tokio::test]
async fn reload_adds_new_adapters_and_removes_deconfigured_ones() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));
    mgr.register(Box::new(ContractChannel::new("gone", "A")))
        .await;
    mgr.start_all().await.unwrap();

    let report = mgr
        .reload(
            vec![Box::new(ContractChannel::new("fresh", "B"))],
            StartPolicy::StartNewlyRegistered,
        )
        .await;
    assert_eq!(report.added, vec!["fresh".to_string()]);
    assert_eq!(report.removed, vec!["gone".to_string()]);
    assert_eq!(mgr.list_names(), vec!["fresh".to_string()]);
    assert!(
        mgr.health_of("gone").is_none(),
        "a removed adapter must not linger in the health surface"
    );
    assert!(mgr.health_of("fresh").is_some());
    mgr.stop_all().await.unwrap();
}

// ---------------------------------------------------------------------------
// F24-C3-H6b — reload must not take the right to poll for itself.
//
// `reload` used to end in an unconditional `start_all`, so applying an adapter
// set and beginning to poll were one act. They are two decisions, and only the
// caller can make the second: the right to poll a home belongs to whoever holds
// the single-owner inbound polling lease. The gateway gates its STARTUP
// `start_all` on that lease and then reached `reload`, which started the poll
// tasks regardless.
//
// Polling is a destructive read (Telegram's `offset=` confirm deletes; IMAP sets
// `\Seen`), so the loser of that race sees NOTHING, not a duplicate. Both
// directions are asserted below, per LANE-BRIEF 3b-iii: a gate that cannot pass
// measures as little as one that cannot fail.
// ---------------------------------------------------------------------------

/// `LeaveStopped` must leave the newly added adapter registered and UNPOLLED.
///
/// `Unknown` is the assertion because it is the state that means "registered,
/// never observed". A started adapter cannot be in it.
#[tokio::test]
async fn reload_with_leave_stopped_registers_without_starting_a_poll_task() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));

    let report = mgr
        .reload(
            vec![Box::new(ContractChannel::new("unpolled", "A"))],
            StartPolicy::LeaveStopped,
        )
        .await;
    assert_eq!(report.added, vec!["unpolled".to_string()]);

    // Long enough that a spawned poll task at the 50 ms interval would have run
    // several times and moved the state off `Unknown`.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let health = mgr
        .health_of("unpolled")
        .expect("a registered adapter must appear in the health surface");
    assert_eq!(
        health.state,
        HealthState::Unknown,
        "reload was told LeaveStopped but polled anyway; a gateway that does not \
         hold the inbound polling lease would be stealing a destructive read \
         from the process that does. reason={:?}",
        health.reason
    );
    mgr.stop_all().await.unwrap();
}

/// The same call with `StartNewlyRegistered` MUST reach a polled state.
///
/// Without this the test above passes on a `reload` that can never start
/// anything at all — the permanently-red instrument LANE-BRIEF 3b-iii describes.
#[tokio::test]
async fn reload_with_start_newly_registered_does_start_a_poll_task() {
    let mut mgr = ChannelManager::new().with_poll_interval(Duration::from_millis(50));

    let report = mgr
        .reload(
            vec![Box::new(ContractChannel::new("polled", "A"))],
            StartPolicy::StartNewlyRegistered,
        )
        .await;
    assert_eq!(report.added, vec!["polled".to_string()]);

    let mut observed = HealthState::Unknown;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Some(h) = mgr.health_of("polled")
            && h.state != HealthState::Unknown
        {
            observed = h.state;
            break;
        }
    }
    assert_ne!(
        observed,
        HealthState::Unknown,
        "StartNewlyRegistered never produced an observation, so the assertion in \
         the LeaveStopped test above would hold no matter what reload did"
    );
    mgr.stop_all().await.unwrap();
}
