//! Bot Framework inbound Activity parsing.
//!
//! A Teams bot receives inbound traffic as Bot Framework **Activity** JSON
//! POSTed to its messaging endpoint by the Azure Bot Service. This module
//! turns the slice of that payload we care about into the enriched
//! [`IncomingMessage`] the inbound dispatch kernel consumes.
//!
//! Only `type == "message"` activities produce a message; lifecycle
//! activities (`conversationUpdate`, `typing`, …) parse to `Ok(None)` so the
//! webhook host can ACK them without enqueuing anything.
//!
//! **Round-trip with the send path**: `conversation_id` is encoded as
//! `{serviceUrl}|{conversationId}` so the reply path's `parse_chat_id`
//! recovers the tenant-specific `serviceUrl` the Connector API requires. The
//! `serviceUrl` is taken from the activity itself (Teams stamps it per
//! activity), falling back to the channel's configured service URL.
//!
//! **Attachments.** Teams delivers files as `attachments[]` entries carrying
//! `contentType` / `contentUrl` / `name`. Those are normalised onto the host's
//! [`Attachment`] shape here through [`wcore_channels::media::normalize_all`],
//! so the adapter's declared [`MediaBounds`] are enforced rather than assumed.
//! Two Teams-specific facts shape the mapping and are handled explicitly:
//!
//! * **Not every `attachments[]` entry is a file.** Teams stamps the message's
//!   own rich-text rendering as an entry with `contentType: "text/html"` and an
//!   inline `content` string, and Adaptive Cards arrive the same way. Those
//!   carry NO `contentUrl`. Treating them as attachments would make every
//!   formatted Teams message sprout a phantom document in the agent's turn, so
//!   an entry without a non-empty `contentUrl` is not an attachment.
//! * **The file wrapper's `contentType` is not a media type.** A real file
//!   arrives as `application/vnd.microsoft.teams.file.download.info`, which
//!   classifies to `Other` and would render as that raw vendor string. For that
//!   wrapper we classify from `name` instead, so `report.pdf` reads as a
//!   Document and `shot.png` as an Image.
//!
//! Teams reports no attachment size, so `size_bytes` is `None` — which the
//! normaliser treats as "unknown", not as "small". Fetching the bytes is still
//! a separate auth-gated download against the Graph/Connector API and remains
//! unimplemented (`fetch_media` stays at the trait default); the reference,
//! type and kind reach the agent regardless.

use serde::Deserialize;
use wcore_channels::event::{ChatType, IncomingMessage};
use wcore_channels::media::{MediaBounds, RawAttachment, normalize_all};

use crate::error::MsTeamsError;

/// Teams' wrapper content type for an uploaded file. It describes the envelope,
/// not the media, so classification falls back to the filename.
const TEAMS_FILE_WRAPPER: &str = "application/vnd.microsoft.teams.file.download.info";

/// The slice of a Bot Framework Activity we consume. Every field is
/// `#[serde(default)]` so partial / unfamiliar payloads deserialize rather
/// than 400 — we validate the fields we actually require explicitly.
#[derive(Debug, Deserialize, Default)]
struct Activity {
    #[serde(rename = "type", default)]
    activity_type: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    from: ChannelAccount,
    #[serde(default)]
    recipient: ChannelAccount,
    #[serde(default)]
    conversation: ConversationAccount,
    #[serde(rename = "serviceUrl", default)]
    service_url: String,
    /// RFC3339 timestamp string, e.g. `2026-06-10T12:34:56.789Z`.
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    attachments: Vec<ActivityAttachment>,
}

/// One `attachments[]` entry on a Bot Framework Activity.
///
/// `content` is deliberately NOT deserialized: for a file it is a wrapper
/// object whose `downloadUrl` is a short-lived pre-authenticated link we do not
/// yet fetch, and for rich text / Adaptive Cards it is inline markup we must
/// not surface as a file. `contentUrl`'s presence is what distinguishes the two.
#[derive(Debug, Deserialize, Default)]
struct ActivityAttachment {
    #[serde(rename = "contentType", default)]
    content_type: String,
    #[serde(rename = "contentUrl", default)]
    content_url: String,
    #[serde(default)]
    name: String,
}

/// `from` / `recipient` — a Bot Framework channel account.
#[derive(Debug, Deserialize, Default)]
struct ChannelAccount {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    /// Bot Framework actor role: `"user"` or `"bot"`. Lets the inbound parser
    /// flag bot-authored activities so the dispatch loop guard drops them.
    #[serde(default)]
    role: String,
}

/// `conversation` — the Bot Framework conversation reference.
#[derive(Debug, Deserialize, Default)]
struct ConversationAccount {
    #[serde(default)]
    id: String,
    #[serde(rename = "conversationType", default)]
    conversation_type: String,
    #[serde(rename = "isGroup", default)]
    is_group: bool,
    #[serde(default)]
    name: String,
}

/// Map a Teams conversation descriptor to a [`ChatType`].
///
/// Teams uses `conversationType` of `"personal"` (1:1 DM), `"groupChat"`
/// (ad-hoc group), or `"channel"` (a channel within a team). `isGroup` is a
/// secondary signal for older payloads that omit `conversationType`.
fn chat_type_of(conv: &ConversationAccount) -> ChatType {
    match conv.conversation_type.as_str() {
        "personal" => ChatType::Direct,
        "channel" => ChatType::Channel,
        "groupChat" => ChatType::Group,
        _ if conv.is_group => ChatType::Group,
        _ => ChatType::Direct,
    }
}

/// Map the Activity's `attachments[]` onto host attachments, enforcing `bounds`.
///
/// Entries without a `contentUrl` are not files (see the module docs) and are
/// skipped. Everything that survives that filter is normalised — never dropped:
/// a degraded attachment keeps its URL and type and is still handed to the
/// agent, and the reason is logged. It is logged rather than carried on the
/// message because [`wcore_channels::Attachment`] has no disposition field, and
/// adding one is a wire-contract change this adapter may not make unilaterally.
fn map_attachments(
    raw: &[ActivityAttachment],
    bounds: MediaBounds,
) -> Vec<wcore_channels::Attachment> {
    let candidates: Vec<RawAttachment> = raw
        .iter()
        .filter(|a| !a.content_url.trim().is_empty())
        .map(|a| {
            // The Teams file wrapper describes the envelope, not the media, so
            // let the filename classify it instead of the vendor MIME.
            let content_type = if a.content_type.is_empty()
                || a.content_type.eq_ignore_ascii_case(TEAMS_FILE_WRAPPER)
            {
                None
            } else {
                Some(a.content_type.clone())
            };
            RawAttachment {
                url: a.content_url.clone(),
                content_type,
                // Teams reports no size on the activity. `None` means unknown,
                // which normalize() is explicit about NOT treating as small.
                size_bytes: None,
                filename: if a.name.is_empty() {
                    None
                } else {
                    Some(a.name.clone())
                },
            }
        })
        .collect();

    normalize_all(&candidates, bounds)
        .into_iter()
        .map(|(attachment, disposition)| {
            if let Some(reason) = disposition.reason() {
                tracing::warn!(
                    url = %attachment.url,
                    kind = ?attachment.kind,
                    reason,
                    "msteams inbound attachment degraded (retained, not fetchable)"
                );
            }
            attachment
        })
        .collect()
}

/// Parse a Bot Framework Activity JSON body into an [`IncomingMessage`].
///
/// Returns:
/// * `Ok(Some(msg))` for a `type == "message"` activity.
/// * `Ok(None)` for any other activity type (lifecycle events such as
///   `conversationUpdate`, `typing`, message reactions, …).
/// * `Err(MsTeamsError::Parse)` if the JSON is malformed or a `message`
///   activity is missing the required `from.id` (the access-control / dedup
///   key — we refuse to fabricate it).
///
/// `service_url_fallback` is used to build `conversation_id` when the
/// activity omits its own `serviceUrl` (the channel's configured service URL).
/// `bounds` is the adapter's declared media intake bound, applied to
/// `attachments[]` — the caller passes `Channel::media_bounds()`.
pub fn activity_to_incoming(
    raw_body: &str,
    service_url_fallback: &str,
    bounds: MediaBounds,
) -> Result<Option<IncomingMessage>, MsTeamsError> {
    let activity: Activity =
        serde_json::from_str(raw_body).map_err(|e| MsTeamsError::Parse(e.to_string()))?;

    // Only message activities carry user text; everything else is a
    // lifecycle/control event the host can ACK without enqueuing.
    if activity.activity_type != "message" {
        return Ok(None);
    }

    if activity.from.id.is_empty() {
        return Err(MsTeamsError::Parse(
            "message activity missing from.id".to_string(),
        ));
    }

    // serviceUrl|conversationId so the reply path (parse_chat_id) recovers
    // the tenant-specific serviceUrl. Strip a trailing slash on the
    // serviceUrl so the encoding is stable regardless of how Teams stamps it.
    let service_url = if activity.service_url.is_empty() {
        service_url_fallback
    } else {
        activity.service_url.as_str()
    };
    let service_url = service_url.strip_suffix('/').unwrap_or(service_url);
    let conversation_id = format!("{service_url}|{}", activity.conversation.id);

    // RFC3339 timestamp → epoch seconds; fall back to now if absent/unparsable.
    let ts_secs = chrono::DateTime::parse_from_rfc3339(&activity.timestamp)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|_| chrono::Utc::now().timestamp());

    let chat_type = chat_type_of(&activity.conversation);
    let chat_name = if activity.conversation.name.is_empty() {
        None
    } else {
        Some(activity.conversation.name.clone())
    };
    let sender_display = if activity.from.name.is_empty() {
        None
    } else {
        Some(activity.from.name.clone())
    };
    // recipient.id is the bot identity that received this activity.
    let account_id = if activity.recipient.id.is_empty() {
        None
    } else {
        Some(activity.recipient.id.clone())
    };

    // Author label: prefer the display name, fall back to the stable id.
    let author = if activity.from.name.is_empty() {
        activity.from.id.clone()
    } else {
        activity.from.name.clone()
    };

    let msg = IncomingMessage {
        sender_id: activity.from.id.clone(),
        sender_display,
        chat_type,
        chat_name,
        account_id,
        platform: Some("msteams".into()),
        // Bot Framework stamps the sender's role; a "bot" actor is another bot,
        // so flag it and let the dispatch kernel's loop guard drop it instead of
        // engaging in a bot-to-bot loop. Teams does not echo the bot's own
        // outbound back, so is_self has no reliable signal and stays false.
        is_bot: activity.from.role.eq_ignore_ascii_case("bot"),
        attachments: map_attachments(&activity.attachments, bounds),
        ..IncomingMessage::new(activity.id, conversation_id, author, activity.text, ts_secs)
    };

    Ok(Some(msg))
}

/// Extract just the `serviceUrl` from a Bot Framework Activity body, without
/// the full enrichment parse.
///
/// Used by the webhook auth gate to cross-check the JWT's `serviceurl` claim
/// against the Activity body (defense-in-depth against a token replayed
/// alongside a swapped `serviceUrl`). Returns `Ok(None)` when the activity
/// omits `serviceUrl`; `Err(Parse)` only on malformed JSON.
pub fn service_url_of(raw_body: &str) -> Result<Option<String>, MsTeamsError> {
    #[derive(Deserialize, Default)]
    struct ServiceUrlOnly {
        #[serde(rename = "serviceUrl", default)]
        service_url: String,
    }
    let parsed: ServiceUrlOnly =
        serde_json::from_str(raw_body).map_err(|e| MsTeamsError::Parse(e.to_string()))?;
    if parsed.service_url.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parsed.service_url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use wcore_channels::MediaKind;

    const SERVICE_FALLBACK: &str = "https://smba.trafficmanager.net/amer/";

    /// The adapter's declared intake bound, as `ingest_activity` passes it.
    fn bounds() -> MediaBounds {
        MediaBounds::default()
    }

    #[test]
    fn message_activity_parses_enriched_fields() {
        let body = r#"{
            "type": "message",
            "id": "1622471234567",
            "text": "hello bot",
            "serviceUrl": "https://smba.trafficmanager.net/emea/",
            "timestamp": "2026-06-10T12:34:56.789Z",
            "from": { "id": "29:user-aad-id", "name": "Ada Lovelace" },
            "recipient": { "id": "28:bot-app-id", "name": "Wayland" },
            "conversation": {
                "id": "19:abc@thread.v2",
                "conversationType": "personal",
                "isGroup": false,
                "name": "Ada / Wayland"
            }
        }"#;

        let msg = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .expect("parse ok")
            .expect("message activity yields Some");

        assert_eq!(msg.id, "1622471234567");
        assert_eq!(msg.sender_id, "29:user-aad-id");
        assert_eq!(msg.author, "Ada Lovelace");
        assert_eq!(msg.sender_display.as_deref(), Some("Ada Lovelace"));
        assert_eq!(msg.text, "hello bot");
        assert_eq!(msg.account_id.as_deref(), Some("28:bot-app-id"));
        assert_eq!(msg.platform.as_deref(), Some("msteams"));
        assert_eq!(msg.chat_type, ChatType::Direct);
        assert_eq!(msg.chat_name.as_deref(), Some("Ada / Wayland"));
        // conversation_id uses the activity's own serviceUrl (trailing slash
        // stripped) so parse_chat_id round-trips it on the reply path.
        assert_eq!(
            msg.conversation_id,
            "https://smba.trafficmanager.net/emea|19:abc@thread.v2"
        );
        // 2026-06-10T12:34:56Z epoch seconds.
        assert_eq!(msg.ts_secs, 1_781_094_896);
        assert!(!msg.is_self);
        // A normal user activity (no "bot" role) is not flagged as a bot.
        assert!(!msg.is_bot);
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn bot_role_activity_is_flagged_is_bot() {
        // An activity whose `from.role` is "bot" must set is_bot so the dispatch
        // kernel's loop guard drops it (prevents bot-to-bot loops).
        let body = r#"{
            "type": "message",
            "id": "id-bot",
            "text": "automated",
            "from": { "id": "28:other-bot", "name": "Other Bot", "role": "bot" },
            "conversation": { "id": "19:abc@thread.v2" },
            "serviceUrl": "https://smba.trafficmanager.net/emea/",
            "timestamp": "2026-06-10T12:34:56Z"
        }"#;
        let msg = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .expect("parses")
            .expect("is a message");
        assert!(msg.is_bot, "from.role=bot must set is_bot");
    }

    #[test]
    fn group_chat_maps_to_group_chat_type() {
        let body = r#"{
            "type": "message",
            "id": "id1",
            "text": "hi all",
            "from": { "id": "29:u", "name": "U" },
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:room@thread.v2", "conversationType": "groupChat" }
        }"#;
        let msg = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .unwrap()
            .unwrap();
        assert_eq!(msg.chat_type, ChatType::Group);
        // No serviceUrl on the activity → fallback is used (slash stripped).
        assert_eq!(
            msg.conversation_id,
            "https://smba.trafficmanager.net/amer|19:room@thread.v2"
        );
    }

    #[test]
    fn conversation_update_yields_none() {
        let body = r#"{
            "type": "conversationUpdate",
            "id": "id2",
            "membersAdded": [{ "id": "29:u" }],
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:abc@thread.v2" }
        }"#;
        let out = activity_to_incoming(body, SERVICE_FALLBACK, bounds()).expect("parse ok");
        assert!(out.is_none(), "non-message activity must yield None");
    }

    #[test]
    fn message_missing_from_id_errors() {
        let body = r#"{
            "type": "message",
            "id": "id3",
            "text": "anon",
            "from": { "name": "No Id" },
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:abc@thread.v2", "conversationType": "personal" }
        }"#;
        let err = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .expect_err("missing from.id errors");
        assert!(matches!(err, MsTeamsError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn file_attachment_is_parsed_and_classified_by_filename() {
        // A real Teams file upload: the contentType is the vendor WRAPPER, not
        // a media type. Classifying from it would yield `Other` and render the
        // raw vendor string to the agent; the filename is the usable signal.
        let body = r#"{
            "type": "message",
            "id": "att-1",
            "text": "here you go",
            "from": { "id": "29:u", "name": "U" },
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:abc@thread.v2", "conversationType": "personal" },
            "attachments": [
                {
                    "contentType": "application/vnd.microsoft.teams.file.download.info",
                    "contentUrl": "https://contoso.sharepoint.com/personal/u/report.pdf",
                    "name": "report.pdf",
                    "content": { "downloadUrl": "https://download/x", "fileType": "pdf" }
                },
                {
                    "contentType": "image/png",
                    "contentUrl": "https://contoso.sharepoint.com/personal/u/shot.png",
                    "name": "shot.png"
                }
            ]
        }"#;
        let msg = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .unwrap()
            .unwrap();
        assert_eq!(msg.attachments.len(), 2, "both files must survive");

        // Wrapper MIME dropped → classified from `report.pdf`.
        assert_eq!(
            msg.attachments[0].url,
            "https://contoso.sharepoint.com/personal/u/report.pdf"
        );
        assert_eq!(msg.attachments[0].kind, MediaKind::Document);
        assert_eq!(
            msg.attachments[0].content_type, None,
            "the vendor wrapper is not a media type and must not be reported as one"
        );

        // A genuine MIME is kept and used.
        assert_eq!(msg.attachments[1].kind, MediaKind::Image);
        assert_eq!(
            msg.attachments[1].content_type.as_deref(),
            Some("image/png")
        );
        assert!(msg.attachments[1].path.is_none(), "nothing is fetched here");
    }

    #[test]
    fn rich_text_and_card_entries_are_not_attachments() {
        // NEGATIVE CONTROL for the phantom-attachment failure: Teams stamps a
        // formatted message's own HTML rendering into `attachments[]`, and
        // Adaptive Cards arrive the same way. Neither carries a `contentUrl`.
        // If this ever regresses, every formatted Teams message grows a bogus
        // document in the agent's turn prompt.
        let body = r#"{
            "type": "message",
            "id": "att-2",
            "text": "formatted",
            "from": { "id": "29:u", "name": "U" },
            "recipient": { "id": "28:bot" },
            "conversation": { "id": "19:abc@thread.v2" },
            "attachments": [
                { "contentType": "text/html", "content": "<p>formatted</p>" },
                {
                    "contentType": "application/vnd.microsoft.card.adaptive",
                    "content": { "type": "AdaptiveCard", "body": [] }
                },
                { "contentType": "image/png", "contentUrl": "   " }
            ]
        }"#;
        let msg = activity_to_incoming(body, SERVICE_FALLBACK, bounds())
            .unwrap()
            .unwrap();
        assert!(
            msg.attachments.is_empty(),
            "inline-content entries are not files; got {:?}",
            msg.attachments
        );
    }

    #[test]
    fn attachments_past_the_declared_bound_are_retained_not_dropped() {
        // The bound withholds the FETCH; it must never shorten the list, or the
        // agent answers about a message it was silently shown less of.
        let entries: Vec<String> = (0..5)
            .map(|i| {
                format!(
                    r#"{{"contentType":"image/png","contentUrl":"https://x/{i}.png","name":"{i}.png"}}"#
                )
            })
            .collect();
        let body = format!(
            r#"{{
                "type": "message",
                "id": "att-3",
                "text": "many",
                "from": {{ "id": "29:u" }},
                "recipient": {{ "id": "28:bot" }},
                "conversation": {{ "id": "19:abc@thread.v2" }},
                "attachments": [{}]
            }}"#,
            entries.join(",")
        );
        let tight = MediaBounds {
            max_bytes: MediaBounds::DEFAULT_MAX_BYTES,
            max_attachments: 2,
        };
        let msg = activity_to_incoming(&body, SERVICE_FALLBACK, tight)
            .unwrap()
            .unwrap();
        assert_eq!(
            msg.attachments.len(),
            5,
            "all five survive the bound; degradation is not truncation"
        );
        assert_eq!(msg.attachments[4].url, "https://x/4.png");
    }

    #[test]
    fn malformed_json_errors() {
        let err = activity_to_incoming("{ not json", SERVICE_FALLBACK, bounds())
            .expect_err("bad json errors");
        assert!(matches!(err, MsTeamsError::Parse(_)), "got {err:?}");
    }
}
