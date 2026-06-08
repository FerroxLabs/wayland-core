//! E.13 — assert the `BrowserToolSpec` mirror lives in `wcore-plugin-api`
//! and has the shape required by `wcore_browser::BrowserTool::new` (without
//! depending on `wcore-browser` from this crate — the lint in build.rs
//! forbids it).
//!
//! The host adapter (in `wcore-agent`) is responsible for translating
//! `BrowserToolSpec` ↔ concrete `BrowserTool`. This test only verifies
//! the api-crate mirror surface.

use wcore_plugin_api::browser_spec::{
    BrowserOpSpec, BrowserPolicySpec, BrowserProviderHint, BrowserToolSpec,
};

#[test]
fn spec_fields_present_for_constructor_args() {
    let s = BrowserToolSpec {
        tool_namespace: "Browser".into(),
        preferred_provider: BrowserProviderHint::Browserbase,
        policy: BrowserPolicySpec {
            default_action: "ask".into(),
            allowed_origins: vec!["*.example.com".into()],
            denied_origins: vec![],
        },
        allow_cloud: true,
    };
    // The fields below must exactly correspond to the constructor args
    // of `wcore_browser::BrowserTool::new` (verified by hand-trace; the
    // host adapter in `wcore-agent` performs the translation).
    assert_eq!(s.tool_namespace, "Browser");
    assert_eq!(s.preferred_provider, BrowserProviderHint::Browserbase);
    assert!(s.allow_cloud);
    assert_eq!(s.policy.default_action, "ask");
    assert_eq!(s.policy.allowed_origins[0], "*.example.com");
}

#[test]
fn provider_hint_is_serde_round_trip() {
    for hint in [
        BrowserProviderHint::Auto,
        BrowserProviderHint::Camoufox,
        BrowserProviderHint::Chromium,
        BrowserProviderHint::Browserbase,
    ] {
        let s = serde_json::to_string(&hint).unwrap();
        let parsed: BrowserProviderHint = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, hint);
    }
}

#[test]
fn op_spec_default_allows_all_kinds() {
    let s = BrowserOpSpec::default();
    assert!(s.allowed_kinds.is_empty());
}
