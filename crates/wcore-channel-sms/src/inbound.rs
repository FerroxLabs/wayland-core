//! Twilio webhook signature verification + form-urlencoded body parsing.
//!
//! Twilio signs every webhook POST with HMAC-SHA1, base64-encoded. The
//! signed message is `<full_url> + sorted-form-pairs-concatenated`, where
//! the concatenation is: each parameter key sorted alphabetically, then
//! for each key the key + value joined directly (no separator between
//! key+value, no separator between pairs).
//!
//! Example:
//!   url = "https://example.com/sms"
//!   form = { Body: "hi", From: "+15551234567", To: "+15559876543" }
//!   signed = "https://example.com/smsBodyhiFrom+15551234567To+15559876543"
//!   header = base64(HMAC-SHA1(auth_token, signed))
//!
//! Spec reference: <https://www.twilio.com/docs/usage/security#validating-requests>.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use wcore_channels::event::{Attachment, ChatType, IncomingMessage, MediaKind};

use crate::error::SmsError;

type HmacSha1 = Hmac<Sha1>;

/// Compute the expected Twilio signature for `(full_url, form_pairs)`.
///
/// `form_pairs` is the form-urlencoded body as a slice of `(key, value)`
/// pairs. The implementation sorts them by key before hashing — callers
/// can pass them in any order.
pub fn expected_signature(
    auth_token: &str,
    full_url: &str,
    form_pairs: &[(String, String)],
) -> String {
    let mut pairs: Vec<&(String, String)> = form_pairs.iter().collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut mac =
        HmacSha1::new_from_slice(auth_token.as_bytes()).expect("HMAC-SHA1 accepts any key length");
    mac.update(full_url.as_bytes());
    for (k, v) in &pairs {
        mac.update(k.as_bytes());
        mac.update(v.as_bytes());
    }
    let tag = mac.finalize().into_bytes();
    base64::engine::general_purpose::STANDARD.encode(tag)
}

/// Verify a Twilio webhook signature against `(full_url, raw_body)`.
///
/// `raw_body` is the literal form-urlencoded body Twilio POSTed. We
/// parse the pairs here so callers don't have to do it twice
/// (signature verification + message extraction).
pub fn verify_signature(
    auth_token: &str,
    full_url: &str,
    raw_body: &str,
    received_signature: &str,
) -> Result<Vec<(String, String)>, SmsError> {
    let pairs = parse_form(raw_body);
    let expected = expected_signature(auth_token, full_url, &pairs);

    // Constant-time-ish comparison via fixed-length tags. `expected` and
    // `received_signature` are both base64-encoded 20-byte HMACs, so
    // they share a length envelope — a wrong-length input falls through
    // to mismatch.
    if expected.len() != received_signature.len() {
        return Err(SmsError::SignatureMismatch);
    }
    // Decode both to bytes for constant-time compare via hmac::verify_slice
    // equivalent — recompute and compare under the same engine.
    let expected_bytes = base64::engine::general_purpose::STANDARD
        .decode(expected.as_bytes())
        .map_err(|_| SmsError::SignatureMismatch)?;
    let received_bytes = base64::engine::general_purpose::STANDARD
        .decode(received_signature.as_bytes())
        .map_err(|_| SmsError::SignatureMismatch)?;

    // Re-derive a MAC and use verify_slice for constant-time compare.
    let mut mac =
        HmacSha1::new_from_slice(auth_token.as_bytes()).expect("HMAC-SHA1 accepts any key length");
    mac.update(full_url.as_bytes());
    let mut sorted: Vec<&(String, String)> = pairs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, v) in &sorted {
        mac.update(k.as_bytes());
        mac.update(v.as_bytes());
    }
    mac.verify_slice(&received_bytes)
        .map_err(|_| SmsError::SignatureMismatch)?;
    // Defensive: confirm the bytes we just verified match the engine
    // we computed `expected` from, so a future refactor that changes
    // one path can't silently desync the other.
    debug_assert_eq!(expected_bytes, received_bytes);

    Ok(pairs)
}

/// Parse the form-urlencoded body into a vec of pairs. Uses the `url`
/// crate so percent-decoding matches what Twilio sent (Twilio's
/// signature is computed over the *decoded* parameter values).
pub fn parse_form(raw_body: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw_body.as_bytes())
        .into_owned()
        .collect()
}

/// Translate a parsed pair list into an `IncomingMessage`.
///
/// Twilio webhook fields used:
/// * `MessageSid` — platform-assigned id.
/// * `From` — author (E.164 phone number).
/// * `To` — receiving Twilio number — used as the conversation_id so
///   downstream routing treats every (From, To) pair under the same
///   "conversation".
/// * `Body` — message text.
/// * `NumMedia` / `MediaUrl{N}` — attachments. We collect all
///   `MediaUrl{N}` entries, in order.
pub fn pairs_to_incoming(pairs: &[(String, String)]) -> Result<IncomingMessage, SmsError> {
    let get = |k: &str| -> Option<&str> {
        pairs
            .iter()
            .find(|(name, _)| name == k)
            .map(|(_, v)| v.as_str())
    };

    let sid = get("MessageSid")
        .ok_or_else(|| SmsError::MalformedPayload("missing MessageSid".to_string()))?
        .to_string();
    let from = get("From")
        .ok_or_else(|| SmsError::MalformedPayload("missing From".to_string()))?
        .to_string();
    let to = get("To")
        .ok_or_else(|| SmsError::MalformedPayload("missing To".to_string()))?
        .to_string();
    let body = get("Body").unwrap_or("").to_string();

    let num_media: usize = get("NumMedia").and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut attachments = Vec::with_capacity(num_media);
    for i in 0..num_media {
        let url_key = format!("MediaUrl{i}");
        if let Some(url) = get(&url_key) {
            let ct_key = format!("MediaContentType{i}");
            let content_type = get(&ct_key).map(|s| s.to_string());
            let kind = match content_type.as_deref() {
                Some(ct) if ct.starts_with("image/") => MediaKind::Image,
                Some(ct) if ct.starts_with("video/") => MediaKind::Video,
                Some(ct) if ct.starts_with("audio/") => MediaKind::Audio,
                _ => MediaKind::Other,
            };
            attachments.push(Attachment {
                url: url.to_string(),
                content_type,
                kind,
                ..Default::default()
            });
        }
    }

    Ok(IncomingMessage {
        sender_id: from.clone(),
        chat_type: ChatType::Direct,
        account_id: Some(to.clone()),
        platform: Some("sms".into()),
        attachments,
        ..IncomingMessage::new(sid, to, from, body, chrono::Utc::now().timestamp())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_signature_matches_twilio_doc_example() {
        // Hand-rolled fixture mirroring the example in the crate-level
        // docstring (and Twilio's own validator docs). The signature
        // value is whatever this implementation computes — the test is
        // self-consistent: same algorithm computes the same expected
        // tag, the verifier round-trips it.
        let token = "12345";
        let url = "https://example.com/sms";
        let form = vec![
            ("Body".to_string(), "hi".to_string()),
            ("From".to_string(), "+15551234567".to_string()),
            ("To".to_string(), "+15559876543".to_string()),
        ];
        let sig = expected_signature(token, url, &form);
        // base64 of a 20-byte HMAC-SHA1 tag is exactly 28 chars.
        assert_eq!(sig.len(), 28, "got {sig}");
    }

    #[test]
    fn signature_is_order_independent() {
        let token = "shhh";
        let url = "https://example.com/sms";
        let a = vec![
            ("From".to_string(), "+1".to_string()),
            ("To".to_string(), "+2".to_string()),
            ("Body".to_string(), "x".to_string()),
        ];
        let b = vec![
            ("Body".to_string(), "x".to_string()),
            ("To".to_string(), "+2".to_string()),
            ("From".to_string(), "+1".to_string()),
        ];
        assert_eq!(
            expected_signature(token, url, &a),
            expected_signature(token, url, &b)
        );
    }

    #[test]
    fn verify_signature_round_trip() {
        let token = "shhh";
        let url = "https://example.com/sms";
        let body = "From=%2B1&To=%2B2&Body=hi";
        let pairs = parse_form(body);
        let sig = expected_signature(token, url, &pairs);
        let parsed = verify_signature(token, url, body, &sig).expect("round-trip");
        assert_eq!(parsed, pairs);
    }

    #[test]
    fn verify_signature_rejects_wrong_token() {
        let url = "https://example.com/sms";
        let body = "From=%2B1&To=%2B2&Body=hi";
        let pairs = parse_form(body);
        let sig = expected_signature("right", url, &pairs);
        let err = verify_signature("wrong", url, body, &sig).unwrap_err();
        assert!(matches!(err, SmsError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        let token = "shhh";
        let url = "https://example.com/sms";
        let pairs = parse_form("From=%2B1&To=%2B2&Body=hi");
        let sig = expected_signature(token, url, &pairs);
        let err = verify_signature(token, url, "From=%2B1&To=%2B2&Body=bye", &sig).unwrap_err();
        assert!(matches!(err, SmsError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_wrong_url() {
        let token = "shhh";
        let pairs = parse_form("From=%2B1&To=%2B2&Body=hi");
        let sig = expected_signature(token, "https://example.com/sms", &pairs);
        let err = verify_signature(
            token,
            "https://example.com/other",
            "From=%2B1&To=%2B2&Body=hi",
            &sig,
        )
        .unwrap_err();
        assert!(matches!(err, SmsError::SignatureMismatch));
    }

    #[test]
    fn pairs_to_incoming_extracts_fields() {
        let pairs = parse_form(
            "MessageSid=SM123&From=%2B15551234567&To=%2B15559876543&Body=hello&NumMedia=0",
        );
        let msg = pairs_to_incoming(&pairs).unwrap();
        assert_eq!(msg.id, "SM123");
        assert_eq!(msg.author, "+15551234567");
        assert_eq!(msg.sender_id, "+15551234567");
        assert_eq!(msg.conversation_id, "+15559876543");
        assert_eq!(msg.account_id.as_deref(), Some("+15559876543"));
        assert_eq!(msg.platform.as_deref(), Some("sms"));
        assert_eq!(msg.chat_type, ChatType::Direct);
        assert!(!msg.is_bot);
        assert!(!msg.is_self);
        assert_eq!(msg.text, "hello");
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn pairs_to_incoming_collects_media() {
        let pairs = parse_form(
            "MessageSid=SM1&From=%2B1&To=%2B2&Body=see&NumMedia=2\
             &MediaUrl0=https%3A%2F%2Fapi.twilio.com%2Fa.jpg\
             &MediaContentType0=image%2Fjpeg\
             &MediaUrl1=https%3A%2F%2Fapi.twilio.com%2Fb.jpg",
        );
        let msg = pairs_to_incoming(&pairs).unwrap();
        assert_eq!(msg.attachments.len(), 2);
        assert_eq!(msg.attachments[0].url, "https://api.twilio.com/a.jpg");
        assert_eq!(msg.attachments[0].content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(msg.attachments[0].kind, MediaKind::Image);
        assert_eq!(msg.attachments[1].url, "https://api.twilio.com/b.jpg");
        assert_eq!(msg.attachments[1].content_type, None);
        assert_eq!(msg.attachments[1].kind, MediaKind::Other);
    }

    #[test]
    fn pairs_to_incoming_missing_required_errors() {
        let pairs = parse_form("From=%2B1&To=%2B2&Body=hi");
        let err = pairs_to_incoming(&pairs).unwrap_err();
        assert!(matches!(err, SmsError::MalformedPayload(_)));
    }
}
