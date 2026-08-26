//! `wcore-channel-msteams` — Microsoft Teams channel adapter (send-only MVP).
//!
//! **Scope**: Outbound send via Bot Framework Connector REST API with OAuth2
//! client-credentials grant. Inbound (webhook receive of Bot Framework activities)
//! is deferred to v0.8.3.
//!
//! Wire protocol:
//! 1. Mint an OAuth2 Bearer token from the Bot Framework token endpoint (cached).
//! 2. POST an Activity (type=message) to `{serviceUrl}/v3/conversations/{conversationId}/activities`.
//!
//! `conversation_id` in `OutgoingMessage` must encode `{serviceUrl}|{conversationId}` —
//! the serviceUrl is tenant-specific and required by the Connector API. This
//! is populated by the caller from a previously received inbound activity.
//!
//! Written from the desktop app's TypeScript `MsTeamsPlugin`; see
//! THIRD-PARTY-NOTICES.md for the derivation chain, and F-045 in the wcore
//! audit triage.

pub mod auth;
pub mod config;
pub mod error;
pub mod inbound;
mod token;

use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::Mutex;

use wcore_channels::Channel;
use wcore_channels::error::ChannelError;
use wcore_channels::event::{ChannelEvent, ConnectionState, MessageReceipt};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::webhook::{WebhookRequest, WebhookResponse};
use wcore_config::credentials::CredentialsStore;

use auth::BotFrameworkAuth;
pub use config::MsTeamsConfig;
pub use error::MsTeamsError;
use token::TokenCache;

/// Activity JSON shape for the Bot Framework Connector send endpoint.
#[derive(Serialize)]
struct Activity<'a> {
    #[serde(rename = "type")]
    activity_type: &'a str,
    text: &'a str,
    #[serde(rename = "textFormat")]
    text_format: &'a str,
}

/// Production MS Teams channel adapter.
pub struct MsTeamsChannel {
    name: String,
    config: MsTeamsConfig,
    state: ConnectionState,
    app_id: Option<String>,
    app_password: Option<String>,
    http: wcore_egress::EgressClient,
    token_cache: TokenCache,
    inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    creds: Arc<dyn CredentialsStore>,
    /// Token endpoint base — overrideable for tests.
    token_url: String,
    /// Inbound Bot Framework JWT validator. `None` until `start()` resolves
    /// the `app_id` (the JWT audience); inbound webhooks are refused until
    /// then.
    auth: Option<BotFrameworkAuth>,
}

impl MsTeamsChannel {
    pub fn new(
        name: impl Into<String>,
        config: MsTeamsConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        // The token endpoint comes from config, which defaults to the live
        // Microsoft host — so production is unchanged and a fixture can be
        // pointed at without a `#[doc(hidden)]` constructor the registry
        // cannot reach.
        let token_url = config.token_url.clone();
        Self::with_token_url(name, config, creds, token_url)
    }

    #[doc(hidden)]
    pub fn with_token_url(
        name: impl Into<String>,
        config: MsTeamsConfig,
        creds: Arc<dyn CredentialsStore>,
        token_url: String,
    ) -> Self {
        let http = wcore_egress::EgressClient::builder()
            .user_agent(concat!("wayland-core/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();

        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            app_id: None,
            app_password: None,
            http,
            token_cache: TokenCache::new(),
            inbox: Arc::new(Mutex::new(VecDeque::new())),
            creds,
            token_url,
            auth: None,
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }

    /// Webhook-router entrypoint for an inbound Bot Framework Activity.
    ///
    /// Parses `raw_body` (the JSON Activity the Azure Bot Service POSTed to
    /// the bot's messaging endpoint) and, for a `type == "message"` activity,
    /// enqueues a [`ChannelEvent::MessageReceived`] for the next
    /// [`poll_events`](Channel::poll_events). Lifecycle activities
    /// (`conversationUpdate`, `typing`, …) are silently ignored.
    ///
    /// The activity's `conversation_id` is encoded as
    /// `{serviceUrl}|{conversationId}` (falling back to the channel's
    /// configured `service_url`) so the reply path round-trips the
    /// tenant-specific serviceUrl. See [`inbound::activity_to_incoming`].
    ///
    /// # Security — authenticate before calling
    ///
    /// This method performs **no** authentication itself — it trusts its
    /// `raw_body`. The webhook entrypoint
    /// [`ingest_webhook`](Channel::ingest_webhook) validates the Bot
    /// Framework JWT (signature + audience + issuer + expiry, via
    /// [`auth::BotFrameworkAuth`]) before calling this. Callers other than
    /// `ingest_webhook` (e.g. tests) must guarantee the body's authenticity
    /// some other way.
    pub async fn ingest_activity(&self, raw_body: &str) -> Result<(), MsTeamsError> {
        if let Some(msg) =
            inbound::activity_to_incoming(raw_body, &self.config.service_url, self.media_bounds())?
        {
            // F9 — bounded, drop-oldest inbox against a flood.
            let mut guard = self.inbox.lock().await;
            wcore_channels::push_bounded(&mut guard, ChannelEvent::MessageReceived { msg });
        }
        Ok(())
    }

    /// Resolve `(serviceUrl, conversationId, connector token)` for a
    /// `{serviceUrl}|{conversationId}` chat id — the three things every
    /// Connector call needs.
    ///
    /// Factored out for the mutate verbs rather than copied a third time.
    /// `send_message` and `send_typing` predate it and are deliberately left
    /// alone: rewriting two working, tested send paths to share a helper is a
    /// drive-by refactor, and this lane's diff is meant to be reviewable.
    async fn connector_context(
        &self,
        chat_id: &str,
    ) -> Result<(String, String, String), ChannelError> {
        let (app_id, app_password) = match (&self.app_id, &self.app_password) {
            (Some(id), Some(pw)) => (id.clone(), pw.clone()),
            _ => return Err(ChannelError::NotStarted),
        };
        let (service_url, conv_id) = parse_chat_id(chat_id, &self.config.service_url);
        let token = self
            .token_cache
            .get_token(&self.http, &app_id, &app_password, &self.token_url)
            .await
            .map_err(|e| ChannelError::Auth(format!("token: {e}")))?;
        Ok((service_url, conv_id, token))
    }
}

#[async_trait]
impl Channel for MsTeamsChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        "msteams"
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.app_id.is_some() {
            return Ok(());
        }
        self.state = ConnectionState::Connecting;

        let app_id = self
            .creds
            .get(&self.config.credential_handle_app_id)
            .map_err(|e| ChannelError::Auth(format!("credentials lookup app_id: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "MS Teams app_id not found at {:?}",
                    self.config.credential_handle_app_id
                ))
            })?;

        let app_password = self
            .creds
            .get(&self.config.credential_handle_app_password)
            .map_err(|e| ChannelError::Auth(format!("credentials lookup app_password: {e}")))?
            .ok_or_else(|| {
                ChannelError::Auth(format!(
                    "MS Teams app_password not found at {:?}",
                    self.config.credential_handle_app_password
                ))
            })?;

        // Fail-fast: verify we can mint a token before marking connected.
        self.token_cache
            .get_token(&self.http, &app_id, &app_password, &self.token_url)
            .await
            .map_err(|e| ChannelError::Auth(format!("Bot Framework token: {e}")))?;

        // Bind the inbound JWT validator now that the audience (app_id) is
        // known. Until this is set, `ingest_webhook` refuses inbound traffic.
        // The metadata URL comes from config (defaulting to the live Bot
        // Framework endpoint); the issuer is NOT configurable, because a
        // configurable issuer is a way to accept tokens minted by someone else.
        self.auth = Some(BotFrameworkAuth::with_endpoints(
            self.http.clone(),
            app_id.clone(),
            self.config.openid_metadata_url.clone(),
            auth::BF_ISSUER.to_string(),
        ));

        self.app_id = Some(app_id);
        self.app_password = Some(app_password);
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
        if self.app_id.is_none() {
            return Ok(());
        }
        self.app_id = None;
        self.app_password = None;
        self.auth = None;
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    /// Drain events enqueued by lifecycle transitions and inbound activities
    /// (via [`ingest_activity`](MsTeamsChannel::ingest_activity)).
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(self.inbox.lock().await.drain(..).collect())
    }

    /// Authenticate and ingest an inbound Bot Framework Activity webhook.
    ///
    /// This is the security gate that makes inbound MS Teams safe to route:
    ///
    /// 1. Require the channel to be started (the JWT validator is bound in
    ///    `start()` once the audience `app_id` is known).
    /// 2. Require an `Authorization` header and validate its Bearer JWT against
    ///    Azure's JWKS — signature (RS256-only), audience (`app_id`), issuer
    ///    (`https://api.botframework.com`), and expiry. See
    ///    [`auth::BotFrameworkAuth::validate`].
    /// 3. **Defense-in-depth**: when both the JWT's `serviceurl` claim and the
    ///    Activity body's `serviceUrl` are present, they must match. This
    ///    blocks replaying a valid token alongside a swapped `serviceUrl` (the
    ///    reply path trusts that `serviceUrl`, so a mismatch could redirect
    ///    bot replies to an attacker endpoint). The JWT `aud == app_id`
    ///    binding remains the primary control; this is a secondary check.
    /// 4. Parse + enqueue via [`ingest_activity`](Self::ingest_activity).
    ///
    /// Returns an empty `200 OK` once enqueued (lifecycle activities are
    /// ACKed without enqueuing). Any auth failure is surfaced as
    /// [`ChannelError::Auth`] so the host returns `401` and never parses the
    /// body of an unauthenticated request.
    async fn ingest_webhook(&self, req: &WebhookRequest) -> Result<WebhookResponse, ChannelError> {
        let auth = self.auth.as_ref().ok_or(ChannelError::NotStarted)?;
        let hdr = req
            .header("authorization")
            .ok_or_else(|| ChannelError::Auth("missing authorization header".into()))?;
        let claims = auth
            .validate(hdr)
            .await
            .map_err(|e| ChannelError::Auth(e.to_string()))?;

        // Defense-in-depth: the JWT's serviceurl claim must match the
        // Activity's serviceUrl when both are present (blocks replay with a
        // swapped serviceUrl). Compare with a stripped trailing slash so a
        // cosmetic difference doesn't cause a false reject.
        if let Some(claim_url) = claims.serviceurl.as_deref() {
            let body_url = inbound::service_url_of(&req.body)
                .map_err(|e| ChannelError::Rejected(e.to_string()))?;
            if let Some(body_url) = body_url.as_deref()
                && !service_urls_match(claim_url, body_url)
            {
                return Err(ChannelError::Auth(format!(
                    "serviceUrl claim mismatch: token={claim_url} activity={body_url}"
                )));
            }
        }

        self.ingest_activity(&req.body)
            .await
            .map_err(|e| ChannelError::Rejected(e.to_string()))?;
        Ok(WebhookResponse::ok())
    }

    /// Send a message via the Bot Framework Connector REST API.
    ///
    /// `conversation_id` must be encoded as `{serviceUrl}|{conversationId}`.
    /// The `|` separator allows passing the tenant-specific serviceUrl alongside
    /// the conversation ID in a single field (matching how the desktop app's
    /// parseChatId helper works).
    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let (app_id, app_password) = match (&self.app_id, &self.app_password) {
            (Some(id), Some(pw)) => (id.clone(), pw.clone()),
            _ => return Err(ChannelError::NotStarted),
        };

        // Parse "serviceUrl|conversationId" or fall back to config.service_url.
        let (service_url, conversation_id) =
            parse_chat_id(&msg.conversation_id, &self.config.service_url);

        let token = self
            .token_cache
            .get_token(&self.http, &app_id, &app_password, &self.token_url)
            .await
            .map_err(|e| ChannelError::Auth(format!("token: {e}")))?;

        let url = format!(
            "{service_url}v3/conversations/{}/activities",
            urlencoding_encode(&conversation_id),
        );

        let activity = Activity {
            activity_type: "message",
            text: &msg.text,
            text_format: "plain",
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&activity)
            .send()
            .await
            .map_err(|e| ChannelError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::Rejected(format!(
                "Teams send failed ({status}): {body}"
            )));
        }

        // Bot Framework returns `{"id": "<activityId>"}` on success.
        let id: serde_json::Value = resp
            .json()
            .await
            .unwrap_or_else(|_| serde_json::json!({"id": "unknown"}));
        let activity_id = id["id"].as_str().unwrap_or("unknown").to_string();

        Ok(MessageReceipt {
            id: activity_id,
            conversation_id: msg.conversation_id.clone(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    /// Send a Bot Framework `typing` activity so the user sees the bot is
    /// composing. Uses the same conversation endpoint + Connector token as
    /// [`send_message`](Self::send_message); a typing activity carries no body.
    ///
    /// (`react` is intentionally left at the trait default: the Bot Framework
    /// Connector REST API exposes no reaction-send endpoint — Teams reactions
    /// are a client/Graph concern, not a connector capability.
    ///
    /// `fetch_media` also stays at the default, but the reason has changed:
    /// inbound attachment PARSING now lands (see [`inbound`]), so the agent is
    /// told what arrived — kind, type and reference. Downloading the bytes is a
    /// separate auth-gated request against Graph/the Connector API and is not
    /// implemented. Parsing without fetching is the useful half: the agent can
    /// see and act on a file reference, where before the attachment vanished.
    ///
    /// `media_bounds` is likewise left at the trait default deliberately. The
    /// default is finite and safe; overriding it would require a Teams-specific
    /// per-file byte cap, and no such figure was verified for this adapter, so
    /// none is asserted. Note the byte bound cannot bind on this platform in any
    /// case — a Bot Framework activity reports no attachment size — so only the
    /// attachment-count bound is reachable here.)
    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let (app_id, app_password) = match (&self.app_id, &self.app_password) {
            (Some(id), Some(pw)) => (id.clone(), pw.clone()),
            _ => return Err(ChannelError::NotStarted),
        };

        let (service_url, conv_id) = parse_chat_id(conversation_id, &self.config.service_url);

        let token = self
            .token_cache
            .get_token(&self.http, &app_id, &app_password, &self.token_url)
            .await
            .map_err(|e| ChannelError::Auth(format!("token: {e}")))?;

        let url = format!(
            "{service_url}v3/conversations/{}/activities",
            urlencoding_encode(&conv_id),
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&serde_json::json!({ "type": "typing" }))
            .send()
            .await
            .map_err(|e| ChannelError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::Rejected(format!(
                "Teams typing failed ({status}): {body}"
            )));
        }
        Ok(())
    }

    /// MS Teams: the Connector REST API has `PUT` and `DELETE` on the ACTIVITY
    /// resource and a `typing` activity, but **no reaction-send endpoint** —
    /// Teams reactions are a client/Graph concern, not a connector capability.
    ///
    /// That last one is the reason this type distinguishes
    /// `PlatformHasNoApi` from `NotImplemented`: reactions here are not a task
    /// somebody forgot, they are absent from the surface this adapter speaks.
    /// The pre-existing note on [`Self::send_typing`] recorded the fact in
    /// prose; this makes it readable by a caller.
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::{Implemented, PlatformHasNoApi};
        wcore_channels::NativeActions::none()
            .edit(Implemented)
            .delete(Implemented)
            .react(PlatformHasNoApi)
            .typing(Implemented)
            .note(
                "react: the Bot Framework Connector REST API exposes no \
                 reaction-send endpoint (Teams reactions are client/Graph)",
            )
    }

    /// `PUT {serviceUrl}v3/conversations/{conv}/activities/{activityId}` — the
    /// Connector's update-activity verb. `message_id` is the activity id a send
    /// receipt carries.
    async fn edit_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<MessageReceipt, ChannelError> {
        let (service_url, conv_id, token) = self.connector_context(conversation_id).await?;
        let url = format!(
            "{service_url}v3/conversations/{}/activities/{}",
            urlencoding_encode(&conv_id),
            urlencoding_encode(message_id),
        );

        let activity = Activity {
            activity_type: "message",
            text: new_text,
            text_format: "plain",
        };

        let resp = self
            .http
            .put(&url)
            .bearer_auth(&token)
            .json(&activity)
            .send()
            .await
            .map_err(|e| ChannelError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::Rejected(format!(
                "Teams edit failed ({status}): {body}"
            )));
        }

        // The update response echoes `{"id": ...}`; an empty body is also legal
        // for a successful update, in which case the id is unchanged.
        let echoed: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let id = echoed
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or(message_id)
            .to_string();

        Ok(MessageReceipt {
            id,
            conversation_id: conversation_id.to_string(),
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    /// `DELETE {serviceUrl}v3/conversations/{conv}/activities/{activityId}`.
    async fn delete_message(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<(), ChannelError> {
        let (service_url, conv_id, token) = self.connector_context(conversation_id).await?;
        let url = format!(
            "{service_url}v3/conversations/{}/activities/{}",
            urlencoding_encode(&conv_id),
            urlencoding_encode(message_id),
        );

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ChannelError::Transport(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::Rejected(format!(
                "Teams delete failed ({status}): {body}"
            )));
        }
        Ok(())
    }

    fn config_schema(&self) -> &str {
        include_str!("schemas/msteams.json")
    }

    /// Microsoft Teams caps a single message at roughly 28000 characters.
    fn max_message_len(&self) -> Option<usize> {
        Some(28_000)
    }
}

/// Compare two `serviceUrl`s for the auth cross-check, ignoring a trailing
/// slash and ASCII case (Bot Framework lowercases the token claim, and the
/// trailing slash is stamped inconsistently across activities).
fn service_urls_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches('/').to_ascii_lowercase();
    norm(a) == norm(b)
}

/// Parse `{serviceUrl}|{conversationId}` or return `(default_service_url, raw)`.
fn parse_chat_id(chat_id: &str, default_service_url: &str) -> (String, String) {
    if let Some(pos) = chat_id.rfind('|') {
        let service_url = chat_id[..pos].to_string();
        let conv_id = chat_id[pos + 1..].to_string();
        // Ensure serviceUrl ends with /
        let service_url = if service_url.ends_with('/') {
            service_url
        } else {
            format!("{service_url}/")
        };
        return (service_url, conv_id);
    }
    (default_service_url.to_string(), chat_id.to_string())
}

/// Minimal percent-encoding for conversation IDs (which contain `19:` prefixes).
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                out.push('%');
                out.push(
                    char::from_digit((byte >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((byte & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use wcore_config::credentials::{CredentialsError, CredentialsStore as CredsTrait};

    struct MemCreds {
        inner: StdMutex<std::collections::HashMap<String, String>>,
    }
    impl MemCreds {
        fn with(entries: &[(&str, &str)]) -> Arc<dyn CredsTrait> {
            let mut map = std::collections::HashMap::new();
            for (k, v) in entries {
                map.insert(k.to_string(), v.to_string());
            }
            Arc::new(Self {
                inner: StdMutex::new(map),
            })
        }
        fn empty() -> Arc<dyn CredsTrait> {
            Arc::new(Self {
                inner: StdMutex::new(std::collections::HashMap::new()),
            })
        }
    }
    impl CredsTrait for MemCreds {
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

    fn cfg() -> MsTeamsConfig {
        MsTeamsConfig {
            credential_handle_app_id: "msteams.test.app_id".to_string(),
            credential_handle_app_password: "msteams.test.app_password".to_string(),
            service_url: "https://smba.trafficmanager.net/amer/".to_string(),
            token_url: token::BF_TOKEN_URL.to_string(),
            openid_metadata_url: auth::BF_OPENID_METADATA_URL.to_string(),
        }
    }

    // 1. Config round-trip through ChannelConfig.options.
    #[test]
    fn config_round_trip_via_channel_config_options() {
        let raw = r#"
name = "acme-teams"
platform = "msteams"

[options]
credential_handle_app_id = "msteams.acme.app_id"
credential_handle_app_password = "msteams.acme.app_password"
service_url = "https://smba.trafficmanager.net/emea/"
"#;
        let outer: wcore_channels::ChannelConfig = toml::from_str(raw).unwrap();
        let cfg: MsTeamsConfig = outer.options.try_into().unwrap();
        assert_eq!(cfg.credential_handle_app_id, "msteams.acme.app_id");
        assert_eq!(cfg.service_url, "https://smba.trafficmanager.net/emea/");
    }

    // 2. platform() returns "msteams".
    #[test]
    fn platform_tag_is_msteams() {
        let ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        assert_eq!(ch.platform(), "msteams");
    }

    #[test]
    fn max_message_len_is_msteams_cap() {
        // wayland#934: this asserts the literal the function returns one line above, so it
        // restates the code rather than testing it. It is retained as a change-detector, but
        // the check that can actually catch a wrong cap is
        // `wcore-channels-registry/tests/delivery_semantics_declaration.rs`, which binds this
        // number to `msteams.cap` in `docs/delivery-semantics.md` through the PRODUCTION
        // factory. Whether either number equals the PLATFORM's real limit is still
        // unmeasured -- see `cap_measured` and §4.2 of that document.
        let ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        assert_eq!(ch.max_message_len(), Some(28_000));
    }

    // 3. send_message before start surfaces NotStarted.
    #[tokio::test]
    async fn send_before_start_errors_not_started() {
        let mut ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let err = ch
            .send_message(OutgoingMessage::text("chat|19:abc", "hi"))
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted));
    }

    // 4. start() with missing credentials surfaces Auth.
    #[tokio::test]
    async fn start_with_missing_creds_errors_auth() {
        let mut ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let err = ch.start().await.expect_err("expected Auth");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
    }

    // 5. parse_chat_id splits on the rightmost '|'.
    #[test]
    fn parse_chat_id_splits_correctly() {
        let (svc, conv) = parse_chat_id(
            "https://smba.trafficmanager.net/amer/|19:abc@thread.v2",
            "https://default.example/",
        );
        assert_eq!(svc, "https://smba.trafficmanager.net/amer/");
        assert_eq!(conv, "19:abc@thread.v2");
    }

    // 6. ingest_activity enqueues a MessageReceived that poll_events drains.
    #[tokio::test]
    async fn ingest_activity_enqueues_message() {
        let mut ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let body = r#"{
            "type": "message",
            "id": "act-1",
            "text": "ping",
            "from": { "id": "29:user", "name": "User" },
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:abc@thread.v2", "conversationType": "personal" }
        }"#;
        ch.ingest_activity(body).await.unwrap();

        let events = ch.poll_events().await.unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.id, "act-1");
                assert_eq!(msg.sender_id, "29:user");
                assert_eq!(msg.text, "ping");
                assert_eq!(msg.platform.as_deref(), Some("msteams"));
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    // 7. ingest_activity ignores lifecycle (conversationUpdate) activities.
    #[tokio::test]
    async fn ingest_activity_ignores_conversation_update() {
        let mut ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let body =
            r#"{ "type": "conversationUpdate", "id": "x", "conversation": { "id": "19:abc" } }"#;
        ch.ingest_activity(body).await.unwrap();
        assert!(ch.poll_events().await.unwrap().is_empty());
    }

    // 8. start + send via mockito (token endpoint + connector).
    #[tokio::test]
    async fn send_message_succeeds_on_200() {
        let mut server = mockito::Server::new_async().await;

        // Mock token endpoint.
        let token_mock = server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_bearer","expires_in":3600,"token_type":"Bearer"}"#)
            .create_async()
            .await;

        // Mock connector send.
        let send_mock = server
            .mock(
                "POST",
                mockito::Matcher::Regex(r"/v3/conversations/[^/]+/activities".to_string()),
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"1:activity_id_here"}"#)
            .create_async()
            .await;

        let creds = MemCreds::with(&[
            ("msteams.test.app_id", "app-id-value"),
            ("msteams.test.app_password", "app-secret-value"),
        ]);
        let mut ch =
            MsTeamsChannel::with_token_url("test", cfg(), creds, format!("{}/token", server.url()));
        ch.start().await.unwrap();
        assert!(matches!(ch.state(), ConnectionState::Connected));

        // Use the mock server as the service URL.
        let chat_id = format!("{}|19:conversation_abc", server.url());
        let receipt = ch
            .send_message(OutgoingMessage::text(&chat_id, "Hello Teams"))
            .await
            .unwrap();

        assert_eq!(receipt.id, "1:activity_id_here");
        token_mock.assert_async().await;
        send_mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    #[tokio::test]
    async fn send_typing_posts_typing_activity() {
        let mut server = mockito::Server::new_async().await;

        let token_mock = server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_bearer","expires_in":3600,"token_type":"Bearer"}"#)
            .create_async()
            .await;

        // The typing activity must POST to the activities endpoint with a body
        // declaring type=typing (and no text).
        let typing_mock = server
            .mock(
                "POST",
                mockito::Matcher::Regex(r"/v3/conversations/[^/]+/activities".to_string()),
            )
            .match_body(mockito::Matcher::Regex(
                r#""type"\s*:\s*"typing""#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"id":"typing-1"}"#)
            .create_async()
            .await;

        let creds = MemCreds::with(&[
            ("msteams.test.app_id", "app-id-value"),
            ("msteams.test.app_password", "app-secret-value"),
        ]);
        let mut ch =
            MsTeamsChannel::with_token_url("test", cfg(), creds, format!("{}/token", server.url()));
        ch.start().await.unwrap();

        let chat_id = format!("{}|19:conversation_abc", server.url());
        ch.send_typing(&chat_id).await.unwrap();

        token_mock.assert_async().await;
        typing_mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    #[tokio::test]
    async fn send_typing_before_start_errors_not_started() {
        let ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let err = ch.send_typing("19:abc").await.unwrap_err();
        assert!(matches!(err, ChannelError::NotStarted));
    }

    // 9. ingest_webhook before start() refuses with NotStarted (the JWT
    //    validator is only bound once the audience app_id is resolved).
    #[tokio::test]
    async fn ingest_webhook_before_start_errors_not_started() {
        let ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let req = WebhookRequest {
            method: "POST".into(),
            headers: vec![("authorization".into(), "Bearer x.y.z".into())],
            body: r#"{"type":"message"}"#.into(),
            ..Default::default()
        };
        let err = ch
            .ingest_webhook(&req)
            .await
            .expect_err("expected NotStarted");
        assert!(matches!(err, ChannelError::NotStarted), "got {err:?}");
    }

    // 10. ingest_webhook with no Authorization header surfaces Auth — the host
    //     must never reach the body parse for an unauthenticated request.
    #[tokio::test]
    async fn ingest_webhook_missing_auth_header_errors_auth() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"tk","expires_in":3600,"token_type":"Bearer"}"#)
            .create_async()
            .await;
        let creds = MemCreds::with(&[
            ("msteams.test.app_id", "app-id-value"),
            ("msteams.test.app_password", "app-secret-value"),
        ]);
        let mut ch =
            MsTeamsChannel::with_token_url("test", cfg(), creds, format!("{}/token", server.url()));
        ch.start().await.unwrap();
        token_mock.assert_async().await;

        let req = WebhookRequest {
            method: "POST".into(),
            headers: vec![],
            body: r#"{"type":"message"}"#.into(),
            ..Default::default()
        };
        let err = ch.ingest_webhook(&req).await.expect_err("expected Auth");
        assert!(matches!(err, ChannelError::Auth(_)), "got {err:?}");
        // The unauthenticated body must NOT have been parsed/enqueued. (start()
        // enqueues a Connected lifecycle event, so assert specifically that no
        // MessageReceived was enqueued rather than full emptiness.)
        let events = ch.poll_events().await.unwrap();
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ChannelEvent::MessageReceived { .. })),
            "no message must be enqueued when the auth header is missing"
        );
    }

    // 10b. The inbound webhook HOST reaches this connector's authenticated
    //      ingest — i.e. msteams inbound is exposed, not merely implemented.
    //
    // The host (`wcore-agent/src/inbound_webhook.rs`) owns no per-platform
    // routing table: its handler is generic over `Path(channel)` and its only
    // dispatch is `manager.ingest_webhook(&channel, &req)`. So driving the
    // manager by name is the host's own code path, and the error VARIANT is
    // what the host turns into a status code (`response_for`):
    //
    //   Config(_)   -> 404  channel not configured
    //   Rejected(_) -> 400  fell through to the DEFAULT trait impl (unexposed)
    //   Auth(_)     -> 401  msteams' own JWT validation ran (exposed)
    //
    // Asserting `Auth` and explicitly NOT `Rejected` is therefore a positive
    // proof of exposure that cannot be satisfied by the unexposed world. This
    // test exists because `README.md` and the `bootstrap.rs` comment both still
    // claim msteams inbound is "parsed but not yet exposed over the host",
    // which is stale — if that claim were true, this test would see `Rejected`.
    #[tokio::test]
    async fn manager_dispatch_reaches_msteams_authenticated_ingest_not_the_default_impl() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"tk","expires_in":3600,"token_type":"Bearer"}"#)
            .create_async()
            .await;
        let creds = MemCreds::with(&[
            ("msteams.test.app_id", "app-id-value"),
            ("msteams.test.app_password", "app-secret-value"),
        ]);
        let mut ch = MsTeamsChannel::with_token_url(
            "msteams",
            cfg(),
            creds,
            format!("{}/token", server.url()),
        );
        ch.start().await.unwrap();
        token_mock.assert_async().await;

        let mut mgr = wcore_channels::ChannelManager::new();
        mgr.register(Box::new(ch)).await;

        let req = WebhookRequest {
            method: "POST".into(),
            headers: vec![],
            body: r#"{"type":"message"}"#.into(),
            ..Default::default()
        };

        let err = mgr
            .ingest_webhook("msteams", &req)
            .await
            .expect_err("unauthenticated POST must not be accepted");
        assert!(
            matches!(err, ChannelError::Auth(_)),
            "host dispatch must reach msteams' JWT validation (-> 401); got {err:?}"
        );
        assert!(
            !matches!(err, ChannelError::Rejected(_)),
            "Rejected would mean the DEFAULT trait impl ran, i.e. msteams inbound is \
             NOT exposed over the host; got {err:?}"
        );

        // Known-negative in the same invocation: an unregistered name must NOT
        // produce the same answer, so the assertion above is not something every
        // input satisfies.
        let unknown = mgr
            .ingest_webhook("not-a-channel", &req)
            .await
            .expect_err("unknown channel must error");
        assert!(
            matches!(unknown, ChannelError::Config(_)),
            "unknown channel must surface Config (-> 404); got {unknown:?}"
        );
    }

    // ---- native actions: edit / delete (Phase 24 C3) ----------------------

    /// A started channel plus a mocked token endpoint, shared by the mutate
    /// cases so each one only mocks the verb it is about.
    async fn started_with_token(server: &mut mockito::Server) -> MsTeamsChannel {
        server
            .mock("POST", "/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_bearer","expires_in":3600,"token_type":"Bearer"}"#)
            .expect_at_least(1)
            .create_async()
            .await;
        let creds = MemCreds::with(&[
            ("msteams.test.app_id", "app-id-value"),
            ("msteams.test.app_password", "app-secret-value"),
        ]);
        let mut ch =
            MsTeamsChannel::with_token_url("test", cfg(), creds, format!("{}/token", server.url()));
        ch.start().await.unwrap();
        ch
    }

    /// The edit is a `PUT` on the ACTIVITY resource
    /// (`…/activities/{activityId}`), not a `POST` to the activities
    /// COLLECTION. Those two differ by one path segment and one verb, and
    /// getting it wrong posts a second message instead of editing the first —
    /// a silent duplicate. The mock pins both.
    #[tokio::test]
    async fn edit_puts_to_the_activity_resource_not_the_collection() {
        let mut server = mockito::Server::new_async().await;
        let edit_mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/v3/conversations/[^/]+/activities/1%3Aactivity_id_here".to_string(),
                ),
            )
            .match_header("authorization", "Bearer test_bearer")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "type": "message",
                "text": "edited body"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"1:activity_id_here"}"#)
            .expect(1)
            .create_async()
            .await;

        let mut ch = started_with_token(&mut server).await;
        let chat_id = format!("{}|19:conversation_abc", server.url());
        let receipt = ch
            .edit_message(&chat_id, "1:activity_id_here", "edited body")
            .await
            .expect("edit succeeds");
        assert_eq!(receipt.id, "1:activity_id_here");

        edit_mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    #[tokio::test]
    async fn delete_hits_the_activity_resource_with_delete() {
        let mut server = mockito::Server::new_async().await;
        let del_mock = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(
                    r"/v3/conversations/[^/]+/activities/1%3Aactivity_id_here".to_string(),
                ),
            )
            .match_header("authorization", "Bearer test_bearer")
            .with_status(200)
            .expect(1)
            .create_async()
            .await;

        let mut ch = started_with_token(&mut server).await;
        let chat_id = format!("{}|19:conversation_abc", server.url());
        ch.delete_message(&chat_id, "1:activity_id_here")
            .await
            .expect("delete succeeds");

        del_mock.assert_async().await;
        ch.stop().await.unwrap();
    }

    /// **The failing direction.** A Connector `403` must surface as an error
    /// carrying the platform's status, not as a success.
    #[tokio::test]
    async fn a_refused_delete_is_an_error_not_a_silent_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"/v3/conversations/[^/]+/activities/.*".to_string()),
            )
            .with_status(403)
            .with_body(r#"{"error":{"code":"BotNotInConversationRoster"}}"#)
            .create_async()
            .await;

        let mut ch = started_with_token(&mut server).await;
        let chat_id = format!("{}|19:conversation_abc", server.url());
        let err = ch.delete_message(&chat_id, "1:x").await.unwrap_err();
        assert!(
            !matches!(err, ChannelError::Unsupported { .. }),
            "got Unsupported — the delete override is missing: {err:?}"
        );
        assert!(
            err.to_string().contains("403"),
            "the connector status must reach the operator, got {err}"
        );
        ch.stop().await.unwrap();
    }

    /// Declaration ↔ behaviour, **both directions in one test** — this is the
    /// only adapter in the workspace with one op on each side of the line, so
    /// it is where the distinction is actually load-bearing.
    #[tokio::test]
    async fn native_action_declaration_matches_behaviour_on_both_sides() {
        use wcore_channels::ActionSupport;
        let ch = MsTeamsChannel::new("test", cfg(), MemCreds::empty());
        let a = ch.native_actions();
        assert_eq!(a.edit, ActionSupport::Implemented);
        assert_eq!(a.delete, ActionSupport::Implemented);
        assert_eq!(a.typing, ActionSupport::Implemented);
        assert_eq!(a.react, ActionSupport::PlatformHasNoApi);

        // POSITIVE side: declared Implemented → the override runs and answers
        // NotStarted, never Unsupported.
        let e = ch.edit_message("svc|conv", "1:x", "y").await.unwrap_err();
        assert!(matches!(e, ChannelError::NotStarted), "got {e:?}");
        let d = ch.delete_message("svc|conv", "1:x").await.unwrap_err();
        assert!(matches!(d, ChannelError::NotStarted), "got {d:?}");

        // NEGATIVE side: declared PlatformHasNoApi → the trait default answers
        // Unsupported, naming the op and the platform. If someone implements
        // Teams reactions and forgets the declaration, this reddens.
        let r = ch.react("svc|conv", "1:x", "👀").await.unwrap_err();
        match r {
            ChannelError::Unsupported { op, platform } => {
                assert_eq!(op, "react");
                assert_eq!(platform, "msteams");
            }
            other => panic!("declared PlatformHasNoApi but got {other:?}"),
        }
    }

    #[test]
    fn service_urls_match_ignores_trailing_slash_and_case() {
        assert!(service_urls_match(
            "https://smba.trafficmanager.net/amer/",
            "https://smba.trafficmanager.net/amer"
        ));
        assert!(service_urls_match(
            "https://SMBA.trafficmanager.net/amer",
            "https://smba.trafficmanager.net/amer/"
        ));
        assert!(!service_urls_match(
            "https://smba.trafficmanager.net/amer",
            "https://evil.example.com/amer"
        ));
    }
}
