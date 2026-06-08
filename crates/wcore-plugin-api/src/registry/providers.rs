//! `ScopedProviderRegistry` — plugin-registered LLM providers.
//!
//! Plugins implement the api-crate-local `PluginProvider` marker trait to
//! register opaque provider handles. The host adapter performs the
//! downcast/translate to a concrete `wcore_providers::LlmProvider` impl in
//! its own crate (out of scope for the api crate per FORBIDDEN_CORE_IMPORTS).

use std::any::Any;
use std::sync::Arc;

use crate::access_gate::PluginAccessGate;
use crate::error::{PluginError, PluginResult};
use crate::manifest::PluginManifest;

/// Marker trait for plugin-supplied LLM provider handles. The host adapter
/// downcasts (via `Arc<dyn Any>`) and threads the value into the real
/// provider chain. Defined here to keep the api crate free of
/// `wcore-providers`.
///
/// `as_any` is the downcast hook. The host (e.g. `wcore-cli`) calls it to
/// recover the concrete plugin type when routing `--model <name>:*` through
/// a plugin-supplied provider. Implementors should `return self` so the
/// `&dyn Any` covers the concrete struct; the canonical pattern is:
///
/// ```ignore
/// impl PluginProvider for MyProvider {
///     fn provider_name(&self) -> &str { "my-provider" }
///     fn as_any(&self) -> &dyn std::any::Any { self }
/// }
/// ```
pub trait PluginProvider: Send + Sync + 'static {
    fn provider_name(&self) -> &str;
    fn as_any(&self) -> &dyn Any;
}

pub trait ProviderRegistrar: Send {
    fn host_register_provider(&mut self, provider: Arc<dyn PluginProvider>) -> Result<(), String>;
}

pub struct ScopedProviderRegistry<'a> {
    plugin_name: String,
    host: &'a mut dyn ProviderRegistrar,
    registered: Vec<String>,
}

impl<'a> ScopedProviderRegistry<'a> {
    pub fn new(
        manifest: &PluginManifest,
        host: &'a mut dyn ProviderRegistrar,
    ) -> PluginResult<Self> {
        PluginAccessGate::require_providers(manifest)?;
        Ok(Self {
            plugin_name: manifest.plugin.name.clone(),
            host,
            registered: Vec::new(),
        })
    }

    pub fn register_provider(&mut self, provider: Arc<dyn PluginProvider>) -> PluginResult<()> {
        let name = provider.provider_name().to_string();
        if self.registered.contains(&name) {
            return Err(PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "provider",
                name,
            });
        }
        self.host.host_register_provider(provider).map_err(|e| {
            PluginError::DuplicateRegistration {
                plugin: self.plugin_name.clone(),
                kind: "provider",
                name: format!("{name} ({e})"),
            }
        })?;
        self.registered.push(name);
        Ok(())
    }
}
