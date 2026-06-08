//! `SmsConfig` — TOML schema for one Twilio SMS channel instance.
//!
//! Secrets are NEVER stored here. The two `credential_handle_*` fields
//! are keys looked up in the `CredentialsStore` at `start()` time so
//! the Account SID + Auth Token only ever live in memory (or the OS
//! keychain).

use serde::{Deserialize, Serialize};

/// Default Twilio REST API base. Tests inject a mockito URL.
pub const DEFAULT_API_BASE: &str = "https://api.twilio.com";

/// Default attempts (including the initial request) before giving up.
pub const DEFAULT_MAX_RETRY_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SmsConfig {
    /// Twilio-owned phone number used as the outbound `From`. E.164.
    pub from_number: String,

    /// CredentialsStore key for the Twilio Account SID (ACxxxx...).
    pub credential_handle_account_sid: String,

    /// CredentialsStore key for the Twilio Auth Token.
    pub credential_handle_auth_token: String,

    /// Override the Twilio REST API base URL — tests point this at mockito.
    #[serde(default = "default_api_base")]
    pub api_base_url: String,

    /// Number of attempts (including the first) for transient failures.
    #[serde(default = "default_max_retry_attempts")]
    pub max_retry_attempts: u32,
}

fn default_api_base() -> String {
    DEFAULT_API_BASE.to_string()
}

fn default_max_retry_attempts() -> u32 {
    DEFAULT_MAX_RETRY_ATTEMPTS
}

impl SmsConfig {
    /// Convenience builder for tests.
    pub fn new_for_test(api_base: impl Into<String>) -> Self {
        Self {
            from_number: "+15550000000".to_string(),
            credential_handle_account_sid: "sms.test.account_sid".to_string(),
            credential_handle_auth_token: "sms.test.auth_token".to_string(),
            api_base_url: api_base.into(),
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
from_number = "+15551234567"
credential_handle_account_sid = "sms.acme.account_sid"
credential_handle_auth_token = "sms.acme.auth_token"
api_base_url = "https://api.twilio.com"
max_retry_attempts = 5
"#;
        let cfg: SmsConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.from_number, "+15551234567");
        assert_eq!(cfg.credential_handle_account_sid, "sms.acme.account_sid");
        assert_eq!(cfg.credential_handle_auth_token, "sms.acme.auth_token");
        assert_eq!(cfg.api_base_url, "https://api.twilio.com");
        assert_eq!(cfg.max_retry_attempts, 5);

        let re = toml::to_string(&cfg).unwrap();
        let cfg2: SmsConfig = toml::from_str(&re).unwrap();
        assert_eq!(cfg, cfg2);
    }

    #[test]
    fn defaults_apply_when_omitted() {
        let body = r#"
from_number = "+15551234567"
credential_handle_account_sid = "k1"
credential_handle_auth_token = "k2"
"#;
        let cfg: SmsConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.api_base_url, DEFAULT_API_BASE);
        assert_eq!(cfg.max_retry_attempts, DEFAULT_MAX_RETRY_ATTEMPTS);
    }

    #[test]
    fn deny_unknown_fields_rejects_extras() {
        let body = r#"
from_number = "+15551234567"
credential_handle_account_sid = "k1"
credential_handle_auth_token = "k2"
bogus_field = "nope"
"#;
        let err = toml::from_str::<SmsConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("bogus_field") || err.to_string().contains("unknown"),
            "expected deny_unknown_fields rejection, got {err}"
        );
    }

    #[test]
    fn missing_required_from_number_errors() {
        let body = r#"
credential_handle_account_sid = "k1"
credential_handle_auth_token = "k2"
"#;
        let err = toml::from_str::<SmsConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("from_number"),
            "expected missing from_number, got {err}"
        );
    }
}
