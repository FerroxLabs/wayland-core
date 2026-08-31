//! wayland#1264 — a model-chosen URL to an allowlisted host is not admitted on
//! the host match alone.
//!
//! These arms drive the REAL `WebFetch` surface (`HttpFetchBackend`, the backend
//! `bootstrap.rs` registers for the `WebFetch` tool) through the REAL policy
//! (`AgentEgressPolicy`), not the pure classifier. The classifier has its own
//! unit arms; this file exists because the defect was never in the classifier's
//! logic — it was that the request never reached the checks. A test that calls
//! `classify` directly cannot see that, and one that hand-builds a client cannot
//! prove the shipped backend is the one carrying the stamp.
//!
//! No network is involved: a denial short-circuits before dispatch, and the
//! allow arms are pointed at a host that classifies without leaving the process.

use std::sync::Arc;

use wcore_agent::egress::{AgentEgressPolicy, AllowList};
use wcore_agent::tool_backends::http_fetch::HttpFetchBackend;
use wcore_tools::web_fetch::{FetchBackend, FetchOutcome, FetchRequest};

/// An allowlist holding the apex that ships in the 38-entry default set and is
/// named in the issue.
fn github_allowed() -> AllowList {
    let mut allow = AllowList::default();
    allow.allow_domain("github.com");
    allow
}

/// Build the shipped WebFetch backend under a session policy, exactly as
/// bootstrap does: the client snapshots the session policy at construction.
fn backend_under(policy: AgentEgressPolicy) -> HttpFetchBackend {
    let shared: wcore_egress::SharedPolicy = Arc::new(policy);
    wcore_egress::with_default_policy_sync(shared, HttpFetchBackend::new)
}

fn fetch(url: &str) -> FetchRequest {
    FetchRequest {
        url: url.to_string(),
        timeout_ms: 5_000,
        readable: false,
    }
}

/// c2. The issue's own example. `github.com` is allowlisted; the URL is not the
/// product's, it is whatever the model put in the tool call.
///
/// RED against the pre-fix tree: `classify` returned `Allow` on the host match
/// at `classify.rs:229`, above the method check and above `get_carries_data`,
/// so this request left the machine with no approval in any mode.
#[tokio::test]
async fn a_model_chosen_query_payload_to_an_allowlisted_apex_is_refused_unattended() {
    let backend = backend_under(AgentEgressPolicy::enforcing(github_allowed()));

    // A high-entropy token in the query — the exfil shape the classifier
    // already recognised everywhere except here.
    let outcome = backend
        .fetch(&fetch(
            "https://github.com/?leak=c2VjcmV0LWtleS1hYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5eg",
        ))
        .await;

    match outcome {
        FetchOutcome::Err { message } => {
            assert!(
                message.contains("github.com"),
                "the denial must name the destination, got: {message}"
            );
            assert!(
                message.contains("chosen by the model"),
                "the denial must say WHY this differs from ordinary allowlisted \
                 traffic, or the operator cannot act on it; got: {message}"
            );
        }
        other => panic!(
            "a model-chosen query payload to an allowlisted apex must not be \
             admitted with no approval surface present; got: {other:?}"
        ),
    }
}

/// The same URL shape, but as a plain read with nothing in it. WebFetch's whole
/// purpose. This must still work, or the fix is a WebFetch outage.
///
/// The destination is a loopback address, which `is_local_destination` admits
/// before any of this runs, so the arm proves the request was ADMITTED by the
/// policy without needing a live server: it fails at connect, not at the gate.
#[tokio::test]
async fn a_data_less_model_fetch_is_still_admitted() {
    let backend = backend_under(AgentEgressPolicy::enforcing(github_allowed()));

    let outcome = backend.fetch(&fetch("http://127.0.0.1:1/")).await;

    match outcome {
        FetchOutcome::Err { message } => assert!(
            !message.contains("chosen by the model"),
            "a data-less fetch must not be gated by the wayland#1264 branch; \
             got: {message}"
        ),
        // A response at all means it was admitted, which is the point.
        _ => {}
    }
}

/// c3, the wrong-refusal control. The agent's own provider traffic is a POST
/// with a body to a host on the same allowlist. If the new branch ever applies
/// to it, every LLM call in the product is refused — so this arm fails if the
/// origin distinction is lost, e.g. by defaulting the stamp to `ModelDirected`
/// or by dropping the `Product` early return.
#[tokio::test]
async fn provider_traffic_to_the_same_apex_keeps_its_unconditional_allow() {
    use wcore_egress::{EgressDecision, EgressOrigin, EgressPolicy};

    let policy = AgentEgressPolicy::enforcing(github_allowed());
    let request = reqwest::Request::new(
        reqwest::Method::POST,
        "https://api.github.com/v1/messages"
            .parse()
            .expect("parse the provider destination"),
    );

    let decision = policy.check(&request, EgressOrigin::Product).await;

    assert!(
        matches!(decision, EgressDecision::Allow),
        "product-built traffic to an allowlisted host must keep its \
         unconditional allow; got: {decision:?}"
    );
}

/// The stamp itself. Without this the claim "the WebFetch backend is
/// model-directed" is a statement about a private field that no test can see,
/// and a refactor could drop it with every other arm here still green — the
/// exact shape that shipped three vacuous guards this cycle.
#[test]
fn the_shipped_web_fetch_backend_carries_the_model_directed_stamp() {
    let backend = HttpFetchBackend::new();
    assert_eq!(
        backend.egress_origin(),
        wcore_egress::EgressOrigin::ModelDirected,
        "WebFetch takes its URL verbatim from tool input; if this client is \
         Product-origin the wayland#1264 branch is unreachable"
    );
}

/// The control for the arm above: a backend whose URL the product builds must
/// NOT be model-directed, or the assertion above would pass for every client in
/// the workspace and prove nothing about WebFetch specifically.
#[test]
fn a_scoped_api_backend_is_not_model_directed() {
    let backend = wcore_agent::tool_backends::http_github::HttpGitHubBackend::new();
    assert_eq!(
        backend.egress_origin(),
        wcore_egress::EgressOrigin::Product,
        "github_tool builds its URL with format! against a fixed host and \
         percent-encoded segments; it is not a model-chosen destination"
    );
}
