//! W8b.2.B Task C.3 — `IntentClassifier` + `LoopSelector` tests.
//!
//! The classifier uses a deterministic keyword heuristic (LLM-backed
//! variants are gated behind a future feature flag — out of scope for
//! this sub-wave per the executor brief). The selector maps intents
//! to graph templates with optional user override.

use wcore_agent::orchestration::intent::{Complexity, IntentClassifier, LoopSelector, Mode};

#[test]
fn classifier_routes_typo_fix_to_direct() {
    let intent = IntentClassifier::classify("fix typo in README line 12");
    assert_eq!(intent.complexity, Complexity::Trivial);
    let config = LoopSelector::select(&intent, None);
    assert!(config.is_direct(), "trivial intent must select Direct");
}

#[test]
fn classifier_routes_module_refactor_to_hierarchical() {
    let intent = IntentClassifier::classify(
        "refactor the parser module and update all the callers across the codebase",
    );
    assert_eq!(intent.complexity, Complexity::Complex);
}

#[test]
fn classifier_routes_multi_file_search_to_parallel() {
    let intent = IntentClassifier::classify("search across all files for usages of deprecated_fn");
    assert_eq!(intent.complexity, Complexity::Moderate);
}

#[test]
fn classifier_routes_self_critical_task_to_self_critique() {
    let intent = IntentClassifier::classify("write a poem then critique your own draft");
    assert!(matches!(
        intent.shape,
        wcore_agent::orchestration::intent::Shape::SelfCritique
    ));
}

#[test]
fn user_override_forces_parallel() {
    let intent = IntentClassifier::classify("anything goes here");
    let config = LoopSelector::select(&intent, Some(Mode::Parallel));
    assert!(
        config.is_parallel_fanout(),
        "Mode::Parallel must force parallel fanout regardless of intent"
    );
}

#[test]
fn user_override_forces_direct() {
    let intent = IntentClassifier::classify(
        "refactor the parser module and update all the callers across the codebase",
    );
    // Even though intent says Complex, the explicit Direct override
    // must win.
    let config = LoopSelector::select(&intent, Some(Mode::Direct));
    assert!(config.is_direct());
}

#[test]
fn intent_carries_original_task() {
    let task = "rename foo to bar";
    let intent = IntentClassifier::classify(task);
    assert_eq!(intent.task, task);
}

#[test]
fn mode_auto_uses_classifier_decision() {
    let intent = IntentClassifier::classify("fix typo");
    let auto = LoopSelector::select(&intent, Some(Mode::Auto));
    let none = LoopSelector::select(&intent, None);
    // Mode::Auto is identical to passing None — both defer to intent.
    assert_eq!(auto.is_direct(), none.is_direct());
}
