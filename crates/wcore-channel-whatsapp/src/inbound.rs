//! WhatsApp Cloud API webhook signature verification + JSON parsing.
//!
//! Meta signs every webhook POST with HMAC-SHA256 over the raw request
//! body keyed by the **app secret**. The signature header is
//! `X-Hub-Signature-256: sha256=<hex>`. There is no timestamp header
//! and no replay-protection window in the Meta protocol — the engine's
//! webhook router is expected to short-circuit duplicate `id` values
//! at a higher layer.
//!
//! Webhook body shape (simplified):
//! ```json
//! {
//!   "object":"whatsapp_business_account",
//!   "entry":[{
//!     "id":"...",
//!     "changes":[{
//!       "value":{
//!         "messaging_product":"whatsapp",
//!         "metadata":{...},
//!         "contacts":[{"profile":{"name":"X"},"wa_id":"15555550100"}],
//!         "messages":[{
//!           "from":"15555550100",
//!           "id":"wamid.HBg...",
//!           "timestamp":"1700000000",
//!           "text":{"body":"hi"},
//!           "type":"text"
//!         }]
//!       },
//!       "field":"messages"
//!     }]
//!   }]
//! }
//! ```

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use wcore_channels::event::{ChannelEvent, IncomingMessage};

use crate::error::WhatsappError;

type HmacSha256 = Hmac<Sha256>;

/// Compute the expected `X-Hub-Signature-256` value for a raw webhook body.
///
/// Format per Meta docs: `sha256=<hex(hmac_sha256(app_secret, raw_body))>`.
pub fn expected_signature(app_secret: &str, raw_body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(raw_body);
    let tag = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(tag))
}

/// Constant-time signature comparison wrapped around `hmac::Mac::verify_slice`.
pub fn verify_signature(
    app_secret: &str,
    raw_body: &[u8],
    received_signature: &str,
) -> Result<(), WhatsappError> {
    let received = received_signature
        .strip_prefix("sha256=")
        .ok_or(WhatsappError::SignatureMismatch)?;
    let received_bytes = hex::decode(received).map_err(|_| WhatsappError::SignatureMismatch)?;

    let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(raw_body);
    mac.verify_slice(&received_bytes)
        .map_err(|_| WhatsappError::SignatureMismatch)
}

/// Top-level webhook envelope. We use a fully-typed parse for the
/// `entry[].changes[].value.messages[]` path so unknown variants don't
/// fail the whole envelope.
#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    entry: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    changes: Vec<Change>,
}

#[derive(Debug, Deserialize)]
struct Change {
    #[serde(default)]
    value: Option<ChangeValue>,
}

#[derive(Debug, Deserialize)]
struct ChangeValue {
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    text: Option<RawText>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawText {
    #[serde(default)]
    body: Option<String>,
}

/// Parse one webhook body. Caller is responsible for first verifying
/// the signature. Returns the list of `ChannelEvent`s to enqueue —
/// a single POST can carry multiple messages.
pub fn parse_webhook(raw_body: &str) -> Result<Vec<ChannelEvent>, WhatsappError> {
    let env: Envelope = serde_json::from_str(raw_body)
        .map_err(|e| WhatsappError::MalformedPayload(format!("envelope: {e}")))?;

    let mut out = Vec::new();
    for entry in env.entry {
        for change in entry.changes {
            let Some(value) = change.value else { continue };
            for raw in value.messages {
                // We currently translate only `text` messages — status
                // events / media / interactive replies surface as
                // PlatformWarning so they don't fail the whole envelope
                // and so the engine sees they arrived.
                let kind = raw.kind.as_deref().unwrap_or("text");
                if kind != "text" {
                    out.push(ChannelEvent::PlatformWarning {
                        message: format!("ignored non-text whatsapp message kind={kind}"),
                    });
                    continue;
                }
                let body = raw
                    .text
                    .as_ref()
                    .and_then(|t| t.body.clone())
                    .unwrap_or_default();
                let from = raw.from.unwrap_or_else(|| "unknown".to_string());
                let id = raw.id.unwrap_or_default();
                let ts_secs: i64 = raw
                    .timestamp
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let msg = IncomingMessage {
                    id: id.clone(),
                    conversation_id: from.clone(),
                    author: from,
                    text: body,
                    ts_secs,
                    attachments: Vec::new(),
                };
                out.push(ChannelEvent::MessageReceived { msg });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "shhh";

    #[test]
    fn expected_signature_shape_is_sha256_hex() {
        let sig = expected_signature(SECRET, b"body");
        assert!(sig.starts_with("sha256="));
        // HMAC-SHA256 hex = 64 chars after the "sha256=" prefix.
        assert_eq!(sig.len(), 7 + 64);
        assert!(sig[7..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn verify_signature_accepts_valid() {
        let body = br#"{"entry":[]}"#;
        let sig = expected_signature(SECRET, body);
        verify_signature(SECRET, body, &sig).expect("valid signature should verify");
    }

    #[test]
    fn verify_signature_rejects_tampered_body() {
        let body = br#"{"entry":[]}"#;
        let sig = expected_signature(SECRET, body);
        let err = verify_signature(SECRET, br#"{"entry":[1]}"#, &sig).unwrap_err();
        assert!(matches!(err, WhatsappError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_wrong_secret() {
        let body = br#"{"entry":[]}"#;
        let sig = expected_signature(SECRET, body);
        let err = verify_signature("nope", body, &sig).unwrap_err();
        assert!(matches!(err, WhatsappError::SignatureMismatch));
    }

    #[test]
    fn verify_signature_rejects_malformed_header() {
        let err = verify_signature(SECRET, b"body", "garbage").unwrap_err();
        assert!(matches!(err, WhatsappError::SignatureMismatch));
    }

    #[test]
    fn parse_webhook_extracts_single_text_message() {
        let body = r#"{
            "object":"whatsapp_business_account",
            "entry":[{
                "id":"WABA_ID",
                "changes":[{
                    "value":{
                        "messaging_product":"whatsapp",
                        "metadata":{"display_phone_number":"+15550000","phone_number_id":"PNID"},
                        "contacts":[{"profile":{"name":"Alice"},"wa_id":"15555550100"}],
                        "messages":[{
                            "from":"15555550100",
                            "id":"wamid.HBgL...",
                            "timestamp":"1700000000",
                            "text":{"body":"hello there"},
                            "type":"text"
                        }]
                    },
                    "field":"messages"
                }]
            }]
        }"#;
        let evs = parse_webhook(body).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hello there");
                assert_eq!(msg.author, "15555550100");
                assert_eq!(msg.conversation_id, "15555550100");
                assert_eq!(msg.id, "wamid.HBgL...");
                assert_eq!(msg.ts_secs, 1700000000);
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn parse_webhook_non_text_kind_surfaces_warning() {
        let body = r#"{
            "entry":[{"changes":[{"value":{"messages":[{
                "from":"15555550100",
                "id":"wamid.X",
                "timestamp":"1700000000",
                "type":"image",
                "image":{"id":"media-id"}
            }]}}]}]
        }"#;
        let evs = parse_webhook(body).unwrap();
        assert_eq!(evs.len(), 1);
        assert!(matches!(evs[0], ChannelEvent::PlatformWarning { .. }));
    }

    #[test]
    fn parse_webhook_malformed_json_errors() {
        let err = parse_webhook("not json at all").unwrap_err();
        assert!(matches!(err, WhatsappError::MalformedPayload(_)));
    }

    #[test]
    fn parse_webhook_empty_entry_is_ok() {
        let evs = parse_webhook(r#"{"entry":[]}"#).unwrap();
        assert!(evs.is_empty());
    }
}
