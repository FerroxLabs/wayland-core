//! `wcore-honcho-adapter` — bridges `wcore-user-model::UserModelBackend`
//! to `wayland-honcho::HonchoClient`.
//!
//! The engine talks to the user model through the
//! `UserModelBackend` trait. `LocalBackend` (in-process, JSON-on-disk)
//! is the default for single-user deployments. This crate adds the
//! second concrete impl: a `HonchoUserModelBackend` that round-trips
//! every trait method against a real Honcho deployment.
//!
//! Selection happens at bootstrap via [`select_backend_from_env`]:
//!
//! ```text
//! WAYLAND_USER_MODEL_BACKEND=local   → LocalBackend (default)
//! WAYLAND_USER_MODEL_BACKEND=honcho  → HonchoUserModelBackend
//! ```
//!
//! The `honcho` flavour reads `HONCHO_API_URL` + `HONCHO_API_KEY`. No
//! silent fallback — a missing env var is a hard `UserModelError::Config`
//! per `[[feedback-no-stubs]]`.

pub mod error;
pub mod map;

use std::sync::Arc;

use async_trait::async_trait;
use wayland_honcho::HonchoClient;
use wcore_user_model::{
    LocalBackend, Observation, Preferences, UserBrief, UserModelBackend, UserModelError,
};

use crate::error::honcho_to_user_model;
use crate::map::{
    honcho_inf_to_user_model, observation_to_writes, profile_to_brief, profile_to_preferences,
};

/// `HonchoClient`-backed implementation of `UserModelBackend`.
///
/// Holds an `Arc<HonchoClient>` so the same client may be shared with
/// other Honcho-aware components (e.g. plugin-reified user models in
/// `wcore-agent::plugins::apply`) without cloning HTTP state.
pub struct HonchoUserModelBackend {
    client: Arc<HonchoClient>,
}

impl HonchoUserModelBackend {
    pub fn new(client: Arc<HonchoClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl UserModelBackend for HonchoUserModelBackend {
    async fn brief(&self, user_id: &str) -> Result<UserBrief, UserModelError> {
        let profile = self
            .client
            .recall_user(user_id)
            .await
            .map_err(honcho_to_user_model)?;
        let mut brief = profile_to_brief(&profile);
        // v0.8.1 U3 — layer in dialectic inferences alongside the
        // explicit-preference-derived brief. Failures here are degraded
        // (warn + empty dialectic) rather than fatal: a representations
        // outage should not block a turn that has a perfectly-good
        // profile available.
        match self.client.representations(user_id).await {
            Ok(infs) => {
                brief.dialectic = infs.into_iter().map(honcho_inf_to_user_model).collect();
            }
            Err(e) => tracing::warn!(
                target: "wcore_honcho_adapter",
                user_id,
                error = %e,
                "representations fetch failed; brief.dialectic empty"
            ),
        }
        Ok(brief)
    }

    async fn preferences(&self, user_id: &str) -> Result<Preferences, UserModelError> {
        let profile = self
            .client
            .recall_user(user_id)
            .await
            .map_err(honcho_to_user_model)?;
        Ok(profile_to_preferences(&profile))
    }

    async fn observe(&self, user_id: &str, obs: Observation) -> Result<(), UserModelError> {
        let writes = observation_to_writes(&obs);
        if writes.is_empty() {
            // No Honcho equivalent for this observation. Log + skip
            // rather than silently swallow — preserves the
            // [[feedback-no-stubs]] honesty contract.
            tracing::warn!(
                target: "wcore_honcho_adapter",
                user_id,
                ?obs,
                "observation has no Honcho mapping; skipping"
            );
            return Ok(());
        }
        for (key, value) in writes {
            self.client
                .learn_preference(user_id, &key, &value)
                .await
                .map_err(honcho_to_user_model)?;
        }
        Ok(())
    }

    fn backend_tag(&self) -> &str {
        "honcho"
    }
}

/// Construct the configured backend based on environment.
///
/// `WAYLAND_USER_MODEL_BACKEND=local` (default) returns an in-memory
/// `LocalBackend`. Persistent path-on-disk wiring stays in `wcore-agent`
/// bootstrap — this helper picks the SHAPE, not the storage location.
///
/// `WAYLAND_USER_MODEL_BACKEND=honcho` returns a `HonchoUserModelBackend`
/// constructed from `HONCHO_API_URL` (or the public default if absent)
/// and `HONCHO_API_KEY` (required — surfaces as `UserModelError::Config`
/// if missing).
pub fn select_backend_from_env() -> Result<Arc<dyn UserModelBackend>, UserModelError> {
    let choice =
        std::env::var("WAYLAND_USER_MODEL_BACKEND").unwrap_or_else(|_| "local".to_string());
    match choice.as_str() {
        "local" => Ok(Arc::new(LocalBackend::in_memory())),
        "honcho" => {
            // HonchoClient::live_from_env already reads HONCHO_API_KEY +
            // HONCHO_BASE_URL and returns HonchoError::MissingApiKey when
            // the key is absent. Surface the failure as Config so the
            // caller knows it's a bootstrap mis-config, not a transient
            // network problem.
            if std::env::var("HONCHO_API_KEY").is_err() {
                return Err(UserModelError::Config(
                    "HONCHO_API_KEY not set (required for WAYLAND_USER_MODEL_BACKEND=honcho)"
                        .to_string(),
                ));
            }
            let client = HonchoClient::live_from_env().map_err(honcho_to_user_model)?;
            Ok(Arc::new(HonchoUserModelBackend::new(Arc::new(client))))
        }
        other => Err(UserModelError::Config(format!(
            "unknown WAYLAND_USER_MODEL_BACKEND: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_user_model::observation::{Observation, Outcome, ToolHint};

    #[tokio::test]
    async fn backend_tag_is_honcho() {
        let client = Arc::new(HonchoClient::mock());
        let backend = HonchoUserModelBackend::new(client);
        assert_eq!(backend.backend_tag(), "honcho");
    }

    #[tokio::test]
    async fn observe_then_brief_round_trips_style() {
        let client = Arc::new(HonchoClient::mock());
        let backend = HonchoUserModelBackend::new(Arc::clone(&client));
        backend
            .observe(
                "alice",
                Observation {
                    style_fingerprint: Some([0.7, 0.3, 0.5, 0.2]),
                    ts_secs: 1000,
                    ..Observation::default()
                },
            )
            .await
            .expect("observe must succeed against mock");
        let brief = backend.brief("alice").await.expect("brief must succeed");
        assert_eq!(brief.last_observed_ts, 1000);
        assert!((brief.style.formality - 0.7).abs() < 1e-6);
        assert!((brief.style.terseness - 0.5).abs() < 1e-6);
    }

    #[tokio::test]
    async fn observe_then_preferences_records_outcome_tag() {
        let client = Arc::new(HonchoClient::mock());
        let backend = HonchoUserModelBackend::new(Arc::clone(&client));
        backend
            .observe(
                "alice",
                Observation {
                    outcome: Some(Outcome::Accepted),
                    hint: ToolHint {
                        domain: Some("rust".to_string()),
                        ..Default::default()
                    },
                    ..Observation::default()
                },
            )
            .await
            .expect("observe must succeed against mock");
        let prefs = backend
            .preferences("alice")
            .await
            .expect("preferences must succeed");
        assert_eq!(
            prefs.tags.get("rust.last_outcome").map(String::as_str),
            Some("accepted")
        );
    }

    #[tokio::test]
    async fn observe_no_signal_is_a_noop() {
        // Observation with no fingerprint, no domain — adapter should
        // not fail (and the mock should remain empty).
        let client = Arc::new(HonchoClient::mock());
        let backend = HonchoUserModelBackend::new(Arc::clone(&client));
        backend
            .observe("alice", Observation::default())
            .await
            .expect("noop observe must succeed");
        let brief = backend.brief("alice").await.expect("brief must succeed");
        assert_eq!(brief.last_observed_ts, 0);
        assert_eq!(brief.style.formality, 0.0);
    }

    #[tokio::test]
    async fn brief_for_unknown_user_is_default() {
        let client = Arc::new(HonchoClient::mock());
        let backend = HonchoUserModelBackend::new(client);
        let brief = backend.brief("nobody").await.expect("brief must succeed");
        assert!(brief.name.is_none());
        assert_eq!(brief.summary, "");
        assert_eq!(brief.last_observed_ts, 0);
        // v0.8.1 U3 — dialectic should be empty for a brand-new user.
        assert!(brief.dialectic.is_empty());
    }

    #[tokio::test]
    async fn brief_merges_dialectic_inferences_from_representations() {
        // v0.8.1 U3 — the brief() impl pulls representations alongside
        // recall_user. Seed the mock with both a preference (round-trips
        // via UserProfile) and a dialectic inference, then assert both
        // surface on the same brief.
        let client = Arc::new(HonchoClient::mock());
        client
            .learn_preference("alice", "wayland.name", "Alice")
            .await
            .expect("learn_preference must succeed");
        let seeded = vec![
            wayland_honcho::DialecticInference {
                kind: "preference".into(),
                subject: "code_style".into(),
                value: "terse".into(),
                confidence: 0.82,
                evidence_count: 4,
            },
            wayland_honcho::DialecticInference {
                kind: "expertise".into(),
                subject: "rust".into(),
                value: "expert".into(),
                confidence: 0.91,
                evidence_count: 12,
            },
        ];
        assert!(client.seed_mock_representations("alice", seeded));

        let backend = HonchoUserModelBackend::new(Arc::clone(&client));
        let brief = backend.brief("alice").await.expect("brief must succeed");

        // Profile-derived fields still work.
        assert_eq!(brief.name.as_deref(), Some("Alice"));
        // Dialectic merged in.
        assert_eq!(brief.dialectic.len(), 2);
        assert_eq!(brief.dialectic[0].subject, "code_style");
        assert_eq!(brief.dialectic[1].kind, "expertise");
        assert_eq!(brief.dialectic[1].evidence_count, 12);
    }

    #[tokio::test]
    async fn brief_dialectic_is_empty_when_no_representations_seeded() {
        // recall_user has a profile, but no dialectic — brief.dialectic
        // must come back empty (not error, not a panic).
        let client = Arc::new(HonchoClient::mock());
        client
            .learn_preference("bob", "wayland.summary", "noted")
            .await
            .unwrap();
        let backend = HonchoUserModelBackend::new(client);
        let brief = backend.brief("bob").await.expect("brief must succeed");
        assert_eq!(brief.summary, "noted");
        assert!(brief.dialectic.is_empty());
    }

    #[test]
    fn selector_defaults_to_local() {
        // Snapshot + restore so other parallel tests in the workspace
        // don't see our temporary unset. SAFETY: env mutation is a
        // process-wide side effect; tokio tests in this module use
        // `#[tokio::test]` (single-thread per test) and this is a plain
        // `#[test]`, so concurrent tests in the same binary would race.
        // Risk accepted because this binary has only the adapter tests
        // and the only mutation here is `remove_var`.
        let prior = std::env::var("WAYLAND_USER_MODEL_BACKEND").ok();
        // SAFETY: see comment above.
        unsafe {
            std::env::remove_var("WAYLAND_USER_MODEL_BACKEND");
        }
        let backend = select_backend_from_env().expect("default selector must succeed");
        assert_eq!(backend.backend_tag(), "local");
        // SAFETY: see comment above.
        unsafe {
            if let Some(v) = prior {
                std::env::set_var("WAYLAND_USER_MODEL_BACKEND", v);
            }
        }
    }

    #[test]
    fn selector_unknown_backend_errors() {
        // SAFETY: see selector_defaults_to_local for env-mutation caveat.
        let prior = std::env::var("WAYLAND_USER_MODEL_BACKEND").ok();
        unsafe {
            std::env::set_var("WAYLAND_USER_MODEL_BACKEND", "fictional");
        }
        let result = select_backend_from_env();
        let is_config_err = matches!(&result, Err(UserModelError::Config(_)));
        assert!(
            is_config_err,
            "expected Config error, got Ok or non-Config error"
        );
        // Drop the Arc explicitly so the success path doesn't outlive the env-var unset below.
        drop(result);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_USER_MODEL_BACKEND", v),
                None => std::env::remove_var("WAYLAND_USER_MODEL_BACKEND"),
            }
        }
    }

    #[test]
    #[ignore = "requires live HONCHO_API_KEY + HONCHO_API_URL; run explicitly with --ignored"]
    fn selector_picks_honcho_when_configured() {
        // SAFETY: see selector_defaults_to_local for env-mutation caveat.
        unsafe {
            std::env::set_var("WAYLAND_USER_MODEL_BACKEND", "honcho");
        }
        let backend = select_backend_from_env().expect("honcho selector must succeed");
        assert_eq!(backend.backend_tag(), "honcho");
    }
}
