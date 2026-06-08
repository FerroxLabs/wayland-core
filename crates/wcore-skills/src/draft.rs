//! F10: autonomous skill creation.
//!
//! Reads `TurnTrace` history (W1), detects repeated tool-call patterns,
//! and writes a staged `Procedure` into P4 (W5). Drafts NEVER promote
//! themselves to `Active` — that transition is human-curator-driven in
//! W9 and eval-driven in W10B (F12 GEPA).
//!
//! See design contract §5.3 (F10 acceptance) and the W9 plan for the
//! end-to-end flow.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use uuid::Uuid;
use wcore_memory::api::MemoryApi;
use wcore_memory::error::Result as MemResult;
use wcore_memory::v2_types::{AccessToken, Procedure, ProcedureId, ProcedureStatus, Tier};
use wcore_observability::trace::TurnTrace;

pub const MODULE_NAME: &str = "wcore-skills::draft";

/// Default minimum repetitions a pattern must reach to qualify as a draft.
pub const DEFAULT_MIN_REPEATS: usize = 3;

/// Default minimum tool-sequence length for a pattern to qualify (design
/// §5.3 F10 acceptance: "after a 7-tool-call task succeeds, a staged P4
/// skill exists". W9 sets the floor at 5 to qualify shorter pipelines
/// while still excluding trivial 1-2 tool turns).
pub const DEFAULT_MIN_SEQ_LEN: usize = 5;

#[derive(Debug, Clone)]
pub struct PatternDetector {
    pub min_repeats: usize,
    pub min_seq_len: usize,
}

impl Default for PatternDetector {
    fn default() -> Self {
        Self {
            min_repeats: DEFAULT_MIN_REPEATS,
            min_seq_len: DEFAULT_MIN_SEQ_LEN,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DraftCandidate {
    /// Tool names in invocation order.
    pub tool_sequence: Vec<String>,
    /// Stable JSON key-shape per tool call: same length as `tool_sequence`,
    /// each entry is the sorted list of top-level keys observed for that
    /// call across every repeat. Used as the signature dedup key.
    pub input_shape: Vec<Vec<String>>,
    /// How many turns the pattern was observed in.
    pub repeat_count: usize,
    /// Synthesised skill name (e.g. `auto-grep-read-edit-bash`).
    pub suggested_name: String,
    /// One-line description for the staged skill — used by F11 dedup
    /// (Levenshtein) and by the human curator's review UI.
    pub suggested_description: String,
}

impl PatternDetector {
    pub fn detect(&self, traces: &[TurnTrace]) -> Vec<DraftCandidate> {
        // Build the signature for each turn: (sequence, shape-per-call).
        let mut groups: BTreeMap<(Vec<String>, Vec<Vec<String>>), usize> = BTreeMap::new();
        for t in traces {
            if t.tool_calls.len() < self.min_seq_len {
                continue;
            }
            let seq: Vec<String> = t.tool_calls.iter().map(|c| c.tool_name.clone()).collect();
            let shape: Vec<Vec<String>> = t
                .tool_calls
                .iter()
                .map(|c| top_level_keys(&c.input))
                .collect();
            *groups.entry((seq, shape)).or_insert(0) += 1;
        }

        groups
            .into_iter()
            .filter(|(_, count)| *count >= self.min_repeats)
            .map(|((seq, shape), count)| DraftCandidate {
                suggested_name: synth_name(&seq),
                suggested_description: synth_description(&seq, count),
                tool_sequence: seq,
                input_shape: shape,
                repeat_count: count,
            })
            .collect()
    }
}

fn top_level_keys(v: &Value) -> Vec<String> {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<String> = m.keys().cloned().collect();
            keys.sort();
            keys
        }
        _ => vec![],
    }
}

fn synth_name(seq: &[String]) -> String {
    let joined = seq
        .iter()
        .map(|s| s.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    format!("auto-{joined}")
}

fn synth_description(seq: &[String], repeats: usize) -> String {
    format!(
        "Auto-drafted from {repeats} repeated turns: {}",
        seq.join(" → ")
    )
}

// ---------------------------------------------------------------------------
// F10.C — DraftWriter (stages DraftCandidate as P4 Procedure)
// ---------------------------------------------------------------------------

/// Fixed UUID v5 namespace for F10-drafted procedures. Generated once
/// and pinned; never regenerate — the determinism of `stage()` depends
/// on this constant being stable across releases.
const NAMESPACE_F10_PROCEDURE: Uuid =
    Uuid::from_u128(0x_F10A_F10A_F10A_F10A_7C0D_E517_C0F1_E10D_u128);

pub struct DraftWriter {
    mem: Arc<dyn MemoryApi>,
}

impl DraftWriter {
    pub fn new(mem: Arc<dyn MemoryApi>) -> Self {
        Self { mem }
    }

    /// Stage a `DraftCandidate` as a P4 procedure with status=Staged.
    ///
    /// **Idempotency.** `Procedure.id` is a `Uuid`. We mint a deterministic
    /// v5 UUID from `NAMESPACE_F10_PROCEDURE` + the candidate's canonical
    /// signature bytes. The same `(tool_sequence, input_shape)` signature
    /// always yields the same UUID, so the existing
    /// `INSERT OR REPLACE INTO procedures` keyed on `id` collapses repeated
    /// stages to one row.
    pub async fn stage(
        &self,
        candidate: &DraftCandidate,
        token: AccessToken,
    ) -> MemResult<ProcedureId> {
        let id = ProcedureId(deterministic_uuid(candidate));
        let proc = Procedure {
            id,
            tier: Tier::Project,
            ts: now_secs(),
            name: candidate.suggested_name.clone(),
            description: candidate.suggested_description.clone(),
            artifact: synth_skill_body(candidate),
            status: ProcedureStatus::Staged,
            created_by: "main-agent-f10".to_string(),
            thompson_alpha: 1.0,
            thompson_beta: 1.0,
            use_count: 0,
            success_count: 0,
        };
        self.mem.upsert_procedure(proc, token).await
    }
}

/// Deterministic UUID v5 derived from the candidate's canonical
/// signature: tool_sequence joined with `\x1f` (ASCII US), a `\x1e`
/// (ASCII RS) separator, then each `input_shape[i]` joined with `,`
/// and terminated by `\x1f`. ASCII RS/US avoid ambiguity when tool
/// names or key names contain `-`, `_`, or `,`.
pub(crate) fn deterministic_uuid(c: &DraftCandidate) -> Uuid {
    let mut buf = String::new();
    buf.push_str(&c.tool_sequence.join("\x1f"));
    buf.push('\x1e');
    for keys in &c.input_shape {
        buf.push_str(&keys.join(","));
        buf.push('\x1f');
    }
    Uuid::new_v5(&NAMESPACE_F10_PROCEDURE, buf.as_bytes())
}

/// Seconds since UNIX_EPOCH. Matches the P4 internal stamping pattern
/// so timestamp shape is consistent across F10 and the partition store.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Render the `TraceEvent.trace` JSON payload for a freshly staged draft.
/// Hosts that have opted into `structured_traces` will decode and surface
/// this; hosts that haven't drop it silently per the W0 host decoder
/// contract. Carries `kind: "skill_drafted"` plus the candidate's
/// suggested name/description, tool sequence, and repeat count — enough
/// for a curator UI to render a review entry without rehydrating the
/// staged P4 procedure.
pub fn render_skill_drafted_payload(c: &DraftCandidate) -> serde_json::Value {
    serde_json::json!({
        "kind": "skill_drafted",
        "name": c.suggested_name,
        "description": c.suggested_description,
        "tool_sequence": c.tool_sequence,
        "repeat_count": c.repeat_count,
    })
}

fn synth_skill_body(c: &DraftCandidate) -> String {
    let tools = c.tool_sequence.join(", ");
    format!(
        "---\nname: {name}\ndescription: {desc}\nstatus: staged\nallowed-tools: {tools}\n---\n\n# {name}\n\n{desc}\n\nObserved tool sequence: {seq}.\nObserved input shape: {shape:?}.\nObserved repeats: {repeats}.\n",
        name = c.suggested_name,
        desc = c.suggested_description,
        tools = tools,
        seq = c.tool_sequence.join(" → "),
        shape = c.input_shape,
        repeats = c.repeat_count,
    )
}
