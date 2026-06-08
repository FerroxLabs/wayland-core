use wcore_replay::{DiffKind, Differ, Trace, TraceEvent};

fn mk(events: Vec<TraceEvent>) -> Trace {
    Trace {
        wcore_version: "0.6.0".into(),
        session_id: "s".into(),
        events,
    }
}

#[test]
fn identical_traces_diff_unchanged() {
    let a = mk(vec![TraceEvent::UserMessage {
        ts_ms: 1,
        text: "x".into(),
    }]);
    let b = a.clone();
    let d = Differ::compare(&a, &b);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].kind, DiffKind::Unchanged);
    assert!(Differ::first_divergence(&a, &b).is_none());
}

#[test]
fn first_divergence_points_at_changed_event() {
    let a = mk(vec![
        TraceEvent::UserMessage {
            ts_ms: 1,
            text: "x".into(),
        },
        TraceEvent::AssistantMessage {
            ts_ms: 2,
            text: "old".into(),
        },
    ]);
    let b = mk(vec![
        TraceEvent::UserMessage {
            ts_ms: 1,
            text: "x".into(),
        },
        TraceEvent::AssistantMessage {
            ts_ms: 2,
            text: "new".into(),
        },
    ]);
    let div = Differ::first_divergence(&a, &b).expect("must find a change");
    assert_eq!(div.index, 1);
    assert_eq!(div.kind, DiffKind::Changed);
}

#[test]
fn added_and_removed_surface_at_correct_positions() {
    let a = mk(vec![TraceEvent::UserMessage {
        ts_ms: 1,
        text: "x".into(),
    }]);
    let b = mk(vec![
        TraceEvent::UserMessage {
            ts_ms: 1,
            text: "x".into(),
        },
        TraceEvent::AssistantMessage {
            ts_ms: 2,
            text: "extra".into(),
        },
    ]);
    let d = Differ::compare(&a, &b);
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].kind, DiffKind::Unchanged);
    assert_eq!(d[1].kind, DiffKind::Added);

    // Reverse: a now has the extra event → it shows as Removed from a's POV.
    let d_rev = Differ::compare(&b, &a);
    assert_eq!(d_rev[1].kind, DiffKind::Removed);
}

#[test]
fn tool_call_payload_diff_is_structural() {
    let a = mk(vec![TraceEvent::ToolCall {
        ts_ms: 1,
        tool: "Read".into(),
        input: serde_json::json!({"path": "/a"}),
        output: serde_json::json!({"ok": true}),
        duration_ms: 5,
    }]);
    let b = mk(vec![TraceEvent::ToolCall {
        ts_ms: 1,
        tool: "Read".into(),
        input: serde_json::json!({"path": "/b"}),
        output: serde_json::json!({"ok": true}),
        duration_ms: 5,
    }]);
    let d = Differ::compare(&a, &b);
    assert_eq!(d[0].kind, DiffKind::Changed);
}
