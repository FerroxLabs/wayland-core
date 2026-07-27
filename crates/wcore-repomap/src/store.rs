//! The persistent, incrementally-maintained repository index (F23-06).
//!
//! [`crate::RepoMap::build`] rebuilds an in-memory map from scratch on every
//! call. That is correct for a one-shot answer and useless for a long-running
//! session over a real codebase. [`IndexStore`] is the persistent counterpart:
//! a SQLite database holding one record per in-scope file keyed by a content
//! hash, the symbols extracted from it, and an FTS5 index over its text.
//!
//! It is **additive**. `RepoMap::build`, `build_with_options` and every public
//! type they touch are unchanged, and the live consumers in `wcore-tools` and
//! the CLI's at-reference resolution keep their current semantics.
//!
//! ## The three properties this module exists to have
//!
//! **Incrementality is a read count, not a stopwatch.** [`IndexStats`] reports
//! how many files were *opened*. Re-opening an unchanged store reads zero
//! files beyond the scope walk's own `stat`. A timing assertion would be
//! flaky under load and would prove the wrong thing; a read count cannot be
//! satisfied by a fast rebuild.
//!
//! **Exclusion is about what is stored, not what is returned.** A gitignored
//! file is never opened, so its bytes never enter the database. Filtering at
//! query time would leave the secret sitting in an artifact that gets backed
//! up, exported and migrated. The incremental suite proves this by planting a
//! run-time nonce in an ignored file and searching the **store file's own
//! bytes** for it.
//!
//! **Paths are normalised at the comparison boundary, on both operands.**
//! Every path written and every path looked up passes through
//! [`crate::scope::normalize_rel`]. See its documentation for why storing one
//! representation and querying another passes every Linux test and silently
//! misses on Windows.
//!
//! ## Schema
//!
//! ```text
//! meta(key, value)              schema_version, root, scope, built_at
//! files(id, path, content_hash, size_bytes, lines, language,
//!       mtime_unix_nanos, indexed_at_unix_secs, indexable)
//! symbols(id, file_id -> files.id, kind, name, line)
//! files_fts(path, content)      FTS5, rowid == files.id
//! ```
//!
//! `files_fts` is a *regular* (content-carrying) FTS5 table. Storing the text
//! is deliberate: it is what makes the exact-search fallback servable without
//! re-reading the tree, and it is what gives the secret-isolation proof its
//! teeth — with no content stored, a nonce could never appear in the store's
//! bytes and the gate could not go red.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::extractor::extract;
use crate::scope::{ScopeEntry, ScopeIdentity, normalize_rel, scope_files};
use crate::types::{IndexOptions, Language, RepoMapError, Symbol, SymbolKind};

/// Schema version written into `meta`. A store carrying a different value is
/// refused with a structured error offering a rebuild rather than being
/// silently migrated, because a wrong migration produces confidently wrong
/// retrieval.
pub const SCHEMA_VERSION: i64 = 1;

/// Content-hash sentinel for a file the index chose not to read the text of
/// (over `max_file_bytes`). The size participates so a growing or shrinking
/// oversized file still changes its hash.
fn oversize_hash(size_bytes: u64) -> String {
    format!("oversize-{size_bytes}")
}

/// What one refresh actually did.
///
/// `files_read` and `files_extracted` are the load-bearing fields: they are
/// how incrementality is asserted. A refresh that changed nothing reports
/// `files_read == 0` and `files_extracted == 0`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct IndexStats {
    /// Files the scope walk found.
    pub scanned: usize,
    /// Files whose bytes were opened and read from disk.
    pub files_read: usize,
    /// Files whose symbols were re-extracted.
    pub files_extracted: usize,
    /// Records created for a path the store had not seen.
    pub added: usize,
    /// Records whose content hash changed.
    pub changed: usize,
    /// Records removed because their path left scope.
    pub deleted: usize,
    /// Records moved to a new path with their content hash — and therefore
    /// their symbols — reused rather than re-extracted.
    pub renamed: usize,
    /// Records the refresh confirmed were already current.
    pub unchanged: usize,
    /// Whether the recorded scope identity differed from the working tree's at
    /// the start of this refresh.
    pub scope_changed: bool,
}

/// The result of [`IndexStore::verify`] — how far the store has drifted from
/// the working tree, without changing either.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct VerifyReport {
    /// Records the store holds.
    pub records: u64,
    /// Files the scope walk currently finds.
    pub in_scope: u64,
    /// In-scope files whose bytes differ from the recorded content hash.
    pub changed: u64,
    /// In-scope files the store has no record for.
    pub missing_from_store: u64,
    /// Records whose file is no longer in scope or no longer readable.
    pub missing_from_disk: u64,
    /// The store's recorded scope identity no longer matches the working
    /// tree.
    pub scope_drifted: bool,
}

impl VerifyReport {
    /// True only when the store describes the working tree exactly.
    ///
    /// Scope drift counts as disagreement. A store built on another branch
    /// may hold byte-identical records and still be answering about a
    /// different checkout than the operator is looking at.
    pub fn agrees(&self) -> bool {
        self.changed == 0
            && self.missing_from_store == 0
            && self.missing_from_disk == 0
            && !self.scope_drifted
    }
}

/// One stored file record, as the store holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileRecord {
    /// Canonical `/`-separated relative path key.
    pub path: String,
    /// Hex SHA-256 of the file's bytes, or an `oversize-<n>` sentinel for a
    /// file whose text the index declined to read.
    pub content_hash: String,
    /// Size in bytes at index time.
    pub size_bytes: u64,
    /// Line count, or `0` when the text was not read.
    pub lines: usize,
    /// Detected language.
    pub language: Language,
    /// Whether the file's text was stored and its symbols extracted.
    pub indexable: bool,
}

/// A persistent repository index.
///
/// Opening is cheap; [`IndexStore::refresh`] is where work happens.
pub struct IndexStore {
    conn: Connection,
    store_path: PathBuf,
    root: PathBuf,
}

impl IndexStore {
    /// Open (creating if absent) the store at `store_path`, indexing `root`.
    ///
    /// # Errors
    ///
    /// - [`RepoMapError::Root`] when `root` cannot be canonicalised.
    /// - [`RepoMapError::Store`] when the file exists but is not a readable
    ///   store — a corrupt or truncated database, or one written by a
    ///   different schema version. The error names the file and says a
    ///   rebuild is the remedy. It never panics and never presents an
    ///   unreadable store as an empty index.
    pub fn open(store_path: &Path, root: &Path) -> Result<Self, RepoMapError> {
        let canonical_root = fs::canonicalize(root).map_err(|e| RepoMapError::Root {
            path: root.to_path_buf(),
            source: e,
        })?;
        if let Some(parent) = store_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|e| RepoMapError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }

        let conn = Connection::open(store_path).map_err(|e| store_err(store_path, &e))?;
        let mut store = Self {
            conn,
            store_path: store_path.to_path_buf(),
            root: canonical_root,
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), RepoMapError> {
        // `PRAGMA journal_mode` RETURNS A ROW, so it is issued as a query
        // rather than folded into the batch below. This is also the first
        // real touch of the file, which is exactly where a corrupt or
        // truncated database must surface as a structured error rather than
        // as an empty index.
        let _mode: String = self
            .conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .map_err(|e| store_err(&self.store_path, &e))?;

        self.conn
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE IF NOT EXISTS meta (
                     key   TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS files (
                     id                   INTEGER PRIMARY KEY,
                     path                 TEXT NOT NULL UNIQUE,
                     content_hash         TEXT NOT NULL,
                     size_bytes           INTEGER NOT NULL,
                     lines                INTEGER NOT NULL,
                     language             TEXT NOT NULL,
                     mtime_unix_nanos     TEXT NOT NULL,
                     indexed_at_unix_secs INTEGER NOT NULL,
                     indexable            INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_files_hash ON files(content_hash);
                 CREATE TABLE IF NOT EXISTS symbols (
                     id      INTEGER PRIMARY KEY,
                     file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                     kind    TEXT NOT NULL,
                     name    TEXT NOT NULL,
                     line    INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
                 CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
                 CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
                     path, content, tokenize = 'unicode61'
                 );",
            )
            .map_err(|e| store_err(&self.store_path, &e))?;

        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| store_err(&self.store_path, &e))?;

        match existing {
            None => {
                self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
            }
            Some(v) if v == SCHEMA_VERSION.to_string() => {}
            Some(v) => {
                return Err(RepoMapError::Store {
                    path: self.store_path.clone(),
                    message: format!(
                        "index schema version {v} is not {SCHEMA_VERSION}; \
                         rebuild the index (delete this file and re-run the build)"
                    ),
                });
            }
        }
        Ok(())
    }

    /// Path of the database file backing this store.
    pub fn store_path(&self) -> &Path {
        &self.store_path
    }

    /// Canonicalised repository root this store indexes.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Borrow the underlying connection. Crate-internal so `search.rs` can
    /// issue its own queries without the store re-exporting every one of
    /// them; the connection is never handed outside the crate.
    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Bytes the store occupies on disk, including its WAL sidecar.
    ///
    /// The sidecar is counted because it is real occupancy that a size gate
    /// measured before a checkpoint would otherwise under-report.
    pub fn on_disk_bytes(&self) -> u64 {
        let mut total = fs::metadata(&self.store_path).map(|m| m.len()).unwrap_or(0);
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = self.store_path.clone().into_os_string();
            sidecar.push(suffix);
            if let Ok(m) = fs::metadata(PathBuf::from(sidecar)) {
                total += m.len();
            }
        }
        total
    }

    /// Number of file records currently held.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the query fails.
    pub fn record_count(&self) -> Result<u64, RepoMapError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| store_err(&self.store_path, &e))
    }

    /// Number of symbol records currently held.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the query fails.
    pub fn symbol_count(&self) -> Result<u64, RepoMapError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get::<_, i64>(0))
            .map(|n| n as u64)
            .map_err(|e| store_err(&self.store_path, &e))
    }

    /// The scope identity recorded at the last refresh, if any.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the metadata table cannot be read.
    pub fn recorded_scope(&self) -> Result<Option<ScopeIdentity>, RepoMapError> {
        Ok(self
            .get_meta("scope")?
            .and_then(|s| ScopeIdentity::from_fingerprint(&s)))
    }

    /// The working tree's current scope identity.
    pub fn current_scope(&self) -> ScopeIdentity {
        ScopeIdentity::detect(&self.root)
    }

    /// Whether the recorded scope identity still matches the working tree.
    ///
    /// A store that has never been refreshed reports drift, because "no
    /// recorded identity" is not the same claim as "identity matches".
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the metadata table cannot be read.
    pub fn scope_drifted(&self) -> Result<bool, RepoMapError> {
        match self.recorded_scope()? {
            Some(recorded) => Ok(recorded != self.current_scope()),
            None => Ok(true),
        }
    }

    /// Look one path up by its canonical key.
    ///
    /// `rel` is normalised through [`normalize_rel`] before comparison, so a
    /// caller may pass a native-separator path on any platform.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the query fails.
    pub fn lookup(&self, rel: &Path) -> Result<Option<FileRecord>, RepoMapError> {
        self.lookup_key(&normalize_rel(rel))
    }

    /// Look one path up by an already-normalised key.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the query fails.
    pub fn lookup_key(&self, key: &str) -> Result<Option<FileRecord>, RepoMapError> {
        self.conn
            .query_row(
                "SELECT path, content_hash, size_bytes, lines, language, indexable
                 FROM files WHERE path = ?1",
                params![key],
                |r| {
                    Ok(FileRecord {
                        path: r.get(0)?,
                        content_hash: r.get(1)?,
                        size_bytes: r.get::<_, i64>(2)? as u64,
                        lines: r.get::<_, i64>(3)? as usize,
                        language: language_from_str(&r.get::<_, String>(4)?),
                        indexable: r.get::<_, i64>(5)? != 0,
                    })
                },
            )
            .optional()
            .map_err(|e| store_err(&self.store_path, &e))
    }

    /// Symbols recorded for one path.
    ///
    /// # Errors
    ///
    /// [`RepoMapError::Store`] if the query fails.
    pub fn symbols_for(&self, key: &str) -> Result<Vec<Symbol>, RepoMapError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT symbols.kind, symbols.name, symbols.line
                 FROM symbols JOIN files ON files.id = symbols.file_id
                 WHERE files.path = ?1 ORDER BY symbols.line",
            )
            .map_err(|e| store_err(&self.store_path, &e))?;
        let rows = stmt
            .query_map(params![key], |r| {
                Ok(Symbol {
                    kind: symbol_kind_from_str(&r.get::<_, String>(0)?),
                    name: r.get(1)?,
                    line: r.get::<_, i64>(2)? as usize,
                })
            })
            .map_err(|e| store_err(&self.store_path, &e))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| store_err(&self.store_path, &e))?);
        }
        Ok(out)
    }

    /// Bring the store into agreement with the working tree.
    ///
    /// Walks the scope, then for each in-scope file:
    ///
    /// - identical size **and** modification time to the stored record →
    ///   **not opened**;
    /// - otherwise opened once, hashed, and re-extracted only if the hash
    ///   changed;
    /// - a path the store has not seen whose content hash matches a record
    ///   whose path just left scope is a **rename**: the record moves, its
    ///   hash is reused and its symbols are not re-extracted;
    /// - a path that left scope with no such match is deleted, taking its
    ///   symbols and its full-text row with it.
    ///
    /// A changed scope identity does **not** wipe the store. It is recorded,
    /// reported in [`IndexStats::scope_changed`], and the same incremental
    /// pass then touches exactly the records whose content differs.
    ///
    /// # Errors
    ///
    /// - [`RepoMapError::Root`] when the root can no longer be canonicalised.
    /// - [`RepoMapError::Store`] on any database failure.
    pub fn refresh(&mut self, opts: &IndexOptions) -> Result<IndexStats, RepoMapError> {
        let entries = scope_files(&self.root, opts)?;
        let scope = ScopeIdentity::detect(&self.root);
        let scope_changed = self.recorded_scope()?.is_none_or(|r| r != scope);

        let mut stats = IndexStats {
            scanned: entries.len(),
            scope_changed,
            ..IndexStats::default()
        };

        let mut existing = self.load_index_rows()?;
        let now = unix_secs();

        // Files the walk found that the store has not yet placed. Held until
        // deletions are known, because a "new" path whose hash matches a path
        // that just vanished is a rename, not an add.
        let mut pending_new: Vec<PreparedFile> = Vec::new();

        let tx = self
            .conn
            .transaction()
            .map_err(|e| store_err(&self.store_path, &e))?;

        for entry in &entries {
            match existing.remove(&entry.key) {
                Some(row) => {
                    if row.size_bytes == entry.size_bytes
                        && row.mtime_unix_nanos == entry.mtime_unix_nanos
                    {
                        // Unchanged by the walk's own stat. No open, no read.
                        stats.unchanged += 1;
                        continue;
                    }
                    let prepared = prepare_file(entry, opts, &mut stats)?;
                    if prepared.content_hash == row.content_hash {
                        // Touched but byte-identical (a checkout that rewrote
                        // mtime, a no-op save). Record the new mtime so the
                        // next refresh can skip it again without a read.
                        tx.execute(
                            "UPDATE files SET mtime_unix_nanos = ?1 WHERE id = ?2",
                            params![entry.mtime_unix_nanos.to_string(), row.id],
                        )
                        .map_err(|e| store_err(&self.store_path, &e))?;
                        stats.unchanged += 1;
                        continue;
                    }
                    write_file_row(&tx, Some(row.id), &prepared, now, &self.store_path)?;
                    if prepared.indexable {
                        stats.files_extracted += 1;
                    }
                    stats.changed += 1;
                }
                None => pending_new.push(prepare_file(entry, opts, &mut stats)?),
            }
        }

        // Whatever is left in `existing` left scope.
        let mut orphans_by_hash: HashMap<String, Vec<IndexRow>> = HashMap::new();
        for row in existing.into_values() {
            orphans_by_hash
                .entry(row.content_hash.clone())
                .or_default()
                .push(row);
        }

        for prepared in pending_new {
            let rename_target = orphans_by_hash
                .get_mut(&prepared.content_hash)
                .and_then(|rows| rows.pop());
            match rename_target {
                Some(row) => {
                    // Rename: the content hash is reused and the symbols and
                    // full-text row stay exactly as they were. Nothing is
                    // re-extracted.
                    tx.execute(
                        "UPDATE files
                         SET path = ?1, size_bytes = ?2, mtime_unix_nanos = ?3
                         WHERE id = ?4",
                        params![
                            prepared.key,
                            prepared.size_bytes as i64,
                            prepared.mtime_unix_nanos.to_string(),
                            row.id
                        ],
                    )
                    .map_err(|e| store_err(&self.store_path, &e))?;
                    tx.execute(
                        "UPDATE files_fts SET path = ?1 WHERE rowid = ?2",
                        params![prepared.key, row.id],
                    )
                    .map_err(|e| store_err(&self.store_path, &e))?;
                    stats.renamed += 1;
                }
                None => {
                    write_file_row(&tx, None, &prepared, now, &self.store_path)?;
                    if prepared.indexable {
                        stats.files_extracted += 1;
                    }
                    stats.added += 1;
                }
            }
        }

        for row in orphans_by_hash.into_values().flatten() {
            tx.execute("DELETE FROM files WHERE id = ?1", params![row.id])
                .map_err(|e| store_err(&self.store_path, &e))?;
            tx.execute("DELETE FROM symbols WHERE file_id = ?1", params![row.id])
                .map_err(|e| store_err(&self.store_path, &e))?;
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![row.id])
                .map_err(|e| store_err(&self.store_path, &e))?;
            stats.deleted += 1;
        }

        tx.commit().map_err(|e| store_err(&self.store_path, &e))?;

        self.set_meta("scope", &scope.fingerprint())?;
        self.set_meta("root", &self.root.to_string_lossy())?;
        self.set_meta("built_at_unix_secs", &now.to_string())?;

        // Fold the write-ahead log back into the database before anyone reads
        // the size. MEASURED, not assumed: immediately after a cold build of
        // this workspace the pair reported 133,366,096 bytes and, once the
        // WAL had been checkpointed, 66,420,792 — a 2x difference that is
        // pure journalling transient. A size gate that sampled the first
        // number would be measuring when it happened to look, not how large
        // the index is. Best-effort: a checkpoint that cannot run (a reader
        // still attached) is not a refresh failure.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        Ok(stats)
    }

    /// Compare the store against the working tree **without modifying it**.
    ///
    /// The read-only counterpart of [`IndexStore::refresh`]: it answers "does
    /// this index still describe what is on disk?" for an operator who needs
    /// the answer before deciding to rebuild. It uses the same size-and-mtime
    /// shortcut, so a store that is genuinely current costs no file reads;
    /// only entries the shortcut flags are opened and hashed, which is what
    /// keeps a false "unchanged" from being reported for a file whose mtime
    /// moved but whose bytes did not.
    ///
    /// # Errors
    ///
    /// - [`RepoMapError::Root`] when the root can no longer be canonicalised.
    /// - [`RepoMapError::Store`] on any database failure.
    pub fn verify(&self, opts: &IndexOptions) -> Result<VerifyReport, RepoMapError> {
        let entries = scope_files(&self.root, opts)?;
        let mut rows = self.load_index_rows()?;
        let mut report = VerifyReport {
            records: rows.len() as u64,
            in_scope: entries.len() as u64,
            scope_drifted: self.scope_drifted()?,
            ..VerifyReport::default()
        };

        for entry in &entries {
            match rows.remove(&entry.key) {
                None => report.missing_from_store += 1,
                Some(row) => {
                    if row.size_bytes == entry.size_bytes
                        && row.mtime_unix_nanos == entry.mtime_unix_nanos
                    {
                        continue;
                    }
                    // The cheap check disagreed; only the hash can say
                    // whether the CONTENT actually moved.
                    let current = if entry.size_bytes > opts.max_file_bytes {
                        oversize_hash(entry.size_bytes)
                    } else {
                        match hash_file_on_disk(&entry.abs_path) {
                            Ok(hash) => hash,
                            Err(_) => {
                                report.missing_from_disk += 1;
                                continue;
                            }
                        }
                    };
                    if current != row.content_hash {
                        report.changed += 1;
                    }
                }
            }
        }
        report.missing_from_disk += rows.len() as u64;
        Ok(report)
    }

    fn load_index_rows(&self) -> Result<HashMap<String, IndexRow>, RepoMapError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, content_hash, size_bytes, mtime_unix_nanos FROM files")
            .map_err(|e| store_err(&self.store_path, &e))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(IndexRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    content_hash: r.get(2)?,
                    size_bytes: r.get::<_, i64>(3)? as u64,
                    mtime_unix_nanos: r.get::<_, String>(4)?.parse().unwrap_or(0),
                })
            })
            .map_err(|e| store_err(&self.store_path, &e))?;
        let mut map = HashMap::new();
        for row in rows {
            let row = row.map_err(|e| store_err(&self.store_path, &e))?;
            map.insert(row.path.clone(), row);
        }
        Ok(map)
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>, RepoMapError> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| store_err(&self.store_path, &e))
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<(), RepoMapError> {
        self.conn
            .execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(|e| store_err(&self.store_path, &e))
    }
}

#[derive(Debug, Clone)]
struct IndexRow {
    id: i64,
    path: String,
    content_hash: String,
    size_bytes: u64,
    mtime_unix_nanos: u128,
}

/// A file read, hashed and (when indexable) extracted, ready to be written.
struct PreparedFile {
    key: String,
    content_hash: String,
    size_bytes: u64,
    mtime_unix_nanos: u128,
    lines: usize,
    language: Language,
    indexable: bool,
    content: String,
    symbols: Vec<Symbol>,
}

/// Read, hash and (when in bounds) extract one file. Every open of a file's
/// bytes on the refresh path goes through here, so `stats.files_read` is a
/// complete count by construction rather than by discipline.
fn prepare_file(
    entry: &ScopeEntry,
    opts: &IndexOptions,
    stats: &mut IndexStats,
) -> Result<PreparedFile, RepoMapError> {
    stats.files_read += 1;

    if entry.size_bytes > opts.max_file_bytes {
        // Matches `RepoMap::build`: an oversized file is recorded by size
        // alone, with no text stored and no symbols. Its text is never read,
        // so it can never contribute content to a query either.
        return Ok(PreparedFile {
            key: entry.key.clone(),
            content_hash: oversize_hash(entry.size_bytes),
            size_bytes: entry.size_bytes,
            mtime_unix_nanos: entry.mtime_unix_nanos,
            lines: 0,
            language: Language::Other,
            indexable: false,
            content: String::new(),
            symbols: Vec::new(),
        });
    }

    let bytes = read_file(&entry.abs_path)?;
    let content_hash = hex_sha256(&bytes);
    let language = Language::from_path(Path::new(&entry.key));

    let Ok(text) = std::str::from_utf8(&bytes) else {
        // Non-UTF-8: hashed and tracked, but no text stored and no symbols —
        // the same stance `RepoMap::build` already takes.
        return Ok(PreparedFile {
            key: entry.key.clone(),
            content_hash,
            size_bytes: entry.size_bytes,
            mtime_unix_nanos: entry.mtime_unix_nanos,
            lines: 0,
            language: Language::Other,
            indexable: false,
            content: String::new(),
            symbols: Vec::new(),
        });
    };

    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count()
    };
    if lines > opts.max_lines {
        return Ok(PreparedFile {
            key: entry.key.clone(),
            content_hash,
            size_bytes: entry.size_bytes,
            mtime_unix_nanos: entry.mtime_unix_nanos,
            lines,
            language,
            indexable: false,
            content: String::new(),
            symbols: Vec::new(),
        });
    }

    let (symbols, _imports) = extract(language, text);
    Ok(PreparedFile {
        key: entry.key.clone(),
        content_hash,
        size_bytes: entry.size_bytes,
        mtime_unix_nanos: entry.mtime_unix_nanos,
        lines,
        language,
        indexable: true,
        content: text.to_string(),
        symbols,
    })
}

fn write_file_row(
    tx: &rusqlite::Transaction<'_>,
    existing_id: Option<i64>,
    prepared: &PreparedFile,
    now: u64,
    store_path: &Path,
) -> Result<i64, RepoMapError> {
    let id = match existing_id {
        Some(id) => {
            tx.execute(
                "UPDATE files SET path = ?1, content_hash = ?2, size_bytes = ?3,
                                  lines = ?4, language = ?5, mtime_unix_nanos = ?6,
                                  indexed_at_unix_secs = ?7, indexable = ?8
                 WHERE id = ?9",
                params![
                    prepared.key,
                    prepared.content_hash,
                    prepared.size_bytes as i64,
                    prepared.lines as i64,
                    language_as_str(prepared.language),
                    prepared.mtime_unix_nanos.to_string(),
                    now as i64,
                    i64::from(prepared.indexable),
                    id
                ],
            )
            .map_err(|e| store_err(store_path, &e))?;
            tx.execute("DELETE FROM symbols WHERE file_id = ?1", params![id])
                .map_err(|e| store_err(store_path, &e))?;
            tx.execute("DELETE FROM files_fts WHERE rowid = ?1", params![id])
                .map_err(|e| store_err(store_path, &e))?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO files(path, content_hash, size_bytes, lines, language,
                                   mtime_unix_nanos, indexed_at_unix_secs, indexable)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    prepared.key,
                    prepared.content_hash,
                    prepared.size_bytes as i64,
                    prepared.lines as i64,
                    language_as_str(prepared.language),
                    prepared.mtime_unix_nanos.to_string(),
                    now as i64,
                    i64::from(prepared.indexable),
                ],
            )
            .map_err(|e| store_err(store_path, &e))?;
            tx.last_insert_rowid()
        }
    };

    if prepared.indexable {
        tx.execute(
            "INSERT INTO files_fts(rowid, path, content) VALUES(?1, ?2, ?3)",
            params![id, prepared.key, prepared.content],
        )
        .map_err(|e| store_err(store_path, &e))?;
        let mut stmt = tx
            .prepare("INSERT INTO symbols(file_id, kind, name, line) VALUES(?1, ?2, ?3, ?4)")
            .map_err(|e| store_err(store_path, &e))?;
        for symbol in &prepared.symbols {
            stmt.execute(params![
                id,
                symbol_kind_as_str(symbol.kind),
                symbol.name,
                symbol.line as i64
            ])
            .map_err(|e| store_err(store_path, &e))?;
        }
    }
    Ok(id)
}

fn read_file(path: &Path) -> Result<Vec<u8>, RepoMapError> {
    let mut file = File::open(path).map_err(|e| RepoMapError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| RepoMapError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(bytes)
}

/// Hex SHA-256 of `bytes`. Used as the content-invalidation key.
pub fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Hex SHA-256 of a file's current bytes on disk, for staleness comparison.
///
/// # Errors
///
/// [`RepoMapError::Io`] when the file cannot be read.
pub fn hash_file_on_disk(path: &Path) -> Result<String, RepoMapError> {
    Ok(hex_sha256(&read_file(path)?))
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn store_err(path: &Path, err: &rusqlite::Error) -> RepoMapError {
    RepoMapError::Store {
        path: path.to_path_buf(),
        message: format!("{err}; rebuild the index if this file is corrupt"),
    }
}

pub(crate) fn language_as_str(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Other => "other",
    }
}

pub(crate) fn language_from_str(s: &str) -> Language {
    match s {
        "rust" => Language::Rust,
        "typescript" => Language::TypeScript,
        "javascript" => Language::JavaScript,
        _ => Language::Other,
    }
}

pub(crate) fn symbol_kind_as_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Impl => "impl",
        SymbolKind::Module => "module",
        SymbolKind::Use => "use",
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::TypeAlias => "type_alias",
        SymbolKind::Export => "export",
    }
}

pub(crate) fn symbol_kind_from_str(s: &str) -> SymbolKind {
    match s {
        "struct" => SymbolKind::Struct,
        "enum" => SymbolKind::Enum,
        "trait" => SymbolKind::Trait,
        "impl" => SymbolKind::Impl,
        "module" => SymbolKind::Module,
        "use" => SymbolKind::Use,
        "class" => SymbolKind::Class,
        "interface" => SymbolKind::Interface,
        "type_alias" => SymbolKind::TypeAlias,
        "export" => SymbolKind::Export,
        _ => SymbolKind::Function,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_kind_and_language_encodings_round_trip() {
        // A wrong encoding here silently mislabels every stored symbol, and
        // the retrieval tests would still pass because they never compare the
        // kind. Pin it directly.
        for kind in [
            SymbolKind::Function,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Trait,
            SymbolKind::Impl,
            SymbolKind::Module,
            SymbolKind::Use,
            SymbolKind::Class,
            SymbolKind::Interface,
            SymbolKind::TypeAlias,
            SymbolKind::Export,
        ] {
            assert_eq!(symbol_kind_from_str(symbol_kind_as_str(kind)), kind);
        }
        for language in [
            Language::Rust,
            Language::TypeScript,
            Language::JavaScript,
            Language::Other,
        ] {
            assert_eq!(language_from_str(language_as_str(language)), language);
        }
    }

    #[test]
    fn hex_sha256_matches_the_known_empty_digest() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn oversize_hash_changes_with_size() {
        assert_ne!(oversize_hash(10), oversize_hash(11));
    }
}
