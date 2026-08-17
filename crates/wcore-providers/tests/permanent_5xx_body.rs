//! wayland#949: a permanent authorization failure delivered as HTTP 500 must
//! not be re-sent as a transient server error.
//!
//! FluxRouter answers a model-permission rejection with `500` and a body of
//! `"type":"auth_error"`. Classified on status alone that is `Timeout`, and
//! `is_retryable_http_status` calls any `>= 500` transient, so the whole
//! request context was re-sent `1 + DEFAULT_MAX_RETRIES` times for an outcome
//! that cannot succeed.
//!
//! These tests grade the two PUBLIC surfaces the engine actually consults —
//! `classify_failover` and `ProviderError::is_retryable` — not the private
//! predicate they share. A test of the predicate alone would stay green if
//! either call site stopped calling it.

use wcore_providers::classify::classify_failover;
use wcore_providers::{FailoverReason, ProviderError};

/// The body as reported in wayland#949.
const FLUX_AUTH_500: &str = r#"{"error":{"type":"auth_error","message":"key not allowed to access model deepseek-v4-pro"}}"#;

fn api(status: u16, message: &str) -> ProviderError {
    ProviderError::Api {
        status,
        message: message.to_string(),
    }
}

#[test]
fn flux_auth_error_at_500_classifies_as_permanent_not_timeout() {
    let err = api(500, FLUX_AUTH_500);
    assert_eq!(
        classify_failover(&err, Some(500), Some(FLUX_AUTH_500), None),
        FailoverReason::AuthPermanent,
        "a 500 whose body says the key is not allowed to access the model is a \
         permission failure, not a transient server error"
    );
}

#[test]
fn flux_auth_error_at_500_is_not_retried() {
    assert!(
        !api(500, FLUX_AUTH_500).is_retryable(),
        "retrying re-sends the entire request context for an outcome that \
         cannot succeed — this is the cost wayland#949 is about"
    );
}

/// Control. Without this, the two tests above would pass just as well against
/// a change that made every 5xx permanent — which would break failover.
#[test]
fn a_plain_500_stays_transient_and_retryable() {
    let body = r#"{"error":{"message":"internal server error"}}"#;
    let err = api(500, body);
    assert_eq!(
        classify_failover(&err, Some(500), Some(body), None),
        FailoverReason::Timeout,
        "an ordinary 500 must keep its transient classification"
    );
    assert!(err.is_retryable(), "an ordinary 500 must still be retried");
}

/// 503 and 529 are EXPLICIT overload signals. A busy server often describes
/// itself in words the body tier matches, so demoting one to permanent would
/// turn a recoverable overload into a hard failure.
#[test]
fn an_explicit_overload_status_is_never_overridden_by_its_body() {
    for status in [503_u16, 529] {
        let body = r#"{"error":{"type":"auth_error","message":"not allowed"}}"#;
        let err = api(status, body);
        assert_eq!(
            classify_failover(&err, Some(status), Some(body), None),
            FailoverReason::Overloaded,
            "{status} is an explicit overload signal and must not be \
             reclassified from its body"
        );
        assert!(
            err.is_retryable(),
            "{status} must remain retryable regardless of body text"
        );
    }
}

/// Only a PERMANENT body reason overrides. A body that names a transient
/// condition must leave the 5xx exactly as it was.
#[test]
fn a_transient_body_reason_does_not_make_a_5xx_permanent() {
    for body in [
        r#"{"error":{"message":"rate limit exceeded"}}"#,
        r#"{"error":{"message":"server is busy, try again"}}"#,
    ] {
        let err = api(500, body);
        assert!(
            err.is_retryable(),
            "a 500 whose body names a transient condition must stay retryable: {body}"
        );
    }
}

/// The override is scoped to 5xx. 4xx classification is untouched.
#[test]
fn four_xx_classification_is_unchanged() {
    let err = api(401, FLUX_AUTH_500);
    assert_eq!(
        classify_failover(&err, Some(401), Some(FLUX_AUTH_500), None),
        FailoverReason::Auth,
        "401 must still classify from its status"
    );
    assert!(!err.is_retryable(), "401 must remain non-retryable");
}

/// A 5xx with no body at all must be unaffected — the override needs evidence,
/// and absence of a body is not evidence of permanence.
#[test]
fn a_5xx_with_no_body_stays_transient() {
    let err = api(502, "");
    assert_eq!(
        classify_failover(&err, Some(502), None, None),
        FailoverReason::Timeout
    );
    assert!(err.is_retryable());
}
