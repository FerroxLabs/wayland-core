//! Canonical trace schema for the agent loop. Serializes to stable JSON.
//!
//! Hierarchy:
//!
//! ```text
//! ExecutionTrace (session-scoped; W6 aggregates)
//!   └── TurnTrace (one per turn; W1 emits over the protocol stream)
//!         └── ToolCallTrace (one per tool call within the turn)
//! ```
//!
//! Every trace tags itself with `source_product = SOURCE_PRODUCT` (S5) so
//! future memory-fusion code can attribute records to the engine that
//! produced them.
//!
//! # Result-snippet capture
//!
//! `ToolCallTrace::with_result_snippet` captures the first
//! [`RESULT_SNIPPET_MAX`] bytes of a tool call's result. This is gated by
//! the `WAYLAND_TRACE_RESULT_SNIPPETS` env var: capture is ON by default;
//! setting the var to (case-insensitive) `off`, `0`, or `false` disables
//! it, leaving `result_snippet` as `None`. Mirrors the
//! `kg_enabled()` / `staleness_enabled()` opt-out convention.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::SOURCE_PRODUCT;

/// Maximum byte length of a `ToolCallTrace::result_snippet`. Truncation
/// happens at a UTF-8 char boundary, so the actual serialized snippet may
/// be a few bytes shorter than this cap.
pub const RESULT_SNIPPET_MAX: usize = 512;

/// Env var gating result-snippet capture. Set to (case-insensitive)
/// `off`, `0`, or `false` to suppress capture; anything else (including
/// unset) keeps it enabled.
pub const ENV_RESULT_SNIPPETS: &str = "WAYLAND_TRACE_RESULT_SNIPPETS";

/// Returns `true` unless `WAYLAND_TRACE_RESULT_SNIPPETS` is set to a
/// recognized disable token (`off`/`0`/`false`/`no`, case-insensitive).
/// D.2 (v0.6.3): routes through [`crate::env_gate::enabled_unless_disabled`]
/// — the canonical `WAYLAND_*` disable vocabulary — so this gate and the
/// other "mirror" gates accept the same opt-out values.
pub fn result_snippets_enabled() -> bool {
    crate::env_gate::enabled_unless_disabled(ENV_RESULT_SNIPPETS)
}

/// One tool invocation within a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallTrace {
    pub call_id: String,
    pub tool_name: String,
    /// Tool input, scrubbed for credentials/PII at construction via
    /// [`wcore_safety::PIIScrubber`] (see [`ToolCallTrace::new`] / [`scrub_input`]).
    /// Scrubbing happens here — not only at the `SpanSink`-level
    /// `PiiScrubbingSink` (which scrubs serialized JSON) — so EVERY sink,
    /// including non-JSON / stdout emitters that never round-trip through that
    /// wrapper, sees the redacted value. Raw paths, command strings, and
    /// secret-shaped fields never reach a sink verbatim.
    pub input: Value,
    /// Bounded summary (≤4096 chars); full output sits in storage.
    pub output_summary: String,
    pub duration_ms: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// S2 cancellation flag. Stays false in W1; populated in W7.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancelled: bool,
    /// S2 partial-flush flag. Stays false in W1; populated in W7.
    #[serde(default, skip_serializing_if = "is_false")]
    pub partial: bool,
    /// T2-A0: First `RESULT_SNIPPET_MAX` bytes of the tool call's result,
    /// truncated at a UTF-8 char boundary. Wired in T2-A1 HallucinationGuard;
    /// absent from v0.6.1 traces and from snippet-disabled runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_snippet: Option<String>,
    /// `(raw_bytes, compacted_bytes)` when this tool call's output was
    /// compacted by native Bash compaction. `None` when not compacted. Feeds
    /// the `gain`-style savings report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_bytes: Option<(u64, u64)>,
    pub source_product: String,
}

/// Scrub credentials/PII out of a tool-input `Value` BEFORE it is stored on a
/// [`ToolCallTrace`]. Reuses the same [`wcore_safety::PIIScrubber`] that
/// `PiiScrubbingSink` applies at the serialized-JSON level — here it runs
/// structurally at construction so the redacted value is what every sink reads,
/// not just the one JSON-scrubbing wrapper.
///
/// Strategy mirrors `PiiScrubbingSink::emit`: serialize → scrub → re-parse.
/// Credentials can appear inside any string field at arbitrary nesting (e.g. a
/// `Bash` command string or a path), so scrubbing the serialized form catches
/// them all. On the fast path (no match) the scrubber returns `Cow::Borrowed`
/// and the input is returned untouched — no allocation, no re-parse.
fn scrub_input(input: Value) -> Value {
    let raw = match serde_json::to_string(&input) {
        Ok(raw) => raw,
        // A serde_json::Value always serializes; if it somehow doesn't, keep
        // the original rather than dropping the field.
        Err(_) => return input,
    };
    match wcore_safety::PIIScrubber.scrub(&raw) {
        // Nothing matched — return the original value unchanged.
        std::borrow::Cow::Borrowed(_) => input,
        // Something was redacted — re-parse the scrubbed JSON. If re-parsing
        // ever fails (shouldn't, the scrubber preserves JSON structure), fall
        // back to a string Value so the redacted text is still what's stored —
        // never the raw input.
        std::borrow::Cow::Owned(clean) => {
            serde_json::from_str(&clean).unwrap_or(Value::String(clean))
        }
    }
}

impl ToolCallTrace {
    /// Construct a fresh `ToolCallTrace` with `source_product` already set.
    ///
    /// `input` is scrubbed for credentials/PII via [`scrub_input`] before being
    /// stored, so secret-shaped fields (and credentials embedded in paths or
    /// command strings) never reach any sink verbatim.
    pub fn new(call_id: String, tool_name: String, input: Value) -> Self {
        Self {
            call_id,
            tool_name,
            input: scrub_input(input),
            output_summary: String::new(),
            duration_ms: 0,
            bytes_in: 0,
            bytes_out: 0,
            error: None,
            cancelled: false,
            partial: false,
            result_snippet: None,
            compaction_bytes: None,
            source_product: SOURCE_PRODUCT.to_string(),
        }
    }

    /// Record native Bash output-compaction savings on this trace. Stored as
    /// `(raw_bytes, compacted_bytes)` for the savings report.
    pub fn record_compaction(&mut self, raw_bytes: u64, compacted_bytes: u64) {
        self.compaction_bytes = Some((raw_bytes, compacted_bytes));
    }

    /// Mark this tool call as having been cancelled — i.e. the result was
    /// produced because a cancellation path won (host cancel token / abort),
    /// not because the tool ran to completion. The authoritative cancel signal
    /// lives in `wcore-agent`; this additive setter lets the emission site
    /// thread that state through to the trace before it is emitted. Lets hosts
    /// distinguish a cancelled tool call from a normal one (the default).
    pub fn with_cancelled(mut self, cancelled: bool) -> Self {
        self.cancelled = cancelled;
        self
    }

    /// Mark this tool call's output as partial — i.e. captured from a forced or
    /// early streaming drain rather than a full read. Like [`Self::with_cancelled`],
    /// the authoritative signal originates in `wcore-agent`; this additive
    /// setter carries it onto the trace at the emission site.
    pub fn with_partial(mut self, partial: bool) -> Self {
        self.partial = partial;
        self
    }

    /// Attach a `result_snippet`, truncating to `RESULT_SNIPPET_MAX` bytes
    /// at a UTF-8 char boundary. Never splits a multi-byte char.
    ///
    /// Capture is gated by [`result_snippets_enabled`]: when
    /// `WAYLAND_TRACE_RESULT_SNIPPETS` is set to `off`/`0`/`false`, this is
    /// a no-op and `result_snippet` stays `None`.
    pub fn with_result_snippet(mut self, raw: &str) -> Self {
        if !result_snippets_enabled() {
            return self;
        }
        if raw.len() <= RESULT_SNIPPET_MAX {
            self.result_snippet = Some(raw.to_string());
        } else {
            // Walk char boundaries and stop at the last one that fits.
            let mut cut = 0;
            for (idx, ch) in raw.char_indices() {
                if idx + ch.len_utf8() > RESULT_SNIPPET_MAX {
                    break;
                }
                cut = idx + ch.len_utf8();
            }
            self.result_snippet = Some(raw[..cut].to_string());
        }
        self
    }
}

/// Aggregate native Bash output-compaction savings across a set of tool-call
/// traces — the data behind the `gain`-style savings report. Built from the
/// `compaction_bytes` each `ToolCallTrace` carries, which already flows to the
/// host in every emitted `TurnTrace`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionSavings {
    /// Number of tool calls whose output was compacted.
    pub calls: u64,
    /// Total raw bytes before compaction (summed over those calls).
    pub raw_bytes: u64,
    /// Total bytes after compaction (summed over those calls).
    pub compacted_bytes: u64,
}

impl CompactionSavings {
    /// Bytes saved (`raw - compacted`, floored at 0).
    pub fn saved_bytes(&self) -> u64 {
        self.raw_bytes.saturating_sub(self.compacted_bytes)
    }

    /// Savings as a fraction of raw bytes in `[0.0, 1.0]`; 0.0 when nothing
    /// was compacted.
    pub fn ratio(&self) -> f64 {
        if self.raw_bytes == 0 {
            0.0
        } else {
            self.saved_bytes() as f64 / self.raw_bytes as f64
        }
    }

    /// Fold another tally into this one.
    pub fn add(&mut self, other: &CompactionSavings) {
        self.calls += other.calls;
        self.raw_bytes += other.raw_bytes;
        self.compacted_bytes += other.compacted_bytes;
    }
}

/// Sum the Bash output-compaction savings recorded across `traces`.
pub fn aggregate_compaction(traces: &[ToolCallTrace]) -> CompactionSavings {
    let mut out = CompactionSavings::default();
    for t in traces {
        if let Some((raw, compacted)) = t.compaction_bytes {
            out.calls += 1;
            out.raw_bytes += raw;
            out.compacted_bytes += compacted;
        }
    }
    out
}

/// One turn of the agent loop: the LLM call + every tool call it triggered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnTrace {
    pub turn: usize,
    pub model: String,
    pub provider: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// `cache_read / input_tokens` for this turn. 0.0 when input_tokens == 0.
    pub cache_hit_rate: f64,
    /// USD cost. Populated by W6; W1 leaves it 0.0.
    pub cost_usd: f64,
    pub tool_calls: Vec<ToolCallTrace>,
    /// HookActionRecords are populated by W2 hook engine extension. W1 emits
    /// an empty vector.
    pub hook_actions: Vec<HookActionRecord>,
    pub source_product: String,
    /// #279(b)+(c): stable per-run id mirroring StreamEnd.agent_run_id.
    /// #[serde(default)] so trace JSON written before this field existed
    /// still deserializes; skip_serializing_if keeps the shape unchanged
    /// when unset.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_run_id: String,
}

impl TurnTrace {
    /// Compute `cache_hit_rate` from input / cache_read with the zero-input
    /// guard. Use this instead of inline arithmetic at call sites.
    pub fn cache_hit_rate_from(input_tokens: u64, cache_read: u64) -> f64 {
        if input_tokens == 0 {
            0.0
        } else {
            cache_read as f64 / input_tokens as f64
        }
    }
}

/// Outcome of an entire `ExecutionTrace`. W1 emits at the turn level so this
/// type isn't directly populated until W6, but reserving the variants here
/// keeps the schema closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    PartialSuccess {
        issues: Vec<String>,
    },
    Failure {
        reason: String,
    },
    Timeout,
    UserAborted,
    /// S4 HITL suspend (W7).
    Suspended {
        reason: String,
    },
}

/// Aggregate trace covering one whole session/task. W6 populates this from
/// the `TurnTrace`s W1 emits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    pub session_id: String,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_description: Option<String>,
    pub turns: Vec<TurnTrace>,
    pub outcome: TaskOutcome,
    pub total_cost_usd: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub duration_ms: u64,
    pub source_product: String,
}

/// One non-`Continue` hook action fired during a turn. The agent-level hook
/// engine records these (InjectMessage / SwitchModel / Block / ModifyInput at
/// a phase where it is honoured) so a `TurnTrace` carries the hook activity it
/// triggered rather than an empty vector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookActionRecord {
    /// Action variant that fired (e.g. `"InjectMessage"`, `"SwitchModel"`).
    pub kind: String,
    /// Registered name of the hook that produced the action. `#[serde(default)]`
    /// so traces written before this field existed still deserialize (empty).
    #[serde(default)]
    pub hook_name: String,
    pub timestamp_ms: u64,
}

/// W10B F12 GEPA evolution event.
///
/// Emitted once per scored child by the `wcore-evolve` loop. Hosts that have
/// flipped the W0 `gepa_enabled` capability flag accept these alongside
/// regular events; v0.1.21 hosts drop them silently per the host decoder
/// contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionEventTrace {
    pub run_id: String,
    pub generation: u32,
    pub parent_id: String,
    /// Stable `{run_id}/{generation}/{child_index}` identifier.
    pub child_id: String,
    /// Serialized `MutationKind` (e.g. "Reorder", "Paraphrase").
    pub mutation_kind: String,
    /// Composite score from `wcore_eval::ScoreDimensions::combined`.
    pub score: f64,
    /// True iff this child is the new top — populated by the loop, not the
    /// scorer.
    pub retained: bool,
    pub source_product: String,
}

impl EvolutionEventTrace {
    /// Construct a fresh `EvolutionEventTrace` with `source_product` already
    /// set to `SOURCE_PRODUCT`.
    pub fn new(
        run_id: String,
        generation: u32,
        parent_id: String,
        child_id: String,
        mutation_kind: String,
        score: f64,
        retained: bool,
    ) -> Self {
        Self {
            run_id,
            generation,
            parent_id,
            child_id,
            mutation_kind,
            score,
            retained,
            source_product: SOURCE_PRODUCT.to_string(),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// M3.3 — one memory-API operation. Emitted by `PartitionDispatcher`
/// around every gated `MemoryApi` call. `success` is `false` when the
/// underlying op returned `Err`.
///
/// The schema is flat (no nested objects) so it round-trips through the
/// existing `SpanSink::emit` JSON channel without a separate transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryOpTrace {
    /// MemoryApi method name (e.g. `"record_episode"`, `"search"`).
    pub op: String,
    /// Partition string per `wcore_memory::v2_types::Partition::as_str()`.
    pub partition: String,
    /// Tier string per `wcore_memory::v2_types::Tier::as_str()`.
    /// Cross-tier ops (`search`, `dream_now`, `compact`) use `"-"`.
    pub tier: String,
    pub latency_ms: u64,
    pub success: bool,
    pub source_product: String,
}

impl MemoryOpTrace {
    /// Construct a fresh `MemoryOpTrace` with `source_product` already set
    /// to `SOURCE_PRODUCT`.
    pub fn new(
        op: String,
        partition: String,
        tier: String,
        latency_ms: u64,
        success: bool,
    ) -> Self {
        Self {
            op,
            partition,
            tier,
            latency_ms,
            success,
            source_product: SOURCE_PRODUCT.to_string(),
        }
    }
}

/// Dynamic Workflows B4 — shadow-mode workflow-detection record.
///
/// Emitted (telemetry-only) when `observability.workflow_detection_enabled`
/// is on AND the per-turn `WorkflowCandidate` heuristic fires at the engine's
/// intent-telemetry seam. It records what the Detected tier *would have*
/// proposed — WITHOUT prompting the user and WITHOUT touching routing. This is
/// the cross-AI "shadow-mode precision phase": real-traffic precision can be
/// measured before the confirm card (B6) ever shows.
///
/// The schema is flat (no nested objects) so it round-trips through the same
/// `SpanSink` / `OutputSink::emit_trace` JSON channel `TurnTrace` and
/// `MemoryOpTrace` already use — no new sink. An operator reviews accumulated
/// records by filtering the structured trace log for `kind ==
/// "workflow_detection"` (see [`WorkflowDetectionRecord::KIND`]) and running
/// them through [`summarize_workflow_detection`].
///
/// # Privacy
///
/// `task_excerpt` is a short prefix of the user's task (capped at
/// [`TASK_EXCERPT_MAX`] bytes, truncated at a UTF-8 char boundary) — never the
/// full prompt. The full prompt is not logged here, so a huge or
/// secret-bearing prompt cannot bloat or leak through the shadow log.
///
/// FIX E — the excerpt is **scrubbed for credentials/PII via
/// [`wcore_safety::PIIScrubber`] BEFORE truncation** (see
/// [`WorkflowDetectionRecord::new`]). Scrubbing the full task first means a
/// secret straddling the [`TASK_EXCERPT_MAX`] boundary is redacted before the
/// cut, so truncation can never split a secret and smuggle a fragment past the
/// scrubber. This record is emitted on the engine's `emit_trace` path, which
/// does NOT run through the `SpanSink`-level `PiiScrubbingSink` wrapper — so the
/// scrubbing must (and does) happen here at construction. `rationale` is
/// token-only (matched keyword constants, never a raw prompt slice), so it
/// carries no user content to scrub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDetectionRecord {
    /// Discriminator so operators / log filters can pick these out of the
    /// shared trace channel. Always [`WorkflowDetectionRecord::KIND`].
    pub kind: String,
    /// RFC3339 / ISO-8601 UTC timestamp of when the candidate fired.
    pub ts: String,
    /// Short prefix of the task that triggered the candidate, capped at
    /// [`TASK_EXCERPT_MAX`] bytes. NOT the full prompt.
    pub task_excerpt: String,
    /// Heuristic confidence in `[0.0, 1.0]` carried from `WorkflowCandidate`.
    pub confidence: f32,
    /// Human-readable explanation of which workflow signals fired.
    pub rationale: String,
    pub source_product: String,
}

/// Maximum byte length of [`WorkflowDetectionRecord::task_excerpt`].
/// Truncation happens at a UTF-8 char boundary, so the stored excerpt may be a
/// few bytes shorter. Deliberately small — the excerpt is a debugging aid for
/// reviewing shadow detections, not a record of the prompt.
pub const TASK_EXCERPT_MAX: usize = 120;

impl WorkflowDetectionRecord {
    /// Trace `kind` discriminator for shadow workflow-detection records.
    pub const KIND: &'static str = "workflow_detection";

    /// Build a record, SCRUBBING `task` for credentials/PII (FIX E) and THEN
    /// truncating to [`TASK_EXCERPT_MAX`] bytes at a UTF-8 char boundary, and
    /// stamping `ts` with the current UTC time. Scrubbing happens before the cut
    /// so a secret crossing the boundary is redacted before truncation can split
    /// it (the `emit_trace` path this record flows through does not apply the
    /// `SpanSink`-level `PiiScrubbingSink`).
    pub fn new(task: &str, confidence: f32, rationale: String) -> Self {
        let scrubbed = wcore_safety::PIIScrubber.scrub(task);
        Self {
            kind: Self::KIND.to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            task_excerpt: truncate_on_char_boundary(&scrubbed, TASK_EXCERPT_MAX),
            confidence,
            rationale,
            source_product: SOURCE_PRODUCT.to_string(),
        }
    }
}

/// Truncate `s` to at most `max` bytes without splitting a multi-byte char.
fn truncate_on_char_boundary(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = 0;
    for (idx, ch) in s.char_indices() {
        if idx + ch.len_utf8() > max {
            break;
        }
        cut = idx + ch.len_utf8();
    }
    s[..cut].to_string()
}

/// Operator helper: summarize a batch of serialized trace `Value`s (e.g. the
/// JSON lines an operator collected from the shadow log) into shadow
/// workflow-detection counts. Non-`workflow_detection` records are ignored, so
/// a mixed trace stream can be passed straight in.
///
/// Returns the number of shadow detections and the mean confidence across
/// them (0.0 when there are none). This is the simplest "would have fired"
/// precision-review primitive — pair it with the records' `rationale` field to
/// see *which* signals drove the detections.
pub fn summarize_workflow_detection(records: &[Value]) -> WorkflowDetectionSummary {
    let mut count = 0usize;
    let mut confidence_sum = 0.0f64;
    for r in records {
        if r.get("kind").and_then(|k| k.as_str()) == Some(WorkflowDetectionRecord::KIND) {
            count += 1;
            confidence_sum += r.get("confidence").and_then(|c| c.as_f64()).unwrap_or(0.0);
        }
    }
    let mean_confidence = if count == 0 {
        0.0
    } else {
        confidence_sum / count as f64
    };
    WorkflowDetectionSummary {
        count,
        mean_confidence,
    }
}

/// Aggregate produced by [`summarize_workflow_detection`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkflowDetectionSummary {
    /// How many shadow workflow-detection records were seen.
    pub count: usize,
    /// Mean `confidence` across those records (0.0 when `count == 0`).
    pub mean_confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;

    #[test]
    fn tool_call_trace_new_sets_source_product() {
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}));
        assert_eq!(t.source_product, SOURCE_PRODUCT);
    }

    #[test]
    fn tool_call_trace_new_redacts_secret_in_input() {
        // RANK 59 — a credential carried in a tool input must be redacted in
        // the STORED `input` (structurally, at construction), so every sink —
        // not just the JSON-scrubbing `PiiScrubbingSink` — sees the redacted
        // value. An AWS access key (`AKIA` + 16 chars) is one of the scrubber's
        // patterns.
        let secret = "AKIAIOSFODNN7EXAMPLE"; // AKIA + 16 = a matching key
        let t = ToolCallTrace::new(
            "c1".into(),
            "Bash".into(),
            json!({ "api_key": secret, "command": format!("deploy --token {secret}") }),
        );

        // The stored value must not contain the raw secret anywhere.
        let stored = serde_json::to_string(&t.input).unwrap();
        assert!(
            !stored.contains(secret),
            "raw secret must not survive in stored input; got {stored}"
        );
        assert!(
            !stored.contains("AKIA"),
            "no raw key fragment may survive; got {stored}"
        );
        // And the redaction marker must be present in its place.
        assert!(
            stored.contains("[REDACTED:AWS_ACCESS_KEY]"),
            "secret must be redacted in stored input; got {stored}"
        );

        // Structure is preserved: the keys still round-trip as an object.
        assert!(t.input.get("api_key").is_some());
        assert!(t.input.get("command").is_some());
    }

    #[test]
    fn tool_call_trace_new_preserves_clean_input_unchanged() {
        // No credentials → the input must round-trip byte-for-byte (fast path,
        // no spurious re-serialization artifacts).
        let raw = json!({ "path": "/etc/hosts", "limit": 100, "nested": [1, 2, 3] });
        let t = ToolCallTrace::new("c1".into(), "Read".into(), raw.clone());
        assert_eq!(t.input, raw);
    }

    #[test]
    fn aggregate_compaction_sums_savings_and_ratio() {
        let mut a = ToolCallTrace::new("a".into(), "Bash".into(), json!({}));
        a.record_compaction(1000, 200);
        let mut b = ToolCallTrace::new("b".into(), "Bash".into(), json!({}));
        b.record_compaction(500, 250);
        // A non-compacted call must not contribute.
        let c = ToolCallTrace::new("c".into(), "Read".into(), json!({}));

        let s = aggregate_compaction(&[a, b, c]);
        assert_eq!(s.calls, 2);
        assert_eq!(s.raw_bytes, 1500);
        assert_eq!(s.compacted_bytes, 450);
        assert_eq!(s.saved_bytes(), 1050);
        assert!((s.ratio() - 0.7).abs() < 1e-9);

        // Empty input yields a zero tally with a safe 0.0 ratio.
        let empty = aggregate_compaction(&[]);
        assert_eq!(empty, CompactionSavings::default());
        assert_eq!(empty.ratio(), 0.0);
    }

    #[test]
    fn tool_call_trace_round_trips_compaction_bytes() {
        let mut t = ToolCallTrace::new("c1".into(), "Bash".into(), json!({}));
        // Absent by default, and elided from the wire when None.
        assert_eq!(t.compaction_bytes, None);
        let none_json = serde_json::to_string(&t).unwrap();
        assert!(!none_json.contains("compaction_bytes"));

        t.record_compaction(1000, 200);
        assert_eq!(t.compaction_bytes, Some((1000, 200)));
        let s = serde_json::to_string(&t).unwrap();
        let back: ToolCallTrace = serde_json::from_str(&s).unwrap();
        assert_eq!(back.compaction_bytes, Some((1000, 200)));
    }

    #[test]
    fn cache_hit_rate_zero_when_input_tokens_zero() {
        assert_eq!(TurnTrace::cache_hit_rate_from(0, 100), 0.0);
    }

    #[test]
    fn cache_hit_rate_is_ratio_otherwise() {
        let r = TurnTrace::cache_hit_rate_from(1000, 800);
        assert!((r - 0.8).abs() < 1e-9);
    }

    #[test]
    fn tool_call_trace_default_flags_omitted_in_serde() {
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}));
        let v = serde_json::to_value(&t).unwrap();
        assert!(
            v.get("cancelled").is_none(),
            "default-false cancelled must be omitted"
        );
        assert!(
            v.get("partial").is_none(),
            "default-false partial must be omitted"
        );
        assert!(v.get("error").is_none(), "None error must be omitted");
    }

    #[test]
    fn tool_call_trace_cancelled_true_appears_in_serde() {
        let mut t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}));
        t.cancelled = true;
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["cancelled"], true);
    }

    #[test]
    fn cancelled_partial_builders_round_trip_through_serde() {
        // The builders carry the real cancel/partial state (sourced from
        // wcore-agent) onto the trace, and a cancelled+partial trace must
        // survive a serde round-trip with both flags set — proving hosts can
        // distinguish a cancelled/partial call from a normal one.
        let t = ToolCallTrace::new("c1".into(), "Bash".into(), json!({}))
            .with_cancelled(true)
            .with_partial(true);
        assert!(t.cancelled);
        assert!(t.partial);

        let wire = serde_json::to_string(&t).unwrap();
        assert!(wire.contains("\"cancelled\":true"));
        assert!(wire.contains("\"partial\":true"));
        let back: ToolCallTrace = serde_json::from_str(&wire).unwrap();
        assert!(back.cancelled, "cancelled flag must round-trip");
        assert!(back.partial, "partial flag must round-trip");

        // Passing `false` keeps the default and stays elided from the wire, so
        // existing trace consumers see no schema drift.
        let normal = ToolCallTrace::new("c2".into(), "Read".into(), json!({}))
            .with_cancelled(false)
            .with_partial(false);
        let v = serde_json::to_value(&normal).unwrap();
        assert!(
            v.get("cancelled").is_none(),
            "default-false must be omitted"
        );
        assert!(v.get("partial").is_none(), "default-false must be omitted");
    }

    // ---- T2-A0: result_snippet ----

    #[test]
    fn result_snippet_none_by_default() {
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}));
        assert!(t.result_snippet.is_none());
    }

    #[test]
    fn with_result_snippet_short_value_preserved() {
        let raw: String = "x".repeat(100);
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({})).with_result_snippet(&raw);
        assert_eq!(t.result_snippet.as_deref(), Some(raw.as_str()));
        assert_eq!(t.result_snippet.as_ref().unwrap().len(), 100);
    }

    #[test]
    fn with_result_snippet_truncates_at_512_bytes() {
        let raw: String = "a".repeat(1000);
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({})).with_result_snippet(&raw);
        let snip = t.result_snippet.expect("snippet present");
        assert_eq!(snip.len(), RESULT_SNIPPET_MAX);
        assert!(snip.chars().all(|c| c == 'a'));
    }

    #[test]
    fn with_result_snippet_truncates_at_utf8_boundary() {
        // "日本語" is 9 bytes (3 chars × 3 bytes). 200 repeats == 1800 bytes,
        // and 512 / 3 = 170 r 2 — so the naive byte-512 cut would land
        // mid-char. We must back off to a char boundary.
        let raw: String = "日本語".repeat(200);
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({})).with_result_snippet(&raw);
        let snip = t.result_snippet.expect("snippet present");
        assert!(snip.len() <= RESULT_SNIPPET_MAX, "len {} > cap", snip.len());
        // Already-typed `String` proves valid UTF-8; double-check by
        // counting whole chars and confirming we used the byte budget.
        let chars: Vec<char> = snip.chars().collect();
        assert!(!chars.is_empty());
        assert!(chars.iter().all(|c| matches!(c, '日' | '本' | '語')));
        // 512 / 3 == 170 full triplets => 510 bytes; the snippet should be
        // the largest multiple of 3 ≤ 512.
        assert_eq!(snip.len(), 510);
    }

    #[test]
    fn serde_roundtrip_old_v061_trace_deserializes() {
        // A v0.6.1-shaped JSON blob with NO result_snippet key.
        let old = json!({
            "call_id": "c1",
            "tool_name": "Read",
            "input": {},
            "output_summary": "",
            "duration_ms": 0,
            "bytes_in": 0,
            "bytes_out": 0,
            "source_product": SOURCE_PRODUCT,
        });
        let parsed: ToolCallTrace = serde_json::from_value(old).expect("v0.6.1 trace must parse");
        assert!(parsed.result_snippet.is_none());
    }

    #[test]
    fn serde_serialization_skips_none() {
        let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}));
        let v = serde_json::to_value(&t).unwrap();
        assert!(
            v.get("result_snippet").is_none(),
            "None result_snippet must be omitted, found {:?}",
            v.get("result_snippet")
        );
    }

    // ---- W9: WAYLAND_TRACE_RESULT_SNIPPETS env gate ----

    /// Restore the env var to its prior state. SAFETY: only called inside a
    /// `#[serial(env)]` test, so no other thread reads/writes env concurrently.
    fn restore_snippet_env(saved: Option<String>) {
        unsafe {
            match saved {
                Some(v) => std::env::set_var(ENV_RESULT_SNIPPETS, v),
                None => std::env::remove_var(ENV_RESULT_SNIPPETS),
            }
        }
    }

    #[test]
    #[serial(env)]
    fn result_snippets_enabled_true_when_unset() {
        let prior = std::env::var(ENV_RESULT_SNIPPETS).ok();
        // SAFETY: serialized via #[serial(env)].
        unsafe { std::env::remove_var(ENV_RESULT_SNIPPETS) };
        assert!(result_snippets_enabled());
        restore_snippet_env(prior);
    }

    #[test]
    #[serial(env)]
    fn snippet_captured_when_env_unset() {
        let prior = std::env::var(ENV_RESULT_SNIPPETS).ok();
        // SAFETY: serialized via #[serial(env)].
        unsafe { std::env::remove_var(ENV_RESULT_SNIPPETS) };
        let t =
            ToolCallTrace::new("c1".into(), "Read".into(), json!({})).with_result_snippet("hello");
        assert_eq!(t.result_snippet.as_deref(), Some("hello"));
        restore_snippet_env(prior);
    }

    #[test]
    #[serial(env)]
    fn snippet_suppressed_when_env_off() {
        let prior = std::env::var(ENV_RESULT_SNIPPETS).ok();
        for val in ["off", "OFF", "0", "false", "False"] {
            // SAFETY: serialized via #[serial(env)].
            unsafe { std::env::set_var(ENV_RESULT_SNIPPETS, val) };
            assert!(
                !result_snippets_enabled(),
                "{val} must disable snippet capture"
            );
            let t = ToolCallTrace::new("c1".into(), "Read".into(), json!({}))
                .with_result_snippet("hello");
            assert!(
                t.result_snippet.is_none(),
                "{val} must leave result_snippet None"
            );
        }
        restore_snippet_env(prior);
    }

    // ---- B4: WorkflowDetectionRecord (shadow-mode) ----

    #[test]
    fn workflow_detection_record_sets_kind_and_source() {
        let r = WorkflowDetectionRecord::new("audit every file", 0.5, "signals: every file".into());
        assert_eq!(r.kind, WorkflowDetectionRecord::KIND);
        assert_eq!(r.kind, "workflow_detection");
        assert_eq!(r.source_product, SOURCE_PRODUCT);
        assert_eq!(r.task_excerpt, "audit every file");
        assert!(!r.ts.is_empty(), "ts must be stamped");
    }

    #[test]
    fn workflow_detection_record_truncates_excerpt_to_bound() {
        let long = "a".repeat(500);
        let r = WorkflowDetectionRecord::new(&long, 0.9, "r".into());
        assert_eq!(
            r.task_excerpt.len(),
            TASK_EXCERPT_MAX,
            "ascii excerpt must hit the byte cap exactly"
        );
        assert!(r.task_excerpt.chars().all(|c| c == 'a'));
    }

    #[test]
    fn workflow_detection_record_truncation_respects_utf8_boundary() {
        // "日本語" is 9 bytes (3 chars × 3 bytes). The byte cap (120) is not a
        // multiple of 3, so a naive cut would split a char. 120 / 3 == 40
        // triplets => 120 bytes exactly here, but the boundary walk must never
        // exceed the cap and must stay valid UTF-8.
        let raw = "日本語".repeat(100);
        let r = WorkflowDetectionRecord::new(&raw, 0.1, "r".into());
        assert!(r.task_excerpt.len() <= TASK_EXCERPT_MAX);
        // Already a valid `String` — confirm only whole chars survived.
        assert!(
            r.task_excerpt
                .chars()
                .all(|c| matches!(c, '日' | '本' | '語'))
        );
    }

    #[test]
    fn workflow_detection_record_scrubs_secret_in_excerpt() {
        // FIX E — a credential anywhere in the task must be redacted in the
        // excerpt. An AWS access key (`AKIA` + 16 chars) sits early enough to
        // survive the 120-byte excerpt window.
        let secret = "AKIAIOSFODNN7EXAMPLE"; // AKIA + 16 = a matching key
        let task = format!("audit every file using key {secret} and report");
        let r = WorkflowDetectionRecord::new(&task, 0.7, "workflow signals: every file".into());
        assert!(
            r.task_excerpt.contains("[REDACTED:AWS_ACCESS_KEY]"),
            "secret must be redacted in the excerpt; got {:?}",
            r.task_excerpt
        );
        assert!(
            !r.task_excerpt.contains(secret),
            "raw secret must not survive in the excerpt; got {:?}",
            r.task_excerpt
        );
    }

    #[test]
    fn workflow_detection_record_scrubs_before_truncation_boundary_split() {
        // FIX E — a secret straddling the TASK_EXCERPT_MAX byte boundary must be
        // redacted BEFORE truncation, so no raw fragment is smuggled past. Pad so
        // the AKIA key begins just before the 120-byte cut: a truncate-then-scrub
        // ordering would leave a raw prefix of the key in the excerpt.
        let secret = "AKIAIOSFODNN7EXAMPLE";
        // 110 bytes of padding puts the key start at byte 110, so a naive cut at
        // 120 would keep ~10 raw chars of the key.
        let pad = "x".repeat(110);
        let task = format!("{pad}{secret} trailing");
        let r = WorkflowDetectionRecord::new(&task, 0.5, "r".into());
        // No contiguous raw run of the key (even a prefix) may appear.
        assert!(
            !r.task_excerpt.contains("AKIA"),
            "no raw key fragment may survive truncation; got {:?}",
            r.task_excerpt
        );
    }

    #[test]
    fn workflow_detection_record_serializes_flat_with_discriminator() {
        let r = WorkflowDetectionRecord::new("scan all files", 0.5, "signals: all files".into());
        let v = serde_json::to_value(&r).expect("serialize");
        assert_eq!(v["kind"], "workflow_detection");
        assert_eq!(v["task_excerpt"], "scan all files");
        assert!(v["confidence"].is_number());
        assert!(v["rationale"].is_string());
        assert!(v["ts"].is_string());
    }

    #[test]
    fn summarize_workflow_detection_counts_only_shadow_records() {
        let records = vec![
            serde_json::to_value(WorkflowDetectionRecord::new("a", 0.4, "r".into())).unwrap(),
            serde_json::to_value(WorkflowDetectionRecord::new("b", 0.8, "r".into())).unwrap(),
            // A foreign trace (e.g. a TurnTrace) must be ignored.
            json!({ "turn": 0, "kind": "turn" }),
            json!({ "no_kind": true }),
        ];
        let s = summarize_workflow_detection(&records);
        assert_eq!(s.count, 2, "only workflow_detection records count");
        assert!(
            (s.mean_confidence - 0.6).abs() < 1e-6,
            "mean confidence = {}",
            s.mean_confidence
        );
    }

    #[test]
    fn summarize_workflow_detection_empty_is_zero() {
        let s = summarize_workflow_detection(&[]);
        assert_eq!(s.count, 0);
        assert_eq!(s.mean_confidence, 0.0);
    }

    #[test]
    fn turn_trace_without_agent_run_id_deserializes_to_empty_279() {
        let old = serde_json::json!({
            "turn": 0, "model": "claude-opus-4", "provider": "anthropic",
            "input_tokens": 100, "output_tokens": 50, "cache_read": 0, "cache_write": 0,
            "cache_hit_rate": 0.0, "cost_usd": 0.0, "tool_calls": [], "hook_actions": [],
            "source_product": "wcore"
        });
        let parsed: TurnTrace =
            serde_json::from_value(old).expect("old trace must still deserialize");
        assert_eq!(
            parsed.agent_run_id, "",
            "missing agent_run_id defaults to empty"
        );
        let mut t = parsed.clone();
        t.agent_run_id = "agent-run-abc".into();
        assert!(serde_json::to_string(&t).unwrap().contains("agent-run-abc"));
        assert!(
            !serde_json::to_string(&parsed)
                .unwrap()
                .contains("agent_run_id")
        );
    }
}
