use serde_json::json;
use wcore_agent::session_journal::{
    replay_from_snapshot, replay_state, snapshot_path_for, state_payload_digest, JournalError,
    SessionEvent, SessionJournal, SessionSnapshot, TurnCompletion,
    LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION,
};

// Exact schema-v4 bytes emitted by the predecessor journal/snapshot writer.
// These are intentionally immutable wire fixtures: constructing current types
// and merely relabelling their schema version would not test compatibility.
const PREDECESSOR_V4_JOURNAL: &[u8] = b"WJ01\x00\x00\x01\x1a\xff\xff\xfe\xe5{\x22schema_version\x22:4,\x22session_id\x22:\x22s1\x22,\x22seq\x22:0,\x22previous_checksum\x22:\x220000000000000000000000000000000000000000000000000000000000000000\x22,\x22event\x22:{\x22type\x22:\x22turn_started\x22,\x22turn_id\x22:\x22t0\x22,\x22user_message\x22:\x22legacy\x22},\x22checksum\x22:\x22f2880a11b8c9c187324bd8031958e2ddb2fc52902bf969e850b58311de071241\x22}\xbb\xa3&J\x02\xf9,\xdb\x97\xc4g\xb5\xaa\x9db\x97S\xe3h\x1d0\xcdQT\xf0\xe6\xe4Ps\x13\x14R";
const PREDECESSOR_V4_SNAPSHOT: &[u8] = br#"{"schema_version":4,"session_id":"s1","cursor":0,"cursor_checksum":"f2880a11b8c9c187324bd8031958e2ddb2fc52902bf969e850b58311de071241","state_digest":"b9bbb71a27d71565794c3d03c634395f8709563ad4e9a2e23ca8f1a797dc737e","state":{"session_id":"s1","last_seq":0,"last_checksum":"f2880a11b8c9c187324bd8031958e2ddb2fc52902bf969e850b58311de071241","imported_baseline":null,"conversation":[],"turns":{"t0":{"user_message":"legacy","completion":null}},"streams":{},"provider_attempts":{},"tools":{},"approvals":{},"budgets":{},"budget_event_ids":{},"checkpoints":{},"children":{},"deliveries":{}}}
"#;

fn write_snapshot_fixture(path: impl AsRef<std::path::Path>, snapshot: &SessionSnapshot) {
    write_private_file(path, serde_json::to_vec(snapshot).unwrap());
}

fn write_private_file(path: impl AsRef<std::path::Path>, bytes: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn authority_head_path(journal_path: &std::path::Path) -> std::path::PathBuf {
    let mut name = journal_path.file_name().unwrap().to_os_string();
    name.push(".authority");
    journal_path.with_file_name(name)
}

fn authority_binding(snapshot: &SessionSnapshot) -> serde_json::Value {
    json!({
        "schema_version": 1,
        "snapshot_schema_version": snapshot.schema_version,
        "session_id": snapshot.session_id,
        "cursor": snapshot.cursor,
        "cursor_checksum": snapshot.cursor_checksum,
        "state_digest": snapshot.state_digest,
    })
}

fn write_authority_head(
    journal_path: &std::path::Path,
    accepted: Option<serde_json::Value>,
    pending: Option<serde_json::Value>,
) {
    write_private_file(
        authority_head_path(journal_path),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "accepted": accepted,
            "pending": pending,
        }))
        .unwrap(),
    );
}

fn legacy_snapshot(mut snapshot: SessionSnapshot) -> SessionSnapshot {
    snapshot.schema_version = LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION;
    snapshot
}

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

fn create_stale_snapshot_fixture(path: &std::path::Path) {
    let journal = SessionJournal::open(path, "s1").unwrap();
    journal.append(turn_started("t0", "first")).unwrap();
    let stale_snapshot =
        legacy_snapshot(SessionSnapshot::new("s1", journal.state().unwrap()).unwrap());
    write_snapshot_fixture(snapshot_path_for(path), &stale_snapshot);
    journal
        .append(message_committed("t0", 0, "answer"))
        .unwrap();
    drop(journal);
}

fn create_published_snapshot(path: &std::path::Path) {
    let journal = SessionJournal::open(path, "s1").unwrap();
    journal
        .append(turn_started("t0", "restart fixture"))
        .unwrap();
    journal.publish_snapshot().unwrap();
    drop(journal);
}

#[test]
fn restart_rejects_oversize_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&snapshot_path)
        .unwrap();
    file.set_len(64 * 1024 * 1024 + 1).unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotTooLarge { path: rejected, .. }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotTooLarge { path: rejected, .. }) if rejected == snapshot_path
    ));
}

#[cfg(unix)]
#[test]
fn restart_rejects_symlinked_snapshot() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let target = dir.path().join("snapshot.target");
    std::fs::rename(&snapshot_path, &target).unwrap();
    symlink(&target, &snapshot_path).unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SymbolicLink { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SymbolicLink { path: rejected }) if rejected == snapshot_path
    ));
}

#[cfg(windows)]
#[test]
fn restart_rejects_symlinked_snapshot() {
    use std::os::windows::fs::symlink_file;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let target = dir.path().join("snapshot.target");
    std::fs::rename(&snapshot_path, &target).unwrap();
    symlink_file(&target, &snapshot_path)
        .unwrap_or_else(|error| panic!("Windows symlink fixture is required: {error}"));

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SymbolicLink { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SymbolicLink { path: rejected }) if rejected == snapshot_path
    ));
}

#[test]
fn restart_rejects_hard_linked_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    std::fs::hard_link(&snapshot_path, dir.path().join("snapshot.alias")).unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::MultipleLinks { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::MultipleLinks { path: rejected }) if rejected == snapshot_path
    ));
}

#[cfg(unix)]
#[test]
fn restart_rejects_public_snapshot_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    std::fs::set_permissions(&snapshot_path, std::fs::Permissions::from_mode(0o640)).unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotUnsafePermissions { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotUnsafePermissions { path: rejected }) if rejected == snapshot_path
    ));
}

#[cfg(windows)]
#[test]
fn restart_rejects_public_snapshot_dacl() {
    use std::os::windows::fs::OpenOptionsExt as _;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Security::Authorization::{SetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
    };

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(&snapshot_path).unwrap();
    // SAFETY: file is live with WRITE_DAC. A null DACL deliberately grants
    // unrestricted access so restart must reject this snapshot.
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    assert_eq!(
        result, 0,
        "failed to install hostile null DACL: {result:#x}"
    );
    drop(file);

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotUnsafePermissions { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotUnsafePermissions { path: rejected }) if rejected == snapshot_path
    ));
}

#[cfg(unix)]
#[test]
fn restart_rejects_foreign_snapshot_owner_when_root() {
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // This mutation is deterministic only in the privileged remote harness.
    if unsafe { geteuid() } != 0 {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let c_path = std::ffi::CString::new(snapshot_path.as_os_str().as_bytes()).unwrap();
    // SAFETY: c_path is NUL-terminated; UID 1 differs from root and the group
    // is deliberately left unchanged.
    assert_eq!(unsafe { libc::chown(c_path.as_ptr(), 1, u32::MAX) }, 0);

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotOwnerMismatch { path: rejected }) if rejected == snapshot_path
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotOwnerMismatch { path: rejected }) if rejected == snapshot_path
    ));
}

#[test]
fn restart_rejects_snapshot_path_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_published_snapshot(&path);
    let snapshot_path = snapshot_path_for(&path);
    let accepted = wcore_agent::session_journal::load_snapshot(&snapshot_path).unwrap();
    let mut forged_state = accepted.state;
    forged_state
        .budget_event_ids
        .insert("forged-event".to_owned(), "forged-digest".to_owned());
    let replacement = SessionSnapshot::new("s1", forged_state).unwrap();
    let replacement_path = dir.path().join("replacement.snapshot");
    write_snapshot_fixture(&replacement_path, &replacement);
    std::fs::remove_file(&snapshot_path).unwrap();
    std::fs::rename(&replacement_path, &snapshot_path).unwrap();

    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
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
        let snapshot = legacy_snapshot(
            SessionSnapshot::new("s1", replay_state(&entries[..prefix_len]).unwrap()).unwrap(),
        );
        // Each iteration is a predecessor-v4 enrollment scenario. A prior
        // iteration's accepted v5 sidecar is deliberately outside that fixture.
        let _ = std::fs::remove_file(authority_head_path(&path));
        write_snapshot_fixture(snapshot_path_for(&path), &snapshot);

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

    // Crash before snapshot publication: the original full log is authoritative.
    assert_eq!(SessionJournal::recovered_state(&path).unwrap(), expected);

    // Crash after snapshot publication but before rotation: snapshot and full
    // log overlap, and their cursor/state must agree exactly.
    let snapshot = journal.publish_snapshot().unwrap();
    let full_log = std::fs::read(&path).unwrap();
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
    write_snapshot_fixture(snapshot_path_for(&target_path), &conflicting);
    assert!(matches!(
        SessionJournal::open(&target_path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
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
    write_snapshot_fixture(snapshot_path_for(&short_path), &ahead);
    assert!(matches!(
        SessionJournal::open(&short_path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));

    let wrong_session_path = dir.path().join("wrong-session.journal");
    let wrong_session = SessionJournal::open(&wrong_session_path, "s1").unwrap();
    wrong_session
        .append(turn_started("t0", "session one"))
        .unwrap();
    drop(wrong_session);
    let foreign = SessionSnapshot::new("s2", Default::default()).unwrap();
    write_snapshot_fixture(snapshot_path_for(&wrong_session_path), &foreign);
    assert!(matches!(
        SessionJournal::open(&wrong_session_path, "s1"),
        Err(JournalError::SessionMismatch { .. })
    ));
}

#[test]
fn self_consistent_snapshot_state_substitution_is_not_authoritative() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "original")).unwrap();
    let mut forged = journal.publish_snapshot().unwrap();
    forged
        .state
        .budget_event_ids
        .insert("forged-event".to_owned(), "forged-digest".to_owned());
    forged.state_digest = forged.state.digest().unwrap();
    write_snapshot_fixture(snapshot_path_for(&path), &forged);
    drop(journal);

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
}

#[test]
fn substituted_anchor_and_binding_cannot_authorize_foreign_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let target_path = dir.path().join("target.journal");
    let foreign_path = dir.path().join("foreign.journal");

    let target = SessionJournal::open(&target_path, "s1").unwrap();
    target.append(turn_started("t0", "target")).unwrap();
    target.compact().unwrap();
    drop(target);

    let foreign = SessionJournal::open(&foreign_path, "s1").unwrap();
    foreign.append(turn_started("t0", "foreign")).unwrap();
    foreign.compact().unwrap();
    drop(foreign);

    std::fs::write(&target_path, std::fs::read(&foreign_path).unwrap()).unwrap();
    assert!(matches!(
        SessionJournal::open(&target_path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
}

#[test]
fn torn_authority_publication_does_not_authorize_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "original")).unwrap();
    let journal_prefix = std::fs::read(&path).unwrap();
    journal.publish_snapshot().unwrap();
    drop(journal);

    let bytes = std::fs::read(&path).unwrap();
    let binding = &bytes[journal_prefix.len()..];
    assert_eq!(&binding[..4], b"WSA1");
    let body_len = u32::from_be_bytes(binding[4..8].try_into().unwrap()) as usize;
    let mut cuts = vec![1, 4, 8, 11, 12, 12 + body_len / 2, 12 + body_len];
    cuts.push(binding.len() - 1);
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts.into_iter().filter(|cut| *cut < binding.len()) {
        let mut torn = journal_prefix.clone();
        torn.extend_from_slice(&binding[..cut]);
        std::fs::write(&path, torn).unwrap();
        assert!(matches!(
            SessionJournal::open(&path, "s1"),
            Err(JournalError::SnapshotAuthorityMismatch)
        ));
    }

    let mut corrupt = bytes;
    *corrupt.last_mut().unwrap() ^= 0x80;
    std::fs::write(&path, corrupt).unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::FrameDigestMismatch { .. })
    ));
}

#[test]
fn representable_legacy_snapshot_is_migrated_under_writer_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    std::fs::write(&path, PREDECESSOR_V4_JOURNAL).unwrap();
    write_private_file(snapshot_path_for(&path), PREDECESSOR_V4_SNAPSHOT);

    let legacy = wcore_agent::session_journal::load_snapshot(snapshot_path_for(&path)).unwrap();
    assert_eq!(
        legacy.schema_version,
        LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION
    );
    assert_eq!(
        replay_from_snapshot(&legacy, &SessionJournal::replay(&path).unwrap()[1..]).unwrap(),
        legacy.state
    );

    let reopened = SessionJournal::open(&path, "s1").unwrap();
    drop(reopened);
    let migrated = wcore_agent::session_journal::load_snapshot(snapshot_path_for(&path)).unwrap();
    assert_ne!(
        migrated.schema_version,
        LEGACY_SESSION_SNAPSHOT_SCHEMA_VERSION
    );
    assert_eq!(
        SessionJournal::recovered_state(&path).unwrap(),
        migrated.state
    );
}

#[test]
fn repeated_current_snapshot_publication_is_byte_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "stable")).unwrap();
    let first = journal.publish_snapshot().unwrap();
    let journal_bytes = std::fs::read(&path).unwrap();
    let snapshot_bytes = std::fs::read(snapshot_path_for(&path)).unwrap();

    assert_eq!(journal.publish_snapshot().unwrap(), first);
    assert_eq!(std::fs::read(&path).unwrap(), journal_bytes);
    assert_eq!(
        std::fs::read(snapshot_path_for(&path)).unwrap(),
        snapshot_bytes
    );
    drop(journal);

    let reopened = SessionJournal::open(&path, "s1").unwrap();
    assert_eq!(reopened.publish_snapshot().unwrap(), first);
    assert_eq!(std::fs::read(&path).unwrap(), journal_bytes);
    assert_eq!(
        std::fs::read(snapshot_path_for(&path)).unwrap(),
        snapshot_bytes
    );
}

#[test]
fn older_journal_snapshot_pair_cannot_roll_back_accepted_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let snapshot_path = snapshot_path_for(&path);
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "first")).unwrap();
    journal.publish_snapshot().unwrap();
    let old_journal = std::fs::read(&path).unwrap();
    let old_snapshot = std::fs::read(&snapshot_path).unwrap();

    journal.append(committed("t0", "newer")).unwrap();
    journal.publish_snapshot().unwrap();
    drop(journal);

    std::fs::write(&path, old_journal).unwrap();
    std::fs::write(&snapshot_path, old_snapshot).unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
}

#[test]
fn pending_snapshot_publication_repairs_forward_from_recovered_journal_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "repairable")).unwrap();
    let target = SessionSnapshot::new("s1", journal.state().unwrap()).unwrap();
    drop(journal);

    write_authority_head(&path, None, Some(authority_binding(&target)));
    assert!(matches!(
        SessionJournal::recovered_state(&path),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));

    let reopened = SessionJournal::open(&path, "s1").unwrap();
    assert_eq!(reopened.state().unwrap(), target.state);
    drop(reopened);
    assert_eq!(
        wcore_agent::session_journal::load_snapshot(snapshot_path_for(&path)).unwrap(),
        target
    );
    assert_eq!(
        SessionJournal::recovered_state(&path).unwrap(),
        target.state
    );
    let head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(authority_head_path(&path)).unwrap()).unwrap();
    assert_eq!(head["pending"], serde_json::Value::Null);
    assert_eq!(head["accepted"], authority_binding(&target));
}

#[test]
fn pending_snapshot_publication_never_clears_when_target_is_not_reconstructible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let snapshot_path = snapshot_path_for(&path);
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "accepted")).unwrap();
    let accepted = journal.publish_snapshot().unwrap();
    let accepted_journal = std::fs::read(&path).unwrap();
    let accepted_snapshot = std::fs::read(&snapshot_path).unwrap();

    journal.append(committed("t0", "pending")).unwrap();
    let pending = SessionSnapshot::new("s1", journal.state().unwrap()).unwrap();
    drop(journal);
    write_authority_head(
        &path,
        Some(authority_binding(&accepted)),
        Some(authority_binding(&pending)),
    );
    std::fs::write(&path, accepted_journal).unwrap();
    std::fs::write(&snapshot_path, accepted_snapshot).unwrap();

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotAuthorityMismatch)
    ));
    let head: serde_json::Value =
        serde_json::from_slice(&std::fs::read(authority_head_path(&path)).unwrap()).unwrap();
    assert_eq!(head["pending"], authority_binding(&pending));
}

#[test]
fn compacted_legacy_snapshot_substitution_cannot_be_laundered_into_current_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0", "original")).unwrap();
    let mut substituted = legacy_snapshot(journal.compact().unwrap());
    substituted
        .state
        .budget_event_ids
        .insert("forged-event".to_owned(), "forged-digest".to_owned());
    substituted.state_digest = substituted.state.digest().unwrap();
    write_snapshot_fixture(snapshot_path_for(&path), &substituted);
    drop(journal);

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(message))
            if message == "snapshot state does not equal its full-log prefix"
    ));
}

#[test]
fn genesis_legacy_snapshot_substitution_cannot_mint_current_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    let mut state = wcore_agent::session_journal::ReducedSessionState {
        session_id: Some("s1".to_owned()),
        ..Default::default()
    };
    state
        .budget_event_ids
        .insert("forged-event".to_owned(), "forged-digest".to_owned());
    let forged = legacy_snapshot(SessionSnapshot::new("s1", state).unwrap());
    std::fs::write(&path, []).unwrap();
    write_snapshot_fixture(snapshot_path_for(&path), &forged);

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
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

#[test]
fn committed_snapshot_with_missing_journal_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_stale_snapshot_fixture(&path);
    std::fs::remove_file(&path).unwrap();

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
    ));
}

#[test]
fn committed_snapshot_with_empty_journal_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_stale_snapshot_fixture(&path);
    std::fs::write(&path, []).unwrap();

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
    ));
}

#[test]
fn committed_snapshot_with_torn_journal_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1.journal");
    create_stale_snapshot_fixture(&path);
    std::fs::write(&path, b"WJ01").unwrap();

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SnapshotJournalMismatch(_))
    ));
}
