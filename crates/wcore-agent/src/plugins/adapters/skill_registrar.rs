//! Skill registrar adapter. W2.5 captures `BundledSkillSpec`s in memory; W8
//! will delegate to `wcore_skills::bundled::register_bundled_skill` via the
//! Box::leak String -> &'static str translation noted in the api crate
//! `bundled_skill_spec` module.

use wcore_plugin_api::BundledSkillSpec;
use wcore_plugin_api::registry::skills::SkillRegistrar;

#[derive(Debug, Default)]
pub struct HostSkillRegistrar {
    pub registered: Vec<BundledSkillSpec>,
}

impl SkillRegistrar for HostSkillRegistrar {
    fn host_register_skill(&mut self, skill: BundledSkillSpec) -> Result<(), String> {
        if self.registered.iter().any(|s| s.name == skill.name) {
            return Err(format!("duplicate skill: {}", skill.name));
        }
        self.registered.push(skill);
        Ok(())
    }
}
