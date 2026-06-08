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
//! Ported from the desktop app's TypeScript `MsTeamsPlugin` (OpenClaw MIT + Apache-2.0).
//! See F-045 in the wcore audit triage.

pub mod config;
pub mod error;
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
use wcore_config::credentials::CredentialsStore;

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
}

impl MsTeamsChannel {
    pub fn new(
        name: impl Into<String>,
        config: MsTeamsConfig,
        creds: Arc<dyn CredentialsStore>,
    ) -> Self {
        Self::with_token_url(name, config, creds, token::BF_TOKEN_URL.to_string())
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
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
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
        self.state = ConnectionState::Disconnected;
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected,
            });
        Ok(())
    }

    /// Always empty — inbound webhook receive is deferred to v0.8.3.
    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        Ok(self.inbox.lock().await.drain(..).collect())
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

    fn config_schema(&self) -> &str {
        include_str!("schemas/msteams.json")
    }
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

    // 6. start + send via mockito (token endpoint + connector).
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
}
