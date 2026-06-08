//! Wave B4 — Honcho user-model plugin shell.
//!
//! F2 invariant: NO dep on `wcore-browser` / `wcore-cua`. Plugin shells
//! talk only through `wcore-plugin-api` mirror types. This crate
//! additionally avoids `wcore-agent` / `wcore-cli` (top-tier).
//!
//! Surface today:
//! - [`HonchoClient::mock`] — in-RAM, deterministic, no network. Default.
//! - [`HonchoClient::live_from_env`] — reads `HONCHO_API_KEY` (+ optional
//!   `HONCHO_BASE_URL`) and talks to a real Honcho deployment. Returns
//!   [`HonchoError::MissingApiKey`] when the key is absent — no silent
//!   fallback to mock (per `[[feedback-no-stubs]]`).
//!
//! v0.6.4 Task 2.3: the `plugin` module now provides the `Plugin` impl +
//! `inventory::submit!` factory so this crate registers a `UserModelSpec`
//! against the host's `ScopedUserModelRegistry` when discovered.

pub mod api;
pub mod error;
mod mock;
pub mod plugin;

pub use api::{DialecticInference, UserProfile};
pub use error::{HonchoError, Result};
pub use plugin::{MANIFEST_TOML, WaylandHoncho, WaylandHonchoFactory};

use std::sync::Arc;

/// Public façade. Pick one of [`HonchoClient::mock`] or
/// [`HonchoClient::live_from_env`] at construction; the surface is the
/// same after that.
pub struct HonchoClient {
    inner: Inner,
}

enum Inner {
    Mock(Arc<mock::MockClient>),
    Live(Arc<api::LiveClient>),
}

impl HonchoClient {
    /// In-RAM, deterministic, no network. The default flavour.
    pub fn mock() -> Self {
        Self {
            inner: Inner::Mock(Arc::new(mock::MockClient::default())),
        }
    }

    /// Talk to a real Honcho deployment. Requires `HONCHO_API_KEY`;
    /// `HONCHO_BASE_URL` is optional and defaults to the public endpoint.
    /// Returns [`HonchoError::MissingApiKey`] if the env var is absent.
    pub fn live_from_env() -> Result<Self> {
        Ok(Self {
            inner: Inner::Live(Arc::new(api::LiveClient::from_env()?)),
        })
    }

    /// v0.6.5 Task 1.5 — build a live client from plugin-supplied
    /// [`UserModelSpec`](wcore_plugin_api::UserModelSpec) fields. `base_url`
    /// and `api_key_env` are the only fields the Honcho backend reads
    /// today; the spec's `config` blob is reserved for future per-backend
    /// extensions.
    ///
    /// Returns [`HonchoError::MissingApiKey`] when the named env var
    /// (defaults to `HONCHO_API_KEY`) is absent — no silent fallback per
    /// `[[feedback-no-stubs]]`.
    pub fn from_spec(base_url: Option<&str>, api_key_env: Option<&str>) -> Result<Self> {
        Ok(Self {
            inner: Inner::Live(Arc::new(api::LiveClient::from_spec(base_url, api_key_env)?)),
        })
    }

    /// Write a single preference key/value for `user_id`.
    pub async fn learn_preference(&self, user_id: &str, key: &str, value: &str) -> Result<()> {
        match &self.inner {
            Inner::Mock(m) => m.learn_preference(user_id, key, value).await,
            Inner::Live(l) => l.learn_preference(user_id, key, value).await,
        }
    }

    /// Fetch the full profile for `user_id`. Unknown users yield an
    /// empty-but-well-formed [`UserProfile`] (mock); live behaviour
    /// depends on the upstream Honcho deployment.
    pub async fn recall_user(&self, user_id: &str) -> Result<UserProfile> {
        match &self.inner {
            Inner::Mock(m) => m.recall_user(user_id).await,
            Inner::Live(l) => l.recall_user(user_id).await,
        }
    }

    /// v0.8.1 U3 — fetch Honcho's dialectic inferences for `user_id`.
    ///
    /// Unlike `recall_user` (CRUD over explicit `learn_preference`
    /// writes), this surfaces traits Honcho *inferred* from prior
    /// conversations. The adapter merges these into `UserBrief.dialectic`
    /// so the engine can surface them in the per-turn system prompt.
    ///
    /// New users (no inferences yet) yield an empty Vec — the live
    /// endpoint returns 404 in that case and we paper over it.
    pub async fn representations(&self, user_id: &str) -> Result<Vec<DialecticInference>> {
        match &self.inner {
            Inner::Mock(m) => m.representations(user_id).await,
            Inner::Live(l) => l.representations(user_id).await,
        }
    }

    /// Test-only — seed dialectic inferences into the mock backing
    /// store. Returns `false` when called on a live client (no
    /// equivalent operation; live mode reads only from the server).
    #[doc(hidden)]
    pub fn seed_mock_representations(
        &self,
        user_id: &str,
        inferences: Vec<DialecticInference>,
    ) -> bool {
        match &self.inner {
            Inner::Mock(m) => {
                m.seed_representations(user_id, inferences);
                true
            }
            Inner::Live(_) => false,
        }
    }
}
