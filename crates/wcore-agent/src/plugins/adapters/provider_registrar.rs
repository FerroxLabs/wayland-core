//! Provider registrar adapter — stores plugin-registered providers and
//! exposes them by name to the host-side bootstrap.
//!
//! Wave OL (shipped — closes the W8c.3.D chain edge that was aspirational
//! from W8a through v0.2.0). The pipeline is now end-to-end:
//!
//! 1. `wayland-ollama`'s `WaylandOllama::initialize` registers an
//!    `Arc<OllamaProvider>` against the scoped registry, which lands here
//!    via `host_register_provider`.
//! 2. `HostProviderRegistrar::lookup_by_name("ollama")` returns the
//!    `Arc<dyn PluginProvider>`.
//! 3. The host (`wcore-cli::make_plugin_provider_router`) calls
//!    `PluginProvider::as_any().downcast_ref::<OllamaProvider>()` to
//!    confirm the concrete type, then constructs an
//!    `Arc<dyn LlmProvider>` that the engine routes through for any
//!    `--model ollama:*` turn.
//!
//! The downcast itself lives in the binary crate (`wcore-cli`) because
//! it's the only crate that links both `wayland-ollama` and
//! `wcore-providers` together — `wcore-agent` deliberately doesn't
//! depend on `wayland-ollama`. Covered end-to-end by
//! `crates/wcore-agent/tests/ollama_e2e_test.rs` (wiremock + real
//! engine turn).

use std::sync::Arc;

use wcore_plugin_api::registry::providers::{PluginProvider, ProviderRegistrar};

#[derive(Default)]
pub struct HostProviderRegistrar {
    pub registered: Vec<Arc<dyn PluginProvider>>,
}

impl std::fmt::Debug for HostProviderRegistrar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.registered.iter().map(|p| p.provider_name()).collect();
        f.debug_struct("HostProviderRegistrar")
            .field("registered", &names)
            .finish()
    }
}

impl ProviderRegistrar for HostProviderRegistrar {
    fn host_register_provider(&mut self, provider: Arc<dyn PluginProvider>) -> Result<(), String> {
        if self
            .registered
            .iter()
            .any(|p| p.provider_name() == provider.provider_name())
        {
            return Err(format!("duplicate provider: {}", provider.provider_name()));
        }
        self.registered.push(provider);
        Ok(())
    }
}

impl HostProviderRegistrar {
    /// Wave OL: look up a registered plugin provider by `provider_name()`.
    /// Returns `None` when no plugin owns that name. The caller is
    /// responsible for downcasting via `PluginProvider::as_any` — see
    /// `wcore-cli` for the `OllamaProvider` route.
    pub fn lookup_by_name(&self, name: &str) -> Option<Arc<dyn PluginProvider>> {
        self.registered
            .iter()
            .find(|p| p.provider_name() == name)
            .cloned()
    }

    /// Number of registered plugin providers. Mirrors the
    /// `InitializeOutcome.providers_registered` count for callers that
    /// hold the registrar directly.
    pub fn len(&self) -> usize {
        self.registered.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registered.is_empty()
    }
}
