//! gh#491 — a missing browser sidecar is a MISSING DEPENDENCY, and has to be
//! reported as one, once, naming the package that provides it and the env var
//! Core actually reads.
//!
//! On a clean install the containment pre-flight ran first and answered a
//! question nobody asked: it could not find a Camoufox install to write the
//! loopback pref into — true, because nothing was installed — and told the
//! operator to point `CAMOUFOX_EXECUTABLE_PATH` at one. Meanwhile the liveness
//! probe named `@askjo/camofox-browser` + `WAYLAND_CAMOUFOX_BIN` and
//! `--doctor` named `apt install chromium-browser`, which is not even compiled
//! into the shipped binary. Three remedies, one missing dependency, and the
//! one the user saw was the wrong one.
//!
//! This file grades the ORDER (existence before containment) and the
//! CONVERGENCE (every surface built from `wcore_browser::install`). The
//! containment refusal itself is graded by
//! `sidecar_loopback_containment_test` and is deliberately still reachable —
//! the control below proves it.

use std::sync::Arc;
use std::time::Duration;

use wcore_browser::install::{CAMOUFOX, CAMOUFOX_SIDECAR_ENV, CAMOUFOX_SIDECAR_PACKAGE};
use wcore_browser::policy::{BrowserPolicy, PolicyAction};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A sidecar that is DOWN, so `ensure_ready` takes the launch path — the only
/// path on which a clean install is discovered at all.
async fn dead_sidecar() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    server
}

/// The production shape: containment required, and a pref directory Core can
/// never write to (a FILE where the directory should be, which bites even as
/// root). `program` is the only thing that varies between the defect and its
/// control.
fn contained_cfg(
    server: &MockServer,
    program: &str,
    blocked_pref: std::path::PathBuf,
) -> SupervisorConfig {
    SupervisorConfig {
        healthcheck_url: format!("{}/health", server.uri()),
        sidecar_program: Some(program.to_string()),
        startup_timeout: Duration::from_millis(300),
        egress_policy: Some(BrowserPolicy::new(PolicyAction::Allow, vec![], vec![])),
        allow_unproxied_sidecar: false,
        loopback_pref_dir: Some(blocked_pref),
        ..SupervisorConfig::default()
    }
}

/// A path that can never be a directory: a regular file with a child path
/// appended. Returned with the tempdir so the caller keeps it alive.
fn unwritable_pref_dir(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let blocked = tmp.path().join("not-a-directory");
    std::fs::write(&blocked, "").unwrap();
    blocked.join("pref")
}

/// THE DEFECT. Nothing is installed. The answer must be "install the sidecar",
/// naming the package and the variable Core reads — not four paragraphs about
/// an egress proxy naming a variable Core does not read for this.
#[tokio::test]
async fn a_missing_sidecar_is_diagnosed_as_missing_not_as_a_containment_failure() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(BrowserSupervisor::with_config(contained_cfg(
        &server,
        "wcore-camoufox-command-that-does-not-exist",
        unwritable_pref_dir(&tmp),
    )));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("nothing is installed, so this cannot succeed");

    assert!(
        error.contains(CAMOUFOX_SIDECAR_PACKAGE),
        "the remedy must name the package that provides the sidecar; got: {error}"
    );
    assert!(
        error.contains(CAMOUFOX_SIDECAR_ENV),
        "the remedy must name the env var the supervisor actually reads; got: {error}"
    );
    assert!(
        !error.contains("CAMOUFOX_EXECUTABLE_PATH"),
        "a host with no sidecar installed was told to set the pref-directory override, \
         which is a third remedy for the same missing dependency; got: {error}"
    );
    assert!(
        !error.contains("allow_hijacking_localhost"),
        "a missing dependency was reported as a containment-plumbing failure; got: {error}"
    );
}

/// CONTROL, and the known-positive for the assertion above. Same supervisor,
/// same unwritable install, a sidecar program that DOES resolve: the
/// containment refusal is still reached, unchanged. Without this arm the test
/// above would also pass if the loopback gate had simply been deleted.
#[tokio::test]
async fn an_installed_sidecar_core_cannot_contain_is_still_refused() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(BrowserSupervisor::with_config(contained_cfg(
        &server,
        // `true` exists on PATH on every unix host and exits immediately.
        "true",
        unwritable_pref_dir(&tmp),
    )));

    let error = supervisor
        .ensure_ready()
        .await
        .expect_err("a browser Core cannot contain must not be started");

    assert!(
        error.contains("allow_hijacking_localhost") && error.contains("gh#1117"),
        "the loopback containment refusal must still fire for an INSTALLED sidecar; got: {error}"
    );
    assert!(
        !error.contains(CAMOUFOX_SIDECAR_PACKAGE),
        "an installed sidecar must not be reported as not installed; got: {error}"
    );
}

/// CONVERGENCE. The message the user hits is built from the same constants the
/// liveness probe publishes as its remedy, so the two cannot drift into
/// different instructions for the same missing dependency.
#[tokio::test]
#[serial_test::serial]
async fn the_liveness_remedy_and_the_runtime_refusal_are_the_same_instruction() {
    let server = dead_sidecar().await;
    let tmp = tempfile::tempdir().unwrap();

    // The probe resolves the program the SUPERVISOR would spawn, which means
    // the env override. Point it at a path that cannot exist so the probe is
    // deciding about a missing sidecar on any host, installed or not.
    let prior = std::env::var_os(CAMOUFOX_SIDECAR_ENV);
    let missing = tmp.path().join("no-such-camofox-browser");
    unsafe { std::env::set_var(CAMOUFOX_SIDECAR_ENV, &missing) };

    let probe = wcore_browser::liveness::probe(&server.uri()).await;

    match prior {
        Some(v) => unsafe { std::env::set_var(CAMOUFOX_SIDECAR_ENV, v) },
        None => unsafe { std::env::remove_var(CAMOUFOX_SIDECAR_ENV) },
    }

    let unavailable = probe
        .unavailable()
        .expect("a sidecar that cannot resolve and does not answer is unavailable");
    assert_eq!(
        unavailable.remedy,
        CAMOUFOX.remedy(),
        "the liveness remedy is not the shared install instruction"
    );

    // And the runtime refusal carries every line of that same instruction.
    let refusal = CAMOUFOX.not_installed(&missing.display().to_string(), Some(&server.uri()));
    for hint in CAMOUFOX.install_hints {
        assert!(
            refusal.contains(hint),
            "the runtime refusal dropped a line of the shared remedy: {hint}\nin: {refusal}"
        );
    }
}
