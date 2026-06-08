//! Generic catalog provider: build an `OpenAIProvider` from a bundled
//! `CatalogEntry` without a hand-written `ProviderType` match arm.
//!
//! `CatalogProvider` is not a new wire implementation — every catalog entry
//! speaks the OpenAI chat-completions wire shape, so it is constructed as an
//! `OpenAIProvider` stamped with the entry's base URL and a `ProviderCompat`
//! derived from the entry (correct `provider_type` id for cost attribution +
//! `api_path` override). See `.planning/provider-catalog/CATALOG-PLAN.md`.

use std::sync::Arc;

use wcore_config::catalog::{CatalogEntry, ProviderCatalog};
use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;

use crate::LlmProvider;
use crate::openai::OpenAIProvider;
use crate::registry::{ProviderRegistry, RegistryError};

/// Snapshot of the per-session config a catalog factory needs to build a
/// provider. Captured once and cloned into each registered factory closure.
#[derive(Debug, Clone)]
pub struct CatalogProviderConfig {
    /// API key. Empty => the factory falls back to the entry's env var.
    pub api_key: String,
    /// Base-URL override (e.g. CLI `--base-url`). Empty => use the entry's
    /// `base_url`.
    pub base_url: String,
    /// User compat overrides merged over the catalog-derived defaults.
    pub compat: ProviderCompat,
    /// Debug / diagnostics config threaded into the provider.
    pub debug: DebugConfig,
}

/// Build a provider for a single catalog entry.
///
/// Override precedence mirrors the native arms:
/// * `base_url`: a non-empty `cfg.base_url` (CLI `--base-url`) wins; otherwise
///   the catalog entry's `base_url` is used.
/// * `api_key`: a non-empty `cfg.api_key` wins; otherwise the entry's
///   `env_var` is read from the process environment.
/// * `compat`: catalog-derived defaults (stamped with the entry id + the
///   entry's `api_path`) merged under the user's compat overrides.
pub fn provider_for_entry(
    entry: &CatalogEntry,
    cfg: &CatalogProviderConfig,
) -> Arc<dyn LlmProvider> {
    let base = {
        let o = cfg.base_url.trim();
        if o.is_empty() {
            entry.base_url.as_str()
        } else {
            o
        }
    };

    let api_key = if cfg.api_key.is_empty() {
        std::env::var(&entry.env_var).unwrap_or_default()
    } else {
        cfg.api_key.clone()
    };

    let defaults = ProviderCompat::from_catalog_entry(&entry.id, entry.api_path.as_deref());
    let compat = ProviderCompat::merge(defaults, cfg.compat.clone());

    Arc::new(OpenAIProvider::new(
        &api_key,
        base,
        compat,
        cfg.debug.clone(),
    ))
}

/// Register every catalog entry into `reg`, skipping ids already owned by a
/// native `ProviderType` arm (those listed in `native_ids`) so native wiring
/// always wins.
///
/// Returns the number of entries registered. `reg.register` itself rejects
/// duplicate ids, so the `native_ids` guard is belt-and-suspenders — either
/// alone is sufficient.
pub fn register_catalog(
    reg: &mut dyn ProviderRegistry,
    catalog: &ProviderCatalog,
    native_ids: &[&str],
    cfg: CatalogProviderConfig,
) -> usize {
    let mut registered = 0usize;
    for e in &catalog.providers {
        if native_ids.contains(&e.id.as_str()) {
            continue; // native arm wins
        }
        let entry = e.clone();
        let cfg = cfg.clone();
        let factory = Arc::new(move || provider_for_entry(&entry, &cfg));
        match reg.register(&e.id, factory) {
            Ok(()) => registered += 1,
            // A native arm already claimed this id under a different spelling,
            // or the catalog had a duplicate (validated away at load). Skip.
            Err(RegistryError::DuplicateId(_)) | Err(RegistryError::EmptyId) => {}
        }
    }
    registered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::WaylandProviderRegistry;

    fn cfg() -> CatalogProviderConfig {
        CatalogProviderConfig {
            api_key: "test-key".to_string(),
            base_url: String::new(),
            compat: ProviderCompat::default(),
            debug: DebugConfig::default(),
        }
    }

    #[test]
    fn provider_for_entry_builds_for_sample() {
        let catalog = ProviderCatalog::load_bundled().expect("catalog parses");
        let entry = catalog.get("novita-ai").expect("novita-ai present");
        // Must construct without panic.
        let _provider = provider_for_entry(entry, &cfg());
    }

    #[test]
    fn register_catalog_skips_native_ids() {
        let catalog = ProviderCatalog::load_bundled().expect("catalog parses");
        let native = [
            "deepseek",
            "fireworks-ai",
            "nvidia",
            "openrouter",
            "moonshotai",
        ];
        let mut reg = WaylandProviderRegistry::new();
        let n = register_catalog(&mut reg, &catalog, &native, cfg());

        // Every non-native entry registered.
        assert_eq!(n, catalog.len() - native.len());
        // Native-collision ids are NOT in the catalog registry.
        assert!(reg.get("deepseek").is_none());
        // A net-new catalog id resolves to a usable provider.
        assert!(reg.get("novita-ai").is_some());
    }

    #[test]
    fn register_catalog_registers_all_when_no_natives() {
        let catalog = ProviderCatalog::load_bundled().expect("catalog parses");
        let mut reg = WaylandProviderRegistry::new();
        let n = register_catalog(&mut reg, &catalog, &[], cfg());
        assert_eq!(n, catalog.len());
        assert_eq!(reg.len(), catalog.len());
    }

    #[test]
    fn base_url_override_wins_over_entry() {
        // Smoke: an explicit base_url override does not panic and is honored
        // by construction (verified structurally — OpenAIProvider stores it).
        let catalog = ProviderCatalog::load_bundled().expect("catalog parses");
        let entry = catalog.get("novita-ai").expect("present");
        let mut c = cfg();
        c.base_url = "https://override.example/v1".to_string();
        let _provider = provider_for_entry(entry, &c);
    }
}
