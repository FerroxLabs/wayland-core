//! Rollback for `migrate` — the write-ahead journal around the apply, and the
//! reverse-apply that returns an interrupted import to the exact pre-operation
//! home (SC3 gap G3).
//!
//! # What was missing, and why it is not the same as "converging forward"
//!
//! `26-PHASE-VERDICT.md` graded Criterion 3 PARTIAL on exactly this: backup and
//! restore survive interruption and roll back exactly, but `migrate` did not
//! roll back at all. An interrupted import left its partial work in place and
//! **converged on the completed state when the product was driven again**. That
//! is a defensible import contract and it was proven — 35 mid-apply `SIGKILL`s
//! per peer, 0 unrecovered after the `save_index` fix — but it is not the
//! criterion's text, which says *restore exact pre-operation state on rollback*.
//!
//! Forward convergence and rollback answer different questions. A user who
//! re-runs the import wants convergence. A user whose import went wrong — the
//! wrong peer home, the wrong `--select`, a machine that died — wants their home
//! back, and before this there was no answer for them at all short of restoring
//! a backup they may not have taken.
//!
//! # Why the journal is SCOPED, and why that is not a shortcut
//!
//! [`crate::backup::journal`] already solves the durable-intent problem, and
//! duplicating it here would be the cross-crate duplication the crate map
//! forbids. What it could not do was operate on part of a tree: `restore`
//! replaces an entire home, so its undo store is a whole-tree copy.
//!
//! `migrate` writes into a home the user is LIVING in. A whole-tree undo store
//! would copy `memory.db`, every session and every asset on every import, and —
//! worse — a rollback would then restore all of them, silently reverting
//! whatever else happened while the import ran. The scope is therefore declared
//! and recorded durably, and it is exactly the production write set:
//!
//! * `migrate-quarantine/` — written once per admitted item by
//!   [`crate::migrate::quarantine::QuarantineStore::admit`], which is the
//!   incremental, interruptible part;
//! * `config.toml` — written once, atomically, at the very end.
//!
//! # The write set is a MEASUREMENT, not a claim
//!
//! A scoped journal is exact only if the operation really does confine itself
//! to its scope, and "I read the code and it only writes two things" is the kind
//! of claim that rots the moment someone adds a third. So the scope is checked
//! rather than asserted: [`OUT_OF_SCOPE_PROBE`] makes a run digest the home's
//! out-of-scope portion before and after the apply and fail loudly if it moved.
//! It is off by default because it costs a full tree walk; the interruption
//! proof turns it on, which is where a regression would be caught.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::backup::journal;

/// The complete production write set of `migrate`'s apply, relative to the
/// Wayland home. Kept next to the guard that uses it so the two cannot drift.
pub const MIGRATE_SCOPE: [&str; 2] = [crate::migrate::quarantine::QUARANTINE_DIR, "config.toml"];

/// Set to `1` to make a run verify that it wrote nothing outside
/// [`MIGRATE_SCOPE`]. Costs a full tree walk of the home, twice.
pub const OUT_OF_SCOPE_PROBE: &str = "WAYLAND_MIGRATE_SCOPE_PROBE";

/// An import in flight, with its pre-operation state captured.
pub struct ApplyGuard {
    home: PathBuf,
    guard: journal::OpGuard,
    /// Digest of everything OUTSIDE the scope, when the probe is armed.
    outside_before: Option<String>,
    /// How many dead-owner operations this run rolled back before it began.
    recovered_before_start: usize,
}

impl ApplyGuard {
    /// Roll back any interrupted operation, then open a journal for this one and
    /// capture the pre-operation state of the scope.
    ///
    /// Recovery comes FIRST for the same reason it does in
    /// [`crate::backup::restore`]: an interrupted import's undo store holds the
    /// user's real prior home, and opening a new operation on top of it would
    /// capture the wreckage as if it were the prior state — after which the
    /// original is unreachable. It also means the ordinary user gesture after a
    /// crash, which is to run the import again, restores the home first.
    pub fn open(home: &Path) -> Result<Self> {
        Self::open_with(home, crate::cron::process_is_alive)
    }

    /// Inner open with an injectable liveness probe, mirroring
    /// [`journal::recover_with`] and [`crate::backup::restore::restore_archive_with`].
    ///
    /// A test that stages an interrupted import does so inside the live test
    /// process, whose pid is correctly reported alive; through the public entry
    /// point it would find nothing to recover and pass without exercising the
    /// recovery at all.
    pub fn open_with(home: &Path, is_alive: impl Fn(u32) -> bool) -> Result<Self> {
        std::fs::create_dir_all(home)
            .with_context(|| format!("create wayland home {}", home.display()))?;
        let recovered_before_start = journal::recover_with(home, is_alive)
            .context("roll back an import left in flight by a dead process")?
            .recovered;

        let outside_before = if scope_probe_armed() {
            Some(digest_outside_scope(home)?)
        } else {
            None
        };

        let mut guard = journal::begin_scoped(home, "migrate", &MIGRATE_SCOPE)
            .context("open the migrate journal")?;
        guard
            .preserve_target(home)
            .context("capture the pre-import state")?;
        Ok(Self {
            home: home.to_path_buf(),
            guard,
            outside_before,
            recovered_before_start,
        })
    }

    pub fn recovered_before_start(&self) -> usize {
        self.recovered_before_start
    }

    /// Digest of the scope before the import, as recorded durably in the record.
    pub fn pre_digest(&self) -> &str {
        self.guard.pre_digest()
    }

    /// The import succeeded: verify the scope held, then close the record.
    pub fn commit(self) -> Result<()> {
        let ApplyGuard {
            home,
            guard,
            outside_before,
            ..
        } = self;
        if let Some(before) = outside_before {
            let after = digest_outside_scope(&home)?;
            if after != before {
                // Deliberately BEFORE the commit: the journal still holds a
                // usable undo store for the scope, and a write outside it means
                // the scope no longer describes the operation, so continuing
                // would be claiming a rollback guarantee that is now false.
                bail!(
                    "migrate wrote outside its declared scope {MIGRATE_SCOPE:?}; \
                     the rollback guarantee does not cover those writes \
                     (out-of-scope digest {before} -> {after})"
                );
            }
        }
        guard.commit().context("close the migrate journal")?;
        Ok(())
    }

    /// The import failed: put the scope back exactly as it was.
    pub fn rollback(mut self) -> Result<()> {
        self.guard
            .rollback(&self.home)
            .context("roll the import back to the pre-import home")?;
        Ok(())
    }
}

fn scope_probe_armed() -> bool {
    std::env::var(OUT_OF_SCOPE_PROBE).is_ok_and(|v| v == "1")
}

/// Digest of the home with the scope — and the journal's own bookkeeping —
/// removed, so the value moves if and only if the operation touched something it
/// declared it would not.
fn digest_outside_scope(home: &Path) -> Result<String> {
    let shadow = tempfile::tempdir().context("out-of-scope shadow dir")?;
    copy_outside(home, shadow.path())?;
    Ok(wcore_config::portability::tree_digest(shadow.path())
        .context("digest the out-of-scope shadow")?
        .digest)
}

fn copy_outside(home: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).context("create shadow root")?;
    let Ok(entries) = std::fs::read_dir(home) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        // The scope is excluded because the journal covers it. Bookkeeping is
        // excluded because a killed process leaves a crash marker behind, and a
        // probe that fired on that would report an out-of-scope write for every
        // interrupted run.
        if MIGRATE_SCOPE.contains(&name_str.as_str()) || journal::is_bookkeeping(&name) {
            continue;
        }
        let from = entry.path();
        let meta = from.symlink_metadata().context("stat shadow entry")?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let to = dst.join(&name);
        if meta.is_dir() {
            copy_dir(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to).context("copy shadow file")?;
        }
    }
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).context("create shadow dir")?;
    let Ok(entries) = std::fs::read_dir(src) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let from = entry.path();
        let meta = from.symlink_metadata().context("stat shadow entry")?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let to = dst.join(entry.file_name());
        if meta.is_dir() {
            copy_dir(&from, &to)?;
        } else if meta.is_file() {
            std::fs::copy(&from, &to).context("copy shadow file")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_home(home: &Path) {
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 0\n").unwrap();
        std::fs::write(home.join("memory.db"), "PRIOR DB").unwrap();
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::write(home.join("sessions/s1.json"), "prior").unwrap();
    }

    /// The declared scope must BE the write set, spelled the way the store
    /// spells it. A literal `"quarantine"` here would silently scope the journal
    /// to a directory that does not exist, and every rollback would be a no-op
    /// that reported success.
    #[test]
    fn the_declared_scope_is_the_stores_real_directory_name() {
        assert_eq!(MIGRATE_SCOPE[0], "migrate-quarantine");
        assert_eq!(MIGRATE_SCOPE[1], "config.toml");
        let store = crate::migrate::quarantine::QuarantineStore::new(
            Path::new("/tmp/home").join(crate::migrate::quarantine::QUARANTINE_DIR),
        );
        assert_eq!(
            store.root().file_name().unwrap().to_string_lossy(),
            MIGRATE_SCOPE[0],
            "the journal scope and the quarantine store have drifted apart"
        );
    }

    #[test]
    fn a_rolled_back_import_leaves_the_home_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        seed_home(&home);
        let pre = journal::target_digest(&home).unwrap();

        let g = ApplyGuard::open(&home).unwrap();
        // A half-applied import.
        let store = home.join(MIGRATE_SCOPE[0]);
        std::fs::create_dir_all(store.join("payloads/item-1")).unwrap();
        std::fs::write(store.join("payloads/item-1/SKILL.md"), "contained").unwrap();
        std::fs::write(store.join("index.json"), "{\"entries\":{\"a\":1}}").unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 13\n").unwrap();
        assert_ne!(journal::target_digest(&home).unwrap(), pre);

        g.rollback().unwrap();

        assert_eq!(
            journal::target_digest(&home).unwrap(),
            pre,
            "a rolled-back import did not leave the home byte-identical"
        );
        assert!(
            !store.exists(),
            "the quarantine store survived the rollback"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "profiles = 0\n"
        );
    }

    #[test]
    fn a_committed_import_keeps_its_work_and_leaves_no_open_record() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        seed_home(&home);

        let g = ApplyGuard::open(&home).unwrap();
        std::fs::write(home.join("config.toml"), "profiles = 13\n").unwrap();
        g.commit().unwrap();

        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "profiles = 13\n",
            "a committed import was reverted"
        );
        assert!(journal::list_open(&home).is_empty());
        assert!(
            !home.join(journal::JOURNAL_DIR).exists(),
            "a committed import left journal bookkeeping in the home"
        );
    }

    /// The interruption case: no `commit`, no `rollback` — the process simply
    /// stops existing — and a LATER process puts the home back.
    #[test]
    fn an_import_whose_process_died_is_rolled_back_by_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        seed_home(&home);
        let pre = journal::target_digest(&home).unwrap();

        let g = ApplyGuard::open(&home).unwrap();
        let store = home.join(MIGRATE_SCOPE[0]);
        std::fs::create_dir_all(store.join("payloads/half")).unwrap();
        std::fs::write(store.join("payloads/half/SKILL.md"), "half").unwrap();
        std::fs::write(store.join("index.json"), "{}").unwrap();
        std::mem::forget(g); // SIGKILL has no Drop

        assert_ne!(journal::target_digest(&home).unwrap(), pre);

        let report = journal::recover_with(&home, |_| false).unwrap();
        assert_eq!(report.recovered, 1);
        assert_eq!(
            journal::target_digest(&home).unwrap(),
            pre,
            "recovery did not return the home to its exact pre-import state"
        );
    }

    /// The next import must recover the dead one first, and say it did.
    #[test]
    fn opening_an_import_recovers_a_dead_predecessor_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        seed_home(&home);
        let pre = journal::target_digest(&home).unwrap();

        let g = ApplyGuard::open(&home).unwrap();
        assert_eq!(
            g.recovered_before_start(),
            0,
            "a clean home recovers nothing"
        );
        std::fs::write(home.join("config.toml"), "WRECKED").unwrap();
        std::mem::forget(g);

        // The staged interruption's record carries THIS process's pid, which is
        // alive, so the real probe would decline to recover it and the test
        // would pass having exercised nothing. `|_| false` is the production
        // situation.
        let g2 = ApplyGuard::open_with(&home, |_| false).unwrap();
        assert_eq!(
            g2.recovered_before_start(),
            1,
            "the dead predecessor was not recovered"
        );
        assert_eq!(
            g2.pre_digest(),
            journal::scoped_digest(&home, &MIGRATE_SCOPE)
                .unwrap()
                .as_str(),
            "the new operation's pre-state is not the home as it now stands"
        );
        g2.rollback().unwrap();
        assert_eq!(
            journal::target_digest(&home).unwrap(),
            pre,
            "the true pre-operation home was not recoverable after a retry"
        );
    }

    /// The out-of-scope probe must be able to FAIL. Without this the guard's
    /// "the write set is bounded" is a claim the code never checks.
    #[test]
    fn the_out_of_scope_probe_catches_a_write_it_does_not_cover() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        seed_home(&home);

        let before = digest_outside_scope(&home).unwrap();
        // In scope: must NOT move the out-of-scope digest.
        std::fs::write(home.join("config.toml"), "profiles = 99\n").unwrap();
        std::fs::create_dir_all(home.join(MIGRATE_SCOPE[0])).unwrap();
        std::fs::write(home.join(MIGRATE_SCOPE[0]).join("index.json"), "{}").unwrap();
        assert_eq!(
            digest_outside_scope(&home).unwrap(),
            before,
            "the probe fired for a write that IS in scope"
        );

        // Out of scope: must move it.
        std::fs::write(home.join("memory.db"), "TOUCHED").unwrap();
        assert_ne!(
            digest_outside_scope(&home).unwrap(),
            before,
            "the probe did not notice a write outside the declared scope"
        );
    }
}
