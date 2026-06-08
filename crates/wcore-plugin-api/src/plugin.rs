//! `Plugin` and `PluginFactory` traits.
//!
//! Plugins receive a `PluginContext` in `initialize()` — that is the **only**
//! boundary to engine internals. The `'a` lifetime on `PluginContext<'a>`
//! forbids retaining references past `initialize()` returning.

use async_trait::async_trait;

use crate::context::PluginContext;
use crate::error::PluginResult;
use crate::manifest::PluginManifest;

/// A plugin compiled into the wcore workspace.
///
/// Design spec §5.17.
#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> &PluginManifest;

    async fn initialize(&self, _ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        Ok(())
    }

    async fn shutdown(&self) -> PluginResult<()> {
        Ok(())
    }
}

/// A compile-time factory submitted via `inventory::submit!`. Factories are
/// cheap (`&'static`); they produce `Box<dyn Plugin>` only when the host's
/// loader actually wants to instantiate the plugin.
pub trait PluginFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn build(&self) -> Box<dyn Plugin>;

    /// Optional filesystem path to the plugin artifact.
    ///
    /// Compiled-in (inventory) plugins return `None`; external / dynamically
    /// loaded plugins return `Some(path)`.  When
    /// `PluginsConfig.plugin_signature_verification` is `true` the loader
    /// requires a non-`None` path with a valid `.sig` sidecar — plugins that
    /// return `None` are rejected as unverifiable (Sec6).
    fn plugin_path(&self) -> Option<&'static std::path::Path> {
        None
    }
}
