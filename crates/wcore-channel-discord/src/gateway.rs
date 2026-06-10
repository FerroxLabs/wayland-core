//! Discord Gateway protocol wiring.
//!
//! Two layers live in this file:
//!
//! 1. **Pure parsing + state machine** — `parse_payload`, `map_message_create`,
//!    and `HeartbeatTracker`. These have zero IO and exist so the unit
//!    tests can exercise the protocol without standing up a fake gateway
//!    server.
//! 2. **WebSocket driver** — `gateway_loop` connects to Discord, sends
//!    IDENTIFY (or RESUME on a resumable reconnect), runs HEARTBEATs on
//!    an interval, and pushes `MESSAGE_CREATE` events into the inbox.
//!    After READY we capture `session_id`, `resume_gateway_url`, and the
//!    latest dispatch sequence; on a resumable disconnect (op 7, a
//!    dropped socket, or op 9 with `d == true`) the next session connects
//!    to `resume_gateway_url` and sends a RESUME (op 6) so Discord
//!    replays the events buffered during the gap. A non-resumable op 9
//!    (`d == false`) clears the session and falls back to a fresh
//!    IDENTIFY after the Discord-required 1–5s wait.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use wcore_channels::event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MediaKind, MentionKind,
};

// =============================================================================
// Opcodes (https://discord.com/developers/docs/topics/gateway-events)
// =============================================================================

pub const OP_DISPATCH: u64 = 0;
pub const OP_HEARTBEAT: u64 = 1;
pub const OP_IDENTIFY: u64 = 2;
pub const OP_RESUME: u64 = 6;
pub const OP_RECONNECT: u64 = 7;
pub const OP_INVALID_SESSION: u64 = 9;
pub const OP_HELLO: u64 = 10;
pub const OP_HEARTBEAT_ACK: u64 = 11;

// =============================================================================
// Wire payloads
// =============================================================================

/// Raw envelope every Gateway frame uses: `{ op, d, s?, t? }`.
#[derive(Debug, Clone, Deserialize)]
pub struct GatewayPayload {
    pub op: u64,
    #[serde(default)]
    pub d: serde_json::Value,
    #[serde(default)]
    pub s: Option<i64>,
    #[serde(default)]
    pub t: Option<String>,
}

/// HELLO payload (`d` for op=10).
#[derive(Debug, Clone, Deserialize)]
pub struct HelloData {
    pub heartbeat_interval: u64,
}

/// READY payload (`d` for op=0 t="READY"). Carries the session handle
/// Discord wants echoed back on a RESUME plus the dedicated resume host.
#[derive(Debug, Clone, Deserialize)]
pub struct ReadyData {
    /// Opaque session identifier — required field of the RESUME payload.
    pub session_id: String,
    /// Host to reconnect to when resuming. Discord recommends using this
    /// instead of the normal gateway URL for resumes; absent on older
    /// gateway versions, in which case we fall back to the normal URL.
    #[serde(default)]
    pub resume_gateway_url: Option<String>,
}

/// MESSAGE_CREATE payload (`d` for op=0 t="MESSAGE_CREATE").
#[derive(Debug, Clone, Deserialize)]
pub struct MessageCreate {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub author: Option<MessageAuthor>,
    /// Present on guild messages; absent for DMs and group DMs.
    #[serde(default)]
    pub guild_id: Option<String>,
    /// Populated when the message is in a thread channel.
    #[serde(default)]
    pub thread: Option<MessageThread>,
    /// File / media attachments.
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
    /// Users explicitly `@mention`ed in the message body.
    #[serde(default)]
    pub mentions: Vec<MessageMention>,
    /// Inlined replied-to message (present when `message_type == 19`).
    #[serde(default)]
    pub referenced_message: Option<Box<ReferencedMessage>>,
    /// Lightweight reply cross-reference (message_id / channel / guild).
    #[serde(default)]
    pub message_reference: Option<MessageReference>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageAuthor {
    pub id: String,
    #[serde(default)]
    pub username: Option<String>,
    /// Pomelo global display name (new username system, 2023+).
    #[serde(default)]
    pub global_name: Option<String>,
    /// Legacy four-digit discriminator (`"0"` for migrated accounts).
    #[serde(default)]
    pub discriminator: Option<String>,
    #[serde(default)]
    pub bot: bool,
}

/// Minimal representation of a Discord attachment object.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageAttachment {
    /// CDN URL for the attachment.
    pub url: String,
    /// MIME type reported by Discord, if any.
    #[serde(default)]
    pub content_type: Option<String>,
}

/// Minimal mention entry — only the stable user id is needed.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageMention {
    pub id: String,
}

/// Inlined replied-to message — only the author id is needed for bot
/// detection (`is_self` / `mention_kind`).
#[derive(Debug, Clone, Deserialize)]
pub struct ReferencedMessage {
    pub id: String,
    #[serde(default)]
    pub author: Option<MessageAuthor>,
}

/// Lightweight reply cross-reference carried on the replying message.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageReference {
    #[serde(default)]
    pub message_id: Option<String>,
}

/// Thread object embedded in MESSAGE_CREATE when the message is posted
/// inside a thread. Only the id is used (as `thread_id`).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageThread {
    pub id: String,
}

// -----------------------------------------------------------------------------
// IDENTIFY (sent by client)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct IdentifyPayload<'a> {
    op: u64,
    d: IdentifyData<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct IdentifyData<'a> {
    token: &'a str,
    intents: u64,
    properties: IdentifyProperties<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct IdentifyProperties<'a> {
    os: &'a str,
    browser: &'a str,
    device: &'a str,
}

// -----------------------------------------------------------------------------
// RESUME (sent by client)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct ResumePayload<'a> {
    op: u64,
    d: ResumeData<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct ResumeData<'a> {
    token: &'a str,
    session_id: &'a str,
    /// Last dispatch sequence number we processed. Discord replays every
    /// event after this seq.
    seq: i64,
}

#[derive(Debug, Clone, Serialize)]
struct HeartbeatPayload {
    op: u64,
    /// Discord wants `null` when no seq has been seen yet.
    d: Option<i64>,
}

pub(crate) fn identify_frame(token: &str, intents: u64) -> String {
    serde_json::to_string(&IdentifyPayload {
        op: OP_IDENTIFY,
        d: IdentifyData {
            token,
            intents,
            properties: IdentifyProperties {
                os: std::env::consts::OS,
                browser: "wayland-core",
                device: "wayland-core",
            },
        },
    })
    .expect("IdentifyPayload always serialises")
}

pub(crate) fn heartbeat_frame(seq: Option<i64>) -> String {
    serde_json::to_string(&HeartbeatPayload {
        op: OP_HEARTBEAT,
        d: seq,
    })
    .expect("HeartbeatPayload always serialises")
}

/// Build a RESUME (op 6) frame echoing the session handle and the last
/// dispatch sequence so Discord replays everything buffered since `seq`.
pub(crate) fn resume_frame(token: &str, session_id: &str, seq: i64) -> String {
    serde_json::to_string(&ResumePayload {
        op: OP_RESUME,
        d: ResumeData {
            token,
            session_id,
            seq,
        },
    })
    .expect("ResumePayload always serialises")
}

// =============================================================================
// Pure parsing + mapping (unit-testable without IO)
// =============================================================================

/// Decode the outer envelope. Returns `None` if the JSON is malformed
/// (callers log + treat as a soft failure).
pub(crate) fn parse_payload(text: &str) -> Option<GatewayPayload> {
    serde_json::from_str(text).ok()
}

/// Pull `heartbeat_interval` out of a HELLO payload.
pub(crate) fn parse_hello(payload: &GatewayPayload) -> Option<HelloData> {
    if payload.op != OP_HELLO {
        return None;
    }
    serde_json::from_value(payload.d.clone()).ok()
}

/// Decode the `d` of a `op=0 t="MESSAGE_CREATE"` payload.
pub(crate) fn parse_message_create(payload: &GatewayPayload) -> Option<MessageCreate> {
    if payload.op != OP_DISPATCH || payload.t.as_deref() != Some("MESSAGE_CREATE") {
        return None;
    }
    serde_json::from_value(payload.d.clone()).ok()
}

/// Decode the `d` of a `op=0 t="READY"` dispatch into the session handle
/// and resume host. `None` for any other frame.
pub(crate) fn parse_ready(payload: &GatewayPayload) -> Option<ReadyData> {
    if payload.op != OP_DISPATCH || payload.t.as_deref() != Some("READY") {
        return None;
    }
    serde_json::from_value(payload.d.clone()).ok()
}

/// True when the payload is `op=0 t="RESUMED"` — Discord's signal that a
/// RESUME succeeded and the buffered events have been (or are being)
/// replayed as normal dispatches.
pub(crate) fn is_resumed(payload: &GatewayPayload) -> bool {
    payload.op == OP_DISPATCH && payload.t.as_deref() == Some("RESUMED")
}

/// Interpret the `d` of an `op=9 Invalid Session` frame. Discord encodes
/// resumability as a bare boolean: `true` → the session can still be
/// resumed (retry RESUME), `false` → it is gone (must re-IDENTIFY).
/// Defaults to non-resumable when `d` is malformed.
pub(crate) fn invalid_session_resumable(payload: &GatewayPayload) -> bool {
    payload.d.as_bool().unwrap_or(false)
}

/// Translate a `MESSAGE_CREATE` payload into an `IncomingMessage`
/// (filtered by the allow-list).
///
/// Returns `None` if the message is from a bot account (we don't echo
/// our own messages back through `poll_events`) or if the channel ID is
/// not in `allowed_channel_ids` (when non-empty).
///
/// `bot_id` — the stable user id of the receiving bot (from the READY
/// event). When `Some`, `is_self` and mention detection are precise.
/// When `None`, both default conservatively to `false`.
pub(crate) fn map_message_create(
    msg: MessageCreate,
    allowed_channel_ids: &HashSet<String>,
    bot_id: Option<&str>,
) -> Option<IncomingMessage> {
    if !allowed_channel_ids.is_empty() && !allowed_channel_ids.contains(&msg.channel_id) {
        return None;
    }
    let author_is_bot = msg.author.as_ref().is_some_and(|a| a.bot);
    if author_is_bot {
        return None;
    }

    // ---- Sender identity ------------------------------------------------
    let sender_id = msg
        .author
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "unknown".to_string());

    // global_name is the preferred display name (Pomelo); fall back to
    // username, then the raw id.
    let sender_display = msg
        .author
        .as_ref()
        .and_then(|a| a.global_name.clone().or_else(|| a.username.clone()));

    // @username#discriminator — omit discriminator when "0" (migrated acct)
    // or absent (same thing in the new system).
    let sender_handle = msg.author.as_ref().and_then(|a| {
        let uname = a.username.as_deref()?;
        let disc = a.discriminator.as_deref().unwrap_or("0");
        if disc == "0" || disc.is_empty() {
            Some(uname.to_string())
        } else {
            Some(format!("{uname}#{disc}"))
        }
    });

    // Human-facing author label: global_name > username > id.
    let author_str = sender_display
        .clone()
        .unwrap_or_else(|| sender_id.clone());

    let is_self = bot_id.is_some_and(|bid| bid == sender_id);

    let ts_secs = msg
        .timestamp
        .as_deref()
        .map(crate::rest::parse_iso8601_to_epoch)
        .unwrap_or(0);

    // ---- Chat type -------------------------------------------------------
    // guild_id present → guild text channel (Channel).
    // guild_id absent  → DM or group DM.
    //   Discord MESSAGE_CREATE does not include `channel_type` directly,
    //   so we cannot distinguish 1:1 DM (type=1) from group DM (type=3)
    //   without a separate channel fetch. We default absent-guild to Direct;
    //   group DMs are uncommon for bots and the distinction is low-risk here.
    let chat_type = if msg.guild_id.is_some() {
        ChatType::Channel
    } else {
        ChatType::Direct
    };

    // ---- Thread context --------------------------------------------------
    let thread_id = msg.thread.as_ref().map(|t| t.id.clone());

    // ---- Attachments -----------------------------------------------------
    let attachments: Vec<Attachment> = msg
        .attachments
        .into_iter()
        .map(|a| {
            let kind = media_kind_from_content_type(a.content_type.as_deref());
            Attachment {
                url: a.url,
                content_type: a.content_type,
                kind,
                ..Default::default()
            }
        })
        .collect();

    // ---- Mention / addressing --------------------------------------------
    // 1. Native @mention: bot's id appears in the `mentions` array.
    // 2. ReplyToBot: the inlined `referenced_message` was authored by the bot.
    let (was_mentioned, mention_kind) = if let Some(bid) = bot_id {
        let native = msg.mentions.iter().any(|m| m.id == bid);
        let reply_to_bot = msg
            .referenced_message
            .as_deref()
            .and_then(|r| r.author.as_ref())
            .is_some_and(|a| a.id == bid);
        match (native, reply_to_bot) {
            (true, _) => (true, Some(MentionKind::Native)),
            (false, true) => (true, Some(MentionKind::ReplyToBot)),
            _ => (false, None),
        }
    } else {
        (false, None)
    };

    // ---- Reply / quote ---------------------------------------------------
    // Prefer the richer inlined object; fall back to the lightweight ref.
    let reply_to_message_id = msg
        .referenced_message
        .as_deref()
        .map(|r| r.id.clone())
        .or_else(|| {
            msg.message_reference
                .as_ref()
                .and_then(|r| r.message_id.clone())
        });
    // referenced_message body is not captured in the struct (would bloat the
    // payload deserialization for limited value). A future REST-enrichment
    // pass can fill this in; leave None for now.
    let reply_to_text: Option<String> = None;

    Some(IncomingMessage {
        id: msg.id,
        conversation_id: msg.channel_id.clone(),
        author: author_str,
        text: msg.content,
        ts_secs,
        attachments,
        sender_id,
        sender_display,
        sender_handle,
        sender_alt_id: None,
        is_bot: false, // already filtered above — only non-bot messages reach here
        is_self,
        chat_type,
        chat_name: None,    // not in MESSAGE_CREATE; requires channel GET
        space_id: msg.guild_id,
        thread_id,
        parent_chat_id: None, // thread's parent channel not in this payload
        account_id: None,     // single-account connector; not tracked
        platform: Some("discord".into()),
        was_mentioned,
        mention_kind,
        reply_to_message_id,
        reply_to_text,
    })
}

/// Coarsely classify a MIME type string into a [`MediaKind`].
fn media_kind_from_content_type(ct: Option<&str>) -> MediaKind {
    match ct {
        Some(s) if s.starts_with("image/") => MediaKind::Image,
        Some(s) if s.starts_with("video/") => MediaKind::Video,
        Some(s) if s.starts_with("audio/") => MediaKind::Audio,
        Some(s)
            if s.starts_with("application/") || s.starts_with("text/") =>
        {
            MediaKind::Document
        }
        _ => MediaKind::Other,
    }
}

// -----------------------------------------------------------------------------
// Heartbeat state machine
// -----------------------------------------------------------------------------

/// Tracks the heartbeat / heartbeat-ack lifecycle. Pure — the WebSocket
/// driver pokes it on each heartbeat sent and each ack received; calls
/// `is_dead()` after each interval tick to decide whether to reconnect.
#[derive(Debug, Clone)]
pub(crate) struct HeartbeatTracker {
    /// `Some(instant)` if a heartbeat has been sent and no ack has
    /// arrived yet. `None` after an ack (or before the first beat).
    awaiting_ack_since: Option<Instant>,
    /// Grace window beyond the heartbeat interval before we consider
    /// the connection dead.
    grace: Duration,
    /// Heartbeat interval from the HELLO frame. Used by `is_dead` to
    /// compute the deadline: `interval + grace`.
    interval: Duration,
}

impl HeartbeatTracker {
    pub(crate) fn new(interval_ms: u64, grace_ms: u64) -> Self {
        Self {
            awaiting_ack_since: None,
            grace: Duration::from_millis(grace_ms),
            interval: Duration::from_millis(interval_ms),
        }
    }

    /// Called when the driver sends a HEARTBEAT frame.
    pub(crate) fn on_send(&mut self, now: Instant) {
        // Only set if not already waiting (an unack'd previous heartbeat
        // is what makes us "dead" — the next-send timestamp doesn't reset
        // that condition).
        if self.awaiting_ack_since.is_none() {
            self.awaiting_ack_since = Some(now);
        }
    }

    /// Called when a HEARTBEAT_ACK arrives.
    pub(crate) fn on_ack(&mut self) {
        self.awaiting_ack_since = None;
    }

    /// True if a heartbeat was sent and no ack has arrived within the
    /// configured grace window. Stays "dead" until reset by `on_ack`.
    pub(crate) fn is_dead(&self, now: Instant) -> bool {
        match self.awaiting_ack_since {
            Some(sent) => now.duration_since(sent) > self.interval + self.grace,
            None => false,
        }
    }
}

// -----------------------------------------------------------------------------
// Resume state + handshake decision (pure, unit-testable)
// -----------------------------------------------------------------------------

/// Everything needed to RESUME a prior session. Populated from READY and
/// kept across reconnects so the outer loop can replay instead of
/// re-identifying. Cleared whenever the session becomes non-resumable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResumeState {
    /// Opaque session id from READY (echoed in the RESUME payload).
    pub session_id: String,
    /// Dedicated resume host from READY, if Discord supplied one. When
    /// `None` the caller resumes against the normal gateway URL.
    pub resume_gateway_url: Option<String>,
    /// Last dispatch sequence number we have processed.
    pub seq: i64,
}

/// Which handshake a (re)connect should perform after HELLO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Handshake {
    /// Fresh login: clear any session and send IDENTIFY.
    Identify,
    /// Replay: send RESUME with the carried session + seq.
    Resume,
}

/// Why a session ended, as far as the resume decision cares. Lets the
/// outer loop pick RESUME-vs-IDENTIFY without re-deriving the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReconnectReason {
    /// op 7 Reconnect, or the socket dropped / heartbeat lapsed — the
    /// session itself is still valid, so resume if we have state.
    Resumable,
    /// op 9 Invalid Session with `d == true` — Discord says retry RESUME.
    InvalidSessionResumable,
    /// op 9 Invalid Session with `d == false` — session is gone.
    InvalidSessionFatal,
}

/// Decide the next handshake given the carried session state and why the
/// last session ended. Pure so the policy is unit-testable in isolation
/// from the socket.
///
/// - We can only RESUME when we actually hold a `session_id`.
/// - A fatal Invalid Session forces a fresh IDENTIFY regardless of state.
/// - Everything else resumes when state is present, else IDENTIFYs.
pub(crate) fn decide_handshake(
    have_session: bool,
    reason: &ReconnectReason,
) -> Handshake {
    match reason {
        ReconnectReason::InvalidSessionFatal => Handshake::Identify,
        ReconnectReason::Resumable | ReconnectReason::InvalidSessionResumable => {
            if have_session {
                Handshake::Resume
            } else {
                Handshake::Identify
            }
        }
    }
}

// =============================================================================
// Gateway driver
// =============================================================================

/// Arguments for the gateway loop spawned by `DiscordChannel::start`.
pub(crate) struct GatewayArgs {
    pub gateway_url: String,
    pub bot_token: String,
    pub intents: u64,
    pub heartbeat_grace_ms: u64,
    pub allowed_channel_ids: HashSet<String>,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub shutdown: watch::Receiver<bool>,
    /// Stable user id of this bot (from Discord READY). Used for
    /// `is_self` detection and `was_mentioned` classification.
    /// `None` until the connector resolves it (e.g. via `/users/@me`).
    pub bot_id: Option<String>,
}

/// Append `?v=10&encoding=json` to a bare gateway host if it lacks a
/// query string. Idempotent — a URL that already carries query params is
/// returned untouched.
fn with_gateway_query(base: &str) -> String {
    if base.contains('?') {
        base.to_string()
    } else {
        format!("{base}?v=10&encoding=json")
    }
}

/// Drive one or more gateway connection cycles until shutdown is
/// signalled. On op=7 / dropped socket / heartbeat-timeout we RESUME
/// (replaying buffered events); on a fatal op=9 we fall back to a fresh
/// IDENTIFY. Reconnects use a short backoff.
pub(crate) async fn gateway_loop(args: GatewayArgs) {
    let GatewayArgs {
        gateway_url,
        bot_token,
        intents,
        heartbeat_grace_ms,
        allowed_channel_ids,
        inbox,
        mut shutdown,
        bot_id,
    } = args;

    // Normalised default gateway URL used for fresh IDENTIFYs and as the
    // fallback when no resume host is known.
    let identify_url = with_gateway_query(&gateway_url);

    let mut backoff_ms: u64 = 1_000;
    // Carried session handle. `Some` once READY lands; cleared on a fatal
    // Invalid Session so the next cycle re-IDENTIFYs from scratch.
    let mut resume: Option<ResumeState> = None;
    // How the *previous* session ended. The very first cycle has no prior
    // session, so it IDENTIFYs (no resume state anyway).
    let mut reason = ReconnectReason::Resumable;

    loop {
        if *shutdown.borrow() {
            break;
        }

        // Pick the handshake + connect URL for this cycle.
        let handshake = decide_handshake(resume.is_some(), &reason);
        let url = match (&handshake, &resume) {
            (Handshake::Resume, Some(state)) => state
                .resume_gateway_url
                .clone()
                .map(|u| with_gateway_query(&u))
                .unwrap_or_else(|| identify_url.clone()),
            _ => identify_url.clone(),
        };
        // A fresh IDENTIFY means any stale session is meaningless.
        if handshake == Handshake::Identify {
            resume = None;
        }

        match run_one_session(
            &url,
            &bot_token,
            intents,
            heartbeat_grace_ms,
            &allowed_channel_ids,
            bot_id.as_deref(),
            &inbox,
            &mut shutdown,
            &handshake,
            &mut resume,
        )
        .await
        {
            Ok(SessionExit::Shutdown) => break,
            Ok(SessionExit::Reconnect(next_reason)) => {
                // Surface Reconnecting so the manager/UI sees the gap.
                inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Reconnecting,
                    });
                // op 9 may demand a fresh IDENTIFY after a random 1–5s
                // delay; clear the session and wait before re-entering.
                if matches!(next_reason, ReconnectReason::InvalidSessionFatal) {
                    resume = None;
                    let delay = invalid_session_backoff();
                    let sleep = tokio::time::sleep(delay);
                    tokio::pin!(sleep);
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => { if *shutdown.borrow() { break; } }
                        _ = &mut sleep => {}
                    }
                }
                reason = next_reason;
                backoff_ms = 1_000;
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_discord::gateway",
                    error = %e,
                    backoff_ms,
                    resumable = resume.is_some(),
                    "gateway session ended; backing off before reconnect"
                );
                inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Reconnecting,
                    });
                // A dropped socket / heartbeat lapse keeps the session
                // valid — resume if we have state.
                reason = ReconnectReason::Resumable;
                // Bounded exponential backoff. Race against shutdown so
                // stop() isn't blocked by the sleep.
                let sleep = tokio::time::sleep(Duration::from_millis(backoff_ms));
                tokio::pin!(sleep);
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                    _ = &mut sleep => {}
                }
                backoff_ms = (backoff_ms.saturating_mul(2)).min(30_000);
            }
        }
    }
}

/// Discord requires a random 1–5s wait before re-IDENTIFYing after a
/// fatal Invalid Session. Uses a cheap time-seeded pick — no extra RNG
/// dependency, and the exact value is non-critical.
fn invalid_session_backoff() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Map into the inclusive 1000..=5000 ms window.
    let ms = 1_000 + u64::from(nanos % 4_001);
    Duration::from_millis(ms)
}

enum SessionExit {
    /// `shutdown` watch flipped — exit the outer loop.
    Shutdown,
    /// Reconnect requested. The carried reason tells the outer loop
    /// whether to RESUME or fall back to a fresh IDENTIFY.
    Reconnect(ReconnectReason),
}

// One Gateway session carries many independent connection parameters;
// grouping them into a struct would add indirection without clarity.
#[allow(clippy::too_many_arguments)]
async fn run_one_session(
    url: &str,
    bot_token: &str,
    intents: u64,
    heartbeat_grace_ms: u64,
    allowed_channel_ids: &HashSet<String>,
    bot_id: Option<&str>,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    shutdown: &mut watch::Receiver<bool>,
    handshake: &Handshake,
    // Shared session handle: updated in place as READY / dispatches
    // arrive so the carried seq + session_id survive even when this
    // function returns via Err (dropped socket).
    resume: &mut Option<ResumeState>,
) -> Result<SessionExit, String> {
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // Wait for HELLO.
    let hello = loop {
        let frame = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return Ok(SessionExit::Shutdown); }
                continue;
            }
            f = stream.next() => f,
        };
        let frame = frame.ok_or_else(|| "stream closed before HELLO".to_string())?;
        let frame = frame.map_err(|e| format!("ws read before HELLO: {e}"))?;
        let text = match frame {
            WsMessage::Text(t) => t,
            WsMessage::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
            WsMessage::Close(_) => return Err("close frame before HELLO".to_string()),
            _ => continue,
        };
        let Some(payload) = parse_payload(&text) else {
            continue;
        };
        if let Some(hello) = parse_hello(&payload) {
            break hello;
        }
    };

    let interval_ms = hello.heartbeat_interval;
    let mut tracker = HeartbeatTracker::new(interval_ms, heartbeat_grace_ms);
    // On a RESUME we continue heartbeating from the carried seq; on a
    // fresh IDENTIFY there is nothing to acknowledge yet.
    let mut last_seq: Option<i64> = match handshake {
        Handshake::Resume => resume.as_ref().map(|s| s.seq),
        Handshake::Identify => None,
    };

    // Send RESUME or IDENTIFY per the handshake decision.
    match handshake {
        Handshake::Resume => {
            let state = resume
                .as_ref()
                .ok_or_else(|| "resume requested without session state".to_string())?;
            sink.send(WsMessage::Text(resume_frame(
                bot_token,
                &state.session_id,
                state.seq,
            )))
            .await
            .map_err(|e| format!("resume send: {e}"))?;
            tracing::debug!(
                target: "wcore_channel_discord::gateway",
                session_id = %state.session_id,
                seq = state.seq,
                "sent RESUME"
            );
        }
        Handshake::Identify => {
            sink.send(WsMessage::Text(identify_frame(bot_token, intents)))
                .await
                .map_err(|e| format!("identify send: {e}"))?;
            tracing::debug!(
                target: "wcore_channel_discord::gateway",
                "sent IDENTIFY"
            );
        }
    }

    // Push Connected once we've handed the handshake off; READY / RESUMED
    // landing is the formal "live" moment but for routing it's close
    // enough — the manager dedupes state-changes anyway.
    inbox
        .lock()
        .await
        .push_back(ChannelEvent::ConnectionStateChanged {
            state: ConnectionState::Connected,
        });

    let mut heartbeat_timer = tokio::time::interval(Duration::from_millis(interval_ms));
    // Skip the immediate tick — Discord wants the first heartbeat
    // delayed by `jitter * interval`. We use a constant 0.5 because
    // it's deterministic and well within Discord's expectation.
    heartbeat_timer.tick().await;

    loop {
        tokio::select! {
            biased;

            // 1. Shutdown.
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return Ok(SessionExit::Shutdown); }
            }

            // 2. Heartbeat timer fires.
            _ = heartbeat_timer.tick() => {
                let now = Instant::now();
                if tracker.is_dead(now) {
                    return Err("heartbeat ack missing past grace window".to_string());
                }
                sink.send(WsMessage::Text(heartbeat_frame(last_seq)))
                    .await
                    .map_err(|e| format!("heartbeat send: {e}"))?;
                tracker.on_send(now);
            }

            // 3. Inbound frame.
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Err("ws stream ended".to_string());
                };
                let frame = frame.map_err(|e| format!("ws read: {e}"))?;
                let text = match frame {
                    WsMessage::Text(t) => t,
                    WsMessage::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
                    WsMessage::Ping(p) => {
                        // Reply to TCP-level pings; tungstenite handles
                        // protocol-level ones for us but be safe.
                        let _ = sink.send(WsMessage::Pong(p)).await;
                        continue;
                    }
                    WsMessage::Close(_) => return Err("close frame".to_string()),
                    _ => continue,
                };

                let Some(payload) = parse_payload(&text) else { continue };
                if let Some(s) = payload.s {
                    last_seq = Some(s);
                    // Keep the carried session's seq current so a later
                    // RESUME asks Discord to replay from the right point —
                    // even if this session ends via a dropped socket.
                    if let Some(state) = resume.as_mut() {
                        state.seq = s;
                    }
                }

                match payload.op {
                    OP_HEARTBEAT_ACK => {
                        tracker.on_ack();
                    }
                    OP_HEARTBEAT => {
                        // Server asked us to send a heartbeat now.
                        sink.send(WsMessage::Text(heartbeat_frame(last_seq)))
                            .await
                            .map_err(|e| format!("heartbeat send: {e}"))?;
                        tracker.on_send(Instant::now());
                    }
                    OP_RECONNECT => {
                        // 7: Discord asks us to reconnect. The session is
                        // still valid — RESUME (if we have state) on the
                        // next cycle to replay buffered events.
                        return Ok(SessionExit::Reconnect(ReconnectReason::Resumable));
                    }
                    OP_INVALID_SESSION => {
                        // 9: session invalidated. `d == true` means the
                        // session can still be resumed; `d == false` means
                        // it's gone and we must re-IDENTIFY after a 1-5s
                        // wait (handled by the outer loop). Note: op 9 can
                        // also arrive right after IDENTIFY (always
                        // non-resumable in that case), which this same
                        // branch handles correctly.
                        let reason = if invalid_session_resumable(&payload) {
                            ReconnectReason::InvalidSessionResumable
                        } else {
                            ReconnectReason::InvalidSessionFatal
                        };
                        return Ok(SessionExit::Reconnect(reason));
                    }
                    OP_DISPATCH => {
                        // Capture the session handle from READY so future
                        // reconnects can RESUME instead of re-IDENTIFY.
                        if let Some(ready) = parse_ready(&payload) {
                            *resume = Some(ResumeState {
                                session_id: ready.session_id,
                                resume_gateway_url: ready.resume_gateway_url,
                                seq: last_seq.unwrap_or(0),
                            });
                            tracing::debug!(
                                target: "wcore_channel_discord::gateway",
                                "READY received; session captured for resume"
                            );
                        } else if is_resumed(&payload) {
                            // RESUME succeeded: buffered events follow as
                            // normal MESSAGE_CREATE dispatches through the
                            // mapping below — nothing else to do here.
                            tracing::debug!(
                                target: "wcore_channel_discord::gateway",
                                "RESUMED received; replayed events will flow as dispatches"
                            );
                        }
                        if let Some(mc) = parse_message_create(&payload)
                            && let Some(im) =
                                map_message_create(mc, allowed_channel_ids, bot_id)
                        {
                            inbox
                                .lock()
                                .await
                                .push_back(ChannelEvent::MessageReceived { msg: im });
                        }
                        // Other DISPATCH events (GUILD_CREATE, …) are not
                        // surfaced.
                    }
                    other => {
                        tracing::trace!(
                            target: "wcore_channel_discord::gateway",
                            op = other,
                            "ignoring unhandled gateway opcode"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_parsing_extracts_heartbeat_interval() {
        let raw = r#"{"op":10,"d":{"heartbeat_interval":41250}}"#;
        let payload = parse_payload(raw).expect("payload parses");
        assert_eq!(payload.op, OP_HELLO);
        let hello = parse_hello(&payload).expect("hello parses");
        assert_eq!(hello.heartbeat_interval, 41_250);
    }

    #[test]
    fn message_create_maps_to_incoming_message() {
        let raw = r#"{
            "op":0,
            "t":"MESSAGE_CREATE",
            "s":42,
            "d":{
                "id":"123456789",
                "channel_id":"55555",
                "content":"hello there",
                "timestamp":"2024-01-02T03:04:05+00:00",
                "author":{
                    "id":"9001",
                    "username":"alice",
                    "bot":false
                }
            }
        }"#;
        let payload = parse_payload(raw).expect("payload parses");
        assert_eq!(payload.s, Some(42));
        let mc = parse_message_create(&payload).expect("message_create parses");
        let allowed = HashSet::new();
        let im = map_message_create(mc, &allowed, None).expect("mapper produces an event");
        assert_eq!(im.id, "123456789");
        assert_eq!(im.conversation_id, "55555");
        assert_eq!(im.author, "alice");
        assert_eq!(im.text, "hello there");
        // 2024-01-02T03:04:05Z = 1704164645
        assert_eq!(im.ts_secs, 1_704_164_645);
    }

    #[test]
    fn message_create_drops_bot_messages() {
        let raw = r#"{
            "op":0,"t":"MESSAGE_CREATE","s":1,
            "d":{"id":"1","channel_id":"2","content":"x","timestamp":null,
                 "author":{"id":"3","username":"botbot","bot":true}}
        }"#;
        let payload = parse_payload(raw).unwrap();
        let mc = parse_message_create(&payload).unwrap();
        let allowed = HashSet::new();
        assert!(
            map_message_create(mc, &allowed, None).is_none(),
            "bot messages should be dropped"
        );
    }

    #[test]
    fn message_create_respects_allow_list() {
        let raw = r#"{
            "op":0,"t":"MESSAGE_CREATE","s":1,
            "d":{"id":"1","channel_id":"WRONG","content":"x","timestamp":null,
                 "author":{"id":"3","username":"alice","bot":false}}
        }"#;
        let payload = parse_payload(raw).unwrap();
        let mc = parse_message_create(&payload).unwrap();
        let mut allowed = HashSet::new();
        allowed.insert("ALLOWED".to_string());
        assert!(
            map_message_create(mc, &allowed, None).is_none(),
            "channel_id outside allow-list should be dropped"
        );
    }

    #[test]
    fn heartbeat_tracker_flags_dead_after_grace() {
        let mut t = HeartbeatTracker::new(1_000, 200);
        let now = Instant::now();
        assert!(!t.is_dead(now), "fresh tracker is alive");

        // First heartbeat sent at t0.
        t.on_send(now);

        // At interval+grace boundary, still alive.
        let boundary = now + Duration::from_millis(1_000 + 200);
        assert!(!t.is_dead(boundary), "exactly at interval+grace is alive");

        // Past the grace window — dead.
        let past_grace = now + Duration::from_millis(1_000 + 200 + 1);
        assert!(t.is_dead(past_grace), "past interval+grace should be dead");

        // ACK clears it.
        t.on_ack();
        assert!(!t.is_dead(past_grace), "ack should clear the dead flag");
    }

    #[test]
    fn heartbeat_tracker_two_sends_without_ack_is_dead() {
        // Simulates "we sent two heartbeats and never saw an ack" —
        // the per-task spec test #7.
        let mut t = HeartbeatTracker::new(1_000, 500);
        let now = Instant::now();

        // Beat 1.
        t.on_send(now);
        // Beat 2, one interval later — still no ack arrived.
        let beat2 = now + Duration::from_millis(1_000);
        t.on_send(beat2);

        // After interval+grace from the FIRST unack'd beat, dead.
        let past = now + Duration::from_millis(1_000 + 500 + 1);
        assert!(
            t.is_dead(past),
            "two heartbeats without an ack should flag dead"
        );
    }

    #[test]
    fn identify_frame_includes_token_and_intents() {
        let raw = identify_frame("BOT-TOKEN", crate::config::DEFAULT_INTENTS);
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["op"], 2);
        assert_eq!(v["d"]["token"], "BOT-TOKEN");
        assert_eq!(v["d"]["intents"], crate::config::DEFAULT_INTENTS);
        assert!(v["d"]["properties"]["browser"].is_string());
    }

    #[test]
    fn heartbeat_frame_carries_seq() {
        let with_seq = heartbeat_frame(Some(7));
        let null_seq = heartbeat_frame(None);
        let v1: serde_json::Value = serde_json::from_str(&with_seq).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&null_seq).unwrap();
        assert_eq!(v1["op"], 1);
        assert_eq!(v1["d"], 7);
        assert_eq!(v2["op"], 1);
        assert!(v2["d"].is_null());
    }

    // ---- RESUME support -------------------------------------------------

    #[test]
    fn resume_frame_carries_op6_token_session_and_seq() {
        let raw = resume_frame("BOT-TOKEN", "sess-abc", 99);
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["op"], 6, "RESUME is opcode 6");
        assert_eq!(v["d"]["token"], "BOT-TOKEN");
        assert_eq!(v["d"]["session_id"], "sess-abc");
        assert_eq!(v["d"]["seq"], 99);
    }

    #[test]
    fn ready_parses_session_id_and_resume_url() {
        let raw = r#"{
            "op":0,"t":"READY","s":1,
            "d":{
                "session_id":"abc123",
                "resume_gateway_url":"wss://resume.example.gg",
                "user":{"id":"42"}
            }
        }"#;
        let payload = parse_payload(raw).unwrap();
        let ready = parse_ready(&payload).expect("READY parses");
        assert_eq!(ready.session_id, "abc123");
        assert_eq!(
            ready.resume_gateway_url.as_deref(),
            Some("wss://resume.example.gg")
        );
    }

    #[test]
    fn ready_tolerates_missing_resume_url() {
        let raw = r#"{"op":0,"t":"READY","s":1,"d":{"session_id":"x"}}"#;
        let payload = parse_payload(raw).unwrap();
        let ready = parse_ready(&payload).expect("READY parses");
        assert_eq!(ready.session_id, "x");
        assert!(ready.resume_gateway_url.is_none());
    }

    #[test]
    fn parse_ready_rejects_non_ready_dispatch() {
        let raw = r#"{"op":0,"t":"MESSAGE_CREATE","s":1,"d":{"session_id":"x"}}"#;
        let payload = parse_payload(raw).unwrap();
        assert!(parse_ready(&payload).is_none());
    }

    #[test]
    fn resumed_dispatch_detected() {
        let resumed = parse_payload(r#"{"op":0,"t":"RESUMED","s":5,"d":{}}"#).unwrap();
        assert!(is_resumed(&resumed));
        let other = parse_payload(r#"{"op":0,"t":"READY","s":5,"d":{}}"#).unwrap();
        assert!(!is_resumed(&other));
    }

    #[test]
    fn invalid_session_resumability_reads_d_boolean() {
        let yes = parse_payload(r#"{"op":9,"d":true}"#).unwrap();
        let no = parse_payload(r#"{"op":9,"d":false}"#).unwrap();
        // Discord may also send op 9 with a missing/null d (post-IDENTIFY)
        // — treat that conservatively as non-resumable.
        let bare = parse_payload(r#"{"op":9}"#).unwrap();
        assert!(invalid_session_resumable(&yes));
        assert!(!invalid_session_resumable(&no));
        assert!(!invalid_session_resumable(&bare));
    }

    #[test]
    fn decide_handshake_resumes_when_session_and_reason_allow() {
        // op 7 / dropped socket with a live session → RESUME.
        assert_eq!(
            decide_handshake(true, &ReconnectReason::Resumable),
            Handshake::Resume
        );
        // op 9 resumable=true with a live session → RESUME.
        assert_eq!(
            decide_handshake(true, &ReconnectReason::InvalidSessionResumable),
            Handshake::Resume
        );
    }

    #[test]
    fn decide_handshake_identifies_without_session() {
        // No session captured yet (e.g. very first connect, or dropped
        // before READY) → IDENTIFY regardless of a resumable reason.
        assert_eq!(
            decide_handshake(false, &ReconnectReason::Resumable),
            Handshake::Identify
        );
        assert_eq!(
            decide_handshake(false, &ReconnectReason::InvalidSessionResumable),
            Handshake::Identify
        );
    }

    #[test]
    fn decide_handshake_fatal_invalid_session_forces_identify() {
        // op 9 with d == false → fresh IDENTIFY even if we still hold a
        // session id.
        assert_eq!(
            decide_handshake(true, &ReconnectReason::InvalidSessionFatal),
            Handshake::Identify
        );
        assert_eq!(
            decide_handshake(false, &ReconnectReason::InvalidSessionFatal),
            Handshake::Identify
        );
    }

    #[test]
    fn seq_tracking_advances_carried_resume_state() {
        // Mirrors the driver's inbound-frame seq update: every dispatch
        // with an `s` bumps the carried session's seq so a later RESUME
        // replays from the correct point.
        let mut resume = Some(ResumeState {
            session_id: "s1".to_string(),
            resume_gateway_url: None,
            seq: 0,
        });

        for raw in [
            r#"{"op":0,"t":"MESSAGE_CREATE","s":10,"d":{}}"#,
            r#"{"op":11}"#, // HEARTBEAT_ACK: no `s`, seq unchanged.
            r#"{"op":0,"t":"MESSAGE_CREATE","s":11,"d":{}}"#,
        ] {
            let payload = parse_payload(raw).unwrap();
            if let Some(s) = payload.s
                && let Some(state) = resume.as_mut()
            {
                state.seq = s;
            }
        }

        assert_eq!(resume.unwrap().seq, 11, "seq tracks the latest dispatch s");
    }

    #[test]
    fn invalid_session_backoff_in_discord_window() {
        // Discord mandates a random 1–5s wait before re-IDENTIFY.
        let d = invalid_session_backoff();
        assert!(
            d >= Duration::from_millis(1_000) && d <= Duration::from_millis(5_000),
            "backoff {d:?} must sit in the 1–5s window"
        );
    }

    #[test]
    fn with_gateway_query_is_idempotent() {
        assert_eq!(
            with_gateway_query("wss://gateway.discord.gg"),
            "wss://gateway.discord.gg?v=10&encoding=json"
        );
        // Already carries query params → untouched.
        assert_eq!(
            with_gateway_query("wss://resume.gg?v=10&encoding=json"),
            "wss://resume.gg?v=10&encoding=json"
        );
    }
}
