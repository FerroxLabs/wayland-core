//! Plugin / PluginFactory glue for the wayland-browser plugin shell.
//!
//! `WaylandBrowserFactory` is submitted via `inventory::submit!` so it's
//! discoverable through the host-side `PluginLoader::discover` path
//! without any explicit registration in main(). `WaylandBrowser::initialize`
//! claims the `Browser` namespace via `ScopedToolRegistry` so duplicate
//! browser-plugin loads are caught by the standard `NamespaceLedger`; the
//! actual `BrowserToolSpec` payload is exposed via
//! [`default_browser_spec`] so the host adapter can construct a real
//! `BrowserTool` once it picks up the plugin during boot.
//!
//! REV-2 audit F2: this crate must NOT depend on `wcore-browser`. The
//! capability flows through the `wcore-plugin-api::browser_spec` mirror.

use std::sync::OnceLock;

use async_trait::async_trait;
use wcore_plugin_api::browser_spec::{BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec};
use wcore_plugin_api::{Plugin, PluginContext, PluginFactory, PluginManifest, PluginResult};

/// Embedded copy of `plugin.toml`. The TOML lives next to `Cargo.toml` so
/// future tooling (publish, audit) can read it without linking the crate.
pub const MANIFEST_TOML: &str = include_str!("../plugin.toml");

fn manifest() -> &'static PluginManifest {
    static M: OnceLock<PluginManifest> = OnceLock::new();
    M.get_or_init(|| {
        // SAFETY: `MANIFEST_TOML` is an `include_str!` of the
        // committed `plugin.toml`. Parsing failure here means the
        // checked-in manifest is malformed — a compile/source bug
        // that the dedicated unit test `plugin_toml_parses` catches
        // before release. Production callers will never see this
        // panic unless the binary is built from a broken source tree.
        PluginManifest::from_toml_str(MANIFEST_TOML)
            .expect("wayland-browser plugin.toml must parse and validate")
    })
}

/// Default `BrowserToolSpec` for the wayland-browser plugin. The host
/// reads this via `WaylandBrowser::browser_spec()` to construct a real
/// `BrowserTool` at boot. Operators override via config (preferred
/// provider, policy lists) — config translation happens in the host
/// adapter, not in this crate.
pub fn default_browser_spec() -> BrowserToolSpec {
    BrowserToolSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Auto,
        // Fail-closed default — operators MUST set
        // `[browser.policy] allowed_origins = [...]` for the plugin to
        // make any request. Pre-v0.2.1 this defaulted to "allow" which
        // was a fail-open SSRF risk; see STABILITY-v0.2.0.md MAJOR #6.
        policy: BrowserPolicySpec::default(),
        allow_cloud: false,
    }
}

pub struct WaylandBrowser;

impl WaylandBrowser {
    /// Expose the BrowserToolSpec to the host. The host adapter calls this
    /// during plugin discovery to know what shape of browser tool to mint.
    pub fn browser_spec(&self) -> BrowserToolSpec {
        default_browser_spec()
    }
}

#[async_trait]
impl Plugin for WaylandBrowser {
    fn manifest(&self) -> &PluginManifest {
        manifest()
    }

    async fn initialize(&self, ctx: &mut PluginContext<'_>) -> PluginResult<()> {
        // Claim the "Browser" namespace via the standard ScopedToolRegistry
        // (gives us the NamespaceLedger duplicate-claim protection).
        // Wave RB STABILITY MINOR #13: typed HostMisconfiguration error.
        let registry = ctx.tools.as_mut().ok_or_else(|| {
            wcore_plugin_api::PluginError::HostMisconfiguration {
                plugin: "wayland-browser".into(),
                surface: "tools".into(),
            }
        })?;
        // The `execute` tool is a namespace-claim only: the real
        // BrowserTool is reified host-side from the BrowserToolSpec below
        // (wayland-browser cannot construct it directly — audit F2). The
        // `PluginTool` carries honest metadata; its closure is never the
        // live execution path.
        registry.register_tool(wcore_plugin_api::tool::PluginTool::host_delegated(
            "execute",
            "Browser tool — reified host-side from the BrowserToolSpec.",
            wcore_protocol::events::ToolCategory::Exec,
        ))?;

        // Wave BR — also publish the BrowserToolSpec through the dedicated
        // ScopedBrowserRegistry so the host adapter (in wcore-agent) can
        // reify it into a real `BrowserTool` post-initialize. The plugin
        // shell never holds a wcore-browser handle (REV-2 audit F2);
        // translation is host-side.
        if let Some(browser_reg) = ctx.browser.as_mut() {
            browser_reg.register_browser_tool(default_browser_spec())?;
        }
        Ok(())
    }
}

pub struct WaylandBrowserFactory;

impl PluginFactory for WaylandBrowserFactory {
    fn name(&self) -> &'static str {
        "wayland-browser"
    }

    fn build(&self) -> Box<dyn Plugin> {
        Box::new(WaylandBrowser)
    }
}

inventory::submit! { &WaylandBrowserFactory as &dyn PluginFactory }
