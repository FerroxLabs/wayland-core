//! Integration tests for the public `BearerToken` contract.

use wcore_permissions::{Actor, BearerToken, DenyReason};

#[test]
fn issued_token_verifies_within_ttl() {
    let secret = b"shared-secret-v0.3";
    let tok = BearerToken::issue(Actor::User("alice".into()), 60_000, secret);
    let got = tok.verify(secret).unwrap();
    assert!(matches!(got, Actor::User(name) if name == "alice"));
}

#[test]
fn issued_token_for_agent_verifies() {
    let secret = b"s";
    let tok = BearerToken::issue(Actor::Agent("planner".into()), 60_000, secret);
    let got = tok.verify(secret).unwrap();
    assert!(matches!(got, Actor::Agent(name) if name == "planner"));
}

#[test]
fn wrong_secret_rejects_with_token_invalid() {
    let tok = BearerToken::issue(Actor::User("alice".into()), 60_000, b"a");
    let res = tok.verify(b"b");
    assert_eq!(res.unwrap_err(), DenyReason::TokenInvalid);
}

#[test]
fn expired_token_rejects_with_token_expired() {
    let mut tok = BearerToken::issue(Actor::User("alice".into()), 60_000, b"s");
    // Force expiry into the past. Expired check runs before signature check,
    // so the signature staying valid for the original `expires_at_ms` is
    // irrelevant — `verify` short-circuits on the clock.
    tok.expires_at_ms = 0;
    let res = tok.verify(b"s");
    assert_eq!(res.unwrap_err(), DenyReason::TokenExpired);
}

#[test]
fn tampered_actor_rejects_with_token_invalid() {
    let mut tok = BearerToken::issue(Actor::User("alice".into()), 60_000, b"s");
    tok.actor = Actor::User("mallory".into());
    let res = tok.verify(b"s");
    assert_eq!(res.unwrap_err(), DenyReason::TokenInvalid);
}

#[test]
fn tampered_expiry_rejects_with_token_invalid() {
    let mut tok = BearerToken::issue(Actor::User("alice".into()), 60_000, b"s");
    tok.expires_at_ms = tok.expires_at_ms.saturating_add(10_000_000);
    let res = tok.verify(b"s");
    assert_eq!(res.unwrap_err(), DenyReason::TokenInvalid);
}

#[test]
fn token_serializes_round_trip_via_json() {
    let secret = b"s";
    let tok = BearerToken::issue(Actor::User("alice".into()), 60_000, secret);
    let wire = serde_json::to_string(&tok).expect("serialize");
    let back: BearerToken = serde_json::from_str(&wire).expect("deserialize");
    let got = back.verify(secret).unwrap();
    assert!(matches!(got, Actor::User(name) if name == "alice"));
}
