//! Live proof: drive the REAL Wayland Desktop `bridge.js` under a REAL Node.
//!
//! # Why these are `#[ignore]`, and why that is not a dodge
//!
//! wayland-core deliberately does not ship the bridge (see
//! [`wcore_channel_whatsapp::bridge`]), so these tests need an artifact that is
//! not in this repository. They are gated behind `#[ignore]` and an env var
//! rather than skipped silently:
//!
//! * `WCORE_TEST_BRIDGE_PATH` unset → the test **panics** with instructions.
//!   There is no path through this file that passes without doing the work.
//! * Set but not a file → the test **panics** naming the path.
//!
//! A suite that exits 0 having run zero tests is the failure shape this
//! project keeps finding, so when these are run the executed count must be
//! read back — `cargo test -p wcore-channel-whatsapp --test live_bridge --
//! --ignored` must report `3 passed`, not merely `ok`.
//!
//! # What these prove, and what they do NOT
//!
//! They prove that this client's framing, spawn and `health` read-back work
//! against the genuine unmodified bridge. They prove **nothing** about
//! delivering a WhatsApp message: that needs a QR pairing against a real
//! account and is recorded as unrun in the lane summary. A mock proves what we
//! put on the wire and nothing about what the destination does.

use std::path::PathBuf;

use wcore_channel_whatsapp::bridge::{WhatsappBackend, WhatsappBridgeConfig};
use wcore_channel_whatsapp::{WhatsappBridgeChannel, bridge};
use wcore_channels::Channel;
use wcore_channels::probe::ProbeOutcome;

/// Resolve the operator-supplied bridge, or fail loudly. Never skips.
fn bridge_path() -> PathBuf {
    let raw = std::env::var("WCORE_TEST_BRIDGE_PATH").unwrap_or_else(|_| {
        panic!(
            "WCORE_TEST_BRIDGE_PATH is not set. These tests drive the real Wayland Desktop \
             bridge, which wayland-core does not ship. Point the variable at a bridge.js and \
             re-run with `-- --ignored`."
        )
    });
    let path = PathBuf::from(&raw);
    assert!(
        path.is_file(),
        "WCORE_TEST_BRIDGE_PATH={raw} is not a file — the live test cannot run against it"
    );
    path
}

fn cfg(backend: WhatsappBackend, path: PathBuf) -> WhatsappBridgeConfig {
    WhatsappBridgeConfig {
        backend,
        bridge_path: path,
        node_path: std::env::var("WCORE_TEST_NODE_PATH")
            .ok()
            .map(PathBuf::from),
        session_dir: None,
        workspace_name: "live".to_string(),
        default_recipient: String::new(),
        handshake_timeout_secs: 30,
        rpc_timeout_secs: 30,
    }
}

/// CAN PASS. The real bridge, spawned under real Node, answers `health` and
/// reports the backend we asked for.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_real_bridge_answers_the_health_handshake_as_baileys() {
    let path = bridge_path();

    // Preflight must clear first — if it does not, the rest measures nothing.
    let launch = bridge::preflight(&cfg(WhatsappBackend::Baileys, path.clone()))
        .expect("preflight must clear before the live handshake can mean anything");
    eprintln!(
        "LIVE: node={} script={} backend={}",
        launch.node.display(),
        launch.script.display(),
        launch.backend
    );

    let ch = WhatsappBridgeChannel::new("live-baileys", cfg(WhatsappBackend::Baileys, path));
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(
        report.outcome,
        ProbeOutcome::Ok,
        "the real bridge must complete the handshake; findings: {:?}",
        report.findings
    );
    assert!(report.outcome.is_ready());
    assert_eq!(report.identity.as_deref(), Some("bridge/baileys"));
}

/// CAN PASS, and it is the one that shows the selector genuinely SELECTS
/// against the real bridge rather than one value happening to agree.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_real_bridge_reports_whatsapp_web_when_that_is_what_was_requested() {
    let path = bridge_path();
    let ch = WhatsappBridgeChannel::new("live-www", cfg(WhatsappBackend::WhatsappWeb, path));
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(
        report.outcome,
        ProbeOutcome::Ok,
        "findings: {:?}",
        report.findings
    );
    assert_eq!(
        report.identity.as_deref(),
        Some("bridge/whatsapp-web"),
        "the bridge must report back the backend we selected, not its own default"
    );
}

/// CAN FAIL — with a real Node present, so the failure is attributable to the
/// missing bridge and nothing else. This is the fail-closed control for the
/// two passes above.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_a_missing_bridge_fails_closed_even_when_node_is_installed() {
    // Prove Node really is present first, otherwise this test would pass for
    // the wrong reason.
    let real = bridge_path();
    bridge::preflight(&cfg(WhatsappBackend::Baileys, real))
        .expect("known-positive: Node and the bridge both resolve");

    let mut broken = cfg(
        WhatsappBackend::Baileys,
        PathBuf::from("/definitely/not/here/bridge.js"),
    );
    broken.handshake_timeout_secs = 5;

    let ch = WhatsappBridgeChannel::new("live-missing", broken);
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(report.outcome, ProbeOutcome::Incomplete);
    assert!(
        !report.outcome.is_ready(),
        "a missing bridge must never be advertised as ready"
    );
    assert_eq!(report.findings, vec!["bridge_path".to_string()]);
}
