//! Plugin / PluginFactory glue for the wayland-ijfw anchor plugin.
//!
//! `WaylandIjfwFactory` is submitted via `inventory::submit!` so it's
//! discoverable through the host-side `PluginLoader::discover` path
//! without any explicit registration in main(). `WaylandIjfw::initialize`
//! walks every `register_*` surface (`tools`, `hooks`, `agents`,
//! `skills`, `rules`, `mcp_servers`) — `providers` is deliberately
//! left untouched because the IJFW anchor does not own a provider
//! (wayland-ollama is the provider-only reference plugin).
//!
//! REV-2 audit F2: this crate must NOT depend on any internal wcore-*
//! crate beyond `wcore-plugin-api`, `wcore-types`, `wcore-protocol`.
//! All payloads (skills, agents, rules, MCP spec) cross the boundary
//! as api-crate-local mirror types.

use std::sync::OnceLock;

use async_trait::async_trait;
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};

use crate::{agents, hooks, mcp, rules, skills, tools};

/// Embedded copy of `plugin.toml`. The TOML lives next to `Cargo.toml`
/// so future tooling (publish, audit) can read it without linking the
/// crate.
pub const MANIFEST_TOML: &str = include_str!("../plugin.toml");

fn manifest() -> &'static PluginManifest {
    static M: OnceLock<PluginManifest> = OnceLock::new();
    M.get_or_init(|| {
        // SAFETY: `MANIFEST_TOML` is `include_str!` of the committed
        // plugin.toml. Failure here is a checked-in-source bug caught
        // by the per-plugin unit test, never a production runtime
        // condition.
        PluginManifest::from_toml_str(MANIFEST_TOML)
            .expect("wayland-ijfw plugin.toml must parse and validate")
    })
}

pub struct WaylandIjfw;

#[async_trait]
impl Plugin for WaylandIjfw {
    fn manifest(&self) -> &PluginManifest {
        manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        // Walk every register_* surface the manifest permits. The order
        // is deliberately stable so test fixtures can assert on the
        // sequence (tools first claims the namespace via NamespaceLedger,
        // everything else follows).
        tools::register(ctx)?;
        hooks::register(ctx)?;
        agents::register(ctx)?;
        skills::register(ctx)?;
        rules::register(ctx)?;
        mcp::register(ctx)?;
        Ok(())
    }
}

pub struct WaylandIjfwFactory;

impl PluginFactory for WaylandIjfwFactory {
    fn name(&self) -> &'static str {
        "wayland-ijfw"
    }

    fn build(&self) -> Box<dyn Plugin> {
        Box::new(WaylandIjfw)
    }
}

inventory::submit! { &WaylandIjfwFactory as &dyn PluginFactory }
