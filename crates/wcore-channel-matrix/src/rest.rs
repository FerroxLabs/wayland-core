//! Matrix CS API REST helpers.
//!
//! Implements the send path: `PUT /_matrix/client/v3/rooms/{roomId}/send/m.room.message/{txnId}`.
//! Transaction IDs use a process-local counter (monotonic u64) to make retries idempotent.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::MatrixError;

static TXN_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Serialize)]
struct TextMessageBody<'a> {
    msgtype: &'a str,
    body: &'a str,
}

#[derive(Deserialize)]
struct SendEventResponse {
    event_id: String,
}

/// Send a plain-text `m.room.message` to `room_id` and return the server-assigned `event_id`.
pub async fn send_text_message(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    body: &str,
) -> Result<String, MatrixError> {
    let txn_id = TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let encoded_room = urlencoding::encode(room_id);
    let url =
        format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/send/m.room.message/{txn_id}");

    let payload = TextMessageBody {
        msgtype: "m.text",
        body,
    };

    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(MatrixError::Http { status, body: text });
    }

    let result: SendEventResponse = resp
        .json()
        .await
        .map_err(|e| MatrixError::Parse(e.to_string()))?;

    Ok(result.event_id)
}

#[derive(Serialize)]
struct TypingBody {
    typing: bool,
    timeout: u64,
}

/// Send a typing notification: `PUT /_matrix/client/v3/rooms/{room}/typing/{userId}`.
/// `timeout_ms` is how long the server should show the indicator before
/// auto-clearing it; the subscriber re-sends on a shorter cadence.
pub async fn send_typing(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    user_id: &str,
    timeout_ms: u64,
) -> Result<(), MatrixError> {
    let encoded_room = urlencoding::encode(room_id);
    let encoded_user = urlencoding::encode(user_id);
    let url = format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/typing/{encoded_user}");
    let payload = TypingBody {
        typing: true,
        timeout: timeout_ms,
    };
    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MatrixError::Http { status, body })
    }
}

#[derive(Serialize)]
struct ReactionBody<'a> {
    #[serde(rename = "m.relates_to")]
    relates_to: RelatesTo<'a>,
}

#[derive(Serialize)]
struct RelatesTo<'a> {
    rel_type: &'a str,
    event_id: &'a str,
    key: &'a str,
}

/// Send an `m.reaction` annotation relating to `event_id` with `emoji` as
/// the key: `PUT /_matrix/client/v3/rooms/{room}/send/m.reaction/{txnId}`.
pub async fn send_reaction(
    http: &wcore_egress::EgressClient,
    api_base: &str,
    access_token: &str,
    room_id: &str,
    event_id: &str,
    emoji: &str,
) -> Result<(), MatrixError> {
    let txn_id = TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    let encoded_room = urlencoding::encode(room_id);
    let url = format!("{api_base}/_matrix/client/v3/rooms/{encoded_room}/send/m.reaction/{txn_id}");
    let payload = ReactionBody {
        relates_to: RelatesTo {
            rel_type: "m.annotation",
            event_id,
            key: emoji,
        },
    };
    let resp = http
        .put(&url)
        .bearer_auth(access_token)
        .json(&payload)
        .send()
        .await
        .map_err(|e| MatrixError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    if resp.status().is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(MatrixError::Http { status, body })
    }
}

// ---------------------------------------------------------------------------
// Minimal urlencoding without adding a dep (percent-encode room IDs).
// ---------------------------------------------------------------------------
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 4);
        for byte in s.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(byte as char)
                }
                _ => {
                    out.push('%');
                    out.push(
                        char::from_digit((byte >> 4) as u32, 16)
                            .unwrap()
                            .to_ascii_uppercase(),
                    );
                    out.push(
                        char::from_digit((byte & 0xf) as u32, 16)
                            .unwrap()
                            .to_ascii_uppercase(),
                    );
                }
            }
        }
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn encodes_exclamation_and_colon() {
            let encoded = encode("!room:example.org");
            assert_eq!(encoded, "%21room%3Aexample.org");
        }
    }
}

#[cfg(test)]
mod ack_tests {
    use super::*;

    const TOKEN: &str = "syt_test";
    const ROOM: &str = "!room123:example.org";

    #[tokio::test]
    async fn send_typing_puts_to_typing_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                "/_matrix/client/v3/rooms/%21room123%3Aexample.org/typing/%40bot%3Aexample.org",
            )
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        send_typing(
            &http,
            &server.url(),
            TOKEN,
            ROOM,
            "@bot:example.org",
            30_000,
        )
        .await
        .expect("typing should succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_reaction_puts_annotation_event() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "PUT",
                mockito::Matcher::Regex(
                    r"/_matrix/client/v3/rooms/[^/]+/send/m\.reaction/\d+".to_string(),
                ),
            )
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex(r#""rel_type":"m\.annotation""#.to_string()),
                mockito::Matcher::Regex(r#""event_id":"\$evt1""#.to_string()),
                mockito::Matcher::Regex(r#""key":"👀""#.to_string()),
            ]))
            .with_status(200)
            .with_body(r#"{"event_id":"$react1"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        send_reaction(&http, &server.url(), TOKEN, ROOM, "$evt1", "👀")
            .await
            .expect("reaction should succeed on 200");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn reaction_http_error_surfaces() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("PUT", mockito::Matcher::Any)
            .with_status(403)
            .with_body(r#"{"errcode":"M_FORBIDDEN"}"#)
            .create_async()
            .await;
        let http = wcore_egress::EgressClient::new();
        let err = send_reaction(&http, &server.url(), TOKEN, ROOM, "$evt1", "👀")
            .await
            .expect_err("403 should error");
        assert!(
            matches!(err, MatrixError::Http { status: 403, .. }),
            "got {err:?}"
        );
    }
}
