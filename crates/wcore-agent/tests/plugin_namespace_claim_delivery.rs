//! Both `wayland-browser` and `wayland-cua` reserve the bare tool name
//! `execute` inside their own plugin namespace so a second copy of the same
//! plugin trips the `NamespaceLedger` duplicate-claim check. Neither claim
//! carries behavior: the real tool is reified host-side from a
//! `BrowserToolSpec` / `CuaToolSpec` (audit F2 forbids the plugin shell from
//! constructing it).
//!
//! `PluginToolAdapter::name()` echoes the BARE name, and `deliver_tools`
//! dedupes on that same bare name — so before the `namespace_claim` marker
//! existed, loading both plugins together produced two `"execute"` entries.
//! That had two consequences, and this test pins both:
//!
//!   1. the second claim tripped the collision `warn!` on every startup of a
//!      binary that force-links both plugins (`packaged_runtime.rs`), and
//!   2. the *survivor* stayed in the registry, so `to_tool_defs()` — which
//!      `Engine` calls unfiltered — offered the model a tool literally named
//!      `execute` whose only possible outcome is `is_error: true`.
//!
//! This drives the real plugin factories through `PluginRunner` and
//! `apply_initialize_outcome`, not a fixture, so the marker cannot be
//! dropped from `wayland-browser` / `wayland-cua` without failing here.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use tracing_subscriber::layer::SubscriberExt as _;
use wcore_agent::plugins::{DiscoveredPlugin, PluginRunner, apply_initialize_outcome};
use wcore_plugin_api::PluginFactory;
use wcore_tools::registry::ToolRegistry;

/// Build a `DiscoveredPlugin` straight from a statically linked factory.
///
/// Deliberately NOT via `PluginLoader::discover`: that walks the process-wide
/// `inventory` slot, so the set under test would depend on which other plugin
/// crates happen to be linked into this test binary.
fn discovered(factory: &'static dyn PluginFactory) -> DiscoveredPlugin {
    let plugin = factory.build();
    let manifest = plugin.manifest().clone();
    DiscoveredPlugin {
        name: factory.name().to_string(),
        manifest,
        plugin,
    }
}

#[derive(Default)]
struct CapturedWarnings(Arc<Mutex<Vec<String>>>);

impl CapturedWarnings {
    fn snapshot(&self) -> Vec<String> {
        self.0.lock().expect("warning capture lock").clone()
    }
}

struct WarnCaptureLayer(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnCaptureLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() > tracing::Level::WARN {
            return;
        }
        let mut visitor = FieldJoiner(String::new());
        event.record(&mut visitor);
        self.0.lock().expect("warning capture lock").push(visitor.0);
    }
}

struct FieldJoiner(String);

impl tracing::field::Visit for FieldJoiner {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

/// Run both real plugin factories through the production initialize → apply
/// path and return the resulting registry plus every WARN-or-worse event.
fn boot_browser_and_cua() -> (ToolRegistry, Vec<String>) {
    let captured = CapturedWarnings::default();
    let subscriber = tracing_subscriber::registry().with(WarnCaptureLayer(Arc::clone(&captured.0)));

    let mut registry = ToolRegistry::new();
    tracing::subscriber::with_default(subscriber, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");
        rt.block_on(async {
            let plugins = vec![
                discovered(&wayland_browser::WaylandBrowserFactory),
                discovered(&wayland_cua::WaylandCuaFactory),
            ];
            let mut runner = PluginRunner::new();
            let outcome = runner
                .initialize_all(&plugins)
                .await
                .expect("initialize_all must not abort the boot");
            apply_initialize_outcome(outcome, &mut registry, runner.browser, runner.cua);
        });
    });

    (registry, captured.snapshot())
}

#[test]
fn namespace_claims_never_reach_the_registry_or_the_model() {
    let (registry, warnings) = boot_browser_and_cua();

    // 1. The model is never offered a bare `execute`. `Engine` calls
    //    `to_tool_defs()` unfiltered, so anything here is advertised.
    let advertised: Vec<String> = registry
        .to_tool_defs()
        .into_iter()
        .map(|d| d.name)
        .collect();
    assert!(
        !advertised.iter().any(|n| n == "execute"),
        "a bare `execute` tool was advertised to the model; \
         its only possible result is is_error: true. advertised: {advertised:?}"
    );

    // 2. The REAL browser tool still arrives, through the separate
    //    BrowserToolSpec reification path. Dropping the claim must not
    //    drop the capability.
    assert!(
        registry.get("Browser").is_some(),
        "the host-reified Browser tool must still be registered; \
         advertised: {advertised:?}"
    );
    assert!(
        advertised.iter().any(|n| n == "Browser"),
        "the Browser tool must also be advertised to the model; \
         advertised: {advertised:?}"
    );

    // 3. No spurious collision WARN. Both plugins claim `execute`; with the
    //    claims dropped before the collision check, nothing collides.
    let collisions: Vec<&String> = warnings
        .iter()
        .filter(|w| w.contains("collide") || w.contains("collision"))
        .collect();
    assert!(
        collisions.is_empty(),
        "startup emitted tool-name collision warnings: {collisions:?}"
    );
}

/// Guards the marker itself: the claim is captured by the runner (so the
/// `NamespaceLedger` duplicate protection the plugins registered it for still
/// works) and is only dropped later, at delivery.
#[test]
fn the_claim_is_still_captured_before_delivery_drops_it() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");
    let outcome = rt.block_on(async {
        let plugins = vec![
            discovered(&wayland_browser::WaylandBrowserFactory),
            discovered(&wayland_cua::WaylandCuaFactory),
        ];
        let mut runner = PluginRunner::new();
        runner
            .initialize_all(&plugins)
            .await
            .expect("initialize_all must not abort the boot")
    });

    let claims: Vec<&str> = outcome
        .tools
        .iter()
        .filter(|c| c.tool.namespace_claim)
        .map(|c| c.fq_name.as_str())
        .collect();
    assert_eq!(
        claims,
        vec!["Browser::execute", "Cua::execute"],
        "both plugins must still CLAIM their namespace; the marker only \
         changes what the host does with the claim at delivery"
    );
}
