//! X1: bootstrap uses Arc<SkillCatalog>; skill bodies are not pinned after
//! load_catalog returns. Activating a skill via SkillTool resolves the body
//! lazily via SkillCatalog::resolve.

use std::fs;
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use wcore_agent::skill_tool::SkillTool;
use wcore_skills::loader::load_catalog;
use wcore_skills::permissions::SkillPermissionChecker;
use wcore_skills::refs::SkillCatalog;
use wcore_tools::Tool;

fn make_fixture(count: usize) -> TempDir {
    let tmp = TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    let dir = tmp.path().join(".wayland-core").join("skills");
    fs::create_dir_all(&dir).unwrap();
    for i in 0..count {
        let d = dir.join(format!("s{i}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("SKILL.md"),
            format!("---\nname: s{i}\ndescription: skill {i}\n---\n\nBODY-{i}-ABC-XYZ\n"),
        )
        .unwrap();
    }
    tmp
}

#[tokio::test]
async fn catalog_carries_no_body_text_in_refs() {
    let tmp = make_fixture(10);
    let refs = load_catalog(tmp.path(), &[tmp.path().to_path_buf()], true, None).await;
    // SkillRef has no `content` field — confirm by reading description only.
    assert!(refs.iter().any(|r| r.name == "s5"));
    for r in &refs {
        // Listing-shape only.
        assert!(r.description.len() < 250);
    }
    assert!(refs.len() >= 10);
}

#[tokio::test]
async fn skill_tool_invocation_resolves_body_lazily() {
    let tmp = make_fixture(3);
    let refs = load_catalog(tmp.path(), &[tmp.path().to_path_buf()], true, None).await;
    let catalog = Arc::new(SkillCatalog::from_refs(refs));
    let tool = SkillTool::new(
        Arc::clone(&catalog),
        tmp.path().to_string_lossy().to_string(),
        SkillPermissionChecker::new(vec![], vec![], false),
    );

    let result = tool.execute(json!({"skill": "s1"})).await;
    assert!(
        !result.is_error,
        "expected ok skill invocation, got: {}",
        result.content
    );
    assert!(
        result.content.contains("BODY-1-ABC-XYZ"),
        "resolved body must reach the result content, got: {}",
        result.content
    );
}
