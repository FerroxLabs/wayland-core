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
        let raw = if let Ok(path) = std::env::var("WAYLAND_PRICING_PATH") {
            std::fs::read_to_string(&path)?
        } else {
            BUNDLED_PRICING_TOML.to_string()
        };
        Self::from_toml_str(&raw)
    }

    /// Parse a catalog from a TOML string. Deserializes directly into the
    /// provider→model map instead of going through `PricingCatalog`'s
    /// `#[serde(flatten)]`, which the `toml` crate mishandles for
    /// externally-supplied catalogs: a perfectly valid `WAYLAND_PRICING_PATH`
    /// file whose first line is a bare `[provider.model]` table otherwise
    /// fails to load with a spurious "TOML parse error at line 1, column 1".
    pub fn from_toml_str(raw: &str) -> Result<Self, PricingError> {
        let providers: HashMap<String, HashMap<String, ModelPrice>> = toml::from_str(raw)?;
        Ok(Self { providers })
    }

    pub fn get(&self, provider: &str, model: &str) -> Result<&ModelPrice, PricingError> {
        let prov = self
            .providers
            .get(provider)
            .ok_or_else(|| PricingError::UnknownProvider(provider.into()))?;
        if let Some(p) = prov.get(model) {
            return Ok(p);
        }
        // Live API model slugs use dots in the version segment
        // (`gemini-2.5-flash`) while catalog keys use dashes
        // (`gemini-2-5-flash`). Retry with dots→dashes so a dotted slug still
        // resolves. Exact match is tried first, so dotted catalog keys
        // (e.g. `gpt-4.1-mini`) are unaffected.
        let normalized = model.replace('.', "-");
        if normalized != model
            && let Some(p) = prov.get(&normalized)
        {
            return Ok(p);
        }
        Err(PricingError::UnknownModel {
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

    /// Cost in microcents, resolving a Flux pinned-tier model to its native SKU.
    ///
    /// Tries the literal `(provider, model)` first, so every non-Flux provider
    /// prices exactly as `estimate_cost_microcents` does. If that misses and the
    /// spec is a `flux-pinned-*` model, derive the native `(provider, model)` via
    /// [`flux_pinned_native`], price THAT exact row, and apply `markup` (Flux's
    /// flat-rate / markup factor — `1.0` means the underlying native rate).
    ///
    /// Returns `None` when neither the literal key nor an exact native row exists.
    /// It is EXACT-MATCH-OR-NONE: it never guesses a "nearest" model and never
    /// silently charges an unpriced member $0 (the caller decides what to do with
    /// an unpriced member — see the Assembler's eligibility filter).
    ///
    /// Stopgap until Flux emits an authoritative per-request cost
    /// (FerroxLabs/wayland#319), which will replace this derivation.
    pub fn estimate_cost_microcents_resolved(
        &self,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        markup: f64,
    ) -> Option<u64> {
        // A nonsense markup must never fabricate a price: NaN/inf/negative would
        // saturate the `as u64` cast to 0 (or u64::MAX) and silently report an
        // unpriced member as priceable, defeating the budget guard. Treat it as
        // unpriceable. (markup == 0.0 is a deliberate "free" choice and allowed.)
        if !markup.is_finite() || markup < 0.0 {
            return None;
        }
        // Literal key first — non-flux providers are unaffected by this path.
        if let Ok(c) = self.estimate_cost_microcents(provider, model, input_tokens, output_tokens) {
            return Some(c);
        }
        // Flux pinned-tier: price the underlying native SKU × markup.
        // `flux_pinned_native` already strips an optional `flux-router:` prefix,
        // so the model token alone is sufficient.
        let (native_provider, native_model) = flux_pinned_native(model)?;
        let native = self
            .estimate_cost_microcents(&native_provider, &native_model, input_tokens, output_tokens)
            .ok()?;
        Some(((native as f64) * markup).round() as u64)
    }
}

/// Flux pinned-tier vendor token → catalog provider key that prices the
/// underlying native SKU. Sanctioned derivation data (like
/// `provider_type_from_slug`), NOT a hardcoded model list.
///
/// Every target MUST be a real provider section in the bundled pricing catalog —
/// `every_flux_vendor_maps_to_a_real_catalog_provider` enforces this so a mapping
/// can never silently point at a nonexistent key. NOTE: Gemini's catalog key is
/// `gemini` (NOT `google`); mapping to `google` would miss every Gemini row.
///
/// Only vendors whose native SKUs are actually in the catalog are listed. Other
/// live Flux vendors (glm, kimi, nova, qwen, …) are deliberately ABSENT: their
/// catalog rows don't exist yet, so `flux_pinned_native` returns `None` for them
/// (EXACT-MATCH-OR-NONE → unpriced) rather than aiming at a phantom provider key.
/// Add them here only alongside their `pricing.toml` rows (or once #319 lands).
const FLUX_VENDOR_TO_CATALOG_PROVIDER: &[(&str, &str)] = &[
    ("claude", "anthropic"),
    ("gpt", "openai"),
    ("gemini", "gemini"),
    ("deepseek", "deepseek"),
    ("grok", "xai"),
];

/// Look up the catalog provider for a vendor token. Fails open: an unknown
/// vendor returns `None` so the caller treats the model as unpriced.
fn flux_vendor_to_catalog_provider(vendor: &str) -> Option<&'static str> {
    FLUX_VENDOR_TO_CATALOG_PROVIDER
        .iter()
        .find(|(v, _)| *v == vendor)
        .map(|(_, p)| *p)
}

/// Extract the vendor token from a Flux pinned-tier spec — the family-grouping
/// discriminator. Accepts an optional `flux-router:` prefix, requires the
/// `flux-pinned-` prefix, and returns the first `-`-separated segment of the
/// remainder (`claude`, `gpt`, `gemini`, `glm`, `kimi`, …).
///
/// Unlike [`flux_pinned_native`] this does NOT consult the pricing alias table,
/// so it yields a token even for vendors with no catalog price (glm, kimi, …) —
/// exactly what diversity grouping needs (each distinct vendor is its own
/// family). Returns `None` for a non-flux-pinned spec or an empty remainder.
pub fn flux_pinned_vendor(spec: &str) -> Option<String> {
    let model = spec.strip_prefix("flux-router:").unwrap_or(spec);
    let rest = model.strip_prefix("flux-pinned-")?;
    let vendor = rest.split('-').next().filter(|v| !v.is_empty())?;
    Some(vendor.to_string())
}

/// Derive the native `(catalog_provider, native_model)` for a Flux pinned-tier
/// model spec so it can be priced against the catalog's underlying SKU.
///
/// Accepts an optional `flux-router:` provider prefix, then REQUIRES the
/// `flux-pinned-` model prefix (returns `None` otherwise). The vendor token (via
/// [`flux_pinned_vendor`]) is mapped to the catalog provider via
/// [`flux_vendor_to_catalog_provider`]; the FULL remainder is the native model
/// id, since catalog model ids carry the vendor prefix (`claude-opus-4-8`,
/// `gpt-5`, `deepseek-v4-pro`, `gemini-3-1-pro`).
///
/// Returns `None` for a non-flux-pinned spec, an empty remainder, or an unknown
/// vendor token — it never guesses a model.
pub fn flux_pinned_native(spec: &str) -> Option<(String, String)> {
    let model = spec.strip_prefix("flux-router:").unwrap_or(spec);
    let rest = model.strip_prefix("flux-pinned-")?;
    let vendor = flux_pinned_vendor(spec)?;
    let provider = flux_vendor_to_catalog_provider(&vendor)?;
    Some((provider.to_string(), rest.to_string()))
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

    // Regression (#4): a custom catalog whose FIRST line is a bare
    // `[provider.model]` table (no leading comment) must parse. The old
    // `#[serde(flatten)]` load path failed here with "parse error line 1".
    #[test]
    fn from_toml_str_parses_leading_bare_table() {
        let raw = "[anthropic.claude-opus-4-7]\ninput_per_mtok_usd = 5.0\noutput_per_mtok_usd = 25.0\n\n[openai.gpt-4o]\ninput_per_mtok_usd = 2.5\noutput_per_mtok_usd = 10.0\n";
        let cat = PricingCatalog::from_toml_str(raw).expect("leading-table catalog parses");
        assert_eq!(cat.providers.len(), 2);
        assert!(cat.get("anthropic", "claude-opus-4-7").is_ok());
    }

    // Regression (#1): a dotted live API slug must resolve to the dashed
    // catalog key. Exact match still wins for genuinely-dotted keys.
    #[test]
    fn dotted_api_slug_resolves_to_dashed_key() {
        let raw = "[gemini.gemini-2-5-flash]\ninput_per_mtok_usd = 0.30\noutput_per_mtok_usd = 2.50\n\n[openai.\"gpt-4.1-mini\"]\ninput_per_mtok_usd = 0.40\noutput_per_mtok_usd = 1.60\n";
        let cat = PricingCatalog::from_toml_str(raw).unwrap();
        // dotted lookup resolves to the dashed key
        assert!(cat.get("gemini", "gemini-2.5-flash").is_ok());
        // exact (dashed) still works
        assert!(cat.get("gemini", "gemini-2-5-flash").is_ok());
        // a genuinely-dotted catalog key is matched exactly (not mangled)
        assert!(cat.get("openai", "gpt-4.1-mini").is_ok());
        // a real miss still errors
        assert!(matches!(
            cat.get("gemini", "ghost-9"),
            Err(PricingError::UnknownModel { .. })
        ));
    }

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

    /// CORE-4: the four Flux tier aliases must be IN the bundled catalog under
    /// both provider keys Core reaches Flux through (`flux-router` and plain
    /// `openai`), priced $0 = "router-priced" (the [openrouter.auto]
    /// convention). This is what stops the W7 catalog-miss warning ("unknown
    /// model flux-auto for provider openai" — 365 occurrences in customer
    /// logs) from firing on every Flux turn.
    #[test]
    fn flux_tier_aliases_priced_in_bundled_catalog() {
        let cat = PricingCatalog::load_default().expect("bundled catalog parses");
        for provider in ["flux-router", "openai"] {
            for alias in ["flux-auto", "flux-fast", "flux-standard", "flux-reasoning"] {
                let p = cat.get(provider, alias).unwrap_or_else(|e| {
                    panic!("{provider}/{alias} must be in the bundled catalog: {e}")
                });
                assert_eq!(
                    p.input_per_mtok_usd, 0.0,
                    "{provider}/{alias} is router-priced: local rate must be $0"
                );
                assert_eq!(p.output_per_mtok_usd, 0.0);
                // And the cost path resolves (Ok(0)) instead of erroring into
                // the heuristic fallback.
                assert_eq!(
                    cat.estimate_cost_microcents(provider, alias, 1_000_000, 1_000_000)
                        .expect("tier alias must price"),
                    0
                );
            }
        }
    }

    #[test]
    fn flux_pinned_native_maps_vendor_to_catalog_provider() {
        // Vendor token → catalog provider; the FULL remainder is the native id.
        let n = flux_pinned_native("flux-router:flux-pinned-claude-opus-4-8").unwrap();
        assert_eq!(n.0, "anthropic");
        assert_eq!(n.1, "claude-opus-4-8");
        // gemini maps to `gemini` (NOT `google`).
        let g = flux_pinned_native("flux-pinned-gemini-3-1-pro").unwrap();
        assert_eq!(g, ("gemini".to_string(), "gemini-3-1-pro".to_string()));
        let d = flux_pinned_native("flux-pinned-deepseek-v4-pro").unwrap();
        assert_eq!(d, ("deepseek".to_string(), "deepseek-v4-pro".to_string()));
    }

    #[test]
    fn flux_pinned_vendor_extracts_token_even_for_unpriced_vendors() {
        assert_eq!(
            flux_pinned_vendor("flux-router:flux-pinned-claude-opus-4-8").as_deref(),
            Some("claude")
        );
        // Unpriced vendors (no catalog row) still yield a token — family grouping
        // needs this so a single-key Flux council groups by real vendor lineage.
        assert_eq!(
            flux_pinned_vendor("flux-pinned-glm-5-2").as_deref(),
            Some("glm")
        );
        assert_eq!(
            flux_pinned_vendor("flux-pinned-kimi-k2").as_deref(),
            Some("kimi")
        );
        assert_eq!(flux_pinned_vendor("openai:gpt-5"), None);
        assert_eq!(flux_pinned_vendor("flux-pinned-"), None);
    }

    #[test]
    fn flux_pinned_native_rejects_malformed() {
        assert!(flux_pinned_native("openai:gpt-5").is_none()); // not flux-pinned
        assert!(flux_pinned_native("flux-pinned-").is_none()); // empty remainder
        assert!(flux_pinned_native("flux-pinned-unknownvendor-x").is_none()); // unknown vendor
        assert!(flux_pinned_native("").is_none());
    }

    #[test]
    fn flux_pinned_resolves_to_native_catalog_price() {
        let cat = PricingCatalog::load_default().unwrap();
        // A flux-pinned model whose native SKU IS in the catalog prices > 0.
        let gpt = cat.estimate_cost_microcents_resolved(
            "flux-router",
            "flux-pinned-gpt-5",
            1000,
            1000,
            1.0,
        );
        assert!(
            gpt.is_some() && gpt.unwrap() > 0,
            "gpt-5 native row must price"
        );
        let ds = cat.estimate_cost_microcents_resolved(
            "flux-router",
            "flux-pinned-deepseek-v4-pro",
            1000,
            1000,
            1.0,
        );
        assert!(
            ds.is_some() && ds.unwrap() > 0,
            "deepseek-v4-pro native row must price"
        );

        // EXACT-MATCH-OR-NONE: a flux-pinned model with NO native catalog row
        // stays unpriced (None) — never a guessed "nearest" price, never a silent
        // $0. claude-opus-4-8 is absent (catalog has -4-7/-4-6 only).
        assert!(
            cat.estimate_cost_microcents_resolved(
                "flux-router",
                "flux-pinned-claude-opus-4-8",
                1000,
                1000,
                1.0
            )
            .is_none(),
            "absent native SKU must be None, not nearest-matched"
        );
        // glm has no catalog rows at all.
        assert!(
            cat.estimate_cost_microcents_resolved(
                "flux-router",
                "flux-pinned-glm-5-2",
                1000,
                1000,
                1.0
            )
            .is_none()
        );
    }

    #[test]
    fn every_flux_vendor_maps_to_a_real_catalog_provider() {
        // Guard against vendor→provider drift: every mapped target must be a real
        // section in the bundled catalog, else flux-pinned models for that vendor
        // silently stay unpriced.
        let cat = PricingCatalog::load_default().unwrap();
        for (vendor, provider) in FLUX_VENDOR_TO_CATALOG_PROVIDER {
            assert!(
                cat.providers.contains_key(*provider),
                "flux vendor '{vendor}' maps to '{provider}', which has no section in pricing.toml"
            );
        }
    }

    #[test]
    fn invalid_markup_is_unpriceable_never_zero() {
        // A nonsense markup must yield None (unpriceable), never a fabricated $0
        // that a budget guard would wave through.
        let cat = PricingCatalog::load_default().unwrap();
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(
                cat.estimate_cost_microcents_resolved(
                    "flux-router",
                    "flux-pinned-gpt-5",
                    1000,
                    1000,
                    bad
                )
                .is_none(),
                "markup {bad} must be unpriceable, not $0"
            );
        }
        // markup == 0.0 is a deliberate "free" choice → priceable at 0.
        assert_eq!(
            cat.estimate_cost_microcents_resolved(
                "flux-router",
                "flux-pinned-gpt-5",
                1000,
                1000,
                0.0
            ),
            Some(0)
        );
    }

    #[test]
    fn flux_markup_scales_native_price_and_non_flux_unchanged() {
        let cat = PricingCatalog::load_default().unwrap();
        // markup multiplies the native cost exactly.
        let base = cat
            .estimate_cost_microcents_resolved("flux-router", "flux-pinned-gpt-5", 1000, 1000, 1.0)
            .unwrap();
        let marked = cat
            .estimate_cost_microcents_resolved("flux-router", "flux-pinned-gpt-5", 1000, 1000, 2.0)
            .unwrap();
        assert_eq!(marked, base * 2, "markup 2.0 doubles the native cost");

        // A literal (non-flux) provider key prices identically to the plain API,
        // proving the resolved path is additive (markup default leaves it alone).
        let literal = cat
            .estimate_cost_microcents("openai", "gpt-5", 1000, 1000)
            .unwrap();
        let resolved = cat
            .estimate_cost_microcents_resolved("openai", "gpt-5", 1000, 1000, 1.0)
            .unwrap();
        assert_eq!(literal, resolved);
    }
}
