use wcore_replay::{Trace, TraceEvent};

#[test]
fn trace_round_trips_through_json() {
    let trace = Trace {
        wcore_version: "0.6.0".into(),
        session_id: "s-1".into(),
        events: vec![
            TraceEvent::UserMessage {
                ts_ms: 1,
                text: "hello".into(),
            },
            TraceEvent::LlmCall {
                ts_ms: 2,
                provider: "anthropic".into(),
                model: "claude-sonnet-4-7".into(),
                prompt_tokens: 42,
                completion_tokens: 17,
                response: "hi back".into(),
            },
            TraceEvent::ToolCall {
                ts_ms: 3,
                tool: "Read".into(),
                input: serde_json::json!({"file_path": "/tmp/a.txt"}),
                output: serde_json::json!({"ok": "contents..."}),
                duration_ms: 12,
            },
        ],
    };
    let json = serde_json::to_string(&trace).expect("serialize");
    let back: Trace = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.session_id, trace.session_id);
    assert_eq!(back.wcore_version, trace.wcore_version);
    assert_eq!(back.events.len(), trace.events.len());
    match (&back.events[1], &trace.events[1]) {
        (TraceEvent::LlmCall { model: a, .. }, TraceEvent::LlmCall { model: b, .. }) => {
            assert_eq!(a, b)
        }
        _ => panic!("LlmCall variant lost during round-trip"),
    }
    // ToolCall round-trip preserves the JSON payloads structurally.
    match (&back.events[2], &trace.events[2]) {
        (
            TraceEvent::ToolCall {
                input: ia,
                output: oa,
                ..
            },
            TraceEvent::ToolCall {
                input: ib,
                output: ob,
                ..
            },
        ) => {
            assert_eq!(ia, ib);
            assert_eq!(oa, ob);
        }
        _ => panic!("ToolCall variant lost during round-trip"),
    }
}

#[test]
fn trace_rejects_unknown_event_variant() {
    let bad = r#"{ "wcore_version":"0.6.0", "session_id":"s", "events":[{"type":"Wormhole"}] }"#;
    let err = serde_json::from_str::<Trace>(bad).expect_err("must reject unknown variant");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("variant") || msg.contains("wormhole"),
        "expected variant-rejection message, got: {msg}"
    );
}

#[test]
fn version_skew_guard_blocks_unless_forced() {
    use wcore_replay::Replayer;
    let trace = Trace {
        wcore_version: "0.5.99".into(),
        session_id: "old".into(),
        events: vec![],
    };
    let r = Replayer::new();
    assert!(r.dry_run(&trace, "0.6.0").is_err());
    let r_force = Replayer {
        force_version_skew: true,
    };
    assert!(r_force.dry_run(&trace, "0.6.0").is_ok());
}
