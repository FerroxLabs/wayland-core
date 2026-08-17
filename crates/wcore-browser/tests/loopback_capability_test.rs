//! gh#911 — adversarial suite for the recoverable local-only loopback
//! capability, and gh#826's "the named setting must exist" guard.
//!
//! The capability reopens exactly one hard block. Every test below is an
//! attempt to spend a grant on something it was not issued for; each must
//! stay refused. The one positive arm proves the grant is not decorative —
//! without it the whole file would pass against a policy that simply blocks
//! everything, which is the vacuity trap for a fail-closed gate.
//!
//! Terminology: "granted" always means the fixture grant below — schema
//! version 1, scope `local-dev`, port 3000 and nothing else.

use std::net::{IpAddr, Ipv4Addr};

use wcore_browser::policy::{
    BrowserPolicy, LOOPBACK_CAPABILITY_VERSION, LoopbackCapability, PolicyAction, PolicyOutcome,
};

/// The grant an operator following the product's own remediation text writes.
fn grant() -> LoopbackCapability {
    LoopbackCapability {
        enabled: true,
        schema_version: LOOPBACK_CAPABILITY_VERSION,
        session_scope: "local-dev".into(),
        ports: vec![3000],
    }
}

/// Fail-closed baseline policy (`default_action = deny`, no origin lists)
/// carrying the fixture grant. Deliberately NOT `default_action = allow`: the
/// grant has to be sufficient authority on its own, because the recovery path
/// Desktop offers is a single action.
fn granted_policy() -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]).with_loopback(grant())
}

fn denied(policy: &BrowserPolicy, url: &str) -> String {
    match policy.evaluate(url) {
        PolicyOutcome::Deny { reason } => reason,
        other => panic!("{url} was NOT denied: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The one positive arm. If this fails, every negative arm below is vacuous.
// ---------------------------------------------------------------------------

#[test]
fn granted_port_is_reachable_on_every_loopback_spelling() {
    let policy = granted_policy();
    for url in [
        "http://localhost:3000/",
        "http://127.0.0.1:3000/",
        "http://127.0.0.53:3000/", // all of 127/8, not just .0.1
        "http://[::1]:3000/",
        "http://[::ffff:127.0.0.1]:3000/", // IPv4-mapped loopback
        "http://app.localhost:3000/",      // *.localhost
    ] {
        policy.check_url(url).unwrap_or_else(|e| {
            panic!(
                "an explicit grant for port 3000 did not admit {url}, so gh#911 has no \
                 working recovery path: {e}"
            )
        });
    }
}

#[test]
fn without_a_grant_loopback_is_still_blocked() {
    let policy = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]);
    let reason = denied(&policy, "http://localhost:3000/");
    assert!(
        reason.contains("loopback"),
        "denial should still name loopback: {reason}"
    );
    // Even a wide-open origin policy must not reach loopback unaided — the
    // capability is the only door.
    let wide = BrowserPolicy::new(PolicyAction::Allow, vec!["localhost".into()], vec![]);
    let reason = denied(&wide, "http://localhost:3000/");
    assert!(
        reason.contains("loopback"),
        "default_action=allow plus an allow-list must NOT substitute for the grant: {reason}"
    );
}

// ---------------------------------------------------------------------------
// A grant is port-scoped, not host-scoped.
// ---------------------------------------------------------------------------

#[test]
fn ungranted_port_on_a_granted_host_stays_blocked() {
    let policy = granted_policy();
    // 9377 is the Camoufox sidecar this crate drives. A localhost grant that
    // reached it would hand the agent its own browser control plane.
    for url in [
        "http://localhost:9377/",
        "http://127.0.0.1:8080/",
        "http://localhost/",       // port 80 by scheme default
        "https://localhost/",      // port 443 by scheme default
        "https://localhost:3000/", // right port, and it IS granted
    ] {
        let outcome = policy.evaluate(url);
        let is_allowed = matches!(outcome, PolicyOutcome::Allow);
        let expected = url == "https://localhost:3000/";
        assert_eq!(
            is_allowed, expected,
            "{url}: expected allowed={expected}, got {outcome:?}"
        );
    }
    let reason = denied(&policy, "http://localhost:9377/");
    assert!(
        reason.contains("9377") && reason.contains("[3000]"),
        "the refusal must name the port asked for AND the ports granted, or an \
         operator cannot tell what to change: {reason}"
    );
}

// ---------------------------------------------------------------------------
// The grant must not leak into any other blocked category. These are the
// gh#911 acceptance rows.
// ---------------------------------------------------------------------------

#[test]
fn grant_does_not_reach_private_link_local_metadata_or_ula() {
    let policy = granted_policy();
    // Every URL uses the GRANTED port, so a pass here would be the grant
    // leaking across categories rather than a port mismatch.
    for (url, want) in [
        ("http://10.0.0.1:3000/", "RFC 1918"),
        ("http://172.16.5.4:3000/", "RFC 1918"),
        ("http://192.168.1.1:3000/", "RFC 1918"),
        ("http://169.254.169.254:3000/", "cloud metadata"),
        ("http://169.254.1.1:3000/", "link-local"),
        ("http://[fd00::1]:3000/", "ULA"),
        ("http://[fe80::1]:3000/", "link-local"),
        ("http://100.64.0.1:3000/", "CGN"),
        ("http://0.0.0.0:3000/", "0.0.0.0/8"),
        ("http://[::ffff:169.254.169.254]:3000/", "cloud metadata"),
        ("http://[::ffff:10.0.0.1]:3000/", "RFC 1918"),
    ] {
        let reason = denied(&policy, url);
        assert!(
            reason.contains(want),
            "{url} was denied for the wrong reason — expected {want:?}, got {reason:?}. \
             A loopback grant must never be spendable on this category."
        );
    }
}

/// `0.0.0.0` routes to the local host on many stacks, so it is the obvious
/// way to spend a loopback grant on a host the grant never named. It is not a
/// loopback address and must keep its own refusal.
#[test]
fn grant_does_not_reach_the_unspecified_address() {
    let reason = denied(&granted_policy(), "http://0.0.0.0:3000/");
    assert!(
        !reason.contains("loopback capability"),
        "0.0.0.0 must not even be evaluated against the loopback grant: {reason}"
    );
}

/// The legacy IPv4 encodings exist to smuggle 127.0.0.1 past filters, so the
/// obvious expectation is that a grant must not honour them. Measured, that
/// expectation is unreachable: `Url::parse` canonicalizes every one of these
/// spellings to `127.0.0.1` before `evaluate` ever reads the host, so the
/// policy is judging the literal address the request will reach — which is the
/// address and port the operator granted.
///
/// This test pins that as the CONTRACT rather than quietly accepting it. What
/// matters for security is not the spelling but the destination: obfuscation
/// must not be able to move the request to a host the grant does not cover.
/// The two assertions below are therefore (a) these all denote granted
/// loopback and are admitted, and (b) the same spellings at an UNGRANTED port
/// are still refused, proving the canonicalized address is really being
/// matched against the grant rather than short-circuited by it.
#[test]
fn obfuscated_loopback_spellings_are_judged_as_the_address_they_denote() {
    let policy = granted_policy();
    for url in [
        "http://0177.0.0.1:3000/", // octal
        "http://0x7f.0.0.1:3000/", // hex octet
        "http://2130706433:3000/", // 32-bit decimal
        "http://0x7f000001:3000/", // 32-bit hex
        "http://127.0x1:3000/",    // two-octet form
        "http://127.1:3000/",      // short form
    ] {
        let parsed = url::Url::parse(url).unwrap();
        assert_eq!(
            parsed.host_str(),
            Some("127.0.0.1"),
            "{url}: this test's premise is that the URL parser canonicalizes the host \
             before the policy sees it; it no longer does, so the grant is now judging an \
             obfuscated spelling and this file must be rewritten"
        );
        policy.check_url(url).unwrap_or_else(|e| {
            panic!("{url} denotes granted 127.0.0.1:3000 and must be admitted: {e}")
        });
    }
    // (b) The destination, not the spelling, is what the grant is matched on.
    for url in [
        "http://0177.0.0.1:9377/",
        "http://2130706433:9377/",
        "http://127.1:9377/",
    ] {
        let reason = denied(&policy, url);
        assert!(
            reason.contains("9377"),
            "{url} resolves to a granted host on an UNGRANTED port and must be refused \
             on the port, proving the grant is really consulted: {reason}"
        );
    }
}

// ---------------------------------------------------------------------------
// Malformed / unknown capability data fails closed (gh#911 acceptance).
// ---------------------------------------------------------------------------

#[test]
fn every_malformed_grant_fails_closed() {
    let cases: Vec<(&str, LoopbackCapability, &str)> = vec![
        (
            "disabled",
            LoopbackCapability {
                enabled: false,
                ..grant()
            },
            "enabled is false",
        ),
        (
            "absent version",
            LoopbackCapability {
                schema_version: 0,
                ..grant()
            },
            "schema_version 0",
        ),
        (
            "future version",
            LoopbackCapability {
                schema_version: LOOPBACK_CAPABILITY_VERSION + 1,
                ..grant()
            },
            "is not the supported version",
        ),
        (
            "empty scope",
            LoopbackCapability {
                session_scope: String::new(),
                ..grant()
            },
            "session_scope is empty",
        ),
        (
            "whitespace scope",
            LoopbackCapability {
                session_scope: "   ".into(),
                ..grant()
            },
            "session_scope is empty",
        ),
        (
            "no ports",
            LoopbackCapability {
                ports: vec![],
                ..grant()
            },
            "ports is empty",
        ),
    ];
    for (label, cap, want) in cases {
        let policy = BrowserPolicy::new(PolicyAction::Allow, vec!["localhost".into()], vec![])
            .with_loopback(cap);
        let reason = denied(&policy, "http://localhost:3000/");
        assert!(
            reason.contains(want),
            "{label}: expected the refusal to name {want:?}, got {reason:?}"
        );
    }
}

#[test]
fn default_capability_grants_nothing() {
    let cap = LoopbackCapability::default();
    assert!(!cap.enabled);
    assert!(cap.authorize(Some(3000)).is_err());
    // And the policy's own default must carry that no-authority value.
    assert_eq!(
        BrowserPolicy::default().loopback,
        LoopbackCapability::default()
    );
}

/// Deserializing a grant from data that omits fields must not synthesize
/// authority — this is the "unknown producer data" path.
#[test]
fn deserialized_partial_grant_fails_closed() {
    let cap: LoopbackCapability = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert!(
        cap.authorize(Some(3000)).is_err(),
        "a grant that only says enabled=true must not authorize anything"
    );
    let cap: LoopbackCapability =
        serde_json::from_str(r#"{"enabled":true,"schema_version":1,"session_scope":"s"}"#).unwrap();
    assert!(
        cap.authorize(Some(3000)).is_err(),
        "a grant with no ports must not authorize anything"
    );
}

// ---------------------------------------------------------------------------
// Interaction with the rest of the gate.
// ---------------------------------------------------------------------------

#[test]
fn denied_origins_beats_an_authorized_grant() {
    let policy = BrowserPolicy::new(PolicyAction::Deny, vec![], vec!["localhost".into()])
        .with_loopback(grant());
    let reason = denied(&policy, "http://localhost:3000/");
    assert!(
        reason.contains("denied pattern"),
        "the deny list must be unconditional: {reason}"
    );
}

#[test]
fn grant_does_not_widen_anything_off_loopback() {
    let policy = granted_policy();
    // Public origin, granted port, fail-closed default: still denied by the
    // default action. The grant is not a general allow.
    let reason = denied(&policy, "http://example.com:3000/");
    assert!(
        reason.contains("default_action=Deny"),
        "a loopback grant must not affect public origins: {reason}"
    );
}

#[test]
fn non_http_schemes_still_refused_on_granted_loopback() {
    let policy = granted_policy();
    for url in [
        "file://localhost/etc/passwd",
        "ftp://localhost:3000/",
        "ws://localhost:3000/",
    ] {
        let reason = denied(&policy, url);
        assert!(
            reason.contains("not in allow list"),
            "{url} must be refused at the scheme gate, before the grant: {reason}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rebinding: the two ways a grant could be turned into a public-name attack.
// ---------------------------------------------------------------------------

/// A public hostname that resolves to 127.0.0.1 is a rebinding attack whether
/// or not a loopback grant exists. The grant authorizes loopback NAMES the
/// operator wrote, never arbitrary names that land there.
#[test]
fn grant_does_not_relax_the_resolved_ip_check() {
    let policy = granted_policy();
    let outcome =
        policy.check_resolved_host("evil.example", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    match outcome {
        PolicyOutcome::Deny { reason } => assert!(
            reason.contains("blocked IP"),
            "expected a resolved-IP refusal: {reason}"
        ),
        other => panic!(
            "a public hostname resolving to loopback was accepted with a loopback grant \
             in hand: {other:?}"
        ),
    }
}

/// The redirect hop policy is built from a cloned snapshot. If the grant is
/// not carried into it the snapshot silently disagrees with the parent gate —
/// so assert the snapshot decides identically on all three interesting URLs.
#[test]
fn redirect_snapshot_carries_the_grant_verbatim() {
    let policy = granted_policy();
    // `reqwest_redirect_policy` consumes the snapshot opaquely, so drive the
    // observable equivalent: a policy round-tripped through serde must decide
    // the same way. This is the same field-copy the snapshot performs.
    let json = serde_json::to_string(&policy).unwrap();
    let restored: BrowserPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(
        restored.loopback, policy.loopback,
        "the grant did not survive a policy copy; the redirect-hop snapshot \
         performs the same copy and would silently disagree with the parent gate"
    );
    for url in [
        "http://localhost:3000/",
        "http://localhost:9377/",
        "http://169.254.169.254:3000/",
    ] {
        assert_eq!(
            matches!(restored.evaluate(url), PolicyOutcome::Allow),
            matches!(policy.evaluate(url), PolicyOutcome::Allow),
            "copied policy disagrees with the original on {url}"
        );
    }
    // And a redirect policy can actually be built from a granted policy.
    let _ = policy.reqwest_redirect_policy();
}

// ---------------------------------------------------------------------------
// The scope is reported, so a consumer can say which target authorized it.
// ---------------------------------------------------------------------------

#[test]
fn authorize_reports_the_scope_that_granted_access() {
    assert_eq!(grant().authorize(Some(3000)).unwrap(), "local-dev");
    // Trimmed, so a scope written with stray whitespace reports cleanly
    // rather than being reported with quotes-shifting padding.
    let padded = LoopbackCapability {
        session_scope: "  chat-42  ".into(),
        ..grant()
    };
    assert_eq!(padded.authorize(Some(3000)).unwrap(), "chat-42");
}

#[test]
fn authorize_refuses_a_url_with_no_resolvable_port() {
    // Belt-and-braces: `port_or_known_default` returns None for schemes with
    // no default. The gate must refuse rather than treat None as a wildcard.
    assert!(grant().authorize(None).is_err());
}
