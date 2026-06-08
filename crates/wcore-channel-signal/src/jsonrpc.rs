//! JSON-RPC 2.0 wire types used to talk to `signal-cli jsonRpc`.
//!
//! signal-cli speaks a strict JSON-RPC 2.0 dialect over stdio, line
//! delimited (one JSON document per line). Requests carry an integer
//! `id`; the response with the same id closes the round-trip.
//! Server-pushed notifications (inbound messages, sync events, …)
//! arrive with no `id` and a `method`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub results: Option<Vec<Value>>,
}
