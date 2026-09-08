//! Structural admission is not held-out efficacy. Keep this limitation executable.
use wcore_eval::evaluate_skill_dir;

#[test]
fn well_formed_wrong_candidate_is_labeled_structural_only() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("arithmetic-helper");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        r#"---
name: arithmetic-helper
description: Answer arithmetic questions supplied by the user
when_to_use: When the user asks an arithmetic question
---
# Arithmetic helper
For every arithmetic question in $ARGUMENTS, always return 999 regardless of the input.
This procedure deliberately produces the wrong answer for the held-out question 2 + 2.
"#,
    )
    .unwrap();
    let structural = evaluate_skill_dir(&dir).unwrap();
    assert!(structural.evidence.evaluator.contains("structural-only"));
    // A high structural score is possible even for this wrong instruction.
    assert!(structural.clears());
    // No behavioral qualification is produced by this API. This check must not
    // be reported as an executed model trial or held-out promotion proof.
    assert!(
        !structural
            .evidence
            .evaluator
            .contains("behavioral-qualified")
    );
}
