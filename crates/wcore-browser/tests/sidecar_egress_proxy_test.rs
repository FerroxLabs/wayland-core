//! gh#1117 — the sidecar must not be able to dial an address Core never
//! screened, and a sidecar Core cannot contain must be refused rather than
//! silently used.
//!
//! ## What the sidecar actually sends, and how that is known
//!
//! MEASURED 2026-08-23 on hetzner-dsm against real Camoufox
//! (`@askjo/camofox-browser@1.13.1`, `camoufox` in `~/.cache/camoufox`), with
//! `PROXY_HOST=127.0.0.1 PROXY_PORT=18899` and a logging proxy on that port:
//!
//! ```text
//! REQLINE: CONNECT api.ipify.org:443 HTTP/1.1
//! REQLINE: GET http://example.com/ HTTP/1.1
//! ```
//!
//! and `POST /tabs/<id>/navigate` returned `{"ok":true,...}` — the page loaded
//! THROUGH the proxy. Firefox sent the HOSTNAME in both forms and resolved
//! nothing itself. That is the seam this proxy sits on, and the two request
//! forms below are those two lines.
//!
//! ## Why the client here is the test and not Firefox
//!
//! Driving real Camoufox in a unit-test job would need a browser build, an
//! X server and the network. The bytes it sends are the two lines above, so
//! the tests send exactly those bytes to the production
//! `PolicyEgressProxy` and assert on what Core does with them. The
//! browser-side half — that Camoufox honours `PROXY_HOST`/`PROXY_PORT` at all
//! — is graded separately by `apply_egress_env` in
//! `sidecar_launch_env_pins_the_proxy_against_the_ambient_environment`, and
//! was verified live as quoted above.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use wcore_browser::egress_proxy::PolicyEgressProxy;
use wcore_browser::policy::{
    BrowserPolicy, DialApproval, LoopbackCapability, PolicyAction, PolicyOutcome,
};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig, apply_egress_env};

/// Resolves to the cloud-metadata endpoint — the address the sidecar must
/// never be handed a tunnel to.
const METADATA_REBIND: &str = "metadata-rebind-probe.example";
/// Resolves to two public addresses. The control: the proxy must approve it,
/// and must approve it AT THOSE ADDRESSES.
const MULTI_A_PUBLIC: &str = "multi-a-probe.example";
/// Resolves to two addresses in TEST-NET-3 (RFC 5737, guaranteed never
/// routed). Neither is on the block-list, so the gate approves them; neither
/// can accept a connection, so the dial fails after being AIMED — which is
/// the only thing this name is used to observe.
const DIAL_PROBE: &str = "dial-probe.example";

fn probe_resolver(host: &str) -> Vec<std::net::IpAddr> {
    use std::net::{IpAddr, Ipv4Addr};
    match host {
        METADATA_REBIND => vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))],
        MULTI_A_PUBLIC => vec![
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            IpAddr::V4(Ipv4Addr::new(93, 184, 216, 35)),
        ],
        DIAL_PROBE => vec![
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
            IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)),
        ],
        _ => Vec::new(),
    }
}

fn probe_policy() -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]).with_resolver(probe_resolver)
}

/// Read until the response head terminator, or EOF.
async fn read_head(stream: &mut TcpStream) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        if out.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A minimal origin server on loopback that answers anything with `pong`.
/// Deliberately not `wiremock`: the plain-HTTP arm forwards an ABSOLUTE-FORM
/// request line, and the assertion is about Core's behaviour, not about which
/// request-target forms a particular HTTP server accepts.
async fn spawn_pong_server() -> (u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    // Exactly ONE connection: the test awaits this handle, so a server that
    // loops waiting for a second connection hangs the test rather than
    // reporting what the first one carried.
    let handle = tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return String::new();
        };
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        let seen = String::from_utf8_lossy(&buf[..n]).into_owned();
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\npong")
            .await;
        let _ = stream.shutdown().await;
        seen
    });
    (port, handle)
}

// ---------------------------------------------------------------------------
// THE DEFECT: an address Core never screened.
// ---------------------------------------------------------------------------

/// gh#1117. The sidecar asks for a tunnel to a public NAME that resolves to
/// the cloud-metadata endpoint. Core resolves it, screens the answer, and
/// refuses — so there is no second lookup for a TTL=0 answer to win, because
/// the sidecar never gets to make one.
///
/// This is also the proxy-side test that fails when the
/// `blocked_resolved_ip_reason` loop in `policy.rs::screen_navigation_target`
/// is deleted.
#[tokio::test]
async fn connect_to_a_name_resolving_to_metadata_is_refused_and_never_tunnelled() {
    let proxy = PolicyEgressProxy::start(probe_policy().address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(
            format!(
                "CONNECT {METADATA_REBIND}:443 HTTP/1.1\r\nHost: {METADATA_REBIND}:443\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let response = read_head(&mut client).await;

    assert!(
        response.starts_with("HTTP/1.1 403"),
        "the sidecar was handed a tunnel to a name resolving to \
         169.254.169.254; got: {response:?}"
    );
    assert!(
        response.contains("169.254.169.254"),
        "the refusal must name the screened address that failed: {response:?}"
    );
    assert_eq!(
        proxy.refused_count(),
        1,
        "the refusal has to be Core's, not the client giving up"
    );
    assert_eq!(
        proxy.approved_count(),
        0,
        "nothing may have been tunnelled: {response:?}"
    );
    proxy.shutdown();
}

/// Same for plain HTTP, which is the OTHER request form real Camoufox was
/// measured sending (`GET http://example.com/ HTTP/1.1`). A gate installed on
/// only one of the two forms leaves the other open — the shape gh#1053 was
/// filed about in the first place.
#[tokio::test]
async fn absolute_form_http_to_a_metadata_rebind_is_refused() {
    let proxy = PolicyEgressProxy::start(probe_policy().address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(
            format!("GET http://{METADATA_REBIND}/ HTTP/1.1\r\nHost: {METADATA_REBIND}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let response = read_head(&mut client).await;

    assert!(response.starts_with("HTTP/1.1 403"), "got: {response:?}");
    assert_eq!(proxy.refused_count(), 1);
    assert_eq!(proxy.approved_count(), 0);
    proxy.shutdown();
}

/// A host the proxy cannot resolve fails CLOSED. "I could not check" is not
/// "allowed", on this path either.
#[tokio::test]
async fn a_name_that_resolves_to_nothing_is_refused_at_the_proxy() {
    let proxy = PolicyEgressProxy::start(probe_policy().address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(b"CONNECT unresolvable-probe.invalid:443 HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let response = read_head(&mut client).await;

    assert!(response.starts_with("HTTP/1.1 403"), "got: {response:?}");
    assert!(
        response.to_ascii_lowercase().contains("resolv"),
        "got: {response:?}"
    );
    assert_eq!(proxy.refused_count(), 1);
    proxy.shutdown();
}

// ---------------------------------------------------------------------------
// CONTROLS — the proxy is a proxy, not a wall.
// ---------------------------------------------------------------------------

/// NEGATIVE CONTROL for all three refusals above, over the CONNECT form.
/// Without it they are all satisfied by a proxy that refuses everything.
///
/// The approved target is a granted-loopback host (gh#911), because that is
/// the only destination an integration test can both reach and have the gate
/// approve: any test server it starts is on loopback, and loopback is exactly
/// what the block-list refuses for every other host.
#[tokio::test]
async fn connect_to_a_granted_loopback_target_is_tunnelled() {
    let (port, server) = spawn_pong_server().await;
    let policy = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![])
        .with_loopback(LoopbackCapability {
            enabled: true,
            schema_version: wcore_browser::policy::LOOPBACK_CAPABILITY_VERSION,
            session_scope: "gh1117-proxy-control".into(),
            ports: vec![port],
        })
        .with_resolver(probe_resolver);
    let proxy = PolicyEgressProxy::start(policy.address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let established = read_head(&mut client).await;
    assert!(
        established.starts_with("HTTP/1.1 200"),
        "the proxy must tunnel an approved target, or every refusal above is \
         vacuous; got: {established:?}"
    );

    // Now speak through the tunnel. Half-closing after the request lets the
    // proxy's bidirectional copy finish instead of holding the socket open
    // until the harness times the test out.
    client
        .write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut body = String::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_string(&mut body))
        .await
        .expect("the tunnel never delivered a response")
        .unwrap();
    assert!(body.contains("pong"), "tunnel carried nothing: {body:?}");
    assert_eq!(proxy.approved_count(), 1);
    assert_eq!(proxy.refused_count(), 0);

    proxy.shutdown();
    let seen = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("origin server never finished")
        .unwrap();
    assert!(
        seen.starts_with("GET /ping"),
        "the origin server never saw the tunnelled request: {seen:?}"
    );
}

/// NEGATIVE CONTROL over the plain-HTTP form, and the framing assertion that
/// goes with it: the forwarded head must carry `Connection: close`, so a
/// SECOND request on the same socket cannot ride the first one's approval.
#[tokio::test]
async fn absolute_form_http_to_a_granted_loopback_target_is_forwarded_once() {
    let (port, server) = spawn_pong_server().await;
    let policy = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![])
        .with_loopback(LoopbackCapability {
            enabled: true,
            schema_version: wcore_browser::policy::LOOPBACK_CAPABILITY_VERSION,
            session_scope: "gh1117-proxy-http-control".into(),
            ports: vec![port],
        })
        .with_resolver(probe_resolver);
    let proxy = PolicyEgressProxy::start(policy.address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(
            format!(
                "GET http://localhost:{port}/ping HTTP/1.1\r\n\
                 Host: localhost:{port}\r\n\
                 Proxy-Connection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut body = String::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_string(&mut body))
        .await
        .expect("the proxy never delivered a response")
        .unwrap();
    assert!(body.contains("pong"), "got: {body:?}");
    assert_eq!(proxy.approved_count(), 1);

    proxy.shutdown();
    let request = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("origin server never finished")
        .unwrap();
    assert!(
        request.starts_with("GET http://"),
        "the origin server must receive the absolute-form request line \
         verbatim; got: {request:?}"
    );
    assert!(
        request.to_ascii_lowercase().contains("connection: close"),
        "a reusable proxy connection lets a second request to a DIFFERENT host \
         ride this one's approval; got: {request:?}"
    );
    assert!(
        !request.to_ascii_lowercase().contains("proxy-connection"),
        "hop-by-hop headers must not be forwarded; got: {request:?}"
    );
}

// ---------------------------------------------------------------------------
// "Core resolves once and dials THAT" — the property, not just the refusal.
// ---------------------------------------------------------------------------

/// The approval carries the screened addresses, all of them, so the dial can
/// use no other. This is what makes the TOCTOU unreachable: there is no second
/// lookup between the check and the connect because the check hands over the
/// addresses.
#[test]
fn an_approval_carries_every_screened_address() {
    let policy = probe_policy();
    match policy.approve_dial_target(&format!("https://{MULTI_A_PUBLIC}:443/")) {
        DialApproval::Approved { host, port, addrs } => {
            assert_eq!(host, MULTI_A_PUBLIC);
            assert_eq!(port, 443);
            assert_eq!(
                addrs,
                probe_resolver(MULTI_A_PUBLIC),
                "the dial set must be the screened set, not a subset chosen later"
            );
        }
        other => panic!("a multi-A public name must be approved, got {other:?}"),
    }
}

/// An IP literal is approved without a lookup, and says so by carrying no
/// addresses — dialling it by name performs no DNS. Pairs with the test above
/// so "empty addrs" cannot be mistaken for "resolved to nothing and approved".
#[test]
fn an_ip_literal_is_approved_without_a_lookup() {
    match probe_policy().approve_dial_target("https://93.184.216.34:443/") {
        DialApproval::Approved { host, port, addrs } => {
            assert_eq!(host, "93.184.216.34");
            assert_eq!(port, 443);
            assert!(addrs.is_empty());
        }
        other => panic!("got {other:?}"),
    }
    // ...and a BLOCKED literal is still blocked on this path.
    assert!(matches!(
        probe_policy().approve_dial_target("https://169.254.169.254:443/"),
        DialApproval::Denied { .. }
    ));
}

/// `address_gate_only` narrows the policy for sub-resource traffic. It must
/// drop `allowed_origins` (an operator's navigation allow-list is not a
/// statement that a permitted page may not load its own fonts) and it must
/// KEEP `denied_origins` and the hard block-list (an explicit block, and the
/// address gate, apply everywhere).
#[test]
fn the_proxy_policy_drops_the_allow_list_and_keeps_every_block() {
    let policy = BrowserPolicy::new(
        PolicyAction::Deny,
        vec!["*.allowed.example".into()],
        vec!["*.blocked.example".into()],
    )
    .with_resolver(probe_resolver);
    let gate = policy.address_gate_only();

    // Off the navigation allow-list, but a perfectly ordinary sub-resource.
    // It has to be a host the resolver KNOWS, or this passes for the wrong
    // reason (resolving to nothing is itself a refusal).
    assert!(matches!(
        gate.approve_dial_target(&format!("https://{MULTI_A_PUBLIC}:443/")),
        DialApproval::Approved { .. }
    ));
    // The operator's explicit block still wins.
    assert!(matches!(
        gate.approve_dial_target("https://x.blocked.example:443/"),
        DialApproval::Denied { .. }
    ));
    // The hard block-list still wins.
    assert!(matches!(
        gate.approve_dial_target("https://169.254.169.254:443/"),
        DialApproval::Denied { .. }
    ));
    // And the NAVIGATION policy is untouched by any of this: the same host
    // the address gate just approved is still off the operator's allow-list.
    assert!(matches!(
        policy.evaluate(&format!("https://{MULTI_A_PUBLIC}/")),
        PolicyOutcome::Deny { .. }
    ));
}

/// THE PROPERTY, ON THE PRODUCTION PATH: the proxy connects to an address the
/// gate handed it, and looks nothing up itself.
///
/// Every other test in this file is satisfied by a proxy that screens the
/// name and then calls `TcpStream::connect((host, port))` — which re-resolves,
/// and is gh#1117 verbatim. MEASURED: reverting `dial` to that form leaves all
/// 217 tests in this crate GREEN, live arm included, because a re-resolving
/// proxy refuses exactly the same targets and tunnels exactly the same ones.
/// The dial target is the only observable that separates them.
#[tokio::test]
async fn the_proxy_dials_a_screened_address_and_never_re_resolves_the_name() {
    let proxy = PolicyEgressProxy::start(probe_policy().address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(format!("CONNECT {DIAL_PROBE}:8443 HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();

    // The dial is recorded before the connect is attempted, so this does not
    // wait on an unroutable address.
    let mut dialled = Vec::new();
    for _ in 0..200 {
        dialled = proxy.dialled_addrs();
        if !dialled.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert!(
        !dialled.is_empty(),
        "the proxy approved {DIAL_PROBE} and then opened no screened \
         connection at all — it re-resolved the name and dialled whatever \
         the second lookup returned, which is the TOCTOU gh#1117 is about"
    );
    let screened: Vec<std::net::IpAddr> = probe_resolver(DIAL_PROBE);
    for addr in &dialled {
        assert!(
            screened.contains(&addr.ip()),
            "the proxy dialled {addr}, which is not in the screened set \
             {screened:?}"
        );
        assert_eq!(
            addr.port(),
            8443,
            "the CONNECT port must survive to the dial, or a port-scoped \
             grant means nothing; got {addr}"
        );
    }
    assert_eq!(
        dialled[0].ip(),
        screened[0],
        "the first screened answer is the first one dialled"
    );
    // Nothing was tunnelled and nothing was refused: the target cleared the
    // gate and then simply could not be reached, which is what makes the dial
    // record above the whole measurement.
    assert_eq!(proxy.approved_count(), 0, "{dialled:?}");
    assert_eq!(proxy.refused_count(), 0, "{dialled:?}");

    proxy.shutdown();
}

/// CONTROL for the test above: an approval that carries NO addresses (an IP
/// literal, or a granted loopback name) legitimately dials by name, and must
/// not be recorded as a screened dial — otherwise "dialled_addrs is non-empty"
/// would be satisfied by the very code path it exists to detect.
#[tokio::test]
async fn a_lookup_free_approval_records_no_screened_dial() {
    let (port, server) = spawn_pong_server().await;
    let policy = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![])
        .with_loopback(LoopbackCapability {
            enabled: true,
            schema_version: wcore_browser::policy::LOOPBACK_CAPABILITY_VERSION,
            session_scope: "gh1117-lookup-free".into(),
            ports: vec![port],
        })
        .with_resolver(probe_resolver);
    let proxy = PolicyEgressProxy::start(policy.address_gate_only())
        .await
        .unwrap();

    let mut client = TcpStream::connect((proxy.host(), proxy.port()))
        .await
        .unwrap();
    client
        .write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let established = read_head(&mut client).await;
    assert!(established.starts_with("HTTP/1.1 200"), "{established:?}");
    assert_eq!(proxy.approved_count(), 1);
    assert!(
        proxy.dialled_addrs().is_empty(),
        "a gh#911 grant approves without a lookup, so there is no screened \
         address to dial; got {:?}",
        proxy.dialled_addrs()
    );

    client.shutdown().await.unwrap();
    proxy.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(10), server).await;
}

// ---------------------------------------------------------------------------
// THE REFUSE PATH, both ways (gh#1117 "option 0").
// ---------------------------------------------------------------------------

async fn healthy_sidecar() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })))
        .mount(&server)
        .await;
    server
}

fn sidecar_cfg(server: &MockServer) -> SupervisorConfig {
    SupervisorConfig {
        healthcheck_url: format!("{}/health", server.uri()),
        sidecar_program: Some("wcore-camoufox-command-that-does-not-exist".into()),
        startup_timeout: Duration::from_millis(200),
        ..SupervisorConfig::default()
    }
}

/// REFUSE. A healthy sidecar Core did not start is not behind Core's egress
/// proxy, so the policy would apply to the name and not to the destination.
/// Refusing is the decision; the message has to name both the loss and the
/// opt-out, or the operator is stuck.
#[tokio::test]
async fn an_unproxied_sidecar_core_did_not_start_is_refused() {
    let server = healthy_sidecar().await;
    let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        egress_policy: Some(probe_policy()),
        allow_unproxied_sidecar: false,
        ..sidecar_cfg(&server)
    }));

    let r = supervisor.ensure_ready().await;
    let error = r.expect_err("a sidecar Core cannot contain must not be used silently");
    assert!(error.contains("gh#1117"), "got: {error}");
    assert!(
        error.contains("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR"),
        "the refusal must name the opt-out or it is a dead end; got: {error}"
    );
    assert!(
        error.contains("169.254.169.254") && error.contains("rebinding"),
        "the opt-out must name what it gives up, concretely; got: {error}"
    );
}

/// OPT OUT. The same setup with the opt-out set proceeds. Pairs with the test
/// above: without this arm, "refused" could be a supervisor that refuses every
/// externally managed sidecar regardless of the switch.
#[tokio::test]
async fn the_opt_out_lets_the_same_unproxied_sidecar_through() {
    let server = healthy_sidecar().await;
    let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        egress_policy: Some(probe_policy()),
        allow_unproxied_sidecar: true,
        ..sidecar_cfg(&server)
    }));

    supervisor
        .ensure_ready()
        .await
        .expect("allow_unproxied_sidecar = true must proceed");
}

/// CONTROL. With no egress policy configured at all — the pre-gh#1117 shape,
/// and what `BrowserSupervisor::new()` still is — nothing is refused. This is
/// what proves the refusal above comes from the containment requirement and
/// not from the healthcheck or the missing program.
#[tokio::test]
async fn a_supervisor_with_no_egress_policy_reuses_the_sidecar_as_before() {
    let server = healthy_sidecar().await;
    let supervisor = Arc::new(BrowserSupervisor::with_config(sidecar_cfg(&server)));
    supervisor
        .ensure_ready()
        .await
        .expect("no egress policy configured means no containment requirement");
    assert!(
        supervisor.egress_proxy().is_none(),
        "no proxy should have been started"
    );
}

/// The proxy is started BEFORE anything is reused or launched, so
/// "not contained" is never discovered after a navigation has gone out.
#[tokio::test]
async fn the_egress_proxy_exists_before_the_sidecar_is_reused() {
    let server = healthy_sidecar().await;
    let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        egress_policy: Some(probe_policy()),
        allow_unproxied_sidecar: true,
        ..sidecar_cfg(&server)
    }));
    supervisor.ensure_ready().await.unwrap();
    let proxy = supervisor
        .egress_proxy()
        .expect("the proxy must be up before the sidecar is used");
    assert!(proxy.port() != 0);
}

// ---------------------------------------------------------------------------
// The launch env — the other half of the seam.
// ---------------------------------------------------------------------------

/// TWO supervisors in ONE process, the shape `HostBrowserRegistrar::reify_all`
/// produces: one `BrowserTool` per registered browser tool spec, each with its
/// own `BrowserPolicy` and its own egress proxy, all pointing at the same
/// sidecar URL.
///
/// The retained-child map they consult to answer "did Core launch this?" is
/// PROCESS-global. Keyed on the pid alone it answers yes for the second
/// supervisor too, so it reuses a sidecar wired to the FIRST policy gate and
/// its own `denied_origins` / gh#911 port grant never reach anything the
/// browser dials — silently, with no refusal and no warning. A sidecar this
/// supervisor cannot contain is exactly what the refusal is for, whoever
/// started it.
///
/// Unix-only because it needs a long-lived argument-free program to stand in
/// for the sidecar process; the ownership rule it grades is platform-neutral.
#[cfg(unix)]
#[tokio::test]
async fn a_second_supervisor_does_not_inherit_the_first_ones_sidecar() {
    let server = MockServer::start().await;
    // Down for the first supervisor probe, up from then on: that is what makes
    // supervisor A LAUNCH a child instead of reusing something, which is the
    // state the ownership answer is read out of.
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "ok": true })))
        .with_priority(2)
        .mount(&server)
        .await;

    let pref_dir = tempfile::tempdir().unwrap();
    let cfg = |policy: BrowserPolicy| SupervisorConfig {
        healthcheck_url: format!("{}/health", server.uri()),
        // `yes` writes to a null stdout and stays alive, so the retained child
        // is genuinely running when the second supervisor asks.
        sidecar_program: Some("yes".into()),
        startup_timeout: Duration::from_secs(5),
        egress_policy: Some(policy),
        allow_unproxied_sidecar: false,
        // gh#1117: this test LAUNCHES, and the launch path requires Core to
        // have written the browser loopback pref. Point it at a scratch
        // directory so what is graded here stays sidecar OWNERSHIP, and does
        // not silently become "is a 300 MB Camoufox install present".
        loopback_pref_dir: Some(pref_dir.path().to_path_buf()),
        ..SupervisorConfig::default()
    };

    let first = Arc::new(BrowserSupervisor::with_config(cfg(probe_policy())));
    first
        .ensure_ready()
        .await
        .expect("the first supervisor launches its own contained sidecar");
    let first_proxy = first.egress_proxy().expect("first proxy");

    // Same supervisor, second call — `BrowserTool` does this before every op.
    first
        .ensure_ready()
        .await
        .expect("a supervisor must keep recognising the sidecar IT launched");

    let second = Arc::new(BrowserSupervisor::with_config(cfg(probe_policy())));
    let outcome = second.ensure_ready().await;
    let second_proxy = second.egress_proxy().expect("second proxy");
    assert_ne!(
        first_proxy.port(),
        second_proxy.port(),
        "the two supervisors must have distinct gates, or this proves nothing"
    );

    let error = outcome.expect_err(
        "the live sidecar is wired to the FIRST supervisor's gate; reusing it \
         applies a policy this supervisor never configured",
    );
    assert!(error.contains("gh#1117"), "got: {error}");
    assert!(
        error.contains("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR"),
        "got: {error}"
    );
}

/// The sidecar is pointed at Core's proxy, and the ambient environment cannot
/// steer it anywhere else.
///
/// `PROXY_PORTS` is the trap: the sidecar's own parser
/// (`lib/config.js::parseProxyPorts`) lets `PROXY_PORTS` WIN over
/// `PROXY_PORT`, so setting only `PROXY_PORT` would leave an operator's
/// ambient `PROXY_PORTS` deciding where the browser's traffic goes. Same for
/// `PROXY_STRATEGY=backconnect`, which ignores host and port entirely.
#[tokio::test]
async fn sidecar_launch_env_pins_the_proxy_against_the_ambient_environment() {
    let proxy = PolicyEgressProxy::start(probe_policy().address_gate_only())
        .await
        .unwrap();
    let mut cmd = tokio::process::Command::new("true");
    // Ambient values an operator could plausibly have set.
    cmd.env("PROXY_PORTS", "10001-10010")
        .env("PROXY_STRATEGY", "backconnect")
        .env("PROXY_BACKCONNECT_HOST", "residential.example")
        .env("PROXY_BACKCONNECT_PORT", "7000");

    apply_egress_env(&mut cmd, &proxy);

    let env: std::collections::HashMap<String, Option<String>> = cmd
        .as_std()
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();

    let port = proxy.port().to_string();
    assert_eq!(env.get("PROXY_HOST"), Some(&Some("127.0.0.1".to_string())));
    assert_eq!(env.get("PROXY_PORT"), Some(&Some(port.clone())));
    assert_eq!(
        env.get("PROXY_PORTS"),
        Some(&Some(port)),
        "PROXY_PORTS wins over PROXY_PORT in the sidecar, so an ambient value \
         would send the browser's egress off Core's gate"
    );
    assert_eq!(
        env.get("PROXY_STRATEGY"),
        Some(&Some("round_robin".to_string())),
        "an ambient backconnect strategy ignores host/port entirely"
    );
    assert_eq!(
        env.get("PROXY_BACKCONNECT_HOST"),
        Some(&None),
        "backconnect settings must be cleared, not left for a later code path"
    );
    proxy.shutdown();
}
