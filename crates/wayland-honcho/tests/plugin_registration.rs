//! v0.6.4 Task 2.3 — end-to-end proof that `wayland-honcho`'s `Plugin`
//! impl registers a `UserModelSpec { backend: "honcho", ... }` against
//! the host's `ScopedUserModelRegistry`.
//!
//! No live HTTP: this exercises the registration path only. `HonchoClient`
//! reification (mock for tests, `live_from_env` at runtime) is downstream
//! of the carrier `AppliedPluginCapabilities.plugin_user_models` from
//! Task 2.2 and is out of scope for the Plugin trait surface itself.

use std::collections::HashMap;

use wayland_honcho::{WaylandHoncho, WaylandHonchoFactory};
use wcore_plugin_api::registry::config::{ConfigReader, ScopedConfigReader};
use wcore_plugin_api::registry::logger::ScopedPluginLogger;
use wcore_plugin_api::registry::memory::{MemoryHost, ScopedMemoryClient};
use wcore_plugin_api::registry::user_models::{ScopedUserModelRegistry, UserModelRegistrar};
use wcore_plugin_api::{
    MemoryItem, MemoryQuery, Partition, Plugin, PluginContext, PluginFactory, UserModelSpec,
};

#[derive(Default)]
struct CaptureUserModels {
    registered: Vec<UserModelSpec>,
}
impl UserModelRegistrar for CaptureUserModels {
    fn host_register_user_model(&mut self, spec: UserModelSpec) -> Result<(), String> {
        if self.registered.iter().any(|s| s.name == spec.name) {
            return Err(format!("duplicate user_model: {}", spec.name));
        }
        self.registered.push(spec);
        Ok(())
    }
}

struct NullConfig;
impl ConfigReader for NullConfig {
    fn get_raw(&self, _key: &str) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Default)]
struct NullMemory {
    writes: HashMap<Partition, Vec<MemoryItem>>,
}
impl MemoryHost for NullMemory {
    fn host_read(
        &self,
        _partition: Partition,
        _query: &MemoryQuery,
    ) -> Result<Vec<MemoryItem>, String> {
        Ok(Vec::new())
    }
    fn host_write(&mut self, partition: Partition, item: MemoryItem) -> Result<(), String> {
        self.writes.entry(partition).or_default().push(item);
        Ok(())
    }
}

#[test]
fn factory_name_matches_manifest() {
    let factory = WaylandHonchoFactory;
    assert_eq!(factory.name(), "wayland-honcho");
}

#[test]
fn factory_build_returns_a_real_plugin() {
    let factory = WaylandHonchoFactory;
    let plugin = factory.build();
    assert_eq!(plugin.manifest().plugin.name, "wayland-honcho");
    assert!(plugin.manifest().permissions.register_user_models);
    // The plugin only requests user_models — no other permissions.
    assert!(!plugin.manifest().permissions.register_tools);
    assert!(!plugin.manifest().permissions.register_hooks);
}

#[tokio::test]
async fn initialize_registers_honcho_user_model_spec() {
    let plugin = WaylandHoncho;
    let manifest = plugin.manifest().clone();

    let mut user_models_host = CaptureUserModels::default();
    let config_host = NullConfig;
    let mut memory_host = NullMemory::default();

    let user_models = ScopedUserModelRegistry::new(&manifest, &mut user_models_host).ok();
    let config = ScopedConfigReader::new(&config_host);
    let logger = ScopedPluginLogger::new(&manifest.plugin.name);
    let memory = ScopedMemoryClient::new(&manifest, &mut memory_host);

    let mut ctx = PluginContext {
        manifest: &manifest,
        tools: None,
        hooks: None,
        agents: None,
        skills: None,
        rules: None,
        mcp_servers: None,
        providers: None,
        browser: None,
        cua: None,
        user_models,
        sandbox: None,
        config,
        logger,
        memory,
    };

    plugin
        .initialize(&mut ctx)
        .await
        .expect("initialize must succeed");

    assert_eq!(
        user_models_host.registered.len(),
        1,
        "wayland-honcho must register exactly one user_model"
    );
    let spec = &user_models_host.registered[0];
    assert_eq!(spec.name, "honcho");
    assert_eq!(spec.backend, "honcho");
    assert_eq!(spec.api_key_env.as_deref(), Some("HONCHO_API_KEY"));
}
