//! `LocalBackend` — in-memory backend with optional JSON persistence.
//!
//! Default backend for single-user deployments. Persists to
//! `~/.wayland/user-model.json` on every write so a restart picks
//! up the running estimates. Concurrency: an `Arc<RwLock<HashMap>>`
//! holds the state; no fancy aggregation, just per-user-id buckets.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::UserModelBackend;
use crate::brief::{UserBrief, UserStyle};
use crate::error::UserModelError;
use crate::observation::{Observation, Outcome};
use crate::preferences::Preferences;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct UserRecord {
    brief: UserBrief,
    preferences: Preferences,
    obs_count: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskState {
    users: HashMap<String, UserRecord>,
}

#[derive(Clone)]
pub struct LocalBackend {
    inner: Arc<RwLock<HashMap<String, UserRecord>>>,
    persist_path: Option<PathBuf>,
}

impl LocalBackend {
    /// Construct an in-memory-only backend. Useful for tests.
    pub fn in_memory() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            persist_path: None,
        }
    }

    /// Construct a backend persisting to `path`. If the file exists,
    /// load existing state; otherwise start empty. Persistence is
    /// best-effort — write failures log and continue.
    pub fn with_persistence(path: impl Into<PathBuf>) -> Result<Self, UserModelError> {
        let path = path.into();
        let mut map = HashMap::new();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let state: DiskState = serde_json::from_slice(&bytes)?;
            map = state.users;
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(map)),
            persist_path: Some(path),
        })
    }

    /// Best-effort persistence. Snapshots the current map and writes
    /// atomically (temp + rename). Failures log via `tracing::warn`
    /// and do not propagate.
    async fn persist(&self) {
        let Some(path) = self.persist_path.clone() else {
            return;
        };
        let snapshot = self.inner.read().await.clone();
        let state = DiskState { users: snapshot };
        let bytes = match serde_json::to_vec_pretty(&state) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(target: "wcore_user_model::local", "persist serialize: {e}");
                return;
            }
        };
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!(target: "wcore_user_model::local", "persist mkdir: {e}");
            return;
        }
        let tmp = path.with_extension("tmp");
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            tracing::warn!(target: "wcore_user_model::local", "persist write tmp: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp, &path) {
            // Cross-fs rename fallback.
            let _ = std::fs::write(&path, &bytes);
            tracing::warn!(target: "wcore_user_model::local", "persist rename fallback: {e}");
        }
    }

    fn fold_style(existing: &UserStyle, obs_style: &[f32; 4], obs_count: u64) -> UserStyle {
        // EMA over the existing average. After many observations the
        // weight on the new sample drops; early on it's larger.
        let alpha = 1.0 / (obs_count as f32 + 1.0).max(1.0);
        let one_minus = 1.0 - alpha;
        UserStyle {
            formality: existing.formality * one_minus + obs_style[0] * alpha,
            energy: existing.energy * one_minus + obs_style[1] * alpha,
            terseness: existing.terseness * one_minus + obs_style[2] * alpha,
            emoji_use: existing.emoji_use * one_minus + obs_style[3] * alpha,
        }
    }
}

#[async_trait]
impl UserModelBackend for LocalBackend {
    async fn brief(&self, user_id: &str) -> Result<UserBrief, UserModelError> {
        let guard = self.inner.read().await;
        Ok(guard
            .get(user_id)
            .map(|r| r.brief.clone())
            .unwrap_or_default())
    }

    async fn preferences(&self, user_id: &str) -> Result<Preferences, UserModelError> {
        let guard = self.inner.read().await;
        Ok(guard
            .get(user_id)
            .map(|r| r.preferences.clone())
            .unwrap_or_default())
    }

    async fn observe(&self, user_id: &str, obs: Observation) -> Result<(), UserModelError> {
        {
            let mut guard = self.inner.write().await;
            let record = guard.entry(user_id.to_string()).or_default();
            record.obs_count = record.obs_count.saturating_add(1);
            if let Some(fp) = obs.style_fingerprint {
                record.brief.style = Self::fold_style(&record.brief.style, &fp, record.obs_count);
            }
            if obs.ts_secs > record.brief.last_observed_ts {
                record.brief.last_observed_ts = obs.ts_secs;
            }
            // Light-touch preference updates: count Accepted/Rejected
            // per (model, domain) — full PreferenceLearner lives in
            // 2.B.3.
            if let (Some(outcome), Some(domain)) = (obs.outcome, obs.hint.domain.as_deref()) {
                let tag = match outcome {
                    Outcome::Accepted | Outcome::Praised => "accepted",
                    Outcome::Rejected | Outcome::Corrected => "rejected",
                    Outcome::Ignored => "ignored",
                };
                record
                    .preferences
                    .tags
                    .entry(format!("{domain}.last_outcome"))
                    .and_modify(|v| *v = tag.to_string())
                    .or_insert_with(|| tag.to_string());
            }
        }
        self.persist().await;
        Ok(())
    }

    fn backend_tag(&self) -> &str {
        "local"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn brief_unknown_user_returns_default() {
        let b = LocalBackend::in_memory();
        let brief = b.brief("nobody").await.unwrap();
        assert!(brief.name.is_none());
        assert_eq!(brief.summary, "");
    }

    #[tokio::test]
    async fn observe_then_brief_reflects_style() {
        let b = LocalBackend::in_memory();
        b.observe(
            "alice",
            Observation {
                style_fingerprint: Some([0.8, 0.5, 0.6, 0.1]),
                ts_secs: 100,
                ..Observation::default()
            },
        )
        .await
        .unwrap();
        let brief = b.brief("alice").await.unwrap();
        assert!(brief.style.formality > 0.0);
        assert_eq!(brief.last_observed_ts, 100);
    }

    #[tokio::test]
    async fn ema_folds_new_observation() {
        let b = LocalBackend::in_memory();
        // First fingerprint — alpha = 1.0, brief.style == fingerprint
        b.observe(
            "alice",
            Observation {
                style_fingerprint: Some([1.0, 0.0, 0.0, 0.0]),
                ..Observation::default()
            },
        )
        .await
        .unwrap();
        // Second — alpha = 0.5 (count was 1 before increment), so
        // we expect movement toward 0.0.
        b.observe(
            "alice",
            Observation {
                style_fingerprint: Some([0.0, 0.0, 0.0, 0.0]),
                ..Observation::default()
            },
        )
        .await
        .unwrap();
        let brief = b.brief("alice").await.unwrap();
        assert!(brief.style.formality < 1.0);
        assert!(brief.style.formality > 0.0);
    }

    #[tokio::test]
    async fn outcome_tag_recorded_with_domain() {
        let b = LocalBackend::in_memory();
        b.observe(
            "alice",
            Observation {
                outcome: Some(Outcome::Accepted),
                hint: crate::observation::ToolHint {
                    domain: Some("rust".to_string()),
                    ..Default::default()
                },
                ..Observation::default()
            },
        )
        .await
        .unwrap();
        let prefs = b.preferences("alice").await.unwrap();
        assert_eq!(
            prefs.tags.get("rust.last_outcome").map(String::as_str),
            Some("accepted")
        );
    }

    #[tokio::test]
    async fn persistence_round_trips() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("user-model.json");
        let b = LocalBackend::with_persistence(&path).unwrap();
        b.observe(
            "alice",
            Observation {
                style_fingerprint: Some([0.7, 0.3, 0.5, 0.2]),
                ts_secs: 1000,
                ..Observation::default()
            },
        )
        .await
        .unwrap();
        // Re-open from disk.
        let b2 = LocalBackend::with_persistence(&path).unwrap();
        let brief = b2.brief("alice").await.unwrap();
        assert_eq!(brief.last_observed_ts, 1000);
        assert!(brief.style.formality > 0.0);
    }

    #[tokio::test]
    async fn backend_tag_is_local() {
        let b = LocalBackend::in_memory();
        assert_eq!(b.backend_tag(), "local");
    }
}
