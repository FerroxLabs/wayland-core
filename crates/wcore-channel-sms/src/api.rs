//! Twilio REST client for `POST /2010-04-01/Accounts/<sid>/Messages.json`.
//!
//! Outbound is HTTP Basic auth (Account SID : Auth Token), form-urlencoded
//! `From=…&To=…&Body=…` body, JSON response. Retry policy: 5 attempts,
//! exponential backoff on 5xx + network failure, `Retry-After` honoured
//! on 429, permanent-error short-circuit on any other 4xx.

use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;

use crate::error::SmsError;

/// Base backoff for transient retries.
pub(crate) const SEND_BASE_BACKOFF_MS: u64 = 250;
/// Cap any single sleep between retries so a malicious or buggy server
/// can't park us indefinitely.
pub(crate) const SEND_MAX_BACKOFF_MS: u64 = 30_000;

/// One Twilio `Messages.json` response. We only model the fields this
/// adapter consumes; unknown fields are tolerated so future API additions
/// don't break us.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageResponse {
    /// Twilio message SID, e.g. `"SMxxxxxxxxxxxx"`.
    pub sid: String,
    /// Message status, e.g. `"queued"`, `"sent"`, `"delivered"`.
    #[serde(default)]
    pub status: Option<String>,
}

/// Send one SMS via Twilio. Returns the response on success; on permanent
/// failure returns the first non-retryable error; on exhausted retries
/// returns `SmsError::RetryExhausted`.
// twilio send accepts http/base/sid/token/from/to/body/attempts; refactoring
// into a struct is needless ceremony for a sub-driver helper.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    account_sid: &str,
    auth_token: &str,
    from_number: &str,
    to: &str,
    body: &str,
    max_attempts: u32,
) -> Result<MessageResponse, SmsError> {
    let url = format!(
        "{}/2010-04-01/Accounts/{}/Messages.json",
        api_base.trim_end_matches('/'),
        account_sid
    );
    let form: [(&str, &str); 3] = [("From", from_number), ("To", to), ("Body", body)];

    let mut last_err: Option<String> = None;

    for attempt in 1..=max_attempts {
        let resp = http
            .post(&url)
            .basic_auth(account_sid, Some(auth_token))
            .form(&form)
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
            let body = resp.text().await.unwrap_or_default();
            return Err(SmsError::Auth(format!("HTTP {}: {body}", status.as_u16())));
        }

        if status.is_client_error() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SmsError::Api(format!("HTTP {}: {body}", status.as_u16())));
        }

        // 2xx — Twilio returns 201 Created on success. Parse the JSON body.
        let parsed: MessageResponse = resp.json().await.map_err(|e| {
            SmsError::MalformedPayload(format!("decode Messages.json response: {e}"))
        })?;
        return Ok(parsed);
    }

    Err(SmsError::RetryExhausted {
        attempts: max_attempts,
        last: last_err.unwrap_or_else(|| "unknown".to_string()),
    })
}

/// Parse the `Retry-After` header. Twilio returns integer seconds.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Sleep with exponential backoff. If an explicit `retry_after` was
/// supplied (from a 429), honour it instead. Both paths are capped at
/// `SEND_MAX_BACKOFF_MS` so a misbehaving server can't park us forever.
async fn sleep_backoff(attempt: u32, retry_after: Option<Duration>) {
    if let Some(d) = retry_after {
        let capped = std::cmp::min(d, Duration::from_millis(SEND_MAX_BACKOFF_MS));
        tokio::time::sleep(capped).await;
        return;
    }
    // attempt is 1-indexed: 250, 500, 1000, 2000, 4000 ms…
    let shift = attempt.saturating_sub(1).min(10);
    let base = SEND_BASE_BACKOFF_MS
        .saturating_mul(1u64 << shift)
        .min(SEND_MAX_BACKOFF_MS);
    tokio::time::sleep(Duration::from_millis(base)).await;
}
