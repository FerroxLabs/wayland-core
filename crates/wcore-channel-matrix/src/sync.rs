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

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::{Mutex, watch};

use wcore_channels::event::{ChannelEvent, ChatType, IncomingMessage};

use crate::error::MatrixError;

/// Long-poll timeout (ms) handed to the homeserver's `/sync`. The HTTP read
/// timeout is this plus a buffer so a wedged proxy can't park us forever.
const SYNC_TIMEOUT_MS: u64 = 30_000;

/// Constructor arguments — flatter than a struct, easier to spawn.
pub(crate) struct SyncArgs {
    pub http: wcore_egress::EgressClient,
    pub api_base: String,
    pub access_token: String,
    pub user_id: String,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub shutdown: watch::Receiver<bool>,
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
        access_token,
        user_id,
        inbox,
        mut shutdown,
    } = args;

    // `None` until the first /sync completes — the first response only seeds
    // the cursor and never emits timeline events (replay guard).
    let mut since: Option<String> = None;
    let mut consecutive_failures: u32 = 0;

    loop {
        if *shutdown.borrow() {
            break;
        }

        // Race the next API call against a shutdown signal so we don't get
        // stuck for ~SYNC_TIMEOUT_MS after stop() flips the flag.
        let result = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
                continue;
            }
            r = sync_once(&http, &api_base, &access_token, since.as_deref()) => r,
        };

        match result {
            Ok(resp) => {
                consecutive_failures = 0;
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
                            guard.push_back(e);
                        }
                    }
                }
                // Advance the cursor only on a non-empty token. A spec-
                // compliant homeserver always returns a non-empty next_batch,
                // but a malformed/proxy response with `next_batch: ""` would,
                // if stored, send `?since=` next tick — which some homeservers
                // treat as an initial sync and could replay backlog. Keep the
                // prior cursor in that case.
                if !next_batch.is_empty() {
                    since = Some(next_batch);
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_matrix::sync",
                    error = %e,
                    "/sync failed; backing off"
                );
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
        .timeout(Duration::from_millis(SYNC_TIMEOUT_MS.saturating_add(10_000)))
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body });
    }

    resp.json::<SyncResponse>()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))
}

/// Pure parse: a decoded `/sync` response + the bot's own user id → the
/// `MessageReceived` events it should emit. Network-free so it can be unit
/// tested directly.
///
/// - Only `m.room.message` timeline events become messages.
/// - Events sent by `bot_user_id` are skipped to avoid self-loops.
/// - `conversation_id` is the room id; `sender`/`author` is the sender mxid.
/// - `chat_type` defaults to [`ChatType::Group`]: Matrix can't cheaply tell DM
///   from group without reading `m.direct` account-data. A later refinement can
///   read `m.direct` and set [`ChatType::Direct`] for 1:1 rooms.
/// - `was_mentioned` is best-effort: set when `m.mentions.user_ids` includes
///   the bot, or the message body literally contains the bot's mxid.
fn parse_sync_events(resp: &SyncResponse, bot_user_id: &str) -> Vec<ChannelEvent> {
    let mut events = Vec::new();
    for (room_id, room) in &resp.rooms.join {
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
                chat_type: ChatType::Group,
                platform: Some("matrix".into()),
                was_mentioned,
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
        assert!(msg.was_mentioned, "m.mentions of the bot must set was_mentioned");
    }

    // 5. Empty /sync (no joined rooms) yields no events.
    #[test]
    fn empty_sync_yields_nothing() {
        let body = r#"{ "next_batch": "s6_batch" }"#;
        let events = parse(body, BOT);
        assert!(events.is_empty());
    }
}
