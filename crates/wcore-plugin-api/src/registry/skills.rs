//! `ScopedSkillRegistry` — plugin-registered bundled skills. Host adapter
//! delegates to `wcore_skills::bundled::register_bundled_skill`.

use crate::access_gate::PluginAccessGate;
use crate::bundled_skill_spec::BundledSkillSpec;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

pub trait SkillRegistrar: Send {
    fn host_register_skill(&mut self, skill: BundledSkillSpec) -> Result<(), String>;
}

pub struct ScopedSkillRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn SkillRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedSkillRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn SkillRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_skills(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_skill(&mut self, mut skill: BundledSkillSpec) -> PluginResult<()> {
        // F-044: auto-namespace on cross-plugin collision. If this plugin already
        // registered a skill with the same name, that is an intra-plugin duplicate
        // and fails immediately. If the HOST rejects it as a duplicate (another
        // plugin already holds the name), we retry with `<plugin>:<skill>` to
        // avoid silently dropping the skill. This matches the colon convention
        // `build_namespace` uses for filesystem skills.
        if self.registered.contains(&skill.name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "skill",
                name: skill.name,
            });
        }
        let original_name = skill.name.clone();
        match self.host.host_register_skill(skill.clone()) {
            Ok(()) => {
                self.registered.push(original_name);
                Ok(())
            }
            Err(_duplicate_err) => {
                // Cross-plugin collision: retry with namespaced name.
                let namespaced = format!("{}:{}", self.plugin_name, original_name);
                skill.name = namespaced.clone();
                self.host.host_register_skill(skill).map_err(|e| {
                    PluginError::DuplicateRegistration {
                        plugin: self.plugin_name.clone(),
                        kind: "skill",
                        name: format!("{namespaced} ({e})"),
                    }
                })?;
                self.registered.push(namespaced);
                Ok(())
            }
        }
    }
}
