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
    ChannelEvent, ChatType, ConnectionState, IncomingMessage, MessageReceipt,
};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::CredentialsStore;

use crate::applescript::{build_send_script, run_osascript};
use crate::config::IMessageConfig;
use crate::db::{apple_ns_to_unix_secs, chat_db_path, fetch_new_messages, max_rowid};

const SEND_QUEUE_MAX: usize = 50;
const OSASCRIPT_TIMEOUT_MS: u64 = 15_000;

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

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.poll_handle.is_some() {
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

        let result = do_send(&msg.conversation_id, &msg.text).await;

        {
            *self.send_queue_depth.lock().await -= 1;
        }

        result.map_err(ChannelError::from).map(|_| MessageReceipt {
            id: format!("imessage-sent-{}", chrono::Utc::now().timestamp()),
            conversation_id: msg.conversation_id.clone(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/imessage.json")
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
                        // No attachment paths, reply guids, display names, or group names
                        // are present in the chat.db row — leave at defaults.
                        ..Default::default()
                    };

                    inbox
                        .lock()
                        .await
                        .push_back(ChannelEvent::MessageReceived { msg });
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
