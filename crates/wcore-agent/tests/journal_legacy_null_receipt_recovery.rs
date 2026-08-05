//! 23B-H1 residual — journals and snapshots ALREADY on disk in the pre-fix
//! encoding must become readable again, with their content intact.
//!
//! # What this file is for
//!
//! 23B-01 raised a HIGH: a run that exits normally can write a session journal
//! the product cannot read back (`journal checksum mismatch at sequence N`).
//! 23B-H1 root-caused it — `effect_receipt` is
//! `#[serde(skip_serializing_if = ...)] Option<serde_json::Value>`, so
//! `Some(Value::Null)` WROTE `"effect_receipt":null`, serde DECODED that back to
//! `None`, and re-serialization then OMITTED the field. The recomputed hash
//! covers different bytes than the stored one, and the reader rejects a journal
//! the writer wrote correctly, permanently, for every operator verb.
//!
//! That was fixed on the WRITE path only. Its disposition named the residual
//! plainly: "a journal ALREADY on disk carrying an explicit
//! `\"effect_receipt\":null` still fails its checksum on read … those sessions
//! remain unreadable and their content is lost." This file closes that residual
//! and pins the boundary it must not cross.
//!
//! # Why these fixtures are byte-crafted rather than written by the product
//!
//! The current writer cannot produce the defective bytes any more — that is
//! what the write-path fix did. So the only way to test the recovery is to
//! reproduce, byte for byte, what the PRE-FIX writer emitted: the field
//! serialized in its declared position, and the SHA-256 the writer stored
//! computed over exactly those bytes. `legacy_journal_bytes` and
//! `legacy_snapshot_bytes` below do that and nothing else.
//!
//! # These tests can fail
//!
//! Comment out the `recover_legacy_effect_receipt` call in
//! `session_journal.rs::parse_complete_frames` and
//! `journal_recovers_a_pre_fix_null_receipt` fails with
//! `ChecksumMismatch { seq: 1 }`; comment out `recover_legacy_effect_receipts`
//! in `snapshot.rs::load_snapshot` and `snapshot_recovers_a_pre_fix_null_receipt`
//! fails with `SnapshotDigestMismatch`. The two `_still_rejected` tests fail if
//! the recovery is ever loosened into "trust the bytes on disk".

use serde::Serialize;
use sha2::{Digest, Sha256};
use wcore_agent::session_journal::{
    GENESIS_CHECKSUM, JournalError, SESSION_JOURNAL_SCHEMA_VERSION, SessionEvent, SessionJournal,
    StoredToolInput, load_snapshot, write_private_snapshot_fixture,
};
use wcore_types::tool::{ToolEffectContract, ToolEffectKind};

const FRAME_MAGIC: &[u8; 4] = b"WJ01";

/// Mirrors the private `ChecksumMaterial` — the envelope minus its trailing
/// `checksum` field, in declaration order. Struct field order is what the
/// stored SHA-256 covers, so this has to match exactly.
#[derive(Serialize)]
struct ChecksumMaterial<'a> {
    schema_version: u32,
    session_id: &'a str,
    seq: u64,
    previous_checksum: &'a str,
    event: &'a SessionEvent,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
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

/// One envelope, encoded the way the CURRENT writer encodes it.
fn envelope_bytes(session_id: &str, seq: u64, previous: &str, event: &SessionEvent) -> Vec<u8> {
    let material = serde_json::to_vec(&ChecksumMaterial {
        schema_version: SESSION_JOURNAL_SCHEMA_VERSION,
        session_id,
        seq,
        previous_checksum: previous,
        event,
    })
    .expect("encode material");
    seal(&material)
}

/// Close a checksum material object into a full envelope body, the way
/// `JournalEnvelope::create` does: hash the material, then append the
/// `checksum` field as the last member.
fn seal(material: &[u8]) -> Vec<u8> {
    let checksum = sha256_hex(material);
    let mut body = material[..material.len() - 1].to_vec();
    body.extend_from_slice(format!(",\"checksum\":\"{checksum}\"}}").as_bytes());
    body
}

/// The declared position of `effect_receipt`: immediately after
/// `effect_contract`, in both `SessionEvent::ToolIntentRecordedV2` and
/// `ToolState`. The splice anchors on the whole member so it cannot land
/// anywhere else.
const EFFECT_CONTRACT: &str =
    "\"effect_contract\":{\"kind\":\"filesystem_transactional\",\"reconciler\":\"filesystem\"}";

/// The tool intent that carries the hazardous shape. The contract names a
/// reconciler because the reducer refuses an effect receipt without one — which
/// is also what makes this shape reachable in production.
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

fn turn_started() -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: "t1".into(),
        user_message: "write the file".into(),
    }
}

/// The two envelope BODIES of a journal exactly as the PRE-FIX writer left it.
/// The second carries an explicit `"effect_receipt":null`, with its stored
/// checksum computed over those bytes.
fn legacy_journal_bodies(session_id: &str) -> Vec<Vec<u8>> {
    let first = envelope_bytes(session_id, 0, GENESIS_CHECKSUM, &turn_started());
    let first_checksum = stored_checksum(&first);

    let canonical = serde_json::to_vec(&ChecksumMaterial {
        schema_version: SESSION_JOURNAL_SCHEMA_VERSION,
        session_id,
        seq: 1,
        previous_checksum: &first_checksum,
        event: &tool_intent(),
    })
    .expect("encode material");
    let legacy = splice_null_receipt(&canonical);
    assert_ne!(
        canonical, legacy,
        "the legacy fixture must actually differ from the canonical encoding"
    );

    vec![first, seal(&legacy)]
}

fn journal_file(bodies: &[Vec<u8>]) -> Vec<u8> {
    bodies.iter().flat_map(|body| frame(body)).collect()
}

/// Insert `"effect_receipt":null` in its declared position — immediately after
/// the `effect_contract` member. This is what the pre-fix
/// `skip_serializing_if = "Option::is_none"` emitted for `Some(Value::Null)`.
fn splice_null_receipt(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).expect("utf8");
    let at = text
        .find(EFFECT_CONTRACT)
        .unwrap_or_else(|| panic!("effect_contract must appear in the encoding: {text}"))
        + EFFECT_CONTRACT.len();
    assert!(
        text[at..].find(EFFECT_CONTRACT).is_none(),
        "effect_contract must appear exactly once"
    );
    let mut out = text[..at].to_owned();
    out.push_str(",\"effect_receipt\":null");
    out.push_str(&text[at..]);
    out.into_bytes()
}

fn stored_checksum(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .expect("body is json")
        .get("checksum")
        .and_then(serde_json::Value::as_str)
        .expect("checksum field")
        .to_owned()
}

#[test]
fn journal_recovers_a_pre_fix_null_receipt_with_its_content_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy.journal");
    std::fs::write(&path, journal_file(&legacy_journal_bodies("s-legacy"))).expect("write fixture");

    let state = SessionJournal::recovered_state(&path)
        .expect("a journal written before the 23B-H1 fix must still be readable");

    // Content, not merely "it opened".
    assert!(state.turns.contains_key("t1"), "the turn must survive");
    let tool = state.tools.get("x1").expect("the tool intent must survive");
    assert_eq!(tool.tool, "Write");
    assert_eq!(tool.idempotency_key, "k1");
    assert_eq!(tool.provider_call_id, "c1");
    assert_eq!(tool.requested_input_digest, "a".repeat(64));
    assert_eq!(tool.effective_input_digest, "b".repeat(64));
    assert_eq!(
        tool.effect_contract.reconciler.as_deref(),
        Some("filesystem")
    );
    // The recovered value is the one the writer actually held, not a
    // normalisation of it: replay reproduces the writer's state exactly.
    assert_eq!(tool.effect_receipt, Some(serde_json::Value::Null));
}

#[test]
fn a_genuinely_corrupt_journal_is_still_rejected() {
    // The same legacy fixture with one byte of CONTENT changed and the frame
    // digest recomputed over the altered bytes, so checks (1) and (2) pass and
    // only the envelope checksum can catch it. If the recovery were ever
    // relaxed into "the frame digest already proved these bytes, accept them",
    // this test goes green and the integrity check is gone.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("corrupt.journal");

    let mut bodies = legacy_journal_bodies("s-corrupt");
    let text = String::from_utf8(bodies[1].clone()).expect("utf8");
    let tampered = text.replacen("\"tool\":\"Write\"", "\"tool\":\"Xrite\"", 1);
    assert_ne!(text, tampered, "the tamper must actually apply");
    // Same length, so the frame header stays valid; `frame` recomputes the
    // frame digest over the tampered body, so check (1) passes.
    bodies[1] = tampered.into_bytes();
    std::fs::write(&path, journal_file(&bodies)).expect("write fixture");

    match SessionJournal::recovered_state(&path) {
        Err(JournalError::ChecksumMismatch { seq }) => assert_eq!(seq, 1),
        other => panic!("tampered content must be rejected, got {other:?}"),
    }
}

#[test]
fn snapshot_recovers_a_pre_fix_null_receipt_with_its_content_intact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal_path = dir.path().join("live.journal");

    // Build a real snapshot through the real writer, then rewrite it into the
    // pre-fix encoding.
    let journal = SessionJournal::open(&journal_path, "s-snap").expect("open journal");
    journal.append(turn_started()).expect("turn must start");
    journal.append(tool_intent()).expect("intent must record");
    let published = journal.publish_snapshot().expect("publish snapshot");
    assert_eq!(
        published.state.tools["x1"].effect_receipt, None,
        "the fixed writer must not emit a receipt here"
    );
    drop(journal);

    let path = dir.path().join("legacy.snapshot");
    write_private_snapshot_fixture(&path, &legacy_snapshot_bytes(&published))
        .expect("write fixture");

    let loaded = load_snapshot(&path)
        .expect("a snapshot written before the 23B-H1 fix must still be readable");
    let tool = loaded.state.tools.get("x1").expect("tool state survives");
    assert_eq!(tool.tool, "Write");
    assert_eq!(tool.requested_input_digest, "a".repeat(64));
    assert_eq!(tool.effect_receipt, Some(serde_json::Value::Null));
    assert_eq!(loaded.state.turns.len(), 1);
}

#[test]
fn a_genuinely_corrupt_snapshot_is_still_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let journal_path = dir.path().join("live2.journal");
    let journal = SessionJournal::open(&journal_path, "s-snap2").expect("open journal");
    journal.append(turn_started()).expect("turn must start");
    journal.append(tool_intent()).expect("intent must record");
    let published = journal.publish_snapshot().expect("publish snapshot");
    drop(journal);

    let bytes = legacy_snapshot_bytes(&published);
    let tampered = String::from_utf8(bytes).expect("utf8").replacen(
        "\"tool\":\"Write\"",
        "\"tool\":\"Xrite\"",
        1,
    );
    let path = dir.path().join("corrupt.snapshot");
    write_private_snapshot_fixture(&path, tampered.as_bytes()).expect("write fixture");

    assert!(
        matches!(
            load_snapshot(&path),
            Err(JournalError::SnapshotDigestMismatch)
        ),
        "tampered snapshot content must be rejected"
    );
}

/// A snapshot exactly as the PRE-FIX writer left it: the tool's
/// `effect_receipt` present as an explicit null, and `state_digest` recomputed
/// over the state bytes that contain it.
fn legacy_snapshot_bytes(published: &wcore_agent::session_journal::SessionSnapshot) -> Vec<u8> {
    let canonical = serde_json::to_vec(published).expect("encode snapshot");
    let text = String::from_utf8(canonical).expect("utf8");
    let state_at = text.find("\"state\":{").expect("state field") + "\"state\":".len();
    let (head, state) = text.split_at(state_at);
    let state = state
        .strip_suffix('}')
        .expect("state is the last member of the snapshot object");

    let legacy_state = String::from_utf8(splice_null_receipt(state.as_bytes())).expect("utf8");
    assert_ne!(
        state, legacy_state,
        "the fixture must differ from canonical"
    );

    let head = head.replacen(
        &published.state_digest,
        &sha256_hex(legacy_state.as_bytes()),
        1,
    );
    format!("{head}{legacy_state}}}").into_bytes()
}
