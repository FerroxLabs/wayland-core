use serde_json::json;
use wcore_agent::session_journal::{
    JournalError, SessionEvent, SessionJournal, SessionSnapshot, TurnCompletion, replay_state,
    snapshot_path_for, state_payload_digest, write_snapshot,
};

fn turn_started(turn_id: &str, user_message: &str) -> SessionEvent {
    SessionEvent::TurnStarted {
        turn_id: turn_id.to_owned(),
        user_message: user_message.to_owned(),
    }
}

fn message_committed(turn_id: &str, message_index: u64, text: &str) -> SessionEvent {
    let message = json!({
        "role": "assistant",
        "content": [{"type": "text", "text": text}]
    });
    SessionEvent::ConversationMessageCommitted {
        turn_id: turn_id.to_owned(),
        message_index,
        message_digest: state_payload_digest(&message).unwrap(),
        message,
    }
}

fn committed(turn_id: &str, text: &str) -> SessionEvent {
    SessionEvent::TurnCommitted {
        turn_id: turn_id.to_owned(),
        assistant_message: text.to_owned(),
    }
}

fn canonical_events() -> Vec<SessionEvent> {
    vec![
        turn_started("t0", "first"),
        message_committed("t0", 0, "answer"),
        committed("t0", "answer"),
        turn_started("t1", "second"),
        SessionEvent::TurnCancelled {
            turn_id: "t1".to_owned(),
        },
    ]
}

#[test]
fn validated_snapshot_at_every_cursor_replays_the_same_committed_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    for event in canonical_events() {
        journal.append(event).unwrap();
    }
    let entries = SessionJournal::replay(&path).unwrap();
    let expected = journal.state().unwrap();
    drop(journal);

    for prefix_len in 0..=entries.len() {
        let snapshot =
            SessionSnapshot::new("s1", replay_state(&entries[..prefix_len]).unwrap()).unwrap();
        write_snapshot(snapshot_path_for(&path), &snapshot).unwrap();

        assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);
        let reopened = SessionJournal::open(&path, "s1").unwrap();
        assert_eq!(reopened.state().unwrap(), expected);
        drop(reopened);
    }
}

#[test]
fn crash_phases_select_snapshot_plus_full_log_or_snapshot_plus_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    for event in canonical_events() {
        journal.append(event).unwrap();
    }
    let expected = journal.state().unwrap();
    let full_log = std::fs::read(&path).unwrap();

    // Crash before snapshot publication: the original full log is authoritative.
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);

    // Crash after snapshot publication but before rotation: snapshot and full
    // log overlap, and their cursor/state must agree exactly.
    let snapshot = SessionSnapshot::new("s1", expected.clone()).unwrap();
    write_snapshot(snapshot_path_for(&path), &snapshot).unwrap();
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);

    // Crash after atomic log rotation: the snapshot plus retained anchor is
    // authoritative. Preserve both possible disk images to exercise selection.
    journal.compact().unwrap();
    let anchor_log = std::fs::read(&path).unwrap();
    assert!(anchor_log.len() < full_log.len());
    drop(journal);

    std::fs::write(&path, &full_log).unwrap();
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);

    std::fs::write(&path, &anchor_log).unwrap();
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);
    let physical = SessionJournal::replay(&path).unwrap();
    assert_eq!(
        physical.len(),
        1,
        "a compacted log retains one authority anchor"
    );
    assert_eq!(physical[0].seq, snapshot.cursor.unwrap());
}

#[test]
fn compacted_suffix_appends_and_reopens_without_duplicate_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "first")).unwrap();
    journal
        .append(message_committed("t0", 0, "answer"))
        .unwrap();

    let snapshot = journal.compact().unwrap();
    assert_eq!(snapshot.cursor, Some(1));
    let terminal = journal.append(committed("t0", "answer")).unwrap();
    assert_eq!(terminal.seq, 2);
    let expected = journal.state().unwrap();
    drop(journal);

    let reopened = SessionJournal::open(&path, "s1").unwrap();
    assert_eq!(reopened.state().unwrap(), expected);
    assert!(matches!(
        reopened.state().unwrap().turns["t0"].completion,
        Some(TurnCompletion::Committed { ref assistant_message }) if assistant_message == "answer"
    ));
    let next = reopened.append(turn_started("t1", "next")).unwrap();
    assert_eq!(next.seq, 3);
    drop(reopened);

    let physical = SessionJournal::replay(&path).unwrap();
    assert_eq!(
        physical.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[test]
fn snapshot_log_mismatch_and_snapshot_corruption_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.journal");
    let other_path = dir.path().join("other.journal");

    let target = SessionJournal::open(&target_path, "s1").unwrap();
    target.append(turn_started("t0", "target history")).unwrap();
    drop(target);

    let other = SessionJournal::open(&other_path, "s1").unwrap();
    other
        .append(turn_started("t0", "different history"))
        .unwrap();
    let conflicting = SessionSnapshot::new("s1", other.state().unwrap()).unwrap();
    drop(other);
    write_snapshot(snapshot_path_for(&target_path), &conflicting).unwrap();
    assert!(matches!(
        SessionJournal::open(&target_path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
    ));

    std::fs::write(snapshot_path_for(&target_path), b"{not valid json").unwrap();
    assert!(matches!(
        SessionJournal::open(&target_path, "s1"),
        Err(JournalError::Json { .. })
    ));

    let short_path = dir.path().join("short.journal");
    let short = SessionJournal::open(&short_path, "s1").unwrap();
    short.append(turn_started("t0", "short")).unwrap();
    drop(short);
    let long_path = dir.path().join("long.journal");
    let long = SessionJournal::open(&long_path, "s1").unwrap();
    long.append(turn_started("t0", "short")).unwrap();
    long.append(committed("t0", "done")).unwrap();
    let ahead = SessionSnapshot::new("s1", long.state().unwrap()).unwrap();
    drop(long);
    write_snapshot(snapshot_path_for(&short_path), &ahead).unwrap();
    assert!(matches!(
        SessionJournal::open(&short_path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
    ));

    let wrong_session_path = dir.path().join("wrong-session.journal");
    let wrong_session = SessionJournal::open(&wrong_session_path, "s1").unwrap();
    wrong_session
        .append(turn_started("t0", "session one"))
        .unwrap();
    drop(wrong_session);
    let foreign = SessionSnapshot::new("s2", Default::default()).unwrap();
    write_snapshot(snapshot_path_for(&wrong_session_path), &foreign).unwrap();
    assert!(matches!(
        SessionJournal::open(&wrong_session_path, "s1"),
        Err(JournalError::SessionMismatch { .. })
    ));
}

#[test]
fn compacted_non_genesis_log_without_snapshot_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "first")).unwrap();
    journal
        .append(message_committed("t0", 0, "answer"))
        .unwrap();
    journal.compact().unwrap();
    drop(journal);

    std::fs::remove_file(snapshot_path_for(&path)).unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::CompactedJournalMissingSnapshot { first_seq: 1 })
    ));
}
