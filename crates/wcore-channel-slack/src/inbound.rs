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
use wcore_channels::event::{ChannelEvent, IncomingMessage};

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
#[derive(Debug)]
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

    let files = ev
        .get("files")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    f.get("url_private")
                        .or_else(|| f.get("permalink"))
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();

    let msg = IncomingMessage {
        id: ts_str.to_string(),
        conversation_id: channel.to_string(),
        author: user.to_string(),
        text: text.to_string(),
        ts_secs: secs,
        attachments: files,
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
                    {"url_private":"https://files.slack.com/a.png"},
                    {"permalink":"https://files.slack.com/b.jpg"}
                ]
            }
        }"#;
        match parse_webhook(body).unwrap() {
            Parsed::Event(ChannelEvent::MessageReceived { msg }) => {
                assert_eq!(msg.attachments.len(), 2);
                assert_eq!(msg.attachments[0], "https://files.slack.com/a.png");
                assert_eq!(msg.attachments[1], "https://files.slack.com/b.jpg");
            }
            other => panic!("expected MessageReceived with files, got {other:?}"),
        }
    }
}
