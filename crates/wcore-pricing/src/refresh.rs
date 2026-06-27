//! Self-healing pricing layer — ported from openclaw MIT (c) Peter Steinberger 2025.
//!
//! Fetches the OpenRouter /api/v1/models catalog with a 24h TTL and diffs the
//! live pricing against the bundled catalog. Diffs emit CatalogChange events
//! for humans to inspect — auto-application is OFF by default
//! (decision locked in BATTLE-PLAN-v2 §Pre-flight).
//!
//! Offline tests use wiremock to fake the OpenRouter endpoint. The single live
//! test is #[ignore]'d so CI doesn't depend on network availability.
//!
//! Rollback flag: WAYLAND_PRICING_AUTO_REFRESH=off keeps the bundled catalog
//! static (no live fetch). Callers are responsible for checking this env var
//! before invoking PricingRefresher::fetch_live.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

use wcore_egress::{EgressClient, EgressError};

use crate::{ModelPrice, PricingCatalog};

const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const DEFAULT_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Error)]
pub enum RefreshError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("egress error: {0}")]
    Egress(#[from] EgressError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// OpenRouter model entry (subset we care about — provider returns more fields we ignore).
#[derive(Debug, Clone, Deserialize)]
struct OpenRouterModel {
    id: String,
    pricing: Option<OpenRouterPricing>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenRouterPricing {
    /// USD per token, encoded as a string in OpenRouter's response.
    prompt: Option<String>,
    completion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenRouterResponse {
    data: Vec<OpenRouterModel>,
}

/// A single observed pricing delta between bundled and live catalogs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogChange {
    pub provider: String,
    pub model: String,
    pub bundled: Option<ModelPrice>,
    pub live: Option<ModelPrice>,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Live catalog has a model the bundled doesn't.
    Added,
    /// Bundled has a model the live doesn't.
    Removed,
    /// Both have the model but prices differ.
    PriceChanged,
}

/// On-disk cached snapshot of the live catalog with timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCatalog {
    pub fetched_at: DateTime<Utc>,
    pub catalog: PricingCatalog,
}

/// HTTP fetcher with a configurable base URL (for testability).
pub struct PricingRefresher {
    base_url: String,
    client: EgressClient,
    ttl: Duration,
}

impl Default for PricingRefresher {
    fn default() -> Self {
        Self {
            base_url: OPENROUTER_MODELS_URL.to_string(),
            client: EgressClient::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .expect("egress client should build"),
            ttl: Duration::from_secs(DEFAULT_TTL_SECONDS),
        }
    }
}

impl PricingRefresher {
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            ..Default::default()
        }
    }

    /// Fetch the live OpenRouter catalog.
    pub async fn fetch_live(&self) -> Result<PricingCatalog, RefreshError> {
        let raw: OpenRouterResponse = self.client.get(&self.base_url).send().await?.json().await?;
        Ok(openrouter_to_catalog(raw))
    }

    /// Load a cached snapshot if it exists and is within TTL.
    pub fn load_cached(
        &self,
        path: &std::path::Path,
    ) -> Result<Option<PricingCatalog>, RefreshError> {
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path)?;
        let cached: CachedCatalog = serde_json::from_str(&raw)?;
        let age = Utc::now().signed_duration_since(cached.fetched_at);
        if age.num_seconds().unsigned_abs() > self.ttl.as_secs() {
            return Ok(None);
        }
        Ok(Some(cached.catalog))
    }

    /// Save a catalog snapshot to disk with current timestamp.
    pub fn save_snapshot(
        &self,
        path: &std::path::Path,
        catalog: &PricingCatalog,
    ) -> Result<(), RefreshError> {
        let cached = CachedCatalog {
            fetched_at: Utc::now(),
            catalog: catalog.clone(),
        };
        let s = serde_json::to_string_pretty(&cached)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, s)?;
        Ok(())
    }

    /// Compute pricing deltas between two catalogs.
    pub fn diff(&self, bundled: &PricingCatalog, live: &PricingCatalog) -> Vec<CatalogChange> {
        let mut changes = Vec::new();
        // Walk bundled — find Removed + PriceChanged
        for (prov_id, bundled_models) in &bundled.providers {
            let live_models = live.providers.get(prov_id);
            for (model_id, bundled_price) in bundled_models {
                let live_price = live_models.and_then(|m| m.get(model_id));
                match live_price {
                    None => changes.push(CatalogChange {
                        provider: prov_id.clone(),
                        model: model_id.clone(),
                        bundled: Some(bundled_price.clone()),
                        live: None,
                        kind: ChangeKind::Removed,
                    }),
                    Some(lp) if !prices_equal(bundled_price, lp) => changes.push(CatalogChange {
                        provider: prov_id.clone(),
                        model: model_id.clone(),
                        bundled: Some(bundled_price.clone()),
                        live: Some(lp.clone()),
                        kind: ChangeKind::PriceChanged,
                    }),
                    _ => {}
                }
            }
        }
        // Walk live — find Added
        for (prov_id, live_models) in &live.providers {
            let bundled_models = bundled.providers.get(prov_id);
            for (model_id, live_price) in live_models {
                if bundled_models.and_then(|m| m.get(model_id)).is_none() {
                    changes.push(CatalogChange {
                        provider: prov_id.clone(),
                        model: model_id.clone(),
                        bundled: None,
                        live: Some(live_price.clone()),
                        kind: ChangeKind::Added,
                    });
                }
            }
        }
        changes
    }
}

fn prices_equal(a: &ModelPrice, b: &ModelPrice) -> bool {
    (a.input_per_mtok_usd - b.input_per_mtok_usd).abs() < 1e-9
        && (a.output_per_mtok_usd - b.output_per_mtok_usd).abs() < 1e-9
}

fn openrouter_to_catalog(raw: OpenRouterResponse) -> PricingCatalog {
    let mut providers: HashMap<String, HashMap<String, ModelPrice>> = HashMap::new();
    for m in raw.data {
        let pricing = match m.pricing {
            Some(p) => p,
            None => continue,
        };
        // Parse prompt+completion as USD/token. A missing, non-numeric, or
        // non-positive price means UNPRICED — never $0 (a $0 row would be
        // seated as the global-cheapest and then billed at the real rate).
        // Drop the row; certification later refuses an unpriced roster.
        let parse_pos = |s: &Option<String>| -> Option<f64> {
            s.as_deref()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        let (Some(prompt), Some(completion)) =
            (parse_pos(&pricing.prompt), parse_pos(&pricing.completion))
        else {
            continue;
        };
        // OpenRouter publishes prompt/completion in USD per TOKEN — convert to per million.
        let input_per_mtok_usd = prompt * 1_000_000.0;
        let output_per_mtok_usd = completion * 1_000_000.0;
        let model_price = ModelPrice {
            input_per_mtok_usd,
            output_per_mtok_usd,
            cache_read_per_mtok_usd: None,
            cache_write_per_mtok_usd: None,
        };
        // OpenRouter model ids are like "anthropic/claude-opus-4-7" — split on /
        if let Some((prov, model_id)) = m.id.split_once('/') {
            providers
                .entry(prov.to_string())
                .or_default()
                .insert(model_id.to_string(), model_price);
        } else {
            // Single-segment id — file under "openrouter"
            providers
                .entry("openrouter".to_string())
                .or_default()
                .insert(m.id, model_price);
        }
    }
    PricingCatalog { providers }
}

/// Suggested on-disk cache path (~/.wayland/pricing-cache.json).
pub fn default_cache_path() -> PathBuf {
    let home = std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .unwrap_or_else(|| PathBuf::from("./.wayland"));
    home.join("pricing-cache.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn fixture_openrouter_response() -> serde_json::Value {
        serde_json::json!({
            "data": [
                {
                    "id": "anthropic/claude-opus-4-7",
                    "pricing": { "prompt": "0.000015", "completion": "0.000075" }
                },
                {
                    "id": "openai/gpt-5",
                    "pricing": { "prompt": "0.000005", "completion": "0.000015" }
                },
                {
                    "id": "deepseek/deepseek-v3",
                    "pricing": { "prompt": "0.00000027", "completion": "0.0000011" }
                }
            ]
        })
    }

    #[tokio::test]
    async fn fetch_live_via_mock_returns_catalog() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(fixture_openrouter_response()))
            .mount(&server)
            .await;

        let url = format!("{}/api/v1/models", server.uri());
        let refresher = PricingRefresher::with_base_url(url);
        let cat = refresher.fetch_live().await.unwrap();

        assert!(cat.providers.contains_key("anthropic"));
        assert!(cat.providers.contains_key("openai"));
        let opus = cat.get("anthropic", "claude-opus-4-7").unwrap();
        assert!((opus.input_per_mtok_usd - 15.0).abs() < 1e-6);
        assert!((opus.output_per_mtok_usd - 75.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn diff_detects_price_change() {
        let bundled: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
"#,
        )
        .unwrap();
        let live: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 12.0
output_per_mtok_usd = 60.0
"#,
        )
        .unwrap();
        let refresher = PricingRefresher::default();
        let changes = refresher.diff(&bundled, &live);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::PriceChanged);
        assert_eq!(changes[0].provider, "anthropic");
    }

    #[tokio::test]
    async fn diff_detects_added_model() {
        let bundled: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
"#,
        )
        .unwrap();
        let live: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0

[anthropic.claude-haiku-5-0]
input_per_mtok_usd = 0.5
output_per_mtok_usd = 2.5
"#,
        )
        .unwrap();
        let refresher = PricingRefresher::default();
        let changes = refresher.diff(&bundled, &live);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Added);
        assert_eq!(changes[0].model, "claude-haiku-5-0");
    }

    #[tokio::test]
    async fn diff_detects_removed_model() {
        let bundled: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0

[anthropic.claude-haiku-4-5]
input_per_mtok_usd = 1.0
output_per_mtok_usd = 5.0
"#,
        )
        .unwrap();
        let live: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
"#,
        )
        .unwrap();
        let refresher = PricingRefresher::default();
        let changes = refresher.diff(&bundled, &live);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(changes[0].model, "claude-haiku-4-5");
    }

    #[tokio::test]
    async fn diff_empty_when_catalogs_match() {
        let bundled: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
"#,
        )
        .unwrap();
        let refresher = PricingRefresher::default();
        let changes = refresher.diff(&bundled, &bundled);
        assert_eq!(changes.len(), 0);
    }

    #[tokio::test]
    async fn save_load_snapshot_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test-pricing-cache.json");
        let cat: PricingCatalog = toml::from_str(
            r#"
[anthropic.claude-opus-4-7]
input_per_mtok_usd = 15.0
output_per_mtok_usd = 75.0
"#,
        )
        .unwrap();
        let refresher = PricingRefresher::default();
        refresher.save_snapshot(&path, &cat).unwrap();
        let loaded = refresher
            .load_cached(&path)
            .unwrap()
            .expect("should load fresh snapshot");
        assert_eq!(loaded.providers.len(), 1);
    }

    #[tokio::test]
    async fn load_cached_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.json");
        let refresher = PricingRefresher::default();
        assert!(refresher.load_cached(&path).unwrap().is_none());
    }

    #[test]
    fn openrouter_id_split_on_slash() {
        let raw = OpenRouterResponse {
            data: vec![OpenRouterModel {
                id: "anthropic/claude-opus-4-7".into(),
                pricing: Some(OpenRouterPricing {
                    prompt: Some("0.000015".into()),
                    completion: Some("0.000075".into()),
                }),
            }],
        };
        let cat = openrouter_to_catalog(raw);
        assert!(cat.providers.contains_key("anthropic"));
        assert!(
            cat.providers
                .get("anthropic")
                .unwrap()
                .contains_key("claude-opus-4-7")
        );
    }

    #[test]
    fn unpriced_or_garbage_rows_are_excluded_not_zeroed() {
        let raw: OpenRouterResponse = serde_json::from_value(serde_json::json!({
            "data": [
                { "id": "openai/gpt-5", "pricing": { "prompt": "0.0000011", "completion": "0.0000044" } },
                { "id": "vendor/nullpriced", "pricing": { "prompt": null, "completion": "0.0000044" } },
                { "id": "vendor/dashpriced", "pricing": { "prompt": "-1", "completion": "0.0000044" } },
                { "id": "vendor/textpriced", "pricing": { "prompt": "auto", "completion": "0.0000044" } },
            ]
        })).unwrap();
        let cat = openrouter_to_catalog(raw);
        assert!(
            cat.providers
                .get("openai")
                .and_then(|m| m.get("gpt-5"))
                .is_some()
        );
        for models in cat.providers.values() {
            for price in models.values() {
                assert!(
                    price.input_per_mtok_usd > 0.0,
                    "an unpriced row leaked in as $0"
                );
                assert!(
                    price.output_per_mtok_usd > 0.0,
                    "an unpriced row leaked in as $0"
                );
            }
        }
    }

    #[test]
    fn change_kind_serde() {
        assert_eq!(
            serde_json::to_string(&ChangeKind::Added).unwrap(),
            "\"added\""
        );
        assert_eq!(
            serde_json::to_string(&ChangeKind::PriceChanged).unwrap(),
            "\"price_changed\""
        );
    }
}
