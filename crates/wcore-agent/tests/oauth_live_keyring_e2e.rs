//! VERIFY-CRED lane. The OAuth token lifecycle against the host's REAL
//! credential store — the flow nobody had driven.
//!
//! The committed suite proves the ladder with `InMemoryCredentialsStore`, and
//! `keyring_blob_size_live.rs` proves an OAuth-sized string survives a real
//! keyring. Neither drives the product's own token path — `OAuthStorage::store`
//! → `open_secure_ladder_store` → OS keyring → `OAuthStorage::load` in a fresh
//! instance — which is where a spanned value, its manifest, the readback gate,
//! the cleartext removal and the login record all have to agree.
//!
//! Synthetic tokens only: every value here is generated in this file.
//! `#[ignore]`d because it writes into the host's real credential store.

use wcore_agent::oauth::{OAuthStorage, OAuthTokens};

/// Build a token set of a realistic size (~4 KB — access JWT + id JWT +
//  refresh token), tagged so two generations are distinguishable without ever
/// printing either.
fn synthetic_tokens(tag: &str) -> OAuthTokens {
    let filler = tag.repeat(1400 / tag.len().max(1));
    OAuthTokens {
        access_token: format!("hdr.{filler}.sig"),
        refresh_token: Some(format!("rt-{filler}")),
        expires_at_unix_secs: Some(1_700_000_000),
        token_type: "Bearer".into(),
        scope: Some("openid profile".into()),
        id_token: Some(format!("idt.{filler}.sig")),
    }
}

fn assert_same(left: &OAuthTokens, right: &OAuthTokens, what: &str) {
    // Compared field by field, and reported by LENGTH and equality only — a
    // failing assert must never print token material.
    assert_eq!(
        left.access_token.len(),
        right.access_token.len(),
        "{what}: access_token length differs"
    );
    assert!(
        left.access_token == right.access_token,
        "{what}: access_token round-tripped with the right LENGTH but different bytes"
    );
    assert!(
        left.refresh_token == right.refresh_token,
        "{what}: refresh_token did not round-trip byte-identical"
    );
    assert!(
        left.id_token == right.id_token,
        "{what}: id_token did not round-trip byte-identical"
    );
    assert_eq!(
        left.expires_at_unix_secs, right.expires_at_unix_secs,
        "{what}: expiry did not round-trip"
    );
    assert_eq!(left.token_type, right.token_type, "{what}: token_type");
    assert_eq!(left.scope, right.scope, "{what}: scope");
}

/// A provider name of our own so this can never touch a developer's real
/// ChatGPT login.
const PROVIDER: &str = "verifycred-synthetic";

#[test]
#[ignore = "writes into the host's real credential store; run with -- --ignored"]
fn an_oauth_login_stores_rotates_and_reads_back_through_the_real_credential_store() {
    let legacy_root = std::env::temp_dir().join(format!(
        "wayland-verifycred-oauth-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let open = || {
        let secure = wcore_config::credentials::open_secure_ladder_store(
            &wcore_config::credentials::CredentialsStorageConfig::default(),
            &wcore_config::config::credentials_storage_path(),
        );
        OAuthStorage::at_root(legacy_root.clone(), secure).expect("open oauth storage")
    };

    // Leave nothing behind even if an assertion below fires.
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let secure = wcore_config::credentials::open_secure_ladder_store(
                &wcore_config::credentials::CredentialsStorageConfig::default(),
                &wcore_config::config::credentials_storage_path(),
            );
            if let Ok(storage) = OAuthStorage::at_root(self.0.clone(), secure) {
                let _ = storage.delete(PROVIDER);
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(legacy_root.clone());

    // Instrument self-test: is a secure rung even mounted here? If not, the
    // store below will refuse and this run measured nothing.
    let first = synthetic_tokens("P");
    let store_result = open().store(PROVIDER, &first);
    assert!(
        store_result.is_ok(),
        "NOT MEASURED — no secure credential rung is mounted on this host, so the OAuth \
         store path was never exercised: {:?}",
        store_result.err().map(|e| e.to_string())
    );
    println!(
        "MEASURED os={} token_json_len~{}",
        std::env::consts::OS,
        serde_json::to_string(&first).unwrap().len()
    );

    // 1. Read back in a FRESH instance — a new process's view of the store.
    let loaded = open()
        .load(PROVIDER)
        .expect("load must not error")
        .expect("a stored login must be readable");
    assert_same(&first, &loaded, "first store");

    // 2. No cleartext anywhere under the legacy root.
    let refresh = first.refresh_token.clone().unwrap();
    for entry in std::fs::read_dir(&legacy_root).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&refresh),
            "{} holds the refresh token in cleartext",
            path.display()
        );
    }

    // 3. Rotate — the case that exercises the generation flip against a live
    //    manifest, on the real backend.
    let second = synthetic_tokens("Q");
    open().store(PROVIDER, &second).expect("rotation must land");
    let rotated = open()
        .load(PROVIDER)
        .expect("load after rotation")
        .expect("still signed in after rotation");
    assert_same(&second, &rotated, "after rotation");
    assert!(
        rotated.refresh_token != first.refresh_token,
        "the rotated login still returns the PREVIOUS refresh token"
    );

    // 4. Rotate again, to a SHORTER value — fewer parts than the live manifest
    //    counts, which is where a torn rewrite would splice on an old tail.
    let third = OAuthTokens {
        access_token: "hdr.short.sig".into(),
        refresh_token: Some("rt-short".into()),
        ..synthetic_tokens("Z")
    };
    open().store(PROVIDER, &third).expect("shrink must land");
    let shrunk = open()
        .load(PROVIDER)
        .expect("load after shrink")
        .expect("still signed in after shrink");
    assert_same(&third, &shrunk, "after shrinking rotation");

    // 5. Logout clears everything, including the login record, so a later load
    //    is an honest "not signed in" rather than the refusal.
    assert!(
        open().delete(PROVIDER).expect("logout"),
        "removed something"
    );
    assert!(
        open().load(PROVIDER).expect("load after logout").is_none(),
        "a logged-out provider must read as not-signed-in, not as a refusal"
    );
    println!("PASS an_oauth_login_stores_rotates_and_reads_back_through_the_real_credential_store");
}
