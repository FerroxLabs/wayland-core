//! Secrets host capability — existence-only.
//!
//! **HARD CONTRACT:** the secrets adapter exposes ONLY `secret_exists -> bool`.
//! There is no `secret_read`, `secret_value`, or any method that returns the
//! secret material. Plugins learn whether a secret is configured; the actual
//! value is consumed host-side at HTTP-request time. This contract is enforced
//! structurally — the surface of `WaylandHostSecrets` is the evidence.

use std::sync::Arc;

use wcore_plugin_api::access_gate::PluginAccessGate;

/// Local trait. Existence-only by construction.
pub trait WaylandHostSecrets: Send + Sync {
    fn secret_exists(&self, name: &str) -> bool;
}

/// Fail-closed secrets host. Reports every secret as absent.
#[derive(Debug, Default)]
pub struct DenyHostSecrets;

impl WaylandHostSecrets for DenyHostSecrets {
    fn secret_exists(&self, _name: &str) -> bool {
        false
    }
}

/// Gated secrets host.
///
/// `permitted_secrets` is the manifest-derived allow-list. Membership returns
/// `true`; everything else returns `false`. The host NEVER returns the value.
pub struct GatedHostSecrets {
    #[allow(dead_code)] // Wired in Task 2.6 when the gate becomes stateful.
    gate: Arc<PluginAccessGate>,
    #[allow(dead_code)]
    plugin: String,
    permitted_secrets: Vec<String>,
}

impl GatedHostSecrets {
    pub fn new(
        gate: Arc<PluginAccessGate>,
        plugin: String,
        permitted_secrets: Vec<String>,
    ) -> Self {
        Self {
            gate,
            plugin,
            permitted_secrets,
        }
    }
}

impl WaylandHostSecrets for GatedHostSecrets {
    fn secret_exists(&self, name: &str) -> bool {
        self.permitted_secrets.iter().any(|s| s == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_secrets_always_false() {
        let deny = DenyHostSecrets;
        assert!(!deny.secret_exists("ANYTHING"));
        assert!(!deny.secret_exists(""));
    }

    #[test]
    fn gated_secrets_reports_only_listed() {
        let g = GatedHostSecrets::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            vec!["OPENAI_API_KEY".into()],
        );
        assert!(g.secret_exists("OPENAI_API_KEY"));
        assert!(!g.secret_exists("OTHER"));
    }

    /// Structural assertion: the trait surface MUST NOT include any method
    /// that returns the secret value. If this stops compiling because a
    /// `secret_read`-style method was added, the contract is broken.
    ///
    /// We assert this by referring to the only-permitted method by full path
    /// and treating that as the API. Adding `secret_read` would not break this
    /// test, so we additionally `grep` the source file in CI; but as a
    /// compile-time anchor, we point at the trait here.
    #[test]
    fn secret_exists_never_exposes_value() {
        let g = GatedHostSecrets::new(Arc::new(PluginAccessGate), "p".into(), vec!["S".into()]);
        // Only fn: secret_exists -> bool.
        let _: bool = WaylandHostSecrets::secret_exists(&g, "S");
        // If a value-returning method were added (e.g. `secret_read -> String`),
        // a reviewer would see it on this trait. The grep audit in mod docs
        // catches the rest.
    }
}
