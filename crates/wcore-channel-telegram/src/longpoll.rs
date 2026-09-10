//! Background long-poll task. Spawned by `TelegramChannel::start()`,
//! signaled to exit by the watch channel in `TelegramChannel`.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use wcore_channels::event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MediaKind, MentionKind,
};

use crate::api::{Update, get_updates};
use crate::error::TelegramError;

/// Constructor arguments — flatter than a struct, easier to spawn.
pub(crate) struct LongPollArgs {
    pub http: wcore_egress::EgressClient,
    pub api_base: String,
    pub bot_token: String,
    /// Channel name — keys the persisted update offset so a restart resumes
    /// past the last-confirmed update instead of re-delivering it.
    pub channel_name: String,
    /// Channel-state root for the persisted offset. `None` = under the profile
    /// home; `Some` states it. See `crate::offset_store::state_path`.
    pub state_dir: Option<std::path::PathBuf>,
    pub timeout_secs: u32,
    pub allowed_chat_ids: HashSet<String>,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub shutdown: watch::Receiver<bool>,
}

/// Drive `getUpdates` in a loop until the shutdown signal flips.
///
/// Backoff on transient failures stays small (2s + jitter-free) — the
/// caller's poll cadence is the load-bearing knob, not this loop's.
pub(crate) async fn longpoll_loop(args: LongPollArgs) {
    let LongPollArgs {
        http,
        api_base,
        bot_token,
        channel_name,
        state_dir,
        timeout_secs,
        allowed_chat_ids,
        inbox,
        mut shutdown,
    } = args;

    // Seed from the persisted watermark so a restart does not re-deliver the
    // final unconfirmed batch as duplicate turns. Absent/corrupt file → 0.
    let mut offset: i64 =
        crate::offset_store::load(state_dir.as_deref(), &channel_name).unwrap_or(0);
    let mut consecutive_failures: u32 = 0;

    loop {
        if *shutdown.borrow() {
            break;
        }

        // Race the next API call against a shutdown signal so we don't
        // get stuck for ~timeout_secs after stop() flips the flag.
        let updates = tokio::select! {
            biased;
            _ = shutdown.changed() => {
                if *shutdown.borrow() { break; }
                continue;
            }
            r = get_updates(&http, &api_base, &bot_token, offset, timeout_secs) => r,
        };

        match updates {
            Ok(updates) => {
                consecutive_failures = 0;
                let before = offset;
                ingest_updates(updates, &allowed_chat_ids, &inbox, &mut offset).await;
                // Persist only when the offset actually advanced — confirming
                // these updates so the next getUpdates (this run or after a
                // restart) starts past them.
                if offset > before {
                    crate::offset_store::save(state_dir.as_deref(), &channel_name, offset);
                }
            }
            // A revoked/invalid bot token makes getUpdates return 401/403,
            // which classifies as TelegramError::Auth. That is terminal — no
            // amount of backoff recovers a dead token, and hammering 401s
            // every few seconds risks Telegram deleting the bot. Surface the
            // failure as an AuthError state-change (so the manager's
            // supervision sees the channel is dead instead of Connected) and
            // break the loop. Transient errors keep their existing backoff.
            Err(TelegramError::Auth(desc)) => {
                tracing::error!(
                    target: "wcore_channel_telegram::longpoll",
                    error = %desc,
                    "getUpdates rejected the bot token (auth); stopping long-poll"
                );
                inbox
                    .lock()
                    .await
                    .push_back(ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::AuthError,
                    });
                break;
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_telegram::longpoll",
                    error = %e,
                    "getUpdates failed; backing off"
                );
                consecutive_failures = consecutive_failures.saturating_add(1);
                // Linear cap at 30s — same family as the send retry cap
                // but without the exponential bias (the poll loop is
                // self-correcting; tight failure loops here are usually
                // a transient outage, not a coding error).
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

/// A media reference pulled off a Telegram message, pre-resolution. The
/// `file_id` still needs a `getFile` round-trip before it points at
/// downloadable bytes; `kind` / `content_type` are known from the field
/// the media arrived in.
struct PendingMedia {
    file_id: String,
    kind: MediaKind,
    content_type: Option<String>,
}

/// Map a `(content_type, MediaKind)` for each media-bearing field on a
/// Telegram message into the pre-resolution `PendingMedia` list. Pure so
/// the field→kind/mime mapping is testable without a network call.
fn pending_media(msg: &crate::api::Message) -> Vec<PendingMedia> {
    let mut out: Vec<PendingMedia> = Vec::new();
    // Photos: take the last (largest) PhotoSize only.
    if let Some(ref sizes) = msg.photo
        && let Some(largest) = sizes.last()
    {
        out.push(PendingMedia {
            file_id: largest.file_id.clone(),
            kind: MediaKind::Image,
            content_type: Some("image/jpeg".to_string()),
        });
    }
    if let Some(ref v) = msg.voice {
        out.push(PendingMedia {
            file_id: v.file_id.clone(),
            kind: MediaKind::Audio,
            // Voice notes are always OGG/Opus; fall back if absent.
            content_type: v
                .mime_type
                .clone()
                .or_else(|| Some("audio/ogg".to_string())),
        });
    }
    if let Some(ref d) = msg.document {
        out.push(PendingMedia {
            file_id: d.file_id.clone(),
            kind: MediaKind::Document,
            content_type: d.mime_type.clone(),
        });
    }
    if let Some(ref vid) = msg.video {
        out.push(PendingMedia {
            file_id: vid.file_id.clone(),
            kind: MediaKind::Video,
            content_type: vid
                .mime_type
                .clone()
                .or_else(|| Some("video/mp4".to_string())),
        });
    }
    if let Some(ref a) = msg.audio {
        out.push(PendingMedia {
            file_id: a.file_id.clone(),
            kind: MediaKind::Audio,
            // Audio files are commonly MP3; fall back when unreported.
            content_type: a
                .mime_type
                .clone()
                .or_else(|| Some("audio/mpeg".to_string())),
        });
    }
    if let Some(ref s) = msg.sticker {
        out.push(PendingMedia {
            file_id: s.file_id.clone(),
            // Stickers are surfaced as images so the agent sees them
            // rather than a blank message.
            kind: MediaKind::Image,
            // Static stickers are WebP; carry no reported mime.
            content_type: Some("image/webp".to_string()),
        });
    }
    if let Some(ref vn) = msg.video_note {
        out.push(PendingMedia {
            file_id: vn.file_id.clone(),
            kind: MediaKind::Video,
            // Round video messages are always MPEG-4.
            content_type: Some("video/mp4".to_string()),
        });
    }
    out
}

/// Map each `PendingMedia` to a typed [`Attachment`], carrying only the opaque
/// Telegram `file_id` in `path`.
///
/// The actual download URL embeds the live bot token in its path
/// (`{base}/file/bot{token}/{file_path}`), so it is deliberately NOT resolved
/// or stored here — storing it would leak the token into `IncomingMessage`,
/// traces, and any log sink. [`TelegramChannel::fetch_media`] resolves the URL
/// on demand (via `getFile`) as an ephemeral local at download time.
fn resolve_attachments(pending: Vec<PendingMedia>) -> Vec<Attachment> {
    pending
        .into_iter()
        .map(|m| Attachment {
            path: Some(m.file_id),
            content_type: m.content_type,
            kind: m.kind,
            ..Default::default()
        })
        .collect()
}

async fn ingest_updates(
    updates: Vec<Update>,
    allowed_chat_ids: &HashSet<String>,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    offset: &mut i64,
) {
    if updates.is_empty() {
        return;
    }
    let mut events = Vec::with_capacity(updates.len());
    // Album coalescing: Telegram delivers a multi-item album as N messages
    // sharing one `media_group_id` (consecutively, in the same batch). We hold
    // the `(group_id, event index)` of the album in flight so each follow-on
    // item merges its attachment into the first item's event instead of firing
    // its own turn. (A split across two getUpdates batches would still split —
    // a debounce buffer would be needed for that rarer case.)
    let mut last_album: Option<(String, usize)> = None;
    for u in updates {
        // Advance offset past every Update we see, even ones we drop —
        // otherwise we'd loop on the same filtered-out message forever.
        *offset = (*offset).max(u.update_id + 1);

        // A bot in a broadcast channel receives posts in `channel_post`, not
        // `message` — treat either as the inbound message so channel posts are
        // not silently dropped. An `edited_message` / `edited_channel_post`
        // is surfaced too (it carries `edit_date`): we request edits in
        // `allowed_updates`, so dropping them here would lose the user's
        // correction. The edit re-dispatches as inbound with the corrected
        // text rather than the stale original.
        let Some(msg) = u
            .message
            .or(u.channel_post)
            .or(u.edited_message)
            .or(u.edited_channel_post)
        else {
            continue;
        };
        let chat_id_str = msg.chat.id.to_string();
        if !allowed_chat_ids.is_empty() && !allowed_chat_ids.contains(&chat_id_str) {
            continue;
        }

        // ---- Attachments --------------------------------------------
        // Carry only the opaque file_id; the token-bearing download URL is
        // resolved lazily in `fetch_media` so the bot token never lands in
        // the event struct, traces, or logs. Computed early so an album
        // follow-on item can merge into the in-flight album event below.
        let pending = pending_media(&msg);
        let attachments = resolve_attachments(pending);

        // ---- Album coalescing ---------------------------------------
        // A follow-on album item (same media_group_id as the event in flight)
        // contributes only its attachment; the album's text/caption rides on
        // the first item. Merge and skip building a separate event/turn.
        let group_id = msg.media_group_id.clone();
        if let Some(gid) = &group_id
            && let Some((last_gid, idx)) = &last_album
            && last_gid == gid
            && let Some(ChannelEvent::MessageReceived { msg: first }) = events.get_mut(*idx)
        {
            first.attachments.extend(attachments);
            continue;
        }

        // ---- Sender identity ----------------------------------------
        let (sender_id, author, sender_display, sender_handle, is_bot) =
            if let Some(ref f) = msg.from {
                let sid = f.id.to_string();
                // author: prefer @username, fall back to first_name, then id
                let display_name = match (f.first_name.as_deref(), f.last_name.as_deref()) {
                    (Some(first), Some(last)) => Some(format!("{first} {last}")),
                    (Some(first), None) => Some(first.to_string()),
                    _ => None,
                };
                let author = f
                    .username
                    .clone()
                    .or_else(|| display_name.clone())
                    .unwrap_or_else(|| sid.clone());
                (sid, author, display_name, f.username.clone(), f.is_bot)
            } else {
                // No `from` — this is a channel post (broadcast channels carry
                // no per-message sender). Fall back to the channel's own
                // identity: the chat id, with the title as the display name.
                let author = msg
                    .chat
                    .title
                    .clone()
                    .unwrap_or_else(|| chat_id_str.clone());
                (
                    chat_id_str.clone(),
                    author,
                    msg.chat.title.clone(),
                    None,
                    false,
                )
            };

        // ---- Chat type ----------------------------------------------
        let chat_type = match msg.chat.chat_type.as_str() {
            "private" => ChatType::Direct,
            "group" | "supergroup" => ChatType::Group,
            "channel" => ChatType::Channel,
            // Unrecognised future type — treat as Group (multi-party)
            _ => ChatType::Group,
        };

        // ---- Mention detection --------------------------------------
        // A `mention` entity in the text signals an @-mention; the bot
        // has no self-identity here so we can only detect the presence of
        // any mention and surface it as Native.
        let has_mention = msg
            .entities
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|e| e.kind == "mention");
        let was_mentioned = has_mention;
        let mention_kind = was_mentioned.then_some(MentionKind::Native);

        // ---- Reply context ------------------------------------------
        let reply_to_message_id = msg
            .reply_to_message
            .as_deref()
            .map(|r| r.message_id.to_string());
        let reply_to_text = msg.reply_to_message.as_deref().and_then(|r| r.text.clone());

        // Media messages carry their words in `caption`, not `text`, so fall
        // back to the caption when `text` is absent — otherwise a captioned
        // photo would reach the engine with an empty body.
        let text = msg.text.or(msg.caption).unwrap_or_default();

        events.push(ChannelEvent::MessageReceived {
            msg: IncomingMessage {
                id: msg.message_id.to_string(),
                conversation_id: chat_id_str,
                author,
                text,
                // An edit timestamps at edit_date; a fresh message at date.
                ts_secs: msg.edit_date.unwrap_or(msg.date),
                attachments,
                // Sender identity
                sender_id,
                sender_display,
                sender_handle,
                sender_alt_id: None,
                is_bot,
                is_self: false,
                // Chat context
                chat_type,
                chat_name: msg.chat.title.clone(),
                space_id: None,
                thread_id: msg.message_thread_id.map(|id| id.to_string()),
                parent_chat_id: None,
                // Account / platform routing
                account_id: None,
                platform: Some("telegram".into()),
                // Mention
                was_mentioned,
                mention_kind,
                // Reply
                reply_to_message_id,
                reply_to_text,
            },
        });

        // Record this as the album in flight (so subsequent same-group items
        // merge into it) — or clear it for a non-album message, so only a
        // contiguous run of same-group items coalesces.
        last_album = group_id.map(|gid| (gid, events.len() - 1));
    }
    if !events.is_empty() {
        let mut guard = inbox.lock().await;
        for e in events {
            // F9 — bounded, drop-oldest inbox against a flood.
            wcore_channels::push_bounded(&mut guard, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Message;

    fn message_from_json(raw: &str) -> Message {
        serde_json::from_str(raw).expect("valid Message JSON")
    }

    #[test]
    fn pending_media_maps_photo_to_image_jpeg() {
        // Photos carry no mime; we synthesize image/jpeg and pick the
        // largest (last) PhotoSize.
        let msg = message_from_json(
            r#"{"message_id":1,"chat":{"id":1},"photo":[{"file_id":"small"},{"file_id":"large"}]}"#,
        );
        let pending = pending_media(&msg);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].file_id, "large");
        assert_eq!(pending[0].kind, MediaKind::Image);
        assert_eq!(pending[0].content_type.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn pending_media_maps_voice_to_audio_ogg_fallback() {
        let msg = message_from_json(r#"{"message_id":1,"chat":{"id":1},"voice":{"file_id":"v"}}"#);
        let pending = pending_media(&msg);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].kind, MediaKind::Audio);
        assert_eq!(pending[0].content_type.as_deref(), Some("audio/ogg"));
    }

    #[test]
    fn resolve_attachments_carries_only_file_id_never_the_token_url() {
        // The bot token must never be stored in the attachment (it would leak
        // into IncomingMessage, traces, and logs). The token-bearing URL is
        // resolved lazily in fetch_media; here only the opaque file_id is kept.
        let pending = vec![PendingMedia {
            file_id: "ABC123".to_string(),
            kind: MediaKind::Image,
            content_type: Some("image/jpeg".to_string()),
        }];
        let atts = resolve_attachments(pending);
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0].path.as_deref(), Some("ABC123"));
        assert!(
            atts[0].url.is_empty(),
            "url must not carry a token-bearing URL"
        );
        assert!(
            !atts[0].url.contains("bot"),
            "no bot-token path segment may appear in the attachment url"
        );
    }

    #[test]
    fn pending_media_prefers_reported_mime() {
        // A document with an explicit mime keeps it; a video without one
        // falls back to video/mp4.
        let msg = message_from_json(
            r#"{"message_id":1,"chat":{"id":1},"document":{"file_id":"d","mime_type":"application/pdf"},"video":{"file_id":"vid"}}"#,
        );
        let pending = pending_media(&msg);
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].kind, MediaKind::Document);
        assert_eq!(pending[0].content_type.as_deref(), Some("application/pdf"));
        assert_eq!(pending[1].kind, MediaKind::Video);
        assert_eq!(pending[1].content_type.as_deref(), Some("video/mp4"));
    }

    #[test]
    fn pending_media_empty_for_text_only_message() {
        let msg = message_from_json(r#"{"message_id":1,"chat":{"id":1},"text":"hello"}"#);
        assert!(pending_media(&msg).is_empty());
    }

    #[test]
    fn pending_media_maps_audio_to_audio() {
        // Audio with an explicit mime keeps it; without one falls back to
        // audio/mpeg. Previously audio was silently dropped → blank message.
        let with_mime = message_from_json(
            r#"{"message_id":1,"chat":{"id":1},"audio":{"file_id":"a","mime_type":"audio/flac"}}"#,
        );
        let p = pending_media(&with_mime);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].file_id, "a");
        assert_eq!(p[0].kind, MediaKind::Audio);
        assert_eq!(p[0].content_type.as_deref(), Some("audio/flac"));

        let no_mime =
            message_from_json(r#"{"message_id":1,"chat":{"id":1},"audio":{"file_id":"a"}}"#);
        let p = pending_media(&no_mime);
        assert_eq!(p[0].content_type.as_deref(), Some("audio/mpeg"));
    }

    #[test]
    fn pending_media_maps_sticker_to_image() {
        // Stickers carry no mime and were previously dropped; surface as an
        // image (image/webp) so the agent sees it.
        let msg =
            message_from_json(r#"{"message_id":1,"chat":{"id":1},"sticker":{"file_id":"s"}}"#);
        let p = pending_media(&msg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].file_id, "s");
        assert_eq!(p[0].kind, MediaKind::Image);
        assert_eq!(p[0].content_type.as_deref(), Some("image/webp"));
    }

    #[test]
    fn pending_media_maps_video_note_to_video() {
        // Round video messages were previously dropped; surface as a video.
        let msg =
            message_from_json(r#"{"message_id":1,"chat":{"id":1},"video_note":{"file_id":"vn"}}"#);
        let p = pending_media(&msg);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].file_id, "vn");
        assert_eq!(p[0].kind, MediaKind::Video);
        assert_eq!(p[0].content_type.as_deref(), Some("video/mp4"));
    }

    #[tokio::test]
    async fn channel_post_update_yields_message_received() {
        // A bot added to a broadcast channel receives posts in `channel_post`,
        // not `message`, and they carry no `from`. ingest_updates must surface
        // the post as a MessageReceived with the channel's chat identity as the
        // sender — otherwise channel posts are silently dropped.
        let update: Update = serde_json::from_str(
            r#"{"update_id":42,"channel_post":{"message_id":7,"chat":{"id":-1001234,"type":"channel","title":"News"},"text":"breaking"}}"#,
        )
        .expect("valid Update JSON");
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let mut offset = 0;
        ingest_updates(vec![update], &HashSet::new(), &inbox, &mut offset).await;

        assert_eq!(offset, 43, "offset must advance past the channel post");
        let guard = inbox.lock().await;
        let event = guard.front().expect("one event ingested");
        match event {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "breaking");
                assert_eq!(msg.conversation_id, "-1001234");
                assert_eq!(msg.chat_type, ChatType::Channel);
                // No `from` → sender falls back to the channel chat identity.
                assert_eq!(msg.sender_id, "-1001234");
                assert_eq!(msg.author, "News");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_album_coalesces_into_one_event_with_all_attachments() {
        // Three photos sharing a media_group_id arrive as three messages in one
        // getUpdates batch — they must surface as ONE inbound message carrying
        // three attachments, not three separate agent turns.
        let raw = [
            r#"{"update_id":1,"message":{"message_id":10,"date":1,"chat":{"id":7,"type":"private"},"from":{"id":1,"username":"u"},"media_group_id":"alb1","caption":"three pics","photo":[{"file_id":"p1"}]}}"#,
            r#"{"update_id":2,"message":{"message_id":11,"date":1,"chat":{"id":7,"type":"private"},"from":{"id":1,"username":"u"},"media_group_id":"alb1","photo":[{"file_id":"p2"}]}}"#,
            r#"{"update_id":3,"message":{"message_id":12,"date":1,"chat":{"id":7,"type":"private"},"from":{"id":1,"username":"u"},"media_group_id":"alb1","photo":[{"file_id":"p3"}]}}"#,
        ];
        let updates: Vec<Update> = raw
            .iter()
            .map(|r| serde_json::from_str(r).expect("valid Update JSON"))
            .collect();
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let mut offset = 0;
        ingest_updates(updates, &HashSet::new(), &inbox, &mut offset).await;

        assert_eq!(offset, 4, "offset must advance past all three album items");
        let guard = inbox.lock().await;
        assert_eq!(guard.len(), 1, "an album must coalesce into ONE event");
        match guard.front().expect("one event") {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "three pics", "caption rides on the first item");
                assert_eq!(msg.attachments.len(), 3, "all three photos are merged");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn edited_message_update_surfaces_the_correction() {
        // We request `edited_message` in allowed_updates, so an edit must
        // reach the agent rather than being silently dropped. The corrected
        // text is surfaced and the inbound timestamp reflects edit_date, not
        // the original send.
        let update: Update = serde_json::from_str(
            r#"{"update_id":50,"edited_message":{"message_id":9,"date":1700000000,"edit_date":1700000900,"chat":{"id":555,"type":"private"},"from":{"id":1,"username":"alice"},"text":"corrected text"}}"#,
        )
        .expect("valid Update JSON");
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let mut offset = 0;
        ingest_updates(vec![update], &HashSet::new(), &inbox, &mut offset).await;

        assert_eq!(offset, 51, "offset must advance past the edit");
        let guard = inbox.lock().await;
        let event = guard
            .front()
            .expect("the edit must be ingested, not dropped");
        match event {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "corrected text");
                assert_eq!(msg.conversation_id, "555");
                // Timestamped at the edit, not the original send.
                assert_eq!(msg.ts_secs, 1700000900);
                assert_eq!(msg.author, "alice");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn auth_failure_terminates_loop_and_surfaces_auth_error() {
        // A revoked/invalid bot token makes getUpdates return 401, which the
        // api layer classifies as TelegramError::Auth. The loop must treat
        // this as terminal: stop retrying (so it can't hammer 401s forever and
        // risk Telegram deleting the bot) and push an AuthError state-change so
        // supervision sees the channel is dead rather than Connected.
        let mut server = mockito::Server::new_async().await;
        // Every getUpdates returns 401. expect_at_least(1) lets us assert the
        // loop polled at least once; the breaker means it must NOT keep
        // polling indefinitely — verified by the loop's JoinHandle completing.
        let _m_401 = server
            .mock("GET", "/bot111:TOKEN/getUpdates")
            // getUpdates carries ?offset&timeout&allowed_updates; without an
            // explicit query matcher mockito won't match the query-bearing
            // request (it would 501 → transient → backoff → never hit the auth
            // arm). Match any query so the 401 is what the loop actually sees.
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#)
            .expect_at_least(1)
            .create_async()
            .await;

        let http = wcore_egress::EgressClient::new();
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let (_tx, rx) = watch::channel(false);
        let args = LongPollArgs {
            http,
            api_base: server.url(),
            bot_token: "111:TOKEN".to_string(),
            channel_name: "auth-test".to_string(),
            timeout_secs: 0,
            allowed_chat_ids: HashSet::new(),
            inbox: Arc::clone(&inbox),
            shutdown: rx,
        };

        // The loop must return on its own (no shutdown signal sent). If the
        // breaker were missing it would back off and retry forever, so the
        // timeout would fire and the test would fail.
        let handle = tokio::spawn(longpoll_loop(args));
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("loop must terminate on auth failure, not retry forever")
            .expect("loop task should not panic");

        let guard = inbox.lock().await;
        let saw_auth_error = guard.iter().any(|e| {
            matches!(
                e,
                ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::AuthError
                }
            )
        });
        assert!(
            saw_auth_error,
            "loop must surface an AuthError state-change so supervision sees the dead channel"
        );
    }

    #[tokio::test]
    async fn caption_only_media_message_yields_nonempty_text() {
        // A captioned photo arrives with `text` absent and the words in
        // `caption`. ingest_updates must surface the caption as the message
        // body — otherwise the engine receives an empty turn.
        let update: Update = serde_json::from_str(
            r#"{"update_id":10,"message":{"message_id":1,"chat":{"id":1},"photo":[{"file_id":"f"}],"caption":"what is this?"}}"#,
        )
        .expect("valid Update JSON");
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let mut offset = 0;
        ingest_updates(vec![update], &HashSet::new(), &inbox, &mut offset).await;

        let guard = inbox.lock().await;
        let event = guard.front().expect("one event ingested");
        match event {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "what is this?");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}
