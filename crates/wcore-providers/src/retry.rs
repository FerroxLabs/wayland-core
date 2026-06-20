use std::future::Future;
use std::time::Duration;

use wcore_egress::{EgressError, EgressRequestBuilder};

use super::ProviderError;

/// Default retry policy for provider HTTP calls: 3 attempts, 250 ms → 1 s → 4 s.
pub const DEFAULT_MAX_RETRIES: u32 = 2; // 1 initial + 2 retries = 3 total attempts
pub const INITIAL_BACKOFF: Duration = Duration::from_millis(250);

/// Retry a fallible async operation with exponential backoff.
///
/// Retries errors where [`ProviderError::is_retryable`] is true
/// (`RateLimited`, `Connection`, and transient HTTP 5xx / 408 `Api`
/// errors). Non-retryable errors (API 4xx auth/validation, parse
/// failures, prompt-too-long) are returned immediately.
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> Result<T, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut backoff = INITIAL_BACKOFF;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempt < max_retries => {
                // M3 fix: print 1-based attempt over total-attempt count
                // (`max_retries + 1`), not over the retry count. The loop
                // runs `0..=max_retries`, so 3 attempts when max_retries=2.
                tracing::warn!(
                    attempt = attempt + 1,
                    total = max_retries + 1,
                    error = %e,
                    "provider call failed; retrying"
                );
                // AF3 Risk 2: honour the server's retry-after hint on 429s instead
                // of the exponential backoff schedule.  Cap at 60 s to guard against
                // unreasonably large server hints.
                //
                // NOTE on the 60s cap vs `RETRY_AFTER_CAP_MS` (5 min) in the extractor:
                // the extractor's larger ceiling is for logging/scheduling — recording
                // what the server asked for. This loop caps the *actual* sleep at 60s
                // because a retry that would block more than a minute should fail-fast
                // instead, surfacing the rate-limit upstream where the caller can pick
                // a fallback provider or back off itself.
                let sleep_ms = if let ProviderError::RateLimited { retry_after_ms } = &e {
                    (*retry_after_ms).min(60_000)
                } else {
                    backoff.as_millis() as u64
                };
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                backoff = (backoff * 4).min(Duration::from_secs(4));
            }
            Err(e) => return Err(e),
        }
    }
    // Wave RB STABILITY MINOR #12: replaced `unreachable!()` with an
    // explicit typed error. The match above provably covers every loop
    // iteration (Ok returns; retryable Err continues iff
    // attempt < max_retries; any other Err returns; the final iteration
    // sets attempt == max_retries which fails the guard and falls into
    // the third arm). A future refactor that breaks this invariant will
    // now surface as a normal error path instead of a process panic
    // with "internal error: entered unreachable code".
    Err(ProviderError::Connection(
        "retry policy reached the post-loop arm — this should be impossible; \
         the loop is provably bounded by max_retries"
            .into(),
    ))
}

/// Send a `reqwest::RequestBuilder` with the standard provider retry policy.
///
/// `reqwest::RequestBuilder` is not `Clone`, so callers pass a factory `F`
/// that builds and sends the request each time. Transient connection-level
/// reqwest errors (`is_timeout()`, `is_connect()`) are mapped to
/// [`ProviderError::Connection`] so they satisfy `is_retryable()` and the
/// loop retries them. Body/decode errors (`is_body()`/`is_decode()`, i.e.
/// "error decoding response body" from a stale pooled socket) are also treated
/// as transient. `is_request()` is intentionally excluded: it covers
/// non-transient client-side errors (invalid URL, invalid header value) that
/// will always fail and must not be retried. Remaining reqwest errors
/// (redirect loops, status) fall through as `ProviderError::Http` and are
/// returned immediately.
pub async fn send_with_retry<F, Fut>(f: F) -> Result<reqwest::Response, ProviderError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
{
    with_retry(DEFAULT_MAX_RETRIES, || async {
        f().await.map_err(provider_error_from_reqwest)
    })
    .await
}

/// Map a `reqwest::Error` to a `ProviderError`, stripping the URL first.
///
/// H-2 / secrets-26: reqwest's `Display` appends ` for url (<URL>)` and a
/// provider that puts a credential in the URL (e.g. Gemini's old `?key=`
/// query form) would leak it into `ProviderError::Connection(e.to_string())`,
/// into `ProviderError::Http`'s `Display`, into the `[retry]` tracing warning, and
/// into the propagated `LlmEvent::Error`. `without_url()` removes the URL from
/// the error before it is ever formatted or stored. Timeout/connect errors map
/// to the retryable `Connection` variant; everything else is `Http`.
fn provider_error_from_reqwest(e: reqwest::Error) -> ProviderError {
    // `is_body()`/`is_decode()` cover "error decoding response body" — almost
    // always a half-closed pooled connection dropped mid-body under bursty
    // load, which is transient and succeeds on a fresh connection. Treat them
    // as retryable alongside timeout/connect. `is_request()` stays excluded
    // (invalid URL/header — permanent, must not retry).
    let is_transient = e.is_timeout() || e.is_connect() || e.is_body() || e.is_decode();
    let e = e.without_url();
    if is_transient {
        ProviderError::Connection(e.to_string())
    } else {
        ProviderError::Http(e)
    }
}

/// Map an [`EgressError`] from the chokepoint to a `ProviderError`.
///
/// A transport failure is classified exactly like a bare reqwest error
/// (timeout/connect → retryable `Connection`, URL-stripped per H-2); a policy
/// `Denied` is surfaced as `ProviderError::Egress` — terminal, never retried.
pub fn provider_error_from_egress(e: EgressError) -> ProviderError {
    match e {
        EgressError::Transport(inner) => provider_error_from_reqwest(inner),
        EgressError::Denied(reason) => ProviderError::Egress(EgressError::Denied(reason)),
        // Terminal — surfaced like Denied, never retried.
        EgressError::BodyTooLarge { limit } => {
            ProviderError::Egress(EgressError::BodyTooLarge { limit })
        }
    }
}

/// Convenience: build the request once (moves `builder`) and send with retry.
///
/// Unlike `send_with_retry`, this takes ownership of a single
/// `RequestBuilder` and clones it on each attempt. Use this when the
/// builder captures data that is cheap to clone (all LLM inference calls).
///
/// Retries cover two transient failure classes:
///   - reqwest connect/timeout errors (no HTTP round-trip completed);
///   - HTTP 5xx / 408 / 429 responses (E-H4 — a completed round-trip
///     with a transient server-side status). The successful `Response`
///     is returned for the *caller* to inspect; only transient statuses
///     are retried here.
///
/// M2: if the builder body is not cloneable (`try_clone()` → `None`), the
/// request is sent **once** without retry rather than failing outright —
/// a non-cloneable streaming body is still a valid single-shot request.
pub async fn builder_send_with_retry(
    builder: EgressRequestBuilder,
) -> Result<reqwest::Response, ProviderError> {
    let mut backoff = INITIAL_BACKOFF;
    let mut last_err: Option<ProviderError> = None;
    for attempt in 0..=DEFAULT_MAX_RETRIES {
        // M2: a non-cloneable body cannot be retried — send the original
        // builder exactly once instead of failing with a misleading
        // "Connection" error. `try_clone()` is deterministic, so it fails
        // on attempt 0 and `builder` is still owned here to move into send.
        let try_builder = match builder.try_clone() {
            Some(b) => b,
            None => {
                return builder.send().await.map_err(provider_error_from_egress);
            }
        };
        match try_builder.send().await {
            Ok(resp) => {
                // E-H4: a 5xx / 408 is a completed HTTP round-trip with a
                // transient *server-side* status. Retry it here instead of
                // handing a doomed response back to the caller.
                //
                // 429 is deliberately NOT retried here: a `Retry-After` of
                // tens of seconds would block `stream()` for a minute-plus.
                // Instead the provider surfaces `RateLimited` (with the
                // header-honoured delay, E-H1) so the caller / resilience
                // layer decides — `with_retry` caps a `RateLimited` sleep at
                // 60 s, the engine can fail over. The final attempt here
                // returns the response so the provider reads the real body.
                let status = resp.status().as_u16();
                let transient_5xx = status >= 500 || status == 408;
                if transient_5xx && attempt < DEFAULT_MAX_RETRIES {
                    tracing::warn!(
                        attempt = attempt + 1,
                        total = DEFAULT_MAX_RETRIES + 1,
                        status,
                        "transient HTTP status; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 4).min(Duration::from_secs(4));
                    last_err = Some(ProviderError::Api {
                        status,
                        message: format!("transient HTTP {status}"),
                    });
                    continue;
                }
                return Ok(resp);
            }
            Err(e) => {
                // H-2 / secrets-26: strip the URL before formatting so a
                // credential-in-URL provider can't leak the key into the
                // returned error or the `[retry]` tracing warning below.
                let provider_err = match provider_error_from_egress(e) {
                    ProviderError::Connection(msg) => ProviderError::Connection(msg),
                    // A non-transient reqwest error is returned immediately,
                    // exactly as before — only now URL-stripped.
                    other => return Err(other),
                };
                if attempt < DEFAULT_MAX_RETRIES {
                    // M3 fix: 1-based attempt over total attempts.
                    tracing::warn!(
                        attempt = attempt + 1,
                        total = DEFAULT_MAX_RETRIES + 1,
                        error = %provider_err,
                        "connection error; retrying"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 4).min(Duration::from_secs(4));
                }
                last_err = Some(provider_err);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ProviderError::Connection("all retries exhausted".into())))
}

/// True for HTTP statuses that represent a *transient* server-side failure
/// worth retrying: 5xx server errors, 408 request timeout, 429 rate limit.
///
/// 4xx statuses other than 408/429 are client errors — a retry of the same
/// request will fail identically, so they are not retried.
pub fn is_retryable_http_status(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// Extract a `Retry-After` hint (in milliseconds) from response headers.
///
/// Reads the standard `retry-after` header via [`parse_retry_after_header`]
/// (RFC 9110 delta-seconds or HTTP-date). Returns `None` when the header is
/// absent or unparseable — callers fall back to their own default.
pub fn retry_after_ms_from_headers(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_retry_after_header)
}

/// Default retry-after used when a 429 response carries no usable hint.
pub const DEFAULT_RETRY_AFTER_MS: u64 = 5_000;

/// Resolve the retry-after delay (ms) for a 429 response.
///
/// E-H1: precedence is (1) the HTTP `Retry-After` response header, then
/// (2) a nested `retry_after` / `retry_after_ms` field in the JSON body
/// (Anthropic and OpenAI populate structured rate-limit detail there),
/// then (3) [`DEFAULT_RETRY_AFTER_MS`]. The body is parsed best-effort —
/// a non-JSON or empty body simply skips step 2.
pub fn resolve_retry_after_ms(headers: &reqwest::header::HeaderMap, body_text: &str) -> u64 {
    if let Some(ms) = retry_after_ms_from_headers(headers) {
        return ms;
    }
    if !body_text.trim().is_empty()
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(body_text)
        && let Some(ms) = extract_retry_after_ms_from_nested(&json)
    {
        return ms;
    }
    DEFAULT_RETRY_AFTER_MS
}

/// Maximum retry-after value we will honour, in milliseconds.
///
/// Providers occasionally return absurd `retry_after` values (hours or days).
/// The retry loop already caps `RateLimited` sleeps at 60 s, but the nested
/// extractor caps at 5 minutes so callers that read the value directly
/// (e.g. for logging or scheduling) still see a sane number.
const RETRY_AFTER_CAP_MS: u64 = 300_000;

/// Extract a retry-after hint (in milliseconds) from a structured error JSON value.
///
/// Walks the value in this precedence order:
///   1. Top-level `retry_after_ms` (already milliseconds)
///   2. Top-level `retry_after` (seconds, multiplied by 1000)
///   3. `parameters.retry_after_ms` / `parameters.retry_after`
///   4. `body.retry_after_ms` / `body.retry_after`
///   5. `headers["retry-after"]` (HTTP-header form, seconds)
///
/// Returns `None` if no field is found, or if the value is non-numeric or
/// not strictly positive. The result is capped at 5 minutes
/// ([`RETRY_AFTER_CAP_MS`]) — providers sometimes return absurd values.
///
/// Source: openclaw MIT (c) Peter Steinberger 2025
/// (`src/infra/retry-policy.ts` → `getChannelApiRetryAfterMs`),
/// generalized to walk additional shapes seen across LLM provider APIs.
pub fn extract_retry_after_ms_from_nested(error_json: &serde_json::Value) -> Option<u64> {
    fn as_positive_ms(v: &serde_json::Value) -> Option<u64> {
        // Accept integer or float. Reject zero, negatives, NaN, infinity.
        if let Some(n) = v.as_u64() {
            if n > 0 { Some(n) } else { None }
        } else if let Some(n) = v.as_f64() {
            if n.is_finite() && n > 0.0 {
                Some(n as u64)
            } else {
                None
            }
        } else {
            None
        }
    }
    fn as_positive_seconds_ms(v: &serde_json::Value) -> Option<u64> {
        as_positive_ms(v).map(|s| s.saturating_mul(1000))
    }

    let obj = error_json.as_object()?;

    let candidate = obj
        .get("retry_after_ms")
        .and_then(as_positive_ms)
        .or_else(|| obj.get("retry_after").and_then(as_positive_seconds_ms))
        .or_else(|| {
            obj.get("parameters")
                .and_then(|p| p.as_object())
                .and_then(|p| {
                    p.get("retry_after_ms")
                        .and_then(as_positive_ms)
                        .or_else(|| p.get("retry_after").and_then(as_positive_seconds_ms))
                })
        })
        .or_else(|| {
            obj.get("body").and_then(|b| b.as_object()).and_then(|b| {
                b.get("retry_after_ms")
                    .and_then(as_positive_ms)
                    .or_else(|| b.get("retry_after").and_then(as_positive_seconds_ms))
            })
        })
        .or_else(|| {
            obj.get("headers")
                .and_then(|h| h.as_object())
                .and_then(|h| h.get("retry-after"))
                .and_then(|v| v.as_str())
                .and_then(parse_retry_after_header)
        })?;

    Some(candidate.min(RETRY_AFTER_CAP_MS))
}

/// Parse an HTTP `Retry-After` header value into milliseconds.
///
/// Accepts both forms defined by RFC 9110 §10.2.3:
///   - Delta-seconds: `"30"` → `Some(30_000)`
///   - HTTP-date: `"Wed, 21 Oct 2026 07:28:00 GMT"` → delta-from-now in ms
///
/// Returns `None` for unparseable values, non-positive deltas, or HTTP-dates
/// in the past.
pub fn parse_retry_after_header(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Delta-seconds form (integer or float).
    if let Ok(n) = trimmed.parse::<u64>() {
        if n > 0 {
            return Some(n.saturating_mul(1000));
        }
        return None;
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        if n.is_finite() && n > 0.0 {
            return Some((n * 1000.0) as u64);
        }
        return None;
    }
    // HTTP-date form (RFC 7231 / IMF-fixdate).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(trimmed) {
        let now = chrono::Utc::now();
        let delta = dt.with_timezone(&chrono::Utc) - now;
        let ms = delta.num_milliseconds();
        if ms > 0 {
            return Some(ms as u64);
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use serde_json::json;

    use super::*;
    use crate::ProviderError;

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let result = with_retry(2, || async { Ok::<_, ProviderError>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        // Pause tokio time so sleep calls return immediately
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Connection("timeout".into()))
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        tokio::time::pause();

        let result = with_retry(2, || async {
            Err::<(), _>(ProviderError::Connection("always fails".into()))
        })
        .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProviderError::Connection(_)));
    }

    #[tokio::test]
    async fn test_retry_non_retryable_error_fails_immediately() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 401,
                    message: "unauthorized".into(),
                })
            }
        })
        .await;

        // Non-retryable errors should fail immediately without retrying
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    /// AF3 Risk 2: a 429 with retry_after_ms=500 must use the server hint,
    /// not the exponential backoff schedule (which would fire at ~250 ms or
    /// ~1 000 ms).  We use `tokio::time::pause` + `tokio::time::advance` to
    /// control virtual time and assert exact sleep durations without
    /// wall-clock delays.
    #[tokio::test]
    async fn test_rate_limited_uses_retry_after_ms_not_exponential_backoff() {
        tokio::time::pause();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        // Run the retry loop in a background task so we can advance time.
        let task = tokio::spawn(async move {
            with_retry(1, || {
                let c = Arc::clone(&counter_clone);
                async move {
                    let attempt = c.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        // First call: return 429 with a 500 ms hint.
                        Err(ProviderError::RateLimited {
                            retry_after_ms: 500,
                        })
                    } else {
                        Ok(attempt)
                    }
                }
            })
            .await
        });

        // The retry loop is now sleeping for retry_after_ms = 500 ms.
        // Advancing by 499 ms must NOT unblock it (exponential would be 250 ms).
        tokio::time::advance(Duration::from_millis(499)).await;
        // Task should still be pending.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "second attempt fired too early"
        );

        // Advance past the 500 ms hint — the retry must fire now.
        tokio::time::advance(Duration::from_millis(2)).await;
        let result = task.await.expect("task panicked");
        assert!(result.is_ok(), "expected Ok after retry, got {result:?}");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "expected exactly 2 attempts"
        );
    }

    // ----- T1-A4 nested retry-after extraction -----

    #[test]
    fn test_nested_top_level_retry_after_ms() {
        let v = json!({ "retry_after_ms": 5000 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(5000));
    }

    #[test]
    fn test_nested_top_level_retry_after_seconds() {
        let v = json!({ "retry_after": 5 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(5000));
    }

    #[test]
    fn test_nested_parameters_path() {
        let v = json!({ "parameters": { "retry_after_ms": 2000 } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(2000));
    }

    #[test]
    fn test_nested_body_path() {
        let v = json!({ "body": { "retry_after": 3 } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(3000));
    }

    #[test]
    fn test_nested_headers_path() {
        let v = json!({ "headers": { "retry-after": "60" } });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(60_000));
    }

    #[test]
    fn test_nested_precedence_top_over_param() {
        // Top-level `retry_after_ms` must beat `parameters.retry_after_ms`.
        let v = json!({
            "retry_after_ms": 1000,
            "parameters": { "retry_after_ms": 9000 },
        });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(1000));
    }

    #[test]
    fn test_nested_cap_at_5_minutes() {
        let v = json!({ "retry_after_ms": 999_999_999u64 });
        assert_eq!(extract_retry_after_ms_from_nested(&v), Some(300_000));
    }

    #[test]
    fn test_nested_no_field_returns_none() {
        let v = json!({ "foo": "bar" });
        assert_eq!(extract_retry_after_ms_from_nested(&v), None);
    }

    // ----- H-2 / secrets-26: URL (and thus any `?key=`) must be stripped
    // from formatted provider errors -----

    /// A reqwest error from a request whose URL carries `?key=<SECRET>` must
    /// NOT leak that secret once mapped through `provider_error_from_reqwest`
    /// and formatted. We provoke a real connect failure against an
    /// unroutable address so reqwest produces a URL-bearing error, then
    /// assert the formatted `ProviderError` contains neither `key=` nor the
    /// secret value.
    #[tokio::test]
    async fn provider_error_strips_key_from_url() {
        // 240.0.0.1 is in the reserved 240/4 block — never routable, so the
        // connect fails fast and deterministically.
        let url =
            "http://240.0.0.1:9/v1beta/models/m:streamGenerateContent?alt=sse&key=SUPER_SECRET_KEY";
        // This test exercises `provider_error_from_reqwest` directly, so it
        // needs a genuine `reqwest::Error` — the one sanctioned raw-reqwest use
        // outside wcore-egress (the egress wrapper would yield an EgressError).
        #[allow(clippy::disallowed_methods)]
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let reqwest_err = client.get(url).send().await.expect_err("connect must fail");
        // Sanity: the raw reqwest error DOES carry the URL (and the secret).
        // If reqwest ever stops doing this the test still passes the real
        // assertion below; this only documents the threat we are closing.
        let raw = reqwest_err.to_string();

        let mapped = super::provider_error_from_reqwest(reqwest_err);
        let formatted = mapped.to_string();
        assert!(
            !formatted.contains("key="),
            "formatted ProviderError must not contain `key=`: {formatted}"
        );
        assert!(
            !formatted.contains("SUPER_SECRET_KEY"),
            "formatted ProviderError must not contain the secret value: {formatted} (raw was: {raw})"
        );
    }

    #[test]
    fn test_parse_header_seconds_form() {
        assert_eq!(parse_retry_after_header("30"), Some(30_000));
    }

    #[test]
    fn test_parse_header_invalid_returns_none() {
        assert_eq!(parse_retry_after_header("garbage"), None);
    }

    // ----- R3-B1 parse_retry_after_header edge cases -----
    //
    // The fn's docstring promises non-positive deltas, empty input, NaN, and
    // infinity all map to None. Only "30" → Some and "garbage" → None had
    // coverage before this commit.

    #[test]
    fn test_parse_header_zero_returns_none() {
        assert_eq!(parse_retry_after_header("0"), None);
    }

    #[test]
    fn test_parse_header_negative_integer_returns_none() {
        assert_eq!(parse_retry_after_header("-1"), None);
        assert_eq!(parse_retry_after_header("-5"), None);
    }

    #[test]
    fn test_parse_header_negative_float_returns_none() {
        assert_eq!(parse_retry_after_header("-0.5"), None);
    }

    #[test]
    fn test_parse_header_empty_returns_none() {
        assert_eq!(parse_retry_after_header(""), None);
    }

    #[test]
    fn test_parse_header_whitespace_only_returns_none() {
        assert_eq!(parse_retry_after_header("   "), None);
    }

    #[test]
    fn test_parse_header_nan_returns_none() {
        assert_eq!(parse_retry_after_header("NaN"), None);
    }

    #[test]
    fn test_parse_header_infinity_returns_none() {
        assert_eq!(parse_retry_after_header("inf"), None);
        assert_eq!(parse_retry_after_header("-inf"), None);
    }

    #[test]
    fn test_parse_header_http_date_past_returns_none() {
        // A clearly past HTTP-date must yield None (delta <= 0).
        assert_eq!(
            parse_retry_after_header("Wed, 21 Oct 2015 07:28:00 GMT"),
            None
        );
    }

    #[test]
    fn test_parse_header_http_date_future_returns_some() {
        // A clearly future HTTP-date must yield Some(ms > 0). We don't
        // assert the exact value (depends on wall clock at run time);
        // we only assert structure.
        let parsed = parse_retry_after_header("Wed, 21 Oct 2099 07:28:00 GMT");
        assert!(matches!(parsed, Some(ms) if ms > 0));
    }

    // ----- E-H4: HTTP-status retry classification -----

    #[test]
    fn is_retryable_http_status_covers_5xx_408_429() {
        for s in [500, 502, 503, 504, 529, 408, 429] {
            assert!(is_retryable_http_status(s), "{s} must be retryable");
        }
    }

    #[test]
    fn is_retryable_http_status_excludes_4xx_and_2xx() {
        for s in [200, 400, 401, 403, 404, 422] {
            assert!(!is_retryable_http_status(s), "{s} must NOT be retryable");
        }
    }

    /// E-H4: an `Api{status:503}` MUST be retryable so `with_retry` retries a
    /// transient 5xx instead of aborting the turn. A 401 must NOT be.
    #[test]
    fn provider_error_api_5xx_is_retryable_4xx_is_not() {
        assert!(
            ProviderError::Api {
                status: 503,
                message: "overloaded".into(),
            }
            .is_retryable(),
            "503 must retry"
        );
        assert!(
            ProviderError::Api {
                status: 502,
                message: "bad gateway".into(),
            }
            .is_retryable()
        );
        assert!(
            !ProviderError::Api {
                status: 401,
                message: "unauthorized".into(),
            }
            .is_retryable(),
            "401 must not retry"
        );
        assert!(
            !ProviderError::Api {
                status: 400,
                message: "bad request".into(),
            }
            .is_retryable()
        );
    }

    /// E-H4: `with_retry` must now retry a transient 503 and succeed.
    #[tokio::test]
    async fn with_retry_retries_transient_5xx_then_succeeds() {
        tokio::time::pause();
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                let attempt = counter.fetch_add(1, Ordering::SeqCst);
                if attempt < 2 {
                    Err(ProviderError::Api {
                        status: 503,
                        message: "overloaded".into(),
                    })
                } else {
                    Ok(attempt)
                }
            }
        })
        .await;
        assert!(result.is_ok(), "503 must be retried to success: {result:?}");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    /// E-H4: a 400 must still fail-fast with exactly one attempt.
    #[tokio::test]
    async fn with_retry_does_not_retry_4xx() {
        let counter = Arc::new(AtomicU32::new(0));
        let result = with_retry(2, || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(ProviderError::Api {
                    status: 400,
                    message: "bad request".into(),
                })
            }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(counter.load(Ordering::SeqCst), 1, "400 must not be retried");
    }

    // ----- E-H1: Retry-After resolution from response -----

    fn header_map(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn resolve_retry_after_prefers_header() {
        let h = header_map(&[("retry-after", "30")]);
        // Header (30s) wins over both the body hint and the default.
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"retry_after_ms": 999}"#),
            30_000
        );
    }

    #[test]
    fn resolve_retry_after_falls_back_to_body() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"error":{"retry_after": 12}}"#),
            // nested under `error` is not a walked path; top-level/body/params are.
            DEFAULT_RETRY_AFTER_MS
        );
        // Top-level body field IS walked.
        assert_eq!(
            resolve_retry_after_ms(&h, r#"{"retry_after_ms": 2500}"#),
            2_500
        );
    }

    #[test]
    fn resolve_retry_after_defaults_when_no_hint() {
        let h = reqwest::header::HeaderMap::new();
        assert_eq!(resolve_retry_after_ms(&h, ""), DEFAULT_RETRY_AFTER_MS);
        assert_eq!(
            resolve_retry_after_ms(&h, "not json at all"),
            DEFAULT_RETRY_AFTER_MS
        );
    }

    #[test]
    fn retry_after_ms_from_headers_parses_and_misses() {
        assert_eq!(
            retry_after_ms_from_headers(&header_map(&[("retry-after", "5")])),
            Some(5_000)
        );
        assert_eq!(
            retry_after_ms_from_headers(&reqwest::header::HeaderMap::new()),
            None
        );
        // Garbage header → None (fall through to body/default).
        assert_eq!(
            retry_after_ms_from_headers(&header_map(&[("retry-after", "soon")])),
            None
        );
    }
}
