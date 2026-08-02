//! LIVE keyring test: an OAuth-sized secret must survive the host's real
//! credential store.
//!
//! This talks to the machine's ACTUAL credential store — Windows Credential
//! Manager, macOS Keychain, Secret Service — so it is `#[ignore]`d and only
//! runs under `-- --ignored`. A host-global singleton is not something a normal
//! `cargo test` run should be writing into.
//!
//! `#[ignore]` rather than an env-var early-return on purpose: an early return
//! reports as `ok`, and a skipped test that renders as a pass is exactly the
//! failure this suite exists to avoid. Ignored reports as ignored.
//!
//! Why it exists: Windows Credential Manager caps `CredentialBlob` at
//! `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2560 bytes) and the `keyring` crate encodes
//! the value as UTF-16, so ONE entry holds 1280 code units. A ChatGPT OAuth
//! token set — access JWT + id JWT + refresh token — is several times that.
//! Once OAuth tokens moved onto the credential ladder, `auth login` on Windows
//! hit the cap, the keyring rung refused, and a desktop with no
//! `WAYLAND_VAULT_PASSPHRASE` had no rung left: the login was refused outright,
//! where it had previously succeeded.
//!
//! The test grades THREE states, never two. A host whose keyring cannot be
//! written at all reports NOT MEASURED and fails — "could not look" must never
//! render as a pass.

use wcore_config::credentials::{CredentialsStore, KeyringCredentialsStore};

/// A service name that cannot collide with the product's own
/// (`wayland-core`) or with a developer's real credentials.
const SERVICE: &str = "wayland-core-live-keyring-blob-test";

/// Realistically sized OAuth token set: access JWT + id JWT + refresh token.
fn oauth_token_set_json() -> String {
    let jwt = format!("hdr.{}.sig", "P".repeat(1400));
    format!(
        r#"{{"access_token":"{jwt}","id_token":"{jwt}","refresh_token":"{}","token_type":"Bearer","expires_at_unix_secs":1700000000}}"#,
        "R".repeat(500)
    )
}

fn utf16_units(value: &str) -> usize {
    value.chars().map(char::len_utf16).sum()
}

#[test]
#[ignore = "writes into the host's real credential store; run with -- --ignored"]
fn an_oauth_sized_secret_survives_the_hosts_real_keyring() {
    let store = KeyringCredentialsStore::new(SERVICE);
    let key = "oauth.livetest.tokens";

    // ── Instrument self-test ────────────────────────────────────────────
    // If a TINY value cannot round-trip, this host's keyring is unusable and
    // the measurement below would be meaningless. Report that as NOT MEASURED
    // and fail: a keyring-less host must not produce a green tick for a
    // property nobody observed.
    let probe_key = "wayland.livetest.probe";
    let probe = store
        .put(probe_key, "probe")
        .and_then(|()| store.get(probe_key));
    let probe_ok = matches!(&probe, Ok(Some(value)) if value == "probe");
    let _ = store.delete(probe_key);
    assert!(
        probe_ok,
        "NOT MEASURED — this host's keyring could not round-trip a 5-character value \
         ({probe:?}); the blob-cap measurement was not taken"
    );

    // ── Positive control: is the cap real on THIS host? ─────────────────
    // Written through a bare `keyring::Entry`, i.e. the unspanned path the
    // ladder used before this fix. On Windows this must FAIL; elsewhere it may
    // succeed, and the run then only proves the spanning path is lossless.
    let value = oauth_token_set_json();
    let raw_entry = keyring::Entry::new(SERVICE, "oauth.livetest.raw").expect("keyring entry");
    let raw_result = raw_entry.set_password(&value);
    let _ = raw_entry.delete_credential();
    println!(
        "MEASURED host={} value_utf16_units={} single_entry_write={}",
        std::env::consts::OS,
        utf16_units(&value),
        match &raw_result {
            Ok(()) => "ACCEPTED (this host has no cap at this size)".to_string(),
            Err(error) => format!("REFUSED ({error})"),
        }
    );
    #[cfg(windows)]
    assert!(
        raw_result.is_err(),
        "NOT MEASURED — Windows Credential Manager accepted a {}-unit blob in one entry, so \
         this run did not exercise the cap this test exists for",
        utf16_units(&value)
    );

    // ── The property ────────────────────────────────────────────────────
    store
        .put(key, &value)
        .unwrap_or_else(|error| panic!("an OAuth-sized login must be storable: {error}"));
    let read_back = store.get(key).expect("read back");
    assert_eq!(
        read_back.as_deref(),
        Some(value.as_str()),
        "the stored login must read back byte-identical"
    );

    // Rewriting must not tear: a second, DIFFERENT oversized value must also
    // round-trip through the same key.
    let second = value.replace('P', "Q");
    store.put(key, &second).expect("rewrite");
    assert_eq!(
        store.get(key).expect("read back").as_deref(),
        Some(second.as_str())
    );

    // ── Logout leaves nothing behind ────────────────────────────────────
    store.delete(key).expect("delete");
    assert!(
        store.get(key).expect("read after delete").is_none(),
        "a deleted login must not read back"
    );
    println!("PASS an_oauth_sized_secret_survives_the_hosts_real_keyring");
}
