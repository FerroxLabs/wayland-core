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

/// Meta's documented cap on `biz_opaque_callback_data`.
///
/// The Cloud API describes it as a tracking string of at most 512 characters.
/// A delivery id is `cron:{job_id}:{scheduled_for_millis}` and is far shorter,
/// so this bound is never reached in practice — it exists so a pathological
/// job id degrades into a truncated tracking string rather than into a Meta
/// rejection that fails the send outright. Losing attributability is bad;
/// losing the message is worse.
pub const MAX_TRACKING_DATA_CHARS: usize = 512;

/// Fit a delivery id into [`MAX_TRACKING_DATA_CHARS`], truncating on a char
/// boundary so a multi-byte id cannot produce invalid UTF-8.
#[must_use]
pub fn clamp_tracking_data(key: &str) -> String {
    match key.char_indices().nth(MAX_TRACKING_DATA_CHARS) {
        None => key.to_string(),
        Some((byte_idx, _)) => key[..byte_idx].to_string(),
    }
}

/// Outbound WhatsApp text-message request body.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub messaging_product: &'static str,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: TextBody,
    /// Reply-in-context: quotes a prior message by its wamid. Serialized as
    /// `"context":{"message_id":"<wamid>"}` per the Cloud API; omitted when the
    /// outbound is not a reply.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<MessageContext>,
    /// The gateway's delivery id, carried in the Cloud API's documented
    /// arbitrary tracking field. See
    /// [`SendMessageRequest::with_tracking_data`]. Omitted entirely on an
    /// unkeyed send.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biz_opaque_callback_data: Option<String>,
}

/// Quoted-message reference for reply-in-context outbound messages.
#[derive(Debug, Clone, Serialize)]
pub struct MessageContext {
    pub message_id: String,
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
            context: None,
            biz_opaque_callback_data: None,
        }
    }

    /// Quote `wamid` as the reply context. A no-op when `wamid` is `None` (a
    /// non-reply message), leaving `context` omitted from the serialized body.
    pub fn with_reply_context(mut self, wamid: Option<String>) -> Self {
        self.context = wamid
            .filter(|id| !id.is_empty())
            .map(|message_id| MessageContext { message_id });
        self
    }

    /// Carry the gateway's delivery id in `biz_opaque_callback_data`.
    ///
    /// # Why this field and not a header
    ///
    /// `biz_opaque_callback_data` is the Cloud API's documented arbitrary
    /// tracking string on the send payload (optional, ≤512 chars), and Meta
    /// echoes it back inside the `statuses` object of the `messages` webhook.
    /// So unlike an inert header it is useful at the REAL platform: a delivery
    /// status arriving hours later can be joined to the exact
    /// `cron:{job_id}:{scheduled_for_millis}` that caused it.
    ///
    /// # What it is NOT
    ///
    /// It is not a dedup token, and nothing here claims Meta collapses two
    /// sends that carry the same value —
    /// [`crate::WhatsappChannel::supports_outbound_idempotency`] stays `false`
    /// for that reason. It is a TRACKING string; attribution and deduplication
    /// are different properties and only the first is being asserted.
    ///
    /// A `None` or empty key leaves the field omitted rather than sending an
    /// empty string, so an unidentified arrival stays visibly unidentified.
    ///
    /// # THIS CARRIER IS CLOUD-API-ONLY — a named assumption, not an oversight
    ///
    /// `biz_opaque_callback_data` is a field of Meta's **Cloud API** send
    /// payload. It does not exist in the WhatsApp **Web** protocol, so a
    /// non-Cloud backend (Baileys, whatsapp-web) reaching this crate through a
    /// future transport seam **cannot** carry a delivery id here and would
    /// silently produce arrivals with no identity — exactly the state this
    /// change exists to remove, reintroduced one layer down and invisible from
    /// the Cloud path's tests.
    ///
    /// Whoever adds that seam owns choosing its carrier; this comment only
    /// refuses to let the choice be made by default. The bit that must not
    /// drift is [`crate::WhatsappChannel::supports_outbound_idempotency`],
    /// which is one value for the whole adapter while the transports beneath it
    /// may differ — a backend that cannot carry an id must not inherit a
    /// declaration made about one that can.
    pub fn with_tracking_data(mut self, key: Option<&str>) -> Self {
        self.biz_opaque_callback_data = key.filter(|k| !k.is_empty()).map(clamp_tracking_data);
        self
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TextBody {
    pub body: String,
    #[serde(default)]
    pub preview_url: bool,
}

/// Outbound WhatsApp media-message request body (link variant).
///
/// The Cloud API media message carries the media object under a key that
/// matches `type` (`image`/`audio`/`video`/`document`), e.g.
/// `{ "type":"image", "image": { "link":"https://…", "caption":"…" } }`.
/// We send by `link` (Meta fetches the URL) rather than uploading bytes, so a
/// non-text reply isn't silently dropped. Audio messages don't accept a
/// caption per the Cloud API, so it's omitted there.
#[derive(Debug, Clone, Serialize)]
pub struct SendMediaRequest {
    pub messaging_product: &'static str,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The media object, serialized under a key equal to `kind`
    /// (`image`/`audio`/`video`/`document`).
    #[serde(flatten)]
    pub media: MediaEnvelope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<MessageContext>,
    /// The gateway's delivery id — see
    /// [`SendMessageRequest::with_tracking_data`].
    ///
    /// A multi-attachment outbound is N Cloud API messages under ONE logical
    /// delivery, so every part carries the SAME value here. That is deliberate
    /// and is the opposite of the chunked-text rule in
    /// `wcore-channels::ChannelManager::send_to_keyed`, which withholds the key
    /// from chunk 2..N so a *deduplicating* destination cannot suppress them.
    /// The distinction is exactly the one this whole change turns on: a
    /// dedup token must be unique per message or it destroys content, whereas a
    /// tracking string is supposed to be shared by every part of one delivery —
    /// that sharing is what makes the parts joinable back to their cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biz_opaque_callback_data: Option<String>,
}

/// Wraps the media object so it serializes under the `kind`-named key.
#[derive(Debug, Clone)]
pub struct MediaEnvelope {
    pub kind: &'static str,
    pub body: MediaBody,
}

impl Serialize for MediaEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.kind, &self.body)?;
        map.end()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaBody {
    pub link: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

impl SendMediaRequest {
    /// Build a media message for `url`, inferring the media kind from the
    /// URL's file extension and defaulting to `document` (the most permissive
    /// kind — accepts any file and a caption). `caption` is attached for all
    /// kinds except `audio`, which the Cloud API rejects with a caption.
    pub fn new_link(
        recipient: impl Into<String>,
        url: impl Into<String>,
        caption: Option<String>,
    ) -> Self {
        let url = url.into();
        let kind = media_kind_for_url(&url);
        // Audio messages cannot carry a caption; drop it (the text is still
        // sent as a separate message by the caller when present).
        let caption = if kind == "audio" {
            None
        } else {
            caption.filter(|c| !c.is_empty())
        };
        Self {
            messaging_product: "whatsapp",
            to: recipient.into(),
            kind,
            media: MediaEnvelope {
                kind,
                body: MediaBody { link: url, caption },
            },
            context: None,
            biz_opaque_callback_data: None,
        }
    }

    /// Quote `wamid` as the reply context (see [`SendMessageRequest::with_reply_context`]).
    pub fn with_reply_context(mut self, wamid: Option<String>) -> Self {
        self.context = wamid
            .filter(|id| !id.is_empty())
            .map(|message_id| MessageContext { message_id });
        self
    }

    /// Carry the gateway's delivery id (see
    /// [`SendMessageRequest::with_tracking_data`], including its Cloud-API-only
    /// caveat).
    pub fn with_tracking_data(mut self, key: Option<&str>) -> Self {
        self.biz_opaque_callback_data = key.filter(|k| !k.is_empty()).map(clamp_tracking_data);
        self
    }
}

/// Map a media URL to its Cloud API message type by file extension.
/// Unknown/extension-less URLs fall back to `document`, which accepts any
/// file and supports a caption.
fn media_kind_for_url(url: &str) -> &'static str {
    // Strip any query/fragment before reading the extension.
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let ext = path
        .rsplit('/')
        .next()
        .and_then(|seg| seg.rsplit_once('.').map(|(_, e)| e))
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "webp" => "image",
        "mp4" | "3gp" => "video",
        "aac" | "mp3" | "m4a" | "amr" | "ogg" | "opus" => "audio",
        _ => "document",
    }
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
    send_to_messages(
        http,
        api_base,
        graph_version,
        phone_number_id,
        access_token,
        req,
        max_attempts,
    )
    .await
}

/// Send a media message (image/audio/video/document by `link`) with the same
/// retry + error-mapping discipline as [`send_message`]. Used when an
/// outbound carries attachments so a non-text reply isn't dropped.
pub async fn send_media(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    graph_version: &str,
    phone_number_id: &str,
    access_token: &str,
    req: &SendMediaRequest,
    max_attempts: u32,
) -> Result<SendMessageResponse, WhatsappError> {
    send_to_messages(
        http,
        api_base,
        graph_version,
        phone_number_id,
        access_token,
        req,
        max_attempts,
    )
    .await
}

/// Shared `POST /{phone_number_id}/messages` retry loop. Generic over the
/// request body so text, media, and any future message kind share identical
/// retry, `Retry-After`, and error-mapping behavior.
async fn send_to_messages(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    graph_version: &str,
    phone_number_id: &str,
    access_token: &str,
    req: &impl Serialize,
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

/// Step-1 media metadata response (Meta Graph API). We only need `url`.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaMetaResponse {
    pub url: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Host allowlist for the step-2 media download. The step-1 metadata response
/// is server-controlled but still untrusted: a compromised/forged response
/// could point `url` at an attacker host to capture the Graph bearer token.
/// Meta serves media bytes from `lookaside.fbsbx.com` and the `*.whatsapp.net`
/// CDN, so we fail closed on anything else *before* attaching bearer auth
/// (mirrors the Discord/Slack/SMS media path).
pub(crate) const MEDIA_DOWNLOAD_HOSTS: &[&str] = &["lookaside.fbsbx.com", ".whatsapp.net"];

/// Download inbound WhatsApp media by id: resolve the id to a temporary URL
/// (Bearer), then download that URL (Bearer). Single attempt per hop; the
/// media enricher bounds the whole thing with its own timeout.
pub async fn download_media(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    graph_version: &str,
    access_token: &str,
    media_id: &str,
    allowed_hosts: &[&str],
) -> Result<Vec<u8>, WhatsappError> {
    // Step 1: id -> temporary URL.
    let meta_url = format!(
        "{}/{}/{}",
        api_base.trim_end_matches('/'),
        graph_version.trim_matches('/'),
        media_id,
    );
    let resp = http
        .get(&meta_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| WhatsappError::Api(format!("media meta send error: {e}")))?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let body = resp.text().await.unwrap_or_default();
        return Err(WhatsappError::Auth(format!(
            "media meta HTTP {}: {}",
            status.as_u16(),
            summarise(&body)
        )));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(WhatsappError::Api(format!(
            "media meta HTTP {}: {}",
            status.as_u16(),
            summarise(&body)
        )));
    }
    let meta: MediaMetaResponse = resp
        .json()
        .await
        .map_err(|e| WhatsappError::MalformedPayload(format!("decode media meta: {e}")))?;

    // Step 2: download the temporary URL (still Bearer-authenticated).
    // The URL came from the (untrusted) step-1 body, so fail closed on the
    // host allowlist BEFORE attaching the bearer token — otherwise a forged
    // metadata response could exfiltrate the Graph token to an attacker host.
    if !wcore_egress::host_in_allowlist(&meta.url, allowed_hosts) {
        return Err(WhatsappError::Api(format!(
            "refusing to fetch media from non-allowlisted host: {}",
            meta.url
        )));
    }
    let resp2 = http
        .get(&meta.url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| WhatsappError::Api(format!("media download send error: {e}")))?;
    if !resp2.status().is_success() {
        return Err(WhatsappError::Api(format!(
            "media download HTTP {}",
            resp2.status().as_u16()
        )));
    }
    // Bounded streamed read so a media response with no/forged Content-Length
    // can't OOM the process (defense-in-depth atop the Meta media-host check).
    // The cap is the adapter's DECLARED bound, not a second hardcoded number.
    let max_bytes = usize::try_from(crate::MEDIA_BOUNDS.max_bytes).unwrap_or(usize::MAX);
    let bytes = wcore_egress::read_body_capped(resp2, max_bytes)
        .await
        .map_err(|e| WhatsappError::Api(format!("media body read: {e}")))?;
    Ok(bytes)
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn send_text_omits_context_when_not_a_reply() {
        let req = SendMessageRequest::new_text("15551234567", "hi").with_reply_context(None);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"]["body"], "hi");
        assert!(
            json.get("context").is_none(),
            "non-reply message must omit context entirely"
        );
    }

    #[test]
    fn send_text_quotes_wamid_in_context_on_reply() {
        let req = SendMessageRequest::new_text("15551234567", "yes")
            .with_reply_context(Some("wamid.QUOTED".to_string()));
        let json = serde_json::to_value(&req).unwrap();
        // Cloud API reply-in-context shape: context.message_id = quoted wamid.
        assert_eq!(json["context"]["message_id"], "wamid.QUOTED");
    }

    #[test]
    fn send_text_treats_empty_reply_id_as_no_reply() {
        // An empty reply_to must not produce an invalid empty-context body.
        let req = SendMessageRequest::new_text("15551234567", "hi")
            .with_reply_context(Some(String::new()));
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("context").is_none());
    }

    #[test]
    fn media_request_image_shape_carries_link_and_caption() {
        let req = SendMediaRequest::new_link(
            "15551234567",
            "https://cdn.example/pic.jpg",
            Some("look at this".to_string()),
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["messaging_product"], "whatsapp");
        assert_eq!(json["to"], "15551234567");
        // Extension inferred -> image; the media object nests under `image`.
        assert_eq!(json["type"], "image");
        assert_eq!(json["image"]["link"], "https://cdn.example/pic.jpg");
        assert_eq!(json["image"]["caption"], "look at this");
        // No text/audio keys leaked.
        assert!(json.get("text").is_none());
    }

    #[test]
    fn media_request_unknown_extension_defaults_to_document() {
        let req = SendMediaRequest::new_link("15551234567", "https://cdn.example/file", None);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "document");
        assert_eq!(json["document"]["link"], "https://cdn.example/file");
        // Caption omitted when None.
        assert!(json["document"].get("caption").is_none());
    }

    #[test]
    fn media_request_audio_drops_caption() {
        // The Cloud API rejects a caption on audio; it must be omitted.
        let req = SendMediaRequest::new_link(
            "15551234567",
            "https://cdn.example/voice.ogg",
            Some("ignored".to_string()),
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "audio");
        assert_eq!(json["audio"]["link"], "https://cdn.example/voice.ogg");
        assert!(json["audio"].get("caption").is_none());
    }

    #[test]
    fn media_request_infers_kind_from_extension_ignoring_query() {
        let req = SendMediaRequest::new_link(
            "15551234567",
            "https://cdn.example/clip.mp4?sig=abc#frag",
            None,
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "video");
        assert_eq!(
            json["video"]["link"],
            "https://cdn.example/clip.mp4?sig=abc#frag"
        );
    }

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

    #[tokio::test]
    async fn download_media_two_step_fetch() {
        let mut server = mockito::Server::new_async().await;
        let dl_path = "/dl/abc";
        let meta_body = format!(
            r#"{{"url":"{}{}","mime_type":"image/jpeg"}}"#,
            server.url(),
            dl_path
        );
        let meta = server
            .mock("GET", "/v20.0/MID")
            .match_header("authorization", "Bearer tok")
            .with_status(200)
            .with_body(meta_body)
            .create_async()
            .await;
        let dl = server
            .mock("GET", dl_path)
            .match_header("authorization", "Bearer tok")
            .with_status(200)
            .with_body(b"\xff\xd8\xff\xe0jpeg".as_slice())
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        // The mock CDN host (127.0.0.1:<port>) is not a real Meta host, so the
        // allowlist must be widened to it for the positive-path test. Step-2
        // host validation itself is exercised by the negative test below.
        let host = reqwest::Url::parse(&server.url())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let bytes = download_media(
            &http,
            &server.url(),
            "v20.0",
            "tok",
            "MID",
            &[host.as_str()],
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..3], b"\xff\xd8\xff");
        meta.assert_async().await;
        dl.assert_async().await;
    }

    #[tokio::test]
    async fn download_media_unauthorized_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(401)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = download_media(&http, &server.url(), "v20.0", "tok", "MID", &[])
            .await
            .unwrap_err();
        assert!(matches!(err, WhatsappError::Auth(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn download_media_rejects_non_meta_step2_host() {
        // Step-1 metadata resolves to a non-Meta host; step-2 must fail closed
        // on the allowlist BEFORE the bearer token is attached, so a forged
        // metadata response can't exfiltrate the Graph token. The download host
        // mock has NO expectation — it must never be hit.
        let mut server = mockito::Server::new_async().await;
        let meta_body = r#"{"url":"https://attacker.example/steal","mime_type":"image/jpeg"}"#;
        let meta = server
            .mock("GET", "/v20.0/MID")
            .match_header("authorization", "Bearer tok")
            .with_status(200)
            .with_body(meta_body)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = download_media(
            &http,
            &server.url(),
            "v20.0",
            "tok",
            "MID",
            MEDIA_DOWNLOAD_HOSTS,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, WhatsappError::Api(ref m) if m.contains("non-allowlisted host")),
            "got {err:?}"
        );
        meta.assert_async().await;
    }
}
