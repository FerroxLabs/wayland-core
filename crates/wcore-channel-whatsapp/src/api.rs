//! WhatsApp Cloud API outbound client with retry-with-jitter +
//! Retry-After handling.
//!
//! Endpoint: `POST {api_base}/{graph_version}/{phone_number_id}/messages`
//! with `Authorization: Bearer <ACCESS_TOKEN>` and JSON body:
//!
//! ```json
//! {
//!   "messaging_product": "whatsapp",
//!   "to": "<recipient_phone>",
//!   "type": "text",
//!   "text": { "body": "<msg.text>" }
//! }
//! ```
//!
//! Successful response shape (simplified):
//! `{ "messaging_product":"whatsapp", "contacts":[...], "messages":[{"id":"wamid.<...>"}] }`.
//!
//! Error response shape: `{ "error": { "message":"...", "code":131000, "type":"OAuthException" } }`.

use std::time::Duration;

use rand::Rng;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::WhatsappError;

/// Outbound WhatsApp text-message request body.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub messaging_product: &'static str,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: TextBody,
}

impl SendMessageRequest {
    pub fn new_text(recipient: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            messaging_product: "whatsapp",
            to: recipient.into(),
            kind: "text",
            text: TextBody {
                body: body.into(),
                preview_url: false,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TextBody {
    pub body: String,
    #[serde(default)]
    pub preview_url: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageResponse {
    #[serde(default)]
    pub messaging_product: Option<String>,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub messages: Vec<MessageIdEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Contact {
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub wa_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageIdEntry {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ErrorEnvelope {
    pub error: ApiErrorBody,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub code: Option<i64>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub error_subcode: Option<i64>,
}

/// Initial base for exponential backoff (jittered ±25%).
const BACKOFF_BASE_MS: u64 = 250;

/// Send the WhatsApp message with retry on transient transport failures.
/// Returns the response on first success; on permanent failure returns
/// the first non-retryable error.
pub async fn send_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    graph_version: &str,
    phone_number_id: &str,
    access_token: &str,
    req: &SendMessageRequest,
    max_attempts: u32,
) -> Result<SendMessageResponse, WhatsappError> {
    let url = format!(
        "{}/{}/{}/messages",
        api_base.trim_end_matches('/'),
        graph_version.trim_matches('/'),
        phone_number_id,
    );
    let mut last_err: Option<String> = None;

    for attempt in 1..=max_attempts {
        let resp = http
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json; charset=utf-8")
            .json(req)
            .send()
            .await;

        let resp = match resp {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(format!("send error: {e}"));
                if attempt < max_attempts {
                    sleep_backoff(attempt, None).await;
                    continue;
                }
                break;
            }
        };

        let status = resp.status();

        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = parse_retry_after(resp.headers());
            last_err = Some("HTTP 429".to_string());
            if attempt < max_attempts {
                sleep_backoff(attempt, retry_after).await;
                continue;
            }
            break;
        }

        if status.is_server_error() {
            last_err = Some(format!("HTTP {}", status.as_u16()));
            if attempt < max_attempts {
                sleep_backoff(attempt, None).await;
                continue;
            }
            break;
        }

        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            // Auth failures are terminal — no retry.
            let body = resp.text().await.unwrap_or_default();
            return Err(WhatsappError::Auth(format!(
                "HTTP {}: {}",
                status.as_u16(),
                summarise(&body)
            )));
        }

        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(WhatsappError::Api(format!(
                "HTTP {}: {}",
                status.as_u16(),
                summarise(&body)
            )));
        }

        // 2xx — parse the body. WhatsApp returns `messages: [{id:"wamid..."}]`
        // on success; an `error` field signals a 200-with-failure (rare but
        // possible during partial responses).
        let raw = resp
            .bytes()
            .await
            .map_err(|e| WhatsappError::MalformedPayload(format!("read response bytes: {e}")))?;

        if let Ok(env) = serde_json::from_slice::<ErrorEnvelope>(&raw) {
            let msg = env
                .error
                .message
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            // Auth-class error codes per Meta's Graph API. Anything else
            // is treated as an Api(..) terminal failure — the request
            // is malformed and a retry will not help.
            if let Some(code) = env.error.code
                && matches!(code, 0 | 190 | 200..=299)
            {
                return Err(WhatsappError::Auth(format!("code {code}: {msg}")));
            }
            return Err(WhatsappError::Api(msg));
        }

        let parsed: SendMessageResponse = serde_json::from_slice(&raw).map_err(|e| {
            WhatsappError::MalformedPayload(format!("decode messages response: {e}"))
        })?;

        if parsed.messages.is_empty() {
            return Err(WhatsappError::MalformedPayload(
                "whatsapp response missing messages[0].id".to_string(),
            ));
        }

        return Ok(parsed);
    }

    Err(WhatsappError::RetryExhausted {
        attempts: max_attempts,
        last: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Parse the `Retry-After` header. WhatsApp returns integer seconds
/// when it includes one (most rate-limit responses just send 429 with
/// no header — the caller's jittered backoff handles those).
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Sleep with exponential backoff + ±25% jitter. If an explicit
/// `retry_after` was supplied (from a 429), honour it instead.
async fn sleep_backoff(attempt: u32, retry_after: Option<Duration>) {
    if let Some(d) = retry_after {
        tokio::time::sleep(d).await;
        return;
    }
    // attempt is 1-indexed: 250, 500, 1000, 2000, 4000 ms…
    let base = BACKOFF_BASE_MS.saturating_mul(1u64 << (attempt.saturating_sub(1).min(6)));
    let jitter = {
        let mut rng = rand::thread_rng();
        // ±25% of base.
        let span = (base as f64 * 0.25) as i64;
        if span > 0 {
            rng.gen_range(-span..=span)
        } else {
            0
        }
    };
    let sleep_ms = (base as i64 + jitter).max(0) as u64;
    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
}

/// Trim long error bodies for logging.
fn summarise(s: &str) -> String {
    const MAX: usize = 256;
    if s.len() <= MAX {
        s.to_string()
    } else {
        format!("{}…", &s[..MAX])
    }
}

/// Outbound WhatsApp reaction message. A reaction is just a message of
/// `type: "reaction"` referencing the inbound message id (`wamid`) and a
/// unicode emoji, sent to the same recipient.
#[derive(Debug, Clone, Serialize)]
pub struct SendReactionRequest {
    pub messaging_product: &'static str,
    pub recipient_type: &'static str,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub reaction: ReactionBody,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReactionBody {
    pub message_id: String,
    pub emoji: String,
}

impl SendReactionRequest {
    pub fn new(
        recipient: impl Into<String>,
        message_id: impl Into<String>,
        emoji: impl Into<String>,
    ) -> Self {
        Self {
            messaging_product: "whatsapp",
            recipient_type: "individual",
            to: recipient.into(),
            kind: "reaction",
            reaction: ReactionBody {
                message_id: message_id.into(),
                emoji: emoji.into(),
            },
        }
    }
}

/// Send a reaction message. Single attempt — the ack reaction is a
/// best-effort, non-fatal signal, so it does not consume the send-retry
/// budget. We don't need the returned message id, so success is `()`.
pub async fn send_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    graph_version: &str,
    phone_number_id: &str,
    access_token: &str,
    req: &SendReactionRequest,
) -> Result<(), WhatsappError> {
    let url = format!(
        "{}/{}/{}/messages",
        api_base.trim_end_matches('/'),
        graph_version.trim_matches('/'),
        phone_number_id,
    );
    let resp = http
        .post(&url)
        .bearer_auth(access_token)
        .header("Content-Type", "application/json; charset=utf-8")
        .json(req)
        .send()
        .await
        .map_err(|e| WhatsappError::Api(format!("send error: {e}")))?;

    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(WhatsappError::Auth(format!(
            "HTTP {}: {}",
            status.as_u16(),
            summarise(&body)
        )));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(WhatsappError::Api(format!(
            "HTTP {}: {}",
            status.as_u16(),
            summarise(&body)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn reaction_request_shape() {
        let req = SendReactionRequest::new("15551234567", "wamid.ABC", "👀");
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["messaging_product"], "whatsapp");
        assert_eq!(json["type"], "reaction");
        assert_eq!(json["to"], "15551234567");
        assert_eq!(json["reaction"]["message_id"], "wamid.ABC");
        assert_eq!(json["reaction"]["emoji"], "👀");
    }

    #[tokio::test]
    async fn send_reaction_succeeds_on_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/v20.0/PHONE/messages")
            .match_header("authorization", "Bearer tok")
            .match_body(mockito::Matcher::Regex(r#""type":"reaction""#.to_string()))
            .with_status(200)
            .with_body(r#"{"messaging_product":"whatsapp","messages":[{"id":"wamid.R"}]}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let req = SendReactionRequest::new("15551234567", "wamid.ABC", "👀");
        send_reaction(&http, &server.url(), "v20.0", "PHONE", "tok", &req)
            .await
            .expect("reaction should succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_reaction_unauthorized_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(401)
            .with_body(r#"{"error":{"message":"bad token"}}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let req = SendReactionRequest::new("15551234567", "wamid.ABC", "👀");
        let err = send_reaction(&http, &server.url(), "v20.0", "PHONE", "tok", &req)
            .await
            .expect_err("401 should error");
        assert!(matches!(err, WhatsappError::Auth(_)), "got {err:?}");
    }
}
