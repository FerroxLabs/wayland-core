//! v0.6.4 Task 1.6 — skill delivery: `BundledSkillSpec` → `BundledSkillDefinition`.
//!
//! Bridges the isolation boundary between the plugin-api's owned-`String`
//! `BundledSkillSpec` and the engine's `&'static str`-fielded
//! `BundledSkillDefinition`.
//!
//! # Why `Box::leak` is correct here
//!
//! Plugin instances are loaded once at process startup and live for the entire
//! process lifetime — there is no unload path. The `BundledSkillSpec`'s own
//! doc comment (`wcore-plugin-api/src/bundled_skill_spec.rs:1`) explicitly
//! sanctions `Box::leak` for this reason: "plugin lifetime == process lifetime,
//! so the leak is acceptable per the bundled-skill registry's own static
//! design." The resulting `&'static str` pointers are valid for the process
//! lifetime, which is exactly what `BundledSkillDefinition` requires. This is
//! intentional and correct — not a defect.
//!
//! # Why the helper returns an owned `BundledSkillDefinition`
//!
//! `BundledSkillDefinition` is a struct of `&'static str` fields — the
//! `'static` bound lives on the *fields*, not on a reference to the struct.
//! `wcore_skills::bundled::register_bundled_skill` (`bundled/mod.rs:57`) takes
//! the definition **by value** (`fn register_bundled_skill(def:
//! BundledSkillDefinition)`). So this helper leaks each owned `String` field
//! into a `&'static str` and returns the owned struct directly — there is no
//! need (and no way) to also leak the struct itself; an outer `Box::leak`
//! would produce a `&'static BundledSkillDefinition` that `register_bundled_skill`
//! cannot consume.
//!
//! # Scope
//!
//! This module provides `spec_to_static_definition` only. Calling
//! `register_bundled_skill` from `bootstrap.rs` (and the ordering relative to
//! `load_catalog`) is Task 1.7's responsibility. Task 1.6 owns only the leak
//! helper and its test.

use wcore_plugin_api::BundledSkillSpec;
use wcore_skills::bundled::BundledSkillDefinition;

/// Convert a plugin-api `BundledSkillSpec` (owned `String` fields) into a
/// `BundledSkillDefinition` (`&'static str` fields) by leaking each owned
/// string into static memory.
///
/// # Safety rationale
///
/// `Box::leak` is used intentionally. Plugin skills are registered once at
/// process start and are never unregistered — their lifetime is the process
/// lifetime. Leaking is therefore the correct way to satisfy the `'static`
/// bound of `BundledSkillDefinition`'s fields without unsafe transmutes or a
/// separate string-interning store.
///
/// The returned `BundledSkillDefinition` is passed by value straight to
/// `wcore_skills::bundled::register_bundled_skill`, which stores it in a
/// process-global `OnceLock<Mutex<Vec<BundledSkillDefinition>>>`. The leaked
/// string allocations are reclaimed by the OS when the process exits.
pub fn spec_to_static_definition(spec: BundledSkillSpec) -> BundledSkillDefinition {
    // Leak each owned Option<String> field → Option<&'static str>.
    let name: &'static str = Box::leak(spec.name.into_boxed_str());
    let description: &'static str = Box::leak(spec.description.into_boxed_str());
    let when_to_use: Option<&'static str> = spec
        .when_to_use
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str);
    let argument_hint: Option<&'static str> = spec
        .argument_hint
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str);
    let model: Option<&'static str> = spec
        .model
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str);
    let context: Option<&'static str> = spec
        .context
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str);
    let agent: Option<&'static str> = spec
        .agent
        .map(|s| Box::leak(s.into_boxed_str()) as &'static str);
    let content: &'static str = Box::leak(spec.content.into_boxed_str());

    // Leak allowed_tools: Vec<String> → &'static [&'static str].
    let allowed_tools: &'static [&'static str] = {
        let leaked: Vec<&'static str> = spec
            .allowed_tools
            .into_iter()
            .map(|s| Box::leak(s.into_boxed_str()) as &'static str)
            .collect();
        Box::leak(leaked.into_boxed_slice())
    };

    // Leak files: Vec<(String, String)> → &'static [(&'static str, &'static str)].
    let files: &'static [(&'static str, &'static str)] = {
        let leaked: Vec<(&'static str, &'static str)> = spec
            .files
            .into_iter()
            .map(|(path, content)| {
                let p: &'static str = Box::leak(path.into_boxed_str());
                let c: &'static str = Box::leak(content.into_boxed_str());
                (p, c)
            })
            .collect();
        Box::leak(leaked.into_boxed_slice())
    };

    // The struct is returned by value — `register_bundled_skill` takes it by
    // value and moves it into the process-global registry. The `'static`
    // lifetime is carried entirely by the leaked `&'static str` fields above.
    BundledSkillDefinition {
        name,
        description,
        when_to_use,
        argument_hint,
        allowed_tools,
        model,
        disable_model_invocation: spec.disable_model_invocation,
        user_invocable: spec.user_invocable,
        context,
        agent,
        files,
        content,
    }
}
