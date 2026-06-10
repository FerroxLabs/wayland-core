//! SQLite access helpers for `~/Library/Messages/chat.db`.
//!
//! Uses `tokio::task::spawn_blocking` so the synchronous SQLite calls
//! don't block the async runtime.

use std::path::PathBuf;

use crate::error::IMessageError;

/// Returned for each new inbound message row.
#[derive(Debug, Clone)]
pub struct ChatDbRow {
    pub rowid: i64,
    pub text: String,
    pub sender_handle: String,
    pub chat_guid: String,
    #[allow(dead_code)] // reserved for future group-message routing
    pub is_group: bool,
    pub ts_apple_ns: i64,
}

/// Path to the default chat.db for the current user.
pub fn chat_db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Messages")
        .join("chat.db")
}

/// Fetch new inbound message rows with `rowid > since_rowid`.
///
/// Runs on a blocking thread. Returns an error if the DB cannot be opened
/// (most commonly: Full Disk Access not granted).
pub async fn fetch_new_messages(
    db_path: PathBuf,
    since_rowid: i64,
) -> Result<Vec<ChatDbRow>, IMessageError> {
    tokio::task::spawn_blocking(move || fetch_new_messages_blocking(&db_path, since_rowid))
        .await
        .map_err(|e| IMessageError::Database(format!("spawn_blocking panic: {e}")))?
}

/// Read the current max rowid from message table (seed the cursor on start).
pub async fn max_rowid(db_path: PathBuf) -> Result<i64, IMessageError> {
    tokio::task::spawn_blocking(move || max_rowid_blocking(&db_path))
        .await
        .map_err(|e| IMessageError::Database(format!("spawn_blocking panic: {e}")))?
}

/// An outgoing (`is_from_me = 1`) message row, used to resolve the real
/// `message.guid` that AppleScript's `send` does not return synchronously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingRow {
    pub rowid: i64,
    pub guid: String,
    pub text: String,
}

/// Fetch outgoing message rows newer than `since_rowid`. Runs on a blocking
/// thread. Used to resolve a just-sent message's GUID after an AppleScript send.
pub async fn fetch_outgoing_since(
    db_path: PathBuf,
    since_rowid: i64,
    chat_id: String,
) -> Result<Vec<OutgoingRow>, IMessageError> {
    tokio::task::spawn_blocking(move || {
        fetch_outgoing_since_blocking(&db_path, since_rowid, &chat_id)
    })
    .await
    .map_err(|e| IMessageError::Database(format!("spawn_blocking panic: {e}")))?
}

// ---------------------------------------------------------------------------
// Blocking implementations
// ---------------------------------------------------------------------------

const SQL_NEW_MESSAGES: &str = "
  SELECT
    m.rowid           AS rowid,
    COALESCE(m.text, '') AS text,
    COALESCE(h.id, '')   AS sender_handle,
    COALESCE(c.guid, '') AS chat_guid,
    CASE WHEN c.style = 43 OR c.chat_identifier LIKE 'chat%' THEN 1 ELSE 0 END AS is_group,
    m.date            AS ts_apple_ns
  FROM message m
  LEFT JOIN handle h ON h.rowid = m.handle_id
  LEFT JOIN chat_message_join cmj ON cmj.message_id = m.rowid
  LEFT JOIN chat c ON c.rowid = cmj.chat_id
  WHERE m.rowid > ?1
    AND m.is_from_me = 0
    AND m.handle_id != 0
    AND COALESCE(m.text, '') != ''
  ORDER BY m.rowid ASC
";

// Outgoing messages in a specific CHAT, newer than the cursor. Scoping to
// the chat (via the same chat_message_join → chat join the inbound query
// uses) keeps a concurrent send to a DIFFERENT conversation from being
// mis-matched as our just-sent message — without it, a same-text send
// elsewhere could steal this send's GUID and corrupt receipt correlation.
// `?2` matches either the chat guid or the chat_identifier (the
// conversation_id can be in either form). `is_from_me = 1` restricts to
// messages we sent. Ordered DESC so the newest (most likely ours) wins.
const SQL_OUTGOING_SINCE: &str = "
  SELECT
    m.rowid           AS rowid,
    COALESCE(m.guid, '') AS guid,
    COALESCE(m.text, '') AS text
  FROM message m
  LEFT JOIN chat_message_join cmj ON cmj.message_id = m.rowid
  LEFT JOIN chat c ON c.rowid = cmj.chat_id
  WHERE m.rowid > ?1
    AND m.is_from_me = 1
    AND (c.guid = ?2 OR c.chat_identifier = ?2)
    AND COALESCE(m.text, '') != ''
  ORDER BY m.rowid DESC
";

/// Pure matcher: from a set of outgoing rows (already filtered to `rowid >
/// cursor`), pick the GUID of the row whose text equals `sent_text`. Returns
/// the highest-rowid match so the most recent send wins when a recipient was
/// sent the same text twice. Returns `None` when no row matches — the caller
/// then falls back to a synthetic pending id.
pub fn match_outgoing_guid(rows: &[OutgoingRow], sent_text: &str) -> Option<String> {
    rows.iter()
        .filter(|r| r.text == sent_text && !r.guid.is_empty())
        .max_by_key(|r| r.rowid)
        .map(|r| r.guid.clone())
}

fn fetch_outgoing_since_blocking(
    db_path: &std::path::Path,
    since_rowid: i64,
    chat_id: &str,
) -> Result<Vec<OutgoingRow>, IMessageError> {
    use rusqlite::{params, Connection, OpenFlags};

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| IMessageError::Database(format!("open chat.db: {e}")))?;

    let mut stmt = conn
        .prepare(SQL_OUTGOING_SINCE)
        .map_err(|e| IMessageError::Database(format!("prepare: {e}")))?;

    let rows = stmt
        .query_map(params![since_rowid, chat_id], |row| {
            Ok(OutgoingRow {
                rowid: row.get(0)?,
                guid: row.get(1)?,
                text: row.get(2)?,
            })
        })
        .map_err(|e| IMessageError::Database(format!("query: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| IMessageError::Database(format!("row: {e}")))?;

    Ok(rows)
}

fn fetch_new_messages_blocking(
    db_path: &std::path::Path,
    since_rowid: i64,
) -> Result<Vec<ChatDbRow>, IMessageError> {
    use rusqlite::{Connection, OpenFlags};

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| IMessageError::Database(format!("open chat.db: {e}")))?;

    let mut stmt = conn
        .prepare(SQL_NEW_MESSAGES)
        .map_err(|e| IMessageError::Database(format!("prepare: {e}")))?;

    let rows = stmt
        .query_map([since_rowid], |row| {
            Ok(ChatDbRow {
                rowid: row.get(0)?,
                text: row.get(1)?,
                sender_handle: row.get(2)?,
                chat_guid: row.get(3)?,
                is_group: row.get::<_, i32>(4)? != 0,
                ts_apple_ns: row.get(5)?,
            })
        })
        .map_err(|e| IMessageError::Database(format!("query: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| IMessageError::Database(format!("row: {e}")))?;

    Ok(rows)
}

fn max_rowid_blocking(db_path: &std::path::Path) -> Result<i64, IMessageError> {
    use rusqlite::{Connection, OpenFlags};

    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| IMessageError::Database(format!("open chat.db: {e}")))?;

    let max: Option<i64> = conn
        .query_row("SELECT MAX(rowid) FROM message", [], |r| r.get(0))
        .map_err(|e| IMessageError::Database(format!("max rowid: {e}")))?;

    Ok(max.unwrap_or(0))
}

/// Convert Apple's CoreData epoch (ns since 2001-01-01) to Unix epoch seconds.
pub fn apple_ns_to_unix_secs(apple_ns: i64) -> i64 {
    // Apple epoch offset: 2001-01-01 00:00:00 UTC = 978307200 Unix seconds.
    let apple_secs = apple_ns / 1_000_000_000;
    apple_secs + 978_307_200
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rowid: i64, guid: &str, text: &str) -> OutgoingRow {
        OutgoingRow {
            rowid,
            guid: guid.into(),
            text: text.into(),
        }
    }

    #[test]
    fn match_outgoing_guid_picks_matching_text() {
        let rows = vec![
            row(10, "GUID-A", "hello there"),
            row(11, "GUID-B", "different message"),
        ];
        assert_eq!(
            match_outgoing_guid(&rows, "hello there"),
            Some("GUID-A".to_string())
        );
    }

    #[test]
    fn match_outgoing_guid_prefers_highest_rowid_on_duplicate_text() {
        let rows = vec![
            row(10, "GUID-OLD", "ping"),
            row(20, "GUID-NEW", "ping"),
            row(15, "GUID-MID", "ping"),
        ];
        assert_eq!(
            match_outgoing_guid(&rows, "ping"),
            Some("GUID-NEW".to_string())
        );
    }

    #[test]
    fn match_outgoing_guid_none_when_no_text_match() {
        let rows = vec![row(10, "GUID-A", "hello")];
        assert_eq!(match_outgoing_guid(&rows, "goodbye"), None);
    }

    #[test]
    fn match_outgoing_guid_skips_empty_guid() {
        // A row whose text matches but whose guid is empty must not be returned;
        // an empty guid is useless for cross-event correlation.
        let rows = vec![row(10, "", "hello"), row(9, "GUID-REAL", "hello")];
        assert_eq!(
            match_outgoing_guid(&rows, "hello"),
            Some("GUID-REAL".to_string())
        );
    }

    #[test]
    fn match_outgoing_guid_empty_rows_is_none() {
        assert_eq!(match_outgoing_guid(&[], "anything"), None);
    }
}
