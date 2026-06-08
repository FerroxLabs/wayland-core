//! F19: audit_corpus produces an AuditReport identifying stale, duplicate,
//! and broken-ref skills.

use std::fs;
use std::time::SystemTime;

use tempfile::TempDir;
use wcore_skills::audit::{AuditFinding, AuditOpts, audit_corpus};
use wcore_skills::loader::load_catalog;

fn make_corpus() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    fs::create_dir(root.join(".git")).unwrap();
    let dir = root.join(".wayland-core").join("skills");
    fs::create_dir_all(&dir).unwrap();

    // Healthy skill
    let healthy = dir.join("healthy");
    fs::create_dir_all(&healthy).unwrap();
    fs::write(
        healthy.join("SKILL.md"),
        "---\nname: healthy\ndescription: A healthy skill\n---\n\nBody\n",
    )
    .unwrap();

    // Duplicate-description pair.
    let dupe_a = dir.join("dupe-a");
    fs::create_dir_all(&dupe_a).unwrap();
    fs::write(
        dupe_a.join("SKILL.md"),
        "---\nname: dupe-a\ndescription: Run a database migration script\n---\n\nBody A\n",
    )
    .unwrap();
    let dupe_b = dir.join("dupe-b");
    fs::create_dir_all(&dupe_b).unwrap();
    fs::write(
        dupe_b.join("SKILL.md"),
        "---\nname: dupe-b\ndescription: Run a database migration script\n---\n\nBody B\n",
    )
    .unwrap();

    // Stale skill: backdate mtime by 1 year.
    let stale = dir.join("stale");
    fs::create_dir_all(&stale).unwrap();
    let stale_md = stale.join("SKILL.md");
    fs::write(
        &stale_md,
        "---\nname: stale\ndescription: Untouched\n---\n\nold\n",
    )
    .unwrap();
    let old = SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 365);
    let _ = filetime::set_file_mtime(&stale_md, filetime::FileTime::from_system_time(old));

    // Broken-ref skill: artifacts path escapes root.
    let broken = dir.join("broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(
        broken.join("SKILL.md"),
        "---\nname: broken\ndescription: has bad artifact\nartifacts:\n  - path: ../../../etc/evil\n    template: x\n---\n\nBody\n",
    )
    .unwrap();

    tmp
}

#[tokio::test]
async fn audit_flags_stale_duplicate_and_broken_ref() {
    let tmp = make_corpus();
    let refs = load_catalog(tmp.path(), &[tmp.path().to_path_buf()], true, None).await;
    let opts = AuditOpts {
        stale_after_days: 180,
        duplicate_description_distance: 5,
    };
    let report = audit_corpus(&refs, &opts);

    let kinds: Vec<&str> = report
        .findings
        .iter()
        .map(|f| match f {
            AuditFinding::Stale { .. } => "stale",
            AuditFinding::Duplicate { .. } => "dup",
            AuditFinding::BrokenRef { .. } => "broken",
        })
        .collect();
    assert!(kinds.contains(&"stale"), "missing stale: {kinds:?}");
    assert!(kinds.contains(&"dup"), "missing dup: {kinds:?}");
    assert!(kinds.contains(&"broken"), "missing broken: {kinds:?}");
}

#[tokio::test]
async fn audit_report_serialises_to_stable_json() {
    let tmp = make_corpus();
    let refs = load_catalog(tmp.path(), &[tmp.path().to_path_buf()], true, None).await;
    let report = audit_corpus(&refs, &AuditOpts::default());
    let value = serde_json::to_value(&report).unwrap();
    assert!(value.get("findings").is_some());
    assert!(value.get("audited_at").is_some());
    assert!(value.get("total_skills").is_some());
}

#[tokio::test]
async fn audit_render_markdown_contains_section_headers() {
    let tmp = make_corpus();
    let refs = load_catalog(tmp.path(), &[tmp.path().to_path_buf()], true, None).await;
    let report = audit_corpus(&refs, &AuditOpts::default());
    let md = wcore_skills::audit::render_markdown(&report);
    assert!(md.contains("# Skills Audit"));
    assert!(md.contains("## Findings"));
    assert!(md.contains("Total skills:"));
}
