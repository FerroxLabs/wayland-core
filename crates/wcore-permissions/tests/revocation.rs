//! M5.9 — revocation contract: revoking a token id makes subsequent
//! `verify_with_store` calls fail even though the signature is still valid.
//!
//! Gated behind `sqlite-revocation` (default-on) because the test instantiates
//! the bundled `SqliteRevocationStore`. Embedders that disable the feature
//! exercise their own backing via the `RevocationStore` trait directly.

#![cfg(feature = "sqlite-revocation")]

use tempfile::tempdir;
use wcore_permissions::{Actor, BearerToken, RevocationStore, SqliteRevocationStore};

#[test]
fn revoked_token_fails_verify() {
    let secret = b"secret-32-bytes-padded-aaaaaaaa";
    let actor = Actor::User("bob".into());
    let token = BearerToken::issue(actor, 60_000, secret);
    let dir = tempdir().unwrap();
    let store = SqliteRevocationStore::open(dir.path().join("revoke.db")).unwrap();
    assert!(token.verify_with_store(secret, &store).is_ok());
    store.revoke(token.id()).unwrap();
    assert!(token.verify_with_store(secret, &store).is_err());
}
