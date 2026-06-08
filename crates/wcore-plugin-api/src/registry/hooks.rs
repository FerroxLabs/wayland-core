//! `ScopedHookRegistry` — plugin-registered hooks against the host's hook
//! engine. Six phases shipping at W2.5 cover the IJFW hook surface enumerated
//! in design spec §5.17.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    SessionStart,
    SessionEnd,
    TurnStart,
    TurnEnd,
    PreToolUse,
    PostToolUse,
    PrePrompt,
    PreCompact,
}

/// Host-side trait the wcore-agent adapter implements.
pub trait HookRegistrar: Send {
    fn host_register_hook(&mut self, phase: HookPhase, name: String) -> Result<(), String>;
}

pub struct ScopedHookRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn HookRegistrar,
    registered: Vec<(HookPhase, String)>,
}

impl<'a> ScopedHookRegistry<'a> {
    pub fn new(manifest: &PluginManifest, host: &'a mut dyn HookRegistrar) -> PluginResult<Self> {
        PluginAccessGate::require_hooks(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_hook(&mut self, phase: HookPhase, name: String) -> PluginResult<()> {
        if self
            .registered
            .iter()
            .any(|(p, n)| p == &phase && n == &name)
        {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "hook",
                name,
            });
        }
        self.host
            .host_register_hook(phase, name.clone())
            .map_err(|e| PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "hook",
                name: format!("{name} ({e})"),
            })?;
        self.registered.push((phase, name));
        Ok(())
    }
}
