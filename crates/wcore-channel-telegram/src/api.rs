//! Thin reqwest wrapper for the two Telegram Bot API endpoints this
//! adapter needs: `sendMessage` (outbound) and `getUpdates` (inbound
//! via long-poll).
//!
//! Both helpers take an explicit `api_base` so tests can point at a
//! `mockito::Server` instead of `https://api.telegram.org`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::TelegramError;

/// Number of retry attempts (including the first one) for sendMessage.
pub(crate) const SEND_MAX_ATTEMPTS: u32 = 5;
/// Base backoff for transient retries.
pub(crate) const SEND_BASE_BACKOFF_MS: u64 = 200;
/// Cap any single sleep between retries — both transient backoff and
/// 429 retry_after collapse to this on the high side so a malicious or
/// buggy server can't park us indefinitely.
pub(crate) const SEND_MAX_BACKOFF_MS: u64 = 30_000;
/// Per-request wall-clock cap for outbound sends. The egress client carries no
/// default timeout, and the inbound subscriber awaits send_to inline while
/// holding the per-channel slot mutex — so one hung send would otherwise freeze
/// ALL inbound dispatch indefinitely. Bound every send so a stalled connection
/// fails fast and releases the lock.
const SEND_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Tighter cap for best-effort ack signals (typing / reactions): they must
/// never delay the turn, so a hung ack is abandoned quickly.
const ACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One Telegram Update from `getUpdates`. We only model the slice this
/// adapter consumes; unknown fields are tolerated so future API additions
/// don't break us.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
    /// New incoming channel post. A bot added to a broadcast channel receives
    /// its posts here rather than in `message`; these carry no `from` (the
    /// sender identity falls back to the channel chat).
    #[serde(default)]
    pub channel_post: Option<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    #[serde(default)]
    pub date: i64,
    pub chat: Chat,
    #[serde(default)]
    pub from: Option<User>,
    #[serde(default)]
    pub text: Option<String>,
    /// Caption — Telegram puts the message text in `caption` (not `text`)
    /// when the message carries media (photo / document / video / …), so a
    /// "photo + what is this?" arrives with `text` empty and the words here.
    #[serde(default)]
    pub caption: Option<String>,
    /// Forum topic / thread id (supergroups with topics enabled).
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    /// The message this one replies to, if any.
    #[serde(default)]
    pub reply_to_message: Option<Box<Message>>,
    /// Photo sizes — present when the message contains a photo.
    #[serde(default)]
    pub photo: Option<Vec<PhotoSize>>,
    /// Voice note — present when the message contains a voice recording.
    #[serde(default)]
    pub voice: Option<Voice>,
    /// Generic document attachment.
    #[serde(default)]
    pub document: Option<Document>,
    /// Video attachment.
    #[serde(default)]
    pub video: Option<Video>,
    /// Text entities (mentions, bot_commands, …).
    #[serde(default)]
    pub entities: Option<Vec<MessageEntity>>,
}

/// Telegram `PhotoSize` — we only need the `file_id` as a reference URL.
#[derive(Debug, Clone, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
}

/// Telegram `Voice` note.
#[derive(Debug, Clone, Deserialize)]
pub struct Voice {
    pub file_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Telegram `Document`.
#[derive(Debug, Clone, Deserialize)]
pub struct Document {
    pub file_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Telegram `Video`.
#[derive(Debug, Clone, Deserialize)]
pub struct Video {
    pub file_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// Telegram `MessageEntity` — text annotations (mentions, commands, …).
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEntity {
    /// Entity type string, e.g. `"mention"`, `"bot_command"`, …
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    /// Telegram chat ids are i64 (can be negative for groups/channels).
    pub id: i64,
    /// Chat type: `"private"`, `"group"`, `"supergroup"`, `"channel"`.
    #[serde(rename = "type", default)]
    pub chat_type: String,
    /// Human-facing title (groups/channels); absent for private chats.
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

/// Envelope wrapping every Telegram Bot API response.
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
pub struct ApiResponse<T> {
    pub ok: bool,
    #[serde(default = "none_option")]
    pub result: Option<T>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub error_code: Option<i64>,
    #[serde(default)]
    pub parameters: Option<ResponseParameters>,
}

fn none_option<T>() -> Option<T> {
    None
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ResponseParameters {
    #[serde(default)]
    pub retry_after: Option<u64>,
}

/// sendMessage request body.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageBody<'a> {
    pub chat_id: &'a str,
    pub text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
}

/// `getFile` result — we only need `file_path`, the relative path under
/// `{api_base}/file/bot{token}/` from which the media bytes download.
#[derive(Debug, Clone, Deserialize)]
pub struct File {
    #[serde(default)]
    pub file_path: Option<String>,
}

/// sendDocument request body. Telegram fetches `document` itself when it
/// is a URL, so we never upload bytes from our side (no SSRF surface).
/// `caption` carries the message text when there is one to attach.
#[derive(Debug, Clone, Serialize)]
pub struct SendDocumentBody<'a> {
    pub chat_id: &'a str,
    pub document: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_message_id: Option<i64>,
}

/// `sendChatAction` request body. We always send `action: "typing"` — the
/// only chat action this adapter emits. Telegram auto-expires the typing
/// indicator after ~5s, so the subscriber refreshes it periodically.
#[derive(Debug, Clone, Serialize)]
pub struct SendChatActionBody<'a> {
    pub chat_id: &'a str,
    pub action: &'a str,
}

/// One reaction in a `setMessageReaction` request. Telegram models a
/// reaction as a tagged object; we only ever send `type: "emoji"`.
#[derive(Debug, Clone, Serialize)]
pub struct ReactionType<'a> {
    #[serde(rename = "type")]
    pub kind: &'a str,
    pub emoji: &'a str,
}

/// `setMessageReaction` request body. `reaction` is an array — an empty
/// array would clear reactions, but we always send exactly one emoji.
#[derive(Debug, Clone, Serialize)]
pub struct SetReactionBody<'a> {
    pub chat_id: &'a str,
    pub message_id: i64,
    pub reaction: Vec<ReactionType<'a>>,
}

/// Send one message with the retry policy described in the crate docs:
/// up to `SEND_MAX_ATTEMPTS` total tries, exponential backoff on 5xx /
/// network failure, Telegram-style 429 (honors `parameters.retry_after`),
/// and a permanent-error short-circuit on any other 4xx.
pub(crate) async fn send_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    body: &SendMessageBody<'_>,
) -> Result<Message, TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/sendMessage");
    post_with_retry(http, &url, body).await
}

/// POST a serializable body to a Telegram send endpoint and decode a
/// `Message` from the envelope, applying the shared retry policy
/// (exponential backoff on 5xx / network, 429 `retry_after` honoured and
/// capped, permanent short-circuit on other 4xx). Shared by `sendMessage`
/// and `sendDocument` so the policy lives in exactly one place.
async fn post_with_retry<B: Serialize>(
    http: &wcore_egress::EgressClient,
    url: &str,
    body: &B,
) -> Result<Message, TelegramError> {
    let mut last_err = TelegramError::Http("no attempts made".to_string());
    let mut last_retry_after: u64 = 0;

    for attempt in 0..SEND_MAX_ATTEMPTS {
        if attempt > 0 {
            let sleep_ms = exp_backoff_ms(attempt);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }

        let resp = match http
            .post(url)
            .json(body)
            .timeout(SEND_REQUEST_TIMEOUT)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = TelegramError::Http(format!("network: {e}"));
                continue;
            }
        };

        let status = resp.status();

        // 4xx (except 429) is permanent — short-circuit.
        if status.is_client_error() && status.as_u16() != 429 {
            let bytes = resp.bytes().await.unwrap_or_default();
            let api: ApiResponse<serde_json::Value> =
                serde_json::from_slice(&bytes).unwrap_or(ApiResponse {
                    ok: false,
                    result: None,
                    description: Some(format!("status {status}")),
                    error_code: Some(status.as_u16() as i64),
                    parameters: None,
                });
            let code = api.error_code.unwrap_or(status.as_u16() as i64);
            let desc = api
                .description
                .unwrap_or_else(|| format!("status {status}"));
            // 401 / 403 are auth.
            if matches!(status.as_u16(), 401 | 403) {
                return Err(TelegramError::Auth(desc));
            }
            return Err(TelegramError::Rejected {
                code,
                description: desc,
            });
        }

        // 5xx — transient, back off and retry.
        if status.is_server_error() {
            last_err = TelegramError::Http(format!("server {status}"));
            continue;
        }

        // 429 — honour parameters.retry_after, capped.
        if status.as_u16() == 429 {
            let bytes = resp.bytes().await.unwrap_or_default();
            let retry_after = serde_json::from_slice::<ApiResponse<serde_json::Value>>(&bytes)
                .ok()
                .and_then(|api| api.parameters)
                .and_then(|p| p.retry_after)
                .unwrap_or(1);
            last_retry_after = retry_after;
            let sleep_ms = (retry_after.saturating_mul(1000)).min(SEND_MAX_BACKOFF_MS);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            last_err = TelegramError::RateLimited {
                retry_after_secs: retry_after,
            };
            continue;
        }

        // 2xx — parse the envelope.
        let bytes = match resp.bytes().await {
            Ok(b) => b,
            Err(e) => {
                last_err = TelegramError::Http(format!("body read: {e}"));
                continue;
            }
        };
        let api: ApiResponse<Message> = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => return Err(TelegramError::Decode(e.to_string())),
        };
        if !api.ok {
            // ok=false with 2xx is unusual but specified — surface as
            // ApiNotOk so callers know it wasn't a transport problem.
            return Err(TelegramError::ApiNotOk(
                api.description.unwrap_or_else(|| "ok=false".to_string()),
            ));
        }
        match api.result {
            Some(m) => return Ok(m),
            None => return Err(TelegramError::ApiNotOk("ok=true but no result".to_string())),
        }
    }

    // Exhausted attempts. If the last failure was a 429, surface that.
    if last_retry_after > 0 {
        return Err(TelegramError::RateLimited {
            retry_after_secs: last_retry_after,
        });
    }
    Err(last_err)
}

/// One call to `getUpdates`. Returns the decoded `Vec<Update>` (possibly
/// empty). Long-poll timeouts surface as `Ok(vec![])`. Network / 5xx
/// surface as `Err`; callers back off and retry.
pub(crate) async fn get_updates(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    offset: i64,
    timeout_secs: u32,
) -> Result<Vec<Update>, TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/getUpdates");
    let timeout_str = timeout_secs.to_string();
    let offset_str = offset.to_string();

    let resp = http
        .get(&url)
        .query(&[
            ("offset", offset_str.as_str()),
            ("timeout", timeout_str.as_str()),
        ])
        // HTTP read timeout = long-poll wait + buffer. If timeout_secs
        // is 0 we still allow a short upper bound so we don't hang
        // forever on a misbehaving proxy.
        .timeout(Duration::from_secs(
            u64::from(timeout_secs).saturating_add(10),
        ))
        .send()
        .await
        .map_err(|e| TelegramError::Http(format!("network: {e}")))?;

    let status = resp.status();
    if status.is_server_error() {
        return Err(TelegramError::Http(format!("server {status}")));
    }
    if status.is_client_error() {
        let bytes = resp.bytes().await.unwrap_or_default();
        let parsed = serde_json::from_slice::<ApiResponse<serde_json::Value>>(&bytes).ok();
        let desc = parsed
            .as_ref()
            .and_then(|a| a.description.clone())
            .unwrap_or_else(|| format!("status {status}"));
        let code = parsed
            .as_ref()
            .and_then(|a| a.error_code)
            .unwrap_or(status.as_u16() as i64);
        if matches!(status.as_u16(), 401 | 403) {
            return Err(TelegramError::Auth(desc));
        }
        return Err(TelegramError::Rejected {
            code,
            description: desc,
        });
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| TelegramError::Http(format!("body read: {e}")))?;
    let api: ApiResponse<Vec<Update>> =
        serde_json::from_slice(&bytes).map_err(|e| TelegramError::Decode(e.to_string()))?;
    if !api.ok {
        return Err(TelegramError::ApiNotOk(
            api.description.unwrap_or_else(|| "ok=false".to_string()),
        ));
    }
    Ok(api.result.unwrap_or_default())
}

/// Build the media download URL for a resolved `file_path`.
///
/// Telegram serves uploaded file bytes from a *different* path than the
/// bot method endpoints: `{api_base}/file/bot{token}/{file_path}` rather
/// than `{api_base}/bot{token}/{method}`. Pure + testable so the URL
/// shape is pinned independently of any network call.
pub(crate) fn file_download_url(api_base: &str, bot_token: &str, file_path: &str) -> String {
    let base = api_base.trim_end_matches('/');
    let path = file_path.trim_start_matches('/');
    format!("{base}/file/bot{bot_token}/{path}")
}

/// Resolve a `file_id` to its downloadable URL via `getFile`.
///
/// Returns the full `{api_base}/file/bot{token}/{file_path}` URL on
/// success. The HTTP read is bounded by a short timeout so a hung
/// `getFile` can't stall the long-poll loop. Any failure (network, non-2xx,
/// `ok=false`, or a missing `file_path`) surfaces as `Err`; the caller
/// decides whether to fall back to the raw `file_id`.
pub(crate) async fn get_file(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    file_id: &str,
) -> Result<String, TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/getFile");
    let resp = http
        .get(&url)
        .query(&[("file_id", file_id)])
        // Bound the resolve so a hung getFile doesn't stall the poll loop.
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| TelegramError::Http(format!("network: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(TelegramError::Http(format!("getFile status {status}")));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| TelegramError::Http(format!("body read: {e}")))?;
    let api: ApiResponse<File> =
        serde_json::from_slice(&bytes).map_err(|e| TelegramError::Decode(e.to_string()))?;
    if !api.ok {
        return Err(TelegramError::ApiNotOk(
            api.description.unwrap_or_else(|| "ok=false".to_string()),
        ));
    }
    match api.result.and_then(|f| f.file_path) {
        Some(path) => Ok(file_download_url(api_base, bot_token, &path)),
        None => Err(TelegramError::ApiNotOk(
            "getFile ok=true but no file_path".to_string(),
        )),
    }
}

/// Build the `sendDocument` JSON body for a single attachment. Factored
/// out so the request shape is testable without a network round-trip.
/// `caption` is `None` when no text accompanies the document.
pub(crate) fn build_send_document<'a>(
    chat_id: &'a str,
    document: &'a str,
    caption: Option<&'a str>,
    reply_to_message_id: Option<i64>,
) -> SendDocumentBody<'a> {
    SendDocumentBody {
        chat_id,
        document,
        caption,
        reply_to_message_id,
    }
}

/// Send one attachment via `sendDocument`, reusing the same retry policy
/// as [`send_message`]. Telegram fetches the `document` URL itself.
pub(crate) async fn send_document(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    body: &SendDocumentBody<'_>,
) -> Result<Message, TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/sendDocument");
    post_with_retry(http, &url, body).await
}

/// Build the `sendChatAction` body for a typing indicator. Factored out so
/// the request shape is testable without a network round-trip.
pub(crate) fn build_send_chat_action(chat_id: &str) -> SendChatActionBody<'_> {
    SendChatActionBody {
        chat_id,
        action: "typing",
    }
}

/// Build the `setMessageReaction` body for a single emoji reaction.
/// Factored out so the nested `reaction` array shape is testable without a
/// network round-trip.
pub(crate) fn build_set_reaction<'a>(
    chat_id: &'a str,
    message_id: i64,
    emoji: &'a str,
) -> SetReactionBody<'a> {
    SetReactionBody {
        chat_id,
        message_id,
        reaction: vec![ReactionType {
            kind: "emoji",
            emoji,
        }],
    }
}

/// POST a serializable body to a Telegram endpoint that returns a bare
/// `result: true` envelope (not a `Message`), with NO retry. Used by the
/// best-effort ack signals (`sendChatAction`, `setMessageReaction`) where
/// a single attempt is sufficient and the caller treats failures as
/// non-fatal. Non-2xx maps to a clean error so the caller can log + ignore.
async fn post_once<B: Serialize>(
    http: &wcore_egress::EgressClient,
    url: &str,
    body: &B,
) -> Result<(), TelegramError> {
    let resp = http
        .post(url)
        .json(body)
        .timeout(ACK_REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| TelegramError::Http(format!("network: {e}")))?;

    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }

    // Non-2xx — surface a clean error. Auth on 401/403, otherwise Rejected
    // with whatever description Telegram returned (e.g. an invalid reaction
    // emoji yields a 400 here).
    let bytes = resp.bytes().await.unwrap_or_default();
    let api = serde_json::from_slice::<ApiResponse<serde_json::Value>>(&bytes).ok();
    let desc = api
        .as_ref()
        .and_then(|a| a.description.clone())
        .unwrap_or_else(|| format!("status {status}"));
    let code = api
        .as_ref()
        .and_then(|a| a.error_code)
        .unwrap_or(status.as_u16() as i64);
    if matches!(status.as_u16(), 401 | 403) {
        return Err(TelegramError::Auth(desc));
    }
    Err(TelegramError::Rejected {
        code,
        description: desc,
    })
}

/// Clear any previously-registered webhook so `getUpdates` long-poll can run.
///
/// A bot can use webhooks OR long-poll, never both: if a webhook is still
/// registered, every `getUpdates` call fails with `409 Conflict` indefinitely
/// while the channel reports Connected. Calling `deleteWebhook` at `start()`
/// guarantees the long-poll path is usable. Single attempt; 401/403 surface as
/// [`TelegramError::Auth`] (handled by [`post_once`]).
pub(crate) async fn delete_webhook(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
) -> Result<(), TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/deleteWebhook");
    // `drop_pending_updates: false` — we keep any updates Telegram queued
    // while a webhook was active so the first long-poll drains them.
    post_once(http, &url, &serde_json::json!({})).await
}

/// Send a `typing` chat action. Best-effort, single attempt.
pub(crate) async fn send_chat_action(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    body: &SendChatActionBody<'_>,
) -> Result<(), TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/sendChatAction");
    post_once(http, &url, body).await
}

/// Set a single emoji reaction on a message. Best-effort, single attempt.
pub(crate) async fn set_message_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    body: &SetReactionBody<'_>,
) -> Result<(), TelegramError> {
    let url = format!("{api_base}/bot{bot_token}/setMessageReaction");
    post_once(http, &url, body).await
}

/// Download an already-resolved media URL (the bot token is embedded in the
/// Telegram file path, so no extra auth header is needed). Single attempt —
/// the media enricher bounds the call with its own timeout.
pub(crate) async fn download_bytes(
    http: &wcore_egress::EgressClient,
    url: &str,
) -> Result<Vec<u8>, TelegramError> {
    let resp = http
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| TelegramError::Http(format!("network: {e}")))?;
    if !resp.status().is_success() {
        return Err(TelegramError::Http(format!(
            "media download returned HTTP {}",
            resp.status().as_u16()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| TelegramError::Http(format!("media body read: {e}")))?;
    Ok(bytes.to_vec())
}

fn exp_backoff_ms(attempt: u32) -> u64 {
    // attempt=1 -> 200ms, attempt=2 -> 400ms, attempt=3 -> 800ms, ...
    let shift = attempt.saturating_sub(1).min(10);
    SEND_BASE_BACKOFF_MS
        .saturating_mul(1u64 << shift)
        .min(SEND_MAX_BACKOFF_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_download_url_builds_file_path_endpoint() {
        let url = file_download_url("https://api.telegram.org", "111:AAA", "photos/file_0.jpg");
        assert_eq!(
            url,
            "https://api.telegram.org/file/bot111:AAA/photos/file_0.jpg"
        );
    }

    #[test]
    fn file_download_url_normalizes_slashes() {
        // Trailing slash on base + leading slash on path must not double up.
        let url = file_download_url("https://api.telegram.org/", "T", "/voice/a.ogg");
        assert_eq!(url, "https://api.telegram.org/file/botT/voice/a.ogg");
    }

    #[test]
    fn getfile_response_parses_file_path() {
        let raw = r#"{"ok":true,"result":{"file_id":"x","file_unique_id":"u","file_size":123,"file_path":"documents/file_3.pdf"}}"#;
        let api: ApiResponse<File> = serde_json::from_slice(raw.as_bytes()).unwrap();
        assert!(api.ok);
        assert_eq!(
            api.result.and_then(|f| f.file_path).as_deref(),
            Some("documents/file_3.pdf")
        );
    }

    #[test]
    fn getfile_response_tolerates_missing_file_path() {
        // Some results omit file_path; we must not fail to deserialize.
        let raw = r#"{"ok":true,"result":{"file_id":"x"}}"#;
        let api: ApiResponse<File> = serde_json::from_slice(raw.as_bytes()).unwrap();
        assert!(api.result.and_then(|f| f.file_path).is_none());
    }

    #[test]
    fn send_document_body_serializes_with_caption() {
        let body = build_send_document("42", "https://x/a.jpg", Some("hello"), Some(7));
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["chat_id"], "42");
        assert_eq!(json["document"], "https://x/a.jpg");
        assert_eq!(json["caption"], "hello");
        assert_eq!(json["reply_to_message_id"], 7);
    }

    #[test]
    fn send_document_body_omits_absent_optionals() {
        let body = build_send_document("42", "https://x/a.jpg", None, None);
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["chat_id"], "42");
        assert_eq!(json["document"], "https://x/a.jpg");
        // caption + reply_to_message_id skip-serialize when None.
        assert!(json.get("caption").is_none());
        assert!(json.get("reply_to_message_id").is_none());
    }

    #[test]
    fn send_chat_action_body_serializes_typing() {
        let body = build_send_chat_action("42");
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["chat_id"], "42");
        assert_eq!(json["action"], "typing");
    }

    #[test]
    fn set_reaction_body_serializes_nested_emoji_array() {
        let body = build_set_reaction("42", 7, "👀");
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["chat_id"], "42");
        // message_id is sent as a JSON number, not a string.
        assert_eq!(json["message_id"], 7);
        let reaction = json["reaction"].as_array().expect("reaction is an array");
        assert_eq!(reaction.len(), 1);
        assert_eq!(reaction[0]["type"], "emoji");
        assert_eq!(reaction[0]["emoji"], "👀");
    }

    #[tokio::test]
    async fn download_bytes_fetches_media() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/file/bot1:tok/photos/x.jpg")
            .with_status(200)
            .with_body(b"\xff\xd8\xff\xe0imagebytes".as_slice())
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let bytes = download_bytes(
            &http,
            &format!("{}/file/bot1:tok/photos/x.jpg", server.url()),
        )
        .await
        .unwrap();
        assert_eq!(&bytes[..3], b"\xff\xd8\xff");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn download_bytes_errors_on_404() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(404)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        assert!(
            download_bytes(&http, &format!("{}/x", server.url()))
                .await
                .is_err()
        );
    }
}
