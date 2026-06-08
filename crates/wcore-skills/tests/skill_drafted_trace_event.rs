//! F10.D — TraceEvent payload shape for `skill_drafted`.

use serde_json::json;
use wcore_skills::draft::{DraftCandidate, render_skill_drafted_payload};

#[test]
fn skill_drafted_payload_shape() {
    let c = DraftCandidate {
        tool_sequence: vec![
            "Grep".into(),
            "Read".into(),
            "Edit".into(),
            "Bash".into(),
            "Bash".into(),
        ],
        input_shape: vec![vec![]; 5],
        repeat_count: 3,
        suggested_name: "auto-grep-read-edit-bash-bash".into(),
        suggested_description: "x".into(),
    };
    let payload = render_skill_drafted_payload(&c);
    assert_eq!(payload["kind"], "skill_drafted");
    assert_eq!(payload["name"], "auto-grep-read-edit-bash-bash");
    assert_eq!(payload["repeat_count"], 3);
    assert_eq!(
        payload["tool_sequence"],
        json!(["Grep", "Read", "Edit", "Bash", "Bash"])
    );
}
