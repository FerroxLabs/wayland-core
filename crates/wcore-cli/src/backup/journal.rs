//! The write-ahead operation journal and its recovery pass (F26-03).
//!
//! # Why this shape and not another
//!
//! [`crate::crash_sentinel`] already settled how this codebase detects an
//! unclean exit, and #181 recorded what happens when you get it wrong: an
//! UNSCOPED marker made every concurrent engine report a false crash. A journal
//! has precisely the same hazard — an unscoped record would make one process's
//! recovery undo another process's in-flight work — so every record here is
//! scoped per operation AND per owning process, and recovery acts only on records
//! whose owner is DEAD, using the same [`crate::cron::process_is_alive`] probe
//! the sentinel uses.
//!
//! # Ordering is the whole property
//!
//! The intent record is durable BEFORE the target is touched. A journal written
//! after the fact records history, not intent, and cannot roll anything back.
//! Every record is written through [`wcore_config::atomic_io::atomic_write`], so
//! a torn record cannot itself become the corruption the journal exists to
//! prevent.
//!
//! # Why the digest still matches after a rollback
//!
//! The journal lives at `<home>/.wayland-backup-journal/`. A home that has never
//! had an operation run against it has no such directory, so its pre-operation
//! digest does not include one. Recovery therefore restores the prior content
//! AND removes the journal directory once its last record is closed — leaving a
//! tree that digests identically to the pre-operation tree rather than one that
//! differs by the bookkeeping.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::BackupError;

/// Journal directory under the home being operated on.
pub const JOURNAL_DIR: &str = ".wayland-backup-journal";

/// True for a name that is Wayland's own bookkeeping rather than user state.
///
/// Two things live in a home that a user never put there and never notices:
/// this module's journal directory, and [`crate::crash_sentinel`]'s
/// `.dirty-death.<pid>` markers. Both must be invisible to a home's tree digest
/// for the same reason — measured 2026-07-29, when a rolled-back import came
/// back with EVERY user file byte-identical and the digest still differed,
/// solely because the killed process had left its own crash marker behind.
///
/// Getting this wrong is not a cosmetic error. The digest is the comparand that
/// SC3's "restore exact pre-operation state" is judged on, so bookkeeping inside
/// it would make an exact rollback report as inexact, and the only way to reach
/// a green would have been to weaken the assertion.
///
/// The constants are imported, never re-spelled: a second copy of
/// `".dirty-death."` here is a constant that can drift from the module that
/// writes it.
pub fn is_bookkeeping(name: &std::ffi::OsStr) -> bool {
    if name == std::ffi::OsStr::new(JOURNAL_DIR) {
        return true;
    }
    let Some(s) = name.to_str() else {
        return false;
    };
    s == crate::crash_sentinel::FLAG_FILE || s.starts_with(crate::crash_sentinel::PID_FLAG_PREFIX)
}

/// One mutating operation's write-ahead record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpRecord {
    /// Unique per operation.
    pub op_id: String,
    /// The owning process. Recovery acts only when this pid is dead — the #181
    /// rule, transplanted.
    pub pid: u32,
    pub kind: String,
    pub started_utc: String,
    /// The tree being mutated.
    pub target: String,
    /// Digest of `target` before the first mutation, for the exactness assertion.
    pub pre_digest: String,
    /// False until the prior tree has been fully captured into the undo store.
    /// While false the target has NOT been touched, so there is nothing to undo.
    pub preserved: bool,
    /// The paths, relative to `target`, that this operation may mutate.
    ///
    /// Empty means the WHOLE tree — the restore case, where the operation
    /// replaces everything and the undo store is a full copy.
    ///
    /// A non-empty scope is what makes this journal usable by an operation that
    /// touches a bounded part of a home it must otherwise not disturb. `migrate`
    /// is that case: it writes `quarantine/` and `config.toml` into a live
    /// Wayland home that also holds `memory.db`, sessions and assets, and
    /// copying all of those on every import would be an unacceptable price for
    /// rollback — while restoring all of them on a rollback would silently undo
    /// unrelated concurrent work.
    ///
    /// `#[serde(default)]` so a record written before scoping existed still
    /// deserializes, and reads as the whole-tree operation it was.
    #[serde(default)]
    pub scope: Vec<String>,
    /// Still open. A completed operation deletes its record entirely, so an
    /// existing file is itself the open signal; the field keeps a record
    /// self-describing when read by a human or a proof script.
    pub open: bool,
}

/// What a recovery pass did.
#[derive(Debug, Clone, Default)]
pub struct RecoveryReport {
    pub recovered: usize,
    /// Records left alone because their owner is still running.
    pub skipped_live_owner: usize,
    pub op_ids: Vec<String>,
}

/// An in-flight operation. Drop without [`OpGuard::commit`] leaves the record
/// open on purpose — that is what a crash looks like, and what recovery finds.
#[derive(Debug)]
pub struct OpGuard {
    root: PathBuf,
    record_path: PathBuf,
    undo_dir: PathBuf,
    record: OpRecord,
}

/// Begin a WHOLE-TREE operation against `target`.
///
/// Writes the durable intent record BEFORE the caller touches anything.
pub fn begin(target: &Path, kind: &str) -> Result<OpGuard, BackupError> {
    begin_scoped(target, kind, &[])
}

/// Begin an operation that may mutate only `scope` (paths relative to
/// `target`). An empty scope is the whole tree, i.e. [`begin`].
///
/// The scope is declared UP FRONT and recorded durably, because it is what a
/// recovery pass in some later process — which has no idea what the operation
/// was — needs in order to undo the right thing and nothing else.
///
/// `pre_digest` is scoped to match: comparing a whole-home digest would make the
/// record's own exactness check fail whenever anything else in the home changed
/// while the import ran, which is not this operation's business. The whole-home
/// comparison is still available to a caller or a proof harness through
/// [`target_digest`], and it is the stronger claim precisely because it also
/// catches a write OUTSIDE the declared scope.
pub fn begin_scoped(target: &Path, kind: &str, scope: &[&str]) -> Result<OpGuard, BackupError> {
    for rel in scope {
        reject_escaping_scope(rel)?;
    }
    let root = target.join(JOURNAL_DIR);
    std::fs::create_dir_all(&root).map_err(BackupError::io("create journal dir"))?;

    let pid = std::process::id();
    let stamp = chrono::Utc::now().timestamp_millis();
    let op_id = format!("{stamp}-{pid}");
    let record_path = root.join(format!("{op_id}.{pid}.json"));
    let undo_dir = root.join(format!("undo-{op_id}"));

    // Digest BEFORE anything exists in the journal beyond the directory itself,
    // and exclude the journal directory so the value is comparable with a tree
    // that has no journal at all.
    let pre_digest = if scope.is_empty() {
        digest_excluding_journal(target)?
    } else {
        scoped_digest(target, scope)?
    };

    let record = OpRecord {
        op_id: op_id.clone(),
        pid,
        kind: kind.to_string(),
        started_utc: chrono::Utc::now().to_rfc3339(),
        target: target.display().to_string(),
        pre_digest,
        preserved: false,
        open: true,
        scope: scope.iter().map(|s| (*s).to_string()).collect(),
    };
    write_record(&record_path, &record)?;

    Ok(OpGuard {
        root,
        record_path,
        undo_dir,
        record,
    })
}

impl OpGuard {
    pub fn op_id(&self) -> &str {
        &self.record.op_id
    }

    pub fn pre_digest(&self) -> &str {
        &self.record.pre_digest
    }

    pub fn undo_dir(&self) -> &Path {
        &self.undo_dir
    }

    /// Capture the ENTIRE prior tree into the undo store, then mark the record
    /// preserved. Only after this returns may the caller mutate the target.
    ///
    /// Capturing the whole tree — rather than only the paths the operation
    /// intends to write — is what makes the rollback exact for a replace: an
    /// entry the new tree does not contain is still restored, instead of being
    /// silently lost because nothing planned to overwrite it.
    pub fn preserve_target(&mut self, target: &Path) -> Result<(), BackupError> {
        std::fs::create_dir_all(&self.undo_dir).map_err(BackupError::io("create undo dir"))?;
        if self.record.scope.is_empty() {
            copy_tree_excluding_journal(target, &self.undo_dir)?;
        } else {
            preserve_scope(target, &self.undo_dir, &self.record.scope)?;
        }
        self.record.preserved = true;
        write_record(&self.record_path, &self.record)
    }

    /// The declared scope, empty for a whole-tree operation.
    pub fn scope(&self) -> &[String] {
        &self.record.scope
    }

    /// Undo this operation to the exact pre-operation tree.
    pub fn rollback(&mut self, target: &Path) -> Result<(), BackupError> {
        if self.record.preserved {
            undo(target, &self.undo_dir, &self.record.scope)?;
        }
        self.close()
    }

    /// The operation completed: remove the record and its undo store, so a
    /// completed operation leaves NO open record.
    pub fn commit(mut self) -> Result<(), BackupError> {
        self.close()
    }

    fn close(&mut self) -> Result<(), BackupError> {
        let _ = std::fs::remove_file(&self.record_path);
        let _ = std::fs::remove_dir_all(&self.undo_dir);
        prune_journal_root(&self.root);
        Ok(())
    }
}

/// Records currently open under `target`'s journal, oldest first.
pub fn list_open(target: &Path) -> Vec<OpRecord> {
    let root = target.join(JOURNAL_DIR);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out: Vec<OpRecord> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x == "json")
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|s| serde_json::from_str::<OpRecord>(&s).ok())
        .collect();
    out.sort_by(|a, b| a.op_id.cmp(&b.op_id));
    out
}

/// Roll back every operation under `target` whose owning process is dead.
///
/// Idempotent: a second pass finds no records and changes nothing. A record
/// whose owner is still alive is never touched — the crash sentinel's dead-pid
/// rule, and the reason #181 exists.
pub fn recover(target: &Path) -> Result<RecoveryReport, BackupError> {
    recover_with(target, crate::cron::process_is_alive)
}

/// Inner recovery with an injectable liveness probe, so the live-owner rule is
/// testable without real process churn.
pub fn recover_with(
    target: &Path,
    is_alive: impl Fn(u32) -> bool,
) -> Result<RecoveryReport, BackupError> {
    let root = target.join(JOURNAL_DIR);
    let mut report = RecoveryReport::default();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Ok(report);
    };

    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| x == "json")
        })
        .collect();
    // Reverse order: the newest operation is undone first, so overlapping
    // operations unwind in the order they were applied.
    paths.sort();
    paths.reverse();

    for path in paths {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<OpRecord>(&text) else {
            continue;
        };

        if is_alive(record.pid) {
            report.skipped_live_owner += 1;
            continue;
        }

        let undo_dir = root.join(format!("undo-{}", record.op_id));
        if record.preserved {
            undo(target, &undo_dir, &record.scope)?;
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&undo_dir);
        report.recovered += 1;
        report.op_ids.push(record.op_id);
    }

    prune_journal_root(&root);
    Ok(report)
}

fn write_record(path: &Path, record: &OpRecord) -> Result<(), BackupError> {
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| BackupError::Journal(format!("serialize record: {e}")))?;
    wcore_config::atomic_io::atomic_write(path, &bytes)
        .map_err(BackupError::io("write journal record"))
}

/// Undo an operation, honouring its declared scope.
fn undo(target: &Path, undo_dir: &Path, scope: &[String]) -> Result<(), BackupError> {
    if !undo_dir.is_dir() {
        return Ok(());
    }
    if scope.is_empty() {
        restore_from_undo(target, undo_dir)
    } else {
        restore_scope(target, undo_dir, scope)
    }
}

/// Replace `target`'s content with the preserved copy, exactly.
fn restore_from_undo(target: &Path, undo_dir: &Path) -> Result<(), BackupError> {
    if !undo_dir.is_dir() {
        return Ok(());
    }
    clear_tree_excluding_journal(target)?;
    copy_tree_all(undo_dir, target)
}

/// The marker recording which scope entries did NOT exist before the operation.
///
/// Without it a rollback cannot tell "this path was preserved as empty" from
/// "this path did not exist", and the difference is the whole property: an
/// import that CREATES `quarantine/` must have that directory removed on
/// rollback, not left behind as an empty shell that changes the home's digest.
const ABSENT_MARKER: &str = "absent.json";

/// Copy each scope entry that exists into the undo store, and record the ones
/// that do not.
fn preserve_scope(target: &Path, undo_dir: &Path, scope: &[String]) -> Result<(), BackupError> {
    std::fs::create_dir_all(undo_dir).map_err(BackupError::io("create undo dir"))?;
    let mut absent: Vec<String> = Vec::new();
    for rel in scope {
        reject_escaping_scope(rel)?;
        let from = target.join(rel);
        let meta = match from.symlink_metadata() {
            Ok(m) => m,
            Err(_) => {
                absent.push(rel.clone());
                continue;
            }
        };
        let to = undo_dir.join(scope_store_name(rel));
        if meta.is_dir() && !meta.file_type().is_symlink() {
            // A live subtree: databases nested inside a scoped directory get
            // the same consistent capture as the whole-tree path.
            capture_tree(&from, &to)?;
        } else if meta.is_file() {
            if wcore_config::sqlite_snapshot::is_sqlite_database(&from) {
                wcore_config::sqlite_snapshot::snapshot_database(&from, &to)
                    .map_err(|e| BackupError::SqliteCapture(format!("{rel}: {e}")))?;
            } else {
                std::fs::copy(&from, &to).map_err(BackupError::io("preserve scoped file"))?;
            }
        } else {
            // A symlink at a scope root is not something this operation may
            // mutate blind, and copying it would not round-trip. Record it as
            // absent so rollback removes whatever replaced it, rather than
            // silently materializing a copy of its destination.
            absent.push(rel.clone());
        }
    }
    let bytes = serde_json::to_vec(&absent)
        .map_err(|e| BackupError::Journal(format!("serialize absent set: {e}")))?;
    wcore_config::atomic_io::atomic_write(undo_dir.join(ABSENT_MARKER), &bytes)
        .map_err(BackupError::io("write absent marker"))
}

/// Restore exactly the declared scope and nothing else.
fn restore_scope(target: &Path, undo_dir: &Path, scope: &[String]) -> Result<(), BackupError> {
    let absent: Vec<String> = std::fs::read(undo_dir.join(ABSENT_MARKER))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    for rel in scope {
        reject_escaping_scope(rel)?;
        let dest = target.join(rel);
        // Remove whatever the operation left there, whether it existed before
        // or not. An entry the operation CREATED must go; an entry it edited
        // must be replaced wholesale rather than merged, or a file the
        // operation added inside a preserved directory would survive.
        remove_any(&dest)?;
        if absent.iter().any(|a| a == rel) {
            continue;
        }
        let from = undo_dir.join(scope_store_name(rel));
        let Ok(meta) = from.symlink_metadata() else {
            continue;
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(BackupError::io("create scope parent"))?;
        }
        if meta.is_dir() {
            copy_tree_all(&from, &dest)?;
        } else {
            std::fs::copy(&from, &dest).map_err(BackupError::io("restore scoped file"))?;
            // A preserved database is a FOLDED capture with no sidecars. The
            // sidecars that stood beside the original are not in scope, so
            // nothing else removes them — and a stale `-wal` left next to a
            // rolled-back database is itself a corruption vector, because
            // SQLite will replay it over a file it does not belong to.
            //
            // Unreachable with today's `MIGRATE_SCOPE` (quarantine,
            // migrate-imported, skills, config.toml — no database among them).
            // Handled anyway: the cost is three `remove_file` calls and the
            // alternative is a silent corruption the day something adds one.
            if wcore_config::sqlite_snapshot::is_sqlite_database(&from) {
                for suffix in wcore_config::sqlite_snapshot::DERIVED_SIDECAR_SUFFIXES {
                    let side = PathBuf::from(format!("{}{suffix}", dest.display()));
                    if side.exists() {
                        std::fs::remove_file(&side)
                            .map_err(BackupError::io("remove stale sidecar on rollback"))?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Digest of just the declared scope, computed through the same tree digest the
/// whole-tree path uses so the two are the same arithmetic.
pub fn scoped_digest(target: &Path, scope: &[&str]) -> Result<String, BackupError> {
    let shadow = tempfile::tempdir().map_err(BackupError::io("scope shadow dir"))?;
    let owned: Vec<String> = scope.iter().map(|s| (*s).to_string()).collect();
    preserve_scope(target, shadow.path(), &owned)?;
    Ok(wcore_config::portability::tree_digest(shadow.path())
        .map_err(BackupError::io("digest scope shadow"))?
        .digest)
}

/// A scope entry is a path INSIDE the target. Anything that could climb out of
/// it — an absolute path, a `..` component, a Windows drive prefix — is refused
/// at declaration time, because a scope is also what a later recovery pass in
/// another process will delete and rewrite.
fn reject_escaping_scope(rel: &str) -> Result<(), BackupError> {
    use std::path::Component;
    if rel.is_empty() {
        return Err(BackupError::Journal("empty scope entry".to_string()));
    }
    let p = Path::new(rel);
    for c in p.components() {
        match c {
            Component::Normal(_) => {}
            _ => {
                return Err(BackupError::Journal(format!(
                    "scope entry must be a relative path inside the target: {rel}"
                )));
            }
        }
    }
    Ok(())
}

/// Flatten a scope entry to one undo-store name, so a nested scope entry does
/// not collide with a preserved directory of the same prefix.
fn scope_store_name(rel: &str) -> String {
    rel.replace(['/', '\\'], "%2F")
}

fn remove_any(path: &Path) -> Result<(), BackupError> {
    let Ok(meta) = path.symlink_metadata() else {
        return Ok(());
    };
    if meta.is_dir() && !meta.file_type().is_symlink() {
        std::fs::remove_dir_all(path).map_err(BackupError::io("remove scoped dir"))
    } else {
        std::fs::remove_file(path).map_err(BackupError::io("remove scoped file"))
    }
}

/// Remove the journal directory once no records remain, so a recovered tree
/// digests identically to one that never carried a journal.
fn prune_journal_root(root: &Path) {
    let empty_of_records = std::fs::read_dir(root)
        .map(|mut it| it.all(|e| e.is_err()))
        .unwrap_or(true);
    if empty_of_records {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// Digest of `target` ignoring Wayland's own bookkeeping ([`is_bookkeeping`]).
///
/// Reuses 26-01's `tree_digest` rather than growing a second digest: the
/// bookkeeping is filtered out through a shadow copy only when some is present,
/// which is never the case for a first operation on a clean home.
fn digest_excluding_journal(target: &Path) -> Result<String, BackupError> {
    let has_bookkeeping = std::fs::read_dir(target)
        .map(|it| it.flatten().any(|e| is_bookkeeping(&e.file_name())))
        .unwrap_or(false);
    if !has_bookkeeping {
        return Ok(wcore_config::portability::tree_digest(target)
            .map_err(BackupError::io("digest target tree"))?
            .digest);
    }
    // Digest a filtered shadow so the value stays comparable with a home that
    // carries no bookkeeping at all.
    let shadow = tempfile::tempdir().map_err(BackupError::io("shadow dir"))?;
    copy_tree_excluding_journal(target, shadow.path())?;
    Ok(wcore_config::portability::tree_digest(shadow.path())
        .map_err(BackupError::io("digest shadow tree"))?
        .digest)
}

/// Public digest used by the tests and the interruption proof, so both sides of
/// the exactness comparison are computed the same way.
pub fn target_digest(target: &Path) -> Result<String, BackupError> {
    digest_excluding_journal(target)
}

/// Which direction a tree copy is running in, and therefore how it must treat a
/// SQLite database it meets.
///
/// The two directions are NOT symmetric, which is why this is an explicit
/// parameter rather than a blanket "snapshot any database you see":
///
/// * [`SqliteMode::Capture`] reads the user's LIVE home. A database there may
///   have a writer committing into it, so it must be captured through
///   [`wcore_config::sqlite_snapshot`] — see below.
/// * [`SqliteMode::Verbatim`] reads the undo store, which is a quiescent copy
///   nothing is writing. Re-snapshotting it would be pointless work, and worse:
///   it would open a read-write connection against a store whose whole job is
///   to be handed back unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteMode {
    Capture,
    Verbatim,
}

fn copy_tree_excluding_journal(src: &Path, dst: &Path) -> Result<(), BackupError> {
    copy_inner(src, dst, true, SqliteMode::Capture)
}

fn copy_tree_all(src: &Path, dst: &Path) -> Result<(), BackupError> {
    copy_inner(src, dst, false, SqliteMode::Verbatim)
}

/// Capture a live subtree into the undo store.
fn capture_tree(src: &Path, dst: &Path) -> Result<(), BackupError> {
    copy_inner(src, dst, false, SqliteMode::Capture)
}

/// Names in one directory that are SQLite databases, and the derived sidecars
/// belonging to them.
///
/// Both answers are computed for the WHOLE directory before anything is read,
/// for the reason `archive::SqliteCapturePlan` documents: a `-wal` is only a
/// sidecar if the file it names is genuinely a database, so deciding per-entry
/// inside the loop would either drop an unrelated file ending in `-wal`, or
/// carry a real sidecar whose database sorted after it.
fn sqlite_plan(
    files: &[(String, PathBuf)],
) -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
) {
    use wcore_config::sqlite_snapshot::{is_derived_sidecar_of, is_sqlite_database};

    let databases: std::collections::BTreeSet<String> = files
        .iter()
        .filter(|(_, path)| is_sqlite_database(path))
        .map(|(name, _)| name.clone())
        .collect();
    let sidecars: std::collections::BTreeSet<String> = files
        .iter()
        .map(|(name, _)| name)
        .filter(|name| {
            !databases.contains(*name) && databases.iter().any(|db| is_derived_sidecar_of(name, db))
        })
        .cloned()
        .collect();
    (databases, sidecars)
}

fn copy_inner(
    src: &Path,
    dst: &Path,
    skip_journal: bool,
    sqlite: SqliteMode,
) -> Result<(), BackupError> {
    std::fs::create_dir_all(dst).map_err(BackupError::io("create copy dir"))?;
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    // Collect first, decide second, copy third. The undo store is the ONLY
    // record of the user's prior home, so the SQLite question has to be settled
    // across the whole directory before any byte of it is read.
    let mut dirs: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut opaque: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(BackupError::io("read copy entry"))?;
        let name = entry.file_name();
        if skip_journal && is_bookkeeping(&name) {
            continue;
        }
        let from = entry.path();
        let meta = from
            .symlink_metadata()
            .map_err(BackupError::io("stat copy entry"))?;
        // Never follow a link out of the tree, matching the archive walk.
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            dirs.push((name, from));
        } else if meta.is_file() {
            match name.to_str() {
                Some(s) => files.push((s.to_string(), from)),
                // A non-UTF-8 name cannot be compared against a database name,
                // so it can be neither a database we capture nor a sidecar we
                // drop. Copied verbatim, which is what it was before.
                None => opaque.push((name, from)),
            }
        }
    }

    let (databases, sidecars) = match sqlite {
        SqliteMode::Capture => sqlite_plan(&files),
        SqliteMode::Verbatim => Default::default(),
    };

    for (name, from) in dirs {
        copy_inner(&from, &dst.join(&name), false, sqlite)?;
    }
    for (name, from) in opaque {
        std::fs::copy(&from, dst.join(&name)).map_err(BackupError::io("copy file"))?;
    }
    for (name, from) in &files {
        // A derived sidecar is not carried: the capture has already absorbed
        // everything it contains, and a restored `-shm` is a wal-index owned by
        // a process that no longer exists.
        if sidecars.contains(name) {
            continue;
        }
        let to = dst.join(name);
        if databases.contains(name) {
            // Failure REFUSES the operation. Falling back to `fs::copy` here
            // would silently reinstate the defect this exists to close, behind
            // an undo store now claiming to hold a consistent home.
            wcore_config::sqlite_snapshot::snapshot_database(from, &to)
                .map_err(|e| BackupError::SqliteCapture(format!("{}: {e}", from.display())))?;
        } else {
            std::fs::copy(from, &to).map_err(BackupError::io("copy file"))?;
        }
    }
    Ok(())
}

fn clear_tree_excluding_journal(target: &Path) -> Result<(), BackupError> {
    let entries = match std::fs::read_dir(target) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry.map_err(BackupError::io("read clear entry"))?;
        // Bookkeeping is not this operation's to destroy. A live process's
        // crash marker in particular belongs to that process, and deleting it
        // would suppress a crash report the operator is entitled to.
        if is_bookkeeping(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let meta = path
            .symlink_metadata()
            .map_err(BackupError::io("stat clear entry"))?;
        if meta.is_dir() && !meta.file_type().is_symlink() {
            std::fs::remove_dir_all(&path).map_err(BackupError::io("remove dir"))?;
        } else {
            std::fs::remove_file(&path).map_err(BackupError::io("remove file"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(target: &Path) {
        std::fs::create_dir_all(target.join("skills")).unwrap();
        std::fs::write(target.join("config.toml"), "original = true").unwrap();
        std::fs::write(target.join("skills/a.md"), "prior-a").unwrap();
    }

    #[test]
    fn intent_is_durable_before_the_target_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let before = target_digest(&target).unwrap();

        let guard = begin(&target, "restore").unwrap();

        // The record exists and is readable the instant begin() returns, i.e.
        // before any caller has had the chance to mutate anything.
        let open = list_open(&target);
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].op_id, guard.op_id());
        assert!(open[0].open);
        assert!(
            !open[0].preserved,
            "nothing is preserved yet, so nothing was touched"
        );
        assert_eq!(open[0].pre_digest, before);

        // And the target itself is untouched by begin().
        assert_eq!(target_digest(&target).unwrap(), before);
    }

    #[test]
    fn a_completed_operation_leaves_no_open_record() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        assert_eq!(list_open(&target).len(), 1, "open while in flight");
        guard.commit().unwrap();

        assert!(
            list_open(&target).is_empty(),
            "a committed op left a record"
        );
        assert!(
            !target.join(JOURNAL_DIR).exists(),
            "the journal directory must not survive the last record"
        );
    }

    #[test]
    fn recovery_undoes_a_dead_owners_operation_to_the_exact_pre_operation_tree() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let pre = target_digest(&target).unwrap();

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();

        // Simulate a half-applied replace: overwrite one file, add another,
        // delete a third. This is the state a kill leaves behind.
        std::fs::write(target.join("config.toml"), "MUTATED").unwrap();
        std::fs::write(target.join("new-file.txt"), "added").unwrap();
        std::fs::remove_file(target.join("skills/a.md")).unwrap();
        std::mem::forget(guard); // the owner "died" without committing
        assert_ne!(
            target_digest(&target).unwrap(),
            pre,
            "the tree really did change"
        );

        // Owner is dead.
        let report = recover_with(&target, |_| false).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(
            target_digest(&target).unwrap(),
            pre,
            "rollback did not reproduce the exact pre-operation tree"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("config.toml")).unwrap(),
            "original = true"
        );
        assert!(
            !target.join("new-file.txt").exists(),
            "an added file survived"
        );
        assert!(
            target.join("skills/a.md").exists(),
            "a deleted file was not restored"
        );
    }

    #[test]
    fn recovery_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let pre = target_digest(&target).unwrap();

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        std::fs::write(target.join("config.toml"), "MUTATED").unwrap();
        std::mem::forget(guard);

        let first = recover_with(&target, |_| false).unwrap();
        let after_first = target_digest(&target).unwrap();
        let second = recover_with(&target, |_| false).unwrap();
        let after_second = target_digest(&target).unwrap();

        assert_eq!(first.recovered, 1);
        assert_eq!(second.recovered, 0, "a second pass undid something again");
        assert_eq!(after_first, pre);
        assert_eq!(
            after_second, pre,
            "running recovery twice differed from once"
        );
    }

    #[test]
    fn recovery_never_touches_a_record_whose_owner_is_alive() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        std::fs::write(target.join("config.toml"), "IN-FLIGHT").unwrap();
        std::mem::forget(guard);

        // The #181 rule: a live owner's work is not somebody else's to undo.
        let report = recover_with(&target, |_| true).unwrap();
        assert_eq!(report.recovered, 0);
        assert_eq!(report.skipped_live_owner, 1);
        assert_eq!(
            std::fs::read_to_string(target.join("config.toml")).unwrap(),
            "IN-FLIGHT",
            "recovery undid an operation whose owner is still running"
        );
        assert_eq!(list_open(&target).len(), 1, "the live record was deleted");
    }

    #[test]
    fn a_record_is_scoped_per_operation_and_per_process() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);

        let g1 = begin(&target, "restore").unwrap();
        let g2 = begin(&target, "restore").unwrap();
        assert_ne!(g1.op_id(), g2.op_id(), "two operations shared one record");

        let names: Vec<String> = std::fs::read_dir(target.join(JOURNAL_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        assert_eq!(names.len(), 2);
        let pid = std::process::id();
        assert!(
            names.iter().all(|n| n.contains(&format!(".{pid}.json"))),
            "record names must carry the owning pid: {names:?}"
        );
    }

    /// The scoped case, which is what `migrate` needs: the operation may only
    /// touch `quarantine/` and `config.toml`, and a rollback must put those two
    /// back EXACTLY while leaving everything else in the home alone — including
    /// work that a concurrent, unrelated process did while the import ran.
    #[test]
    fn a_scoped_rollback_restores_its_scope_exactly_and_touches_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("quarantine/payloads/keep")).unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 0\n").unwrap();
        std::fs::write(home.join("quarantine/index.json"), "{\"entries\":{}}").unwrap();
        std::fs::write(home.join("quarantine/payloads/keep/a.md"), "prior").unwrap();
        // Out of scope, and expensive: this must never be copied or restored.
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::write(home.join("memory.db"), "PRIOR DB").unwrap();
        std::fs::write(home.join("sessions/s1.json"), "prior session").unwrap();

        let scope = ["quarantine", "config.toml"];
        let pre_all = target_digest(&home).unwrap();

        let mut guard = begin_scoped(&home, "migrate", &scope).unwrap();
        assert_eq!(guard.scope(), &["quarantine", "config.toml"]);
        guard.preserve_target(&home).unwrap();

        // The undo store holds the scope and NOTHING else.
        let stored: Vec<String> = std::fs::read_dir(guard.undo_dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !stored.iter().any(|n| n.contains("memory.db")),
            "an out-of-scope path was copied into the undo store: {stored:?}"
        );

        // A half-applied import: a payload written, the index rewritten, a
        // profile added, and a prior payload removed.
        std::fs::create_dir_all(home.join("quarantine/payloads/new")).unwrap();
        std::fs::write(home.join("quarantine/payloads/new/b.md"), "imported").unwrap();
        std::fs::write(
            home.join("quarantine/index.json"),
            "{\"entries\":{\"x\":1}}",
        )
        .unwrap();
        std::fs::remove_file(home.join("quarantine/payloads/keep/a.md")).unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 13\n").unwrap();
        // Meanwhile, an unrelated part of the home moves on.
        std::fs::write(home.join("memory.db"), "CONCURRENT DB").unwrap();

        std::mem::forget(guard); // the owner died mid-apply

        let report = recover_with(&home, |_| false).unwrap();
        assert_eq!(report.recovered, 1);

        // The scope is exactly back.
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "profiles = 0\n"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("quarantine/index.json")).unwrap(),
            "{\"entries\":{}}"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("quarantine/payloads/keep/a.md")).unwrap(),
            "prior",
            "a payload the interrupted import deleted was not put back"
        );
        assert!(
            !home.join("quarantine/payloads/new").exists(),
            "a payload the interrupted import created survived the rollback"
        );
        // And out-of-scope work was NOT reverted.
        assert_eq!(
            std::fs::read_to_string(home.join("memory.db")).unwrap(),
            "CONCURRENT DB",
            "a scoped rollback reverted an out-of-scope file it never owned"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("sessions/s1.json")).unwrap(),
            "prior session"
        );
        assert_ne!(
            target_digest(&home).unwrap(),
            pre_all,
            "premise check: the whole-home digest legitimately differs, because \
             the concurrent out-of-scope write is not this rollback's business"
        );
    }

    /// A scope entry that did not exist before must be REMOVED on rollback, not
    /// left behind as an empty shell. `quarantine/` is created by the first
    /// import, so this is the ordinary first-run case, and the home's digest
    /// only returns to its pre-operation value if the directory goes.
    #[test]
    fn a_scope_entry_created_by_the_operation_is_removed_on_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("memory.db"), "DB").unwrap();
        let pre = target_digest(&home).unwrap();
        assert!(!home.join("quarantine").exists());
        assert!(!home.join("config.toml").exists());

        let mut guard = begin_scoped(&home, "migrate", &["quarantine", "config.toml"]).unwrap();
        guard.preserve_target(&home).unwrap();
        std::fs::create_dir_all(home.join("quarantine/payloads/x")).unwrap();
        std::fs::write(home.join("quarantine/payloads/x/s.md"), "contained").unwrap();
        std::fs::write(home.join("quarantine/index.json"), "{}").unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 3\n").unwrap();
        std::mem::forget(guard);

        assert_eq!(recover_with(&home, |_| false).unwrap().recovered, 1);
        assert!(
            !home.join("quarantine").exists(),
            "a directory the operation created survived its rollback"
        );
        assert!(!home.join("config.toml").exists());
        assert_eq!(
            target_digest(&home).unwrap(),
            pre,
            "a first-run rollback did not return the home to its exact \
             pre-operation state"
        );
    }

    /// A scope entry is a path inside the target. Refusing at declaration time
    /// matters because the scope is later acted on by a DIFFERENT process, which
    /// deletes and rewrites every entry it names.
    #[test]
    fn a_scope_entry_that_could_escape_the_target_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        for bad in ["../elsewhere", "/etc/passwd", "a/../../b", ""] {
            let err = begin_scoped(&home, "migrate", &[bad]).unwrap_err();
            assert!(
                matches!(err, BackupError::Journal(_)),
                "scope entry {bad:?} was accepted: {err:?}"
            );
        }
        // Positive control: an ordinary nested entry IS accepted, so the check
        // above is not passing by refusing everything.
        let ok = begin_scoped(&home, "migrate", &["a/b/c"]).unwrap();
        assert_eq!(ok.scope(), &["a/b/c"]);
    }

    /// Measured on hetzner 2026-07-29: a rolled-back import came back with every
    /// user file byte-identical, and the digest still differed — solely because
    /// the SIGKILLed process had left `.dirty-death.<pid>` behind. The digest is
    /// the comparand SC3's "exact pre-operation state" is judged on, so
    /// bookkeeping inside it makes an exact rollback report as inexact.
    #[test]
    fn a_crash_marker_left_by_a_killed_process_is_not_part_of_the_home() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed(&home);
        let pre = target_digest(&home).unwrap();

        // Exactly what a killed process leaves: a per-pid crash marker.
        std::fs::write(home.join(".dirty-death.4242"), b"{}").unwrap();
        assert_eq!(
            target_digest(&home).unwrap(),
            pre,
            "a crash marker moved the home's digest"
        );
        // And the legacy un-scoped name too.
        std::fs::write(home.join(".dirty-death"), b"{}").unwrap();
        assert_eq!(target_digest(&home).unwrap(), pre);

        // The predicate must be able to say NO, or it would hide real files.
        assert!(!is_bookkeeping(std::ffi::OsStr::new("config.toml")));
        assert!(!is_bookkeeping(std::ffi::OsStr::new("dirty-death.1")));
        assert!(is_bookkeeping(std::ffi::OsStr::new(".dirty-death.1")));
        assert!(is_bookkeeping(std::ffi::OsStr::new(JOURNAL_DIR)));

        // A real user file with a similar-looking name still moves the digest,
        // so the exclusion is not swallowing user state.
        std::fs::write(home.join(".dirty-deathbed-notes.md"), b"user").unwrap();
        assert_ne!(
            target_digest(&home).unwrap(),
            pre,
            "the bookkeeping exclusion swallowed a user file"
        );
    }

    /// A record written before scoping existed must still deserialize, and must
    /// read as the whole-tree operation it was — otherwise an upgrade turns a
    /// pending rollback into a silent no-op.
    #[test]
    fn a_pre_scope_record_still_reads_as_a_whole_tree_operation() {
        let json = serde_json::json!({
            "op_id": "1-2", "pid": 2, "kind": "restore",
            "started_utc": "1970-01-01T00:00:00Z", "target": "/tmp/x",
            "pre_digest": "d", "preserved": true, "open": true
        });
        let rec: OpRecord = serde_json::from_value(json).unwrap();
        assert!(
            rec.scope.is_empty(),
            "a pre-scope record must mean whole-tree"
        );
    }

    #[test]
    fn an_unpreserved_record_undoes_nothing_because_nothing_was_touched() {
        // The window between writing intent and capturing the prior tree: the
        // target is untouched there, so recovery must be a no-op rather than
        // clearing a tree it has no copy of.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let pre = target_digest(&target).unwrap();

        let guard = begin(&target, "restore").unwrap();
        std::mem::forget(guard);

        let report = recover_with(&target, |_| false).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(target_digest(&target).unwrap(), pre);
        assert!(
            target.join("config.toml").exists(),
            "recovery wiped a tree it never preserved"
        );
    }

    // ---- BL-F26-SC3-O1-ROLLBACK -------------------------------------------
    //
    // The undo store captured the prior home with `std::fs::copy` per file,
    // walked with `read_dir` — each member of a WAL trio read at a different
    // instant. Measured on `hetzner-dsm` through the real binary: a rolled-back
    // home came back with `memory.db-wal` and `memory.db-shm` beside a database
    // failing `integrity_check` with 101 problem lines, while `backup restore`
    // and `backup recover` both exited 0.

    /// A database with UNCHECKPOINTED content in its WAL, so a capture is only
    /// correct if it folds the WAL in. A checkpointed database would pass even
    /// on an implementation that ignored the WAL entirely.
    fn seeded_wal_db(path: &Path, rows: i64) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .unwrap();
        for i in 0..rows {
            conn.execute("INSERT INTO t (id, v) VALUES (?1, ?2)", (i, "x"))
                .unwrap();
        }
        conn
    }

    #[test]
    fn the_undo_store_holds_a_folded_database_and_none_of_its_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);

        // The connection is held open across the capture, so the WAL is live
        // and uncheckpointed exactly as it is for a running Wayland.
        let live = seeded_wal_db(&target.join("memory.db"), 400);
        assert!(
            target.join("memory.db-wal").exists(),
            "no WAL was produced; this test would prove nothing"
        );
        assert!(
            std::fs::metadata(target.join("memory.db-wal"))
                .unwrap()
                .len()
                > 0,
            "the WAL is empty; this test would prove nothing"
        );

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        let undo = guard.undo_dir().to_path_buf();

        assert!(
            undo.join("memory.db").is_file(),
            "no database was preserved"
        );
        assert!(
            !undo.join("memory.db-wal").exists(),
            "the undo store carried a -wal; a restored sidecar is derived state \
             belonging to a process that no longer exists"
        );
        assert!(!undo.join("memory.db-shm").exists());

        let cap = rusqlite::Connection::open(undo.join("memory.db")).unwrap();
        let verdict: String = cap
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(verdict, "ok", "the preserved database does not verify");
        let n: i64 = cap
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 400, "the capture lost uncheckpointed WAL content");
        drop(live);
    }

    #[test]
    fn a_rolled_back_home_gets_a_database_that_opens_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let live = seeded_wal_db(&target.join("memory.db"), 250);

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();

        // What a killed `--replace` leaves: the target cleared, donor payloads
        // part-written. The live connection is dropped first, because after
        // `clear_target` the database it holds has been unlinked.
        drop(live);
        clear_tree_excluding_journal(&target).unwrap();
        std::fs::write(target.join("donor.txt"), "from the archive").unwrap();
        assert!(
            !target.join("memory.db").exists(),
            "the target really was cleared"
        );

        std::mem::forget(guard); // the owner died without committing
        let report = recover_with(&target, |_| false).unwrap();
        assert_eq!(report.recovered, 1);

        assert!(
            !target.join("donor.txt").exists(),
            "rollback left the interrupted operation's output behind"
        );
        assert!(
            !target.join("memory.db-wal").exists(),
            "rollback restored a stale -wal beside the database"
        );
        assert!(!target.join("memory.db-shm").exists());

        let back = rusqlite::Connection::open(target.join("memory.db")).unwrap();
        let verdict: String = back
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))
            .unwrap();
        assert_eq!(verdict, "ok");
        let n: i64 = back
            .query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 250, "the rolled-back database lost committed rows");
    }

    #[test]
    fn a_file_merely_named_like_a_database_is_still_byte_identical() {
        // This is the test that keeps the EXISTING interruption proofs valid.
        // `portability-migrate-rollback-proof.sh` writes a 22-byte text stub
        // called `memory.db`; if capture were selected by FILENAME that stub
        // would be rewritten and the proofs' byte-identity assertion would
        // start failing for a reason that has nothing to do with SQLite.
        // Detection is by header magic, so it must survive untouched.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let stub = b"PRIOR-USER-MEMORY-DB\n";
        std::fs::write(target.join("memory.db"), stub).unwrap();
        // And a file that merely ENDS in `-wal` while naming no database.
        std::fs::write(target.join("notes-wal"), b"user notes, not a sidecar").unwrap();
        let pre = target_digest(&target).unwrap();

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        let undo = guard.undo_dir().to_path_buf();

        assert_eq!(
            std::fs::read(undo.join("memory.db")).unwrap(),
            stub,
            "a non-database named memory.db was not preserved byte-for-byte"
        );
        assert!(
            undo.join("notes-wal").is_file(),
            "an unrelated file ending in -wal was dropped as a sidecar"
        );

        std::fs::write(target.join("config.toml"), "MUTATED").unwrap();
        std::mem::forget(guard);
        assert_eq!(recover_with(&target, |_| false).unwrap().recovered, 1);
        assert_eq!(
            target_digest(&target).unwrap(),
            pre,
            "a tree with no real database must still roll back byte-exactly"
        );
    }

    #[test]
    fn a_sidecar_is_dropped_only_for_its_own_database() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("home");
        std::fs::create_dir_all(&target).unwrap();
        seed(&target);
        let live = seeded_wal_db(&target.join("memory.db"), 32);
        // A real database's sidecar goes; an identically-suffixed file naming a
        // DIFFERENT, non-database stem stays.
        std::fs::write(target.join("other.db-wal"), b"not a sidecar of anything").unwrap();

        let mut guard = begin(&target, "restore").unwrap();
        guard.preserve_target(&target).unwrap();
        let undo = guard.undo_dir().to_path_buf();

        assert!(!undo.join("memory.db-wal").exists());
        assert!(
            undo.join("other.db-wal").is_file(),
            "a -wal whose named database does not exist was wrongly dropped"
        );
        drop(live);
    }
}
