// `Memory` — top-level convenience facade that bundles a Db pool, an
// access gate, an embedder, a CDC writer, a tier resolver, and a
// PartitionDispatcher.
//
// Most consumers will use Memory::open / Memory::open_in_memory + the
// MemoryApi trait it implements; the underlying components stay public
// for advanced wiring (custom gates, alternative CDC sinks, etc.).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use wcore_config::config::{EmbedderBackend, EmbedderConfig};

use crate::api::MemoryApi;
use crate::audit::AuditLog;
use crate::cdc::CdcWriter;
use crate::db::Db;
use crate::embed::{Embedder, HashedEmbedder, OpenAiEmbedder, VoyageEmbedder};
use crate::error::{MemoryError, Result};
use crate::gate::{AccessPolicy, MemoryAccessGate};
use crate::legacy_import::{self, LegacyImportReport};
use crate::partition::PartitionDispatcher;
use crate::paths;
use crate::v2_types::{
    AccessToken, CompactReport, DreamReport, Episode, EpisodeId, Fact, FactId, Hit, Procedure,
    ProcedureId, Query, Tier, UserModel,
};

/// Resolve the configured embedder backend into a concrete `Arc<dyn Embedder>`.
///
/// Selection rules (in order of the [`EmbedderBackend`] variant):
/// * `OpenAi` / `Voyage` — read the API key from the env var named in
///   `cfg.api_key_env` (defaulting to the provider's conventional var) and
///   build the cloud backend. A missing/empty key is a hard error rather than
///   a silent downgrade — the user explicitly asked for a cloud embedder.
/// * `LocalBge` — build the real candle-backed BERT embedder, but ONLY when
///   the `bge-local` feature is compiled in. Without the feature the local
///   backend is the same keyword-hash stub as `Hashed`, so we surface the
///   degraded mode and fall through to `HashedEmbedder`.
/// * `Hashed` — the deterministic keyword-hash bag. Functional but NOT real
///   semantic similarity; we emit a one-time warning so degraded retrieval is
///   never silent.
///
/// Extension point: new cloud/local backends slot into the `match` below and
/// reuse the same env-var resolution + degraded-mode warning machinery.
async fn build_embedder(cfg: &EmbedderConfig) -> Result<Arc<dyn Embedder>> {
    fn resolve_key(cfg: &EmbedderConfig, default_env: &str) -> Result<String> {
        let var = cfg.api_key_env.as_deref().unwrap_or(default_env);
        match std::env::var(var) {
            Ok(k) if !k.is_empty() => Ok(k),
            _ => Err(MemoryError::Embedding(format!(
                "embedder backend requires an API key, but env var {var:?} is unset or empty"
            ))),
        }
    }

    match cfg.backend {
        EmbedderBackend::OpenAi => {
            let key = resolve_key(cfg, "OPENAI_API_KEY")?;
            Ok(Arc::new(OpenAiEmbedder::new(key, cfg.model.as_deref())?))
        }
        EmbedderBackend::Voyage => {
            let key = resolve_key(cfg, "VOYAGE_API_KEY")?;
            Ok(Arc::new(VoyageEmbedder::new(key, cfg.model.clone()).await?))
        }
        #[cfg(feature = "bge-local")]
        EmbedderBackend::LocalBge => {
            Ok(Arc::new(crate::embed::LocalBgeSmallEmbedder::new().await?))
        }
        #[cfg(not(feature = "bge-local"))]
        EmbedderBackend::LocalBge => {
            tracing::warn!(
                target: "wcore_memory::embed",
                "semantic search degraded: backend=local_bge requested but this binary \
                 was built without the `bge-local` feature; using keyword-hash embedder. \
                 Rebuild with `--features bge-local` or configure a cloud embedder for \
                 real semantic similarity."
            );
            Ok(Arc::new(HashedEmbedder::new().await?))
        }
        EmbedderBackend::Hashed => {
            tracing::warn!(
                target: "wcore_memory::embed",
                "semantic search degraded: using keyword-hash embedder; enable bge-local \
                 or configure a cloud embedder (openai/voyage) for real semantic similarity."
            );
            Ok(Arc::new(HashedEmbedder::new().await?))
        }
    }
}

/// Public facade. Cheap to clone (everything inside is Arc-wrapped).
#[derive(Clone)]
pub struct Memory {
    pub dispatcher: PartitionDispatcher,
    pub gate: Arc<MemoryAccessGate>,
    pub audit: Arc<AuditLog>,
    pub embedder: Arc<dyn Embedder>,
    pub db: Arc<Db>,
    pub cdc: Arc<CdcWriter>,
    /// Resolved project root (for legacy import).
    pub project_root: Option<PathBuf>,
}

impl Memory {
    /// Open a Memory rooted at the given project_root + session_id with the
    /// default embedder config (keyword-hash bag — degraded semantic search).
    ///
    /// Production bootstrap should prefer [`Memory::open_with_config`] so the
    /// embedder backend honours `[memory.embedder]` in `wcore.toml`. This
    /// signature is preserved for existing callers and tests.
    pub async fn open(project_root: &Path, session_id: &str) -> Result<Self> {
        Self::open_with_config(project_root, session_id, &EmbedderConfig::default()).await
    }

    /// Open a Memory rooted at the given project_root + session_id, selecting
    /// the embedder backend from `embedder` (see [`build_embedder`]).
    /// Uses `paths::*` for DB locations; creates all required parent
    /// directories.
    pub async fn open_with_config(
        project_root: &Path,
        session_id: &str,
        embedder_cfg: &EmbedderConfig,
    ) -> Result<Self> {
        let session_path = paths::session_db_path(session_id);
        let project_path = Some(paths::project_db_path(project_root));
        let global_path = paths::global_db_path().ok_or_else(|| {
            MemoryError::PathValidation("no global memory base dir resolvable".into())
        })?;

        let db = Arc::new(Db::open(session_path, project_path, global_path)?);
        let audit_path = paths::audit_db_path()
            .ok_or_else(|| MemoryError::PathValidation("no audit base dir resolvable".into()))?;
        let audit = Arc::new(AuditLog::open(audit_path)?);
        let gate = Arc::new(MemoryAccessGate::new(audit.clone(), AccessPolicy::empty()));
        let embedder: Arc<dyn Embedder> = build_embedder(embedder_cfg).await?;
        let cdc = Arc::new(CdcWriter::new_with_sinks(
            paths::changelog_path("session"),
            paths::changelog_path("project"),
            paths::changelog_path("global"),
        )?);
        let dispatcher = PartitionDispatcher::new(
            gate.clone(),
            db.clone(),
            embedder.clone(),
            cdc.clone(),
            Some(session_id.to_string()),
        );
        Ok(Self {
            dispatcher,
            gate,
            audit,
            embedder,
            db,
            cdc,
            project_root: Some(project_root.to_path_buf()),
        })
    }

    /// All-in-memory Memory (for tests). Uses the default embedder config
    /// (keyword-hash bag).
    pub async fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_config(&EmbedderConfig::default()).await
    }

    /// All-in-memory Memory selecting the embedder backend from `embedder_cfg`.
    pub async fn open_in_memory_with_config(embedder_cfg: &EmbedderConfig) -> Result<Self> {
        let db = Arc::new(Db::open_memory()?);
        let audit = Arc::new(AuditLog::open_memory()?);
        let gate = Arc::new(MemoryAccessGate::new(audit.clone(), AccessPolicy::empty()));
        let embedder: Arc<dyn Embedder> = build_embedder(embedder_cfg).await?;
        let cdc = Arc::new(CdcWriter::new_stub());
        let dispatcher = PartitionDispatcher::new(
            gate.clone(),
            db.clone(),
            embedder.clone(),
            cdc.clone(),
            Some("test".into()),
        );
        Ok(Self {
            dispatcher,
            gate,
            audit,
            embedder,
            db,
            cdc,
            project_root: None,
        })
    }

    /// Import legacy YAML memory files (v1 surface) from the project's
    /// memory directory, if present + not yet imported. Returns the
    /// report (idempotent; safe to call on every bootstrap).
    pub async fn import_legacy_if_present(&self) -> Result<LegacyImportReport> {
        let dir = match self.project_root.as_ref() {
            Some(root) => paths::auto_memory_dir(root),
            None => None,
        };
        match dir {
            Some(d) => legacy_import::import_if_present(&self.db, self.embedder.as_ref(), &d).await,
            None => Ok(LegacyImportReport::default()),
        }
    }

    /// Convenience accessor: the underlying MemoryApi.
    pub fn api(&self) -> &dyn MemoryApi {
        &self.dispatcher
    }

    /// M3.3 — attach an observability sink. The trait lives in
    /// `wcore_observability::sink::MemoryTraceSink` to respect the
    /// existing `wcore-memory → wcore-observability` dep edge.
    /// Every subsequent `MemoryApi` call routed through this `Memory`
    /// emits one event around the gated op.
    pub fn with_trace_sink(
        mut self,
        sink: Arc<dyn wcore_observability::sink::MemoryTraceSink>,
    ) -> Self {
        self.dispatcher = self.dispatcher.with_trace_sink(sink);
        self
    }

    /// M3.2 — spawn a background tokio task that ticks
    /// [`ConsolidationEngine::decay`] every `interval`. Returns the
    /// `JoinHandle` so callers can `.abort()` on shutdown.
    ///
    /// The scheduler tolerates transient decay errors (logs via
    /// `tracing::warn!` and keeps ticking) so a single bad row does not
    /// silently disable memory housekeeping.
    ///
    /// The first `interval` tick is skipped — newly-opened memory has a
    /// beat to settle before the first real decay sweep fires (`tokio`'s
    /// `interval` semantics fire the first tick immediately, which we
    /// don't want at boot).
    pub fn spawn_decay_scheduler(
        &self,
        interval: std::time::Duration,
    ) -> tokio::task::JoinHandle<()> {
        let dispatcher = self.dispatcher.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            // First tick fires immediately; skip it so the first real
            // tick is `interval` after spawn.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let engine = crate::consolidate::ConsolidationEngine::new(dispatcher.clone());
                if let Err(e) = engine.decay().await {
                    tracing::warn!(
                        target: "wcore_memory::decay",
                        error = %e,
                        "decay scheduler tick failed; continuing"
                    );
                }
            }
        })
    }
}

#[async_trait]
impl MemoryApi for Memory {
    async fn record_episode(&self, ep: Episode, tok: AccessToken) -> Result<EpisodeId> {
        self.dispatcher.record_episode(ep, tok).await
    }
    async fn assert_fact(&self, f: Fact, tok: AccessToken) -> Result<FactId> {
        self.dispatcher.assert_fact(f, tok).await
    }
    async fn upsert_procedure(&self, p: Procedure, tok: AccessToken) -> Result<ProcedureId> {
        self.dispatcher.upsert_procedure(p, tok).await
    }
    async fn list_procedures(&self, tier: Tier, tok: AccessToken) -> Result<Vec<Procedure>> {
        self.dispatcher.list_procedures(tier, tok).await
    }
    async fn update_user_model(&self, key: &str, val: Value, tok: AccessToken) -> Result<()> {
        self.dispatcher.update_user_model(key, val, tok).await
    }
    async fn search(&self, q: Query, tok: AccessToken) -> Result<Vec<Hit>> {
        self.dispatcher.search(q, tok).await
    }
    async fn get_episode(&self, id: &EpisodeId, tok: AccessToken) -> Result<Episode> {
        self.dispatcher.get_episode(id, tok).await
    }
    async fn user_model(&self, tok: AccessToken) -> Result<UserModel> {
        self.dispatcher.user_model(tok).await
    }
    async fn dream_now(&self) -> Result<DreamReport> {
        self.dispatcher.dream_now().await
    }
    async fn compact(&self, target_tokens: u64) -> Result<CompactReport> {
        self.dispatcher.compact(target_tokens).await
    }
    async fn record_skill_use(
        &self,
        skill_name: &str,
        succeeded: bool,
        latency_ms: u64,
    ) -> Result<()> {
        self.dispatcher
            .record_skill_use(skill_name, succeeded, latency_ms)
            .await
    }
    async fn top_procedures(
        &self,
        tier: Tier,
        k: usize,
        min_uses: u64,
        tok: AccessToken,
    ) -> Result<Vec<Procedure>> {
        self.dispatcher.top_procedures(tier, k, min_uses, tok).await
    }
    async fn kg_ingest_facts(&self, transcript: &str) -> Result<usize> {
        self.dispatcher.kg_ingest_facts(transcript).await
    }
    async fn rebind_session(&self, session_id: &str) -> Result<()> {
        self.dispatcher.rebind_session(session_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default config + default cargo features must resolve to the
    /// keyword-hash fallback. This is the degraded-but-functional path the
    /// finding flags — selection is config-driven, not hardcoded.
    #[tokio::test]
    async fn default_config_selects_hashed_fallback() {
        let embedder = build_embedder(&EmbedderConfig::default()).await.unwrap();
        assert_eq!(embedder.name(), "hashed/384");
    }

    /// `open_in_memory` (default config) wires the hashed embedder into a live
    /// `Memory` — the degraded fallback is observable on the facade.
    #[tokio::test]
    async fn open_in_memory_uses_hashed_embedder() {
        let mem = Memory::open_in_memory().await.unwrap();
        assert_eq!(mem.embedder.name(), "hashed/384");
    }

    /// Requesting a cloud backend with no resolvable API key is a hard error,
    /// proving the cloud arms are actually dispatched (not silently ignored).
    /// Uses a deliberately-unset env var name so the test is hermetic
    /// regardless of the developer's real `OPENAI_API_KEY`.
    #[tokio::test]
    async fn cloud_backend_without_key_errors() {
        let cfg = EmbedderConfig {
            backend: EmbedderBackend::OpenAi,
            api_key_env: Some("WCORE_MEMORY_TEST_UNSET_KEY_VAR".to_string()),
            model: None,
        };
        // `Arc<dyn Embedder>` isn't Debug, so unwrap_err()'s bound won't hold —
        // extract the error by match instead.
        let err = match build_embedder(&cfg).await {
            Ok(_) => panic!("expected an error when the cloud API key env var is unset"),
            Err(e) => e,
        };
        assert!(matches!(err, MemoryError::Embedding(_)));
        assert!(err.to_string().contains("API key"));
    }

    /// Without the `bge-local` feature, requesting the local BERT backend
    /// degrades to the hashed fallback rather than failing.
    #[cfg(not(feature = "bge-local"))]
    #[tokio::test]
    async fn local_bge_without_feature_degrades_to_hashed() {
        let cfg = EmbedderConfig {
            backend: EmbedderBackend::LocalBge,
            api_key_env: None,
            model: None,
        };
        let embedder = build_embedder(&cfg).await.unwrap();
        assert_eq!(embedder.name(), "hashed/384");
    }
}
