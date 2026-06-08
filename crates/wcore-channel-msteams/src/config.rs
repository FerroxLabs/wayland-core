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
}

fn default_service_url() -> String {
    DEFAULT_SERVICE_URL.to_string()
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
