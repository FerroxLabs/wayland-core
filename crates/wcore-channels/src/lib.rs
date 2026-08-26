//! `wcore-channels` — runtime abstraction for chat-platform adapters
//! (Slack, Discord, Telegram, WhatsApp, Signal, email, SMS, …).
//!
//! Defines the `Channel` trait + `ChannelEvent` enum + config loader
//! (landed in the v0.7.0 channels foundation). Individual channel impls
//! land as their own crates (`wcore-channel-slack` etc.) in the
//! v0.8 channels release. The `ChannelManager` that drives them lives
//! in `manager.rs`.
//!
//! Channels are message-passing surfaces, not transport primitives —
//! they wrap whatever platform-native API exists (HTTP REST, WS
//! gateway, subprocess, IMAP/SMTP) behind a uniform send + poll
//! interface so the engine + UI don't care which platform a message
//! came from.

pub mod actions;
pub mod auto_register;
pub mod binding;
pub mod chunk;
pub mod config;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod health;
pub mod manager;
pub mod media;
pub mod mock;
pub mod outgoing;
pub mod probe;
pub mod untrusted;
pub mod webhook;

pub use actions::{ActionSupport, NativeActions};
pub use binding::{Binding, BindingSource, BindingTable, ConversationRef, RouteTarget};
pub use chunk::chunk_message;
pub use config::{
    CREDENTIAL_HANDLE_MARKER, ChannelConfig, ChannelConfigLoader, credential_handles,
    parse_channel_config,
};
pub use dispatch::{
    ABSENT_FIELD, ADMISSION_SHAPE_VERSION, AccessDecision, AckMode, AdmissionFieldChange,
    AdmissionShape, AutoReplyOutcome, AutoReplyRateLimiter, ChannelOpenAdmission,
    ChannelToolPosture, DEFAULT_AGENT, DEFAULT_AUTO_REPLY_WINDOW, DEFAULT_CODE_TTL_MS,
    DEFAULT_CONVERSATION_CAP, DEFAULT_MAX_AUTO_REPLIES, DedupeCache, DedupeKey, DispatchOutcome,
    DmPolicy, GroupPolicy, InboundPolicy, LegacyToken, OPTION_FIELD_PREFIX, OpenAdmission,
    OpenAdmissionFault, OpenAdmissionRefusal, PAIRING_DENY_REASON, PairedSender, PairingBook,
    PairingState, PairingStore, RATE_LIMIT_NOTICE, SHAPE_FIELDS, TurnAdmission,
    admission_shape_token, build_session_key, classify, decide_access, decide_access_paired,
    evaluate, evaluate_paired, open_admission_faults, open_admissions, refuse_open_admission,
    required_acknowledgement,
};
pub use error::ChannelError;
pub use event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MAX_INBOX, MediaKind,
    MentionKind, MessageReceipt, push_bounded,
};
pub use health::{ChannelHealth, HealthState};
pub use manager::{ChannelManager, StartPolicy, TaggedEvent};
pub use media::{MediaBounds, MediaDisposition, RawAttachment};
pub use mock::MockChannel;
pub use outgoing::OutgoingMessage;
pub use probe::{ProbeOutcome, ProbeReport};
pub use webhook::{WebhookRequest, WebhookResponse};

use async_trait::async_trait;

/// One chat-platform adapter — wraps the platform's native API
/// behind a uniform send + poll surface.
///
/// Lifecycle: construct → `start()` → loop `poll_events()` /
/// `send_message()` until `stop()` is called. `start`/`stop` are
/// idempotent (calling `start` on an already-started channel is a
/// no-op, same for `stop` on a stopped one).
#[async_trait]
pub trait Channel: Send + Sync {
    /// Stable identifier for this channel. Matches the config file
    /// stem at `~/.wayland/channels/<name>.toml`. Used for routing.
    fn name(&self) -> &str;

    /// Platform tag — `"slack"`, `"discord"`, `"telegram"`, etc.
    /// Multiple channel instances can share a platform (two Slack
    /// workspaces, for example) but each has a unique `name()`.
    fn platform(&self) -> &str;

    /// Open the underlying connection / start polling. Idempotent.
    async fn start(&mut self) -> Result<(), ChannelError>;

    /// Close the underlying connection. Idempotent. After `stop()`
    /// further `poll_events` / `send_message` calls surface
    /// `ChannelError::NotStarted`.
    async fn stop(&mut self) -> Result<(), ChannelError>;

    /// Poll for any events that have arrived since the last call.
    /// Returns an empty vec if no events are ready. Non-blocking by
    /// contract — channels that need to wait spawn an internal task
    /// in `start()` and buffer into a queue.
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError>;

    /// Send a message through this channel. Returns a receipt with
    /// the platform-assigned ID (so callers can correlate with
    /// later `ChannelEvent::MessageReceived` echoes).
    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError>;

    /// Send `msg` carrying a caller-supplied idempotency `key`, so a retry of
    /// the SAME logical delivery produces one message at the destination.
    ///
    /// # Why this exists, measured
    ///
    /// Phase 24, lane 24c. The gateway's delivery ledger keeps four states so
    /// that only an attempt whose outcome is UNKNOWN is retried on restart.
    /// It does that correctly. But retrying an unknown-outcome delivery at a
    /// destination that cannot recognise the replay **is** the duplicate the
    /// phase's first Success Criterion forbids — and it was measured, against
    /// an independent sink, on real `systemd`: delivery `f24c-delivery-09`
    /// landed, the gateway was `kill -9`'d before it could settle, the
    /// platform restarted it, and the destination recorded the SAME body a
    /// second time. The ledger even knew the key was identical.
    ///
    /// The ledger's own module documentation named the missing half: the key
    /// lives in the ledger and not on the wire, so "a destination which needs
    /// the key transmitted must be handed it explicitly by its adapter". This
    /// is that hand-off.
    ///
    /// # The default is deliberately a pass-through, and that is why
    /// [`supports_outbound_idempotency`](Self::supports_outbound_idempotency)
    /// exists
    ///
    /// An adapter whose platform has no idempotency surface cannot suppress a
    /// replay, and pretending otherwise would be worse than not trying. So the
    /// default ignores the key — and declares, through the capability method,
    /// that it did. The gateway consults that declaration and refuses to
    /// re-dispatch an unknown-outcome delivery to a destination that cannot
    /// deduplicate it, recording it by name instead. A silent default here
    /// would convert a visible duplicate into an invisible one.
    async fn send_message_idempotent(
        &mut self,
        msg: OutgoingMessage,
        _key: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        self.send_message(msg).await
    }

    /// Whether this adapter actually transmits an idempotency key the
    /// destination will honour.
    ///
    /// Default `false`, because most platforms have no such surface. This is a
    /// CAPABILITY declaration, not a preference: the delivery spine reads it to
    /// decide whether an outcome-unknown delivery may be retried at all, so an
    /// adapter that returns `true` without transmitting the key would
    /// reintroduce exactly the duplicate this method exists to prevent.
    fn supports_outbound_idempotency(&self) -> bool {
        false
    }

    /// Answer the setup and authentication probe WITHOUT sending a message:
    /// is the configuration complete, does the credential authenticate, and
    /// what identity did it authenticate as.
    ///
    /// # The default reports `Unsupported`, never a green
    ///
    /// An adapter that has not implemented this returns
    /// [`ProbeOutcome::Unsupported`](crate::probe::ProbeOutcome::Unsupported) —
    /// a NAMED state meaning "nothing was checked". A default of `Ok` would be
    /// an adapter attesting to its own configuration without looking at it,
    /// which is the failure shape this phase keeps measuring: lane 24c's
    /// gateway reported a clean carry from its own ledger while an independent
    /// destination held a duplicate. The probe must never be the sole witness
    /// to a configuration it did not read.
    ///
    /// Takes `&self` (like `react`/`ingest_webhook`): a probe reads, it does
    /// not drive the lifecycle, so it must be callable while the poll loop
    /// holds the adapter.
    async fn probe(&self) -> Result<ProbeReport, ChannelError> {
        Ok(ProbeReport::unsupported(self.name(), self.platform()))
    }

    /// This adapter's DECLARED inbound media bounds. The default is finite —
    /// an unbounded default is a fetch whose size a hostile sender chooses.
    ///
    /// # Where this is enforced, and why the answer used to be "nowhere"
    ///
    /// `max_bytes` is enforced on every fetched payload by
    /// [`ChannelManager::fetch_media_on`](crate::ChannelManager::fetch_media_on),
    /// which is the only production path to adapter media, and again inside
    /// each adapter's own download path so an oversize body is refused while
    /// streaming rather than after it has been buffered. Both read one constant
    /// per adapter crate, so the advertised number and the enforced number are
    /// the same number by construction.
    ///
    /// That last sentence is load-bearing and was not previously true. This
    /// method was declared, overridden, and then read at exactly one site in
    /// the entire workspace — a unit test. Every adapter that enforced a cap
    /// enforced an unrelated hardcoded constant instead, diverging in BOTH
    /// directions: discord advertised 25 MiB and fetched up to 100, email
    /// advertised 10 MiB and never inlined past 2, and seven further adapters
    /// advertised nothing at all while enforcing 100/64/16 MiB. Read
    /// [`media::normalize`](crate::media::normalize) for the declaration-time
    /// half (platform-reported size, before any fetch); it is deliberately NOT
    /// the only enforcement point, because a platform that reports no size is
    /// not a platform sending a small file.
    fn media_bounds(&self) -> MediaBounds {
        MediaBounds::default()
    }

    /// This adapter's DECLARED native-action surface — which of `edit`,
    /// `delete`, `react` and `typing` it drives against the platform, and for
    /// each one it does not, whether **the platform has no such API** or
    /// **nobody has written it yet**.
    ///
    /// # Why a declaration and not just the call's error
    ///
    /// [`edit_message`](Self::edit_message) and
    /// [`delete_message`](Self::delete_message) already fail honestly, with a
    /// named [`ChannelError::Unsupported`]. But on 2026-07-30 **zero of ten
    /// adapters overrode either**, and that `Unsupported` was indistinguishable
    /// from WhatsApp Cloud API's permanent absence of an edit endpoint. One is a
    /// fact about somebody else's product; the other was our backlog. The only
    /// way to tell them apart was to make the call — which, for a delete, is a
    /// request a caller may not want to issue speculatively.
    ///
    /// This is the same construct as
    /// [`supports_outbound_idempotency`](Self::supports_outbound_idempotency),
    /// applied to the native-action half: a capability read BEFORE dispatch so
    /// the caller can choose a different action rather than learn from a
    /// failure.
    ///
    /// # The declaration is checked against behaviour, in both directions
    ///
    /// `wcore-channels-registry/tests/native_action_matrix.rs` builds every
    /// adapter the registry can construct and asserts:
    /// declared-[`Implemented`](crate::ActionSupport::Implemented) ops must NOT
    /// answer `Unsupported`, and everything else MUST. An adapter that claims a
    /// capability it lacks reddens, and so does one that implements a capability
    /// it forgot to declare.
    ///
    /// Default: [`NativeActions::none`] — everything `NotImplemented`. A new
    /// adapter therefore cannot inherit a green by staying silent.
    fn native_actions(&self) -> NativeActions {
        NativeActions::none()
    }

    /// Edit an already-sent message.
    ///
    /// Default: [`ChannelError::Unsupported`] — a NAMED outcome, never a silent
    /// `Ok`. A caller that receives `Ok` from a platform with no edit API
    /// believes the message changed; the next reader sees the original.
    ///
    /// Ask [`native_actions`](Self::native_actions) first if you need to know
    /// whether this will reach a real implementation without calling it.
    async fn edit_message(
        &self,
        _conversation_id: &str,
        _message_id: &str,
        _new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        Err(ChannelError::Unsupported {
            op: "edit".to_string(),
            platform: self.platform().to_string(),
        })
    }

    /// Delete an already-sent message.
    ///
    /// Default: [`ChannelError::Unsupported`]. Same reasoning as
    /// [`edit_message`](Self::edit_message), and worse in consequence — a
    /// silent success here reads as "the message is gone" when it is not.
    async fn delete_message(
        &self,
        _conversation_id: &str,
        _message_id: &str,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::Unsupported {
            op: "delete".to_string(),
            platform: self.platform().to_string(),
        })
    }

    /// Fingerprint of the configuration this adapter was constructed from, used
    /// by [`ChannelManager::reload`](crate::manager::ChannelManager::reload) to
    /// decide whether a re-registered adapter actually CHANGED.
    ///
    /// Default `None` meaning "cannot tell", which reload treats as CHANGED and
    /// therefore replaces. That direction is deliberate: replacing an unchanged
    /// adapter costs a reconnect, whereas keeping a changed one running means
    /// an operator edits a credential, reloads, sees success, and keeps sending
    /// through the old one.
    ///
    /// An adapter that returns a fingerprint MUST derive it from configuration
    /// only, and must never let a secret's VALUE into it — a fingerprint is
    /// surfaced in reload output. Hash it.
    fn config_fingerprint(&self) -> Option<String> {
        None
    }

    /// Returns the JSON-schema doc string for this channel's
    /// config TOML. UI uses this to render a setup form; tests use
    /// it to validate config files.
    fn config_schema(&self) -> &str;

    /// Handle of the connector's internal background task, if any. The manager
    /// uses this to detect a dead task and trigger supervised reconnect even when
    /// `poll_events` returns `Ok(vec![])` (the inbox-drain connectors whose
    /// background task can die silently). Default `None`: webhook-only connectors
    /// have no task.
    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        None
    }

    /// Maximum length (in Unicode scalar values) of a single outbound
    /// message this platform accepts, or `None` when effectively
    /// unbounded / unknown. [`ChannelManager::send_to`] splits longer
    /// bodies into in-order chunks via
    /// [`chunk_message`](crate::chunk::chunk_message) before sending, so
    /// an over-long agent reply is delivered in pieces instead of being
    /// rejected and dropped by the platform. Each connector declares its
    /// own cap here — the shared layer never hardcodes a per-platform
    /// limit.
    fn max_message_len(&self) -> Option<usize> {
        None
    }

    /// Send a transient "typing…" indicator to `conversation_id`.
    ///
    /// Default: no-op `Ok(())` — platforms without a typing API simply do
    /// nothing. The inbound subscriber calls this periodically while a turn
    /// is running (when the channel's ack mode enables typing) so a human
    /// sees the bot is working. Must be cheap and best-effort; a failure is
    /// logged and ignored, never fatal to the turn.
    async fn send_typing(&self, _conversation_id: &str) -> Result<(), ChannelError> {
        Ok(())
    }

    /// React to a message with a single unicode emoji — the ack/status
    /// signal used by the subscriber's ack state machine (👀 received →
    /// ✅ done / ❌ failed).
    ///
    /// Default: [`ChannelError::Unsupported`] — the platform has no reaction
    /// API, or it isn't implemented for this connector. The subscriber treats a
    /// reaction failure as non-fatal.
    ///
    /// This was `Rejected("reactions unsupported")` before Phase 24 made edit,
    /// delete and reaction contract operations. `Rejected` means the platform
    /// looked at the request and said no, which is a retryable condition;
    /// "there is no reaction API" is not. Folding the two together let a caller
    /// retry forever against a surface that will never exist.
    async fn react(
        &self,
        _conversation_id: &str,
        _message_id: &str,
        _emoji: &str,
    ) -> Result<(), ChannelError> {
        Err(ChannelError::Unsupported {
            op: "react".to_string(),
            platform: self.platform().to_string(),
        })
    }

    /// Handle an inbound webhook HTTP request routed to this channel by
    /// the inbound webhook host.
    ///
    /// Default: **unsupported** — poll-based connectors (telegram,
    /// matrix, signal, …) and any connector whose inbound path is not yet
    /// authenticated return `Rejected`, so the host never exposes an
    /// unauthenticated parse to the network. This default is the ONLY thing
    /// keeping such a connector off the network: the host routes
    /// `/webhooks/:channel` by name and holds no per-platform allow-list.
    ///
    /// Connectors that authenticate the caller override this to verify →
    /// parse → enqueue (mirroring their existing `ingest_*` methods) and
    /// return a [`WebhookResponse`]. Today: Slack, WhatsApp and Twilio SMS
    /// (HMAC signature over the raw body) and MS Teams (Bot Framework JWT —
    /// signature, issuer, audience, expiry, plus a `serviceUrl` claim binding).
    ///
    /// Takes `&self` (not `&mut self`): connectors enqueue through their
    /// interior-mutable inbox, so the host can ingest concurrently with
    /// the poll loop without an exclusive borrow.
    async fn ingest_webhook(&self, _req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        Err(ChannelError::Rejected(
            "channel does not accept inbound webhooks".to_string(),
        ))
    }

    /// Fetch the raw bytes of an inbound [`Attachment`](crate::event::Attachment)
    /// using THIS connector's own credentials and platform media protocol.
    ///
    /// Media URLs differ per platform: Telegram/Discord expose a directly
    /// fetchable URL; Slack needs a bearer on `url_private`; WhatsApp resolves
    /// a media-id to a short-lived URL then downloads it; Matrix translates an
    /// `mxc://` URI to the authenticated download endpoint. The agent-side
    /// media enricher calls this through [`ChannelManager::fetch_media_on`] so
    /// credentials never leave the connector boundary.
    ///
    /// Default: **unsupported** — a connector that doesn't override this (no
    /// inbound media, or none wired yet) returns `Rejected`, and the enricher
    /// falls back to the bare-URL summary. Takes `&self` (like `react` /
    /// `ingest_webhook`): the read-only download uses the connector's
    /// immutable client + token.
    async fn fetch_media(
        &self,
        _attachment: &crate::event::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        Err(ChannelError::Rejected(
            "media fetch unsupported".to_string(),
        ))
    }
}
