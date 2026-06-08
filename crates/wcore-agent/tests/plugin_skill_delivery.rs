//! v0.6.4 Task 1.6 — skill delivery: BundledSkillSpec → BundledSkillDefinition.
//!
//! Tests the round-trip: a plugin-api `BundledSkillSpec` (owned Strings) run
//! through `spec_to_static_definition` produces a `BundledSkillDefinition`
//! (with leaked `&'static str` fields) whose fields match, and after
//! `register_bundled_skill` that definition appears in `get_bundled_skills()`.
//!
//! # Global-state note
//!
//! `wcore_skills::bundled::register_bundled_skill` writes a process-global
//! `OnceLock<Mutex<Vec>>`. The `clear_bundled_skills` helper is `#[cfg(test)]`
//! inside `wcore-skills` — it is NOT accessible to external test crates.
//! Strategy: use skill names that are guaranteed unique across this test binary
//! (UUID-suffix or a fixed long name that cannot collide with real bundled
//! skills). Assertions check for `any(|s| s.name == EXPECTED_NAME)` rather
//! than an exact registry length, so they are safe even if other bundled skills
//! (e.g., "hello") are present.

use wcore_agent::plugins::skill_delivery::spec_to_static_definition;
use wcore_plugin_api::BundledSkillSpec;
use wcore_skills::bundled::{get_bundled_skills, register_bundled_skill};

// A name that cannot collide with any real bundled skill.
const SKILL_NAME: &str = "tc-1-6-plugin-skill-delivery-unique-fixture-skill";

// ---------------------------------------------------------------------------
// Helper: build a fully-populated BundledSkillSpec.
// ---------------------------------------------------------------------------

fn fixture_spec() -> BundledSkillSpec {
    BundledSkillSpec {
        name: SKILL_NAME.into(),
        description: "TC-1.6 fixture skill — proves the leak bridge".into(),
        when_to_use: Some("when testing skill delivery".into()),
        argument_hint: Some("--fixture".into()),
        allowed_tools: vec!["Bash".into(), "Read".into()],
        model: Some("claude-sonnet".into()),
        disable_model_invocation: false,
        user_invocable: true,
        context: Some("inline".into()),
        agent: Some("fixture-agent".into()),
        files: vec![("guide.md".into(), "# guide".into())],
        content: "# TC-1.6 fixture skill content".into(),
    }
}

// ---------------------------------------------------------------------------
// TC-1.6-A: field fidelity — every spec field survives the leak bridge.
// ---------------------------------------------------------------------------

#[test]
fn tc_1_6_a_spec_to_static_definition_field_fidelity() {
    let spec = fixture_spec();
    let def: wcore_skills::bundled::BundledSkillDefinition = spec_to_static_definition(spec);

    assert_eq!(def.name, SKILL_NAME);
    assert_eq!(
        def.description,
        "TC-1.6 fixture skill — proves the leak bridge"
    );
    assert_eq!(def.when_to_use, Some("when testing skill delivery"));
    assert_eq!(def.argument_hint, Some("--fixture"));
    assert_eq!(def.allowed_tools, &["Bash", "Read"]);
    assert_eq!(def.model, Some("claude-sonnet"));
    assert!(!def.disable_model_invocation);
    assert!(def.user_invocable);
    assert_eq!(def.context, Some("inline"));
    assert_eq!(def.agent, Some("fixture-agent"));
    assert_eq!(def.files, &[("guide.md", "# guide")]);
    assert_eq!(def.content, "# TC-1.6 fixture skill content");
}

// ---------------------------------------------------------------------------
// TC-1.6-B: round-trip — spec → leak → register → appears in get_bundled_skills().
// ---------------------------------------------------------------------------

#[test]
fn tc_1_6_b_round_trip_register_and_get() {
    // Use a distinct name from TC-A so the two tests don't interfere even if
    // both run in the same process and the registry is not cleared between them.
    const RT_NAME: &str = "tc-1-6-b-round-trip-unique-skill";

    let spec = BundledSkillSpec {
        name: RT_NAME.into(),
        description: "round-trip proof".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: true,
        context: None,
        agent: None,
        files: vec![],
        content: "round-trip content".into(),
    };

    let def = spec_to_static_definition(spec);

    // Register the leaked definition into the global bundled-skill registry.
    register_bundled_skill(def);

    // The skill must now appear in get_bundled_skills().
    let skills = get_bundled_skills();
    let found = skills.iter().find(|s| s.name == RT_NAME);
    assert!(
        found.is_some(),
        "round-trip skill '{RT_NAME}' must appear in get_bundled_skills() after register_bundled_skill"
    );

    let meta = found.unwrap();
    assert_eq!(meta.description, "round-trip proof");
    assert_eq!(meta.content, "round-trip content");
    assert!(meta.user_invocable);
}

// ---------------------------------------------------------------------------
// TC-1.6-C: None optional fields survive as None in the definition.
// ---------------------------------------------------------------------------

#[test]
fn tc_1_6_c_none_optionals_stay_none() {
    let spec = BundledSkillSpec {
        name: "tc-1-6-c-none-optionals-unique".into(),
        description: "minimal".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: true,
        user_invocable: false,
        context: None,
        agent: None,
        files: vec![],
        content: "min".into(),
    };

    let def = spec_to_static_definition(spec);

    assert_eq!(def.when_to_use, None);
    assert_eq!(def.argument_hint, None);
    assert_eq!(def.allowed_tools, &[] as &[&str]);
    assert_eq!(def.model, None);
    assert!(def.disable_model_invocation);
    assert!(!def.user_invocable);
    assert_eq!(def.context, None);
    assert_eq!(def.agent, None);
    assert_eq!(def.files, &[] as &[(&str, &str)]);
}

// ---------------------------------------------------------------------------
// TC-1.6-D: the returned definition is truly 'static (compile-time proof).
//
// This test exists purely to exercise the 'static bound at the type level.
// `BundledSkillDefinition` only satisfies `T: 'static` if every field is
// itself `'static` — i.e. every `&str` field is `&'static str`. If the leak
// helper produced shorter-lived `&str` fields, `assert_static` would not
// compile.
// ---------------------------------------------------------------------------

fn assert_static<T: 'static>(_: &T) {}

#[test]
fn tc_1_6_d_returned_definition_is_static() {
    let spec = BundledSkillSpec {
        name: "tc-1-6-d-static-check-unique".into(),
        description: "static".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: false,
        context: None,
        agent: None,
        files: vec![],
        content: "s".into(),
    };

    let def: wcore_skills::bundled::BundledSkillDefinition = spec_to_static_definition(spec);
    // The definition's fields are all 'static, so the whole struct is 'static.
    assert_static(&def);
    assert_eq!(def.name, "tc-1-6-d-static-check-unique");
}
