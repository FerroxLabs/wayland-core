//! RED ARM for gh#1112 — `ChromiumBackend` applies no browser policy at all.
//!
//! `selection.rs:99-104` hands back `ChromiumBackend::new()` on a
//! `ProviderHint::Chromium` without ever looking at `inputs.policy`, and the
//! backend calls `page.goto(url)` raw at `backends/chromium.rs:168`.
//!
//! Absence claim, verified at `0ccaa90b`:
//!   `grep -c policy crates/wcore-browser/src/backends/chromium.rs` → **0**.
//! Positive control, same query against the sibling backend:
//!   `grep -c policy crates/wcore-browser/src/backends/camoufox.rs` → **many**,
//!   including the pre-flight and post-navigation calls at `camoufox.rs:302`
//!   and `camoufox.rs:548`. The query works; the absence is real.
//!
//! Consequences under `--features chromium`: no `allowed_origins` /
//! `denied_origins`, no loopback-capability gate, no RFC1918 / metadata /
//! loopback refusal, no post-navigation landing-URL re-check. Every guarantee
//! `BrowserPolicy` is documented to provide is silently absent, with no
//! indication that containment is missing.
//!
//! The in-crate precedent for the fix already exists: `BrowserbaseBackend` has
//! the same unenforceable-policy problem and `selection.rs:56-84` refuses it
//! outright when a policy is in force, with a `tracing::warn!` naming why.
//! Chromium must mirror that arm.
//!
//! This file is compiled only under `--features chromium`, the same gate that
//! makes the backend reachable at all. Run it with:
//!   `cargo nextest run -p wcore-browser --features chromium`

#![cfg(feature = "chromium")]

use wcore_browser::policy::{BrowserPolicy, PolicyAction};
use wcore_browser::selection::{ProviderHint, SelectionInputs, select_provider};

/// RED. With a policy in force, a Chromium hint must NOT resolve to a backend
/// that enforces nothing — it must fall through to Camoufox, mirroring the
/// Browserbase arm.
#[test]
fn chromium_hint_is_refused_when_a_policy_is_in_force() {
    let provider = select_provider(SelectionInputs {
        hint: ProviderHint::Chromium,
        allow_cloud: false,
        camoufox_url: Some("http://unused.invalid:9377".into()),
        policy: Some(BrowserPolicy::new(
            PolicyAction::Deny,
            vec!["example.com".into()],
            vec![],
        )),
    });

    assert_ne!(
        provider.backend_name(),
        "chromium",
        "a BrowserPolicy is in force and ChromiumBackend enforces none of it \
         (0 policy references in backends/chromium.rs); selection must refuse \
         it the way selection.rs:56-84 refuses Browserbase"
    );
    assert_eq!(
        provider.backend_name(),
        "camoufox",
        "the refusal must fall through to the backend that DOES enforce the \
         policy, not to some other unenforced backend"
    );
}

/// RED. The fail-closed default policy is the one most deployments run, and it
/// is exactly the configuration under which an unenforced backend is worst:
/// the operator believes everything is denied by default.
#[test]
fn chromium_hint_is_refused_under_the_default_fail_closed_policy() {
    let provider = select_provider(SelectionInputs {
        hint: ProviderHint::Chromium,
        allow_cloud: false,
        camoufox_url: Some("http://unused.invalid:9377".into()),
        policy: Some(BrowserPolicy::default()),
    });
    assert_ne!(
        provider.backend_name(),
        "chromium",
        "default_action=Deny is the shipped posture; a backend that ignores it \
         entirely must not be selectable under it"
    );
}

/// NEGATIVE CONTROL — pairs with both tests above and must pass BOTH before
/// and after the fix. `policy: None` is the legacy "no enforcement expectation"
/// mode (the same escape `selection.rs:70` leaves for Browserbase), so the
/// Chromium hint must still be honoured there. Without this control the
/// refusal tests could pass by never selecting Chromium at all, which would
/// make the `chromium` feature dead rather than gated.
#[test]
fn chromium_hint_is_honored_when_no_policy_is_in_force() {
    let provider = select_provider(SelectionInputs {
        hint: ProviderHint::Chromium,
        allow_cloud: false,
        camoufox_url: Some("http://unused.invalid:9377".into()),
        policy: None,
    });
    assert_eq!(
        provider.backend_name(),
        "chromium",
        "with no policy in force there is nothing to bypass; the hint must \
         still be honoured or the feature is dead code"
    );
}
