//! Provider selection — Camoufox primary → Chromium fallback → Browserbase cloud.
//!
//! Order:
//!   1. If `ProviderHint::Browserbase` AND `BROWSERBASE_*` env present → Browserbase.
//!   2. If `ProviderHint::Camoufox` or `Auto` → Camoufox (it's a sidecar; assume
//!      reachable; the first op gets a typed error if not).
//!   3. If `chromium` feature is on → fall through to Chromium.
//!   4. Otherwise default Camoufox.
//!
//! The function returns a `Box<dyn BrowserProvider>` so the tool layer is
//! provider-neutral.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[cfg(feature = "browserbase")]
use crate::backends::BrowserbaseBackend;
use crate::backends::CamoufoxBackend;
#[cfg(feature = "chromium")]
use crate::backends::ChromiumBackend;

use crate::policy::BrowserPolicy;
use crate::provider::BrowserProvider;

/// Operator-facing hint about which provider to prefer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHint {
    #[default]
    Auto,
    Camoufox,
    Chromium,
    Browserbase,
}

/// Selection inputs. `allow_cloud` gates the Browserbase candidate even
/// when env creds are present (operator opt-in safety net).
#[derive(Debug, Clone, Default)]
pub struct SelectionInputs {
    pub hint: ProviderHint,
    pub allow_cloud: bool,
    /// Override Camoufox URL — useful in tests so wiremock can stand in
    /// for the sidecar.
    pub camoufox_url: Option<String>,
    /// Policy to plumb into the chosen backend. The backend installs a
    /// `reqwest::redirect::Policy` derived from this so any 3xx hop is
    /// policy-checked (closes BLOCKER #3). Pass `None` only in tests
    /// that explicitly want the legacy untyped-redirect behavior.
    pub policy: Option<BrowserPolicy>,
}

/// Pick a provider per the selection rules above.
pub fn select_provider(inputs: SelectionInputs) -> Arc<dyn BrowserProvider> {
    // 1. Browserbase: explicit hint OR cloud-allow-AND-env.
    #[cfg(feature = "browserbase")]
    {
        if inputs.allow_cloud
            && matches!(inputs.hint, ProviderHint::Browserbase)
            && let Some(bb) = BrowserbaseBackend::from_env()
        {
            return Arc::new(bb) as Arc<dyn BrowserProvider>;
        }
    }
    // 2. Chromium: explicit hint AND feature on.
    #[cfg(feature = "chromium")]
    {
        if matches!(inputs.hint, ProviderHint::Chromium) {
            return Arc::new(ChromiumBackend::new()) as Arc<dyn BrowserProvider>;
        }
    }
    // 3. Camoufox (default). Wire the policy in when provided.
    let url = inputs
        .camoufox_url
        .unwrap_or_else(|| CamoufoxBackend::default_url().to_string());
    match inputs.policy {
        Some(p) => Arc::new(CamoufoxBackend::with_policy(url, p)) as Arc<dyn BrowserProvider>,
        None => Arc::new(CamoufoxBackend::new(url)) as Arc<dyn BrowserProvider>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_camoufox() {
        let p = select_provider(SelectionInputs::default());
        assert_eq!(p.backend_name(), "camoufox");
    }

    #[test]
    fn camoufox_hint_picks_camoufox() {
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Camoufox,
            ..Default::default()
        });
        assert_eq!(p.backend_name(), "camoufox");
    }

    #[cfg(feature = "chromium")]
    #[test]
    fn chromium_hint_picks_chromium_when_feature_on() {
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Chromium,
            ..Default::default()
        });
        assert_eq!(p.backend_name(), "chromium");
    }

    #[test]
    fn browserbase_hint_without_env_falls_back_to_camoufox() {
        // Even with the feature on, no BROWSERBASE_* env => fallback.
        // We don't unset env here (test isolation); just assert the fallback
        // behaviour by checking the result is *not* "browserbase" when env
        // vars are absent. CI runs without those vars.
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Browserbase,
            allow_cloud: true,
            ..Default::default()
        });
        // In default builds (no `browserbase` feature) we always land on camoufox.
        // With the feature, from_env() returns None unless creds are set in env.
        assert_eq!(p.backend_name(), "camoufox");
    }
}
