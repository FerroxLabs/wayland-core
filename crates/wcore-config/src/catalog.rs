// Data-driven OpenAI-compatible provider catalog.
//
// A bundled TOML table (`data/providers.toml`, compiled in via `include_str!`)
// lets `--provider <id>` resolve a generic OpenAI-compatible provider WITHOUT a
// hand-written `ProviderType` match arm per provider. Each entry pairs a CLI id
// with a static OpenAI-wire base URL, the env var holding its key, and the
// chat-completions path suffix.
//
// Source of truth: the models.dev catalog (the same one OpenCode consumes).
// Curation rules and exclusions are documented in the bundled file header and
// in `.planning/provider-catalog/CATALOG-PLAN.md`.

use serde::Deserialize;
use std::sync::OnceLock;

use crate::compat::ProviderCompat;

/// Raw catalog text compiled into the binary. No runtime file dependency.
const RAW_CATALOG: &str = include_str!("data/providers.toml");

/// One OpenAI-compatible provider row.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    /// CLI id for `--provider <id>`. Unique across the catalog.
    pub id: String,
    /// Human-readable label.
    pub display_name: String,
    /// OpenAI-compatible REST root (no trailing slash).
    pub base_url: String,
    /// Env var holding the API key (e.g. `NOVITA_API_KEY`).
    pub env_var: String,
    /// Always `true` in the bundled file; kept so the schema is explicit.
    #[serde(default = "default_true")]
    pub openai_compatible: bool,
    /// Path appended to `base_url` for chat completions.
    ///
    /// `None` => `OpenAIProvider`/`ProviderCompat` default `/v1/chat/completions`
    /// (use when `base_url` is a bare host). `Some("/chat/completions")` when the
    /// base already ends in a version segment (`/v1`, `/v4`, `/openai`, ...).
    /// `Some("")` when `base_url` IS the full endpoint.
    #[serde(default)]
    pub api_path: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Parsed catalog: the ordered list of entries.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderCatalog {
    #[serde(rename = "provider", default)]
    pub providers: Vec<CatalogEntry>,
}

/// Errors raised while loading or validating the bundled catalog.
#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    /// The bundled TOML failed to parse.
    #[error("bundled provider catalog is not valid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    /// Two entries share an `id`.
    #[error("duplicate catalog id '{0}'")]
    DuplicateId(String),
    /// An entry is missing a required non-empty field.
    #[error("catalog entry '{id}' has an empty {field}")]
    EmptyField { id: String, field: &'static str },
}

impl ProviderCatalog {
    /// Parse and validate the bundled catalog.
    ///
    /// Fallible by design: a malformed bundled file is a build-time programmer
    /// error, but returning `Result` keeps `expect`/`unwrap` out of the hot
    /// path and lets tests assert the failure modes.
    pub fn load_bundled() -> Result<Self, CatalogError> {
        let catalog: ProviderCatalog = toml::from_str(RAW_CATALOG)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Process-wide cached catalog, parsed once. Returns `None` if the bundled
    /// file is malformed (a build-time error that surfaces in tests); callers
    /// on the resolution path treat `None`/miss as "not a catalog id".
    pub fn bundled() -> Option<&'static ProviderCatalog> {
        static CACHE: OnceLock<Option<ProviderCatalog>> = OnceLock::new();
        CACHE
            .get_or_init(|| ProviderCatalog::load_bundled().ok())
            .as_ref()
    }

    /// Validate structural invariants: unique ids, non-empty required fields.
    fn validate(&self) -> Result<(), CatalogError> {
        let mut seen = std::collections::HashSet::with_capacity(self.providers.len());
        for e in &self.providers {
            if e.id.trim().is_empty() {
                return Err(CatalogError::EmptyField {
                    id: e.id.clone(),
                    field: "id",
                });
            }
            if e.base_url.trim().is_empty() {
                return Err(CatalogError::EmptyField {
                    id: e.id.clone(),
                    field: "base_url",
                });
            }
            if e.env_var.trim().is_empty() {
                return Err(CatalogError::EmptyField {
                    id: e.id.clone(),
                    field: "env_var",
                });
            }
            if !seen.insert(e.id.as_str()) {
                return Err(CatalogError::DuplicateId(e.id.clone()));
            }
        }
        Ok(())
    }

    /// Look up an entry by exact id.
    pub fn get(&self, id: &str) -> Option<&CatalogEntry> {
        self.providers.iter().find(|e| e.id == id)
    }

    /// Number of entries in the catalog.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// True when the catalog has no entries.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

impl ProviderCompat {
    /// Build an OpenAI-wire compat preset for a catalog entry.
    ///
    /// Delegates to the OpenAI-compatible secondary preset (stamps the catalog
    /// `id` as `provider_type` for cost attribution and sets the `Some(0.0)`
    /// cost sentinel), then applies the entry's `api_path` override so
    /// `base_url + api_path` lands on the real chat-completions endpoint.
    pub fn from_catalog_entry(id: &str, api_path: Option<&str>) -> Self {
        let mut c = Self::openai_compat_provider(id);
        if let Some(p) = api_path {
            c.api_path = Some(p.to_string());
        }
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses() {
        let catalog = ProviderCatalog::load_bundled().expect("bundled catalog parses");
        assert!(
            catalog.len() >= 75,
            "expected >= 75 entries (OpenCode parity floor), got {}",
            catalog.len()
        );
    }

    #[test]
    fn cached_bundled_is_some() {
        assert!(ProviderCatalog::bundled().is_some());
    }

    #[test]
    fn all_ids_unique() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        let mut ids: Vec<&str> = catalog.providers.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(total, ids.len(), "catalog ids must be unique");
    }

    #[test]
    fn every_entry_has_required_fields() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        for e in &catalog.providers {
            assert!(!e.id.trim().is_empty(), "empty id");
            assert!(!e.base_url.trim().is_empty(), "empty base_url for {}", e.id);
            assert!(!e.env_var.trim().is_empty(), "empty env_var for {}", e.id);
            assert!(e.openai_compatible, "non-openai entry {}", e.id);
        }
    }

    #[test]
    fn get_returns_known_entry() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        let e = catalog.get("novita-ai").expect("novita-ai present");
        assert_eq!(e.base_url, "https://api.novita.ai/openai");
        assert_eq!(e.env_var, "NOVITA_API_KEY");
        assert_eq!(e.api_path.as_deref(), Some("/chat/completions"));
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        assert!(catalog.get("definitely-not-a-real-provider").is_none());
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let raw = r#"
            [[provider]]
            id = "dup"
            display_name = "A"
            base_url = "https://a.example/v1"
            env_var = "A_KEY"
            [[provider]]
            id = "dup"
            display_name = "B"
            base_url = "https://b.example/v1"
            env_var = "B_KEY"
        "#;
        let catalog: ProviderCatalog = toml::from_str(raw).expect("toml parses");
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::DuplicateId(id)) if id == "dup"
        ));
    }

    #[test]
    fn empty_base_url_is_rejected() {
        let raw = r#"
            [[provider]]
            id = "x"
            display_name = "X"
            base_url = ""
            env_var = "X_KEY"
        "#;
        let catalog: ProviderCatalog = toml::from_str(raw).expect("toml parses");
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::EmptyField {
                field: "base_url",
                ..
            })
        ));
    }

    // --- from_catalog_entry: compat derivation -------------------------------

    #[test]
    fn from_catalog_entry_stamps_id_and_path() {
        let c = ProviderCompat::from_catalog_entry("novita-ai", Some("/chat/completions"));
        assert_eq!(c.provider_type(), "novita-ai");
        assert_eq!(c.api_path(), "/chat/completions");
        // Cost sentinel: emit cost events, report $0 for unknown-model pricing.
        assert_eq!(c.cost_per_input_token, Some(0.0));
    }

    #[test]
    fn from_catalog_entry_none_path_defaults() {
        // Bare-host entry (no api_path) => OpenAI default endpoint.
        let c = ProviderCompat::from_catalog_entry("deepseek", None);
        assert_eq!(c.provider_type(), "deepseek");
        assert_eq!(c.api_path(), "/v1/chat/completions");
    }

    #[test]
    fn from_catalog_entry_empty_path_is_full_endpoint() {
        let c = ProviderCompat::from_catalog_entry("bailing", Some(""));
        assert_eq!(c.api_path(), "");
    }

    // --- URL resolution: base_url + api_path lands on a single endpoint ------

    #[test]
    fn version_segment_entries_yield_single_chat_completions() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        for e in &catalog.providers {
            let compat = ProviderCompat::from_catalog_entry(&e.id, e.api_path.as_deref());
            let url = format!("{}{}", e.base_url, compat.api_path());
            assert!(
                url.contains("/chat/completions") || url == e.base_url,
                "entry {} resolves to a non-chat endpoint: {}",
                e.id,
                url
            );
            // No doubled version segment.
            assert!(
                !url.contains("/v1/v1/"),
                "entry {} doubled a version segment: {}",
                e.id,
                url
            );
        }
    }

    #[test]
    fn bare_host_entry_resolves_to_v1_chat_completions() {
        let catalog = ProviderCatalog::load_bundled().expect("parses");
        let e = catalog.get("deepseek").expect("deepseek present");
        // deepseek omits api_path → default suffix.
        let compat = ProviderCompat::from_catalog_entry(&e.id, e.api_path.as_deref());
        let url = format!("{}{}", e.base_url, compat.api_path());
        assert_eq!(url, "https://api.deepseek.com/v1/chat/completions");
    }
}
