//! JSON-RPC 2.0 wire types used to talk to `signal-cli jsonRpc`.
//!
//! signal-cli speaks a strict JSON-RPC 2.0 dialect over stdio, line
//! delimited (one JSON document per line). Requests carry an integer
//! `id`; the response with the same id closes the round-trip.
//! Server-pushed notifications (inbound messages, sync events, …)
//! arrive with no `id` and a `method`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize)]
pub struct Request<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: Value,
    pub id: u64,
}

impl<'a> Request<'a> {
    pub fn new(id: u64, method: &'a str, params: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
            id,
        }
    }
}

/// One parsed line from signal-cli's stdout. Either:
/// - a response to one of our requests (carries `id`), or
/// - a server-pushed notification (carries `method`, no `id`).
///
/// We accept both with a permissive shape, then classify in the
/// reader task. signal-cli is allowed to send shapes we don't care
/// about (other notification methods) — those are logged + dropped.
#[derive(Debug, Clone, Deserialize)]
pub struct Frame {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Shape of the `params` payload for the server-pushed `receive`
/// notification. Many fields signal-cli sends are ignored — we only
/// care about what's needed to populate an `IncomingMessage`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReceiveParams {
    pub envelope: Envelope,
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    /// Sender's address (phone number or UUID).
    #[serde(default)]
    pub source: Option<String>,
    /// Sender's display name when known.
    #[serde(default, rename = "sourceName")]
    pub source_name: Option<String>,
    /// Sender's UUID when known.
    #[serde(default, rename = "sourceUuid")]
    pub source_uuid: Option<String>,
    /// Server-side timestamp (ms since epoch).
    #[serde(default)]
    pub timestamp: Option<i64>,
    /// Data message — present for user-sent text messages.
    #[serde(default, rename = "dataMessage")]
    pub data_message: Option<DataMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DataMessage {
    #[serde(default)]
    pub message: Option<String>,
    /// Group context, if this is a group message.
    #[serde(default, rename = "groupInfo")]
    pub group_info: Option<GroupInfo>,
    /// Server-side timestamp (ms since epoch). signal-cli echoes the
    /// envelope timestamp inside `dataMessage`, but we prefer the
    /// envelope's value when both are present.
    #[serde(default)]
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupInfo {
    /// Base64-encoded group id.
    #[serde(default, rename = "groupId")]
    pub group_id: Option<String>,
}

/// Shape of the `result` field returned by a successful `send`
/// JSON-RPC call. signal-cli returns a list of per-recipient results;
/// we surface the first one's timestamp as the canonical receipt id.
#[derive(Debug, Clone, Deserialize)]
pub struct SendResult {
    /// Server-side message timestamp (ms since epoch). Used as the
    /// platform-assigned message id in the `MessageReceipt`.
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub results: Option<Vec<SendResultEntry>>,
}

/// One per-recipient delivery outcome inside a `send` result. signal-cli
/// reports a `type` per recipient: `"SUCCESS"` on delivery, or a failure
/// discriminant (`"UNREGISTERED_FAILURE"`, `"NETWORK_FAILURE"`,
/// `"IDENTITY_FAILURE"`, …) otherwise. Fields beyond `type` (recipient
/// address, etc.) are intentionally not captured — they carry no
/// content we surface, and parsing them would risk leaking PII into
/// logs.
#[derive(Debug, Clone, Deserialize)]
pub struct SendResultEntry {
    /// Delivery outcome discriminant. Treated case-insensitively; any
    /// value other than `SUCCESS` is a per-recipient failure.
    #[serde(default, rename = "type")]
    pub result_type: Option<String>,
}

impl SendResultEntry {
    /// `true` when this entry reports a successful delivery. A missing
    /// `type` is treated as success: older signal-cli builds omit the
    /// field entirely on the happy path, and we must not regress those
    /// sends into spurious failures.
    pub fn is_success(&self) -> bool {
        match self.result_type.as_deref() {
            None => true,
            Some(t) => t.eq_ignore_ascii_case("SUCCESS"),
        }
    }
}

/// Aggregate delivery outcome computed from a `send` result's
/// per-recipient `results` array.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Every recipient succeeded (or no per-recipient detail was
    /// reported, which signal-cli does for trivially-accepted sends).
    AllSucceeded,
    /// Some — but not all — recipients failed. Carries the failed and
    /// total counts (counts only, never recipient identities).
    Partial { failed: usize, total: usize },
    /// Every recipient failed.
    AllFailed { total: usize },
}

/// Classify a `send` result's per-recipient `results` into a single
/// [`DeliveryOutcome`]. An absent or empty `results` array is treated
/// as `AllSucceeded`: signal-cli omits per-recipient detail for sends
/// it accepts outright, and the round-trip already succeeded by the
/// time we parse this.
pub fn classify_delivery(results: Option<&[SendResultEntry]>) -> DeliveryOutcome {
    let entries = match results {
        Some(e) if !e.is_empty() => e,
        _ => return DeliveryOutcome::AllSucceeded,
    };
    let total = entries.len();
    let failed = entries.iter().filter(|e| !e.is_success()).count();
    match failed {
        0 => DeliveryOutcome::AllSucceeded,
        f if f == total => DeliveryOutcome::AllFailed { total },
        f => DeliveryOutcome::Partial { failed: f, total },
    }
}

/// Build the `params` object for a `send` JSON-RPC call.
///
/// signal-cli's `send` method takes EITHER a `recipient` array (1:1 /
/// direct addresses) OR a `groupId` (groups) — never both. We pick
/// based on the conversation id's shape: a value that starts with `+`
/// (an e164 phone number) is a direct recipient; anything else is
/// treated as a base64 group id. This mirrors how inbound group
/// messages are keyed — `build_incoming` sets `conversation_id` to the
/// raw base64 `groupId` for group envelopes and to the `+`-prefixed
/// source for direct ones, so the discriminator is symmetric across the
/// inbound/outbound boundary.
pub fn build_send_params(conversation_id: &str, text: &str) -> Value {
    if is_direct_recipient(conversation_id) {
        json!({
            "recipient": [conversation_id],
            "message": text,
        })
    } else {
        json!({
            "groupId": conversation_id,
            "message": text,
        })
    }
}

/// Discriminator between a direct recipient and a group id. A direct
/// recipient is an e164 phone number: a leading `+` followed by ALL digits.
/// The leading `+` alone is NOT sufficient — Signal group ids are base64
/// (the standard alphabet includes `+`), so an id like `+abc/123==` starts
/// with `+` yet must route via `groupId`. Requiring digits after the `+`
/// disambiguates the ~1.5% of group ids that happen to start with `+`.
pub fn is_direct_recipient(conversation_id: &str) -> bool {
    match conversation_id.strip_prefix('+') {
        Some(rest) => !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_send_params_uses_recipient_for_phone_number() {
        let params = build_send_params("+15550001111", "hi");
        assert_eq!(params["recipient"][0], "+15550001111");
        assert_eq!(params["message"], "hi");
        // A direct send must NOT carry a groupId — signal-cli rejects both.
        assert!(params.get("groupId").is_none());
    }

    #[test]
    fn build_send_params_uses_group_id_for_group_conversation() {
        // Base64-ish group id (as keyed by inbound `groupInfo.groupId`).
        let gid = "abcd1234EFGH==";
        let params = build_send_params(gid, "hello group");
        assert_eq!(params["groupId"], gid);
        assert_eq!(params["message"], "hello group");
        // A group send must NOT carry a recipient array.
        assert!(params.get("recipient").is_none());
    }

    #[test]
    fn is_direct_recipient_discriminates_on_leading_plus() {
        assert!(is_direct_recipient("+15551234567"));
        assert!(!is_direct_recipient("abcd1234EFGH=="));
        assert!(!is_direct_recipient(""));
        // A base64 group id that happens to start with '+' is NOT a phone
        // number and must route via groupId, not recipient.
        assert!(!is_direct_recipient("+abc/123Qo=="));
        assert!(!is_direct_recipient("+"));
        // build_send_params must follow the same discrimination.
        let p = build_send_params("+abc/123Qo==", "hi");
        assert!(p.get("recipient").is_none());
        assert_eq!(p["groupId"], "+abc/123Qo==");
    }

    #[test]
    fn classify_delivery_absent_or_empty_results_is_success() {
        assert_eq!(classify_delivery(None), DeliveryOutcome::AllSucceeded);
        assert_eq!(classify_delivery(Some(&[])), DeliveryOutcome::AllSucceeded);
    }

    fn entry(ty: Option<&str>) -> SendResultEntry {
        SendResultEntry {
            result_type: ty.map(|s| s.to_string()),
        }
    }

    #[test]
    fn classify_delivery_all_success_entries() {
        let entries = [entry(Some("SUCCESS")), entry(None)];
        assert_eq!(
            classify_delivery(Some(&entries)),
            DeliveryOutcome::AllSucceeded
        );
    }

    #[test]
    fn classify_delivery_all_failed_returns_all_failed() {
        let entries = [
            entry(Some("UNREGISTERED_FAILURE")),
            entry(Some("NETWORK_FAILURE")),
        ];
        assert_eq!(
            classify_delivery(Some(&entries)),
            DeliveryOutcome::AllFailed { total: 2 }
        );
    }

    #[test]
    fn classify_delivery_some_failed_returns_partial_with_counts() {
        let entries = [
            entry(Some("SUCCESS")),
            entry(Some("IDENTITY_FAILURE")),
            entry(Some("SUCCESS")),
        ];
        assert_eq!(
            classify_delivery(Some(&entries)),
            DeliveryOutcome::Partial {
                failed: 1,
                total: 3
            }
        );
    }

    #[test]
    fn send_result_entry_is_success_is_case_insensitive() {
        assert!(entry(Some("success")).is_success());
        assert!(entry(Some("SUCCESS")).is_success());
        assert!(entry(None).is_success());
        assert!(!entry(Some("network_failure")).is_success());
    }

    #[test]
    fn send_result_parses_typed_per_recipient_entries() {
        // Round-trip the real signal-cli `result` shape through serde so
        // the typed `results` array stays compatible with the wire form.
        let raw = serde_json::json!({
            "timestamp": 1700000000000i64,
            "results": [
                {"recipientAddress": {"number": "+15550001111"}, "type": "SUCCESS"},
                {"recipientAddress": {"number": "+15550002222"}, "type": "UNREGISTERED_FAILURE"}
            ]
        });
        let parsed: SendResult = serde_json::from_value(raw).unwrap();
        let outcome = classify_delivery(parsed.results.as_deref());
        assert_eq!(
            outcome,
            DeliveryOutcome::Partial {
                failed: 1,
                total: 2
            }
        );
    }
}
