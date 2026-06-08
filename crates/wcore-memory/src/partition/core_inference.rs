//! PUM (Persistent User Model) — P5 user-model inference.
//!
//! Design doc §4.16 refers to this deliverable as "M8 P5 user model";
//! W9 uses **PUM** to avoid collision with W5's M8 (candle embeddings).
//! Consumes `TurnTrace` history (W1) and derives user-model k/v
//! deltas. Writes always go through `MemoryApi::update_user_model`
//! with `AccessToken::System` (P5 is system-only-write per W5 L4).
//!
//! Keys produced by W9 (additive — new keys can be added without
//! schema changes):
//!
//! - `preferences.tool_order` — top-5 most-used tools by raw frequency
//!   (ties broken by first-seen turn index for determinism)
//! - `working_hours.local_tz_window` — `{ start: "HH:MM", end: "HH:MM" }`
//!   derived from observed turn timestamps. W9 ships a 24h stub since
//!   TurnTrace lacks wall-clock timestamps; W6 adds them and this
//!   tightens automatically.
//! - `language.primary` — `"en" | "ja" | "zh" | ...` from user-message
//!   sampling. W9 emits `"en"` as a best-effort stub.
//! - `tool_habits.recent_top5` — tools by recency-weighted frequency
//!   (last turn × 3, prev × 2, others × 1) so recent shifts surface.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::api::MemoryApi;
use crate::error::Result;
use crate::v2_types::AccessToken;
use wcore_observability::trace::TurnTrace;

pub struct UserModelInferencer {
    mem: Arc<dyn MemoryApi>,
}

impl UserModelInferencer {
    pub fn new(mem: Arc<dyn MemoryApi>) -> Self {
        Self { mem }
    }

    /// Pure inference. Returns the deltas without writing them.
    /// Returns `(key, value)` pairs in stable order.
    pub fn infer(&self, traces: &[TurnTrace]) -> Vec<(String, Value)> {
        let mut out: Vec<(String, Value)> = Vec::new();
        if traces.is_empty() {
            return out;
        }

        // preferences.tool_order — top 5 by raw frequency, ties broken
        // by first-seen turn index (deterministic).
        let mut freq: HashMap<&str, usize> = HashMap::new();
        let mut first_seen: HashMap<&str, usize> = HashMap::new();
        for (turn_idx, t) in traces.iter().enumerate() {
            for c in &t.tool_calls {
                *freq.entry(c.tool_name.as_str()).or_default() += 1;
                first_seen.entry(c.tool_name.as_str()).or_insert(turn_idx);
            }
        }
        let mut by_freq: Vec<(&&str, &usize)> = freq.iter().collect();
        by_freq.sort_by(|a, b| {
            b.1.cmp(a.1)
                .then_with(|| first_seen[a.0].cmp(&first_seen[b.0]))
        });
        let top5: Vec<Value> = by_freq
            .iter()
            .take(5)
            .map(|(n, _)| Value::String((*n).to_string()))
            .collect();
        out.push(("preferences.tool_order".into(), Value::Array(top5)));

        // tool_habits.recent_top5 — recency-weighted (last turn × 3,
        // previous × 2, all others × 1).
        let mut weighted: HashMap<&str, f64> = HashMap::new();
        let last = traces.len().saturating_sub(1);
        let prev = traces.len().saturating_sub(2);
        for (turn_idx, t) in traces.iter().enumerate() {
            let w = if turn_idx == last {
                3.0
            } else if turn_idx == prev {
                2.0
            } else {
                1.0
            };
            for c in &t.tool_calls {
                *weighted.entry(c.tool_name.as_str()).or_default() += w;
            }
        }
        let mut by_weight: Vec<(&&str, &f64)> = weighted.iter().collect();
        by_weight.sort_by(|a, b| {
            b.1.partial_cmp(a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| first_seen[a.0].cmp(&first_seen[b.0]))
        });
        let recent5: Vec<Value> = by_weight
            .iter()
            .take(5)
            .map(|(n, _)| Value::String((*n).to_string()))
            .collect();
        out.push(("tool_habits.recent_top5".into(), Value::Array(recent5)));

        // language.primary — best-effort stub. W9 emits "en" unless a
        // future signal source overrides.
        out.push(("language.primary".into(), Value::String("en".into())));

        // working_hours.local_tz_window — W9 stub (24h window). W6 adds
        // wall-clock timestamps to TurnTrace, then this tightens.
        out.push((
            "working_hours.local_tz_window".into(),
            serde_json::json!({ "start": "00:00", "end": "23:59" }),
        ));

        out
    }

    /// Convenience for callers that want one round-trip: infer then write.
    /// Each `update_user_model` call is gated by `MemoryAccessGate` and
    /// requires `AccessToken::System`. Errors short-circuit; partial state
    /// remains for the next call (deltas are idempotent on key).
    pub async fn infer_and_persist(&self, traces: &[TurnTrace]) -> Result<usize> {
        let deltas = self.infer(traces);
        for (k, v) in &deltas {
            self.mem
                .update_user_model(k, v.clone(), AccessToken::System)
                .await?;
        }
        Ok(deltas.len())
    }
}
