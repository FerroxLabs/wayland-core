//! USD cost extraction from the `session_cost` json-stream event.
//!
//! Cross-audit C-2 — `ProtocolSink` (json-stream mode) is the ONLY sink
//! that emits USD figures. TerminalSink prints token counts on stderr in
//! a `[turns: ... | tokens: ...]` block, never dollars. Session JSON has
//! `total_usage: TokenUsage` (tokens only). So the json-stream
//! `session_cost` event is the single authoritative source we parse here.
//!
//! ## Wire shape
//!
//! `ProtocolEvent` in `wcore-protocol::events` derives `Serialize` only
//! (not `Deserialize`) — it is the host-facing emit-side schema. Hosts
//! (including this harness) decode as `serde_json::Value` and dispatch
//! by the `type` tag. The shape we parse here matches:
//!
//! ```json
//! {
//!   "type": "session_cost",
//!   "session_id": "...",
//!   "total_cost_usd": 0.0123,
//!   "per_turn": [
//!     { "turn": 0, "model": "gpt-4o", "provider": "openai", "cost_usd": 0.008 },
//!     ...
//!   ]
//! }
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Aggregated cost across one scenario run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostReport {
    pub total_usd: f64,
    pub per_turn: Vec<TurnCost>,
}

/// Per-turn cost row. Mirrors `wcore_protocol::events::TurnCost`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCost {
    pub turn: usize,
    pub model: String,
    pub provider: String,
    pub cost_usd: f64,
}

/// Parse a generic decoded JSON event into a [`CostReport`] when it's
/// a `session_cost`. Returns `None` for any other event type.
///
/// The runner reads stdout line-by-line as `serde_json::Value` and
/// hands every event through here; the first matching event wins.
pub fn parse(event: &Value) -> Option<CostReport> {
    if event.get("type").and_then(Value::as_str) != Some("session_cost") {
        return None;
    }
    let total_usd = event.get("total_cost_usd").and_then(Value::as_f64)?;
    // `per_turn` is `Vec<TurnCost>` in the protocol and always serializes as
    // an array, but treat its absence as an empty breakdown rather than
    // dropping the whole (valid) `total_cost_usd` — a `session_cost` event
    // with a present total is authoritative even if the per-turn rows are
    // missing under a future schema change (cross-audit finding #5).
    let per_turn_json = event
        .get("per_turn")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut per_turn = Vec::with_capacity(per_turn_json.len());
    for row in &per_turn_json {
        let tc = TurnCost {
            turn: row.get("turn").and_then(Value::as_u64).unwrap_or(0) as usize,
            model: row
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            provider: row
                .get("provider")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            cost_usd: row.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
        };
        per_turn.push(tc);
    }
    Some(CostReport {
        total_usd,
        per_turn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_session_cost_event() {
        let ev = json!({
            "type": "session_cost",
            "session_id": "sess-1",
            "total_cost_usd": 0.0123,
            "per_turn": [
                { "turn": 0, "model": "gpt-4o", "provider": "openai", "cost_usd": 0.0080 },
                { "turn": 1, "model": "gpt-4o", "provider": "openai", "cost_usd": 0.0043 },
            ],
        });
        let cr = parse(&ev).expect("session_cost should parse");
        assert!((cr.total_usd - 0.0123).abs() < 1e-9);
        assert_eq!(cr.per_turn.len(), 2);
        assert_eq!(cr.per_turn[0].turn, 0);
        assert_eq!(cr.per_turn[1].turn, 1);
        assert_eq!(cr.per_turn[0].provider, "openai");
    }

    #[test]
    fn parse_returns_none_for_non_cost_event() {
        let ev = json!({"type": "info", "msg_id": "m1", "message": "hi"});
        assert!(parse(&ev).is_none());
    }

    #[test]
    fn parse_returns_none_for_non_object() {
        let ev = json!(42);
        assert!(parse(&ev).is_none());
    }
}
