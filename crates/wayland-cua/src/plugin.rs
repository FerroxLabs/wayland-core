//! Plugin / PluginFactory glue for the wayland-cua plugin shell.
//!
//! `WaylandCuaFactory` is submitted via `inventory::submit!` so it's
//! discoverable through the host-side `PluginLoader::discover` path
//! without any explicit registration in main(). `WaylandCua::initialize`
//! claims the `Cua` namespace via `ScopedToolRegistry` so duplicate
//! cua-plugin loads are caught by the standard `NamespaceLedger`; the
//! actual `CuaToolSpec` payload is exposed via [`default_cua_spec`] so
//! the host adapter can construct a real `CuaTool` once it picks up
//! the plugin during boot.
//!
//! REV-2 audit F2: this crate must NOT depend on `wcore-cua`. The
//! capability flows through the `wcore-plugin-api::cua_spec` mirror.
//!
//! REV-2 audit F7: the host adapter (in wcore-cua) refuses to register
//! on restricted Wayland compositors at boot time. The plugin layer
//! propagates that as a registration failure rather than silently
//! falling back.

use std::sync::OnceLock;

use async_trait::async_trait;
use wcore_plugin_api::cua_spec::{CuaPolicySpec, CuaToolSpec};
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};

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
            .expect("wayland-cua plugin.toml must parse and validate")
    })
}

/// Default `CuaToolSpec` for the wayland-cua plugin. The host reads
/// this via `WaylandCua::cua_spec()` to construct a real `CuaTool` at
/// boot. Operators override via config (policy lists, redaction);
/// config translation happens in the host adapter, not in this crate.
pub fn default_cua_spec() -> CuaToolSpec {
    CuaToolSpec {
        tool_namespace: "Cua".into(),
        policy: CuaPolicySpec {
            require_approval_for_app: Vec::new(),
            forbidden_apps: Vec::new(),
            forbidden_key_combos: Vec::new(),
            // Default-on: every first-touch app routes through HITL
            // approval. Operators with "I know what I'm doing" use
            // cases can flip this off in config.
            first_time_per_app_approval: true,
        },
        redact_screenshots: false,
    }
}

pub struct WaylandCua;

impl WaylandCua {
    /// Expose the `CuaToolSpec` to the host. The host adapter calls
    /// this during plugin discovery to know what shape of cua tool to
    /// mint.
    pub fn cua_spec(&self) -> CuaToolSpec {
        default_cua_spec()
    }
}

#[async_trait]
impl Plugin for WaylandCua {
    fn manifest(&self) -> &PluginManifest {
        manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        // Claim the "Cua" namespace via the standard ScopedToolRegistry
        // (gives us the NamespaceLedger duplicate-claim protection). The
        // actual CuaTool reification happens in the host adapter via
        // `cua_spec()` — wayland-cua cannot construct it directly
        // because that would require a dependency on wcore-cua
        // (forbidden by audit F2).
        // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
        let registry = ctx.tools.as_mut().ok_or_else(|| {
            wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-cua".into(),
                surface: "tools".into(),
            }
        })?;
        // The `execute` tool is a namespace-claim only: the real CuaTool
        // is reified host-side from `cua_spec()` (wayland-cua cannot
        // construct it directly — audit F2). The `PluginTool` carries
        // honest metadata; its closure is never the live execution path.
        registry.register_tool(wcore_plugin_api::tool::PluginTool::host_delegated(
            "execute",
            "Computer-use tool — reified host-side from the CUA spec.",
            wcore_protocol::events::ToolCategory::Exec,
        ))?;
        // FleetDispatcher-class fix (audit 2026-05-24): also publish the
        // CuaToolSpec through the dedicated ScopedCuaRegistry so the host
        // adapter (in wcore-agent) can reify it into a real `CuaTool`
        // post-initialize. Mirrors the wayland-browser pattern. Without
        // this call, `HostCuaRegistrar.collected` stays empty and no CUA
        // tool reaches the registry — even when the host flips
        // `with_computer_use_advertised(true)`, there are no captured
        // specs to reify.
        if let Some(cua_reg) = ctx.cua.as_mut() {
            cua_reg.register_cua_tool(default_cua_spec())?;
        }
        Ok(())
    }
}

pub struct WaylandCuaFactory;

impl PluginFactory for WaylandCuaFactory {
    fn name(&self) -> &'static str {
        "wayland-cua"
    }

    fn build(&self) -> Box<dyn Plugin> {
        Box::new(WaylandCua)
    }
}

inventory::submit! { &WaylandCuaFactory as &dyn PluginFactory }
