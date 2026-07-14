//! Plugin skill delivery into an owned, session-local bundled catalog.

use wcore_agent::plugins::skill_delivery::spec_to_bundled_entry;
use wcore_plugin_api::BundledSkillSpec;
use wcore_skills::bundled::BundledSkillCatalog;

const SKILL_NAME: &str = "tc-1-6-plugin-skill-delivery-unique-fixture-skill";

fn fixture_spec() -> BundledSkillSpec {
    BundledSkillSpec {
        name: SKILL_NAME.into(),
        description: "TC-1.6 fixture skill — proves the owned bridge".into(),
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

#[test]
fn tc_1_6_a_spec_to_owned_entry_field_fidelity() {
    let entry = spec_to_bundled_entry(fixture_spec());

    assert_eq!(entry.name, SKILL_NAME);
    assert_eq!(
        entry.description,
        "TC-1.6 fixture skill — proves the owned bridge"
    );
    assert_eq!(
        entry.when_to_use.as_deref(),
        Some("when testing skill delivery")
    );
    assert_eq!(entry.argument_hint.as_deref(), Some("--fixture"));
    assert_eq!(
        entry.allowed_tools,
        vec!["Bash".to_owned(), "Read".to_owned()]
    );
    assert_eq!(entry.model.as_deref(), Some("claude-sonnet"));
    assert!(!entry.disable_model_invocation);
    assert!(entry.user_invocable);
    assert_eq!(entry.context.as_deref(), Some("inline"));
    assert_eq!(entry.agent.as_deref(), Some("fixture-agent"));
    assert_eq!(
        entry.files,
        vec![("guide.md".to_owned(), "# guide".to_owned())]
    );
    assert_eq!(entry.content, "# TC-1.6 fixture skill content");
}

#[test]
fn tc_1_6_b_round_trip_register_and_get() {
    let mut catalog = BundledSkillCatalog::new();
    catalog.register(spec_to_bundled_entry(BundledSkillSpec {
        name: "tc-1-6-b-round-trip-unique-skill".into(),
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
    }));

    let skills = catalog.get_bundled_skills();
    let meta = skills
        .iter()
        .find(|skill| skill.name == "tc-1-6-b-round-trip-unique-skill")
        .expect("plugin skill should be present in its catalog");
    assert_eq!(meta.description, "round-trip proof");
    assert_eq!(meta.content, "round-trip content");
    assert!(meta.user_invocable);
}

#[test]
fn tc_1_6_c_none_optionals_stay_none() {
    let entry = spec_to_bundled_entry(BundledSkillSpec {
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
    });

    assert_eq!(entry.when_to_use, None);
    assert_eq!(entry.argument_hint, None);
    assert!(entry.allowed_tools.is_empty());
    assert_eq!(entry.model, None);
    assert!(entry.disable_model_invocation);
    assert!(!entry.user_invocable);
    assert_eq!(entry.context, None);
    assert_eq!(entry.agent, None);
    assert!(entry.files.is_empty());
}

#[test]
fn tc_1_6_d_entry_owns_runtime_strings() {
    let runtime_name = format!("owned-{SKILL_NAME}");
    let runtime_content = format!("content-for-{SKILL_NAME}");
    let entry = spec_to_bundled_entry(BundledSkillSpec {
        name: runtime_name.clone(),
        description: "owned".into(),
        when_to_use: None,
        argument_hint: None,
        allowed_tools: vec![],
        model: None,
        disable_model_invocation: false,
        user_invocable: false,
        context: None,
        agent: None,
        files: vec![],
        content: runtime_content.clone(),
    });

    assert_eq!(entry.name, runtime_name);
    assert_eq!(entry.content, runtime_content);
}
