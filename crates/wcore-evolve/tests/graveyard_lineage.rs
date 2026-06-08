//! Graveyard writer test: loser entries record full lineage on disk.

use tempfile::tempdir;
use wcore_evolve::evolve::{LoserEntry, graveyard};

#[test]
fn loser_entry_has_full_lineage() {
    let dir = tempdir().expect("tempdir");
    let loser = LoserEntry {
        run_id: "run-1".into(),
        generation: 2,
        child_index: 3,
        parent_id: "skill-refactor-imports".into(),
        mutation_kind: "Reorder".into(),
        score: 0.42,
        body_excerpt: "...".into(),
    };
    graveyard::write(dir.path(), &loser).expect("write ok");

    let path = dir.path().join("run-1/2/3.json");
    let raw = std::fs::read_to_string(&path).expect("read graveyard file");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    assert_eq!(parsed["parent_id"], "skill-refactor-imports");
    assert_eq!(parsed["mutation_kind"], "Reorder");
    assert_eq!(parsed["generation"], 2);
    assert_eq!(parsed["child_index"], 3);
    let f = parsed["score"].as_f64().expect("score is number");
    assert!((f - 0.42).abs() < 1e-9);
}
