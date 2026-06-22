//! Pricing-as-data for LLM providers.
//!
//! Loads a TOML catalog of provider × model × input/output token rates
//! (USD per million tokens) and exposes a microcent-integer cost API.
//! Default catalog is bundled at compile time. Override via
//! WAYLAND_PRICING_PATH env var.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

pub mod refresh;
pub use refresh::{
    CachedCatalog, CatalogChange, ChangeKind, PricingRefresher, RefreshError, default_cache_path,
};

const BUNDLED_PRICING_TOML: &str = include_str!("../pricing.toml");

#[derive(Debug, Error)]
pub enum PricingError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("unknown model {model} for provider {provider}")]
    UnknownModel { provider: String, model: String },
    #[error("toml parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ModelPrice {
    pub input_per_mtok_usd: f64,
    pub output_per_mtok_usd: f64,
    #[serde(default)]
    pub cache_read_per_mtok_usd: Option<f64>,
    #[serde(default)]
    pub cache_write_per_mtok_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PricingCatalog {
    #[serde(flatten)]
    pub providers: HashMap<String, HashMap<String, ModelPrice>>,
}

impl PricingCatalog {
    pub fn load_default() -> Result<Self, PricingError> {
        if let Ok(path) = std::env::var("WAYLAND_PRICING_PATH") {
            let raw = std::fs::read_to_string(&path)?;
            return Ok(toml::from_str(&raw)?);
        }
        Ok(toml::from_str(BUNDLED_PRICING_TOML)?)
    }

    pub fn get(&self, provider: &str, model: &str) -> Result<&ModelPrice, PricingError> {
        let prov = self
            .providers
            .get(provider)
            .ok_or_else(|| PricingError::UnknownProvider(provider.into()))?;
        prov.get(model).ok_or_else(|| PricingError::UnknownModel {
            provider: provider.into(),
            model: model.into(),
        })
    }

    pub fn estimate_cost_microcents(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Result<u64, PricingError> {
        let p = self.get(provider, model)?;
        let in_usd = (input_tokens as f64 / 1_000_000.0) * p.input_per_mtok_usd;
        let out_usd = (output_tokens as f64 / 1_000_000.0) * p.output_per_mtok_usd;
        let total_microcents = ((in_usd + out_usd) * 100.0 * 1_000_000.0).round() as u64;
        Ok(total_microcents)
    }
}

pub static DEFAULT_CATALOG: Lazy<PricingCatalog> = Lazy::new(|| {
    PricingCatalog::load_default().unwrap_or_else(|e| {
        eprintln!("wcore-pricing: failed to load default catalog: {e}; using empty");
        PricingCatalog {
            providers: HashMap::new(),
        }
    })
});

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_catalog() -> PricingCatalog {
        let raw = r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
cache_read_per_mtok_usd = 1.5
cache_write_per_mtok_usd = 18.75

[openai.gpt-5]
input_per_mtok_usd = 5.0
output_per_mtok_usd = 15.0
"#;
        toml::from_str(raw).unwrap()
    }

    #[test]
    fn load_default_succeeds() {
        let cat = PricingCatalog::load_default().expect("bundled catalog should parse");
        assert!(!cat.providers.is_empty());
    }

    #[test]
    fn get_known_model() {
        let cat = fixture_catalog();
        let p = cat.get("anthropic", "claude-opus-4-7").unwrap();
        assert!((p.input_per_mtok_usd - 15.0).abs() < 1e-9);
    }

    #[test]
    fn unknown_provider_errors() {
        let cat = fixture_catalog();
        assert!(matches!(
            cat.get("nonexistent", "x"),
            Err(PricingError::UnknownProvider(_))
        ));
    }

    // #240: MiniMax-M2 must resolve from the bundled catalog so estimates use
    // real per-token pricing ($0.30/$1.20 per MTok) instead of the heuristic.
    #[test]
    fn minimax_m2_in_bundled_catalog() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        let p = cat
            .get("minimax", "MiniMax-M2")
            .expect("MiniMax-M2 must be in the bundled catalog");
        assert!((p.input_per_mtok_usd - 0.30).abs() < 1e-9);
        assert!((p.output_per_mtok_usd - 1.20).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_errors() {
        let cat = fixture_catalog();
        assert!(matches!(
            cat.get("anthropic", "nonexistent"),
            Err(PricingError::UnknownModel { .. })
        ));
    }

    #[test]
    fn cost_in_microcents() {
        let cat = fixture_catalog();
        let mc = cat
            .estimate_cost_microcents("anthropic", "claude-opus-4-7", 1_000_000, 0)
            .unwrap();
        assert_eq!(mc, 1_500_000_000);
    }

    #[test]
    fn cost_combined_in_out() {
        let cat = fixture_catalog();
        let mc = cat
            .estimate_cost_microcents("anthropic", "claude-opus-4-7", 500_000, 100_000)
            .unwrap();
        assert_eq!(mc, 1_500_000_000);
    }

    #[test]
    fn cost_zero_tokens_zero_cost() {
        let cat = fixture_catalog();
        let mc = cat
            .estimate_cost_microcents("openai", "gpt-5", 0, 0)
            .unwrap();
        assert_eq!(mc, 0);
    }

    /// D.2 (v0.6.3) — the bundled catalog must carry entries for the 6
    /// new Tier-2 providers keyed by their REAL provider id (not "openai"),
    /// so the budget chain resolves a real per-Mtok rate instead of the
    /// GPT-class fallback. Each rate must be a non-zero open-weight price,
    /// well below GPT-4o's $8/Mtok input.
    #[test]
    fn bundled_catalog_has_tier2_provider_entries() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        let cases: &[(&str, &str)] = &[
            ("azure-openai", "gpt-5"),
            ("together", "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
            (
                "fireworks",
                "accounts/fireworks/models/llama-v3p3-70b-instruct",
            ),
            ("nvidia", "meta/llama-3.3-70b-instruct"),
            ("perplexity", "sonar"),
            ("cerebras", "llama-3.3-70b"),
        ];
        for (provider, model) in cases {
            let p = cat
                .get(provider, model)
                .unwrap_or_else(|e| panic!("{provider}/{model} must be in catalog: {e}"));
            assert!(
                p.input_per_mtok_usd > 0.0,
                "{provider}/{model} input rate must be non-zero"
            );
            assert!(
                p.input_per_mtok_usd < 8.0,
                "{provider}/{model} input rate must be below GPT-4o's $8/Mtok"
            );
        }
    }

    /// A model NOT in the catalog for a Tier-2 provider must MISS
    /// gracefully (Err) — the engine then falls back to the ProviderCompat
    /// heuristic, which is 0.0 for these presets. An honest absent charge,
    /// never a confidently-wrong one.
    #[test]
    fn tier2_unknown_model_misses_gracefully() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        assert!(matches!(
            cat.get("together", "some/unlisted-model"),
            Err(PricingError::UnknownModel { .. })
        ));
    }
}
