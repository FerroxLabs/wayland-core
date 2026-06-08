//! v0.6.4 Task 2.3 — Plugin / PluginFactory glue for `wayland-honcho`.
//!
//! `WaylandHonchoFactory` is submitted via `inventory::submit!` so it's
//! discoverable through the host-side `PluginLoader::discover` path without
//! any explicit registration in `main()`. `WaylandHoncho::initialize`
//! registers a single `UserModelSpec { backend: "honcho", ... }` against the
//! scoped `UserModelRegistrar`; the manifest declares
//! `register_user_models = true` and no other surfaces.
//!
//! No live HTTP at registration time — the spec is plain data; reification
//! into a `HonchoClient` happens host-side in bootstrap (mock for tests,
//! `live_from_env` when `api_key_env` resolves at runtime).

use std::sync::OnceLock;

use async_trait::async_trait;
use wcore_plugin_api::{
    Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult, UserModelSpec,
};

/// Embedded copy of the plugin's `plugin.toml`. The TOML lives next to
/// `Cargo.toml` so future tooling (publish, audit) can read it without
/// linking the crate; `include_str!` keeps the manifest single-source.
pub const MANIFEST_TOML: &str = include_str!("../plugin.toml");

fn manifest() -> &'static PluginManifest {
    static M: OnceLock<PluginManifest> = OnceLock::new();
    M.get_or_init(|| {
        // SAFETY: `MANIFEST_TOML` is `include_str!` of the committed
        // plugin.toml. Failure here is a checked-in-source bug caught by
        // the per-plugin unit test, never a production runtime condition.
        PluginManifest::from_toml_str(MANIFEST_TOML)
            .expect("wayland-honcho plugin.toml must parse and validate")
    })
}

pub struct WaylandHoncho;

#[async_trait]
impl Plugin for WaylandHoncho {
    fn manifest(&self) -> &PluginManifest {
        manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        let spec = UserModelSpec {
            name: "honcho".to_string(),
            description: "Honcho user-model backend".to_string(),
            backend: "honcho".to_string(),
            base_url: None,
            api_key_env: Some("HONCHO_API_KEY".to_string()),
            config: serde_json::Value::Null,
        };
        let registry = ctx.user_models.as_mut().ok_or_else(|| {
            wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-honcho".into(),
                surface: "user_models".into(),
            }
        })?;
        registry.register_user_model(spec)?;
        Ok(())
    }
}

pub struct WaylandHonchoFactory;

impl PluginFactory for WaylandHonchoFactory {
    fn name(&self) -> &'static str {
        "wayland-honcho"
    }

    fn build(&self) -> Box<dyn Plugin> {
        Box::new(WaylandHoncho)
    }
}

inventory::submit! { &WaylandHonchoFactory as &dyn PluginFactory }
