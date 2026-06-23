//! F10 pattern detector — synthetic `TurnTrace` fixtures asserting the
//! detector's contract.
//!
//! Contract:
//! - A pattern is a sequence of `tool_name`s of length ≥ MIN_TOOL_SEQ_LEN.
//! - A pattern qualifies as a draft candidate when it appears ≥ MIN_REPEATS
//!   times across the input traces with identical sequence and stable
//!   input *shape* (same JSON keys, but values may differ).
//! - The detector returns one `DraftCandidate` per qualifying pattern,
//!   deduped by signature (sequence + key-shape).

use std::sync::Arc;

use serde_json::json;
use wcore_memory::Memory;
use wcore_memory::api::MemoryApi;
use wcore_memory::v2_types::{AccessToken, ProcedureStatus, Tier};
use wcore_observability::trace::{ToolCallTrace, TurnTrace};
use wcore_skills::draft::{DraftCandidate, DraftWriter, PatternDetector};

fn turn_with_calls(turn: usize, calls: Vec<(&str, serde_json::Value)>) -> TurnTrace {
    TurnTrace {
        turn,
        model: "test".into(),
        provider: "test".into(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read: 0,
        cache_write: 0,
        cache_hit_rate: 0.0,
        cost_usd: 0.0,
        tool_calls: calls
            .into_iter()
            .enumerate()
            .map(|(i, (n, inp))| ToolCallTrace::new(format!("c-{turn}-{i}"), n.into(), inp))
            .collect(),
        hook_actions: vec![],
        source_product: "wcore-agent".into(),
        agent_run_id: String::new(),
    }
}

#[test]
fn detect_returns_empty_when_no_repeats() {
    let traces = vec![turn_with_calls(
        0,
        vec![
            ("Grep", json!({"pattern": "foo"})),
            ("Read", json!({"path": "a"})),
        ],
    )];
    let out = PatternDetector::default().detect(&traces);
    assert!(out.is_empty(), "single turn cannot repeat");
}

#[test]
fn detect_returns_empty_when_below_min_seq_len() {
    // Default MIN_TOOL_SEQ_LEN = 5. Sequence of 2 never qualifies even if
    // it repeats 10 times.
    let turn = turn_with_calls(0, vec![("Grep", json!({})), ("Read", json!({}))]);
    let traces: Vec<TurnTrace> = (0..10)
        .map(|i| {
            let mut t = turn.clone();
            t.turn = i;
            t
        })
        .collect();
    let out = PatternDetector::default().detect(&traces);
    assert!(out.is_empty(), "short sequences never qualify");
}

#[test]
fn detect_returns_candidate_when_pattern_repeats_3_times_at_min_len() {
    let seq = vec![
        ("Grep", json!({"pattern": "fn run("})),
        ("Read", json!({"path": "src/lib.rs"})),
        (
            "Edit",
            json!({"path": "src/lib.rs", "old": "x", "new": "y"}),
        ),
        ("Bash", json!({"cmd": "cargo test"})),
        ("Bash", json!({"cmd": "cargo clippy"})),
    ];
    let traces: Vec<TurnTrace> = (0..3).map(|i| turn_with_calls(i, seq.clone())).collect();
    let out = PatternDetector::default().detect(&traces);
    assert_eq!(
        out.len(),
        1,
        "5-tool sequence repeating 3x must produce one candidate"
    );
    let c: &DraftCandidate = &out[0];
    assert_eq!(
        c.tool_sequence,
        vec!["Grep", "Read", "Edit", "Bash", "Bash"]
    );
    assert_eq!(c.repeat_count, 3);
}

#[test]
fn detect_ignores_value_drift_within_stable_shape() {
    // Same tool sequence, same JSON keys — different VALUES count as the
    // same pattern (the user is doing the same task with different inputs).
    let mk = |pattern: &str, path: &str| {
        vec![
            ("Grep", json!({"pattern": pattern})),
            ("Read", json!({"path": path})),
            ("Edit", json!({"path": path, "old": "x", "new": "y"})),
            ("Bash", json!({"cmd": "cargo test"})),
            ("Bash", json!({"cmd": "cargo clippy"})),
        ]
    };
    let traces = vec![
        turn_with_calls(0, mk("fn a", "a.rs")),
        turn_with_calls(1, mk("fn b", "b.rs")),
        turn_with_calls(2, mk("fn c", "c.rs")),
    ];
    let out = PatternDetector::default().detect(&traces);
    assert_eq!(
        out.len(),
        1,
        "shape-stable variation collapses to one candidate"
    );
    assert_eq!(out[0].repeat_count, 3);
}

#[test]
fn detect_distinguishes_different_key_shapes() {
    // Same tool sequence but DIFFERENT JSON key sets in one of the inputs
    // → not the same pattern.
    let mk_keys_a = || {
        vec![
            ("Grep", json!({"pattern": "x"})),
            ("Read", json!({"path": "a"})),
            ("Edit", json!({"path": "a", "old": "x", "new": "y"})),
            ("Bash", json!({"cmd": "z"})),
            ("Bash", json!({"cmd": "w"})),
        ]
    };
    let mk_keys_b = || {
        vec![
            ("Grep", json!({"regex": "x"})), // "regex" instead of "pattern"
            ("Read", json!({"path": "a"})),
            ("Edit", json!({"path": "a", "old": "x", "new": "y"})),
            ("Bash", json!({"cmd": "z"})),
            ("Bash", json!({"cmd": "w"})),
        ]
    };
    let traces = vec![
        turn_with_calls(0, mk_keys_a()),
        turn_with_calls(1, mk_keys_a()),
        turn_with_calls(2, mk_keys_b()),
    ];
    let out = PatternDetector::default().detect(&traces);
    assert!(
        out.is_empty(),
        "two-then-one with different key shape does not meet min-repeats threshold"
    );
}

#[test]
fn detect_min_repeats_is_configurable() {
    let seq = vec![
        ("Grep", json!({"q": "x"})),
        ("Read", json!({"p": "x"})),
        ("Edit", json!({"p": "x"})),
        ("Bash", json!({"c": "x"})),
        ("Bash", json!({"c": "y"})),
    ];
    let traces: Vec<TurnTrace> = (0..2).map(|i| turn_with_calls(i, seq.clone())).collect();

    let pd_default = PatternDetector::default();
    assert!(
        pd_default.detect(&traces).is_empty(),
        "default needs 3 repeats"
    );

    let pd_loose = PatternDetector {
        min_repeats: 2,
        min_seq_len: 5,
    };
    assert_eq!(
        pd_loose.detect(&traces).len(),
        1,
        "loose threshold accepts 2 repeats"
    );
}

// ---------------------------------------------------------------------------
// DraftWriter integration tests
// ---------------------------------------------------------------------------

/// In-memory `Memory` backing W9 fixtures. Returns the concrete `Memory`
/// (cheaply cloneable; everything inside is Arc-wrapped) so tests can
/// access `dispatcher.procedural.get()` directly to verify P4 state —
/// `MemoryApi::search` currently only spans P2 episodes.
async fn open_memory_tmp() -> Memory {
    let tmp = tempfile::tempdir().unwrap();
    wcore_memory::open_for_test(tmp.path()).await.unwrap()
}

#[tokio::test]
async fn stage_writes_staged_procedure_into_p4() {
    let mem = open_memory_tmp().await;
    let api: Arc<dyn MemoryApi> = Arc::new(mem.clone());
    let writer = DraftWriter::new(api);
    let candidate = DraftCandidate {
        tool_sequence: vec![
            "Grep".into(),
            "Read".into(),
            "Edit".into(),
            "Bash".into(),
            "Bash".into(),
        ],
        input_shape: vec![
            vec!["pattern".into()],
            vec!["path".into()],
            vec!["new".into(), "old".into(), "path".into()],
            vec!["cmd".into()],
            vec!["cmd".into()],
        ],
        repeat_count: 3,
        suggested_name: "auto-grep-read-edit-bash-bash".into(),
        suggested_description:
            "Auto-drafted from 3 repeated turns: Grep → Read → Edit → Bash → Bash".into(),
    };

    let pid = writer
        .stage(&candidate, AccessToken::MainAgent)
        .await
        .expect("stage must succeed");

    // Verify: read the procedure back directly from P4 and confirm
    // status=Staged + the synthesised body contains the suggested name.
    let proc = mem
        .dispatcher
        .procedural
        .get(&pid, Tier::Project)
        .await
        .expect("procedure row exists in P4");
    assert_eq!(proc.status, ProcedureStatus::Staged);
    assert_eq!(proc.name, "auto-grep-read-edit-bash-bash");
    assert!(proc.artifact.contains("auto-grep-read-edit-bash-bash"));
    assert_eq!(proc.created_by, "main-agent-f10");
}

#[tokio::test]
async fn stage_is_idempotent_on_same_signature() {
    let mem = open_memory_tmp().await;
    let api: Arc<dyn MemoryApi> = Arc::new(mem.clone());
    let writer = DraftWriter::new(api);
    let candidate = DraftCandidate {
        tool_sequence: vec!["A".into(); 5],
        input_shape: vec![vec![]; 5],
        repeat_count: 3,
        suggested_name: "auto-a-a-a-a-a".into(),
        suggested_description: "x".into(),
    };
    let pid_first = writer
        .stage(&candidate, AccessToken::MainAgent)
        .await
        .unwrap();
    let pid_second = writer
        .stage(&candidate, AccessToken::MainAgent)
        .await
        .unwrap();
    // Determinism: same (tool_sequence, input_shape) signature → same v5
    // UUID. The underlying INSERT OR REPLACE on procedures.id collapses
    // the row to a single one.
    assert_eq!(
        pid_first, pid_second,
        "same signature must collapse to one P4 row"
    );
    // The second upsert must not have created a second row.
    let _proc = mem
        .dispatcher
        .procedural
        .get(&pid_first, Tier::Project)
        .await
        .expect("single P4 row exists after two stages");
}
