//! `CouncilProviderResolver` — turns a `"provider"` / `"provider:model"`
//! spec string into a keyed `Arc<dyn LlmProvider>`, memoized per spec.
//!
//! # Why this lives in `wcore-agent`
//!
//! Resolution (`String` → `Arc<dyn LlmProvider>`) needs both `wcore-config`
//! (to derive a per-provider [`Config`]) and `wcore-providers` (to build the
//! actual provider). `wcore-types` stays a leaf and carries only the plain
//! `Option<String>` provider/model fields; the keyed-provider resolution
//! happens here.
//!
//! # Keyed, per-provider credentials
//!
//! A cross-provider council must talk to *genuinely different* upstreams, each
//! with its own credentials. The credential source is therefore the on-disk
//! `[providers]` map (`HashMap<String, ProviderConfig>`), NOT a single
//! already-resolved [`Config`]. The heavy lifting — alias + catalog resolution,
//! the inline-key → store → env credential chain, and compat derivation — is
//! reused verbatim from `wcore_config::config::resolve_council_provider`, which
//! shares its logic with `Config::resolve`. Every non-provider runtime setting
//! is inherited from the `base` config, so council members share the session's
//! policy surface and differ only in provider identity, endpoint, model, key.
//!
//! A council member whose key cannot be resolved surfaces as
//! [`ResolveError::Keyless`] so the caller can skip it (BYO-key members are
//! simply skipped, not fatal); an unresolvable id surfaces as
//! [`ResolveError::Unknown`].

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use wcore_config::config::{
    Config, CouncilProviderError, ProviderConfig, resolve_council_provider,
};
use wcore_pricing::{flux_pinned_native, flux_pinned_vendor};
use wcore_providers::{LlmProvider, create_provider};

/// Derive a provider-family key for a spec at runtime — the discriminator the
/// Assembler uses to maximize provider diversity in a council.
///
/// - A priceable Flux pinned-tier model maps to its CATALOG PROVIDER (so
///   `flux-pinned-gpt-5` and `openai:gpt-5` share the family `openai` — the same
///   underlying model never counts as two "diverse" picks).
/// - An unpriced Flux vendor (no catalog row: glm, kimi, …) falls back to its
///   vendor token, still a distinct family per vendor lineage.
/// - A plain `provider` / `provider:model` spec → the provider token.
/// - Anything else → the spec itself (a fail-open singleton family).
///
/// Runtime derivation, NOT a hardcoded provider enum (which would violate the
/// No-Hardcoded-Provider-Quirks rule).
pub fn family(spec: &str) -> String {
    // BYO OpenRouter route: `openrouter:<upstream>/<model>`. Canonicalize to the
    // UPSTREAM vendor so the same model via OpenRouter vs direct shares a family
    // and judge-independence is not defeated by the routing prefix.
    if let Some(rest) = spec.strip_prefix("openrouter:")
        && let Some((upstream, _model)) = rest.split_once('/')
        && !upstream.is_empty()
    {
        return upstream.to_string();
    }
    if let Some((provider, _model)) = flux_pinned_native(spec) {
        return provider;
    }
    if let Some(vendor) = flux_pinned_vendor(spec) {
        return vendor;
    }
    // Plain `provider` / `provider:model` → the provider token. A leading-colon
    // spec (`:x`) would otherwise yield an empty token and collapse all such
    // specs into one bogus family; degrade to the spec itself so it forms a
    // fail-open singleton instead.
    let token = spec.split(':').next().unwrap_or(spec);
    if token.is_empty() {
        spec.to_string()
    } else {
        token.to_string()
    }
}

/// Errors raised while resolving a council provider spec.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// The provider id is neither a built-in provider, a `[providers]` alias,
    /// nor a bundled catalog entry.
    #[error("unknown provider '{0}'")]
    Unknown(String),
    /// The derived config has no usable api key — a BYO-key provider the
    /// council can skip rather than fail on.
    #[error("provider '{0}' has no usable api key")]
    Keyless(String),
    /// The provider could be identified and keyed, but construction failed.
    ///
    /// `create_provider` is infallible at the type level today, so this is not
    /// currently produced; it is retained so that if a future provider arm
    /// starts returning a fallible/sentinel provider the public surface is
    /// already in place.
    #[error("provider build failed for '{0}': {1}")]
    Build(String, String),
}

impl From<CouncilProviderError> for ResolveError {
    fn from(e: CouncilProviderError) -> Self {
        match e {
            CouncilProviderError::Unknown(id) => ResolveError::Unknown(id),
            CouncilProviderError::Keyless(id) => ResolveError::Keyless(id),
        }
    }
}

/// Resolves `"provider"` / `"provider:model"` specs to keyed providers,
/// memoizing the built `Arc` per full spec string.
pub struct CouncilProviderResolver {
    base: Config,
    providers: HashMap<String, ProviderConfig>,
    cache: Mutex<HashMap<String, Arc<dyn LlmProvider>>>,
}

impl CouncilProviderResolver {
    /// Build a resolver over a `base` [`Config`] and the on-disk `[providers]`
    /// map. Each resolved provider derives a keyed [`Config`] from `providers`
    /// (pulling that provider's own credentials) while inheriting every
    /// non-provider runtime setting from `base`.
    pub fn new(base: Config, providers: HashMap<String, ProviderConfig>) -> Self {
        Self {
            base,
            providers,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a `spec` (`"provider"` or `"provider:model"`) to a keyed
    /// provider plus the resolved model (the spec's model if pinned, else the
    /// derived config's model when non-empty).
    ///
    /// Memoized: repeated calls with the same `spec` return the same `Arc`.
    pub fn resolve(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        // Derive the keyed Config + resolved model via the shared wcore-config
        // helper (alias/catalog resolution, credential chain, compat).
        let (derived, resolved_model) =
            resolve_council_provider(&self.providers, &self.base, spec)?;

        // Memoize by the FULL spec string so "openai" and "openai:gpt-5.5"
        // are distinct cache entries.
        let mut cache = self.cache.lock();
        if let Some(existing) = cache.get(spec) {
            return Ok((existing.clone(), resolved_model));
        }

        let provider = create_provider(&derived);
        cache.insert(spec.to_string(), provider.clone());
        Ok((provider, resolved_model))
    }

    /// Filter `candidates` (`provider` / `provider:model` specs) to those that
    /// resolve to a keyed provider — dropping keyless (BYO-key) and unknown
    /// specs. This is the auto Assembler's runnable candidate pool. Order-
    /// preserving and deduplicated by the full spec string. Never logs a key.
    ///
    /// The pool is the set of `provider:model` specs (from `[crucible].proposers`
    /// / `candidate_pool`), NOT the `[providers]` map keys: a Flux council shares
    /// one provider (`flux-router`) across many models, so enumerating provider
    /// keys would collapse to a single spec and destroy diversity.
    pub fn resolvable_specs(&self, candidates: &[String]) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        candidates
            .iter()
            .filter(|s| seen.insert((*s).clone()))
            .filter(|s| self.resolve(s).is_ok())
            .cloned()
            .collect()
    }
}

/// Abstraction over council provider resolution so the spawner can resolve a
/// pinned provider spec without depending on the concrete
/// [`CouncilProviderResolver`] — and so tests can inject a resolver that hands
/// back mock providers (the cross-provider-diversity guard relies on this).
///
/// `CouncilProviderResolver` is the production implementation; bootstrap
/// constructs one and attaches it to the `AgentSpawner` as
/// `Arc<dyn ProviderResolver>`.
pub trait ProviderResolver: Send + Sync {
    /// Resolve a `"provider"` / `"provider:model"` spec to a keyed provider
    /// plus the resolved model (the spec's model if pinned, else the provider
    /// default when non-empty).
    fn resolve_provider(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError>;
}

impl ProviderResolver for CouncilProviderResolver {
    fn resolve_provider(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        self.resolve(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::config::Config;

    /// Build a `[providers]` map with one inline-keyed entry. An inline key
    /// short-circuits the credential chain, keeping the test hermetic (no
    /// store / env access).
    fn providers_with(id: &str, key: &str, model: Option<&str>) -> HashMap<String, ProviderConfig> {
        let mut map = HashMap::new();
        map.insert(
            id.to_string(),
            ProviderConfig {
                api_key: Some(key.to_string()),
                model: model.map(|m| m.to_string()),
                ..Default::default()
            },
        );
        map
    }

    #[test]
    fn resolve_splits_provider_and_model() {
        let r = CouncilProviderResolver::new(
            Config::default(),
            providers_with("openai", "sk-test", None),
        );
        let (_p, model) = r.resolve("openai:gpt-5.5").expect("resolve");
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn resolve_skips_genuinely_keyless_provider() {
        // `cohere` REQUIRES an inline key; with none configured and
        // COHERE_API_KEY unset, resolution is Keyless (skip). Out-of-band
        // providers (vertex/bedrock/chatgpt) are NOT keyless — see the
        // config-layer `council_resolves_out_of_band_provider` test.
        let r = CouncilProviderResolver::new(Config::default(), HashMap::new());
        assert!(matches!(r.resolve("cohere"), Err(ResolveError::Keyless(_))));
    }

    #[test]
    fn resolve_errors_unknown_provider() {
        let r =
            CouncilProviderResolver::new(Config::default(), providers_with("openai", "sk", None));
        assert!(matches!(
            r.resolve("nope-xyz"),
            Err(ResolveError::Unknown(_))
        ));
    }

    #[test]
    fn resolve_is_memoized() {
        let r =
            CouncilProviderResolver::new(Config::default(), providers_with("openai", "sk", None));
        let a = r.resolve("openai").unwrap().0;
        let b = r.resolve("openai").unwrap().0;
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn family_groups_by_vendor_and_is_cross_source_consistent() {
        // Priced flux-pinned models map to their catalog provider.
        assert_eq!(
            family("flux-router:flux-pinned-claude-opus-4-8"),
            "anthropic"
        );
        assert_eq!(family("flux-pinned-gpt-5"), "openai");
        // Same underlying model via flux vs direct → SAME family (no false
        // "diversity" from the same model on two sources).
        assert_eq!(family("flux-pinned-gpt-5"), family("openai:gpt-5"));
        // Unpriced flux vendors still get a distinct family from their token.
        assert_eq!(family("flux-pinned-glm-5-2"), "glm");
        assert_eq!(family("flux-pinned-kimi-k2"), "kimi");
        assert_ne!(family("flux-pinned-glm-5-2"), family("flux-pinned-kimi-k2"));
        // Plain specs use the provider token; unknown → fail-open singleton.
        assert_eq!(family("anthropic"), "anthropic");
        assert_eq!(family("totally-unknown:x"), "totally-unknown");
        assert!(!family("totally-unknown:x").is_empty());
        // A leading-colon spec degrades to a fail-open singleton, NOT a shared
        // empty family that would mis-count diversity.
        assert_eq!(family(":x"), ":x");
        // BYO OpenRouter routes canonicalize to the UPSTREAM vendor, else an
        // OpenRouter-routed Claude judge vs OpenRouter-routed GPT proposer both
        // read as family "openrouter" (false same-vendor) and independence leaks.
        assert_eq!(family("openrouter:anthropic/claude-opus-4-8"), "anthropic");
        assert_eq!(family("openrouter:openai/gpt-5"), "openai");
        assert_eq!(family("openrouter:openai/gpt-5"), family("openai:gpt-5"));
    }

    #[test]
    fn resolvable_specs_keeps_keyed_drops_keyless_and_dedups() {
        let r = CouncilProviderResolver::new(
            Config::default(),
            providers_with("openai", "sk-test", None),
        );
        let candidates = vec![
            "openai".to_string(),
            "openai".to_string(),       // duplicate → collapsed
            "openai:gpt-5".to_string(), // distinct spec → kept (keyed via the map)
            "cohere".to_string(),       // requires an inline key, none set → Keyless skip
            "nope-xyz".to_string(),     // unknown → skip
        ];
        let runnable = r.resolvable_specs(&candidates);
        assert_eq!(
            runnable,
            vec!["openai".to_string(), "openai:gpt-5".to_string()]
        );
    }

    #[test]
    fn resolve_pulls_distinct_per_provider_keys() {
        // The core cross-provider guarantee at the resolver layer: two members
        // keyed to two providers each carry their own credentials.
        let mut providers = HashMap::new();
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                api_key: Some("sk-openai-aaa".to_string()),
                ..Default::default()
            },
        );
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                api_key: Some("sk-ant-bbb".to_string()),
                ..Default::default()
            },
        );
        let r = CouncilProviderResolver::new(Config::default(), providers);
        // Distinct specs → distinct memoized providers (no Arc aliasing across
        // different providers).
        let oa = r.resolve("openai").expect("openai").0;
        let an = r.resolve("anthropic").expect("anthropic").0;
        assert!(!Arc::ptr_eq(&oa, &an));
    }
}
