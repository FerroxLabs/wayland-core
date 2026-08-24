//! gh#1117 LIVE ARM — real Camoufox, launched by Core's supervisor, behind
//! Core's real `PolicyEgressProxy`.
//!
//! `#[ignore]` by default: it needs the `camofox-browser` sidecar, a Camoufox
//! browser build, a virtual display and the network. Run it with
//!
//! ```text
//! cargo nextest run -p wcore-browser --run-ignored all \
//!   -E 'binary(camoufox_live_egress_test)' --no-capture
//! ```
//!
//! ## What it proves that no hermetic test can
//!
//! The hermetic tests speak the two request forms Camoufox was MEASURED
//! sending. This one has Firefox send them: Core launches the sidecar, the
//! sidecar launches Firefox with Core's proxy, and the navigations below are
//! driven straight at the sidecar's HTTP API — deliberately **around**
//! `BrowserTool::policy_check`, so the only thing that can refuse the rebind
//! target is the egress proxy. That is the gh#1117 case exactly: a URL Core's
//! navigation gate never saw, reaching the address the browser dials.
//!
//! `169-254-169-254.nip.io` resolves to `169.254.169.254` (nip.io encodes the
//! address in the label), so it is a real public name that really answers with
//! the cloud-metadata endpoint. The control is `example.com`, which must still
//! load — a proxy that refuses everything proves nothing.
//!
//! ## Phase 3 is the loopback half
//!
//! Firefox dials loopback around a configured proxy unless
//! `network.proxy.allow_hijacking_localhost` is true. Core sets it in the
//! browser install (`sidecar_prefs`), with no cooperation from the sidecar.
//! Phase 3 is what proves that took effect on the real browser: the local
//! service must receive nothing at all.
//!
//! ## One test, three phases, on purpose
//!
//! All three phases share ONE sidecar. Core refuses a sidecar it did not
//! launch, so two live tests running concurrently would have the second one
//! refuse the first one's sidecar — a flake manufactured by the test layout
//! rather than by the code.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wcore_browser::policy::{BrowserPolicy, PolicyAction};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};

const SIDECAR: &str = "http://127.0.0.1:9377";
/// A real public name whose A record is the cloud-metadata endpoint.
const METADATA_NAME: &str = "169-254-169-254.nip.io";

async fn open_tab(client: &wcore_egress::EgressClient) -> String {
    let body: serde_json::Value = client
        .post(format!("{SIDECAR}/tabs"))
        .json(&serde_json::json!({ "userId": "gh1117", "sessionKey": "gh1117" }))
        .send()
        .await
        .expect("open tab")
        .json()
        .await
        .expect("tab json");
    body["tabId"].as_str().expect("tabId").to_string()
}

async fn navigate(client: &wcore_egress::EgressClient, tab: &str, url: &str) -> serde_json::Value {
    client
        .post(format!("{SIDECAR}/tabs/{tab}/navigate"))
        .json(&serde_json::json!({ "userId": "gh1117", "url": url }))
        .timeout(Duration::from_secs(90))
        .send()
        .await
        .expect("navigate")
        .json()
        .await
        .unwrap_or(serde_json::Value::Null)
}

async fn read_text(client: &wcore_egress::EgressClient, tab: &str) -> String {
    let body: serde_json::Value = client
        .get(format!("{SIDECAR}/tabs/{tab}/snapshot"))
        .query(&[("userId", "gh1117")])
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .expect("snapshot")
        .json()
        .await
        .unwrap_or(serde_json::Value::Null);
    body["snapshot"].as_str().unwrap_or_default().to_string()
}

/// A server on the host's own loopback, standing in for "some local service".
/// Returns its port and a counter of connections it accepted.
async fn spawn_loopback_service() -> (u16, Arc<AtomicU64>) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU64::new(0));
    let server_hits = Arc::clone(&hits);
    tokio::spawn(async move {
        const BODY: &[u8] = b"<html><body>loopbackpong</body></html>";
        while let Ok((mut stream, _)) = listener.accept().await {
            server_hits.fetch_add(1, Ordering::Relaxed);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    BODY.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(BODY).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (port, hits)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "live: needs camofox-browser, a Camoufox build, a display and DNS"]
async fn live_camoufox_egress_goes_through_cores_gate() {
    let (loopback_port, loopback_hits) = spawn_loopback_service().await;

    let policy = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
    let mut config = SupervisorConfig::local_camoufox(SIDECAR);
    config.egress_policy = Some(policy);
    config.startup_timeout = Duration::from_secs(90);
    let supervisor = Arc::new(BrowserSupervisor::with_config(config));

    supervisor
        .ensure_ready()
        .await
        .expect("Core must be able to launch a contained sidecar");
    let proxy = supervisor
        .egress_proxy()
        .expect("a contained sidecar means a running egress proxy");
    eprintln!("egress proxy on {}:{}", proxy.host(), proxy.port());

    // `BrowserTool` calls `ensure_ready` before EVERY op, and the second call
    // finds the sidecar already healthy — the same branch that refuses a
    // sidecar Core did not start. Without the ownership check this refuses
    // every operation after the first, which would be a worse bug than the one
    // being fixed.
    supervisor
        .ensure_ready()
        .await
        .expect("Core's OWN sidecar must stay usable on the second call");

    let client = wcore_egress::EgressClient::new();
    let tab = open_tab(&client).await;

    // ── PHASE 1, CONTROL: an ordinary public site must still load, THROUGH
    //    the proxy.
    let before = proxy.approved_count();
    let ok = navigate(&client, &tab, "http://example.com/").await;
    assert_eq!(
        ok["ok"].as_bool(),
        Some(true),
        "CONTROL FAILED — a permitted site did not load through Core's proxy, \
         so the refusal below would prove only that browsing is broken: {ok}"
    );
    assert!(
        proxy.approved_count() > before,
        "CONTROL FAILED — example.com loaded without going through the proxy; \
         the sidecar is not contained and nothing below is a measurement"
    );
    let control_text = read_text(&client, &tab).await;
    assert!(
        control_text.to_ascii_lowercase().contains("example domain"),
        "CONTROL FAILED — the page did not render: {control_text:?}"
    );

    // ── PHASE 2, THE DEFECT: a name Core's navigation gate never saw,
    //    pointing at the cloud-metadata endpoint. Only the egress proxy can
    //    stop this.
    let refused_before = proxy.refused_count();
    let _ = navigate(&client, &tab, &format!("http://{METADATA_NAME}/")).await;
    assert!(
        proxy.refused_count() > refused_before,
        "the sidecar dialled {METADATA_NAME} without Core refusing it: \
         refused_count stayed at {refused_before}"
    );
    let probe_text = read_text(&client, &tab).await;
    assert!(
        probe_text.contains("Wayland browser policy refused this request"),
        "the browser should be showing Core's refusal, not metadata: \
         {probe_text:?}"
    );
    assert!(
        !probe_text.contains("ami-") && !probe_text.contains("instance-id"),
        "metadata content reached the page: {probe_text:?}"
    );

    // ── PHASE 3, THE LOOPBACK HALF. Firefox dials loopback around a
    //    configured proxy unless `network.proxy.allow_hijacking_localhost` is
    //    true, and `@askjo/camofox-browser` exposes no seam for browser
    //    prefs. Core sets it where Firefox reads it without the launcher's
    //    help — `sidecar_prefs`, called from `ensure_ready` above — so these
    //    requests now arrive at Core's gate like any other.
    //
    //    This phase asserted the GAP until 0.13.6. It now asserts the FIX:
    //    the local service must receive NOTHING, and the refusal must come
    //    from Core.
    // Both request forms Firefox sends a proxy, on both loopback spellings:
    // absolute-form GET for http, and CONNECT for https. The https arm needs
    // no TLS server — if the browser bypassed the proxy it would open a raw
    // connection to the plain-HTTP listener below, and that is what
    // `loopback_hits` counts.
    for (label, scheme, host) in [
        ("http ip literal", "http", "127.0.0.1"),
        ("http name", "http", "localhost"),
        ("https ip literal (CONNECT)", "https", "127.0.0.1"),
        ("https name (CONNECT)", "https", "localhost"),
    ] {
        let refused_before = proxy.refused_count();
        let approved_before = proxy.approved_count();
        let direct_before = loopback_hits.load(Ordering::Relaxed);

        let _ = navigate(
            &client,
            &tab,
            &format!("{scheme}://{host}:{loopback_port}/probe"),
        )
        .await
        .clone();

        // CONTROL for the assertion below: a counter that did not move at all
        // would mean the browser never tried, and "the local server got
        // nothing" would then be reporting nothing.
        assert!(
            proxy.refused_count() + proxy.approved_count() > refused_before + approved_before,
            "{label}: the browser did not reach Core's proxy at all, so this \
             phase measures nothing about where the request went"
        );
        assert!(
            proxy.refused_count() > refused_before,
            "{label}: loopback reached the proxy but was not refused by the \
             address gate"
        );
        assert_eq!(
            loopback_hits.load(Ordering::Relaxed),
            direct_before,
            "{label}: the browser reached the loopback service directly, \
             around Core's proxy — the gh#1117 loopback hole is open"
        );
    }

    // The whole point of the phase above: across all of it, the local service
    // was never touched. Stated once, absolutely, so a future reader does not
    // have to add up deltas.
    assert_eq!(
        loopback_hits.load(Ordering::Relaxed),
        0,
        "the loopback service was reached {} time(s) by a browser that is \
         supposed to have no route to it except Core's gate",
        loopback_hits.load(Ordering::Relaxed)
    );
}
