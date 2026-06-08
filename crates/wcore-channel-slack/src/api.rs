//! `chat.postMessage` client with retry-with-jitter + Retry-After handling.
//!
//! Slack response shape: `{ "ok": true, "ts": "1234.567", "channel": "..." }`
//! on success, `{ "ok": false, "error": "<code>" }` on failure. Some errors
//! arrive as HTTP 200 with `ok: false` (the platform's preferred surface),
//! others as HTTP 4xx/5xx — we treat both as failure modes and classify by
//! "is this retryable?" rather than by transport status.

use std::time::Duration;

use rand::Rng;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::error::SlackError;

/// Slack `chat.postMessage` request body. Threading via `thread_ts` is
/// optional; we serialise only the fields the platform requires.
#[derive(Debug, Clone, Serialize)]
pub struct PostMessageRequest {
    pub channel: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PostMessageResponse {
    pub ok: bool,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Permanent Slack error codes (authoritative — no point retrying).
/// Everything else surfaces as Api(...) but is treated as terminal
/// at the API boundary (transport-layer retries are handled around
/// HTTP failures, not application-layer ok:false rejections).
const PERMANENT_ERROR_CODES: &[&str] = &[
    "invalid_auth",
    "not_authed",
    "account_inactive",
    "token_revoked",
    "token_expired",
    "no_permission",
    "channel_not_found",
    "is_archived",
    "msg_too_long",
    "rate_limited", // handled by Retry-After path, but still terminal if we surface it here
];

/// Initial base for exponential backoff (jittered ±25%).
const BACKOFF_BASE_MS: u64 = 250;

/// Send `chat.postMessage` with retry. Returns the response on first
/// success; on permanent failure returns the first non-retryable error.
pub async fn post_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    bot_token: &str,
    req: &PostMessageRequest,
    max_attempts: u32,
) -> Result<PostMessageResponse, SlackError> {
    let url = format!("{}/api/chat.postMessage", api_base.trim_end_matches('/'));
    let mut last_err: Option<String> = None;

    for attempt in 1..=max_attempts {
        let resp = http
            .post(&url)
            .bearer_auth(bot_token)
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
            return Err(SlackError::Auth(format!(
                "HTTP {}: {body}",
                status.as_u16()
            )));
        }

        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SlackError::Api(format!("HTTP {}: {body}", status.as_u16())));
        }

        // 2xx — parse the body. ok:false is still terminal (Slack's preferred
        // failure surface for permanent app-level errors).
        let parsed: PostMessageResponse = resp.json().await.map_err(|e| {
            SlackError::MalformedPayload(format!("decode chat.postMessage response: {e}"))
        })?;

        if !parsed.ok {
            let code = parsed
                .error
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            if PERMANENT_ERROR_CODES.contains(&code.as_str()) {
                if code == "invalid_auth"
                    || code == "not_authed"
                    || code == "token_revoked"
                    || code == "token_expired"
                {
                    return Err(SlackError::Auth(code));
                }
                return Err(SlackError::Api(code));
            }
            // Unknown ok:false code — treat as terminal API error (don't
            // burn the retry budget on an unknown semantic failure).
            return Err(SlackError::Api(code));
        }

        return Ok(parsed);
    }

    Err(SlackError::RetryExhausted {
        attempts: max_attempts,
        last: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Parse the `Retry-After` header. Slack returns integer seconds.
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
