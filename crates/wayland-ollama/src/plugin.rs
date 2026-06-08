//! Plugin / PluginFactory glue for the wayland-ollama reference plugin.
//!
//! `WaylandOllamaFactory` is submitted via `inventory::submit!` so it's
//! discoverable through the host-side `PluginLoader::discover` path
//! without any explicit registration in main(). `WaylandOllama::initialize`
//! registers a single `OllamaProvider` against the scoped registry; the
//! manifest declares `register_providers = true` and no other surfaces.

use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};

use crate::provider::OllamaProvider;

/// Embedded copy of the plugin's `plugin.toml`. The TOML lives next to
/// `Cargo.toml` so future tooling (publish, audit) can read it without
/// linking the crate; `include_str!` keeps the manifest single-source.
pub const MANIFEST_TOML: &str = include_str!("../plugin.toml");

fn manifest() -> &'static PluginManifest {
    static M: OnceLock<PluginManifest> = OnceLock::new();
    M.get_or_init(|| {
        // SAFETY: `MANIFEST_TOML` is `include_str!` of the committed
        // plugin.toml. Failure here is a checked-in-source bug caught
        // by the per-plugin unit test, never a production runtime
        // condition.
        PluginManifest::from_toml_str(MANIFEST_TOML)
            .expect("wayland-ollama plugin.toml must parse and validate")
    })
}

pub struct WaylandOllama;

#[async_trait]
impl Plugin for WaylandOllama {
    fn manifest(&self) -> &PluginManifest {
        manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        let provider = Arc::new(OllamaProvider::new(
            "http://localhost:11434/api/chat",
            "llama3",
        ));
        // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
        let registry = ctx.providers.as_mut().ok_or_else(|| {
            wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-ollama".into(),
                surface: "providers".into(),
            }
        })?;
        registry.register_provider(provider)?;
        Ok(())
    }
}

pub struct WaylandOllamaFactory;

impl PluginFactory for WaylandOllamaFactory {
    fn name(&self) -> &'static str {
        "wayland-ollama"
    }

    fn build(&self) -> Box<dyn Plugin> {
        Box::new(WaylandOllama)
    }
}

inventory::submit! { &WaylandOllamaFactory as &dyn PluginFactory }
