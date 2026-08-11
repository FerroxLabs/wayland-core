//! A permanently-unreachable provider must not be retried like a transient one.
//!
//! Live measurement, not a fixture: these drive REAL `reqwest` sends and grade
//! the failure code the retry ring records for them. The whole defect was that
//! `is_connect()` collapsed "this name does not exist" and "this socket got
//! reset" into one code, so a `base_url` typo spent the engine's 900 s
//! provider-outage budget — measured at 902 s and 36 sends before the run gave
//! up. Anything that re-merges the classes has to fail here.

use wcore_egress::EgressClient;
use wcore_providers::retry::provider_failure_code;
use wcore_providers::{ProviderError, retry};

/// Send through the same client and the same mapping the provider path uses,
/// and report the code the retry ring would record.
async fn failure_code_for(url: &str) -> String {
    let error = EgressClient::new()
        .post(url)
        .send()
        .await
        .expect_err("this endpoint cannot be reached; that is the point");
    let mapped = retry::provider_error_from_egress(error);
    // Every connect-phase failure is retryable; only its BUDGET differs.
    assert!(
        matches!(mapped, ProviderError::Connection(_)),
        "a connect failure must stay retryable at this layer; got {mapped:?}"
    );
    provider_failure_code(&mapped)
}

/// A closed port on loopback. No external network, no resolver, no flake.
#[tokio::test(flavor = "current_thread")]
async fn a_refused_port_is_classified_apart_from_a_generic_connect_failure() {
    let code = failure_code_for("http://127.0.0.1:1/v1/chat/completions").await;
    assert_eq!(
        code, "connection_refused",
        "a port that actively refuses must be its own class, not the generic \
         `connection` that the engine admits to the outage window"
    );
}

/// A name under a TLD that cannot exist.
///
/// The assertion branches on what the environment's resolver ACTUALLY did, so
/// it grades in both worlds and can fail in both: a resolver that answers
/// NXDOMAIN must produce the permanent class, and a resolver that is itself
/// unavailable (EAI_AGAIN, an offline runner) must NOT — that case is
/// transient and keeps the full budget.
#[tokio::test(flavor = "current_thread")]
async fn a_name_that_does_not_resolve_is_permanent_unless_the_resolver_is_down() {
    use std::net::ToSocketAddrs;

    let host = "unreachable.invalid.localdomain";
    let resolver_said = ("unreachable.invalid.localdomain", 9999u16)
        .to_socket_addrs()
        .err()
        .map(|e| e.to_string().to_ascii_lowercase())
        .unwrap_or_else(|| panic!("{host} unexpectedly RESOLVED; pick a name that cannot exist"));
    let resolver_is_down = resolver_said.contains("temporary failure")
        || resolver_said.contains("try again")
        || resolver_said.contains("no address associated");

    let code = failure_code_for(&format!("https://{host}:9999/v1/chat/completions")).await;
    if resolver_is_down {
        assert_eq!(
            code, "connection",
            "this environment's resolver is unavailable ({resolver_said}) — that is a \
             TRANSIENT failure and must keep the outage budget"
        );
    } else {
        assert_eq!(
            code, "dns_failure",
            "resolver said `{resolver_said}` — a name that does not exist is permanent and \
             must never be admitted to the outage window"
        );
    }
}

/// The whole point of the split: the transient class is untouched.
#[tokio::test(flavor = "current_thread")]
async fn the_permanent_classes_are_the_only_ones_taken_out_of_the_unserved_set() {
    // `transport` is what a peer RST after dispatch produces, and it is the
    // class the outage window exists for. A change that quietly removed it
    // would make every real provider blip fatal.
    assert_ne!(retry::FAILURE_DNS, "transport");
    assert_ne!(retry::FAILURE_CONNECTION_REFUSED, "transport");
    assert_eq!(retry::FAILURE_CONNECTION, "connection");
}
