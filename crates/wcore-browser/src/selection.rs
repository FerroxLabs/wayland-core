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
            // F17: Browserbase navigation happens server-side in the cloud
            // browser, so a client-side `reqwest::redirect::Policy` on our
            // calls to `api.browserbase.com` CANNOT constrain where the
            // remote browser is redirected — the BLOCKER #3 redirect-SSRF
            // defense (and the always-on metadata/loopback/private blocks)
            // are unenforceable for this backend. When a `BrowserPolicy` is
            // in force we therefore REFUSE Browserbase and fall through to
            // Camoufox, which does enforce the policy. Only `policy == None`
            // (the legacy "trust the sidecar" mode with no enforcement
            // expectation) permits the cloud backend.
            if inputs.policy.is_some() {
                tracing::warn!(
                    "ProviderHint::Browserbase requested with a BrowserPolicy set, but the \
                     Browserbase backend cannot enforce the URL policy (navigation + redirects \
                     happen server-side in the cloud browser). Refusing Browserbase and falling \
                     back to Camoufox so the policy is enforced. Clear the policy to use \
                     Browserbase, or use Camoufox/Chromium for policy-enforced browsing."
                );
            } else {
                return Arc::new(bb) as Arc<dyn BrowserProvider>;
            }
        }
    }
    // 1b. Browserbase requested but feature compiled out (the default build):
    // surface the misconfiguration instead of silently dropping the hint and
    // using Camoufox, mirroring the chromium-off arm below. The whole feature
    // block above is compiled out without `--features browserbase`, so a
    // Browserbase hint would otherwise fall through with no signal at all.
    #[cfg(not(feature = "browserbase"))]
    if matches!(inputs.hint, ProviderHint::Browserbase) {
        tracing::warn!(
            "ProviderHint::Browserbase requested but wcore-browser was built without the \
             `browserbase` feature; falling back to Camoufox. Rebuild with \
             `--features browserbase` (and set BROWSERBASE_* env) to honor the hint."
        );
    }
    // 2. Chromium: explicit hint AND feature on.
    #[cfg(feature = "chromium")]
    {
        if matches!(inputs.hint, ProviderHint::Chromium) {
            return Arc::new(ChromiumBackend::new()) as Arc<dyn BrowserProvider>;
        }
    }
    // 2b. Chromium requested but feature compiled out: surface the
    // misconfiguration instead of silently dropping the hint and using
    // Camoufox. The selection is provider-neutral (`Arc<...>`, not a
    // `Result`), so we warn rather than error and still fall through.
    #[cfg(not(feature = "chromium"))]
    if matches!(inputs.hint, ProviderHint::Chromium) {
        tracing::warn!(
            "ProviderHint::Chromium requested but wcore-browser was built without the \
             `chromium` feature; falling back to Camoufox. Rebuild with `--features chromium` \
             to honor the hint."
        );
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

    // #664: a Browserbase hint on a build without the `browserbase` feature
    // (the default) must still resolve to Camoufox (the warn arm falls through),
    // not panic or drop the request silently.
    #[cfg(not(feature = "browserbase"))]
    #[test]
    fn browserbase_hint_without_feature_falls_back_to_camoufox() {
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Browserbase,
            allow_cloud: true,
            ..Default::default()
        });
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

    #[cfg(not(feature = "chromium"))]
    #[test]
    fn chromium_hint_without_feature_warns_and_falls_back_to_camoufox() {
        // When the `chromium` feature is compiled out, a Chromium hint must
        // not be silently dropped: the warn-log path runs and selection still
        // falls back to Camoufox (the only non-cloud default).
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Chromium,
            ..Default::default()
        });
        assert_eq!(p.backend_name(), "camoufox");
    }

    #[cfg(feature = "browserbase")]
    #[test]
    fn browserbase_refused_when_policy_set_falls_back_to_camoufox() {
        // F17: Browserbase cannot enforce a BrowserPolicy (navigation +
        // redirects are server-side in the cloud browser). When a policy is
        // present, selection must refuse Browserbase and fall back to Camoufox
        // — regardless of whether BROWSERBASE_* creds are set in the env.
        let p = select_provider(SelectionInputs {
            hint: ProviderHint::Browserbase,
            allow_cloud: true,
            policy: Some(BrowserPolicy::default()),
            ..Default::default()
        });
        assert_eq!(
            p.backend_name(),
            "camoufox",
            "a set policy must refuse the policy-incapable Browserbase backend"
        );
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
