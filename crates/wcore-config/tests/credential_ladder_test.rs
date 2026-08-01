//! The credential ladder, ablated in both directions.
//!
//! `FallbackCredentialsStore::put` used to end `Err(_) => self.plaintext.put(..)`
//! — a keyring write that failed produced a cleartext API key on disk, skipping
//! the `EncryptedFileCredentialsStore` tier that already shipped. The ladder
//! that replaced it is: **OS keyring → encrypted vault → refuse**, with the
//! legacy cleartext file mounted READ-ONLY so existing installs keep resolving,
//! and cleartext WRITES reachable only through an explicit
//! `CredentialsBackend::Plaintext`.
//!
//! HOST INDEPENDENCE. Every case here sets `WAYLAND_HOME`, which makes
//! `open_store` skip the OS keyring by construction (the keyring service is
//! process-global and would bleed secrets across profiles — C4/D1). So the
//! "keyring unavailable" arm is a property of the fixture, not of the host, and
//! these run identically on headless Linux, macOS and Windows. The
//! keyring-present arms are unit tests against an injected tier
//! (`credentials.rs`, `the_ladder_promotes_...`), for the same reason: a test
//! that needs a real keyring goes silent on exactly the hosts the ladder exists
//! for.
//!
//! NON-VACUITY. "No plaintext file exists" is also true when the write did
//! nothing at all, so every arm that expects a write to LAND reads the value
//! back from the tier it is supposed to have landed in.

use std::path::Path;

use serial_test::serial;
use tempfile::tempdir;
use wcore_config::credentials::{CredentialsBackend, CredentialsStorageConfig, open_store};

const KEY: &str = "providers.anthropic.api_key";
const SECRET: &str = "sk-ant-ladder-ablation-3f9c1d";
const PASS: &str = "test-vault-passphrase";

/// RAII env guard. Restores the prior value on drop so a test cannot leak its
/// fixture into the next one in the same process.
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    fn apply(pairs: &[(&'static str, Option<&str>)]) -> Self {
        let saved = pairs
            .iter()
            .map(|(key, value)| {
                let prior = std::env::var_os(key);
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
                (*key, prior)
            })
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in &self.saved {
            unsafe {
                match prior {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// An isolated profile home WITH vault unlock material: keyring off (isolated),
/// vault on.
fn home_with_vault(home: &Path) -> EnvGuard {
    EnvGuard::apply(&[
        ("WAYLAND_HOME", Some(home.to_str().unwrap())),
        ("WAYLAND_VAULT_PASSPHRASE", Some(PASS)),
        ("WAYLAND_VAULT_PASSPHRASE_FD", None),
    ])
}

/// An isolated profile home with NO unlock material: keyring off, vault off.
/// This is the headless/CI/container shape that used to reach cleartext.
fn home_without_any_secure_tier(home: &Path) -> EnvGuard {
    EnvGuard::apply(&[
        ("WAYLAND_HOME", Some(home.to_str().unwrap())),
        ("WAYLAND_VAULT_PASSPHRASE", None),
        ("WAYLAND_VAULT_PASSPHRASE_FD", None),
    ])
}

fn auto() -> CredentialsStorageConfig {
    CredentialsStorageConfig::default()
}

/// Assert the secret appears nowhere in cleartext under `home`, byte-scanning
/// every file in the tree rather than only the paths we expect to exist.
///
/// A per-path check ("credentials.toml does not exist") is satisfied by a leak
/// into any file we forgot to name — a temp file, a log, a `.env`. This walks.
fn assert_no_cleartext_secret_anywhere(home: &Path) {
    let mut stack = vec![home.to_path_buf()];
    let mut scanned = 0_usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            scanned += 1;
            assert!(
                !bytes
                    .windows(SECRET.len())
                    .any(|window| window == SECRET.as_bytes()),
                "the secret appears in CLEARTEXT in {}",
                path.display()
            );
        }
    }
    assert!(
        scanned > 0,
        "scanned zero files under {} — the cleartext scan cannot fail, so it proves nothing",
        home.display()
    );
}

// ---------------------------------------------------------------------------
// ABLATION (a) — keyring unavailable: the write lands in the ENCRYPTED VAULT.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn without_a_keyring_a_put_lands_in_the_encrypted_vault_and_never_in_cleartext() {
    let home = tempdir().unwrap();
    let _env = home_with_vault(home.path());
    let credentials_path = home.path().join("credentials.toml");

    let store = open_store(&auto(), &credentials_path).expect("the ladder always opens");
    store.put(KEY, SECRET).expect("the vault tier accepts it");

    // NON-VACUITY FIRST. Without this, every assertion below is also satisfied
    // by a `put` that silently did nothing.
    assert_eq!(
        store.get(KEY).expect("read back").as_deref(),
        Some(SECRET),
        "the credential must be readable back from the tier it landed in"
    );

    // It landed in the VAULT, specifically.
    let cipher = home.path().join("credentials.enc");
    assert!(
        cipher.exists(),
        "the encrypted vault must have been materialized"
    );
    assert!(
        home.path().join("credentials.kdf.json").exists(),
        "the vault's KDF params must have been persisted"
    );

    // And NOT in cleartext. Both the specific old sink and a whole-tree scan.
    assert!(
        !credentials_path.exists(),
        "credentials.toml must NOT exist — that is the sink the ladder removed"
    );
    assert_no_cleartext_secret_anywhere(home.path());

    // The vault bytes must be ciphertext, checked directly rather than inferred
    // from the filename.
    let blob = std::fs::read(&cipher).unwrap();
    assert!(!blob.is_empty(), "an empty vault proves nothing");
    assert!(
        !blob
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes()),
        "the vault holds the secret in cleartext"
    );
}

// ---------------------------------------------------------------------------
// ABLATION (b) — keyring AND vault unavailable: the write FAILS.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn without_any_secure_tier_a_put_fails_closed_rather_than_downgrading() {
    let home = tempdir().unwrap();
    let _env = home_without_any_secure_tier(home.path());
    let credentials_path = home.path().join("credentials.toml");

    // Opening still SUCCEEDS. "Cannot write" must never become "cannot start" —
    // an install with existing secrets has to keep booting and reading them.
    let store = open_store(&auto(), &credentials_path).expect("the ladder always opens");

    let error = store
        .put(KEY, SECRET)
        .expect_err("with no secure tier the write must be refused, not downgraded");
    let message = error.to_string();

    // The refusal has to be ACTIONABLE, and it must not sell cleartext as the
    // remedy. A refusal the operator cannot act on is how they end up reaching
    // for the thing the refusal exists to prevent.
    assert!(
        message.contains("WAYLAND_VAULT_PASSPHRASE_FD") && message.contains("WAYLAND_VAULT_PASSPHRASE"),
        "the refusal must name the vault passphrase route: {message}"
    );
    assert!(
        message.contains("not recommended") || message.contains("NOT recommended"),
        "the refusal may mention the cleartext opt-in but must mark it as discouraged: {message}"
    );
    assert!(
        !message.to_ascii_lowercase().contains("falling back")
            && !message.to_ascii_lowercase().contains("stored as plaintext"),
        "the refusal must not describe a downgrade it did not perform: {message}"
    );

    // Nothing was written anywhere, in any form.
    assert!(
        !credentials_path.exists(),
        "a refused write must not create a cleartext credentials file"
    );
    assert!(
        !home.path().join("credentials.enc").exists(),
        "a refused write must not create a vault either"
    );
    assert_eq!(
        store.get(KEY).expect("read"),
        None,
        "a refused write must not be readable from anywhere"
    );
}

// ---------------------------------------------------------------------------
// ABLATION (c) — the explicit opt-in must not be collateral damage.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn the_explicit_plaintext_backend_still_works_when_the_operator_names_it() {
    let home = tempdir().unwrap();
    let _env = home_without_any_secure_tier(home.path());
    let credentials_path = home.path().join("credentials.toml");

    let cfg = CredentialsStorageConfig {
        backend: CredentialsBackend::Plaintext,
        service_name: None,
    };
    let store = open_store(&cfg, &credentials_path).expect("the explicit backend opens");
    store
        .put(KEY, SECRET)
        .expect("an explicitly configured plaintext backend must still accept writes");

    assert_eq!(
        store.get(KEY).expect("read back").as_deref(),
        Some(SECRET),
        "non-vacuity: the opt-in path must actually store the value"
    );
    assert!(
        credentials_path.exists(),
        "the explicit plaintext backend writes the cleartext file it promises"
    );

    // The point of the opt-in is that it is CLEARTEXT and the operator knows.
    // Assert the cleartext, so this test also fails if the backend is silently
    // rerouted somewhere encrypted (which would make the opt-in a lie).
    let body = std::fs::read_to_string(&credentials_path).unwrap();
    assert!(
        body.contains(SECRET),
        "the explicit plaintext backend must write the value in cleartext"
    );

    // Opted in, but still 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&credentials_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "even the opt-in file must be 0600, got {mode:#o}");
    }
}

// ---------------------------------------------------------------------------
// B3 — an existing install must keep READING when it can no longer WRITE.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn an_existing_cleartext_install_still_reads_when_no_secure_tier_exists() {
    let home = tempdir().unwrap();
    let credentials_path = home.path().join("credentials.toml");

    // Seed the shape a pre-ladder headless install is already in: a cleartext
    // credentials.toml written by the old fallback.
    {
        let _env = home_without_any_secure_tier(home.path());
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Plaintext,
            service_name: None,
        };
        open_store(&cfg, &credentials_path)
            .unwrap()
            .put(KEY, SECRET)
            .unwrap();
    }
    assert!(credentials_path.exists(), "fixture must seed the legacy file");

    // Now open the DEFAULT (Auto) ladder on the same host. No keyring, no vault.
    let _env = home_without_any_secure_tier(home.path());
    let store = open_store(&auto(), &credentials_path).expect("opening must not fail");

    assert_eq!(
        store.get(KEY).expect("read").as_deref(),
        Some(SECRET),
        "the legacy cleartext tier is mounted READ-ONLY; an existing install must not \
         lose access to the keys it already has"
    );
    assert_eq!(
        store.get_many(&[KEY, "providers.openai.api_key"]).unwrap(),
        vec![Some(SECRET.to_string()), None],
        "the batched read path must resolve the legacy tier too"
    );

    // Read-only means exactly that: a NEW write is still refused.
    assert!(
        store.put("providers.openai.api_key", "sk-new").is_err(),
        "the legacy tier is readable, not writable — a new secret must not join it"
    );
    let body = std::fs::read_to_string(&credentials_path).unwrap();
    assert!(
        !body.contains("sk-new"),
        "the refused write reached the legacy file anyway: {body}"
    );

    // Delete still reaches the legacy tier, so a user can remove a key they can
    // no longer replace.
    store.delete(KEY).expect("delete");
    assert_eq!(
        store.get(KEY).expect("read after delete"),
        None,
        "a deleted key must not resurface from the legacy tier"
    );
}

// ---------------------------------------------------------------------------
// The downgrade is not permanent — the cleartext→vault direction, end to end
// through the public API.
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_cleartext_install_is_migrated_up_once_the_vault_can_be_unlocked() {
    let home = tempdir().unwrap();
    let credentials_path = home.path().join("credentials.toml");

    // A host that was stuck on cleartext.
    {
        let _env = home_without_any_secure_tier(home.path());
        let cfg = CredentialsStorageConfig {
            backend: CredentialsBackend::Plaintext,
            service_name: None,
        };
        open_store(&cfg, &credentials_path)
            .unwrap()
            .put(KEY, SECRET)
            .unwrap();
    }
    let seeded = std::fs::read_to_string(&credentials_path).unwrap();
    assert!(
        seeded.contains(SECRET),
        "fixture must actually start from a cleartext copy, else the migration \
         below proves nothing"
    );

    // The operator supplies a vault passphrase. The secure tier is back.
    let _env = home_with_vault(home.path());
    let store = open_store(&auto(), &credentials_path).expect("open");

    assert_eq!(
        store.get(KEY).expect("read").as_deref(),
        Some(SECRET),
        "non-vacuity: the secret must survive the move"
    );
    assert!(
        home.path().join("credentials.enc").exists(),
        "the secret must have moved INTO the vault"
    );
    assert!(
        !credentials_path.exists(),
        "the cleartext copy must be GONE — a heal that leaves the original behind \
         has not removed the exposure"
    );
    assert_no_cleartext_secret_anywhere(home.path());
}
