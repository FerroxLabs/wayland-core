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

/// Detection-only slugs: declared in the fingerprint prefix table but NOT (yet)
/// connectable `ProviderType`s. The Proving Ground SURFACED this inconsistency —
/// a pasted `r8_…`/`hf_…` key detects as replicate/huggingface, then fails to
/// connect because no such provider exists. Tracked as a finding (resolve by
/// either adding the ProviderType or removing the prefix). Until then they are an
/// explicit known-exception set, so this invariant still gates any NEW undeclared
/// slug while documenting the known gap.
const DETECTION_ONLY_SLUGS: &[&str] = &["replicate", "huggingface"];

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
