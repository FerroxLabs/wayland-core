//! 23B-H1, generalised — the journal's own compatibility contract and its
//! integrity check contradict each other.
//!
//! # The defect
//!
//! Reading a journal frame runs two independent field-level judgements:
//!
//!   * `reject_unknown_event_fields` walks the RAW json against the canonical
//!     re-encoding of the decoded event and rejects any raw field the decode
//!     dropped — EXCEPT the fifteen paths `known_omitted_default` blesses as
//!     "an explicit default, equivalent to absent". For
//!     `tool_intent_recorded_v2` those are `retry_of`, `effect_receipt` and
//!     `pre_hook_phase_id`, all three named in one match arm.
//!
//!   * `verify_chain_from` recomputes `JournalEnvelope::computed_checksum`,
//!     which hashes a RE-SERIALIZATION of the decoded event, and compares it to
//!     the SHA-256 the producer stored.
//!
//! Every blessed encoding is by definition one the decode drops. So for all
//! fifteen, the re-serialization covers different bytes than the producer
//! hashed, and the second check calls corrupt exactly what the first check just
//! declared valid. The frame parses; the chain then rejects it with
//! `journal checksum mismatch at sequence N`. Every one of the twelve `session`
//! operator verbs reads through that chain, so the session is permanently
//! unreachable — this is the 23B-01 symptom.
//!
//! # Why the existing tests cannot see it
//!
//! Two tests already cover this ground and both are vacuous with respect to the
//! checksum:
//!
//!   * `session_journal.rs::known_explicit_event_defaults_are_wire_compatible…`
//!     injects the explicit nulls into a body whose `checksum` was computed from
//!     the CLEAN encoding, so the stored hash happens to match the stripped
//!     form. It also stops at `parse_complete_frames` and never reaches
//!     `verify_chain_from`. No real producer of those bytes could store that
//!     hash.
//!
//!   * `journal_envelope_roundtrip.rs` asserts
//!     `serialize(deserialize(serialize(e))) == serialize(e)` over Rust VALUES.
//!     `retry_of` is `Option<String>`, and no `Option<String>` value serializes
//!     to `null` — so that invariant is structurally incapable of constructing
//!     the failing case. It found `effect_receipt` only because
//!     `Option<serde_json::Value>` can hold `Some(Value::Null)`.
//!
//! These tests close both gaps by building the frame the way a real producer
//! must: the stored SHA-256 is computed over the bytes actually on disk.
//!
//! # These tests can fail
//!
//! `wire_blessed_defaults_are_readable` is red at `f8b8ec25` with
//! `ChecksumMismatch { seq: 1 }`. `a_tampered_body_is_still_rejected` is the
//! guard in the other direction: it goes green — and the integrity check is
//! gone — if the fix is ever mistaken for "the frame digest already proved
//! these bytes, accept them".

use serde::Serialize;
use sha2::{Digest, Sha256};
use wcore_agent::session_journal::{
    GENESIS_CHECKSUM, JournalError, SESSION_JOURNAL_SCHEMA_VERSION, SessionEvent, SessionJournal,
    StoredToolInput,
};
use wcore_types::tool::{ToolEffectContract, ToolEffectKind};

const FRAME_MAGIC: &[u8; 4] = b"WJ01";

/// Mirrors the private `ChecksumMaterial` — the envelope minus its trailing
/// `checksum`, in declaration order. Struct field order is what the stored
/// SHA-256 covers, so this has to match exactly.
#[derive(Serialize)]
struct ChecksumMaterial<'a> {
    schema_version: u32,
    session_id: &'a str,
    seq: u64,
    previous_checksum: &'a str,
    event: &'a SessionEvent,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn frame(body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len()).expect("frame length");
    let mut out = Vec::new();
    out.extend_from_slice(FRAME_MAGIC);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&(!length).to_be_bytes());
    out.extend_from_slice(body);
    out.extend_from_slice(&Sha256::digest(body));
    out
}

/// Close a checksum material object into a full envelope body the way
/// `JournalEnvelope::create` does: hash the material, then append `checksum` as
/// the last member. **The hash covers the bytes that end up on disk** — which is
/// the whole point of these fixtures, and what the vacuous tests do not do.
fn seal(material: &[u8]) -> Vec<u8> {
    let checksum = sha256_hex(material);
    let mut body = material[..material.len() - 1].to_vec();
    body.extend_from_slice(format!(",\"checksum\":\"{checksum}\"}}").as_bytes());
    body
}

fn material(session_id: &str, seq: u64, previous: &str, event: &SessionEvent) -> Vec<u8> {
    serde_json::to_vec(&ChecksumMaterial {
        schema_version: SESSION_JOURNAL_SCHEMA_VERSION,
        session_id,
        seq,
        previous_checksum: previous,
        event,
    })
    .expect("encode material")
}

fn stored_checksum(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .expect("body is json")
        .get("checksum")
        .and_then(serde_json::Value::as_str)
        .expect("checksum field")
        .to_owned()
}

/// Splice one member into an encoded event immediately after `anchor`, which is
/// where serde would have written it had the producer emitted it explicitly.
/// The anchor is asserted unique so the insertion cannot land anywhere else.
fn splice_after(bytes: &[u8], anchor: &str, member: &str) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).expect("utf8");
    let at = text
        .find(anchor)
        .unwrap_or_else(|| panic!("anchor {anchor:?} must appear in the encoding: {text}"))
        + anchor.len();
    assert!(
        text[at..].find(anchor).is_none(),
        "anchor {anchor:?} must appear exactly once"
    );
    let mut out = text[..at].to_owned();
    out.push_str(member);
    out.push_str(&text[at..]);
    out.into_bytes()
}

const IDEMPOTENCY_KEY: &str = "\"idempotency_key\":\"k1\"";
const EFFECT_CONTRACT: &str =
    "\"effect_contract\":{\"kind\":\"filesystem_transactional\",\"reconciler\":\"filesystem\"}";

fn turn_started() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: "t1".into(),
        user_message: "write the file".into(),
    }
}

/// The tool intent that carries the blessed fields. The contract names a
/// reconciler because the reducer refuses an effect receipt without one, which
/// is also what makes this event shape reachable in production.
fn tool_intent() -> SessionEvent {
    SessionEvent::ToolIntentRecordedV2 {
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
        effect_receipt: None,
        pre_hook_phase_id: None,
    }
}

/// A two-frame journal: a turn, then the tool intent. `splice` rewrites the
/// SECOND frame's checksum material before it is sealed, so the stored hash
/// covers the spliced bytes — exactly what a producer emitting that encoding
/// stores. The spliced frame is last, so no later frame's `previous_checksum`
/// depends on it and the failure can only be the envelope checksum.
fn journal_bodies(session_id: &str, splice: impl Fn(&[u8]) -> Vec<u8>) -> Vec<Vec<u8>> {
    let first = seal(&material(session_id, 0, GENESIS_CHECKSUM, &turn_started()));
    let previous = stored_checksum(&first);
    let second = seal(&splice(&material(session_id, 1, &previous, &tool_intent())));
    vec![first, second]
}

fn journal_file(bodies: &[Vec<u8>]) -> Vec<u8> {
    bodies.iter().flat_map(|body| frame(body)).collect()
}

fn load_bodies(bodies: &[Vec<u8>]) -> Result<(), JournalError> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("blessed.journal");
    std::fs::write(&path, journal_file(bodies)).expect("write fixture");
    SessionJournal::recovered_state(&path).map(|_| ())
}

fn load(session_id: &str, splice: impl Fn(&[u8]) -> Vec<u8>) -> Result<(), JournalError> {
    load_bodies(&journal_bodies(session_id, splice))
}

/// KNOWN-POSITIVE. The identical construction with nothing spliced must load.
///
/// Without this the negative cases below would pass on a broken fixture builder
/// — a wrong field order, a stale schema version or a bad frame digest all
/// produce a red for reasons that have nothing to do with the defect.
#[test]
fn the_unspliced_fixture_loads() {
    load("s-control", <[u8]>::to_vec).expect("the fixture builder must produce a readable journal");
}

/// THE DEFECT. Each of these encodings is blessed by `known_omitted_default`
/// and pinned as wire-compatible by
/// `known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed`.
/// The producer stored the SHA-256 of the bytes it wrote. The reader must
/// accept them.
#[test]
fn wire_blessed_defaults_are_readable() {
    for (what, anchor, member) in [
        ("retry_of", IDEMPOTENCY_KEY, ",\"retry_of\":null"),
        (
            "pre_hook_phase_id",
            EFFECT_CONTRACT,
            ",\"pre_hook_phase_id\":null",
        ),
        (
            "effect_receipt",
            EFFECT_CONTRACT,
            ",\"effect_receipt\":null",
        ),
    ] {
        let result = load(&format!("s-{what}"), |bytes| {
            splice_after(bytes, anchor, member)
        });
        assert!(
            result.is_ok(),
            "an explicit \"{what}\":null is declared wire-compatible by the \
             reader's own contract, and its producer stored the hash of the \
             bytes it wrote — the journal must be readable, got {:?}",
            result.err()
        );
    }
}

/// THE GUARD, in the opposite direction. One byte of CONTENT changed, with the
/// frame digest recomputed over the altered body so checks (1) and (2) pass and
/// only the envelope checksum can catch it.
///
/// If the fix is ever mistaken for "the frame already digested these bytes,
/// trust them", this test goes green and the integrity check is gone.
#[test]
fn a_tampered_body_is_still_rejected() {
    let mut bodies = journal_bodies("s-tamper", |body| {
        splice_after(body, IDEMPOTENCY_KEY, ",\"retry_of\":null")
    });
    let text = String::from_utf8(bodies[1].clone()).expect("utf8");
    // Same length, so the frame header stays valid; `frame` then recomputes the
    // frame digest over the tampered body, so check (1) passes and the envelope
    // checksum is the only thing left that can reject it.
    let tampered = text.replacen("\"tool\":\"Write\"", "\"tool\":\"Xrite\"", 1);
    assert_ne!(text, tampered, "the tamper must actually apply");
    bodies[1] = tampered.into_bytes();

    match load_bodies(&bodies) {
        Err(JournalError::ChecksumMismatch { seq }) => assert_eq!(seq, 1),
        other => panic!("tampered content must be rejected, got {other:?}"),
    }
}

/// THE THIRD ASSERTION — the shape that already exists would have missed this.
///
/// `journal_envelope_roundtrip.rs` tests
/// `serialize(deserialize(serialize(e))) == serialize(e)` over Rust values. It
/// caught `effect_receipt` because `Option<serde_json::Value>` can hold
/// `Some(Value::Null)`. It is structurally incapable of catching `retry_of`:
/// no value of `Option<String>` serializes to `null`, so the invariant holds
/// for every input it can construct while the on-disk encoding still breaks the
/// journal.
#[test]
fn the_value_level_round_trip_invariant_is_blind_to_this() {
    for retry_of in [None, Some("prior-attempt".to_owned())] {
        let SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id,
            idempotency_key,
            provider_call_id,
            turn_id,
            ordinal,
            tool,
            requested_input,
            requested_input_digest,
            effective_input,
            effective_input_digest,
            effect_contract,
            effect_receipt,
            pre_hook_phase_id,
            ..
        } = tool_intent()
        else {
            unreachable!("tool_intent is a ToolIntentRecordedV2")
        };
        let event = SessionEvent::ToolIntentRecordedV2 {
            tool_execution_id,
            idempotency_key,
            retry_of,
            provider_call_id,
            turn_id,
            ordinal,
            tool,
            requested_input,
            requested_input_digest,
            effective_input,
            effective_input_digest,
            effect_contract,
            effect_receipt,
            pre_hook_phase_id,
        };

        let first = serde_json::to_vec(&event).expect("serialize");
        let decoded: SessionEvent = serde_json::from_slice(&first).expect("decode");
        let second = serde_json::to_vec(&decoded).expect("re-serialize");
        assert_eq!(
            String::from_utf8_lossy(&first),
            String::from_utf8_lossy(&second),
            "the value-level invariant holds here — which is precisely why it \
             cannot see the defect"
        );
        assert!(
            !String::from_utf8_lossy(&first).contains("\"retry_of\":null"),
            "no Option<String> value can produce the on-disk encoding that \
             breaks the journal, so no value-level test can reach it"
        );
    }
}
