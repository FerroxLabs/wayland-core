//! Skill registrar adapter. Captures owned `BundledSkillSpec`s in memory so
//! bootstrap can move them into its session-local bundled skill catalog.

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
