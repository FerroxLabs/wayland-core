//! FerroxLabs/wayland#1264 c2 + c3 — the REAL `WebFetch` surface against the
//! REAL egress policy.
//!
//! The unit tests in `wcore-agent::egress` grade the classifier and the policy
//! wrapper. Neither of them proves the wiring: that the client `WebFetch`
//! actually holds stamps its origin, that the policy the process installs is
//! the one that client consults, and that a refusal reaches the model as a
//! tool error rather than being logged and walked past. That wiring is what
//! this file drives, through `HttpFetchBackend` — the type at
//! `tool_backends/http_fetch.rs` whose `.get(&req.url)` the ticket names.
//!
//! Provenance note the ticket asked for: the original finding was graded from
//! source reading, with no process having issued a request to an allowlisted
//! apex with a query payload and observed the absence of a prompt. These arms
//! issue it.
//!
//! No network is touched on the denial arms. The egress gate short-circuits
//! inside `EgressRequestBuilder::send` BEFORE `client.execute`, which is
//! itself part of what is being asserted — a policy that denied after dispatch
//! would have already leaked the payload.

use std::sync::Arc;

use wcore_agent::egress::{AgentEgressPolicy, AllowList};
use wcore_agent::tool_backends::HttpFetchBackend;
use wcore_tools::web_fetch::{FetchBackend, FetchOutcome, FetchRequest};

/// A base64url payload of the shape a real exfiltration uses. `-` is in this
/// alphabet on purpose: excluding it from the data-bearing token run was one
/// of the two shapes external review refuted, precisely because it blinds the
/// check to its actual target.
const PAYLOAD: &str = "ANTHROPIC-API-KEY-sk-ant-api03-0123456789abcdefghijklmnop";

fn allowing(domains: &[&str]) -> Arc<AgentEgressPolicy> {
    let mut allow = AllowList::default();
    for domain in domains {
        allow.allow_domain(domain);
    }
    Arc::new(AgentEgressPolicy::enforcing(allow))
}

/// Build the REAL backend with `policy` as the policy its client consults.
/// `HttpFetchBackend::new` reads `wcore_egress::default_policy()` at
/// construction, so the scope has to wrap the construction, not the fetch.
fn backend_under(policy: Arc<AgentEgressPolicy>) -> HttpFetchBackend {
    wcore_egress::with_default_policy_sync(policy, HttpFetchBackend::new)
}

fn request(url: &str) -> FetchRequest {
    FetchRequest {
        url: url.to_string(),
        timeout_ms: 5_000,
        readable: false,
    }
}

/// **c2.** A `WebFetch` of an allowlisted apex carrying a model-chosen query
/// payload is refused, in a session with no approval surface.
///
/// RED against `classify.rs`'s early return: with `Allow` returned on the host
/// match, this fetch was admitted, dispatched, and answered by github.com.
#[tokio::test]
async fn webfetch_of_an_allowlisted_apex_with_a_query_payload_is_refused() {
    let backend = backend_under(allowing(&["github.com"]));

    let outcome = backend
        .fetch(&request(&format!("https://github.com/?leak={PAYLOAD}")))
        .await;

    let FetchOutcome::Err { message } = outcome else {
        panic!(
            "a tool-driven payload to an allowlisted apex must be refused, got \
             {outcome:?}"
        );
    };
    assert!(
        message.contains("github.com"),
        "the refusal must name the host it refused: {message}"
    );
    assert!(
        !message.contains(PAYLOAD),
        "the refusal must not echo the payload back into the transcript: {message}"
    );
}

/// **c2**, the same defect through a body-bearing method rather than a query
/// string, so the criterion is met for the shape and not for one spelling.
#[tokio::test]
async fn webfetch_of_an_allowlisted_apex_with_a_long_path_payload_is_refused() {
    let backend = backend_under(allowing(&["github.com"]));

    let outcome = backend
        .fetch(&request(&format!("https://api.github.com/{PAYLOAD}")))
        .await;

    assert!(
        matches!(outcome, FetchOutcome::Err { .. }),
        "a high-entropy path payload to an allowlisted apex must be refused, \
         got {outcome:?}"
    );
}

/// **The wrong-refusal control.** The same backend, the same policy, the same
/// allowlisted apex — and a fetch that carries no data still reaches the
/// network.
///
/// Without this arm, a fix that refused every `WebFetch` would pass both tests
/// above and break the tool completely.
///
/// Stated exactly: the origin is served by wiremock on loopback, so what
/// admits it is `classify`'s local-destination short-circuit, which runs
/// before the allowlist. That is the honest bound of this arm — it proves the
/// tool-origin stamp did not make the whole `WebFetch` path refuse, and it
/// does NOT prove the allowlisted-apex data-less case, which
/// `classify::tests::a_data_less_tool_read_of_an_allowlisted_host_is_still_allowed`
/// and `policy::tests::unattended_tool_data_to_an_allowlisted_apex_is_denied`
/// control instead. Reaching github.com by name from a test would make the
/// arm a network measurement.
#[tokio::test]
async fn a_data_less_webfetch_still_reaches_the_origin_under_the_same_policy() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/docs"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("hello")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let backend = backend_under(allowing(&["github.com"]));
    let outcome = backend
        .fetch(&request(&format!("{}/docs", server.uri())))
        .await;

    match outcome {
        FetchOutcome::Ok { text, status, .. } => {
            assert_eq!(status, 200);
            assert!(text.contains("hello"), "unexpected body: {text}");
        }
        other => panic!("a data-less fetch must still be served, got {other:?}"),
    }
}
