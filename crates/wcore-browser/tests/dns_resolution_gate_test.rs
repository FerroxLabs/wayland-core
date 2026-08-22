//! RED ARM for gh#1053 — `BrowserPolicy::check_resolved_host` has no
//! production caller, so no URL-bearing op is ever re-checked after DNS
//! resolution.
//!
//! Verified at `0ccaa90b`:
//!   `git grep -E 'to_socket_addrs|lookup_host|getaddrinfo' -- crates/wcore-browser`
//!   returns **0** hits. Positive control: the same query against `-- crates`
//!   returns **8** hits (`wcore-tools/src/url_safety.rs` x2,
//!   `wcore-providers/src/retry.rs` x2, `wcore-plugin-wasm`,
//!   `wcore-tools/src/bash/policy.rs`, plus two test files). The query works;
//!   the absence in `wcore-browser` is real. The resolution step does not
//!   exist and must be BUILT — there is no call site to "wire up".
//!
//! ## Both production entry points are graded here, not just one
//!
//! `evaluate()` is reached from two independent places and a gate installed at
//! only one of them leaves the other open:
//!   * pre-flight — `BrowserTool::policy_check` (`tool.rs:345`)
//!   * post-navigation landing URL — `CamoufoxBackend::dispatch`
//!     (`camoufox.rs:302`) and `enforce_post_navigation_policy`
//!     (`camoufox.rs:548`)
//!
//! Each has its own test below.
//!
//! ## What is hermetically testable here, and what is not
//!
//! `.invalid` is reserved by RFC 6761 §6.4 and is guaranteed never to resolve,
//! on a box with a resolver and on a box without one alike — so "this host
//! resolves to nothing" is the one resolution outcome that can be asserted
//! from an integration test with no resolver seam and no network. The
//! *resolves-to-a-blocked-IP* case (the actual rebinding attack) needs an
//! injected resolver and is graded inline in `src/policy.rs`, mirroring the
//! `type Resolver = fn(&str) -> Vec<IpAddr>` seam `wcore-tools/src/url_safety.rs`
//! already uses (`:202-207`).
//!
//! ## Honest scope of the fix these tests describe
//!
//! Camoufox is a SIDECAR: Firefox performs its own DNS resolution in another
//! process, so we cannot pin the addresses it dials. This gate closes STATIC
//! DNS SSRF (currently 100% open). It does NOT close TTL=0 intra-navigation
//! rebinding.

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_browser::backends::CamoufoxBackend;
use wcore_browser::op::BrowserOp;
use wcore_browser::policy::{BrowserPolicy, LoopbackCapability, PolicyAction};
use wcore_browser::provider::{BrowserOpError, BrowserProvider};
use wcore_browser::supervisor::BrowserSupervisor;
use wcore_browser::tool::BrowserTool;

/// RFC 6761 §6.4 — guaranteed never to resolve, with or without a resolver.
const UNRESOLVABLE: &str = "unresolvable-rebind-probe.invalid";
/// A public literal. Reaches the gate WITHOUT needing DNS at all, which is
/// what makes it the negative control for every resolution assertion below.
const PUBLIC_LITERAL: &str = "93.184.216.34";

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

fn tool(server_uri: String, policy: BrowserPolicy) -> BrowserTool {
    let backend = CamoufoxBackend::with_policy(server_uri, policy.clone());
    BrowserTool::new(
        Arc::new(backend) as Arc<dyn BrowserProvider>,
        policy,
        Arc::new(BrowserSupervisor::new()),
    )
}

// ---------------------------------------------------------------------------
// Entry point 1 — pre-flight (`BrowserTool::policy_check`).
// ---------------------------------------------------------------------------

/// RED. A host the gate cannot resolve must be refused: the policy has no idea
/// where the request will land, and "I could not check" is not "allowed".
///
/// Today `evaluate()` decides from the URL string alone, so this navigation is
/// permitted and the op reaches the sidecar.
#[tokio::test]
async fn preflight_refuses_a_navigation_whose_host_resolves_to_nothing() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-nx").await;
    mount_navigate(&server, "tab-nx", &format!("http://{UNRESOLVABLE}/")).await;

    // default_action = Allow, no lists: the ONLY thing that can refuse this
    // URL is a resolution gate.
    let t = tool(
        server.uri(),
        BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]),
    );
    let input = json!({ "op": { "kind": "navigate", "url": format!("http://{UNRESOLVABLE}/") } });

    use wcore_tools::Tool;
    let r = t.execute(input).await;

    assert!(
        r.is_error,
        "a host that resolves to no address at all must be refused before the \
         op reaches the sidecar; got a success: {}",
        r.content
    );
    assert!(
        r.content.to_ascii_lowercase().contains("resolv"),
        "the refusal must say the resolution gate is why, or an operator cannot \
         tell this apart from an allow-list miss; got: {}",
        r.content
    );
}

/// GRADES THE PRE-FLIGHT SEAM ALONE.
///
/// `preflight_refuses_a_navigation_whose_host_resolves_to_nothing` above does
/// NOT: the sidecar mock there hands back the same unresolvable name as the
/// landing URL, so the post-navigation gate in `CamoufoxBackend::dispatch`
/// refuses it even when `BrowserTool::policy_check` is left completely
/// un-gated. Measured 2026-08-22: reverting only `tool.rs:353` to the
/// string-only `evaluate` left all six tests in this file GREEN. That is the
/// "unit-tested guard shipped through an ungraded call site" shape this file
/// was written to prevent, reproduced inside the file itself.
///
/// Here the sidecar answers with a PUBLIC LITERAL landing URL, which the
/// post-navigation gate is obliged to allow. The pre-flight gate is therefore
/// the only thing left that can refuse the op, so this test fails if and only
/// if the pre-flight seam loses the resolution gate.
#[tokio::test]
async fn preflight_alone_refuses_it_even_when_the_landing_url_is_clean() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-pre").await;
    // Landing URL needs no DNS => the post-navigation gate cannot object.
    mount_navigate(
        &server,
        "tab-pre",
        &format!("http://{PUBLIC_LITERAL}/landed"),
    )
    .await;

    let t = tool(
        server.uri(),
        BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]),
    );
    let input = json!({ "op": { "kind": "navigate", "url": format!("http://{UNRESOLVABLE}/") } });

    use wcore_tools::Tool;
    let r = t.execute(input).await;

    assert!(
        r.is_error,
        "the requested URL is unresolvable and only the PRE-FLIGHT gate can \
         say so here -- the landing URL is a clean literal. Got a success: {}",
        r.content
    );
    assert!(
        r.content.contains(UNRESOLVABLE),
        "the refusal must name the REQUESTED host, proving it came from the \
         pre-flight seam and not from the landing-URL check; got: {}",
        r.content
    );
}

/// NEGATIVE CONTROL for the test above — pairs with it and must pass BOTH
/// before and after the fix. A public literal IP needs no DNS, so the
/// resolution gate must let it straight through. Without this control the
/// test above could be satisfied by refusing every navigation.
#[tokio::test]
async fn preflight_allows_a_public_literal_ip_which_needs_no_resolution() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-lit").await;
    mount_navigate(&server, "tab-lit", &format!("http://{PUBLIC_LITERAL}/")).await;

    let t = tool(
        server.uri(),
        BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]),
    );
    let input = json!({ "op": { "kind": "navigate", "url": format!("http://{PUBLIC_LITERAL}/") } });

    use wcore_tools::Tool;
    let r = t.execute(input).await;

    assert!(
        !r.is_error,
        "a public IP literal requires no DNS and must still be permitted; the \
         resolution gate must not refuse everything: {}",
        r.content
    );
}

// ---------------------------------------------------------------------------
// Entry point 2 — post-navigation landing URL (`CamoufoxBackend::dispatch`).
// ---------------------------------------------------------------------------

/// RED. The landing URL after the sidecar followed its redirects goes through
/// `policy.evaluate` (`camoufox.rs:302`) and therefore inherits the same blind
/// spot: a 3xx chain that lands on a name is never resolved.
///
/// A gate installed only at the pre-flight seam leaves this one open, which is
/// exactly the "unit-tested guard shipped through an ungraded call site" shape.
#[tokio::test]
async fn landing_url_goes_through_the_resolution_gate_too() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-land").await;
    mount_navigate(
        &server,
        "tab-land",
        &format!("http://{UNRESOLVABLE}/landed"),
    )
    .await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
    let session = backend.open_session(false).await.unwrap();

    let r = backend
        .dispatch(
            &session.ctx,
            BrowserOp::Navigate {
                // Starts at an address needing no DNS; the sidecar redirects
                // it to a name that resolves nowhere.
                url: format!("http://{PUBLIC_LITERAL}/start"),
                wait_until_loaded: true,
            },
        )
        .await;

    match r {
        Err(BrowserOpError::PolicyDenied { url, reason }) => {
            assert!(
                url.contains(UNRESOLVABLE),
                "the denial must name the landing URL, got {url}"
            );
            assert!(
                reason.to_ascii_lowercase().contains("resolv"),
                "the denial must name the resolution gate, got {reason}"
            );
        }
        other => panic!("post-navigation landing URL escaped the resolution gate, got {other:?}"),
    }
}

/// NEGATIVE CONTROL for the test above. A landing URL on a public literal must
/// still be accepted, so the post-navigation gate cannot pass by refusing
/// every landing.
#[tokio::test]
async fn landing_url_on_a_public_literal_is_still_accepted() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-land-ok").await;
    mount_navigate(
        &server,
        "tab-land-ok",
        &format!("http://{PUBLIC_LITERAL}/landed"),
    )
    .await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
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
        "a literal-IP landing URL needs no resolution and must pass: {r:?}"
    );
}

/// GRADES THE THIRD SEAM: `enforce_post_navigation_policy`, the Back/Forward
/// landing-URL check (`camoufox.rs:554`).
///
/// The Navigate arm and this one are separate call sites, and the test above
/// only exercises the Navigate arm. Measured 2026-08-22: reverting ONLY
/// `camoufox.rs:554` to the string-only `evaluate` left all 194 tests green.
/// Back/Forward land on a URL the sidecar chose from its own history, which is
/// exactly the kind of URL the model never typed and nobody re-checks.
#[tokio::test]
async fn history_landing_url_goes_through_the_resolution_gate_too() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-hist").await;
    Mock::given(method("POST"))
        .and(path("/tabs/tab-hist/back"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "ok": true, "url": format!("http://{UNRESOLVABLE}/previous") }),
            ),
        )
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
    let session = backend.open_session(false).await.unwrap();

    let r = backend.dispatch(&session.ctx, BrowserOp::Back {}).await;

    match r {
        Err(BrowserOpError::PolicyDenied { url, reason }) => {
            assert!(
                url.contains(UNRESOLVABLE),
                "the denial must name the history landing URL, got {url}"
            );
            assert!(
                reason.to_ascii_lowercase().contains("resolv"),
                "the denial must name the resolution gate, got {reason}"
            );
        }
        other => panic!("Back landed outside the resolution gate, got {other:?}"),
    }
}

/// NEGATIVE CONTROL for the test above. A history landing URL on a public
/// literal needs no lookup and must still be accepted, so the Back/Forward
/// gate cannot pass by refusing every history step.
#[tokio::test]
async fn history_landing_url_on_a_public_literal_is_still_accepted() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-hist-ok").await;
    Mock::given(method("POST"))
        .and(path("/tabs/tab-hist-ok/back"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({ "ok": true, "url": format!("http://{PUBLIC_LITERAL}/previous") }),
        ))
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
    let session = backend.open_session(false).await.unwrap();

    let r = backend.dispatch(&session.ctx, BrowserOp::Back {}).await;
    assert!(
        r.is_ok(),
        "a literal-IP history landing URL needs no resolution and must pass: {r:?}"
    );
}

// ---------------------------------------------------------------------------
// THE TRAP. These two pass TODAY. They are here so the fix cannot break gh#911.
// ---------------------------------------------------------------------------

/// REGRESSION GUARD (passes today, must still pass after the fix).
///
/// `check_resolved_host` refuses loopback UNCONDITIONALLY
/// (`policy.rs:408` → `blocked_resolved_ip_reason`). A naive wiring that
/// resolves every host and feeds the result to it therefore denies
/// `http://localhost:3000/` even with a valid `LoopbackCapability` — which
/// deletes the entire gh#911 recovery path.
///
/// The resolution gate MUST be skipped for a canonical loopback host holding
/// an authorising grant (`is_canonical_loopback_host` at `policy.rs:508` +
/// `self.loopback.authorize(port)` at `policy.rs:158`).
#[tokio::test]
async fn granted_loopback_is_not_broken_by_the_resolution_gate() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-lb").await;
    mount_navigate(&server, "tab-lb", "http://localhost:3000/").await;

    let policy =
        BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]).with_loopback(LoopbackCapability {
            enabled: true,
            schema_version: wcore_browser::policy::LOOPBACK_CAPABILITY_VERSION,
            session_scope: "red-arm-local-dev".into(),
            ports: vec![3000],
        });
    let t = tool(server.uri(), policy);
    let input = json!({ "op": { "kind": "navigate", "url": "http://localhost:3000/" } });

    use wcore_tools::Tool;
    let r = t.execute(input).await;

    assert!(
        !r.is_error,
        "gh#911: a granted loopback port must remain reachable. A resolution \
         gate that does not skip canonical loopback under an authorising grant \
         breaks the only recovery path an operator has: {}",
        r.content
    );
}

/// NEGATIVE CONTROL for the guard above — the grant must stay narrow. An
/// ungranted port on loopback is still refused, so the guard above cannot be
/// satisfied by exempting loopback wholesale.
#[tokio::test]
async fn loopback_outside_the_granted_ports_is_still_refused() {
    let server = MockServer::start().await;
    mount_open(&server, "tab-lb-no").await;
    mount_navigate(&server, "tab-lb-no", "http://localhost:9999/").await;

    let policy =
        BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]).with_loopback(LoopbackCapability {
            enabled: true,
            schema_version: wcore_browser::policy::LOOPBACK_CAPABILITY_VERSION,
            session_scope: "red-arm-local-dev".into(),
            ports: vec![3000],
        });
    let t = tool(server.uri(), policy);
    let input = json!({ "op": { "kind": "navigate", "url": "http://localhost:9999/" } });

    use wcore_tools::Tool;
    let r = t.execute(input).await;
    assert!(
        r.is_error,
        "a grant for port 3000 must not reach port 9999: {}",
        r.content
    );
}
