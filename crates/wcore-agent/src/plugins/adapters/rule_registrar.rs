//! Rule registrar adapter — stores plugin-registered system-prompt fragments.
//! The system-prompt builder picks them up during session boot in W7.

use wcore_plugin_api::RuleSpec;
use wcore_plugin_api::registry::rules::RuleRegistrar;

#[derive(Debug, Default)]
pub struct HostRuleRegistrar {
    pub registered: Vec<RuleSpec>,
}

impl RuleRegistrar for HostRuleRegistrar {
    fn host_register_rule(&mut self, rule: RuleSpec) -> Result<(), String> {
        if self.registered.iter().any(|r| r.name == rule.name) {
            return Err(format!("duplicate rule: {}", rule.name));
        }
        self.registered.push(rule);
        Ok(())
    }
}
