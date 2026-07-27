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

/// Begin an operation against `target`.
///
/// Writes the durable intent record BEFORE the caller touches anything.
pub fn begin(target: &Path, kind: &str) -> Result<OpGuard, BackupError> {
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
    let pre_digest = digest_excluding_journal(target)?;

    let record = OpRecord {
        op_id: op_id.clone(),
        pid,
        kind: kind.to_string(),
        started_utc: chrono::Utc::now().to_rfc3339(),
        target: target.display().to_string(),
        pre_digest,
        preserved: false,
        open: true,
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
        copy_tree_excluding_journal(target, &self.undo_dir)?;
        self.record.preserved = true;
        write_record(&self.record_path, &self.record)
    }

    /// Undo this operation to the exact pre-operation tree.
    pub fn rollback(&mut self, target: &Path) -> Result<(), BackupError> {
        if self.record.preserved {
            restore_from_undo(target, &self.undo_dir)?;
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
            restore_from_undo(target, &undo_dir)?;
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

/// Replace `target`'s content with the preserved copy, exactly.
fn restore_from_undo(target: &Path, undo_dir: &Path) -> Result<(), BackupError> {
    if !undo_dir.is_dir() {
        return Ok(());
    }
    clear_tree_excluding_journal(target)?;
    copy_tree_all(undo_dir, target)
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

/// Digest of `target` ignoring the journal directory.
///
/// Reuses 26-01's `tree_digest` rather than growing a second digest: the journal
/// directory is moved aside for the measurement only when one exists, which is
/// never the case for a first operation on a clean home.
fn digest_excluding_journal(target: &Path) -> Result<String, BackupError> {
    let journal = target.join(JOURNAL_DIR);
    if !journal.exists() {
        return Ok(wcore_config::portability::tree_digest(target)
            .map_err(BackupError::io("digest target tree"))?
            .digest);
    }
    // A concurrent operation already created the journal. Digest a filtered
    // shadow so the value stays comparable with a journal-free tree.
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

fn copy_tree_excluding_journal(src: &Path, dst: &Path) -> Result<(), BackupError> {
    copy_inner(src, dst, true)
}

fn copy_tree_all(src: &Path, dst: &Path) -> Result<(), BackupError> {
    copy_inner(src, dst, false)
}

fn copy_inner(src: &Path, dst: &Path, skip_journal: bool) -> Result<(), BackupError> {
    std::fs::create_dir_all(dst).map_err(BackupError::io("create copy dir"))?;
    let entries = match std::fs::read_dir(src) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry.map_err(BackupError::io("read copy entry"))?;
        let name = entry.file_name();
        if skip_journal && name == std::ffi::OsStr::new(JOURNAL_DIR) {
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
        let to = dst.join(&name);
        if meta.is_dir() {
            copy_inner(&from, &to, false)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to).map_err(BackupError::io("copy file"))?;
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
        if entry.file_name() == std::ffi::OsStr::new(JOURNAL_DIR) {
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
}
