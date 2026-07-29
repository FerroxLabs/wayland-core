//! `MsTeamsConfig` — per-channel MS Teams options.

use serde::{Deserialize, Serialize};

/// Default Bot Framework service URL (Americas region).
const DEFAULT_SERVICE_URL: &str = "https://smba.trafficmanager.net/amer/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MsTeamsConfig {
    /// Credentials-store key for the Azure AD application (client) ID.
    pub credential_handle_app_id: String,
    /// Credentials-store key for the Azure AD client secret.
    pub credential_handle_app_password: String,
    /// Bot Framework service URL. Defaults to the Americas endpoint.
    #[serde(default = "default_service_url")]
    pub service_url: String,
    /// OAuth2 token endpoint. Defaults to the live Microsoft endpoint;
    /// overrideable so the adapter can be pointed at a local fixture.
    #[serde(default = "default_token_url")]
    pub token_url: String,
    /// OpenID Connect metadata document whose `jwks_uri` supplies the inbound
    /// JWT signing keys. Defaults to the live Bot Framework endpoint;
    /// overrideable so the adapter can be pointed at a local fixture.
    ///
    /// Both overrides exist for the same reason Discord grew `api_base_url` /
    /// `gateway_url`: without them this adapter's inbound path could not be
    /// exercised end-to-end without a vendor credential, because `start()`
    /// mints a token and `ingest_webhook` fetches a JWKS, both from hardcoded
    /// Microsoft hosts. They default to the production values, so an operator
    /// who sets neither is unaffected.
    #[serde(default = "default_openid_metadata_url")]
    pub openid_metadata_url: String,
}

fn default_service_url() -> String {
    DEFAULT_SERVICE_URL.to_string()
}

fn default_token_url() -> String {
    crate::token::BF_TOKEN_URL.to_string()
}

fn default_openid_metadata_url() -> String {
    crate::auth::BF_OPENID_METADATA_URL.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let raw = r#"
credential_handle_app_id = "msteams.acme.app_id"
credential_handle_app_password = "msteams.acme.app_password"
"#;
        let cfg: MsTeamsConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.credential_handle_app_id, "msteams.acme.app_id");
        assert_eq!(cfg.service_url, DEFAULT_SERVICE_URL);
        // The endpoint overrides must default to PRODUCTION Microsoft hosts —
        // an operator who sets neither must be unaffected by their existence.
        assert_eq!(cfg.token_url, crate::token::BF_TOKEN_URL);
        assert_eq!(cfg.openid_metadata_url, crate::auth::BF_OPENID_METADATA_URL);
    }

    #[test]
    fn endpoint_overrides_are_honoured() {
        let raw = r#"
credential_handle_app_id = "id"
credential_handle_app_password = "pw"
token_url = "http://127.0.0.1:19191/token"
openid_metadata_url = "http://127.0.0.1:19191/openid"
"#;
        let cfg: MsTeamsConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.token_url, "http://127.0.0.1:19191/token");
        assert_eq!(cfg.openid_metadata_url, "http://127.0.0.1:19191/openid");
    }

    #[test]
    fn custom_service_url() {
        let raw = r#"
credential_handle_app_id = "id"
credential_handle_app_password = "pw"
service_url = "https://smba.trafficmanager.net/emea/"
"#;
        let cfg: MsTeamsConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.service_url, "https://smba.trafficmanager.net/emea/");
    }
}
