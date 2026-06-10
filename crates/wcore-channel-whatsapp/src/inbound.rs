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
use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage, MediaKind};

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
    metadata: Option<RawMetadata>,
    #[serde(default)]
    contacts: Vec<RawContact>,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMetadata {
    /// Receiving phone number id — stable account routing key.
    #[serde(default)]
    phone_number_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawContact {
    /// Stable WhatsApp user id (same value as `messages[].from`).
    #[serde(default)]
    wa_id: Option<String>,
    #[serde(default)]
    profile: Option<RawProfile>,
}

#[derive(Debug, Deserialize)]
struct RawProfile {
    #[serde(default)]
    name: Option<String>,
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
    /// Present when this message is a reply — carries the quoted message id
    /// (and optionally its body if the platform inlines it).
    #[serde(default)]
    context: Option<RawContext>,
    // Media message objects — present when kind != "text".
    #[serde(default)]
    image: Option<RawMedia>,
    #[serde(default)]
    audio: Option<RawMedia>,
    #[serde(default)]
    video: Option<RawMedia>,
    #[serde(default)]
    document: Option<RawMedia>,
}

#[derive(Debug, Deserialize)]
struct RawText {
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawContext {
    /// Message id of the message being replied to.
    #[serde(default)]
    id: Option<String>,
}

/// Shared shape for image / audio / video / document objects.
/// WhatsApp Cloud API returns a media `id` (to be fetched via the Media
/// API), not a direct URL, so we store the media id as the `url` field
/// until a later fetch resolves it to a download URL.
#[derive(Debug, Deserialize)]
struct RawMedia {
    #[serde(default)]
    id: Option<String>,
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

            // Receiving account id (phone_number_id from metadata).
            let account_id = value
                .metadata
                .as_ref()
                .and_then(|m| m.phone_number_id.clone());
            // Destructure before the consuming loop so both fields are
            // accessible inside it without a partial-move error.
            let ChangeValue { contacts, messages, .. } = value;

            for raw in messages {
                let kind = raw.kind.as_deref().unwrap_or("text");

                // Build an attachment list for media message types.
                // WhatsApp Cloud API returns a media id rather than a direct
                // URL; we store it as the `url` field (prefixed with the
                // media id) for the fetching layer to resolve.
                let (body, attachments): (String, Vec<Attachment>) = match kind {
                    "text" => {
                        let text = raw
                            .text
                            .as_ref()
                            .and_then(|t| t.body.clone())
                            .unwrap_or_default();
                        (text, Vec::new())
                    }
                    "image" => {
                        let att = raw.image.as_ref().and_then(|m| m.id.as_deref()).map(|id| {
                            Attachment {
                                url: id.to_string(),
                                kind: MediaKind::Image,
                                ..Default::default()
                            }
                        });
                        (String::new(), att.into_iter().collect())
                    }
                    "audio" => {
                        let att =
                            raw.audio.as_ref().and_then(|m| m.id.as_deref()).map(|id| {
                                Attachment {
                                    url: id.to_string(),
                                    kind: MediaKind::Audio,
                                    ..Default::default()
                                }
                            });
                        (String::new(), att.into_iter().collect())
                    }
                    "video" => {
                        let att =
                            raw.video.as_ref().and_then(|m| m.id.as_deref()).map(|id| {
                                Attachment {
                                    url: id.to_string(),
                                    kind: MediaKind::Video,
                                    ..Default::default()
                                }
                            });
                        (String::new(), att.into_iter().collect())
                    }
                    "document" => {
                        let att =
                            raw.document
                                .as_ref()
                                .and_then(|m| m.id.as_deref())
                                .map(|id| Attachment {
                                    url: id.to_string(),
                                    kind: MediaKind::Document,
                                    ..Default::default()
                                });
                        (String::new(), att.into_iter().collect())
                    }
                    _ => {
                        // Status events / interactive replies / stickers etc. —
                        // surface as PlatformWarning so the engine sees they
                        // arrived without failing the whole envelope.
                        out.push(ChannelEvent::PlatformWarning {
                            message: format!(
                                "ignored non-text whatsapp message kind={kind}"
                            ),
                        });
                        continue;
                    }
                };

                let from = raw.from.clone().unwrap_or_else(|| "unknown".to_string());
                let id = raw.id.unwrap_or_default();
                let ts_secs: i64 = raw
                    .timestamp
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                // Look up the contact entry whose wa_id matches this message's
                // `from` field to resolve the display name.
                let sender_display: Option<String> = contacts
                    .iter()
                    .find(|c| c.wa_id.as_deref() == Some(from.as_str()))
                    .and_then(|c| c.profile.as_ref())
                    .and_then(|p| p.name.clone());

                // WhatsApp Cloud API is strictly 1:1 DMs in the standard
                // messages webhook; group-chat webhooks carry a group id in
                // `from` that starts with a numeric prefix but is distinct from
                // individual phone numbers. The Meta Cloud API does not expose a
                // reliable "is this a group" flag in the messages object itself,
                // so we default to Direct — the correct value for every standard
                // Business API integration. Group-aware connectors should
                // override this after construction if they can detect group ids.
                let chat_type = ChatType::Direct;

                // Reply context: messages[].context.id is the id of the message
                // being replied to.
                let reply_to_message_id: Option<String> =
                    raw.context.as_ref().and_then(|c| c.id.clone());

                let msg = IncomingMessage {
                    id,
                    // conversation_id: for 1:1 DMs on WhatsApp Cloud API the
                    // natural conversation key is the sender's wa_id (= `from`).
                    conversation_id: from.clone(),
                    // author: human-facing label; same as sender_id here because
                    // WhatsApp Cloud API does not provide a separate display-name
                    // in the message object (only in contacts[].profile.name).
                    author: from.clone(),
                    text: body,
                    ts_secs,
                    attachments,
                    // sender_id: the `from` field is the stable wa_id / phone
                    // number id assigned by the platform — the correct
                    // access-control and dedup key.
                    sender_id: from,
                    sender_display,
                    // sender_handle / sender_alt_id: not exposed by WhatsApp
                    // Cloud API in the standard webhook payload.
                    sender_handle: None,
                    sender_alt_id: None,
                    is_bot: false,
                    is_self: false,
                    chat_type,
                    // chat_name / space_id / thread_id / parent_chat_id: not
                    // present in the WhatsApp Cloud API webhook payload.
                    chat_name: None,
                    space_id: None,
                    thread_id: None,
                    parent_chat_id: None,
                    account_id: account_id.clone(),
                    platform: Some("whatsapp".into()),
                    // was_mentioned / mention_kind: bots in 1:1 DMs are always
                    // directly addressed — but WhatsApp has no explicit mention
                    // syntax, so we leave this false; the dispatch kernel can
                    // infer addressing from chat_type == Direct.
                    was_mentioned: false,
                    mention_kind: None,
                    reply_to_message_id,
                    // reply_to_text: WhatsApp Cloud API does not inline the
                    // quoted body in the context object.
                    reply_to_text: None,
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
                assert_eq!(msg.sender_id, "15555550100");
                assert_eq!(msg.conversation_id, "15555550100");
                assert_eq!(msg.id, "wamid.HBgL...");
                assert_eq!(msg.ts_secs, 1700000000);
                assert_eq!(msg.sender_display.as_deref(), Some("Alice"));
                assert_eq!(msg.account_id.as_deref(), Some("PNID"));
                assert_eq!(msg.platform.as_deref(), Some("whatsapp"));
                assert_eq!(msg.chat_type, ChatType::Direct);
                assert!(!msg.is_bot);
                assert!(!msg.is_self);
                assert!(msg.attachments.is_empty());
                assert!(msg.reply_to_message_id.is_none());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn parse_webhook_image_message_produces_attachment() {
        let body = r#"{
            "entry":[{"changes":[{"value":{
                "metadata":{"phone_number_id":"PNID"},
                "contacts":[],
                "messages":[{
                    "from":"15555550100",
                    "id":"wamid.IMG",
                    "timestamp":"1700000001",
                    "type":"image",
                    "image":{"id":"media-abc123"}
                }]
            }}]}]
        }"#;
        let evs = parse_webhook(body).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.attachments.len(), 1);
                assert_eq!(msg.attachments[0].url, "media-abc123");
                assert_eq!(msg.attachments[0].kind, MediaKind::Image);
                assert!(msg.text.is_empty());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn parse_webhook_reply_context_sets_reply_to_message_id() {
        let body = r#"{
            "entry":[{"changes":[{"value":{
                "metadata":{"phone_number_id":"PNID"},
                "contacts":[],
                "messages":[{
                    "from":"15555550100",
                    "id":"wamid.REPLY",
                    "timestamp":"1700000002",
                    "type":"text",
                    "text":{"body":"that's great"},
                    "context":{"id":"wamid.ORIGINAL"}
                }]
            }}]}]
        }"#;
        let evs = parse_webhook(body).unwrap();
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(
                    msg.reply_to_message_id.as_deref(),
                    Some("wamid.ORIGINAL")
                );
                assert!(msg.reply_to_text.is_none());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn parse_webhook_unhandled_kind_surfaces_warning() {
        // Stickers, interactive replies, reactions — not yet translated.
        let body = r#"{
            "entry":[{"changes":[{"value":{"messages":[{
                "from":"15555550100",
                "id":"wamid.X",
                "timestamp":"1700000000",
                "type":"sticker"
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
