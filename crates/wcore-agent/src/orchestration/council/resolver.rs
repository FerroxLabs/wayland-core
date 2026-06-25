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
use wcore_providers::{LlmProvider, create_provider};

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
    fn resolve_skips_keyless_provider() {
        // Vertex resolves to an empty key when unconfigured → Keyless (skip).
        let r = CouncilProviderResolver::new(Config::default(), HashMap::new());
        assert!(matches!(r.resolve("vertex"), Err(ResolveError::Keyless(_))));
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
