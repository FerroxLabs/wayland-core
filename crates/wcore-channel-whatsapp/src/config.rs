//! `WhatsappConfig` — TOML schema for one WhatsApp Cloud API channel instance.
//!
//! Secrets are NEVER stored here. The `credential_handle_*` fields are
//! keys looked up in the `CredentialsStore` at `start()` time so the
//! access token + app secret only ever live in memory (or the OS keychain).

use serde::{Deserialize, Serialize};

/// Default WhatsApp Cloud API base. Tests inject a mockito URL.
pub const DEFAULT_API_BASE: &str = "https://graph.facebook.com";

/// Default Graph API version pinned for the adapter. Pinning protects
/// against silent platform-side breakage; bump deliberately on review.
pub const DEFAULT_GRAPH_VERSION: &str = "v18.0";

/// Default attempts (including the initial request) before giving up.
pub const DEFAULT_MAX_RETRY_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WhatsappConfig {
    /// Which WhatsApp backend this instance drives.
    ///
    /// Defaults to [`WhatsappBackend::MetaBusiness`] — this struct configures
    /// the Cloud API adapter and nothing else. The key exists so an operator
    /// may state the default explicitly, and so `deny_unknown_fields` does not
    /// reject a config that does. Selecting a bridged backend routes to
    /// [`crate::bridge::WhatsappBridgeConfig`] instead; see [`crate::bridge`].
    #[serde(default)]
    pub backend: crate::bridge::WhatsappBackend,

    /// Human-readable workspace label — used in logs only.
    pub workspace_name: String,

    /// Meta-assigned Phone Number ID — appears in the outbound URL path.
    pub phone_number_id: String,

    /// Fallback recipient phone (E.164 with leading "+") used when an
    /// `OutgoingMessage::conversation_id` arrives empty.
    #[serde(default)]
    pub default_recipient: String,

    /// CredentialsStore key for the WhatsApp Cloud API access token.
    pub credential_handle_access_token: String,

    /// CredentialsStore key for the Meta App Secret (used for webhook
    /// signature verification via `X-Hub-Signature-256`).
    pub credential_handle_app_secret: String,

    /// Verify token for Meta's GET `hub.challenge` subscription handshake.
    /// This is an operator-chosen string (set identically in the Meta App
    /// dashboard), not a platform-issued secret, so it lives in config
    /// rather than the CredentialsStore. When unset, the GET verification
    /// handshake is rejected.
    #[serde(default)]
    pub verify_token: Option<String>,

    /// Override the Graph API base URL — tests point this at mockito.
    #[serde(default = "default_api_base")]
    pub api_base_url: String,

    /// Graph API version. Pinned per-instance so a rollout can stage
    /// the bump.
    #[serde(default = "default_graph_version")]
    pub graph_version: String,

    /// Number of attempts (including the first) for transient failures.
    #[serde(default = "default_max_retry_attempts")]
    pub max_retry_attempts: u32,
}

fn default_api_base() -> String {
    DEFAULT_API_BASE.to_string()
}

fn default_graph_version() -> String {
    DEFAULT_GRAPH_VERSION.to_string()
}

fn default_max_retry_attempts() -> u32 {
    DEFAULT_MAX_RETRY_ATTEMPTS
}

impl WhatsappConfig {
    /// Convenience builder for tests.
    pub fn new_for_test(api_base: impl Into<String>) -> Self {
        Self {
            backend: crate::bridge::WhatsappBackend::MetaBusiness,
            workspace_name: "test".to_string(),
            phone_number_id: "10000000000".to_string(),
            default_recipient: String::new(),
            credential_handle_access_token: "whatsapp.test.access_token".to_string(),
            credential_handle_app_secret: "whatsapp.test.app_secret".to_string(),
            verify_token: None,
            api_base_url: api_base.into(),
            graph_version: DEFAULT_GRAPH_VERSION.to_string(),
            max_retry_attempts: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_toml() {
        let body = r#"
workspace_name = "acme"
phone_number_id = "10987654321"
default_recipient = "+15555550100"
credential_handle_access_token = "whatsapp.acme.access_token"
credential_handle_app_secret = "whatsapp.acme.app_secret"
api_base_url = "https://graph.facebook.com"
graph_version = "v18.0"
max_retry_attempts = 5
"#;
        let cfg: WhatsappConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.workspace_name, "acme");
        assert_eq!(cfg.phone_number_id, "10987654321");
        assert_eq!(cfg.default_recipient, "+15555550100");
        assert_eq!(
            cfg.credential_handle_access_token,
            "whatsapp.acme.access_token"
        );
        assert_eq!(cfg.api_base_url, DEFAULT_API_BASE);
        assert_eq!(cfg.graph_version, "v18.0");
        assert_eq!(cfg.max_retry_attempts, 5);

        let re = toml::to_string(&cfg).unwrap();
        let cfg2: WhatsappConfig = toml::from_str(&re).unwrap();
        assert_eq!(cfg, cfg2);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let body = r#"
workspace_name = "acme"
phone_number_id = "10987654321"
credential_handle_access_token = "k1"
credential_handle_app_secret = "k2"
"#;
        let cfg: WhatsappConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.api_base_url, DEFAULT_API_BASE);
        assert_eq!(cfg.graph_version, DEFAULT_GRAPH_VERSION);
        assert_eq!(cfg.max_retry_attempts, DEFAULT_MAX_RETRY_ATTEMPTS);
        assert!(cfg.default_recipient.is_empty());
    }

    #[test]
    fn an_existing_config_with_no_backend_key_stays_on_the_cloud_api() {
        // The opt-in guarantee, as a test. Every config written before the
        // bridge existed omits `backend`, and must keep resolving to the Cloud
        // API adapter — which needs no Node and no bridge.
        let body = r#"
workspace_name = "acme"
phone_number_id = "10987654321"
credential_handle_access_token = "k1"
credential_handle_app_secret = "k2"
"#;
        let cfg: WhatsappConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.backend, crate::bridge::WhatsappBackend::MetaBusiness);
        assert!(
            !cfg.backend.is_bridged(),
            "the default backend must never route through the Node bridge"
        );
    }

    #[test]
    fn the_default_backend_may_be_stated_explicitly() {
        // Control for the test above: `deny_unknown_fields` must not reject a
        // config that spells the default out.
        let body = r#"
backend = "meta-business"
workspace_name = "acme"
phone_number_id = "10987654321"
credential_handle_access_token = "k1"
credential_handle_app_secret = "k2"
"#;
        let cfg: WhatsappConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.backend, crate::bridge::WhatsappBackend::MetaBusiness);
    }

    #[test]
    fn deny_unknown_fields_rejects_extras() {
        let body = r#"
workspace_name = "acme"
phone_number_id = "1"
credential_handle_access_token = "k1"
credential_handle_app_secret = "k2"
bogus_field = "nope"
"#;
        let err = toml::from_str::<WhatsappConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("bogus_field") || err.to_string().contains("unknown"),
            "expected deny_unknown_fields rejection, got {err}"
        );
    }
}
