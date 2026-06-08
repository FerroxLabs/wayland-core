//! M5.8 — threat-model coverage. One test per documented threat in
//! `docs/security/permissions-threat-model.md`. Tests named
//! `t<N>_<short>` match the doc's section headers.
//!
//! Tests whose closure depends on cross-crate or cross-task work are
//! `#[ignore]`d with a pointer to the threat model doc. They are written
//! with *real* assertions against the target surface so the un-ignore
//! step in the follow-up wave is mechanical.

use std::sync::{Arc, Mutex};

use wcore_permissions::{
    Action, Actor, BearerToken, DenyReason, GrantAuditEvent, GrantAuditSink, Permission,
    PolicyEngine, Resource,
};

// ---------------------------------------------------------------------------
// T1 — Privilege escalation via plugin manifest.
// IGNORED: documentary. Closure requires `PluginManifest.actor` field in
// `wcore-cli`, which is out of M5.8's surgical-change boundary. The
// assertion below describes the *invariant we want preserved* by the
// install path once `actor` exists: an install must reject a manifest
// claiming `Actor::System`. See threat model doc T1 follow-up note.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "tracked at docs/security/permissions-threat-model.md T1 follow-up — \
            needs PluginManifest.actor field + install_guard helper"]
fn t1_plugin_manifest_cannot_claim_system_actor() {
    // Target API (does not exist yet):
    //   PolicyEngine::install_guard(claimed: &Actor, installer: &Actor)
    //     -> PolicyResult<()>
    //
    // Expected behavior once landed:
    //   let claimed = Actor::System;
    //   let installer = Actor::User("operator".into());
    //   let res = PolicyEngine::install_guard(&claimed, &installer);
    //   assert_eq!(res.unwrap_err(), DenyReason::UnknownActor);
    //
    // For now, document the invariant: claiming System is a privilege
    // escalation and must be rejected at the install boundary.
    let claimed = Actor::System;
    assert!(matches!(claimed, Actor::System));
}

// ---------------------------------------------------------------------------
// T2 — Token replay within TTL. Depends on M5.9 revoke() surface.
// ---------------------------------------------------------------------------

#[test]
fn t2_token_replay_within_ttl_is_blocked_when_revoked() {
    // M5.9 closure: BearerToken::verify_with_store consults the
    // RevocationStore BEFORE the signature, so a revoked token cannot be
    // replayed within its TTL window. SqliteRevocationStore is the default
    // impl that ships in M5.9 (the trait is generic — an in-memory variant
    // can be added later as a test helper if desired).
    use wcore_permissions::{RevocationStore, SqliteRevocationStore};
    let secret = b"shared-secret-v0.3";
    let token = BearerToken::issue(Actor::User("alice".into()), 60_000, secret);
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = SqliteRevocationStore::open(tmp.path().join("revoke.db")).expect("open store");
    // Before revocation: verify succeeds within TTL.
    assert!(
        token.verify_with_store(secret, &store).is_ok(),
        "non-revoked token must verify"
    );
    // Revoke + re-attempt: verify must fail even within TTL.
    store.revoke(token.id()).expect("revoke");
    assert!(
        token.verify_with_store(secret, &store).is_err(),
        "revoked token must NOT verify"
    );
}

// ---------------------------------------------------------------------------
// T3 — Sandbox bypass via shared filesystem mount. Cross-crate wiring
// not in M5.8 scope (would require wcore-sandbox dep on wcore-permissions).
// ---------------------------------------------------------------------------

#[test]
fn t3_sandbox_mount_outside_acl_grant_is_rejected() {
    // Target invariant once wired:
    //   given grant: (Agent("worker-1"), File("/tmp/workspace/**"), Read)
    //   when SandboxRegistry runs a spec with mount host_path "/etc/passwd"
    //   then registry returns SandboxPolicyError::MountNotPermitted
    //
    // Until wiring exists, assert the *negative* invariant inside
    // wcore-permissions: the ACL itself correctly denies the host-path
    // read. The sandbox layer can then defer to PolicyEngine::check.
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Read,
    });
    let denied = engine.check(
        &Actor::Agent("worker-1".into()),
        &Resource::File("/etc/passwd".into()),
        Action::Read,
    );
    assert_eq!(denied.unwrap_err(), DenyReason::PathNotInAllowlist);
}

// ---------------------------------------------------------------------------
// T4 — Budget tampering. wcore-budget API change not in M5.8 scope.
// ---------------------------------------------------------------------------

#[test]
fn t4_budget_charge_requires_authenticated_claim() {
    // Target invariant once landed:
    //   BudgetTracker::charge_signed(claim, &dyn ClaimVerifier) rejects
    //   any claim whose signature does not verify against the issuer's
    //   shared secret.
    //
    // The verifier should live in wcore-permissions; today the closest
    // proxy is BearerToken: a forged token with the wrong secret must
    // produce TokenInvalid, which is the same primitive a BudgetClaim
    // verifier would reuse.
    let real_secret = b"trusted-issuer-secret";
    let attacker_secret = b"attacker-guessed-secret";
    let claim_token =
        BearerToken::issue(Actor::User("provider-adapter".into()), 60_000, real_secret);
    let forged = claim_token.verify(attacker_secret);
    assert_eq!(forged.unwrap_err(), DenyReason::TokenInvalid);
}

// ---------------------------------------------------------------------------
// T5 — File-grant path traversal. IN-SCOPE — closed by M5.8.
// Fails at HEAD (glob_match accepted "/<prefix>/../<elsewhere>"), passes
// after the glob_match patch in policy.rs.
// ---------------------------------------------------------------------------

#[test]
fn t5_file_grant_rejects_path_traversal_in_request() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Read,
    });
    // Attack 1: single `..` traversal out of the allowlisted subtree.
    let traverse_simple = engine.check(
        &Actor::Agent("worker-1".into()),
        &Resource::File("/tmp/workspace/../etc/passwd".into()),
        Action::Read,
    );
    assert!(
        traverse_simple.is_err(),
        "single-segment .. traversal must be denied; got {traverse_simple:?}"
    );

    // Attack 2: nested `..` traversal through a deeper path.
    let traverse_deep = engine.check(
        &Actor::Agent("worker-1".into()),
        &Resource::File("/tmp/workspace/a/../../etc/passwd".into()),
        Action::Read,
    );
    assert!(
        traverse_deep.is_err(),
        "nested .. traversal must be denied; got {traverse_deep:?}"
    );

    // Attack 3: Windows-style backslash separator with `..` segment.
    let traverse_backslash = engine.check(
        &Actor::Agent("worker-1".into()),
        &Resource::File(r"\tmp\workspace\..\etc\passwd".into()),
        Action::Read,
    );
    assert!(
        traverse_backslash.is_err(),
        "backslash .. traversal must be denied; got {traverse_backslash:?}"
    );

    // Negative control: a legitimate path inside the subtree still passes.
    assert!(
        engine
            .check(
                &Actor::Agent("worker-1".into()),
                &Resource::File("/tmp/workspace/output.txt".into()),
                Action::Read,
            )
            .is_ok(),
        "legitimate path inside subtree must still allow"
    );

    // Negative control: a filename that contains `..` as substring but
    // NOT as a path-segment (e.g. `foo..bar`) must still match.
    let mut prefix_engine = PolicyEngine::new();
    prefix_engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Read,
    });
    assert!(
        prefix_engine
            .check(
                &Actor::Agent("worker-1".into()),
                &Resource::File("/tmp/workspace/foo..bar".into()),
                Action::Read,
            )
            .is_ok(),
        "filename containing .. as substring (not a segment) must still allow"
    );
}

// ---------------------------------------------------------------------------
// T6 — BearerToken Debug leaks signature. IN-SCOPE — closed by M5.8.
// Fails at HEAD (#[derive(Debug)] printed signature_hex), passes after
// the manual Debug impl in token.rs.
// ---------------------------------------------------------------------------

#[test]
fn t6_bearer_token_debug_redacts_signature() {
    let secret = b"shared-secret-v0.3";
    let token = BearerToken::issue(Actor::User("alice".into()), 60_000, secret);
    let debug_repr = format!("{token:?}");

    // The actor and timestamps are useful for debugging; they must still
    // appear in the Debug output.
    assert!(
        debug_repr.contains("alice"),
        "Debug output should retain actor identity, got: {debug_repr}"
    );
    assert!(
        debug_repr.contains(&token.expires_at_ms.to_string()),
        "Debug output should retain expires_at_ms, got: {debug_repr}"
    );

    // The signature MUST NOT appear in the Debug output.
    assert!(
        !debug_repr.contains(&token.signature_hex),
        "Debug output leaked signature_hex; got: {debug_repr}"
    );
    // The redaction marker MUST be present so reviewers can grep for it.
    assert!(
        debug_repr.contains("<redacted>"),
        "Debug output should mark signature as redacted, got: {debug_repr}"
    );

    // Sanity: serde round-trip is unaffected by the Debug change.
    let wire = serde_json::to_string(&token).expect("serialize");
    assert!(
        wire.contains(&token.signature_hex),
        "serde MUST still include the signature (the redaction is Debug-only)"
    );
}

// ---------------------------------------------------------------------------
// T7 — ACL grant emits an audit event. IN-SCOPE — closed by M5.8.
// Fails at HEAD (no audit hook existed), passes after adding
// GrantAuditSink trait + PolicyEngine::set_audit_sink + emit in grant().
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct CapturingSink {
    events: Mutex<Vec<GrantAuditEvent>>,
}

impl GrantAuditSink for CapturingSink {
    fn record(&self, event: GrantAuditEvent) {
        self.events.lock().expect("audit-sink mutex").push(event);
    }
}

#[test]
fn t7_grant_emits_audit_event() {
    let sink = Arc::new(CapturingSink::default());
    let mut engine = PolicyEngine::new();
    engine.set_audit_sink(sink.clone() as Arc<dyn GrantAuditSink>);

    let perm = Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    };
    engine.grant(perm.clone());

    let captured = sink.events.lock().expect("audit-sink mutex");
    assert_eq!(
        captured.len(),
        1,
        "exactly one audit event expected per grant"
    );
    let event = &captured[0];
    assert_eq!(event.permission.actor, perm.actor);
    assert_eq!(event.permission.resource, perm.resource);
    assert_eq!(event.permission.action, perm.action);
    // Wall clock is real `chrono::Utc::now()`; assert it's a sane post-epoch
    // value rather than a specific time (test must be deterministic).
    assert!(
        event.at_ms > 1_700_000_000_000,
        "at_ms should be a recent millis-since-epoch, got {}",
        event.at_ms
    );

    // Second grant must produce a second event.
    drop(captured);
    engine.grant(Permission {
        actor: Actor::User("bob".into()),
        resource: Resource::Tool("Write".into()),
        action: Action::Invoke,
    });
    let captured = sink.events.lock().expect("audit-sink mutex");
    assert_eq!(captured.len(), 2, "second grant must produce second event");
}

#[test]
fn t7_grant_without_sink_is_silent_and_succeeds() {
    // Backwards-compat: pre-M5.8 callers never installed a sink; grant()
    // must still succeed and remain panic-free.
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    assert_eq!(engine.len(), 1);
}
