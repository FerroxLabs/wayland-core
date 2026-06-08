//! Wave SD — credential storage tests.
//!
//! Closes SECURITY MAJOR #16 verification: the plaintext credentials
//! file is created with `0o600` perms on Unix; the default storage
//! backend is plaintext; the keyring backend round-trips when the
//! local OS keyring is available.

use tempfile::tempdir;

use wcore_config::credentials::{
    CredentialsBackend, CredentialsStorageConfig, CredentialsStore, PlaintextCredentialsStore,
};
// `secure_credential_file` uses Unix file permissions (0o600) and is only
// exercised by `secure_credential_file_tightens_loose_perms` below, which
// is itself `#[cfg(unix)]`. Gate the import to match — otherwise Windows
// clippy fails with `unused import`.
#[cfg(unix)]
use wcore_config::credentials::secure_credential_file;

#[test]
fn default_backend_is_plaintext() {
    let cfg = CredentialsStorageConfig::default();
    assert_eq!(cfg.backend, CredentialsBackend::Plaintext);
}

#[test]
fn plaintext_round_trip_through_trait_object() {
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("creds.toml");
    let store: Box<dyn CredentialsStore> = Box::new(PlaintextCredentialsStore::new(&path));

    assert!(store.get("anthropic").expect("get clean").is_none());
    store.put("anthropic", "sk-ant-test").expect("put");
    assert_eq!(
        store.get("anthropic").expect("get after put").as_deref(),
        Some("sk-ant-test")
    );

    store.put("openai", "sk-test").expect("put 2");
    assert_eq!(
        store.get("openai").expect("get 2").as_deref(),
        Some("sk-test")
    );

    store.delete("anthropic").expect("delete");
    assert!(store.get("anthropic").expect("get after delete").is_none());
    assert_eq!(
        store
            .get("openai")
            .expect("get other after delete")
            .as_deref(),
        Some("sk-test"),
        "unrelated key must survive a delete"
    );
}

#[cfg(unix)]
#[test]
fn plaintext_write_enforces_0600_perms() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("creds.toml");
    let store = PlaintextCredentialsStore::new(&path);
    store.put("k", "v").expect("put");
    let meta = std::fs::metadata(&path).expect("metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "plaintext credentials file must land at 0o600, got {mode:#o}"
    );
}

#[cfg(unix)]
#[test]
fn secure_credential_file_tightens_loose_perms() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().expect("tempdir");
    let path = dir.path().join("loose.toml");
    std::fs::write(&path, "secrets = {}").expect("seed file");
    // Set to world-readable to simulate a default-umask write.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    secure_credential_file(&path).expect("secure");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn keyring_backend_round_trip_when_available() {
    // The keyring service is best-effort — CI sandboxes don't always
    // have a Secret Service / Keychain socket. We attempt the round
    // trip and on transport failure tag the test as "no keyring here"
    // rather than failing — the smoke is: when the backend IS
    // available, put/get/delete round-trips correctly.
    use wcore_config::credentials::KeyringCredentialsStore;
    // Unique key per run so we never clobber a real entry.
    let key = format!("wayland-core-test-{}", uuid_like_suffix());
    let store = KeyringCredentialsStore::new("wayland-core-test-suite");

    let Ok(()) = store.put(&key, "test-secret") else {
        eprintln!("keyring put failed — host has no keyring, skipping");
        return;
    };
    let got = store.get(&key).expect("get after put");
    assert_eq!(got.as_deref(), Some("test-secret"));
    store.delete(&key).expect("delete");
    let after = store.get(&key).expect("get after delete");
    assert!(after.is_none(), "deleted entry should not return");
}

fn uuid_like_suffix() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:032x}")
}
