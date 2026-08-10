//! B8 — compaction must not be the only copy of the conversation.
//!
//! Autocompact replaces the whole message buffer with one synthetic summary
//! message; `save_session_mirror` then overwrites the session file with that
//! collapsed buffer; the journal reducer *replaces* `state.conversation` on
//! `ConversationStateCommitted`; and `SessionJournal::compact` rewrites the log
//! file down to its anchor. So before this archive existed, a compacted
//! conversation had no surviving copy `--resume` could reach — measured live:
//! after one autocompact, a `--continue` resume sent 2 of 6 read canaries and 1
//! of 6 write canaries upstream, against 6/6 with compaction disabled.
//!
//! These tests cover the store's three load-bearing properties. The end-to-end
//! property — that the engine writes a window at the fold and
//! `--restore-compaction` puts it back on the wire — is graded from recorded
//! provider bodies by the `p20_compaction_resume` durability probe, because it
//! needs a real binary, a real session and a real provider.

use wcore_agent::session::SessionManager;
use wcore_types::message::{ContentBlock, Message, Role};

fn msg(text: &str) -> Message {
    Message::now(
        Role::User,
        vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    )
}

fn text_of(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn an_archived_window_round_trips_verbatim() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);

    let before = vec![msg("READ-CANARY-1"), msg("WRITE-CANARY-1"), msg("frontier")];
    let window = mgr.archive_precompaction("sess", &before, 3).unwrap();
    assert_eq!(window, 1, "the first archived window is numbered 1");

    let loaded = mgr.load_precompaction_windows("sess").unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].window, 1);
    let texts: Vec<String> = loaded[0].messages.iter().map(text_of).collect();
    assert_eq!(
        texts,
        vec![
            "READ-CANARY-1".to_string(),
            "WRITE-CANARY-1".to_string(),
            "frontier".to_string()
        ],
        "the archive must return the pre-compaction buffer verbatim and in order"
    );
}

#[test]
fn windows_accumulate_and_are_bounded_by_the_retention_setting() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);

    // Four compactions with room for two. A very long session must not grow an
    // unbounded archive, and the windows that survive must be the NEWEST ones —
    // dropping the newest would leave the user able to recover only ancient
    // context, which is worse than useless.
    for i in 1..=4 {
        let window = mgr
            .archive_precompaction("sess", &[msg(&format!("window-{i}"))], 2)
            .unwrap();
        assert_eq!(window as usize, i, "window numbering must keep counting up");
    }

    let loaded = mgr.load_precompaction_windows("sess").unwrap();
    assert_eq!(
        loaded.iter().map(|w| w.window).collect::<Vec<_>>(),
        vec![3, 4],
        "retention keeps the newest `keep_windows` windows and drops the rest"
    );
    assert_eq!(text_of(&loaded[1].messages[0]), "window-4");
}

#[test]
fn a_torn_trailing_record_does_not_hide_the_committed_windows() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);
    mgr.archive_precompaction("sess", &[msg("survivor")], 3)
        .unwrap();

    // A crash mid-append leaves bytes with no terminating newline. Recovery is
    // the entire point of this file, so a half-written record must not take the
    // complete ones down with it.
    let path = mgr.precompaction_path("sess");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(br#"{"window":2,"recorded_at":"2026-08-10T00:00"#);
    std::fs::write(&path, &bytes).unwrap();

    let loaded = mgr.load_precompaction_windows("sess").unwrap();
    assert_eq!(loaded.len(), 1, "the committed prefix is still readable");
    assert_eq!(text_of(&loaded[0].messages[0]), "survivor");
}

#[test]
fn corruption_inside_the_committed_prefix_is_reported_not_swallowed() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);
    mgr.archive_precompaction("sess", &[msg("first")], 3)
        .unwrap();
    mgr.archive_precompaction("sess", &[msg("second")], 3)
        .unwrap();

    // Damage the FIRST record. It is newline-terminated, so it is part of the
    // committed prefix and silently dropping it would misreport how much
    // history is recoverable.
    let path = mgr.precompaction_path("sess");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    lines[0] = "{not json";
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let error = mgr
        .load_precompaction_windows("sess")
        .expect_err("corruption inside the committed prefix must surface");
    assert!(
        error.to_string().contains("corrupt pre-compaction archive"),
        "the error must name what is corrupt: {error}"
    );
}

#[test]
fn a_session_that_never_compacted_has_no_archive_and_no_error() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);
    assert!(
        mgr.load_precompaction_windows("never-compacted")
            .unwrap()
            .is_empty(),
        "a missing archive is an empty history, not a failure"
    );
}

#[test]
fn archiving_is_refused_when_retention_is_zero() {
    // `precompact_archive_windows = 0` is the operator opting out. Writing a
    // window anyway would ignore the setting; writing nothing and returning a
    // window number would report an archive that does not exist.
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf(), 50);
    assert!(
        mgr.archive_precompaction("sess", &[msg("dropped")], 0)
            .is_err()
    );
    assert!(!mgr.precompaction_path("sess").exists());
}
