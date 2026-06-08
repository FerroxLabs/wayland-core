//! Convenience facade over the `keyring` crate for Phase 1.B.2 (`wayland
//! init`) and Phase 2.A (channel adapters).
//!
//! Thin wrapper that does not duplicate
//! [`crate::credentials::KeyringCredentialsStore`] — for the richer
//! backend-selectable API (plaintext / keyring / encrypted-file) see that
//! module. This file exists so callers that just need a per-service
//! prefixed `(service, account) -> secret` triple do not have to wire a
//! full [`crate::credentials::CredentialsStore`] trait object.

use thiserror::Error;

/// Service-name prefix applied to every keychain entry written by this
/// facade. Keeps Wayland secrets namespaced inside the OS keychain so
/// users can audit (and revoke) them as a group.
pub const SERVICE_PREFIX: &str = "wayland-core";

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KeychainError {
    #[error("secret not found for service={service} account={account}")]
    NotFound { service: String, account: String },
    #[error("no keychain backend available: {0}")]
    NoKeychain(String),
    #[error("keychain backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, KeychainError>;

/// Build the namespaced service identifier passed to the OS keychain.
pub fn service_name(service: &str) -> String {
    format!("{SERVICE_PREFIX}.{service}")
}

/// Store `value` under `(service, account)` in the OS keychain.
pub fn store_secret(service: &str, account: &str, value: &str) -> Result<()> {
    let entry = keyring::Entry::new(&service_name(service), account).map_err(classify_error)?;
    entry.set_password(value).map_err(classify_error)?;
    Ok(())
}

/// Retrieve the secret stored under `(service, account)`.
///
/// Returns [`KeychainError::NotFound`] when the entry does not exist.
pub fn get_secret(service: &str, account: &str) -> Result<String> {
    let entry = keyring::Entry::new(&service_name(service), account).map_err(classify_error)?;
    match entry.get_password() {
        Ok(s) => Ok(s),
        Err(keyring::Error::NoEntry) => Err(KeychainError::NotFound {
            service: service.to_string(),
            account: account.to_string(),
        }),
        Err(e) => Err(classify_error(e)),
    }
}

/// Delete the secret stored under `(service, account)`. Idempotent —
/// deleting a missing entry is a no-op.
pub fn delete_secret(service: &str, account: &str) -> Result<()> {
    let entry = keyring::Entry::new(&service_name(service), account).map_err(classify_error)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(classify_error(e)),
    }
}

fn classify_error(e: keyring::Error) -> KeychainError {
    match e {
        keyring::Error::PlatformFailure(inner) => KeychainError::NoKeychain(inner.to_string()),
        keyring::Error::NoStorageAccess(inner) => KeychainError::NoKeychain(inner.to_string()),
        other => KeychainError::Backend(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_prefixed() {
        assert_eq!(service_name("acp"), "wayland-core.acp");
        assert_eq!(service_name(""), "wayland-core.");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn roundtrip_macos_keychain() {
        let svc = "test-roundtrip-1b1";
        let acct = "1B1-test-account";
        let _ = delete_secret(svc, acct);
        store_secret(svc, acct, "hunter2").expect("store");
        assert_eq!(get_secret(svc, acct).expect("get"), "hunter2");
        delete_secret(svc, acct).expect("delete");
        let err = get_secret(svc, acct).expect_err("expected NotFound");
        assert!(matches!(err, KeychainError::NotFound { .. }));
    }
}
