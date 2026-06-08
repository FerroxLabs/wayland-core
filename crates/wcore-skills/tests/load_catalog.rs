//! Tests for X1's `load_catalog` — the ref-only sibling of `load_all_skills`.

use std::fs;

use tempfile::TempDir;
use wcore_skills::loader::load_catalog;
use wcore_skills::refs::SkillCatalog;

fn make_project_with_skills(count: usize) -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    let dir = root.join(".wayland-core").join("skills");
    fs::create_dir_all(&dir).unwrap();
    for i in 0..count {
        let skill_dir = dir.join(format!("skill-{i}"));
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: skill-{i}\ndescription: Test skill {i}\nwhen-to-use: when testing {i}\n---\n\nBody {i}\n",
            ),
        )
        .unwrap();
    }
    tmp
}

#[tokio::test]
async fn load_catalog_returns_one_ref_per_skill_file() {
    let tmp = make_project_with_skills(5);
    // Use `bare = true` + add_dirs pointing at the fixture so user-level
    // skills outside the temp dir don't leak into the test.
    // additional_skills_dirs appends .wayland-core/skills/ to each entry,
    // so pass the project root, not the skills subdir.
    let add = vec![tmp.path().to_path_buf()];
    let refs = load_catalog(tmp.path(), &add, true, None).await;
    let catalog: SkillCatalog = SkillCatalog::from_refs(refs);
    assert!(
        catalog.len() >= 5,
        "expected at least 5 refs from fixture, got {}",
        catalog.len()
    );
    let names: Vec<String> = catalog.refs().map(|r| r.name.clone()).collect();
    for i in 0..5 {
        let expected = format!("skill-{i}");
        assert!(
            names.iter().any(|n| n == &expected),
            "missing {expected} in {names:?}"
        );
    }
}

#[tokio::test]
async fn load_catalog_carries_listing_fields_only() {
    let tmp = make_project_with_skills(1);
    // additional_skills_dirs appends .wayland-core/skills/ to each entry,
    // so pass the project root, not the skills subdir.
    let add = vec![tmp.path().to_path_buf()];
    let refs = load_catalog(tmp.path(), &add, true, None).await;
    let r = refs.iter().find(|r| r.name == "skill-0").expect("skill-0");
    assert_eq!(r.description, "Test skill 0");
    assert_eq!(r.when_to_use.as_deref(), Some("when testing 0"));
    // file_path points at the SKILL.md so resolve() can read it.
    assert!(
        r.file_path.ends_with("SKILL.md"),
        "expected SKILL.md path, got {:?}",
        r.file_path
    );
    // content_length_hint is non-zero for a non-empty body.
    assert!(r.content_length_hint > 0);
}
