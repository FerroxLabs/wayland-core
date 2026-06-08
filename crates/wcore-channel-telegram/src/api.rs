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

/// One Telegram Update from `getUpdates`. We only model the slice this
/// adapter consumes; unknown fields are tolerated so future API additions
/// don't break us.
#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    /// Telegram chat ids are i64 (can be negative for groups/channels).
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
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
    let mut last_err = TelegramError::Http("no attempts made".to_string());
    let mut last_retry_after: u64 = 0;

    for attempt in 0..SEND_MAX_ATTEMPTS {
        if attempt > 0 {
            let sleep_ms = exp_backoff_ms(attempt);
            tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
        }

        let resp = match http.post(&url).json(body).send().await {
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

fn exp_backoff_ms(attempt: u32) -> u64 {
    // attempt=1 -> 200ms, attempt=2 -> 400ms, attempt=3 -> 800ms, ...
    let shift = attempt.saturating_sub(1).min(10);
    SEND_BASE_BACKOFF_MS
        .saturating_mul(1u64 << shift)
        .min(SEND_MAX_BACKOFF_MS)
}
