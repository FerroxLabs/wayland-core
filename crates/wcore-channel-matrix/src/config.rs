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
    /// Credentials-store key for the Matrix **refresh** token, if the login
    /// that produced the access token asked for one (`"refresh_token": true`).
    ///
    /// Optional because a homeserver on the legacy `m.login.password` flow
    /// issues access tokens that live until logout, and configuring a handle
    /// with nothing behind it would be worse than declaring none. When it IS
    /// set, an expired access token is renewed in place instead of taking the
    /// channel down — see [`crate::token`].
    #[serde(default)]
    pub credential_handle_refresh_token: Option<String>,
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
        assert_eq!(
            cfg.credential_handle_refresh_token, None,
            "a config written before refresh support must still parse",
        );
    }

    /// The refresh handle is optional but must be REACHABLE. `deny_unknown_fields`
    /// means a typo'd or unregistered key is rejected outright, so a config
    /// that sets it and silently gets no refresh support is not possible.
    #[test]
    fn refresh_token_handle_round_trips() {
        let cfg: MatrixConfig = toml::from_str(
            r#"
homeserver_url = "https://matrix.org"
credential_handle_access_token = "matrix.prod.token"
credential_handle_refresh_token = "matrix.prod.refresh"
user_id = "@wayland-bot:matrix.org"
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.credential_handle_refresh_token.as_deref(),
            Some("matrix.prod.refresh"),
        );
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
