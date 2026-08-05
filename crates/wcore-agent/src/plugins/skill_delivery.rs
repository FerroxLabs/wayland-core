//! Host-side plugin skill delivery.
//!
//! The plugin API intentionally owns its strings. Bootstrap moves them into a
//! session-local `BundledSkillEntry`, preserving the plugin wire shape without
//! leaking allocations into process-global state.

use wcore_plugin_api::BundledSkillSpec;
use wcore_skills::bundled::BundledSkillEntry;

/// Move a plugin skill specification into the host catalog's owned shape.
pub fn spec_to_bundled_entry(spec: BundledSkillSpec) -> BundledSkillEntry {
    BundledSkillEntry {
        name: spec.name,
        description: spec.description,
        when_to_use: spec.when_to_use,
        argument_hint: spec.argument_hint,
        allowed_tools: spec.allowed_tools,
        model: spec.model,
        disable_model_invocation: spec.disable_model_invocation,
        user_invocable: spec.user_invocable,
        context: spec.context,
        agent: spec.agent,
        files: spec.files,
        content: spec.content,
    }
}
