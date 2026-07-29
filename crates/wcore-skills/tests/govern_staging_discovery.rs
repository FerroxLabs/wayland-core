//! F23A-C1-H4: where a rollback stages its restore, and whether the loader can see it.
//!
//! `GovernanceStore::rollback` builds the restored tree in
//! `promote::staging_root_for(parent)` and publishes it with one `rename(2)`. That closes
//! F23A-C1-H3 for the *target* directory. This file asks the next question: what happens to
//! the **staging** directory when a restore dies inside that window.
//!
//! `staging_root_for` is `skills_root.parent().join(".promote-staging")`, and `rollback`
//! passes it the **skill's** parent. For a flat skill `<root>/<name>` that is `<root>`, so
//! staging lands at `<root>/../.promote-staging` — outside the skills tree, exactly as the
//! comment in `govern.rs` intends.
//!
//! But the auto-drafter does not write flat skills. It writes
//! `$WAYLAND_HOME/skills/auto/auto-<sig>/` (`paths::wayland_home_skills_dirs` returns
//! `skills/auto` *and* `skills`, and `collect_skill_md` recurses). For that layout the
//! skill's parent is `skills/auto`, so staging resolves to **`skills/.promote-staging`** —
//! inside the skills root the loader walks.
//!
//! That matters because `govern.rs` states the hazard itself: *"`collect_skill_md` does not
//! skip dot-directories -- a half-built staging directory holding a `SKILL.md` inside the
//! skills root would be discovered and loaded."* The mitigation chosen was to stage outside
//! the tree; for the one layout this module exists to govern, it does not.
//!
//! Both tests below carry a live control in the same invocation, because each is a negative
//! and LANE-BRIEF §3b-i is explicit that a negative passes for free on a dead instrument.

use std::path::{Path, PathBuf};

use wcore_skills::govern::GovernanceStore;

/// The staging directory name `promote.rs` uses. Duplicated as a literal on purpose: if the
/// constant is renamed, this test should fail loudly rather than silently follow it and stop
/// testing the thing it was written for.
const STAGING_DIR: &str = ".promote-staging";

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: fixture\n---\nbody\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        format!(r#"{{"name":"{name}","auto_drafted":true,"signature":"sig-{name}"}}"#),
    )
    .unwrap();
}

/// A project root laid out the way `additional_skills_dirs` expects: it maps each `--add-dir`
/// to `<dir>/.wayland-core/skills`, so the ROOT is what gets passed, not the skills directory.
/// Getting that wrong makes the loader return `[]` and every negative below pass for free.
struct Proj {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    skills: PathBuf,
}

fn proj() -> Proj {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let skills = root.join(".wayland-core").join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    Proj {
        _tmp: tmp,
        root,
        skills,
    }
}

async fn load_names(p: &Proj) -> Vec<String> {
    wcore_skills::loader::load_all_skills(&p.root, std::slice::from_ref(&p.root), true, None)
        .await
        .into_iter()
        .map(|s| s.name)
        .collect()
}

/// **The consequence.** A half-built staging tree inside the skills root, exactly as a kill
/// mid-restore leaves one, must not enter the catalog.
#[tokio::test]
async fn a_staging_tree_inside_the_skills_root_is_not_discovered_as_a_skill() {
    let p = proj();

    // Known-positive, driven through the same loader call.
    write_skill(
        &p.skills.join("auto").join("control-visible"),
        "control-visible",
    );

    // The state a kill inside `rollback` leaves for a namespaced (auto-drafted) skill.
    write_skill(
        &p.skills.join(STAGING_DIR).join("0f8b-uuid-like"),
        "half-restored",
    );

    let names = load_names(&p).await;

    assert!(
        names.iter().any(|n| n.contains("control-visible")),
        "KNOWN-POSITIVE FAILED: the loader found nothing, so the negative below proves \
         nothing. loaded = {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains("half-restored")),
        "a half-built rollback staging tree was discovered as a skill: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.contains(STAGING_DIR)),
        "the staging directory was discovered under its own name: {names:?}"
    );
}

/// **The cause, asserted as the fact it is.** For the layout the auto-drafter actually
/// writes, staging lands *inside* the skills root rather than beside it.
///
/// This test asserts the behaviour rather than demanding it change, because the location
/// cannot be guaranteed in general (see `promote::STAGING`): `rename(2)` needs the staging
/// area on the target's filesystem, and skills roots nest arbitrarily. The fix is the
/// loader's name fence, verified by the test above. **This test is what makes that fence
/// load-bearing rather than defensive** — if a later change moved staging genuinely outside
/// every skills root, this test would fail and the fence could then be reconsidered on
/// evidence instead of assumption.
///
/// Driven through the real `rollback`, not by hand: a payload deeper than `copy_tree`'s depth
/// cap makes the restore fail after `create_dir_all` has made the staging directory and
/// before the `rename`, which is precisely the window a kill lands in.
#[test]
fn a_failed_restore_of_a_namespaced_skill_stages_inside_the_skills_root() {
    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    let namespaced = skills.join("auto");
    std::fs::create_dir_all(&namespaced).unwrap();
    let store = GovernanceStore::new(tmp.path().join("skills-governance"));

    let dir = namespaced.join("auto-nested");
    write_skill(&dir, "auto-nested");
    let rec = store.revoke(&dir).unwrap();

    // Control, in the same test: a *flat* skill stages outside the skills root, which is what
    // the design intends and what makes the namespaced result below a real difference rather
    // than a property of the harness.
    let flat = skills.join("auto-flat");
    write_skill(&flat, "auto-flat");
    let flat_rec = store.revoke(&flat).unwrap();

    // Make each restore fail inside the staging window by planting an over-deep payload.
    for id in [&rec.revocation_id, &flat_rec.revocation_id] {
        let payload = store.root().join("generations").join(id).join("payload");
        let mut deep = payload.clone();
        for i in 0..10 {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("deep.txt"), "too deep\n").unwrap();
        store
            .rollback(id)
            .expect_err("an over-deep payload must not restore");
    }

    let inside = skills.join(STAGING_DIR);
    let outside = tmp.path().join(STAGING_DIR);

    assert!(
        outside.is_dir(),
        "CONTROL FAILED: a flat skill did not stage at {} either, so the assertion below \
         is not measuring a namespaced-vs-flat difference",
        outside.display()
    );
    assert!(
        inside.is_dir(),
        "F23A-C1-H4 appears to have been fixed at the source: a namespaced skill no longer \
         stages at {}. If staging is now genuinely outside every skills root, the loader's \
         name fence in `collect_skill_md` can be revisited -- but only with a measurement \
         like this one, never by assumption.",
        inside.display()
    );

    // The staging tree is left behind (the train's `rollback` only cleans up on a rename
    // failure, not a copy failure), which is exactly why the loader fence has to hold for
    // the whole life of the profile and not merely for the width of a restore.
    assert!(
        std::fs::read_dir(&inside)
            .map(|rd| rd.count() > 0)
            .unwrap_or(false),
        "a failed restore left no staging content, so the discovery risk this file \
         documents would not arise"
    );
}
