//! F7 — the supervisor's readiness path must actually *invoke* binary
//! provisioning.
//!
//! `BrowserBinaryManager::ensure_camoufox` shipped complete — SHA-256
//! verification, atomic move, bounded redirects, wiremock coverage — and had
//! **zero production callers**: its only other references were two doc
//! comments and one unit test. A host without `camofox-browser` on PATH
//! therefore failed, with a working downloader sitting one call away. Unit
//! tests of the downloader cannot detect that, because the downloader was
//! never broken. Only a test that drives `BrowserSupervisor::ensure_ready`
//! can.
//!
//! Both arms are paired, and the wire is the instrument. The origin is a
//! `wiremock` server mounted with an explicit `.expect(n)`; `MockServer`
//! verifies expectations on drop, so "it downloaded" and "it did not
//! download" are both measured against real HTTP rather than inferred from a
//! return value. No test contacts a real network.
//!
//! The configured sidecar program is a name that provably does not resolve on
//! PATH, so a successful `ensure_ready` is attributable to provisioning and
//! nothing else — and the disabled arm is the control that stops
//! "provisioning ran" from being satisfied by a supervisor that would have
//! succeeded anyway.
//!
//! **Unix only** (the stand-in sidecar is a `#!`-script and the executable
//! bit is a Unix concept). Windows is not covered here; that is a gap, not a
//! pass.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use wcore_browser::binary::{CAMOUFOX_VERSION, sha256_hex};
use wcore_browser::supervisor::{BrowserSupervisor, SupervisorConfig};
use wcore_config::browser::{BinaryArtifact, CamoufoxDownloadConfig, platform_key};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A name no PATH entry can satisfy. `which` must fail on it, otherwise the
/// arms below measure a pre-installed binary instead of the download.
const ABSENT_PROGRAM: &str = "wcore-f7-camoufox-that-does-not-exist";

/// Gzipped tar holding one member at `rel`. Written mode 0o644 on purpose:
/// if the spawned sidecar runs, the executable bit came from the
/// provisioning path, not from the archive.
fn tar_gz_with(rel: &str, body: &[u8]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    builder.append_data(&mut header, rel, body).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

/// An argument-free HTTP health server. `launch_camoufox_program` passes NO
/// args, so the stand-in sidecar has to be self-contained.
fn stub_sidecar_script(port: u16) -> String {
    format!(
        r#"#!/usr/bin/env python3
import http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Length","2"); self.end_headers()
        self.wfile.write(b"ok")
    def log_message(self, *a): pass
socketserver.TCPServer.allow_reuse_address = True
socketserver.TCPServer(("127.0.0.1", {port}), H).serve_forever()
"#
    )
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn artifacts(url: &str, sha: &str, exe: &str) -> BTreeMap<String, BinaryArtifact> {
    let mut m = BTreeMap::new();
    m.insert(
        platform_key(),
        BinaryArtifact {
            url: url.to_string(),
            sha256: sha.to_string(),
            archive_exe_path: exe.to_string(),
        },
    );
    m
}

fn cfg(port: u16, tmp: &std::path::Path, download: CamoufoxDownloadConfig) -> SupervisorConfig {
    SupervisorConfig {
        pid_dir: tmp.join("pids"),
        reaper_interval: Duration::from_millis(200),
        healthcheck_interval: Duration::from_secs(30),
        healthcheck_url: format!("http://127.0.0.1:{port}/health"),
        sidecar_program: Some(ABSENT_PROGRAM.to_string()),
        startup_timeout: Duration::from_secs(20),
        camoufox_download: download,
        binary_install_root: tmp.join("bin"),
        // These arms grade the PINNED-artifact path. `sidecar_auto_install`
        // now defaults ON, and leaving it on would let the disabled-download
        // control arm satisfy itself by npm-installing the sidecar instead -
        // which is a different path, and would stop that arm being a control.
        sidecar_auto_install: wcore_config::browser::SidecarAutoInstall {
            enabled: false,
            ..Default::default()
        },
        ..SupervisorConfig::default()
    }
}

/// Precondition shared by both arms: the configured program really is absent.
/// Without this the "download happened" arm could be satisfied by a machine
/// that happened to have the binary, and the control arm proves nothing.
#[test]
fn configured_program_is_genuinely_unresolvable() {
    assert!(
        which::which(ABSENT_PROGRAM).is_err(),
        "{ABSENT_PROGRAM} resolves on this host; both arms below would be vacuous"
    );
}

/// ARM 1 — enabled + pinned digest ⇒ `ensure_ready` downloads, verifies,
/// unpacks, chmods, and spawns the provisioned executable, which then
/// answers the healthcheck.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_ready_provisions_camoufox_when_enabled() {
    let port = free_port();
    let payload = tar_gz_with("camoufox/camoufox", stub_sidecar_script(port).as_bytes());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/camoufox.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let download = CamoufoxDownloadConfig {
        enabled: true,
        artifacts: artifacts(
            &format!("{}/camoufox.tar.gz", server.uri()),
            &sha256_hex(&payload),
            "camoufox/camoufox",
        ),
    };
    let sup = Arc::new(BrowserSupervisor::with_config(cfg(
        port,
        tmp.path(),
        download,
    )));

    sup.ensure_ready().await.expect(
        "ensure_ready must provision the sidecar; the configured program is not on PATH, \
         so this can only succeed via the download path",
    );

    // The supervisor spawned something and is tracking it.
    let live = sup.live_sessions();
    assert_eq!(
        live.len(),
        1,
        "exactly one sidecar session must be tracked, got {live:?}"
    );

    // The thing it spawned is the artifact we served, on disk, executable.
    let exe = tmp
        .path()
        .join("bin")
        .join(format!("camoufox-{CAMOUFOX_VERSION}-unpacked"))
        .join("camoufox")
        .join("camoufox");
    assert!(
        exe.is_file(),
        "provisioned executable missing at {}",
        exe.display()
    );
    let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
    assert_ne!(
        mode & 0o111,
        0,
        "provisioned executable is not executable (mode {mode:o}); the archive ships it 0o644"
    );

    // Mock `.expect(1)`, verified on drop, is the wire-level proof that
    // ensure_ready reached the downloader exactly once.
    drop(server);
}

/// ARM 2 (control) — the SAME setup with the switch off. Nothing is fetched,
/// and the operator still gets the pre-existing actionable install message.
/// Without this arm, ARM 1 is satisfied by an implementation that downloads
/// unconditionally, which is exactly the behaviour the policy forbids.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_ready_fetches_nothing_when_download_disabled() {
    let port = free_port();
    let payload = tar_gz_with("camoufox/camoufox", stub_sidecar_script(port).as_bytes());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/camoufox.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let download = CamoufoxDownloadConfig {
        enabled: false,
        artifacts: artifacts(
            &format!("{}/camoufox.tar.gz", server.uri()),
            &sha256_hex(&payload),
            "camoufox/camoufox",
        ),
    };
    let sup = Arc::new(BrowserSupervisor::with_config(cfg(
        port,
        tmp.path(),
        download,
    )));

    let err = sup
        .ensure_ready()
        .await
        .expect_err("with auto-download off there is no binary, so readiness must fail");
    assert!(
        err.contains(wcore_browser::install::CAMOUFOX_SIDECAR_PACKAGE),
        "the disabled path must keep the actionable install message, got: {err}"
    );
    assert!(
        !tmp.path().join("bin").exists(),
        "the disabled path created an install root"
    );
    assert!(sup.live_sessions().is_empty());
    drop(server);
}

/// ARM 3 — enabled but the operator pinned no digest. `ensure_ready` must
/// REFUSE with an actionable message and never touch the origin. This is the
/// fail-closed rule stated at the supervisor boundary rather than only at the
/// downloader's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ensure_ready_refuses_when_no_digest_is_pinned() {
    let port = free_port();
    let payload = tar_gz_with("camoufox/camoufox", stub_sidecar_script(port).as_bytes());
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/camoufox.tar.gz"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(payload))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let download = CamoufoxDownloadConfig {
        enabled: true,
        artifacts: artifacts(
            &format!("{}/camoufox.tar.gz", server.uri()),
            "",
            "camoufox/camoufox",
        ),
    };
    let sup = Arc::new(BrowserSupervisor::with_config(cfg(
        port,
        tmp.path(),
        download,
    )));

    let err = sup
        .ensure_ready()
        .await
        .expect_err("an unpinned artifact must be refused, not fetched");
    assert!(
        err.contains("no sha256 is pinned"),
        "refusal must name the missing digest, got: {err}"
    );
    assert!(
        err.contains("browser.camoufox_download.artifacts"),
        "refusal must name the config key the operator has to set, got: {err}"
    );
    assert!(sup.live_sessions().is_empty());
    drop(server);
}
