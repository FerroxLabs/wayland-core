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
//! A suite that exits 0 having run zero tests is the failure shape this project
//! keeps finding, so when these are run the executed count must be read back:
//! `cargo test -p wcore-channel-whatsapp --test live_bridge -- --ignored` must
//! report `6 passed`, not merely `ok`.
//!
//! # What these prove, and what they do NOT
//!
//! They prove this client's spawn, framing, `health` read-back and every
//! readiness gate behave as claimed against the genuine unmodified bridge under
//! a real Node. They prove **nothing** about delivering a WhatsApp message:
//! that needs a QR pairing against a real account, and is recorded as unrun in
//! the lane summary. A mock proves what we put on the wire and nothing about
//! what the destination does.

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
        // The live test drives the launch path, not the chunker, so it takes
        // the shipped default rather than pinning a width of its own.
        max_message_chars: None,
    }
}

/// CAN PASS — the whole launch path against the real artifact.
///
/// Preflight clears (real Node, real script, real `@whiskeysockets/baileys`),
/// the subprocess spawns, and the real bridge answers `health` reporting the
/// backend that was requested.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_preflight_clears_against_the_real_bridge() {
    let path = bridge_path();
    let launch = bridge::preflight(&cfg(WhatsappBackend::Baileys, path))
        .expect("preflight must clear against a real, dependency-installed bridge");
    eprintln!(
        "LIVE: node={} script={} backend={}",
        launch.node.display(),
        launch.script.display(),
        launch.backend
    );
    assert_eq!(launch.backend, WhatsappBackend::Baileys);
    assert!(launch.node.is_file(), "a real Node must have been resolved");
}

/// CAN PASS. The handshake reaches the real bridge and the real bridge names
/// the backend back. Pairing is a separate gate, so the verdict here is
/// `Incomplete{whatsapp_pairing}` on an unpaired host — NOT a green.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_handshake_succeeds_and_the_verdict_stops_at_pairing() {
    let path = bridge_path();
    let ch = WhatsappBridgeChannel::new("live-baileys", cfg(WhatsappBackend::Baileys, path));
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    // The handshake got past every filesystem gate — had it not, the finding
    // would name node_runtime / bridge_path / bridge_dependencies instead.
    assert_eq!(
        report.findings,
        vec!["whatsapp_pairing".to_string()],
        "the only thing left must be the human pairing step; got {report:?}"
    );
    assert!(
        !report.outcome.is_ready(),
        "an unpaired bridge must never be advertised as ready"
    );
}

/// CAN PASS, and it is what shows the selector genuinely SELECTS against the
/// real bridge rather than one value happening to agree with its default.
///
/// `whatsapp-web` reaching the pairing gate means the real bridge accepted
/// `--backend whatsapp-web` and echoed it back through `health`; had it
/// ignored the flag it would have reported `baileys` and the session would
/// have been refused with `BackendMismatch`, surfacing as `Unauthenticated`.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_the_backend_selector_selects_against_the_real_bridge() {
    let path = bridge_path();
    let ch = WhatsappBridgeChannel::new("live-www", cfg(WhatsappBackend::WhatsappWeb, path));
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_ne!(
        report.outcome,
        ProbeOutcome::Unauthenticated,
        "a backend mismatch would mean the real bridge ignored --backend: {report:?}"
    );
    assert_eq!(
        report.findings,
        vec!["whatsapp_pairing".to_string()],
        "whatsapp-web must clear every gate up to pairing; got {report:?}"
    );
}

/// CAN PASS — and this one exists because **a gate that can never go green
/// proves as little as one that can never go red**.
///
/// Every other live verdict here is `Incomplete`, because the host has never
/// been paired. Without this test the probe would be a permanently-red
/// instrument and nobody could tell a working bridge from a broken one.
///
/// It is a REACHABILITY proof, not an authentication proof. The pairing
/// material it reads is placed by the harness, so what is demonstrated is that
/// the last gate opens when the material is where the bridge would write it —
/// **not** that WhatsApp has accepted anything. That limit is real and is
/// stated in `docs/whatsapp-bridge.md`: session material that exists but has
/// been revoked server-side reads as paired until the bridge reports
/// `logged_out`.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_the_ok_verdict_is_reachable_when_pairing_material_is_present() {
    let session = match std::env::var("WCORE_TEST_SESSION_DIR") {
        Ok(s) => PathBuf::from(s),
        Err(_) => panic!(
            "WCORE_TEST_SESSION_DIR is not set. Point it at a session root containing \
             baileys/creds.json to prove the Ok verdict is reachable."
        ),
    };
    let marker = session.join("baileys").join("creds.json");
    assert!(
        marker.exists(),
        "known-positive setup: {} must exist for this proof to mean anything",
        marker.display()
    );

    let mut c = cfg(WhatsappBackend::Baileys, bridge_path());
    c.session_dir = Some(session);

    let report = WhatsappBridgeChannel::new("live-paired", c)
        .probe()
        .await
        .expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(
        report.outcome,
        ProbeOutcome::Ok,
        "the Ok verdict must be reachable; got {report:?}"
    );
    assert!(report.outcome.is_ready());
    assert!(report.config_complete);
    assert_eq!(report.identity.as_deref(), Some("bridge/baileys"));
    assert!(report.findings.is_empty());
}

/// CAN FAIL — with a real Node present, so the red is attributable to the
/// missing bridge and nothing else.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_a_missing_bridge_fails_closed_even_when_node_is_installed() {
    // Known-positive first: prove Node and the real bridge both resolve, so
    // this test cannot pass for the wrong reason.
    bridge::preflight(&cfg(WhatsappBackend::Baileys, bridge_path()))
        .expect("known-positive: Node and the real bridge both resolve");

    let mut broken = cfg(
        WhatsappBackend::Baileys,
        PathBuf::from("/definitely/not/here/bridge.js"),
    );
    broken.handshake_timeout_secs = 5;

    let ch = WhatsappBridgeChannel::new("live-missing", broken);
    let report = ch.probe().await.expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(report.outcome, ProbeOutcome::Incomplete);
    assert!(!report.outcome.is_ready());
    assert_eq!(report.findings, vec!["bridge_path".to_string()]);
}

/// CAN FAIL — the dependency gate, against a real bridge script with real Node
/// and NO `node_modules`.
///
/// This gate exists because the real bridge answers `health` perfectly well
/// with nothing installed and only fails at the first `connect`. A readiness
/// verdict taken from the handshake alone would report a green for a bridge
/// that cannot send a single message.
#[tokio::test]
#[ignore = "needs WCORE_TEST_BRIDGE_PATH — wayland-core does not ship the bridge"]
async fn live_a_dependency_free_bridge_copy_is_refused_before_it_can_lie() {
    // Known-positive: the real bridge, with its dependencies, clears preflight.
    bridge::preflight(&cfg(WhatsappBackend::Baileys, bridge_path()))
        .expect("known-positive: the installed bridge clears preflight");

    // Known-negative: byte-identical script, no node_modules anywhere above it.
    let bare = tempfile::tempdir().expect("tempdir");
    let copy = bare.path().join("bridge.js");
    std::fs::copy(bridge_path(), &copy).expect("copy the real bridge");

    let report = WhatsappBridgeChannel::new("live-bare", cfg(WhatsappBackend::Baileys, copy))
        .probe()
        .await
        .expect("probe must not error");
    eprintln!("LIVE: probe report = {report:?}");

    assert_eq!(
        report.findings,
        vec!["bridge_dependencies".to_string()],
        "a dependency-free bridge must be named as such, not handed a green: {report:?}"
    );
    assert!(!report.outcome.is_ready());
}
