//! `IMessageChannel` — production iMessage `Channel` impl.
//!
//! - Inbound: polls chat.db every `poll_interval_ms` milliseconds. A Tokio
//!   task runs the poll loop and pushes events into `inbox`.
//! - Outbound: serialises osascript sends through a single async chain so
//!   concurrent calls don't interleave AppleScript invocations (Messages.app
//!   is not re-entrant).

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{
    Attachment, ChannelEvent, ChatType, ConnectionState, IncomingMessage, MediaKind, MessageReceipt,
};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::CredentialsStore;

use crate::applescript::{build_send_script, run_osascript};
use crate::config::IMessageConfig;
use crate::db::{
    apple_ns_to_unix_secs, chat_db_path, fetch_new_messages, fetch_outgoing_since,
    match_outgoing_guid, max_rowid,
};

const SEND_QUEUE_MAX: usize = 50;
const OSASCRIPT_TIMEOUT_MS: u64 = 15_000;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and [`MAX_ATTACHMENT_BYTES`] is
/// derived from it. One constant, both sites, so the advertised number and the
/// enforced number cannot drift apart.
///
/// This adapter previously declared NOTHING, so it advertised the 25 MiB trait
/// default while enforcing a hardcoded 64 MiB, because the declaration had no
/// reader anywhere in the workspace. 64 MiB is the value that has actually
/// governed inbound reads since 2026-06-12.
pub const MEDIA_BOUNDS: wcore_channels::MediaBounds = wcore_channels::MediaBounds {
    max_bytes: 64 * 1024 * 1024,
    max_attachments: 10,
};

/// Upper bound on a single attachment read in `fetch_media`. iMessage media is
/// already on local disk (no network fetch), but a multi-GB video must not be
/// slurped into memory; the enricher only needs bytes for vision/transcription.
const MAX_ATTACHMENT_BYTES: u64 = MEDIA_BOUNDS.max_bytes;

/// Coarse [`MediaKind`] for a local attachment path, keyed off its extension.
/// Used only to label inbound attachments so the host enricher routes them
/// (image → vision, audio/voice → transcription). Unknown → `Other`.
fn media_kind_for_path(path: &str) -> MediaKind {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "tiff" | "bmp" => {
            MediaKind::Image
        }
        "mov" | "mp4" | "m4v" | "avi" | "mkv" | "webm" => MediaKind::Video,
        "caf" | "m4a" | "mp3" | "aac" | "wav" | "amr" | "aiff" => MediaKind::Audio,
        "pdf" | "doc" | "docx" | "txt" | "rtf" | "pages" | "key" | "numbers" | "zip" => {
            MediaKind::Document
        }
        _ => MediaKind::Other,
    }
}

/// Build inbound [`Attachment`]s from on-disk chat.db attachment paths. The
/// path is local (already downloaded by Messages.app), so there is no URL and
/// no network/SSRF surface — `fetch_media` reads the bytes straight off disk.
fn attachments_from_paths(paths: &[String]) -> Vec<Attachment> {
    paths
        .iter()
        .map(|p| Attachment {
            url: String::new(),
            path: Some(p.clone()),
            content_type: None,
            kind: media_kind_for_path(p),
            transcribed: None,
        })
        .collect()
}

// Budget for resolving a just-sent message's real GUID from chat.db. The send
// is committed by AppleScript before the row lands in SQLite, so we poll a few
// times. Kept short so send_message stays responsive; on miss we fall back to a
// clearly-named synthetic pending id (see `resolve_sent_guid`).
const GUID_LOOKUP_ATTEMPTS: usize = 10;
const GUID_LOOKUP_INTERVAL_MS: u64 = 100;

pub struct IMessageChannel {
    name: String,
    config: IMessageConfig,
    state: ConnectionState,
    allowed_handles: Option<HashSet<String>>,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    poll_handle: Option<JoinHandle<()>>,
    shutdown: Option<watch::Sender<bool>>,
    send_queue_depth: Arc<Mutex<usize>>,
    // Not used — iMessage has no token-based auth; kept for trait consistency.
    _creds: Arc<dyn CredentialsStore>,
}

impl IMessageChannel {
    pub fn new(
        name: impl Into<String>,
        config: IMessageConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        let allowed_handles: Option<HashSet<String>> = if config.allowed_handles.is_empty() {
            None
        } else {
            Some(
                config
                    .allowed_handles
                    .iter()
                    .map(|h| h.to_lowercase())
                    .collect(),
            )
        };

        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            allowed_handles,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            poll_handle: None,
            shutdown: None,
            send_queue_depth: Arc::new(Mutex::new(0)),
            _creds: creds,
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }
}

#[async_trait]
impl Channel for IMessageChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "imessage"
    }

    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        self.poll_handle.as_ref()
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.as_ref().is_some_and(|h| !h.is_finished()) {
            // Already running — idempotent. A finished handle (the chat.db poll
            // task died) falls through to respawn so supervised reconnect heals
            // the channel instead of treating a dead task as alive.
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        let db_path = chat_db_path();

        // Seed cursor to current max rowid so we only pick up NEW messages.
        let seed = max_rowid(db_path.clone())
            .await
            .map_err(ChannelError::from)?;

        let (tx, rx) = watch::channel(false);
        let interval_ms = self.config.clamped_poll_interval_ms();
        let inbox = Arc::clone(&self.inbox);
        let allowed = self.allowed_handles.clone();

        let handle = tokio::spawn(async move {
            poll_loop(db_path, seed, interval_ms, inbox, allowed, rx).await;
        });

        self.poll_handle = Some(handle);
        self.shutdown = Some(tx);
        self.state = ConnectionState::Connected;

        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.is_none() {
            return Ok(());
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.poll_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), handle).await;
        }
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        {
            let depth = self.send_queue_depth.lock().await;
            if *depth >= SEND_QUEUE_MAX {
                return Err(ChannelError::Rejected(format!(
                    "iMessage send queue full ({SEND_QUEUE_MAX} in-flight)"
                )));
            }
        }
        {
            *self.send_queue_depth.lock().await += 1;
        }

        let db_path = chat_db_path();
        // Snapshot the rowid cursor BEFORE sending so the post-send lookup only
        // considers messages produced by this send. A failure to read the
        // cursor (e.g. Full Disk Access not granted) is non-fatal: we still
        // send, then fall back to a synthetic pending id.
        let pre_send_rowid = max_rowid(db_path.clone()).await.ok();

        let result = do_send(&msg.conversation_id, &msg.text).await;

        {
            *self.send_queue_depth.lock().await -= 1;
        }

        result.map_err(ChannelError::from)?;

        let ts_secs = chrono::Utc::now().timestamp();
        let id = resolve_sent_guid(
            db_path,
            pre_send_rowid,
            &msg.conversation_id,
            &msg.text,
            ts_secs,
        )
        .await;

        Ok(MessageReceipt {
            id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs,
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/imessage.json")
    }

    /// iMessage: **permanently absent, measured against the scripting
    /// dictionary rather than assumed.**
    ///
    /// Messages.app on macOS 13+ lets a *human* edit and unsend a message. That
    /// is a UI affordance, and this adapter's only outbound path is
    /// [`applescript::run_osascript`] — so what matters is whether the
    /// capability is exposed to AppleScript. It is not, and the measurement is
    /// unusually clean because Messages.app's dictionary is tiny:
    ///
    /// ```text
    /// $ sdef /System/Applications/Messages.app | grep -o '<command name="[^"]*"' | sort -u
    /// <command name="login"
    /// <command name="logout"
    /// <command name="send"
    /// ```
    ///
    /// Three commands, total. Measured on macOS 26.3 (build 25D125),
    /// Messages.app 26.0, 2026-07-30, with `send` as the known-positive
    /// (1 hit — the instrument was alive) and
    /// `edit|unsend|delete|redact|recall|remove` returning **0**.
    ///
    /// The stronger fact is the class list: `account`, `chat`, `file transfer`,
    /// `participant`. **There is no `message` class at all.** So there is not
    /// merely no delete verb — there is no scriptable object representing a sent
    /// message to address one to. That is what makes this `PlatformHasNoApi`
    /// rather than a backlog item.
    ///
    /// Reactions (tapbacks) and the typing indicator are absent from the
    /// dictionary for the same reason.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::PlatformHasNoApi;
        wcore_channels::NativeActions::none()
            .edit(PlatformHasNoApi)
            .delete(PlatformHasNoApi)
            .react(PlatformHasNoApi)
            .typing(PlatformHasNoApi)
            .note(
                "Messages.app's entire AppleScript dictionary is login/logout/send with no \
                 `message` class — there is no scriptable handle for a sent message. \
                 Measured via sdef on macOS 26.3 / Messages 26.0.",
            )
    }

    /// Read an inbound attachment's bytes off local disk. iMessage attachments
    /// are written under `~/Library/Messages/Attachments` by Messages.app, so
    /// the path is already local — there is no URL to fetch and no SSRF surface.
    /// Bounded by [`MAX_ATTACHMENT_BYTES`] to avoid slurping a huge video into
    /// memory.
    async fn fetch_media(&self, attachment: &Attachment) -> Result<Vec<u8>, ChannelError> {
        let Some(path) = attachment.path.clone() else {
            return Err(ChannelError::Rejected(
                "iMessage attachment has no local path".to_string(),
            ));
        };

        tokio::task::spawn_blocking(move || {
            let meta = std::fs::metadata(&path)
                .map_err(|e| ChannelError::Transport(format!("stat attachment {path}: {e}")))?;
            if meta.len() > MAX_ATTACHMENT_BYTES {
                return Err(ChannelError::Rejected(format!(
                    "iMessage attachment {} exceeds {} byte cap",
                    meta.len(),
                    MAX_ATTACHMENT_BYTES
                )));
            }
            std::fs::read(&path)
                .map_err(|e| ChannelError::Transport(format!("read attachment {path}: {e}")))
        })
        .await
        .map_err(|e| ChannelError::Transport(format!("attachment read task panic: {e}")))?
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], from which
    /// [`MAX_ATTACHMENT_BYTES`] is derived.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }
}

// ---------------------------------------------------------------------------
// Poll loop (background task)
// ---------------------------------------------------------------------------

async fn poll_loop(
    db_path: std::path::PathBuf,
    seed_rowid: i64,
    interval_ms: u64,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    allowed: Option<HashSet<String>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut last_rowid = seed_rowid;
    let interval = std::time::Duration::from_millis(interval_ms);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {},
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }

        match fetch_new_messages(db_path.clone(), last_rowid).await {
            Ok(rows) => {
                for row in rows {
                    if row.rowid > last_rowid {
                        last_rowid = row.rowid;
                    }

                    if let Some(ref set) = allowed
                        && !set.contains(&row.sender_handle.to_lowercase())
                    {
                        continue;
                    }

                    let conversation_id = if row.chat_guid.is_empty() {
                        row.sender_handle.clone()
                    } else {
                        row.chat_guid.clone()
                    };
                    let msg = IncomingMessage {
                        id: row.rowid.to_string(),
                        conversation_id,
                        // sender_handle is the phone/email handle from chat.db — the
                        // stable identity key for access control and dedup.
                        sender_id: row.sender_handle.clone(),
                        author: row.sender_handle.clone(),
                        sender_handle: Some(row.sender_handle.clone()),
                        text: row.text,
                        ts_secs: apple_ns_to_unix_secs(row.ts_apple_ns),
                        // SQL query filters is_from_me = 0; these are never self-sent.
                        is_self: false,
                        // is_group is derived in SQL: c.style=43 OR chat_identifier LIKE 'chat%'
                        chat_type: if row.is_group {
                            ChatType::Group
                        } else {
                            ChatType::Direct
                        },
                        platform: Some("imessage".into()),
                        // Local on-disk attachment paths from the chat.db join;
                        // empty for text-only messages.
                        attachments: attachments_from_paths(&row.attachment_paths),
                        // No reply guids, display names, or group names are
                        // present in the chat.db row — leave at defaults.
                        ..Default::default()
                    };

                    // F9 — bounded, drop-oldest inbox against a flood.
                    let mut guard = inbox.lock().await;
                    wcore_channels::push_bounded(&mut guard, ChannelEvent::MessageReceived { msg });
                }
            }
            Err(e) => {
                tracing::warn!(
                    target: "wcore_channel_imessage",
                    error = %e,
                    "iMessage poll error; will retry"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Send helper
// ---------------------------------------------------------------------------

async fn do_send(chat_id: &str, text: &str) -> Result<(), crate::error::IMessageError> {
    let script = build_send_script(chat_id, text, None);
    run_osascript(&script, OSASCRIPT_TIMEOUT_MS).await?;
    Ok(())
}

/// Resolve the receipt id for a just-sent message.
///
/// AppleScript's `send` returns no message id, so the real `message.guid` must
/// be read back from chat.db. The outgoing row is written asynchronously by
/// Messages.app, so we poll briefly (`GUID_LOOKUP_ATTEMPTS` ×
/// `GUID_LOOKUP_INTERVAL_MS`) for an outgoing row newer than `pre_send_rowid`
/// whose text matches what we sent, and return its GUID. This GUID is the
/// stable cross-event key, so a later inbound echo or read receipt for the same
/// message correlates with this receipt for dedup.
///
/// Fallback: if the cursor could not be read (Full Disk Access not granted) or
/// the row has not landed within the budget, return a clearly-named synthetic
/// `imessage-pending-<unix>` id. It is deliberately NOT shaped like a real GUID
/// so callers can tell a resolved receipt from an unresolved one.
async fn resolve_sent_guid(
    db_path: std::path::PathBuf,
    pre_send_rowid: Option<i64>,
    chat_id: &str,
    sent_text: &str,
    ts_secs: i64,
) -> String {
    let fallback = || format!("imessage-pending-{ts_secs}");

    let Some(since_rowid) = pre_send_rowid else {
        return fallback();
    };

    for attempt in 0..GUID_LOOKUP_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(GUID_LOOKUP_INTERVAL_MS)).await;
        }
        match fetch_outgoing_since(db_path.clone(), since_rowid, chat_id.to_string()).await {
            Ok(rows) => {
                if let Some(guid) = match_outgoing_guid(&rows, sent_text) {
                    return guid;
                }
            }
            Err(e) => {
                // Read-back is best-effort; the message was already sent. Log
                // once and keep retrying within the budget.
                tracing::debug!(
                    target: "wcore_channel_imessage",
                    error = %e,
                    "iMessage GUID read-back error; will retry"
                );
            }
        }
    }

    tracing::debug!(
        target: "wcore_channel_imessage",
        "iMessage GUID not resolved within budget; returning synthetic pending id"
    );
    fallback()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::credentials::{CredentialsError, CredentialsStore as CredsTrait};

    struct NoopCreds;
    impl CredsTrait for NoopCreds {
        fn get(&self, _k: &str) -> Result<Option<String>, CredentialsError> {
            Ok(None)
        }
        fn put(&self, _k: &str, _v: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
        fn delete(&self, _k: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
    }
    fn noop_creds() -> Arc<dyn CredsTrait> {
        Arc::new(NoopCreds)
    }

    fn default_config() -> IMessageConfig {
        toml::from_str("").unwrap()
    }

    // 1. Config round-trip: parse a TOML options table → IMessageConfig.
    #[test]
    fn config_parses_from_toml_options() {
        let raw = r#"
poll_interval_ms = 3000
allowed_handles = ["+15555550100"]
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(&format!(
            "name=\"test\"\nplatform=\"imessage\"\n[options]\n{}",
            raw
        ))
        .unwrap();
        let cfg: IMessageConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.poll_interval_ms, 3_000);
        assert_eq!(cfg.allowed_handles, vec!["+15555550100"]);
    }

    // 2. Message serde: an IMessageChannel has platform() == "imessage".
    #[test]
    fn platform_tag_is_imessage() {
        let ch = IMessageChannel::new("test", default_config(), noop_creds());
        assert_eq!(ch.platform(), "imessage");
    }

    // Media-kind classification by extension (drives enricher routing).
    #[test]
    fn media_kind_classifies_by_extension() {
        assert_eq!(media_kind_for_path("/x/IMG_1.HEIC"), MediaKind::Image);
        assert_eq!(media_kind_for_path("/x/clip.mov"), MediaKind::Video);
        assert_eq!(media_kind_for_path("/x/voice.caf"), MediaKind::Audio);
        assert_eq!(media_kind_for_path("/x/doc.pdf"), MediaKind::Document);
        assert_eq!(media_kind_for_path("/x/unknown.bin"), MediaKind::Other);
        assert_eq!(media_kind_for_path("/x/noext"), MediaKind::Other);
    }

    // attachments_from_paths: local path set, url empty (no network), kind set.
    #[test]
    fn attachments_from_paths_builds_local_attachments() {
        let atts = attachments_from_paths(&["/a/p.png".to_string(), "/a/v.mp4".to_string()]);
        assert_eq!(atts.len(), 2);
        assert_eq!(atts[0].path.as_deref(), Some("/a/p.png"));
        assert!(atts[0].url.is_empty(), "local attachment carries no URL");
        assert_eq!(atts[0].kind, MediaKind::Image);
        assert_eq!(atts[1].kind, MediaKind::Video);
    }

    // fetch_media reads the bytes straight off the local path.
    #[tokio::test]
    async fn fetch_media_reads_local_bytes() {
        let mut dir = std::env::temp_dir();
        dir.push("wcore_imsg_fetch_media_reads.bin");
        std::fs::write(&dir, b"IMSGBYTES").unwrap();

        let ch = IMessageChannel::new("t", default_config(), noop_creds());
        let att = Attachment {
            path: Some(dir.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let bytes = ch.fetch_media(&att).await.unwrap();
        assert_eq!(bytes, b"IMSGBYTES");

        let _ = std::fs::remove_file(&dir);
    }

    // fetch_media rejects an attachment with no resolvable local path.
    #[tokio::test]
    async fn fetch_media_rejects_missing_path() {
        let ch = IMessageChannel::new("t", default_config(), noop_creds());
        let att = Attachment::default(); // path: None
        let err = ch.fetch_media(&att).await.unwrap_err();
        assert!(matches!(err, ChannelError::Rejected(_)));
    }

    // 3. Error-mapping: IMessageError variants map to the correct ChannelError.
    #[test]
    fn error_mapping_smoke() {
        let err: ChannelError = crate::error::IMessageError::AutomationDenied.into();
        assert!(matches!(err, ChannelError::Auth(_)));

        let err2: ChannelError = crate::error::IMessageError::ChatNotFound.into();
        assert!(matches!(err2, ChannelError::Rejected(_)));

        let err3: ChannelError = crate::error::IMessageError::AppleScript {
            exit_code: 1,
            stderr: "x".into(),
        }
        .into();
        assert!(matches!(err3, ChannelError::Transport(_)));
    }
}
