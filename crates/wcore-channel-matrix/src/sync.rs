//! Background `/sync` long-poll task. Spawned by `MatrixChannel::start()`,
//! signaled to exit by the watch channel held in `MatrixChannel`.
//!
//! Mirrors the Telegram `getUpdates` long-poll: a loop that races each API
//! call against a shutdown signal, backs off on transient failure, and pushes
//! decoded `MessageReceived` events into the shared inbox.
//!
//! **Initial-sync replay guard**: the first `/sync` (no `since` token) returns
//! the full current room state plus a `next_batch` cursor. We store that cursor
//! but DO NOT emit its timeline events — otherwise the bot would replay the
//! entire room backlog on every startup. Only sync responses AFTER the first
//! (once `since` is set) contribute `MessageReceived` events.
//!
//! **Cursor persistence (F24-C3-H6)**: the replay guard above is only half the
//! contract. Held in a process-local alone, `since` resets to `None` on every
//! restart, so the first sync after a restart is an initial sync and the guard
//! discards its timeline — which is precisely the window the process was down
//! for. Every message delivered during a deploy, crash or reboot was lost
//! silently: no error, no retry, no log, and a healthy-looking channel.
//! [`crate::sync_store`] persists the cursor across restarts so that a restart
//! **neither replays the backlog nor skips what arrived while we were down** —
//! the same contract `wcore-channel-email`'s IMAP UID watermark already holds,
//! and deliberately the same shape rather than a second mechanism.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Mutex, watch};

use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage, MediaKind};

use crate::error::MatrixError;
use crate::sync_store::{self, Loaded};
use crate::token::{self, Renewal, TokenSource};

/// Long-poll timeout (ms) handed to the homeserver's `/sync`. The HTTP read
/// timeout is this plus a buffer so a wedged proxy can't park us forever.
pub(crate) const SYNC_TIMEOUT_MS: u64 = 30_000;

/// Hard cap on a single `/sync` response body. The body is buffered fully to
/// parse `SyncResponse`, so without a cap a homeserver (or a wedged proxy)
/// streaming an unbounded body inside this infinite long-poll loop could OOM
/// the host. 32 MiB comfortably exceeds any legitimate `/sync` payload.
const MAX_SYNC_BODY_BYTES: usize = 32 * 1024 * 1024;

/// Max length of a homeserver error body we retain in `MatrixError::Http`.
/// Truncated so a large error payload can't bloat the error/log path.
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;

/// Constructor arguments — flatter than a struct, easier to spawn.
pub(crate) struct SyncArgs {
    pub http: wcore_egress::EgressClient,
    pub api_base: String,
    /// The live access token and the only path that renews it. Shared with
    /// `MatrixChannel`, so a renewal driven from here is immediately visible
    /// to the send path (and vice versa) rather than leaving the outbound
    /// half authenticating with a token this loop already replaced.
    pub tokens: Arc<TokenSource>,
    pub user_id: String,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub shutdown: watch::Receiver<bool>,
    /// Where this channel's `/sync` cursor is persisted across restarts.
    /// Computed from the production account identity in `MatrixChannel::start`;
    /// tests point it at a temp file so they exercise the real loop.
    pub state_path: PathBuf,
}

// ---------------------------------------------------------------------------
// /sync response model — only the slice this adapter consumes. Matrix payloads
// are large; `#[serde(default)]` keeps us tolerant of everything we ignore.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct SyncResponse {
    next_batch: String,
    #[serde(default)]
    rooms: Rooms,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Rooms {
    #[serde(default)]
    join: std::collections::HashMap<String, JoinedRoom>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct JoinedRoom {
    #[serde(default)]
    timeline: Timeline,
    #[serde(default)]
    summary: RoomSummary,
}

/// Subset of a joined room's `summary` block. Matrix reports
/// `m.joined_member_count` here; a value of `2` is the standard signal for a
/// 1:1 direct chat. (The fuller signal is the `m.direct` account-data event;
/// this count is the cheapest in-band approximation and degrades to Group when
/// the homeserver omits the summary on an incremental sync.)
#[derive(Debug, Clone, Deserialize, Default)]
struct RoomSummary {
    #[serde(rename = "m.joined_member_count", default)]
    joined_member_count: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Timeline {
    #[serde(default)]
    events: Vec<TimelineEvent>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct TimelineEvent {
    #[serde(rename = "type", default)]
    event_type: String,
    #[serde(default)]
    sender: String,
    #[serde(default)]
    event_id: String,
    /// Matrix `origin_server_ts` is milliseconds since the epoch.
    #[serde(default)]
    origin_server_ts: i64,
    #[serde(default)]
    content: MessageContent,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MessageContent {
    #[serde(default)]
    body: String,
    /// `m.mentions` rich-mention block (MSC3952). We only read `user_ids`.
    #[serde(rename = "m.mentions", default)]
    mentions: Option<Mentions>,
    /// `m.image` / `m.audio` / `m.video` / `m.file` for media events, else
    /// `m.text` / `m.notice` / etc. Empty when absent.
    #[serde(default)]
    msgtype: String,
    /// `mxc://server/id` content URI for UNENCRYPTED media. Encrypted rooms
    /// carry media under `content.file` (with a decryption key) which this
    /// raw-REST adapter does not handle (it can't read encrypted bodies
    /// either) — so only plaintext-room media is surfaced.
    #[serde(default)]
    url: Option<String>,
    /// Media `info` block — we only read `mimetype`.
    #[serde(default)]
    info: Option<MediaInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MediaInfo {
    #[serde(default)]
    mimetype: Option<String>,
}

/// Map a Matrix `msgtype` to a coarse [`MediaKind`], or `None` for non-media
/// message types (`m.text`, `m.notice`, …).
fn media_kind_for(msgtype: &str) -> Option<MediaKind> {
    match msgtype {
        "m.image" => Some(MediaKind::Image),
        "m.audio" => Some(MediaKind::Audio),
        "m.video" => Some(MediaKind::Video),
        "m.file" => Some(MediaKind::Document),
        _ => None,
    }
}

/// Build the typed attachment list for one message event. Only unencrypted
/// media (a plain `mxc://` `url`) of a recognised media msgtype is mapped;
/// everything else yields an empty list. The `mxc://` URI is carried in
/// `Attachment.url` and resolved to bytes later via the connector's
/// `fetch_media` (authenticated `/_matrix/client/v1/media/download`).
fn attachments_for(content: &MessageContent) -> Vec<Attachment> {
    let Some(kind) = media_kind_for(&content.msgtype) else {
        return Vec::new();
    };
    let Some(url) = content.url.as_deref().filter(|u| u.starts_with("mxc://")) else {
        return Vec::new();
    };
    vec![Attachment {
        url: url.to_string(),
        content_type: content.info.as_ref().and_then(|i| i.mimetype.clone()),
        kind,
        ..Default::default()
    }]
}

#[derive(Debug, Clone, Deserialize, Default)]
struct Mentions {
    #[serde(default)]
    user_ids: Vec<String>,
}

/// Drive `/sync` in a loop until the shutdown signal flips.
///
/// Backoff on transient failure is linear-capped at 30s — the same family as
/// the Telegram long-poll loop. A tight failure loop here is usually a
/// transient outage, not a coding error, so the loop is self-correcting.
pub(crate) async fn sync_loop(args: SyncArgs) {
    let SyncArgs {
        http,
        api_base,
        tokens,
        user_id,
        inbox,
        mut shutdown,
        state_path,
    } = args;

    // F24-C3-H6. Resume the cursor from disk so a restart neither replays the
    // room backlog nor skips what the homeserver accumulated while we were
    // down. `None` here means "seed from an initial sync" — whose timeline the
    // replay guard discards — so the three states are kept distinct: a corrupt
    // cursor must not read as a first run, because a first run starts from now
    // and says nothing about it.
    let mut since: Option<String> = match sync_store::load_from(&state_path) {
        Loaded::Cursor(cursor) => {
            tracing::info!(
                target: "wcore_channel_matrix::sync",
                "resumed persisted /sync cursor; messages delivered while this process was down will be served",
            );
            Some(cursor)
        }
        Loaded::Absent => {
            tracing::info!(
                target: "wcore_channel_matrix::sync",
                "no persisted /sync cursor (first start for this account); seeding from an initial sync — existing room backlog will NOT be replayed",
            );
            None
        }
        Loaded::Corrupt(reason) => {
            tracing::warn!(
                target: "wcore_channel_matrix::sync",
                reason = reason,
                path = %state_path.display(),
                "persisted /sync cursor is unusable; re-seeding from an initial sync. Messages delivered while this process was down are NOT recoverable for this start",
            );
            None
        }
    };
    // True while `since` came from disk and the homeserver has not yet accepted
    // it. A cursor the homeserver rejects would otherwise back off forever —
    // a permanent wedge on a channel that reports healthy. Cleared on the first
    // success, and on the one re-seed below, so this can happen at most once.
    let mut resumed_unverified = since.is_some();
    let mut consecutive_failures: u32 = 0;

    loop {
        if *shutdown.borrow() {
            break;
        }

        // Proactive renewal. The homeserver states `expires_in_ms` on every
        // refresh, so once this adapter has renewed once it knows the deadline
        // and replaces the token BEFORE a call fails. Without this the only
        // trigger is a 401, i.e. at least one rejected `/sync` per expiry.
        if tokens.renewal_due() {
            match tokens.renew_before_expiry().await {
                Renewal::Renewed => {}
                Renewal::Deferred(why) => {
                    tracing::warn!(
                        target: "wcore_channel_matrix::sync",
                        reason = %why,
                        "could not renew the Matrix access token ahead of expiry; continuing on the current one",
                    );
                }
                // `AuthExpired` is already published by the token source.
                Renewal::Fatal => break,
            }
        }

        // The token this iteration authenticates with. Captured before the
        // call so the 401 handler can tell "the token I presented" from "the
        // token in the store now", which is how a peer process's refresh is
        // detected without POSTing.
        let presented = tokens.access();

        // Race the next API call against a shutdown signal so we don't get
        // stuck for ~SYNC_TIMEOUT_MS after stop() flips the flag.
        let result = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
                continue;
            }
            r = sync_once(&http, &api_base, &presented, since.as_deref()) => r,
        };

        match result {
            Ok(resp) => {
                consecutive_failures = 0;
                // A served `/sync` proves the token in play is live, which
                // releases the renewal-loop guard.
                tokens.mark_progress();
                // The homeserver accepted whatever cursor we presented, so a
                // resumed one is now proven good.
                resumed_unverified = false;
                let is_initial = since.is_none();
                let next_batch = resp.next_batch.clone();
                // Only emit events once `since` is set (i.e. after the first
                // sync). The initial full-state sync is consumed for its
                // cursor only, never replayed into the inbox.
                if !is_initial {
                    let events = parse_sync_events(&resp, &user_id);
                    if !events.is_empty() {
                        let mut guard = inbox.lock().await;
                        for e in events {
                            // F9 — bounded, drop-oldest inbox against a flood.
                            wcore_channels::push_bounded(&mut guard, e);
                        }
                    }
                }
                // Advance the cursor only on a non-empty token. A spec-
                // compliant homeserver always returns a non-empty next_batch,
                // but a malformed/proxy response with `next_batch: ""` would,
                // if stored, send `?since=` next tick — which some homeservers
                // treat as an initial sync and could replay backlog. Keep the
                // prior cursor in that case.
                if !next_batch.is_empty() && since.as_deref() != Some(next_batch.as_str()) {
                    // Persist AFTER the events above are in the inbox, so a
                    // crash in between re-delivers rather than skips. The
                    // dedupe layer collapses the duplicate; nothing recovers a
                    // skip.
                    sync_store::save_to(&state_path, &next_batch);
                    since = Some(next_batch);
                }
            }
            Err(e) => {
                // A cursor loaded from disk that this homeserver rejects (an
                // expired token, a server that lost its state, a file edited by
                // hand) is unusable and re-presenting it can only fail again.
                // Without this the loop backs off on it forever: a permanently
                // wedged channel that still reports healthy. Drop it once, say
                // so, and fall through to a clean initial sync.
                if resumed_unverified && matches!(e, MatrixError::Http { status: 400, .. }) {
                    tracing::warn!(
                        target: "wcore_channel_matrix::sync",
                        error = %e,
                        path = %state_path.display(),
                        "homeserver rejected the persisted /sync cursor; discarding it and re-seeding from an initial sync. Messages delivered while this process was down are NOT recoverable for this start",
                    );
                    sync_store::clear_at(&state_path);
                    since = None;
                    resumed_unverified = false;
                    consecutive_failures = 0;
                    continue;
                }
                // A 401/403 is the homeserver REJECTING the access token
                // (`M_UNKNOWN_TOKEN` on a revoked one), not a transient fault.
                // Backoff cannot recover a dead credential, and — this was the
                // original defect — the manager cannot infer it either: this
                // loop owns `consecutive_failures` privately, `poll_events()`
                // drains an empty inbox and returns `Ok(vec![])`, and the
                // manager's Ok arm RESETS its error count. So the channel
                // reported `Healthy` while every single sync 401'd, measured
                // live at 21 consecutive failures.
                //
                // #936 splits that one verdict into three. A token the
                // homeserver says is merely EXPIRED (`soft_logout: true`) is
                // renewed in place and the loop continues; a transient fault on
                // the refresh endpoint backs off WITHOUT accusing the
                // credential; and a genuine revocation still publishes
                // `AuthExpired` (exactly once, from the token source) and stops
                // — because a refresh path that papers over a real revocation
                // is worse than no refresh path.
                if token::is_credential_rejection(&e) {
                    match tokens.renew_after_rejection(&presented, &e).await {
                        Renewal::Renewed => {
                            consecutive_failures = 0;
                            continue;
                        }
                        Renewal::Deferred(why) => {
                            tracing::warn!(
                                target: "wcore_channel_matrix::sync",
                                reason = %why,
                                "the Matrix access token was rejected and could not be renewed yet; backing off",
                            );
                        }
                        Renewal::Fatal => {
                            tracing::error!(
                                target: "wcore_channel_matrix::sync",
                                "homeserver rejected the access token and it cannot be renewed; stopping /sync",
                            );
                            break;
                        }
                    }
                } else {
                    tracing::warn!(
                        target: "wcore_channel_matrix::sync",
                        error = %e,
                        "/sync failed; backing off"
                    );
                }
                consecutive_failures = consecutive_failures.saturating_add(1);
                let sleep_secs = (2_u64.saturating_mul(consecutive_failures as u64)).min(30);
                tokio::select! {
                    biased;
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() { break; }
                    }
                    _ = tokio::time::sleep(Duration::from_secs(sleep_secs)) => {}
                }
            }
        }
    }
}

/// One `GET /_matrix/client/v3/sync` call. Returns the decoded response.
/// 4xx/5xx and network failures surface as `Err`; callers back off and retry.
async fn sync_once(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    since: Option<&str>,
) -> Result<SyncResponse, MatrixError> {
    let url = format!("{api_base}/_matrix/client/v3/sync");
    let timeout_str = SYNC_TIMEOUT_MS.to_string();

    let mut query: Vec<(&str, &str)> = vec![("timeout", timeout_str.as_str())];
    if let Some(s) = since {
        query.push(("since", s));
    }

    let resp = http
        .get(&url)
        .bearer_auth(access_token)
        .query(&query)
        // HTTP read timeout = long-poll wait + buffer so we don't hang
        // forever on a misbehaving proxy.
        .timeout(Duration::from_millis(
            SYNC_TIMEOUT_MS.saturating_add(10_000),
        ))
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    // Read the body through a capped helper so neither the error nor the
    // success path can buffer an unbounded body inside this long-poll loop.
    let body_bytes = wcore_egress::read_body_capped(resp, MAX_SYNC_BODY_BYTES)
        .await
        .map_err(|e| MatrixError::Network(format!("sync body read: {e}")))?;

    if !(200..300).contains(&status) {
        // Truncate the retained error body so a large payload can't bloat the
        // error/log path. Slice on a char boundary to keep the string valid.
        let mut body = String::from_utf8_lossy(&body_bytes).into_owned();
        if body.len() > MAX_ERROR_BODY_BYTES {
            let mut end = MAX_ERROR_BODY_BYTES;
            while !body.is_char_boundary(end) {
                end -= 1;
            }
            body.truncate(end);
        }
        return Err(MatrixError::Http { status, body });
    }

    serde_json::from_slice::<SyncResponse>(&body_bytes)
        .map_err(|e| MatrixError::Parse(e.to_string()))
}

/// Pure parse: a decoded `/sync` response + the bot's own user id → the
/// `MessageReceived` events it should emit. Network-free so it can be unit
/// tested directly.
///
/// - Only `m.room.message` timeline events become messages.
/// - Events sent by `bot_user_id` are skipped to avoid self-loops.
/// - `conversation_id` is the room id; `sender`/`author` is the sender mxid.
/// - `chat_type` is [`ChatType::Direct`] when the room summary reports exactly
///   two joined members (the standard 1:1-DM signal), else [`ChatType::Group`].
///   This stops DMs being misrouted through group policy and silently dropped.
/// - `was_mentioned` is best-effort: set when `m.mentions.user_ids` includes
///   the bot, or the message body literally contains the bot's mxid.
fn parse_sync_events(resp: &SyncResponse, bot_user_id: &str) -> Vec<ChannelEvent> {
    let mut events = Vec::new();
    for (room_id, room) in &resp.rooms.join {
        // A 1:1 room (2 joined members) is a direct chat; anything else is a
        // group. Falls back to Group when the homeserver omits the count.
        let chat_type = match room.summary.joined_member_count {
            Some(2) => ChatType::Direct,
            _ => ChatType::Group,
        };
        for ev in &room.timeline.events {
            if ev.event_type != "m.room.message" {
                continue;
            }
            // Skip the bot's own echoes — prevents self-loops.
            if ev.sender == bot_user_id {
                continue;
            }

            let ts_secs = ev.origin_server_ts / 1000;

            // Best-effort mention detection.
            let mentioned_via_block = ev
                .content
                .mentions
                .as_ref()
                .map(|m| m.user_ids.iter().any(|u| u == bot_user_id))
                .unwrap_or(false);
            let mentioned_in_body =
                !bot_user_id.is_empty() && ev.content.body.contains(bot_user_id);
            let was_mentioned = mentioned_via_block || mentioned_in_body;

            let msg = IncomingMessage {
                sender_id: ev.sender.clone(),
                chat_type,
                platform: Some("matrix".into()),
                was_mentioned,
                attachments: attachments_for(&ev.content),
                ..IncomingMessage::new(
                    ev.event_id.clone(),
                    room_id.clone(),
                    ev.sender.clone(),
                    ev.content.body.clone(),
                    ts_secs,
                )
            };
            events.push(ChannelEvent::MessageReceived { msg });
        }
    }
    events
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: &str = "@bot:matrix.example.org";

    fn parse(body: &str, bot: &str) -> Vec<ChannelEvent> {
        let resp: SyncResponse = serde_json::from_str(body).expect("valid /sync body");
        parse_sync_events(&resp, bot)
    }

    // 1. One joined room with one m.room.message → one enriched IncomingMessage.
    #[test]
    fn parses_single_room_message() {
        let body = r#"{
            "next_batch": "s2_batch",
            "rooms": {
                "join": {
                    "!room123:matrix.example.org": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.message",
                                    "sender": "@alice:matrix.example.org",
                                    "event_id": "$evt1",
                                    "origin_server_ts": 1700000010000,
                                    "content": { "msgtype": "m.text", "body": "hi there" }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;

        let events = parse(body, BOT);
        assert_eq!(events.len(), 1, "expected exactly one message event");
        let ChannelEvent::MessageReceived { msg } = &events[0] else {
            panic!("expected MessageReceived, got {:?}", events[0]);
        };
        assert_eq!(msg.id, "$evt1");
        assert_eq!(msg.sender_id, "@alice:matrix.example.org");
        assert_eq!(msg.author, "@alice:matrix.example.org");
        assert_eq!(msg.conversation_id, "!room123:matrix.example.org");
        assert_eq!(msg.text, "hi there");
        // origin_server_ts is millis → seconds.
        assert_eq!(msg.ts_secs, 1_700_000_010);
        assert_eq!(msg.platform.as_deref(), Some("matrix"));
        assert_eq!(msg.chat_type, ChatType::Group);
        assert!(!msg.was_mentioned);
    }

    // 1b. A room whose summary reports two joined members is a direct chat, so
    //     its messages must be ChatType::Direct (not misrouted as a group).
    #[test]
    fn two_member_room_is_direct() {
        let body = r#"{
            "next_batch": "s3",
            "rooms": {
                "join": {
                    "!dm:matrix.example.org": {
                        "summary": { "m.joined_member_count": 2 },
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.message",
                                    "sender": "@alice:matrix.example.org",
                                    "event_id": "$dm1",
                                    "origin_server_ts": 1700000020000,
                                    "content": { "msgtype": "m.text", "body": "psst" }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;
        let events = parse(body, BOT);
        let ChannelEvent::MessageReceived { msg } = &events[0] else {
            panic!("expected MessageReceived, got {:?}", events[0]);
        };
        assert_eq!(msg.chat_type, ChatType::Direct);
    }

    // 2. An event sent by the bot's own user id is skipped (no self-loop).
    #[test]
    fn skips_bot_own_message() {
        let body = r#"{
            "next_batch": "s3_batch",
            "rooms": {
                "join": {
                    "!room123:matrix.example.org": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.message",
                                    "sender": "@bot:matrix.example.org",
                                    "event_id": "$self",
                                    "origin_server_ts": 1700000020000,
                                    "content": { "msgtype": "m.text", "body": "my own reply" }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;

        let events = parse(body, BOT);
        assert!(
            events.is_empty(),
            "bot's own message must be skipped, got {events:?}"
        );
    }

    // 3. Non-message timeline events (e.g. m.room.member) are ignored.
    #[test]
    fn ignores_non_message_events() {
        let body = r#"{
            "next_batch": "s4_batch",
            "rooms": {
                "join": {
                    "!room123:matrix.example.org": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.member",
                                    "sender": "@alice:matrix.example.org",
                                    "event_id": "$member",
                                    "origin_server_ts": 1700000030000,
                                    "content": { "membership": "join" }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;

        let events = parse(body, BOT);
        assert!(events.is_empty(), "non-message events must be ignored");
    }

    // 4. m.mentions.user_ids referencing the bot sets was_mentioned.
    #[test]
    fn detects_native_mention() {
        let body = r#"{
            "next_batch": "s5_batch",
            "rooms": {
                "join": {
                    "!room123:matrix.example.org": {
                        "timeline": {
                            "events": [
                                {
                                    "type": "m.room.message",
                                    "sender": "@alice:matrix.example.org",
                                    "event_id": "$mention",
                                    "origin_server_ts": 1700000040000,
                                    "content": {
                                        "msgtype": "m.text",
                                        "body": "hey can you help",
                                        "m.mentions": { "user_ids": ["@bot:matrix.example.org"] }
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        }"#;

        let events = parse(body, BOT);
        assert_eq!(events.len(), 1);
        let ChannelEvent::MessageReceived { msg } = &events[0] else {
            panic!("expected MessageReceived");
        };
        assert!(
            msg.was_mentioned,
            "m.mentions of the bot must set was_mentioned"
        );
    }

    // 5. Empty /sync (no joined rooms) yields no events.
    #[test]
    fn empty_sync_yields_nothing() {
        let body = r#"{ "next_batch": "s6_batch" }"#;
        let events = parse(body, BOT);
        assert!(events.is_empty());
    }

    // 6. An m.image message maps an Attachment carrying the mxc:// URI.
    #[test]
    fn maps_image_attachment_from_mxc() {
        let body = r#"{
            "next_batch": "s7",
            "rooms": { "join": { "!r:ex.org": { "timeline": { "events": [
                {
                    "type": "m.room.message",
                    "sender": "@alice:ex.org",
                    "event_id": "$img",
                    "origin_server_ts": 1700000050000,
                    "content": {
                        "msgtype": "m.image",
                        "body": "cat.png",
                        "url": "mxc://ex.org/abc123",
                        "info": { "mimetype": "image/png" }
                    }
                }
            ] } } } }
        }"#;
        let events = parse(body, BOT);
        assert_eq!(events.len(), 1);
        let ChannelEvent::MessageReceived { msg } = &events[0] else {
            panic!("expected MessageReceived");
        };
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].url, "mxc://ex.org/abc123");
        assert_eq!(msg.attachments[0].kind, MediaKind::Image);
        assert_eq!(
            msg.attachments[0].content_type.as_deref(),
            Some("image/png")
        );
    }

    // 7. A plain m.text message has no attachments; an m.file with a
    //    non-mxc url is ignored (encrypted/relative refs aren't fetchable).
    #[test]
    fn text_has_no_attachment_and_non_mxc_is_skipped() {
        let text = r#"{ "next_batch": "s8", "rooms": { "join": { "!r:ex.org": { "timeline": { "events": [
            { "type": "m.room.message", "sender": "@a:ex.org", "event_id": "$t", "origin_server_ts": 1700000060000,
              "content": { "msgtype": "m.text", "body": "hello" } }
        ] } } } } }"#;
        let ChannelEvent::MessageReceived { msg } = &parse(text, BOT)[0] else {
            panic!();
        };
        assert!(msg.attachments.is_empty());

        let nonmxc = r#"{ "next_batch": "s9", "rooms": { "join": { "!r:ex.org": { "timeline": { "events": [
            { "type": "m.room.message", "sender": "@a:ex.org", "event_id": "$f", "origin_server_ts": 1700000070000,
              "content": { "msgtype": "m.file", "body": "doc", "url": "https://evil/x" } }
        ] } } } } }"#;
        let ChannelEvent::MessageReceived { msg } = &parse(nonmxc, BOT)[0] else {
            panic!();
        };
        assert!(
            msg.attachments.is_empty(),
            "non-mxc media url must be skipped"
        );
    }

    // -----------------------------------------------------------------------
    // F24-C3-H6 — restart behaviour, driven through the real `sync_loop`.
    //
    // These run the actual loop against a `mockito` homeserver and a real
    // state file, because the defect is not in any pure function: it is in
    // what the loop does with the cursor across two process lifetimes. Every
    // assertion below is a COUNT or an exact request-shape, never "no error
    // occurred" — a loop that delivered nothing at all would otherwise pass.
    // -----------------------------------------------------------------------

    /// A `/sync` with NO `since` parameter — i.e. an initial sync. The
    /// production query is `timeout=<ms>` alone on the initial call and
    /// `timeout=<ms>&since=<cursor>` thereafter, so anchoring the whole query
    /// distinguishes the two exactly.
    fn initial_query() -> mockito::Matcher {
        mockito::Matcher::Regex(r"^timeout=\d+$".to_string())
    }

    fn resume_query(cursor: &str) -> mockito::Matcher {
        mockito::Matcher::UrlEncoded("since".into(), cursor.into())
    }

    /// A `/sync` body with one `m.room.message` in a joined room's timeline.
    fn body_with_event(next_batch: &str, event_id: &str, text: &str) -> String {
        format!(
            r#"{{"next_batch":"{next_batch}","rooms":{{"join":{{"!r:f24.invalid":{{"timeline":{{"events":[
                {{"type":"m.room.message","sender":"@alice:f24.invalid","event_id":"{event_id}",
                  "origin_server_ts":1700000000000,"content":{{"msgtype":"m.text","body":"{text}"}}}}
            ]}}}}}}}}}}"#
        )
    }

    fn body_empty(next_batch: &str) -> String {
        format!(r#"{{"next_batch":"{next_batch}"}}"#)
    }

    fn tmp_state_path(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "wcore-matrix-h6-{tag}-{}-{n}.since",
            std::process::id()
        ))
    }

    /// One channel "process lifetime": spawn the real loop, wait for it to
    /// persist `want_cursor`, then shut it down and return what it delivered.
    ///
    /// Waiting on the PERSISTED cursor (rather than on a sleep) is what makes
    /// these tests deterministic: the loop pushes a response's events into the
    /// inbox BEFORE persisting that response's `next_batch`, so seeing the
    /// cursor guarantees the events are already there.
    async fn run_lifetime(
        server_url: &str,
        state_path: &std::path::Path,
        want_cursor: &str,
        label: &str,
    ) -> Vec<ChannelEvent> {
        let inbox: Arc<Mutex<VecDeque<ChannelEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, rx) = watch::channel(false);
        let http = wcore_egress::EgressClient::builder()
            .user_agent("wcore-matrix-h6-test")
            .build()
            .unwrap_or_default();
        let handle = tokio::spawn(sync_loop(SyncArgs {
            http,
            api_base: server_url.to_string(),
            tokens: crate::token::tests::plain_source(server_url, "syt_test", &inbox),
            user_id: BOT.to_string(),
            inbox: Arc::clone(&inbox),
            shutdown: rx,
            state_path: state_path.to_path_buf(),
        }));

        // Bounded wait — never an unbounded `loop`. 20s ceiling against a
        // mockito server answering in microseconds.
        let mut reached = false;
        for _ in 0..200 {
            if matches!(sync_store::load_from(state_path), Loaded::Cursor(ref c) if c == want_cursor)
            {
                reached = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let _ = tx.send(true);
        if tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .is_err()
        {
            panic!("{label}: sync_loop did not exit within 5s of shutdown");
        }

        assert!(
            reached,
            "{label}: loop never persisted cursor {want_cursor:?}; file held {:?}",
            std::fs::read_to_string(state_path).ok(),
        );

        let drained: Vec<ChannelEvent> = inbox.lock().await.drain(..).collect();
        drained
    }

    fn message_ids(events: &[ChannelEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                ChannelEvent::MessageReceived { msg } => Some(msg.id.clone()),
                _ => None,
            })
            .collect()
    }

    /// **PROOF 1 (the defect, positively) and PROOF 2 (no duplicate).**
    ///
    /// Lifetime 1 seeds from an initial sync whose timeline carries `$pre`.
    /// The process then goes down, and `$gap` is delivered to the homeserver
    /// during the downtime. Lifetime 2 must deliver `$gap` — and must deliver
    /// `$pre` NOT AT ALL, because the replay guard already declined it.
    ///
    /// On the unfixed code lifetime 2 starts from `since = None`, so it issues
    /// a SECOND initial sync and discards its timeline: the initial-sync mock's
    /// `expect(1)` reddens, and `$gap` never arrives. Both halves of the
    /// contract — never replay, never skip — are asserted here, and they pull
    /// in opposite directions, so neither can be satisfied by doing nothing.
    #[tokio::test]
    async fn gap_message_survives_a_restart_and_is_not_duplicated() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("gap");
        let _ = std::fs::remove_file(&state);

        // Exactly ONE initial sync may ever happen for this account. Its
        // timeline carries `$pre`, which the replay guard must swallow.
        let initial = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(initial_query())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_with_event("s1", "$pre", "before the restart"))
            .expect(1)
            .create_async()
            .await;

        let first = run_lifetime(&server.url(), &state, "s1", "lifetime-1").await;
        assert_eq!(
            message_ids(&first),
            Vec::<String>::new(),
            "the initial sync's timeline must NOT be replayed into the inbox",
        );

        // --- the process is down here; the homeserver accumulates `$gap` ---

        let resumed = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(resume_query("s1"))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_with_event(
                "s2",
                "$gap",
                "delivered while you were down",
            ))
            .expect(1)
            .create_async()
            .await;

        let second = run_lifetime(&server.url(), &state, "s2", "lifetime-2").await;

        assert_eq!(
            message_ids(&second),
            vec!["$gap".to_string()],
            "the restarted process must deliver exactly the downtime window: \
             `$gap` once, `$pre` never",
        );
        // The restart resumed rather than re-seeding. This is the assertion the
        // unfixed code fails: it would issue a second initial sync.
        initial.assert_async().await;
        resumed.assert_async().await;

        let _ = std::fs::remove_file(&state);
    }

    /// **PROOF 3a — a MISSING cursor degrades safely and says so.**
    ///
    /// First start for an account: seed from an initial sync, discard its
    /// timeline (no backlog replay), and persist immediately so the NEXT
    /// restart resumes. The "says so" half is the `Absent` classification the
    /// loop logs from; asserted directly because a log line is not a value.
    #[tokio::test]
    async fn a_missing_cursor_seeds_from_now_and_persists_immediately() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("missing");
        let _ = std::fs::remove_file(&state);
        assert!(
            matches!(sync_store::load_from(&state), Loaded::Absent),
            "a missing cursor file must classify as Absent, not Corrupt",
        );

        let initial = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(initial_query())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_with_event("s1", "$backlog", "old room history"))
            .expect(1)
            .create_async()
            .await;

        let events = run_lifetime(&server.url(), &state, "s1", "first-start").await;
        assert_eq!(
            message_ids(&events),
            Vec::<String>::new(),
            "a first start must not replay the room backlog",
        );
        initial.assert_async().await;
        assert!(
            matches!(sync_store::load_from(&state), Loaded::Cursor(ref c) if c == "s1"),
            "the seed must be persisted immediately, or the next restart loses its window",
        );
        let _ = std::fs::remove_file(&state);
    }

    /// **PROOF 3b — a CORRUPT cursor degrades safely and says so.**
    ///
    /// Garbage on disk must not be sent to the homeserver, must not wedge the
    /// loop, and must not be silently mistaken for a first run. It re-seeds,
    /// classifies as `Corrupt` (which is what the loop's `warn!` is keyed on),
    /// and replaces the junk with a usable cursor so the wedge cannot persist
    /// across restarts either.
    #[tokio::test]
    async fn a_corrupt_cursor_reseeds_and_does_not_wedge() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("corrupt");
        std::fs::write(&state, b"\x00not a cursor at all\n").unwrap();
        assert!(
            matches!(sync_store::load_from(&state), Loaded::Corrupt(_)),
            "garbage must classify as Corrupt — Absent would restart from now in silence",
        );

        let initial = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(initial_query())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_empty("s_recovered"))
            .expect(1)
            .create_async()
            .await;

        run_lifetime(&server.url(), &state, "s_recovered", "corrupt-recovery").await;
        initial.assert_async().await;
        assert!(
            matches!(sync_store::load_from(&state), Loaded::Cursor(ref c) if c == "s_recovered"),
            "the corrupt file must be replaced, not left to re-break the next start",
        );
        let _ = std::fs::remove_file(&state);
    }

    /// **PROOF 3c — a cursor the HOMESERVER rejects degrades safely.**
    ///
    /// The nastiest shape: the file is structurally fine, so validation passes,
    /// but the homeserver answers 400. Without the wedge guard the loop backs
    /// off on that cursor forever — a channel that never delivers another
    /// message while reporting healthy, which is the permanent-wedge class this
    /// program has already fixed twice. The loop must discard it once, re-seed,
    /// and recover inside this test's bounded wait.
    #[tokio::test]
    async fn a_cursor_the_homeserver_rejects_is_discarded_rather_than_wedging() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("rejected");
        std::fs::write(&state, b"s_stale_from_another_homeserver").unwrap();

        let rejected = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(resume_query("s_stale_from_another_homeserver"))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":"M_INVALID_PARAM","error":"invalid from token"}"#)
            .expect_at_least(1)
            .create_async()
            .await;
        let reseed = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(initial_query())
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_empty("s_fresh"))
            .expect_at_least(1)
            .create_async()
            .await;

        run_lifetime(&server.url(), &state, "s_fresh", "rejected-cursor").await;

        rejected.assert_async().await;
        reseed.assert_async().await;
        assert!(
            matches!(sync_store::load_from(&state), Loaded::Cursor(ref c) if c == "s_fresh"),
            "the rejected cursor must be replaced so the wedge cannot survive a restart",
        );
        let _ = std::fs::remove_file(&state);
    }

    /// **PROOF 4 — steady-state delivery is unaffected, COUNTED.**
    ///
    /// Three successive incremental syncs after a resume must yield three
    /// messages, in order, with the cursor advancing each time. Counted rather
    /// than assumed: a regression that delivered one of three, or delivered
    /// them out of order, or stopped advancing the cursor, reddens here. The
    /// assertion demands arrivals > 0, so a path that denied everything cannot
    /// satisfy it.
    #[tokio::test]
    async fn steady_state_delivery_is_unaffected_by_cursor_persistence() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("steady");
        std::fs::write(&state, b"s1").unwrap();

        for (from, to, id) in [
            ("s1", "s2", "$m1"),
            ("s2", "s3", "$m2"),
            ("s3", "s4", "$m3"),
        ] {
            server
                .mock("GET", "/_matrix/client/v3/sync")
                .match_query(resume_query(from))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(body_with_event(to, id, "steady"))
                .expect(1)
                .create_async()
                .await;
        }
        // No mock for `since=s4`: the loop backs off there instead of spinning.

        let events = run_lifetime(&server.url(), &state, "s4", "steady").await;
        let ids = message_ids(&events);
        assert_eq!(
            ids,
            vec!["$m1".to_string(), "$m2".to_string(), "$m3".to_string()],
            "three steady-state messages must arrive, once each, in order — got {} of 3",
            ids.len(),
        );
        let _ = std::fs::remove_file(&state);
    }

    // -----------------------------------------------------------------------
    // Credential rejection — `HealthState::Unauthenticated`'s producer.
    //
    // The measured defect: the homeserver 401s every `/sync`, this loop
    // swallowed it into a private backoff counter, `poll_events()` therefore
    // returned `Ok(vec![])`, and the manager reported the channel `Healthy`
    // through 21 consecutive failures.
    //
    // These drive a REAL HTTP 401 through the REAL loop against `mockito`, and
    // they run in BOTH directions: the 401 must produce the event, and a 500
    // and a 200 must NOT. A producer that fires on any failure would be worse
    // than the bug it fixes, so the negative controls below are load-bearing,
    // not decoration.
    // -----------------------------------------------------------------------

    /// A token value that must never reach the health surface. Distinctive so a
    /// leak is unambiguous rather than a coincidental substring.
    const TOKEN_CANARY: &str = "syt_CANARY_2f9c41ab7de6_MUSTNOTLEAK";

    fn auth_events(events: &[ChannelEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                ChannelEvent::AuthExpired { reason } => Some(reason.clone()),
                _ => None,
            })
            .collect()
    }

    /// Spawn the real loop and wait — bounded — for it to exit ON ITS OWN, with
    /// no shutdown signal. Returns `(exited, inbox_contents)`.
    ///
    /// Self-exit is the assertion that matters for a terminal condition: a loop
    /// that kept 401-ing forever would hang here and report `exited == false`,
    /// which is exactly the pre-fix behaviour.
    async fn run_until_self_exit(
        server_url: &str,
        state_path: &std::path::Path,
        settle: Duration,
    ) -> (bool, Vec<ChannelEvent>) {
        let inbox: Arc<Mutex<VecDeque<ChannelEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
        let (tx, rx) = watch::channel(false);
        let http = wcore_egress::EgressClient::builder()
            .user_agent("wcore-matrix-auth-test")
            .build()
            .unwrap_or_default();
        let handle = tokio::spawn(sync_loop(SyncArgs {
            http,
            api_base: server_url.to_string(),
            tokens: crate::token::tests::plain_source(server_url, TOKEN_CANARY, &inbox),
            user_id: BOT.to_string(),
            inbox: Arc::clone(&inbox),
            shutdown: rx,
            state_path: state_path.to_path_buf(),
        }));

        let exited = tokio::time::timeout(settle, handle).await.is_ok();
        // Always release the loop if it is still parked, so a negative-control
        // test does not leak a task into the rest of the suite.
        let _ = tx.send(true);

        let drained: Vec<ChannelEvent> = inbox.lock().await.drain(..).collect();
        (exited, drained)
    }

    /// **Quadrant 1 — the platform rejects the credential.**
    ///
    /// A real 401 `M_UNKNOWN_TOKEN` over HTTP must produce exactly one
    /// `AuthExpired`, and the loop must stop rather than hammer a dead token.
    /// On the unfixed code the inbox stays EMPTY and the loop runs forever, so
    /// both halves of this assertion redden without the fix.
    #[tokio::test]
    async fn a_401_publishes_auth_expired_and_stops_the_loop() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("auth401");
        let _ = std::fs::remove_file(&state);

        let m = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":"M_UNKNOWN_TOKEN","error":"Token is not active"}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let (exited, events) =
            run_until_self_exit(&server.url(), &state, Duration::from_secs(10)).await;

        m.assert_async().await;
        assert!(
            exited,
            "a rejected token is terminal: the loop must exit, not back off forever"
        );

        let reasons = auth_events(&events);
        assert_eq!(
            reasons.len(),
            1,
            "expected exactly 1 AuthExpired, got {} — {reasons:?}",
            reasons.len()
        );
        assert!(
            reasons[0].contains("M_UNKNOWN_TOKEN"),
            "the reason must name the platform errcode so an operator can act: {:?}",
            reasons[0]
        );
        // Secret-free by construction, with the canary proved to be the value
        // actually in play (the loop authenticated with it above).
        assert!(
            !reasons[0].contains(TOKEN_CANARY),
            "the health reason leaked the access token: {:?}",
            reasons[0]
        );
        let _ = std::fs::remove_file(&state);
    }

    /// **Quadrant 3 — a transient fault must NOT be reported as an auth
    /// rejection.** This is the control that stops the fix being worse than the
    /// defect: a health surface that cries `Unauthenticated` at every 500 would
    /// send operators to rotate a perfectly good credential.
    ///
    /// A 500 must produce ZERO `AuthExpired` and must NOT stop the loop — the
    /// existing backoff still owns it.
    #[tokio::test]
    async fn a_500_is_not_an_auth_rejection_and_does_not_stop_the_loop() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("auth500");
        let _ = std::fs::remove_file(&state);

        let m = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .with_body(r#"{"errcode":"M_UNKNOWN","error":"Internal server error"}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let (exited, events) =
            run_until_self_exit(&server.url(), &state, Duration::from_secs(4)).await;

        m.assert_async().await;
        assert!(
            !exited,
            "a 500 is transient — the loop must keep backing off, not treat it as terminal"
        );
        assert!(
            auth_events(&events).is_empty(),
            "a 500 must NOT be reported as a credential rejection: {:?}",
            auth_events(&events)
        );
        let _ = std::fs::remove_file(&state);
    }

    /// **Quadrant 4 — everything fine.** A healthy 200 flow must produce no
    /// `AuthExpired` at all. Paired with a known-positive (the message actually
    /// arrives) so this cannot pass by the loop doing nothing.
    #[tokio::test]
    async fn a_healthy_sync_publishes_no_auth_expired() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("authok");
        let _ = std::fs::remove_file(&state);

        server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(initial_query())
            .with_status(200)
            .with_body(body_empty("h1"))
            .create_async()
            .await;
        server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_query(resume_query("h1"))
            .with_status(200)
            .with_body(body_with_event("h2", "$ok1", "hello"))
            .create_async()
            .await;

        let events = run_lifetime(&server.url(), &state, "h2", "healthy-noauth").await;

        assert_eq!(
            message_ids(&events),
            vec!["$ok1".to_string()],
            "known-positive: the healthy flow must actually deliver, or the \
             absence assertion below is free"
        );
        assert!(
            auth_events(&events).is_empty(),
            "a healthy channel must never publish AuthExpired: {:?}",
            auth_events(&events)
        );
        let _ = std::fs::remove_file(&state);
    }

    /// What counts as a CREDENTIAL rejection, asserted against the production
    /// predicate.
    ///
    /// This test used to build a `MatrixError` and then `matches!` it against
    /// the same `status: 401 | 403` pattern written inline — it asserted a fact
    /// about `matches!`, referenced no production code, and would have stayed
    /// green through any change to the real classification. It now calls
    /// [`token::is_credential_rejection`], the one predicate both this loop and
    /// the send path gate on.
    ///
    /// Three obligations, and each row can fail on its own:
    ///
    /// * The 400 cursor-rejection path predates #936 and must keep re-seeding
    ///   rather than becoming a terminal "rotate your token".
    /// * A token errcode IS the credential, on either status.
    /// * `M_FORBIDDEN` on 403 is the bot's POWER LEVEL, not its identity.
    ///   Classifying it as a credential rejection latches the channel
    ///   `Unauthenticated` for a token that still works.
    #[test]
    fn credential_rejection_is_the_errcode_not_the_bare_status() {
        for status in [400_u16, 404, 429, 500, 502] {
            assert!(
                !token::is_credential_rejection(&MatrixError::Http {
                    status,
                    body: r#"{"errcode":"M_UNKNOWN"}"#.to_string(),
                }),
                "HTTP {status} must not be classified as a credential rejection"
            );
        }
        for status in [401_u16, 403] {
            assert!(
                token::is_credential_rejection(&MatrixError::Http {
                    status,
                    body: r#"{"errcode":"M_UNKNOWN_TOKEN"}"#.to_string(),
                }),
                "HTTP {status} M_UNKNOWN_TOKEN IS a credential rejection"
            );
        }
        // A gateway that stripped the Matrix body still refused our identity.
        assert!(
            token::is_credential_rejection(&MatrixError::Http {
                status: 401,
                body: "<html>Unauthorized</html>".to_string(),
            }),
            "a bare 401 is still a credential rejection; no retry fixes it"
        );
        // The row this predicate exists for.
        assert!(
            !token::is_credential_rejection(&MatrixError::Http {
                status: 403,
                body: r#"{"errcode":"M_FORBIDDEN","error":"no permission to redact"}"#.to_string(),
            }),
            "M_FORBIDDEN is a power level, not a dead token; classifying it \
             latches the channel Unauthenticated while every send still works"
        );
        assert!(
            !token::is_credential_rejection(&MatrixError::Http {
                status: 403,
                body: "blocked by upstream proxy".to_string(),
            }),
            "a bare 403 is an upstream block, not a credential verdict"
        );
        assert!(
            !token::is_credential_rejection(&MatrixError::Network("timeout".to_string())),
            "a network fault is not a credential verdict"
        );
    }

    /// **Quadrant 2 — the token merely EXPIRED (#936).**
    ///
    /// This is the case the adapter had no answer for: a homeserver on the
    /// OIDC / Matrix Authentication Service path answers `M_UNKNOWN_TOKEN`
    /// with `soft_logout: true` when a short-lived access token ages out. The
    /// loop must refresh in place and keep delivering — no `AuthExpired`, no
    /// exit. Before the fix the identical response took the channel down
    /// permanently.
    ///
    /// It is deliberately paired with the hard-revocation test above, which
    /// still exits: a refresh path that recovered from BOTH would be worse
    /// than no refresh path.
    #[tokio::test]
    async fn a_soft_logout_is_refreshed_in_place_and_delivery_continues() {
        let mut server = mockito::Server::new_async().await;
        let state = tmp_state_path("softlogout");
        let _ = std::fs::remove_file(&state);

        // The aged-out token is refused ...
        let refused = server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_header("authorization", "Bearer syt_expired")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"errcode":"M_UNKNOWN_TOKEN","soft_logout":true}"#)
            .expect_at_least(1)
            .create_async()
            .await;
        // ... the refresh endpoint mints a replacement pair, exactly once ...
        let refreshed = server
            .mock("POST", "/_matrix/client/v3/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"access_token":"syt_renewed","refresh_token":"rot_next","expires_in_ms":3600000}"#,
            )
            .expect(1)
            .create_async()
            .await;
        // ... and only the replacement is served.
        server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_header("authorization", "Bearer syt_renewed")
            .match_query(initial_query())
            .with_status(200)
            .with_body(body_empty("g1"))
            .create_async()
            .await;
        server
            .mock("GET", "/_matrix/client/v3/sync")
            .match_header("authorization", "Bearer syt_renewed")
            .match_query(resume_query("g1"))
            .with_status(200)
            .with_body(body_with_event("g2", "$after_refresh", "delivered anyway"))
            .create_async()
            .await;

        let lock_dir = tempfile::tempdir().unwrap();
        let inbox: Arc<Mutex<VecDeque<ChannelEvent>>> = Arc::new(Mutex::new(VecDeque::new()));
        let creds = crate::token::tests::MemCreds::new(&[
            ("matrix.test.access", "syt_expired"),
            ("matrix.test.refresh", "rot_first"),
        ]);
        let tokens = crate::token::tests::refreshing_source(
            &server.url(),
            "syt_expired",
            creds.clone(),
            lock_dir.path(),
            &inbox,
        );
        let (tx, rx) = watch::channel(false);
        let handle = tokio::spawn(sync_loop(SyncArgs {
            http: wcore_egress::EgressClient::builder()
                .user_agent("wcore-matrix-936-test")
                .build()
                .unwrap_or_default(),
            api_base: server.url(),
            tokens: Arc::clone(&tokens),
            user_id: BOT.to_string(),
            inbox: Arc::clone(&inbox),
            shutdown: rx,
            state_path: state.clone(),
        }));

        // Bounded wait for the post-refresh cursor; the loop persists it only
        // after the message it accompanied is already in the inbox.
        let mut reached = false;
        for _ in 0..200 {
            if matches!(sync_store::load_from(&state), Loaded::Cursor(ref c) if c == "g2") {
                reached = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let _ = tx.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
        let events: Vec<ChannelEvent> = inbox.lock().await.drain(..).collect();

        assert!(
            reached,
            "the loop never got past the expired token: {:?}",
            auth_events(&events),
        );
        refused.assert_async().await;
        refreshed.assert_async().await;
        assert_eq!(
            message_ids(&events),
            vec!["$after_refresh".to_string()],
            "the message that arrived after the renewal must be delivered",
        );
        assert!(
            auth_events(&events).is_empty(),
            "a credential that was RECOVERED must not be reported unauthenticated: {:?}",
            auth_events(&events),
        );
        assert_eq!(
            tokens.access(),
            "syt_renewed",
            "the renewed token must be the one in play",
        );
        assert_eq!(
            creds.peek("matrix.test.refresh").as_deref(),
            Some("rot_next"),
            "the rotated refresh token must be persisted for the next process",
        );
        let _ = std::fs::remove_file(&state);
    }
}
