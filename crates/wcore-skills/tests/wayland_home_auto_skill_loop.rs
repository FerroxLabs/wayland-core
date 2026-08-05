//! Closes the auto-skill on-disk loop: a `SKILL.md` the `SkillDrafter` writes
//! under `$WAYLAND_HOME/skills/auto/<name>/` must be discoverable by the
//! loader on the next session, so a skill "learned" in session 1 is usable in
//! session 2.
//!
//! These tests pin `$WAYLAND_HOME` at a tempdir (hermetic) and assert that the
//! drafter's write path is on the loader's read path. The env var is process-
//! global, so the tests are serialized via `serial_test` (other suites read
//! `WAYLAND_HOME` through `app_config_dir`).

use std::fs;
use std::path::Path;

use serial_test::serial;
use tempfile::TempDir;
use wcore_skills::loader::{load_all_skills, load_catalog};

/// Pin `$WAYLAND_HOME` for the test body and restore the prior value on drop.
/// Env mutation is `unsafe`; the `#[serial]` attribute guarantees no other
/// thread observes the mutated env concurrently.
struct WaylandHomeGuard {
    prev: Option<String>,
}
impl WaylandHomeGuard {
    fn set(dir: &Path) -> Self {
        let prev = std::env::var("WAYLAND_HOME").ok();
        unsafe {
            std::env::set_var("WAYLAND_HOME", dir);
        }
        Self { prev }
    }
}
impl Drop for WaylandHomeGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
    }
}

/// Write a loadable draft exactly where the `SkillDrafter` legacy path writes:
/// `$WAYLAND_HOME/skills/auto/<name>/SKILL.md`.
fn write_auto_skill(home: &Path, name: &str) {
    let dir = home.join("skills").join("auto").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: Auto-drafted recall skill\nwhen-to-use: when the task resembles the learned signature\n---\n\nApproach derived from prior successful turns.\n",
        ),
    )
    .unwrap();
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn auto_drafted_skill_is_loaded_next_session() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_auto_skill(home.path(), "auto-code-refactor");

    // A non-tempdir cwd with no project skills keeps the discovery focused on
    // the `$WAYLAND_HOME` read path. `bare = false` so user-tier dirs (incl.
    // the new `wayland_home_skills_dirs`) are consulted.
    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;

    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-code-refactor"));
    assert!(
        hit.is_some(),
        "auto-drafted skill under $WAYLAND_HOME/skills/auto/ was not loaded; got: {:?}",
        skills.iter().map(|s| &s.name).collect::<Vec<_>>()
    );
    assert_eq!(hit.unwrap().description, "Auto-drafted recall skill");
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn auto_drafted_skill_appears_in_catalog() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_auto_skill(home.path(), "auto-test-writer");

    let cwd = TempDir::new().unwrap();
    let refs = load_catalog(cwd.path(), &[], false, None).await;

    let hit = refs.iter().find(|r| r.name.ends_with("auto-test-writer"));
    assert!(
        hit.is_some(),
        "auto-drafted skill missing from load_catalog; got: {:?}",
        refs.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    // The ref must carry the on-disk SKILL.md path so resolve() can read the body.
    assert!(
        hit.unwrap().file_path.ends_with("SKILL.md"),
        "expected SKILL.md path, got {:?}",
        hit.unwrap().file_path
    );
}

/// Write an auto-drafted skill WITH the `SkillDrafter`'s sibling `manifest.json`.
fn write_auto_skill_with_manifest(home: &Path, name: &str, needs_review: bool) {
    let dir = home.join("skills").join("auto").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Auto-drafted recall skill\n---\n\nBody.\n"),
    )
    .unwrap();
    fs::write(
        dir.join("manifest.json"),
        format!("{{\"auto_drafted\":true,\"needs_review\":{needs_review},\"name\":\"{name}\"}}"),
    )
    .unwrap();
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn unreviewed_auto_draft_loads_but_is_hidden_from_the_model() {
    // Regression: an auto-drafted skill the drafter wrote from trivial/test
    // turns leaked into the model's catalog and got narrated in user-facing
    // output. An unreviewed draft (`needs_review: true`) must NOT be
    // model-invocable, yet must still LOAD so the user can review/invoke it.
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_auto_skill_with_manifest(home.path(), "auto-needs-review", true);

    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;

    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-needs-review"))
        .expect("an unreviewed draft must still be loaded (for review/invocation)");
    assert!(
        hit.disable_model_invocation,
        "an unreviewed auto-draft (needs_review=true) must be hidden from the model"
    );
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn generated_provenance_stays_quarantined_when_review_flag_is_false() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_auto_skill_with_manifest(home.path(), "auto-reviewed", false);

    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;

    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-reviewed"))
        .expect("a reviewed draft loads");
    assert!(
        hit.disable_model_invocation,
        "F06 quarantines generated provenance until governed F23 promotion"
    );
}

fn write_released_generated_body(home: &Path, name: &str, manifest: Option<&str>) {
    let dir = home.join("skills").join("auto").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!(
            "# Auto-drafted skill: {name}\n\n\
             > NOTE: This skill was auto-drafted from a streak of successful turns with similar \
             task signatures. Review and edit before treating it as canonical.\n\n\
             Signature: `read-write`\n"
        ),
    )
    .unwrap();
    if let Some(body) = manifest {
        fs::write(dir.join("manifest.json"), body).unwrap();
    }
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn released_manifestless_generated_body_is_quarantined() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_released_generated_body(home.path(), "auto-read-write", None);

    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;
    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-read-write"))
        .expect("released generated draft remains inspectable");
    assert!(
        hit.disable_model_invocation,
        "a released manifest-less generated draft must fail closed"
    );
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn malformed_manifest_does_not_unquarantine_released_generated_body() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_released_generated_body(home.path(), "auto-malformed", Some("{not-json"));

    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;
    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-malformed"))
        .expect("generated draft with damaged metadata remains inspectable");
    assert!(
        hit.disable_model_invocation,
        "damaged metadata must not turn generated content into a model-visible skill"
    );
}

#[tokio::test]
#[serial(wayland_home_env)]
async fn user_authored_auto_prefixed_skill_remains_visible() {
    let home = TempDir::new().unwrap();
    let _guard = WaylandHomeGuard::set(home.path());
    write_auto_skill(home.path(), "auto-user-authored");

    let cwd = TempDir::new().unwrap();
    let skills = load_all_skills(cwd.path(), &[], false, None).await;
    let hit = skills
        .iter()
        .find(|s| s.name.ends_with("auto-user-authored"))
        .expect("ordinary auto-prefixed skill loads");
    assert!(
        !hit.disable_model_invocation,
        "name shape alone must not classify user content as generated"
    );
}
