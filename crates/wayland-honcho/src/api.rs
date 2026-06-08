//! Wave B4 — live HTTP client for the Honcho user-model service.
//!
//! `UserProfile` is the only public wire type today. Two endpoints are
//! covered:
//! - `POST /v1/users/{user_id}/preferences` — write a single key/value.
//! - `GET  /v1/users/{user_id}`              — read the full profile.
//!
//! Live mode is feature-gated at the integration-test level (`live-honcho`)
//! and additionally guarded at runtime by `HONCHO_API_KEY`. The client is
//! constructible in the default build but only useful when the env var is
//! set; this keeps the dependency-graph cost paid up-front while
//! preserving the "no silent fallback" rule.

use crate::error::{HonchoError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A Honcho user profile. The exact shape Honcho returns is richer than
/// this in practice, but this shell only needs the round-trip-able
/// preferences map. `#[serde(default)]` on the map keeps deserialization
/// resilient to upstream responses that omit the field for new users.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfile {
    pub user_id: String,
    #[serde(default)]
    pub preferences: HashMap<String, String>,
}

/// v0.8.1 U3 — one dialectic inference returned by Honcho's
/// `/v1/users/{id}/representations` endpoint.
///
/// Honcho's API has evolved; this struct pins the canonical contract
/// our adapter expects. If/when the upstream response shape drifts, the
/// real translation lives here — every downstream consumer (the
/// `wcore-honcho-adapter`, the user-context block) reads this stable
/// shape.
///
/// Sibling shape lives in `wcore-user-model::brief::DialecticInference`;
/// the F2 invariant forbids `wayland-honcho` from depending on
/// `wcore-user-model`, so the adapter does the cross-crate mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DialecticInference {
    /// Inference category — `"preference"`, `"expertise"`, `"trait"`,
    /// or any backend-specific label.
    pub kind: String,
    /// What the inference is about (e.g. `"code_style"`).
    pub subject: String,
    /// The inferred value (e.g. `"terse"`).
    pub value: String,
    /// Honcho-reported confidence in this inference, 0.0..=1.0.
    pub confidence: f32,
    /// Number of underlying observations Honcho folded into this
    /// inference.
    pub evidence_count: u32,
}

/// Live HTTP client. Constructed via `HonchoClient::live_from_env`.
pub(crate) struct LiveClient {
    base_url: String,
    api_key: String,
    http: wcore_egress::EgressClient,
}

impl LiveClient {
    /// Build from `HONCHO_API_KEY` (required) and `HONCHO_BASE_URL`
    /// (optional, defaults to the public Honcho endpoint).
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("HONCHO_API_KEY").map_err(|_| HonchoError::MissingApiKey)?;
        let base_url = std::env::var("HONCHO_BASE_URL")
            .unwrap_or_else(|_| "https://api.honcho.dev".to_string());
        Ok(Self {
            base_url,
            api_key,
            http: wcore_egress::EgressClient::new(),
        })
    }

    /// v0.6.5 Task 1.5 — build from a plugin-supplied `UserModelSpec`.
    ///
    /// `base_url` overrides `HONCHO_BASE_URL` if `Some`; otherwise falls
    /// back to the env var, then the public default. `api_key_env` names
    /// the env var the host should read for the API key; falls back to
    /// `HONCHO_API_KEY` when `None`. Returns [`HonchoError::MissingApiKey`]
    /// when the named env var is absent — no silent fallback per
    /// `[[feedback-no-stubs]]`.
    pub fn from_spec(base_url: Option<&str>, api_key_env: Option<&str>) -> Result<Self> {
        let key_var = api_key_env.unwrap_or("HONCHO_API_KEY");
        let api_key = std::env::var(key_var).map_err(|_| HonchoError::MissingApiKey)?;
        let base_url = base_url.map(str::to_string).unwrap_or_else(|| {
            std::env::var("HONCHO_BASE_URL")
                .unwrap_or_else(|_| "https://api.honcho.dev".to_string())
        });
        Ok(Self {
            base_url,
            api_key,
            http: wcore_egress::EgressClient::new(),
        })
    }

    pub async fn learn_preference(&self, user_id: &str, key: &str, value: &str) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/v1/users/{user_id}/preferences", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({ "key": key, "value": value }))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(HonchoError::Api(format!("status {}", resp.status())));
        }
        Ok(())
    }

    pub async fn recall_user(&self, user_id: &str) -> Result<UserProfile> {
        let resp = self
            .http
            .get(format!("{}/v1/users/{user_id}", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(HonchoError::Api(format!("status {}", resp.status())));
        }
        Ok(resp.json::<UserProfile>().await?)
    }

    /// v0.8.1 U3 — fetch Honcho's dialectic inferences for `user_id`.
    ///
    /// Hits `GET /v1/users/{user_id}/representations` and parses the
    /// response into our pinned [`DialecticInference`] contract. A 404
    /// (new user / no inferences yet) returns an empty Vec rather than
    /// an error — the engine treats "no inferences" as the default,
    /// indistinguishable-from-anonymous state.
    pub async fn representations(&self, user_id: &str) -> Result<Vec<DialecticInference>> {
        let resp = self
            .http
            .get(format!(
                "{}/v1/users/{user_id}/representations",
                self.base_url
            ))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            return Err(HonchoError::Api(format!("status {}", resp.status())));
        }
        Ok(resp.json::<Vec<DialecticInference>>().await?)
    }
}
