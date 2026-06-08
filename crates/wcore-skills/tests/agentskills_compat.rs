//! F19 compat: agentskills.io-SHAPED fixture skills parse cleanly through
//! wcore's loader and round-trip every field wcore currently understands.
//!
//! **Limitation (audit MEDIUM-2):** these fixtures are illustrative and
//! hand-authored, NOT pinned to an agentskills.io spec commit. They
//! document the shape wcore currently understands. Full spec-anchored
//! compat (with checked-in spec URL + commit hash + diff guard) is a
//! follow-up. Do NOT cite this test as proof of agentskills.io
//! compatibility in PR descriptions or shipping docs — cite it as
//! "the fields wcore currently round-trips" instead.

use std::path::PathBuf;

use wcore_skills::frontmatter::{parse_frontmatter, parse_skill_fields};
use wcore_skills::types::{LoadedFrom, SkillSource};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agentskills_io")
        .join(format!("{name}.md"))
}

fn load(name: &str) -> wcore_skills::types::SkillMetadata {
    let raw = std::fs::read_to_string(fixture_path(name)).unwrap();
    let parsed = parse_frontmatter(&raw);
    parse_skill_fields(
        &parsed.frontmatter,
        &parsed.content,
        name,
        SkillSource::Project,
        LoadedFrom::Skills,
        None,
    )
}

#[test]
fn basic_fixture_roundtrips_name_description_when_to_use() {
    let m = load("basic");
    assert_eq!(m.name, "basic");
    assert!(m.description.contains("minimal"));
    assert_eq!(
        m.when_to_use.as_deref(),
        Some("when testing baseline compatibility")
    );
}

#[test]
fn with_artifacts_fixture_parses_artifacts_field() {
    let m = load("with_artifacts");
    assert_eq!(m.artifacts.len(), 1);
    assert_eq!(m.artifacts[0].path, "out/report.md");
    assert!(m.artifacts[0].template.contains("${args.name}"));
}

#[test]
fn with_paths_fixture_parses_paths_for_conditional_activation() {
    let m = load("with_paths");
    assert!(m.paths.iter().any(|p| p.contains("*.rs")));
}
