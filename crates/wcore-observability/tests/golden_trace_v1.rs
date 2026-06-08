//! Golden snapshots that lock the on-wire JSON shape of TurnTrace and
//! ToolCallTrace. Future PRs that drift a field name, default-skip rule, or
//! serialization tag fail one of these tests before the schema reaches
//! downstream consumers (W4 RPC, W5 memory, W6 cost).
//!
//! Adding new fields: extend the relevant golden and remember to update
//! TurnTrace / ToolCallTrace serialization in the SAME PR.

use serde_json::json;
use wcore_observability::SOURCE_PRODUCT;
use wcore_observability::trace::{HookActionRecord, TaskOutcome, ToolCallTrace, TurnTrace};

fn tcc_full() -> ToolCallTrace {
    let mut t = ToolCallTrace::new(
        "call_001".into(),
        "Read".into(),
        json!({ "path": "/etc/hosts" }),
    );
    t.output_summary = "127.0.0.1 localhost".into();
    t.duration_ms = 42;
    t.bytes_in = 18;
    t.bytes_out = 24;
    t
}

#[test]
fn golden_tool_call_trace_default_flags_omitted() {
    let got = serde_json::to_value(tcc_full()).unwrap();
    let want = json!({
        "call_id": "call_001",
        "tool_name": "Read",
        "input": { "path": "/etc/hosts" },
        "output_summary": "127.0.0.1 localhost",
        "duration_ms": 42,
        "bytes_in": 18,
        "bytes_out": 24,
        "source_product": SOURCE_PRODUCT,
        // error / cancelled / partial absent when at their defaults
    });
    assert_eq!(got, want, "ToolCallTrace schema drift");
}

#[test]
fn golden_tool_call_trace_with_error_and_cancel() {
    let mut t = tcc_full();
    t.error = Some("ENOENT".into());
    t.cancelled = true;
    t.partial = true;
    let got = serde_json::to_value(&t).unwrap();
    assert_eq!(got["error"], "ENOENT");
    assert_eq!(got["cancelled"], true);
    assert_eq!(got["partial"], true);
}

#[test]
fn golden_turn_trace_full_shape() {
    let turn = TurnTrace {
        turn: 3,
        model: "claude-3-5-haiku".into(),
        provider: "anthropic".into(),
        input_tokens: 10_000,
        output_tokens: 250,
        cache_read: 8_000,
        cache_write: 0,
        cache_hit_rate: 0.8,
        cost_usd: 0.0,
        tool_calls: vec![tcc_full()],
        hook_actions: vec![],
        source_product: SOURCE_PRODUCT.to_string(),
    };
    let got = serde_json::to_value(&turn).unwrap();
    let want = json!({
        "turn": 3,
        "model": "claude-3-5-haiku",
        "provider": "anthropic",
        "input_tokens": 10000,
        "output_tokens": 250,
        "cache_read": 8000,
        "cache_write": 0,
        "cache_hit_rate": 0.8,
        "cost_usd": 0.0,
        "tool_calls": [{
            "call_id": "call_001",
            "tool_name": "Read",
            "input": { "path": "/etc/hosts" },
            "output_summary": "127.0.0.1 localhost",
            "duration_ms": 42,
            "bytes_in": 18,
            "bytes_out": 24,
            "source_product": SOURCE_PRODUCT,
        }],
        "hook_actions": [],
        "source_product": SOURCE_PRODUCT,
    });
    assert_eq!(got, want, "TurnTrace schema drift");
}

#[test]
fn golden_task_outcome_variants_serialize_with_kind_tag() {
    let cases = [
        (TaskOutcome::Success, json!({ "kind": "success" })),
        (TaskOutcome::Timeout, json!({ "kind": "timeout" })),
        (TaskOutcome::UserAborted, json!({ "kind": "user_aborted" })),
    ];
    for (variant, want) in cases {
        let got = serde_json::to_value(&variant).unwrap();
        assert_eq!(got, want, "TaskOutcome variant drift");
    }

    let failure = TaskOutcome::Failure {
        reason: "rate-limited".into(),
    };
    let got = serde_json::to_value(&failure).unwrap();
    assert_eq!(got, json!({ "kind": "failure", "reason": "rate-limited" }));

    let suspended = TaskOutcome::Suspended {
        reason: "approval_required".into(),
    };
    let got = serde_json::to_value(&suspended).unwrap();
    assert_eq!(
        got,
        json!({ "kind": "suspended", "reason": "approval_required" })
    );
}

#[test]
fn golden_hook_action_record_shape() {
    // W1 leaves HookActionRecord as a minimal stub; W2 extends it. Locking
    // the stub shape now prevents W2 from accidentally renaming the slot.
    let r = HookActionRecord {
        kind: "pre_tool".into(),
        timestamp_ms: 1_700_000_000_000,
    };
    let got = serde_json::to_value(&r).unwrap();
    assert_eq!(
        got,
        json!({ "kind": "pre_tool", "timestamp_ms": 1_700_000_000_000_u64 })
    );
}
