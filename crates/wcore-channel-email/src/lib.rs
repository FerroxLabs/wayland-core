//! `wcore-channel-email` — production email adapter for Wayland-Core.
//!
//! Outbound goes through SMTP via `lettre` (rustls-tls); inbound polls
//! IMAP via the sync `imap` crate run on `tokio::task::spawn_blocking`.
//! Credentials live in the OS keychain via `wcore-config::credentials`
//! and are resolved at `start()`; the TOML config carries only the
//! credential-handle keys.
//!
//! Shape mirrors `wcore-channel-telegram` deliberately — same lifecycle,
//! same inbox queue + shutdown watch, same retry policy on outbound.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_config::credentials::CredentialsStore;

pub use crate::config::{EmailConfig, ImapConfig, MailSecurity, SmtpConfig, is_loopback_host};
pub use crate::error::EmailError;
pub use crate::smtp::{LettreSender, MailSender, SendError};

pub mod config;
pub mod error;
mod imap;
mod sent_index;
pub mod smtp;
mod uid_store;

/// The single source of this adapter's inbound media bounds.
///
/// [`Channel::media_bounds`] returns this, and the IMAP parser's
/// `MAX_INLINE_ATTACHMENT_BYTES` is derived from it. One constant, both sites,
/// so the advertised number and the enforced number cannot drift apart.
///
/// Email is the adapter whose divergence ran the OTHER way: it advertised
/// 10 MiB while the parser has only ever inlined parts up to 2 MiB (since
/// 2026-06-12), so it under-delivered against its own promise by 5x. The
/// enforced 2 MiB is retained and now declared, because for email the parser's
/// inline ceiling IS the intake bound — attachments arrive base64-inlined in
/// the message body rather than being fetched over the network, and a part
/// above the ceiling never becomes fetchable bytes at all. Declaring 10 MiB
/// would advertise an intake this adapter has never performed.
///
/// `max_attachments` stays at email's own 20 (a mail message carries more
/// parts than a chat message), and is enforced by the inbound media enricher.
pub const MEDIA_BOUNDS: wcore_channels::MediaBounds = wcore_channels::MediaBounds {
    max_bytes: 2 * 1024 * 1024,
    max_attachments: 20,
};

/// Production email channel adapter.
pub struct EmailChannel {
    name: String,
    config: EmailConfig,
    state: ConnectionState,
    /// SMTP sender. `None` until `start()` resolves credentials and
    /// constructs the transport. Boxed so tests can swap in a mock
    /// sender via [`EmailChannel::with_sender`].
    sender: Option<Arc<dyn MailSender>>,
    /// Inbound queue. The blocking IMAP task pushes here; `poll_events`
    /// drains it.
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    /// Background IMAP poll task handle (only set when imap config is
    /// present and `start()` succeeded).
    poll_handle: Option<JoinHandle<()>>,
    shutdown: Option<watch::Sender<bool>>,
    /// Monotonic high-water UID for IMAP. Shared with the blocking poll
    /// task; std Mutex because the task is sync.
    last_seen_uid: Arc<StdMutex<u32>>,
    /// Reply-threading index: inbound RFC Message-ID -> threading context.
    /// The IMAP poll task records entries; `send_message` reads them to set
    /// In-Reply-To / References / Re: subject on outbound replies. Shared
    /// `std::Mutex` because the poll task is synchronous.
    reply_index: crate::imap::ReplyIndex,
    /// Outbound Message-ID index: `send_message` records every id it
    /// stamps; the IMAP poll task marks a matching inbound `is_self` so the
    /// dispatch kernel's loop guard drops the channel's own echoed mail
    /// (wayland#547). Shared `std::Mutex` because the poll task is sync.
    sent_ids: crate::sent_index::SentIdIndex,
    /// Credentials store used to resolve SMTP+IMAP creds at `start()`.
    creds: Arc<dyn CredentialsStore>,
    /// Optional test override — when set, `start()` reuses this sender
    /// instead of building a `LettreSender`. Boxed `dyn` so the override
    /// type is opaque.
    sender_override: Option<Arc<dyn MailSender>>,
}

impl EmailChannel {
    /// Construct an email channel bound to the production lettre
    /// transport.
    pub fn new(
        name: impl Into<String>,
        config: EmailConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            sender: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            poll_handle: None,
            shutdown: None,
            last_seen_uid: Arc::new(StdMutex::new(0)),
            reply_index: Arc::new(StdMutex::new(HashMap::new())),
            sent_ids: crate::sent_index::new_index(),
            creds,
            sender_override: None,
        }
    }

    /// Test-only constructor that overrides the SMTP sender.
    #[doc(hidden)]
    pub fn with_sender(
        name: impl Into<String>,
        config: EmailConfig,
        creds: Arc<dyn CredentialsStore>,
        sender: Arc<dyn MailSender>,
    ) -> Self {
        let mut me = Self::new(name, config, creds);
        me.sender_override = Some(sender);
        me
    }

    /// Current connection state. Mostly useful for tests.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Current IMAP high-water UID (monotonic). Test-visible.
    pub fn last_seen_uid(&self) -> u32 {
        *self.last_seen_uid.lock().unwrap()
    }

    /// Resolve IMAP creds and spawn the blocking poll task, storing its handle
    /// and shutdown sender. No-op when IMAP is not configured. Called from
    /// `start()` for the cold-start path and from the reconnect path when the
    /// previous poll task died (its finished handle + stale shutdown sender are
    /// overwritten here). Sync: cred lookups and `spawn_blocking` don't await.
    fn respawn_imap_poll(&mut self) -> Result<(), ChannelError> {
        let Some(imap_cfg) = self.config.imap.clone() else {
            return Ok(());
        };
        // Only resolve IMAP creds when imap is configured.
        let imap_user = self
            .creds
            .get(&imap_cfg.user_credential_handle)
            .map_err(|e| ChannelError::Auth(format!("imap user lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "imap user not found at credential_handle {:?}",
                    imap_cfg.user_credential_handle
                ))
            })?;
        let imap_pass = self
            .creds
            .get(&imap_cfg.password_credential_handle)
            .map_err(|e| ChannelError::Auth(format!("imap password lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "imap password not found at credential_handle {:?}",
                    imap_cfg.password_credential_handle
                ))
            })?;

        let own_addresses = own_address_set(&self.config.from_address, &imap_user);

        let (tx, rx) = watch::channel(false);
        let args = crate::imap::ImapPollArgs {
            host: imap_cfg.host,
            port: imap_cfg.port,
            security: imap_cfg.security,
            user: imap_user,
            pass: imap_pass,
            mailbox: imap_cfg.mailbox,
            allowed_senders: imap_cfg.allowed_senders,
            own_addresses,
            sent_ids: Arc::clone(&self.sent_ids),
            poll_interval_secs: imap_cfg.poll_interval_secs,
            inbox: Arc::clone(&self.inbox),
            last_seen_uid: Arc::clone(&self.last_seen_uid),
            reply_index: Arc::clone(&self.reply_index),
            shutdown: rx,
            runtime_handle: tokio::runtime::Handle::current(),
        };
        // `spawn_blocking` returns JoinHandle<()> directly when the
        // closure returns ().
        let handle = tokio::task::spawn_blocking(move || crate::imap::imap_poll_blocking(args));
        self.poll_handle = Some(handle);
        self.shutdown = Some(tx);
        Ok(())
    }
}

/// Own-address set for the inbound self-mail guard (wayland#547): the
/// configured From address, plus the IMAP account when it is address-shaped
/// (some servers use bare usernames — skip those). Normalized (bare
/// addr-spec, lowercased) and deduplicated.
fn own_address_set(from_address: &str, imap_user: &str) -> Vec<String> {
    let mut own = vec![crate::imap::normalize_from_addr(from_address)];
    if imap_user.contains('@') {
        let normalized = crate::imap::normalize_from_addr(imap_user);
        if !own.contains(&normalized) {
            own.push(normalized);
        }
    }
    own
}

#[async_trait]
impl Channel for EmailChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "email"
    }

    fn task_handle(&self) -> Option<&tokio::task::JoinHandle<()>> {
        self.poll_handle.as_ref()
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        // Idempotent only when fully healthy: the SMTP sender is built AND, if
        // IMAP is configured, its background poll task is still running. If the
        // IMAP task died while `sender` stayed `Some`, fall through to respawn
        // JUST the poll task (the sender is left intact) so supervised reconnect
        // heals the channel instead of treating a dead poll task as alive.
        let imap_configured = self.config.imap.is_some();
        let poll_alive = self.poll_handle.as_ref().is_some_and(|h| !h.is_finished());
        if self.sender.is_some() && (!imap_configured || poll_alive) {
            return Ok(());
        }

        // Sender already built (a dead IMAP task brought us here): respawn only
        // the poll task without re-resolving SMTP creds or rebuilding the
        // transport. The sender stays as-is.
        if self.sender.is_some() {
            self.respawn_imap_poll()?;
            self.inbox
                .lock()
                .await
                .push_back(ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Connected,
                });
            self.state = ConnectionState::Connected;
            return Ok(());
        }

        self.state = ConnectionState::Connecting;

        // Resolve SMTP creds.
        let smtp_user = self
            .creds
            .get(&self.config.smtp.user_credential_handle)
            .map_err(|e| ChannelError::Auth(format!("smtp user lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "smtp user not found at credential_handle {:?}",
                    self.config.smtp.user_credential_handle
                ))
            })?;
        let smtp_pass = self
            .creds
            .get(&self.config.smtp.password_credential_handle)
            .map_err(|e| ChannelError::Auth(format!("smtp password lookup: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "smtp password not found at credential_handle {:?}",
                    self.config.smtp.password_credential_handle
                ))
            })?;

        // Build sender (or use the test override).
        let sender: Arc<dyn MailSender> = if let Some(ref s) = self.sender_override {
            Arc::clone(s)
        } else {
            Arc::new(
                LettreSender::new(
                    &self.config.smtp.host,
                    self.config.smtp.port,
                    smtp_user.clone(),
                    smtp_pass,
                    self.config.smtp.tls_root_cert_path.as_deref(),
                    self.config.smtp.security,
                )
                .map_err(ChannelError::from)?,
            )
        };
        self.sender = Some(sender);

        // Push the Connected state-change so subscribers know we're live.
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected,
            });

        // If imap config is set, spawn the blocking poll loop.
        self.respawn_imap_poll()?;

        self.state = ConnectionState::Connected;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if self.sender.is_none() && self.poll_handle.is_none() {
            return Ok(());
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(true);
        }
        if let Some(handle) = self.poll_handle.take() {
            // Give the blocking poll loop up to 2s to observe shutdown.
            let abort = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
            if abort.is_err() {
                tracing::warn!(
                    target: "wcore_channel_email",
                    channel = %self.name,
                    "imap poll task did not exit within shutdown grace; aborted"
                );
            }
        }
        self.sender = None;
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
        let sender = self.sender.clone().ok_or(ChannelError::NotStarted)?;

        // Reply threading: when the outbound names a reply target, look up
        // the inbound message's threading context (Message-ID + Subject +
        // References) so we can set In-Reply-To / References / Re: subject.
        // If the id is unknown (e.g. the index was cleared), fall back to a
        // single-id chain built from the reply_to id itself — still a valid,
        // correctly-threaded reply, just without the original Subject.
        let reply_ctx = msg.reply_to.as_ref().map(|rid| {
            self.reply_index
                .lock()
                .ok()
                .and_then(|idx| idx.get(rid).cloned())
                .unwrap_or_else(|| crate::smtp::ReplyContext {
                    message_id: rid.clone(),
                    subject: None,
                    references: None,
                })
        });

        // Resolve any attachments to embeddable bytes (local path or data URL;
        // remote URLs are rejected loudly — see resolve_attachment). A bad
        // reference fails the send rather than silently dropping the file.
        let resolved: Vec<crate::smtp::OutboundAttachment> = msg
            .attachments
            .iter()
            .map(|a| crate::smtp::resolve_attachment(a))
            .collect::<Result<_, _>>()
            .map_err(ChannelError::from)?;

        let envelope = crate::smtp::build_message(
            &self.config.from_address,
            &msg.conversation_id,
            &msg.text,
            reply_ctx.as_ref(),
            None,
            &resolved,
        )
        .map_err(ChannelError::from)?;

        // Loop guard (wayland#547): remember the outbound Message-ID BEFORE
        // the wire send so the IMAP poll task can never race ahead of the
        // recording — by the time the mail can possibly be delivered (and
        // echo back into a monitored mailbox) the id is already indexed. A
        // failed send leaves a harmless stale entry (that id never reaches
        // the wild).
        if let Some(id) = crate::smtp::outbound_message_id(&envelope)
            && let Ok(mut idx) = self.sent_ids.lock()
        {
            idx.record(id);
        }

        let response = crate::smtp::send_with_retry(sender, envelope)
            .await
            .map_err(ChannelError::from)?;
        Ok(MessageReceipt {
            id: crate::smtp::response_message_id(&response),
            conversation_id: msg.conversation_id,
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/email.json")
    }

    /// SMTP: **all four are permanently absent, by the shape of the protocol.**
    ///
    /// Once a remote MTA has returned `250` for `DATA`, the message is that
    /// MTA's property. SMTP defines no verb to alter or withdraw it, and there
    /// is no addressable handle for the copy sitting in the recipient's
    /// mailbox. ("Recall" in Exchange is an Exchange-internal courtesy between
    /// mailboxes on the same organisation, not an SMTP capability, and it fails
    /// silently the moment the recipient is elsewhere or has read the mail.)
    ///
    /// This adapter also runs an IMAP poll loop, and IMAP *can* delete — but
    /// only from **our own** mailbox. Deleting our copy of a sent message is
    /// not deleting the message; reporting it as
    /// [`Channel::delete_message`] would be the silent lie that method's
    /// documentation exists to forbid.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::PlatformHasNoApi;
        wcore_channels::NativeActions::none()
            .edit(PlatformHasNoApi)
            .delete(PlatformHasNoApi)
            .react(PlatformHasNoApi)
            .typing(PlatformHasNoApi)
            .note(
                "SMTP defines no verb to alter or recall a message a remote MTA has \
                 accepted, and no handle for the recipient's copy. IMAP can delete only \
                 OUR copy, which is not the same operation.",
            )
    }

    /// Setup and authentication probe — reference implementation for the
    /// POLLING half of the Phase 24 channel matrix.
    ///
    /// The two reference adapters are deliberately different SHAPES, not two
    /// spellings of the same shape. Discord's probe is one HTTP round trip
    /// against a live persistent-connection API. This one has to resolve four
    /// credential handles across two protocols and then open a real IMAP
    /// session, so it exercises the parts of the contract a single-credential
    /// HTTP adapter never touches: partial configuration, and an
    /// authentication answer that costs a blocking socket.
    ///
    /// # Which credential is checked, and the gap this leaves
    ///
    /// The identity a mailbox authenticates as is the IMAP account, so IMAP is
    /// what is logged into. SMTP's credentials are checked for PRESENCE only —
    /// a probe that also sent through SMTP would be sending a message, which
    /// this surface exists to avoid. An SMTP credential that is present but
    /// wrong therefore passes this probe and fails at first send. That is a
    /// stated gap, not an oversight; closing it needs an SMTP `NOOP`-style
    /// handshake the sender abstraction does not currently expose.
    async fn probe(&self) -> Result<wcore_channels::ProbeReport, ChannelError> {
        use wcore_channels::ProbeReport;

        // ---- 1. Configuration completeness. Every finding names the KEY,
        // never a value.
        let mut missing: Vec<String> = Vec::new();
        if self.config.from_address.trim().is_empty() {
            missing.push("from_address".to_string());
        }
        if self.config.smtp.host.trim().is_empty() {
            missing.push("smtp.host".to_string());
        }
        for (label, handle) in [
            (
                "smtp.user_credential_handle",
                &self.config.smtp.user_credential_handle,
            ),
            (
                "smtp.password_credential_handle",
                &self.config.smtp.password_credential_handle,
            ),
        ] {
            if handle.trim().is_empty() {
                missing.push(label.to_string());
            } else if matches!(self.creds.get(handle), Ok(None)) {
                missing.push(format!(
                    "{label} -> {handle:?} absent from credentials store"
                ));
            }
        }

        let Some(imap_cfg) = self.config.imap.clone() else {
            // No IMAP configured: this mailbox can send but never receives.
            // That is a complete, legal configuration for an outbound-only
            // channel, and it is NOT an authenticated one — there is no
            // credential this probe can exercise without sending mail, so the
            // honest verdict is Incomplete with the reason named, not Ok.
            missing.push(
                "imap (absent — outbound only; no credential can be verified without sending)"
                    .to_string(),
            );
            return Ok(ProbeReport::incomplete(&self.name, "email", missing));
        };

        let user = match self.creds.get(&imap_cfg.user_credential_handle) {
            Ok(Some(u)) => Some(u),
            Ok(None) => {
                missing.push(format!(
                    "imap.user_credential_handle -> {:?} absent from credentials store",
                    imap_cfg.user_credential_handle
                ));
                None
            }
            Err(e) => {
                missing.push(format!("credentials store unreadable: {e}"));
                None
            }
        };
        let pass = match self.creds.get(&imap_cfg.password_credential_handle) {
            Ok(Some(p)) => Some(p),
            Ok(None) => {
                missing.push(format!(
                    "imap.password_credential_handle -> {:?} absent from credentials store",
                    imap_cfg.password_credential_handle
                ));
                None
            }
            Err(e) => {
                missing.push(format!("credentials store unreadable: {e}"));
                None
            }
        };

        let (Some(user), Some(pass)) = (user, pass) else {
            return Ok(ProbeReport::incomplete(&self.name, "email", missing));
        };
        if !missing.is_empty() {
            // Configuration is incomplete somewhere else. Report that rather
            // than opening a socket: an operator fixes the missing key first,
            // and a probe that authenticates anyway just adds noise.
            return Ok(ProbeReport::incomplete(&self.name, "email", missing));
        }

        // ---- 2. Authentication. A real IMAP LOGIN, then an immediate LOGOUT.
        // Nothing is fetched and no mailbox state is touched, so running this
        // against a production mailbox is safe.
        let host = imap_cfg.host.clone();
        let port = imap_cfg.port;
        let mailbox = imap_cfg.mailbox.clone();
        let identity = user.clone();
        let outcome = tokio::task::spawn_blocking(move || -> Result<(), (bool, String)> {
            // `(is_auth_failure, reason)` — the bool is what separates
            // "rotate the password" from "the server was unreachable", and
            // collapsing it would make an operator rotate a working password
            // because a firewall was in the way.
            // Implicit TLS, unconditionally — unchanged by the move off
            // native-tls. The probe has always opened IMAPS here regardless of
            // `imap.security`; only the TLS implementation underneath changed.
            let client = crate::imap::connect_implicit_tls(host.as_str(), port)
                .map_err(|e| (false, e.to_string()))?;
            let mut session = client
                .login(&user, &pass)
                .map_err(|(e, _)| (true, format!("imap login rejected: {e}")))?;
            // SELECT proves the configured mailbox actually exists. A login
            // that succeeds against a mailbox name with a typo is a channel
            // that starts and then receives nothing, forever.
            let select = session
                .select(&mailbox)
                .map_err(|e| (false, format!("select {mailbox}: {e}")));
            let _ = session.logout();
            select.map(|_| ())
        })
        .await
        .map_err(|e| ChannelError::Other(format!("probe task panicked: {e}")))?;

        match outcome {
            Ok(()) => Ok(ProbeReport::ok(&self.name, "email", identity)),
            Err((true, reason)) => Ok(ProbeReport::unauthenticated(&self.name, "email", reason)),
            Err((false, reason)) => Ok(ProbeReport::unreachable(&self.name, "email", reason)),
        }
    }

    /// This adapter's inbound intake policy — see [`MEDIA_BOUNDS`], from which
    /// the IMAP parser's inline ceiling is derived.
    fn media_bounds(&self) -> wcore_channels::MediaBounds {
        MEDIA_BOUNDS
    }

    /// Return the bytes of an inbound email attachment. The IMAP parser already
    /// decoded each attachment part and inlined it as a `data:<mime>;base64,…`
    /// URL (bounded — oversize parts stay metadata-only), so there is no network
    /// fetch and no SSRF surface; this just decodes the inline payload.
    async fn fetch_media(
        &self,
        attachment: &wcore_channels::event::Attachment,
    ) -> Result<Vec<u8>, ChannelError> {
        let rest = attachment.url.strip_prefix("data:").ok_or_else(|| {
            ChannelError::Rejected("email attachment has no inline data".to_string())
        })?;
        let b64 = rest.split_once(";base64,").map(|(_, b)| b).ok_or_else(|| {
            ChannelError::Rejected("unsupported email attachment data URL".to_string())
        })?;
        Ok(crate::imap::decode_base64_bytes(b64))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smtp::SendError;
    use lettre::message::Message;
    use lettre::transport::smtp::response::Response;
    use std::str::FromStr;
    use std::sync::Mutex as StdMutex2;
    use wcore_config::credentials::CredentialsError;

    // ----- in-memory creds stub -----
    struct InMemoryCreds {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }
    impl InMemoryCreds {
        fn new() -> Self {
            Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            }
        }
        fn with(pairs: &[(&str, &str)]) -> Arc<dyn CredentialsStore> {
            let s = Self::new();
            for (k, v) in pairs {
                s.inner
                    .lock()
                    .unwrap()
                    .insert((*k).to_string(), (*v).to_string());
            }
            Arc::new(s)
        }
    }
    impl CredentialsStore for InMemoryCreds {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }

    // ----- recording mock sender (decoupled from smtp::tests so we can drive
    // it from EmailChannel-level tests) -----
    struct RecordingSender {
        sent: StdMutex2<Vec<Message>>,
        outcomes: StdMutex2<Vec<Result<Response, SendError>>>,
    }

    impl RecordingSender {
        fn new(outcomes: Vec<Result<Response, SendError>>) -> Arc<Self> {
            Arc::new(Self {
                sent: StdMutex2::new(Vec::new()),
                outcomes: StdMutex2::new(outcomes),
            })
        }

        fn ok(queue_id: &str) -> Result<Response, SendError> {
            Ok(Response::from_str(&format!("250 2.0.0 Ok: queued as {queue_id}\r\n")).unwrap())
        }
    }

    #[async_trait]
    impl MailSender for RecordingSender {
        async fn send(&self, msg: Message) -> Result<Response, SendError> {
            self.sent.lock().unwrap().push(msg);
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                return Err(SendError::Transient("no scripted outcomes".into()));
            }
            outcomes.remove(0)
        }
    }

    fn cfg_outbound_only() -> EmailConfig {
        EmailConfig {
            from_address: "bot@acme.com".to_string(),
            smtp: SmtpConfig {
                host: "smtp.example".to_string(),
                port: 587,
                user_credential_handle: "email.test.smtp_user".to_string(),
                password_credential_handle: "email.test.smtp_pass".to_string(),
                tls_root_cert_path: None,
                security: Default::default(),
            },
            imap: None,
        }
    }

    fn creds_for_outbound() -> Arc<dyn CredentialsStore> {
        InMemoryCreds::with(&[
            ("email.test.smtp_user", "user"),
            ("email.test.smtp_pass", "pass"),
        ])
    }

    #[tokio::test]
    async fn fetch_media_decodes_inline_data_url() {
        let ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            RecordingSender::new(vec![]),
        );
        let att = wcore_channels::event::Attachment {
            url: "data:image/png;base64,aGVsbG8=".to_string(),
            ..Default::default()
        };
        assert_eq!(ch.fetch_media(&att).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn fetch_media_rejects_attachment_without_inline_data() {
        let ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            RecordingSender::new(vec![]),
        );
        let att = wcore_channels::event::Attachment::default(); // empty url
        assert!(matches!(
            ch.fetch_media(&att).await.unwrap_err(),
            ChannelError::Rejected(_)
        ));
    }

    // -----------------------------------------------------------------
    // wayland#547 loop guard: the outbound Message-ID is stamped on the
    // wire message, recorded in the sent index, and an inbound echo of it
    // is marked `is_self` (so the dispatch kernel's loop guard drops it).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_message_records_outbound_id_and_marks_echo_self() {
        let sender = RecordingSender::new(vec![RecordingSender::ok("QID-9")]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();
        ch.send_message(OutgoingMessage::text("bot@acme.com", "note to self"))
            .await
            .unwrap();

        // The wire message carries an explicit stamped Message-ID…
        let stamped = {
            let sent = sender.sent.lock().unwrap();
            crate::smtp::outbound_message_id(&sent[0]).expect("outbound Message-ID stamped")
        };
        assert!(stamped.starts_with("wl-"), "stamped = {stamped}");
        assert!(stamped.ends_with("@acme.com"), "stamped = {stamped}");
        // …and was recorded in the sent index before the send.
        assert!(ch.sent_ids.lock().unwrap().contains(&stamped));

        // An inbound echo of that id — what the IMAP poll task sees when
        // the agent's mail lands back in the monitored inbox — is marked
        // is_self even with no own-address configured.
        let mut echo = wcore_channels::event::IncomingMessage::new(
            stamped.clone(),
            "bot@acme.com",
            "Bot <bot@acme.com>",
            "note to self",
            0,
        );
        let hit = crate::imap::mark_self_inbound(&mut echo, &[], &ch.sent_ids);
        assert_eq!(hit, Some(crate::imap::SelfMatch::MessageId));
        assert!(echo.is_self, "echoed outbound mail must be marked is_self");
    }

    // -----------------------------------------------------------------
    // wayland#547: own-address set construction (fed to the IMAP poll
    // task by respawn_imap_poll).
    // -----------------------------------------------------------------
    #[test]
    fn own_address_set_normalizes_and_includes_address_shaped_imap_user() {
        let own = own_address_set("Bot <BOT@Acme.com>", "shared@acme.com");
        assert_eq!(
            own,
            vec!["bot@acme.com".to_string(), "shared@acme.com".to_string()]
        );
    }

    #[test]
    fn own_address_set_skips_bare_username_and_dedups() {
        // Bare (non-address) IMAP usernames are skipped…
        assert_eq!(
            own_address_set("bot@acme.com", "bot"),
            vec!["bot@acme.com".to_string()]
        );
        // …and an IMAP user equal to the From address isn't doubled.
        assert_eq!(
            own_address_set("bot@acme.com", "BOT@ACME.COM"),
            vec!["bot@acme.com".to_string()]
        );
    }

    // -----------------------------------------------------------------
    // 1. send via abstracted MailSender records expected envelope.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_records_expected_envelope() {
        let sender = RecordingSender::new(vec![RecordingSender::ok("QID-1")]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();
        let receipt = ch
            .send_message(OutgoingMessage::text("ops@acme.com", "hello"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "QID-1");
        assert_eq!(receipt.conversation_id, "ops@acme.com");
        {
            let sent = sender.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let rfc = String::from_utf8_lossy(&sent[0].formatted()).to_string();
            assert!(rfc.contains("From: bot@acme.com"), "rfc = {rfc}");
            assert!(rfc.contains("To: ops@acme.com"), "rfc = {rfc}");
            assert!(rfc.contains("hello"), "rfc = {rfc}");
        }
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 2. send retries transient then succeeds.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_retries_then_succeeds() {
        let sender = RecordingSender::new(vec![
            Err(SendError::Transient("conn reset".into())),
            RecordingSender::ok("QID-2"),
        ]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();
        let receipt = ch
            .send_message(OutgoingMessage::text("ops@acme.com", "retry"))
            .await
            .unwrap();
        assert_eq!(receipt.id, "QID-2");
        assert_eq!(sender.sent.lock().unwrap().len(), 2);
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 3. send permanent auth failure short-circuits.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_auth_failure_short_circuits() {
        let sender = RecordingSender::new(vec![
            Err(SendError::Auth("535 5.7.8 bad creds".into())),
            RecordingSender::ok("must-not-be-used"),
        ]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();
        let err = ch
            .send_message(OutgoingMessage::text("ops@acme.com", "x"))
            .await
            .expect_err("auth");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        assert_eq!(sender.sent.lock().unwrap().len(), 1);
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 4. parse_basic_rfc5322 round-trip — covered in imap::tests but
    // re-asserted at channel scope to keep the public surface honest.
    // -----------------------------------------------------------------
    #[test]
    fn parse_basic_rfc5322_public_shape() {
        let raw = b"From: Alice <alice@acme.com>\r\nSubject: Hi\r\n\r\nbody\r\n";
        let m = crate::imap::parse_basic_rfc5322(1, raw).unwrap();
        assert_eq!(m.author, "Alice <alice@acme.com>");
        assert!(m.text.contains("Hi"));
        assert!(m.text.contains("body"));
    }

    // -----------------------------------------------------------------
    // 5. last_seen_uid is monotonic across direct mutation
    // (the IMAP poll task uses this exact std::Mutex to advance).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn last_seen_uid_monotonic() {
        let sender = RecordingSender::new(vec![]);
        let ch =
            EmailChannel::with_sender("test", cfg_outbound_only(), creds_for_outbound(), sender);
        assert_eq!(ch.last_seen_uid(), 0);
        {
            let mut g = ch.last_seen_uid.lock().unwrap();
            *g = 17;
        }
        assert_eq!(ch.last_seen_uid(), 17);
        // Simulate a stale read trying to lower it — explicit check on the
        // poll-loop invariant (it uses `.max()` to advance).
        {
            let mut g = ch.last_seen_uid.lock().unwrap();
            *g = (*g).max(5);
        }
        assert_eq!(ch.last_seen_uid(), 17);
    }

    // -----------------------------------------------------------------
    // 6. config TOML round-trip + deny_unknown_fields.
    // -----------------------------------------------------------------
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-email"
platform = "email"

[options]
from_address = "bot@acme.com"

[options.smtp]
host = "smtp.acme.com"
port = 587
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: EmailConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.from_address, "bot@acme.com");
        assert_eq!(cfg.smtp.host, "smtp.acme.com");
        assert!(cfg.imap.is_none());
        // Absent key stays absent — the extra trust anchor is opt-in, so an
        // existing config must keep resolving to the compiled-in roots alone.
        assert_eq!(cfg.smtp.tls_root_cert_path, None);
    }

    /// `smtp.tls_root_cert_path` survives the TOML -> `EmailConfig` hop.
    ///
    /// `SmtpConfig` is `deny_unknown_fields`, so before the field existed this
    /// exact document was a hard parse ERROR. That is what makes this a real
    /// assertion about the config plumbing rather than a restatement of serde's
    /// defaults: it is the known-positive half of the pair with the
    /// `assert_eq!(.., None)` above.
    #[test]
    fn config_carries_smtp_tls_root_cert_path() {
        let raw = r#"
name = "acme-email"
platform = "email"

[options]
from_address = "bot@acme.com"

[options.smtp]
host = "smtp.acme.com"
port = 587
user_credential_handle = "email.acme.smtp_user"
password_credential_handle = "email.acme.smtp_pass"
tls_root_cert_path = "/etc/wayland/corporate-ca.pem"
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: EmailConfig = outer.options.try_into().unwrap();
        assert_eq!(
            cfg.smtp.tls_root_cert_path.as_deref(),
            Some("/etc/wayland/corporate-ca.pem")
        );
    }

    // -----------------------------------------------------------------
    // 7. stop() ends the task cleanly.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn stop_clears_sender_and_state() {
        let sender = RecordingSender::new(vec![]);
        let mut ch =
            EmailChannel::with_sender("test", cfg_outbound_only(), creds_for_outbound(), sender);
        ch.start().await.unwrap();
        assert!(ch.sender.is_some());
        assert_eq!(ch.state(), ConnectionState::Connected);
        ch.stop().await.unwrap();
        assert!(ch.sender.is_none(), "sender should be cleared on stop");
        assert_eq!(ch.state(), ConnectionState::Disconnected);
        // Idempotent second stop.
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // 8. start() without creds → Err(Auth).
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_without_smtp_creds_errors_auth() {
        let creds: Arc<dyn CredentialsStore> = Arc::new(InMemoryCreds::new());
        let sender = RecordingSender::new(vec![]);
        let mut ch = EmailChannel::with_sender("test", cfg_outbound_only(), creds, sender);
        let err = ch.start().await.expect_err("expected Auth");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // Bonus: send before start surfaces NotStarted.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let sender = RecordingSender::new(vec![]);
        let mut ch =
            EmailChannel::with_sender("test", cfg_outbound_only(), creds_for_outbound(), sender);
        let err = ch
            .send_message(OutgoingMessage::text("ops@acme.com", "x"))
            .await
            .expect_err("not started");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // -----------------------------------------------------------------
    // Reply threading (FIX 1): a reply OutgoingMessage whose reply_to names
    // a recorded inbound message produces In-Reply-To + References + a
    // non-empty Re: subject on the outbound envelope.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn reply_threads_with_in_reply_to_references_and_re_subject() {
        let sender = RecordingSender::new(vec![RecordingSender::ok("QID-R")]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();

        // Simulate an inbound message having been recorded by the poll loop.
        crate::imap::record_reply_context(
            &ch.reply_index,
            "orig-99@acme.com".to_string(),
            crate::smtp::ReplyContext {
                message_id: "orig-99@acme.com".into(),
                subject: Some("Quarterly plan".into()),
                references: Some("<root@acme.com>".into()),
            },
        );

        let out = OutgoingMessage {
            conversation_id: "ops@acme.com".into(),
            text: "here is my reply".into(),
            reply_to: Some("orig-99@acme.com".into()),
            attachments: Vec::new(),
        };
        ch.send_message(out).await.unwrap();

        {
            let sent = sender.sent.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let rfc = String::from_utf8_lossy(&sent[0].formatted()).to_string();
            assert!(
                rfc.contains("In-Reply-To: <orig-99@acme.com>"),
                "rfc = {rfc}"
            );
            assert!(
                rfc.contains("References: <root@acme.com> <orig-99@acme.com>"),
                "rfc = {rfc}"
            );
            assert!(rfc.contains("Subject: Re: Quarterly plan"), "rfc = {rfc}");
        }
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // Reply threading fallback: unknown reply_to id (index cleared / cold
    // start) still threads via a single-id chain, never errors.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn reply_to_unknown_id_falls_back_to_single_id_chain() {
        let sender = RecordingSender::new(vec![RecordingSender::ok("QID-F")]);
        let mut ch = EmailChannel::with_sender(
            "test",
            cfg_outbound_only(),
            creds_for_outbound(),
            sender.clone(),
        );
        ch.start().await.unwrap();
        let out = OutgoingMessage {
            conversation_id: "ops@acme.com".into(),
            text: "reply to unknown".into(),
            reply_to: Some("never-seen@x".into()),
            attachments: Vec::new(),
        };
        ch.send_message(out).await.unwrap();
        {
            let sent = sender.sent.lock().unwrap();
            let rfc = String::from_utf8_lossy(&sent[0].formatted()).to_string();
            assert!(rfc.contains("In-Reply-To: <never-seen@x>"), "rfc = {rfc}");
            assert!(rfc.contains("References: <never-seen@x>"), "rfc = {rfc}");
            // Unknown subject -> bare "Re:".
            assert!(rfc.contains("Subject: Re:"), "rfc = {rfc}");
        }
        ch.stop().await.unwrap();
    }

    // -----------------------------------------------------------------
    // Bonus: start() emits a Connected event into the inbox so poll_events
    // surfaces it on the first drain.
    // -----------------------------------------------------------------
    #[tokio::test]
    async fn start_pushes_connected_event() {
        let sender = RecordingSender::new(vec![]);
        let mut ch =
            EmailChannel::with_sender("test", cfg_outbound_only(), creds_for_outbound(), sender);
        ch.start().await.unwrap();
        let evs = ch.poll_events().await.unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Connected
                }
            )),
            "expected Connected event, got {evs:?}"
        );
        ch.stop().await.unwrap();
    }
}
