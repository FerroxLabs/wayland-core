//! 23A-C1: what governance does to the **catalog**, which is what decides whether a
//! skill can run at all.
//!
//! # Why this file exists separately from `govern_revoke_rollback.rs`
//!
//! That file proves the store behaves: bytes retained, tombstone durable, rollback exact.
//! None of it proves the *product* honours a revocation, because the store is not what the
//! engine consults — the **catalog** is. A revocation that wrote a perfect tombstone and
//! left the skill loadable would pass every test in that file and revoke nothing.
//!
//! So these tests assert on `load_all_skills`, the function the engine actually calls.
//!
//! # The claim being pinned, and why the catalog is the right place for it
//!
//! `bootstrap.rs` derives `candidate_names` from `catalog.visible()` and scopes **every**
//! router-hydration layer to it — GEPA `bench` winners, the `auto_drafter` Layer 1b
//! read-back, and `seed_from_prioritizer`. `PromptStore::best_for_skill` is a per-name
//! lookup (`WHERE skill_name = ?1`), so a name absent from the candidate list is never
//! queried in any of the three.
//!
//! That is what makes dropping from the catalog sufficient: a revoked skill's
//! `evolved_prompts` row is not deleted, it is made **unreachable**. And unreachable is
//! achievable here, one layer below both the procedure store and the evolved-prompt
//! store, without `wcore-skills` taking a dependency on `wcore-evolve`.
//!
//! # Dropped, not quarantined — the distinction is the whole test
//!
//! Quarantine (`disable_model_invocation`) only hides a skill from the model. The skill is
//! still loaded, still resolvable by name, and still executable through the user-invocable
//! path. **A revoked skill that is merely quarantined is still a revoked skill you can
//! run**, so these tests assert absence from the catalog, not a flag on it.
//!
//! # Isolation
//!
//! `WAYLAND_HOME` points at a tempdir for every test, which is what both
//! `paths::wayland_home_skills_dirs` and `govern::governance_root` resolve against. The
//! developer's real skills directory is never read and never written. `#[serial]` because
//! the process environment is shared.

use std::path::{Path, PathBuf};

use serial_test::serial;
use tempfile::TempDir;
use wcore_skills::govern::GovernanceStore;
use wcore_skills::loader::load_all_skills;
use wcore_skills::types::SkillMetadata;

/// Install a generated draft the way the auto-draft loop does.
fn install_draft(home: &Path, name: &str, body: &str) -> PathBuf {
    let dir = home.join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: draft {name}\n---\n\n{body}\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"auto_drafted":true,"signature":"sig-{name}"}}"#),
    )
    .unwrap();
    dir
}

/// SAFETY: the process env is shared, so every test using this is `#[serial]`.
fn set_home(home: &Path) {
    unsafe {
        std::env::set_var("WAYLAND_HOME", home);
    }
}

async fn catalog(cwd: &Path) -> Vec<SkillMetadata> {
    load_all_skills(cwd, &[], false, None).await
}

fn names(cat: &[SkillMetadata]) -> Vec<String> {
    cat.iter().map(|m| m.name.clone()).collect()
}

/// The load-bearing test. One variable changes between the two catalog loads — a
/// revocation — and the control skill must stay present across it.
///
/// Without the control, "the revoked skill is absent" is satisfied by a loader that
/// returned nothing, a `WAYLAND_HOME` pointing somewhere empty, or a fixture that never
/// wrote the files. A known-negative assertion is self-passing on a dead instrument, and
/// this is exactly that shape of assertion.
#[tokio::test]
#[serial]
async fn a_revoked_skill_leaves_the_catalog_while_its_neighbour_stays() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    install_draft(home.path(), "auto-keep", "the control");
    let drop_dir = install_draft(home.path(), "auto-drop", "the subject");

    // ---- KNOWN-POSITIVE: both load before anything is revoked. ----
    let before = names(&catalog(project.path()).await);
    assert!(
        before.iter().any(|n| n == "auto-keep"),
        "instrument dead: the control skill did not load at all, so an absence below \
         would prove nothing. Catalog: {before:?}"
    );
    assert!(
        before.iter().any(|n| n == "auto-drop"),
        "instrument dead: the subject skill did not load before revocation, so its \
         absence afterwards is not caused by the revocation. Catalog: {before:?}"
    );

    // ---- the single variable ----
    let store = GovernanceStore::open_default().unwrap();
    store.revoke(&drop_dir).unwrap();

    let after = names(&catalog(project.path()).await);
    assert!(
        after.iter().any(|n| n == "auto-keep"),
        "the control skill vanished too. Revocation removed more than it was asked to, \
         or the catalog collapsed for an unrelated reason. Catalog: {after:?}"
    );
    assert!(
        !after.iter().any(|n| n == "auto-drop"),
        "THE REVOKED SKILL IS STILL IN THE CATALOG. It remains resolvable by name and \
         executable through the user-invocable path, and its name still enters \
         `candidate_names`, which is what every router-hydration layer is scoped to. \
         Catalog: {after:?}"
    );
}

/// Revocation must not be defeated by putting the bytes back.
///
/// The drafter's trigger is designed to recur, so "delete the directory" is not a
/// revocation — the next qualifying streak recreates it. The tombstone has to keep
/// working against an artifact that has physically returned.
#[tokio::test]
#[serial]
async fn a_revoked_skill_stays_out_even_if_the_files_come_back() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    let dir = install_draft(home.path(), "auto-returns", "body");
    assert!(
        names(&catalog(project.path()).await)
            .iter()
            .any(|n| n == "auto-returns"),
        "instrument dead: subject never loaded"
    );

    let store = GovernanceStore::open_default().unwrap();
    store.revoke(&dir).unwrap();

    // Anything at all puts the bytes back: the drafter, a sync tool, a user copy.
    install_draft(home.path(), "auto-returns", "body");
    // Control in the same state: a freshly installed, never-revoked sibling DOES load,
    // so the absence below is the tombstone's doing and not a broken install path.
    install_draft(home.path(), "auto-fresh", "body");

    let after = names(&catalog(project.path()).await);
    assert!(
        after.iter().any(|n| n == "auto-fresh"),
        "control failed: a freshly installed draft did not load, so the subject's \
         absence proves nothing. Catalog: {after:?}"
    );
    assert!(
        !after.iter().any(|n| n == "auto-returns"),
        "a revoked skill returned to the catalog simply by having its files rewritten. \
         The tombstone is the durable statement of user intent and must outlive the \
         artifact. Catalog: {after:?}"
    );
}

/// Rollback restores the skill to the catalog — otherwise "undo" undoes nothing the
/// product can observe.
#[tokio::test]
#[serial]
async fn rollback_returns_the_skill_to_the_catalog_quarantined() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    let dir = install_draft(home.path(), "auto-back", "body");
    let store = GovernanceStore::open_default().unwrap();
    let rec = store.revoke(&dir).unwrap();

    assert!(
        !names(&catalog(project.path()).await)
            .iter()
            .any(|n| n == "auto-back"),
        "precondition failed: the skill was not actually revoked"
    );

    store.rollback(&rec.revocation_id).unwrap();

    let restored = catalog(project.path()).await;
    let found = restored
        .iter()
        .find(|m| m.name == "auto-back")
        .unwrap_or_else(|| {
            panic!(
                "rollback did not return the skill to the catalog; an undo the product \
                 cannot observe is not an undo. Catalog: {:?}",
                names(&restored)
            )
        });

    // Restored quarantined, not promoted: rollback returns the *exact prior state*, and
    // the prior state of an auto-drafted skill is not model-facing. A rollback that
    // restored it promoted would grant visibility nobody reviewed.
    assert!(
        found.disable_model_invocation,
        "rollback restored the skill un-quarantined. Rollback returns the prior state; \
         promotion is a separate, governed decision."
    );
}

/// Promotion lifts the quarantine — the whole point of the transaction — and the effect
/// is visible on the catalog the engine consumes.
#[tokio::test]
#[serial]
async fn promotion_lifts_quarantine_and_an_edit_puts_it_back() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    let dir = install_draft(home.path(), "auto-promoted", "body");
    let store = GovernanceStore::open_default().unwrap();

    // KNOWN-POSITIVE for the quarantine itself: a generated draft starts hidden. If it
    // did not, "promotion lifted the quarantine" would be true without promotion.
    let before = catalog(project.path()).await;
    let b = before.iter().find(|m| m.name == "auto-promoted").unwrap();
    assert!(
        b.disable_model_invocation,
        "instrument dead: a generated draft was not quarantined to begin with, so \
         lifting the quarantine below would prove nothing"
    );

    store
        .promote_existing(&dir, None, "test", &clearing_evidence())
        .expect("promote the draft");

    let after = catalog(project.path()).await;
    let a = after.iter().find(|m| m.name == "auto-promoted").unwrap();
    assert!(
        !a.disable_model_invocation,
        "promotion did not lift the quarantine, so a promoted skill is still invisible \
         to the model and promotion does nothing observable"
    );

    // One variable: the bytes.
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: auto-promoted\n---\n\nTAMPERED\n",
    )
    .unwrap();

    let edited = catalog(project.path()).await;
    let e = edited.iter().find(|m| m.name == "auto-promoted").unwrap();
    assert!(
        e.disable_model_invocation,
        "a promoted skill whose bytes changed stayed model-facing. The grant is bound to \
         a content digest precisely so unreviewed edits cannot inherit the review."
    );
}

/// Revoking a promoted skill must withdraw its grant, or the artifact returns
/// model-facing rather than quarantined — strictly worse than before the revocation.
#[tokio::test]
#[serial]
async fn revocation_withdraws_the_promotion_grant() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    let dir = install_draft(home.path(), "auto-both", "body");
    let store = GovernanceStore::open_default().unwrap();
    store
        .promote_existing(&dir, None, "test", &clearing_evidence())
        .unwrap();
    assert_eq!(
        store.promotions().unwrap().len(),
        1,
        "instrument dead: the grant was never written, so its later absence is free"
    );

    store.revoke(&dir).unwrap();
    assert_eq!(
        store.promotions().unwrap().len(),
        0,
        "the promotion grant outlived its revocation"
    );

    // Now put the artifact back and confirm it does NOT come back model-facing.
    install_draft(home.path(), "auto-both", "body");
    let after = catalog(project.path()).await;
    assert!(
        !after.iter().any(|m| m.name == "auto-both"),
        "a revoked-then-restored artifact re-entered the catalog. Catalog: {:?}",
        names(&after)
    );
}

/// Evidence that clears any threshold, for the tests in this file.
///
/// These cases are about the catalog effect of a grant, not about scoring, so they supply
/// the passing input explicitly rather than depending on how a fixture body happens to
/// score. The gate's own behaviour is exercised in `wcore-eval`'s
/// `tests/promotion_gate.rs` and in `promotion_refuses_below_threshold` below.
fn clearing_evidence() -> wcore_skills::promote::PromotionEvidence {
    wcore_skills::promote::PromotionEvidence {
        evaluator: "test".into(),
        score: 1.0,
        threshold: 0.65,
        verdict: "good".into(),
    }
}

/// The gate, at the governance boundary rather than at a caller.
///
/// `promote_existing` takes the evidence as a required argument, so no promotion path can
/// omit the check; this asserts the *refusal* half — that supplying failing evidence stops
/// the grant being written, rather than merely being recorded alongside it.
#[tokio::test]
#[serial]
async fn promotion_refuses_below_threshold() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    set_home(home.path());

    let dir = install_draft(home.path(), "auto-unscored", "body");
    let store = GovernanceStore::open_default().unwrap();

    // Known-positive in the same test: the identical call with clearing evidence DOES
    // promote. Without it, a refusal could equally be caused by anything else in the setup.
    let failing = wcore_skills::promote::PromotionEvidence {
        evaluator: "test".into(),
        score: 0.10,
        threshold: 0.65,
        verdict: "bad".into(),
    };
    let err = store
        .promote_existing(&dir, None, "test", &failing)
        .expect_err("a below-threshold artifact must not be promoted");
    assert!(
        err.to_string()
            .contains("below the 0.650 promotion threshold"),
        "the refusal must say what it refused on: {err}"
    );
    assert_eq!(
        store.promotions().unwrap().len(),
        0,
        "a refused promotion still wrote a grant"
    );
    let still = catalog(project.path()).await;
    let q = still.iter().find(|m| m.name == "auto-unscored").unwrap();
    assert!(
        q.disable_model_invocation,
        "a refused promotion left the artifact model-facing"
    );

    store
        .promote_existing(&dir, None, "test", &clearing_evidence())
        .expect("known-positive: the same call with clearing evidence must succeed");
    assert_eq!(store.promotions().unwrap().len(), 1);
}
