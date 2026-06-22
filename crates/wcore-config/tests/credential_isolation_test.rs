//! Task 0.1 / D1 — per-profile credential isolation.
//!
//! When WAYLAND_HOME is set (isolated profile), the Auto backend defaults to
//! the in-home encrypted vault (not the process-global OS keyring, which bleeds
//! across profiles). These tests prove cross-profile isolation via the file
//! backend on every platform (matches the Hetzner empirical run).

use std::path::Path;

use serial_test::serial;
use tempfile::tempdir;
use wcore_config::credentials::{CredentialsStorageConfig, open_store};

const PASS: &str = "test-vault-passphrase";
const KEY: &str = "providers.anthropic.api_key";

/// RAII guard: set/remove env vars, restore prior values on drop (keeps tests
/// hermetic even on a thread-per-test `cargo test` runner).
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}
impl EnvGuard {
    fn set(pairs: &[(&'static str, &str)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(k, v)| {
                let prev = std::env::var_os(k);
                unsafe { std::env::set_var(k, v) };
                (*k, prev)
            })
            .collect();
        Self { saved }
    }

    /// Remove the given vars now, restoring whatever was there on drop. Used so
    /// a test that requires a var ABSENT does not permanently clobber it for
    /// later tests in the same process (the `cargo test` thread-per-test case).
    fn remove(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|k| {
                let prev = std::env::var_os(k);
                unsafe { std::env::remove_var(k) };
                (*k, prev)
            })
            .collect();
        Self { saved }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, prev) in &self.saved {
            match prev {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

fn put_secret(home: &Path, value: &str) {
    let _g = EnvGuard::set(&[
        ("WAYLAND_HOME", home.to_str().unwrap()),
        ("WAYLAND_VAULT_PASSPHRASE", PASS),
    ]);
    let cfg = CredentialsStorageConfig::default(); // Auto
    let store = open_store(&cfg, &home.join("credentials.toml")).expect("open A");
    store.put(KEY, value).expect("put");
}

fn get_secret(home: &Path) -> Option<String> {
    let _g = EnvGuard::set(&[
        ("WAYLAND_HOME", home.to_str().unwrap()),
        ("WAYLAND_VAULT_PASSPHRASE", PASS),
    ]);
    let cfg = CredentialsStorageConfig::default();
    let store = open_store(&cfg, &home.join("credentials.toml")).expect("open get");
    store.get(KEY).expect("get")
}

#[test]
#[serial]
fn vault_lands_in_home_and_secret_does_not_cross_profiles() {
    let a = tempdir().unwrap();
    let b = tempdir().unwrap();
    put_secret(a.path(), "secret-A");

    // (a) vault materialized in home A — this is the discriminating assertion:
    // against the OLD keyring-Auto code this file would never appear (old Auto
    // writes credentials.toml or the OS keyring), so it pins the new behavior.
    assert!(
        a.path().join("credentials.enc").exists(),
        "encrypted vault must land inside WAYLAND_HOME A"
    );
    // Pre-condition sanity (nothing has been written to B yet).
    assert!(
        !b.path().join("credentials.enc").exists(),
        "home B must have no vault yet"
    );

    // Same passphrase, different home → secret is NOT resolvable.
    assert_eq!(
        get_secret(b.path()),
        None,
        "secret written under home A must not resolve under home B"
    );
    // Sanity: it IS resolvable under home A (per-home salt, same passphrase).
    assert_eq!(get_secret(a.path()).as_deref(), Some("secret-A"));
}

#[test]
#[serial]
fn deleting_profile_dir_leaves_zero_residual_secret() {
    let c = tempdir().unwrap();
    put_secret(c.path(), "secret-C");
    assert!(c.path().join("credentials.enc").exists());

    // Delete the whole profile home.
    std::fs::remove_dir_all(c.path()).unwrap();
    assert!(!c.path().join("credentials.enc").exists());

    // A fresh isolated home at the same path resolves nothing.
    std::fs::create_dir_all(c.path()).unwrap();
    assert_eq!(
        get_secret(c.path()),
        None,
        "no residual secret after profile dir deletion"
    );
}

#[cfg(unix)]
#[test]
#[serial]
fn vault_files_are_0600() {
    use std::os::unix::fs::PermissionsExt;
    let h = tempdir().unwrap();
    put_secret(h.path(), "secret-perms");
    for name in ["credentials.enc", "credentials.kdf.json"] {
        let p = h.path().join(name);
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{name} must be 0o600, got {mode:#o}");
    }
}

#[test]
#[serial]
fn no_passphrase_falls_back_to_plaintext_not_keyring() {
    // WAYLAND_HOME set but NO passphrase material → plaintext-0600 in-home,
    // round-trips, and writes credentials.toml (not credentials.enc).
    let h = tempdir().unwrap();
    let _g = EnvGuard::set(&[("WAYLAND_HOME", h.path().to_str().unwrap())]);
    // Ensure no stray passphrase from the ambient environment — routed through
    // the guard so it is restored for later tests in the same process.
    let _g2 = EnvGuard::remove(&["WAYLAND_VAULT_PASSPHRASE", "WAYLAND_VAULT_PASSPHRASE_FD"]);

    let cfg = CredentialsStorageConfig::default();
    let store = open_store(&cfg, &h.path().join("credentials.toml")).expect("open");
    store.put(KEY, "secret-plain").expect("put");
    assert_eq!(
        store.get(KEY).expect("get").as_deref(),
        Some("secret-plain")
    );
    assert!(
        h.path().join("credentials.toml").exists(),
        "plaintext fallback must write credentials.toml"
    );
    assert!(
        !h.path().join("credentials.enc").exists(),
        "must NOT create an encrypted vault without unlock material"
    );

    // The plaintext fallback must still be 0o600 (D1: warned, but never loose).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(h.path().join("credentials.toml"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "plaintext fallback must be 0o600, got {mode:#o}"
        );
    }
}
