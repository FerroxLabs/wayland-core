use super::{BundledSkillCatalog, BundledSkillDefinition, register_bundled_skill};

/// Register the built-in "hello" skill used to validate the bundled skill framework.
pub fn register_hello_skill(catalog: &mut BundledSkillCatalog) {
    register_bundled_skill(
        catalog,
        BundledSkillDefinition {
            name: "hello",
            description: "A simple greeting skill for testing the bundled skill framework.",
            content: "Hello! I'm a bundled skill. How can I help you today?\n\n$ARGUMENTS",
            user_invocable: true,
            when_to_use: None,
            argument_hint: None,
            allowed_tools: &[],
            model: None,
            disable_model_invocation: false,
            context: None,
            agent: None,
            files: &[],
        },
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::BundledSkillCatalog;
    use super::register_hello_skill;

    // TC-10.18: hello skill fields are correct
    #[test]
    fn tc_10_18_hello_skill_fields_correct() {
        let mut catalog = BundledSkillCatalog::new();
        register_hello_skill(&mut catalog);
        let skills = catalog.get_bundled_skills();
        let hello = skills
            .iter()
            .find(|s| s.name == "hello")
            .expect("hello skill should be registered");
        assert!(hello.user_invocable, "hello should be user_invocable");
        assert!(
            !hello.description.is_empty(),
            "hello should have a non-empty description"
        );
    }
}
