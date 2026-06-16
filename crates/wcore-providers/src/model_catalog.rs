//! Per-provider disk cache for live model lists.
//!
//! Mirrors the pricing-layer cache pattern (`wcore-pricing::refresh`): each
//! provider's live `/v1/models` (or equivalent) result is snapshotted to
//! `~/.wayland/cache/models/{provider}.json` with a `fetched_at` timestamp and
//! a 24h TTL. A live model fetch consults this cache first; a fresh snapshot is
//! served without re-hitting the provider, and the file is rewritten after every
//! successful live fetch.
//!
//! This module is *only* the storage layer — it never performs HTTP. The
//! discovery service (Phase 3) wires it to the providers' `list_models`. The
//! engine's hard invariant that `list_models` never errors is upheld by the
//! callers: every fallible op here returns `Option`/`io::Result` so a corrupt
//! or missing cache degrades to "no cache" rather than propagating an error.
//!
//! Rollback flag: `WAYLAND_MODEL_DISCOVERY=off` disables live discovery; check
//! [`discovery_enabled`] before invoking a live fetch path.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use wcore_config::config::{Config, ProviderType, connected_providers, provider_type_slug};

use crate::{LlmProvider, ModelInfo, alias_models, create_native_provider};

/// Default cache lifetime: model lists change rarely, so a 24h TTL keeps the
/// `/model` picker snappy without serving stale catalogs for long.
pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Per-provider wall-clock cap for a single live model fetch during a warm.
/// Model-list fetches are quick (<1s) in the normal case; this only bites a
/// provider whose endpoint hangs (e.g. a blocked egress route), so it is
/// abandoned rather than stalling the concurrent warm for the others.
const PROVIDER_REFRESH_TIMEOUT: Duration = Duration::from_secs(8);

/// Rollback env var. When set to `off` (case-insensitive), live model
/// discovery is disabled and callers should fall back to the static alias
/// catalog without touching the network or this cache.
const DISCOVERY_ENV: &str = "WAYLAND_MODEL_DISCOVERY";

/// Where a cached model list came from: a live provider `/v1/models` fetch, or
/// the static built-in alias catalog (for providers with no live endpoint, or
/// as the floor when a live fetch yields nothing). Drives the picker's
/// "synced Nh ago" vs "built-in" heading label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// Fetched live from the provider's model-list endpoint.
    #[default]
    Live,
    /// The static built-in alias catalog (no live endpoint / live floor).
    BuiltIn,
}

/// On-disk snapshot of a provider's model list with the time it was fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedModels {
    pub fetched_at: DateTime<Utc>,
    /// Provenance of `models` — defaults to `Live` for back-compat with cache
    /// files written before this field existed.
    #[serde(default)]
    pub source: ModelSource,
    pub models: Vec<ModelInfo>,
}

/// Whether live model discovery is enabled. Returns `false` only when
/// `WAYLAND_MODEL_DISCOVERY` is set to `off` (case-insensitive); the default
/// (unset, or any other value) is enabled.
pub fn discovery_enabled() -> bool {
    match std::env::var(DISCOVERY_ENV) {
        Ok(v) => !v.trim().eq_ignore_ascii_case("off"),
        Err(_) => true,
    }
}

/// Resolve the cache file for `provider`:
/// `${WAYLAND_HOME|~/.wayland|./.wayland}/cache/models/{provider}.json`.
///
/// The provider segment is sanitized (path separators and NULs rewritten to
/// `_`, same rule as `OAuthStorage::path_for`) so a hostile provider name can't
/// escape the cache directory.
pub fn cache_path(provider: &str) -> PathBuf {
    let home = std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .unwrap_or_else(|| PathBuf::from("./.wayland"));
    let safe = provider.replace(['/', '\\', '\0'], "_");
    home.join("cache")
        .join("models")
        .join(format!("{safe}.json"))
}

/// Load `provider`'s cached snapshot (models + metadata) if present and within
/// `ttl`. `None` for a missing, stale, or corrupt cache — never an error.
pub fn load_cached_meta(provider: &str, ttl: Duration) -> Option<CachedModels> {
    let path = cache_path(provider);
    if !path.exists() {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    let cached: CachedModels = serde_json::from_str(&raw).ok()?;
    let age = Utc::now().signed_duration_since(cached.fetched_at);
    if age.num_seconds().unsigned_abs() > ttl.as_secs() {
        return None;
    }
    Some(cached)
}

/// Load just the model list for `provider` (see [`load_cached_meta`]).
pub fn load_cached(provider: &str, ttl: Duration) -> Option<Vec<ModelInfo>> {
    load_cached_meta(provider, ttl).map(|c| c.models)
}

/// Source + age-in-seconds of `provider`'s fresh, non-empty snapshot, for the
/// picker's section-heading label. `None` when there is no fresh snapshot (the
/// picker then shows the static alias list and labels it "built-in").
pub fn cache_freshness(provider: &str, ttl: Duration) -> Option<(ModelSource, i64)> {
    let meta = load_cached_meta(provider, ttl)?;
    if meta.models.is_empty() {
        return None;
    }
    let age = Utc::now()
        .signed_duration_since(meta.fetched_at)
        .num_seconds()
        .max(0);
    Some((meta.source, age))
}

/// Snapshot `models` for `provider` to disk, stamped with the current time and
/// the given source. Creates the cache directory tree if needed.
pub fn save_with_source(
    provider: &str,
    models: &[ModelInfo],
    source: ModelSource,
) -> std::io::Result<()> {
    let path = cache_path(provider);
    let cached = CachedModels {
        fetched_at: Utc::now(),
        source,
        models: models.to_vec(),
    };
    let json = serde_json::to_string_pretty(&cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, json)
}

/// Snapshot a live-fetched `models` list (source = [`ModelSource::Live`]).
pub fn save(provider: &str, models: &[ModelInfo]) -> std::io::Result<()> {
    save_with_source(provider, models, ModelSource::Live)
}

/// Refresh the on-disk model cache for every connected provider, fetching a
/// live model list where possible and falling back to the static alias catalog
/// otherwise.
///
/// For each provider reported by [`connected_providers`]
/// (`wcore-config` — the single credential source of truth):
///
/// - **Disabled**: when `WAYLAND_MODEL_DISCOVERY=off` ([`discovery_enabled`] is
///   false) the whole refresh is a no-op; the static alias catalog is served as
///   the live floor by each provider's `list_models`, so there's nothing to
///   pre-warm.
/// - **Fresh cache**: providers whose cache is present and within
///   [`DEFAULT_TTL`] are skipped — no network, no rewrite.
/// - **ChatGPT** (`openai-chatgpt`): the Codex backend has no `/models`
///   endpoint and `create_native_provider` panics for it (it is constructed in
///   `bootstrap` with an OAuth bearer source), so we snapshot the static alias
///   catalog directly and skip the live path.
/// - **Everything else**: a per-provider discovery [`Config`] is derived from
///   `base` ([`Config::for_provider_discovery`]), a native provider is built,
///   and its `list_models` result is snapshotted. `list_models` upholds the
///   engine invariant (never `Err` — it floors to the alias catalog on any
///   HTTP/parse/auth failure), so the worst case still writes a usable cache.
///
/// Best-effort throughout: a cache-write error for one provider is logged-by-
/// omission (the stale/alias entry stays) and never aborts the others. This is
/// fire-and-forget warm-up — callers do not await per-provider success.
pub async fn refresh_connected(base: &Config) {
    if !discovery_enabled() {
        return;
    }
    // Concurrent + individually time-bounded. A sequential loop stalls entirely
    // if the first provider's `list_models` hangs (e.g. a blocked egress route
    // that drops packets without a RST) — so a single slow provider would
    // starve the warm for everyone after it. Run each provider concurrently and
    // cap each at `PROVIDER_REFRESH_TIMEOUT`; a hung one is abandoned without
    // blocking the rest. Each writes a distinct cache file, so there is no
    // shared mutable state across the concurrent refreshes.
    let tasks = connected_providers()
        .into_iter()
        .map(|provider| async move {
            let _ = tokio::time::timeout(
                PROVIDER_REFRESH_TIMEOUT,
                refresh_one(base, provider, create_native_provider),
            )
            .await;
        });
    futures::future::join_all(tasks).await;
}

/// Best-effort delete of `provider`'s cache file (ignores a missing file).
pub fn clear_cache(provider: &str) {
    let _ = std::fs::remove_file(cache_path(provider));
}

/// Force a full re-fetch of every connected provider's model list, bypassing
/// the staleness skip: each connected provider's cache is cleared, then
/// `refresh_one` re-fetches it. Drives the `/model refresh` command.
pub async fn refresh_connected_force(base: &Config) {
    if !discovery_enabled() {
        return;
    }
    // Concurrent + time-bounded, same rationale as `refresh_connected` (one
    // hung provider must not stall the rest). Clear each cache first so the
    // staleness skip in `refresh_one` can't short-circuit the re-fetch.
    let tasks = connected_providers()
        .into_iter()
        .map(|provider| async move {
            clear_cache(provider_type_slug(provider));
            let _ = tokio::time::timeout(
                PROVIDER_REFRESH_TIMEOUT,
                refresh_one(base, provider, create_native_provider),
            )
            .await;
        });
    futures::future::join_all(tasks).await;
}

/// Refresh a single `provider`'s cache, building the live provider via `build`.
///
/// Split out (and generic over `build`) so tests can inject a fake provider
/// without a network call or the `create_native_provider` panic for
/// ChatGPT. The ChatGPT special-case and the staleness skip live here so both
/// the production path and tests share them.
async fn refresh_one<F>(base: &Config, provider: ProviderType, build: F)
where
    F: FnOnce(&Config) -> Arc<dyn LlmProvider>,
{
    let slug = provider_type_slug(provider);

    // Only refresh a stale/missing cache — a fresh snapshot is served as-is.
    if load_cached(slug, DEFAULT_TTL).is_some() {
        return;
    }

    // ChatGPT Codex has no live model endpoint and cannot be built via
    // `create_native_provider` (it panics — constructed in bootstrap). Snapshot
    // the static alias catalog so the picker still has a warm cache entry.
    if provider == ProviderType::OpenAIChatGpt {
        let _ = save_with_source(slug, &alias_models(slug), ModelSource::BuiltIn);
        return;
    }

    let cfg = base.for_provider_discovery(provider);
    let live = build(&cfg);
    // `list_models` never errors today (the invariant floors to the alias
    // catalog). The `Err` path is still handled defensively so a future
    // fallible override degrades to "keep the stale/alias cache" rather than
    // panicking; an `Err` simply leaves whatever cache already exists in place.
    if let Ok(models) = live.list_models().await {
        // `list_models` returns `Ok` even when it floored to the alias catalog
        // (HTTP/auth/parse failure), so a bare `Live` tag would mislabel the
        // built-in list as a "synced" live snapshot. The floor returns exactly
        // `alias_models(alias_key)` (== `alias_models(slug)`), so an equal
        // result means the live fetch did not actually succeed → tag BuiltIn.
        let source = if models == alias_models(slug) {
            ModelSource::BuiltIn
        } else {
            ModelSource::Live
        };
        let _ = save_with_source(slug, &models, source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Point WAYLAND_HOME at a fresh tempdir for the duration of the returned
    /// guard. The guard keeps the dir alive and restores the prior env on drop.
    struct HomeGuard {
        _tmp: TempDir,
        prior: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let prior = std::env::var_os("WAYLAND_HOME");
            // SAFETY: tests are serialized via #[serial]; no other thread reads
            // the env concurrently.
            unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
            Self { _tmp: tmp, prior }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: serialized; restore the prior value (or clear it).
            unsafe {
                match &self.prior {
                    Some(v) => std::env::set_var("WAYLAND_HOME", v),
                    None => std::env::remove_var("WAYLAND_HOME"),
                }
            }
        }
    }

    fn sample_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: "gpt-5".into(),
                display: "GPT-5".into(),
            },
            ModelInfo {
                id: "gpt-5-mini".into(),
                display: "GPT-5 Mini".into(),
            },
        ]
    }

    #[test]
    #[serial]
    fn save_then_load_round_trips() {
        let _guard = HomeGuard::new();
        let models = sample_models();
        save("openai", &models).unwrap();
        let loaded = load_cached("openai", DEFAULT_TTL).expect("fresh cache present");
        assert_eq!(loaded, models);
    }

    #[test]
    #[serial]
    fn save_with_source_tags_built_in() {
        let _guard = HomeGuard::new();
        save_with_source("vertex", &sample_models(), ModelSource::BuiltIn).unwrap();
        let meta = load_cached_meta("vertex", DEFAULT_TTL).expect("fresh cache present");
        assert_eq!(meta.source, ModelSource::BuiltIn);
    }

    #[test]
    #[serial]
    fn load_returns_none_when_stale() {
        let _guard = HomeGuard::new();
        save("openai", &sample_models()).unwrap();
        // A zero TTL makes any non-zero age stale; rewrite fetched_at into the
        // past to be unambiguous even when the write completes in <1s.
        let path = cache_path("openai");
        let raw = std::fs::read_to_string(&path).unwrap();
        let mut cached: CachedModels = serde_json::from_str(&raw).unwrap();
        cached.fetched_at = Utc::now() - chrono::Duration::hours(48);
        std::fs::write(&path, serde_json::to_string_pretty(&cached).unwrap()).unwrap();
        assert!(load_cached("openai", DEFAULT_TTL).is_none());
    }

    #[test]
    #[serial]
    fn load_returns_none_when_missing() {
        let _guard = HomeGuard::new();
        assert!(load_cached("never-saved", DEFAULT_TTL).is_none());
    }

    #[test]
    #[serial]
    fn load_returns_none_when_corrupt() {
        let _guard = HomeGuard::new();
        let path = cache_path("openai");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not valid json ][").unwrap();
        assert!(load_cached("openai", DEFAULT_TTL).is_none());
    }

    #[test]
    #[serial]
    fn cache_path_sanitizes_traversal() {
        let _guard = HomeGuard::new();
        let p = cache_path("../../etc/passwd");
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(
            !name.contains('/') && !name.contains('\\'),
            "path traversal must be neutralized: {name}"
        );
        assert!(p.ends_with("cache/models/.._.._etc_passwd.json"));
    }

    #[test]
    #[serial]
    fn discovery_enabled_respects_off_flag() {
        // SAFETY: serialized test; restore handled below.
        let prior = std::env::var_os(DISCOVERY_ENV);
        unsafe { std::env::set_var(DISCOVERY_ENV, "off") };
        assert!(!discovery_enabled());
        unsafe { std::env::set_var(DISCOVERY_ENV, "OFF") };
        assert!(!discovery_enabled());
        unsafe { std::env::set_var(DISCOVERY_ENV, "on") };
        assert!(discovery_enabled());
        unsafe { std::env::remove_var(DISCOVERY_ENV) };
        assert!(discovery_enabled());
        unsafe {
            match prior {
                Some(v) => std::env::set_var(DISCOVERY_ENV, v),
                None => std::env::remove_var(DISCOVERY_ENV),
            }
        }
    }

    // -------------------------------------------------------------------------
    // refresh_one() — per-provider live-discovery write path (fake-provider seam)
    // -------------------------------------------------------------------------

    use async_trait::async_trait;
    use tokio::sync::mpsc;
    use wcore_types::llm::{LlmEvent, LlmRequest};

    use crate::{LlmProvider, ProviderError};

    /// A hermetic `LlmProvider` whose `list_models` returns a fixed list and
    /// whose `stream` is never exercised — lets the refresh service be tested
    /// without a network call or the real `create_native_provider`.
    struct FakeProvider {
        models: Vec<ModelInfo>,
    }

    #[async_trait]
    impl LlmProvider for FakeProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
            unreachable!("refresh_one only calls list_models")
        }
        async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
            Ok(self.models.clone())
        }
    }

    #[tokio::test]
    #[serial]
    async fn refresh_one_writes_live_models_to_cache() {
        let _guard = HomeGuard::new();
        let base = Config::default();
        let live = vec![ModelInfo {
            id: "gpt-5-live".into(),
            display: "GPT-5 (live)".into(),
        }];
        let live_for_closure = live.clone();
        refresh_one(&base, ProviderType::OpenAI, move |_cfg| {
            Arc::new(FakeProvider {
                models: live_for_closure.clone(),
            })
        })
        .await;
        let meta =
            load_cached_meta("openai", DEFAULT_TTL).expect("refresh must write a cache entry");
        assert_eq!(
            meta.models, live,
            "the live model list must be snapshotted verbatim"
        );
        assert_eq!(
            meta.source,
            ModelSource::Live,
            "a result distinct from the alias catalog is a real live fetch"
        );
    }

    #[tokio::test]
    #[serial]
    async fn refresh_one_tags_floored_alias_as_built_in() {
        let _guard = HomeGuard::new();
        // `list_models` floors to the alias catalog on failure but still returns
        // Ok — refresh_one must recognise that (result == alias catalog) and tag
        // the snapshot BuiltIn, not mislabel it as a live "synced" fetch.
        let aliases = alias_models("openai");
        let aliases_for_closure = aliases.clone();
        refresh_one(&Config::default(), ProviderType::OpenAI, move |_cfg| {
            Arc::new(FakeProvider {
                models: aliases_for_closure.clone(),
            })
        })
        .await;
        let meta = load_cached_meta("openai", DEFAULT_TTL).expect("cache written");
        assert_eq!(meta.models, aliases);
        assert_eq!(
            meta.source,
            ModelSource::BuiltIn,
            "a result equal to the alias catalog is a floored fetch → BuiltIn"
        );
    }

    #[tokio::test]
    #[serial]
    async fn refresh_one_skips_fresh_cache() {
        let _guard = HomeGuard::new();
        // Pre-seed a fresh cache; refresh must not overwrite it.
        let seeded = vec![ModelInfo {
            id: "seeded".into(),
            display: "Seeded".into(),
        }];
        save("openai", &seeded).unwrap();
        refresh_one(&Config::default(), ProviderType::OpenAI, |_cfg| {
            // If this ran, it would write a DIFFERENT list — the assertion below
            // would catch the unwanted overwrite.
            Arc::new(FakeProvider {
                models: vec![ModelInfo {
                    id: "overwritten".into(),
                    display: "Overwritten".into(),
                }],
            })
        })
        .await;
        let cached = load_cached("openai", DEFAULT_TTL).unwrap();
        assert_eq!(cached, seeded, "a fresh cache must not be refreshed");
    }

    #[tokio::test]
    #[serial]
    async fn refresh_one_chatgpt_writes_alias_catalog_without_building() {
        let _guard = HomeGuard::new();
        // The closure must NEVER run for ChatGPT — create_native_provider
        // panics for it, so refresh_one snapshots the alias catalog directly.
        refresh_one(&Config::default(), ProviderType::OpenAIChatGpt, |_cfg| {
            panic!("ChatGPT must not be built via the live path");
        })
        .await;
        let cached = load_cached("openai-chatgpt", DEFAULT_TTL)
            .expect("ChatGPT alias catalog must be cached");
        assert_eq!(
            cached,
            alias_models("openai-chatgpt"),
            "ChatGPT cache must be the static alias catalog"
        );
        assert!(!cached.is_empty(), "alias catalog is non-empty for ChatGPT");
    }
}
