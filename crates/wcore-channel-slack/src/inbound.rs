//! Slack Events API webhook JSON parsing.
//!
//! Two top-level shapes:
//! * `url_verification` — Slack's app-config handshake. The adapter
//!   echoes back the `challenge` field; we surface it as `Parsed::Challenge`
//!   so the engine's webhook router can respond with the right body.
//! * `event_callback` — wraps an inner `event` object. We currently
//!   only translate `message` events into `IncomingMessage`. Other
//!   event types ride through as `Parsed::Ignored` so they don't
//!   surface as errors.

use serde::Deserialize;
use wcore_channels::event::{Attachment, ChannelEvent, ChatType, IncomingMessage, MediaKind};

use crate::error::SlackError;

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Envelope {
    #[serde(rename = "url_verification")]
    UrlVerification { challenge: String },

    #[serde(rename = "event_callback")]
    EventCallback {
        #[serde(default)]
        event: Option<serde_json::Value>,
    },
}

/// Outcome of parsing one webhook body.
///
/// `Event` wraps the enriched `ChannelEvent`, which is intentionally
/// large (the dominant `MessageReceived` variant); boxing it here would
/// only complicate the nested match arms for no real gain.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Parsed {
    /// The webhook is the app-config challenge handshake. The HTTP host
    /// should respond `200 OK` with `challenge` as the body.
    Challenge(String),
    /// The webhook produced a `ChannelEvent` for the inbox queue.
    Event(ChannelEvent),
    /// The webhook was a valid Slack envelope of an event type we don't
    /// currently translate (e.g. `team_join`, `reaction_added`).
    Ignored,
}

/// Parse one webhook body. Caller is responsible for first verifying
/// the signature + timestamp.
pub fn parse_webhook(raw_body: &str) -> Result<Parsed, SlackError> {
    let env: Envelope = serde_json::from_str(raw_body)
        .map_err(|e| SlackError::MalformedPayload(format!("envelope: {e}")))?;
    match env {
        Envelope::UrlVerification { challenge } => Ok(Parsed::Challenge(challenge)),
        Envelope::EventCallback { event: None } => Ok(Parsed::Ignored),
        Envelope::EventCallback { event: Some(ev) } => parse_inner_event(&ev),
    }
}

fn parse_inner_event(ev: &serde_json::Value) -> Result<Parsed, SlackError> {
    let ty = ev
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SlackError::MalformedPayload("inner event missing type".to_string()))?;

    if ty != "message" {
        return Ok(Parsed::Ignored);
    }

    // Skip bot-edits + thread-broadcast echoes etc. — Slack ships these
    // with a `subtype` we don't want to feed back as a fresh user message.
    if ev.get("subtype").is_some()
        && ev.get("subtype").and_then(|v| v.as_str()) != Some("thread_broadcast")
    {
        return Ok(Parsed::Ignored);
    }

    let channel = ev
        .get("channel")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SlackError::MalformedPayload("message event missing channel".to_string()))?;
    let user = ev.get("user").and_then(|v| v.as_str()).unwrap_or("unknown");
    let text = ev.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let ts_str = ev
        .get("ts")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SlackError::MalformedPayload("message event missing ts".to_string()))?;
    // Slack `ts` is "1234567890.123456" — split on '.' to extract seconds.
    let secs: i64 = ts_str
        .split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // --- Attachments ---
    // Map Slack `mimetype` to a coarse `MediaKind`; fall back to Other.
    let attachments: Vec<Attachment> = ev
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let url = f
                        .get("url_private")
                        .or_else(|| f.get("permalink"))
                        .and_then(|v| v.as_str())?;
                    let mime = f.get("mimetype").and_then(|v| v.as_str()).unwrap_or("");
                    let kind = if mime.starts_with("image/") {
                        MediaKind::Image
                    } else if mime.starts_with("video/") {
                        MediaKind::Video
                    } else if mime.starts_with("audio/") {
                        MediaKind::Audio
                    } else if mime.starts_with("application/")
                        || mime.starts_with("text/")
                    {
                        MediaKind::Document
                    } else {
                        MediaKind::Other
                    };
                    let content_type = if mime.is_empty() {
                        None
                    } else {
                        Some(mime.to_owned())
                    };
                    Some(Attachment {
                        url: url.to_owned(),
                        content_type,
                        kind,
                        ..Default::default()
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // --- chat_type ---
    // Prefer explicit `channel_type` field; fall back to channel-id prefix.
    // Slack channel_type values: "im" (1:1 DM), "mpim" (multi-person DM),
    // "channel" (public), "group" (private channel).
    // Channel id prefixes: D = im, G = group/mpim, C = public channel.
    let chat_type = match ev
        .get("channel_type")
        .and_then(|v| v.as_str())
    {
        Some("im") => ChatType::Direct,
        Some("mpim") => ChatType::Group,
        Some("channel" | "group") => ChatType::Channel,
        // No channel_type field — infer from channel id prefix.
        _ => match channel.chars().next() {
            Some('D') => ChatType::Direct,
            Some('G') => ChatType::Group,
            _ => ChatType::Channel,
        },
    };

    // --- Thread / reply ---
    // `thread_ts` is the id of the thread root. If it equals `ts` this
    // message IS the root; otherwise it's a reply within that thread.
    let thread_ts = ev.get("thread_ts").and_then(|v| v.as_str());
    let thread_id = thread_ts.map(str::to_owned);
    let reply_to_message_id = thread_ts
        .filter(|&tts| tts != ts_str)
        .map(str::to_owned);

    // --- Bot flag ---
    // `bot_message` subtype is already filtered above (returns Ignored).
    // A `thread_broadcast` can still arrive here and may carry `bot_id`.
    let is_bot = ev.get("bot_id").is_some();

    // --- Workspace id ---
    let space_id = ev.get("team").and_then(|v| v.as_str()).map(str::to_owned);

    let msg = IncomingMessage {
        id: ts_str.to_string(),
        conversation_id: channel.to_string(),
        author: user.to_string(),
        text: text.to_string(),
        ts_secs: secs,
        attachments,
        // `user` is the stable Slack user id (e.g. U012ABC) — correct
        // access-control key. Falls back to "unknown" only when the event
        // truly has no `user` field (should not happen for human messages).
        sender_id: user.to_string(),
        is_bot,
        chat_type,
        space_id,
        thread_id,
        reply_to_message_id,
        platform: Some("slack".into()),
        // Fields we cannot populate from the inner event alone:
        //   sender_display / sender_handle — require a users.info API call.
        //   sender_alt_id — Slack exposes no secondary stable id in events.
        //   is_self — requires knowing our own bot user id (not in scope).
        //   chat_name — not present in the event payload.
        //   parent_chat_id — not applicable to Slack's flat channel model.
        //   account_id — multi-account routing not tracked at this layer.
        //   was_mentioned / mention_kind — requires our bot user id.
        //   reply_to_text — Slack does not inline quoted text in events.
        ..Default::default()
    };
    Ok(Parsed::Event(ChannelEvent::MessageReceived { msg }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_verification_extracts_challenge() {
        let body = r#"{"type":"url_verification","challenge":"abc123","token":"x"}"#;
        match parse_webhook(body).unwrap() {
            Parsed::Challenge(c) => assert_eq!(c, "abc123"),
            other => panic!("expected Challenge, got {other:?}"),
        }
    }

    #[test]
    fn message_event_round_trips() {
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "channel":"C123",
                "user":"U456",
                "text":"hello world",
                "ts":"1700000000.000100"
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.conversation_id, "C123");
                assert_eq!(msg.author, "U456");
                assert_eq!(msg.text, "hello world");
                assert_eq!(msg.ts_secs, 1700000000);
                assert_eq!(msg.id, "1700000000.000100");
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn message_with_bot_subtype_is_ignored() {
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "subtype":"bot_message",
                "channel":"C123",
                "text":"x",
                "ts":"1700000000.000100"
            }
        }"#;
        assert!(matches!(parse_webhook(body).unwrap(), Parsed::Ignored));
    }

    #[test]
    fn non_message_event_is_ignored() {
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"team_join",
                "user":"U123"
            }
        }"#;
        assert!(matches!(parse_webhook(body).unwrap(), Parsed::Ignored));
    }

    #[test]
    fn malformed_json_errors() {
        let err = parse_webhook("not json at all").unwrap_err();
        assert!(matches!(err, SlackError::MalformedPayload(_)));
    }

    #[test]
    fn message_with_files_extracts_attachments() {
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "channel":"C1",
                "user":"U1",
                "text":"see attached",
                "ts":"1700000000.000200",
                "files":[
                    {"url_private":"https://files.slack.com/a.png","mimetype":"image/png"},
                    {"permalink":"https://files.slack.com/b.jpg","mimetype":"image/jpeg"}
                ]
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.attachments.len(), 2);
                assert_eq!(msg.attachments[0].url, "https://files.slack.com/a.png");
                assert_eq!(msg.attachments[0].kind, MediaKind::Image);
                assert_eq!(
                    msg.attachments[0].content_type.as_deref(),
                    Some("image/png")
                );
                assert_eq!(msg.attachments[1].url, "https://files.slack.com/b.jpg");
                assert_eq!(msg.attachments[1].kind, MediaKind::Image);
            }
            other => panic!("expected MessageReceived with files, got {other:?}"),
        }
    }

    #[test]
    fn message_round_trips_structured_fields() {
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "channel":"D012ABC",
                "user":"U456",
                "text":"hello",
                "ts":"1700000001.000100",
                "team":"T789",
                "channel_type":"im"
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.sender_id, "U456");
                assert_eq!(msg.chat_type, ChatType::Direct);
                assert_eq!(msg.space_id.as_deref(), Some("T789"));
                assert_eq!(msg.platform.as_deref(), Some("slack"));
                assert!(!msg.is_bot);
                assert!(msg.thread_id.is_none());
                assert!(msg.reply_to_message_id.is_none());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn channel_id_prefix_drives_chat_type_fallback() {
        // No channel_type field — inferred from prefix.
        for (channel, expected) in [
            ("D001", ChatType::Direct),
            ("G001", ChatType::Group),
            ("C001", ChatType::Channel),
        ] {
            let body = format!(
                r#"{{"type":"event_callback","event":{{"type":"message","channel":"{channel}","user":"U1","text":"x","ts":"1700000000.000100"}}}}"#
            );
            match parse_webhook(&body).unwrap() {
                Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                    assert_eq!(
                        msg.chat_type, expected,
                        "channel {channel} should map to {expected:?}"
                    );
                }
                other => panic!("expected MessageReceived, got {other:?}"),
            }
        }
    }

    #[test]
    fn threaded_reply_sets_thread_and_reply_ids() {
        // thread_ts != ts  →  reply within an existing thread.
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "channel":"C1",
                "user":"U1",
                "text":"reply",
                "ts":"1700000002.000100",
                "thread_ts":"1700000001.000100"
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.thread_id.as_deref(), Some("1700000001.000100"));
                assert_eq!(
                    msg.reply_to_message_id.as_deref(),
                    Some("1700000001.000100")
                );
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[test]
    fn thread_root_has_thread_id_but_no_reply_to() {
        // thread_ts == ts  →  this message is the thread root itself.
        let body = r#"{
            "type":"event_callback",
            "event": {
                "type":"message",
                "channel":"C1",
                "user":"U1",
                "text":"root",
                "ts":"1700000001.000100",
                "thread_ts":"1700000001.000100"
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.thread_id.as_deref(), Some("1700000001.000100"));
                assert!(msg.reply_to_message_id.is_none());
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }
}
