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
//! # The keyed-vs-keyless registry
//!
//! `wcore_providers::ProviderRegistry` is the *keyless* registry (it builds a
//! provider with no credential threading). The council instead derives a real
//! [`Config`] — carrying the api key / base url / compat / model from the
//! base config — and builds a keyed provider via
//! [`wcore_providers::create_provider`]. A council proposer must talk to the
//! actual upstream endpoint with real credentials, so the keyless registry is
//! deliberately *not* used here.
//!
//! # Credential source
//!
//! The resolved [`Config`] is the *runtime* config — it does NOT carry the
//! on-disk `[providers]` / `[profiles]` maps (those live on `ConfigFile` and
//! are consumed during `Config::resolve`). The resolver therefore derives each
//! provider's credentials from the `base` config it was constructed with:
//! `api_key`, `base_url`, `compat`, and `model` are inherited from `base`, and
//! only the `provider` / `provider_label` are overwritten to the requested
//! provider id. A `None`/empty derived key surfaces as [`ResolveError::Keyless`]
//! so the caller can decide skip-vs-fail (BYO-key council members are simply
//! skipped, not fatal).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use wcore_config::config::{Config, provider_type_from_slug, provider_type_slug};
use wcore_providers::{LlmProvider, create_provider};

/// Errors raised while resolving a council provider spec.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// The provider id is neither a built-in provider nor a known alias.
    #[error("unknown provider '{0}'")]
    Unknown(String),
    /// The derived config has no usable api key — a BYO-key provider the
    /// council can skip rather than fail on.
    #[error("provider '{0}' has no usable api key")]
    Keyless(String),
    /// The provider could be identified and keyed, but construction failed.
    #[error("provider build failed for '{0}': {1}")]
    Build(String, String),
}

/// Resolves `"provider"` / `"provider:model"` specs to keyed providers,
/// memoizing the built `Arc` per full spec string.
pub struct CouncilProviderResolver {
    base: Config,
    cache: Mutex<HashMap<String, Arc<dyn LlmProvider>>>,
}

impl CouncilProviderResolver {
    /// Build a resolver over a base [`Config`]. Each resolved provider clones
    /// `base` and overwrites only the provider identity, inheriting the base
    /// config's credentials / compat / model.
    pub fn new(base: Config) -> Self {
        Self {
            base,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve a `spec` (`"provider"` or `"provider:model"`) to a keyed
    /// provider plus the resolved model (the spec's model if given, else the
    /// derived config's model when non-empty).
    ///
    /// Memoized: repeated calls with the same `spec` return the same `Arc`.
    pub fn resolve(
        &self,
        spec: &str,
    ) -> Result<(Arc<dyn LlmProvider>, Option<String>), ResolveError> {
        // Split on the FIRST ':' → (provider_id, model?). A bare "provider"
        // has no model; "provider:model" pins the model.
        let (provider_id, spec_model) = match spec.split_once(':') {
            Some((id, model)) => (id, Some(model.to_string())),
            None => (spec, None),
        };

        // Normalize the provider id to a built-in `ProviderType`. The resolved
        // `Config` carries no `[providers]` alias map, so a non-built-in id is
        // genuinely unresolvable here → Unknown.
        let provider_type = provider_type_from_slug(provider_id)
            .ok_or_else(|| ResolveError::Unknown(provider_id.to_string()))?;

        // Derive a Config for this provider: clone the base (inheriting
        // api_key / base_url / compat / model) and overwrite the provider
        // identity. `provider_label` mirrors `provider_id` so cost labels and
        // resilience tags stay accurate.
        let mut derived = self.base.clone();
        derived.provider = provider_type;
        derived.provider_label = provider_type_slug(provider_type).to_string();
        if let Some(model) = &spec_model {
            derived.model = model.clone();
        }

        // A council proposer with no key is skipped, not fatal — surface
        // Keyless so the caller decides.
        if derived.api_key.trim().is_empty() {
            return Err(ResolveError::Keyless(provider_id.to_string()));
        }

        // The resolved model: the spec's model if present, else the derived
        // config's model when it carries a non-empty default.
        let resolved_model = spec_model.or_else(|| {
            if derived.model.is_empty() {
                None
            } else {
                Some(derived.model.clone())
            }
        });

        // Memoize by the FULL spec string so "openai" and "openai:gpt-5.5"
        // are distinct cache entries.
        let mut cache = self.cache.lock();
        if let Some(existing) = cache.get(spec) {
            return Ok((existing.clone(), resolved_model));
        }

        // `create_provider` is infallible at the type level (returns
        // `Arc<dyn LlmProvider>`, not a Result). We still funnel through a
        // `Build` variant so that if a future provider arm starts returning a
        // sentinel/error provider the wiring is already in place; today this
        // path always succeeds.
        let provider = create_provider(&derived);
        cache.insert(spec.to_string(), provider.clone());
        Ok((provider, resolved_model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::config::{Config, ProviderType};

    /// Build a resolved `Config` standing in for a base config that carries a
    /// single provider's credentials. The resolved `Config` has no on-disk
    /// `[providers]` map, so the council inherits `api_key` (and model) from
    /// the base directly.
    fn config_with_provider(id: &str, key: &str, model: Option<&str>) -> Config {
        let provider = provider_type_from_slug(id).unwrap_or(ProviderType::OpenAI);
        Config {
            provider,
            provider_label: id.to_string(),
            api_key: key.to_string(),
            model: model.map(|m| m.to_string()).unwrap_or_default(),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_splits_provider_and_model() {
        let base = config_with_provider("openai", "sk-test", None);
        let r = CouncilProviderResolver::new(base);
        let (_p, model) = r.resolve("openai:gpt-5.5").expect("resolve");
        assert_eq!(model.as_deref(), Some("gpt-5.5"));
    }

    #[test]
    fn resolve_skips_keyless_provider() {
        let base = config_with_provider("openai", "", None); // empty key
        let r = CouncilProviderResolver::new(base);
        assert!(matches!(r.resolve("openai"), Err(ResolveError::Keyless(_))));
    }

    #[test]
    fn resolve_errors_unknown_provider() {
        let r = CouncilProviderResolver::new(config_with_provider("openai", "sk", None));
        assert!(matches!(
            r.resolve("nope-xyz"),
            Err(ResolveError::Unknown(_))
        ));
    }

    #[test]
    fn resolve_is_memoized() {
        let r = CouncilProviderResolver::new(config_with_provider("openai", "sk", None));
        let a = r.resolve("openai").unwrap().0;
        let b = r.resolve("openai").unwrap().0;
        assert!(Arc::ptr_eq(&a, &b));
    }
}
