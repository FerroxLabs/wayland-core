//! 23A-C1 acceptance: revocation and rollback of auto-drafted skills.
//!
//! Every test pins an explicit governance root in a tempdir. None of them resolves a
//! process-global path, so they neither race each other nor touch the developer's real
//! profile — the #564 lesson from this same subsystem.
//!
//! These tests assert the properties the criterion names, not the implementation's shape:
//! bytes survive, the exact prior state returns, suppression is real, and the crash-ordering
//! invariant holds.

use std::path::{Path, PathBuf};

use wcore_skills::govern::{GovernError, GovernanceStore, JournalEvent};

/// Materialise a draft exactly as `SkillDrafter::draft` writes one: a directory holding
/// `SKILL.md` plus a sibling `manifest.json` carrying `auto_drafted` and `signature`.
fn write_draft(skills_root: &Path, name: &str, signature: &str, body: &str) -> PathBuf {
    let dir = skills_root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), body).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "name": name,
            "auto_drafted": true,
            "drafted_at": "2026-07-29T00:00:00Z",
            "signature": signature,
            "evidence_count": 3,
            "needs_review": true,
        }))
        .unwrap(),
    )
    .unwrap();
    dir
}

struct Fixture {
    _tmp: tempfile::TempDir,
    skills: PathBuf,
    store: GovernanceStore,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    let store = GovernanceStore::new(tmp.path().join("skills-governance"));
    Fixture {
        _tmp: tmp,
        skills,
        store,
    }
}

// ---------------------------------------------------------------------------
// The core criterion clauses
// ---------------------------------------------------------------------------

#[test]
fn revoke_removes_the_artifact_and_retains_every_byte() {
    let f = fixture();
    let body = "# Auto-drafted skill: auto-alpha\n\nbody bytes that must survive\n";
    let dir = write_draft(&f.skills, "auto-alpha", "alpha-sig", body);

    let rec = f.store.revoke(&dir).unwrap();

    assert!(
        !dir.exists(),
        "revoke must remove the artifact from the user's directory"
    );
    assert_eq!(rec.skill_name, "auto-alpha");
    assert_eq!(rec.signature.as_deref(), Some("alpha-sig"));
    assert_eq!(
        rec.file_count, 2,
        "SKILL.md + manifest.json must both be retained"
    );
    assert!(rec.byte_count > 0);

    // The retained copy must be byte-identical, not a re-render.
    let retained = f
        .store
        .root()
        .join("generations")
        .join(&rec.revocation_id)
        .join("payload")
        .join("SKILL.md");
    assert_eq!(
        std::fs::read_to_string(&retained).unwrap(),
        body,
        "retained bytes must be identical to what was removed"
    );
}

#[test]
fn rollback_restores_the_exact_prior_state() {
    let f = fixture();
    let body = "# Auto-drafted skill: auto-beta\n\nexact bytes\n";
    let dir = write_draft(&f.skills, "auto-beta", "beta-sig", body);
    let manifest_before = std::fs::read(dir.join("manifest.json")).unwrap();

    let rec = f.store.revoke(&dir).unwrap();
    assert!(!dir.exists());

    let restored = f.store.rollback(&rec.revocation_id).unwrap();

    assert_eq!(restored, dir, "rollback must restore to the original path");
    assert!(dir.exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        body,
        "SKILL.md must come back byte-identical"
    );
    assert_eq!(
        std::fs::read(dir.join("manifest.json")).unwrap(),
        manifest_before,
        "manifest.json must come back byte-identical"
    );
}

#[test]
fn rollback_clears_the_suppression_so_the_tombstone_is_not_itself_irreversible() {
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-gamma", "gamma-sig", "body\n");

    let rec = f.store.revoke(&dir).unwrap();
    assert!(f.store.is_revoked("auto-gamma", Some("gamma-sig")));

    f.store.rollback(&rec.revocation_id).unwrap();

    assert!(
        !f.store.is_revoked("auto-gamma", Some("gamma-sig")),
        "after rollback the draft must no longer be suppressed, or the tombstone \
         becomes a new irreversible mutation"
    );
}

// ---------------------------------------------------------------------------
// Identity: the dual key
// ---------------------------------------------------------------------------

#[test]
fn revocation_matches_on_signature_even_when_the_name_differs() {
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-delta", "shared-sig", "body\n");
    f.store.revoke(&dir).unwrap();

    assert!(
        f.store
            .is_revoked("auto-renamed-by-a-future-scheme", Some("shared-sig")),
        "signature must suppress regardless of the name the drafter would choose"
    );
}

#[test]
fn revocation_matches_on_name_when_the_manifest_is_damaged_and_the_signature_is_unknown() {
    // loader.rs already tolerates drafts with a missing/damaged manifest. For exactly
    // those the signature is unrecoverable, so a signature-only key would leave them
    // permanently un-revocable. This is the case the dual key exists for.
    let f = fixture();
    let dir = f.skills.join("auto-epsilon");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), "body\n").unwrap();
    std::fs::write(dir.join("manifest.json"), b"{ this is not json").unwrap();

    let rec = f.store.revoke(&dir).unwrap();
    assert_eq!(
        rec.signature, None,
        "a damaged manifest must not fail the revocation"
    );
    assert!(
        f.store.is_revoked("auto-epsilon", None),
        "a draft with an unreadable manifest must still be revocable by name"
    );
}

#[test]
fn an_unrelated_skill_is_not_suppressed() {
    // Guards the manufactured-green failure mode: a revocation that suppressed everything
    // would pass every "did it come back?" test while destroying the feature.
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-zeta", "zeta-sig", "body\n");
    f.store.revoke(&dir).unwrap();

    assert!(
        !f.store.is_revoked("auto-eta", Some("eta-sig")),
        "revoking one draft must not suppress a different one"
    );
    assert!(!f.store.is_revoked("some-user-authored-skill", None));
}

// ---------------------------------------------------------------------------
// Crash ordering — the invariant the whole design turns on
// ---------------------------------------------------------------------------

#[test]
fn suppression_is_durable_before_the_artifact_is_removed() {
    // The ordering invariant stated as a property: at no point can the artifact be gone
    // while nothing suppresses it. Proven by reconstructing the mid-crash state directly --
    // a durable tombstone with the source directory still present -- and asserting that
    // state already suppresses.
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-theta", "theta-sig", "body\n");
    let rec = f.store.revoke(&dir).unwrap();

    // Recreate the artifact to simulate "removal had not happened yet".
    write_draft(&f.skills, "auto-theta", "theta-sig", "body\n");

    assert!(
        f.store.is_revoked("auto-theta", Some("theta-sig")),
        "with the tombstone durable, suppression must already hold even though the \
         directory still exists -- this is what makes the crash window safe"
    );
    assert!(!rec.revocation_id.is_empty());
}

#[test]
fn revoke_is_idempotent_across_a_crash_between_tombstone_and_removal() {
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-iota", "iota-sig", "body\n");
    let first = f.store.revoke(&dir).unwrap();

    // Simulate the crash window: tombstone is durable, artifact came back.
    write_draft(&f.skills, "auto-iota", "iota-sig", "body\n");
    assert!(dir.exists());

    let second = f.store.revoke(&dir).unwrap();

    assert_eq!(
        first.revocation_id, second.revocation_id,
        "completing an interrupted revocation must not mint a second one"
    );
    assert!(!dir.exists(), "the retry must finish the removal");
    assert_eq!(
        f.store.live_revocations().unwrap().len(),
        1,
        "exactly one revocation must be in force"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

#[test]
fn rollback_refuses_to_overwrite_an_occupied_target() {
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-kappa", "kappa-sig", "original\n");
    let rec = f.store.revoke(&dir).unwrap();

    // The user hand-authored something at the same path after revoking.
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "hand written, must not be clobbered\n",
    )
    .unwrap();

    let err = f.store.rollback(&rec.revocation_id).unwrap_err();
    assert!(
        matches!(err, GovernError::RestoreTargetOccupied { .. }),
        "expected RestoreTargetOccupied, got {err:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("SKILL.md")).unwrap(),
        "hand written, must not be clobbered\n",
        "the user's own file must survive a refused rollback"
    );
}

#[test]
fn rollback_of_an_unknown_id_fails_rather_than_succeeding_silently() {
    let f = fixture();
    let err = f
        .store
        .rollback("00000000-0000-0000-0000-000000000000")
        .unwrap_err();
    assert!(
        matches!(err, GovernError::NoSuchRevocation(_)),
        "got {err:?}"
    );
}

#[test]
fn revoking_a_missing_directory_fails() {
    let f = fixture();
    let err = f
        .store
        .revoke(&f.skills.join("does-not-exist"))
        .unwrap_err();
    assert!(matches!(err, GovernError::NotFound(_)), "got {err:?}");
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_a_skill_directory_is_refused_not_followed() {
    // A followed symlink would let a snapshot copy arbitrary user files into the
    // governance store, and let a rollback write them back out somewhere else.
    let f = fixture();
    let secret = f._tmp.path().join("private.txt");
    std::fs::write(&secret, "must never be copied").unwrap();

    let dir = write_draft(&f.skills, "auto-lambda", "lambda-sig", "body\n");
    std::os::unix::fs::symlink(&secret, dir.join("escape.txt")).unwrap();

    let err = f.store.revoke(&dir).unwrap_err();
    assert!(
        matches!(err, GovernError::RefusedSnapshot { .. }),
        "expected RefusedSnapshot, got {err:?}"
    );
    assert!(
        dir.exists(),
        "a refused revocation must not remove anything"
    );
}

// ---------------------------------------------------------------------------
// The journal
// ---------------------------------------------------------------------------

#[test]
fn the_journal_is_append_only_and_a_rollback_does_not_erase_the_revocation() {
    let f = fixture();
    let dir = write_draft(&f.skills, "auto-mu", "mu-sig", "body\n");
    let rec = f.store.revoke(&dir).unwrap();
    f.store.rollback(&rec.revocation_id).unwrap();

    let events = f.store.journal().unwrap();
    assert_eq!(events.len(), 2, "both events must be present: {events:?}");
    assert!(matches!(events[0], JournalEvent::Revoked { .. }));
    assert!(matches!(events[1], JournalEvent::RolledBack { .. }));
}

#[test]
fn empty_state_reads_as_empty_rather_than_erroring() {
    let f = fixture();
    assert!(f.store.live_revocations().unwrap().is_empty());
    assert!(f.store.journal().unwrap().is_empty());
    assert!(!f.store.is_revoked("anything", None));
}

#[test]
fn an_unreadable_tombstone_does_not_un_suppress_the_others() {
    let f = fixture();
    let a = write_draft(&f.skills, "auto-nu", "nu-sig", "body\n");
    f.store.revoke(&a).unwrap();

    std::fs::write(
        f.store.root().join("tombstones").join("garbage.json"),
        b"{ not valid",
    )
    .unwrap();

    assert!(
        f.store.is_revoked("auto-nu", Some("nu-sig")),
        "one corrupt tombstone must not silently un-suppress every other revocation"
    );
}
