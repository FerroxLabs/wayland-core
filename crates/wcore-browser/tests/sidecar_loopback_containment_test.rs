//! gh#1117 — the loopback half of sidecar containment, at the supervisor.
//!
//! [`wcore_browser::sidecar_prefs`] grades locating and writing the pref.
//! This file grades the DECISION built on it: Core launches a sidecar only
//! when it has put the pref where the browser will read it, and says so
//! precisely when it has not.
//!
//! The proof that the pref actually changes the browser's behaviour is not
//! here and cannot be — it needs a real browser. It is
//! `camoufox_live_egress_test::live_camoufox_egress_goes_through_cores_gate`
//! phase 3, which fails when this file's mechanism is removed.

use std::sync::Arc;
use std::time::Duration;

use wcore_browser::policy::{BrowserPolicy, PolicyAction};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn probe_policy() -> BrowserPolicy {
    BrowserPolicy::new(PolicyAction::Allow, vec![], vec![])
}

/// A sidecar that is DOWN, so `ensure_ready` takes the launch path — the only
/// path on which Core makes the containment promise.
async fn dead_sidecar() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    server
}

fn launching_cfg(server: &MockServer, pref_dir: Option<std::path::PathBuf>) -> SupervisorConfig {
    SupervisorConfig {
        healthcheck_url: format!("{}/health", server.uri()),
        // Never becomes healthy, so the launch always ends in the startup
        // error below. That error is the CONTROL: reaching it means the
        // containment gate let the launch through.
        sidecar_program: Some("true".into()),
        startup_timeout: Duration::from_millis(300),
        egress_policy: Some(probe_policy()),
        allow_unproxied_sidecar: false,
        loopback_pref_dir: pref_dir,
        ..SupervisorConfig::default()
    }
}

/// REFUSE. Core cannot write the pref, so the browser it is about to start
/// would dial loopback around the proxy. Refusing is the decision, and the
/// message has to name what is missing and the opt-out.
#[tokio::test]
async fn a_sidecar_whose_loopback_core_cannot_screen_is_not_launched() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();
    // A FILE where the pref directory should be: unwritable in a way that
    // bites even as root, unlike a permission bit.
    let blocked = tmp.path().join("not-a-directory");
    std::fs::write(&blocked, "").unwrap();

    let supervisor = Arc::new(BrowserSupervisor::with_config(launching_cfg(
        &server,
        Some(blocked.join("pref")),
    )));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("a browser Core cannot contain must not be started");
    assert!(error.contains("gh#1117"), "got: {error}");
    assert!(
        error.contains("WAYLAND_BROWSER_ALLOW_UNPROXIED_SIDECAR"),
        "the refusal must name the opt-out or it is a dead end; got: {error}"
    );
    assert!(
        error.contains("127.0.0.1") && error.contains("allow_hijacking_localhost"),
        "the refusal must name what is unprotected, concretely; got: {error}"
    );
    assert!(
        !error.contains("did not become healthy"),
        "the launch happened anyway — the gate is decorative; got: {error}"
    );
}

/// CONTROL, and the known-positive for the test above. Same supervisor, same
/// dead sidecar, a pref directory Core CAN write: the launch proceeds and
/// fails on the startup timeout instead. Without this arm, "refused" could be
/// a supervisor that refuses every launch.
#[tokio::test]
async fn a_writable_pref_dir_lets_the_launch_proceed() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();

    let supervisor = Arc::new(BrowserSupervisor::with_config(launching_cfg(
        &server,
        Some(tmp.path().to_path_buf()),
    )));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("the stand-in program exits immediately, so startup fails");
    assert!(
        error.contains("exited before becoming healthy")
            || error.contains("did not become healthy"),
        "the launch should have been attempted; got: {error}"
    );
    assert!(
        tmp.path().join("wayland-core-egress-gate.js").is_file(),
        "the launch proceeded without the pref actually being written"
    );
}

/// OPT OUT. The same unwritable install with the opt-out set proceeds, with
/// the loss written into the log rather than into silence.
#[tokio::test]
async fn the_opt_out_lets_an_unscreened_loopback_sidecar_launch() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();
    let blocked = tmp.path().join("not-a-directory");
    std::fs::write(&blocked, "").unwrap();

    let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        allow_unproxied_sidecar: true,
        ..launching_cfg(&server, Some(blocked.join("pref")))
    }));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("the stand-in program exits immediately, so startup fails");
    assert!(
        !error.contains("allow_hijacking_localhost"),
        "the opt-out did not apply; got: {error}"
    );
}

/// CONTROL. A supervisor with no egress policy is the pre-gh#1117 shape and
/// makes no containment promise, so an unwritable install is not its problem.
/// This is what proves the refusal above comes from the containment
/// requirement and not from the path being odd.
#[tokio::test]
async fn no_egress_policy_means_no_loopback_requirement() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();
    let blocked = tmp.path().join("not-a-directory");
    std::fs::write(&blocked, "").unwrap();

    let supervisor = Arc::new(BrowserSupervisor::with_config(SupervisorConfig {
        egress_policy: None,
        ..launching_cfg(&server, Some(blocked.join("pref")))
    }));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("the stand-in program exits immediately, so startup fails");
    assert!(
        !error.contains("allow_hijacking_localhost"),
        "a supervisor that makes no containment promise refused anyway; got: {error}"
    );
}
