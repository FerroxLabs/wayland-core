//! M5.2 — session trace schema + JSON I/O.
//!
//! `TraceEvent` is the canonical wire form for agent-flow events the
//! runtime emits over the course of a session. Variants are tagged with
//! `#[serde(tag = "type")]` so that an unknown variant in a stored trace
//! produces a decoder error rather than being silently dropped.
//!
//! `ToolCall::input` / `ToolCall::output` are `serde_json::Value` so the
//! schema can host arbitrary tool argument shapes without a generic
//! parameter leaking into the rest of the crate. `PartialEq` on
//! `serde_json::Value` is structural, which is exactly what the
//! [`crate::Differ`] needs to detect divergence.

use serde::{Deserialize, Serialize};

// NOTE: `Eq` is intentionally NOT derived — `ToolCall::input` /
// `output` are `serde_json::Value`, which only implements `PartialEq`
// (JSON allows `f64`). Structural `PartialEq` is sufficient for the
// `Differ`, which never relies on `Eq`-only constructs like `HashSet`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TraceEvent {
    UserMessage {
        ts_ms: u64,
        text: String,
    },
    LlmCall {
        ts_ms: u64,
        provider: String,
        model: String,
        prompt_tokens: u32,
        completion_tokens: u32,
        response: String,
    },
    ToolCall {
        ts_ms: u64,
        tool: String,
        input: serde_json::Value,
        output: serde_json::Value,
        duration_ms: u64,
    },
    AssistantMessage {
        ts_ms: u64,
        text: String,
    },
    SessionEnd {
        ts_ms: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trace {
    pub wcore_version: String,
    pub session_id: String,
    pub events: Vec<TraceEvent>,
}

impl Trace {
    /// Load a trace from a JSON file. Surfaces I/O and decode errors via
    /// the crate's typed [`crate::ReplayError`].
    pub fn load_from_path(path: &std::path::Path) -> crate::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    /// Persist the trace to a JSON file with pretty formatting (so a
    /// human-readable diff against the file is meaningful).
    pub fn save_to_path(&self, path: &std::path::Path) -> crate::Result<()> {
        let body = serde_json::to_string_pretty(self)?;
        wcore_config::atomic_write(path, body.as_bytes())?;
        Ok(())
    }
}
