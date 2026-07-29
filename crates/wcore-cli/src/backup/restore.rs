//! Verification-before-restore, occupied-target refusal, and the journalled
//! replace path (F26-03).
//!
//! # Order of operations, and why it is this order
//!
//! 1. **Verify the whole archive.** A restore that half-overwrites a live home
//!    and only then discovers the archive was corrupt has destroyed the thing the
//!    backup existed to protect. Nothing is written until the archive has been
//!    verified end to end.
//! 2. **Refuse paths this platform cannot materialize.** An archive travels
//!    between platforms and can carry names that are ordinary where it was made
//!    and impossible here. The target root is known, so this is exact.
//! 3. **Settle the credential remap.** A refusal must happen while the target is
//!    still untouched, or it is not a refusal.
//! 4. **Refuse an occupied target** unless `--replace` was passed explicitly.
//! 5. **Open the journal and preserve the prior tree**, then write.
//!
//! # Why `--replace` exists, and why it is the interesting path
//!
//! Refusing an occupied target is the right default and matches the peer bar.
//! But a restore that can ONLY ever write into an empty directory has a trivial
//! rollback — "delete what was created" — and an interruption proof against it
//! proves almost nothing, because there is no prior state to get wrong. The
//! scenario that can actually break is restoring OVER a home that already holds
//! diverged state, so that is the path the journal protects and the path the
//! interruption proof exercises.

use std::path::Path;

use super::archive::{self, Manifest};
use super::platform_paths;
use super::remap::{self, RemapPlan};
use super::{BackupError, dir_holds_state, journal};

#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Replace an occupied target, journalling the prior tree first.
    pub replace: bool,
    /// Proceed although the archive could not carry some credential sources.
    pub accept_missing_secrets: bool,
    /// Test seam: pause between payload writes so an interruption can be aimed
    /// at the middle of the operation on hardware whose speed is not known in
    /// advance. Zero in normal operation. It changes only WHEN bytes are
    /// written, never which bytes or in what order.
    pub pace_ms: u64,
}

#[derive(Debug)]
pub struct RestoreOutcome {
    pub written: usize,
    pub remap: RemapPlan,
    pub manifest: Manifest,
    /// Digest of the target before the operation, as recorded in the journal.
    pub pre_digest: String,
}

pub fn restore_archive(
    archive_path: &Path,
    target: &Path,
    opts: RestoreOptions,
) -> Result<RestoreOutcome, BackupError> {
    // 1. Verify before writing anything. `unpack` re-reads the payload bytes;
    //    `verify_archive` is what proves they match the manifest.
    let manifest = archive::verify_archive(archive_path)?;
    let (_, payloads) = archive::unpack(archive_path)?;

    // 2. Refuse paths this platform cannot materialize, while the target is
    //    still untouched (F26-03-D). The archive travels between platforms, so
    //    it can legitimately carry names that are ordinary where it was made
    //    and impossible here — a reserved device name, a forbidden character,
    //    an overlong component. Discovering that half way through the write
    //    loop is what made a backup unrestorable in the first place; the target
    //    root is known here, so every destination is known exactly and the
    //    refusal is a statement of fact rather than a guess.
    let declared: Vec<String> = manifest.payloads.iter().map(|p| p.path.clone()).collect();
    let objections = platform_paths::objections_for_target(target, &declared);
    if !objections.is_empty() {
        return Err(BackupError::UnrestorablePaths(platform_paths::render(
            &objections,
            target,
        )));
    }

    // 3. Settle the remap while the target is still untouched. A refusal
    //    propagates as an error from here, so nothing is written.
    let plan = remap::plan_remap(&manifest, target, opts.accept_missing_secrets)?;

    // 4. Refuse an occupied target unless replacement was asked for explicitly.
    let occupied = dir_holds_state(target);
    if occupied && !opts.replace {
        return Err(BackupError::TargetOccupied(target.to_path_buf()));
    }

    std::fs::create_dir_all(target).map_err(BackupError::io("create target home"))?;

    // 5. Write-ahead intent, then preserve the prior tree, then mutate.
    let mut guard = journal::begin(target, "restore")?;
    let pre_digest = guard.pre_digest().to_string();
    guard.preserve_target(target)?;

    let result = write_payloads(target, &manifest, &payloads, &plan, opts.pace_ms);

    match result {
        Ok(written) => {
            guard.commit()?;
            Ok(RestoreOutcome {
                written,
                remap: plan,
                manifest,
                pre_digest,
            })
        }
        Err(e) => {
            // The partial-failure path: an unwritable target part-way through is
            // interruption too, and it rolls back to the exact prior tree rather
            // than leaving a home that is neither its old self nor its new one.
            guard.rollback(target)?;
            Err(e)
        }
    }
}

fn write_payloads(
    target: &Path,
    manifest: &Manifest,
    payloads: &std::collections::BTreeMap<String, Vec<u8>>,
    plan: &RemapPlan,
    pace_ms: u64,
) -> Result<usize, BackupError> {
    // A replace produces the archive's tree, not a merge of it with whatever was
    // there. The prior tree is already captured in the undo store.
    clear_target(target)?;

    let mut written = 0usize;
    for entry in &manifest.payloads {
        // Re-checked here as well as at verification: the path is the one thing
        // an attacker controls, and it is about to become a filesystem write.
        archive::reject_traversal(&entry.path)?;
        let Some(bytes) = payloads.get(&entry.path) else {
            return Err(BackupError::VerificationFailed(format!(
                "payload '{}' vanished between verification and write",
                entry.path
            )));
        };
        let dest = target.join(&entry.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(BackupError::io("create restored dir"))?;
        }
        // Every target write goes through the existing atomic primitive, so a
        // crash mid-file leaves the old bytes or the new bytes, never a torn file
        // that the journal would then have to distinguish from a real edit.
        wcore_config::atomic_io::atomic_write(&dest, bytes)
            .map_err(BackupError::io("write restored payload"))?;
        apply_mode(&dest, entry.mode);
        written += 1;

        if pace_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(pace_ms));
        }
    }

    // The restored config must never carry the source machine's absolute secret
    // paths. Applied last, so it operates on the restored copy.
    remap::apply_rewrites(target, plan)?;

    Ok(written)
}

fn clear_target(target: &Path) -> Result<(), BackupError> {
    let entries = match std::fs::read_dir(target) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries {
        let entry = entry.map_err(BackupError::io("read target entry"))?;
        // The journal is bookkeeping for THIS operation; clearing it would
        // destroy the undo store the rollback depends on.
        if entry.file_name() == std::ffi::OsStr::new(journal::JOURNAL_DIR) {
            continue;
        }
        let path = entry.path();
        let meta = path
            .symlink_metadata()
            .map_err(BackupError::io("stat target entry"))?;
        if meta.is_dir() && !meta.file_type().is_symlink() {
            std::fs::remove_dir_all(&path).map_err(BackupError::io("clear target dir"))?;
        } else {
            std::fs::remove_file(&path).map_err(BackupError::io("clear target file"))?;
        }
    }
    Ok(())
}

fn apply_mode(path: &Path, mode: Option<u32>) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(m) = mode {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(m));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::archive::create_archive;

    fn seed_source(home: &Path) {
        std::fs::create_dir_all(home.join("skills/demo")).unwrap();
        std::fs::write(home.join("config.toml"), "[storage]\nx = 1\n").unwrap();
        std::fs::write(home.join("skills/demo/SKILL.md"), "archived body").unwrap();
    }

    #[test]
    fn restore_refuses_an_occupied_target_rather_than_replacing_it_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();

        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("live.txt"), "LIVE STATE").unwrap();
        let before = journal::target_digest(&target).unwrap();

        let err = restore_archive(&arc, &target, RestoreOptions::default()).unwrap_err();
        assert!(matches!(err, BackupError::TargetOccupied(_)), "{err:?}");
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            before,
            "a refusal must leave the target byte-identical"
        );
        assert!(
            !target.join(journal::JOURNAL_DIR).exists(),
            "a refusal must not even open a journal"
        );
    }

    #[test]
    fn restore_verifies_before_it_writes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();

        // Corrupt the archive bytes wholesale.
        let mut bytes = std::fs::read(&arc).unwrap();
        let n = bytes.len();
        bytes[n / 2] ^= 0xff;
        std::fs::write(&arc, &bytes).unwrap();

        let target = dir.path().join("target");
        let err = restore_archive(&arc, &target, RestoreOptions::default()).unwrap_err();
        assert!(
            matches!(
                err,
                BackupError::NotAnArchive(_)
                    | BackupError::VerificationFailed(_)
                    | BackupError::Io { .. }
            ),
            "{err:?}"
        );
        assert!(
            !target.exists() || !dir_holds_state(&target),
            "a failed verification must not have written into the target"
        );
    }

    #[test]
    fn a_secret_free_home_round_trips_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let src_digest = journal::target_digest(&src).unwrap();

        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();

        let target = dir.path().join("target");
        let out = restore_archive(&arc, &target, RestoreOptions::default()).unwrap();
        assert_eq!(out.written, 2);
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            src_digest,
            "a secret-free home did not round-trip byte-identically"
        );
        assert!(
            !target.join(journal::JOURNAL_DIR).exists(),
            "a completed restore left journal bookkeeping behind"
        );
    }

    #[test]
    fn a_redacted_archive_cannot_round_trip_the_secret_values_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        std::fs::write(
            src.join("credentials.toml"),
            "key = \"CANARY-SECRET-VALUE\"",
        )
        .unwrap();

        let arc = dir.path().join("a.tar.gz");
        let m = create_archive(&src, &arc, false).unwrap();
        assert_eq!(m.absent_secrets, vec!["credentials.toml".to_string()]);

        let target = dir.path().join("target");
        // The gap is a refusal by default; the operator must acknowledge it.
        let err = restore_archive(&arc, &target, RestoreOptions::default()).unwrap_err();
        assert!(matches!(err, BackupError::RemapRefused(_)), "{err:?}");

        let out = restore_archive(
            &arc,
            &target,
            RestoreOptions {
                accept_missing_secrets: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(out.remap.absent.contains(&"credentials.toml".to_string()));
        assert!(
            !target.join("credentials.toml").exists(),
            "a redacted archive restored a secret it never carried"
        );
        // The difference between source and restored tree is EXACTLY the
        // recorded absent set — nothing else was lost.
        assert!(target.join("config.toml").exists());
        assert!(target.join("skills/demo/SKILL.md").exists());
        assert_ne!(
            journal::target_digest(&target).unwrap(),
            journal::target_digest(&src).unwrap(),
            "the redacted round trip must NOT claim to equal its source"
        );
    }

    #[test]
    fn replace_over_a_diverged_target_produces_the_archives_tree_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let src_digest = journal::target_digest(&src).unwrap();
        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();

        // A target that has DIVERGED: an edited file, an extra file, and a
        // directory the archive knows nothing about.
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("stale")).unwrap();
        std::fs::write(target.join("config.toml"), "[storage]\nx = 999\n").unwrap();
        std::fs::write(target.join("stale/leftover.txt"), "should not survive").unwrap();

        let out = restore_archive(
            &arc,
            &target,
            RestoreOptions {
                replace: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.written, 2);
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            src_digest,
            "a replace over a diverged target did not reproduce the archive's tree"
        );
        assert!(
            !target.join("stale").exists(),
            "diverged state survived a replace"
        );
    }

    #[test]
    fn a_partially_written_backup_leaves_an_occupied_target_byte_identical() {
        // The scenario that matters: the target is a live profile, and the
        // archive is one whose write was interrupted. Restoring into an empty
        // directory would prove nothing here -- there would be nothing to lose.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();

        let full = std::fs::read(&arc).unwrap();
        let truncated = dir.path().join("truncated.tar.gz");
        std::fs::write(&truncated, &full[..full.len() / 2]).unwrap();

        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("skills")).unwrap();
        std::fs::write(target.join("config.toml"), "LIVE PROFILE").unwrap();
        std::fs::write(target.join("skills/live.md"), "LIVE SKILL").unwrap();
        let pre = journal::target_digest(&target).unwrap();

        // Even with --replace, which is the destructive mode.
        let err = restore_archive(
            &truncated,
            &target,
            RestoreOptions {
                replace: true,
                accept_missing_secrets: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            !matches!(err, BackupError::TargetOccupied(_)),
            "should have failed on the archive, not the target: {err:?}"
        );
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            pre,
            "a partially-written backup damaged a live target"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("config.toml")).unwrap(),
            "LIVE PROFILE"
        );
        assert!(
            !target.join(journal::JOURNAL_DIR).exists(),
            "verification failed before writing, so no journal should have opened"
        );
    }

    #[test]
    fn an_older_schema_archive_restores_over_an_existing_profile() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        let m = create_archive(&src, &arc, false).unwrap();
        let src_digest = journal::target_digest(&src).unwrap();
        let (_, payloads) = archive::unpack(&arc).unwrap();
        let blobs: Vec<(String, Vec<u8>)> = payloads.into_iter().collect();

        // An archive written by an older build: the manifest predates the
        // fields this build added, so they are absent rather than empty.
        let older: Manifest = serde_json::from_value(serde_json::json!({
            "format": crate::backup::archive::FORMAT_ID,
            "version": 1,
            "created_utc": "1970-01-01T00:00:00Z",
            "digest_algo": crate::backup::archive::DIGEST_ALGO,
            "tree_digest": m.tree_digest,
            "payloads": m.payloads,
            "credentials": { "backend": "plaintext", "carried": true, "secrets_outside_tree": false }
        }))
        .unwrap();
        let old_arc = dir.path().join("older.tar.gz");
        std::fs::write(&old_arc, archive::pack(&older, &blobs).unwrap()).unwrap();

        // A target that is an EXISTING, diverged profile.
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("stale")).unwrap();
        std::fs::write(target.join("config.toml"), "OLD LIVE CONFIG").unwrap();
        std::fs::write(target.join("stale/gone.txt"), "must not survive").unwrap();

        let out = restore_archive(
            &old_arc,
            &target,
            RestoreOptions {
                replace: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.written, 2);
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            src_digest,
            "an older-schema archive did not restore exactly over an existing profile"
        );
        assert!(!target.join("stale").exists());
    }

    /// F26-SC3-H1. A restore that follows an INTERRUPTED restore must not leave
    /// the interrupted operation's undo store armed behind it.
    ///
    /// The sequence is the one a real user produces, because `backup recover` is
    /// a command they have never heard of: a restore is killed, they simply run
    /// the restore again, and it succeeds. If the first operation's record and
    /// undo store survive that, then the NEXT recovery pass — triggered by any
    /// later interruption, or by the user finally being told about `recover` —
    /// rolls the home back to a tree that predates the successful restore, and
    /// the restored content is destroyed.
    ///
    /// This test asserts the property in the only place it is observable: run a
    /// recovery pass after the successful restore and require the tree to still
    /// be the restored one.
    #[test]
    fn a_restore_after_an_interrupted_one_does_not_leave_a_stale_undo_store_armed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        create_archive(&src, &arc, false).unwrap();
        let archive_digest = journal::target_digest(&src).unwrap();

        // A live, diverged home.
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("skills")).unwrap();
        std::fs::write(target.join("config.toml"), "LIVE PROFILE").unwrap();
        std::fs::write(target.join("skills/live.md"), "LIVE SKILL").unwrap();
        let pristine = journal::target_digest(&target).unwrap();

        // --- restore #1, killed mid-apply ---------------------------------
        // Exactly what `restore_archive` does up to the kill: open the journal,
        // preserve the prior tree, clear the target, write one payload, die.
        {
            let mut guard = journal::begin(&target, "restore").unwrap();
            guard.preserve_target(&target).unwrap();
            clear_target(&target).unwrap();
            std::fs::write(target.join("config.toml"), "[storage]\nx = 1\n").unwrap();
            std::mem::forget(guard); // the owner died without committing
        }
        assert_ne!(
            journal::target_digest(&target).unwrap(),
            pristine,
            "premise: the interrupted restore really did damage the tree"
        );
        assert_eq!(
            journal::list_open(&target).len(),
            1,
            "premise: the interrupted restore left exactly one open record"
        );

        // --- restore #2: the user simply runs it again, and it succeeds ----
        let out = restore_archive(
            &arc,
            &target,
            RestoreOptions {
                replace: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out.written, 2);
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            archive_digest,
            "premise: the second restore produced the archive's tree"
        );

        // --- the property ---------------------------------------------------
        // A recovery pass with EVERY owner dead must find nothing left to undo.
        // At base this finds the first restore's record, restores its undo store
        // and destroys the successful restore.
        let report = journal::recover_with(&target, |_| false).unwrap();
        assert_eq!(
            report.recovered, 0,
            "a stale undo store from an interrupted restore was still armed \
             after a later restore completed"
        );
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            archive_digest,
            "a recovery pass rolled a COMPLETED restore back to a pre-operation \
             tree — the restored content was destroyed"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("skills/demo/SKILL.md")).unwrap(),
            "archived body"
        );
    }

    #[test]
    fn a_partial_failure_part_way_through_rolls_back_to_the_exact_prior_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        seed_source(&src);
        let arc = dir.path().join("a.tar.gz");
        let mut m = create_archive(&src, &arc, false).unwrap();

        // An archive that VERIFIES cleanly but cannot be fully written: `a.txt`
        // is a file and `a.txt/b.txt` needs it to be a directory. Every digest
        // matches and every declared payload is present, so verification passes
        // and the failure lands PART WAY THROUGH the write loop — which is the
        // case under test. A manifest entry with a missing payload would have
        // been rejected at verification, before the journal ever opened, and
        // would have exercised nothing.
        let blobs = vec![
            ("a.txt".to_string(), b"file-not-dir".to_vec()),
            (
                "a.txt/b.txt".to_string(),
                b"needs a.txt to be a dir".to_vec(),
            ),
        ];
        m.payloads = blobs
            .iter()
            .map(|(p, b)| crate::backup::archive::PayloadEntry {
                path: p.clone(),
                bytes: b.len() as u64,
                sha256: crate::backup::sha256_hex(b),
                mode: None,
            })
            .collect();
        m.absent_secrets.clear();
        m.tree_digest = Manifest::compute_tree_digest(&m.payloads);
        let bad = dir.path().join("bad.tar.gz");
        std::fs::write(&bad, archive::pack(&m, &blobs).unwrap()).unwrap();
        // Prove the premise: this archive really does verify.
        archive::verify_archive(&bad).expect("the partial-failure archive must verify cleanly");

        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("keep.txt"), "PRIOR").unwrap();
        std::fs::write(target.join("config.toml"), "prior = true").unwrap();
        let pre = journal::target_digest(&target).unwrap();

        let err = restore_archive(
            &bad,
            &target,
            RestoreOptions {
                replace: true,
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, BackupError::Io { .. }),
            "expected the write loop to fail on the filesystem, got {err:?}"
        );
        assert_eq!(
            journal::target_digest(&target).unwrap(),
            pre,
            "a partial failure did not roll back to the exact prior tree"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("keep.txt")).unwrap(),
            "PRIOR"
        );
    }
}
