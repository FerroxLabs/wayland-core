//! 23B-H1 — the journal envelope round-trip invariant.
//!
//! # What this file is for
//!
//! 23B-01 raised a HIGH: a `wayland-core` run that exits normally can write a
//! session journal the product cannot read back, failing with
//! `journal checksum mismatch at sequence N`. Every operator verb reads the
//! journal, so the session becomes permanently unreachable.
//!
//! That error has exactly one precondition worth testing. Reading a journal
//! runs three checks in order (`session_journal.rs::verify_chain_from`):
//!
//!   1. the frame's own SHA-256 digest, over the exact bytes on disk;
//!   2. `previous_checksum` linking this envelope to the last one;
//!   3. `computed_checksum() == checksum`.
//!
//! A `ChecksumMismatch` means (1) and (2) PASSED. So the bytes on disk are
//! byte-for-byte what the writer wrote, and the chain is intact — and yet
//! re-hashing the deserialized event disagrees with the stored hash. Since the
//! writer hashes and writes the same immutable value, the remaining mechanism
//! is that **serializing a deserialized event does not reproduce the bytes it
//! was deserialized from**.
//!
//! That is not hypothetical for this schema. `SessionEvent` carries fields
//! shaped `#[serde(default, skip_serializing_if = "Option::is_none")]
//! Option<serde_json::Value>`. `Some(Value::Null)` serializes to `"f":null`,
//! but serde deserializes a JSON `null` into `Option<_>` as `None`, which the
//! skip attribute then OMITS on re-serialization. Write and read disagree, the
//! recomputed hash differs, and the journal becomes unreadable — intermittently,
//! because it depends on a producer happening to supply `Some(Value::Null)`.
//!
//! These tests assert the invariant the journal's integrity rests on:
//!
//!     serialize(deserialize(serialize(event))) == serialize(event)
//!
//! They can go red, and one of them DID: before the fix,
//! `option_value_null_is_stable_across_a_round_trip` failed with the written
//! bytes ending `..."effect_receipt":null}` and the re-serialized bytes ending
//! `...}`. Restore `skip_serializing_if = "Option::is_none"` on either
//! `effect_receipt` field in `session_journal/model.rs` and it fails again on
//! exactly that byte.

use serde_json::json;
use wcore_agent::session_journal::{CompletionOutcome, SessionEvent, StoredToolInput};
use wcore_types::tool::{ToolEffectContract, ToolEffectKind};

/// The invariant, expressed over one event.
fn assert_round_trip_stable(event: &SessionEvent, what: &str) {
    let first = serde_json::to_vec(event).expect("serialize");
    let decoded: SessionEvent = serde_json::from_slice(&first)
        .unwrap_or_else(|e| panic!("{what}: on-disk bytes must decode: {e}"));
    let second = serde_json::to_vec(&decoded).expect("re-serialize");
    assert_eq!(
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second),
        "{what}: re-serializing a decoded event must reproduce the bytes it \
         came from — otherwise the journal's own checksum check rejects a \
         journal the product wrote correctly"
    );
}

#[test]
fn plain_events_are_stable_across_a_round_trip() {
    let events = vec![
        (
            "turn_started",
            SessionEvent::TurnStarted {
                turn_id: "t1".into(),
                user_message: "hello".into(),
            },
        ),
        (
            "turn_failed_with_control_chars",
            SessionEvent::TurnFailed {
                turn_id: "t1".into(),
                // Provider errors carry arbitrary text. Control characters,
                // tabs and non-ASCII all have to survive verbatim.
                error: "dispatch failed:\n\ttimeout \u{1f600} \u{7f} \"quoted\"".into(),
            },
        ),
        (
            "conversation_message",
            SessionEvent::ConversationMessageCommitted {
                turn_id: "t1".into(),
                message_index: 0,
                message: json!({"role": "user", "content": [{"type": "text", "text": "hi"}]}),
                message_digest: "d".repeat(64),
            },
        ),
        (
            "tool_finished_with_null_result",
            SessionEvent::ToolExecutionFinished {
                tool_execution_id: "x1".into(),
                outcome: CompletionOutcome::Succeeded,
                // A plain `serde_json::Value` field carrying null: this one is
                // stable, because Value's own Deserialize maps null to
                // Value::Null rather than to a missing field.
                result: serde_json::Value::Null,
            },
        ),
    ];
    for (what, ev) in &events {
        assert_round_trip_stable(ev, what);
    }
}

#[test]
fn float_bearing_values_are_stable_across_a_round_trip() {
    // Provider payloads carry computed floats. serde_json writes the shortest
    // representation that round-trips, so these must be exact; if they were
    // not, every journal carrying a cost or a score would be unreadable.
    //
    // THIS TEST SHIPPED VACUOUS. It carried this name and this reason while
    // the defect it describes was live in every release from v0.10.0 to
    // v0.13.11, because all five of its original values happen to survive the
    // imprecise parser. WRITING the float was never the problem -- serde uses
    // ryu and is exact. READING it back was: without serde_json's
    // `float_roundtrip` feature the fast parse path double-rounds and lands
    // 1 ULP off, so the re-serialization differs from the bytes on disk, the
    // recomputed `state_payload_digest` disagrees, and every later turn in
    // that conversation is refused forever with `state digest mismatch`.
    //
    // MEASURED on this workspace, both arms, 200,000 uniform f64 in [0,1):
    // 10.51% fail to round-trip WITHOUT the feature, 0.00% WITH it. All five
    // original values are in the 89% that pass, which is the whole reason this
    // test was green. Do not reduce this set to "nice" constants again.
    let mut named: Vec<f64> = vec![0.1, 1.0 / 3.0, 1e-10, 1e22, f64::MIN_POSITIVE];
    // Each of these was MEASURED to break with the feature off.
    named.extend([
        0.39897699999999997,
        0.38595771669529844,
        0.9238829120510785,
        0.20599677708342345,
        0.17634486044635123,
        0.9956056817917075,
    ]);
    for v in named {
        let ev = SessionEvent::ToolExecutionFinished {
            tool_execution_id: "f".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!({ "value": v, "nested": [v, {"again": v}] }),
        };
        assert_round_trip_stable(&ev, &format!("float {v}"));
    }

    // A named set can only ever prove the values someone thought of, and the
    // last one missed a defect that hits one value in ten. This sweep is what
    // makes the test unable to go vacuous: a deterministic xorshift over a
    // realistic cost/score range, carried in ONE event so it costs one
    // round-trip. At 10.51% per value the feature being dropped is caught with
    // certainty, and the sequence is fixed so a failure is reproducible.
    let mut s: u64 = 0x2545F4914F6CDD1D;
    let mut swept: Vec<serde_json::Value> = Vec::with_capacity(2048);
    for _ in 0..2048 {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let v = (s >> 11) as f64 / (1u64 << 53) as f64;
        swept.push(json!(v));
    }
    let ev = SessionEvent::ToolExecutionFinished {
        tool_execution_id: "sweep".into(),
        outcome: CompletionOutcome::Succeeded,
        result: json!({ "costs": swept }),
    };
    assert_round_trip_stable(
        &ev,
        "2048 swept floats -- if this is the only arm that failed, \
         serde_json's `float_roundtrip` feature has been dropped from the \
         workspace Cargo.toml",
    );
}

#[test]
fn option_value_null_is_stable_across_a_round_trip() {
    // THE hazardous shape. `effect_receipt` is
    //   #[serde(default, skip_serializing_if = "Option::is_none")]
    //   Option<serde_json::Value>
    // so `Some(Value::Null)` writes `"effect_receipt":null` and reads back as
    // `None`, which re-serializes to nothing at all. A journal carrying this
    // shape is written successfully and can never be read back.
    let ev = SessionEvent::ToolIntentRecordedV2 {
        tool_execution_id: "x1".into(),
        idempotency_key: "k1".into(),
        retry_of: None,
        provider_call_id: "c1".into(),
        turn_id: "t1".into(),
        ordinal: 0,
        tool: "Read".into(),
        requested_input: StoredToolInput::Redacted {
            exact_digest: "a".repeat(64),
            summary: None,
        },
        requested_input_digest: "a".repeat(64),
        effective_input: StoredToolInput::Redacted {
            exact_digest: "b".repeat(64),
            summary: None,
        },
        effective_input_digest: "b".repeat(64),
        effect_contract: ToolEffectContract::default(),
        effect_receipt: Some(serde_json::Value::Null),
        pre_hook_phase_id: None,
    };
    assert_round_trip_stable(&ev, "effect_receipt = Some(Value::Null)");
}

#[test]
fn nested_object_key_order_is_stable_across_a_round_trip() {
    // Provider responses are objects whose key order is whatever the provider
    // emitted. Whether serde_json is built with `preserve_order` or not, the
    // order the writer emitted must be the order the reader re-emits.
    let ev = SessionEvent::ConversationMessageCommitted {
        turn_id: "t1".into(),
        message_index: 3,
        message: json!({
            "zulu": 1, "alpha": 2, "mike": {"yankee": true, "bravo": [1, 2, 3]},
            "": "empty key", "unicode\u{00e9}": "value"
        }),
        message_digest: "c".repeat(64),
    };
    assert_round_trip_stable(&ev, "nested object key order");
}

#[test]
fn a_null_receipt_survives_a_real_journal_write_and_read() {
    // The end-to-end form of the invariant, through the real writer and the
    // real reader rather than through serde alone. Before the fix this wrote a
    // journal that `JournalError::ChecksumMismatch` rejected on read — the
    // exact 23B-H1 symptom, from a run that exited normally.
    //
    // The contract declares a reconciler because the reducer refuses an effect
    // receipt without one. That is also what makes this shape REACHABLE in
    // production: any tool whose effect contract names a reconciler can supply
    // a receipt, and `JournalEffectScope::prepare_tool_with_effect_receipt`
    // takes a bare `Value` and does not reject a null one.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("null-receipt.journal");

    let journal = wcore_agent::session_journal::SessionJournal::open(&path, "s-null")
        .expect("open journal for write");
    journal
        .append(SessionEvent::TurnStarted {
            turn_id: "t1".into(),
            user_message: "do a thing".into(),
        })
        .expect("turn must start");
    journal
        .append(SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id: "x1".into(),
            idempotency_key: "k1".into(),
            retry_of: None,
            provider_call_id: "c1".into(),
            turn_id: "t1".into(),
            ordinal: 0,
            tool: "Write".into(),
            requested_input: StoredToolInput::redacted("a".repeat(64)),
            requested_input_digest: "a".repeat(64),
            effective_input: StoredToolInput::redacted("b".repeat(64)),
            effective_input_digest: "b".repeat(64),
            effect_contract: ToolEffectContract {
                kind: ToolEffectKind::FilesystemTransactional,
                reconciler: Some("filesystem".into()),
            },
            effect_receipt: Some(serde_json::Value::Null),
            pre_hook_phase_id: None,
        })
        .expect("append must succeed");
    drop(journal);

    // A fresh reader, as `--resume` and every operator verb use.
    let reopened = wcore_agent::session_journal::SessionJournal::open(&path, "s-null")
        .expect("a journal the product just wrote must be readable back");
    drop(reopened);
}
