//! `MatrixConfig` — per-channel Matrix options.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MatrixConfig {
    /// HTTPS base URL of the homeserver (e.g. `https://matrix.org`).
    pub homeserver_url: String,
    /// Credentials-store key for the Matrix access token.
    pub credential_handle_access_token: String,
    /// Full Matrix user ID of the bot (e.g. `@bot:matrix.org`).
    pub user_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_round_trip() {
        let raw = r#"
homeserver_url = "https://matrix.org"
credential_handle_access_token = "matrix.prod.token"
user_id = "@wayland-bot:matrix.org"
"#;
        let cfg: MatrixConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.homeserver_url, "https://matrix.org");
        assert_eq!(cfg.credential_handle_access_token, "matrix.prod.token");
        assert_eq!(cfg.user_id, "@wayland-bot:matrix.org");
    }

    #[test]
    fn missing_required_field_errors() {
        let err = toml::from_str::<MatrixConfig>(
            "homeserver_url = \"https://matrix.org\"\nuser_id = \"@bot:matrix.org\"",
        )
        .expect_err("should fail without access token handle");
        assert!(err.to_string().contains("credential_handle_access_token"));
    }
}
