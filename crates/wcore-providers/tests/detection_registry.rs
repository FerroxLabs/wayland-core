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

/// Detection-only slugs: prefixes declared in the fingerprint table that do NOT
/// map to a connectable `ProviderType`. Intentionally EMPTY — every advertised
/// prefix must be connectable, or we make a broken promise to the user (detect a
/// key, then fail to connect). The Proving Ground surfaced `r8_`/`hf_`
/// (replicate/huggingface) as exactly this; they were removed rather than left
/// detectable-but-dead. If you add a prefix for a provider that isn't wired yet,
/// the invariant below will fail — wire the `ProviderType`, or don't declare the
/// prefix until it connects.
const DETECTION_ONLY_SLUGS: &[&str] = &[];

#[test]
fn every_declared_prefix_slug_parses_or_is_known_detection_only() {
    use wcore_config::config::provider_type_from_slug;
    for (prefix, slug) in declared_prefixes() {
        if DETECTION_ONLY_SLUGS.contains(slug) {
            continue;
        }
        assert!(
            provider_type_from_slug(slug).is_some(),
            "slug {slug:?} from prefix {prefix:?} has no ProviderType and is not in \
             DETECTION_ONLY_SLUGS — add it to parse_builtin_provider, or document it as detection-only"
        );
    }
}
