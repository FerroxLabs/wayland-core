//! Wave BR — host browser-tool adapter.
//!
//! `HostBrowserRegistrar` implements `wcore_plugin_api::registry::browser::BrowserToolRegistrar`.
//! When the `wayland-browser` plugin calls `register_browser_tool(spec)` in
//! its `initialize()`, the host captures the `BrowserToolSpec` here, then
//! AFTER `PluginRunner::initialize_all` returns, the host calls
//! [`HostBrowserRegistrar::reify_all`] which translates each captured spec
//! into a real `BrowserTool` via [`wcore_browser::adapter::from_spec`].
//!
//! The resulting `BrowserTool` instances are returned for the engine's
//! tool dispatcher to register (a `wcore_tools::Tool` impl).
//!
//! REV-2 audit F2: plugin shell stays free of `wcore-browser`; the host
//! (this crate) is where the real translation happens.

use std::sync::Arc;

use wcore_browser::adapter::{
    BrowserToolSpec as CoreBrowserToolSpec, from_spec as core_from_spec, make_policy,
};
use wcore_browser::policy::{LoopbackCapability, PolicyAction};
use wcore_browser::selection::ProviderHint as CoreProviderHint;
use wcore_browser::tool::BrowserTool;
use wcore_plugin_api::browser_spec::{BrowserLoopbackSpec, BrowserProviderHint, BrowserToolSpec};
use wcore_plugin_api::registry::browser::BrowserToolRegistrar;

/// Captures every `BrowserToolSpec` registered by a `wayland-browser` plugin.
/// The runner installs one per session; `reify_all` is called after plugin
/// initialization to build the real `BrowserTool` set.
#[derive(Debug, Default)]
pub struct HostBrowserRegistrar {
    /// Specs captured from `register_browser_tool` calls, indexed by
    /// `tool_namespace` so duplicate registrations from different plugins
    /// collide here (rather than only at the engine's tool registry).
    pub specs: Vec<BrowserToolSpec>,
}

impl BrowserToolRegistrar for HostBrowserRegistrar {
    fn host_register(&mut self, spec: BrowserToolSpec) -> Result<(), String> {
        if self
            .specs
            .iter()
            .any(|s| s.tool_namespace == spec.tool_namespace)
        {
            return Err(format!(
                "duplicate browser_tool namespace: {}",
                spec.tool_namespace
            ));
        }
        self.specs.push(spec);
        Ok(())
    }
}

impl HostBrowserRegistrar {
    /// Translate every captured `BrowserToolSpec` into a real `BrowserTool`.
    /// The returned tools are ready to be registered in the engine's
    /// tool dispatcher.
    pub fn reify_all(&self) -> Vec<Arc<BrowserTool>> {
        self.specs
            .iter()
            .map(|s| core_from_spec(spec_to_core(s)))
            .collect()
    }
}

/// Map the api-crate-local `BrowserToolSpec` mirror to the
/// `wcore_browser::adapter::BrowserToolSpec` value the core adapter
/// expects. The two structs are field-for-field equivalent — the mirror
/// pattern from `BundledSkillSpec` / `BundledSkillDefinition`.
pub fn spec_to_core(s: &BrowserToolSpec) -> CoreBrowserToolSpec {
    let policy = make_policy(
        match s.policy.default_action.as_str() {
            "allow" => PolicyAction::Allow,
            "ask" => PolicyAction::Ask,
            _ => PolicyAction::Deny,
        },
        s.policy.allowed_origins.clone(),
        s.policy.denied_origins.clone(),
    )
    .with_loopback(loopback_spec_to_core(&s.policy.loopback));
    CoreBrowserToolSpec {
        tool_namespace: s.tool_namespace.clone(),
        preferred_provider: match s.preferred_provider {
            BrowserProviderHint::Auto => CoreProviderHint::Auto,
            BrowserProviderHint::Camoufox => CoreProviderHint::Camoufox,
            BrowserProviderHint::Browserbase => CoreProviderHint::Browserbase,
        },
        policy,
        allow_cloud: s.allow_cloud,
    }
}

/// Translate the plugin-api loopback mirror into the core grant (gh#911).
///
/// A straight field copy on purpose: this function must not decide anything.
/// `wcore_browser::policy::LoopbackCapability::authorize` owns every
/// validation gate, so there is exactly one place that can say yes and the
/// mirror cannot widen authority by drifting.
fn loopback_spec_to_core(s: &BrowserLoopbackSpec) -> LoopbackCapability {
    LoopbackCapability {
        enabled: s.enabled,
        schema_version: s.schema_version,
        session_scope: s.session_scope.clone(),
        ports: s.ports.clone(),
    }
}

/// Copy the operator's resolved `[browser.policy]` config onto every captured
/// `BrowserToolSpec` before the host reifies them.
///
/// The plugin shell registers a `BrowserPolicySpec::default()` (deny-all);
/// without this copy every navigate denies regardless of what the operator put
/// in their `config.toml`. Extracted out of `AgentBootstrap` (27-C2(a)) so the
/// config-hint round-trip guard exercises the SAME code the engine runs rather
/// than a re-implementation of it — a second copy of this mapping is exactly
/// how a hint and a loader drift apart unnoticed.
pub fn apply_config_policy(
    policy: &wcore_config::browser::BrowserPolicyConfig,
    specs: &mut [BrowserToolSpec],
) {
    for spec in specs {
        spec.policy.default_action = policy.default_action.clone();
        spec.policy.allowed_origins = policy.allowed_origins.clone();
        spec.policy.denied_origins = policy.denied_origins.clone();
        spec.policy.loopback = BrowserLoopbackSpec {
            enabled: policy.loopback.enabled,
            schema_version: policy.loopback.schema_version,
            session_scope: policy.loopback.session_scope.clone(),
            ports: policy.loopback.ports.clone(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_plugin_api::browser_spec::BrowserPolicySpec;
    use wcore_tools::Tool;

    fn fixture_spec(ns: &str) -> BrowserToolSpec {
        BrowserToolSpec {
            tool_namespace: ns.into(),
            preferred_provider: BrowserProviderHint::Camoufox,
            policy: BrowserPolicySpec {
                default_action: "allow".into(),
                allowed_origins: vec!["*.example.com".into()],
                denied_origins: vec!["*.evil.example".into()],
                loopback: BrowserLoopbackSpec::default(),
            },
            allow_cloud: false,
        }
    }

    #[test]
    fn captures_spec_via_registrar_trait() {
        let mut reg = HostBrowserRegistrar::default();
        reg.host_register(fixture_spec("Browser")).unwrap();
        assert_eq!(reg.specs.len(), 1);
        assert_eq!(reg.specs[0].tool_namespace, "Browser");
    }

    #[test]
    fn rejects_duplicate_namespace() {
        let mut reg = HostBrowserRegistrar::default();
        reg.host_register(fixture_spec("Browser")).unwrap();
        let r = reg.host_register(fixture_spec("Browser"));
        assert!(r.is_err());
    }

    #[test]
    fn reify_all_builds_browser_tools() {
        let mut reg = HostBrowserRegistrar::default();
        reg.host_register(fixture_spec("Browser")).unwrap();
        let tools = reg.reify_all();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "Browser");
    }

    #[test]
    fn spec_to_core_translates_policy_action() {
        let mut s = fixture_spec("Browser");
        s.policy.default_action = "deny".into();
        let core = spec_to_core(&s);
        assert_eq!(core.policy.default_action, PolicyAction::Deny);
        s.policy.default_action = "ask".into();
        let core = spec_to_core(&s);
        assert_eq!(core.policy.default_action, PolicyAction::Ask);
        s.policy.default_action = "anything-else".into();
        let core = spec_to_core(&s);
        // Unknown / typo defaults to Deny (fail-closed).
        assert_eq!(core.policy.default_action, PolicyAction::Deny);
    }

    #[test]
    fn spec_to_core_carries_origins_and_provider() {
        let s = fixture_spec("Browser");
        let core = spec_to_core(&s);
        assert_eq!(core.tool_namespace, "Browser");
        assert_eq!(core.preferred_provider, CoreProviderHint::Camoufox);
        assert_eq!(core.policy.allowed_origins.len(), 1);
        assert_eq!(core.policy.denied_origins.len(), 1);
    }

    /// gh#911 — the grant has to survive `config -> mirror -> core`. This is
    /// the seam that made the reporter of gh#900 believe browser policy had
    /// its own config resolver: a field that stops here is invisible.
    #[test]
    fn apply_config_policy_carries_the_loopback_grant_to_core() {
        let mut specs = vec![fixture_spec("Browser")];
        let cfg = wcore_config::browser::BrowserPolicyConfig {
            default_action: "deny".into(),
            allowed_origins: vec![],
            denied_origins: vec![],
            loopback: wcore_config::browser::BrowserLoopbackConfig {
                enabled: true,
                schema_version: 1,
                session_scope: "chat-42".into(),
                ports: vec![3000],
            },
        };
        apply_config_policy(&cfg, &mut specs);
        let core = spec_to_core(&specs[0]);
        assert!(core.policy.loopback.enabled);
        assert_eq!(core.policy.loopback.session_scope, "chat-42");
        assert_eq!(core.policy.loopback.ports, vec![3000]);
        // And it is load-bearing, not just carried: the granted port passes
        // and an ungranted one on the same host does not.
        assert!(core.policy.check_url("http://localhost:3000/").is_ok());
        assert!(core.policy.check_url("http://localhost:9377/").is_err());
    }
}
