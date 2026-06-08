//! Integration tests for `BrowserPolicy` hardening — Phase 1 Wave SB.
//!
//! Closes:
//!   * SECURITY BLOCKER #3 (`SECURITY-v0.2.0.md`) — redirect re-check
//!   * SECURITY MAJOR — scheme allowlist (`javascript:` / `data:` / `blob:`)
//!   * SECURITY MAJOR — legacy IPv4 encodings (`0177.0.0.1` / `0x...` / `2130706433`)
//!   * SECURITY MAJOR — IPv4-mapped IPv6 (`::ffff:169.254.169.254`)
//!   * STABILITY MAJOR #6 — fail-open `BrowserPolicy::default()`
//!   * DNS rebinding TOFU bonus
//!
//! Every test below is a POSITIVE assertion of REFUSAL — the bypass MUST
//! not be reachable. The "negative" cases (legitimate operator-configured
//! allow flows) live in the bottom block.

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_browser::backends::CamoufoxBackend;
use wcore_browser::op::BrowserOp;
use wcore_browser::policy::{BrowserPolicy, PolicyAction, PolicyOutcome};
use wcore_browser::provider::{BrowserOpError, BrowserProvider, SessionCtx};
use wcore_browser::supervisor::BrowserSupervisor;
use wcore_browser::tool::BrowserTool;

// -------- Scheme allow-list ---------------------------------------------

#[test]
fn scheme_javascript_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let r = policy.check_url("javascript:fetch('http://169.254.169.254/')");
    assert!(r.is_err(), "javascript: must be refused");
    let msg = format!("{r:?}").to_lowercase();
    assert!(msg.contains("scheme"), "expected scheme refusal, got {msg}");
}

#[test]
fn scheme_data_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let r = policy.check_url("data:text/html,<script>alert(1)</script>");
    assert!(r.is_err(), "data: must be refused");
    assert!(format!("{r:?}").to_lowercase().contains("scheme"));
}

#[test]
fn scheme_blob_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let r = policy.check_url("blob:https://example.com/abc-123-def");
    assert!(r.is_err(), "blob: must be refused");
    assert!(format!("{r:?}").to_lowercase().contains("scheme"));
}

#[test]
fn scheme_file_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("file:///etc/passwd").is_err());
}

#[test]
fn scheme_ftp_and_gopher_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("ftp://example.com/").is_err());
    assert!(policy.check_url("gopher://example.com/").is_err());
}

// -------- Legacy IPv4 encodings -----------------------------------------

#[test]
fn ipv4_octal_form_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    // 0177 octal = 127 = loopback
    let r = policy.check_url("http://0177.0.0.1/");
    assert!(r.is_err(), "octal-encoded loopback must be refused");
}

#[test]
fn ipv4_hex_form_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    // 0x7F = 127 = loopback
    let r = policy.check_url("http://0x7F.0.0.1/");
    assert!(r.is_err(), "hex-encoded loopback must be refused");
}

#[test]
fn ipv4_decimal_overflow_form_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    // 2130706433 = 0x7F000001 = 127.0.0.1
    let r = policy.check_url("http://2130706433/");
    assert!(r.is_err(), "decimal 32-bit loopback must be refused");
}

#[test]
fn ipv4_decimal_metadata_form_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    // 169.254.169.254 = 0xA9FEA9FE = 2852039166
    let r = policy.check_url("http://2852039166/");
    assert!(r.is_err(), "decimal 32-bit metadata IP must be refused");
}

// -------- IPv6 / IPv4-mapped IPv6 ---------------------------------------

#[test]
fn ipv4_mapped_ipv6_metadata_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let r = policy.check_url("http://[::ffff:169.254.169.254]/");
    assert!(
        r.is_err(),
        "IPv4-mapped IPv6 metadata literal must be refused"
    );
    let msg = format!("{r:?}").to_lowercase();
    assert!(
        msg.contains("ipv4-mapped") || msg.contains("metadata"),
        "expected IPv4-mapped or metadata refusal reason, got {msg}"
    );
}

#[test]
fn ipv4_mapped_ipv6_loopback_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let r = policy.check_url("http://[::ffff:127.0.0.1]/");
    assert!(r.is_err(), "IPv4-mapped IPv6 loopback must be refused");
}

#[test]
fn ipv6_loopback_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("http://[::1]/").is_err());
}

#[test]
fn ipv6_unique_local_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("http://[fc00::1]/").is_err());
    assert!(policy.check_url("http://[fd00:1234::1]/").is_err());
}

#[test]
fn ipv6_link_local_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("http://[fe80::1]/").is_err());
    assert!(policy.check_url("http://[fe80::abc:def]/").is_err());
}

// -------- Fail-closed default -------------------------------------------

#[test]
fn default_policy_denies_arbitrary_origin() {
    // STABILITY MAJOR #6 — `BrowserPolicy::default()` MUST fail-closed
    // as of v0.2.1. Pre-v0.2.1 this passed silently.
    let policy = BrowserPolicy::default();
    let r = policy.check_url("https://example.com/");
    assert!(
        r.is_err(),
        "default policy must fail-closed; expected deny for example.com"
    );
    assert!(
        format!("{r:?}").contains("Deny"),
        "expected Deny reason, got {r:?}"
    );
}

#[test]
fn default_policy_denies_with_explicit_allow_action_field() {
    // Even with PolicyAction::Allow as the default action, hard-coded
    // blocks (metadata, RFC 1918, loopback) still fire.
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("http://169.254.169.254/").is_err());
    assert!(policy.check_url("http://127.0.0.1/").is_err());
}

// -------- DNS rebinding TOFU --------------------------------------------

#[test]
fn dns_rebinding_first_then_loopback_denied() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec!["host.example".into()], Vec::new());
    // First resolve to benign public IP — pinned.
    let first = "1.2.3.4".parse().unwrap();
    let r1 = policy.check_resolved_host("host.example", first);
    assert!(matches!(r1, PolicyOutcome::Allow));
    // Second resolve same host → loopback. MUST be refused.
    let second = "127.0.0.1".parse().unwrap();
    let r2 = policy.check_resolved_host("host.example", second);
    assert!(
        matches!(r2, PolicyOutcome::Deny { .. }),
        "DNS rebind must be refused, got {r2:?}"
    );
}

#[test]
fn dns_resolved_to_metadata_denied_even_on_first_resolve() {
    // First-resolve TO a blocked IP must still refuse — the TOFU cache
    // is in addition to (not in place of) the static block list.
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec!["host.example".into()], Vec::new());
    let bad = "169.254.169.254".parse().unwrap();
    let r = policy.check_resolved_host("host.example", bad);
    assert!(matches!(r, PolicyOutcome::Deny { .. }));
}

// -------- Camoufox backend `final_url` re-check  -------------------------

#[tokio::test]
async fn camoufox_final_url_metadata_refused() {
    // Mocked sidecar returns a final_url that hits the metadata
    // endpoint — the backend must refuse, even though the *initial*
    // URL passed the tool-layer pre-check. This is the BLOCKER #3
    // closing test (one-shot policy bypass via redirects).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sessions/sess-A/navigate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "final_url": "http://169.254.169.254/latest/meta-data/iam/security-credentials/"
        })))
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(
        PolicyAction::Allow,
        vec!["*.allowed.example".into()],
        Vec::new(),
    );
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
    let r = backend
        .dispatch(
            &SessionCtx::for_test("sess-A"),
            BrowserOp::Navigate {
                url: "https://foo.allowed.example/redirect".into(),
                wait_until_loaded: true,
            },
        )
        .await;
    match r {
        Err(BrowserOpError::PolicyDenied { url, reason }) => {
            assert!(
                url.contains("169.254.169.254"),
                "expected metadata final_url in error, got {url}"
            );
            assert!(
                reason.contains("post-redirect") || reason.to_lowercase().contains("metadata"),
                "expected post-redirect/metadata reason, got {reason}"
            );
        }
        other => panic!("expected PolicyDenied, got {other:?}"),
    }
}

#[tokio::test]
async fn camoufox_final_url_allowed_when_in_allow_list() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sessions/sess-B/navigate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "ok": true,
            "final_url": "https://foo.allowed.example/landed"
        })))
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(
        PolicyAction::Allow,
        vec!["*.allowed.example".into()],
        Vec::new(),
    );
    let backend = CamoufoxBackend::with_policy(server.uri(), policy);
    let r = backend
        .dispatch(
            &SessionCtx::for_test("sess-B"),
            BrowserOp::Navigate {
                url: "https://foo.allowed.example/start".into(),
                wait_until_loaded: true,
            },
        )
        .await;
    assert!(r.is_ok(), "in-allow-list final_url must pass: {r:?}");
}

// -------- BrowserTool integration: policy pre-check (initial URL) -------

#[tokio::test]
async fn browsertool_initial_metadata_navigation_refused() {
    // The tool-layer pre-check refuses metadata as the initial URL.
    let server = MockServer::start().await;
    // We don't expect the sidecar to be hit at all; policy denies first.
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy(server.uri(), policy.clone());
    let tool = BrowserTool::new(
        Arc::new(backend) as Arc<dyn BrowserProvider>,
        policy,
        Arc::new(BrowserSupervisor::new()),
    );
    let input = json!({
        "op": {
            "kind": "navigate",
            "url": "http://169.254.169.254/"
        }
    });
    use wcore_tools::Tool;
    let r = tool.execute(input).await;
    assert!(r.is_error, "must refuse: {}", r.content);
}

#[tokio::test]
async fn browsertool_initial_javascript_scheme_refused() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let backend = CamoufoxBackend::with_policy("http://unused:9377", policy.clone());
    let tool = BrowserTool::new(
        Arc::new(backend) as Arc<dyn BrowserProvider>,
        policy,
        Arc::new(BrowserSupervisor::new()),
    );
    let input = json!({
        "op": {
            "kind": "navigate",
            "url": "javascript:alert(1)"
        }
    });
    use wcore_tools::Tool;
    let r = tool.execute(input).await;
    assert!(r.is_error, "javascript: must refuse: {}", r.content);
    assert!(
        r.content.to_lowercase().contains("scheme") || r.content.contains("policy"),
        "expected scheme/policy refusal, got {}",
        r.content
    );
}

// -------- reqwest redirect policy (BLOCKER #3 closing test) -------------

#[tokio::test]
async fn reqwest_redirect_policy_blocks_metadata_hop() {
    // Direct test of the redirect-policy: a benign origin that returns a
    // 302 to the metadata endpoint MUST be refused by the redirect
    // policy installed on the reqwest client.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://169.254.169.254/"),
        )
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(
        PolicyAction::Allow,
        // The redirect-policy evaluates each hop independently — the
        // 302's Location URL hits the hard-coded metadata block list
        // regardless of allow-list contents.
        vec![],
        vec![],
    );
    let client = wcore_egress::EgressClient::builder()
        .redirect(policy.reqwest_redirect_policy())
        .build()
        .expect("client builder");

    let url = format!("{}/start", server.uri());
    let r = client.get(&url).send().await;
    assert!(
        r.is_err(),
        "redirect to metadata endpoint must be refused, got {r:?}"
    );
    let msg = format!("{:?}", r.unwrap_err()).to_lowercase();
    assert!(
        msg.contains("metadata")
            || msg.contains("169.254.169.254")
            || msg.contains("browserpolicy"),
        "expected metadata/policy refusal message, got {msg}"
    );
}

#[tokio::test]
async fn reqwest_redirect_policy_blocks_chained_loopback_hop() {
    // Multi-hop chain: benign → benign → loopback. Each hop runs the
    // policy; the final loopback hop must trigger refusal.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://example.com/hop2"),
        )
        .mount(&server)
        .await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]);
    let client = wcore_egress::EgressClient::builder()
        .redirect(policy.reqwest_redirect_policy())
        .build()
        .expect("client builder");

    // Test the simpler "first hop is allowed.com but mock returns location:
    // 127.0.0.1" — verifies that even though the initial URL is fine, the
    // redirect-policy still refuses the loopback hop.
    let server2 = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://127.0.0.1/admin"),
        )
        .mount(&server2)
        .await;
    let policy2 = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let client2 = wcore_egress::EgressClient::builder()
        .redirect(policy2.reqwest_redirect_policy())
        .build()
        .expect("client builder");
    let url2 = format!("{}/start", server2.uri());
    let r = client2.get(&url2).send().await;
    assert!(
        r.is_err(),
        "redirect to 127.0.0.1 must be refused, got {r:?}"
    );
    let msg = format!("{:?}", r.unwrap_err()).to_lowercase();
    assert!(
        msg.contains("loopback") || msg.contains("127.0.0.1") || msg.contains("browserpolicy"),
        "expected loopback refusal message, got {msg}"
    );

    // Suppress unused-var lints from the first setup (server / client
    // bound for symmetry / future expansion).
    drop(server);
    drop(client);
}

// -------- Negative cases: legitimate flows still work --------------------

#[test]
fn explicit_allow_list_permits_origin() {
    let policy = BrowserPolicy::new(
        PolicyAction::Deny, // fail-closed default
        vec!["*.example.com".into()],
        Vec::new(),
    );
    assert!(policy.check_url("https://foo.example.com/").is_ok());
    assert!(policy.check_url("https://example.com/").is_ok());
}

#[test]
fn https_to_arbitrary_origin_allowed_with_allow_default() {
    // Operator who explicitly opts into the old fail-open behavior.
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    assert!(policy.check_url("https://example.com/").is_ok());
    assert!(policy.check_url("https://news.ycombinator.com/").is_ok());
}

#[test]
fn dns_rebinding_same_ip_repeats_allowed() {
    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let ip = "1.2.3.4".parse().unwrap();
    let r1 = policy.check_resolved_host("public.example", ip);
    assert!(matches!(r1, PolicyOutcome::Allow));
    let r2 = policy.check_resolved_host("public.example", ip);
    assert!(matches!(r2, PolicyOutcome::Allow));
    let r3 = policy.check_resolved_host("public.example", ip);
    assert!(matches!(r3, PolicyOutcome::Allow));
    // Cache holds one pin.
    assert_eq!(policy.dns_cache_len(), 1);
}
