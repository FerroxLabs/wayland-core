use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use wcore_types::message::{Message, TokenUsage};

use crate::session_journal::{
    JournalError, SessionEvent, SessionJournal, SessionSnapshot, SessionStorageLease,
    state_payload_digest, write_snapshot,
};

/// Current on-disk schema version. Increment when adding required fields.
/// Readers must accept any version ≤ CURRENT and refuse versions > CURRENT.
pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// A single saved agent session.
///
/// **Schema versioning (F-031):** Every field added after v1 MUST carry
/// `#[serde(default)]` so that older sessions without the field still
/// deserialise.  The migration ladder in `Session::migrate` handles
/// structural mutations (field renames, type changes).
///
/// **Forward-compat (F-032):** Unknown fields from newer schema versions are
/// preserved in `extra` via `#[serde(flatten)]` so that a round-trip through
/// an older binary does not silently delete data written by a newer one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Monotonically-increasing schema version.  Absent in pre-v1 sessions;
    /// `serde(default)` causes those to deserialise as 0 so the migration
    /// ladder can run.
    #[serde(default)]
    pub schema_version: u32,

    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    /// LLM provider label (e.g. "anthropic", "openai").
    /// `serde(default)` keeps pre-v1 sessions (which lacked this field)
    /// loadable; the migration ladder fills a sensible default.
    #[serde(default)]
    pub provider: String,

    /// Model identifier (e.g. "claude-opus-4-5").
    #[serde(default)]
    pub model: String,

    /// Working directory at the time the session was created.
    #[serde(default)]
    pub cwd: String,

    #[serde(default)]
    pub total_usage: TokenUsage,

    #[serde(default)]
    pub messages: Vec<Message>,

    /// Overflow bucket for unknown fields from newer schema versions (F-032).
    /// Preserved verbatim on save so a round-trip through an older binary
    /// does not lose data written by a newer one.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Session {
    /// Run the migration ladder.  Called by `SessionManager::load` after
    /// deserialising the raw JSON.
    ///
    /// Returns `true` when the session was migrated (so the caller can
    /// re-save it, stamping the new `schema_version` on disk).
    fn migrate(&mut self) -> bool {
        let original_version = self.schema_version;
        // v0 → v1: stamp schema_version; fill empty provider/model/cwd with
        // sensible defaults so callers never see the empty string as a silent
        // failure.
        if self.schema_version == 0 {
            self.schema_version = 1;
            if self.provider.is_empty() {
                self.provider = "unknown".to_string();
            }
            if self.model.is_empty() {
                self.model = "unknown".to_string();
            }
        }
        self.schema_version > original_version
    }
}

/// A session paired with its exclusive full-lifetime journal authority.
///
/// Production execution paths must keep this value intact until ownership is
/// transferred into `AgentEngine`. Dropping it releases the writer lease.
#[derive(Debug)]
pub struct ActiveSession {
    pub session: Session,
    pub journal: SessionJournal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndex {
    pub sessions: Vec<SessionMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    /// First user message, truncated to 80 chars
    pub summary: String,
    pub message_count: usize,
}

pub struct SessionManager {
    directory: PathBuf,
    max_sessions: usize,
}

impl SessionManager {
    pub fn new(directory: PathBuf, max_sessions: usize) -> Self {
        Self {
            directory,
            max_sessions,
        }
    }

    /// Create a new session and return it.
    ///
    /// **F-034**: the session file is NOT written to disk here; writing is
    /// deferred until `persist_first_message` is called with the first user
    /// message.  This prevents phantom empty-session rows in the index when
    /// the process exits before the user sends anything.
    pub fn create(
        &self,
        provider: &str,
        model: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<Session> {
        // F-084: validate custom session IDs
        let id = if let Some(custom_id) = session_id {
            validate_session_id(custom_id)?;
            // Check for duplicate IDs now (before we defer the write).
            if self.directory.join("index.json").exists() {
                let index = self.load_index()?;
                if index.sessions.iter().any(|s| s.id == custom_id) {
                    anyhow::bail!("Session ID '{}' already exists", custom_id);
                }
            }
            custom_id.to_string()
        } else {
            generate_session_id() // F-085
        };

        Ok(Session {
            schema_version: SESSION_SCHEMA_VERSION,
            id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            provider: provider.to_string(),
            model: model.to_string(),
            cwd: cwd.to_string(),
            total_usage: TokenUsage::default(),
            messages: Vec::new(),
            extra: serde_json::Map::new(),
        })
    }

    /// Create a fresh session together with its exclusive journal authority.
    ///
    /// The legacy snapshot remains deferred until the first user message, but
    /// the journal lease is held from session initialization onward.
    pub fn create_for_run(
        &self,
        provider: &str,
        model: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<ActiveSession> {
        let session = self.create(provider, model, cwd, session_id)?;
        std::fs::create_dir_all(&self.directory)?;
        let journal = SessionJournal::open(self.journal_path(&session.id), session.id.clone())?;
        self.ensure_journal_imported(&session, &journal)?;
        Ok(ActiveSession { session, journal })
    }

    /// Called by the engine WAL hook (F-030) to record the first user message
    /// before any LLM call.  Also triggers the first disk write of the session
    /// file and index entry so the session is visible to `--list-sessions`
    /// from this point forward.
    pub fn persist_first_message(&self, session: &Session) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        self.save(session)?;
        with_index_lock(&self.directory, |index| {
            upsert_meta(index, session);
            Ok(())
        })?;
        self.cleanup_old()?;
        Ok(())
    }

    /// Save current session state (called after each turn).
    pub fn save(&self, session: &Session) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let path = self.session_path(session);
        let json = serde_json::to_string_pretty(session)?;
        wcore_config::atomic_write(&path, json.as_bytes())?;
        Ok(())
    }

    /// Atomically with respect to WAL writers, save the canonical snapshot and
    /// remove the now-redundant WAL. This prevents another process from
    /// appending between the snapshot read and WAL deletion.
    pub fn save_and_clear_wal(&self, session: &Session) -> anyhow::Result<()> {
        let wal_path = self.wal_path(session);
        with_wal_lock(&wal_path, || {
            self.save(session)?;
            match std::fs::remove_file(&wal_path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
    }

    /// Append the user message text to the WAL file for this session (F-030).
    ///
    /// The WAL survives a SIGKILL.  On the next `load()` the engine merges
    /// it back (see `merge_wal`).  Each WAL line is a JSON object:
    /// `{"role":"user","content":"<text>"}` so it can be parsed without the
    /// full `Message` type.
    pub fn append_wal(&self, session: &Session, user_text: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.directory)?;
        let wal_path = self.wal_path(session);
        let entry = serde_json::json!({
            "role": "user",
            "content": user_text,
            "ts": Utc::now().to_rfc3339(),
            // The canonical snapshot has this many messages immediately before
            // this prompt. Recovery can therefore distinguish an unapplied WAL
            // record from a save-completed/delete-interrupted stale record,
            // even when the prompt text is intentionally repeated.
            "base_message_count": session.messages.len(),
        });
        let mut encoded = serde_json::to_vec(&entry)?;
        encoded.push(b'\n');
        with_wal_lock(&wal_path, || {
            let mut file = open_secure_append(&wal_path)?;
            file.write_all(&encoded)?;
            file.sync_all()?;
            Ok(())
        })
    }

    /// Merge a WAL file (if present) into `session.messages` and delete it.
    ///
    /// Called at `SessionManager::load` time.  If the WAL contains entries
    /// not already in `session.messages` (comparing by text), they are
    /// appended as `Role::User` messages so a SIGKILL mid-turn is recoverable.
    pub fn merge_wal(&self, session: &mut Session) -> anyhow::Result<()> {
        let wal_path = self.wal_path(session);
        with_wal_lock(&wal_path, || {
            if !wal_path.exists() {
                return Ok(());
            }

            let bytes = std::fs::read(&wal_path)?;
            // A durable record always ends in '\n'. Bytes after the final
            // newline are an incomplete crash-time append and are not part of
            // the committed prefix. A malformed newline-terminated record is
            // complete corruption and fails below.
            let committed_len = if bytes.ends_with(b"\n") {
                bytes.len()
            } else {
                bytes
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(0, |index| index + 1)
            };
            let committed = std::str::from_utf8(&bytes[..committed_len]).map_err(|error| {
                anyhow::anyhow!(
                    "Session WAL '{}' has invalid UTF-8 in its committed prefix: {}",
                    wal_path.display(),
                    error
                )
            })?;
            let mut recovered_records = Vec::new();
            for (line_index, line) in committed.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let obj = serde_json::from_str::<serde_json::Map<String, Value>>(line).map_err(
                    |error| {
                        anyhow::anyhow!(
                            "Session WAL '{}' is corrupt at line {}: {}",
                            wal_path.display(),
                            line_index + 1,
                            error
                        )
                    },
                )?;
                let Some(Value::String(role)) = obj.get("role") else {
                    anyhow::bail!(
                        "Session WAL '{}' is corrupt at line {}: missing string role",
                        wal_path.display(),
                        line_index + 1
                    );
                };
                let Some(Value::String(text)) = obj.get("content") else {
                    anyhow::bail!(
                        "Session WAL '{}' is corrupt at line {}: missing string content",
                        wal_path.display(),
                        line_index + 1
                    );
                };
                if role != "user" {
                    anyhow::bail!(
                        "Session WAL '{}' is corrupt at line {}: unsupported role '{}'",
                        wal_path.display(),
                        line_index + 1,
                        role
                    );
                }
                let base_message_count = match obj.get("base_message_count") {
                    None => None,
                    Some(Value::Number(value)) => value.as_u64().and_then(|value| {
                        if value <= usize::MAX as u64 {
                            Some(value as usize)
                        } else {
                            None
                        }
                    }),
                    Some(_) => None,
                };
                if obj.contains_key("base_message_count") && base_message_count.is_none() {
                    anyhow::bail!(
                        "Session WAL '{}' is corrupt at line {}: invalid base_message_count",
                        wal_path.display(),
                        line_index + 1
                    );
                }
                recovered_records.push((text.clone(), base_message_count));
            }

            use wcore_types::message::{ContentBlock, Role};
            let original_message_count = session.messages.len();
            for (text, base_message_count) in recovered_records {
                let already_committed = if let Some(base_message_count) = base_message_count {
                    original_message_count > base_message_count
                } else {
                    // Pre-F12 WAL records lack a cursor. Preserve their legacy
                    // duplicate-suppression behavior because exact ordering is
                    // unknowable; all new records use base_message_count.
                    session.messages.iter().any(|message| {
                        message.role == Role::User
                            && message.content.iter().any(|block| {
                                matches!(block, ContentBlock::Text { text: existing } if existing == &text)
                            })
                    })
                };
                if !already_committed {
                    session
                        .messages
                        .push(Message::now(Role::User, vec![ContentBlock::Text { text }]));
                }
            }

            // Commit the merged state before removing its recovery evidence. If
            // either operation fails, load fails loud and the WAL remains available.
            self.save(session)?;
            std::fs::remove_file(&wal_path)?;
            Ok(())
        })
    }

    /// Delete the WAL for a session (called after a clean save).
    pub fn delete_wal(&self, session: &Session) {
        let wal_path = self.wal_path(session);
        let _ = with_wal_lock(&wal_path, || match std::fs::remove_file(&wal_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        });
    }

    /// Load a session by ID (or "latest").
    ///
    /// Automatically merges any WAL file (F-030) and runs the migration
    /// ladder (F-031).
    ///
    /// **F-030 crash recovery (#273):** When the canonical `.json` file is
    /// absent but a `.wal` exists on disk, the session is reconstructed
    /// from the WAL alone instead of failing with "not found". This covers
    /// two dirty-death scenarios:
    ///
    /// 1. The `.json` was never written (process killed before the first
    ///    `save()`) but `.wal` entries from `append_wal` survive.
    /// 2. The index lost (or never received) the session row but a
    ///    `.wal` orphan matching the requested id exists.
    ///
    /// In both cases the recovered session is re-persisted (`save` +
    /// `update_index_for`) so the next `load` takes the normal `.json`
    /// path, and `extra.recovered_from_wal = true` is set so callers /
    /// telemetry can observe that recovery happened.
    pub fn load(&self, id_or_latest: &str) -> anyhow::Result<Session> {
        // F-084: validate non-"latest" IDs
        if id_or_latest != "latest" {
            validate_session_id(id_or_latest)?;
        }

        let index = self.load_index()?;

        let meta_opt = if id_or_latest == "latest" {
            index.sessions.last().cloned()
        } else {
            index
                .sessions
                .iter()
                .find(|s| s.id == id_or_latest)
                .cloned()
        };

        // Branch A: index has the meta — try `.json` first, fall back to WAL.
        if let Some(meta) = meta_opt {
            let path = self.session_path_by_id(&meta.id);
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let mut session: Session = serde_json::from_str(&content)?;
                    reject_future_schema(session.schema_version)?;
                    let migrated = session.migrate();
                    self.merge_wal(&mut session)?;
                    if migrated {
                        let _ = self.save(&session);
                    }
                    return Ok(session);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // `.json` missing but the index knows about it. If a
                    // `.wal` exists we can rebuild from it; otherwise this
                    // is true data loss.
                    if let Some(session) = self.recover_from_wal(&meta.id, Some(&meta))? {
                        return Ok(session);
                    }
                    anyhow::bail!(
                        "Session '{}' is in the index but neither '.json' nor '.wal' exist on disk",
                        meta.id
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Branch B: index has no entry. An atomic session snapshot can be
        // durable before its separate index update, so explicit-id recovery
        // must search for the JSON before falling back to an orphan WAL.
        if id_or_latest != "latest" {
            let unindexed_path = self.session_path_by_id(id_or_latest);
            if unindexed_path.exists() {
                let content = std::fs::read_to_string(&unindexed_path)?;
                let mut session: Session = serde_json::from_str(&content)?;
                reject_future_schema(session.schema_version)?;
                let migrated = session.migrate();
                self.merge_wal(&mut session)?;
                if migrated {
                    self.save(&session)?;
                }
                self.update_index_for(&session)?;
                return Ok(session);
            }
            if let Some(session) = self.recover_from_wal(id_or_latest, None)? {
                return Ok(session);
            }
        }

        if id_or_latest == "latest" {
            anyhow::bail!("No sessions found")
        } else {
            anyhow::bail!("Session '{}' not found", id_or_latest)
        }
    }

    /// Load a session for execution while holding its exclusive journal lease.
    ///
    /// The concrete id is resolved using read-only metadata first. The journal
    /// is then opened before `load` can migrate a snapshot, merge/delete a WAL,
    /// or repair the index, so no execution path mutates legacy state without
    /// full-lifetime writer authority.
    pub fn load_for_run(&self, id_or_latest: &str) -> anyhow::Result<ActiveSession> {
        let session_id = self.resolve_session_id_for_run(id_or_latest)?;
        std::fs::create_dir_all(&self.directory)?;
        let journal = SessionJournal::open(self.journal_path(&session_id), session_id.clone())?;
        let mut session = self.load(&session_id)?;
        self.ensure_journal_imported(&session, &journal)?;
        self.restore_journal_conversation(&mut session, &journal)?;
        Ok(ActiveSession { session, journal })
    }

    /// Restore the canonical provider-neutral transcript before execution.
    ///
    /// The JSON session file is a compatibility mirror and may lag when a
    /// process dies after a durable journal append. Loading its messages into
    /// the engine would let the next turn overwrite the recovered transcript
    /// with stale state, so the verified reduced journal always wins here.
    fn restore_journal_conversation(
        &self,
        session: &mut Session,
        journal: &SessionJournal,
    ) -> anyhow::Result<()> {
        let state = journal.state()?;
        if state.imported_baseline.is_none() {
            anyhow::bail!(
                "Session journal '{}' has no canonical imported baseline",
                session.id
            );
        }
        if state.session_id.as_deref() != Some(session.id.as_str()) {
            anyhow::bail!(
                "Session journal authority does not match session '{}'",
                session.id
            );
        }
        session.messages = state
            .conversation
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<Message>, _>>()?;
        Ok(())
    }

    /// Seed an empty journal from the exact provider-neutral legacy session.
    ///
    /// The append is fsynced by `SessionJournal::append`; the reduced-state
    /// snapshot is then atomically persisted and directory-synced. Reopening
    /// an already imported journal refreshes the snapshot without appending a
    /// duplicate baseline.
    fn ensure_journal_imported(
        &self,
        session: &Session,
        journal: &SessionJournal,
    ) -> anyhow::Result<()> {
        let state = journal.state()?;
        if state.imported_baseline.is_none() {
            if state.last_seq.is_some() {
                anyhow::bail!(
                    "Session journal '{}' contains events without a canonical import",
                    session.id
                );
            }
            let value = serde_json::to_value(session)?;
            let session_digest = state_payload_digest(&value)?;
            journal.append(SessionEvent::SessionImported {
                source_schema_version: session.schema_version,
                session: value,
                session_digest,
            })?;
        }

        let snapshot = SessionSnapshot::new(session.id.clone(), journal.state()?)?;
        write_snapshot(self.journal_snapshot_path(&session.id), &snapshot)?;
        Ok(())
    }

    /// Read-only existence probe followed by an authoritative execution load.
    ///
    /// This supports load-or-create hosts without swallowing corruption or a
    /// contested writer lease as if the session did not exist.
    pub fn load_for_run_if_exists(&self, id: &str) -> anyhow::Result<Option<ActiveSession>> {
        validate_session_id(id)?;
        let indexed = self
            .load_index()?
            .sessions
            .iter()
            .any(|session| session.id == id);
        let snapshot_exists = self.session_path_by_id(id).exists();
        let wal_exists = self.find_wal_path(id).is_some();
        if !indexed && !snapshot_exists && !wal_exists {
            return Ok(None);
        }
        self.load_for_run(id).map(Some)
    }

    fn resolve_session_id_for_run(&self, id_or_latest: &str) -> anyhow::Result<String> {
        if id_or_latest != "latest" {
            validate_session_id(id_or_latest)?;
            return Ok(id_or_latest.to_string());
        }

        self.load_index()?
            .sessions
            .last()
            .map(|session| session.id.clone())
            .ok_or_else(|| anyhow::anyhow!("No sessions found"))
    }

    /// F-030: reconstruct a session from its `.wal` file alone (the
    /// canonical `.json` is missing). Returns `Ok(None)` if no matching
    /// `.wal` exists.
    ///
    /// When `meta` is supplied (index has the row but `.json` is gone) we
    /// preserve `created_at` / `model` from the index. When `meta` is
    /// `None` (orphan recovery) we parse `created_at` from the WAL
    /// filename's date prefix and fall back to "unknown" for
    /// provider/model/cwd — the WAL only carries user-message text, not
    /// session metadata.
    ///
    /// The recovered session is persisted (`save` + `update_index_for`)
    /// so the next `load` finds a normal `.json`, and the in-memory copy
    /// is tagged via `extra.recovered_from_wal = true`.
    fn recover_from_wal(
        &self,
        id: &str,
        meta: Option<&SessionMeta>,
    ) -> anyhow::Result<Option<Session>> {
        let wal_path = match self.find_wal_path(id) {
            Some(p) => p,
            None => return Ok(None),
        };

        // Parse the date prefix from the filename so created_at is plausible.
        let created_at = parsed_date_from_wal_filename(&wal_path)
            .or_else(|| meta.map(|m| m.created_at))
            .unwrap_or_else(Utc::now);
        let updated_at = meta.map(|m| m.updated_at).unwrap_or_else(Utc::now);
        let model = meta
            .map(|m| m.model.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let mut session = Session {
            schema_version: SESSION_SCHEMA_VERSION,
            id: id.to_string(),
            created_at,
            updated_at,
            provider: "unknown".to_string(),
            model,
            cwd: String::new(),
            total_usage: TokenUsage::default(),
            messages: Vec::new(),
            extra: serde_json::Map::new(),
        };
        session
            .extra
            .insert("recovered_from_wal".to_string(), Value::Bool(true));

        // Fold the WAL entries into messages and delete the WAL.
        self.merge_wal(&mut session)?;

        // `merge_wal` persisted the reconstructed `.json` before deleting the
        // recovery evidence. Index repair must also succeed; otherwise callers
        // get an explicit error instead of a false-green recovered session.
        self.update_index_for(&session)?;

        Ok(Some(session))
    }

    /// Locate the `.wal` file for an id without needing a Session struct.
    /// Mirrors `session_path_by_id` (glob on `*_{id}.wal`) so orphan
    /// recovery works when the filename's date prefix is unknown.
    fn find_wal_path(&self, id: &str) -> Option<PathBuf> {
        let pattern = format!("*_{}.wal", id);
        if let Ok(mut hits) = glob::glob(self.directory.join(&pattern).to_string_lossy().as_ref())
            && let Some(Ok(p)) = hits.next()
            && p.exists()
        {
            return Some(p);
        }
        // Fallback (shouldn't happen if the WAL was written by `append_wal`).
        let fallback = self.directory.join(format!("{}.wal", id));
        if fallback.exists() {
            return Some(fallback);
        }
        None
    }

    /// List all sessions.
    pub fn list(&self) -> anyhow::Result<Vec<SessionMeta>> {
        let index = self.load_index()?;
        Ok(index.sessions)
    }

    /// Update the session index (public, called from engine after save).
    pub fn update_index_for(&self, session: &Session) -> anyhow::Result<()> {
        with_index_lock(&self.directory, |index| {
            upsert_meta(index, session);
            Ok(())
        })
    }

    fn load_index(&self) -> anyhow::Result<SessionIndex> {
        let index_path = self.directory.join("index.json");
        match std::fs::read_to_string(&index_path) {
            Ok(content) => Ok(serde_json::from_str(&content)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SessionIndex {
                sessions: Vec::new(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    /// Remove oldest sessions beyond max_sessions (F-034: also sweeps empty
    /// sessions older than 5 minutes).
    fn cleanup_old(&self) -> anyhow::Result<()> {
        let _leases = with_index_lock(&self.directory, |index| {
            let now = SystemTime::now();
            let five_min = Duration::from_secs(5 * 60);
            let mut leases = Vec::new();
            let mut retained = Vec::with_capacity(index.sessions.len());

            // F-034: remove empty sessions (message_count == 0) older than 5 min.
            for meta in std::mem::take(&mut index.sessions) {
                let created = meta
                    .created_at
                    .signed_duration_since(DateTime::<Utc>::from(UNIX_EPOCH))
                    .to_std()
                    .ok();
                let expired_empty = meta.message_count == 0
                    && created
                        .and_then(|created_secs| {
                            now.duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|now_secs| now_secs.saturating_sub(created_secs) >= five_min)
                        })
                        .unwrap_or(true);
                if expired_empty {
                    match self.remove_session_storage(&meta)? {
                        Some(lease) => leases.push(lease),
                        None => retained.push(meta),
                    }
                } else {
                    retained.push(meta);
                }
            }

            retained.sort_by_key(|meta| meta.created_at);
            let mut candidate = 0;
            while retained.len() > self.max_sessions && candidate < retained.len() {
                match self.remove_session_storage(&retained[candidate])? {
                    Some(lease) => {
                        retained.remove(candidate);
                        leases.push(lease);
                    }
                    None => candidate += 1,
                }
            }
            index.sessions = retained;
            Ok(leases)
        })?;
        Ok(())
    }

    fn remove_session_storage(
        &self,
        meta: &SessionMeta,
    ) -> anyhow::Result<Option<SessionStorageLease>> {
        let journal_path = self.journal_path(&meta.id);
        let lease = match SessionJournal::acquire_storage_lease(&journal_path, &meta.id) {
            Ok(lease) => lease,
            Err(JournalError::AlreadyOwned { .. }) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let session_path = self.directory.join(format!(
            "{}_{}.json",
            meta.created_at.format("%Y-%m-%d"),
            meta.id
        ));
        let wal_path = session_path.with_extension("wal");
        with_wal_lock(&wal_path, || {
            lease
                .remove_files(&session_path, &wal_path)
                .map_err(anyhow::Error::from)
        })?;
        Ok(Some(lease))
    }

    fn session_path(&self, session: &Session) -> PathBuf {
        self.directory.join(format!(
            "{}_{}.json",
            session.created_at.format("%Y-%m-%d"),
            session.id
        ))
    }

    fn session_path_by_id(&self, id: &str) -> PathBuf {
        // Try glob first (date prefix is unknown), fall back to a simple name.
        let pattern = format!("*_{}.json", id);
        if let Ok(mut hits) = glob::glob(self.directory.join(&pattern).to_string_lossy().as_ref())
            && let Some(Ok(p)) = hits.next()
        {
            return p;
        }
        // Fallback (shouldn't happen if index is consistent).
        self.directory.join(format!("{}.json", id))
    }

    fn wal_path(&self, session: &Session) -> PathBuf {
        self.directory.join(format!(
            "{}_{}.wal",
            session.created_at.format("%Y-%m-%d"),
            session.id
        ))
    }

    fn journal_path(&self, session_id: &str) -> PathBuf {
        self.directory.join(format!("{session_id}.journal"))
    }

    fn journal_snapshot_path(&self, session_id: &str) -> PathBuf {
        self.directory
            .join(format!("{session_id}.journal.snapshot"))
    }
}

// ── Index locking (F-033) ────────────────────────────────────────────────────

/// Execute `f(index)` with an exclusive advisory lock on the index file.
///
/// Uses a `.lock` sentinel file with a stale-lock timeout of 30 s so a
/// SIGKILL of a writer does not permanently block readers.
///
/// The closure receives a `&mut SessionIndex` and any mutations are
/// atomically written back to `index.json` after `f` returns.
fn with_index_lock<F, T>(directory: &Path, f: F) -> anyhow::Result<T>
where
    F: FnOnce(&mut SessionIndex) -> anyhow::Result<T>,
{
    std::fs::create_dir_all(directory)?;
    let lock_path = directory.join("index.lock");
    let index_path = directory.join("index.json");

    // Acquire the sentinel lock with stale-lock timeout.
    acquire_sentinel_lock(&lock_path, Duration::from_secs(30))?;

    let result = (|| -> anyhow::Result<T> {
        // Read current index (inside the lock).
        let mut index = match std::fs::read_to_string(&index_path) {
            Ok(content) => serde_json::from_str::<SessionIndex>(&content)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => SessionIndex {
                sessions: Vec::new(),
            },
            Err(error) => return Err(error.into()),
        };

        let value = f(&mut index)?;

        let json = serde_json::to_string_pretty(&index)?;
        wcore_config::atomic_write(&index_path, json.as_bytes())?;
        Ok(value)
    })();

    // Always release the lock, even on error.
    let _ = std::fs::remove_file(&lock_path);
    result
}

/// Write `lock_path` atomically.  If the file already exists and is younger
/// than `stale_timeout`, spin-wait up to 1 s then steal the lock.
fn acquire_sentinel_lock(lock_path: &Path, stale_timeout: Duration) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        // Try to create the lock file exclusively (fails if it exists).
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut f) => {
                // Write our PID for diagnostic purposes.
                let _ = writeln!(f, "{}", std::process::id());
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Check for stale lock: steal if older than stale_timeout.
                if let Ok(meta) = std::fs::metadata(lock_path)
                    && let Ok(modified) = meta.modified()
                    && SystemTime::now()
                        .duration_since(modified)
                        .unwrap_or_default()
                        > stale_timeout
                {
                    // Stale — steal it.
                    let _ = std::fs::remove_file(lock_path);
                    continue;
                }
                if std::time::Instant::now() > deadline {
                    // Give up after 1 s to avoid a deadlock.
                    anyhow::bail!("Could not acquire index lock after 1s");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn reject_future_schema(schema_version: u32) -> anyhow::Result<()> {
    if schema_version > SESSION_SCHEMA_VERSION {
        anyhow::bail!(
            "Session schema version {} is newer than supported version {}; refusing to rewrite it",
            schema_version,
            SESSION_SCHEMA_VERSION
        );
    }
    Ok(())
}

fn with_wal_lock<T>(
    wal_path: &Path,
    operation: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let lock_path = wal_path.with_extension("wal.lock");
    acquire_sentinel_lock(&lock_path, Duration::from_secs(30))?;
    let result = operation();
    let _ = std::fs::remove_file(&lock_path);
    result
}

#[cfg(unix)]
fn open_secure_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_secure_append(path: &Path) -> std::io::Result<std::fs::File> {
    OpenOptions::new().create(true).append(true).open(path)
}

fn upsert_meta(index: &mut SessionIndex, session: &Session) {
    let summary = session
        .messages
        .iter()
        .find(|m| m.role == wcore_types::message::Role::User)
        .and_then(|m| {
            m.content.iter().find_map(|c| {
                if let wcore_types::message::ContentBlock::Text { text } = c {
                    Some(truncate_str(text, 80))
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    let meta = SessionMeta {
        id: session.id.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        model: session.model.clone(),
        summary,
        message_count: session.messages.len(),
    };

    if let Some(existing) = index.sessions.iter_mut().find(|s| s.id == session.id) {
        *existing = meta;
    } else {
        index.sessions.push(meta);
    }
}

/// F-084: reject IDs that could be used for path-traversal or glob injection.
/// Accepts only `[a-f0-9-]{6,40}` (hex short IDs + UUIDv4 with hyphens).
fn validate_session_id(id: &str) -> anyhow::Result<()> {
    let valid =
        id.len() >= 6 && id.len() <= 40 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if !valid {
        anyhow::bail!(
            "Invalid session ID '{}': must be 6-40 hex characters (optionally with hyphens)",
            id
        );
    }
    Ok(())
}

/// F-085: generate a collision-resistant session ID using UUIDv4.
/// The first 8 hex chars of the UUID give a 32-bit ID space (4 billion
/// distinct values) — a massive improvement over the 24-bit subsec-nanos
/// approach.  The full UUID is used for `--session-id` display; short-form
/// is still supported for `--resume <first8>` via the index.
fn generate_session_id() -> String {
    uuid::Uuid::new_v4().to_string().replace('-', "")[..12].to_string()
}

/// F-030 recovery helper: parse the `YYYY-MM-DD` prefix from a WAL path
/// like `2026-05-24_aabbccdd.wal` into a `DateTime<Utc>` at midnight. Used
/// to give an orphan-recovered session a plausible `created_at` when the
/// index has no entry. Returns `None` if the filename doesn't match.
fn parsed_date_from_wal_filename(path: &Path) -> Option<DateTime<Utc>> {
    let stem = path.file_stem()?.to_str()?;
    let (date_part, _) = stem.split_once('_')?;
    let naive = chrono::NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    let dt = naive.and_hms_opt(0, 0, 0)?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(dt, Utc))
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}

#[cfg(test)]
mod tests {
    // The "gpt-4"/"claude-3" strings throughout this module are opaque
    // placeholders for SessionManager round-tripping (create/load/list/limit).
    // The tests care about the manager's plumbing, not the model's identity,
    // so the values are deliberately generic. Tests that exercise real-model
    // behaviour use canonical aliases from wcore_types::model_aliases.
    use super::*;
    use tempfile::tempdir;
    use wcore_types::message::{ContentBlock, Message, Role};

    fn make_user_msg(text: &str) -> Message {
        Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        )
    }

    #[test]
    fn test_create_session() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let result = manager.create("openai", "gpt-4", "/tmp", None);
        assert!(result.is_ok());

        let session = result.unwrap();
        assert_eq!(session.provider, "openai");
        assert_eq!(session.model, "gpt-4");
        assert_eq!(session.cwd, "/tmp");
        assert!(session.messages.is_empty());
        // F-031: schema_version set
        assert_eq!(session.schema_version, SESSION_SCHEMA_VERSION);
    }

    #[test]
    fn test_save_and_load_session() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let mut session = manager
            .create("anthropic", "claude-3", "/home", None)
            .unwrap();
        session.messages.push(make_user_msg("hello"));
        manager.persist_first_message(&session).unwrap();
        let loaded = manager.load(&session.id).unwrap();

        assert_eq!(loaded.id, session.id);
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.model, "claude-3");
        assert_eq!(loaded.cwd, "/home");
    }

    #[test]
    fn test_load_nonexistent_returns_error() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let result = manager.load("aabbccdd");
        assert!(result.is_err());
    }

    #[test]
    fn active_session_holds_exclusive_lease_until_drop() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        manager.persist_first_message(&session).unwrap();

        let active = manager.load_for_run(&session.id).unwrap();
        assert_eq!(active.session.id, session.id);

        let contested = manager.load_for_run(&session.id).unwrap_err();
        assert!(
            contested.to_string().contains("already held"),
            "second execution owner must fail deterministically: {contested}"
        );

        drop(active);
        let reacquired = manager.load_for_run(&session.id).unwrap();
        assert_eq!(reacquired.session.id, session.id);
    }

    #[test]
    fn execution_session_imports_exactly_once_and_snapshots_the_journal() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        session.messages.push(make_user_msg("preserve me"));
        manager.persist_first_message(&session).unwrap();

        let active = manager.load_for_run(&session.id).unwrap();
        let state = active.journal.state().unwrap();
        let baseline = state.imported_baseline.as_ref().unwrap();
        assert_eq!(
            baseline.session,
            serde_json::to_value(&active.session).unwrap()
        );
        assert_eq!(state.conversation.len(), 1);
        let snapshot = crate::session_journal::load_snapshot(
            manager.journal_snapshot_path(&active.session.id),
        )
        .unwrap();
        assert_eq!(snapshot.state, state);
        drop(active);

        let reopened = manager.load_for_run(&session.id).unwrap();
        assert_eq!(
            SessionJournal::replay(manager.journal_path(&session.id))
                .unwrap()
                .len(),
            1,
            "reopening must not append a duplicate import"
        );
        assert_eq!(reopened.journal.state().unwrap().conversation.len(), 1);
    }

    #[test]
    fn execution_load_restores_journal_conversation_over_stale_legacy_mirror() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        session.messages.push(make_user_msg("stale mirror"));
        manager.persist_first_message(&session).unwrap();

        let active = manager.load_for_run(&session.id).unwrap();
        active
            .journal
            .append(SessionEvent::TurnStarted {
                turn_id: "t1".into(),
                user_message: "continue".into(),
            })
            .unwrap();
        let assistant = Message::now(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "journal is canonical".into(),
            }],
        );
        let value = serde_json::to_value(&assistant).unwrap();
        active
            .journal
            .append(SessionEvent::ConversationMessageCommitted {
                turn_id: "t1".into(),
                message_index: 1,
                message_digest: state_payload_digest(&value).unwrap(),
                message: value,
            })
            .unwrap();
        drop(active);

        let reopened = manager.load_for_run(&session.id).unwrap();
        assert_eq!(reopened.session.messages.len(), 2);
        assert_eq!(
            serde_json::to_value(&reopened.session.messages[1]).unwrap(),
            serde_json::to_value(assistant).unwrap()
        );
    }

    #[test]
    fn test_list_sessions_empty() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let sessions = manager.list().unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_list_sessions_sorted_by_time() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let mut s1 = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        s1.messages.push(make_user_msg("first"));
        manager.persist_first_message(&s1).unwrap();

        let mut s2 = manager
            .create("anthropic", "claude-3", "/home", None)
            .unwrap();
        s2.messages.push(make_user_msg("second"));
        manager.persist_first_message(&s2).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 2);

        let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&s1.id.as_str()));
        assert!(ids.contains(&s2.id.as_str()));
    }

    #[test]
    fn test_update_index() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        let msg = make_user_msg("hello");
        session.messages.push(msg);
        manager.persist_first_message(&session).unwrap();
        manager.update_index_for(&session).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].summary, "hello");
        assert_eq!(list[0].message_count, 1);
    }

    #[test]
    fn test_cleanup_old_sessions() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 2);

        for i in 0..3 {
            let mut s = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
            s.messages.push(make_user_msg(&format!("msg {i}")));
            manager.persist_first_message(&s).unwrap();
        }

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 2);
    }

    // F-031: old session JSON without `provider` should load via migration
    #[test]
    fn test_f031_migration_missing_provider() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        // Write a pre-v1 session (no schema_version, no provider)
        let id = generate_session_id();
        let now = Utc::now();
        let filename = format!("{}_{}.json", now.format("%Y-%m-%d"), id);
        let json = serde_json::json!({
            "id": id,
            "created_at": now,
            "updated_at": now,
            "model": "gpt-4",
            "cwd": "/tmp",
            "total_usage": {"input_tokens": 0, "output_tokens": 0},
            "messages": []
        });
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join(&filename), json.to_string()).unwrap();

        // Seed the index so load() can find it.
        let index = SessionIndex {
            sessions: vec![SessionMeta {
                id: id.clone(),
                created_at: now,
                updated_at: now,
                model: "gpt-4".to_string(),
                summary: String::new(),
                message_count: 0,
            }],
        };
        std::fs::write(
            dir.path().join("index.json"),
            serde_json::to_string(&index).unwrap(),
        )
        .unwrap();

        let session = manager.load(&id).expect("migration should succeed");
        assert_eq!(session.schema_version, SESSION_SCHEMA_VERSION);
        assert!(!session.provider.is_empty());
    }

    // F-032: unknown future fields round-trip losslessly
    #[test]
    fn test_f032_unknown_fields_preserved() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let id = generate_session_id();
        let now = Utc::now();
        let filename = format!("{}_{}.json", now.format("%Y-%m-%d"), id);
        // Include a future field not in the Session struct.
        let json = serde_json::json!({
            "schema_version": SESSION_SCHEMA_VERSION,
            "id": id,
            "created_at": now,
            "updated_at": now,
            "provider": "openai",
            "model": "gpt-4",
            "cwd": "/tmp",
            "total_usage": {"input_tokens": 0, "output_tokens": 0},
            "messages": [],
            "future_field_v09": "preserved"
        });
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join(&filename), json.to_string()).unwrap();

        let index = SessionIndex {
            sessions: vec![SessionMeta {
                id: id.clone(),
                created_at: now,
                updated_at: now,
                model: "gpt-4".to_string(),
                summary: String::new(),
                message_count: 0,
            }],
        };
        std::fs::write(
            dir.path().join("index.json"),
            serde_json::to_string(&index).unwrap(),
        )
        .unwrap();

        let session = manager.load(&id).unwrap();
        assert_eq!(
            session.extra.get("future_field_v09"),
            Some(&Value::String("preserved".to_string()))
        );

        // Round-trip: save and reload.
        manager.save(&session).unwrap();
        let on_disk = std::fs::read_to_string(dir.path().join(&filename)).unwrap();
        assert!(
            on_disk.contains("future_field_v09"),
            "future field must survive save round-trip"
        );
    }

    // F-033: parallel index writes don't lose entries
    #[test]
    fn test_f033_index_lock_parallel() {
        use std::sync::Arc;
        use std::thread;

        let dir = tempdir().unwrap();
        let dir_path = Arc::new(dir.path().to_path_buf());
        let n = 10;

        let handles: Vec<_> = (0..n)
            .map(|i| {
                let d = Arc::clone(&dir_path);
                thread::spawn(move || {
                    let manager = SessionManager::new((*d).clone(), 100);
                    let mut s = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
                    s.messages.push(Message::now(
                        Role::User,
                        vec![ContentBlock::Text {
                            text: format!("msg {i}"),
                        }],
                    ));
                    manager.persist_first_message(&s).unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let manager = SessionManager::new((*dir_path).clone(), 100);
        let list = manager.list().unwrap();
        assert_eq!(
            list.len(),
            n,
            "all {n} sessions must appear in the index; got {}",
            list.len()
        );
    }

    // F-034: empty sessions older than 5 min are GC'd by cleanup_old
    #[test]
    fn test_f034_empty_session_gc() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 100);

        // Create a session that's "old" by back-dating its created_at in the
        // index (we can't actually wait 5 min in a unit test).
        let id = generate_session_id();
        let old_time = Utc::now() - chrono::Duration::minutes(10);
        let filename = format!("{}_{}.json", old_time.format("%Y-%m-%d"), id);
        let json = serde_json::json!({
            "schema_version": SESSION_SCHEMA_VERSION,
            "id": id,
            "created_at": old_time,
            "updated_at": old_time,
            "provider": "openai",
            "model": "gpt-4",
            "cwd": "/tmp",
            "total_usage": {"input_tokens": 0, "output_tokens": 0},
            "messages": []
        });
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::write(dir.path().join(&filename), json.to_string()).unwrap();
        let session_path = dir.path().join(&filename);
        let wal_path = session_path.with_extension("wal");
        std::fs::write(&wal_path, b"durable wal evidence\n").unwrap();
        let journal_path = manager.journal_path(&id);
        let journal = SessionJournal::open(&journal_path, id.clone()).unwrap();
        let snapshot = SessionSnapshot::new(
            id.clone(),
            crate::session_journal::ReducedSessionState::default(),
        )
        .unwrap();
        write_snapshot(manager.journal_snapshot_path(&id), &snapshot).unwrap();
        drop(journal);

        // Seed index with message_count=0 and old created_at.
        let index = SessionIndex {
            sessions: vec![SessionMeta {
                id: id.clone(),
                created_at: old_time,
                updated_at: old_time,
                model: "gpt-4".to_string(),
                summary: String::new(),
                message_count: 0,
            }],
        };
        std::fs::write(
            dir.path().join("index.json"),
            serde_json::to_string(&index).unwrap(),
        )
        .unwrap();

        // Trigger cleanup via a new create (which calls cleanup_old internally
        // via persist_first_message → cleanup_old).
        let mut s = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        s.messages.push(make_user_msg("hello"));
        manager.persist_first_message(&s).unwrap();

        let list = manager.list().unwrap();
        // The old empty session must have been evicted.
        assert!(
            !list.iter().any(|m| m.id == id),
            "old empty session must be GC'd"
        );
        assert!(!session_path.exists());
        assert!(!wal_path.exists());
        assert!(!journal_path.exists());
        assert!(!manager.journal_snapshot_path(&id).exists());
        assert!(
            dir.path()
                .join(format!("{id}.journal.writer.lock"))
                .exists(),
            "cleanup must retain the race-safe lock sentinel"
        );
    }

    #[test]
    fn count_cleanup_retains_active_session_and_its_index_authority() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 1);

        let mut active_session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        active_session.created_at = Utc::now() - chrono::Duration::days(1);
        active_session.updated_at = active_session.created_at;
        active_session.messages.push(make_user_msg("active"));
        manager.persist_first_message(&active_session).unwrap();
        let active = manager.load_for_run(&active_session.id).unwrap();

        let mut newer = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        newer.messages.push(make_user_msg("newer"));
        manager.persist_first_message(&newer).unwrap();

        let list = manager.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, active_session.id);
        assert!(manager.session_path(&active_session).exists());
        assert!(manager.journal_path(&active_session.id).exists());
        assert!(manager.journal_snapshot_path(&active_session.id).exists());
        assert!(
            !manager.session_path(&newer).exists(),
            "an inactive candidate may be evicted instead"
        );

        drop(active);
    }

    #[test]
    fn cleanup_error_attempts_all_artifacts_but_retains_index_authority() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let id = generate_session_id();
        let old_time = Utc::now() - chrono::Duration::minutes(10);
        let session_path = dir
            .path()
            .join(format!("{}_{}.json", old_time.format("%Y-%m-%d"), id));
        std::fs::create_dir_all(&session_path).unwrap();
        let wal_path = session_path.with_extension("wal");
        std::fs::write(&wal_path, b"plaintext wal").unwrap();
        let journal_path = manager.journal_path(&id);
        let journal = SessionJournal::open(&journal_path, id.clone()).unwrap();
        let snapshot_path = manager.journal_snapshot_path(&id);
        std::fs::write(&snapshot_path, b"snapshot evidence").unwrap();
        drop(journal);
        let index = SessionIndex {
            sessions: vec![SessionMeta {
                id: id.clone(),
                created_at: old_time,
                updated_at: old_time,
                model: "gpt-4".to_owned(),
                summary: String::new(),
                message_count: 0,
            }],
        };
        std::fs::write(
            dir.path().join("index.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();

        assert!(manager.cleanup_old().is_err());
        assert!(session_path.exists(), "failed artifact must remain visible");
        assert!(
            !wal_path.exists(),
            "cleanup must still attempt later artifacts"
        );
        assert!(
            !snapshot_path.exists(),
            "cleanup must still attempt later artifacts"
        );
        assert!(
            manager.list().unwrap().iter().any(|meta| meta.id == id),
            "any deletion error must retain index authority"
        );
    }

    // F-030: WAL append + merge round-trip
    #[test]
    fn test_f030_wal_roundtrip() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        // Persist session file (simulating the initial defer-write step).
        manager.save(&session).unwrap();

        // Simulate WAL append of a user message before LLM call.
        manager.append_wal(&session, "hello from user").unwrap();
        assert!(manager.wal_path(&session).exists());

        // Simulate "crash" by loading fresh (without full save).
        let mut recovered = session.clone();
        manager.merge_wal(&mut recovered).unwrap();

        assert!(recovered.messages.iter().any(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text == "hello from user"))
        }));
        // WAL must be deleted after merge.
        assert!(!manager.wal_path(&session).exists());
    }

    // F-030 #273: --resume reads .wal when .json is missing
    //
    // Reproduces the dirty-death scenario the audit caught: a session has
    // been started, an `append_wal` succeeded with a fresh user message,
    // then the process was SIGKILL'd before the next `save()` flushed the
    // `.json`. The `--resume <id>` path must rebuild the session from the
    // `.wal` instead of returning "not found".
    #[test]
    fn test_f030_resume_reads_wal_when_json_absent() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        // Set up session metadata in the index (this is what
        // `persist_first_message` would have written on a previous turn).
        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        manager
            .append_wal(&session, "msg from before the crash")
            .unwrap();
        assert!(manager.wal_path(&session).exists(), "WAL must be on disk");

        // Seed the index with a meta row for this session — mirroring the
        // state after one successful turn — but DO NOT call save(), so the
        // `.json` is absent (simulating dirty-death before the next flush).
        with_index_lock(dir.path(), |index| {
            upsert_meta(index, &session);
            Ok(())
        })
        .unwrap();
        assert!(
            !manager.session_path(&session).exists(),
            "json must be absent — that is the bug we are reproducing"
        );

        // --resume <id> must succeed and surface the WAL contents.
        let recovered = manager
            .load(&session.id)
            .expect("resume must recover from .wal when .json is missing");

        assert_eq!(recovered.id, session.id);
        assert!(
            recovered.messages.iter().any(|m| m.role == Role::User
                && m.content.iter().any(|c| matches!(
                    c,
                    ContentBlock::Text { text } if text == "msg from before the crash"
                ))),
            "recovered session must contain the WAL user message"
        );
        assert_eq!(
            recovered.extra.get("recovered_from_wal"),
            Some(&Value::Bool(true)),
            "recovered sessions must be tagged so callers / telemetry can see it"
        );

        // After recovery the `.json` exists, the WAL is gone, and a
        // second --resume takes the normal `.json` path.
        assert!(manager.session_path(&session).exists());
        assert!(!manager.wal_path(&session).exists());

        let reloaded = manager.load(&session.id).expect("second resume works");
        assert_eq!(reloaded.id, session.id);
        assert_eq!(reloaded.messages.len(), recovered.messages.len());
    }

    // F-030 #273: orphan WAL recovery — the index entry is missing too.
    #[test]
    fn test_f030_resume_recovers_orphan_wal() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);

        // A bare WAL file with no index entry and no `.json` — the
        // worst-case dirty-death (index update itself was lost).
        let id = generate_session_id();
        let now = Utc::now();
        let wal_filename = format!("{}_{}.wal", now.format("%Y-%m-%d"), id);
        std::fs::create_dir_all(dir.path()).unwrap();
        let entry = serde_json::json!({
            "role": "user",
            "content": "orphan wal message",
            "ts": now.to_rfc3339(),
        });
        std::fs::write(dir.path().join(&wal_filename), format!("{}\n", entry)).unwrap();

        let recovered = manager
            .load(&id)
            .expect("orphan .wal must be recoverable via --resume");

        assert_eq!(recovered.id, id);
        assert!(
            recovered.messages.iter().any(|m| m.role == Role::User
                && m.content.iter().any(|c| matches!(
                    c,
                    ContentBlock::Text { text } if text == "orphan wal message"
                ))),
            "orphan-recovered session must contain the WAL message"
        );
        assert_eq!(
            recovered.extra.get("recovered_from_wal"),
            Some(&Value::Bool(true)),
        );

        // Index was repaired so --list-sessions surfaces it.
        let list = manager.list().unwrap();
        assert!(list.iter().any(|m| m.id == id), "index must be repaired");
    }

    // F-030 #273: when neither `.json` nor `.wal` exists, --resume still
    // returns NotFound (we must not invent empty sessions).
    #[test]
    fn test_f030_resume_genuinely_missing_still_errors() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let err = manager.load("aabbccddeeff").unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected 'not found', got: {err}"
        );
    }

    // F-084: --resume validates ID format
    #[test]
    fn test_f084_id_validation() {
        assert!(validate_session_id("aabbcc").is_ok());
        assert!(validate_session_id("aabbccdd1122").is_ok());
        // Too short
        assert!(validate_session_id("abc").is_err());
        // Path traversal
        assert!(validate_session_id("../../../etc/passwd").is_err());
        // Glob wildcard
        assert!(validate_session_id("xx*").is_err());
    }

    // F-085: generated IDs are unique across rapid-fire calls
    #[test]
    fn test_f085_id_uniqueness() {
        let ids: Vec<_> = (0..4096).map(|_| generate_session_id()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "no collisions in 4096 IDs");
    }

    #[test]
    fn test_session_id_uniqueness() {
        // Keep old test working (was testing generate_short_id; now tests
        // generate_session_id which is trivially unique via UUIDv4).
        let id1 = generate_session_id();
        let id2 = generate_session_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn future_schema_is_rejected_without_rewriting_source_bytes() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let id = generate_session_id();
        let now = Utc::now();
        let filename = format!("{}_{}.json", now.format("%Y-%m-%d"), id);
        let path = dir.path().join(&filename);
        let source = serde_json::json!({
            "schema_version": SESSION_SCHEMA_VERSION + 1,
            "id": id,
            "created_at": now,
            "updated_at": now,
            "provider": "future-provider",
            "model": "future-model",
            "cwd": "/future",
            "total_usage": {"input_tokens": 0, "output_tokens": 0},
            "messages": [],
            "future_required_state": {"must_not_be_lost": true}
        })
        .to_string();
        std::fs::write(&path, &source).unwrap();
        let index = SessionIndex {
            sessions: vec![SessionMeta {
                id: id.clone(),
                created_at: now,
                updated_at: now,
                model: "future-model".to_string(),
                summary: String::new(),
                message_count: 0,
            }],
        };
        std::fs::write(
            dir.path().join("index.json"),
            serde_json::to_vec(&index).unwrap(),
        )
        .unwrap();

        let error = manager.load(&id).unwrap_err();
        assert!(
            error.to_string().contains("newer than supported"),
            "unexpected error: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            source,
            "a future session must never be rewritten by an older binary"
        );
    }

    #[test]
    fn corrupt_index_fails_loud_instead_of_appearing_empty() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        std::fs::write(dir.path().join("index.json"), b"{not valid json").unwrap();

        let error = manager.list().unwrap_err();
        assert!(
            error.to_string().contains("expected") || error.to_string().contains("key"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_wal_is_preserved_and_fails_loud() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        manager.save(&session).unwrap();
        let wal_path = manager.wal_path(&session);
        std::fs::write(&wal_path, b"{not valid json}\n").unwrap();

        let mut recovered = session.clone();
        let error = manager.merge_wal(&mut recovered).unwrap_err();
        assert!(
            error.to_string().contains("WAL"),
            "unexpected error: {error}"
        );
        assert!(wal_path.exists(), "corrupt evidence must not be deleted");
        assert!(recovered.messages.is_empty());
    }

    #[test]
    fn repeated_identical_wal_records_are_not_collapsed() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        manager.save(&session).unwrap();
        manager.append_wal(&session, "repeat me").unwrap();
        manager.append_wal(&session, "repeat me").unwrap();

        let mut recovered = session.clone();
        manager.merge_wal(&mut recovered).unwrap();
        let repeated = recovered
            .messages
            .iter()
            .filter(|message| {
                message.role == Role::User
                    && message.content.iter().any(
                        |block| matches!(block, ContentBlock::Text { text } if text == "repeat me"),
                    )
            })
            .count();
        assert_eq!(repeated, 2);
    }

    #[test]
    fn wal_record_already_in_a_completed_snapshot_is_not_replayed() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        session.messages.push(make_user_msg("already committed"));
        session.messages.push(Message::now(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "answer".to_string(),
            }],
        ));
        manager.save(&session).unwrap();

        // Simulate a crash after the full snapshot save but before WAL deletion.
        let stale_view = Session {
            messages: Vec::new(),
            ..session.clone()
        };
        manager
            .append_wal(&stale_view, "already committed")
            .unwrap();

        let loaded = manager.load(&session.id).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(
            loaded
                .messages
                .iter()
                .filter(|message| message.role == Role::User)
                .count(),
            1
        );
    }

    #[test]
    fn torn_final_wal_record_recovers_the_durable_prefix() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        manager.save(&session).unwrap();
        manager.append_wal(&session, "durable prefix").unwrap();
        let wal_path = manager.wal_path(&session);
        let mut file = OpenOptions::new().append(true).open(&wal_path).unwrap();
        file.write_all(b"{\"role\":\"user\",\"content\":\"")
            .unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();

        let mut recovered = session.clone();
        manager.merge_wal(&mut recovered).unwrap();
        assert_eq!(recovered.messages.len(), 1);
        assert!(
            recovered.messages[0].content.iter().any(
                |block| matches!(block, ContentBlock::Text { text } if text == "durable prefix")
            )
        );
    }

    #[test]
    fn unindexed_json_is_recovered_by_explicit_session_id() {
        let dir = tempdir().unwrap();
        let manager = SessionManager::new(dir.path().to_path_buf(), 10);
        let mut session = manager.create("openai", "gpt-4", "/tmp", None).unwrap();
        session.messages.push(make_user_msg("reachable"));
        manager.save(&session).unwrap();
        assert!(!dir.path().join("index.json").exists());

        let recovered = manager.load(&session.id).unwrap();
        assert_eq!(recovered.messages.len(), 1);
        assert!(
            manager
                .list()
                .unwrap()
                .iter()
                .any(|meta| meta.id == session.id)
        );
    }
}
