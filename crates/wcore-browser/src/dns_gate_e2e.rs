//! gh#1053 — END-TO-END grading of the DNS resolution gate against a host
//! that resolves to a **denied address**.
//!
//! ## Why this file exists, and why it is here and not in `tests/`
//!
//! `tests/dns_resolution_gate_test.rs` drives the same three production seams,
//! but the only resolution outcome it can produce hermetically is
//! *resolves-to-nothing* (an RFC 6761 `.invalid` name). That leaves the
//! block-list loop in `BrowserPolicy::evaluate_navigation_target_with`
//! — the half that actually stops a rebind — ungraded end to end.
//!
//! MEASURED on this tree before this file existed: deleting that loop
//! (`policy.rs:560-564`, replaced with `let _ = &addrs;`) left all **9** tests
//! in `tests/dns_resolution_gate_test.rs` GREEN. Positive control for the same
//! mutation: **4** in-crate tests went red
//! (`policy::tests::navigation_gate_refuses_a_name_that_resolves_to_the_metadata_endpoint`
//! and siblings), so the mutation is real and reachable — it was simply
//! invisible from every seam-level test.
//!
//! Making a name resolve to a chosen address needs a resolver seam, and that
//! seam must not be reachable from production code — so it is
//! `#[cfg(test)] pub(crate) BrowserPolicy::with_resolver`, and the tests that
//! use it live in-crate. They are still end-to-end in the sense the issue
//! asks for: every one of them enters through a production entry point
//! (`BrowserTool::execute` or `CamoufoxBackend::dispatch`) against a wiremock
//! sidecar, not through the policy predicate.
//!
//! ## Seams graded here
//!
//!   1. pre-flight — `BrowserTool::policy_check` (`tool.rs:353`)
//!   2. post-navigation landing URL — `CamoufoxBackend::dispatch`
//!      (`camoufox.rs:306`)
//!   3. Back / Forward landing URL — `enforce_post_navigation_policy`
//!      (`camoufox.rs:554`)
//!
//! Every refusal assertion requires the reason to contain `DNS resolved` —
//! the wording produced ONLY by `blocked_resolved_ip_reason`. A refusal from
//! the empty-answer branch says `resolved to no address at all` instead, so
//! these tests cannot be satisfied by the branch the `.invalid` tests already
//! cover. Each is paired with a control host that resolves to a PUBLIC
//! address and must be allowed, so none of them can pass by refusing
//! everything.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_tools::Tool;

use crate::backends::CamoufoxBackend;
use crate::op::BrowserOp;
use crate::policy::{BrowserPolicy, PolicyAction, Resolver};
use crate::provider::{BrowserOpError, BrowserProvider};
use crate::supervisor::BrowserSupervisor;
use crate::tool::BrowserTool;

/// `.test` is RFC 6761 §6.2 reserved: it can never resolve for real, so these
/// names have no answer except the one the stub resolver gives them.
const METADATA_NAME: &str = "metadata-rebind.probe.test";
const PRIVATE_NAME: &str = "private-rebind.probe.test";
const MIXED_NAME: &str = "mixed-rebind.probe.test";
const CLEAN_NAME: &str = "clean-public.probe.test";

/// A public literal, used where a URL must reach the gate without needing DNS.
const PUBLIC_LITERAL: &str = "93.184.216.34";

/// The rebinding resolver. A plain `fn` pointer, the same shape production
/// uses for `system_resolver`, so it can be installed on a `BrowserPolicy`
/// that the real tool and the real backend then share.
fn rebinding_resolver(host: &str) -> Vec<IpAddr> {
    match host {
        METADATA_NAME => vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))],
        PRIVATE_NAME => vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7))],
        // One clean address FIRST, one blocked address second: a gate that
        // checks only `addrs[0]` — the shape an attacker gets by ordering
        // their own answer — lets this through.
        MIXED_NAME => vec![
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
        ],
        CLEAN_NAME => vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
        _ => Vec::new(),
    }
}

fn policy() -> BrowserPolicy {
    // default_action = Allow with no lists: the ONLY thing that can refuse
    // any URL below is the resolution gate.
    BrowserPolicy::new(PolicyAction::Allow, vec![], vec![])
        .with_resolver(rebinding_resolver as Resolver)
}

async fn mount_open(server: &MockServer, tab: &str) {
    Mock::given(method("POST"))
        .and(path("/tabs"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "tabId": tab, "url": "about:blank" })),
        )
        .mount(server)
        .await;
}

async fn mount_navigate(server: &MockServer, tab: &str, landing: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/tabs/{tab}/navigate")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "url": landing })),
        )
        .mount(server)
        .await;
}

async fn mount_back(server: &MockServer, tab: &str, landing: &str) {
    Mock::given(method("POST"))
        .and(path(format!("/tabs/{tab}/back")))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "url": landing })),
        )
        .mount(server)
        .await;
}

fn tool(server_uri: String) -> BrowserTool {
    let p = policy();
    let backend = CamoufoxBackend::with_policy(server_uri, p.clone());
    BrowserTool::new(
        Arc::new(backend) as Arc<dyn BrowserProvider>,
        p,
        Arc::new(BrowserSupervisor::new()),
    )
}

/// Navigate through the whole tool, with a landing URL the post-navigation
/// gate is obliged to allow, so the PRE-FLIGHT seam is the only thing that can
/// refuse. Returns the tool result.
async fn navigate_via_tool(tab: &str, url: &str) -> wcore_types::tool::ToolResult {
    let server = MockServer::start().await;
    mount_open(&server, tab).await;
    // Landing on a public literal needs no DNS => the landing-URL gate cannot
    // object, so a refusal below can only have come from pre-flight.
    mount_navigate(&server, tab, &format!("http://{PUBLIC_LITERAL}/landed")).await;
    let t = tool(server.uri());
    t.execute(json!({ "op": { "kind": "navigate", "url": url } }))
        .await
}

// ---------------------------------------------------------------------------
// Seam 1 — pre-flight (`BrowserTool::policy_check`).
// ---------------------------------------------------------------------------

/// The attack this issue was filed about: a public NAME pointing at the cloud
/// metadata endpoint. Nothing in the string is objectionable, so only the
/// resolution gate block-list loop can refuse it.
#[tokio::test]
async fn end_to_end_preflight_refuses_a_name_that_resolves_to_the_metadata_endpoint() {
    let r = navigate_via_tool(
        "tab-md",
        &format!("http://{METADATA_NAME}/latest/meta-data/"),
    )
    .await;

    assert!(
        r.is_error,
        "a name resolving to 169.254.169.254 must be refused before the op \
         reaches the sidecar; got a success: {}",
        r.content
    );
    assert!(
        r.content.contains("DNS resolved"),
        "the refusal must come from the RESOLVED-ADDRESS block list, not from \
         the empty-answer branch (which says \"resolved to no address at all\"); \
         got: {}",
        r.content
    );
    assert!(
        r.content.contains("169.254.169.254"),
        "the refusal must name the address that was refused: {}",
        r.content
    );
}

/// Same seam, RFC 1918 instead of the metadata endpoint — the other half of
/// static DNS SSRF (a public name pointing into an internal network).
#[tokio::test]
async fn end_to_end_preflight_refuses_a_name_that_resolves_into_the_private_range() {
    let r = navigate_via_tool("tab-priv", &format!("http://{PRIVATE_NAME}/admin")).await;

    assert!(
        r.is_error,
        "a name resolving into 10/8 must be refused: {}",
        r.content
    );
    assert!(
        r.content.contains("DNS resolved") && r.content.contains("RFC 1918"),
        "the refusal must name the resolved-address block list and the \
         category: {}",
        r.content
    );
}

/// Grades the "EVERY address" half of the loop. The clean address is FIRST in
/// the answer, so a gate that inspects only the first address lets this
/// through.
#[tokio::test]
async fn end_to_end_preflight_refuses_when_only_one_resolved_address_is_blocked() {
    let r = navigate_via_tool("tab-mix", &format!("http://{MIXED_NAME}/")).await;

    assert!(
        r.is_error,
        "one blocked address anywhere in the answer set must refuse the whole \
         navigation, even when a clean address is listed first: {}",
        r.content
    );
    assert!(
        r.content.contains("DNS resolved"),
        "must be the resolved-address block list: {}",
        r.content
    );
}

/// CONTROL for all three above. A name that resolves to a PUBLIC address must
/// still be permitted. Without this the tests above are satisfied by a gate
/// that refuses every name it has to resolve.
#[tokio::test]
async fn end_to_end_preflight_allows_a_name_that_resolves_to_a_public_address() {
    let r = navigate_via_tool("tab-clean", &format!("http://{CLEAN_NAME}/")).await;

    assert!(
        !r.is_error,
        "a name resolving to a public address must pass the gate; the gate \
         must not refuse every name it resolves: {}",
        r.content
    );
}

// ---------------------------------------------------------------------------
// Seam 2 — post-navigation landing URL (`CamoufoxBackend::dispatch`).
// ---------------------------------------------------------------------------

/// The redirect door: the model asks for a clean literal, the sidecar follows
/// a 3xx chain and lands on a name that points at the metadata endpoint.
#[tokio::test]
async fn end_to_end_landing_url_that_resolves_to_a_denied_address_is_refused() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-land-md").await;
    mount_navigate(
        &server,
        "tab-land-md",
        &format!("http://{METADATA_NAME}/latest/"),
    )
    .await;

    let backend = CamoufoxBackend::with_policy(server.uri(), policy());
    let session = backend.open_session(false).await.unwrap();

    let r = backend
        .dispatch(
            &session.ctx,
            BrowserOp::Navigate {
                url: format!("http://{PUBLIC_LITERAL}/start"),
                wait_until_loaded: true,
            },
        )
        .await;

    match r {
        Err(BrowserOpError::PolicyDenied { url, reason }) => {
            assert!(
                url.contains(METADATA_NAME),
                "the denial must name the landing URL, got {url}"
            );
            assert!(
                reason.contains("DNS resolved") && reason.contains("169.254.169.254"),
                "the denial must come from the resolved-address block list, got {reason}"
            );
        }
        other => panic!(
            "a landing URL resolving to the metadata endpoint escaped the \
             resolution gate, got {other:?}"
        ),
    }
}

/// CONTROL for the test above — a landing URL on a name that resolves to a
/// public address must still be accepted.
#[tokio::test]
async fn end_to_end_landing_url_that_resolves_to_a_public_address_is_accepted() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-land-ok").await;
    mount_navigate(
        &server,
        "tab-land-ok",
        &format!("http://{CLEAN_NAME}/landed"),
    )
    .await;

    let backend = CamoufoxBackend::with_policy(server.uri(), policy());
    let session = backend.open_session(false).await.unwrap();

    let r = backend
        .dispatch(
            &session.ctx,
            BrowserOp::Navigate {
                url: format!("http://{PUBLIC_LITERAL}/start"),
                wait_until_loaded: true,
            },
        )
        .await;
    assert!(
        r.is_ok(),
        "a landing URL resolving to a public address must pass: {r:?}"
    );
}

// ---------------------------------------------------------------------------
// Seam 3 — Back / Forward landing URL (`enforce_post_navigation_policy`).
// ---------------------------------------------------------------------------

/// Back lands on a URL the sidecar chose from its own history — a URL the
/// model never typed and nobody else re-checks.
#[tokio::test]
async fn end_to_end_history_landing_url_that_resolves_to_a_denied_address_is_refused() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-hist-md").await;
    mount_back(
        &server,
        "tab-hist-md",
        &format!("http://{PRIVATE_NAME}/previous"),
    )
    .await;

    let backend = CamoufoxBackend::with_policy(server.uri(), policy());
    let session = backend.open_session(false).await.unwrap();

    let r = backend.dispatch(&session.ctx, BrowserOp::Back {}).await;

    match r {
        Err(BrowserOpError::PolicyDenied { url, reason }) => {
            assert!(
                url.contains(PRIVATE_NAME),
                "the denial must name the history landing URL, got {url}"
            );
            assert!(
                reason.contains("DNS resolved") && reason.contains("RFC 1918"),
                "the denial must come from the resolved-address block list, got {reason}"
            );
        }
        other => panic!("Back landed on an inward-pointing name unchecked, got {other:?}"),
    }
}

/// CONTROL for the test above.
#[tokio::test]
async fn end_to_end_history_landing_url_that_resolves_to_a_public_address_is_accepted() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-hist-ok").await;
    mount_back(
        &server,
        "tab-hist-ok",
        &format!("http://{CLEAN_NAME}/previous"),
    )
    .await;

    let backend = CamoufoxBackend::with_policy(server.uri(), policy());
    let session = backend.open_session(false).await.unwrap();

    let r = backend.dispatch(&session.ctx, BrowserOp::Back {}).await;
    assert!(
        r.is_ok(),
        "a history landing URL resolving to a public address must pass: {r:?}"
    );
}
