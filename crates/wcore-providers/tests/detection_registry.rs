use wcore_providers::fingerprint::{declared_prefixes, fingerprint_key};

#[test]
fn every_declared_prefix_resolves_to_exactly_one_provider() {
    for (prefix, slug) in declared_prefixes() {
        let key = format!("{prefix}TESTTESTTEST1234");
        let fp = fingerprint_key(&key);
        assert!(fp.is_unambiguous(), "{prefix} must resolve unambiguously");
        assert_eq!(fp.best().unwrap().slug, *slug, "{prefix} -> wrong provider");
    }
}

#[test]
fn sk_flux_is_a_declared_prefix() {
    assert!(
        declared_prefixes()
            .iter()
            .any(|(p, s)| *p == "sk-flux-" && *s == "flux-router"),
        "the Flux Router prefix must be registered (regression: it was missing)"
    );
}
