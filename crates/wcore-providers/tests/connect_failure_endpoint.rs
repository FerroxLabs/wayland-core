//! A connect failure must name the endpoint it could not reach.
//!
//! Issue #1077: pointing a provider at a dead `base_url` rendered
//! `Connection error: error sending request: Connection refused (os error 111)`
//! — the same sentence for a wrong port, a wrong host and a dropped packet,
//! and it never said WHICH endpoint. The one thing the operator has to know to
//! fix a `base_url` typo is the one thing the message omitted.
//!
//! Live sends against loopback, not fixtures: the rendering under test is
//! produced by `reqwest` plus this crate's error mapping, and a fixture would
//! grade this crate's idea of that rendering rather than the rendering.

use wcore_egress::EgressClient;
use wcore_providers::retry::provider_failure_code;
use wcore_providers::{ProviderError, retry};

/// Send through the same client and the same mapping the provider path uses,
/// and return the message the user is shown.
async fn connection_message_for(url: &str) -> String {
    let error = EgressClient::new()
        .post(url)
        .send()
        .await
        .expect_err("this endpoint cannot be reached; that is the point");
    match retry::provider_error_from_egress(error) {
        ProviderError::Connection(message) => message,
        other => panic!("a connect failure must stay a retryable Connection; got {other:?}"),
    }
}

/// The reported case: a port with nothing listening.
#[tokio::test(flavor = "current_thread")]
async fn a_refused_connection_names_the_host_and_port_it_was_refused_by() {
    let message = connection_message_for("http://127.0.0.1:1/v1/chat/completions").await;
    assert!(
        message.contains("127.0.0.1:1"),
        "a refusal must name the endpoint nothing was listening on, so the \
            operator can see the typo; got: {message}"
    );
    // The cause must survive alongside the endpoint — the endpoint says WHERE,
    // the OS error says WHAT, and dropping either one re-opens #1077.
    //
    // Asserted on the MEANING, not on Unix's phrasing. The two platforms share
    // no sentence: Unix renders `Connection refused (os error 111)`, Windows 11
    // 26200 renders `No connection could be made because the target machine
    // actively refused it. (os error 10061)`. What they do share is the word
    // the OS uses for the event, and the class the shipped classifier puts it
    // in — `classify_connect_chain` already matches both spellings, so the
    // product was correct here and only this assertion was not.
    //
    // Note what is NOT weakened. The endpoint assertion above is untouched, so
    // this test still fails the moment the host and port stop being named. And
    // the reason is asserted on the message with the endpoint text REMOVED, so
    // it cannot be satisfied by the suffix #1077 added — the cause has to come
    // from the OS.
    let without_endpoint = message.replace("127.0.0.1:1", "");
    assert!(
        without_endpoint.to_ascii_lowercase().contains("refused"),
        "naming the endpoint must not displace the reason; got: {message}"
    );
    assert_eq!(
        provider_failure_code(&ProviderError::Connection(message.clone())),
        "connection_refused",
        "the reason must still classify as a refusal on this platform; got: {message}"
    );
}

/// The other half of the ticket's table: a name that does not resolve.
#[tokio::test(flavor = "current_thread")]
async fn an_unresolvable_host_names_the_host_and_port_it_could_not_resolve() {
    let message =
        connection_message_for("https://unreachable.invalid.localdomain:9999/v1/messages").await;
    assert!(
        message.contains("unreachable.invalid.localdomain:9999"),
        "an unresolvable base_url must name the host it tried; got: {message}"
    );
}

/// A scheme with no explicit port still names the port that was dialled.
///
/// `https://host/v1` dials 443, and an operator reading a bare host name
/// cannot tell which port the client actually used.
///
/// Driven through an unresolvable NAME rather than a closed port: a loopback
/// port cannot be held closed on a shared build host (this box answers :80),
/// and an ambient listener would fail this test for a reason that has nothing
/// to do with the rendering under test.
#[tokio::test(flavor = "current_thread")]
async fn an_implicit_port_is_named_explicitly() {
    let message =
        connection_message_for("https://unreachable.invalid.localdomain/v1/messages").await;
    assert!(
        message.contains("unreachable.invalid.localdomain:443"),
        "the dialled port must be shown even when the URL left it implicit; got: {message}"
    );
}

/// NEGATIVE CONTROL — H-2 / secrets-26.
///
/// `provider_error_from_reqwest` calls `without_url()` precisely because a
/// provider may carry a credential in the URL (Gemini's old `?key=` form).
/// Only the authority's host and port may be added back: they cannot hold a
/// secret, and everything that can — userinfo, path, query, fragment — must
/// stay out. This test fails if the endpoint is ever widened to the URL.
#[tokio::test(flavor = "current_thread")]
async fn naming_the_endpoint_must_not_reintroduce_the_url() {
    let message =
        connection_message_for("http://user:hunter2@127.0.0.1:1/v1/messages?key=sk-SECRET-1077")
            .await;
    for leaked in [
        "sk-SECRET-1077",
        "hunter2",
        "user",
        "/v1/messages",
        "key=",
        "http://",
    ] {
        assert!(
            !message.contains(leaked),
            "the URL must not come back with the endpoint: {leaked:?} leaked into: {message}"
        );
    }
    // Known-positive control for the assertions above: this probe DOES reach
    // the endpoint-naming path, so the six absences are absences of a leak and
    // not of a message.
    assert!(
        message.contains("127.0.0.1:1"),
        "control: this probe must be on the endpoint-naming path; got: {message}"
    );
}

/// NEGATIVE CONTROL — the added text must not be read as a cause.
///
/// `provider_failure_code` classifies `ProviderError::Connection` by matching
/// substrings of its rendered message, so text appended to that message is
/// input to the classifier. Host and port are attacker-adjacent (a `base_url`
/// is user-supplied) and must not be able to move a failure between classes.
#[tokio::test(flavor = "current_thread")]
async fn the_endpoint_text_cannot_change_the_failure_class() {
    let refused = EgressClient::new()
        .post("http://127.0.0.1:1/v1/chat/completions")
        .send()
        .await
        .expect_err("nothing listens on port 1");
    assert_eq!(
        provider_failure_code(&retry::provider_error_from_egress(refused)),
        "connection_refused",
        "the endpoint suffix must not move a refusal out of its own class"
    );
}
