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
