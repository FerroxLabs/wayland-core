//! Webhook signature verification + bot-token resolution helpers.
//!
//! Slack signs every Events API request with HMAC-SHA256 over
//! `v0:<timestamp>:<raw-body>` using the per-app signing secret. The
//! signature header is `v0=<hex>` and the timestamp header carries
//! seconds-since-epoch. Replay protection: reject timestamps outside
//! a ±5-minute window (Slack docs recommend 5 minutes).
//!
//! Bot-token resolution goes through the `CredentialsStore` trait —
//! the adapter only ever holds the resolved token in memory; on-disk
//! config carries credential-handle keys only.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::SlackError;

type HmacSha256 = Hmac<Sha256>;

/// Maximum allowed skew between client clock and `X-Slack-Request-Timestamp`.
/// Slack recommends 5 minutes for replay protection.
pub const MAX_TIMESTAMP_SKEW_SECS: i64 = 5 * 60;

/// Compute the expected v0 signature for a webhook body.
///
/// Format per Slack docs: `v0=<hex(hmac_sha256(secret, "v0:" + ts + ":" + body))>`.
pub fn expected_signature(signing_secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body.as_bytes());
    let tag = mac.finalize().into_bytes();
    format!("v0={}", hex::encode(tag))
}

/// Constant-time signature comparison wrapped around `hmac::Mac::verify_slice`.
pub fn verify_signature(
    signing_secret: &str,
    timestamp: &str,
    body: &str,
    received_signature: &str,
) -> Result<(), SlackError> {
    let received = received_signature
        .strip_prefix("v0=")
        .ok_or(SlackError::SignatureMismatch)?;
    let received_bytes = hex::decode(received).map_err(|_| SlackError::SignatureMismatch)?;

    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(b"v0:");
    mac.update(timestamp.as_bytes());
    mac.update(b":");
    mac.update(body.as_bytes());
    mac.verify_slice(&received_bytes)
        .map_err(|_| SlackError::SignatureMismatch)
}

/// Verify the timestamp is within the replay window relative to `now_secs`.
pub fn verify_timestamp(timestamp: &str, now_secs: i64) -> Result<(), SlackError> {
    let ts: i64 = timestamp
        .parse()
        .map_err(|_| SlackError::MalformedPayload("timestamp not an integer".to_string()))?;
    let delta = (now_secs - ts).abs();
    if delta > MAX_TIMESTAMP_SKEW_SECS {
        return Err(SlackError::StaleTimestamp(delta));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_signature_shape_is_v0_hex() {
        let sig = expected_signature("secret", "1700000000", "body");
        assert!(sig.starts_with("v0="));
        // HMAC-SHA256 hex = 64 chars after the "v0=" prefix.
        assert_eq!(sig.len(), 3 + 64);
        assert!(sig[3..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn expected_signature_is_deterministic() {
        let a = expected_signature("secret", "1700000000", "body");
        let b = expected_signature("secret", "1700000000", "body");
        assert_eq!(a, b);
    }

    #[test]
    fn verify_signature_accepts_valid() {
        let secret = "shhh";
        let body = r#"{"event":"x"}"#;
        let ts = "1700000000";
        let sig = expected_signature(secret, ts, body);
        verify_signature(secret, ts, body, &sig).expect("valid signature should verify");
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        let secret = "shhh";
        let body = r#"{"event":"x"}"#;
        let ts = "1700000000";
        let sig = expected_signature(secret, ts, body);
        let err = verify_signature(secret, ts, r#"{"event":"y"}"#, &sig).unwrap_err();
        assert!(matches!(err, SlackError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_wrong_secret() {
        let secret = "shhh";
        let body = r#"{"event":"x"}"#;
        let ts = "1700000000";
        let sig = expected_signature(secret, ts, body);
        let err = verify_signature("nope", ts, body, &sig).unwrap_err();
        assert!(matches!(err, SlackError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_malformed_header() {
        let err = verify_signature("shhh", "1700000000", "body", "garbage").unwrap_err();
        assert!(matches!(err, SlackError::SignatureMismatch));
    }

    #[test]
    fn verify_timestamp_accepts_recent() {
        verify_timestamp("1700000000", 1700000010).expect("10s skew ok");
        verify_timestamp("1700000000", 1700000000 - 200).expect("future skew ok within window");
    }

    #[test]
    fn verify_timestamp_rejects_stale() {
        let err = verify_timestamp("1700000000", 1700000000 + 600).unwrap_err();
        assert!(matches!(err, SlackError::StaleTimestamp(_)));
    }

    #[test]
    fn verify_timestamp_rejects_garbage() {
        let err = verify_timestamp("not-a-number", 1700000000).unwrap_err();
        assert!(matches!(err, SlackError::MalformedPayload(_)));
    }
}
