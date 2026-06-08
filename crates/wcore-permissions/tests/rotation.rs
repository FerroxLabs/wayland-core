//! M5.9 — rotation contract: rotating a bearer token under a new secret
//! produces a token that verifies under the new secret only; the original
//! token remains valid for its grace period under the old secret.

use wcore_permissions::{Actor, BearerToken};

#[test]
fn rotated_token_verifies_old_secret_fails_new_token_under_new_secret() {
    let old_secret = b"old-secret-32-bytes-padded-aaaaa";
    let new_secret = b"new-secret-32-bytes-padded-bbbbb";
    let actor = Actor::User("alice".into());
    let token = BearerToken::issue(actor.clone(), 60_000, old_secret);

    let rotated = token.rotate(new_secret).expect("rotate");
    assert!(
        token.verify(old_secret).is_ok(),
        "original token still good for grace period"
    );
    assert!(
        rotated.verify(new_secret).is_ok(),
        "rotated token good under new secret"
    );
    assert!(
        rotated.verify(old_secret).is_err(),
        "rotated token must NOT verify under old secret"
    );
}
