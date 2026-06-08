//! `ScopedRuleRegistry` — plugin-registered system-prompt fragments.

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;
use crate::rule_spec::RuleSpec;

pub trait RuleRegistrar: Send {
    fn host_register_rule(&mut self, rule: RuleSpec) -> Result<(), String>;
}

pub struct ScopedRuleRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn RuleRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedRuleRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn RuleRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_rules(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_rule(&mut self, rule: RuleSpec) -> PluginResult<()> {
        if self.registered.contains(&rule.name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "rule",
                name: rule.name,
            });
        }
        let name = rule.name.clone();
        self.host
            .host_register_rule(rule)
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "rule",
                name: format!("{name} ({e})"),
            })?;
        self.registered.push(name);
        Ok(())
    }
}
